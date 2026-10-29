//! Bounded control-side messages for the desktop-owned local update worker.
//! The view never owns installer files or a helper process.
use gpui::{App, Global};
use std::sync::{
    Arc, Mutex,
    mpsc::{self, Receiver, SyncSender},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateAction {
    InstallOnQuit,
    SaveAndRestart,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateStatus {
    Idle,
    Available,
    PreviousResult { message: &'static str, failed: bool },
    Selected(u64),
    Preparing(u64),
    Ready(u64),
    Failed(u64, String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateCommand {
    Select(u64),
    DismissResult,
    Prepare { id: u64, action: UpdateAction },
    Cancel(u64),
}

/// One verified helper, consumed only by the same editor action that closes
/// the window. Dropping it without approval refuses publication.
pub trait UpdateApproval: Send {
    fn approve(self: Box<Self>) -> Result<(), String>;
}

pub struct UpdateReady {
    pub id: u64,
    pub approval: Box<dyn UpdateApproval>,
}

#[derive(Clone)]
pub struct UpdateClient {
    commands: SyncSender<UpdateCommand>,
    state: Arc<Mutex<UpdateStatus>>,
    ready: Arc<Mutex<Option<UpdateReady>>>,
}
impl Global for UpdateClient {}

impl UpdateClient {
    pub fn send(&self, command: UpdateCommand) -> Result<(), &'static str> {
        self.commands
            .try_send(command)
            .map_err(|_| "update worker is unavailable")
    }
    pub fn status(&self) -> UpdateStatus {
        self.state
            .lock()
            .map(|state| state.clone())
            .unwrap_or_else(|_| UpdateStatus::Failed(0, "update worker is unavailable".into()))
    }
    pub fn take_ready(&self) -> Option<UpdateReady> {
        self.ready.lock().ok()?.take()
    }
}

pub struct UpdateWorker {
    pub commands: Receiver<UpdateCommand>,
    state: Arc<Mutex<UpdateStatus>>,
    ready: Arc<Mutex<Option<UpdateReady>>>,
}
impl UpdateWorker {
    pub fn publish(&self, status: UpdateStatus) {
        if let Ok(mut state) = self.state.lock() {
            *state = status;
        }
    }
    pub fn deliver(&self, ready: UpdateReady) -> Result<(), UpdateReady> {
        match self.ready.lock() {
            Ok(mut slot) => {
                *slot = Some(ready);
                Ok(())
            }
            Err(_) => Err(ready),
        }
    }
    pub fn revoke(&self, id: u64) {
        if let Ok(mut slot) = self.ready.lock()
            && slot.as_ref().is_some_and(|ready| ready.id == id)
        {
            slot.take();
        }
    }
}

pub fn channel() -> (UpdateClient, UpdateWorker) {
    let (commands, receiver) = mpsc::sync_channel(4);
    let state = Arc::new(Mutex::new(UpdateStatus::Idle));
    let ready = Arc::new(Mutex::new(None));
    (
        UpdateClient {
            commands,
            state: state.clone(),
            ready: ready.clone(),
        },
        UpdateWorker {
            commands: receiver,
            state,
            ready,
        },
    )
}

pub fn install_update_client(cx: &mut App, client: UpdateClient) {
    cx.set_global(client);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Approval(Arc<AtomicUsize>);
    impl Drop for Approval {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    impl UpdateApproval for Approval {
        fn approve(self: Box<Self>) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn cancelled_ready_cannot_block_or_replace_a_new_request() {
        let (client, worker) = channel();
        let dropped = Arc::new(AtomicUsize::new(0));
        worker
            .deliver(UpdateReady {
                id: 1,
                approval: Box::new(Approval(dropped.clone())),
            })
            .unwrap_or_else(|_| panic!("first delivery"));
        // A newer ready may arrive before the UI has drained the old result.
        worker
            .deliver(UpdateReady {
                id: 2,
                approval: Box::new(Approval(dropped.clone())),
            })
            .unwrap_or_else(|_| panic!("second delivery"));
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        worker.revoke(1);
        let ready = client.take_ready().unwrap();
        assert_eq!(ready.id, 2);
        drop(ready);
        assert_eq!(dropped.load(Ordering::SeqCst), 2);
    }
}
