//! FORK-ONLY: the tray's cache-rate refresh. The number is a decoration on a
//! visible row, so it must never be awaited on the attach or settle path — the
//! repo's paint-path rule forbids holding a frame for it. Instead it refreshes
//! in the background with the factory/heartbeat serialization: one request in
//! flight, one queued trailing refresh, and a session epoch so a stale reply can
//! never repaint a newer session's tray.
use super::{AgentView, DaemonCommand, Duration, Map, SessionUi, UI_REQUEST_TIMEOUT_MS};

/// What a landed refresh says about the tray's rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheRateOutcome {
    /// The fold resolved: render this rate (including a real `Some(0)`).
    Rate(Option<u64>),
    /// The fetch failed or the fold was unresolved: keep the rate already on
    /// screen rather than blank a number the user is reading.
    Unchanged,
}

/// What [`CacheRateUpdate::paint`] decided the tray should show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaintCacheRate {
    /// Render this rate (`None` hides the segment: no cache traffic at all).
    Rate(Option<u64>),
    /// Leave the painted rate alone (a stale reply, or a fetch that failed).
    LeaveTray,
}

/// A landed cache-rate refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CacheRateUpdate {
    /// The session the request addressed: a reply from a superseded session
    /// never lands on the current tray.
    pub session: String,
    /// The refresh epoch it was issued under.
    pub epoch: u64,
    pub outcome: CacheRateOutcome,
}

impl CacheRateUpdate {
    /// The rate this reply may paint, or `LeaveTray` when it must not touch it. A
    /// reply from a superseded session, or one issued under an older epoch than the
    /// session has issued, is stale: the tray already moved past it. An `Unchanged`
    /// outcome (a failed or unresolved fetch) always keeps the painted rate, so a
    /// transient failure never blanks a number the user is reading.
    fn paint(&self, session: &str, epoch: u64) -> PaintCacheRate {
        if self.session != session || self.epoch < epoch {
            return PaintCacheRate::LeaveTray;
        }
        match self.outcome {
            CacheRateOutcome::Rate(rate) => PaintCacheRate::Rate(rate),
            CacheRateOutcome::Unchanged => PaintCacheRate::LeaveTray,
        }
    }
}

impl SessionUi {
    /// Fire a background cache-rate refresh. Called at attach (every fold but a
    /// held one, which keeps its already-painted rate) and after a settled turn;
    /// both callers are on the paint path, so neither awaits the round-trip.
    pub(crate) fn spawn_cache_rate_refresh(&mut self) {
        if self.cache_rate_in_flight {
            self.cache_rate_queued = true;
            return;
        }
        self.cache_rate_in_flight = true;
        self.cache_rate_epoch += 1;
        let epoch = self.cache_rate_epoch;
        let client = self.client.clone();
        let session = self.active_session_id.clone();
        let tx = self.cache_rate_updates.clone();
        tokio::spawn(async move {
            let request = DaemonCommand::GetCacheStats {
                id: None,
                active_session_id: session.clone(),
                rest: Map::default(),
            };
            let fetched = tokio::time::timeout(
                Duration::from_millis(UI_REQUEST_TIMEOUT_MS),
                client.request_ok(request),
            )
            .await;
            let outcome = match fetched {
                // `get_cache_stats` answers the same payload `/cache` renders; the
                // panel's own fold gives the tray its rate.
                Ok(Ok(tree)) => {
                    CacheRateOutcome::Rate(crate::info_commands::cache_totals(&tree).hit_percent)
                }
                Ok(Err(_)) | Err(_) => CacheRateOutcome::Unchanged,
            };
            let _ = tx.send(CacheRateUpdate {
                session,
                epoch,
                outcome,
            });
        });
    }

    /// Fold a landed refresh: free the slot, drop a stale reply, and hand a
    /// resolved rate to the chrome. `Unchanged` leaves the painted rate alone.
    pub(crate) fn apply_cache_rate(&mut self, update: &CacheRateUpdate, view: &mut AgentView) {
        self.cache_rate_in_flight = false;
        let queued = std::mem::take(&mut self.cache_rate_queued);
        if let PaintCacheRate::Rate(hit_percent) =
            update.paint(&self.active_session_id, self.cache_rate_epoch)
        {
            self.cache_hit_percent = hit_percent;
            view.chrome.cache_hit_percent = hit_percent;
            self.dirty = true;
        }
        if queued {
            self.spawn_cache_rate_refresh();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(session: &str, epoch: u64, outcome: CacheRateOutcome) -> CacheRateUpdate {
        CacheRateUpdate {
            session: session.to_string(),
            epoch,
            outcome,
        }
    }

    /// A landed rate paints; a stale epoch, a foreign session, and a failed fetch all
    /// leave the painted number alone.
    #[test]
    fn only_a_fresh_resolved_reply_paints_the_tray() {
        assert_eq!(
            update("s1", 3, CacheRateOutcome::Rate(Some(91))).paint("s1", 3),
            PaintCacheRate::Rate(Some(91)),
            "the session's own freshest reply paints"
        );
        // `Some(0)` is a real measurement (reads exist, no hits) and must paint.
        assert_eq!(
            update("s1", 3, CacheRateOutcome::Rate(Some(0))).paint("s1", 3),
            PaintCacheRate::Rate(Some(0))
        );
        // A resolved empty fold HIDES the segment rather than rendering `0%`.
        assert_eq!(
            update("s1", 3, CacheRateOutcome::Rate(None)).paint("s1", 3),
            PaintCacheRate::Rate(None)
        );
        // An older epoch: the tray already moved past that request.
        assert_eq!(
            update("s1", 2, CacheRateOutcome::Rate(Some(12))).paint("s1", 3),
            PaintCacheRate::LeaveTray
        );
        // A superseded session's reply never repaints this tray.
        assert_eq!(
            update("s2", 3, CacheRateOutcome::Rate(Some(12))).paint("s1", 3),
            PaintCacheRate::LeaveTray
        );
        // A failed or unresolved fetch keeps the number the user is reading.
        assert_eq!(
            update("s1", 3, CacheRateOutcome::Unchanged).paint("s1", 3),
            PaintCacheRate::LeaveTray
        );
    }
}
