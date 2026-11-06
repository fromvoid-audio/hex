//! Pulse #4144/#4147: bounce, clip bounce, track bake and grouping of file-backed
//! (streamed) sources render through a host page provider with the same PCM as
//! the resident equivalents, including offsets, trims and release tails. Without
//! a provider, or on a page miss, they refuse explicitly and atomically.
use super::*;
use crate::{DocumentAudioMode, SourcePlanReservation, verified_import::VerifiedImportDestination};
use crate::{StreamedBounceRequest, StreamedBouncer};
use hex_engine::{BorrowedMediaPage, MediaPages, STREAM_PAGE_FRAMES, StreamSourceKey};
use hex_export::{ExportControl, ExportError, ExportReport, PreparedExportPages};
use hex_project::{AssetId, ClipId, Command, TimeRange, TrackId};
use std::collections::BTreeMap;

const FRAMES: usize = 6_000;
/// Distinct per-frame stereo values: any offset, trim or channel error shows.
fn source() -> Vec<hex_engine::Frame> {
    (0..FRAMES)
        .map(|i| {
            let x = (i as f32 + 1.0) / FRAMES as f32;
            [0.8 * x, -0.5 * x + 0.1]
        })
        .collect()
}
fn reservation() -> SourcePlanReservation {
    SourcePlanReservation {
        reader: hex_media::VerifiedWavReaderLimits {
            reader: hex_media::WavReaderLimits {
                max_file_bytes: 1 << 20,
                max_source_frames: 1 << 20,
                max_header_count: 32,
                max_window_frames: 4096,
            },
            max_manifest_bytes: 1024 * 1024,
        },
        max_retained_sources: 8,
        max_path_bytes: 1024 * 1024,
        max_manifest_bytes: 4 * 1024 * 1024,
    }
}
/// One imported clip at frame 0, plus a second placement of the same asset with
/// a source offset and trimmed length on the same track.
fn fixture(path: &Path, streamed: bool) -> (Session, TrackId, ClipId, ClipId) {
    let mut session = Session::new_with_audio_mode(
        48_000,
        16,
        // Room for the bounce budget plus its release-tail allowance.
        8_000_000,
        DocumentAudioMode::Resident,
        Some(reservation()),
    )
    .unwrap();
    let created = if streamed {
        let file = session.verify_import(path.to_path_buf()).unwrap();
        session
            .import_verified_revision(
                file,
                VerifiedImportDestination::NewTrack {
                    before: None,
                    start: 0,
                },
                None,
            )
            .unwrap()
            .0
    } else {
        let decoded =
            hex_media::decode_wav_file(path, hex_media::Limits::new(1 << 20, FRAMES)).unwrap();
        session
            .import_prepared_revision(crate::PreparedImport::new(decoded).unwrap(), None)
            .unwrap()
            .0
    };
    let Created::Imported { track, asset, clip } = created else {
        panic!("audio import expected")
    };
    assert_eq!(
        session.audio(asset).is_none(),
        streamed,
        "the streamed fixture must stay file-backed"
    );
    let Created::Clip(placed) = session
        .command(Command::PlaceClip {
            track,
            asset,
            start: 10_000,
            duration: 1_500,
            source_offset: 1_000,
            source_length: 1_500,
            looping: false,
        })
        .unwrap()
    else {
        panic!("placement expected")
    };
    (session, track, clip, placed)
}
/// Test host provider: pages served from the decoded original, demand checked
/// against the file's extent so a tail or crop can never ask past its end.
struct MemoryPages {
    sources: BTreeMap<StreamSourceKey, Vec<hex_engine::Frame>>,
    serve: bool,
    missed: bool,
}
impl MemoryPages {
    fn capture(request: &StreamedBounceRequest<'_>, serve: bool) -> Result<Self, String> {
        let mut sources = BTreeMap::new();
        for metadata in request.prepared().arrangement_stream_sources() {
            let file = request
                .file(metadata.key().asset())
                .ok_or("streamed bounce source has no captured file")?;
            let decoded =
                hex_media::decode_wav_file(file.path(), hex_media::Limits::new(1 << 20, FRAMES))
                    .map_err(|e| e.to_string())?;
            sources.insert(metadata.key(), decoded.audio().frames().to_vec());
        }
        Ok(Self {
            sources,
            serve,
            missed: false,
        })
    }
}
impl MediaPages for MemoryPages {
    fn page(&mut self, key: StreamSourceKey, index: u64) -> Option<BorrowedMediaPage<'_>> {
        let frames = self.sources.get(&key)?;
        let start = usize::try_from(index)
            .ok()?
            .checked_mul(STREAM_PAGE_FRAMES)?;
        let page = frames.get(start..(start + STREAM_PAGE_FRAMES).min(frames.len()));
        if page.is_none() || !self.serve {
            self.missed = true;
            return None;
        }
        page.map(|frames| BorrowedMediaPage {
            page_index: index,
            frames,
        })
    }
    fn request_page(&mut self, _: StreamSourceKey, _: u64) {
        self.missed = true;
    }
}
impl PreparedExportPages for MemoryPages {
    fn prepare_chunk(
        &mut self,
        playback: &hex_engine::SessionPlayback,
        frames: usize,
        _: &ExportControl,
    ) -> Result<(), ExportError> {
        self.missed = false;
        let sources = &self.sources;
        let window = playback
            .visit_next_stream_chunk(frames, None, &mut |extent| {
                let hex_engine::StreamResourceKey::Audio(key) = extent.source else {
                    return Err(hex_engine::Invalid("unexpected scalar stream demand"));
                };
                let length = sources
                    .get(&key)
                    .ok_or(hex_engine::Invalid("demand for an uncaptured source"))?
                    .len() as u64;
                let end = extent.page_index * STREAM_PAGE_FRAMES as u64 + extent.end as u64;
                if extent.start >= extent.end || end > length {
                    return Err(hex_engine::Invalid(
                        "page demand past the end of the source",
                    ));
                }
                Ok(())
            })
            .map_err(ExportError::Render)?;
        if window.frames != frames {
            return Err(ExportError::Invalid("linear export demand width differs"));
        }
        Ok(())
    }
    fn complete_chunk(&mut self) -> Result<(), ExportError> {
        if self.missed {
            Err(ExportError::Invalid("bounce encountered a media page miss"))
        } else {
            Ok(())
        }
    }
    fn finish(&mut self, _: &ExportControl) -> Result<(), ExportError> {
        Ok(())
    }
}
fn render_with(request: &StreamedBounceRequest<'_>, serve: bool) -> Result<ExportReport, String> {
    let mut pages = MemoryPages::capture(request, serve)?;
    hex_export::render_arrangement_bounce_with_pages(
        request.path(),
        std::sync::Arc::clone(request.prepared()),
        request.resources(),
        request.start(),
        request.frames(),
        request.include_tails(),
        request.control(),
        Some(&mut pages),
    )
    .map_err(|e| e.to_string())
}
fn serving(request: &StreamedBounceRequest<'_>) -> Result<ExportReport, String> {
    render_with(request, true)
}
fn missing(request: &StreamedBounceRequest<'_>) -> Result<ExportReport, String> {
    render_with(request, false)
}

/// PCM of the single asset a bounce adds.
fn bounce(
    session: &mut Session,
    folder: &Path,
    target: BounceTarget,
    in_place: bool,
    streamed: Option<StreamedBouncer>,
) -> Result<Vec<hex_engine::Frame>, String> {
    let before: Vec<AssetId> = session.project().assets().map(|a| a.id()).collect();
    session.bounce_arrangement_adopting_with(
        target,
        in_place,
        hex_project::BounceAdoption::NewTracks,
        folder,
        &ExportControl::default(),
        None,
        streamed,
    )?;
    let added: Vec<AssetId> = session
        .project()
        .assets()
        .map(|a| a.id())
        .filter(|id| !before.contains(id))
        .collect();
    assert_eq!(added.len(), 1, "one bounced asset");
    Ok(session.audio(added[0]).unwrap().frames().to_vec())
}
fn assert_matches(actual: &[hex_engine::Frame], expected: &[hex_engine::Frame], case: &str) {
    assert!(actual.len() >= expected.len(), "{case}: short render");
    let error = actual
        .iter()
        .zip(expected)
        .flat_map(|(a, b)| a.iter().zip(b).map(|(a, b)| (a - b).abs()))
        .fold(0.0_f32, f32::max);
    assert!(error < 1e-6, "{case}: max sample error {error}");
}
#[allow(clippy::too_many_arguments)]
fn assert_refused_atomically(
    session: &mut Session,
    folder: &Path,
    target: BounceTarget,
    in_place: bool,
    streamed: Option<StreamedBouncer>,
    expected: &str,
    case: &str,
) {
    let original = session.project().snapshot();
    let refused = bounce(session, folder, target, in_place, streamed).unwrap_err();
    assert!(refused.contains(expected), "{case}: got {refused}");
    assert_eq!(
        session.project().snapshot(),
        original,
        "{case}: atomic refusal"
    );
    assert!(
        std::fs::read_dir(folder).map_or(true, |mut entries| entries.next().is_none()),
        "{case}: no bounce media left behind"
    );
}

type Target = fn(TrackId, ClipId, ClipId) -> (BounceTarget, bool);

#[test]
fn latent_streamed_bounce_matches_resident_and_missing_pages_remain_atomic() {
    let folder = Folder::new();
    let path = folder.0.join("source.wav");
    grouping::write_fixture(&path, 48_000, &source());
    let limiter = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.limiter"),
    )
    .unwrap();
    let mut outputs = Vec::new();
    for streamed in [false, true] {
        let (mut session, track, clip, _) = fixture(&path, streamed);
        session
            .command(Command::InsertDevice {
                track,
                before: None,
                state: limiter.clone(),
            })
            .unwrap();
        assert!(
            session
                .compile_prepared_revision(None)
                .unwrap()
                .arrangement()
                .main_latency_frames()
                > 0
        );
        let output = bounce(
            &mut session,
            &folder.0.join(format!("latent-{streamed}")),
            BounceTarget::Clips(vec![clip]),
            false,
            streamed.then_some(serving as StreamedBouncer),
        )
        .unwrap();
        outputs.push(output);
    }
    assert_eq!(outputs[0], outputs[1]);
    let (mut session, track, clip, _) = fixture(&path, true);
    session
        .command(Command::InsertDevice {
            track,
            before: None,
            state: limiter,
        })
        .unwrap();
    assert_refused_atomically(
        &mut session,
        &folder.0.join("latent-page-miss"),
        BounceTarget::Clips(vec![clip]),
        false,
        Some(missing),
        "page miss",
        "latent page miss",
    );
}

#[test]
fn streamed_bounces_match_resident_spans_and_refuse_atomically_without_pages() {
    let folder = Folder::new();
    let path = folder.0.join("source.wav");
    let source = source();
    grouping::write_fixture(&path, 48_000, &source);
    // Whole-track bake: the clip, silence up to the offset placement, then it.
    let mut track_bake = source.clone();
    track_bake.resize(10_000, [0.0; 2]);
    track_bake.extend_from_slice(&source[1_000..2_500]);
    let cases: [(&str, Target, &[hex_engine::Frame]); 5] = [
        (
            "whole clip",
            |_, clip, _| (BounceTarget::Clips(vec![clip]), false),
            &source,
        ),
        (
            "offset placement",
            |_, _, placed| (BounceTarget::Clips(vec![placed]), false),
            &source[1_000..2_500],
        ),
        (
            "trimmed range",
            |track, _, _| {
                let range = TimeRange::new(vec![track], 500, 2_500).unwrap();
                (BounceTarget::Range(range), false)
            },
            &source[500..2_500],
        ),
        (
            "grouped crop",
            |track, clip, _| {
                let range = TimeRange::new(vec![track], 500, 2_500).unwrap();
                (
                    BounceTarget::AudioGroup {
                        clips: vec![clip],
                        range: Some(range),
                    },
                    true,
                )
            },
            &source[500..2_500],
        ),
        (
            "track bake in place",
            |track, _, _| (BounceTarget::Track(track), true),
            &track_bake,
        ),
    ];
    for (case, target, expected) in cases {
        let (mut session, track, clip, placed) = fixture(&path, false);
        let (resident_target, in_place) = target(track, clip, placed);
        let directory = folder.0.join(format!("{case}-resident"));
        let resident = bounce(&mut session, &directory, resident_target, in_place, None).unwrap();
        assert_matches(&resident, expected, &format!("{case} resident"));

        let (mut session, track, clip, placed) = fixture(&path, true);
        let (streamed_target, in_place) = target(track, clip, placed);
        let directory = folder.0.join(format!("{case}-streamed"));
        let rendered = bounce(
            &mut session,
            &directory,
            streamed_target,
            in_place,
            Some(serving),
        )
        .unwrap();
        assert_matches(&rendered, expected, &format!("{case} streamed"));
        assert_eq!(rendered.len(), resident.len(), "{case}: same tail length");

        let refusals: [(Option<StreamedBouncer>, &str, &str); 2] = [
            (None, "media page provider", "without provider"),
            (Some(missing), "page miss", "page miss"),
        ];
        for (bouncer, refusal, label) in refusals {
            let (mut session, track, clip, placed) = fixture(&path, true);
            let (refused_target, in_place) = target(track, clip, placed);
            let directory = folder.0.join(format!("{case}-{label}"));
            assert_refused_atomically(
                &mut session,
                &directory,
                refused_target,
                in_place,
                bouncer,
                refusal,
                &format!("{case} {label}"),
            );
            assert!(
                session.undo(),
                "{case}: history keeps only the fixture edits"
            );
            assert_eq!(session.project().track(track).unwrap().clips().len(), 1);
        }
    }
}
