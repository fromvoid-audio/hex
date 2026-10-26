//! Feature-only observation of final control-source teardown. The registry owns
//! thread identities and notifications, never source owners or weak aliases.
use std::sync::{Mutex, mpsc};
use std::thread::{self, ThreadId};

static OBSERVERS: Mutex<Vec<(ThreadId, mpsc::SyncSender<ThreadId>)>> = Mutex::new(Vec::new());

pub struct Observer(ThreadId);
pub fn observe(creator: ThreadId) -> (Observer, mpsc::Receiver<ThreadId>) {
    let (sender, receiver) = mpsc::sync_channel(16);
    if let Ok(mut observers) = OBSERVERS.lock() {
        observers.push((creator, sender));
    }
    (Observer(creator), receiver)
}
impl Drop for Observer {
    fn drop(&mut self) {
        if let Ok(mut observers) = OBSERVERS.lock() {
            observers.retain(|(creator, _)| *creator != self.0);
        }
    }
}

/// Last field of the source payload, so bank and catalog have already dropped.
pub(super) struct SourceDropped(ThreadId);
impl SourceDropped {
    pub(super) fn new() -> Self {
        Self(thread::current().id())
    }
}
impl Drop for SourceDropped {
    fn drop(&mut self) {
        if let Ok(observers) = OBSERVERS.lock() {
            for (_, sender) in observers.iter().filter(|(creator, _)| *creator == self.0) {
                let _ = sender.try_send(thread::current().id());
            }
        }
    }
}
