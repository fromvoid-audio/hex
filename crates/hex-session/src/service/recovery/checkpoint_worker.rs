//! One control-owned publication worker. No journal is shared between threads.
use super::PreparedCheckpoint;
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, SyncSender, TrySendError},
    thread::{self, JoinHandle},
};

type Publication = (
    Option<hex_storage::Journal>,
    Result<Option<PathBuf>, String>,
);

struct Job {
    checkpoint: PreparedCheckpoint,
    journal: Option<hex_storage::Journal>,
    reply: SyncSender<Publication>,
}

/// At most one admitted publication. The control owner joins on destruction.
pub(super) struct CheckpointWorker {
    sender: Option<SyncSender<Job>>,
    pending: Option<Receiver<Publication>>,
    thread: Option<JoinHandle<()>>,
}

impl CheckpointWorker {
    pub(super) fn new() -> Result<Self, String> {
        let (sender, receiver) = mpsc::sync_channel::<Job>(1);
        let thread = thread::Builder::new()
            .name("hex-recovery-checkpoint".into())
            .spawn(move || {
                while let Ok(mut job) = receiver.recv() {
                    let result = job.checkpoint.publish(&mut job.journal);
                    let _ = job.reply.send((job.journal, result));
                }
            })
            .map_err(|error| format!("could not start recovery checkpoint worker: {error}"))?;
        Ok(Self {
            sender: Some(sender),
            pending: None,
            thread: Some(thread),
        })
    }

    /// Refusal retains journal custody on control. A pending result occupies the
    /// only slot until collected, even if the worker has already finished.
    pub(super) fn start(
        &mut self,
        checkpoint: PreparedCheckpoint,
        journal: &mut Option<hex_storage::Journal>,
    ) -> Result<(), String> {
        if self.pending.is_some() {
            return Err("recovery checkpoint is already pending".into());
        }
        let sender = self
            .sender
            .as_ref()
            .ok_or("recovery checkpoint worker is closed")?;
        let (reply, receiver) = mpsc::sync_channel(1);
        let job = Job {
            checkpoint,
            journal: journal.take(),
            reply,
        };
        match sender.try_send(job) {
            Ok(()) => {
                self.pending = Some(receiver);
                Ok(())
            }
            Err(TrySendError::Full(job)) => {
                *journal = job.journal;
                Err("recovery checkpoint worker is full".into())
            }
            Err(TrySendError::Disconnected(job)) => {
                *journal = job.journal;
                Err("recovery checkpoint worker is unavailable".into())
            }
        }
    }

    /// Wait only on the service control worker; publication itself owns the journal.
    pub(super) fn finish(
        &mut self,
        journal: &mut Option<hex_storage::Journal>,
    ) -> Result<Option<PathBuf>, String> {
        if journal.is_some() {
            return Err("cannot replace existing recovery journal custody".into());
        }
        let pending = self
            .pending
            .take()
            .ok_or("no recovery checkpoint is pending")?;
        match pending.recv() {
            Ok((returned, result)) => {
                *journal = returned;
                result
            }
            Err(_) => Err("recovery checkpoint worker stopped before returning journal custody; check the recovery file before restarting".into()),
        }
    }
}

impl Drop for CheckpointWorker {
    fn drop(&mut self) {
        self.sender.take();
        // A pending reply has capacity one, so joining cannot wait for its receiver.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::SaveImage;
    use super::*;
    use crate::Session;
    use hex_project::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn path() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "hex-checkpoint-worker-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn capture(session: &Session, path: &std::path::Path) -> PreparedCheckpoint {
        PreparedCheckpoint {
            path: path.to_path_buf(),
            untitled: true,
            image: SaveImage::capture(session, None).unwrap(),
        }
    }

    #[test]
    fn bounded_worker_returns_custody_and_preserves_conflicting_bytes() {
        let directory = path();
        std::fs::create_dir(&directory).unwrap();
        let target = directory.join("recovery.hex-journal");
        let other = directory.join("other.hex-journal");
        let mut session = Session::new(48_000, 8, 1024).unwrap();
        session
            .command(Command::AddTrack { name: "one".into() })
            .unwrap();
        let expected = session.project().clone();
        let mut journal = None;
        let mut worker = CheckpointWorker::new().unwrap();
        worker
            .start(capture(&session, &target), &mut journal)
            .unwrap();
        let mut other_journal = Some(hex_storage::Journal::open_empty(other.clone()).unwrap());
        assert!(
            worker
                .start(capture(&session, &other), &mut other_journal)
                .is_err()
        );
        assert_eq!(other_journal.as_ref().unwrap().path(), other);
        assert!(worker.finish(&mut other_journal).is_err());
        assert_eq!(other_journal.as_ref().unwrap().path(), other);
        worker.finish(&mut journal).unwrap();
        assert_eq!(journal.as_ref().unwrap().path(), target);
        assert_eq!(hex_storage::load_journal(&target).unwrap(), expected);
        let foreign = b"another writer";
        std::fs::write(&target, foreign).unwrap();
        worker
            .start(capture(&session, &target), &mut journal)
            .unwrap();
        assert!(worker.finish(&mut journal).is_err());
        assert!(journal.is_some());
        assert_eq!(std::fs::read(&target).unwrap(), foreign);
        drop(worker);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn closed_worker_refuses_without_taking_journal() {
        let target = path();
        let session = Session::new(48_000, 8, 1024).unwrap();
        let mut journal = Some(hex_storage::Journal::open_empty(target.clone()).unwrap());
        let mut worker = CheckpointWorker::new().unwrap();
        worker.sender.take();
        assert!(
            worker
                .start(capture(&session, &target), &mut journal)
                .is_err()
        );
        assert_eq!(journal.as_ref().unwrap().path(), target);
        drop(worker);
        assert!(!target.exists());
    }

    #[test]
    fn dropping_worker_joins_the_admitted_publication() {
        let directory = path();
        std::fs::create_dir(&directory).unwrap();
        let target = directory.join("recovery.hex-journal");
        let mut session = Session::new(48_000, 8, 1024).unwrap();
        session
            .command(Command::AddTrack { name: "one".into() })
            .unwrap();
        let expected = session.project().clone();
        let mut journal = None;
        let mut worker = CheckpointWorker::new().unwrap();
        worker
            .start(capture(&session, &target), &mut journal)
            .unwrap();
        drop(worker);
        assert_eq!(hex_storage::load_journal(&target).unwrap(), expected);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
