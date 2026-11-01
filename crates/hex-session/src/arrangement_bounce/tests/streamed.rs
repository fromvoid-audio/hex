//! Pulse #4144: bounce, clip bounce and grouping of file-backed (streamed)
//! sources. The offline bounce renderer has no media page provider yet, so a
//! streamed source must refuse explicitly and atomically, never publish silence.
//! The resident equivalents pin the exact PCM a future streamed render must match,
//! including offsets and trims.
use super::*;
use crate::{DocumentAudioMode, SourcePlanReservation, verified_import::VerifiedImportDestination};
use hex_project::{AssetId, ClipId, Command, TimeRange, TrackId};

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
                max_file_bytes: crate::MAX_STREAMED_FILE_BYTES,
                max_source_frames: crate::MAX_STREAMED_SOURCE_FRAMES,
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
/// PCM of the single asset a bounce adds.
fn bounce(
    session: &mut Session,
    folder: &Path,
    target: BounceTarget,
    in_place: bool,
) -> Result<Vec<hex_engine::Frame>, String> {
    let before: Vec<AssetId> = session.project().assets().map(|a| a.id()).collect();
    session.bounce_arrangement(
        target,
        in_place,
        folder,
        &hex_export::ExportControl::default(),
        None,
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

#[test]
fn streamed_bounces_refuse_atomically_while_resident_bounces_render_exact_spans() {
    let folder = Folder::new();
    let path = folder.0.join("source.wav");
    let source = source();
    grouping::write_fixture(&path, 48_000, &source);
    type Case = (
        &'static str,
        fn(TrackId, ClipId, ClipId) -> (BounceTarget, bool),
        std::ops::Range<usize>,
    );
    let cases: [Case; 4] = [
        (
            "whole clip",
            |_, clip, _| (BounceTarget::Clips(vec![clip]), false),
            0..FRAMES,
        ),
        (
            "offset placement",
            |_, _, placed| (BounceTarget::Clips(vec![placed]), false),
            1_000..2_500,
        ),
        (
            "trimmed range",
            |track, _, _| {
                let range = TimeRange::new(vec![track], 500, 2_500).unwrap();
                (BounceTarget::Range(range), false)
            },
            500..2_500,
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
            500..2_500,
        ),
    ];
    for (case, target, span) in cases {
        let (mut session, track, clip, placed) = fixture(&path, false);
        let (resident_target, in_place) = target(track, clip, placed);
        let directory = folder.0.join(format!("{case}-resident"));
        let rendered = bounce(&mut session, &directory, resident_target, in_place).unwrap();
        assert_matches(&rendered, &source[span], case);

        let (mut session, track, clip, placed) = fixture(&path, true);
        let (streamed_target, in_place) = target(track, clip, placed);
        let original = session.project().snapshot();
        let directory = folder.0.join(format!("{case}-streamed"));
        let refused = bounce(&mut session, &directory, streamed_target, in_place).unwrap_err();
        assert!(
            refused.contains("media page provider"),
            "{case}: streamed bounce must refuse explicitly, got {refused}"
        );
        assert_eq!(
            session.project().snapshot(),
            original,
            "{case}: atomic refusal"
        );
        assert!(
            session.undo(),
            "{case}: history keeps only the fixture edits"
        );
        assert!(session.project().track(track).unwrap().clips().len() == 1);
    }
}
