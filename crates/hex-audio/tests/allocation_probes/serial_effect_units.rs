//! Headless engine handoff/render probe; no CPAL or physical deadline claim.
use hex_engine::{Audio, PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn nested_effect_render_and_parent_power_handoff_do_not_allocate_or_free() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "memory/unit.wav".into(),
            sample_rate: 48_000,
            frames: 4096,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 4096,
            source_offset: 0,
            source_length: 4096,
            looping: false,
        })
        .unwrap();
    let definition = &hex_devices::utility::UTILITY_DEFINITION;
    let initial = definition.default_state().unwrap();
    let mut ids = Vec::new();
    for state in [
        definition
            .patch_parameter(
                &initial,
                hex_devices::utility::parameter::INVERT_RIGHT,
                ParameterValue::Bool(true),
            )
            .unwrap(),
        definition
            .patch_parameter(
                &initial,
                hex_devices::utility::parameter::CHANNEL,
                ParameterValue::Choice(3),
            )
            .unwrap(),
    ] {
        let Created::Device(id) = editor
            .execute(Command::InsertMainDevice {
                before: None,
                state,
            })
            .unwrap()
        else {
            panic!("effect")
        };
        ids.push(id);
    }
    editor
        .execute(Command::ReparentDevice {
            device: ids[1],
            parent: Some(ids[0]),
        })
        .unwrap();
    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![[0.25, -0.5]; 4096]).unwrap())]);
    let table = Arc::new(SineTable::new());
    let prepare = |project: &Project, previous: Option<&hex_engine::PreparedSessionInventory>| {
        Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), previous).unwrap())
    };
    let mut playback = SessionPlayback::new(prepare(editor.project(), None)).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 512];
    let cold = allocation_counter::measure(|| playback.render(&mut output).unwrap());
    assert_eq!(
        (
            cold.count_total,
            cold.bytes_total,
            cold.count_current,
            cold.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(output[511], [0.5, 0.25]);
    for bypassed in [true, false] {
        editor
            .execute(Command::SetDeviceBypass {
                device: ids[0],
                bypassed,
            })
            .unwrap();
        let mut next = SessionPlayback::new(prepare(
            editor.project(),
            Some(playback.prepared_revision().session()),
        ))
        .unwrap();
        let counts = allocation_counter::measure(|| {
            next.try_inherit_state_from(&mut playback).unwrap();
            next.render(&mut output).unwrap();
        });
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(
            output[511],
            if bypassed { [0.25, -0.5] } else { [0.5, 0.25] }
        );
        assert!(!editor.project().device(ids[1]).unwrap().bypassed());
        playback = next;
    }
}
