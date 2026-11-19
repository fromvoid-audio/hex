//! Pulse #3949: sources past the per-file resident decode caps stream, and the
//! operations that would turn a streamed clip into resident PCM refuse cleanly.
use super::*;
use crate::{AudioSourceBinding, DocumentAudioMode, SourcePlanReservation};
use std::{fs, io::Write, sync::atomic::AtomicU64};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-long-source-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    /// Sparse 8-bit mono PCM: long extents without a large construction buffer.
    fn long_wav(&self, name: &str, frames: u32) -> PathBuf {
        let path = self.0.join(name);
        let padded = frames + frames % 2;
        let mut header = Vec::from(*b"RIFF");
        header.extend_from_slice(&(36 + padded).to_le_bytes());
        header.extend_from_slice(b"WAVEfmt ");
        header.extend_from_slice(&16u32.to_le_bytes());
        header.extend_from_slice(&1u16.to_le_bytes());
        header.extend_from_slice(&1u16.to_le_bytes());
        header.extend_from_slice(&48_000u32.to_le_bytes());
        header.extend_from_slice(&48_000u32.to_le_bytes());
        header.extend_from_slice(&1u16.to_le_bytes());
        header.extend_from_slice(&8u16.to_le_bytes());
        header.extend_from_slice(b"data");
        header.extend_from_slice(&frames.to_le_bytes());
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(&header).unwrap();
        file.set_len(44 + u64::from(padded)).unwrap();
        path
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
/// The streaming contract hex-desktop reserves, not the resident decode caps.
fn streaming_reservation() -> SourcePlanReservation {
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
        max_retained_sources: 4,
        max_path_bytes: 1024 * 1024,
        max_manifest_bytes: 4 * 1024 * 1024,
    }
    .validate()
    .unwrap()
}
fn next(service: &Service) -> Event {
    service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap()
}
fn service_in(mode: DocumentAudioMode) -> Service {
    // The resident working set is far larger than the file: only the per-file
    // decode caps (64 MiB / 8M frames) stand between it and resident PCM.
    let service = Service::new(48_000, 16, 64_000_000).unwrap();
    let _ = next(&service);
    service
        .submit(Request::NewWithAudioMode {
            mode,
            reservation: Some(streaming_reservation()),
        })
        .unwrap();
    let new = next(&service);
    assert!(new.error.is_none(), "{:?}", new.error);
    service
}
fn streamed_frames(event: &Event) -> u64 {
    let snapshot = event.state.canonical_audio_source.as_ref().unwrap();
    let AudioSourceBinding::VerifiedFile(source) = snapshot.sources().values().next().unwrap()
    else {
        panic!("a streamed binding is expected, not resident pcm")
    };
    source.source_frames()
}
fn save_and_reopen(service: &Service, path: PathBuf) -> Event {
    service.submit(Request::SaveNew(path.clone())).unwrap();
    let save = next(service);
    assert!(save.error.is_none(), "{:?}", save.error);
    service.submit(Request::Open(path)).unwrap();
    let reopened = next(service);
    assert!(reopened.error.is_none(), "{:?}", reopened.error);
    reopened
}

/// One import serves every resident-document check: each long-source waveform
/// pass dominates this test's time in unoptimized builds.
#[test]
fn resident_document_streams_long_source_reopens_it_and_refuses_resident_bounces() {
    let files = Files::new();
    // Past both the 8M-frame resident decode cap and the 8M-frame bounce budget.
    let frames = u32::try_from(
        IMPORT_FRAME_LIMIT
            .max(usize::try_from(hex_project::MAX_ARRANGEMENT_BOUNCE_FRAMES).unwrap()),
    )
    .unwrap()
        + 48_000;
    let path = files.long_wav("long.wav", frames);
    let original = fs::read(&path).unwrap();
    let service = service_in(DocumentAudioMode::Resident);
    service.submit(Request::Import(path.clone())).unwrap();
    let imported = next(&service);
    assert!(
        imported.error.is_none(),
        "a source past the resident decode caps must stream: {:?}",
        imported.error
    );
    let clip = &imported.state.project.tracks()[0].clips()[0];
    assert_eq!((clip.start(), clip.duration()), (0, u64::from(frames)));
    let snapshot = imported.state.canonical_audio_source.as_ref().unwrap();
    assert_eq!(snapshot.audio_mode(), DocumentAudioMode::Resident);
    assert_eq!(streamed_frames(&imported), u64::from(frames));
    let generation = snapshot.context().project_generation;
    let track = imported.state.project.tracks()[0].id();
    let clip = clip.id();
    let whole = hex_project::TimeRange::new(vec![track], 0, u64::from(frames)).unwrap();
    // Bounce, clip bounce and grouping/consolidation all render resident PCM.
    for (target, in_place) in [
        (hex_project::BounceTarget::Track(track), true),
        (hex_project::BounceTarget::Clips(vec![clip]), false),
        (hex_project::BounceTarget::Range(whole.clone()), false),
        (
            hex_project::BounceTarget::AudioGroup {
                clips: vec![clip],
                range: Some(whole.clone()),
            },
            true,
        ),
    ] {
        service
            .submit(Request::BounceArrangement {
                project_generation: generation,
                revision: imported.state.revision,
                target: target.clone(),
                in_place,
                adoption: hex_project::BounceAdoption::NewTracks,
                control: hex_export::ExportControl::default(),
            })
            .unwrap();
        let refused = next(&service);
        assert!(
            refused
                .error
                .as_deref()
                .is_some_and(|e| e.contains("frame budget")),
            "{target:?} must refuse against the resident budget, got {:?}",
            refused.error
        );
        assert_eq!(refused.state.project, imported.state.project);
        assert_eq!(refused.state.revision, imported.state.revision);
    }
    // Reopening keeps the source file-backed although the working set would fit.
    let reopened = save_and_reopen(&service, files.0.join("long.hex"));
    assert_eq!(
        reopened.state.project.snapshot(),
        imported.state.project.snapshot()
    );
    assert_eq!(streamed_frames(&reopened), u64::from(frames));
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn streamed_source_past_the_edit_domain_imports_and_reopens_complete() {
    // A six-minute 48 kHz original exceeds the 16,777,216-frame edit domain.
    // Plain placement and playback do not need that domain; domain edits keep
    // their own explicit refusals.
    let files = Files::new();
    let frames = u32::try_from(hex_project::MAX_AUDIO_DOMAIN_FRAMES).unwrap() + 1;
    let path = files.long_wav("six-minutes.wav", frames);
    let service = service_in(DocumentAudioMode::VerifiedStreaming);
    service.submit(Request::Import(path)).unwrap();
    let imported = next(&service);
    assert!(imported.error.is_none(), "{:?}", imported.error);
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    assert_eq!(
        imported.state.project.clip(clip).unwrap().duration(),
        u64::from(frames)
    );
    let reopened = save_and_reopen(&service, files.0.join("long.hex"));
    assert_eq!(
        reopened.state.project.snapshot(),
        imported.state.project.snapshot()
    );
    assert_eq!(streamed_frames(&reopened), u64::from(frames));
}

/// Pulse #4145: timeline edits on a streamed clip past the 16,777,216-frame
/// edit domain need no resident PCM, so trim, move and split succeed and
/// survive Undo/Redo and save/reopen with the binding still streamed.
#[test]
fn timeline_edits_on_a_streamed_clip_past_the_edit_domain_survive_history_and_reopen() {
    let files = Files::new();
    let frames = 20_000_000u32;
    assert!(u64::from(frames) > hex_project::MAX_AUDIO_DOMAIN_FRAMES);
    let path = files.long_wav("twenty-million.wav", frames);
    let service = service_in(DocumentAudioMode::VerifiedStreaming);
    service.submit(Request::Import(path)).unwrap();
    let imported = next(&service);
    assert!(imported.error.is_none(), "{:?}", imported.error);
    let track = imported.state.project.tracks()[0].id();
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    let edit = |command: hex_project::Command| {
        service.submit(Request::Edit(command)).unwrap();
        let event = next(&service);
        assert!(event.error.is_none(), "{:?}", event.error);
        event
    };
    let trimmed = edit(hex_project::Command::TrimClipEdge {
        clip,
        edge: hex_project::TrimEdge::Right,
        frame: 19_000_000,
    });
    assert_eq!(
        trimmed.state.project.clip(clip).unwrap().duration(),
        19_000_000
    );
    let moved = edit(hex_project::Command::MoveClip {
        clip,
        track,
        start: 48_000,
    });
    assert_eq!(moved.state.project.clip(clip).unwrap().start(), 48_000);
    let split = edit(hex_project::Command::SplitClip {
        clip,
        frame: 10_000_000,
    });
    let clips = split.state.project.tracks()[0].clips();
    assert_eq!(clips.len(), 2);
    assert_eq!(
        clips.iter().map(|clip| clip.duration()).sum::<u64>(),
        19_000_000
    );
    let history = |request: Request| {
        service.submit(request).unwrap();
        let event = next(&service);
        assert!(event.error.is_none(), "{:?}", event.error);
        event
    };
    for _ in 0..3 {
        history(Request::Undo);
    }
    // Clip identities are never reused, so history compares the clip layout.
    let layout = |event: &Event| {
        event.state.project.tracks()[0]
            .clips()
            .iter()
            .map(|clip| (clip.start(), clip.duration(), clip.source_offset()))
            .collect::<Vec<_>>()
    };
    assert_eq!(layout(&history(Request::Redo)), layout(&trimmed));
    assert_eq!(layout(&history(Request::Redo)), layout(&moved));
    assert_eq!(layout(&history(Request::Redo)), layout(&split));
    let reopened = save_and_reopen(&service, files.0.join("edited.hex"));
    assert_eq!(
        reopened.state.project.snapshot(),
        split.state.project.snapshot()
    );
    assert_eq!(streamed_frames(&reopened), u64::from(frames));
}

// Each case owns a real 20M-frame source and the same imported state.
fn long_plain_fixture() -> (Files, Service, Event, hex_project::ClipId) {
    let files = Files::new();
    let path = files.long_wav("twenty-million.wav", 20_000_000);
    let service = service_in(DocumentAudioMode::VerifiedStreaming);
    service.submit(Request::Import(path)).unwrap();
    let imported = next(&service);
    assert!(imported.error.is_none(), "{:?}", imported.error);
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    service
        .submit(Request::Edit(hex_project::Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        }))
        .unwrap();
    let imported = next(&service);
    assert!(imported.error.is_none(), "{:?}", imported.error);
    (files, service, imported, clip)
}

#[test]
fn long_plain_original_reopens_after_verified_pitch() {
    let (files, service, imported, clip) = long_plain_fixture();
    let project_path = files.0.join("long-reversed.hex");
    service
        .submit(Request::SaveNew(project_path.clone()))
        .unwrap();
    let saved_base = next(&service);
    assert!(saved_base.error.is_none(), "{:?}", saved_base.error);
    service
        .submit(Request::Edit(hex_project::Command::SetAudioClipPitch {
            clip,
            cents: 100,
        }))
        .unwrap();
    let pitched = next(&service);
    assert!(pitched.error.is_none(), "{:?}", pitched.error);
    let pitch_path = files.0.join("long-pitch.hex");
    service
        .submit(Request::SaveNew(pitch_path.clone()))
        .unwrap();
    let saved_pitch = next(&service);
    assert!(saved_pitch.error.is_none(), "{:?}", saved_pitch.error);
    service
        .submit(Request::OpenWithAudioMode {
            source: ProjectSource::Saved(pitch_path),
            relinks: BTreeMap::new(),
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(streaming_reservation()),
        })
        .unwrap();
    let verified_pitch = next(&service);
    assert!(verified_pitch.error.is_none(), "{:?}", verified_pitch.error);
    assert_eq!(
        verified_pitch.state.project.snapshot(),
        pitched.state.project.snapshot()
    );
    assert_eq!(streamed_frames(&verified_pitch), 20_000_000);
    service.submit(Request::Open(project_path.clone())).unwrap();
    let restored = next(&service);
    assert!(restored.error.is_none(), "{:?}", restored.error);
    assert_eq!(
        restored.state.project.snapshot(),
        imported.state.project.snapshot()
    );
    assert_eq!(streamed_frames(&restored), 20_000_000);
}

/// Stretch and plain pitch keep the original file through history and reopen.
#[test]
fn long_plain_pitch_round_trips_after_stretch_undo() {
    use hex_project::{Command, TrimEdge, WarpMode};

    let (files, service, imported, clip) = long_plain_fixture();
    service
        .submit(Request::Edit(Command::SetAudioClipMusical {
            clip,
            enabled: true,
        }))
        .unwrap();
    let refused = next(&service);
    let error = refused.error.as_deref().expect("musical timing refusal");
    let expected = "processed reverse, pitch, warp and stretch support clips up to about 5:49 at 48 khz; trim or split the clip first";
    assert!(error.contains(expected), "{error}");
    assert_eq!(refused.state.project, imported.state.project);
    assert_eq!(refused.state.revision, imported.state.revision);

    let before = imported.state.project.clip(clip).unwrap();
    let frame = 19_000_000;
    service
        .submit(Request::Edit(Command::StretchAudioClipEdge {
            clip,
            edge: TrimEdge::Right,
            frame,
        }))
        .unwrap();
    let stretched = next(&service);
    assert!(stretched.error.is_none(), "{:?}", stretched.error);
    let edited = stretched.state.project.clip(clip).unwrap();
    assert_eq!(edited.duration(), frame);
    assert_eq!(edited.asset(), before.asset());
    let domain = &edited.audio_edit().unwrap().domain;
    assert!(domain.warp_enabled);
    assert_eq!(
        domain.warp.as_ref().unwrap().mode,
        WarpMode::Tones { grain_ms: 40 }
    );
    assert_eq!(streamed_frames(&stretched), 20_000_000);
    let original_source = imported.state.canonical_audio_source.as_ref().unwrap();
    let AudioSourceBinding::VerifiedFile(original_file) =
        &original_source.sources()[&before.asset()]
    else {
        panic!("expected the verified original file");
    };
    let stretched_source = stretched.state.canonical_audio_source.as_ref().unwrap();
    let mut roots = stretched_source.required_granular_roots(1);
    let (file, root) = roots.next().unwrap().unwrap();
    assert_eq!(file, original_file);
    assert_eq!(root.metadata().playback_frames(), frame);
    assert!(roots.next().is_none());

    service.submit(Request::Undo).unwrap();
    let undone = next(&service);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert_eq!(undone.state.project, imported.state.project);

    service
        .submit(Request::Edit(Command::SetAudioClipPitch {
            clip,
            cents: 100,
        }))
        .unwrap();
    let pitched = next(&service);
    assert!(pitched.error.is_none(), "{:?}", pitched.error);
    let pitch_snapshot = pitched.state.project.snapshot();
    assert_eq!(
        pitched
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .required_plain_pitch_sources()
            .count(),
        1
    );
    assert_eq!(streamed_frames(&pitched), 20_000_000);
    service.submit(Request::Undo).unwrap();
    let undo_pitch = next(&service);
    assert!(undo_pitch.error.is_none(), "{:?}", undo_pitch.error);
    assert_eq!(
        undo_pitch.state.project.snapshot(),
        imported.state.project.snapshot()
    );
    service.submit(Request::Redo).unwrap();
    let redo_pitch = next(&service);
    assert!(redo_pitch.error.is_none(), "{:?}", redo_pitch.error);
    assert_eq!(redo_pitch.state.project.snapshot(), pitch_snapshot);
    let pitch_path = files.0.join("long-pitch.hex");
    service
        .submit(Request::SaveNew(pitch_path.clone()))
        .unwrap();
    let saved_pitch = next(&service);
    assert!(saved_pitch.error.is_none(), "{:?}", saved_pitch.error);
    service.submit(Request::Open(pitch_path.clone())).unwrap();
    let ordinary_pitch = next(&service);
    assert!(ordinary_pitch.error.is_none(), "{:?}", ordinary_pitch.error);
    assert_eq!(ordinary_pitch.state.project.snapshot(), pitch_snapshot);
    assert_eq!(streamed_frames(&ordinary_pitch), 20_000_000);
    let before = pitched.state.canonical_audio_source.as_ref().unwrap();
    let after = ordinary_pitch
        .state
        .canonical_audio_source
        .as_ref()
        .unwrap();
    let (AudioSourceBinding::VerifiedFile(before), AudioSourceBinding::VerifiedFile(after)) = (
        before.sources().values().next().unwrap(),
        after.sources().values().next().unwrap(),
    ) else {
        panic!("verified file identity")
    };
    assert_eq!(before.fingerprint(), after.fingerprint());
    service
        .submit(Request::OpenWithAudioMode {
            source: ProjectSource::Saved(pitch_path),
            relinks: BTreeMap::new(),
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(streaming_reservation()),
        })
        .unwrap();
    let reopened_pitch = next(&service);
    assert!(reopened_pitch.error.is_none(), "{:?}", reopened_pitch.error);
    assert_eq!(reopened_pitch.state.project.snapshot(), pitch_snapshot);
    assert_eq!(streamed_frames(&reopened_pitch), 20_000_000);
}

/// Plain reverse remains streamed through history and format upgrade.
#[test]
fn long_plain_reverse_round_trips_while_other_transforms_refuse() {
    let (files, service, imported, clip) = long_plain_fixture();
    let project_path = files.0.join("long-reversed.hex");
    service
        .submit(Request::SaveNew(project_path.clone()))
        .unwrap();
    let saved_base = next(&service);
    assert!(saved_base.error.is_none(), "{:?}", saved_base.error);
    let original_project_bytes = fs::read(&project_path).unwrap();
    service
        .submit(Request::Edit(hex_project::Command::ReverseClips(vec![
            clip,
        ])))
        .unwrap();
    let reversed = next(&service);
    assert!(reversed.error.is_none(), "{:?}", reversed.error);
    let expected = reversed.state.project.snapshot();
    let edited = reversed.state.project.clip(clip).unwrap();
    assert_eq!(
        edited.audio_edit().unwrap().plain_reversed_source(edited),
        Some((0, 20_000_000))
    );
    assert_eq!(streamed_frames(&reversed), 20_000_000);
    for command in [
        hex_project::Command::SetAudioClipPitch { clip, cents: 100 },
        hex_project::Command::SetClipFades {
            clip,
            fade_in: 10,
            fade_out: 10,
        },
    ] {
        service.submit(Request::Edit(command)).unwrap();
        let refused = next(&service);
        assert!(refused.error.is_some());
        assert_eq!(refused.state.project.snapshot(), expected);
    }
    service.submit(Request::Undo).unwrap();
    let undone = next(&service);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert_eq!(
        undone.state.project.snapshot(),
        imported.state.project.snapshot()
    );
    service.submit(Request::Redo).unwrap();
    let redone = next(&service);
    assert!(redone.error.is_none(), "{:?}", redone.error);
    assert_eq!(redone.state.project.snapshot(), expected);
    service.submit(Request::SaveCurrent).unwrap();
    let refused = next(&service);
    assert!(refused.save_confirmation_required && refused.error.is_some());
    assert_eq!(fs::read(&project_path).unwrap(), original_project_bytes);
    let generation = redone
        .state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .context()
        .project_generation;
    service
        .submit(Request::SaveCurrentWithFormatUpgrade {
            project_generation: generation,
            path: project_path.clone(),
        })
        .unwrap();
    let saved = next(&service);
    assert!(saved.error.is_none(), "{:?}", saved.error);
    service.submit(Request::Open(project_path)).unwrap();
    let reopened = next(&service);
    assert!(reopened.error.is_none(), "{:?}", reopened.error);
    assert_eq!(reopened.state.project.snapshot(), expected);
    assert_eq!(streamed_frames(&reopened), 20_000_000);
}

#[test]
fn fifo_new_track_gain_applies_to_verified_file_imports() {
    let files = Files::new();
    let path = files.long_wav("gain.wav", 256);
    let service = service_in(DocumentAudioMode::VerifiedStreaming);
    service
        .submit(Request::ImportToNewTrackWithGain {
            path,
            before_track: None,
            start: 13,
            gain: 0.25,
        })
        .unwrap();
    let imported = next(&service);
    assert!(imported.error.is_none(), "{:?}", imported.error);
    let track = &imported.state.project.tracks()[0];
    assert_eq!(track.gain(), 0.25);
    assert_eq!(track.clips()[0].start(), 13);
    assert_eq!(streamed_frames(&imported), 256);
    service.submit(Request::Undo).unwrap();
    let undo = next(&service);
    assert!(undo.error.is_none(), "{:?}", undo.error);
    assert!(undo.state.project.tracks().is_empty());
    service.submit(Request::Redo).unwrap();
    let redo = next(&service);
    assert!(redo.error.is_none(), "{:?}", redo.error);
    assert_eq!(redo.state.project.tracks()[0].gain(), 0.25);
}

#[test]
fn long_repitch_edit_survives_history_and_reopen_without_resident_pcm() {
    let files = Files::new();
    let path = files.long_wav("long-repitch.wav", 20_000_000);
    let service = service_in(DocumentAudioMode::VerifiedStreaming);
    service.submit(Request::Import(path)).unwrap();
    let imported = next(&service);
    assert!(imported.error.is_none(), "{:?}", imported.error);
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    service
        .submit(Request::Edit(hex_project::Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        }))
        .unwrap();
    let neutral = next(&service);
    assert!(neutral.error.is_none(), "{:?}", neutral.error);
    let warp = hex_project::default_audio_warp(
        neutral.state.project.clip(clip).unwrap(),
        neutral.state.project.tempo_map(),
        hex_project::WarpMode::RePitch,
    )
    .unwrap();
    service
        .submit(Request::Edit(hex_project::Command::SetAudioClipWarp {
            clip,
            settings: Some(warp),
        }))
        .unwrap();
    let warped = next(&service);
    assert!(warped.error.is_none(), "{:?}", warped.error);
    let expected = warped.state.project.snapshot();
    assert_eq!(streamed_frames(&warped), 20_000_000);
    service
        .submit(Request::Edit(hex_project::Command::SetAudioClipPitch {
            clip,
            cents: 100,
        }))
        .unwrap();
    let refused = next(&service);
    assert!(refused.error.is_some());
    assert_eq!(refused.state.project.snapshot(), expected);
    let beats = hex_project::default_audio_warp(
        warped.state.project.clip(clip).unwrap(),
        warped.state.project.tempo_map(),
        hex_project::WarpMode::Beats {
            preserve: hex_project::BeatsPreserve::Transients,
            tail: hex_project::BeatsTail::Forward,
            decay: 100,
        },
    )
    .unwrap();
    service
        .submit(Request::Edit(hex_project::Command::SetAudioClipWarp {
            clip,
            settings: Some(beats),
        }))
        .unwrap();
    let refused = next(&service);
    assert!(refused.error.is_some());
    assert_eq!(refused.state.project.snapshot(), expected);
    service.submit(Request::Undo).unwrap();
    let undo = next(&service);
    assert!(undo.error.is_none(), "{:?}", undo.error);
    assert_eq!(
        undo.state.project.snapshot(),
        neutral.state.project.snapshot()
    );
    service.submit(Request::Redo).unwrap();
    let redo = next(&service);
    assert!(redo.error.is_none(), "{:?}", redo.error);
    assert_eq!(redo.state.project.snapshot(), expected);
    let reopened = save_and_reopen(&service, files.0.join("long-repitch.hex"));
    assert_eq!(reopened.state.project.snapshot(), expected);
    assert_eq!(streamed_frames(&reopened), 20_000_000);
}
