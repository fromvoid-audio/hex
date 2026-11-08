//! Rendered audio grouping uses the real float WAV render/adoption and finite
//! playback laws, not a synthetic event-flattening oracle.
use super::*;
use hex_project::{AudioViewResizeOperation, ClipFade, TimeRange};
pub(super) fn write_fixture(path: &Path, rate: u32, frames: &[hex_engine::Frame]) {
    use std::io::Write;
    let bytes = (frames.len() * 8) as u32;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap();
    for part in [
        b"RIFF".as_slice(),
        &(36 + bytes).to_le_bytes(),
        b"WAVEfmt ",
        &16_u32.to_le_bytes(),
        &3_u16.to_le_bytes(),
        &2_u16.to_le_bytes(),
        &rate.to_le_bytes(),
        &(rate * 8).to_le_bytes(),
        &8_u16.to_le_bytes(),
        &32_u16.to_le_bytes(),
        b"data",
        &bytes.to_le_bytes(),
    ] {
        f.write_all(part).unwrap();
    }
    for frame in frames {
        for sample in frame {
            f.write_all(&sample.to_le_bytes()).unwrap();
        }
    }
}
fn audio_fixture(rate: u32) -> (Folder, Session, TrackId, ClipId, ClipId, Vec<PathBuf>) {
    let folder = Folder::new();
    let mut s = Session::new(rate, 32, 100_000).unwrap();
    let mut track = None;
    let mut clips = Vec::new();
    let mut paths = Vec::new();
    for (i, length) in [64, 48, 37].into_iter().enumerate() {
        let path = folder.0.join(format!("original-{i}.wav"));
        let samples = (0..length)
            .map(|n| {
                let x = ((n as f32 + 1.0) * 0.137).sin() * 0.31;
                [x, x * -0.6]
            })
            .collect::<Vec<_>>();
        write_fixture(&path, rate, &samples);
        let audio = Audio::new(rate, samples).unwrap();
        let prepared = PreparedImport {
            path: path.clone(),
            source_sample_rate: rate,
            source_frames: length as u64,
            conversion_version: hex_project::IDENTITY_CONVERSION,
            source_audio: audio.clone(),
            waveform: waveform::summarize(&audio, length).unwrap(),
            audio,
            file_identity: None,
        };
        let (created, _) = s.import_prepared_revision(prepared, None).unwrap();
        let Created::Imported { track: t, clip, .. } = created else {
            panic!()
        };
        let owner = *track.get_or_insert(t);
        s.command(Command::MoveClip {
            clip,
            track: owner,
            start: [0, 80, 355][i],
        })
        .unwrap();
        clips.push(clip);
        paths.push(path);
    }
    let track = track.unwrap();
    let Created::Clip(group) = s.command(Command::GroupClips(clips[..2].to_vec())).unwrap() else {
        panic!()
    };
    s.command(Command::ResizeAudioClipView {
        clip: group,
        operation: AudioViewResizeOperation::RepeatOuterEnd { project_frame: 333 },
    })
    .unwrap();
    s.command(Command::SetAudioClipGain {
        clip: group,
        millidb: -3111,
    })
    .unwrap();
    s.command(Command::SetClipFadeEnvelope {
        clip: group,
        fade: ClipFade::new(333, 0, 17, 23).unwrap(),
    })
    .unwrap();
    (folder, s, track, group, clips[2], paths)
}
fn close(a: &[hex_engine::Frame], b: &[hex_engine::Frame]) {
    assert_eq!(a.len(), b.len());
    for (i, (a, b)) in a.iter().zip(b).enumerate() {
        for c in 0..2 {
            assert!((a[c] - b[c]).abs() < 2e-6, "frame {i}: {a:?}/{b:?}");
        }
    }
}

#[test]
fn nested_group_reopens_with_the_same_sound_and_reactivates_its_source() {
    let (folder, mut session, _, group, extra, _) = audio_fixture(48_000);
    let inner_event = session.project().clip(group).unwrap().audio_events().events[1].id;
    let set_active = |active| Command::EditAudioEvents {
        clip: group,
        operation: hex_project::AudioEventEdit::SetActive {
            events: vec![inner_event],
            active,
        },
    };
    session.command(set_active(false)).unwrap();
    let muted = pcm(session.compile_prepared_revision(None).unwrap(), 440);
    session.command(set_active(true)).unwrap();
    let active = pcm(session.compile_prepared_revision(None).unwrap(), 440);
    assert_ne!(active, muted);
    session.command(set_active(false)).unwrap();
    let Created::Clip(outer) = session
        .command(Command::GroupClips(vec![group, extra]))
        .unwrap()
    else {
        panic!("group")
    };
    close(
        &pcm(session.compile_prepared_revision(None).unwrap(), 440),
        &muted,
    );
    let path = folder.0.join("nested.hex");
    hex_storage::save_new(&path, session.project()).unwrap();
    let loaded = hex_storage::load(&path).unwrap();
    let audio = loaded
        .assets()
        .map(|asset| {
            (
                asset.id(),
                hex_media::decode_wav_file(
                    asset.path(),
                    hex_media::Limits::new(1024 * 1024, 100_000),
                )
                .unwrap(),
            )
        })
        .collect();
    let mut reopened = Session::from_loaded(loaded, 32, 100_000, audio).unwrap();
    close(
        &pcm(reopened.compile_prepared_revision(None).unwrap(), 440),
        &muted,
    );
    let source = reopened
        .project()
        .clip(outer)
        .unwrap()
        .audio_events()
        .sources
        .iter()
        .find(|root| root.clip.has_retained_audio_roots())
        .unwrap()
        .id;
    reopened
        .command(Command::EditAudioEvents {
            clip: outer,
            operation: hex_project::AudioEventEdit::SetRetainedPathActive {
                sources: vec![source],
                events: vec![inner_event],
                active: true,
            },
        })
        .unwrap();
    close(
        &pcm(reopened.compile_prepared_revision(None).unwrap(), 440),
        &active,
    );
    assert!(reopened.undo());
    close(
        &pcm(reopened.compile_prepared_revision(None).unwrap(), 440),
        &muted,
    );
}
#[test]
fn looping_group_render_preserves_partial_edges_direction_gaps_processing_history_and_storage() {
    for rate in [8000, 44100, 48000] {
        for reverse in [false, true] {
            let (folder, mut s, track, group, extra, paths) = audio_fixture(rate);
            if reverse {
                s.command(Command::ReverseClips(vec![group])).unwrap();
            }
            // Track processing must stay live rather than being baked twice.
            s.command(Command::Gain { track, gain: 0.37 }).unwrap();
            let state = hex_engine::default_native_device_state_for_class(
                hex_engine::NativeDeviceClass::Effect,
                hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
            )
            .unwrap()
            .with_output_gain_db(-7.0)
            .unwrap();
            s.command(Command::InsertDevice {
                track,
                before: None,
                state,
            })
            .unwrap();
            let original_files: Vec<_> = paths.iter().map(|p| fs::read(p).unwrap()).collect();
            let before = s.project().snapshot();
            let previous = s.compile_prepared_revision(None).unwrap();
            let expected = pcm(previous.clone(), 440);
            let target = s
                .project()
                .audio_group_render_target(&Command::GroupClips(vec![group, extra]))
                .unwrap()
                .unwrap();
            let (Created::Clip(result), prepared) = s
                .bounce_arrangement(
                    target,
                    true,
                    &folder.0,
                    &hex_export::ExportControl::default(),
                    Some(&previous),
                )
                .unwrap()
            else {
                panic!()
            };
            assert!(!s.project().clip(result).unwrap().looping());
            assert_eq!(s.project().clip(result).unwrap().duration(), 392);
            assert_eq!(s.project().track(track).unwrap().devices().len(), 1);
            close(&pcm(prepared.clone(), 440), &expected);
            let after = s.project().snapshot();
            let undone = s.undo_prepared_revision(Some(&prepared)).unwrap().unwrap();
            let mut restored = s.project().snapshot();
            restored.next_id = before.next_id;
            assert_eq!(restored, before);
            close(&pcm(undone.clone(), 440), &expected);
            let redone = s.redo_prepared_revision(Some(&undone)).unwrap().unwrap();
            assert_eq!(s.project().snapshot(), after);
            close(&pcm(redone, 440), &expected);
            let bytes = hex_storage::encode(s.project()).unwrap();
            assert_eq!(hex_storage::decode(&bytes).unwrap().snapshot(), after);
            for (p, raw) in paths.iter().zip(original_files) {
                assert_eq!(fs::read(p).unwrap(), raw);
            }
        }
    }
}
#[test]
fn grouping_time_span_preserves_exterior_fragments_and_cancellation_is_atomic() {
    let (folder, mut s, track, group, _, _) = audio_fixture(48000);
    let range = TimeRange::new(vec![track], 11, 321).unwrap();
    let target = s
        .project()
        .audio_group_render_target(&Command::GroupTimeRange(range))
        .unwrap()
        .unwrap();
    let before = s.project().snapshot();
    let history = (s.can_undo(), s.can_redo());
    let previous = s.compile_prepared_revision(None).unwrap();
    let expected = pcm(previous.clone(), 440);
    let control = hex_export::ExportControl::default();
    assert!(control.cancel());
    assert!(
        s.bounce_arrangement(target.clone(), true, &folder.0, &control, Some(&previous))
            .is_err()
    );
    assert_eq!(s.project().snapshot(), before);
    assert_eq!((s.can_undo(), s.can_redo()), history);
    let (Created::Clip(result), prepared) = s
        .bounce_arrangement(
            target,
            true,
            &folder.0,
            &hex_export::ExportControl::default(),
            Some(&previous),
        )
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        (
            s.project().clip(result).unwrap().start(),
            s.project().clip(result).unwrap().duration()
        ),
        (11, 310)
    );
    assert!(s.project().clip(group).is_some());
    close(&pcm(prepared, 440), &expected);
}

#[test]
fn grouping_final_preparation_failure_removes_unpublished_render_and_preserves_custody() {
    let (folder, mut s, _, group, extra, paths) = audio_fixture(48000);
    let target = s
        .project()
        .audio_group_render_target(&Command::GroupClips(vec![group, extra]))
        .unwrap()
        .unwrap();
    let captured = target.clone();
    let before = s.project().snapshot();
    let resident = s.resident_frames;
    let mut rendered = None;
    let result: Result<(), String> = s.bounce_arrangement_using(
        target,
        true,
        &folder.0,
        &hex_export::ExportControl::default(),
        |session, project, _| {
            let waveforms =
                std::cell::RefCell::new(session.audio_clip_waveforms().map_err(|e| e.to_string())?);
            crate::prepare_with_waveforms(
                project,
                &session.media_bank(),
                session.sine_table(),
                None,
                &session.audio_transforms,
                &waveforms,
            )
            .map(Arc::new)
            .map_err(|e| e.to_string())
        },
        |path, revision, item, tails, control| {
            assert!(!tails);
            let report = hex_export::render_arrangement_bounce_new(
                path,
                revision,
                None,
                item.start,
                item.end - item.start,
                tails,
                control,
            )
            .map_err(|e| e.to_string())?;
            rendered = Some(path.to_owned());
            Ok(report.frames)
        },
        |session, prepared, files| {
            session
                .adopt_arrangement_bounce_using(
                    captured,
                    true,
                    hex_project::BounceAdoption::NewTracks,
                    files,
                    prepared,
                    None,
                    |_| Err::<(), _>(SessionError::Invalid("injected final refusal")),
                )
                .map(|_| ())
                .map_err(|e| e.to_string())
        },
    );
    assert!(result.unwrap_err().contains("injected final refusal"));
    assert!(!rendered.unwrap().exists());
    assert!(paths.iter().all(|p| p.exists()));
    assert_eq!(s.project().snapshot(), before);
    assert_eq!(s.resident_frames, resident);
}

#[test]
fn inactive_group_keeps_latent_audio_and_activation_after_render() {
    let (folder, mut s, _, group, extra, _) = audio_fixture(48000);
    let expected = pcm(s.compile_prepared_revision(None).unwrap(), 440);
    for clip in [group, extra] {
        s.command(Command::SetClipActive {
            clip,
            active: false,
        })
        .unwrap();
    }
    let target = s
        .project()
        .audio_group_render_target(&Command::GroupClips(vec![group, extra]))
        .unwrap()
        .unwrap();
    let (Created::Clip(result), revision) = s
        .bounce_arrangement(
            target,
            true,
            &folder.0,
            &hex_export::ExportControl::default(),
            None,
        )
        .unwrap()
    else {
        panic!()
    };
    assert!(!s.project().clip(result).unwrap().active());
    assert!(pcm(revision, 440).iter().all(|f| *f == [0.0; 2]));
    s.command(Command::SetClipActive {
        clip: result,
        active: true,
    })
    .unwrap();
    close(
        &pcm(s.compile_prepared_revision(None).unwrap(), 440),
        &expected,
    );
}
