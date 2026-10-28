//! Isolated finite analytic rendering. Public preparation/native constructor
//! evidence lives in the ordinary inventory and actual CURRENT consumer tests.
use super::*;
use hex_project::{Command, Created, Editor};
use std::collections::BTreeMap;

fn beat(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}

#[test]
fn voice_preparation_quote_rejects_invalid_counts_and_overflow() {
    let empty = VoicePreparationQuote::from_counts(0, 0, 0, 0).unwrap();
    assert_eq!(
        (empty.retained, empty.workspace, empty.data_rows),
        (0, 0, 0)
    );
    let inactive = VoicePreparationQuote::from_counts(0, 0, 0, 3).unwrap();
    assert_eq!(inactive.retained, 0);
    assert_eq!(
        inactive.workspace,
        3 * std::mem::size_of::<hex_project::MidiVoiceGainIdentity>()
    );
    assert!(VoicePreparationQuote::from_counts(0, 1, 0, 1).is_none());
    assert!(VoicePreparationQuote::from_counts(1, 0, 0, 1).is_none());
    assert!(VoicePreparationQuote::from_counts(1, 2, 0, 1).is_none());
    assert!(VoicePreparationQuote::from_counts(usize::MAX, 1, 0, 1).is_none());
    assert!(VoicePreparationQuote::from_counts(1, 1, usize::MAX, 1).is_none());
    assert!(VoicePreparationQuote::from_counts(1, 1, 0, usize::MAX).is_none());
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
fn grouping_differently_gained_clips_refuses_without_changing_release_tails() {
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
    let snapshot = editor.project().snapshot();
    let error = editor.execute(Command::GroupClips(clips)).unwrap_err();
    assert!(error.to_string().contains("different clip gains"));
    assert_eq!(editor.project().snapshot(), snapshot);
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
    for (frame, (left, right)) in before.iter().zip(after).enumerate() {
        for channel in 0..2 {
            assert!(
                (left[channel] - right[channel]).abs() < 1e-6,
                "frame {frame}, channel {channel}: {} vs {}",
                left[channel],
                right[channel]
            );
        }
    }
}

#[test]
fn grouping_different_curves_refuses_without_changing_release_tails() {
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
    let snapshot = editor.project().snapshot();
    let error = editor.execute(Command::GroupClips(clips)).unwrap_err();
    assert!(error.to_string().contains("clip volume curves"));
    assert_eq!(editor.project().snapshot(), snapshot);
    let after = render(
        &crate::compile::project(editor.project(), &BTreeMap::new()).unwrap(),
        &[64, 1, 127],
        8500,
    );
    assert!(before[4000..4100].iter().any(|frame| frame[0].abs() > 1e-4));
    for (frame, (left, right)) in before.iter().zip(after).enumerate() {
        for channel in 0..2 {
            assert!(
                (left[channel] - right[channel]).abs() < 1e-6,
                "frame {frame}, channel {channel}: {} vs {}",
                left[channel],
                right[channel]
            );
        }
    }
}

#[test]
fn grouped_different_curves_keep_each_release_tail_at_its_source_gain() {
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
    editor.execute(Command::GroupClips(clips)).unwrap();
    let after = render(
        &crate::compile::project(editor.project(), &BTreeMap::new()).unwrap(),
        &[64, 1, 127],
        8500,
    );
    assert!(before[4000..4100].iter().any(|frame| frame[0].abs() > 1e-4));
    for (frame, (left, right)) in before.iter().zip(after).enumerate() {
        for channel in 0..2 {
            assert!(
                (left[channel] - right[channel]).abs() < 1e-6,
                "frame {frame}, channel {channel}: {} vs {}",
                left[channel],
                right[channel]
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
    // Exercise the actual analytic renderer with two note owners on one
    // grouped source. Admission is still guarded, so this component test
    // injects the prepared owners after compiling the otherwise valid graph.
    let mut grouped = Editor::new(editor.project().clone(), 8).unwrap();
    let Created::Clip(grouped_id) = grouped.execute(Command::GroupClips(clips.clone())).unwrap()
    else {
        panic!("grouped clip");
    };
    let grouped_clip = grouped.project().midi_clip(grouped_id).unwrap();
    let source = grouped_clip.source_playback().unwrap();
    let timing =
        crate::PreparedTempoMap::prepare(grouped.project().tempo_map().clone(), 8000).unwrap();
    let mut voice_rows = Vec::with_capacity(2);
    let mut voice_programs = Vec::new();
    let mut metadata_bytes = 0;
    let mut law_quote = 0;
    for note in grouped_clip.notes() {
        let (start, value) = if note.pitch() == 60 {
            (0, 0.5)
        } else {
            (1, 2.0)
        };
        let curve = MidiClipVolume::new(
            vec![MidiClipVolumePoint::linear(
                beat(0),
                hex_project::MidiClipGain::new(value).unwrap(),
            )],
            beat(1),
        )
        .unwrap();
        let layer =
            hex_project::MidiVoiceGainLayer::new(beat(start), beat(start + 1), beat(2), curve)
                .unwrap();
        let law = hex_project::MidiVoiceGain::new(vec![layer]).unwrap();
        law_quote += crate::midi_clip_volume::voice::retained_law_quote(law.layers()).unwrap();
        let program =
            crate::midi_clip_volume::voice::PreparedVoiceGain::prepare(&law, source, &timing)
                .unwrap();
        metadata_bytes += program.retained_control_metadata_bytes();
        voice_rows.push(PreparedArrangementVoiceRow {
            clip: grouped_id,
            note: note.id(),
            start: beat(0),
            source,
            program: voice_programs.len(),
        });
        voice_programs.push(program);
    }
    voice_rows.sort_unstable_by_key(|row| (row.clip, row.note));
    let mut grouped_graph = crate::compile::project(grouped.project(), &BTreeMap::new()).unwrap();
    let prepared_voices = PreparedArrangementVolumes {
        rows: Vec::new(),
        voice_rows,
        voice_programs,
        metadata_bytes,
    };
    let identity = prepared_voices.preparation_identity().unwrap();
    assert!(identity.matches(&prepared_voices));
    let expected_bytes = prepared_voices.allocated_row_bytes().unwrap()
        + prepared_voices.retained_control_metadata_bytes()
        + identity.retained_bytes().unwrap();
    let quote = VoicePreparationQuote::from_counts(2, 2, law_quote, 2).unwrap();
    assert!(expected_bytes <= quote.retained);
    assert_eq!(quote.data_rows, 12);
    let mut ledger =
        crate::AudioRootStorageLedger::try_new_on_control(crate::AudioStorageLedgerReservation {
            data_rows: 64,
            handle_rows: 8,
            root_rows: 8,
            max_retained_bytes: usize::MAX,
            max_construction_peak_bytes: usize::MAX,
        })
        .unwrap();
    prepared_voices.visit_storage(&mut ledger).unwrap();
    identity.visit_storage(&mut ledger).unwrap();
    let summary = ledger.summary().unwrap();
    assert_eq!(summary.data_bytes, expected_bytes);
    // The two empty outer clip-volume vectors are accounted by the existing
    // arrangement quote; the additional voice visitors fit this quote.
    assert!(summary.data_allocations <= quote.data_rows + 2);
    drop(ledger);
    let mut changed = prepared_voices.clone();
    changed.voice_rows[0].program = usize::MAX;
    assert!(!identity.matches(&changed));
    let mut changed = prepared_voices.clone();
    changed.voice_programs.swap(0, 1);
    assert!(!identity.matches(&changed));
    grouped_graph.tracks[0].instrument.as_mut().unwrap().volumes = Some(prepared_voices);
    let grouped_audio = render(&grouped_graph, &[17, 256], 8500);
    assert_eq!(grouped_audio, render(&grouped_graph, &[1, 63, 511], 8500));
    for (frame, sample) in grouped_audio.iter().enumerate() {
        for (channel, value) in sample.iter().enumerate() {
            let expected = (f64::from(isolated[0][frame][channel]) * 0.5) as f32
                + (f64::from(isolated[1][frame][channel]) * 2.0) as f32;
            assert!(
                (*value - expected).abs() < 1e-7,
                "frame {frame}, channel {channel}: {value} vs {expected}"
            );
        }
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
