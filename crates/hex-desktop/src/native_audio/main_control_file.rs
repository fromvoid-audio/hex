//! Exact-token loans; no File I/O occurs while WorkerPublication is borrowed.
use hex_audio::{
    AudioCandidatePreparationProgress, AudioCandidatePreparationToken, WorkerPreparedOutput,
};
use hex_engine::AudioStorageError;
use hex_session::service::{
    MainControlFileError, MainControlFileJob, MainControlFileLoan, MainControlFileProgress,
    MainControlHostError,
};

// Actual Service dispatch is installed only with the complete input union; this
// bridge does not select a token by revision or manufacture publication Ready.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "actual Service Main dispatch and whole-union binding in progress"
    )
)]
pub(super) fn advance(
    output: &mut WorkerPreparedOutput,
    token: AudioCandidatePreparationToken,
    job: &mut MainControlFileJob,
) -> Result<MainControlFileProgress, MainControlFileError> {
    job.advance_on_control(|continuation| {
        // Obtain and end this control borrow on EACH loan, not around the File
        // job's whole advance. Work/bank exclusions end inside the operation.
        let mut control = output
            .publication
            .control()
            .ok_or(MainControlHostError::Closed)?;
        run_loan(&mut control, token, continuation)
    })
}

fn run_loan(
    control: &mut hex_audio::prepared::WorkerPublication<'_>,
    token: AudioCandidatePreparationToken,
    continuation: &mut dyn MainControlFileLoan,
) -> Result<(), MainControlHostError> {
    let mut invoked = false;
    let progress = control.prepare_audio_candidate_on_control(token, |mut operations| {
        invoked = true;
        match continuation.run(&mut operations) {
            // The continuation already recorded the real pending result. Err
            // here would mark this SAME parent terminally refused in the host.
            Err(AudioStorageError::ExternalReadersPending) => Ok(()),
            result => result,
        }
    });
    match progress {
        // Ready/Collecting describe the preparation cell, NEVER publication.
        Ok(
            AudioCandidatePreparationProgress::Ready
            | AudioCandidatePreparationProgress::Collecting,
        ) if invoked => Ok(()),
        Err("candidate preparation is busy") if !invoked => Err(MainControlHostError::Busy),
        _ => Err(MainControlHostError::Refused),
    }
}
