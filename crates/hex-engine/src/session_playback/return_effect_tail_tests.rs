use super::return_effect_tails::{ReturnEffectTail, ReturnEffectTailPlan};
use super::*;
use hex_lattice::{
    NativeProcessorRuntime, NativeRuntimeChain, OutputTail, PreparedNativeProcessor,
    PreparedProcessorInstance, ProcessorChainOwner, StereoMatrix,
};
use hex_lattice_contract::{DeviceTypeId, NativeDeviceMetadata, NativeDeviceState, ParameterValue};
use hex_project::{DeviceSnapshot, ReturnId, ReturnTrackSnapshot};
use std::any::Any;

struct HoldDefinition;

#[derive(Clone)]
struct HoldRuntime {
    remaining: u8,
}

impl PreparedNativeProcessor for HoldDefinition {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }

    fn runtime_version(&self) -> u32 {
        1
    }

    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<HoldRuntime>()
    }

    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite { max_tail_frames: 4 }
    }

    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(HoldRuntime { remaining: 0 })
    }
}

impl NativeProcessorRuntime for HoldRuntime {
    fn process_block(
        &mut self,
        frames: &mut [[f32; 2]],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        for frame in frames {
            if *frame != [0.0; 2] {
                self.remaining = 4;
            } else if self.remaining > 0 {
                *frame = [0.25, -0.5];
                self.remaining -= 1;
            }
        }
    }

    fn reset(&mut self) {
        self.remaining = 0;
    }

    fn is_quiescent(&self) -> bool {
        self.remaining == 0
    }

    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous.as_any().is::<Self>()
    }

    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.remaining = previous.remaining;
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct ScaleDefinition(f32);

#[derive(Clone)]
struct ScaleRuntime(f32);

struct TailDefinition {
    frames: u64,
    bytes: usize,
}

impl PreparedNativeProcessor for ScaleDefinition {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }

    fn runtime_version(&self) -> u32 {
        1
    }

    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<ScaleRuntime>()
    }

    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite { max_tail_frames: 0 }
    }

    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(ScaleRuntime(self.0))
    }
}

impl NativeProcessorRuntime for ScaleRuntime {
    fn process_block(
        &mut self,
        frames: &mut [[f32; 2]],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        for frame in frames {
            frame[0] *= self.0;
            frame[1] *= self.0;
        }
    }

    fn reset(&mut self) {}

    fn is_quiescent(&self) -> bool {
        true
    }

    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous.as_any().is::<Self>()
    }

    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.0 = previous.0;
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl PreparedNativeProcessor for TailDefinition {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }

    fn runtime_version(&self) -> u32 {
        1
    }

    fn declared_runtime_bytes(&self) -> usize {
        self.bytes
    }

    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite {
            max_tail_frames: self.frames,
        }
    }

    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(HoldRuntime { remaining: 0 })
    }
}

const HOLD_TYPE: DeviceTypeId = DeviceTypeId::from_static("test.return.hold");
const SCALE_TYPE: DeviceTypeId = DeviceTypeId::from_static("test.return.scale");
const TAIL_TYPE: DeviceTypeId = DeviceTypeId::from_static("test.return.tail");

fn processor_instances(
    return_id: ReturnId,
    tail_matrix: StereoMatrix,
) -> Vec<PreparedProcessorInstance> {
    processor_instances_with_scale(return_id, tail_matrix, 2.0, 2)
}

fn processor_instances_with_scale(
    return_id: ReturnId,
    tail_matrix: StereoMatrix,
    scale: f32,
    state_value: u8,
) -> Vec<PreparedProcessorInstance> {
    let hold_state = Arc::new(NativeDeviceState::new(HOLD_TYPE, 1, vec![1]).unwrap());
    let scale_state = Arc::new(NativeDeviceState::new(SCALE_TYPE, 1, vec![state_value]).unwrap());
    vec![
        PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Return(return_id.value()),
            device_id: 20,
            device_type: HOLD_TYPE,
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48_000,
            configuration: Some(hold_state),
            definition: Some(Arc::new(HoldDefinition)),
        },
        PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Return(return_id.value()),
            device_id: 21,
            device_type: SCALE_TYPE,
            matrix: tail_matrix,
            bypassed: false,
            sample_rate: 48_000,
            configuration: Some(scale_state),
            definition: Some(Arc::new(ScaleDefinition(scale))),
        },
    ]
}

fn tail_instances(
    return_id: ReturnId,
    count: usize,
    frames: u64,
    bytes: usize,
) -> Vec<PreparedProcessorInstance> {
    (0..count)
        .map(|index| PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Return(return_id.value()),
            device_id: 100 + index as u64,
            device_type: TAIL_TYPE,
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48_000,
            configuration: Some(Arc::new(
                NativeDeviceState::new(TAIL_TYPE, 1, vec![index as u8]).unwrap(),
            )),
            definition: Some(Arc::new(TailDefinition { frames, bytes })),
        })
        .collect()
}

fn active_tail_chain(
    return_id: ReturnId,
    count: usize,
    frames: u64,
    bytes: usize,
) -> NativeRuntimeChain {
    let mut chain =
        NativeRuntimeChain::new(tail_instances(return_id, count, frames, bytes)).unwrap();
    chain.begin_block();
    chain.process_block(&mut [[0.1, -0.1]]).unwrap();
    chain.commit_block();
    chain
}

fn prepared_return_project(include_return: bool, master_gain: f32) -> Project {
    prepared_return_project_with_bus(include_return, master_gain, 0.75, false)
}

fn prepared_return_project_with_bus(
    include_return: bool,
    master_gain: f32,
    return_gain: f32,
    return_muted: bool,
) -> Project {
    let mut snapshot = super::tests::project(1.0, 0, 8 << 32).snapshot();
    snapshot.master_gain = master_gain;
    if include_return {
        let return_id = snapshot.next_id;
        snapshot.next_id += 1;
        snapshot.tracks[0]
            .sends
            .push(hex_project::SendLevelSnapshot {
                return_track: return_id,
                gain: 1.0,
            });
        snapshot.return_tracks.push(ReturnTrackSnapshot {
            id: return_id,
            name: "return".into(),
            gain: return_gain,
            muted: return_muted,
            devices: vec![DeviceSnapshot {
                modulation: Default::default(),
                parent: None,
                automation: Vec::new(),
                comparison: None,
                id: snapshot.next_id,
                bypassed: false,
                state: {
                    let mut state = hex_devices::utility::UTILITY_DEFINITION
                        .default_state()
                        .unwrap();
                    state = hex_devices::utility::UTILITY_DEFINITION
                        .patch_parameter(
                            &state,
                            hex_devices::utility::parameter::WIDTH,
                            ParameterValue::Float(0.0),
                        )
                        .unwrap();
                    hex_devices::utility::UTILITY_DEFINITION
                        .patch_parameter(
                            &state,
                            hex_devices::utility::parameter::DC_REMOVE,
                            ParameterValue::Bool(true),
                        )
                        .unwrap()
                },
            }],
        });
        snapshot.next_id += 1;
    } else {
        snapshot.tracks[0].sends.clear();
        snapshot.return_tracks.clear();
    }
    Project::from_snapshot(snapshot).unwrap()
}

fn eq8_return_project(gain_db: f32, q: f32, kind: u32) -> Project {
    let mut snapshot = super::tests::project(1.0, 0, 8 << 32).snapshot();
    let return_id = snapshot.next_id;
    snapshot.next_id += 1;
    snapshot.tracks[0]
        .sends
        .push(hex_project::SendLevelSnapshot {
            return_track: return_id,
            gain: 1.0,
        });
    let mut state = hex_devices::eq8::EQ8_DEFINITION.default_state().unwrap();
    for parameter in [
        hex_devices::eq8::parameter::enabled(0, 0),
        hex_devices::eq8::parameter::enabled(1, 0),
    ] {
        state = hex_devices::eq8::EQ8_DEFINITION
            .patch_parameter(&state, parameter, ParameterValue::Bool(true))
            .unwrap();
    }
    for (parameter, value) in [
        (
            hex_devices::eq8::parameter::gain(0, 0),
            ParameterValue::Float(gain_db),
        ),
        (
            hex_devices::eq8::parameter::q(0, 0),
            ParameterValue::Float(q),
        ),
        (
            hex_devices::eq8::parameter::kind(0, 0),
            ParameterValue::Choice(kind),
        ),
    ] {
        state = hex_devices::eq8::EQ8_DEFINITION
            .patch_parameter(&state, parameter, value)
            .unwrap();
    }
    snapshot.return_tracks.push(ReturnTrackSnapshot {
        id: return_id,
        name: "eq8 return".into(),
        gain: 1.0,
        muted: false,
        devices: vec![DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: snapshot.next_id,
            bypassed: false,
            state,
        }],
    });
    snapshot.next_id += 1;
    Project::from_snapshot(snapshot).unwrap()
}

fn prepared_matrix_return_project(master_gain: f32) -> Project {
    let mut snapshot = prepared_return_project(true, master_gain).snapshot();
    let state = &mut snapshot.return_tracks[0].devices[0].state;
    *state = hex_devices::utility::UTILITY_DEFINITION
        .patch_parameter(
            state,
            hex_devices::utility::parameter::DC_REMOVE,
            ParameterValue::Bool(false),
        )
        .unwrap();
    Project::from_snapshot(snapshot).unwrap()
}

fn playback(project: &Project) -> SessionPlayback {
    let revision = super::tests::revision(project, Arc::new(hex_devices::SineTable::new()));
    SessionPlayback::new(revision).unwrap()
}

fn active_chain(return_id: ReturnId, matrix: StereoMatrix) -> NativeRuntimeChain {
    let mut chain = NativeRuntimeChain::new(processor_instances(return_id, matrix)).unwrap();
    chain.begin_block();
    chain.process_block(&mut [[0.1, -0.1]]).unwrap();
    chain.commit_block();
    chain
}

fn warm(playback: &mut SessionPlayback) {
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 256];
    playback.render(&mut output).unwrap();
}

#[test]
fn unchanged_main_configuration_with_incompatible_history_refuses_without_losing_old_audio() {
    let project = super::tests::project(1.0, 0, 8 << 32);
    let state = Arc::new(NativeDeviceState::new(HOLD_TYPE, 1, vec![0]).unwrap());
    let chain = |definition: Arc<dyn PreparedNativeProcessor>| {
        NativeRuntimeChain::new(vec![PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Main,
            device_id: 17,
            device_type: HOLD_TYPE,
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48_000,
            configuration: Some(Arc::clone(&state)),
            definition: Some(definition),
        }])
        .unwrap()
    };
    let mut previous = playback(&project);
    let mut oracle = playback(&project);
    previous.main_chain = chain(Arc::new(HoldDefinition));
    oracle.main_chain = chain(Arc::new(HoldDefinition));
    warm(&mut previous);
    warm(&mut oracle);
    let mut candidate = playback(&project);
    // Model an incompatible provider behind unchanged immutable metadata.
    // No changed suffix was selected, so no history exclusion is justified.
    candidate.main_chain = chain(Arc::new(ScaleDefinition(0.5)));
    assert_eq!(
        candidate.try_inherit_state_from(&mut previous),
        Err(SessionPlaybackError::DeviceRuntime)
    );
    assert!(
        candidate
            .return_effect_tails
            .slots()
            .iter()
            .all(Option::is_none)
    );
    let mut actual = [[0.0; 2]; 32];
    let mut expected = actual;
    previous.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(previous.performance_frame, oracle.performance_frame);
}

#[test]
fn capture_retired_only_and_same_id_replacement_preserve_original_tail_routing() {
    for (replacement, partitions, carry) in [
        (false, &[8][..], false),
        (true, &[8][..], false),
        (false, &[1, 2, 5][..], true),
        (true, &[1, 2, 5][..], true),
    ] {
        let mut snapshot = prepared_return_project(true, 1.0).snapshot();
        snapshot.session = hex_project::SessionSnapshot::default();
        // One-frame source creates actual history, then only residuals remain.
        snapshot.assets = vec![hex_project::AssetSnapshot {
            original_source: None,
            id: 30,
            path: "tail.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 32,
            playback_frames: 32,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        }];
        snapshot.tracks[0].content = hex_project::TrackContentSnapshot::Hybrid {
            audio_clips: vec![hex_project::ClipSnapshot {
                active: true,
                name: "clip".into(),
                flavor: hex_project::ClipFlavor::default(),
                edit: None,
                gain: hex_project::ClipGain::UNITY,
                id: 31,
                asset: 30,
                start: 0,
                duration: 1,
                source_offset: 0,
                source_length: 32,
                looping: false,
                source_phase: 0,
                fade: hex_project::ClipFade::neutral(1).unwrap(),
            }],
            midi_clips: Vec::new(),
        };
        snapshot.tracks[0].gain = 0.5;
        snapshot.tracks[0].balance = 0.5;
        snapshot.next_id = 40;
        let source_project = Project::from_snapshot(snapshot).unwrap();
        let source_project = super::tests::with_utility_filter_state(&source_project, true, false);
        let audio = crate::Audio::new(48_000, vec![[0.4, -0.2]; 32]).unwrap();
        let original_pcm = Arc::downgrade(&audio.frames);
        let media = BTreeMap::from([(source_project.assets().next().unwrap().id(), audio)]);
        let table = Arc::new(hex_devices::SineTable::new());
        let prepare = |project: &Project| {
            Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
        };
        let mut old = SessionPlayback::new(prepare(&source_project)).unwrap();
        let track_id = old.routes[0].id;
        let return_id = old.revision.arrangement().return_buses()[0].id;
        let mut track_instances = processor_instances(return_id, StereoMatrix::identity());
        for instance in &mut track_instances {
            instance.owner = ProcessorChainOwner::Track(track_id.value());
        }
        old.device_chains[0] = NativeRuntimeChain::new(track_instances).unwrap();
        old.return_chains[0] = NativeRuntimeChain::new(processor_instances(
            return_id,
            StereoMatrix::new([[0.5, 0.5], [0.5, 0.5]]).unwrap(),
        ))
        .unwrap();
        old.apply(SessionPlaybackCommand::Play).unwrap();
        old.render(&mut [[0.0; 2]; 1]).unwrap();
        let donor_plan = old
            .output_capture_plan(&[OutputCaptureId::Track(track_id)], 8)
            .unwrap();
        let mut candidate_snapshot = source_project.snapshot();
        candidate_snapshot.master_gain = 0.5;
        if replacement {
            candidate_snapshot.tracks[0].gain = 0.25;
            candidate_snapshot.tracks[0].balance = -0.5;
            candidate_snapshot.tracks[0].devices.clear();
            if let hex_project::TrackContentSnapshot::Hybrid { audio_clips, .. } =
                &mut candidate_snapshot.tracks[0].content
            {
                audio_clips[0].duration = 32;
                audio_clips[0].fade = hex_project::ClipFade::neutral(32).unwrap();
            }
            candidate_snapshot.return_tracks[0].gain = 0.25;
            candidate_snapshot.return_tracks[0].devices[0].state =
                hex_devices::utility::UTILITY_DEFINITION
                    .patch_parameter(
                        &candidate_snapshot.return_tracks[0].devices[0].state,
                        hex_devices::utility::parameter::DC_REMOVE,
                        ParameterValue::Bool(false),
                    )
                    .unwrap();
        } else {
            candidate_snapshot.tracks.clear();
            for source in &mut candidate_snapshot.native_sources {
                source.receiver = None;
            }
            candidate_snapshot.return_tracks.clear();
        }
        let candidate_project = Project::from_snapshot(candidate_snapshot).unwrap();
        let mut candidate = SessionPlayback::new(prepare(&candidate_project)).unwrap();
        let before_inherit_plan = candidate.output_capture_plan(&[], 8).unwrap();
        candidate.try_inherit_state_from(&mut old).unwrap();
        let mut donor_main = [[9.0; 2]; 8];
        let mut donor_buffer = donor_main;
        let mut donor_buffers: [&mut [Frame]; 1] = [&mut donor_buffer];
        let mut donor_capture = OutputCapture::new(&donor_plan, &mut donor_buffers);
        let donor_clock = old.performance_frame();
        assert_eq!(
            old.render_with_capture(&mut donor_main, &mut donor_capture),
            Err(SessionPlaybackError::Capture(OutputCaptureError::StalePlan))
        );
        assert_eq!(donor_capture.valid_frames(), 0);
        assert_eq!(old.performance_frame(), donor_clock);
        assert!(donor_main.iter().flatten().all(|sample| *sample == 0.0));
        assert_eq!(
            candidate
                .tail_track_ids
                .iter()
                .flatten()
                .copied()
                .collect::<Vec<_>>(),
            vec![track_id]
        );
        assert_eq!(
            candidate
                .return_effect_tails
                .slots()
                .iter()
                .flatten()
                .count(),
            1
        );
        let mut none: [&mut [Frame]; 0] = [];
        let mut stale = OutputCapture::new(&before_inherit_plan, &mut none);
        assert_eq!(
            candidate.render_with_capture(&mut [[9.0; 2]; 8], &mut stale),
            Err(SessionPlaybackError::Capture(OutputCaptureError::StalePlan))
        );
        if carry {
            let mut next = SessionPlayback::new(prepare(&candidate_project)).unwrap();
            next.try_inherit_state_from(&mut candidate).unwrap();
            if !replacement {
                assert_eq!(
                    candidate
                        .output_capture_plan(&[OutputCaptureId::Track(track_id)], 8)
                        .unwrap_err(),
                    OutputCaptureError::MissingOwner
                );
                assert_eq!(
                    candidate
                        .output_capture_plan(&[OutputCaptureId::Return(return_id)], 8)
                        .unwrap_err(),
                    OutputCaptureError::MissingOwner
                );
            }
            candidate = next;
            assert!(candidate.tail_track_ids.contains(&Some(track_id)));
        }
        // Capture plans are separate control owners of their revision, not transferred tails.
        // The exhausted capture is no longer borrowed after its stale-plan assertions above.
        drop(donor_plan);
        // Neither transferred effect tails nor their copied scalar release entries retain old
        // ordinary Audio leaves. Destroy the actual donor on control before rendering its tails.
        drop(media);
        assert!(original_pcm.upgrade().is_some());
        drop(old);
        if !replacement {
            assert!(
                original_pcm.upgrade().is_none(),
                "retiring the donor frees old clip PCM while its effect tail continues"
            );
        }
        let plan = candidate
            .output_capture_plan(
                &[
                    OutputCaptureId::Track(track_id),
                    OutputCaptureId::Return(return_id),
                ],
                512,
            )
            .unwrap();
        candidate.set_device_metering(true);
        let observation =
            crate::PreparedPlaybackObservation::prepare_on_control(&candidate.revision).unwrap();
        let arrangement_before = candidate.arrangement_frame;
        let performance_before = candidate.performance_frame;
        candidate.arrangement_frame = u64::MAX - 300;
        let mut refused_main = [[9.0; 2]; 512];
        let mut refused_track = refused_main;
        let mut refused_return = refused_main;
        let mut refused_buffers: [&mut [Frame]; 2] = [&mut refused_track, &mut refused_return];
        let mut refused_capture = OutputCapture::new(&plan, &mut refused_buffers);
        assert!(
            candidate
                .render_with_capture(&mut refused_main, &mut refused_capture)
                .is_err()
        );
        assert_eq!(refused_capture.valid_frames(), 0);
        assert_eq!(candidate.performance_frame, performance_before);
        assert_eq!(candidate.arrangement_frame, u64::MAX - 300);
        assert!(refused_main.iter().flatten().all(|sample| *sample == 0.0));
        assert!(refused_capture.output(0).unwrap().is_empty());
        if replacement {
            assert_eq!(
                observation
                    .track_output_meter(track_id)
                    .unwrap()
                    .take_track_output(),
                0.0
            );
            assert_eq!(
                observation
                    .return_output_meter(return_id)
                    .unwrap()
                    .take_track_output(),
                0.0
            );
        }
        candidate.arrangement_frame = arrangement_before;
        let mut main = [[0.0; 2]; 8];
        let mut track = main;
        let mut returned = main;
        let mut offset = 0;
        for &frames in partitions {
            let mut buffers: [&mut [Frame]; 2] = [
                &mut track[offset..offset + frames],
                &mut returned[offset..offset + frames],
            ];
            let mut capture = OutputCapture::new(&plan, &mut buffers);
            candidate
                .render_with_capture(&mut main[offset..offset + frames], &mut capture)
                .unwrap();
            assert_eq!(capture.valid_frames(), frames);
            if replacement {
                for (index, meter) in [
                    observation.track_output_meter(track_id).unwrap(),
                    observation.return_output_meter(return_id).unwrap(),
                ]
                .into_iter()
                .enumerate()
                {
                    let expected = capture
                        .output(index)
                        .unwrap()
                        .iter()
                        .flatten()
                        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
                    assert_eq!(meter.take_track_output(), expected);
                }
            }
            offset += frames;
        }
        for index in 0..8 {
            let current = if replacement {
                [0.1, -0.1 + 0.075 * (index + 1) as f32 / 240.0]
            } else {
                [0.0; 2]
            };
            let current_return = if replacement {
                [(current[0] + current[1]) * 0.5 * 0.25; 2]
            } else {
                [0.0; 2]
            };
            let retired_track = if index < 4 { [0.125, -0.5] } else { [0.0; 2] };
            let retired_return = if index < 4 { [-0.1875; 2] } else { [0.0; 2] };
            for channel in 0..2 {
                let expected_track = current[channel] + retired_track[channel];
                let expected_return = current_return[channel] + retired_return[channel];
                assert!(
                    (track[index][channel] - expected_track).abs() < 1e-7,
                    "track frame {index}"
                );
                assert!(
                    (returned[index][channel] - expected_return).abs() < 1e-7,
                    "return frame {index}, replacement {replacement}, actual {:?}, expected {expected_return}",
                    returned[index]
                );
                // Retired track tail must NOT send into current Return.
                assert!(
                    (main[index][channel] - (expected_track + expected_return) * 0.5).abs() < 1e-7
                );
            }
        }
        let actual = candidate.accounted_runtime_storage_bytes().unwrap();
        assert!(actual >= std::mem::size_of::<SessionPlayback>());
        assert!(
            std::mem::size_of::<SessionPlayback>()
                >= std::mem::size_of::<[Option<TrackId>; SESSION_TAIL_GROUP_CAPACITY]>()
        );
    }
}

fn assert_old_return_tail_with_candidate_chain(
    return_id: ReturnId,
    old_matrix: StereoMatrix,
    candidate_instances: Vec<PreparedProcessorInstance>,
) {
    let old_project = prepared_return_project(true, 1.0);
    let new_project = prepared_return_project(true, 0.5);
    let mut previous = playback(&old_project);
    previous.return_chains[0] = active_chain(return_id, old_matrix);
    warm(&mut previous);
    let mut candidate = playback(&new_project);
    candidate.return_chains[0] = NativeRuntimeChain::new(candidate_instances.clone()).unwrap();

    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert_eq!(
        candidate
            .return_effect_tails
            .slots()
            .iter()
            .flatten()
            .count(),
        1
    );
    assert!(candidate.return_chains[0].committed_range_is_quiescent(0..2));

    let mut baseline = playback(&new_project);
    baseline.return_chains[0] = NativeRuntimeChain::new(candidate_instances).unwrap();
    warm(&mut baseline);
    let mut actual = [[0.0; 2]; 8];
    let mut expected = actual;
    candidate.render(&mut actual).unwrap();
    baseline.render(&mut expected).unwrap();
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        let tail_active = index < 4;
        let scale = if tail_active { 0.75 * 0.5 } else { 0.0 };
        let mut matrix_frame = [0.25, -0.5];
        old_matrix.apply_block(std::slice::from_mut(&mut matrix_frame));
        assert!((actual[0] - expected[0] - matrix_frame[0] * 2.0 * scale).abs() < 1.0e-6);
        assert!((actual[1] - expected[1] - matrix_frame[1] * 2.0 * scale).abs() < 1.0e-6);
    }
}

#[test]
fn removed_return_keeps_the_whole_old_chain_and_uses_current_master() {
    let old_project = prepared_matrix_return_project(1.0);
    let new_project = prepared_return_project(false, 0.5);
    let return_id = old_project.return_tracks()[0].id();
    let mut previous = playback(&old_project);
    let old_matrix = previous.revision.arrangement().return_buses()[0].device_chain[0].matrix;
    previous.return_chains[0] = active_chain(return_id, old_matrix);
    warm(&mut previous);
    assert!(!previous.return_chains[0].committed_range_is_quiescent(0..2));

    let mut candidate = playback(&new_project);
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert_eq!(
        candidate
            .return_effect_tails
            .slots()
            .iter()
            .flatten()
            .count(),
        1
    );
    assert!(previous.return_chains[0].is_empty());

    let mut baseline = playback(&new_project);
    warm(&mut baseline);
    let mut actual = [[0.0; 2]; 8];
    let mut expected = actual;
    let mut observed = actual;
    let mut observations = 0;
    candidate
        .render_with_post_node_and_main_observers(
            &mut actual,
            None,
            Some(&mut |frames: &[Frame]| {
                observations += 1;
                observed.copy_from_slice(frames);
            }),
        )
        .unwrap();
    assert_eq!(observations, 1);
    assert_eq!(observed, actual);
    baseline.render(&mut expected).unwrap();
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        let tail_active = index < 4;
        let scale = if tail_active { 0.75 * 0.5 } else { 0.0 };
        let mut matrix_frame = [0.25, -0.5];
        old_matrix.apply_block(std::slice::from_mut(&mut matrix_frame));
        assert!((actual[0] - expected[0] - matrix_frame[0] * 2.0 * scale).abs() < 1.0e-6);
        assert!((actual[1] - expected[1] - matrix_frame[1] * 2.0 * scale).abs() < 1.0e-6);
    }
}

#[test]
fn unchanged_chain_inherits_history_once_without_a_duplicate_tail() {
    let project = prepared_return_project(true, 1.0);
    let return_id = project.return_tracks()[0].id();
    let matrix = playback(&project).revision.arrangement().return_buses()[0].device_chain[0].matrix;
    let instances = processor_instances(return_id, matrix);
    let mut previous = playback(&project);
    previous.return_chains[0] = NativeRuntimeChain::new(instances.clone()).unwrap();
    warm(&mut previous);
    let mut candidate = playback(&project);
    candidate.return_chains[0] = NativeRuntimeChain::new(instances).unwrap();

    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert!(
        candidate
            .return_effect_tails
            .slots()
            .iter()
            .all(Option::is_none)
    );
    let mut actual = [[0.0; 2]; 128];
    let mut expected = actual;
    candidate.render(&mut actual).unwrap();
    previous.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn eq8_return_parameter_edits_inherit_history_once_without_a_duplicate_tail() {
    let mut previous = playback(&eq8_return_project(3.0, 1.0, 0));
    let mut expected_previous = playback(&eq8_return_project(3.0, 1.0, 0));
    warm(&mut previous);
    warm(&mut expected_previous);
    assert!(!previous.return_chains[0].committed_range_is_quiescent(0..1));

    let mut output = [[0.0; 2]; 128];
    let mut expected = output;
    for (gain_db, q, kind) in [(6.0, 1.0, 0), (6.0, 2.0, 0), (6.0, 2.0, 1)] {
        let project = eq8_return_project(gain_db, q, kind);
        let mut candidate = playback(&project);
        let mut expected_candidate = playback(&project);
        candidate.try_inherit_state_from(&mut previous).unwrap();
        expected_candidate
            .try_inherit_state_from(&mut expected_previous)
            .unwrap();
        // Build the PCM oracle from the independently advanced old runtime,
        // explicitly copying only the EQ8 history into the new settings.
        expected_candidate.return_chains[0].reset();
        expected_candidate.return_chains[0].inherit_from(&expected_previous.return_chains[0]);
        assert!(
            candidate
                .return_effect_tails
                .slots()
                .iter()
                .all(Option::is_none)
        );
        assert!(
            expected_candidate
                .return_effect_tails
                .slots()
                .iter()
                .all(Option::is_none)
        );
        assert!(!candidate.return_chains[0].committed_range_is_quiescent(0..1));

        candidate.render(&mut output).unwrap();
        expected_candidate.render(&mut expected).unwrap();
        assert_eq!(output, expected, "retargeted Return EQ8 PCM oracle");
        assert!(output.iter().flatten().all(|sample| sample.is_finite()));
        assert!(output.iter().flatten().any(|sample| sample.abs() > 1.0e-6));
        previous = candidate;
        expected_previous = expected_candidate;
    }
}

#[test]
fn held_eq8_return_audition_survives_compatible_gain_q_and_filter_type_edits() {
    use crate::PostNodeIdentity;
    use hex_lattice::{ProcessorAuditionSelection, ProcessorChainOwner};

    let initial = eq8_return_project(3.0, 1.0, 0);
    let return_track = &initial.return_tracks()[0];
    let device_id = return_track.devices()[0].id().value();
    let identity = PostNodeIdentity::new(
        ProcessorChainOwner::Return(return_track.id().value()),
        device_id,
        DeviceTypeId::from_static("hex.eq8"),
    );
    let selection = ProcessorAuditionSelection { index: 0, lane: 0 };
    let mut previous = playback(&initial);
    warm(&mut previous);
    assert!(previous.select_processor_audition(identity, Some(selection)));
    let mut output = [[0.0; 2]; 128];
    previous.render(&mut output).unwrap();
    assert!(previous.processor_audition_is_active(identity, selection));
    assert!(output.iter().flatten().any(|sample| sample.is_finite()));

    for (gain_db, q, kind) in [(6.0, 1.0, 0), (6.0, 2.0, 0), (6.0, 2.0, 1)] {
        let mut candidate = playback(&eq8_return_project(gain_db, q, kind));
        candidate.try_inherit_state_from(&mut previous).unwrap();
        assert!(
            candidate
                .return_effect_tails
                .slots()
                .iter()
                .all(Option::is_none),
            "compatible EQ8 return retarget must not duplicate its current chain into a tail"
        );
        assert!(candidate.processor_audition_is_active(identity, selection));
        candidate.render(&mut output).unwrap();
        assert!(output.iter().flatten().all(|sample| sample.is_finite()));
        assert!(output.iter().flatten().any(|sample| sample.abs() > 1.0e-6));
        previous = candidate;
    }
}

#[test]
fn gain_only_bus_edit_inherits_history_under_new_controls() {
    let old_project = prepared_return_project_with_bus(true, 1.0, 0.75, false);
    let new_project = prepared_return_project_with_bus(true, 0.5, 0.25, false);
    let return_id = old_project.return_tracks()[0].id();
    let matrix =
        playback(&old_project).revision.arrangement().return_buses()[0].device_chain[0].matrix;
    let instances = processor_instances(return_id, matrix);
    let mut previous = playback(&old_project);
    previous.return_chains[0] = NativeRuntimeChain::new(instances.clone()).unwrap();
    warm(&mut previous);
    let mut candidate = playback(&new_project);
    candidate.return_chains[0] = NativeRuntimeChain::new(instances.clone()).unwrap();

    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert!(
        candidate
            .return_effect_tails
            .slots()
            .iter()
            .all(Option::is_none)
    );

    let mut baseline = playback(&new_project);
    baseline.return_chains[0] = NativeRuntimeChain::new(instances).unwrap();
    warm(&mut baseline);
    let mut actual = [[0.0; 2]; 128];
    let mut expected = actual;
    candidate.render(&mut actual).unwrap();
    baseline.render(&mut expected).unwrap();
    assert_eq!(actual, expected);

    let muted_project = prepared_return_project_with_bus(true, 1.0, 0.75, true);
    let mut previous = playback(&old_project);
    previous.return_chains[0] =
        NativeRuntimeChain::new(processor_instances(return_id, matrix)).unwrap();
    warm(&mut previous);
    let mut muted_candidate = playback(&muted_project);
    muted_candidate.return_chains[0] =
        NativeRuntimeChain::new(processor_instances(return_id, matrix)).unwrap();
    muted_candidate
        .try_inherit_state_from(&mut previous)
        .unwrap();
    assert!(
        muted_candidate
            .return_effect_tails
            .slots()
            .iter()
            .all(Option::is_none)
    );
    assert!(!muted_candidate.return_chains[0].committed_range_is_quiescent(0..2));
}

#[test]
fn changed_return_suffix_keeps_old_prefix_state_and_starts_new_chain_fresh() {
    let old_project = prepared_return_project(true, 1.0);
    let new_project = prepared_return_project(true, 0.5);
    let return_id = old_project.return_tracks()[0].id();
    let mut previous = playback(&old_project);
    let old_matrix = previous.revision.arrangement().return_buses()[0].device_chain[0].matrix;
    previous.return_chains[0] = active_chain(return_id, old_matrix);
    warm(&mut previous);
    let mut candidate = playback(&new_project);
    candidate.return_chains[0] = NativeRuntimeChain::new(processor_instances_with_scale(
        return_id,
        StereoMatrix::new([[0.0, 1.0], [1.0, 0.0]]).unwrap(),
        3.0,
        3,
    ))
    .unwrap();

    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert!(candidate.return_chains[0].committed_range_is_quiescent(0..2));
    assert_eq!(
        candidate
            .return_effect_tails
            .slots()
            .iter()
            .flatten()
            .count(),
        1
    );

    let mut baseline = playback(&new_project);
    baseline.return_chains[0] = NativeRuntimeChain::new(processor_instances_with_scale(
        return_id,
        StereoMatrix::new([[0.0, 1.0], [1.0, 0.0]]).unwrap(),
        3.0,
        3,
    ))
    .unwrap();
    warm(&mut baseline);
    let mut actual = [[0.0; 2]; 8];
    let mut expected = actual;
    candidate.render(&mut actual).unwrap();
    baseline.render(&mut expected).unwrap();
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        let tail_active = index < 4;
        let scale = if tail_active { 0.75 * 0.5 } else { 0.0 };
        let mut matrix_frame = [0.25, -0.5];
        old_matrix.apply_block(std::slice::from_mut(&mut matrix_frame));
        assert!((actual[0] - expected[0] - matrix_frame[0] * 2.0 * scale).abs() < 1.0e-6);
        assert!((actual[1] - expected[1] - matrix_frame[1] * 2.0 * scale).abs() < 1.0e-6);
    }
}

#[test]
fn bypass_and_reorder_changes_keep_the_entire_previous_return_chain() {
    let old_project = prepared_return_project(true, 1.0);
    let return_id = old_project.return_tracks()[0].id();
    let old_matrix =
        playback(&old_project).revision.arrangement().return_buses()[0].device_chain[0].matrix;
    let mut bypassed = processor_instances(return_id, old_matrix);
    bypassed[1].bypassed = true;
    assert_old_return_tail_with_candidate_chain(return_id, old_matrix, bypassed);

    let mut reordered = processor_instances(return_id, old_matrix);
    reordered.reverse();
    assert_old_return_tail_with_candidate_chain(return_id, old_matrix, reordered);
}

#[test]
fn active_return_tail_moves_forward_across_later_revisions() {
    let old_project = prepared_return_project(true, 1.0);
    let first_candidate_project = prepared_return_project(false, 0.5);
    let second_candidate_project = prepared_return_project(false, 0.25);
    let return_id = old_project.return_tracks()[0].id();
    let mut previous = playback(&old_project);
    let old_matrix = previous.revision.arrangement().return_buses()[0].device_chain[0].matrix;
    previous.return_chains[0] = active_chain(return_id, old_matrix);
    warm(&mut previous);

    let mut first_candidate = playback(&first_candidate_project);
    first_candidate
        .try_inherit_state_from(&mut previous)
        .unwrap();
    assert_eq!(
        first_candidate
            .return_effect_tails
            .slots()
            .iter()
            .flatten()
            .count(),
        1
    );

    let mut second_candidate = playback(&second_candidate_project);
    second_candidate
        .try_inherit_state_from(&mut first_candidate)
        .unwrap();
    assert_eq!(
        second_candidate
            .return_effect_tails
            .slots()
            .iter()
            .flatten()
            .count(),
        1
    );
    assert!(
        first_candidate
            .return_effect_tails
            .slots()
            .iter()
            .all(Option::is_none)
    );

    let mut baseline = playback(&second_candidate_project);
    warm(&mut baseline);
    let mut actual = [[0.0; 2]; 8];
    let mut expected = actual;
    second_candidate.render(&mut actual).unwrap();
    baseline.render(&mut expected).unwrap();
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        let tail_active = index < 4;
        let scale = if tail_active { 0.75 * 0.25 } else { 0.0 };
        let mut matrix_frame = [0.25, -0.5];
        old_matrix.apply_block(std::slice::from_mut(&mut matrix_frame));
        assert!((actual[0] - expected[0] - matrix_frame[0] * 2.0 * scale).abs() < 1.0e-6);
        assert!((actual[1] - expected[1] - matrix_frame[1] * 2.0 * scale).abs() < 1.0e-6);
    }
}

#[test]
fn seek_resets_return_tail_state() {
    let project = prepared_return_project(true, 1.0);
    let return_id = project.return_tracks()[0].id();
    let mut playback = playback(&project);
    let matrix = playback.revision.arrangement().return_buses()[0].device_chain[0].matrix;
    let bus = &playback.revision.arrangement().return_buses()[0];
    playback.return_effect_tails.slots_mut()[0] =
        Some(ReturnEffectTail::new(active_chain(return_id, matrix), bus));
    playback.apply(SessionPlaybackCommand::Seek(0)).unwrap();
    let tail = playback.return_effect_tails.slots()[0].as_ref().unwrap();
    assert!(!tail.output_active);
    assert!(tail.chain.committed_range_is_quiescent(0..tail.chain.len()));
}

#[test]
fn muted_or_zero_gain_return_removal_does_not_capture_an_inaudible_chain() {
    let target = prepared_return_project(false, 1.0);
    for (gain, muted) in [(0.75, true), (0.0, false)] {
        let old_project = prepared_return_project_with_bus(true, 1.0, gain, muted);
        let return_id = old_project.return_tracks()[0].id();
        let mut previous = playback(&old_project);
        previous.return_chains[0] = active_chain(return_id, StereoMatrix::identity());
        let mut candidate = playback(&target);
        candidate.try_inherit_state_from(&mut previous).unwrap();
        assert!(
            candidate
                .return_effect_tails
                .slots()
                .iter()
                .all(Option::is_none)
        );
        assert!(!previous.return_chains[0].is_empty());

        let mut previous = playback(&old_project);
        previous.return_chains[0] = active_chain(return_id, StereoMatrix::identity());
        let mut candidate = playback(&target);
        candidate.request_live_restart();
        candidate
            .try_inherit_state_live_from(&mut previous)
            .unwrap();
    }
}

#[test]
fn failed_block_restores_tail_history_for_the_next_render_attempt() {
    let project = prepared_return_project(true, 1.0);
    let return_id = project.return_tracks()[0].id();
    let playback = playback(&project);
    let bus = &playback.revision.arrangement().return_buses()[0];
    let mut bank = ReturnEffectTailBank::new().unwrap();
    bank.slots_mut()[0] = Some(ReturnEffectTail::new(
        active_chain(return_id, StereoMatrix::identity()),
        bus,
    ));

    bank.begin_block();
    let mut failed_attempt = [[0.0; 2]; 1];
    bank.render_chunk(
        &mut failed_attempt,
        0,
        &mut None,
        &mut hex_lattice::RefuseForeign,
    )
    .unwrap();
    assert_eq!(failed_attempt[0], [0.375, -0.75]);
    bank.rollback_block();
    assert!(bank.slots()[0].as_ref().unwrap().output_active);

    bank.begin_block();
    let mut retry = [[0.0; 2]; 1];
    bank.render_chunk(&mut retry, 0, &mut None, &mut hex_lattice::RefuseForeign)
        .unwrap();
    assert_eq!(retry, failed_attempt);
    bank.commit_block();
    assert!(bank.slots()[0].as_ref().unwrap().output_active);
}

#[test]
fn return_tails_share_exact_processor_paired_byte_and_entry_ceilings() {
    let project = prepared_return_project(true, 1.0);
    let return_id = project.return_tracks()[0].id();
    let mut playback = playback(&project);
    let bus = &playback.revision.arrangement().return_buses()[0];
    playback.return_effect_tails.slots_mut()[0] = Some(ReturnEffectTail::new(
        active_tail_chain(return_id, 1, 4, hex_lattice::MAX_PROCESSOR_RUNTIME_BYTES),
        bus,
    ));
    let bank = &playback.return_effect_tails;
    let buses = &[];
    let chains = &[];
    let bytes_cap = 2 * hex_lattice::MAX_PROCESSOR_BANK_RUNTIME_BYTES;
    let mut processors = hex_lattice::MAX_STATEFUL_PROCESSORS - 1;
    let mut bytes = bytes_cap - 2 * hex_lattice::MAX_PROCESSOR_RUNTIME_BYTES;
    let mut entries = 4_095;
    assert!(
        ReturnEffectTailPlan::preflight(
            buses,
            chains,
            bank,
            buses,
            chains,
            48_000,
            &mut processors,
            &mut bytes,
            &mut entries,
        )
        .is_ok()
    );
    assert_eq!(processors, hex_lattice::MAX_STATEFUL_PROCESSORS);
    assert_eq!(bytes, bytes_cap);
    assert_eq!(entries, 4_096);

    let mut processors = hex_lattice::MAX_STATEFUL_PROCESSORS;
    let mut bytes = 0;
    let mut entries = 0;
    assert_eq!(
        ReturnEffectTailPlan::preflight(
            buses,
            chains,
            bank,
            buses,
            chains,
            48_000,
            &mut processors,
            &mut bytes,
            &mut entries,
        )
        .err(),
        Some(SessionPlaybackError::ReleaseBankFull)
    );
    let mut processors = 0;
    let mut bytes = bytes_cap;
    let mut entries = 0;
    assert_eq!(
        ReturnEffectTailPlan::preflight(
            buses,
            chains,
            bank,
            buses,
            chains,
            48_000,
            &mut processors,
            &mut bytes,
            &mut entries,
        )
        .err(),
        Some(SessionPlaybackError::ReleaseBankFull)
    );
    let mut processors = 0;
    let mut bytes = 0;
    let mut entries = 4_096;
    assert_eq!(
        ReturnEffectTailPlan::preflight(
            buses,
            chains,
            bank,
            buses,
            chains,
            48_000,
            &mut processors,
            &mut bytes,
            &mut entries,
        )
        .err(),
        Some(SessionPlaybackError::ReleaseBankFull)
    );
}

#[test]
fn excessive_or_overflowing_finite_return_tail_refuses_atomically_and_retries_after_stop() {
    let project = prepared_return_project(true, 1.0);
    let return_id = project.return_tracks()[0].id();
    let too_long = 48_000 * MAX_FINITE_EFFECT_TAIL_SECONDS + 1;
    for (frames, count) in [(too_long, 1), (u64::MAX, 2)] {
        let mut previous = playback(&project);
        previous.return_chains[0] = active_tail_chain(return_id, count, frames, 8);
        let mut candidate = playback(&prepared_return_project(false, 1.0));
        assert_eq!(
            candidate.try_inherit_state_from(&mut previous),
            Err(SessionPlaybackError::ReleaseBankFull)
        );
        assert!(!previous.return_chains[0].is_empty());
        assert!(
            candidate
                .return_effect_tails
                .slots()
                .iter()
                .all(Option::is_none)
        );

        previous.apply(SessionPlaybackCommand::Stop).unwrap();
        candidate.try_inherit_state_from(&mut previous).unwrap();
        assert!(
            candidate
                .return_effect_tails
                .slots()
                .iter()
                .all(Option::is_none)
        );
    }
}

#[test]
fn full_return_tail_bank_refuses_atomically_and_retries_after_stop() {
    let project = prepared_return_project(true, 1.0);
    let return_id = project.return_tracks()[0].id();
    let mut previous = playback(&project);
    let matrix = previous.revision.arrangement().return_buses()[0].device_chain[0].matrix;
    previous.return_chains[0] = active_chain(return_id, matrix);
    let bus = &previous.revision.arrangement().return_buses()[0];
    for slot in previous.return_effect_tails.slots_mut() {
        *slot = Some(ReturnEffectTail::new(active_chain(return_id, matrix), bus));
    }
    assert!(
        previous.return_effect_tails.slots()[0]
            .as_ref()
            .is_some_and(|tail| !tail.chain.committed_range_is_quiescent(0..tail.chain.len()))
    );
    assert!(
        !previous.return_chains[0].committed_range_is_quiescent(0..previous.return_chains[0].len())
    );
    let mut candidate = playback(&prepared_return_project(false, 1.0));
    let error = candidate.try_inherit_state_from(&mut previous);
    assert_eq!(error, Err(SessionPlaybackError::ReleaseBankFull));
    assert!(
        candidate
            .return_effect_tails
            .slots()
            .iter()
            .all(Option::is_none)
    );
    assert_eq!(
        previous
            .return_effect_tails
            .slots()
            .iter()
            .flatten()
            .count(),
        64
    );
    assert!(!previous.return_chains[0].committed_range_is_quiescent(0..2));

    previous.apply(SessionPlaybackCommand::Stop).unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert!(
        candidate
            .return_effect_tails
            .slots()
            .iter()
            .all(Option::is_none)
    );
}

#[test]
fn feed_capture_includes_retired_return_tail_once_without_refeeding_it() {
    let mut snapshot = prepared_return_project(true, 0.5).snapshot();
    snapshot.tracks.clear();
    for source in &mut snapshot.native_sources {
        source.receiver = None;
    }
    snapshot.return_tracks[0].gain = 1.0;
    snapshot.session = hex_project::SessionSnapshot::default();
    let source_id = snapshot.return_tracks[0].id;
    let target_id = snapshot.next_id;
    let feed_id = target_id + 1;
    snapshot.next_id += 2;
    snapshot.return_tracks.push(ReturnTrackSnapshot {
        id: target_id,
        name: "downstream".into(),
        gain: 1.0,
        muted: false,
        devices: Vec::new(),
    });
    snapshot.return_feeds.push(hex_project::ReturnFeedSnapshot {
        id: feed_id,
        source: source_id,
        destination: target_id,
        gain: 0.5,
    });
    let project = Project::from_snapshot(snapshot).unwrap();
    let mut editor = hex_project::Editor::new(project, 8).unwrap();
    let hex_project::Created::Track(track) = editor
        .execute(hex_project::Command::AddTrack {
            name: "input".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    editor
        .execute(hex_project::Command::SetAudioFeed {
            source: hex_project::RoutingOwner::Return(editor.project().return_tracks()[1].id()),
            destination: track,
            gain: 0.5,
        })
        .unwrap();
    let project = editor.project().clone();
    let mut runtime = playback(&project);
    assert!(runtime.compensation.is_some());
    assert_eq!(runtime.revision.arrangement().main_latency_frames(), 0);
    let source = runtime.revision.arrangement().return_buses()[0].id;
    let target = runtime.revision.arrangement().return_buses()[1].id;
    runtime.return_effect_tails.slots_mut()[0] = Some(ReturnEffectTail::new(
        active_chain(source, StereoMatrix::identity()),
        &runtime.revision.arrangement().return_buses()[0],
    ));
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    let plan = runtime
        .output_capture_plan(
            &[
                OutputCaptureId::Return(source),
                OutputCaptureId::Return(target),
            ],
            8,
        )
        .unwrap();
    let mut offset = 0;
    for count in [1, 2, 5] {
        let mut main = vec![[9.0; 2]; count];
        let mut source = main.clone();
        let mut target = main.clone();
        let mut buffers = [&mut source[..], &mut target[..]];
        let mut capture = OutputCapture::new(&plan, &mut buffers);
        runtime
            .render_with_capture(&mut main, &mut capture)
            .unwrap();
        for (index, frame) in main.iter().enumerate() {
            let tail = if offset + index < 4 {
                [0.5, -1.0]
            } else {
                [0.0; 2]
            };
            assert_eq!(capture.output(0).unwrap()[index], tail);
            assert_eq!(capture.output(1).unwrap()[index], [0.0; 2]);
            assert_eq!(*frame, tail.map(|sample| sample * 0.5));
        }
        offset += count;
    }
}

#[test]
fn detached_track_tail_reaches_group_once_and_unsafe_reparent_refuses() {
    let mut snapshot = prepared_return_project(true, 1.0).snapshot();
    snapshot.tracks[0].content = hex_project::TrackContentSnapshot::Audio(Vec::new());
    snapshot.session = hex_project::SessionSnapshot::default();
    let project = Project::from_snapshot(snapshot).unwrap();
    let track = project.tracks()[0].id();
    let return_id = project.return_tracks()[0].id();
    let mut editor = hex_project::Editor::new(project, 8).unwrap();
    let hex_project::Created::Track(group) = editor
        .execute(hex_project::Command::GroupTracks {
            tracks: hex_project::TrackSelectionIds::new(&[track]).unwrap(),
            name: "group".into(),
        })
        .unwrap()
    else {
        panic!("group")
    };
    editor
        .execute(hex_project::Command::Gain {
            track: group,
            gain: 0.5,
        })
        .unwrap();
    let grouped = editor.project();
    let mut snapshot = grouped.snapshot();
    for track in &mut snapshot.tracks {
        track.kind = hex_project::TrackKind::Ordinary;
        track.parent_group = None;
    }
    let flat = Project::from_snapshot(snapshot).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let prepare = |grouped_graph: bool| {
        let mut revision =
            PreparedRevision::prepare(&flat, &BTreeMap::new(), Arc::clone(&table), None).unwrap();
        if grouped_graph {
            revision.test_install_group_graph(grouped);
        }
        Arc::new(revision)
    };
    let revision = prepare(true);
    let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 1]).unwrap();
    old.tail_groups[0] = Some(active_tail_chain(
        return_id,
        1,
        4,
        std::mem::size_of::<HoldRuntime>(),
    ));
    old.tail_track_ids[0] = Some(track);
    old.tail_mixes[0] = crate::PreparedTrackMix::new(1.0, 0.0, false).unwrap();
    old.tail_suffix_starts[0] = 0;
    old.tail_output_active[0] = true;
    let mut refused = SessionPlayback::new(prepare(false)).unwrap();
    assert!(matches!(
        refused.preview_stream_handoff_with_report(&mut old, 1, None, &mut |_| Ok(())),
        Err(SessionPlaybackError::GroupTailRoutingBusy)
    ));
    assert!(matches!(
        refused.try_inherit_state_from(&mut old),
        Err(SessionPlaybackError::GroupTailRoutingBusy)
    ));
    assert!(old.tail_groups[0].is_some());
    let mut changed = Editor::new(grouped.clone(), 8).unwrap();
    changed
        .execute(Command::InsertDevice {
            track: group,
            before: None,
            state: hex_devices::utility::UTILITY_DEFINITION
                .default_state()
                .unwrap(),
        })
        .unwrap();
    let mut changed_snapshot = changed.project().snapshot();
    for row in &mut changed_snapshot.tracks {
        row.kind = hex_project::TrackKind::Ordinary;
        row.parent_group = None;
    }
    let changed_flat = Project::from_snapshot(changed_snapshot).unwrap();
    let mut changed_revision =
        PreparedRevision::prepare(&changed_flat, &BTreeMap::new(), Arc::clone(&table), None)
            .unwrap();
    changed_revision.test_install_group_graph(changed.project());
    let mut changed_candidate = SessionPlayback::new(Arc::new(changed_revision)).unwrap();
    assert!(matches!(
        changed_candidate.try_inherit_state_from(&mut old),
        Err(SessionPlaybackError::GroupTailRoutingBusy)
    ));
    assert!(old.tail_groups[0].is_some());
    let mut next = SessionPlayback::new(revision).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let plan = next
        .output_capture_plan(
            &[
                OutputCaptureId::Track(track),
                OutputCaptureId::Return(return_id),
            ],
            4,
        )
        .unwrap();
    let mut main = [[0.0; 2]; 4];
    let mut track_output = main;
    let mut returned = main;
    let mut buffers: [&mut [Frame]; 2] = [&mut track_output, &mut returned];
    let mut capture = OutputCapture::new(&plan, &mut buffers);
    next.render_with_capture(&mut main, &mut capture).unwrap();
    assert_eq!(track_output, [[0.25, -0.5]; 4]);
    assert_eq!(
        returned, [[0.0; 2]; 4],
        "old tails must not enter current sends"
    );
    assert_eq!(main, [[0.125, -0.25]; 4]);
}

#[test]
fn removing_owning_return_transfers_tail_and_refuses_restart() {
    for later_return in [false, true] {
        retained_tail_transfers_with_return_removal(later_return);
    }
}

fn retained_tail_transfers_with_return_removal(later_return: bool) {
    use hex_project::{Command, Created, Editor};

    let mut editor = Editor::new(super::tests::project(1.0, 0, 8 << 32), 8).unwrap();
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: crate::default_native_device_state(crate::DeviceTypeId::from_static(
                "hex.limiter",
            ))
            .unwrap(),
        })
        .unwrap();
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack {
            name: "unused".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    if later_return {
        editor
            .execute(Command::AddReturnTrack {
                name: "later".into(),
            })
            .unwrap();
    }
    let mut previous = playback(editor.project());
    let mut reference = playback(editor.project());
    for runtime in [&mut previous, &mut reference] {
        warm(runtime);
        let bus_ref = &runtime.revision.arrangement().return_buses()[0];
        let tail = ReturnEffectTail::new(active_chain(bus, StereoMatrix::identity()), bus_ref);
        runtime.return_effect_tails.slots_mut()[0] = Some(tail);
    }
    editor.execute(Command::RemoveReturnTrack(bus)).unwrap();
    let mut candidate = playback(editor.project());
    candidate.request_live_restart();
    assert_eq!(
        candidate.try_inherit_state_live_from(&mut previous),
        Err(SessionPlaybackError::LatencyLayoutChanged)
    );
    assert!(
        previous.return_effect_tails.slots()[0]
            .as_ref()
            .unwrap()
            .output_active
    );
    let mut candidate = playback(editor.project());
    candidate
        .try_inherit_state_live_from(&mut previous)
        .unwrap();
    let mut actual = [[0.0; 2]; 8];
    let mut expected = actual;
    candidate.render(&mut actual).unwrap();
    reference.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn real_return_tail_survives_or_refuses_later_main_latency() {
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::{Command, Created, Editor, Project};
    use std::collections::BTreeMap;

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
    let Created::Device(device) = editor
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("device")
    };
    let mut samples = vec![[0.0; 2]; 12_000];
    samples[..512].fill([0.25, -0.125]);
    let media = BTreeMap::from([(asset, crate::Audio::new(48_000, samples).unwrap())]);
    let table = Arc::new(hex_devices::SineTable::new());
    let prepare = |project: &Project| {
        Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
    };
    let initial = prepare(editor.project());
    let mut original = SessionPlayback::new(Arc::clone(&initial)).unwrap();
    let mut reference = SessionPlayback::new(initial).unwrap();
    for playback in [&mut original, &mut reference] {
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback.render(&mut vec![[0.0; 2]; 6000]).unwrap();
    }
    editor.execute(Command::RemoveDevice(device)).unwrap();
    let removed = prepare(editor.project());
    let mut with_tail = SessionPlayback::new(Arc::clone(&removed)).unwrap();
    let mut reference_tail = SessionPlayback::new(removed).unwrap();
    with_tail
        .try_inherit_state_live_from(&mut original)
        .unwrap();
    reference_tail
        .try_inherit_state_live_from(&mut reference)
        .unwrap();
    assert!(
        with_tail
            .return_effect_tails
            .slots()
            .iter()
            .flatten()
            .any(|tail| tail.output_active),
        "actual removed reverb must create an active retained tail"
    );
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: crate::default_native_device_state(crate::DeviceTypeId::from_static(
                "hex.limiter",
            ))
            .unwrap(),
        })
        .unwrap();
    let mut candidate = SessionPlayback::new(prepare(editor.project())).unwrap();
    assert!(matches!(
        candidate.try_inherit_state_live_from(&mut with_tail),
        Err(SessionPlaybackError::LatencyLayoutChanged)
    ));
    candidate.request_live_restart();
    assert!(matches!(
        candidate.try_inherit_state_live_from(&mut with_tail),
        Err(SessionPlaybackError::LatencyLayoutChanged)
    ));
    assert!(
        with_tail
            .return_effect_tails
            .slots()
            .iter()
            .flatten()
            .any(|tail| tail.output_active),
        "refusal must preserve the donor tail"
    );
    let mut actual = [[0.0; 2]; 512];
    let mut expected = actual;
    with_tail.render(&mut actual).unwrap();
    reference_tail.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|sample| *sample != 0.0));

    // A separately authenticated bounce fence represents audio already baked
    // into the successor and deliberately resets the old tail.
    let mut bounced = SessionPlayback::new(prepare(editor.project())).unwrap();
    bounced.request_live_restart();
    bounced
        .try_inherit_state_fenced_restart_from(&mut reference_tail)
        .unwrap();
}

fn real_main_limiter_reverb_fixture() -> (
    hex_project::Editor,
    hex_project::TrackId,
    hex_project::ReturnId,
    hex_project::DeviceId,
    std::collections::BTreeMap<hex_project::AssetId, crate::Audio>,
    Arc<hex_devices::SineTable>,
) {
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::{Command, Created, Editor, Project};
    use std::collections::BTreeMap;

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
    let Created::Device(device) = editor
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("device")
    };
    let mut samples = vec![[0.0; 2]; 12_000];
    samples[..512].fill([0.25, -0.125]);
    let media = BTreeMap::from([(asset, crate::Audio::new(48_000, samples).unwrap())]);
    let table = Arc::new(hex_devices::SineTable::new());
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: crate::default_native_device_state(crate::DeviceTypeId::from_static(
                "hex.limiter",
            ))
            .unwrap(),
        })
        .unwrap();
    (editor, track, bus, device, media, table)
}

#[test]
fn disconnect_send_preserves_detached_real_return_tail() {
    use hex_project::{Command, Created, Project};
    let (mut editor, track, bus, device, media, table) = real_main_limiter_reverb_fixture();
    let Created::ReturnTrack(other_bus) = editor
        .execute(Command::AddReturnTrack {
            name: "other".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track: other_bus,
            gain: 0.25,
        })
        .unwrap();
    let prepare = |project: &Project| {
        Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
    };
    let initial = prepare(editor.project());
    let mut original = SessionPlayback::new(Arc::clone(&initial)).unwrap();
    let mut reference = SessionPlayback::new(initial).unwrap();
    for playback in [&mut original, &mut reference] {
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback.render(&mut vec![[0.0; 2]; 6000]).unwrap();
    }
    editor.execute(Command::RemoveDevice(device)).unwrap();
    let mut detached = SessionPlayback::new(prepare(editor.project())).unwrap();
    assert!(
        !detached
            .try_inherit_state_live_from(&mut original)
            .unwrap()
            .running_audio_restart
    );
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 0.0,
        })
        .unwrap();
    let mut stale = SessionPlayback::new(prepare(editor.project())).unwrap();
    detached.performance_frame += 1;
    assert!(matches!(
        stale.try_inherit_state_live_from(&mut detached),
        Err(SessionPlaybackError::LatencyLayoutChanged)
    ));
    detached.performance_frame -= 1;
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track: other_bus,
            gain: 0.5,
        })
        .unwrap();
    let mut changed_route = SessionPlayback::new(prepare(editor.project())).unwrap();
    assert!(matches!(
        changed_route.try_inherit_state_live_from(&mut detached),
        Err(SessionPlaybackError::LatencyLayoutChanged)
    ));
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track: other_bus,
            gain: 0.25,
        })
        .unwrap();
    let mut forced = SessionPlayback::new(prepare(editor.project())).unwrap();
    forced.request_live_restart();
    assert!(matches!(
        forced.try_inherit_state_live_from(&mut detached),
        Err(SessionPlaybackError::LatencyLayoutChanged)
    ));
    let mut disconnected = SessionPlayback::new(prepare(editor.project())).unwrap();
    assert!(
        !disconnected
            .try_inherit_state_live_from(&mut detached)
            .unwrap()
            .running_audio_restart
    );
    let active = |playback: &SessionPlayback| {
        playback
            .return_effect_tails
            .slots()
            .iter()
            .flatten()
            .filter(|tail| tail.output_active)
            .count()
    };
    assert_eq!(active(&detached), 0);
    assert_eq!(active(&disconnected), 1);
    let id = OutputCaptureId::Return(bus);
    let actual_plan = disconnected.output_capture_plan(&[id], 257).unwrap();
    let expected_plan = reference.output_capture_plan(&[id], 257).unwrap();
    for count in [1, 257, 254] {
        let mut actual = vec![[0.0; 2]; count];
        let mut expected = actual.clone();
        let mut actual_return = actual.clone();
        let mut expected_return = actual.clone();
        let mut actual_buffers = [&mut actual_return[..]];
        let mut expected_buffers = [&mut expected_return[..]];
        let mut actual_capture = OutputCapture::new(&actual_plan, &mut actual_buffers);
        let mut expected_capture = OutputCapture::new(&expected_plan, &mut expected_buffers);
        disconnected
            .render_with_capture(&mut actual, &mut actual_capture)
            .unwrap();
        reference
            .render_with_capture(&mut expected, &mut expected_capture)
            .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(actual_capture.output(0), expected_capture.output(0));
        assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
    }
    // The inverse edit is not the zero-delay removal this handoff proves.
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 1.0,
        })
        .unwrap();
    let mut reconnected = SessionPlayback::new(prepare(editor.project())).unwrap();
    assert!(matches!(
        reconnected.try_inherit_state_live_from(&mut disconnected),
        Err(SessionPlaybackError::LatencyLayoutChanged)
    ));
    assert_eq!(active(&disconnected), 1);
}

#[test]
fn disconnect_send_stops_live_input_without_cutting_the_tail() {
    use hex_project::{Command, Project};
    let (mut editor, track, bus, device, mut media, table) = real_main_limiter_reverb_fixture();
    let silent_media = media.clone();
    let asset = *media.keys().next().unwrap();
    let mut samples = vec![[0.0; 2]; 12_000];
    samples[..512].fill([0.25, -0.125]);
    samples[6000..].fill([0.125, 0.0625]);
    media.insert(asset, crate::Audio::new(48_000, samples).unwrap());
    let prepare = |project: &Project, input| {
        Arc::new(PreparedRevision::prepare(project, input, Arc::clone(&table), None).unwrap())
    };
    let mut original = SessionPlayback::new(prepare(editor.project(), &media)).unwrap();
    let mut reference = SessionPlayback::new(prepare(editor.project(), &silent_media)).unwrap();
    for playback in [&mut original, &mut reference] {
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback.render(&mut vec![[0.0; 2]; 6000]).unwrap();
    }
    editor.execute(Command::RemoveDevice(device)).unwrap();
    let mut detached = SessionPlayback::new(prepare(editor.project(), &media)).unwrap();
    detached.try_inherit_state_live_from(&mut original).unwrap();
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 0.0,
        })
        .unwrap();
    let mut disconnected = SessionPlayback::new(prepare(editor.project(), &media)).unwrap();
    disconnected
        .try_inherit_state_live_from(&mut detached)
        .unwrap();
    let id = OutputCaptureId::Return(bus);
    let actual_plan = disconnected.output_capture_plan(&[id], 257).unwrap();
    let expected_plan = reference.output_capture_plan(&[id], 257).unwrap();
    let mut actual = [[0.0; 2]; 257];
    let mut expected = actual;
    let mut actual_return = actual;
    let mut expected_return = actual;
    let mut actual_buffers = [&mut actual_return[..]];
    let mut expected_buffers = [&mut expected_return[..]];
    let mut actual_capture = OutputCapture::new(&actual_plan, &mut actual_buffers);
    let mut expected_capture = OutputCapture::new(&expected_plan, &mut expected_buffers);
    disconnected
        .render_with_capture(&mut actual, &mut actual_capture)
        .unwrap();
    reference
        .render_with_capture(&mut expected, &mut expected_capture)
        .unwrap();
    assert_eq!(actual_capture.output(0), expected_capture.output(0));
    assert!(
        actual_capture
            .output(0)
            .unwrap()
            .iter()
            .flatten()
            .any(|sample| *sample != 0.0)
    );
    assert_ne!(actual, expected);
}

#[test]
fn main_only_latency_preserves_real_return_reverb_and_captures() {
    use hex_project::{Command, Editor, Project};
    let (mut editor, track, bus, device, media, table) = real_main_limiter_reverb_fixture();
    let prepare = |project: &Project| {
        Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
    };
    let starting_project = editor.project().clone();
    let initial = prepare(editor.project());
    assert!(initial.arrangement().main_latency_frames() > 0);
    let mut original = SessionPlayback::new(Arc::clone(&initial)).unwrap();
    let mut reference = SessionPlayback::new(Arc::clone(&initial)).unwrap();
    let mut unchanged = SessionPlayback::new(Arc::clone(&initial)).unwrap();
    let mut forced = SessionPlayback::new(Arc::clone(&initial)).unwrap();
    let mut quiet = SessionPlayback::new(Arc::clone(&initial)).unwrap();
    for playback in [&mut original, &mut reference, &mut unchanged, &mut forced] {
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback.render(&mut vec![[0.0; 2]; 6000]).unwrap();
    }
    let mut same_candidate = SessionPlayback::new(initial).unwrap();
    same_candidate.request_live_restart();
    same_candidate
        .try_inherit_state_live_from(&mut unchanged)
        .unwrap();

    editor.execute(Command::RemoveDevice(device)).unwrap();
    let removed = prepare(editor.project());
    let mut quiet_candidate = SessionPlayback::new(Arc::clone(&removed)).unwrap();
    quiet_candidate.request_live_restart();
    quiet_candidate
        .try_inherit_state_live_from(&mut quiet)
        .unwrap();
    let mut refused = SessionPlayback::new(Arc::clone(&removed)).unwrap();
    refused.request_live_restart();
    assert!(matches!(
        refused.try_inherit_state_live_from(&mut forced),
        Err(SessionPlaybackError::LatencyLayoutChanged)
    ));

    let mut candidate = SessionPlayback::new(Arc::clone(&removed)).unwrap();
    let report = candidate
        .try_inherit_state_live_from(&mut original)
        .unwrap();
    assert!(!report.running_audio_restart);
    assert_eq!(
        candidate
            .return_effect_tails
            .slots()
            .iter()
            .flatten()
            .filter(|tail| tail.output_active)
            .count(),
        1
    );
    let id = OutputCaptureId::Return(bus);
    let actual_plan = candidate.output_capture_plan(&[id], 257).unwrap();
    let expected_plan = reference.output_capture_plan(&[id], 257).unwrap();
    for count in [1, 257, 254] {
        let mut actual = vec![[0.0; 2]; count];
        let mut expected = actual.clone();
        let mut actual_return = actual.clone();
        let mut expected_return = actual.clone();
        let mut actual_buffers = [&mut actual_return[..]];
        let mut expected_buffers = [&mut expected_return[..]];
        let mut actual_capture = OutputCapture::new(&actual_plan, &mut actual_buffers);
        let mut expected_capture = OutputCapture::new(&expected_plan, &mut expected_buffers);
        candidate
            .render_with_capture(&mut actual, &mut actual_capture)
            .unwrap();
        reference
            .render_with_capture(&mut expected, &mut expected_capture)
            .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(actual_capture.output(0), expected_capture.output(0));
        assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
    }

    // A later equivalent revision must carry the already detached tail.
    let mut later = SessionPlayback::new(Arc::clone(&removed)).unwrap();
    let report = later.try_inherit_state_live_from(&mut candidate).unwrap();
    assert!(!report.running_audio_restart);
    assert_eq!(
        candidate
            .return_effect_tails
            .slots()
            .iter()
            .flatten()
            .filter(|tail| tail.output_active)
            .count(),
        0
    );
    assert_eq!(
        later
            .return_effect_tails
            .slots()
            .iter()
            .flatten()
            .filter(|tail| tail.output_active)
            .count(),
        1
    );
    let mut actual = [[0.0; 2]; 257];
    let mut expected = actual;
    later.render(&mut actual).unwrap();
    reference.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|sample| *sample != 0.0));

    let mut bounced = SessionPlayback::new(removed).unwrap();
    bounced.request_live_restart();
    bounced
        .try_inherit_state_fenced_restart_from(&mut reference)
        .unwrap();

    // Neither an upstream latency nor an outgoing Return feed may enter the
    // direct-to-Main path. In both cases refusal must preserve the old audio.
    let mut routed_snapshot = starting_project.snapshot();
    let target = routed_snapshot.next_id;
    let feed = target + 1;
    routed_snapshot.next_id += 2;
    routed_snapshot.return_tracks.push(ReturnTrackSnapshot {
        id: target,
        name: "downstream".into(),
        gain: 1.0,
        muted: false,
        devices: Vec::new(),
    });
    routed_snapshot
        .return_feeds
        .push(hex_project::ReturnFeedSnapshot {
            id: feed,
            source: bus.value(),
            destination: target,
            gain: 0.5,
        });
    let routed_project = Project::from_snapshot(routed_snapshot).unwrap();
    for (project, upstream_latency) in [(routed_project, false), (starting_project, true)] {
        let mut editor = Editor::new(project, 16).unwrap();
        if upstream_latency {
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
        }
        let initial = prepare(editor.project());
        let mut previous = SessionPlayback::new(Arc::clone(&initial)).unwrap();
        let mut oracle = SessionPlayback::new(initial).unwrap();
        for runtime in [&mut previous, &mut oracle] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut vec![[0.0; 2]; 6000]).unwrap();
        }
        editor.execute(Command::RemoveDevice(device)).unwrap();
        let mut candidate = SessionPlayback::new(prepare(editor.project())).unwrap();
        assert!(matches!(
            candidate.try_inherit_state_live_from(&mut previous),
            Err(SessionPlaybackError::LatencyLayoutChanged)
        ));
        let mut actual = [[0.0; 2]; 257];
        let mut expected = actual;
        previous.render(&mut actual).unwrap();
        oracle.render(&mut expected).unwrap();
        assert_eq!(actual, expected);
        assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
    }
}

#[test]
fn main_only_latency_preserves_real_return_reverb_through_loop_wrap() {
    use hex_project::{Command, Project};
    let (editor, _, _, device, media, table) = real_main_limiter_reverb_fixture();
    let prepare = |project: &Project| {
        Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
    };
    // The same real tail must remain continuous through an actual loop wrap.
    let mut loop_editor = editor;
    loop_editor
        .execute(Command::SetLoopRegion {
            start: 6000,
            end: 6032,
            enabled: true,
        })
        .unwrap();
    let initial = prepare(loop_editor.project());
    let mut previous = SessionPlayback::new(Arc::clone(&initial)).unwrap();
    let mut oracle = SessionPlayback::new(initial).unwrap();
    for runtime in [&mut previous, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut vec![[0.0; 2]; 6000]).unwrap();
    }
    loop_editor.execute(Command::RemoveDevice(device)).unwrap();
    let mut candidate = SessionPlayback::new(prepare(loop_editor.project())).unwrap();
    let report = candidate
        .try_inherit_state_live_from(&mut previous)
        .unwrap();
    assert!(!report.running_audio_restart);
    let mut heard_tail = false;
    for count in [1, 31, 257] {
        let mut actual = vec![[0.0; 2]; count];
        let mut expected = actual.clone();
        candidate.render(&mut actual).unwrap();
        oracle.render(&mut expected).unwrap();
        assert_eq!(actual, expected);
        heard_tail |= actual.iter().flatten().any(|sample| *sample != 0.0);
    }
    assert!(heard_tail);
    assert!((6000..6032).contains(&candidate.arrangement_frame));
}

#[test]
fn main_only_latency_rejects_a_stale_return_tail_clock() {
    use hex_project::{Command, Project};
    let (mut editor, _, _, device, media, table) = real_main_limiter_reverb_fixture();
    let prepare = |project: &Project| {
        Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
    };
    let initial = prepare(editor.project());
    let mut previous = SessionPlayback::new(Arc::clone(&initial)).unwrap();
    let mut oracle = SessionPlayback::new(initial).unwrap();
    for runtime in [&mut previous, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut vec![[0.0; 2]; 6000]).unwrap();
    }
    editor.execute(Command::RemoveDevice(device)).unwrap();
    let mut candidate = SessionPlayback::new(prepare(editor.project())).unwrap();
    previous.performance_frame += 1;
    let stale_frame = previous.performance_frame;
    assert!(matches!(
        candidate.try_inherit_state_live_from(&mut previous),
        Err(SessionPlaybackError::LatencyLayoutChanged)
    ));
    assert_eq!(previous.performance_frame, stale_frame);
    previous.performance_frame -= 1;
    let mut actual = [[0.0; 2]; 257];
    let mut expected = actual;
    previous.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
}

#[test]
fn restart_refuses_incompatible_return_history_with_matching_configuration() {
    let project = prepared_return_project(true, 1.0);
    let return_id = project.return_tracks()[0].id();
    let mut previous = playback(&project);
    previous.return_chains[0] = active_chain(return_id, StereoMatrix::identity());
    let mut candidate = playback(&project);
    let mut instances = processor_instances(return_id, StereoMatrix::identity());
    instances[0].definition = Some(Arc::new(ScaleDefinition(0.5)));
    candidate.return_chains[0] = NativeRuntimeChain::new(instances).unwrap();
    assert!(
        previous.return_chains[0].configuration_matches_range(&candidate.return_chains[0], 0..2)
    );
    assert!(!candidate.return_chains[0].can_inherit_from(&previous.return_chains[0]));
    candidate.request_live_restart();
    assert!(matches!(
        candidate.try_inherit_state_live_from(&mut previous),
        Err(SessionPlaybackError::LatencyLayoutChanged)
    ));
}

#[test]
fn main_only_latency_refuses_a_new_return_feed_without_losing_the_tail() {
    use hex_project::{Command, Created, Project};
    let (mut editor, _, bus, device, media, table) = real_main_limiter_reverb_fixture();
    let Created::ReturnTrack(downstream) = editor
        .execute(Command::AddReturnTrack {
            name: "downstream".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    let prepare = |project: &Project| {
        Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
    };
    let initial = prepare(editor.project());
    let mut previous = SessionPlayback::new(Arc::clone(&initial)).unwrap();
    let mut oracle = SessionPlayback::new(initial).unwrap();
    for runtime in [&mut previous, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut vec![[0.0; 2]; 6000]).unwrap();
    }
    editor
        .execute(Command::SetReturnFeed {
            source: bus,
            destination: downstream,
            gain: 0.5,
        })
        .unwrap();
    editor.execute(Command::RemoveDevice(device)).unwrap();
    let mut candidate = SessionPlayback::new(prepare(editor.project())).unwrap();
    assert!(matches!(
        candidate.try_inherit_state_live_from(&mut previous),
        Err(SessionPlaybackError::LatencyLayoutChanged)
    ));
    let mut actual = [[0.0; 2]; 257];
    let mut expected = actual;
    previous.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
}

#[test]
fn main_only_latency_rejects_missing_return_tail_clock_history() {
    use hex_project::{Command, Project};
    let (mut editor, _, _, device, media, table) = real_main_limiter_reverb_fixture();
    let prepare = |project: &Project| {
        Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
    };
    let initial = prepare(editor.project());
    let mut previous = SessionPlayback::new(Arc::clone(&initial)).unwrap();
    let mut oracle = SessionPlayback::new(initial).unwrap();
    for runtime in [&mut previous, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut vec![[0.0; 2]; 6000]).unwrap();
    }
    editor.execute(Command::RemoveDevice(device)).unwrap();
    let mut candidate = SessionPlayback::new(prepare(editor.project())).unwrap();
    previous.compensation.as_mut().unwrap().delays.reset_epoch();
    oracle.compensation.as_mut().unwrap().delays.reset_epoch();
    let stale_frame = previous.performance_frame;
    let result = candidate.try_inherit_state_live_from(&mut previous);
    assert!(
        matches!(result, Err(SessionPlaybackError::LatencyLayoutChanged)),
        "{result:?}"
    );
    assert_eq!(previous.performance_frame, stale_frame);
    let mut actual = [[0.0; 2]; 257];
    let mut expected = actual;
    previous.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
}

#[test]
fn removing_unused_return_keeps_a_real_return_tail_with_main_latency() {
    use hex_project::{Command, Created, Editor, Project};
    for earlier in [false, true] {
        let (mut editor, _, bus, device, media, table) = real_main_limiter_reverb_fixture();
        let Created::ReturnTrack(unused) = editor
            .execute(Command::AddReturnTrack {
                name: "unused".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        if earlier {
            let mut snapshot = editor.project().snapshot();
            snapshot.return_tracks.swap(0, 1);
            editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap();
        }
        let prepare = |project: &Project| {
            Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
        };
        let initial = prepare(editor.project());
        let mut previous = SessionPlayback::new(Arc::clone(&initial)).unwrap();
        let mut oracle = SessionPlayback::new(initial).unwrap();
        for runtime in [&mut previous, &mut oracle] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut vec![[0.0; 2]; 6000]).unwrap();
        }
        editor.execute(Command::RemoveDevice(device)).unwrap();
        let mut with_tail = SessionPlayback::new(prepare(editor.project())).unwrap();
        let report = with_tail
            .try_inherit_state_live_from(&mut previous)
            .unwrap();
        assert!(!report.running_audio_restart);
        assert_eq!(
            with_tail
                .return_effect_tails
                .slots()
                .iter()
                .flatten()
                .filter(|tail| tail.output_active)
                .count(),
            1
        );
        editor.execute(Command::RemoveReturnTrack(unused)).unwrap();
        let mut candidate = SessionPlayback::new(prepare(editor.project())).unwrap();
        let report = candidate
            .try_inherit_state_live_from(&mut with_tail)
            .unwrap();
        assert!(!report.running_audio_restart);
        assert_eq!(
            with_tail
                .return_effect_tails
                .slots()
                .iter()
                .flatten()
                .filter(|tail| tail.output_active)
                .count(),
            0
        );
        assert_eq!(
            candidate
                .return_effect_tails
                .slots()
                .iter()
                .flatten()
                .filter(|tail| tail.output_active)
                .count(),
            1
        );
        let id = OutputCaptureId::Return(bus);
        let actual_plan = candidate.output_capture_plan(&[id], 257).unwrap();
        let expected_plan = oracle.output_capture_plan(&[id], 257).unwrap();
        for count in [1, 257] {
            let mut actual = vec![[0.0; 2]; count];
            let mut expected = actual.clone();
            let mut actual_return = actual.clone();
            let mut expected_return = actual.clone();
            let mut actual_buffers = [&mut actual_return[..]];
            let mut expected_buffers = [&mut expected_return[..]];
            let mut actual_capture = OutputCapture::new(&actual_plan, &mut actual_buffers);
            let mut expected_capture = OutputCapture::new(&expected_plan, &mut expected_buffers);
            candidate
                .render_with_capture(&mut actual, &mut actual_capture)
                .unwrap();
            oracle
                .render_with_capture(&mut expected, &mut expected_capture)
                .unwrap();
            assert_eq!(actual, expected);
            assert_eq!(actual_capture.output(0), expected_capture.output(0));
            assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
        }
    }
}

#[test]
fn removing_unused_return_refuses_stale_or_missing_tail_clock() {
    use hex_project::{Command, Created, Project};
    let (mut editor, _, _, device, media, table) = real_main_limiter_reverb_fixture();
    let Created::ReturnTrack(unused) = editor
        .execute(Command::AddReturnTrack {
            name: "unused".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    let prepare = |project: &Project| {
        Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
    };
    let initial = prepare(editor.project());
    let mut source = SessionPlayback::new(Arc::clone(&initial)).unwrap();
    let mut oracle = SessionPlayback::new(initial).unwrap();
    for runtime in [&mut source, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut vec![[0.0; 2]; 6000]).unwrap();
    }
    editor.execute(Command::RemoveDevice(device)).unwrap();
    let mut with_tail = SessionPlayback::new(prepare(editor.project())).unwrap();
    with_tail.try_inherit_state_live_from(&mut source).unwrap();
    editor.execute(Command::RemoveReturnTrack(unused)).unwrap();
    let mut candidate = SessionPlayback::new(prepare(editor.project())).unwrap();
    with_tail.performance_frame += 1;
    assert!(matches!(
        candidate.try_inherit_state_live_from(&mut with_tail),
        Err(SessionPlaybackError::LatencyLayoutChanged)
    ));
    with_tail.performance_frame -= 1;
    with_tail
        .compensation
        .as_mut()
        .unwrap()
        .delays
        .reset_epoch();
    oracle.compensation.as_mut().unwrap().delays.reset_epoch();
    assert!(matches!(
        candidate.try_inherit_state_live_from(&mut with_tail),
        Err(SessionPlaybackError::LatencyLayoutChanged)
    ));
    let mut actual = [[0.0; 2]; 257];
    let mut expected = actual;
    with_tail.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
}

#[test]
fn removing_unused_return_refuses_new_return_feed_with_tail() {
    use hex_project::{Command, Created, Project};
    let (mut editor, _, bus, device, media, table) = real_main_limiter_reverb_fixture();
    let Created::ReturnTrack(unused) = editor
        .execute(Command::AddReturnTrack {
            name: "unused".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    let Created::ReturnTrack(downstream) = editor
        .execute(Command::AddReturnTrack {
            name: "downstream".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    let prepare = |project: &Project| {
        Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
    };
    let initial = prepare(editor.project());
    let mut source = SessionPlayback::new(Arc::clone(&initial)).unwrap();
    let mut oracle = SessionPlayback::new(initial).unwrap();
    for runtime in [&mut source, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut vec![[0.0; 2]; 6000]).unwrap();
    }
    editor.execute(Command::RemoveDevice(device)).unwrap();
    let mut with_tail = SessionPlayback::new(prepare(editor.project())).unwrap();
    with_tail.try_inherit_state_live_from(&mut source).unwrap();
    editor
        .execute(Command::SetReturnFeed {
            source: bus,
            destination: downstream,
            gain: 0.5,
        })
        .unwrap();
    editor.execute(Command::RemoveReturnTrack(unused)).unwrap();
    let mut candidate = SessionPlayback::new(prepare(editor.project())).unwrap();
    assert!(matches!(
        candidate.try_inherit_state_live_from(&mut with_tail),
        Err(SessionPlaybackError::LatencyLayoutChanged)
    ));
    let mut actual = [[0.0; 2]; 257];
    let mut expected = actual;
    with_tail.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
}

#[test]
fn removing_the_return_keeps_its_real_detached_tail() {
    use hex_project::{Command, Project};
    let (mut editor, track, bus, device, media, table) = real_main_limiter_reverb_fixture();
    let prepare = |project: &Project| {
        Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
    };
    let initial = prepare(editor.project());
    let mut original = SessionPlayback::new(Arc::clone(&initial)).unwrap();
    let mut oracle = SessionPlayback::new(initial).unwrap();
    for runtime in [&mut original, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut vec![[0.0; 2]; 6000]).unwrap();
    }
    editor.execute(Command::RemoveDevice(device)).unwrap();
    let mut detached = SessionPlayback::new(prepare(editor.project())).unwrap();
    detached.try_inherit_state_live_from(&mut original).unwrap();
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 0.0,
        })
        .unwrap();
    let mut disconnected = SessionPlayback::new(prepare(editor.project())).unwrap();
    disconnected
        .try_inherit_state_live_from(&mut detached)
        .unwrap();
    editor.execute(Command::RemoveReturnTrack(bus)).unwrap();
    let mut stale = SessionPlayback::new(prepare(editor.project())).unwrap();
    disconnected.performance_frame += 1;
    assert!(matches!(
        stale.try_inherit_state_live_from(&mut disconnected),
        Err(SessionPlaybackError::LatencyLayoutChanged)
    ));
    disconnected.performance_frame -= 1;
    let mut forced = SessionPlayback::new(prepare(editor.project())).unwrap();
    forced.request_live_restart();
    assert!(matches!(
        forced.try_inherit_state_live_from(&mut disconnected),
        Err(SessionPlaybackError::LatencyLayoutChanged)
    ));
    let mut candidate = SessionPlayback::new(prepare(editor.project())).unwrap();
    let result = candidate.try_inherit_state_live_from(&mut disconnected);
    assert!(result.is_ok(), "owning return removal refused: {result:?}");
    assert_eq!(
        disconnected
            .return_effect_tails
            .slots()
            .iter()
            .flatten()
            .filter(|tail| tail.output_active)
            .count(),
        0
    );
    assert_eq!(
        candidate
            .return_effect_tails
            .slots()
            .iter()
            .flatten()
            .filter(|tail| tail.output_active)
            .count(),
        1
    );
    let id = OutputCaptureId::Return(bus);
    let actual_plan = candidate.output_capture_plan(&[id], 257).unwrap();
    let expected_plan = oracle.output_capture_plan(&[id], 257).unwrap();
    for count in [1, 257] {
        let mut actual = vec![[0.0; 2]; count];
        let mut expected = actual.clone();
        let mut actual_return = actual.clone();
        let mut expected_return = actual.clone();
        let mut actual_buffers = [&mut actual_return[..]];
        let mut expected_buffers = [&mut expected_return[..]];
        let mut actual_capture = OutputCapture::new(&actual_plan, &mut actual_buffers);
        let mut expected_capture = OutputCapture::new(&expected_plan, &mut expected_buffers);
        candidate
            .render_with_capture(&mut actual, &mut actual_capture)
            .unwrap();
        oracle
            .render_with_capture(&mut expected, &mut expected_capture)
            .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(actual_capture.output(0), expected_capture.output(0));
        assert!(
            actual_capture
                .output(0)
                .unwrap()
                .iter()
                .flatten()
                .any(|sample| *sample != 0.0)
        );
        assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
    }
}
