//! The store worker thread (PRD §7.2): all SQLite writes are serialized here.
//! The UI applies each transaction optimistically to its in-memory graph and
//! sends it here; failures come back as [`Event::Failed`] so the UI can revert.

use crate::Store;
use soma_core::Tx;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

pub enum Request {
    Commit(Tx),
    Undo,
    Redo,
}

pub enum Event {
    Committed {
        label: String,
    },
    /// The tx did not persist; the UI must apply `tx.inverse()`.
    Failed {
        tx: Tx,
        error: String,
    },
    /// Apply this tx to the in-memory graph.
    Undone(Tx),
    Redone(Tx),
    NothingToUndo,
    NothingToRedo,
    Error(String),
}

pub struct StoreWorker {
    tx: Option<Sender<Request>>,
    rx: Receiver<Event>,
    handle: Option<JoinHandle<()>>,
}

impl StoreWorker {
    /// `wake` is called after every event (e.g. to request a UI repaint).
    pub fn spawn(mut store: Store, wake: impl Fn() + Send + 'static) -> Self {
        let (req_tx, req_rx) = mpsc::channel::<Request>();
        let (ev_tx, ev_rx) = mpsc::channel::<Event>();
        let handle = std::thread::Builder::new()
            .name("soma-store".into())
            .spawn(move || {
                for req in req_rx {
                    let ev = match req {
                        Request::Commit(tx) => match store.commit(&tx) {
                            Ok(()) => Event::Committed { label: tx.label },
                            Err(e) => Event::Failed { tx, error: e.to_string() },
                        },
                        Request::Undo => match store.undo() {
                            Ok(Some(tx)) => Event::Undone(tx),
                            Ok(None) => Event::NothingToUndo,
                            Err(e) => Event::Error(e.to_string()),
                        },
                        Request::Redo => match store.redo() {
                            Ok(Some(tx)) => Event::Redone(tx),
                            Ok(None) => Event::NothingToRedo,
                            Err(e) => Event::Error(e.to_string()),
                        },
                    };
                    if ev_tx.send(ev).is_err() {
                        break;
                    }
                    wake();
                }
            })
            .expect("spawn store worker");
        Self { tx: Some(req_tx), rx: ev_rx, handle: Some(handle) }
    }

    pub fn send(&self, req: Request) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(req);
        }
    }

    pub fn poll(&self) -> impl Iterator<Item = Event> + '_ {
        self.rx.try_iter()
    }
}

impl Drop for StoreWorker {
    /// Flush: close the queue and wait for pending writes.
    fn drop(&mut self) {
        self.tx.take();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
