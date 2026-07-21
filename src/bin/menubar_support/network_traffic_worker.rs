// Runs `network::collect_system_traffic()` (shells out to `nettop`) on a
// dedicated background thread — nettop always takes ~5s per call
// regardless of the requested sample count, so calling it from the
// Network window's main-thread timer would freeze the whole UI for that
// long every tick. Mirrors CollectorWorker's request/channel/poll pattern.

use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use system_monitor::network::SystemTraffic;

pub struct NetworkTrafficWorker {
    request_tx: Sender<()>,
    traffic_rx: Receiver<SystemTraffic>,
}

impl NetworkTrafficWorker {
    pub fn spawn() -> Self {
        let (request_tx, request_rx) = std::sync::mpsc::channel::<()>();
        let (traffic_tx, traffic_rx) = std::sync::mpsc::channel::<SystemTraffic>();

        std::thread::spawn(move || {
            while request_rx.recv().is_ok() {
                while request_rx.try_recv().is_ok() {}
                let Some(traffic) = system_monitor::network::collect_system_traffic() else {
                    continue;
                };
                if traffic_tx.send(traffic).is_err() {
                    break;
                }
            }
        });

        Self {
            request_tx,
            traffic_rx,
        }
    }

    pub fn request_traffic(&self) {
        let _ = self.request_tx.send(());
    }

    pub fn try_recv_traffic(&self) -> Option<SystemTraffic> {
        match self.traffic_rx.try_recv() {
            Ok(traffic) => Some(traffic),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => None,
        }
    }
}
