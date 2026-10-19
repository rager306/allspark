//! M1 — coherence-weighted memory compaction (ruvector `agent-memory`).
//!
//! Agents accumulate memory faster than they retire it: near-duplicate facts
//! crowd the store, stale entries crowd out fresh ones, and brute-force recall
//! costs grow with the pile. Compaction merges entries that are *about the same
//! thing* and keeps the ones that matter most.
//!
//! An entry's intensity follows the ruvector `CoherencePolicy`:
//!
//! ```text
//! I = α·recency + β·frequency + γ·coherence
//! ```
//!
//! where `coherence` is the maximum cosine between the entry's embedding and
//! the rest of the context window (the "is this about what the agent is
//! currently doing?" signal). [`CoherenceCompactor::compact`] first merges
//! entries whose embeddings are at least `merge_threshold` similar (unioning
//! provenance), then ranks the survivors by intensity and truncates to
//! `max_entries`. Everything is LLM-free and deterministic.

use crate::hybrid::cosine;

/// One memory entry subject to compaction.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Stable entry id.
    pub id: String,
    /// Human-readable text (the payload).
    pub text: String,
    /// Dense embedding used for the coherence / duplicate signal.
    pub embedding: Vec<f32>,
    /// Recency weight (`[0, 1]` by convention; higher = newer).
    pub recency: f64,
    /// Frequency weight (access count, etc.; higher = more used).
    pub frequency: f64,
    /// Ids of the sources that produced this entry (preserved through merges).
    pub provenance: Vec<String>,
}

impl Entry {
    /// A new entry with `recency = 0`, `frequency = 1`, empty provenance.
    #[must_use]
    pub fn new(id: impl Into<String>, text: impl Into<String>, embedding: Vec<f32>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            embedding,
            recency: 0.0,
            frequency: 1.0,
            provenance: Vec::new(),
        }
    }

    /// Set the recency weight.
    #[must_use]
    pub fn with_recency(mut self, recency: f64) -> Self {
        self.recency = recency;
        self
    }

    /// Set the frequency weight.
    #[must_use]
    pub fn with_frequency(mut self, frequency: f64) -> Self {
        self.frequency = frequency;
        self
    }

    /// Set the provenance ids.
    #[must_use]
    pub fn with_provenance(mut self, provenance: Vec<String>) -> Self {
        self.provenance = provenance;
        self
    }
}

/// The compaction policy: intensity weights, merge threshold, and cap.
#[derive(Debug, Clone, PartialEq)]
pub struct CoherencePolicy {
    /// Weight on recency.
    pub alpha: f64,
    /// Weight on frequency.
    pub beta: f64,
    /// Weight on coherence.
    pub gamma: f64,
    /// Cosine at or above which two entries are merged.
    pub merge_threshold: f32,
    /// Maximum number of entries to keep (`usize::MAX` = no cap).
    pub max_entries: usize,
}

impl Default for CoherencePolicy {
    fn default() -> Self {
        Self {
            alpha: 1.0,
            beta: 0.5,
            gamma: 1.0,
            merge_threshold: 0.9,
            max_entries: usize::MAX,
        }
    }
}

impl CoherencePolicy {
    /// The default policy.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the intensity weights.
    #[must_use]
    pub fn with_weights(mut self, alpha: f64, beta: f64, gamma: f64) -> Self {
        self.alpha = alpha;
        self.beta = beta;
        self.gamma = gamma;
        self
    }

    /// Set the merge threshold (clamped to `[0, 1]`).
    #[must_use]
    pub fn with_merge_threshold(mut self, threshold: f32) -> Self {
        self.merge_threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Set the maximum number of entries to keep.
    #[must_use]
    pub fn with_max_entries(mut self, max_entries: usize) -> Self {
        self.max_entries = max_entries;
        self
    }

    /// The intensity `I = α·recency + β·frequency + γ·coherence`.
    #[must_use]
    pub fn intensity(&self, entry: &Entry, coherence: f64) -> f64 {
        self.alpha * entry.recency + self.beta * entry.frequency + self.gamma * coherence
    }
}

/// Maximum cosine between `entry` and the rest of `context` (excluding entries
/// sharing its id). Returns `0.0` for an empty context.
#[must_use]
pub fn coherence(entry: &Entry, context: &[Entry]) -> f64 {
    let mut best = 0.0_f64;
    for other in context {
        if other.id == entry.id {
            continue;
        }
        let value = f64::from(cosine(&entry.embedding, &other.embedding));
        if value > best {
            best = value;
        }
    }
    best
}

/// Coherence-weighted memory compactor.
#[derive(Debug, Clone, Copy, Default)]
pub struct CoherenceCompactor;

impl CoherenceCompactor {
    /// Merge near-duplicates, then rank by intensity and truncate to
    /// `policy.max_entries`.
    ///
    /// Merging keeps the first entry seen as the representative (so the output
    /// is a deterministic function of the input order), unions provenance, sums
    /// frequency, and takes the max recency. The final ranking is by intensity
    /// descending, ties broken by id ascending.
    #[must_use]
    pub fn compact(entries: &[Entry], policy: &CoherencePolicy) -> Vec<Entry> {
        let mut representatives: Vec<Entry> = Vec::new();
        for entry in entries {
            let mut merged = false;
            for rep in &mut representatives {
                if f64::from(cosine(&entry.embedding, &rep.embedding))
                    >= f64::from(policy.merge_threshold)
                {
                    rep.provenance.extend(entry.provenance.iter().cloned());
                    rep.provenance.sort();
                    rep.provenance.dedup();
                    rep.frequency += entry.frequency;
                    rep.recency = rep.recency.max(entry.recency);
                    merged = true;
                    break;
                }
            }
            if !merged {
                representatives.push(entry.clone());
            }
        }

        let mut scored: Vec<(f64, Entry)> = representatives
            .into_iter()
            .map(|rep| {
                let coh = coherence(&rep, entries);
                (policy.intensity(&rep, coh), rep)
            })
            .collect();
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.id.cmp(&b.1.id))
        });
        if policy.max_entries < scored.len() {
            scored.truncate(policy.max_entries);
        }
        scored.into_iter().map(|(_, entry)| entry).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, recency: f64, frequency: f64, embedding: Vec<f32>) -> Entry {
        Entry::new(id, id, embedding)
            .with_recency(recency)
            .with_frequency(frequency)
    }

    #[test]
    fn near_duplicates_are_merged_with_provenance() {
        let entries = vec![
            entry("a", 0.5, 1.0, vec![1.0, 0.0, 0.0]).with_provenance(vec!["src-a".into()]),
            entry("b", 0.9, 2.0, vec![0.99, 0.01, 0.0]).with_provenance(vec!["src-b".into()]),
            entry("c", 0.1, 1.0, vec![0.0, 1.0, 0.0]),
        ];
        let policy = CoherencePolicy::new().with_merge_threshold(0.9);
        let out = CoherenceCompactor::compact(&entries, &policy);
        assert_eq!(out.len(), 2, "a and b merge; c stays separate");
        let merged = out.iter().find(|e| e.id == "a").expect("a present");
        assert_eq!(merged.provenance, vec!["src-a", "src-b"]);
        assert_eq!(
            merged.frequency.to_bits(),
            3.0_f64.to_bits(),
            "frequencies sum on merge"
        );
        assert_eq!(
            merged.recency.to_bits(),
            0.9_f64.to_bits(),
            "max recency wins"
        );
    }

    #[test]
    fn truncation_keeps_the_highest_intensity() {
        // Orthogonal embeddings keep coherence at 0, so intensity is driven by
        // the recency/frequency weights alone.
        let entries = vec![
            entry("low", 0.0, 0.0, vec![1.0, 0.0, 0.0]),
            entry("high", 1.0, 5.0, vec![0.0, 1.0, 0.0]),
            entry("mid", 0.5, 0.5, vec![0.0, 0.0, 1.0]),
        ];
        let policy = CoherencePolicy::new()
            .with_merge_threshold(0.5)
            .with_max_entries(2);
        let out = CoherenceCompactor::compact(&entries, &policy);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "high");
        assert_eq!(out[1].id, "mid");
    }

    #[test]
    fn coherence_prefers_entries_about_the_context() {
        let context = vec![
            entry("q", 0.0, 0.0, vec![1.0, 0.0]),
            entry("aligned", 0.0, 0.0, vec![1.0, 0.0]),
            entry("orthogonal", 0.0, 0.0, vec![0.0, 1.0]),
        ];
        let aligned = coherence(&context[1], &context);
        let orthogonal = coherence(&context[2], &context);
        assert!(aligned > 0.9);
        assert!(orthogonal < 0.01);
    }

    #[test]
    fn empty_input_compacts_to_empty() {
        let out = CoherenceCompactor::compact(&[], &CoherencePolicy::new());
        assert!(out.is_empty(), "empty input compacts to empty");
    }

    #[test]
    fn compact_is_deterministic() {
        // Orthogonal embeddings (cosine 0) and equal weights produce a tie,
        // which must break by id ascending.
        let entries = vec![
            entry("b", 0.4, 1.0, vec![1.0, 0.0]),
            entry("a", 0.4, 1.0, vec![0.0, 1.0]),
        ];
        let policy = CoherencePolicy::new().with_merge_threshold(0.5);
        let first = CoherenceCompactor::compact(&entries, &policy);
        let second = CoherenceCompactor::compact(&entries, &policy);
        assert_eq!(first, second);
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].id, "a", "ties break by id ascending");
    }
}
