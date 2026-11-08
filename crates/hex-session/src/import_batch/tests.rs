use super::*;
use hex_media::{Limits, ResampleLimits, decode_wav_file};
use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);

// Real file decode/conversion; delete the file before commit to prove history
// and playback use owned prepared PCM rather than reopening the foreign path.
fn media(rate: u32, frames: u32, value: i16) -> PreparedImport {
    let path = std::env::temp_dir().join(format!(
        "hex-batch-{}-{}.wav",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let bytes = frames * 2;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 2).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&bytes.to_le_bytes());
    for _ in 0..frames {
        wav.extend_from_slice(&value.to_le_bytes());
    }
    fs::write(&path, wav).unwrap();
    let decoded = decode_wav_file(&path, Limits::new(64 * 1024 * 1024, 8_000_000)).unwrap();
    let prepared = PreparedImport::at_rate(
        decoded,
        48_000,
        ResampleLimits {
            max_source_frames: 8_000_000,
            max_output_frames: 8_000_000,
        },
    )
    .unwrap();
    fs::remove_file(path).unwrap();
    prepared
}
fn session() -> Session {
    Session::new(48_000, 16, 8_000_000).unwrap()
}
fn track(s: &mut Session) -> TrackId {
    match s
        .editor
        .execute(Command::AddTrack {
            name: "target".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("expected track"),
    }
}
fn empty_destination() -> AudioBatchDestination {
    AudioBatchDestination::NewTracks {
        before_track: None,
        start: 19,
    }
}
fn import(
    s: &mut Session,
    members: Vec<PreparedImport>,
    dest: AudioBatchDestination,
) -> Vec<Created> {
    let expected = s.project().clone();
    s.import_prepared_batch_revision(members, dest, &expected, None, || Ok(()))
        .unwrap()
        .0
}
fn assert_unchanged(
    s: &Session,
    before: &Project,
    bank: usize,
    frames: u64,
    undo: bool,
    redo: bool,
) {
    assert_eq!(s.project(), before);
    assert_eq!(s.media.len(), bank);
    assert_eq!(s.resident_frames(), frames);
    assert_eq!(s.editor.can_undo(), undo);
    assert_eq!(s.editor.can_redo(), redo);
}

#[test]
fn ordered_tracks_pcm_paths_and_whole_batch_undo_redo_without_files() {
    let mut s = session();
    let anchor = track(&mut s);
    let before = s.project().clone();
    let a = media(48_000, 12, 8192);
    let path_a = a.path.clone();
    let b = media(24_000, 10, -8192);
    let path_b = b.path.clone();
    let created = import(
        &mut s,
        vec![a, b],
        AudioBatchDestination::NewTracks {
            before_track: Some(anchor),
            start: 17,
        },
    );
    let mut ids = Vec::new();
    for result in &created {
        let Created::Imported { track, asset, clip } = *result else {
            panic!()
        };
        ids.push((track, asset, clip));
    }
    assert_eq!(
        s.project()
            .tracks()
            .iter()
            .map(|t| t.id())
            .collect::<Vec<_>>(),
        vec![ids[0].0, ids[1].0, anchor]
    );
    assert_eq!(s.resident_frames(), 42);
    for (i, (_, asset, clip)) in ids.iter().enumerate() {
        let metadata = s.project().assets().find(|a| a.id() == *asset).unwrap();
        assert_eq!(
            metadata.path(),
            if i == 0 {
                path_a.as_path()
            } else {
                path_b.as_path()
            }
        );
        let audio = s.audio(*asset).unwrap();
        assert_eq!(audio.frames().len(), if i == 0 { 12 } else { 20 });
        assert_eq!(audio.sample_rate(), 48_000);
        assert!(
            s.project()
                .tracks()
                .iter()
                .flat_map(|t| t.clips())
                .find(|c| c.id() == *clip)
                .is_some_and(|c| c.start() == 17)
        );
    }
    let after = s.project().clone();
    assert!(s.undo());
    assert_eq!(s.project().tracks().len(), before.tracks().len());
    assert_eq!(s.resident_frames(), 42);
    assert!(s.redo());
    assert_eq!(s.project(), &after);
}

#[test]
fn lane_concatenation_uses_converted_lengths_and_existing_overwrite() {
    let mut s = session();
    let id = track(&mut s);
    import(
        &mut s,
        vec![media(48_000, 40, 100)],
        AudioBatchDestination::Track {
            track: id,
            start: 0,
        },
    );
    let before = s.project().clone();
    import(
        &mut s,
        vec![media(24_000, 4, 1000), media(48_000, 5, 2000)],
        AudioBatchDestination::Track {
            track: id,
            start: 10,
        },
    );
    let intervals: Vec<_> = s
        .project()
        .tracks()
        .iter()
        .flat_map(|t| t.clips())
        .map(|c| (c.start(), c.duration()))
        .collect();
    assert!(intervals.contains(&(10, 8)));
    assert!(intervals.contains(&(18, 5)));
    assert!(intervals.contains(&(0, 10)));
    assert!(intervals.contains(&(23, 17)));
    assert!(s.undo()); // Source IDs do not rewind, compare actual clips.
    assert_eq!(
        s.project()
            .tracks()
            .iter()
            .flat_map(|t| t.clips())
            .collect::<Vec<_>>(),
        before
            .tracks()
            .iter()
            .flat_map(|t| t.clips())
            .collect::<Vec<_>>()
    );
    assert!(s.redo());
}

#[test]
fn single_file_equivalent_to_existing_methods_for_both_destinations() {
    for lane in [false, true] {
        // Both import routes start from the same authored project. Cyan is the
        // only unused hue, so creation remains deterministic without replacing
        // production randomness or discarding colors from the comparison.
        let mut fixture = hex_project::Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
        for color in hex_project::ElementColor::TRACK_CHOICES
            .into_iter()
            .filter(|color| *color != hex_project::ElementColor::Cyan)
        {
            let Created::Track(track) = fixture
                .execute(hex_project::Command::AddTrack {
                    name: "authored color fixture".into(),
                })
                .unwrap()
            else {
                panic!("expected track")
            };
            fixture
                .execute(hex_project::Command::SetTrackColor { track, color })
                .unwrap();
        }
        let baseline = fixture.project().clone();
        let mut old = Session::from_loaded(baseline.clone(), 16, 8_000_000, Vec::new()).unwrap();
        let mut new = Session::from_loaded(baseline, 16, 8_000_000, Vec::new()).unwrap();
        let old_track = old.project().tracks()[0].id();
        let new_track = new.project().tracks()[0].id();
        let a = media(24_000, 6, 100);
        let mut b = media(24_000, 6, 100);
        b.path = a.path.clone();
        if lane {
            old.import_prepared_audio_clip_with_overwrite_revision(a, old_track, 31, None)
                .unwrap();
        } else {
            old.import_prepared_audio_track_at_revision(a, Some(old_track), 31, None)
                .unwrap();
        }
        let destination = if lane {
            AudioBatchDestination::Track {
                track: new_track,
                start: 31,
            }
        } else {
            AudioBatchDestination::NewTracks {
                before_track: Some(new_track),
                start: 31,
            }
        };
        import(&mut new, vec![b], destination);
        if !lane {
            assert_eq!(
                old.project().tracks()[0].color(),
                hex_project::ElementColor::Cyan
            );
        }
        assert_eq!(old.project(), new.project());
        assert_eq!(old.resident_frames(), new.resident_frames());
        assert_eq!(old.resident_waveform_bins(), new.resident_waveform_bins());
        assert_eq!(old.resident_waveform_bytes(), new.resident_waveform_bytes());
        let after = old.project().clone();
        assert!(old.undo() && new.undo());
        assert_eq!(
            old.project(),
            new.project(),
            "complete canonical undo states"
        );
        assert!(old.redo() && new.redo());
        assert_eq!(old.project(), &after);
        assert_eq!(new.project(), &after);
    }
}

#[test]
fn budget_includes_undo_media_and_later_member_refuses_atomically() {
    let mut s = Session::new(48_000, 16, 20).unwrap();
    import(&mut s, vec![media(48_000, 10, 1)], empty_destination());
    assert!(s.undo());
    let before = s.project().clone();
    let bins = s.resident_waveform_bins();
    assert!(
        s.import_prepared_batch_revision(
            vec![media(48_000, 5, 2), media(48_000, 6, 3)],
            empty_destination(),
            &before,
            None,
            || Ok(())
        )
        .is_err()
    );
    assert_unchanged(&s, &before, 1, 10, false, true);
    assert_eq!(s.resident_waveform_bins(), bins);
    assert!(s.redo());
}

#[test]
fn stale_snapshot_and_admission_cancellation_or_gesture_refusal_are_atomic() {
    let mut s = session();
    let stale = s.project().clone();
    track(&mut s);
    let current = s.project().clone();
    assert!(
        s.import_prepared_batch_revision(
            vec![media(48_000, 2, 1)],
            empty_destination(),
            &stale,
            None,
            || Ok(())
        )
        .is_err()
    );
    assert_unchanged(&s, &current, 0, 0, true, false);
    let mut calls = 0;
    assert!(
        s.import_prepared_batch_revision(
            vec![media(48_000, 2, 1)],
            empty_destination(),
            &current,
            None,
            || {
                calls += 1;
                if calls == 2 {
                    Err(SessionError::Invalid("audio batch canceled"))
                } else {
                    Ok(())
                }
            }
        )
        .is_err()
    );
    assert_eq!(calls, 2);
    assert_unchanged(&s, &current, 0, 0, true, false);
    assert!(
        s.import_prepared_batch_revision(
            vec![media(48_000, 2, 1)],
            empty_destination(),
            &current,
            None,
            || Err(SessionError::Invalid("active device gesture"))
        )
        .is_err()
    );
    assert_unchanged(&s, &current, 0, 0, true, false);
}

#[test]
fn metadata_failure_at_end_and_overflow_preserve_candidate_ids_and_redo() {
    let mut s = session();
    import(&mut s, vec![media(48_000, 2, 1)], empty_destination());
    assert!(s.undo());
    let before = s.project().clone();
    let mut bad = media(48_000, 2, 1);
    bad.conversion_version = u32::MAX;
    assert!(
        s.import_prepared_batch_revision(
            vec![media(48_000, 2, 1), bad],
            empty_destination(),
            &before,
            None,
            || Ok(())
        )
        .is_err()
    );
    assert_unchanged(&s, &before, 1, 2, false, true);
    assert!(
        s.import_prepared_batch_revision(
            vec![media(48_000, 2, 1)],
            AudioBatchDestination::NewTracks {
                before_track: None,
                start: u64::MAX
            },
            &before,
            None,
            || Ok(())
        )
        .is_err()
    );
    assert_unchanged(&s, &before, 1, 2, false, true);
}

#[test]
fn actual_unregistered_device_compile_failure_commits_no_batch() {
    let mut s = session();
    let id = track(&mut s);
    let state = hex_lattice_contract::NativeDeviceState::new(
        hex_lattice_contract::DeviceTypeId::from_static("test.unregistered.batch"),
        1,
        Vec::new(),
    )
    .unwrap();
    s.editor
        .execute(Command::InsertDevice {
            track: id,
            before: None,
            state,
        })
        .unwrap();
    let before = s.project().clone();
    assert!(matches!(
        s.import_prepared_batch_revision(
            vec![media(48_000, 3, 1)],
            empty_destination(),
            &before,
            None,
            || Ok(())
        ),
        Err(SessionError::Engine(_))
    ));
    assert_unchanged(&s, &before, 0, 0, true, false);
}

#[test]
fn empty_batch_refuses_without_history() {
    let mut s = session();
    let before = s.project().clone();
    assert!(
        s.import_prepared_batch_revision(Vec::new(), empty_destination(), &before, None, || Ok(()))
            .is_err()
    );
    assert_unchanged(&s, &before, 0, 0, false, false);
}

#[test]
fn duplicate_foreign_paths_are_not_deduplicated_and_pcm_association_is_ordered() {
    let mut s = session();
    let a = media(48_000, 4, 8192);
    let mut b = media(48_000, 4, -8192);
    b.path = a.path.clone();
    let created = import(&mut s, vec![a, b], empty_destination());
    let assets: Vec<_> = created
        .iter()
        .map(|r| match r {
            Created::Imported { asset, .. } => *asset,
            _ => panic!(),
        })
        .collect();
    assert_ne!(assets[0], assets[1]);
    assert!(s.audio(assets[0]).unwrap().frames()[1][0] > 0.0);
    assert!(s.audio(assets[1]).unwrap().frames()[1][0] < 0.0);
    assert_eq!(s.project().assets().count(), 2);
}

#[test]
fn exact_overwrite_fragment_limit_refuses_without_media_or_history_commit() {
    let mut s = session();
    let id = track(&mut s);
    import(
        &mut s,
        vec![media(48_000, 10, 1)],
        AudioBatchDestination::Track {
            track: id,
            start: 0,
        },
    );
    let mut snapshot = s.project().snapshot();
    let template = snapshot.tracks[0].audio_clips_mut().unwrap()[0].clone();
    for n in 1..hex_project::MAX_CLIPS {
        let mut clip = template.clone();
        clip.id = snapshot.next_id;
        snapshot.next_id += 1;
        clip.start = 100 + (n as u64) * 20;
        snapshot.tracks[0].audio_clips_mut().unwrap().push(clip);
    }
    let full = Project::from_snapshot(snapshot).unwrap();
    s.editor = hex_project::Editor::new(full, 16).unwrap();
    let before = s.project().clone();
    let result = s.import_prepared_batch_revision(
        vec![media(48_000, 2, 2)],
        AudioBatchDestination::Track {
            track: id,
            start: 3,
        },
        &before,
        None,
        || Ok(()),
    );
    assert!(matches!(result, Err(SessionError::Project(_))));
    assert_unchanged(&s, &before, 1, 10, false, false);
}

#[test]
fn track_capacity_and_waveform_overflow_refuse_atomically() {
    let mut s = session();
    track(&mut s);
    let mut snapshot = s.project().snapshot();
    let template = snapshot.tracks[0].clone();
    for _ in 1..hex_project::MAX_TRACKS {
        let mut t = template.clone();
        t.id = snapshot.next_id;
        snapshot.next_id += 1;
        if let Some(instrument) = t.instrument.as_mut() {
            instrument.id = snapshot.next_id;
            snapshot.next_id += 1;
        }
        for device in &mut t.devices {
            device.id = snapshot.next_id;
            snapshot.next_id += 1;
        }
        snapshot.tracks.push(t);
    }
    s.editor = hex_project::Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap();
    let before = s.project().clone();
    assert!(
        s.import_prepared_batch_revision(
            vec![media(48_000, 1, 1)],
            empty_destination(),
            &before,
            None,
            || Ok(())
        )
        .is_err()
    );
    assert_unchanged(&s, &before, 0, 0, false, false);
    let mut s = session();
    let before = s.project().clone();
    s.resident_waveform_bytes = usize::MAX;
    assert!(
        s.import_prepared_batch_revision(
            vec![media(48_000, 1, 1)],
            empty_destination(),
            &before,
            None,
            || Ok(())
        )
        .is_err()
    );
    assert_unchanged(&s, &before, 0, 0, false, false);
    assert_eq!(s.resident_waveform_bytes, usize::MAX);
}

#[test]
fn unsupported_metadata_in_middle_or_end_never_imports_other_members() {
    for rejected in [1, 2] {
        let mut s = session();
        let before = s.project().clone();
        let mut members = vec![
            media(48_000, 2, 1),
            media(48_000, 3, 2),
            media(48_000, 4, 3),
        ];
        members[rejected].conversion_version = u32::MAX;
        assert!(
            s.import_prepared_batch_revision(
                members,
                empty_destination(),
                &before,
                None,
                || Ok(())
            )
            .is_err()
        );
        assert_unchanged(&s, &before, 0, 0, false, false);
    }
}

#[test]
fn final_admission_refusal_preserves_exact_current_waveform_owners() {
    for warp in [
        hex_project::AudioImportWarp::Off,
        hex_project::AudioImportWarp::Beats,
    ] {
        let mut s = session();
        import(
            &mut s,
            vec![media(48_000, 24_000, 8192)],
            empty_destination(),
        );
        // Raw clips use their asset summary rather than a derived clip view.
        // Seed a real processed view so refusal must preserve an existing owner.
        let clip = s.project().tracks()[0].clips()[0].id();
        s.command_prepared_revision(Command::SetAudioClipPitch { clip, cents: 100 }, None)
            .unwrap();
        let before = s.project().clone();
        let waveforms = s.audio_clip_waveforms().unwrap();
        assert_eq!(waveforms.len(), 1);
        let frames = s.resident_frames();
        let bank = s.media.len();
        let cache_bytes = s.audio_transforms.cached_pcm_bytes().unwrap();
        let pending = media(48_000, 12_000, -4096);
        let rejected_pcm = pending.audio.clone();
        let mut calls = 0;
        let refused = s.import_prepared_batch_revision_using(
            vec![pending],
            empty_destination(),
            &before,
            None,
            AudioImportOptions {
                warp,
                new_track_gain: Some(0.25),
                ..Default::default()
            },
            || {
                calls += 1;
                if calls == 2 {
                    Err(SessionError::Invalid("injected final admission refusal"))
                } else {
                    Ok(())
                }
            },
            |input, admit| {
                assert_eq!(input.project.tracks().last().unwrap().gain(), 0.25);
                let staged = input.waveforms;
                prepare_asset_free(input, &mut || {
                    if warp == hex_project::AudioImportWarp::Beats {
                        assert!(
                            staged
                                .borrow()
                                .keys()
                                .any(|clip| !waveforms.contains_key(clip)),
                            "the final refusal must discard a genuinely staged new waveform owner"
                        );
                    }
                    admit()
                })
            },
        );
        assert!(refused.is_err());
        assert_eq!(calls, 2, "the refusal must follow completed preparation");
        assert_unchanged(&s, &before, bank, frames, true, false);
        let after = s.audio_clip_waveforms().unwrap();
        assert_eq!(
            after.keys().collect::<Vec<_>>(),
            waveforms.keys().collect::<Vec<_>>()
        );
        for (clip, old) in &waveforms {
            let current = &after[clip];
            assert!(Arc::ptr_eq(&old.waveform, &current.waveform));
            assert_eq!((current.offset, current.length), (old.offset, old.length));
        }
        assert!(
            after
                .values()
                .all(|view| !view.waveform.shares_audio_with(&rejected_pcm))
        );
        if warp == hex_project::AudioImportWarp::Off {
            // The raw refusal has no paid transform cache: no unpublished PCM
            // may escape through either canonical waveform or transform storage.
            assert_eq!(s.audio_transforms.cached_pcm_bytes().unwrap(), cache_bytes);
        }
        // Beats transforms retain their existing independently charged bounded
        // cache contract. This check targets the uncharged canonical display map.
    }
}
