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
    fn process_block(&mut self, frames: &mut [[f32; 2]]) {
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
    fn process_block(&mut self, frames: &mut [[f32; 2]]) {
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
    chain.process_block(&mut [[0.1, -0.1]]);
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
    chain.process_block(&mut [[0.1, -0.1]]);
    chain.commit_block();
    chain
}

fn warm(playback: &mut SessionPlayback) {
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 256];
    playback.render(&mut output).unwrap();
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
            id: 30,
            path: "tail.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 32,
            playback_frames: 32,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        }];
        snapshot.tracks[0].content = hex_project::TrackContentSnapshot::Hybrid {
            audio_clips: vec![hex_project::ClipSnapshot {
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
        let plan = candidate
            .output_capture_plan(
                &[
                    OutputCaptureId::Track(track_id),
                    OutputCaptureId::Return(return_id),
                ],
                512,
            )
            .unwrap();
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
            offset += frames;
        }
        for index in 0..8 {
            let current = if replacement { [0.1, -0.025] } else { [0.0; 2] };
            let current_return = if replacement {
                [(0.1 - 0.025) * 0.5 * 0.25; 2]
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
    bank.render_chunk(&mut failed_attempt, 0, &mut None)
        .unwrap();
    assert_eq!(failed_attempt[0], [0.375, -0.75]);
    bank.rollback_block();
    assert!(bank.slots()[0].as_ref().unwrap().output_active);

    bank.begin_block();
    let mut retry = [[0.0; 2]; 1];
    bank.render_chunk(&mut retry, 0, &mut None).unwrap();
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
    let bytes_cap =
        2 * hex_lattice::MAX_STATEFUL_PROCESSORS * hex_lattice::MAX_PROCESSOR_RUNTIME_BYTES;
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
