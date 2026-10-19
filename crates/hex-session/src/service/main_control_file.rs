//! Productive File turns around short loans of the EXISTING preparation parent.
//! No Runtime, lock, owner callback or source/cost permit is stored in this job.
use hex_engine::{
    AudioCandidateControlBlock as Block, AudioCandidateControlProgress as Progress,
    AudioCandidateIdentity, AudioCandidatePreparationOperations as Operations, AudioStorageError,
    Frame,
};
use hex_export::{ExportError, ExportReport, StagedWavFile};
use std::any::Any;

/// Concrete adapter outcome. Busy is valid only BEFORE invoking the loan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MainControlHostError {
    Busy,
    Closed,
    Refused,
}

/// Stack-scoped continuation; the actual adapter invokes it once under its
/// existing generation-checked preparation token. It cannot return an owner.
pub trait MainControlFileLoan {
    fn run(&mut self, operations: &mut Operations<'_>) -> Result<(), AudioStorageError>;
}

#[derive(Debug)]
pub enum MainControlFileError {
    Host(MainControlHostError),
    Storage(AudioStorageError),
    File(ExportError),
    Protocol,
    Panic,
}
pub enum MainControlFileProgress {
    Busy,
    Progress { written_frames: u64 },
    Published(ExportReport),
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Unbound,
    NeedBlock,
    Copied,
    Written,
    Retiring,
    LoanPending,
    FilePending,
    Published,
    Failed,
}
#[derive(Clone, Copy)]
enum Action {
    RenderCopy,
    Acknowledge(Block),
    Retire(AudioCandidateIdentity),
}
struct Loan<'a> {
    action: Action,
    output: &'a mut [Frame; 256],
    result: Option<Result<Progress, AudioStorageError>>,
    duplicate: bool,
    invoked: bool,
}
impl MainControlFileLoan for Loan<'_> {
    fn run(&mut self, operations: &mut Operations<'_>) -> Result<(), AudioStorageError> {
        if self.invoked {
            self.duplicate = true;
            return Err(AudioStorageError::SourceMismatch);
        }
        self.invoked = true;
        let result = match self.action {
            Action::RenderCopy => {
                operations
                    .step_main_control_work_on_control(256)
                    .and_then(|progress| {
                        if let Progress::Block(block) = progress {
                            operations.copy_main_control_block_on_control(block, self.output)?;
                        }
                        Ok(progress)
                    })
            }
            Action::Acknowledge(block) => operations
                .acknowledge_main_control_block_on_control(block)
                .map(|()| Progress::Block(block)),
            Action::Retire(identity) => operations
                .finish_main_control_work_on_control(identity)
                .map(|()| Progress::Complete),
        };
        self.result = Some(result);
        result.map(|_| ())
    }
}

/// The host reserves/quotes this actual job, its paths and all simultaneous
/// runtime/queued/copied/encoding buffers BEFORE constructing Main owners.
/// Association with worker/request/incarnation and the preparation token stays
/// in that concrete host; this job/its progress are not admission authority.
/// It must remain in an existing worker destination through failure/retirement.
pub struct MainControlFileJob {
    file: StagedWavFile,
    identity: Option<AudioCandidateIdentity>,
    copy: [Frame; 256],
    pending: Option<Block>,
    phase: Phase,
    first_panic: Option<Box<dyn Any + Send>>,
}
impl MainControlFileJob {
    pub fn reserve_on_control(file: StagedWavFile) -> Result<Self, MainControlFileError> {
        if file.written_frames() != 0 {
            return Err(MainControlFileError::Protocol);
        }
        Ok(Self {
            file,
            identity: None,
            copy: [[0.0; 2]; 256],
            pending: None,
            phase: Phase::Unbound,
            first_panic: None,
        })
    }
    /// Bind only after the host has constructed the actual Main parent. This
    /// scalar is correlation, never admission or a replacement for its token.
    pub fn bind_main_on_control(
        &mut self,
        identity: AudioCandidateIdentity,
    ) -> Result<(), MainControlFileError> {
        if self.phase != Phase::Unbound || self.identity.is_some() {
            return Err(MainControlFileError::Protocol);
        }
        self.identity = Some(identity);
        self.phase = Phase::NeedBlock;
        Ok(())
    }
    pub fn failed_incomplete(&self) -> bool {
        matches!(
            self.phase,
            Phase::LoanPending | Phase::FilePending | Phase::Failed
        )
    }
    /// No continuation/guard is retained across this call's File I/O. A block
    /// written before host Busy remains Written and retries ONLY its exact ACK.
    pub fn advance_on_control(
        &mut self,
        mut host: impl FnMut(&mut dyn MainControlFileLoan) -> Result<(), MainControlHostError>,
    ) -> Result<MainControlFileProgress, MainControlFileError> {
        if self.failed_incomplete() || matches!(self.phase, Phase::Published | Phase::Unbound) {
            return Err(MainControlFileError::Protocol);
        }
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.advance(&mut host)));
        match result {
            Ok(result) => {
                if result.is_err() {
                    self.phase = Phase::Failed;
                }
                result
            }
            Err(payload) => {
                self.first_panic = Some(payload);
                self.phase = Phase::Failed;
                Err(MainControlFileError::Panic)
            }
        }
    }
    fn loan(
        &mut self,
        action: Action,
        host: &mut impl FnMut(&mut dyn MainControlFileLoan) -> Result<(), MainControlHostError>,
    ) -> Result<Option<Progress>, MainControlFileError> {
        let previous = self.phase;
        self.phase = Phase::LoanPending;
        let mut loan = Loan {
            action,
            output: &mut self.copy,
            result: None,
            duplicate: false,
            invoked: false,
        };
        let outcome = host(&mut loan);
        if loan.duplicate {
            return Err(MainControlFileError::Protocol);
        }
        match (outcome, loan.result) {
            (Err(MainControlHostError::Busy), None) if !loan.invoked => {
                self.phase = previous;
                Ok(None)
            }
            (Err(MainControlHostError::Busy), _) => Err(MainControlFileError::Protocol),
            (Err(MainControlHostError::Closed), _) => {
                Err(MainControlFileError::Host(MainControlHostError::Closed))
            }
            (Err(reason), None) => Err(MainControlFileError::Host(reason)),
            (Ok(()), Some(Ok(progress))) => {
                self.phase = previous;
                Ok(Some(progress))
            }
            (_, Some(Err(AudioStorageError::ExternalReadersPending)))
                if matches!(action, Action::Retire(_)) =>
            {
                self.phase = previous;
                Ok(None)
            }
            (_, Some(Err(reason))) => Err(MainControlFileError::Storage(reason)),
            _ => Err(MainControlFileError::Protocol),
        }
    }
    fn advance(
        &mut self,
        host: &mut impl FnMut(&mut dyn MainControlFileLoan) -> Result<(), MainControlHostError>,
    ) -> Result<MainControlFileProgress, MainControlFileError> {
        if self.phase == Phase::NeedBlock {
            match self.loan(Action::RenderCopy, host)? {
                None => return Ok(MainControlFileProgress::Busy),
                Some(Progress::Block(block)) => {
                    if Some(block.runtime_identity()) != self.identity {
                        return Err(MainControlFileError::Protocol);
                    }
                    self.pending = Some(block);
                    self.phase = Phase::Copied;
                }
                Some(Progress::Complete) => self.phase = Phase::Retiring,
            }
        }
        if self.phase == Phase::Copied {
            let block = self.pending.ok_or(MainControlFileError::Protocol)?;
            self.phase = Phase::FilePending;
            self.file
                .append_on_control(block.offset(), &self.copy[..block.frames()])
                .map_err(MainControlFileError::File)?;
            self.phase = Phase::Written;
        }
        if self.phase == Phase::Written {
            let block = self.pending.ok_or(MainControlFileError::Protocol)?;
            if self.loan(Action::Acknowledge(block), host)?.is_none() {
                return Ok(MainControlFileProgress::Busy);
            }
            self.pending = None;
            self.phase = Phase::NeedBlock;
            return Ok(MainControlFileProgress::Progress {
                written_frames: self.file.written_frames(),
            });
        }
        if self.phase != Phase::Retiring {
            return Err(MainControlFileError::Protocol);
        }
        let identity = self.identity.ok_or(MainControlFileError::Protocol)?;
        if self.loan(Action::Retire(identity), host)?.is_none() {
            return Ok(MainControlFileProgress::Busy);
        }
        self.phase = Phase::FilePending;
        let report = self
            .file
            .publish_on_control()
            .map_err(MainControlFileError::File)?;
        self.phase = Phase::Published;
        Ok(MainControlFileProgress::Published(report))
    }
}

#[cfg(test)]
mod tests;
