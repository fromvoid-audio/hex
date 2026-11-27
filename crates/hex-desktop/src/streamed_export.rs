//! Independent offline source verification/pages, never native admission.
//!
//! One existing Session export job owns this control-thread facade, fixed pool
//! and joining worker. Native output/domain/catalog/readiness tokens are not
//! borrowed. Optional job-owned ordinary resources are forwarded as the exact
//! borrowed compiler scope. Core export owns DSP, cancellation/progress and
//! file publication; page preparation never transfers resource custody.
//! Pool/catalog/reader/kernel bounds are the existing StreamConfig limits; no
//! full-file resident PCM fallback or process-RSS/deadline claim is made.
#[cfg(all(test, feature = "allocation-probe"))]
use hex_engine::{BorrowedMediaPage, MediaPages};
use hex_engine::{SessionPlayback, StreamSourceKey};
#[cfg(all(test, feature = "allocation-probe"))]
use hex_export::{ExportControl, PreparedExportPages};
use hex_export::{ExportError, ExportReport, ForeignBounceRenderer};
use hex_session::{VerifiedAudioFile, service::StreamedExportRequest};
use hex_stream::StreamConfig;
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

#[path = "streamed_export/capture.rs"]
mod capture;
#[path = "streamed_export/derived.rs"]
mod derived;
#[path = "streamed_export/pages.rs"]
mod pages;
use derived::{DerivedCapture, check};
use pages::{VerifiedPages, seek_error};

const PREPARATION_TIMEOUT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(5);
// Page replies wake the offline consumer. The timeout bounds cancellation
// and fault checks even if no reply arrives.
const PAGE_POLL: Duration = Duration::from_millis(1);

pub(super) fn render(
    request: &StreamedExportRequest<'_>,
) -> Result<ExportReport, hex_session::service::ExportFailure> {
    render_with_config(request, StreamConfig::initial()).map_err(Into::into)
}

pub(crate) fn render_with_config(
    request: &StreamedExportRequest<'_>,
    config: StreamConfig,
) -> Result<ExportReport, ExportError> {
    render_with_config_and_foreign(request, config, request.take_foreign())
}

/// The job retains exclusive custody of the supplied offline processors; the
/// independent verified page worker and WAV publisher retain their own owners.
pub(crate) fn render_with_config_and_foreign(
    request: &StreamedExportRequest<'_>,
    config: StreamConfig,
    foreign: Option<&mut dyn ForeignBounceRenderer>,
) -> Result<ExportReport, ExportError> {
    check(request.control())?;
    let mut pages = VerifiedPages::capture(request, config)?;
    let spec = hex_export::ExportJobSpec {
        path: request.path(),
        revision: Arc::clone(request.prepared()),
        resources: request.resources(),
        start: request.start(),
        frames: request.frames(),
        signal: request.signal().clone(),
        format: request.format(),
        control: request.control(),
        publication: request.publication(),
    };
    if let Some(foreign) = foreign {
        return hex_export::render_prepared_with_foreign(spec, Some(&mut pages), foreign);
    }
    hex_export::render_prepared_with_pages_and_policy(spec, &mut pages)
}

/// [`hex_session::StreamedBouncer`]: one isolated bounce item through the same
/// independent verification, bounded pool and joining worker as export. File
/// facts come from the document's captured bindings; the worker is joined and
/// every file re-hashed before the bounce file is published. Nothing native is
/// borrowed, so a bounce never waits on or disturbs the playing transport.
pub(super) fn bounce(
    request: &hex_session::StreamedBounceRequest<'_>,
    foreign: Option<&mut dyn ForeignBounceRenderer>,
) -> Result<ExportReport, String> {
    bounce_with_config(request, StreamConfig::initial(), foreign)
        .map_err(|error| hex_session::service::export_error_text(&error))
}

pub(crate) fn bounce_with_config(
    request: &hex_session::StreamedBounceRequest<'_>,
    config: StreamConfig,
    foreign: Option<&mut dyn ForeignBounceRenderer>,
) -> Result<ExportReport, ExportError> {
    check(request.control())?;
    let mut pages = VerifiedPages::capture_files(
        request.prepared(),
        |key| {
            request.file(key.asset()).ok_or(ExportError::Invalid(
                "verified bounce source binding is missing",
            ))
        },
        request.repitches(),
        DerivedCapture::Bounce(request),
        Some(request.path()),
        config,
        request.control(),
    )?;
    let spec = hex_export::BounceSpec {
        path: request.path(),
        revision: Arc::clone(request.prepared()),
        resources: request.resources(),
        start: request.start(),
        frames: request.frames(),
        include_tails: request.include_tails(),
        control: request.control(),
    };
    if let Some(foreign) = foreign {
        return hex_export::render_arrangement_bounce_with_foreign(spec, Some(&mut pages), foreign);
    }
    hex_export::render_arrangement_bounce_with_pages(spec, Some(&mut pages))
}

/// Quote the original stream constructor's listed data before any pool/thread
/// exists, including this adapter's bounded metadata. No source I/O occurs here.
pub(super) fn resident_seek_storage_bytes(
    prepared: &hex_engine::PreparedRevision,
    sources: &hex_session::service::ResidentFileSet,
) -> Result<usize, String> {
    let pages = VerifiedPages::capture_files(
        prepared,
        |key| {
            sources
                .files()
                .iter()
                .find(|(asset, _)| *asset == key.asset())
                .map(|(_, file)| file)
                .ok_or(ExportError::Invalid(
                    "verified seek source binding is missing",
                ))
        },
        sources.repitches().requirements(),
        DerivedCapture::Captured(sources.granulars().requirements()),
        None,
        StreamConfig::initial(),
        &AtomicBool::new(false),
    )
    .map_err(|error| error.to_string())?;
    pages.storage_bytes().map_err(|error| error.to_string())
}

/// Run only on the original seek worker. This composes its exact admitted
/// ordinary resource scope with this adapter's independent bounded page owner.
/// All readers/pages are joined before returning to ordinary native publication.
pub(super) fn prepare_resident_seek(
    request: hex_session::service::ResidentFileSeekRequest<'_>,
) -> Result<hex_engine::playback::Playback, hex_engine::ArrangementPrerollError> {
    let runtime = prepare_file_seek_runtime(
        request.spec(),
        request.resources(),
        request.sources().context(),
        |key| {
            request
                .sources()
                .files()
                .iter()
                .find(|(asset, _)| *asset == key.asset())
                .map(|(_, file)| file)
                .ok_or(ExportError::Invalid(
                    "verified seek source binding is missing",
                ))
        },
        request.sources().repitches().requirements(),
        request.sources().granulars().requirements(),
        request.cancelled(),
    )?;
    hex_engine::playback::Playback::from_interactive_arrangement_preroll(runtime)
}

// Borrowed factual inputs only: the request's canonical custody and native
// publication remain with their owners. This helper joins all preparation pages.
fn prepare_file_seek_runtime<'a>(
    spec: &hex_session::arrangement_seek::ArrangementSeekSpec,
    resources: &hex_engine::native_device_resources::PreparedDeviceResources,
    context: hex_engine::AudioSourceContext,
    resolve: impl FnMut(StreamSourceKey) -> Result<&'a VerifiedAudioFile, ExportError>,
    repitches: &[hex_engine::audio_transform::StreamedRepitchRequirement],
    granulars: &[hex_engine::audio_transform::StreamedGranularRequirement],
    cancelled: &AtomicBool,
) -> Result<SessionPlayback, hex_engine::ArrangementPrerollError> {
    if context.project_generation != spec.generation {
        return Err(hex_engine::ArrangementPrerollError::MediaPreparation(
            "file seek source context differs",
        ));
    }
    check(cancelled).map_err(seek_error)?;
    if spec.note_chase == hex_engine::NoteChase::Off
        && spec.mode
            == hex_session::arrangement_seek::ArrangementSeekMode::HistoryPreservingInteractive
    {
        return Err(
            hex_engine::SessionPlaybackError::MutableInstrumentDiagnostic(
                "note chase off requires ordinary interactive playback",
            )
            .into(),
        );
    }
    let section = match spec.mode {
        hex_session::arrangement_seek::ArrangementSeekMode::SectionLoop(region) => Some(region),
        hex_session::arrangement_seek::ArrangementSeekMode::Interactive
        | hex_session::arrangement_seek::ArrangementSeekMode::HistoryPreservingInteractive => None,
        _ => {
            return Err(hex_engine::ArrangementPrerollError::MediaPreparation(
                "ordinary seek requires interactive policy",
            ));
        }
    };
    let mut pages = VerifiedPages::capture_files(
        &spec.prepared_revision,
        resolve,
        repitches,
        DerivedCapture::Captured(granulars),
        None,
        StreamConfig::initial(),
        cancelled,
    )
    .map_err(seek_error)?;
    let runtime = if spec.mode
        == hex_session::arrangement_seek::ArrangementSeekMode::HistoryPreservingInteractive
    {
        SessionPlayback::prepare_history_preserving_arrangement_preroll_with_pages(
            Arc::clone(&spec.prepared_revision),
            Some(resources),
            spec.destination_frame,
            spec.work_budget_frames,
            spec.start_playing,
            cancelled,
            &mut pages,
        )?
    } else {
        SessionPlayback::prepare_ordinary_arrangement_preroll_with_pages_and_note_chase(
            Arc::clone(&spec.prepared_revision),
            resources,
            spec.destination_frame,
            spec.work_budget_frames,
            spec.start_playing,
            section,
            spec.note_chase,
            cancelled,
            &mut pages,
        )?
    };
    pages.finish_pages(cancelled).map_err(seek_error)?;
    Ok(runtime)
}

#[cfg(all(test, feature = "allocation-probe"))]
#[path = "streamed_export/test_support.rs"]
pub(crate) mod test_support;

#[cfg(test)]
#[path = "streamed_export/tests.rs"]
mod tests;
