// Runs `Collector::collect()` on a single dedicated background thread for
// the whole app's lifetime, since it can block for a while: on macOS it
// shells out to `lsappinfo` (see app_coalitions.rs) to group processes,
// and that call can queue up behind heavy disk I/O — e.g. the Storage
// tab's full-disk scan running at the same time. Doing this on the main
// thread froze the whole UI (menu bar clicks, window opens) for however
// long that queued subprocess took.
//
// The rest of the app asks for a fresh snapshot by sending `()` on
// `request_tx` and polls `try_recv_snapshot()` from a short NSTimer,
// mirroring the pattern already used for the Storage tab's scan
// (background thread + channel + main-thread timer, since AppKit calls
// aren't thread-safe and can't happen off the main thread anyway).

use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use system_monitor::{Collector, Snapshot};

pub struct CollectorWorker {
    request_tx: Sender<()>,
    snapshot_rx: Receiver<Snapshot>,
}

impl CollectorWorker {
    pub fn spawn() -> Self {
        let (request_tx, request_rx) = std::sync::mpsc::channel::<()>();
        let (snapshot_tx, snapshot_rx) = std::sync::mpsc::channel::<Snapshot>();

        std::thread::spawn(move || {
            let mut collector = Collector::new();
            while request_rx.recv().is_ok() {
                // Drain any requests that piled up while we were busy
                // collecting, so a burst of ticks collapses into one
                // fresh collect() rather than queuing up stale work.
                while request_rx.try_recv().is_ok() {}
                let snapshot = collector.collect();
                if snapshot_tx.send(snapshot).is_err() {
                    break;
                }
            }
        });

        Self {
            request_tx,
            snapshot_rx,
        }
    }

    // Fire-and-forget: asks the worker thread to produce a fresh
    // snapshot. Safe to call even while a previous request is still in
    // flight (see the drain loop above).
    pub fn request_snapshot(&self) {
        let _ = self.request_tx.send(());
    }

    // Non-blocking: returns the most recent snapshot if one has arrived
    // since the last poll, or None if the worker hasn't finished yet.
    pub fn try_recv_snapshot(&self) -> Option<Snapshot> {
        match self.snapshot_rx.try_recv() {
            Ok(snapshot) => Some(snapshot),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => None,
        }
    }
}
