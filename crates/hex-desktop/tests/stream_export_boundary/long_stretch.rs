//! The ordinary Session edit, Main export and bounce of a stretched file.
use super::*;
use hex_export::{ExportControl, ExportFormat, ExportSignal};
use hex_project::{BounceAdoption, BounceTarget, Command, TimeRange, TrimEdge, WarpMode};
use hex_session::AudioSourceBinding;
use std::path::Path;

const RATE: u32 = 48_000;
const LONG_FRAMES: u64 = 20_000_000;
const SHARED_FRAMES: u32 = 24_000;
const OUTPUT_FRAMES: u64 = 512;
const FIRST: u64 = hex_engine::STREAM_PAGE_FRAMES as u64 - OUTPUT_FRAMES / 2;
const PCM_TOLERANCE: f32 = 1e-6;

#[test]
fn stretched_long_tones_matches_resident_main_export_and_bounce() {
    compare(
        WarpMode::Tones { grain_ms: 40 },
        None, // A fresh long clip selects Tones when its edge is stretched.
    );
}

#[test]
fn stretched_long_texture_matches_resident_main_export_and_bounce() {
    let texture = WarpMode::Texture {
        grain_ms: 32,
        fluctuation: 35,
        seed: 42,
    };
    compare(texture, Some(texture));
}

fn compare(mode: WarpMode, authored: Option<WarpMode>) {
    let files = Files::new();
    let short = varied(&files, "resident-prefix.wav", RATE, SHARED_FRAMES);
    let long = long_wav(&files, &short);
    let resident = Service::new(RATE, 16, 65_536).unwrap();
    event(&resident);
    let reference = run(&resident, Request::Import(short));
    assert!(matches!(
        reference
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .required_sources()
            .next()
            .unwrap()
            .1,
        AudioSourceBinding::Resident(_)
    ));
    let (reference, _) = stretch(
        &resident,
        reference,
        Some(mode),
        mode,
        u64::from(SHARED_FRAMES),
    );
    let expected = export_pcm(&resident, &reference, &files.0.join("reference.wav"));

    let source_reservation = AudioPolicy::source_reservation();
    let streamed = Service::with_audio_sink(
        16,
        65_536,
        Some(files.0.join("recovery/untitled.hex-journal")),
        Box::new(OfflineSink {
            rate: RATE,
            render: streamed_export::render,
        }),
    )
    .unwrap();
    event(&streamed);
    run(
        &streamed,
        Request::NewWithAudioMode {
            mode: DocumentAudioMode::Resident,
            reservation: Some(source_reservation),
        },
    );
    let imported = run(&streamed, Request::Import(long.clone()));
    let (stretched, before_stretch) = stretch(&streamed, imported, authored, mode, LONG_FRAMES);
    let project = &stretched.state.project;
    let track = &project.tracks()[0];
    let asset = track.clips()[0].asset();
    let source = stretched.state.canonical_audio_source.as_ref().unwrap();
    let AudioSourceBinding::VerifiedFile(file) = &source.sources()[&asset] else {
        panic!("stretch must keep the long original file-backed");
    };
    assert_eq!(
        source
            .candidate_reservation_quote()
            .unwrap()
            .prepared_pcm_frames,
        0
    );
    let mut roots = source.required_granular_roots(1);
    let (root_file, root) = roots.next().unwrap().unwrap();
    assert_eq!(root_file, file);
    assert_eq!(root.metadata().playback_frames(), LONG_FRAMES * 6 / 5);
    assert!(roots.next().is_none());

    let exported = export_pcm(&streamed, &stretched, &files.0.join("streamed-main.wav"));
    assert_pcm(&exported, &expected);
    let bounced = run(
        &streamed,
        Request::BounceArrangement {
            project_generation: source.context().project_generation,
            revision: stretched.state.revision,
            target: BounceTarget::Range(
                TimeRange::new(vec![track.id()], FIRST, FIRST + OUTPUT_FRAMES).unwrap(),
            ),
            in_place: false,
            adoption: BounceAdoption::NewTracks,
            control: ExportControl::default(),
        },
    );
    let mut new_assets = bounced
        .state
        .project
        .assets()
        .filter(|candidate| project.asset(candidate.id()).is_none());
    let bounce = new_assets.next().unwrap();
    assert!(new_assets.next().is_none());
    assert_pcm(&pcm(bounce.path()), &expected);

    let undone = run(&streamed, Request::Undo);
    assert_eq!(undone.state.project.tracks(), project.tracks());
    assert!(undone.state.project.asset(bounce.id()).is_none());
    let original = run(&streamed, Request::Undo);
    assert_eq!(
        original.state.project.tracks(),
        before_stretch.state.project.tracks()
    );
    let restored = original.state.canonical_audio_source.as_ref().unwrap();
    assert!(
        matches!(&restored.sources()[&asset], AudioSourceBinding::VerifiedFile(original)
        if original == file)
    );
    let verified = hex_media::VerifiedWavReader::open(long, source_reservation.reader).unwrap();
    assert_eq!(verified.fingerprint(), file.fingerprint());
}

fn stretch(
    service: &Service,
    mut before: Event,
    authored: Option<WarpMode>,
    mode: WarpMode,
    source_frames: u64,
) -> (Event, Event) {
    let clip = before.state.project.tracks()[0].clips()[0].id();
    if let Some(authored) = authored {
        let project = &before.state.project;
        let settings = hex_project::default_audio_warp(
            project.clip(clip).unwrap(),
            project.tempo_map(),
            authored,
        )
        .unwrap();
        before = run(
            service,
            Request::Edit(Command::SetAudioClipWarp {
                clip,
                settings: Some(settings),
            }),
        );
    }
    let frame = source_frames * 6 / 5;
    let after = run(
        service,
        Request::Edit(Command::StretchAudioClipEdge {
            clip,
            edge: TrimEdge::Right,
            frame,
        }),
    );
    let clip = after.state.project.clip(clip).unwrap();
    assert_eq!(clip.duration(), frame);
    let domain = &clip.audio_edit().unwrap().domain;
    assert!(domain.warp_enabled);
    assert_eq!(domain.warp.as_ref().unwrap().mode, mode);
    (after, before)
}

fn export_pcm(service: &Service, revision: &Event, destination: &Path) -> Vec<hex_engine::Frame> {
    let queued = run(
        service,
        Request::ExportRange {
            path: destination.to_path_buf(),
            start: FIRST,
            frames: Some(OUTPUT_FRAMES),
            signal: ExportSignal::Main,
            format: ExportFormat::Float32,
            overwrite: false,
            control: ExportControl::default(),
        },
    );
    assert_eq!(queued.state.revision, revision.state.revision);
    assert_eq!(queued.state.project, revision.state.project);
    let completion = exported(service);
    assert_eq!(completion.revision, revision.state.revision);
    assert_eq!(completion.result.unwrap().frames, OUTPUT_FRAMES);
    pcm(destination)
}

fn pcm(path: &Path) -> Vec<hex_engine::Frame> {
    hex_media::decode_wav_file(path, hex_media::Limits::new(1 << 20, 65_536))
        .unwrap()
        .audio()
        .frames()
        .to_vec()
}

fn assert_pcm(actual: &[hex_engine::Frame], expected: &[hex_engine::Frame]) {
    assert_eq!(expected.len(), OUTPUT_FRAMES as usize);
    assert_eq!(actual.len(), expected.len());
    assert!(expected.iter().flatten().any(|sample| sample.abs() > 0.01));
    let error = actual
        .iter()
        .flatten()
        .zip(expected.iter().flatten())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0_f32, f32::max);
    assert!(error <= PCM_TOLERANCE, "stretched PCM difference {error}");
}

fn long_wav(files: &Files, prefix: &Path) -> PathBuf {
    let mut bytes = fs::read(prefix).unwrap();
    let data_bytes = (LONG_FRAMES * 2) as u32;
    bytes[4..8].copy_from_slice(&(36 + data_bytes).to_le_bytes());
    bytes[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    let path = files.0.join("long.wav");
    fs::write(&path, bytes).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(44 + u64::from(data_bytes))
        .unwrap();
    path
}
