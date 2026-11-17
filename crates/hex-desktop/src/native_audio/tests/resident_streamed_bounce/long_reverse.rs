//! Long reverse reads through the ordinary Session export and bounce owners.
use super::*;
use crate::native_audio::tests::resident_export::completed;
use hex_export::{ExportControl, ExportFormat};
use hex_project::{Command, Created};
use hex_session::service::ExportSignal;
use std::io::{Read, Seek, SeekFrom, Write};

const LONG_FRAMES: u64 = 20_000_000;
const TAIL_FRAMES: u64 = 8192;
const RANGE_FRAMES: u64 = 512;
const WAV_HEADER_BYTES: usize = 44;

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
    let short = files.0.join("resident.wav");
    let long = files.0.join("streamed.wav");
    let pattern: Vec<u8> = (0..TAIL_FRAMES)
        .map(|frame| (frame * 37 % 251 + 1) as u8)
        .collect();
    write_pcm8(&short, TAIL_FRAMES, &pattern);
    let header = write_pcm8(&long, LONG_FRAMES, &pattern);

    let page = hex_engine::STREAM_PAGE_FRAMES as u64;
    let start = LONG_FRAMES % page - RANGE_FRAMES / 2;
    let end = start + RANGE_FRAMES;
    // The reverse cursor starts in the last source page and crosses back into
    // the preceding page. The resident tail has the same relative coordinates.
    assert_ne!((LONG_FRAMES - 1 - start) / page, (LONG_FRAMES - end) / page);
    let oracle: Vec<_> = pattern
        .iter()
        .rev()
        .skip(start as usize)
        .take(RANGE_FRAMES as usize)
        .map(|sample| [(f32::from(*sample) - 128.0) / 128.0; 2])
        .collect();
    let resident = render_resident(&short, start);
    let streamed = render_streamed(&long, start, output);
    assert_eq!(resident.len(), RANGE_FRAMES as usize);
    assert_eq!(streamed.len(), RANGE_FRAMES as usize);
    assert_matches(&resident, &oracle, "resident reverse source frame oracle");
    assert_matches(&streamed, &resident, "streamed reverse");
    assert_original_bytes(&long, &header, &pattern);
}

fn write_pcm8(path: &Path, frames: u64, tail: &[u8]) -> Vec<u8> {
    let data_bytes = frames as u32;
    let mut header = Vec::from(*b"RIFF");
    header.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    header.extend_from_slice(b"WAVEfmt ");
    header.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    header.extend_from_slice(&1u16.to_le_bytes()); // linear PCM
    header.extend_from_slice(&1u16.to_le_bytes()); // mono
    header.extend_from_slice(&48_000u32.to_le_bytes()); // sample rate
    header.extend_from_slice(&48_000u32.to_le_bytes()); // byte rate
    header.extend_from_slice(&1u16.to_le_bytes()); // bytes per frame
    header.extend_from_slice(&8u16.to_le_bytes());
    header.extend_from_slice(b"data");
    header.extend_from_slice(&data_bytes.to_le_bytes());
    assert_eq!(header.len(), WAV_HEADER_BYTES);
    let mut wav = fs::File::create(path).unwrap();
    wav.write_all(&header).unwrap();
    wav.seek(SeekFrom::Start(
        WAV_HEADER_BYTES as u64 + frames - tail.len() as u64,
    ))
    .unwrap();
    wav.write_all(tail).unwrap();
    header
}

fn render_resident(path: &Path, start: u64) -> Vec<hex_engine::Frame> {
    let mut session = hex_session::Session::new(48_000, 1, 65_536).unwrap();
    let decoded = hex_media::decode_wav_file(
        path,
        hex_media::Limits::new(64 * 1024, TAIL_FRAMES as usize),
    )
    .unwrap();
    let prepared = hex_session::PreparedImport::new(decoded).unwrap();
    let Created::Imported { clip, .. } = session.import(prepared).unwrap() else {
        panic!("resident fixture must have a clip");
    };
    let (_, project) = session
        .command_prepared(Command::ReverseClips(vec![clip]))
        .unwrap();
    let mut playback = hex_engine::playback::Playback::new(project);
    playback.apply(PlaybackCommand::PlayFrom(start));
    let mut pcm = vec![[0.0; 2]; RANGE_FRAMES as usize];
    playback.render(&mut pcm).unwrap();
    pcm
}

fn assert_original_bytes(path: &Path, header: &[u8], tail: &[u8]) {
    let mut wav = fs::File::open(path).unwrap();
    assert_eq!(
        wav.metadata().unwrap().len(),
        WAV_HEADER_BYTES as u64 + LONG_FRAMES
    );
    let mut actual_header = [0; WAV_HEADER_BYTES];
    wav.read_exact(&mut actual_header).unwrap();
    assert_eq!(actual_header, header);
    let zeroes = [0; 65_536];
    let mut block = [0; 65_536];
    let mut remaining = LONG_FRAMES - TAIL_FRAMES;
    while remaining != 0 {
        let frames = remaining.min(block.len() as u64) as usize;
        wav.read_exact(&mut block[..frames]).unwrap();
        assert_eq!(block[..frames], zeroes[..frames]);
        remaining -= frames as u64;
    }
    let mut actual_tail = vec![0; tail.len()];
    wav.read_exact(&mut actual_tail).unwrap();
    assert_eq!(actual_tail, tail);
}

fn render_streamed(path: &Path, start: u64, output: Output) -> Vec<hex_engine::Frame> {
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
    let AudioSourceBinding::VerifiedFile(original) = &context.sources()[&asset] else {
        panic!("long source must remain file-backed");
    };
    let reversed = h.run(Request::Edit(Command::ReverseClips(vec![clip.id()])));
    let source = reversed.state.canonical_audio_source.as_ref().unwrap();
    let AudioSourceBinding::VerifiedFile(file) = &source.sources()[&asset] else {
        panic!("reversed clip must remain file-backed");
    };
    assert_eq!(file.fingerprint(), original.fingerprint());
    assert_eq!(
        source
            .candidate_reservation_quote()
            .unwrap()
            .prepared_pcm_frames,
        0
    );
    let (applied, _) = h.applied(applied);

    match output {
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
    }
}
