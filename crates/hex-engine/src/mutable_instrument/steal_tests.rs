//! Admission for a provider that declares `steals_releasing_voices`: only
//! proofs still gated at an onset occupy a voice slot. Released proofs keep
//! their quiet deadlines for retirement only.
use super::*;
use hex_devices::sampler::{
    Mode, PreparedSamplerPlan, Region, SampleChannels, SampleShape, Settings,
};
use hex_lattice::{
    BorrowedDeviceResources, DeviceResourceIdentity, DeviceResourcePcm, DeviceResourceView,
};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetSlot};
use hex_project::{Command, Created, Editor, Project};

const RATE: u32 = 8000;
const RELEASE: u64 = 64;

fn view(pcm: &[f32]) -> DeviceResourceView<'_> {
    DeviceResourceView {
        slot: DeviceAssetSlot::new(1).unwrap(),
        identity: DeviceResourceIdentity {
            asset: DeviceAssetKey::new(1).unwrap(),
            content: [2; 32],
            derivation_version: 1,
            derivation: [3; 32],
        },
        sample_rate: RATE,
        pcm: DeviceResourcePcm::Mono(pcm),
    }
}

/// One-voice looping DC sampler with a 64-frame release.
fn one_voice(view: DeviceResourceView<'_>) -> MutableInstrumentTrackRuntime {
    let frames = view.pcm.frames();
    let mut settings = Settings::new(Region {
        start: 0,
        end: frames,
    });
    settings.mode = Mode::Classic {
        loop_region: Some(Region {
            start: 0,
            end: frames,
        }),
    };
    settings.envelope.release_seconds = RELEASE as f64 / f64::from(RATE);
    let factory = PreparedSamplerPlan::new(
        SampleShape::new(frames, RATE, SampleChannels::Mono).unwrap(),
        RATE,
        settings,
    )
    .unwrap()
    .bind_source_on_control(view)
    .unwrap()
    .prepare_runtime_on_control(1, 259, &hex_lattice::TuningTable::default())
    .unwrap();
    let runtime = MutableInstrumentTrackRuntime::new(&factory, RATE).unwrap();
    assert!(runtime.steals_releasing_voices());
    assert_eq!(runtime.max_active_occurrences(), 1);
    assert!(runtime.max_release_tail_frames() >= RELEASE);
    runtime
}

/// Iterations of one note at `(start, gate_end)` source frames.
fn onsets(spans: &[(u64, u64)]) -> (TrackId, MutableOnsetBuffer) {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "steal".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let note = editor.project().midi_clip(clip).unwrap().notes()[0].id();
    let mut buffer = MutableOnsetBuffer::default();
    for (iteration, &(start, gate)) in spans.iter().enumerate() {
        buffer
            .push(MutableNoteOnset {
                key: MutableOccurrenceKey::new(track, clip, note, iteration as u64, 0),
                onset_beat: BeatTime::from_raw(start),
                gate_end_beat: BeatTime::from_raw(gate),
                source_start: start,
                source_gate_end: gate,
                pitch: 60,
                channel: 0,
                velocity: 127,
            })
            .unwrap();
    }
    (track, buffer)
}

/// Render 16 frames in `partitions`, returning the first refusal.
fn run(
    spans: &[(u64, u64)],
    partitions: &[usize],
) -> Result<MutableInstrumentTrackRuntime, MutableInstrumentRuntimeError> {
    let pcm = [1.0; 16];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let (track, mut onsets) = onsets(spans);
    let mut runtime = one_voice(views[0]);
    let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
    let mut output = [[0.0; 2]; 16];
    let mut offset = 0;
    for &count in partitions {
        runtime.begin_block();
        runtime.process_segment_with_resources(
            track,
            offset as u64,
            offset as u64,
            &mut output[offset..offset + count],
            &mut onsets,
            &[],
            &mut events,
            None,
            &resources,
        )?;
        runtime.commit_block();
        offset += count;
    }
    assert!(output.iter().flatten().all(|v| v.is_finite()));
    Ok(runtime)
}

#[test]
fn one_voice_retrigger_steals_the_release_but_held_overflow_still_refuses() {
    for partitions in [&[16][..], &[4, 12][..], &[1, 3, 1, 11][..]] {
        // A releases at 4, exactly when B starts: NoteOff sorts first, so B
        // steals A's slot. A's proof keeps its quiet deadline beside B's.
        let runtime = run(&[(0, 4), (4, 12)], partitions).unwrap();
        let proofs = runtime.transition_proofs(false);
        assert_eq!(proofs.len(), 2, "{partitions:?}");
        assert!(proofs[0].note_off_sent);
        assert_eq!(
            proofs[0].quiet_deadline_frame,
            4 + runtime.max_release_tail_frames()
        );
        // C starts while B is still held: no releasing slot exists.
        assert_eq!(
            run(&[(0, 4), (4, 12), (8, 16)], partitions).err(),
            Some(MutableInstrumentRuntimeError::OccurrenceCapacity),
            "{partitions:?}"
        );
    }
}

/// Constant half gain for every occurrence through the volume path.
fn half(
    _: MutableOccurrenceSignature,
    at: u64,
    _: usize,
) -> Result<Option<volume::VolumeSlice>, MutableInstrumentRuntimeError> {
    Ok(Some(volume::VolumeSlice {
        at,
        ramp: crate::midi_clip_volume::VolumeRamp {
            start_frame: 0,
            end_frame: u64::MAX,
            start_gain: 0.5,
            end_gain: 0.5,
            next_boundary: None,
        },
        terminal: None,
    }))
}

#[test]
fn stolen_occurrence_receives_no_host_gain_after_the_steal() {
    let pcm = [1.0; 16];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let mut results = Vec::new();
    for partitions in [&[16][..], &[4, 12][..], &[1, 3, 1, 11][..], &[1; 16][..]] {
        let (track, mut onsets) = onsets(&[(0, 4), (4, 12)]);
        let mut runtime = one_voice(views[0]);
        let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
        let mut output = [[0.0; 2]; 16];
        let mut offset = 0;
        for &count in partitions {
            runtime.begin_block();
            // A stale gain for the stolen A would be refused by the sampler.
            runtime
                .process_segment_with_volume(
                    track,
                    offset as u64,
                    offset as u64,
                    &mut output[offset..offset + count],
                    &mut onsets,
                    &[],
                    &mut events,
                    None,
                    &resources,
                    &mut half,
                )
                .unwrap_or_else(|error| panic!("{partitions:?} at {offset}: {error:?}"));
            runtime.commit_block();
            offset += count;
        }
        // A at half until 4; then B at half plus A's release, its gain frozen
        // at half, under the provider's declick; B releases from 12.
        let fade = (f64::from(RATE) / 1000.0).round();
        for (index, frame) in output.iter().enumerate() {
            let i = index as f64;
            let expected = match index {
                0..4 => 0.5,
                4..12 => {
                    let stolen = if i - 4.0 < fade {
                        0.5 * (1.0 - (i - 4.0) / RELEASE as f64) * (1.0 - (i - 4.0) / fade)
                    } else {
                        0.0
                    };
                    0.5 + stolen
                }
                _ => 0.5 * (1.0 - (i - 12.0) / RELEASE as f64),
            };
            assert!(
                (f64::from(frame[0]) - expected).abs() < 1e-6,
                "{partitions:?} frame {index}: {frame:?} != {expected}"
            );
        }
        results.push(output);
    }
    assert!(results.windows(2).all(|pair| pair[0] == pair[1]));
}
