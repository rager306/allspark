
//! R8 — delta core: event-sourced change records with sparse/RLE/hybrid
//! encoding.
//!
//! A [`Delta`] is one immutable, ordered change to a key/value space (the graph
//! is the motivating space, but the core is generic). Deltas are appended to a
//! [`DeltaStream`], which compacts adjacent same-key deltas and can expose
//! sliding [`DeltaWindow`]s over the tail of the log.
//!
//! Encoding. A delta payload is a list of `(key, op)` changes. Three encodings
//! trade size against decode cost:
//!
//! * [`Encoding::Sparse`] — literal `(key, op)` pairs; best for scattered keys.
//! * [`Encoding::Rle`] — run-length over *consecutive* identical ops; best for
//!   bulk add/remove over contiguous keys.
//! * [`Encoding::Hybrid`] — RLE runs where they help, literals otherwise; the
//!   default, and what [`Delta::encode`] chooses automatically.
//!
//! Everything is `serde`-serializable and deterministic.

use serde::{Deserialize, Serialize};

/// What a delta does to one key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    /// Insert or update `key` with `value`.
    Set(String),
    /// Remove `key`.
    Remove,
}

impl Op {
    /// Stable wire tag.
    #[must_use]
    pub const fn tag(&self) -> &'static str {
        match self {
            Self::Set(_) => "set",
            Self::Remove => "remove",
        }
    }
}

/// One keyed change within a delta.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    /// The changed key.
    pub key: String,
    /// The change itself.
    pub op: Op,
}

impl Change {
    /// A `Set` change.
    #[must_use]
    pub fn set(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            op: Op::Set(value.into()),
        }
    }

    /// A `Remove` change.
    #[must_use]
    pub fn remove(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            op: Op::Remove,
        }
    }
}

/// The encoding of a delta's payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Encoding {
    /// Literal `(key, op)` pairs.
    Sparse,
    /// Run-length over consecutive identical ops.
    Rle,
    /// RLE runs plus literals (the default).
    Hybrid,
}

/// A compacted delta payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Payload {
    /// Literal changes.
    Sparse(Vec<Change>),
    /// Runs of `(op, count)` over consecutive keys of one delta.
    Rle(Vec<(Op, u32)>),
    /// A mix: `(op, count)` runs and literal changes, in order.
    Hybrid(Vec<HybridItem>),
}

/// One item of a [`Payload::Hybrid`] payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HybridItem {
    /// A run of `count` identical ops.
    Run(Op, u32),
    /// A single literal change.
    Lit(Change),
}

/// A single immutable delta: a monotonic sequence number, a logical clock, and
/// a payload of keyed changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delta {
    /// Monotonic sequence number within its stream.
    pub seq: u64,
    /// Logical timestamp (caller-supplied; used by windows).
    pub clock: u64,
    /// The changes, in application order.
    pub changes: Vec<Change>,
}

impl Delta {
    /// Build a delta from `changes`.
    #[must_use]
    pub fn new(seq: u64, clock: u64, changes: Vec<Change>) -> Self {
        Self {
            seq,
            clock,
            changes,
        }
    }

    /// Number of keyed changes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.changes.len()
    }

    /// Whether the delta is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Choose and apply an encoding to the payload.
    #[must_use]
    pub fn encode(&self, encoding: Encoding) -> Payload {
        match encoding {
            Encoding::Sparse => Payload::Sparse(self.changes.clone()),
            Encoding::Rle => Payload::Rle(rle_runs(&self.changes)),
            Encoding::Hybrid => Payload::Hybrid(hybrid_items(&self.changes)),
        }
    }

    /// Decode a payload back into its literal changes. `keys` supplies the
    /// consecutive keys a run covers (run-length drops key names, so the key
    /// list must come from the caller's key space — the stream keeps it).
    #[must_use]
    pub fn decode(payload: &Payload, keys: &[String]) -> Vec<Change> {
        match payload {
            Payload::Sparse(changes) => changes.clone(),
            Payload::Rle(runs) => expand_runs(runs, keys),
            Payload::Hybrid(items) => {
                let mut out = Vec::new();
                let mut cursor = 0usize;
                for item in items {
                    match item {
                        HybridItem::Run(op, count) => {
                            for _ in 0..*count {
                                if let Some(k) = keys.get(cursor) {
                                    out.push(Change {
                                        key: k.clone(),
                                        op: op.clone(),
                                    });
                                }
                                cursor += 1;
                            }
                        }
                        HybridItem::Lit(change) => {
                            out.push(change.clone());
                            cursor += 1;
                        }
                    }
                }
                out
            }
        }
    }

    /// The approximate encoded size in bytes (for compression comparisons).
    #[must_use]
    pub fn encoded_size(&self, encoding: Encoding) -> usize {
        match self.encode(encoding) {
            Payload::Sparse(changes) => changes.iter().map(|c| c.key.len() + 8).sum(),
            Payload::Rle(runs) => runs.len() * 12,
            Payload::Hybrid(items) => items
                .iter()
                .map(|i| match i {
                    HybridItem::Run(_, _) => 12,
                    HybridItem::Lit(c) => c.key.len() + 8,
                })
                .sum(),
        }
    }
}

/// Run-length encode consecutive identical ops.
fn rle_runs(changes: &[Change]) -> Vec<(Op, u32)> {
    let mut runs: Vec<(Op, u32)> = Vec::new();
    for change in changes {
        match runs.last_mut() {
            Some((op, count)) if *op == change.op => *count += 1,
            _ => runs.push((change.op.clone(), 1)),
        }
    }
    runs
}

/// Hybrid encode: a run is only worth it when it saves bytes (`count >= 3`);
/// shorter runs degrade to literals.
fn hybrid_items(changes: &[Change]) -> Vec<HybridItem> {
    let mut out: Vec<HybridItem> = Vec::new();
    let mut i = 0usize;
    while i < changes.len() {
        let op = &changes[i].op;
        let mut j = i;
        while j < changes.len() && &changes[j].op == op {
            j += 1;
        }
        let count = j - i;
        if count >= 3 {
            out.push(HybridItem::Run(op.clone(), count as u32));
        } else {
            for change in &changes[i..j] {
                out.push(HybridItem::Lit(change.clone()));
            }
        }
        i = j;
    }
    out
}

fn expand_runs(runs: &[(Op, u32)], keys: &[String]) -> Vec<Change> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    for (op, count) in runs {
        for _ in 0..*count {
            if let Some(k) = keys.get(cursor) {
                out.push(Change {
                    key: k.clone(),
                    op: op.clone(),
                });
            }
            cursor += 1;
        }
    }
    out
}

/// An append-only stream of deltas with compaction and windowing.
#[derive(Debug, Clone, Default)]
pub struct DeltaStream {
    deltas: Vec<Delta>,
    next_seq: u64,
    /// The key space, in order — needed to expand run-length payloads.
    keys: Vec<String>,
}

impl DeltaStream {
    /// An empty stream.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of deltas in the stream.
    #[must_use]
    pub fn len(&self) -> usize {
        self.deltas.len()
    }

    /// Whether the stream is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.deltas.is_empty()
    }

    /// The deltas, oldest first.
    #[must_use]
    pub fn deltas(&self) -> &[Delta] {
        &self.deltas
    }

    /// Append a delta, assigning it the next sequence number. Returns the seq.
    pub fn push(&mut self, clock: u64, changes: Vec<Change>) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.deltas.push(Delta::new(seq, clock, changes));
        seq
    }

    /// Compact the stream by folding each delta's repeated same-key changes to
    /// its final op (last write wins). Adjacent deltas are *not* merged across
    /// sequence boundaries — sequence identity is preserved.
    pub fn compact(&mut self) {
        for delta in &mut self.deltas {
            let mut folded: Vec<Change> = Vec::with_capacity(delta.changes.len());
            // Walk backwards so the last op for a key wins; then reverse.
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            for change in delta.changes.iter().rev() {
                if seen.insert(change.key.clone()) {
                    folded.push(change.clone());
                }
            }
            folded.reverse();
            delta.changes = folded;
        }
    }

    /// Set the key space (needed to decode run-length payloads).
    pub fn set_keys(&mut self, keys: Vec<String>) {
        self.keys = keys;
    }

    /// The keys, in order.
    #[must_use]
    pub fn keys(&self) -> &[String] {
        &self.keys
    }

    /// Round-trip a delta through an encoding and back.
    #[must_use]
    pub fn round_trip(&self, seq: u64, encoding: Encoding) -> Option<Vec<Change>> {
        let delta = self.deltas.iter().find(|d| d.seq == seq)?;
        let payload = delta.encode(encoding);
        Some(Delta::decode(&payload, &self.keys))
    }

    /// A window over the last `count` deltas.
    #[must_use]
    pub fn window(&self, count: usize) -> DeltaWindow<'_> {
        let start = self.deltas.len().saturating_sub(count);
        DeltaWindow {
            deltas: &self.deltas[start..],
        }
    }

    /// A window over deltas whose clock is in `[from, to]` (inclusive).
    #[must_use]
    pub fn window_by_clock(&self, from: u64, to: u64) -> DeltaWindow<'_> {
        let lo = self.deltas.partition_point(|d| d.clock < from);
        let hi = self.deltas.partition_point(|d| d.clock <= to);
        DeltaWindow {
            deltas: &self.deltas[lo..hi],
        }
    }
}

/// A borrowed, read-only window over a contiguous slice of a stream.
#[derive(Debug, Clone, Copy)]
pub struct DeltaWindow<'a> {
    deltas: &'a [Delta],
}

impl<'a> DeltaWindow<'a> {
    /// Number of deltas in the window.
    #[must_use]
    pub fn len(&self) -> usize {
        self.deltas.len()
    }

    /// Whether the window is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.deltas.is_empty()
    }

    /// The deltas in the window.
    #[must_use]
    pub fn deltas(&self) -> &'a [Delta] {
        self.deltas
    }

    /// Fold the window into one delta: keys are folded last-write-wins, with
    /// the window's max clock and a seq equal to the last delta's.
    #[must_use]
    pub fn fold(&self) -> Delta {
        let mut merged: Vec<Change> = Vec::new();
        let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut clock = 0u64;
        let mut seq = 0u64;
        for delta in self.deltas {
            clock = clock.max(delta.clock);
            seq = seq.max(delta.seq);
            for change in &delta.changes {
                if let Some(&i) = index.get(&change.key) {
                    merged[i] = change.clone();
                } else {
                    index.insert(change.key.clone(), merged.len());
                    merged.push(change.clone());
                }
            }
        }
        Delta::new(seq, clock, merged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Delta {
        Delta::new(
            0,
            1,
            vec![
                Change::set("a", "1"),
                Change::set("b", "2"),
                Change::remove("c"),
                Change::remove("d"),
                Change::remove("e"),
            ],
        )
    }

    #[test]
    fn rle_and_sparse_and_hybrid_round_trip() {
        let keys: Vec<String> = ["a", "b", "c", "d", "e"].iter().map(|s| (*s).to_string()).collect();
        let delta = sample();
        for enc in [Encoding::Sparse, Encoding::Rle, Encoding::Hybrid] {
            let payload = delta.encode(enc);
            let decoded = Delta::decode(&payload, &keys);
            assert_eq!(decoded, delta.changes, "round trip failed for {enc:?}");
        }
    }

    #[test]
    fn hybrid_prefers_runs_for_long_sequences() {
        let delta = sample();
        let hybrid = delta.encode(Encoding::Hybrid);
        match hybrid {
            Payload::Hybrid(items) => {
                // Two sets (run of 2 < 3) become literals; three removes (run
                // of 3) become one run.
                assert!(items.iter().any(|i| matches!(i, HybridItem::Run(Op::Remove, 3))));
                assert_eq!(items.iter().filter(|i| matches!(i, HybridItem::Lit(_))).count(), 2);
            }
            _ => panic!("expected hybrid payload"),
        }
    }

    #[test]
    fn hybrid_encoding_is_not_larger_than_sparse_for_runs() {
        let delta = sample();
        assert!(delta.encoded_size(Encoding::Hybrid) <= delta.encoded_size(Encoding::Sparse));
    }

    #[test]
    fn stream_assigns_monotonic_sequences() {
        let mut stream = DeltaStream::new();
        assert_eq!(stream.push(1, vec![Change::set("a", "1")]), 0);
        assert_eq!(stream.push(2, vec![Change::set("a", "2")]), 1);
        assert_eq!(stream.len(), 2);
    }

    #[test]
    fn compact_folds_repeated_keys_last_write_wins() {
        let mut stream = DeltaStream::new();
        stream.push(
            1,
            vec![
                Change::set("a", "1"),
                Change::set("a", "2"),
                Change::set("b", "9"),
            ],
        );
        stream.compact();
        let delta = &stream.deltas()[0];
        assert_eq!(delta.len(), 2, "a folded to its last write: {delta:?}");
        assert_eq!(
            delta.changes[0],
            Change::set("a", "2"),
            "last write wins and order is preserved"
        );
    }

    #[test]
    fn window_over_tail() {
        let mut stream = DeltaStream::new();
        for i in 0..5 {
            stream.push(i, vec![Change::set(format!("k{i}"), "v")]);
        }
        let win = stream.window(2);
        assert_eq!(win.len(), 2);
        assert_eq!(win.deltas()[0].clock, 3);
    }

    #[test]
    fn window_by_clock_selects_a_range() {
        let mut stream = DeltaStream::new();
        for i in 0..10 {
            stream.push(i, vec![Change::set(format!("k{i}"), "v")]);
        }
        let win = stream.window_by_clock(3, 6);
        assert_eq!(win.len(), 4, "clocks 3,4,5,6");
        assert_eq!(win.deltas()[0].clock, 3);
    }

    #[test]
    fn window_fold_merges_last_write_wins() {
        let mut stream = DeltaStream::new();
        stream.push(1, vec![Change::set("a", "1")]);
        stream.push(2, vec![Change::set("a", "2"), Change::set("b", "3")]);
        let folded = stream.window(2).fold();
        assert_eq!(folded.len(), 2);
        assert_eq!(folded.clock, 2);
        assert_eq!(folded.changes[0], Change::set("a", "2"));
    }

    #[test]
    fn stream_round_trip_uses_key_space() {
        let mut stream = DeltaStream::new();
        stream.set_keys(vec!["a".into(), "b".into(), "c".into()]);
        stream.push(1, vec![Change::remove("a"), Change::remove("b"), Change::remove("c")]);
        let decoded = stream.round_trip(0, Encoding::Rle).expect("round trip");
        assert_eq!(decoded.len(), 3);
        assert_eq!(decoded[0], Change::remove("a"));
    }

    #[test]
    fn empty_delta_and_stream_edges() {
        let d = Delta::new(0, 0, Vec::new());
        assert!(d.is_empty());
        assert_eq!(d.len(), 0);
        let stream = DeltaStream::new();
        assert!(stream.is_empty());
        assert_eq!(stream.window(3).len(), 0);
    }
}
