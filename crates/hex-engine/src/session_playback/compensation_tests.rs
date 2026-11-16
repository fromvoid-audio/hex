//! Whole requested-output transaction and exact content-origin tests.
use super::*;
use crate::signal_graph::tests::{fixture, prepare};
use crate::{Audio, SineTable};

#[test]
fn adding_an_empty_track_keeps_compensated_pcm_continuous() {
    use hex_project::Command;
    for partition in [1, 257] {
        let mut f = fixture(&[&[7]], 1024);
        for audio in f.media.values_mut() {
            *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
        }
        let revision = prepare(&f).unwrap();
        let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut reference = SessionPlayback::new(revision).unwrap();
        for runtime in [&mut old, &mut reference] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 32]).unwrap();
        }
        f.editor
            .execute(Command::AddTrack {
                name: "empty".into(),
            })
            .unwrap();
        let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        let mut actual = vec![[0.0; 2]; 257];
        let mut expected = actual.clone();
        for chunk in actual.chunks_mut(partition) {
            next.render(chunk).unwrap();
        }
        for chunk in expected.chunks_mut(partition) {
            reference.render(chunk).unwrap();
        }
        assert_eq!(actual, expected, "partition {partition}");
    }
}

#[test]
fn adding_future_audio_track_preserves_existing_compensated_pcm() {
    use hex_project::{Command, Created};
    for partition in [1, 257] {
        let mut f = fixture(&[&[7]], 2048);
        for audio in f.media.values_mut() {
            *audio = Audio::new(48_000, vec![[1.0, -0.5]; 2048]).unwrap();
        }
        let revision = prepare(&f).unwrap();
        let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut reference = SessionPlayback::new(revision).unwrap();
        for runtime in [&mut old, &mut reference] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 32]).unwrap();
        }
        let Created::Imported { track, asset, clip } = f
            .editor
            .execute(Command::ImportAudioTrack {
                name: "future".into(),
                path: "future.wav".into(),
                sample_rate: 48_000,
                frames: 1024,
            })
            .unwrap()
        else {
            panic!("import")
        };
        f.editor
            .execute(Command::MoveClip {
                clip,
                track,
                start: 1024,
            })
            .unwrap();
        f.media
            .insert(asset, Audio::new(48_000, vec![[0.25, 0.25]; 1024]).unwrap());
        let updated = prepare(&f).unwrap();
        let mut future_oracle = SessionPlayback::new(Arc::clone(&updated)).unwrap();
        future_oracle.apply(SessionPlaybackCommand::Play).unwrap();
        future_oracle.render(&mut [[0.0; 2]; 32]).unwrap();
        let mut next = SessionPlayback::new(updated).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        let mut actual = vec![[0.0; 2]; 1280];
        let mut expected = actual.clone();
        for chunk in actual.chunks_mut(partition) {
            next.render(chunk).unwrap();
        }
        for chunk in expected.chunks_mut(partition) {
            future_oracle.render(chunk).unwrap();
        }
        assert_eq!(actual, expected, "partition {partition}");
        let mut retained = [[0.0; 2]; 257];
        reference.render(&mut retained).unwrap();
        assert_eq!(&actual[..257], &retained);
        assert!(expected[1000..].iter().any(|frame| *frame != [1.0, -0.5]));
    }
}

#[test]
fn newly_added_clip_near_cut_keeps_compensated_onset_in_time() {
    use hex_project::{Command, Created};
    for start in [33, 34, 39] {
        for partition in [1, 257] {
            let mut f = fixture(&[&[7]], 512);
            for audio in f.media.values_mut() {
                *audio = Audio::new(48_000, vec![[0.5, -0.25]; 512]).unwrap();
            }
            let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
            old.apply(SessionPlaybackCommand::Play).unwrap();
            old.render(&mut [[0.0; 2]; 32]).unwrap();
            let Created::Imported { track, asset, clip } = f
                .editor
                .execute(Command::ImportAudioTrack {
                    name: "new".into(),
                    path: "near.wav".into(),
                    sample_rate: 48_000,
                    frames: 128,
                })
                .unwrap()
            else {
                panic!("import")
            };
            f.editor
                .execute(Command::MoveClip { clip, track, start })
                .unwrap();
            f.media
                .insert(asset, Audio::new(48_000, vec![[0.25; 2]; 128]).unwrap());
            let revision = prepare(&f).unwrap();
            let mut oracle = SessionPlayback::new(Arc::clone(&revision)).unwrap();
            oracle.apply(SessionPlaybackCommand::Play).unwrap();
            oracle.render(&mut [[0.0; 2]; 32]).unwrap();
            let mut next = SessionPlayback::new(revision).unwrap();
            next.try_inherit_state_from(&mut old).unwrap();
            let mut actual = [[0.0; 2]; 128];
            let mut expected = actual;
            for chunk in actual.chunks_mut(partition) {
                next.render(chunk).unwrap();
            }
            for chunk in expected.chunks_mut(partition) {
                oracle.render(chunk).unwrap();
            }
            assert_eq!(actual, expected, "start={start} partition={partition}");
        }
    }
}

#[test]
fn future_audio_handoff_keeps_unsafe_cases_on_restart_path() {
    use hex_project::{Command, Created};
    for case in [
        "active",
        "historical",
        "at_cut",
        "loop",
        "routed",
        "latency",
    ] {
        let mut f = fixture(&[&[7]], 2048);
        for audio in f.media.values_mut() {
            *audio = Audio::new(48_000, vec![[1.0, -0.5]; 2048]).unwrap();
        }
        let return_track = if case == "routed" {
            let Created::ReturnTrack(id) = f
                .editor
                .execute(Command::AddReturnTrack { name: "bus".into() })
                .unwrap()
            else {
                panic!("return")
            };
            Some(id)
        } else {
            None
        };
        if case == "loop" {
            f.editor
                .execute(Command::SetLoopRegion {
                    start: 0,
                    end: 64,
                    enabled: true,
                })
                .unwrap();
        }
        let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        old.apply(SessionPlaybackCommand::Play).unwrap();
        old.render(&mut [[0.0; 2]; 32]).unwrap();
        let frames = if case == "historical" { 16 } else { 1024 };
        let Created::Imported { track, asset, clip } = f
            .editor
            .execute(Command::ImportAudioTrack {
                name: "new".into(),
                path: "future.wav".into(),
                sample_rate: 48_000,
                frames,
            })
            .unwrap()
        else {
            panic!("import")
        };
        let start = match case {
            "active" | "historical" => 0,
            "loop" => 50,
            "at_cut" => 32,
            _ => 1024,
        };
        if start != 0 {
            f.editor
                .execute(Command::MoveClip { clip, track, start })
                .unwrap();
        }
        if let Some(return_track) = return_track {
            f.editor
                .execute(Command::SetReturnSend {
                    track,
                    return_track,
                    gain: 0.5,
                })
                .unwrap();
        }
        f.media.insert(
            asset,
            Audio::new(48_000, vec![[0.25; 2]; frames as usize]).unwrap(),
        );
        if case == "latency" {
            let device = f.editor.project().track(f.tracks[0]).unwrap().devices()[0].id();
            f.editor
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: crate::signal_graph::tests::state(8).unwrap(),
                })
                .unwrap();
        }
        let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        let mut actual = [[0.0; 2]; 7];
        next.render(&mut actual).unwrap();
        assert_eq!(actual, [[0.0; 2]; 7], "unsafe case {case}");
    }
}

#[test]
fn adding_an_unused_return_keeps_compensated_pcm_continuous() {
    use hex_project::Command;
    for partition in [1, 257] {
        let mut f = fixture(&[&[7]], 1024);
        for audio in f.media.values_mut() {
            *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
        }
        let revision = prepare(&f).unwrap();
        let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut reference = SessionPlayback::new(revision).unwrap();
        for runtime in [&mut old, &mut reference] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 32]).unwrap();
        }
        f.editor
            .execute(Command::AddReturnTrack {
                name: "unused".into(),
            })
            .unwrap();
        let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        let mut actual = vec![[0.0; 2]; 257];
        let mut expected = actual.clone();
        for chunk in actual.chunks_mut(partition) {
            next.render(chunk).unwrap();
        }
        for chunk in expected.chunks_mut(partition) {
            reference.render(chunk).unwrap();
        }
        assert_eq!(actual, expected, "partition {partition}");
    }
}

#[test]
fn adding_an_unused_return_preserves_a_sounding_return() {
    use hex_project::{Command, Created};
    for partition in [1, 257] {
        let mut f = fixture(&[&[7]], 1024);
        for audio in f.media.values_mut() {
            *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
        }
        let Created::ReturnTrack(bus) = f
            .editor
            .execute(Command::AddReturnTrack {
                name: "active".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        f.editor
            .execute(Command::SetReturnSend {
                track: f.tracks[0],
                return_track: bus,
                gain: 0.5,
            })
            .unwrap();
        let revision = prepare(&f).unwrap();
        let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut reference = SessionPlayback::new(revision).unwrap();
        for runtime in [&mut old, &mut reference] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 32]).unwrap();
        }
        f.editor
            .execute(Command::AddReturnTrack {
                name: "unused".into(),
            })
            .unwrap();
        let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        let mut actual = vec![[0.0; 2]; 257];
        let mut expected = actual.clone();
        for chunk in actual.chunks_mut(partition) {
            next.render(chunk).unwrap();
        }
        for chunk in expected.chunks_mut(partition) {
            reference.render(chunk).unwrap();
        }
        assert_eq!(actual, expected, "partition {partition}");
        assert!(expected.iter().any(|frame| frame[0] > 1.0));
    }
}

#[test]
fn changing_an_unused_return_preserves_an_existing_reverb_tail() {
    for remove in [false, true] {
        unused_return_reverb_tail_case(remove);
    }
}

fn unused_return_reverb_tail_case(remove: bool) {
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::{Command, Created, Editor, Project};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "impulse".into(),
            path: "memory.wav".into(),
            sample_rate: 48_000,
            frames: 12_000,
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: crate::default_native_device_state(crate::DeviceTypeId::from_static(
                "hex.limiter",
            ))
            .unwrap(),
        })
        .unwrap();
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack {
            name: "room".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 1.0,
        })
        .unwrap();
    let reverb = &hex_devices::reverb::REVERB_DEFINITION;
    let state = reverb
        .patch_parameter(
            &reverb.default_state().unwrap(),
            hex_devices::reverb::parameter::DRY_WET,
            ParameterValue::Float(100.0),
        )
        .unwrap();
    editor
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state,
        })
        .unwrap();
    let unused = if remove {
        let Created::ReturnTrack(id) = editor
            .execute(Command::AddReturnTrack {
                name: "unused".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        Some(id)
    } else {
        None
    };
    let mut samples = vec![[0.0; 2]; 12_000];
    samples[..512].fill([0.25, -0.125]);
    let media = std::collections::BTreeMap::from([(asset, Audio::new(48_000, samples).unwrap())]);
    let table = Arc::new(SineTable::new());
    let prepare = |project: &Project| {
        Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
    };
    let revision = prepare(editor.project());
    let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut reference = SessionPlayback::new(revision).unwrap();
    for runtime in [&mut old, &mut reference] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut vec![[0.0; 2]; 6000]).unwrap();
    }
    if let Some(id) = unused {
        editor.execute(Command::RemoveReturnTrack(id)).unwrap();
    } else {
        editor
            .execute(Command::AddReturnTrack {
                name: "unused".into(),
            })
            .unwrap();
    }
    let mut next = SessionPlayback::new(prepare(editor.project())).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut actual = [[0.0; 2]; 512];
    let mut expected = actual;
    next.render(&mut actual).unwrap();
    reference.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert!(
        expected.iter().flatten().any(|sample| sample.abs() > 1e-7),
        "reverb tail must be audible"
    );
}

#[test]
fn adding_a_routed_return_keeps_the_conservative_restart() {
    use hex_project::{Command, Created};
    let mut f = fixture(&[&[7]], 1024);
    for audio in f.media.values_mut() {
        *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
    }
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 32]).unwrap();
    let Created::ReturnTrack(bus) = f
        .editor
        .execute(Command::AddReturnTrack {
            name: "routed".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    f.editor
        .execute(Command::SetReturnSend {
            track: f.tracks[0],
            return_track: bus,
            gain: 0.5,
        })
        .unwrap();
    let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut actual = [[0.0; 2]; 16];
    next.render(&mut actual).unwrap();
    assert_eq!(&actual[..7], &[[0.0; 2]; 7]);
    assert!(actual[7][0] > 0.0);
}

#[test]
fn removing_an_unused_return_keeps_compensated_pcm_continuous() {
    use hex_project::{Command, Created};
    for partition in [1, 257] {
        let mut f = fixture(&[&[7]], 1024);
        for audio in f.media.values_mut() {
            *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
        }
        let Created::ReturnTrack(bus) = f
            .editor
            .execute(Command::AddReturnTrack {
                name: "unused".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        let revision = prepare(&f).unwrap();
        let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut reference = SessionPlayback::new(revision).unwrap();
        for runtime in [&mut old, &mut reference] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 32]).unwrap();
        }
        f.editor.execute(Command::RemoveReturnTrack(bus)).unwrap();
        let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        let mut actual = vec![[0.0; 2]; 257];
        let mut expected = actual.clone();
        for chunk in actual.chunks_mut(partition) {
            next.render(chunk).unwrap();
        }
        for chunk in expected.chunks_mut(partition) {
            reference.render(chunk).unwrap();
        }
        assert_eq!(actual, expected, "partition {partition}");
    }
}

#[test]
fn removing_and_undoing_an_earlier_unused_return_preserves_later_send() {
    use hex_project::{Command, Created};
    for partition in [1, 257] {
        let mut f = fixture(&[&[7]], 2048);
        for audio in f.media.values_mut() {
            *audio = Audio::new(48_000, vec![[1.0, -0.5]; 2048]).unwrap();
        }
        let Created::ReturnTrack(unused) = f
            .editor
            .execute(Command::AddReturnTrack {
                name: "unused".into(),
            })
            .unwrap()
        else {
            panic!("unused return")
        };
        let Created::ReturnTrack(active) = f
            .editor
            .execute(Command::AddReturnTrack {
                name: "active".into(),
            })
            .unwrap()
        else {
            panic!("active return")
        };
        f.editor
            .execute(Command::SetReturnSend {
                track: f.tracks[0],
                return_track: active,
                gain: 0.5,
            })
            .unwrap();
        let revision = prepare(&f).unwrap();
        let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut reference = SessionPlayback::new(revision).unwrap();
        for runtime in [&mut old, &mut reference] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 32]).unwrap();
        }
        f.editor
            .execute(Command::RemoveReturnTrack(unused))
            .unwrap();
        for restored in [false, true] {
            if restored {
                assert!(f.editor.undo());
                assert_eq!(f.editor.project().return_tracks()[0].id(), unused);
            }
            let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
            next.try_inherit_state_from(&mut old).unwrap();
            let mut actual = vec![[0.0; 2]; 257];
            let mut expected = actual.clone();
            for chunk in actual.chunks_mut(partition) {
                next.render(chunk).unwrap();
            }
            for chunk in expected.chunks_mut(partition) {
                reference.render(chunk).unwrap();
            }
            assert_eq!(
                actual, expected,
                "partition {partition}, restored={restored}"
            );
            assert!(expected.iter().any(|frame| frame[0] > 1.0));
            old = next;
        }
    }
}

#[test]
fn removing_an_unused_return_does_not_hide_a_changed_send_destination() {
    use hex_project::{Command, Created};
    let mut f = fixture(&[&[7]], 1024);
    for audio in f.media.values_mut() {
        *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
    }
    let mut returns = Vec::new();
    for name in ["unused", "first", "second"] {
        let Created::ReturnTrack(id) = f
            .editor
            .execute(Command::AddReturnTrack { name: name.into() })
            .unwrap()
        else {
            panic!("return")
        };
        returns.push(id);
    }
    f.editor
        .execute(Command::SetReturnSend {
            track: f.tracks[0],
            return_track: returns[1],
            gain: 0.5,
        })
        .unwrap();
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 32]).unwrap();
    f.editor
        .execute(Command::RemoveReturnTrack(returns[0]))
        .unwrap();
    f.editor
        .execute(Command::SetReturnSend {
            track: f.tracks[0],
            return_track: returns[1],
            gain: 0.0,
        })
        .unwrap();
    f.editor
        .execute(Command::SetReturnSend {
            track: f.tracks[0],
            return_track: returns[2],
            gain: 0.5,
        })
        .unwrap();
    let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut actual = [[0.0; 2]; 16];
    next.render(&mut actual).unwrap();
    assert_eq!(&actual[..7], &[[0.0; 2]; 7]);
    assert!(actual[7][0] > 0.0);
}

#[test]
fn removing_a_routed_return_keeps_the_conservative_restart() {
    use hex_project::{Command, Created};
    let mut f = fixture(&[&[7]], 1024);
    for audio in f.media.values_mut() {
        *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
    }
    let Created::ReturnTrack(bus) = f
        .editor
        .execute(Command::AddReturnTrack {
            name: "routed".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    f.editor
        .execute(Command::SetReturnSend {
            track: f.tracks[0],
            return_track: bus,
            gain: 0.5,
        })
        .unwrap();
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 32]).unwrap();
    f.editor.execute(Command::RemoveReturnTrack(bus)).unwrap();
    let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut actual = [[0.0; 2]; 16];
    next.render(&mut actual).unwrap();
    assert_eq!(&actual[..7], &[[0.0; 2]; 7]);
    assert_eq!(actual[7], [1.0, -0.5]);
}

#[test]
fn removing_an_empty_track_keeps_compensated_pcm_continuous() {
    use hex_project::{Command, Created};
    for partition in [1, 257] {
        let mut f = fixture(&[&[7]], 1024);
        for audio in f.media.values_mut() {
            *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
        }
        let Created::Track(empty) = f
            .editor
            .execute(Command::AddTrack {
                name: "empty".into(),
            })
            .unwrap()
        else {
            panic!("track creation");
        };
        let revision = prepare(&f).unwrap();
        let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut reference = SessionPlayback::new(revision).unwrap();
        for runtime in [&mut old, &mut reference] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 32]).unwrap();
        }
        f.editor.execute(Command::RemoveTrack(empty)).unwrap();
        let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        let mut actual = vec![[0.0; 2]; 257];
        let mut expected = actual.clone();
        for chunk in actual.chunks_mut(partition) {
            next.render(chunk).unwrap();
        }
        for chunk in expected.chunks_mut(partition) {
            reference.render(chunk).unwrap();
        }
        assert_eq!(actual, expected, "partition {partition}");
    }
}

#[test]
fn adding_a_routed_empty_track_keeps_the_conservative_restart() {
    use hex_project::{Command, Created};
    let mut f = fixture(&[&[7]], 1024);
    for audio in f.media.values_mut() {
        *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
    }
    let Created::ReturnTrack(bus) = f
        .editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return");
    };
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 32]).unwrap();
    let Created::Track(track) = f
        .editor
        .execute(Command::AddTrack {
            name: "routed".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    f.editor
        .execute(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 0.5,
        })
        .unwrap();
    let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut output = [[0.0; 2]; 16];
    next.render(&mut output).unwrap();
    assert_eq!(&output[..7], &[[0.0; 2]; 7]);
    assert_eq!(output[7], [1.0, -0.5]);
}
#[test]
fn compensated_fader_ramps_at_the_delayed_content_origin() {
    let mut f = fixture(&[&[7]], 1024);
    for audio in f.media.values_mut() {
        *audio = Audio::new(48000, vec![[1.0, -0.5]; 1024]).unwrap();
    }
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 32]).unwrap();
    f.editor
        .execute(hex_project::Command::Gain {
            track: f.tracks[0],
            gain: 0.0,
        })
        .unwrap();
    let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut frames = [[0.0; 2]; 256];
    next.render(&mut frames).unwrap();
    for (index, frame) in frames.iter().enumerate() {
        let gain = 1.0 - (index + 1).min(240) as f32 / 240.0;
        assert!((frame[0] - gain).abs() < 1e-5, "frame {index}: {frame:?}");
        assert!(
            (frame[1] + 0.5 * gain).abs() < 1e-5,
            "frame {index}: {frame:?}"
        );
    }
}

#[test]
fn noncompensated_device_fader_ramps_without_applying_gain_twice() {
    let mut f = fixture(&[&[0]], 1024);
    for audio in f.media.values_mut() {
        *audio = Audio::new(48000, vec![[1.0, -0.5]; 1024]).unwrap();
    }
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    assert!(old.compensation.is_none());
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 32]).unwrap();
    f.editor
        .execute(hex_project::Command::Gain {
            track: f.tracks[0],
            gain: 0.0,
        })
        .unwrap();
    let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut frames = [[0.0; 2]; 256];
    next.render(&mut frames).unwrap();
    for (index, frame) in frames.iter().enumerate() {
        let gain = 1.0 - (index + 1).min(240) as f32 / 240.0;
        assert!((frame[0] - gain).abs() < 1e-5, "frame {index}: {frame:?}");
        assert!(
            (frame[1] + 0.5 * gain).abs() < 1e-5,
            "frame {index}: {frame:?}"
        );
    }
}

#[test]
fn compensated_fader_waits_for_origin_and_rolls_back_on_failure() {
    let f = fixture(&[&[7]], 1024);
    let revision = prepare(&f).unwrap();
    let mut tested = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut oracle = SessionPlayback::new(revision).unwrap();
    for runtime in [&mut tested, &mut oracle] {
        runtime.track_faders[0].retarget([0.0; 2], 48000);
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let initial = tested.track_faders[0];
    tested.render(&mut [[0.0; 2]; 6]).unwrap();
    oracle.render(&mut [[0.0; 2]; 6]).unwrap();
    assert_eq!(tested.track_faders[0], initial);
    tested.arrangement_frame = u64::MAX - 300;
    assert!(tested.render(&mut [[0.0; 2]; 512]).is_err());
    assert_eq!(tested.track_faders[0], initial);
    tested.arrangement_frame = 6;
    let mut actual = [[0.0; 2]; 32];
    let mut expected = actual;
    tested.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(tested.track_faders[0], oracle.track_faders[0]);
}

#[test]
fn track_fader_ramp_feeds_capture_and_send_once() {
    use hex_project::{Command, Created};
    let mut f = fixture(&[&[]], 1024);
    for audio in f.media.values_mut() {
        *audio = Audio::new(48000, vec![[1.0, -0.5]; 1024]).unwrap();
    }
    let Created::ReturnTrack(bus) = f
        .editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    f.editor
        .execute(Command::SetReturnSend {
            track: f.tracks[0],
            return_track: bus,
            gain: 0.5,
        })
        .unwrap();
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 32]).unwrap();
    f.editor
        .execute(Command::Gain {
            track: f.tracks[0],
            gain: 0.0,
        })
        .unwrap();
    let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let plan = next
        .output_capture_plan(
            &[
                OutputCaptureId::Track(f.tracks[0]),
                OutputCaptureId::Return(bus),
            ],
            256,
        )
        .unwrap();
    let mut main = [[0.0; 2]; 256];
    let mut track = main;
    let mut returned = main;
    let mut buffers = [&mut track[..], &mut returned[..]];
    let mut capture = OutputCapture::new(&plan, &mut buffers);
    next.render_with_capture(&mut main, &mut capture).unwrap();
    let track = capture.output(0).unwrap();
    let returned = capture.output(1).unwrap();
    for index in 0..256 {
        let gain = 1.0 - (index + 1).min(240) as f32 / 240.0;
        assert!((track[index][0] - gain).abs() < 1e-5);
        assert!((returned[index][0] - 0.5 * gain).abs() < 1e-5);
        assert!((main[index][0] - 1.5 * gain).abs() < 1e-5);
    }
}

#[test]
fn rapid_gain_retarget_and_equivalent_publication_continue_from_current_level() {
    use hex_project::Command;
    let mut f = fixture(&[&[]], 1024);
    for audio in f.media.values_mut() {
        *audio = Audio::new(48000, vec![[1.0; 2]; 1024]).unwrap();
    }
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 32]).unwrap();
    f.editor
        .execute(Command::Gain {
            track: f.tracks[0],
            gain: 0.0,
        })
        .unwrap();
    let mut down = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    down.try_inherit_state_from(&mut old).unwrap();
    let mut down_frames = [[0.0; 2]; 64];
    down.render(&mut down_frames).unwrap();
    let current = 1.0 - 64.0 / 240.0;
    assert!((down_frames[63][0] - current).abs() < 1e-5);
    f.editor
        .execute(Command::Gain {
            track: f.tracks[0],
            gain: 1.0,
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    let mut up = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    up.try_inherit_state_from(&mut down).unwrap();
    let mut first = [[0.0; 2]; 32];
    up.render(&mut first).unwrap();
    let mut republished = SessionPlayback::new(revision).unwrap();
    republished.try_inherit_state_from(&mut up).unwrap();
    let mut rest = [[0.0; 2]; 208];
    republished.render(&mut rest).unwrap();
    for (index, frame) in first.iter().chain(rest.iter()).enumerate() {
        let gain = current + (1.0 - current) * (index + 1) as f32 / 240.0;
        assert!((frame[0] - gain).abs() < 1e-5, "frame {index}: {frame:?}");
    }
}
#[test]
fn compensation_late_failure_rolls_back_all_rings_and_node_histories() {
    let f = fixture(&[&[], &[7]], 1024);
    let revision = prepare(&f).unwrap();
    let mut tested = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut oracle = SessionPlayback::new(revision).unwrap();
    for runtime in [&mut tested, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut [[0.0; 2]; 4]).unwrap();
    }
    tested.arrangement_frame = u64::MAX - 300;
    let mut observed = [0; 2];
    let mut output = [[9.0; 2]; 512];
    assert!(
        tested
            .render_with_node_signal_observer(
                &mut output,
                Some(&mut |_, phase, _, _: &[Frame]| {
                    let index = match phase {
                        crate::NodeSignalPhase::Input => 0,
                        crate::NodeSignalPhase::Output => 1,
                    };
                    observed[index] += 1;
                })
            )
            .is_err()
    );
    assert!(
        observed.iter().all(|count| *count > 0),
        "fault must occur after speculative processing"
    );
    assert_eq!(output, [[0.0; 2]; 512]);
    assert_eq!(tested.performance_frame, 4);
    assert_eq!(tested.arrangement_frame, u64::MAX - 300);
    tested.arrangement_frame = 4;
    let mut actual = [[0.0; 2]; 32];
    let mut expected = actual;
    tested.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual[3], [0.5, -1.0]);
}
#[test]
fn recording_clock_waits_for_real_fader_origin_and_preserves_content_loop_pass() {
    use hex_project::Command;
    let mut f = fixture(&[&[7]], 8);
    f.editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 8,
            enabled: true,
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    let revision = Arc::try_unwrap(revision)
        .unwrap()
        .arm_gain_recording(f.editor.project(), f.tracks[0])
        .unwrap();
    let mut runtime = SessionPlayback::new(Arc::new(revision)).unwrap();
    let id = f.tracks[0].value();
    assert!(!runtime.gain_recording_ready(id));
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    runtime.render(&mut [[0.0; 2]; 6]).unwrap();
    assert_eq!(runtime.gain_recording_clock(id), None);
    runtime.render(&mut [[0.0; 2]; 1]).unwrap();
    assert_eq!(runtime.gain_recording_clock(id), Some((0, 0, 0)));
    runtime.render(&mut [[0.0; 2]; 3]).unwrap();
    assert_eq!(runtime.gain_recording_clock(id), Some((3, 0, 0)));
    runtime.apply(SessionPlaybackCommand::Stop).unwrap();
    assert_eq!(runtime.gain_recording_clock(id), None);
}
#[test]
fn dry_fader_clock_reports_downstream_main_delivery_delay() {
    let f = fixture(&[&[], &[7]], 8);
    let revision = Arc::try_unwrap(prepare(&f).unwrap())
        .unwrap()
        .arm_gain_recording(f.editor.project(), f.tracks[0])
        .unwrap();
    let runtime = SessionPlayback::new(Arc::new(revision)).unwrap();
    assert_eq!(
        runtime.gain_recording_clock(f.tracks[0].value()),
        Some((0, 7, 0))
    );
}

/// Pulse #3772: a latency device on an unrelated track no longer disables
/// computer-keyboard monitoring. The keyboard plays its instrument through the
/// compensated graph, delayed only by the project's PDC alignment.
#[test]
fn computer_keyboard_plays_through_positive_latency_on_another_track() {
    let keyboard_output = |delays: &[&[u32]], playing: bool| {
        let mut f = fixture(delays, 1024);
        f.editor
            .execute(hex_project::Command::InsertInstrument {
                track: f.tracks[1],
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
        // Silence the clip track so only the keyboard track is compared.
        f.editor
            .execute(hex_project::Command::Gain {
                track: f.tracks[0],
                gain: 0.0,
            })
            .unwrap();
        let revision = prepare(&f).unwrap();
        let latency = revision.arrangement().main_latency_frames() as usize;
        let mut playback = crate::playback::Playback::from_prepared(revision);
        let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 100, 1))).unwrap();
        playback
            .prepare_computer_keyboard(f.editor.project(), f.tracks[1], input)
            .unwrap();
        if playing {
            playback.apply(crate::playback::PlaybackCommand::Play);
        }
        let mut output = vec![[0.0; 2]; 512];
        playback.render(&mut output).unwrap();
        (latency, output)
    };
    for playing in [false, true] {
        let (latency, delayed) = keyboard_output(&[&[7], &[]], playing);
        let (none, direct) = keyboard_output(&[&[], &[]], playing);
        assert_eq!((latency, none), (7, 0));
        assert!(direct.iter().flatten().any(|sample| *sample != 0.0));
        assert!(
            delayed[..latency]
                .iter()
                .flatten()
                .all(|sample| *sample == 0.0)
        );
        for (index, (actual, expected)) in delayed[latency..].iter().zip(&direct).enumerate() {
            for channel in 0..2 {
                assert!(
                    (actual[channel] - expected[channel]).abs() < 1e-6,
                    "playing={playing} frame {index}: {actual:?} != {expected:?}"
                );
            }
        }
    }
}

#[test]
fn paused_source_monitor_preserves_buffered_arrangement_audio() {
    use crate::playback::{Playback, PlaybackCommand};
    use hex_project::Command;

    let mut mismatches = Vec::new();
    for (effect_delay, keyboard, partition) in [
        (false, false, 1),
        (false, false, 257),
        (false, true, 1),
        (false, true, 257),
        (true, false, 1),
        (true, false, 257),
        (true, true, 1),
        (true, true, 257),
    ] {
        let mut f = fixture(&[&[7], &[]], 1024);
        let keyboard_track = f.tracks[usize::from(effect_delay)];
        f.editor
            .execute(Command::InsertInstrument {
                track: keyboard_track,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
        // The muted keyboard track contributes no PCM. The other track's
        // impulse is queued either in compensation or a device delay.
        f.editor
            .execute(Command::Gain {
                track: keyboard_track,
                gain: 0.0,
            })
            .unwrap();
        let revision = prepare(&f).unwrap();
        let mut actual = Playback::from_prepared(Arc::clone(&revision));
        let mut oracle = Playback::from_prepared(revision);
        let keys = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
        if keyboard {
            actual
                .prepare_computer_keyboard(f.editor.project(), keyboard_track, Arc::clone(&keys))
                .unwrap();
        }
        for playback in [&mut actual, &mut oracle] {
            playback.apply(PlaybackCommand::Play);
            playback.render(&mut [[0.0; 2]; 4]).unwrap();
            playback.apply(PlaybackCommand::Pause);
        }
        if keyboard {
            keys.set_key(0, Some((60, 100, 1))).unwrap();
        }
        let mut monitored = [[0.0; 2]; 257];
        for block in monitored.chunks_mut(partition) {
            actual.render(block).unwrap();
        }
        if keyboard {
            keys.set_key(0, None).unwrap();
        }
        for playback in [&mut actual, &mut oracle] {
            playback.apply(PlaybackCommand::Play);
        }
        let mut got = [[0.0; 2]; 32];
        let mut expected = got;
        actual.render(&mut got).unwrap();
        oracle.render(&mut expected).unwrap();
        assert!(expected.iter().any(|frame| *frame != [0.0; 2]));
        let leaked_while_paused = monitored.iter().any(|frame| *frame != [0.0; 2]);
        let resume_differs = got != expected;
        if leaked_while_paused || resume_differs {
            mismatches.push((
                effect_delay,
                keyboard,
                partition,
                leaked_while_paused,
                resume_differs,
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "mismatches: (device delay, keyboard active, paused partition, leaked, resume differs) {mismatches:?}"
    );
}

#[test]
fn paused_keyboard_preserves_same_track_device_history() {
    use crate::playback::{Playback, PlaybackCommand};
    use hex_project::Command;

    let mut f = fixture(&[&[7]], 1024);
    f.editor
        .execute(Command::InsertInstrument {
            track: f.tracks[0],
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    let mut monitored = Playback::from_prepared(Arc::clone(&revision));
    let mut oracle = Playback::from_prepared(revision);
    let keys = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
    monitored
        .prepare_computer_keyboard(f.editor.project(), f.tracks[0], keys)
        .unwrap();
    for playback in [&mut monitored, &mut oracle] {
        playback.apply(PlaybackCommand::Play);
        playback.render(&mut [[0.0; 2]; 4]).unwrap();
        playback.apply(PlaybackCommand::Pause);
    }
    let mut paused = [[0.0; 2]; 257];
    monitored.render(&mut paused).unwrap();
    for playback in [&mut monitored, &mut oracle] {
        playback.apply(PlaybackCommand::Play);
    }
    let mut actual = [[0.0; 2]; 32];
    let mut expected = actual;
    monitored.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert!(expected.iter().any(|frame| *frame != [0.0; 2]));
    let leaked_while_paused = paused.iter().any(|frame| *frame != [0.0; 2]);
    let resume_differs = actual != expected;
    assert!(
        !leaked_while_paused && !resume_differs,
        "queued clip audio leaked while paused: {leaked_while_paused}; resume differs: {resume_differs}"
    );
}

#[test]
fn operator_sync_companions_preserve_pcm_through_compensated_startup_and_partitions() {
    use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
    use hex_project::{BeatTime, Command, Created};
    let frames = 16_000;
    let configure = |delay: u32| {
        let mut f = fixture(&[&[delay]], frames);
        f.editor
            .execute(Command::InsertInstrument {
                track: f.tracks[0],
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
        let class = crate::NativeDeviceClass::Instrument;
        let mut state = crate::default_native_device_state_for_class(
            class,
            DeviceTypeId::from_static("hex.instrument.operator"),
        )
        .unwrap();
        for (id, value) in [
            (357, ParameterValue::Bool(false)),
            (360, ParameterValue::Float(0.5)),
            (362, ParameterValue::Choice(3)),
            (364, ParameterValue::Choice(0)),
        ] {
            state = crate::patch_native_device_parameter_for_class(
                class,
                &state,
                ParameterId::from_static(id),
                value,
            )
            .unwrap();
        }
        f.editor
            .execute(Command::RemoveInstrument { track: f.tracks[0] })
            .unwrap();
        f.editor
            .execute(Command::InsertInstrument {
                track: f.tracks[0],
                state,
            })
            .unwrap();
        let Created::Clip(clip) = f
            .editor
            .execute(Command::PlaceMidiClip {
                track: f.tracks[0],
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(8).unwrap(),
            })
            .unwrap()
        else {
            panic!("midi clip")
        };
        f.editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(2).unwrap(),
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        prepare(&f).unwrap()
    };
    let dry = configure(0);
    let delayed = configure(7);
    assert_eq!(delayed.arrangement().main_latency_frames(), 7);
    let render = |revision, length, partition| {
        let mut runtime = SessionPlayback::new(revision).unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        let mut pcm = vec![[0.0; 2]; length];
        for chunk in pcm.chunks_mut(partition) {
            runtime.render(chunk).unwrap();
        }
        pcm
    };
    let expected = render(dry, frames, 127);
    assert!(
        expected[512..1024]
            .iter()
            .any(|frame| frame[0].abs() > 1e-6)
    );
    for partition in [1, 257] {
        let actual = render(Arc::clone(&delayed), frames + 7, partition);
        assert_eq!(actual[..7], [[0.0; 2]; 7]);
        assert_eq!(actual[7..], expected);
    }
}

#[test]
fn main_removal_and_serial_replacement_keep_old_larger_stamp_horizon_and_pending_pcm() {
    use crate::signal_graph::tests::state;
    use hex_project::{Command, Created};
    for routed in [false, true] {
        for replacement in [false, true] {
            let mut f = if routed {
                fixture(&[&[7]], 64)
            } else {
                fixture(&[&[]], 64)
            };
            if routed {
                let Created::ReturnTrack(bus) = f
                    .editor
                    .execute(Command::AddReturnTrack {
                        name: "delayed".into(),
                    })
                    .unwrap()
                else {
                    panic!("return")
                };
                f.editor
                    .execute(Command::InsertReturnDevice {
                        return_track: bus,
                        before: None,
                        state: state(5).unwrap(),
                    })
                    .unwrap();
                f.editor
                    .execute(Command::SetReturnSend {
                        track: f.tracks[0],
                        return_track: bus,
                        gain: 1.0,
                    })
                    .unwrap();
            }
            let Created::Device(main) = f
                .editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state: state(3).unwrap(),
                })
                .unwrap()
            else {
                panic!("main")
            };
            let original = prepare(&f).unwrap();
            let old_horizon = original.arrangement().main_latency_frames();
            let warm = old_horizon as usize - 1;
            let mut old = SessionPlayback::new(Arc::clone(&original)).unwrap();
            let mut oracle = SessionPlayback::new(original).unwrap();
            for runtime in [&mut old, &mut oracle] {
                runtime.apply(SessionPlaybackCommand::Play).unwrap();
                runtime.render(&mut vec![[0.0; 2]; warm]).unwrap();
            }
            f.editor.execute(Command::RemoveDevice(main)).unwrap();
            if replacement {
                for _ in 0..2 {
                    f.editor
                        .execute(Command::InsertMainDevice {
                            before: None,
                            state: state(1).unwrap(),
                        })
                        .unwrap();
                }
            }
            let revision = prepare(&f).unwrap();
            let graph = Arc::clone(revision.arrangement().signal_graph.as_ref().unwrap());
            assert!(revision.arrangement().main_latency_frames() < old_horizon);
            let mut next = SessionPlayback::new(revision).unwrap();
            next.try_inherit_state_from(&mut old).unwrap();
            let history = next.compensation.as_ref().unwrap();
            assert!(Arc::ptr_eq(&history.graph, &graph));
            assert!(
                history.delays.history_horizon() >= old_horizon,
                "old origins cannot shrink with current Main"
            );
            // A second coherent handoff while the detached Main is still
            // audible carries the same bank owner, without refeeding it.
            let mut continued = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
            continued.try_inherit_state_from(&mut next).unwrap();
            let mut next = continued;
            let mut expected = [[0.0; 2]; 4];
            let mut actual = expected;
            oracle.render(&mut expected).unwrap();
            assert!(expected.iter().flatten().any(|s| *s != 0.0));
            next.render(&mut actual).unwrap();
            assert_eq!(
                actual, expected,
                "old output once; routed={routed}, replacement={replacement}"
            );
        }
    }
}

#[test]
fn neutral_zero_graph_metadata_allocates_no_delay_runtime_and_keeps_first_sample() {
    let f = fixture(&[&[]], 8);
    let revision = prepare(&f).unwrap();
    let graph = revision.arrangement().signal_graph.as_ref().unwrap();
    assert!(!graph.needs_runtime());
    let mut runtime = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    assert!(runtime.compensation.is_none());
    let renderer = crate::Renderer::new(revision.arrangement());
    assert!(renderer.runtime.compensation.is_none());
    assert!(runtime.accounted_runtime_storage_bytes().unwrap() >= graph.table_bytes().unwrap());
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    let mut actual = [[0.0; 2]; 8];
    runtime.render(&mut actual).unwrap();
    assert_eq!(&actual[..], f.samples.as_slice());
}

#[test]
fn main_removal_retains_old_tail_and_applies_future_source_mix_once() {
    use crate::signal_graph::tests::state;
    use hex_project::{Command, Created};
    let mut f = fixture(&[&[]], 64);
    f.editor
        .execute(Command::Gain {
            track: f.tracks[0],
            gain: 0.5,
        })
        .unwrap();
    f.editor
        .execute(Command::Balance {
            track: f.tracks[0],
            balance: 0.25,
        })
        .unwrap();
    f.editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    let Created::Device(main) = f
        .editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: state(3).unwrap(),
        })
        .unwrap()
    else {
        panic!("main")
    };
    let original = prepare(&f).unwrap();
    let mut donor = SessionPlayback::new(Arc::clone(&original)).unwrap();
    let mut old_tail = SessionPlayback::new(original).unwrap();
    for runtime in [&mut donor, &mut old_tail] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut [[0.0; 2]; 2]).unwrap();
    }
    f.editor.execute(Command::RemoveDevice(main)).unwrap();
    let current = prepare(&f).unwrap();
    assert!(
        !current
            .arrangement()
            .signal_graph
            .as_ref()
            .unwrap()
            .needs_runtime()
    );
    let mut ordinary = SessionPlayback::new(Arc::clone(&current)).unwrap();
    assert!(ordinary.compensation.is_none());
    ordinary.apply(SessionPlaybackCommand::Seek(2)).unwrap();
    ordinary.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected = [[0.0; 2]; 64];
    ordinary.render(&mut expected).unwrap();
    assert!(expected[61].iter().any(|sample| *sample != 0.0));
    let mut residual = [[0.0; 2]; 64];
    old_tail.render(&mut residual).unwrap();
    assert!(residual[1].iter().any(|sample| *sample != 0.0));
    assert!(residual[2..].iter().flatten().all(|sample| *sample == 0.0));
    for (frame, tail) in expected.iter_mut().zip(residual) {
        frame[0] += tail[0];
        frame[1] += tail[1];
    }
    let mut carried = SessionPlayback::new(current).unwrap();
    carried.try_inherit_state_from(&mut donor).unwrap();
    assert!(carried.compensation.is_some());
    let mut actual = [[0.0; 2]; 64];
    carried.render(&mut actual).unwrap();
    assert_eq!(
        actual, expected,
        "future source mix once plus independent old residual"
    );
}

#[path = "compensation_tests/monitor_clock.rs"]
mod monitor_clock;

#[path = "compensation_tests/cue.rs"]
mod cue;

#[test]
fn ordinary_stopped_compensation_preview_is_pure_and_retains_candidate_admission() {
    let mut f = fixture(&[&[], &[]], 128);
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    f.editor
        .execute(hex_project::Command::SetAudioFeed {
            source: hex_project::RoutingOwner::Track(f.tracks[0]),
            destination: f.tracks[1],
            gain: 0.5,
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    let mut candidate = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    assert!(candidate.compensation.is_some());
    let before = (
        old.capture_epoch,
        candidate.capture_epoch,
        old.performance_frame,
        candidate.performance_frame,
        candidate.candidate_admission_open,
    );
    let mut pages = Vec::new();
    let (window, report) = candidate
        .preview_stream_handoff_with_policy_and_report(
            &mut old,
            64,
            None,
            &mut |extent| {
                pages.push(extent);
                Ok(())
            },
            true,
            false,
        )
        .unwrap();
    assert!(!report.running_audio_restart());
    assert_eq!(window.frames, 0);
    assert!(pages.is_empty());
    assert_eq!(
        (
            old.capture_epoch,
            candidate.capture_epoch,
            old.performance_frame,
            candidate.performance_frame,
            candidate.candidate_admission_open
        ),
        before
    );
    let running = candidate
        .preview_stream_handoff_with_scope(&mut old, 64, Some(true), &mut |_| Ok(()), true)
        .unwrap();
    assert_eq!(running.frames, 64);
    assert_eq!(
        (
            old.capture_epoch,
            candidate.capture_epoch,
            old.performance_frame,
            candidate.performance_frame,
            candidate.candidate_admission_open
        ),
        before
    );
    let committed = candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(
        report.running_audio_restart(),
        committed.running_audio_restart()
    );
    let mut oracle = SessionPlayback::new(revision).unwrap();
    for runtime in [&mut candidate, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut actual = [[0.0; 2]; 64];
    let mut expected = actual;
    candidate.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn gain_recording_delivery_delay_includes_the_complete_serial_main_chain() {
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::{Command, Created, Editor, Project};
    for (lookaheads, expected_delay) in [(&[2_u32][..], 288_u64), (&[0_u32, 2][..], 360_u64)] {
        let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: "serial main".into(),
                path: "memory/serial-main.wav".into(),
                source_sample_rate: 48000,
                source_frames: 4096,
                playback_frames: 4096,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })
            .unwrap()
        else {
            panic!("import")
        };
        for &lookahead in lookaheads {
            let definition = &hex_devices::limiter::DEFINITION;
            let state = definition
                .patch_parameter(
                    &definition.default_state().unwrap(),
                    hex_devices::limiter::parameter::LOOKAHEAD,
                    ParameterValue::Choice(lookahead),
                )
                .unwrap();
            editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state,
                })
                .unwrap();
        }
        let media = std::collections::BTreeMap::from([(
            asset,
            Audio::new(48000, vec![[0.01, -0.02]; 4096]).unwrap(),
        )]);
        let revision = PreparedRevision::prepare_for_gain_recording(
            editor.project(),
            &media,
            Arc::new(SineTable::new()),
            None,
            track,
        )
        .unwrap();
        assert_eq!(
            u64::from(revision.arrangement().main_latency_frames()),
            expected_delay
        );
        let mut runtime = SessionPlayback::new(Arc::new(revision)).unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        assert_eq!(
            runtime.gain_recording_clock(track.value()),
            Some((0, expected_delay, 0)),
            "serial Main delay must be added after the pre-Main sum"
        );
        let mut initial = [[1.0; 2]; 16];
        runtime.render(&mut initial).unwrap();
        assert_eq!(initial, [[0.0; 2]; 16]);
        assert_eq!(
            runtime.gain_recording_clock(track.value()),
            Some((16, expected_delay, 0))
        );
    }
}

#[test]
fn compensated_mutable_source_applies_live_input_with_and_without_shared_contributors() {
    use hex_lattice::{NativeTransientOperation, NativeTransientValue};
    use hex_lattice_contract::{
        DeviceAutomationTarget, NativeDeviceMetadata, ParameterId, ParameterValue,
    };
    use hex_project::{BeatTime, Command, Created, Editor, Project, TrackDeviceSlot};
    use std::collections::BTreeMap;

    for shared in [false, true] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(receiver) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "instrument".into(),
                before_track: None,
                slot: TrackDeviceSlot::Instrument,
                state: hex_devices::fm4::device::FM4_DEFINITION
                    .default_state()
                    .unwrap(),
            })
            .unwrap()
        else {
            panic!("instrument track")
        };
        let track = editor.project().track(receiver).unwrap();
        let device = track.instrument().unwrap().id();
        let source = track.source_id().unwrap();
        let note_track = if shared {
            let Created::Track(contributor) = editor
                .execute(Command::AddMidiTrack {
                    name: "contributor".into(),
                })
                .unwrap()
            else {
                panic!("contributor")
            };
            editor
                .execute(Command::BindTrackSource {
                    track: contributor,
                    source: Some(source),
                })
                .unwrap();
            contributor
        } else {
            receiver
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track: note_track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(2).unwrap(),
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        editor
            .execute(Command::InsertDevice {
                track: receiver,
                before: None,
                state: hex_devices::limiter::DEFINITION.default_state().unwrap(),
            })
            .unwrap();
        let parameter = ParameterId::from_static(29);
        let target = DeviceAutomationTarget::Parameter(parameter);
        let revision = PreparedRevision::prepare_for_native_parameter_input(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
            device,
            target,
        )
        .unwrap();
        assert!(revision.arrangement().main_latency_frames() > 0);
        assert_eq!(revision.source_routing().has_shared_sources(), shared);
        let mut runtime = SessionPlayback::new(Arc::new(revision)).unwrap();
        assert!(
            runtime
                .compensation
                .as_ref()
                .unwrap()
                .graph
                .needs_upstream_frame_routing()
        );
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        let operation = NativeTransientOperation::Set(NativeTransientValue::Parameter {
            id: parameter,
            value: ParameterValue::Float(0.5),
        });
        runtime.set_native_parameter_input(operation).unwrap();
        let mut heard = false;
        let mut applied = None;
        for _ in 0..8 {
            let mut output = [[0.0; 2]; 256];
            runtime.render(&mut output).unwrap();
            heard |= output.iter().flatten().any(|value| value.abs() > 1e-8);
            applied = applied.or_else(|| runtime.take_native_parameter_application());
        }
        assert!(
            heard,
            "mutable instrument must remain audible, shared={shared}"
        );
        assert_eq!(
            applied.expect("live input must be applied").operation,
            operation
        );
    }
}

#[path = "compensation_tests/paused_layout.rs"]
mod paused_layout;

#[test]
fn removing_isolated_return_refuses_active_detached_main_tail() {
    use crate::signal_graph::tests::state;
    use hex_project::{Command, Created};
    let mut f = fixture(&[&[]], 64);
    let Created::ReturnTrack(bus) = f
        .editor
        .execute(Command::AddReturnTrack {
            name: "unused".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    let Created::Device(main) = f
        .editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: state(3).unwrap(),
        })
        .unwrap()
    else {
        panic!("main")
    };
    f.editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: state(3).unwrap(),
        })
        .unwrap();
    let original = prepare(&f).unwrap();
    let mut donor = SessionPlayback::new(Arc::clone(&original)).unwrap();
    let mut oracle = SessionPlayback::new(original).unwrap();
    for runtime in [&mut donor, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut [[0.0; 2]; 5]).unwrap();
    }
    f.editor.execute(Command::RemoveDevice(main)).unwrap();
    let mut detached = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    detached.try_inherit_state_live_from(&mut donor).unwrap();
    assert!(
        detached
            .return_effect_tails
            .slots()
            .iter()
            .flatten()
            .any(|tail| tail.output_active && tail.return_id().is_none())
    );
    f.editor.execute(Command::RemoveReturnTrack(bus)).unwrap();
    let mut candidate = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    assert!(matches!(
        candidate.try_inherit_state_live_from(&mut detached),
        Err(SessionPlaybackError::LatencyLayoutChanged)
    ));
    let mut actual = [[0.0; 2]; 8];
    let mut expected = actual;
    detached.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
}
