//! Isolated finite analytic rendering. Public preparation/native constructor
//! evidence lives in the ordinary inventory and actual CURRENT consumer tests.
use super::*;
use hex_project::{Command, Created, Editor};
use std::collections::BTreeMap;

fn beat(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
fn render(project: &crate::Project, partitions: &[usize], frames: usize) -> Vec<crate::Frame> {
    let mut renderer = crate::Renderer::new(project);
    renderer.play();
    let mut result = vec![[0.0; 2]; frames];
    let mut offset = 0;
    let mut index = 0;
    while offset < frames {
        let count = partitions[index % partitions.len()].min(frames - offset);
        renderer
            .render(&mut result[offset..offset + count])
            .unwrap();
        offset += count;
        index += 1;
    }
    result
}

#[test]
fn grouping_differently_gained_clips_uses_one_curve_for_release_tails() {
    let mut editor = Editor::new(Project::new(8000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "keys".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let mut clips = Vec::new();
    for (start, pitch, db) in [(0, 60, -6_000), (1, 67, 6_000)] {
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: beat(start),
                length: beat(1),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: beat(0),
                duration: beat(1),
                pitch,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        editor
            .execute(Command::SetMidiClipGain {
                clip,
                gain: hex_project::ClipGain::new(db).unwrap(),
            })
            .unwrap();
        clips.push(clip);
    }
    let before = render(
        &crate::compile::project(editor.project(), &BTreeMap::new()).unwrap(),
        &[17, 256],
        8500,
    );
    editor.execute(Command::GroupClips(clips)).unwrap();
    let after = render(
        &crate::compile::project(editor.project(), &BTreeMap::new()).unwrap(),
        &[17, 256],
        8500,
    );
    assert!(
        before[4000..4100]
            .iter()
            .any(|sample| sample[0].abs() > 1e-4)
    );
    for (frame, (left, right)) in before.iter().zip(&after).take(4000).enumerate() {
        for channel in 0..2 {
            assert!(
                (left[channel] - right[channel]).abs() < 1e-6,
                "frame {frame}"
            );
        }
    }
    assert!((before[4000][0] - after[4000][0]).abs() > 1e-4);
    let partitioned = render(
        &crate::compile::project(editor.project(), &BTreeMap::new()).unwrap(),
        &[64, 1, 127],
        8500,
    );
    for (frame, (left, right)) in after.iter().zip(partitioned).enumerate() {
        for channel in 0..2 {
            assert!(
                (left[channel] - right[channel]).abs() < 1e-6,
                "frame {frame}"
            );
        }
    }
}

#[test]
fn grouping_different_curves_uses_one_curve_for_release_tails() {
    use hex_project::{MidiClipGain, MidiClipVolume, MidiClipVolumePoint};
    let mut editor = Editor::new(Project::new(8000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "keys".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let mut clips = Vec::new();
    for (start, pitch, gain) in [(0, 60, 0.5), (1, 67, 2.0)] {
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: beat(start),
                length: beat(1),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: beat(0),
                duration: beat(1),
                pitch,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        editor
            .execute(Command::ReplaceMidiClipVolume {
                clip,
                volume: MidiClipVolume::new(
                    vec![MidiClipVolumePoint::linear(
                        beat(0),
                        MidiClipGain::new(gain).unwrap(),
                    )],
                    beat(1),
                )
                .unwrap(),
            })
            .unwrap();
        clips.push(clip);
    }
    let before = render(
        &crate::compile::project(editor.project(), &BTreeMap::new()).unwrap(),
        &[17, 256],
        8500,
    );
    // An independent project with both clips at the second level supplies
    // the expected shared-curve output once the second clip starts.
    let mut uniform = Editor::new(editor.project().clone(), 1).unwrap();
    uniform
        .execute(Command::ReplaceMidiClipVolume {
            clip: clips[0],
            volume: MidiClipVolume::new(
                vec![MidiClipVolumePoint::linear(
                    beat(0),
                    MidiClipGain::new(2.0).unwrap(),
                )],
                beat(1),
            )
            .unwrap(),
        })
        .unwrap();
    let expected_tail = render(
        &crate::compile::project(uniform.project(), &BTreeMap::new()).unwrap(),
        &[31, 97],
        8500,
    );
    editor.execute(Command::GroupClips(clips)).unwrap();
    let after = render(
        &crate::compile::project(editor.project(), &BTreeMap::new()).unwrap(),
        &[17, 256],
        8500,
    );
    assert!(before[4000..4100].iter().any(|frame| frame[0].abs() > 1e-4));
    for (frame, (left, right)) in before.iter().zip(&after).take(4000).enumerate() {
        for channel in 0..2 {
            assert!(
                (left[channel] - right[channel]).abs() < 1e-6,
                "frame {frame}"
            );
        }
    }
    assert!((before[4000][0] - after[4000][0]).abs() > 1e-4);
    for (frame, (actual, expected)) in after.iter().zip(&expected_tail).enumerate().skip(4000) {
        for channel in 0..2 {
            assert!(
                (actual[channel] - expected[channel]).abs() < 1e-6,
                "frame {frame}"
            );
        }
    }
    let partitioned = render(
        &crate::compile::project(editor.project(), &BTreeMap::new()).unwrap(),
        &[64, 1, 127],
        8500,
    );
    for (frame, (left, right)) in after.iter().zip(partitioned).enumerate() {
        for channel in 0..2 {
            assert!(
                (left[channel] - right[channel]).abs() < 1e-6,
                "frame {frame}"
            );
        }
    }
}
#[test]
fn real_finite_voices_keep_independent_gain_endpoint_tails_and_partition_identity() {
    let mut editor = Editor::new(Project::new(8000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "keys".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let mut clips = Vec::new();
    for (start, pitch) in [(0, 60), (1, 67)] {
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: beat(start),
                length: beat(1),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: beat(0),
                duration: beat(1),
                pitch,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        clips.push(clip);
    }
    let mut isolated = Vec::new();
    for removed in [clips[1], clips[0]] {
        let mut one = Editor::new(editor.project().clone(), 4).unwrap();
        one.execute(Command::RemoveMidiClip(removed)).unwrap();
        isolated.push(render(
            &crate::compile::project(one.project(), &BTreeMap::new()).unwrap(),
            &[256],
            8500,
        ));
    }
    let mut compiled = crate::compile::project(editor.project(), &BTreeMap::new()).unwrap();
    let curve = MidiClipVolume::new(
        vec![
            MidiClipVolumePoint::linear(beat(0), hex_project::MidiClipGain::new(0.0).unwrap()),
            MidiClipVolumePoint {
                beat: beat(1),
                incoming: hex_project::MidiClipGain::new(0.5).unwrap(),
                value: hex_project::MidiClipGain::new(2.0).unwrap(),
            },
        ],
        beat(1),
    )
    .unwrap();
    editor
        .execute(Command::ReplaceMidiClipVolume {
            clip: clips[0],
            volume: curve,
        })
        .unwrap();
    let timing =
        crate::PreparedTempoMap::prepare(editor.project().tempo_map().clone(), 8000).unwrap();
    // Attach the actual prepared owner directly only for this component test;
    // this remains isolated analytic evidence, separate from actual publication.
    compiled.tracks[0].instrument.as_mut().unwrap().volumes = PreparedArrangementVolumes::prepare(
        editor.project(),
        editor.project().track(track).unwrap(),
        &timing,
    )
    .unwrap();
    let mut untouched = [[9.0, -9.0]; 8];
    assert_eq!(
        compiled.tracks[0].render_additive(0, &mut untouched),
        Err(Invalid("clip volume requires project rendering timing"))
    );
    assert_eq!(untouched, [[9.0, -9.0]; 8]);
    let actual = render(&compiled, &[256], 8500);
    assert_eq!(actual, render(&compiled, &[1, 17, 63, 511, 7], 8500));
    for (frame, sample) in actual.iter().enumerate() {
        let gain = if frame < 4000 {
            0.5 * frame as f64 / 4000.0
        } else {
            0.5
        };
        for (channel, value) in sample.iter().enumerate() {
            let expected = (f64::from(isolated[0][frame][channel]) * gain) as f32
                + isolated[1][frame][channel];
            assert!(
                (*value - expected).abs() < 1.0e-7,
                "frame {frame}, channel {channel}: {value} vs {expected}"
            );
        }
    }
    assert!(
        isolated[0][4000..4100]
            .iter()
            .any(|sample| sample[0].abs() > 1.0e-4),
        "fixture must expose an actual outgoing release tail"
    );
    assert!(
        isolated[1][4000..4100]
            .iter()
            .any(|sample| sample[0].abs() > 1.0e-4),
        "fixture must expose another clip on the same track"
    );

    let scalar = hex_project::ClipGain::new(-6_000).unwrap();
    editor
        .execute(Command::SetMidiClipGain {
            clip: clips[0],
            gain: scalar,
        })
        .unwrap();
    let compiled = crate::compile::project(editor.project(), &BTreeMap::new()).unwrap();
    let scaled = render(&compiled, &[1, 17, 63, 511, 7], 8500);
    for (frame, sample) in scaled.iter().enumerate() {
        let curve = if frame < 4000 {
            0.5 * frame as f64 / 4000.0
        } else {
            0.5
        };
        for (channel, value) in sample.iter().enumerate() {
            let expected = (f64::from(isolated[0][frame][channel])
                * curve
                * f64::from(scalar.linear())) as f32
                + isolated[1][frame][channel];
            assert!(
                (*value - expected).abs() < 1.0e-7,
                "frame {frame}, channel {channel}: {value} vs {expected}"
            );
        }
    }
}
