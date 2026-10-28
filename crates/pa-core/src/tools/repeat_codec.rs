//! E1 codecs: deterministic, reversible representation of REPEATED tool output.
//!
//! The premise this module was planned on — "a long log is mostly duplicates" — is
//! false for exact duplicates, and false for CONSECUTIVE repeats too. Measured on a
//! real daemon log (2 192 009 bytes / 13 376 lines):
//!
//! | structure | runs | lines covered |
//! |---|---|---|
//! | exact duplicate lines | 20 | 20 |
//! | consecutive same-shape runs ≥ 4 | 1 | 83 |
//! | shapes appearing ≥ 4 times ANYWHERE | 185 | 12 948 |
//!
//! A first version of this module collapsed consecutive runs only, and its measured
//! saving over the same log was 6 490 bytes — 0.3%. The information is in the
//! DISTRIBUTION, not in adjacency: a busy daemon interleaves its events, so the
//! same shape recurs throughout.
//!
//! The codec is therefore dictionary-based: every frequent shape becomes a template
//! (its literal text with `\u{1}` standing where a value goes), each line that
//! matches one becomes `(template id, values…)`, and everything else stays verbatim.
//! Recovery is exact: template plus that line's values reproduces it byte for byte,
//! which is what makes this a representation rather than a summary.
//!
//! A rough model of this encoding over the measured log is 843 671 bytes — 0.385 of
//! the original.

/// How many times a shape must appear before it earns a dictionary slot.
///
/// A template costs its own text plus one id per use; below this the id and the
/// values cost more than the literal line.
pub const MIN_TEMPLATE_USES: usize = 8;

/// One dictionary entry: a line shape with `\u{1}` where each value goes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Template {
    /// The shape: literal text with one `\u{1}` per field.
    pub shape: String,
    /// How many input lines used it.
    pub uses: usize,
}

/// What the encoding produced.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Encoded {
    /// The dictionary, most-used first (so ids of common shapes are small).
    pub templates: Vec<Template>,
    /// One entry per input line: `Some((template id, values))` when encoded,
    /// `None` when the line is kept verbatim in [`Self::lines`].
    pub entries: Vec<Option<(usize, Vec<String>)>>,
    /// The verbatim lines, in input order, for the `None` entries.
    pub lines: Vec<String>,
    /// Bytes of the input.
    pub input_bytes: usize,
}

impl Encoded {
    /// Whether the encoding is worth using: the payload it would serialize to is
    /// smaller than the input.
    ///
    /// Measured, not estimated: [`Self::encoded_size`] serializes the shape a
    /// reader would receive, so a "saving" that exists only in memory does not
    /// count.
    #[must_use]
    pub fn is_smaller(&self) -> bool {
        self.encoded_size() < self.input_bytes
    }

    /// The size of the encoded form, as it would travel.
    #[must_use]
    pub fn encoded_size(&self) -> usize {
        serde_json::to_vec(self).map_or(usize::MAX, |bytes| bytes.len())
    }

    /// Lines covered by a template.
    #[must_use]
    pub fn encoded_lines(&self) -> usize {
        self.entries.iter().filter(|entry| entry.is_some()).count()
    }
}

/// Encode `text` with a shape dictionary, or `None` when no shape repeats enough.
#[must_use]
pub fn encode(text: &str) -> Option<Encoded> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let shapes: Vec<String> = lines.iter().map(|line| shape_of(line)).collect();
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for shape in &shapes {
        *counts.entry(shape.as_str()).or_insert(0) += 1;
    }
    // Deterministic dictionary: by uses (descending), then by shape, so the same
    // input always yields the same ids and a cached encoding stays valid.
    let mut frequent: Vec<(&str, usize)> = counts
        .into_iter()
        .filter(|(_, uses)| *uses >= MIN_TEMPLATE_USES)
        .collect();
    frequent.sort_by(|(shape_a, uses_a), (shape_b, uses_b)| {
        uses_b.cmp(uses_a).then_with(|| shape_a.cmp(shape_b))
    });
    if frequent.is_empty() {
        return None;
    }
    let ids: std::collections::HashMap<&str, usize> = frequent
        .iter()
        .enumerate()
        .map(|(id, (shape, _))| (*shape, id))
        .collect();
    let templates: Vec<Template> = frequent
        .iter()
        .map(|(shape, uses)| Template {
            shape: (*shape).to_string(),
            uses: *uses,
        })
        .collect();

    let mut encoded = Vec::with_capacity(lines.len());
    let mut kept = Vec::new();
    for (line, shape) in lines.iter().zip(&shapes) {
        if let Some(id) = ids.get(shape.as_str()) {
            encoded.push(Some((
                *id,
                fields_of(line)
                    .into_iter()
                    .filter_map(|(start, end)| line.get(start..end).map(str::to_string))
                    .collect(),
            )));
        } else {
            encoded.push(None);
            kept.push((*line).to_string());
        }
    }
    Some(Encoded {
        templates,
        entries: encoded,
        lines: kept,
        input_bytes: text.len(),
    })
}

/// Restore the original text from [`Encoded`].
///
/// Exact by construction. A template whose value count disagrees with a line's
/// recorded values is rebuilt with the values it has and the template's remaining
/// text, so a damaged encoding stays readable instead of panicking.
#[must_use]
pub fn decode(encoded: &Encoded) -> String {
    let mut out = String::with_capacity(encoded.input_bytes);
    let mut kept = encoded.lines.iter();
    for entry in &encoded.entries {
        match entry {
            Some((id, values)) => {
                // An id past the dictionary emits nothing rather than a wrong line,
                // and the loop keeps going so the rest of the text survives.
                if let Some(template) = encoded.templates.get(*id) {
                    out.push_str(&rebuild(&template.shape, values));
                }
            }
            None => {
                if let Some(line) = kept.next() {
                    out.push_str(line);
                }
            }
        }
        out.push('\n');
    }
    out
}

/// A line's shape: every digit run and ISO timestamp replaced by `\u{1}`.
fn shape_of(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut cursor = 0usize;
    for (start, end) in fields_of(line) {
        out.push_str(&line[cursor..start]);
        out.push('\u{1}');
        cursor = end;
    }
    out.push_str(&line[cursor..]);
    out
}

/// Byte spans of every varying field in `line`, in order.
///
/// One pass over the whole line: offsets computed from a moving remainder are how
/// an earlier version captured a single digit of a multi-digit number.
fn fields_of(line: &str) -> Vec<(usize, usize)> {
    let mut fields = Vec::new();
    let bytes = line.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        let len = span_len(&line[index..]);
        fields.push((index, index + len));
        index += len;
    }
    fields
}

/// The length of the varying field at the head of `text`, which starts with a digit.
fn span_len(text: &str) -> usize {
    if let Some(len) = iso_timestamp_len(text) {
        return len;
    }
    text.bytes().take_while(u8::is_ascii_digit).count()
}

/// Length of an ISO-8601 timestamp at the head of `text`, when there is one.
fn iso_timestamp_len(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let digit = |at: usize| bytes.get(at).is_some_and(u8::is_ascii_digit);
    if !(digit(0) && digit(1) && digit(2) && digit(3) && bytes.get(4) == Some(&b'-')) {
        return None;
    }
    let mut end = 19;
    if text.len() < end {
        return None;
    }
    if bytes.get(19) == Some(&b'.') {
        end = 20;
        while digit(end) {
            end += 1;
        }
    }
    if matches!(bytes.get(end), Some(b'Z' | b'z')) {
        end += 1;
    } else if matches!(bytes.get(end), Some(b'+' | b'-')) {
        end += 3;
    }
    Some(end.min(text.len()))
}

/// One line from a template shape and its values.
fn rebuild(shape: &str, values: &[String]) -> String {
    let mut out = String::with_capacity(shape.len());
    let mut values = values.iter();
    for part in shape.split('\u{1}') {
        out.push_str(part);
        if let Some(value) = values.next() {
            out.push_str(value);
        }
    }
    // Extra values past the shape's slots cannot be placed; appending them keeps
    // their information rather than dropping it silently.
    for value in values {
        out.push_str(value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    /// The measured structure: the same shape recurs THROUGHOUT an interleaved log,
    /// not in one block. A consecutive-run codec found 83 lines on the real log; the
    /// dictionary must find the distribution instead.
    #[test]
    fn a_dictionary_finds_shapes_that_are_never_adjacent() {
        let mut log = String::new();
        for i in 0..40 {
            // Two shapes interleaved: neither ever repeats consecutively.
            let _ = writeln!(log, "acquire id={i} in_flight={i}");
            let _ = writeln!(log, "settle id={i} queued={}", i + 1);
        }
        let encoded = encode(&log).expect("both shapes are frequent");
        assert_eq!(encoded.templates.len(), 2, "one template per shape");
        assert_eq!(
            encoded.encoded_lines(),
            80,
            "every line is covered even though no two neighbours share a shape"
        );
        assert_eq!(decode(&encoded), log, "and it decodes exactly");
    }

    /// Exact reversal is the whole claim: a representation that loses bytes is a
    /// summary, and a summary cannot be trusted as evidence.
    #[test]
    fn the_encoding_decodes_back_to_the_original_byte_for_byte() {
        let mut log = String::new();
        for i in 0..30 {
            let _ = writeln!(log, "step {i}: elapsed {}ms, ok", i * 7);
        }
        let encoded = encode(&log).expect("one frequent shape");
        assert_eq!(decode(&encoded), log);
    }

    /// The numbers are the information: values must be captured per LINE, not at the
    /// template's offsets. `in_flight=400` puts its second number where `in_flight=1`
    /// has `queued`, so a positional slice captures the wrong text.
    #[test]
    fn varying_values_are_captured_from_each_line_not_the_template() {
        let mut log = String::new();
        for i in [1usize, 7, 400, 2, 99, 5, 12, 3] {
            let _ = writeln!(log, "in_flight={i} queued={}", i * 3);
        }
        let encoded = encode(&log).expect("frequent shape");
        let restored = decode(&encoded);
        assert_eq!(restored, log);
        assert!(
            restored.contains("in_flight=400"),
            "values are not averaged"
        );
        assert!(restored.contains("queued=1200"), "and the second field too");
    }

    /// Below the threshold nothing is encoded, and the caller is told so rather than
    /// receiving a payload that grew.
    #[test]
    fn a_rare_shape_is_left_alone() {
        let log = "unique a 1\nunique b 2\nunique c 3\n";
        assert!(encode(log).is_none(), "no shape reaches MIN_TEMPLATE_USES");
        assert!(encode("").is_none());
    }

    /// ISO timestamps are one field, not six numbers: otherwise every log line
    /// fragments into a shape that matches nothing else.
    #[test]
    fn a_timestamp_is_one_field() {
        let line = "2026-10-28T00:00:07.123Z event x";
        assert_eq!(fields_of(line), vec![(0, 24)]);
    }

    /// Measured on the daemon log this module documents, when it is available: the
    /// dictionary must find the distribution, decode exactly, and shrink the payload.
    #[test]
    fn the_codec_probe_over_a_real_log() {
        let Ok(path) = std::env::var("PA_CODEC_PROBE_LOG") else {
            eprintln!("SKIPPED: set PA_CODEC_PROBE_LOG to a large log file");
            return;
        };
        let text = std::fs::read_to_string(&path).expect("probe log readable");
        let encoded = encode(&text).expect("the probe log has frequent shapes");
        let restored = decode(&encoded);
        assert_eq!(
            restored.lines().count(),
            text.lines().count(),
            "every line must come back"
        );
        assert_eq!(restored, text, "byte for byte");
        eprintln!(
            "input {} bytes / {} lines -> encoded {} bytes; {} template(s) cover {} line(s)",
            encoded.input_bytes,
            text.lines().count(),
            encoded.encoded_size(),
            encoded.templates.len(),
            encoded.encoded_lines()
        );
        assert!(
            encoded.is_smaller(),
            "the measured log must shrink: {} vs {}",
            encoded.encoded_size(),
            encoded.input_bytes
        );
    }
}
