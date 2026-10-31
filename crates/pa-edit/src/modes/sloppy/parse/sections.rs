//! Envelope handling, section splitting, the normalize/recover ladder, and the
//! copy-ready payload renderers.
//!
//! Everything here runs either before the op stream is walked
//! ([`strip_envelope_noise`], [`split_sloppy_sections`],
//! [`extract_inline_sloppy_regions`], [`normalize_input`]) or after an
//! operation has been assembled ([`ir_to_payload`], [`operation_payload`]).
//! [`parse_section`] is the walk alone, and it calls into [`super::operations`]
//! for every `*** Find`/`*** Replace` pair it closes.

use std::collections::HashMap;

use super::super::apply::normalize_text;
use super::super::types::{
    InlineSloppyRegion, Operation, OperationRewrite, Placement, SloppySection,
};
use crate::error::EditError;

use super::operations::{
    create_operation, decode_literal_markers, finish_operation, finish_pattern, has_marker_lines,
    marker_line_content,
};
use super::{
    ADD_LINE, AFTER_HEADER, BEFORE_HEADER, ELIDED_LINE_RE, ENVELOPE_END_RE, ENVELOPE_LINE_RE,
    ENVELOPE_WORDS_RE, FENCE_LINE_RE, GLUED_CONTROL_RE, HEADER_RE, NUMBERED_LINE_RE,
    NUMBERED_LITERAL_RE, OPENER, ORDINAL_OPENER_RE, OUTER_FENCE_RE, READ_MORE_NOTICE_RE,
    REFERENCE_RE, REMOVE_LINE, REWRITE_HEADER, SELECT_CLOSE, SELECT_DIVIDER, SELECT_OPEN,
    SHOWING_LINES_RE, parse_error,
};

#[derive(Debug)]
enum Header {
    Edit { path: Option<String>, all: bool },
    Find,
    Replace,
    Insert(Placement),
}

fn parse_header(line: &str) -> Option<Header> {
    let trimmed = line.trim();
    if !trimmed.starts_with("***") {
        return None;
    }
    let captures = HEADER_RE.captures(trimmed)?;
    let name = captures.get(1)?.as_str().to_ascii_lowercase();
    let rest = captures.get(2).map_or("", |value| value.as_str()).trim();
    if name.starts_with("edit") {
        let (path, all) = if rest.eq_ignore_ascii_case("all") {
            ("", true)
        } else if let Some((path, flag)) = rest.rsplit_once(char::is_whitespace)
            && flag.eq_ignore_ascii_case("all")
        {
            (path.trim_end(), true)
        } else {
            (rest, false)
        };
        let path = if path.is_empty() {
            None
        } else if path.starts_with('"') {
            Some(serde_json::from_str::<String>(path).ok()?).filter(|path| !path.is_empty())
        } else {
            Some(path.to_owned())
        };
        return Some(Header::Edit { path, all });
    }
    if !rest.is_empty() {
        return None;
    }
    Some(if name == "find" {
        Header::Find
    } else if name == "replace" {
        Header::Replace
    } else if name.ends_with("before") {
        Header::Insert(Placement::Before)
    } else {
        Header::Insert(Placement::After)
    })
}

fn envelope_words(line: &str) -> bool {
    ENVELOPE_WORDS_RE.is_match(line)
}

fn envelope_line(line: &str) -> bool {
    ENVELOPE_LINE_RE.is_match(line)
}

/// Remove foreign patch envelopes and the noise following end sentinels.
///
/// Insert bodies are literal, so envelope rows inside them are kept — except a
/// closing sentinel (`*** End Patch`, …) followed only by blank or fence lines,
/// which closes a wrapper around the whole payload rather than inserted text.
pub fn strip_envelope_noise(lines: Vec<&str>) -> Vec<String> {
    let mut result = Vec::new();
    let mut skipping = false;
    let mut inserting = false;
    let mut index = 0;
    while index < lines.len() {
        let mut line = lines[index].to_owned();
        if let Some(header) = parse_header(&line) {
            inserting = !skipping && matches!(header, Header::Insert(_));
        } else if inserting {
            if ENVELOPE_END_RE.is_match(line.trim())
                && lines[index + 1..]
                    .iter()
                    .all(|rest| rest.trim().is_empty() || FENCE_LINE_RE.is_match(rest))
            {
                break;
            }
            result.push(line);
            index += 1;
            continue;
        }
        if line.trim() == "***" && index + 1 < lines.len() && envelope_words(lines[index + 1]) {
            line = format!("*** {}", lines[index + 1].trim());
            index += 1;
        }
        if envelope_line(line.trim()) {
            skipping = ENVELOPE_END_RE.is_match(line.trim());
            index += 1;
            continue;
        }
        if skipping {
            if matches!(parse_header(&line), Some(Header::Edit { .. })) {
                skipping = false;
                result.push(line);
            }
            index += 1;
            continue;
        }
        result.push(line);
        index += 1;
    }
    result
}

struct RewriteBody {
    lines: Vec<String>,
    insert: Option<Placement>,
}

#[derive(Debug)]
struct CompiledSection {
    path: String,
    ir: Vec<String>,
}

/// Internal op-stream prefix carrying a JSON-encoded insertion body.
const fn insert_header(at: Placement) -> &'static str {
    match at {
        Placement::Before => BEFORE_HEADER,
        Placement::After => AFTER_HEADER,
    }
}

/// Split an internal insertion line into its placement and JSON body.
fn strip_insert_header(line: &str) -> Option<(Placement, &str)> {
    line.strip_prefix(AFTER_HEADER)
        .map(|encoded| (Placement::After, encoded))
        .or_else(|| {
            line.strip_prefix(BEFORE_HEADER)
                .map(|encoded| (Placement::Before, encoded))
        })
}

fn trim_blank(buffer: &[String]) -> Vec<String> {
    let start = buffer
        .iter()
        .position(|line| !line.trim().is_empty())
        .unwrap_or(buffer.len());
    let end = buffer
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map_or(start, |index| index + 1);
    buffer[start..end].to_vec()
}

fn flush_compiled(
    sections: &mut Vec<CompiledSection>,
    all: bool,
    find_lines: &mut Vec<String>,
    put_lines: &mut Option<RewriteBody>,
) {
    let find = trim_blank(find_lines);
    let put = put_lines.take();
    find_lines.clear();
    if find.is_empty()
        && put.as_ref().is_none_or(|rewrite| {
            rewrite.insert.is_none() && rewrite.lines.iter().all(|line| line.trim().is_empty())
        })
    {
        return;
    }
    if sections.is_empty() {
        sections.push(CompiledSection {
            path: String::new(),
            ir: Vec::new(),
        });
    }
    let ir = &mut sections.last_mut().expect("section exists").ir;
    ir.push(format!("{OPENER}{}", if all { "*" } else { "" }));
    if let Some(RewriteBody {
        lines,
        insert: Some(at),
    }) = &put
    {
        ir.extend(find);
        let mut text = lines.join("\n");
        if !lines.is_empty() {
            text.push('\n');
        }
        ir.push(format!(
            "{}{}",
            insert_header(*at),
            serde_json::to_string(&text).expect("string serialization")
        ));
        return;
    }
    let put = put.map(|rewrite| trim_blank(&rewrite.lines));
    if find.is_empty() {
        if let Some(put) = put {
            ir.extend(put);
        }
        return;
    }
    ir.extend(find);
    if let Some(put) = put {
        ir.push(REWRITE_HEADER.to_owned());
        ir.extend(put);
    }
}

fn compile_sections(lines: &[String]) -> Vec<CompiledSection> {
    #[derive(Clone, Copy)]
    enum State {
        Idle,
        Find,
        Put,
    }

    let mut sections = Vec::new();
    let mut all = false;
    let mut state = State::Idle;
    let mut find_lines = Vec::new();
    let mut put_lines: Option<RewriteBody> = None;

    for line in lines {
        let Some(header) = parse_header(line) else {
            match state {
                State::Put => put_lines
                    .as_mut()
                    .expect("put buffer exists")
                    .lines
                    .push(line.clone()),
                State::Find => find_lines.push(line.clone()),
                State::Idle if !line.trim().is_empty() => {
                    state = State::Find;
                    find_lines.push(line.clone());
                }
                State::Idle => {}
            }
            continue;
        };
        match header {
            Header::Edit {
                path,
                all: next_all,
            } => {
                flush_compiled(&mut sections, all, &mut find_lines, &mut put_lines);
                state = State::Idle;
                if let Some(path) = path {
                    sections.push(CompiledSection {
                        path,
                        ir: Vec::new(),
                    });
                }
                all = next_all;
            }
            Header::Find => {
                flush_compiled(&mut sections, all, &mut find_lines, &mut put_lines);
                state = State::Find;
            }
            Header::Replace => {
                put_lines = Some(RewriteBody {
                    lines: Vec::new(),
                    insert: None,
                });
                state = State::Put;
            }
            Header::Insert(at) => {
                put_lines = Some(RewriteBody {
                    lines: Vec::new(),
                    insert: Some(at),
                });
                state = State::Put;
            }
        }
    }
    flush_compiled(&mut sections, all, &mut find_lines, &mut put_lines);
    sections
}

/// Split a sloppy payload into ordered, same-path-coalesced file sections.
pub fn split_sloppy_sections(input: &str) -> Vec<SloppySection> {
    let mut lines = strip_envelope_noise(input.lines().collect());
    while lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    if !matches!(
        parse_header(lines.first().map_or("", String::as_str)),
        Some(Header::Edit { path: Some(_), .. })
    ) {
        return Vec::new();
    }
    let sections = compile_sections(&lines);
    let mut bodies: HashMap<String, Vec<String>> = HashMap::new();
    let mut paths = Vec::new();
    for section in sections {
        if section.path.is_empty() || section.ir.is_empty() {
            continue;
        }
        if !bodies.contains_key(&section.path) {
            paths.push(section.path.clone());
        }
        bodies.entry(section.path).or_default().extend(section.ir);
    }
    paths
        .into_iter()
        .map(|path| SloppySection {
            body: bodies.remove(&path).unwrap_or_default().join("\n"),
            path,
        })
        .collect()
}

/// Extract pathful sloppy payloads embedded in prose, using UTF-16 offsets.
pub fn extract_inline_sloppy_regions(text: &str) -> Vec<InlineSloppyRegion> {
    if !text.contains("***") {
        return Vec::new();
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let mut starts = Vec::with_capacity(lines.len());
    let mut position = 0;
    for line in &lines {
        starts.push(position);
        position += line.encode_utf16().count() + 1;
    }
    let text_length = text.encode_utf16().count();
    let line_end =
        |index: usize| (starts[index] + lines[index].encode_utf16().count() + 1).min(text_length);
    let mut regions = Vec::new();
    let mut in_fence = false;
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if FENCE_LINE_RE.is_match(line) {
            in_fence = !in_fence;
            index += 1;
            continue;
        }
        let opener = (!in_fence).then(|| parse_header(line)).flatten();
        if !matches!(opener, Some(Header::Edit { path: Some(_), .. })) {
            index += 1;
            continue;
        }
        #[derive(Clone, Copy)]
        enum Block {
            Outside,
            Find,
            Put,
        }
        let mut last = index;
        let mut block = Block::Outside;
        let mut scan = index + 1;
        while scan < lines.len() {
            if ENVELOPE_END_RE.is_match(lines[scan].trim()) {
                break;
            }
            let Some(header) = parse_header(lines[scan]) else {
                if matches!(block, Block::Outside) {
                    if lines[scan].trim().is_empty() {
                        scan += 1;
                        continue;
                    }
                    break;
                }
                last = scan;
                scan += 1;
                continue;
            };
            block = match header {
                Header::Find => Block::Find,
                Header::Replace | Header::Insert(_) => Block::Put,
                Header::Edit { .. } => Block::Outside,
            };
            last = scan;
            scan += 1;
        }
        let payload = lines[index..=last].join("\n");
        if !split_sloppy_sections(&payload).is_empty() {
            regions.push(InlineSloppyRegion {
                start: starts[index],
                end: line_end(last),
                payload,
            });
        }
        index = scan.max(last + 1);
    }
    regions
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenerKind {
    Single,
    All,
}

fn parse_opener(line: &str) -> Option<OpenerKind> {
    match line.trim() {
        "«" => Some(OpenerKind::Single),
        "«*" => Some(OpenerKind::All),
        _ => None,
    }
}

fn is_ordinal_opener(line: &str) -> bool {
    ORDINAL_OPENER_RE.is_match(line.trim())
}

/// Normalize section headers or internal IR into a canonical op stream.
pub fn normalize_input(input: &str) -> String {
    let mut lines = strip_envelope_noise(input.lines().collect());
    while lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    if lines
        .first()
        .is_some_and(|line| OUTER_FENCE_RE.is_match(line.trim()))
    {
        lines.remove(0);
        while lines.last().is_some_and(|line| line.trim().is_empty()) {
            lines.pop();
        }
        if lines.last().is_some_and(|line| line.trim() == "```") {
            lines.pop();
        }
    }
    if lines.iter().any(|line| parse_header(line).is_some()) {
        lines = compile_sections(&lines)
            .into_iter()
            .flat_map(|section| section.ir)
            .collect();
    } else {
        lines = lines
            .into_iter()
            .flat_map(|line| {
                GLUED_CONTROL_RE.captures(&line).map_or_else(
                    || vec![line.clone()],
                    |captures| vec![captures[1].to_owned(), captures[2].to_owned()],
                )
            })
            .collect();
    }
    while lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// Shared with [`super::operations`]: the finishing passes normalize the
/// pattern and rewrite text they render back into a payload.
pub(super) fn normalize_block(lines: &[String], rewrite: bool) -> String {
    let mut cleaned: Vec<String> = lines
        .iter()
        .filter(|line| {
            let trimmed = line.trim();
            !SHOWING_LINES_RE.is_match(trimmed)
                && !READ_MORE_NOTICE_RE.is_match(trimmed)
                && !ELIDED_LINE_RE.is_match(trimmed)
        })
        .cloned()
        .collect();
    if !rewrite {
        for line in &mut cleaned {
            if line.trim().is_empty() {
                line.clear();
            }
        }
    }
    while cleaned.last().is_some_and(|line| line.trim().is_empty()) {
        cleaned.pop();
    }
    let has_non_blank = cleaned.iter().any(|line| !line.trim().is_empty());
    let numbered_lines = has_non_blank
        && cleaned
            .iter()
            .filter(|line| !line.trim().is_empty())
            .all(|line| NUMBERED_LINE_RE.is_match(line));
    let numbered_literals = has_non_blank
        && cleaned
            .iter()
            .filter(|line| !line.trim().is_empty())
            .all(|line| NUMBERED_LITERAL_RE.is_match(line));
    let all_non_blank_plus = has_non_blank
        && cleaned
            .iter()
            .filter(|line| !line.trim().is_empty())
            .all(|line| line.starts_with('+'));
    if numbered_lines && !numbered_literals {
        for line in &mut cleaned {
            *line = NUMBERED_LINE_RE.replace(line, "").into_owned();
        }
    }
    if rewrite {
        if cleaned.first().is_some_and(|line| line.trim() == "//") {
            cleaned.remove(0);
        }
        if cleaned
            .iter()
            .any(|line| marker_line_content(line, REMOVE_LINE).is_some())
            && cleaned
                .iter()
                .any(|line| marker_line_content(line, ADD_LINE).is_some())
        {
            cleaned.retain(|line| marker_line_content(line, REMOVE_LINE).is_none());
        }
        for line in &mut cleaned {
            if let Some(stripped) = marker_line_content(line, ADD_LINE) {
                *line = stripped;
            }
        }
        let has_old = cleaned
            .iter()
            .any(|line| line.starts_with('-') && !line.starts_with("---"));
        let has_new = cleaned
            .iter()
            .any(|line| line.starts_with('+') && !line.starts_with("+++"));
        if has_old && has_new {
            return cleaned
                .into_iter()
                .filter(|line| !line.starts_with('-') && !line.starts_with("+++"))
                .map(|line| line.strip_prefix('+').unwrap_or(&line).to_owned())
                .collect::<Vec<_>>()
                .join("\n");
        }
        if all_non_blank_plus {
            return cleaned
                .into_iter()
                .map(|line| line.strip_prefix('+').unwrap_or(&line).to_owned())
                .collect::<Vec<_>>()
                .join("\n");
        }
    }
    cleaned.join("\n")
}

fn recover_alternating_separators(lines: &[String], content: &str) -> Option<Vec<String>> {
    if lines.iter().any(|line| line.trim() == REWRITE_HEADER) {
        return None;
    }
    let headers: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| parse_opener(line).map(|_| index))
        .collect();
    if headers.len() < 2 || !headers.len().is_multiple_of(2) || headers[0] != 0 {
        return None;
    }
    let normalized_content = normalize_text(content).text;
    let mut recovered = Vec::new();
    for pair in (0..headers.len()).step_by(2) {
        let match_start = headers[pair];
        let rewrite_start = headers[pair + 1];
        let next = headers.get(pair + 2).copied().unwrap_or(lines.len());
        let match_lines = &lines[match_start + 1..rewrite_start];
        let rewrite_lines = &lines[rewrite_start + 1..next];
        let normalized_match = normalize_text(&normalize_block(match_lines, false)).text;
        let normalized_rewrite = normalize_text(&normalize_block(rewrite_lines, true)).text;
        if normalized_match.is_empty()
            || normalized_rewrite.is_empty()
            || rewrite_lines.iter().any(|line| line.contains(SELECT_OPEN))
            || !normalized_content.contains(&normalized_match)
            || normalized_content.contains(&normalized_rewrite)
        {
            return None;
        }
        recovered.push(lines[match_start].clone());
        recovered.extend_from_slice(match_lines);
        recovered.push(REWRITE_HEADER.to_owned());
        recovered.extend_from_slice(rewrite_lines);
    }
    Some(recovered)
}

fn recover_bracket_pairs(lines: &[String], content: &str) -> Option<Vec<String>> {
    let mut items = Vec::new();
    let mut open = None;
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if parse_opener(line).is_some() {
            if open.is_some() {
                return None;
            }
            open = Some(index);
        } else if trimmed == REWRITE_HEADER {
            items.push((open?, index));
            open = None;
        } else if open.is_none() && !trimmed.is_empty() {
            return None;
        }
    }
    if open.is_some() || items.len() < 2 || !items.len().is_multiple_of(2) {
        return None;
    }
    let normalized_content = normalize_text(content).text;
    let mut recovered = Vec::new();
    for pair in (0..items.len()).step_by(2) {
        let first = items[pair];
        let second = items[pair + 1];
        let match_lines = &lines[first.0 + 1..first.1];
        let rewrite_lines = &lines[second.0 + 1..second.1];
        let normalized_match = normalize_text(&normalize_block(match_lines, false)).text;
        let normalized_rewrite = normalize_text(&normalize_block(rewrite_lines, true)).text;
        if normalized_match.is_empty()
            || normalized_rewrite.is_empty()
            || rewrite_lines.iter().any(|line| line.contains(SELECT_OPEN))
            || !normalized_content.contains(&normalized_match)
            || normalized_content.contains(&normalized_rewrite)
        {
            return None;
        }
        recovered.push(lines[first.0].clone());
        recovered.extend_from_slice(match_lines);
        recovered.push(REWRITE_HEADER.to_owned());
        recovered.extend_from_slice(rewrite_lines);
    }
    Some(recovered)
}

/// Rows collected for one side of an operation, with running `⟪`/`⟫` counts
/// and a non-blank flag so each new row costs O(row) instead of re-scanning
/// the block.
#[derive(Default)]
struct Rows {
    lines: Vec<String>,
    opens: usize,
    closes: usize,
    has_content: bool,
}

impl Rows {
    fn push(&mut self, line: &str) {
        self.opens += line.matches(SELECT_OPEN).count();
        self.closes += line.matches(SELECT_CLOSE).count();
        self.has_content |= !line.trim().is_empty();
        self.lines.push(line.to_owned());
    }

    fn clear(&mut self) {
        self.lines.clear();
        self.opens = 0;
        self.closes = 0;
        self.has_content = false;
    }

    const fn balanced(&self) -> bool {
        self.opens == self.closes
    }
}

/// [`parse_operations`] for one section; while `streaming`, a trailing
/// `*** Find` without its `*** Replace` is dropped instead of recovered.
pub(in crate::modes::sloppy) fn parse_section(
    input: &str,
    content: &str,
    path: &str,
    streaming: bool,
) -> Result<Vec<Operation>, EditError> {
    let payload = normalize_input(input);
    let mut lines: Vec<String> = payload.split('\n').map(str::to_owned).collect();
    if parse_opener(lines.first().map_or("", String::as_str)).is_none()
        && (lines.iter().any(|line| line.trim() == REWRITE_HEADER)
            || payload.contains(SELECT_OPEN)
            || payload.contains(SELECT_CLOSE)
            || has_marker_lines(&payload))
    {
        lines.insert(0, OPENER.to_owned());
    }
    if !lines.iter().any(|line| strip_insert_header(line).is_some()) {
        lines = recover_alternating_separators(&lines, content).unwrap_or(lines);
        lines = recover_bracket_pairs(&lines, content).unwrap_or(lines);
    }
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Outside,
        Pattern,
        Rewrite,
    }
    let mut operations = Vec::new();
    let mut state = State::Outside;
    let mut all = false;
    let mut pattern = Rows::default();
    let mut rewrite = Rows::default();
    let mut reference_separator: Option<String> = None;
    let mut pending = Vec::new();

    for index in 0..lines.len() {
        let line = &lines[index];
        let parsed_opener = parse_opener(line);
        let trimmed = line.trim();
        let register_reference = REFERENCE_RE.captures(trimmed);
        if let Some((at, encoded)) = strip_insert_header(line) {
            let number = operations.len() + 1;
            let header = at.header();
            if state != State::Pattern || !pattern.has_content {
                return Err(parse_error(format!(
                    "Operation {number} needs a non-empty *** Find before {header}."
                )));
            }
            let text = serde_json::from_str(encoded)
                .map_err(|error| parse_error(format!("Invalid {header} body: {error}")))?;
            operations.push(create_operation(
                &normalize_block(&pattern.lines, false),
                OperationRewrite::Insert { text, at },
                all,
                content,
                number,
            )?);
            state = State::Outside;
            continue;
        }
        if is_ordinal_opener(line) {
            return Err(parse_error(format!(
                "{trimmed} is not a valid opener. Use a *** Find that matches once — add context only \
				 the intended match has — or *** Edit File: path all to change every match."
            )));
        }
        if trimmed == format!("{OPENER}{REWRITE_HEADER}") {
            if state == State::Pattern && pattern.has_content {
                state = State::Rewrite;
            }
            continue;
        }
        if parsed_opener.is_none()
            && (trimmed.starts_with(OPENER)
                || (trimmed.starts_with(REWRITE_HEADER)
                    && trimmed != REWRITE_HEADER
                    && register_reference.is_none()))
        {
            let json = serde_json::to_string(trimmed).expect("string serialization cannot fail");
            return Err(parse_error(format!(
                "Invalid control line {json}; use only {OPENER}, {OPENER}*, {REWRITE_HEADER}, or \
				 {REWRITE_HEADER}N in REWRITE."
            )));
        }
        if state == State::Outside {
            if let Some(opener) = parsed_opener {
                all = opener == OpenerKind::All;
                pattern.clear();
                rewrite.clear();
                reference_separator = None;
                state = State::Pattern;
            } else if !trimmed.is_empty() {
                return Err(parse_error(format!(
                    "Expected a *** Edit File: or *** Find header on input line {}.",
                    index + 1
                )));
            }
            continue;
        }
        if state == State::Pattern {
            let balanced = pattern.balanced();
            if trimmed == REWRITE_HEADER
                || (trimmed == "***" && balanced && pattern.has_content)
                || (trimmed == SELECT_CLOSE && balanced && pattern.has_content)
            {
                state = State::Rewrite;
            } else if register_reference.is_some() && balanced {
                if !pattern.has_content {
                    return Err(parse_error(format!(
                        "{trimmed} is valid only in REWRITE, never MATCH."
                    )));
                }
                state = State::Rewrite;
                reference_separator = Some(trimmed.to_owned());
            } else if let Some(opener) = parsed_opener {
                if pattern.has_content {
                    finish_pattern(
                        &lines,
                        index,
                        content,
                        path,
                        &pattern.lines,
                        all,
                        false,
                        &mut operations,
                        &mut pending,
                    )?;
                }
                all = opener == OpenerKind::All;
                pattern.clear();
                rewrite.clear();
                reference_separator = None;
            } else {
                pattern.push(line);
            }
            continue;
        }
        if let Some(opener) = parsed_opener {
            finish_operation(
                &lines,
                index,
                path,
                &pattern.lines,
                &rewrite.lines,
                reference_separator.as_deref(),
                all,
                &mut operations,
            )?;
            all = opener == OpenerKind::All;
            pattern.clear();
            rewrite.clear();
            reference_separator = None;
            state = State::Pattern;
        } else if trimmed == "***" {
        } else if trimmed == REWRITE_HEADER {
            let next_content = lines[index + 1..]
                .iter()
                .find(|entry| !entry.trim().is_empty());
            if next_content.is_some_and(|next| parse_opener(next).is_none()) {
                return Err(parse_error(format!(
                    "Operation {} has a second {REWRITE_HEADER} line.",
                    operations.len() + 1
                )));
            }
        } else if trimmed == SELECT_CLOSE && rewrite.balanced() {
        } else {
            rewrite.push(line);
        }
    }
    match state {
        State::Rewrite => finish_operation(
            &lines,
            lines.len(),
            path,
            &pattern.lines,
            &rewrite.lines,
            reference_separator.as_deref(),
            all,
            &mut operations,
        )?,
        State::Pattern => finish_pattern(
            &lines,
            lines.len(),
            content,
            path,
            &pattern.lines,
            all,
            streaming,
            &mut operations,
            &mut pending,
        )?,
        State::Outside => {}
    }
    if operations.is_empty() {
        return Err(parse_error(
            "Empty patch. Provide *** Find followed by *** Replace, *** Insert Before, or *** Insert \
			 After.",
        ));
    }
    for (index, operation) in operations.iter().enumerate() {
        let rewrites: Vec<&str> = match &operation.rewrite {
            OperationRewrite::Explicit { text } => vec![text],
            OperationRewrite::Insert { .. } => continue,
            OperationRewrite::Inline { replacements } => {
                replacements.iter().map(String::as_str).collect()
            }
        };
        for rewrite in rewrites {
            for line in rewrite.split('\n') {
                if let Some(reference) = REFERENCE_RE.captures(line.trim())
                    && reference[1]
                        .parse::<usize>()
                        .is_ok_and(|number| number > index)
                {
                    return Err(parse_error(format!(
                        "{REWRITE_HEADER}{} must reference an earlier operation, not self/forward.",
                        &reference[1]
                    )));
                }
            }
        }
    }
    for (index, message) in pending {
        let normalized_pattern = normalize_text(&operations[index].pattern_text).text;
        let justified = operations.iter().enumerate().any(|(other_index, other)| {
            if other_index == index {
                return false;
            }
            let rewrites: Vec<&str> = match &other.rewrite {
                OperationRewrite::Explicit { text } | OperationRewrite::Insert { text, .. } => {
                    vec![text]
                }
                OperationRewrite::Inline { replacements } => {
                    replacements.iter().map(String::as_str).collect()
                }
            };
            rewrites.iter().any(|rewrite| {
                normalize_text(rewrite).text.contains(&normalized_pattern)
                    || rewrite
                        .split('\n')
                        .any(|line| line.trim() == format!("{REWRITE_HEADER}{}", index + 1))
            })
        });
        if !justified {
            return Err(parse_error(message));
        }
    }
    Ok(operations)
}

/// Parse a canonical or taught sloppy payload into operations. `path` is the
/// authored target, echoed in copy-ready payloads inside parse errors.
pub fn parse_operations(
    input: &str,
    content: &str,
    path: &str,
) -> Result<Vec<Operation>, EditError> {
    parse_section(input, content, path, false)
}

fn render_inline_pattern(pattern_text: &str, replacements: &[String]) -> String {
    let mut rendered = String::new();
    let mut replacement_index = 0;
    let mut index = 0;
    while index < pattern_text.len() {
        let Some(relative) = pattern_text[index..].find(SELECT_OPEN) else {
            rendered.push_str(&pattern_text[index..]);
            break;
        };
        let open = index + relative;
        rendered.push_str(&pattern_text[index..open]);
        let close = pattern_text[open + SELECT_OPEN.len()..]
            .find(SELECT_CLOSE)
            .map_or(pattern_text.len(), |at| at + open + SELECT_OPEN.len());
        rendered.push_str(&pattern_text[open..close]);
        rendered.push_str(SELECT_DIVIDER);
        rendered.push_str(&decode_literal_markers(
            replacements
                .get(replacement_index)
                .map_or("", String::as_str),
        ));
        rendered.push_str(SELECT_CLOSE);
        replacement_index += 1;
        index = (close + SELECT_CLOSE.len()).min(pattern_text.len());
    }
    rendered
}

fn operation_pattern(operation: &Operation, pattern_text: Option<&str>) -> String {
    let pattern = pattern_text.unwrap_or(&operation.pattern_text);
    match &operation.rewrite {
        OperationRewrite::Inline { replacements } if pattern != operation.pattern_text => {
            render_inline_pattern(pattern, replacements)
        }
        OperationRewrite::Inline { .. } => operation.source_pattern_text.clone(),
        OperationRewrite::Explicit { .. } | OperationRewrite::Insert { .. } => pattern.to_owned(),
    }
}

/// File header for a copy-ready payload, quoting paths ambiguous with `all`.
pub(crate) fn edit_header(path: &str, all: bool) -> String {
    let quoted = (path.trim() != path
        || path.starts_with('"')
        || path.contains(['\n', '\r'])
        || path.eq_ignore_ascii_case("all")
        || path
            .rsplit_once(char::is_whitespace)
            .is_some_and(|(_, flag)| flag.eq_ignore_ascii_case("all")))
    .then(|| serde_json::to_string(path).expect("path serialization"));
    format!(
        "*** Edit File: {}{}",
        quoted.as_deref().unwrap_or(path),
        if all { " all" } else { "" }
    )
}

/// Render internal operations as copy-ready section-delimited edits.
pub fn ir_to_payload(lines: &[&str], path: &str) -> String {
    let mut out = String::new();
    let mut in_find = false;
    for line in lines {
        if let Some(opener) = parse_opener(line) {
            out.push_str(&edit_header(path, opener == OpenerKind::All));
            out.push_str("\n*** Find\n");
            in_find = true;
        } else if let Some((at, text)) = strip_insert_header(line)
            .and_then(|(at, encoded)| Some((at, serde_json::from_str::<String>(encoded).ok()?)))
            .filter(|_| in_find)
        {
            out.push_str(at.header());
            out.push('\n');
            out.push_str(&text);
            in_find = false;
        } else if line.trim() == REWRITE_HEADER && in_find {
            out.push_str("*** Replace\n");
            in_find = false;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Render an operation as a copy-ready payload; `all` selects every match.
pub fn operation_payload(
    operation: &Operation,
    path: &str,
    all: bool,
    pattern_text: Option<&str>,
) -> String {
    let open = edit_header(path, all);
    let pattern = operation_pattern(operation, pattern_text);
    match &operation.rewrite {
        OperationRewrite::Inline { .. } => format!("{open}\n*** Find\n{pattern}\n"),
        OperationRewrite::Insert { text, at } => {
            format!("{open}\n*** Find\n{pattern}\n{}\n{text}", at.header())
        }
        OperationRewrite::Explicit { text } => {
            format!("{open}\n*** Find\n{pattern}\n*** Replace\n{text}")
        }
    }
}
