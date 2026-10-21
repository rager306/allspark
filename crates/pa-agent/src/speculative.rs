//! Speculative execution for tool calls: prepare candidates ahead of time and
//! commit them along a dependency graph.
//!
//! A [`SpecEngine`] owns a fixed set of candidate calls ([`SpecCall`]), each
//! already carrying its *prepared* artifact `T` — a rendered tool call, a
//! resolved argument set, a fetched resource: whatever the caller computed
//! speculatively. Calls declare their prerequisites in `depends_on`:
//!
//! ```text
//! diamond:   a ──▶ b ──┐
//!              └──▶ c ──┴──▶ d
//! ```
//!
//! * Every call with no dependencies starts **ready** — it may be committed
//!   (or aborted) immediately.
//! * [`SpecEngine::commit`] marks a ready call committed and returns the
//!   dependents it just unblocked (every prerequisite of theirs is now
//!   committed).
//! * [`SpecEngine::abort`] marks a call aborted and **cascades** the abort
//!   transitively through all of its dependents.
//!
//! [`ParallelSpec::run`] is a small `std::thread` runner on top of the engine
//! for callers whose artifacts come from closures: the ready closures run
//! concurrently, their results are committed, and a panicking closure fails
//! its call and cascades the abort.

use std::collections::{HashMap, VecDeque};
use std::fmt;

/// Stable identifier of a candidate call within one [`SpecEngine`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CallId(u64);

impl CallId {
    /// Wraps a raw numeric id.
    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// Unwraps the raw numeric id.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for CallId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Lifecycle of a single candidate call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecState {
    /// At least one prerequisite is not committed yet.
    Pending,
    /// Every prerequisite is committed (or there are none); committable now.
    Ready,
    /// Committed by the caller.
    Committed,
    /// Aborted by the caller (directly or through a cascade).
    Aborted,
}

/// Failures produced while building or driving a [`SpecEngine`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SpecError {
    /// No call with this id exists in the plan.
    #[error("unknown call id {0}")]
    UnknownCall(CallId),
    /// Two calls share the same id.
    #[error("duplicate call id {0}")]
    DuplicateCall(CallId),
    /// A call depends on an id that is not part of the plan.
    #[error("call {call} depends on unknown call {dep}")]
    UnknownDependency {
        /// The dependent call.
        call: CallId,
        /// The missing prerequisite.
        dep: CallId,
    },
    /// The dependency graph contains a cycle, so some calls can never commit.
    #[error("dependency cycle in speculative plan")]
    CyclicDependency,
    /// The call was already committed.
    #[error("call {0} is already committed")]
    AlreadyCommitted(CallId),
    /// The call was already aborted.
    #[error("call {0} is already aborted")]
    AlreadyAborted(CallId),
    /// The call still has prerequisites that are not committed.
    #[error("call {0} still has uncommitted dependencies")]
    DependenciesPending(CallId),
}

/// One candidate: an id, its prerequisites, and the value prepared ahead of
/// time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecCall<T> {
    /// This call's id (unique within the plan).
    pub id: CallId,
    /// Ids that must be committed before this call becomes ready.
    pub depends_on: Vec<CallId>,
    /// The artifact prepared speculatively for this call.
    pub prepare: T,
}

impl<T> SpecCall<T> {
    /// Builds a candidate from its id, prerequisites, and prepared artifact.
    #[must_use]
    pub fn new(id: CallId, depends_on: Vec<CallId>, prepare: T) -> Self {
        Self {
            id,
            depends_on,
            prepare,
        }
    }
}

/// A whole speculative plan: the candidate calls handed to
/// [`SpecEngine::new`].
pub type SpeculativePlan<T> = Vec<SpecCall<T>>;

#[derive(Debug)]
struct Node<T> {
    id: CallId,
    /// The prepared artifact; `None` once taken by a runner.
    prepare: Option<T>,
    state: SpecState,
    /// Indices of calls that depend on this one.
    dependents: Vec<usize>,
    /// How many of this call's prerequisites are still uncommitted.
    remaining: usize,
}

/// Dependency-graph scheduler over a fixed set of prepared candidates.
///
/// See the module docs for the commit/abort semantics.
#[derive(Debug)]
pub struct SpecEngine<T> {
    nodes: Vec<Node<T>>,
    index: HashMap<CallId, usize>,
    committed: Vec<CallId>,
    aborted: Vec<CallId>,
}

impl<T> SpecEngine<T> {
    /// Builds an engine and marks every dependency-free call ready.
    ///
    /// # Errors
    ///
    /// Returns [`SpecError::DuplicateCall`] on repeated ids,
    /// [`SpecError::UnknownDependency`] when a prerequisite is not part of the
    /// plan, and [`SpecError::CyclicDependency`] when the graph has a cycle.
    pub fn new(calls: SpeculativePlan<T>) -> Result<Self, SpecError> {
        let mut index = HashMap::with_capacity(calls.len());
        for (i, call) in calls.iter().enumerate() {
            if index.insert(call.id, i).is_some() {
                return Err(SpecError::DuplicateCall(call.id));
            }
        }

        // Resolve every edge first so an unknown prerequisite is reported
        // before the calls are consumed into nodes.
        let mut edges: Vec<(usize, usize)> = Vec::new();
        for (i, call) in calls.iter().enumerate() {
            for dep in &call.depends_on {
                let Some(&j) = index.get(dep) else {
                    return Err(SpecError::UnknownDependency {
                        call: call.id,
                        dep: *dep,
                    });
                };
                edges.push((j, i));
            }
        }

        let mut nodes: Vec<Node<T>> = calls
            .into_iter()
            .map(|call| Node {
                id: call.id,
                remaining: call.depends_on.len(),
                prepare: Some(call.prepare),
                state: SpecState::Pending,
                dependents: Vec::new(),
            })
            .collect();
        for (dep, dependent) in edges {
            nodes[dep].dependents.push(dependent);
        }

        if has_cycle(&nodes) {
            return Err(SpecError::CyclicDependency);
        }

        for node in &mut nodes {
            if node.remaining == 0 {
                node.state = SpecState::Ready;
            }
        }

        Ok(Self {
            nodes,
            index,
            committed: Vec::new(),
            aborted: Vec::new(),
        })
    }

    fn lookup(&self, id: CallId) -> Result<usize, SpecError> {
        self.index
            .get(&id)
            .copied()
            .ok_or(SpecError::UnknownCall(id))
    }

    /// Ids of the calls that are ready to commit, in plan order.
    #[must_use]
    pub fn ready(&self) -> Vec<CallId> {
        self.nodes
            .iter()
            .filter(|n| n.state == SpecState::Ready)
            .map(|n| n.id)
            .collect()
    }

    /// Ids of the calls still blocked on uncommitted prerequisites.
    #[must_use]
    pub fn pending(&self) -> Vec<CallId> {
        self.nodes
            .iter()
            .filter(|n| n.state == SpecState::Pending)
            .map(|n| n.id)
            .collect()
    }

    /// Ids of the committed calls, in commit order.
    #[must_use]
    pub fn committed(&self) -> Vec<CallId> {
        self.committed.clone()
    }

    /// Ids of the aborted calls, in abort order.
    #[must_use]
    pub fn aborted(&self) -> Vec<CallId> {
        self.aborted.clone()
    }

    /// Current lifecycle state of a call, if it exists.
    #[must_use]
    pub fn state(&self, id: CallId) -> Option<SpecState> {
        self.lookup(id).ok().map(|i| self.nodes[i].state)
    }

    /// Borrows the artifact prepared for a call, if it exists.
    #[must_use]
    pub fn prepared(&self, id: CallId) -> Option<&T> {
        self.lookup(id)
            .ok()
            .and_then(|i| self.nodes[i].prepare.as_ref())
    }

    /// Takes the artifact prepared for a call out of the engine.
    #[must_use]
    pub fn take_prepared(&mut self, id: CallId) -> Option<T> {
        let i = self.lookup(id).ok()?;
        self.nodes[i].prepare.take()
    }

    /// Number of calls in the plan.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the plan has no calls.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Commits a ready call and returns the dependents it unblocked, in plan
    /// order.
    ///
    /// # Errors
    ///
    /// Returns [`SpecError::UnknownCall`] for an id outside the plan,
    /// [`SpecError::AlreadyCommitted`] / [`SpecError::AlreadyAborted`] when the
    /// call is not live, and [`SpecError::DependenciesPending`] when a
    /// prerequisite is still uncommitted.
    pub fn commit(&mut self, id: CallId) -> Result<Vec<CallId>, SpecError> {
        let i = self.lookup(id)?;
        match self.nodes[i].state {
            SpecState::Committed => return Err(SpecError::AlreadyCommitted(id)),
            SpecState::Aborted => return Err(SpecError::AlreadyAborted(id)),
            SpecState::Pending => return Err(SpecError::DependenciesPending(id)),
            SpecState::Ready => {}
        }
        self.nodes[i].state = SpecState::Committed;
        self.committed.push(id);

        let mut unblocked = Vec::new();
        let dependents = std::mem::take(&mut self.nodes[i].dependents);
        for j in &dependents {
            self.nodes[*j].remaining -= 1;
            if self.nodes[*j].remaining == 0 && self.nodes[*j].state == SpecState::Pending {
                self.nodes[*j].state = SpecState::Ready;
                unblocked.push(self.nodes[*j].id);
            }
        }
        self.nodes[i].dependents = dependents;
        Ok(unblocked)
    }

    /// Aborts a call and cascades the abort transitively through its
    /// dependents. Returns the aborted call followed by every dependent that
    /// the cascade cancelled, in breadth-first order.
    ///
    /// # Errors
    ///
    /// Returns [`SpecError::UnknownCall`] for an id outside the plan,
    /// [`SpecError::AlreadyCommitted`] when the call already committed, and
    /// [`SpecError::AlreadyAborted`] when it was already aborted.
    pub fn abort(&mut self, id: CallId) -> Result<Vec<CallId>, SpecError> {
        let start = self.lookup(id)?;
        match self.nodes[start].state {
            SpecState::Committed => return Err(SpecError::AlreadyCommitted(id)),
            SpecState::Aborted => return Err(SpecError::AlreadyAborted(id)),
            SpecState::Pending | SpecState::Ready => {}
        }

        let mut cancelled = Vec::new();
        let mut queue = VecDeque::from([start]);
        while let Some(i) = queue.pop_front() {
            match self.nodes[i].state {
                SpecState::Aborted | SpecState::Committed => continue,
                SpecState::Pending | SpecState::Ready => {}
            }
            self.nodes[i].state = SpecState::Aborted;
            let call = self.nodes[i].id;
            self.aborted.push(call);
            cancelled.push(call);
            for &d in &self.nodes[i].dependents {
                queue.push_back(d);
            }
        }
        Ok(cancelled)
    }
}

/// Kahn's algorithm over the prerequisite counts: `true` when the graph has a
/// cycle, i.e. some calls never reach zero uncommitted prerequisites.
fn has_cycle<T>(nodes: &[Node<T>]) -> bool {
    let mut remaining: Vec<usize> = nodes.iter().map(|n| n.remaining).collect();
    let mut queue: VecDeque<usize> = (0..nodes.len()).filter(|&i| remaining[i] == 0).collect();
    let mut seen = 0_usize;
    while let Some(i) = queue.pop_front() {
        seen += 1;
        for &j in &nodes[i].dependents {
            remaining[j] -= 1;
            if remaining[j] == 0 {
                queue.push_back(j);
            }
        }
    }
    seen != nodes.len()
}

/// Result of a [`ParallelSpec::run`].
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome<T> {
    /// Every call committed; the committed artifacts in commit order.
    Completed {
        /// `(call id, artifact)` for each committed call, in commit order.
        committed: Vec<(CallId, T)>,
    },
    /// At least one call was aborted; the surviving committed artifacts plus
    /// every aborted id, in abort order.
    Aborted {
        /// `(call id, artifact)` for each committed call, in commit order.
        committed: Vec<(CallId, T)>,
        /// Every call cancelled directly or by the cascade, in abort order.
        aborted: Vec<CallId>,
    },
}

/// Concurrent runner for callers whose artifacts come from closures.
///
/// Each ready [`SpecJob`] is spawned on its own `std::thread` (through
/// [`std::thread::scope`]); successful results are committed and a panicking
/// closure aborts its call, cascading through the dependents. Waves repeat
/// until no call is ready, so a job always observes its prerequisites
/// committed.
pub struct ParallelSpec;

/// A speculatively prepared job: a boxed closure that produces the artifact.
pub type SpecJob<T> = Box<dyn FnOnce() -> T + Send>;

impl ParallelSpec {
    /// Runs every call in the plan, returning the committed artifacts and, if
    /// anything failed, the aborted ids.
    ///
    /// # Errors
    ///
    /// Propagates the same construction errors as [`SpecEngine::new`]
    /// (duplicate ids, unknown prerequisites, cycles).
    pub fn run<T>(calls: SpeculativePlan<SpecJob<T>>) -> Result<Outcome<T>, SpecError>
    where
        T: Send,
    {
        let mut engine = SpecEngine::new(calls)?;
        let mut results: Vec<(CallId, T)> = Vec::new();

        loop {
            let ready = engine.ready();
            if ready.is_empty() {
                break;
            }

            let mut batch: Vec<(CallId, SpecJob<T>)> = Vec::with_capacity(ready.len());
            for id in ready {
                if let Some(job) = engine.take_prepared(id) {
                    batch.push((id, job));
                }
            }

            let outcomes: Vec<(CallId, std::thread::Result<T>)> = std::thread::scope(|scope| {
                let handles: Vec<_> = batch
                    .into_iter()
                    .map(|(id, job)| (id, scope.spawn(job)))
                    .collect();
                handles
                    .into_iter()
                    .map(|(id, handle)| (id, handle.join()))
                    .collect()
            });

            for (id, outcome) in outcomes {
                match outcome {
                    Ok(value) => {
                        engine.commit(id)?;
                        results.push((id, value));
                    }
                    Err(_) => {
                        engine.abort(id)?;
                    }
                }
            }
        }

        let aborted = engine.aborted();
        if aborted.is_empty() {
            Ok(Outcome::Completed { committed: results })
        } else {
            Ok(Outcome::Aborted {
                committed: results,
                aborted,
            })
        }
    }
}

/// Boxes a closure into a [`SpecJob`], for use with [`ParallelSpec::run`].
pub fn job<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> SpecJob<T> {
    Box::new(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(raw: u64) -> CallId {
        CallId::new(raw)
    }

    /// a -> b, a -> c, b/c -> d
    fn diamond() -> SpecEngine<&'static str> {
        SpecEngine::new(vec![
            SpecCall::new(id(1), vec![], "a"),
            SpecCall::new(id(2), vec![id(1)], "b"),
            SpecCall::new(id(3), vec![id(1)], "c"),
            SpecCall::new(id(4), vec![id(2), id(3)], "d"),
        ])
        .unwrap()
    }

    #[test]
    fn dependency_free_calls_start_ready() {
        let engine = diamond();
        assert_eq!(engine.ready(), vec![id(1)]);
        assert_eq!(engine.pending(), vec![id(2), id(3), id(4)]);
        assert_eq!(engine.committed(), Vec::<CallId>::new());
        assert_eq!(engine.aborted(), Vec::<CallId>::new());
        assert_eq!(engine.state(id(1)), Some(SpecState::Ready));
    }

    #[test]
    fn commit_unblocks_dependents_in_order() {
        let mut engine = diamond();
        assert_eq!(engine.commit(id(1)).unwrap(), vec![id(2), id(3)]);
        assert_eq!(engine.ready(), vec![id(2), id(3)]);
        assert_eq!(engine.pending(), vec![id(4)]);
        assert_eq!(engine.committed(), vec![id(1)]);
    }

    #[test]
    fn commit_requires_dependencies_first() {
        let mut engine = diamond();
        assert_eq!(
            engine.commit(id(4)).unwrap_err(),
            SpecError::DependenciesPending(id(4))
        );
    }

    #[test]
    fn commit_order_respects_dependencies() {
        let mut engine = diamond();
        engine.commit(id(1)).unwrap();
        engine.commit(id(2)).unwrap();
        // d still waits on c.
        assert_eq!(engine.pending(), vec![id(4)]);
        assert_eq!(engine.commit(id(3)).unwrap(), vec![id(4)]);
        assert_eq!(engine.commit(id(4)).unwrap(), Vec::<CallId>::new());
        assert_eq!(engine.committed(), vec![id(1), id(2), id(3), id(4)]);
        assert_eq!(engine.ready().len(), 0);
    }

    #[test]
    fn abort_cascades_but_spares_independent_branch() {
        let mut engine = diamond();
        engine.commit(id(1)).unwrap();
        assert_eq!(engine.abort(id(2)).unwrap(), vec![id(2), id(4)]);
        assert_eq!(engine.aborted(), vec![id(2), id(4)]);
        // c is untouched and still committable.
        assert_eq!(engine.state(id(3)), Some(SpecState::Ready));
        assert_eq!(engine.commit(id(3)).unwrap(), Vec::<CallId>::new());
        assert_eq!(engine.committed(), vec![id(1), id(3)]);
        assert_eq!(engine.ready().len(), 0);
    }

    #[test]
    fn abort_transitively_cancels_a_chain() {
        let mut engine = SpecEngine::new(vec![
            SpecCall::new(id(1), vec![], "a"),
            SpecCall::new(id(2), vec![id(1)], "b"),
            SpecCall::new(id(3), vec![id(2)], "c"),
        ])
        .unwrap();
        assert_eq!(engine.abort(id(1)).unwrap(), vec![id(1), id(2), id(3)]);
        assert_eq!(engine.aborted(), vec![id(1), id(2), id(3)]);
        assert_eq!(engine.state(id(3)), Some(SpecState::Aborted));
    }

    #[test]
    fn recommitting_a_committed_call_errors() {
        let mut engine = diamond();
        engine.commit(id(1)).unwrap();
        assert_eq!(
            engine.commit(id(1)).unwrap_err(),
            SpecError::AlreadyCommitted(id(1))
        );
    }

    #[test]
    fn recommitting_an_aborted_call_errors() {
        let mut engine = diamond();
        engine.commit(id(1)).unwrap();
        engine.abort(id(2)).unwrap();
        assert_eq!(
            engine.commit(id(2)).unwrap_err(),
            SpecError::AlreadyAborted(id(2))
        );
        assert_eq!(
            engine.abort(id(2)).unwrap_err(),
            SpecError::AlreadyAborted(id(2))
        );
    }

    #[test]
    fn committing_after_abort_of_the_whole_branch_errors() {
        let mut engine = diamond();
        engine.abort(id(1)).unwrap();
        assert_eq!(
            engine.commit(id(4)).unwrap_err(),
            SpecError::AlreadyAborted(id(4))
        );
    }

    #[test]
    fn unknown_ids_are_rejected() {
        let mut engine = diamond();
        assert_eq!(
            engine.commit(id(99)).unwrap_err(),
            SpecError::UnknownCall(id(99))
        );
        assert_eq!(
            engine.abort(id(99)).unwrap_err(),
            SpecError::UnknownCall(id(99))
        );
        assert_eq!(engine.state(id(99)), None);
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let err = SpecEngine::new(vec![
            SpecCall::new(id(1), vec![], "a"),
            SpecCall::new(id(1), vec![], "b"),
        ])
        .unwrap_err();
        assert_eq!(err, SpecError::DuplicateCall(id(1)));
    }

    #[test]
    fn unknown_dependency_is_rejected() {
        let err = SpecEngine::new(vec![SpecCall::new(id(1), vec![id(2)], "a")]).unwrap_err();
        assert_eq!(
            err,
            SpecError::UnknownDependency {
                call: id(1),
                dep: id(2),
            }
        );
    }

    #[test]
    fn cycles_are_rejected() {
        let err = SpecEngine::new(vec![
            SpecCall::new(id(1), vec![id(2)], "a"),
            SpecCall::new(id(2), vec![id(1)], "b"),
        ])
        .unwrap_err();
        assert_eq!(err, SpecError::CyclicDependency);
    }

    #[test]
    fn prepared_values_are_borrowable_and_takeable() {
        let mut engine = diamond();
        assert_eq!(engine.prepared(id(3)), Some(&"c"));
        assert_eq!(engine.take_prepared(id(3)), Some("c"));
        assert_eq!(engine.prepared(id(3)), None);
    }

    #[test]
    fn parallel_spec_commits_independent_calls() {
        let calls = vec![
            SpecCall::new(id(1), vec![], job(|| 1)),
            SpecCall::new(id(2), vec![], job(|| 2)),
            SpecCall::new(id(3), vec![id(1), id(2)], job(|| 3)),
        ];
        let outcome = ParallelSpec::run::<i32>(calls).unwrap();
        assert_eq!(
            outcome,
            Outcome::Completed {
                committed: vec![(id(1), 1), (id(2), 2), (id(3), 3)],
            }
        );
    }

    #[test]
    fn parallel_spec_aborts_and_cascades_on_panic() {
        let calls = vec![
            SpecCall::new(id(1), vec![], job(|| 1)),
            SpecCall::new(id(2), vec![], job(|| -> i32 { panic!("boom") })),
            SpecCall::new(id(3), vec![id(1)], job(|| 3)),
            SpecCall::new(id(4), vec![id(1), id(2)], job(|| 4)),
        ];
        let outcome = ParallelSpec::run::<i32>(calls).unwrap();
        match outcome {
            Outcome::Aborted { committed, aborted } => {
                // 2 panicked; 4 depended on it and is cascaded; 1 and 3 survive.
                assert_eq!(aborted, vec![id(2), id(4)]);
                assert_eq!(committed, vec![(id(1), 1), (id(3), 3)]);
            }
            other @ Outcome::Completed { .. } => {
                panic!("expected aborted outcome, got {other:?}")
            }
        }
    }
}
