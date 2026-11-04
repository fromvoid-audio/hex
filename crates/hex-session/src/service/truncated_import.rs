//! Typed truncated-WAV refusal and explicit readable-frame recovery for the
//! single-file audio import requests.
//!
//! `hex_media` refuses a WAV whose data chunk crosses physical end of file, or
//! that ends in a partial frame, as [`hex_media::ImportError::Truncated`]. The
//! worker reports that refusal as an [`ImportTruncation`] on the import event,
//! beside the existing error text, so a consumer can offer recovery. Recovery
//! happens only on an explicit [`Request::RecoverTruncatedImport`]. It decodes
//! the readable whole frames with [`hex_media::recover_wav_file`] into resident
//! media; the verified streaming path cannot represent a truncated file. The
//! original file is only read, never modified. Recovery in a verified-streaming
//! document is refused, and a recovery of an intact file imports it normally.
//! No saved format changes: the recovered audio is ordinary resident media.
use super::Request;
use crate::{DocumentAudioMode, Session, SessionError, audio_sources::VerifiedAudioFile};
use hex_media::{DecodedAudio, ImportError, Limits, WavTruncation};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_RECOVERED_SOURCE: AtomicU64 = AtomicU64::new(0);

/// A single-file import refused, or recovered, because its WAV data is truncated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportTruncation {
    /// Absolute source path the import read.
    pub path: PathBuf,
    /// Complete frames present in the file; recovery imports exactly these.
    pub readable_frames: u64,
    /// Declared data bytes absent before physical end of file.
    pub missing_bytes: u64,
    /// Bytes of a trailing incomplete frame that recovery discards.
    pub partial_frame_bytes: u64,
}

impl ImportTruncation {
    fn new(path: &Path, truncation: WavTruncation) -> Self {
        Self {
            path: path.to_path_buf(),
            readable_frames: truncation.readable_frames,
            missing_bytes: truncation.missing_bytes,
            partial_frame_bytes: truncation.partial_frame_bytes,
        }
    }
}

/// One single-file import request explicitly asking to recover readable frames.
/// Construct it with [`TruncatedImportRecovery::new`]; only `Import`,
/// `ImportToTrack`, `ImportToTrackWithOverwrite`, `ImportToNewTrack`,
/// `ImportToNewTrackWithGain` and `ImportToSession` qualify.
#[derive(Debug)]
pub struct TruncatedImportRecovery(Box<Request>);

impl TruncatedImportRecovery {
    /// Wrap a single-file import, or hand back any other request unchanged.
    pub fn new(request: Request) -> Result<Self, Box<Request>> {
        let request = Box::new(request);
        if is_single_file_import(&request) {
            Ok(Self(request))
        } else {
            Err(request)
        }
    }

    /// The import this recovery applies to.
    pub fn request(&self) -> &Request {
        &self.0
    }
}

fn is_single_file_import(request: &Request) -> bool {
    matches!(
        request,
        Request::Import(_)
            | Request::ImportToTrack { .. }
            | Request::ImportToTrackWithOverwrite { .. }
            | Request::ImportToNewTrack { .. }
            | Request::ImportToNewTrackWithGain { .. }
            | Request::ImportToSession { .. }
    )
}

/// Unwrap an explicit recovery into its import and the decode policy for it.
pub(super) fn admit(request: Request) -> (Request, ImportDecode) {
    match request {
        Request::RecoverTruncatedImport(recovery) => (*recovery.0, ImportDecode::new(true)),
        request => (request, ImportDecode::new(false)),
    }
}

/// Per-request decode policy and its truncation observations.
#[derive(Debug, Default)]
pub(super) struct ImportDecode {
    recover: bool,
    refused: Option<ImportTruncation>,
    recovered: Option<ImportTruncation>,
    recovered_path: Option<PathBuf>,
}

const RECOVERY_NEEDS_RESIDENT: &str =
    "recovering a truncated recording needs a resident document · open it in resident mode";

pub(super) fn decode_error_text(error: SessionError) -> String {
    match error {
        SessionError::Media(error) => error.to_string(),
        error => error.to_string(),
    }
}

impl ImportDecode {
    fn new(recover: bool) -> Self {
        Self {
            recover,
            ..Self::default()
        }
    }

    /// The verified-source plan, or `None` for the resident decode path.
    /// Recovery always uses the resident decoder.
    pub(super) fn plan(
        &mut self,
        session: &Session,
        path: &Path,
        allowance: Option<u64>,
    ) -> Result<Option<VerifiedAudioFile>, SessionError> {
        if self.recover {
            if session.audio_mode() == DocumentAudioMode::VerifiedStreaming {
                return Err(SessionError::Invalid(RECOVERY_NEEDS_RESIDENT));
            }
            return Ok(None);
        }
        let planned = match allowance {
            Some(allowance) => session.plan_file_import_with_allowance(path, allowance),
            None => session.plan_file_import(path),
        };
        if let Err(SessionError::Media(ImportError::Truncated(truncation))) = &planned {
            self.refused = Some(ImportTruncation::new(path, *truncation));
        }
        planned
    }

    /// Decode `path` for resident import, recovering readable frames only when
    /// this request asked for it.
    pub(super) fn decode(
        &mut self,
        path: PathBuf,
        limits: Limits,
    ) -> Result<DecodedAudio, SessionError> {
        if self.recover {
            match crate::resident_import::decode_resident_import(&path) {
                Ok(decoded) => return Ok(decoded),
                Err(ImportError::Truncated(_)) => {}
                Err(error) => return Err(error.into()),
            }
            let parent = path
                .parent()
                .ok_or(SessionError::Invalid("source has no parent directory"))?;
            let max_output = limits
                .max_frames
                .checked_mul(8)
                .and_then(|n| n.checked_add(hex_export::FLOAT32_WAV_HEADER_BYTES as usize))
                .ok_or(SessionError::Invalid("recovery output limit overflows"))?;
            for _ in 0..16 {
                let serial = NEXT_RECOVERED_SOURCE.fetch_add(1, Ordering::Relaxed);
                let destination =
                    parent.join(format!("hex-recovered-{}-{serial}.wav", std::process::id()));
                match crate::recover_wav_to_new_source(&path, &destination, limits, max_output) {
                    Ok(result) => {
                        self.recovered_path = Some(destination);
                        let Some(truncation) = result.truncation else {
                            return Err(SessionError::Invalid(
                                "source changed before recovery; retry import",
                            ));
                        };
                        self.recovered = Some(ImportTruncation::new(&path, truncation));
                        return Ok(result.decoded);
                    }
                    Err(crate::RecoverySourceError::Publish(hex_export::ExportError::Io {
                        source,
                        ..
                    })) if source.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(SessionError::NativeAudio(error.to_string())),
                }
            }
            return Err(SessionError::Invalid(
                "could not reserve a new recovered source name",
            ));
        }
        crate::resident_import::decode_resident_import(&path)
            .inspect_err(|error| {
                if let ImportError::Truncated(truncation) = error {
                    self.refused = Some(ImportTruncation::new(&path, *truncation));
                }
            })
            .map_err(Into::into)
    }

    /// The refusal to report on the event, if this import was refused as truncated.
    pub(super) fn refused(&self) -> Option<&ImportTruncation> {
        self.refused.as_ref()
    }

    /// Status suffix recording the fidelity loss of a successful recovery.
    pub(super) fn recovery_note(&self) -> Option<String> {
        self.recovered.as_ref().zip(self.recovered_path.as_ref()).map(|(truncation, path)| {
            format!(
                "recovered {} readable frames · {} missing bytes and {} partial-frame bytes discarded · original file unchanged · recovered source at {}",
                truncation.readable_frames,
                truncation.missing_bytes,
                truncation.partial_frame_bytes,
                hex_storage::text::verbatim(path.display()),
            )
        })
    }

    pub(super) fn recovered_path(&self) -> Option<&Path> {
        self.recovered_path.as_deref()
    }
}

#[cfg(test)]
#[path = "truncated_import_tests.rs"]
mod tests;
