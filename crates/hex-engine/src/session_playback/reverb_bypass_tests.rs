//! Real routed room histories use one bounded host bypass transition.
use super::*;
use hex_devices::reverb::{REVERB_DEFINITION as DEFINITION, parameter as p};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{ReturnTrackSnapshot, SendLevelSnapshot};

fn render(playback: &mut SessionPlayback, frames: usize, block: usize) -> Vec<Frame> {
    let mut output = vec![[0.0; 2]; frames];
    for part in output.chunks_mut(block) {
        playback.render(part).unwrap();
    }
    output
}

#[test]
fn reverb_bypass_track_return_main_matches_single_wet_history_and_dry_reference() {
    for location in 0..3 {
        let mut snapshot = project(0.5, 0, 8 << 32).snapshot();
        let id = snapshot.next_id;
        snapshot.next_id += 2;
        let mut state = DEFINITION.default_state().unwrap();
        for (id, value) in [
            (p::DRY_WET, ParameterValue::Float(100.0)),
            (p::SPIN, ParameterValue::Bool(false)),
            (p::CHORUS, ParameterValue::Bool(false)),
        ] {
            state = DEFINITION.patch_parameter(&state, id, value).unwrap();
        }
        let device = DeviceSnapshot {
            id,
            automation: Vec::new(),
            comparison: None,
            bypassed: false,
            state,
        };
        match location {
            0 => snapshot.tracks[0].devices.push(device),
            1 => {
                snapshot.tracks[0].sends.push(SendLevelSnapshot {
                    return_track: id + 1,
                    gain: 0.5,
                });
                snapshot.return_tracks.push(ReturnTrackSnapshot {
                    id: id + 1,
                    name: "room".into(),
                    gain: 1.0,
                    muted: false,
                    devices: vec![device],
                });
            }
            _ => snapshot.main_devices.push(device),
        }
        let source = Project::from_snapshot(snapshot).unwrap();
        let device = match location {
            0 => source.tracks()[0].devices()[0].id(),
            1 => source.return_tracks()[0].devices()[0].id(),
            _ => source.main_devices()[0].id(),
        };
        let table = Arc::new(hex_devices::SineTable::new());
        let make = |project: &Project, previous: Option<&PreparedSessionInventory>| {
            SessionPlayback::new(Arc::new(
                PreparedRevision::prepare(project, &BTreeMap::new(), Arc::clone(&table), previous)
                    .unwrap(),
            ))
            .unwrap()
        };
        let mut editor = Editor::new(source.clone(), 16).unwrap();
        let mut candidate = make(&source, None);
        let mut wet = make(&source, None);
        editor
            .execute(Command::ToggleDeviceBypass { device })
            .unwrap();
        let mut dry = make(editor.project(), None);
        for playback in [&mut candidate, &mut wet, &mut dry] {
            playback.apply(SessionPlaybackCommand::Play).unwrap();
            render(playback, 6000, 127);
        }
        for bypassed in [true, false] {
            if !bypassed {
                editor
                    .execute(Command::ToggleDeviceBypass { device })
                    .unwrap();
            }
            let mut next = make(editor.project(), Some(candidate.revision.session()));
            next.try_inherit_state_from(&mut candidate).unwrap();
            assert!(next.tail_groups.iter().all(Option::is_none));
            assert!(next.return_effect_tails.slots().iter().all(Option::is_none));
            let actual = render(&mut next, 512, 7);
            let wet = render(&mut wet, 512, 127);
            let dry = render(&mut dry, 512, 64);
            assert!(
                wet.iter()
                    .zip(&dry)
                    .any(|(w, d)| (w[0] - d[0]).abs() > 0.001)
            );
            for (index, ((actual, wet), dry)) in actual.iter().zip(wet).zip(dry).enumerate() {
                let progress = (index + 1).min(240) as f32 / 240.0;
                let mix = if bypassed { 1.0 - progress } else { progress };
                for lane in 0..2 {
                    let expected = dry[lane] + (wet[lane] - dry[lane]) * mix;
                    assert!(
                        (actual[lane] - expected).abs() < 1e-5,
                        "location={location} bypassed={bypassed} frame={index}: {actual:?} expected={expected}"
                    );
                }
            }
            candidate = next;
        }
    }
}
