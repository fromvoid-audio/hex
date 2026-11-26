//! Bounded control-side messages for the desktop-owned issue report worker.
//! The view never reads session logs, holds intake tokens, or performs network I/O.

use gpui::{App, Global};
use hex_report::{ReportKind, RuntimeSnapshot};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender},
    },
};

/// One user-initiated issue/request submission dispatched off the UI thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportRequest {
    pub id: u64,
    pub kind: ReportKind,
    pub user_summary: String,
    pub include_diagnostics: bool,
    pub snapshot: RuntimeSnapshot,
    pub active_log: Option<PathBuf>,
}

/// Request to build a redacted preview off-thread using the frozen `T_report`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportPreviewRequest {
    pub id: u64,
    pub kind: ReportKind,
    pub user_summary: String,
    pub include_diagnostics: bool,
    pub snapshot: RuntimeSnapshot,
    pub active_log: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReportCommand {
    Submit(ReportRequest),
    Preview(ReportPreviewRequest),
    FlushOutbox,
    DismissStatus,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ReportStatus {
    #[default]
    Idle,
    Submitting(u64),
    Submitted {
        id: u64,
        report_id: u64,
        deduplicated: bool,
    },
    QueuedOffline {
        id: u64,
        reason: String,
    },
    Failed {
        id: u64,
        error: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportPreview {
    pub id: u64,
    pub text: String,
}

#[derive(Clone)]
pub struct ReportClient {
    commands: SyncSender<ReportCommand>,
    status: Arc<Mutex<ReportStatus>>,
    preview: Arc<Mutex<Option<ReportPreview>>>,
}

impl Global for ReportClient {}

impl ReportClient {
    pub fn send(&self, command: ReportCommand) -> Result<(), &'static str> {
        self.commands
            .try_send(command)
            .map_err(|_| "report worker is unavailable")
    }

    pub fn status(&self) -> ReportStatus {
        self.status
            .lock()
            .map(|state| state.clone())
            .unwrap_or_else(|_| ReportStatus::Failed {
                id: 0,
                error: "report worker is unavailable".into(),
            })
    }

    pub fn take_preview(&self) -> Option<ReportPreview> {
        self.preview.lock().ok()?.take()
    }
}

pub struct ReportWorker {
    pub commands: Receiver<ReportCommand>,
    status: Arc<Mutex<ReportStatus>>,
    preview: Arc<Mutex<Option<ReportPreview>>>,
}

impl ReportWorker {
    pub fn publish(&self, status: ReportStatus) {
        if let Ok(mut state) = self.status.lock() {
            *state = status;
        }
    }

    pub fn deliver_preview(&self, preview: ReportPreview) {
        if let Ok(mut slot) = self.preview.lock() {
            *slot = Some(preview);
        }
    }
}

pub fn channel() -> (ReportClient, ReportWorker) {
    let (commands, receiver) = mpsc::sync_channel(8);
    let status = Arc::new(Mutex::new(ReportStatus::Idle));
    let preview = Arc::new(Mutex::new(None));
    (
        ReportClient {
            commands,
            status: Arc::clone(&status),
            preview: Arc::clone(&preview),
        },
        ReportWorker {
            commands: receiver,
            status,
            preview,
        },
    )
}

pub fn install_report_client(cx: &mut App, client: ReportClient) {
    cx.set_global(client);
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::*;

    #[test]
    fn report_channel_hands_off_commands_status_and_latest_preview() {
        let (client, worker) = channel();
        assert_eq!(client.status(), ReportStatus::Idle);
        assert!(client.take_preview().is_none());

        let req = ReportRequest {
            id: 1,
            kind: ReportKind::Bug,
            user_summary: "test issue".into(),
            include_diagnostics: true,
            snapshot: RuntimeSnapshot::default(),
            active_log: None,
        };
        client.send(ReportCommand::Submit(req.clone())).unwrap();
        assert_eq!(worker.commands.recv().unwrap(), ReportCommand::Submit(req));

        worker.publish(ReportStatus::Submitting(1));
        assert_eq!(client.status(), ReportStatus::Submitting(1));

        worker.deliver_preview(ReportPreview {
            id: 1,
            text: "first".into(),
        });
        worker.deliver_preview(ReportPreview {
            id: 2,
            text: "second".into(),
        });
        assert_eq!(
            client.take_preview(),
            Some(ReportPreview {
                id: 2,
                text: "second".into()
            })
        );
        assert!(client.take_preview().is_none());

        worker.publish(ReportStatus::Submitted {
            id: 1,
            report_id: 99,
            deduplicated: false,
        });
        assert_eq!(
            client.status(),
            ReportStatus::Submitted {
                id: 1,
                report_id: 99,
                deduplicated: false
            }
        );
    }
}
