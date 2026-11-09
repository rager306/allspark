//! The cron scheduler: wake-timer loop, claim-due dispatch, per-session dispatch lanes.
//!
//! Deliberate TS divergence (`cron-jobs.ts` re-fires a failing job at full
//! cadence forever): consecutive failures back off (see [`FAILURE_BACKOFF_BASE_MS`]).

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::{Mutex, Notify};

use super::store::{AgentCronDispatch, AgentCronJobStore, DispatchResultOptions};

const MAX_TIMEOUT_MS: u64 = 2_147_483_647;
/// How long the wake timer waits before retrying a startup recovery that could not
/// write its state (a sibling process held the lock). One attempt per wait, and the
/// wait never depends on the job catalog: the repaired one-shot has no `next_run_at`.
const RECOVERY_RETRY: std::time::Duration = std::time::Duration::from_secs(30);

const FAILURE_BACKOFF_BASE_MS: u64 = 120_000;

/// The backoff ceiling: a failing job still gets one fire per hour, so a
/// recovered route is picked back up without a manual wake.
const FAILURE_BACKOFF_CAP_MS: u64 = 3_600_000;

pub(crate) fn failure_backoff_ms(consecutive_failures: u32) -> u64 {
    FAILURE_BACKOFF_BASE_MS
        .saturating_mul(2u64.saturating_pow(consecutive_failures.saturating_sub(1)))
        .min(FAILURE_BACKOFF_CAP_MS)
}

type PendingDispatch = (AgentCronDispatch, Option<Box<dyn FnOnce() + Send>>);

/// Scheduler hooks: how claimed jobs actually run.
pub trait AgentCronSchedulerHooks: Send + Sync {
    /// Run one claimed job; return `Some("skipped")` to record a skip.
    fn run_job(
        &self,
        job: &super::AgentCronJob,
    ) -> impl Future<Output = anyhow::Result<Option<&'static str>>> + Send;
    /// Observe a dispatch being handed to a lane; the returned closure is
    /// invoked when the lane's work settles (even on errors).
    fn begin_dispatch(&self, _dispatch: &AgentCronDispatch) -> Option<Box<dyn FnOnce() + Send>> {
        None
    }
    fn now(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or_default()
    }
    fn on_error(&self, _job: &super::AgentCronJob, _error: &str) {}
}

pub struct AgentCronScheduler<H: AgentCronSchedulerHooks> {
    core: Arc<SchedulerCore<H>>,
    timer: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// State shared with the timer task.
pub struct SchedulerCore<H: AgentCronSchedulerHooks> {
    store: Arc<AgentCronJobStore>,
    hooks: Arc<H>,
    running: AtomicBool,
    stopped: AtomicBool,
    has_started: AtomicBool,
    dispatch_lanes: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// Consecutive failed fires per job id (in-memory; a daemon restart starts fresh).
    failure_streaks: std::sync::Mutex<HashMap<String, u32>>,
    /// The startup recovery did not land (a state lock held elsewhere, a failed write).
    /// A claimed one-shot has no `next_run_at` once it is dispatched, so nothing in the
    /// job catalog keeps the wake timer armed for it: without this flag the scheduler
    /// parks forever and the interrupted job stays stuck after the lock clears
    /// (review #28).
    recovery_pending: AtomicBool,
    wake: Notify,
}

impl<H: AgentCronSchedulerHooks + 'static> AgentCronScheduler<H> {
    pub fn new(store: Arc<AgentCronJobStore>, hooks: Arc<H>) -> Self {
        Self {
            core: Arc::new(SchedulerCore {
                store,
                hooks,
                running: AtomicBool::new(false),
                stopped: AtomicBool::new(true),
                has_started: AtomicBool::new(false),
                dispatch_lanes: Mutex::new(HashMap::new()),
                failure_streaks: std::sync::Mutex::new(HashMap::new()),
                recovery_pending: AtomicBool::new(false),
                wake: Notify::new(),
            }),
            timer: Mutex::new(None),
        }
    }

    /// Start the scheduler; recovers interrupted dispatches on first start.
    ///
    /// A recovery that cannot write (the state lock belongs to another process) is
    /// retried by the wake timer with a bounded backoff rather than dropped: the
    /// catalogs' `next_run_at` cannot express a claimed one-shot, so the timer is the
    /// only thing that can come back to it (review #28).
    pub async fn start(&self) {
        self.core.stopped.store(false, Ordering::SeqCst);
        if !self.core.has_started.swap(true, Ordering::SeqCst) {
            self.core.recover_interrupted();
        }
        self.schedule_next().await;
    }

    pub async fn stop(&self) {
        self.core.stopped.store(true, Ordering::SeqCst);
        if let Some(handle) = self.timer.lock().await.take() {
            handle.abort();
        }
    }

    pub async fn wake(&self) {
        if self.core.stopped.load(Ordering::SeqCst) {
            return;
        }
        self.core.wake.notify_waiters();
        self.schedule_next().await;
    }

    /// Claim all due jobs and dispatch them. Returns how many ran.
    ///
    /// # Errors
    ///
    /// Returns an error when a cron state lock or write fails.
    pub async fn run_due(&self) -> anyhow::Result<usize> {
        self.core.run_due_at(self.core.hooks.now()).await
    }
}

/// Panic-safe reset for the scheduler's run-pass flag: cleared on drop
/// however the pass unwinds.
struct RunningGuard<'a>(&'a AtomicBool);

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl<H: AgentCronSchedulerHooks + 'static> SchedulerCore<H> {
    /// Run one dispatch pass for jobs due at or before `now` dispatching when the scheduler is
    /// stopped or another pass is already running.
    ///
    /// # Errors
    ///
    /// Returns an error when a cron state lock or write fails.
    pub async fn run_due_at(&self, now: u64) -> anyhow::Result<usize> {
        // The pass claim is atomic: exactly one pass runs at a time.
        if (self.stopped.load(Ordering::SeqCst) && self.has_started.load(Ordering::SeqCst))
            || self
                .running
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
        {
            return Ok(0);
        }
        // Panic safety: an unwinding pass must not wedge the re-entrancy flag at `true`.
        let _running = RunningGuard(&self.running);
        // Recover interrupted dispatches before claiming: a claim left
        // by an unwound pass is released here.
        self.store.recover_interrupted_dispatches(now)?;
        // A partial multi-partition claim still produced work: those rows are
        // marked claimed on disk, so they must RUN before the error surfaces —
        // otherwise the claim is left for a recovery pass to treat as interrupted
        // (review #3). Dispatch first, then report.
        let (claimed, claim_failure) = match self.store.claim_due(now, self.hooks.now()) {
            Ok(claimed) => (claimed, None),
            Err((error, landed)) => (landed, Some(error)),
        };
        let dispatches: Vec<PendingDispatch> = claimed
            .into_iter()
            .map(|dispatch| {
                let end_dispatch = self.hooks.begin_dispatch(&dispatch);
                (dispatch, end_dispatch)
            })
            .collect();
        let ran = self.dispatch_all(dispatches).await;
        if let Some(error) = claim_failure {
            return Err(error);
        }
        Ok(ran)
    }

    async fn dispatch_all(&self, dispatches: Vec<PendingDispatch>) -> usize {
        let mut handles = Vec::new();
        for (dispatch, end_dispatch) in dispatches {
            handles.push(self.queue_dispatch(dispatch, end_dispatch));
        }
        let results = futures::future::join_all(handles).await;
        results
            .into_iter()
            .filter(|result| *result != Some("skipped"))
            .count()
    }

    async fn queue_dispatch(
        &self,
        dispatch: AgentCronDispatch,
        end_dispatch: Option<Box<dyn FnOnce() + Send>>,
    ) -> Option<&'static str> {
        let lane_key = dispatch.job.active_session_id.clone();
        let lane = {
            let mut lanes = self.dispatch_lanes.lock().await;
            lanes
                .entry(lane_key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        // Serialize per-session dispatches behind in-flight work for the same session.
        let _guard = lane.lock().await;
        let result = self.run_dispatch(dispatch, end_dispatch).await;
        let mut lanes = self.dispatch_lanes.lock().await;
        lanes.remove(&lane_key);
        result
    }

    async fn run_dispatch(
        &self,
        dispatch: AgentCronDispatch,
        end_dispatch: Option<Box<dyn FnOnce() + Send>>,
    ) -> Option<&'static str> {
        let outcome = async {
            let Some(job) = self.store.get_claimed_job(&dispatch.job.id) else {
                if let Err(error) = self.store.record_dispatch_result(
                    &dispatch.id,
                    &DispatchResultOptions {
                        now: Some(self.hooks.now()),
                        outcome: "skipped",
                        error: None,
                    },
                ) {
                    tracing::warn!(%error, "cron dispatch result not saved");
                }
                return Some("skipped");
            };
            let mut run_error: Option<String> = None;
            let run_result = match self.hooks.run_job(&job).await {
                Ok(result) => result,
                Err(error) => {
                    let message = error.to_string();
                    self.hooks.on_error(&job, &message);
                    run_error = Some(message);
                    None
                }
            };
            let outcome = if run_result == Some("skipped") && run_error.is_none() {
                "skipped"
            } else {
                "ran"
            };
            let failed = run_error.is_some();
            if let Err(error) = self.store.record_dispatch_result(
                &dispatch.id,
                &DispatchResultOptions {
                    now: Some(self.hooks.now()),
                    outcome,
                    error: run_error,
                },
            ) {
                tracing::warn!(%error, "cron dispatch result not saved");
            }
            if failed {
                let now = self.hooks.now();
                let streak = self.note_run_failure(&dispatch.job.id);
                if let Err(error) = self
                    .store
                    .defer_next_run(&dispatch.job.id, now + failure_backoff_ms(streak))
                {
                    tracing::warn!(%error, "cron failure backoff not saved");
                }
            } else if outcome == "ran" {
                self.clear_run_failure(&dispatch.job.id);
            }
            run_result
        }
        .await;
        if let Some(end) = end_dispatch {
            end();
        }
        outcome
    }

    fn note_run_failure(&self, job_id: &str) -> u32 {
        let mut streaks = self
            .failure_streaks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let count = streaks.entry(job_id.to_string()).or_insert(0);
        *count = count.saturating_add(1);
        *count
    }

    fn clear_run_failure(&self, job_id: &str) {
        let mut streaks = self
            .failure_streaks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        streaks.remove(job_id);
    }
}

impl<H: AgentCronSchedulerHooks + 'static> SchedulerCore<H> {
    /// Run the startup recovery and record whether it landed.
    fn recover_interrupted(&self) {
        match self.store.recover_interrupted_dispatches(self.hooks.now()) {
            Ok(recovered) => {
                self.recovery_pending.store(false, Ordering::SeqCst);
                if !recovered.is_empty() {
                    tracing::info!(
                        count = recovered.len(),
                        "cron recovery settled interrupted dispatches"
                    );
                }
            }
            Err(error) => {
                // Keep the flag: the wake timer arms a bounded retry for it.
                self.recovery_pending.store(true, Ordering::SeqCst);
                tracing::warn!(%error, "cron recovery failed at start");
            }
        }
    }
}

impl<H: AgentCronSchedulerHooks + 'static> AgentCronScheduler<H> {
    /// (Re)start the wake timer to the next active run.
    ///
    /// The task parks on the notify while no runs are active, so a mutation's wake can always
    /// re-arm it (TS `recomputeScheduledSessionWake`).
    async fn schedule_next(&self) {
        let mut timer = self.timer.lock().await;
        // A live task is never aborted: the parked loop re-evaluates on the notify, so a mid-pass
        // wake needs no respawn.
        if let Some(previous) = timer.as_ref() {
            if !previous.is_finished() {
                self.core.wake.notify_waiters();
                return;
            }
        }
        timer.take();
        let core = self.core.clone();
        let handle = tokio::spawn(async move {
            loop {
                // The wake is registered before the store read, so a notify landing between the
                // read and the wait is captured.
                let notified = core.wake.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                // A failed startup recovery is retried here, INDEPENDENTLY of the job
                // catalog: the interrupted job it repairs has no `next_run_at` left to arm
                // a wake from, so parking on the catalog alone would leave it stuck for
                // the life of the process (review #28).
                if core.recovery_pending.load(Ordering::SeqCst) {
                    core.recover_interrupted();
                    if core.recovery_pending.load(Ordering::SeqCst) {
                        if core.stopped.load(Ordering::SeqCst) {
                            return;
                        }
                        tokio::select! {
                            () = tokio::time::sleep(RECOVERY_RETRY) => {}
                            () = &mut notified => {}
                        }
                        continue;
                    }
                }
                let Some(next) = core.store.next_active_run_at() else {
                    // Nothing to fire: park until a mutation re-arms.
                    if core.stopped.load(Ordering::SeqCst) {
                        return;
                    }
                    notified.await;
                    continue;
                };
                let now = core.hooks.now();
                let delay = tokio::time::Duration::from_millis(
                    next.saturating_sub(now).clamp(1, MAX_TIMEOUT_MS),
                );
                tokio::select! {
                    () = tokio::time::sleep(delay) => {}
                    () = &mut notified => continue,
                }
                if core.stopped.load(Ordering::SeqCst) {
                    return;
                }
                core.run_due_at(core.hooks.now()).await.ok();
            }
        });

        timer.replace(handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::store::CreateAgentCronJobInput;
    use crate::cron::{AgentCronJob, ScheduleKind};
    use std::sync::atomic::AtomicUsize;

    struct CountingHooks {
        runs: Arc<AtomicUsize>,
        outcomes: Mutex<Vec<&'static str>>,
    }

    impl AgentCronSchedulerHooks for CountingHooks {
        async fn run_job(&self, _job: &AgentCronJob) -> anyhow::Result<Option<&'static str>> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            let outcome = *self.outcomes.lock().await.last().unwrap_or(&"ran");
            Ok(Some(outcome))
        }
        fn now(&self) -> u64 {
            1_700_000_000_000
        }
    }

    fn input(prompt: &str, schedule_text: &str, now: u64) -> CreateAgentCronJobInput {
        CreateAgentCronJobInput {
            active_session_id: "live-1".to_string(),
            session_id: "session-1".to_string(),
            session_file: "/w/session.jsonl".to_string(),
            cwd: "/w".to_string(),
            prompt: prompt.to_string(),
            schedule_text: schedule_text.to_string(),
            now: Some(now),
            ..Default::default()
        }
    }

    /// Review #28: a startup recovery that cannot write (a sibling process holds the
    /// state lock) must be retried, not dropped.
    ///
    /// The job it repairs is invisible to the wake timer: a DISPATCHED one-shot has no
    /// `next_run_at` left, so `next_active_run_at()` is `None` and the old task parked on
    /// the notify forever. The interrupted job stayed claimed after the lock cleared,
    /// with no error a human could see.
    #[tokio::test(start_paused = true)]
    async fn a_failed_startup_recovery_is_retried_without_a_next_run() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("jobs.json");
        let store = Arc::new(AgentCronJobStore::new(path.clone()));
        let now = 1_700_000_000_000;
        // A one-shot claimed by the previous process: dispatched, so the catalog no
        // longer schedules it.
        let job = store.create(&input("tick", "in 5m", now)).expect("create");
        let dispatches = store
            .claim_due(now + 300_000, now + 300_000)
            .expect("claim");
        assert_eq!(dispatches.len(), 1, "the job must be claimed");
        assert_eq!(
            store.next_active_run_at(),
            None,
            "a claimed one-shot leaves nothing for the wake timer to arm on"
        );

        // Hold the lock across the start: recovery cannot land.
        let held =
            crate::platform::lock_dir::LockDir::acquire(&path, std::time::Duration::from_secs(30))
                .expect("hold the state lock");
        let runs = Arc::new(AtomicUsize::new(0));
        let hooks = Arc::new(CountingHooks {
            runs: runs.clone(),
            outcomes: Mutex::new(vec!["ran"]),
        });
        let scheduler = AgentCronScheduler::new(store.clone(), hooks);
        scheduler.start().await;
        assert!(
            scheduler.core.recovery_pending.load(Ordering::SeqCst),
            "the failed recovery must stay pending"
        );
        assert_eq!(
            store.get_claimed_job(&job.id).map(|job| job.id),
            Some(job.id.clone()),
            "nothing was recovered while the lock was held"
        );

        // The lock clears. A retry must come from the timer alone: nothing else wakes the
        // scheduler, and no job carries a next run.
        drop(held);
        tokio::time::advance(RECOVERY_RETRY + std::time::Duration::from_millis(10)).await;
        for _ in 0..50 {
            if store.get_claimed_job(&job.id).is_none() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            store.get_claimed_job(&job.id).is_none(),
            "the retry must settle the interrupted dispatch"
        );
        assert!(
            !scheduler.core.recovery_pending.load(Ordering::SeqCst),
            "a landed recovery clears the flag"
        );
        assert_eq!(
            store.list()[0].status,
            crate::cron::JobStatus::Completed,
            "the interrupted one-shot is closed out, not left claimed"
        );
        scheduler.stop().await;
    }

    /// Dogfood P0 regression: a heartbeat created after the bind-time arm
    /// never fires unless the mutation's wake re-arms.
    #[tokio::test]
    async fn wake_rearms_a_timer_for_a_job_created_after_start() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(AgentCronJobStore::new(dir.path().join("jobs.json")));
        let now = 1_700_000_000_000;
        let runs = Arc::new(AtomicUsize::new(0));
        let hooks = Arc::new(CountingHooks {
            runs: runs.clone(),
            outcomes: Mutex::new(vec!["ran"]),
        });
        let scheduler = AgentCronScheduler::new(store.clone(), hooks);
        scheduler.start().await;
        // Created already-due on the fixed test clock, so the re-armed
        // timer fires within milliseconds.
        store
            .create(&input("tick", "in 1m", now - 61_000))
            .expect("create job");
        scheduler.wake().await;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while runs.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            runs.load(Ordering::SeqCst) >= 1,
            "the woken timer never fired"
        );
        scheduler.stop().await;
    }

    /// The frozen-heartbeat incident: a panicking dispatch must not wedge
    /// the run-pass flag at `true` (later passes silently no-op forever).
    #[tokio::test]
    async fn a_panicking_dispatch_does_not_wedge_the_run_pass() {
        struct PanickingHooks {
            runs: Arc<AtomicUsize>,
            panic_first: AtomicBool,
        }
        impl AgentCronSchedulerHooks for PanickingHooks {
            fn run_job(
                &self,
                _job: &AgentCronJob,
            ) -> impl std::future::Future<Output = anyhow::Result<Option<&'static str>>>
            {
                self.runs.fetch_add(1, Ordering::SeqCst);
                assert!(
                    !self.panic_first.swap(false, Ordering::SeqCst),
                    "the first dispatch unwinds"
                );
                std::future::ready(Ok(Some("ran")))
            }
            fn now(&self) -> u64 {
                1_700_000_000_000
            }
        }
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(AgentCronJobStore::new(dir.path().join("jobs.json")));
        let now = 1_700_000_000_000;
        // Created 10m ago on the fixed test clock: due immediately (at
        // `now` it would sit 10m out).
        store
            .create(&input("tick", "every 10m", now - 600_000))
            .unwrap();
        let hooks = Arc::new(PanickingHooks {
            runs: Arc::new(AtomicUsize::new(0)),
            panic_first: AtomicBool::new(true),
        });
        // No start(): the manual passes below drive `run_due` directly;
        // a started timer would race them for the same due job.
        let scheduler = Arc::new(AgentCronScheduler::new(store.clone(), hooks.clone()));
        let first = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::spawn({
                let scheduler = Arc::clone(&scheduler);
                async move { scheduler.run_due().await }
            }),
        )
        .await
        // The timeout layer unwraps; the panic surfaces as the join error.
        .expect("first pass settles");
        assert!(first.is_err(), "the panicking pass surfaced: {first:?}");
        store
            .create(&input("tock", "every 10m", now - 600_000))
            .expect("second job");
        let ran = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::spawn({
                let scheduler = Arc::clone(&scheduler);
                async move { scheduler.run_due().await }
            }),
        )
        .await
        .expect("second pass settles")
        .expect("second pass joined")
        .expect("second pass ran");
        assert!(ran > 0, "the wedged flag skipped the second pass: {ran}");
        scheduler.stop().await;
    }

    /// The live incident: a wake landing while a fire pass is in-flight must
    /// not strand the claimed dispatch.
    #[tokio::test]
    async fn a_mutation_wake_during_an_in_flight_pass_does_not_wedge_the_flag() {
        struct BlockingHooks {
            runs: Arc<AtomicUsize>,
            block_first: AtomicBool,
        }
        impl AgentCronSchedulerHooks for BlockingHooks {
            async fn run_job(&self, _job: &AgentCronJob) -> anyhow::Result<Option<&'static str>> {
                self.runs.fetch_add(1, Ordering::SeqCst);
                if self.block_first.swap(false, Ordering::SeqCst) {
                    // Hold the first delivery in-flight so the
                    // mutation's wake lands mid-pass.
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
                Ok(Some("ran"))
            }
            fn now(&self) -> u64 {
                1_700_000_000_000
            }
        }
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(AgentCronJobStore::new(dir.path().join("jobs.json")));
        let now = 1_700_000_000_000;
        let hooks = Arc::new(BlockingHooks {
            runs: Arc::new(AtomicUsize::new(0)),
            block_first: AtomicBool::new(true),
        });
        let scheduler = AgentCronScheduler::new(store.clone(), hooks.clone());
        store
            .create(&input("tick", "in 1m", now - 61_000))
            .expect("first job");
        scheduler.start().await;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while hooks.runs.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(
            hooks.runs.load(Ordering::SeqCst),
            1,
            "the first fire started"
        );
        store
            .create(&input("tock", "in 1m", now - 61_000))
            .expect("second job");
        scheduler.wake().await;
        scheduler.wake().await;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while hooks.runs.load(Ordering::SeqCst) < 2 && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            hooks.runs.load(Ordering::SeqCst) >= 2,
            "a wedged flag silently skipped every later pass (runs: {})",
            hooks.runs.load(Ordering::SeqCst)
        );
        scheduler.stop().await;
    }

    /// The store emptying mid-life must leave a parked timer a later
    /// wake re-arms (the mid-life death behind a re-adopted worker).
    #[tokio::test]
    async fn the_timer_parks_on_an_empty_store_and_re_arms_on_wake() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(AgentCronJobStore::new(dir.path().join("jobs.json")));
        let now = 1_700_000_000_000;
        let runs = Arc::new(AtomicUsize::new(0));
        let hooks = Arc::new(CountingHooks {
            runs: runs.clone(),
            outcomes: Mutex::new(vec!["ran"]),
        });
        let scheduler = AgentCronScheduler::new(store.clone(), hooks);
        scheduler.start().await;
        let job = store
            .create(&input("tick", "every 10m", now))
            .expect("first job");
        store
            .cancel(&job.id, now)
            .unwrap()
            .expect("cancel the only job");
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
        while std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        store
            .create(&input("tock", "in 1m", now - 61_000))
            .expect("later job");
        scheduler.wake().await;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while runs.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            runs.load(Ordering::SeqCst) >= 1,
            "the parked timer never fired the later job"
        );
        scheduler.stop().await;
    }

    #[tokio::test]
    async fn claims_and_runs_due_jobs() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(AgentCronJobStore::new(dir.path().join("jobs.json")));
        let now = 1_700_000_000_000;
        store.create(&input("tick", "every 10m", now)).unwrap();
        let runs = Arc::new(AtomicUsize::new(0));
        let hooks = Arc::new(CountingHooks {
            runs: runs.clone(),
            outcomes: Mutex::new(vec!["ran"]),
        });
        let scheduler = AgentCronScheduler::new(store.clone(), hooks);
        scheduler.start().await;
        let ran = scheduler.run_due().await.unwrap();
        assert_eq!(ran, 0);
        assert_eq!(runs.load(Ordering::SeqCst), 0);
        store
            .create(&input("tick2", "in 1m", now - 61_000))
            .unwrap();
        let ran = scheduler.run_due().await.unwrap();
        assert!(ran >= 1);
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        scheduler.stop().await;
    }

    #[tokio::test]
    async fn skip_outcomes_are_recorded() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(AgentCronJobStore::new(dir.path().join("jobs.json")));
        let now = 1_700_000_000_000;
        store.create(&input("tick", "in 1m", now - 60_000)).unwrap();
        let runs = Arc::new(AtomicUsize::new(0));
        let hooks = Arc::new(CountingHooks {
            runs: runs.clone(),
            outcomes: Mutex::new(vec!["skipped"]),
        });
        let scheduler = AgentCronScheduler::new(store.clone(), hooks);
        scheduler.start().await;
        let ran = scheduler.run_due().await.unwrap();
        // The skip still claimed the job, so zero runs are counted.
        assert_eq!(ran, 0);
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        let jobs = store.list();
        assert_eq!(jobs[0].status, crate::cron::JobStatus::Completed);
        assert!(jobs[0].last_skipped_at.is_some());
        scheduler.stop().await;
    }

    #[tokio::test]
    async fn lane_serializes_same_session_dispatches() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(AgentCronJobStore::new(dir.path().join("jobs.json")));
        let now = 1_700_000_000_000;
        store.create(&input("a", "in 1m", now - 60_000)).unwrap();
        store.create(&input("b", "in 1m", now - 60_000)).unwrap();
        let runs = Arc::new(AtomicUsize::new(0));
        let hooks = Arc::new(CountingHooks {
            runs: runs.clone(),
            outcomes: Mutex::new(vec!["ran"]),
        });
        let scheduler = AgentCronScheduler::new(store, hooks);
        scheduler.start().await;
        let ran = scheduler.run_due().await.unwrap();
        assert_eq!(ran, 2);
        assert_eq!(runs.load(Ordering::SeqCst), 2);
        scheduler.stop().await;
    }

    #[tokio::test]
    async fn schedule_kind_from_create_is_interval() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = AgentCronJobStore::new(dir.path().join("jobs.json"));
        let now = 1_700_000_000_000;
        let job = store.create(&input("tick", "every 10m", now)).unwrap();
        assert_eq!(job.schedule.kind, ScheduleKind::Interval);
    }

    #[test]
    fn failure_backoff_doubles_then_caps() {
        assert_eq!(failure_backoff_ms(1), 120_000);
        assert_eq!(failure_backoff_ms(2), 240_000);
        assert_eq!(failure_backoff_ms(3), 480_000);
        assert_eq!(failure_backoff_ms(5), 1_920_000);
        assert_eq!(failure_backoff_ms(6), 3_600_000);
        assert_eq!(failure_backoff_ms(60), 3_600_000);
    }

    /// Dogfood incident: a dead route re-fired an every-2m heartbeat
    /// ~120 times.
    #[tokio::test]
    async fn consecutive_failures_back_off_the_next_run() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(AgentCronJobStore::new(dir.path().join("jobs.json")));
        let start = 1_700_000_000_000;
        let job = store.create(&input("tick", "every 1m", start)).unwrap();
        let job_id = job.id.clone();
        // First due moment: 60s after creation.
        let t1 = start + 61_000;
        let hooks = Arc::new(FailingHooks {
            runs: Arc::new(AtomicUsize::new(0)),
            error: std::sync::Mutex::new(Some("model route gone".to_string())),
            now: std::sync::Mutex::new(t1),
        });
        let scheduler = AgentCronScheduler::new(store.clone(), hooks.clone());
        // No `start()`: the timer would race the explicit `run_due` calls below.
        // Failure 1: the schedule rolls to t1 + 60s, the backoff raises it to t1 + 120s.
        assert_eq!(scheduler.run_due().await.unwrap(), 1);
        let job = store
            .list()
            .into_iter()
            .find(|job| job.id == job_id)
            .expect("job kept");
        assert_eq!(job.last_error.as_deref(), Some("model route gone"));
        assert_eq!(job.run_count, 1);
        assert_eq!(
            crate::cron::parse_iso_millis(job.next_run_at.as_deref().unwrap()),
            Some(t1 + failure_backoff_ms(1))
        );
        // Failure 2 (clock advanced past the deferred run): the pause doubles.
        let t2 = t1 + failure_backoff_ms(1) + 1;
        *hooks.now.lock().unwrap() = t2;
        assert_eq!(scheduler.run_due().await.unwrap(), 1);
        let job = store
            .list()
            .into_iter()
            .find(|job| job.id == job_id)
            .expect("job kept");
        assert_eq!(
            crate::cron::parse_iso_millis(job.next_run_at.as_deref().unwrap()),
            Some(t2 + failure_backoff_ms(2))
        );
        // A good run clears the streak: the next run goes back to the bare schedule.
        let t3 = t2 + failure_backoff_ms(2) + 1;
        *hooks.now.lock().unwrap() = t3;
        *hooks.error.lock().unwrap() = None;
        assert_eq!(scheduler.run_due().await.unwrap(), 1);
        let job = store
            .list()
            .into_iter()
            .find(|job| job.id == job_id)
            .expect("job kept");
        assert_eq!(job.last_error, None);
        assert_eq!(
            crate::cron::parse_iso_millis(job.next_run_at.as_deref().unwrap()),
            Some(t3 + 60_000)
        );
        // The failure after it restarts at the base pause.
        let t4 = t3 + 60_000 + 1;
        *hooks.now.lock().unwrap() = t4;
        *hooks.error.lock().unwrap() = Some("model route gone".to_string());
        assert_eq!(scheduler.run_due().await.unwrap(), 1);
        let job = store
            .list()
            .into_iter()
            .find(|job| job.id == job_id)
            .expect("job kept");
        assert_eq!(
            crate::cron::parse_iso_millis(job.next_run_at.as_deref().unwrap()),
            Some(t4 + failure_backoff_ms(1))
        );
    }

    /// Hooks whose runs can fail on demand, over a controllable clock.
    struct FailingHooks {
        runs: Arc<AtomicUsize>,
        error: std::sync::Mutex<Option<String>>,
        now: std::sync::Mutex<u64>,
    }

    impl AgentCronSchedulerHooks for FailingHooks {
        fn run_job(
            &self,
            _job: &AgentCronJob,
        ) -> impl std::future::Future<Output = anyhow::Result<Option<&'static str>>> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            let error = self.error.lock().unwrap().clone();
            std::future::ready(match error {
                Some(message) => Err(anyhow::anyhow!(message)),
                None => Ok(None),
            })
        }
        fn now(&self) -> u64 {
            *self.now.lock().unwrap()
        }
    }
}
