//! Long reverse reads through the ordinary Session export and bounce owners.
use super::*;
use crate::native_audio::tests::resident_export::completed;
use hex_export::{ExportControl, ExportFormat};
use hex_project::Command;
use hex_session::service::ExportSignal;
use std::io::{Seek, SeekFrom, Write};

const LONG_FRAMES: u64 = 20_000_000;
const TAIL_FRAMES: u64 = 8192;
const RANGE_FRAMES: u64 = 512;

#[derive(Clone, Copy)]
enum Output {
    Export,
    Bounce,
}

#[test]
fn long_reverse_export_matches_resident_across_a_stream_page() {
    compare(Output::Export);
}

#[test]
fn long_reverse_bounce_matches_resident_across_a_stream_page() {
    compare(Output::Bounce);
}

fn compare(output: Output) {
    let _native = native_fixture();
    let files = Files::new();
    let short = files.constant_wav("resident.wav", 48_000, TAIL_FRAMES as u32, &[0]);
    let long = super::super::granular_playback::long_source_at_rate(&files, 48_000);
    let ramp: Vec<i16> = (0..TAIL_FRAMES)
        .map(|frame| frame as i16 * 3 - 12_000)
        .collect();
    let bytes: Vec<u8> = ramp
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect();
    for (path, frames) in [(&short, TAIL_FRAMES), (&long, LONG_FRAMES)] {
        let mut wav = fs::OpenOptions::new().write(true).open(path).unwrap();
        wav.seek(SeekFrom::Start(44 + (frames - TAIL_FRAMES) * 2))
            .unwrap();
        wav.write_all(&bytes).unwrap();
    }

    let page = hex_engine::STREAM_PAGE_FRAMES as u64;
    let start = LONG_FRAMES % page - RANGE_FRAMES / 2;
    let end = start + RANGE_FRAMES;
    // The reverse cursor starts in the last source page and crosses back into
    // the preceding page. The resident tail has the same relative coordinates.
    assert_ne!((LONG_FRAMES - 1 - start) / page, (LONG_FRAMES - end) / page);
    let oracle: Vec<_> = ramp
        .iter()
        .rev()
        .skip(start as usize)
        .take(RANGE_FRAMES as usize)
        .map(|sample| [f32::from(*sample) / 32_768.0; 2])
        .collect();
    let resident = render(&short, start, false, output);
    let streamed = render(&long, start, true, output);
    assert_eq!(resident.len(), RANGE_FRAMES as usize);
    assert_eq!(streamed.len(), RANGE_FRAMES as usize);
    assert_matches(&resident, &oracle, "resident reverse source frame oracle");
    assert_matches(&streamed, &resident, "streamed reverse");
}

fn render(path: &Path, start: u64, streamed: bool, output: Output) -> Vec<hex_engine::Frame> {
    let mut h = ResidentHarness::with_frame_limit_at_rate(65_536, 48_000);
    h.next();
    let (applied, _) = h.applied(0);
    let imported = h.run(Request::ImportToNewTrack {
        path: path.to_path_buf(),
        before_track: None,
        start: 0,
    });
    let (applied, _) = h.applied(applied);
    let track = &imported.state.project.tracks()[0];
    let clip = &track.clips()[0];
    let asset = clip.asset();
    let context = imported.state.canonical_audio_source.as_ref().unwrap();
    let fingerprint = match &context.sources()[&asset] {
        AudioSourceBinding::VerifiedFile(file) => Some(*file.fingerprint()),
        AudioSourceBinding::Resident(_) => None,
    };
    assert_eq!(fingerprint.is_some(), streamed);
    let reversed = h.run(Request::Edit(Command::ReverseClips(vec![clip.id()])));
    let source = reversed.state.canonical_audio_source.as_ref().unwrap();
    if let Some(fingerprint) = fingerprint {
        let AudioSourceBinding::VerifiedFile(file) = &source.sources()[&asset] else {
            panic!("reversed clip must remain file-backed");
        };
        assert_eq!(*file.fingerprint(), fingerprint);
        assert_eq!(
            source
                .candidate_reservation_quote()
                .unwrap()
                .prepared_pcm_frames,
            0
        );
    }
    let (applied, _) = h.applied(applied);

    let pcm = match output {
        Output::Export => {
            let destination = path.with_extension("export.wav");
            let queued = h.run(Request::ExportRange {
                path: destination.clone(),
                start,
                frames: Some(RANGE_FRAMES),
                signal: ExportSignal::Main,
                control: ExportControl::default(),
                format: ExportFormat::Float32,
                overwrite: false,
            });
            let exported = completed(&mut h);
            assert_eq!(exported.revision, reversed.state.revision);
            assert_eq!(exported.result.unwrap().frames, RANGE_FRAMES);
            assert_eq!(queued.state.revision, reversed.state.revision);
            assert_eq!(queued.state.project, reversed.state.project);
            decoded(&destination)
        }
        Output::Bounce => {
            let target = TimeRange::new(vec![track.id()], start, start + RANGE_FRAMES).unwrap();
            let bounced = bounce(&mut h, &reversed, BounceTarget::Range(target), false);
            assert!(bounced.error.is_none(), "{:?}", bounced.error);
            let new_asset = added_asset(&reversed.state.project, &bounced.state.project);
            let pcm = decoded(bounced.state.project.asset(new_asset).unwrap().path());
            h.applied(applied);
            let undone = h.run(Request::Undo);
            assert_eq!(
                undone.state.project.tracks(),
                reversed.state.project.tracks()
            );
            assert!(undone.state.project.asset(new_asset).is_none());
            pcm
        }
    };
    if let Some(fingerprint) = fingerprint {
        let actual =
            hex_media::VerifiedWavReader::open(path, AudioPolicy::source_reservation().reader)
                .unwrap();
        assert_eq!(*actual.fingerprint(), fingerprint);
    }
    pcm
}
