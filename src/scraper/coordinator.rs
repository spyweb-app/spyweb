use crate::config::types::MAX_BATCH;
use smol::channel::Sender;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

// Worker→coordinator events. One shared unbounded stream: each send is an atomic append.
pub(crate) enum Message {
    RequestWork { worker_id: usize },
    Push { url: String },
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PushError {
    Inactive,
    Full,
}

// Per-job queue-cycle state shared between the `queue_push` binding (sync,
// worker-side) and the coordinator (async, single consumer). The only
// worker-visible fields are the two atomics, everything else is owned by
// the coordinator task.
pub(crate) struct QueueState {
    slot: Mutex<Option<Sender<Message>>>,
    active: AtomicBool,
    reserved: AtomicUsize,
}

impl QueueState {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            slot: Mutex::new(None),
            active: AtomicBool::new(false),
            reserved: AtomicUsize::new(0),
        })
    }

    // Clear all cycle state. Called at every cycle start (`run_iteration`,
    // covers all modes) and when the coordinator ends — reset-at-start makes
    // cancellation (dropped cycle future) safe with no cleanup path.
    pub(crate) fn deactivate(&self) {
        self.active.store(false, Ordering::SeqCst);
        self.slot.lock().unwrap_or_else(|e| e.into_inner()).take();
        self.reserved.store(0, Ordering::SeqCst);
    }

    pub(crate) fn begin_cycle(&self, tx: Sender<Message>, initial: usize) {
        self.reserved.store(initial, Ordering::SeqCst);
        *self.slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
        self.active.store(true, Ordering::SeqCst);
    }

    pub(crate) fn release_reservation(&self) {
        self.reserved.fetch_sub(1, Ordering::SeqCst);
    }

    // `queue_push` binding entry: sync gate + capacity reservation + send.
    // Never blocks; all shared state touched here is atomic or a short-lived
    // mutex on a clone-only slot.
    pub(crate) fn push_url(&self, url: String) -> Result<(), PushError> {
        if !self.active.load(Ordering::SeqCst) {
            return Err(PushError::Inactive);
        }
        let prev = self.reserved.fetch_add(1, Ordering::SeqCst);
        if prev >= MAX_BATCH {
            self.reserved.fetch_sub(1, Ordering::SeqCst);
            return Err(PushError::Full);
        }
        let tx = self.slot.lock().unwrap_or_else(|e| e.into_inner()).clone();
        match tx {
            Some(tx) if tx.try_send(Message::Push { url }).is_ok() => Ok(()),
            _ => {
                self.reserved.fetch_sub(1, Ordering::SeqCst);
                Err(PushError::Inactive)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smol::channel::unbounded;

    #[test]
    fn push_url_rejected_outside_active_cycle() {
        let state = QueueState::new();
        assert_eq!(
            state.push_url("u".into()),
            Err(PushError::Inactive),
            "gate must reject before any cycle starts"
        );
    }

    #[test]
    fn push_url_rejected_after_deactivate() {
        let state = QueueState::new();
        let (tx, _rx) = unbounded::<Message>();
        state.begin_cycle(tx, 1);
        assert_eq!(state.push_url("u".into()), Ok(()));
        state.deactivate();
        assert_eq!(state.push_url("u".into()), Err(PushError::Inactive));
    }

    #[test]
    fn push_url_rejects_over_cap_with_rollback() {
        let state = QueueState::new();
        let (tx, _rx) = unbounded::<Message>();
        state.begin_cycle(tx, MAX_BATCH);
        assert_eq!(state.push_url("u".into()), Err(PushError::Full));
        // Reservation must roll back — a later accept must still fit.
        state.release_reservation();
        assert_eq!(state.push_url("u2".into()), Ok(()));
    }

    #[test]
    fn begin_cycle_resets_previous_state() {
        let state = QueueState::new();
        let (tx1, _rx1) = unbounded::<Message>();
        state.begin_cycle(tx1, 5);
        assert_eq!(state.push_url("u".into()), Ok(()));
        // Next cycle starts fresh: old pushes are gone, budget reset.
        let (tx2, rx2) = unbounded::<Message>();
        state.begin_cycle(tx2, 2);
        assert!(rx2.try_recv().is_err(), "old channel must be dropped");
        assert_eq!(state.reserved.load(Ordering::SeqCst), 2);
    }
}
