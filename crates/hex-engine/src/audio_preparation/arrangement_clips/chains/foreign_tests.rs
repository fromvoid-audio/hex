//! Closed compiler metadata and real lattice processing; no native admission.
use super::*;
use crate::signal_graph::tests::{Definition, state};
use hex_lattice::{DevicePreparationContext, ForeignProcessorNode, ForeignProcessors};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, ForeignProcessorBinding, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterValue,
};
use hex_project::{Command, Created, Editor};

struct Facts {
    identity: ProcessorInstanceIdentity,
    binding: Option<ForeignProcessorBinding>,
    decline: bool,
    processor_latency: u32,
    calls: std::sync::atomic::AtomicUsize,
}
impl NativeDeviceMetadata for Facts {
    fn type_id(&self) -> DeviceTypeId {
        Definition.type_id()
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        state(19)
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Definition.migrate_state(state)
    }
    fn read_parameter(
        &self,
        _: &NativeDeviceState,
        _: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        Err(DeviceContractError::UnknownParameter)
    }
    fn patch_parameter(
        &self,
        _: &NativeDeviceState,
        _: ParameterId,
        _: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Err(DeviceContractError::UnknownParameter)
    }
}
impl NativeDeviceDefinition for Facts {
    fn prepare_matrix(&self, _: &NativeDeviceState) -> Result<StereoMatrix, DeviceContractError> {
        Ok(StereoMatrix::identity())
    }
    fn prepare_instance_runtime_factory(
        &self,
        identity: ProcessorInstanceIdentity,
        value: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if identity != self.identity
            || context.sample_rate != 48_000
            || value.payload() != 19_u32.to_le_bytes()
        {
            return Err(DeviceContractError::InvalidId);
        }
        if self.decline {
            return Ok(None);
        }
        let mut facts = PreparedForeignProcessor::new(8, self.processor_latency, 16)?;
        if let Some(binding) = self.binding {
            facts = facts.with_binding(binding)?;
        }
        Ok(Some(PreparedProcessorFactory::Foreign(facts)))
    }
}
fn setup(location: usize) -> (hex_project::Project, crate::OwnedAudioInputs, Facts) {
    let (project, mut bank) = crate::audio_preparation::tests::fixture(2);
    bank.reservation.handle_obligations = 256;
    bank.reservation.allocation_items = 1024;
    let mut editor = Editor::new(project, 16).unwrap();
    let track = editor.project().tracks()[0].id();
    let (command, owner) = match location {
        0 => (
            Command::InsertDevice {
                track,
                before: None,
                state: state(19).unwrap(),
            },
            ProcessorChainOwner::Track(track.value()),
        ),
        1 => {
            let Created::ReturnTrack(id) = editor
                .execute(Command::AddReturnTrack { name: "bus".into() })
                .unwrap()
            else {
                panic!("return")
            };
            editor
                .execute(Command::SetReturnSend {
                    track,
                    return_track: id,
                    gain: 1.0,
                })
                .unwrap();
            (
                Command::InsertReturnDevice {
                    return_track: id,
                    before: None,
                    state: state(19).unwrap(),
                },
                ProcessorChainOwner::Return(id.value()),
            )
        }
        _ => (
            Command::InsertMainDevice {
                before: None,
                state: state(19).unwrap(),
            },
            ProcessorChainOwner::Main,
        ),
    };
    let Created::Device(id) = editor.execute(command).unwrap() else {
        panic!("device")
    };
    let facts = Facts {
        identity: ProcessorInstanceIdentity::new(owner, id.value()).unwrap(),
        binding: Some(ForeignProcessorBinding {
            root: 7,
            instance: 91,
            graph: 12,
            epoch: 1,
        }),
        decline: false,
        processor_latency: 3,
        calls: std::sync::atomic::AtomicUsize::new(0),
    };
    (editor.project().clone(), bank, facts)
}
struct Borrowed<'a> {
    expected: &'a Facts,
    configuration_address: Option<usize>,
    ring: [[f32; 2]; 19],
    cursor: usize,
    visited: usize,
}
impl ForeignProcessors for Borrowed<'_> {
    fn process_segment(
        &mut self,
        node: ForeignProcessorNode<'_>,
        _: usize,
        frames: &mut [[f32; 2]],
        _: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        assert_eq!(node.owner, self.expected.identity.owner());
        assert_eq!(node.device_id, self.expected.identity.device_id());
        assert_eq!(node.prepared.binding(), self.expected.binding);
        assert_eq!(node.prepared.total_latency_frames(), 19);
        assert_eq!(node.configuration.payload(), 19_u32.to_le_bytes());
        if let Some(address) = self.configuration_address {
            assert_eq!(
                node.configuration as *const NativeDeviceState as usize, address,
                "control query must borrow the same original configuration used for rendering"
            );
        }
        self.visited += frames.len();
        for frame in frames {
            let gain = if node.bypassed {
                1.0
            } else {
                node.input_gain * 0.25
            };
            let mut wet = [frame[0] * gain, frame[1] * gain];
            std::mem::swap(&mut wet, &mut self.ring[self.cursor]);
            self.cursor = (self.cursor + 1) % self.ring.len();
            *frame = wet;
        }
        Ok(())
    }
    fn reject_callback(&mut self) {
        panic!("valid borrowed fixture must not reject")
    }
}
#[test]
fn closed_foreign_chains_copy_state_and_process_at_each_original_owner() {
    for location in 0..3 {
        let (project, _, facts) = setup(location);
        let defs: [&dyn NativeDeviceDefinition; 1] = [&facts];
        let registry = NativeDeviceRegistry::new(&defs).unwrap();
        let devices = match location {
            0 => project.tracks()[0].devices(),
            1 => project.return_tracks()[0].devices(),
            _ => project.main_devices(),
        };
        let mut custody = ChainCustody::default();
        let chain = custody
            .prepare_with_effects(devices, facts.identity.owner(), 48_000, Some(&registry))
            .unwrap();
        let config = chain[0].configuration.as_ref().unwrap();
        assert!(!config.shares_payload_storage_with(devices[0].state()));
        assert_eq!(config.as_ref(), devices[0].state());
        assert!(custody.bytes(&chain).unwrap() > config.payload().len());
        // The existing runtime receives the closed constructor's actual
        // configuration/meter handles and scalar facts; no provider owns DSP.
        let mut runtime = hex_lattice::NativeRuntimeChain::new_prepared(chain.to_vec()).unwrap();
        let mut borrowed = Borrowed {
            expected: &facts,
            configuration_address: None,
            ring: [[0.0; 2]; 19],
            cursor: 0,
            visited: 0,
        };
        let mut output = [[1.0; 2]; 32];
        for frames in output.chunks_mut(8) {
            runtime.begin_block();
            runtime
                .process_range_with_foreign(0..1, frames, 0, None, None, &mut borrowed)
                .unwrap();
        }
        assert_eq!(&output[..19], &[[0.0; 2]; 19]);
        assert_eq!(&output[19..], &[[0.25; 2]; 13]);
        assert_eq!(borrowed.visited, 32);
        drop(runtime);
        assert!(custody.bytes(&chain).is_ok());
    }
}
#[test]
fn closed_foreign_revision_ledger_and_cancellation_preserve_original_bank() {
    if !crate::audio_storage_constructor_layout_supported() {
        return;
    }
    for location in 0..3 {
        let (project, bank, facts) = setup(location);
        let original = bank.sources.as_ptr();
        let defs: [&dyn NativeDeviceDefinition; 1] = [&facts];
        let registry = NativeDeviceRegistry::new(&defs).unwrap();
        let mut bank = Some(bank);
        let mut custody = crate::AudioPreparationCustody::try_new_on_control().unwrap();
        let context = crate::AudioSourceContext {
            project_generation: 1,
            editor_revision: 1,
        };
        let refused = custody
            .begin_prepared_revision_in_place_on_control(&project, 11, context, &mut bank)
            .err()
            .expect("default closed class must still refuse foreign effects");
        assert_eq!(refused.reason, AudioStorageError::UnsupportedGraph);
        assert_eq!(bank.as_ref().unwrap().sources.as_ptr(), original);
        let token = custody
            .begin_effect_revision_in_place_on_control(
                &project,
                11,
                crate::AudioSourceContext {
                    project_generation: 1,
                    editor_revision: 1,
                },
                &mut bank,
                None,
                &registry,
            )
            .unwrap();
        assert!(bank.is_none());
        let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(
            crate::AudioStorageLedgerReservation {
                data_rows: 2048,
                handle_rows: 512,
                root_rows: 8,
                max_retained_bytes: usize::MAX,
                max_construction_peak_bytes: usize::MAX,
            },
        )
        .unwrap();
        ledger
            .borrow_custody_on_control(&custody, crate::AudioStorageRole::Current)
            .unwrap();
        assert_eq!(ledger.summary().unwrap().unknown_storage, 0);
        let revision = custody.candidate_root_view(&token).unwrap();
        let actual = custody
            .arrangement_clips
            .root
            .capacity_at(false, Some(revision.arrangement()))
            .unwrap();
        assert_eq!(
            custody.capacity.arrangement_clips.root.main_prepared_bytes, actual.main_prepared_bytes,
            "the reserved Main chain must match its actual constructed nested state/meter backing"
        );
        drop(ledger);
        assert!(custody.cancel(&token));
        custody.cleanup_on_control();
        let bank = custody
            .recover_on_control(&token)
            .expect("original source recovery");
        assert_eq!(bank.sources.as_ptr(), original);
    }
}
#[test]
fn closed_foreign_constructor_refuses_unbound_absent_and_wrong_instance_facts() {
    for variant in 0..4 {
        let (project, _, mut facts) = setup(0);
        match variant {
            0 => facts.binding = None,
            1 => facts.decline = true,
            2 => {
                facts.identity = ProcessorInstanceIdentity::new(
                    ProcessorChainOwner::Main,
                    facts.identity.device_id(),
                )
                .unwrap()
            }
            _ => {}
        }
        let defs: [&dyn NativeDeviceDefinition; 1] = [&facts];
        let registry = NativeDeviceRegistry::new(&defs).unwrap();
        let mut custody = ChainCustody::default();
        assert!(matches!(
            custody.prepare_with_effects(
                project.tracks()[0].devices(),
                ProcessorChainOwner::Track(project.tracks()[0].id().value()),
                48_000,
                (variant != 3).then_some(&registry)
            ),
            Err(AudioStorageError::UnsupportedGraph)
        ));
    }
}

#[test]
fn closed_foreign_main_coexists_with_original_sampler_sources() {
    if !crate::audio_storage_constructor_layout_supported() {
        return;
    }
    use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
    use hex_project::BeatTime;
    let (project, old_bank, facts) = setup(2);
    let limits = old_bank.reservation;
    drop(old_bank);
    let audio_asset = project.assets().next().unwrap().id();
    let mut editor = Editor::new(project, 16).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            path: "original-sampler.wav".into(),
            sample_rate: 8000,
            frames: 4,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let mut sampler = hex_devices::sampler::state::SamplerState::for_source_frames(4).unwrap();
    sampler.mode = hex_devices::sampler::state::AuthoredMode::OneShot {
        gate: hex_devices::sampler::GateMode::Trigger,
    };
    let state = sampler
        .to_native()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(asset.value()).unwrap(),
        }])
        .unwrap();
    editor
        .execute(Command::InsertInstrument { track, state })
        .unwrap();
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
    let originals = vec![
        (
            audio_asset,
            crate::OwnedAudioSource::try_resident(48000, vec![[0.25; 2]; 8])
                .ok()
                .unwrap(),
        ),
        (
            asset,
            crate::OwnedAudioSource::try_resident(
                8000,
                vec![[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0, 0.0]],
            )
            .ok()
            .unwrap(),
        ),
    ];
    let inputs = crate::OwnedAudioInputs::try_new(originals, limits)
        .ok()
        .unwrap();
    let token = inputs.source_token(asset).unwrap();
    let original_frames = inputs
        .source_view(&token)
        .unwrap()
        .resident_frames()
        .unwrap()
        .as_ptr();
    let original_bank = inputs.sources.as_ptr();
    let mut inputs = Some(inputs);
    let defs: [&dyn NativeDeviceDefinition; 1] = [&facts];
    let registry = NativeDeviceRegistry::new(&defs).unwrap();
    let mut custody = crate::AudioPreparationCustody::try_new_on_control().unwrap();
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    // Independent existing Current sampler renderer, with the same original
    // rate/PCM and authored state but without the Main effect.
    let mut snapshot = editor.project().snapshot();
    snapshot.main_devices.clear();
    let reference_project = hex_project::Project::from_snapshot(snapshot).unwrap();
    let mut reference_sources = Vec::new();
    for id in [audio_asset, asset] {
        let bank = inputs.as_ref().unwrap();
        let view = bank.source_view(&bank.source_token(id).unwrap()).unwrap();
        reference_sources.push((
            id,
            crate::OwnedAudioSource::try_resident(
                view.sample_rate(),
                view.resident_frames().unwrap().to_vec(),
            )
            .ok()
            .unwrap(),
        ));
    }
    let mut reference_inputs = Some(
        crate::OwnedAudioInputs::try_new(reference_sources, limits)
            .ok()
            .unwrap(),
    );
    let mut reference_custody = crate::AudioPreparationCustody::try_new_on_control().unwrap();
    let reference_token = reference_custody
        .begin_current_sampler_in_place_on_control(
            &reference_project,
            11,
            crate::AudioSourceContext {
                project_generation: 1,
                editor_revision: 1,
            },
            &mut reference_inputs,
            4,
            &cancelled,
        )
        .unwrap();
    let mut reference =
        crate::AudioCandidateOwner::try_new_on_control(reference_custody, reference_token)
            .ok()
            .unwrap();
    let construction = custody
        .begin_effect_revision_in_place_on_control(
            editor.project(),
            11,
            crate::AudioSourceContext {
                project_generation: 1,
                editor_revision: 1,
            },
            &mut inputs,
            Some((4, &cancelled)),
            &registry,
        )
        .unwrap();
    assert!(inputs.is_none());
    let mut ledger =
        crate::AudioRootStorageLedger::try_new_on_control(crate::AudioStorageLedgerReservation {
            data_rows: 4096,
            handle_rows: 1024,
            root_rows: 8,
            max_retained_bytes: usize::MAX,
            max_construction_peak_bytes: usize::MAX,
        })
        .unwrap();
    ledger
        .borrow_custody_on_control(&custody, crate::AudioStorageRole::Current)
        .unwrap();
    assert_eq!(ledger.summary().unwrap().unknown_storage, 0);
    drop(ledger);
    let mut owner = crate::AudioCandidateOwner::try_new_on_control(custody, construction)
        .ok()
        .expect("closed original sampler plus compensated effect runtime");
    owner
        .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
            data_rows: 8192,
            handle_rows: 2048,
            root_rows: 8,
            max_retained_bytes: usize::MAX,
            max_construction_peak_bytes: usize::MAX,
        })
        .unwrap();
    for runtime in [&mut owner, &mut reference] {
        runtime
            .test_apply_action(crate::PlaybackAction::Transport(
                crate::PlaybackCommand::Play,
            ))
            .unwrap();
    }
    let mut actual = [[0.0; 2]; 64];
    let mut expected_source = [[0.0; 2]; 64];
    let mut foreign = Borrowed {
        expected: &facts,
        configuration_address: None,
        ring: [[0.0; 2]; 19],
        cursor: 0,
        visited: 0,
    };
    for (output, expected) in actual.chunks_mut(8).zip(expected_source.chunks_mut(8)) {
        owner
            .test_render_with_foreign(output, &mut foreign)
            .unwrap();
        reference.test_render(expected).unwrap();
    }
    assert!(
        expected_source[..16].iter().any(|f| *f != [0.25; 2]),
        "the reference includes actual MIDI sampler output beside audio"
    );
    let mut expected = [[0.0; 2]; 64];
    for (to, from) in expected[19..].iter_mut().zip(&expected_source) {
        *to = [from[0] * 0.25, from[1] * 0.25];
    }
    assert_eq!(
        actual, expected,
        "the same original sampler/PCM must reach the native effect under compensation"
    );
    owner.dispose_on_control().unwrap();
    reference.dispose_on_control().unwrap();
    assert!(reference.recover_inputs_on_control().is_some());
    let inputs = owner
        .recover_inputs_on_control()
        .expect("actual mixed source bank recovery");
    assert_eq!(inputs.sources.as_ptr(), original_bank);
    let source = inputs.source_view(&token).unwrap();
    assert_eq!(source.sample_rate(), 8000);
    assert_eq!(source.resident_frames().unwrap().as_ptr(), original_frames);
    assert_eq!(source.resident_frames().unwrap().len(), 4);
}

#[test]
fn main_effect_data_and_peak_refuse_before_factory_and_source_transfer() {
    if !crate::audio_storage_constructor_layout_supported() {
        return;
    }
    let (project, bank, facts) = setup(2);
    let original = bank.sources.as_ptr();
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&facts];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let context = crate::AudioSourceContext {
        project_generation: 1,
        editor_revision: 1,
    };
    let mut custody = crate::AudioPreparationCustody::try_new_on_control().unwrap();
    let mut blank = Editor::new(project.clone(), 4).unwrap();
    blank
        .execute(Command::RemoveDevice(project.main_devices()[0].id()))
        .unwrap();
    let mut baseline = crate::AudioPreparationCustody::try_new_on_control().unwrap();
    let mut inputs = Some(bank);
    let baseline_token = baseline
        .begin_effect_revision_in_place_on_control(
            blank.project(),
            11,
            context,
            &mut inputs,
            None,
            &registry,
        )
        .unwrap();
    let baseline_capacity = baseline.capacity;
    assert!(baseline.cancel(&baseline_token));
    baseline.cleanup_on_control();
    let bank = baseline.recover_on_control(&baseline_token).unwrap();
    let mut inputs = Some(bank);
    let token = custody
        .begin_effect_revision_in_place_on_control(
            &project,
            11,
            context,
            &mut inputs,
            None,
            &registry,
        )
        .unwrap();
    let capacity = custody.capacity;
    assert!(
        capacity.retained_stage_bytes - baseline_capacity.retained_stage_bytes
            >= capacity.arrangement_clips.root.main_prepared_bytes
                - baseline_capacity.arrangement_clips.root.main_prepared_bytes,
        "aggregate reservation must include the actual additional Main state/meter/chain backing"
    );
    assert!(
        capacity
            .arrangement_clips
            .root
            .main_construction_overlap_bytes
            > 0
    );
    assert!(
        capacity.construction_peak_bytes
            >= capacity.retained_stage_bytes
                + capacity
                    .arrangement_clips
                    .root
                    .main_construction_overlap_bytes
    );
    assert_eq!(facts.calls.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert!(custody.cancel(&token));
    custody.cleanup_on_control();
    let mut bank = custody.recover_on_control(&token).unwrap();
    for retained in [true, false] {
        let mut limits = bank.reservation;
        if retained {
            limits.max_prepared_data_bytes = capacity.retained_stage_bytes - 1;
        } else {
            limits.max_construction_data_bytes = capacity.construction_peak_bytes - 1;
        }
        let original_limits = bank.reservation;
        bank.reservation = limits;
        let mut inputs = Some(bank);
        facts.calls.store(0, std::sync::atomic::Ordering::Relaxed);
        let mut refused = crate::AudioPreparationCustody::try_new_on_control().unwrap();
        let error = refused
            .begin_effect_revision_in_place_on_control(
                &project,
                11,
                context,
                &mut inputs,
                None,
                &registry,
            )
            .err()
            .expect("Main reservation refusal");
        assert_eq!(error.reason, AudioStorageError::ReservationExceeded);
        assert_eq!(
            facts.calls.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "Main constructor must not run before its reservation"
        );
        bank = inputs.unwrap();
        assert_eq!(bank.sources.as_ptr(), original);
        bank.reservation = original_limits;
    }
}

#[test]
fn closed_compensated_foreign_runtime_has_literal_pcm_and_complete_constructor_data() {
    if !crate::audio_storage_constructor_layout_supported() {
        return;
    }
    for (location, bypassed) in
        (0..3).flat_map(|location| [false, true].map(move |bypassed| (location, bypassed)))
    {
        let (project, bank, facts) = setup(location);
        let device = match location {
            0 => project.tracks()[0].devices()[0].id(),
            1 => project.return_tracks()[0].devices()[0].id(),
            _ => project.main_devices()[0].id(),
        };
        let mut editor = Editor::new(project, 16).unwrap();
        editor
            .execute(Command::SetDeviceBypass { device, bypassed })
            .unwrap();
        let project = editor.project().clone();
        let original = bank.sources.as_ptr();
        let defs: [&dyn NativeDeviceDefinition; 1] = [&facts];
        let registry = NativeDeviceRegistry::new(&defs).unwrap();
        let mut inputs = Some(bank);
        let mut custody = crate::AudioPreparationCustody::try_new_on_control().unwrap();
        let token = custody
            .begin_effect_revision_in_place_on_control(
                &project,
                11,
                crate::AudioSourceContext {
                    project_generation: 1,
                    editor_revision: 1,
                },
                &mut inputs,
                None,
                &registry,
            )
            .unwrap();
        assert_eq!(
            custody.phase(),
            crate::AudioPreparationPhase::ArrangementAndSessionSources
        );
        assert_eq!(
            custody
                .candidate_root_view(&token)
                .unwrap()
                .arrangement()
                .main_output_latency_frames,
            19
        );
        let mut owner = crate::AudioCandidateOwner::try_new_on_control(custody, token)
            .ok()
            .expect("actual closed compensated runtime");
        let summary = owner
            .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
                data_rows: 4096,
                handle_rows: 2048,
                root_rows: 8,
                max_retained_bytes: usize::MAX,
                max_construction_peak_bytes: usize::MAX,
            })
            .unwrap();
        assert_eq!(summary.data.unknown_storage, 0);
        assert!(
            summary.data.require_complete().is_err(),
            "closed DATA is not native publication/disposal authority"
        );
        assert!(
            owner.requires_effect_graph_custody_on_control().unwrap(),
            "constructor success is not native source admission"
        );
        let configuration_address = {
            let mut nodes = owner.foreign_nodes_on_control().unwrap();
            let node = nodes.next().expect("actual original foreign node");
            assert!(nodes.next().is_none());
            assert_eq!(node.owner, facts.identity.owner());
            assert_eq!(node.device_id, facts.identity.device_id());
            assert_eq!(node.device_type, facts.type_id());
            assert_eq!(node.sample_rate, 48_000);
            assert_eq!(node.bypassed, bypassed);
            assert_eq!(
                node.configuration.as_deref().unwrap().payload(),
                19_u32.to_le_bytes()
            );
            let Some(PreparedProcessorFactory::Foreign(prepared)) = &node.definition else {
                panic!("foreign node query returned another factory");
            };
            assert_eq!(prepared.binding(), facts.binding);
            assert_eq!(prepared.total_latency_frames(), 19);
            assert_ne!(
                prepared.binding().unwrap().instance,
                node.device_id,
                "binding and processor identity are independent namespaces"
            );
            node.configuration.as_deref().unwrap() as *const NativeDeviceState as usize
        };
        owner
            .test_apply_action(crate::PlaybackAction::Transport(
                crate::PlaybackCommand::Play,
            ))
            .unwrap();
        let mut foreign = Borrowed {
            expected: &facts,
            configuration_address: Some(configuration_address),
            ring: [[0.0; 2]; 19],
            cursor: 0,
            visited: 0,
        };
        let mut output = [[99.0; 2]; 48];
        for block in output.chunks_mut(8) {
            owner.test_render_with_foreign(block, &mut foreign).unwrap();
        }
        let gain = match (location, bypassed) {
            (1, true) => 0.5,
            (1, false) => 0.3125,
            (_, true) => 0.25,
            (_, false) => 0.0625,
        };
        let mut expected = [[0.0; 2]; 48];
        expected[19..35].fill([gain; 2]);
        assert_eq!(
            output, expected,
            "closed renderer must align the original dry and processed paths"
        );
        assert_eq!(foreign.visited, 48);
        owner.dispose_on_control().unwrap();
        let inputs = owner
            .recover_inputs_on_control()
            .expect("original bank after actual runtime disposal");
        assert_eq!(inputs.sources.as_ptr(), original);
    }
}

// Real closed roots for the lower owning-frontier test. These are engine-only
// scalar foreign facts, not admitted native sources or a DSP acceptance oracle.
impl crate::AudioCandidateOwner {
    pub(crate) fn test_compensation_origin_owner(latency: Option<u32>) -> Self {
        let (mut project, bank, mut facts) = setup(2);
        if let Some(latency) = latency {
            facts.processor_latency = latency;
        } else {
            let device = project.main_devices()[0].id();
            let mut editor = Editor::new(project, 8).unwrap();
            editor.execute(Command::RemoveDevice(device)).unwrap();
            project = editor.project().clone();
        }
        let definitions: [&dyn NativeDeviceDefinition; 1] = [&facts];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        let mut custody = crate::AudioPreparationCustody::try_new_on_control().unwrap();
        let token = custody
            .begin_input_on_control(
                crate::AudioArrangementInput::new(&project, 11, bank)
                    .with_audio_prepared_revision()
                    .with_effect_registry(&registry),
            )
            .ok()
            .unwrap();
        let mut owner = Self::try_new_on_control(custody, token).ok().unwrap();
        owner
            .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
                data_rows: 4096,
                handle_rows: 2048,
                root_rows: 8,
                max_retained_bytes: usize::MAX,
                max_construction_peak_bytes: usize::MAX,
            })
            .unwrap();
        owner
    }
}

#[test]
fn grouped_faded_pcm_preserves_closed_foreign_track_return_and_main_roots() {
    if !crate::audio_storage_constructor_layout_supported() {
        return;
    }
    for location in 0..3 {
        let (project, bank, facts) = setup(location);
        let original_bank = bank.sources.as_ptr();
        let mut editor = Editor::new(project, 16).unwrap();
        let clips = editor.project().tracks()[0]
            .clips()
            .iter()
            .map(|c| c.id())
            .collect();
        let Created::Clip(group) = editor.execute(Command::GroupClips(clips)).unwrap() else {
            panic!("group")
        };
        editor
            .execute(Command::EditAudioEvents {
                clip: group,
                operation: hex_project::AudioEventEdit::SetFade {
                    event: 1,
                    fade: Some(
                        hex_project::AudioEventFade::new(
                            0,
                            hex_project::ClipFade::new(8, 0, 3, 3).unwrap(),
                        )
                        .unwrap(),
                    ),
                },
            })
            .unwrap();
        let defs: [&dyn NativeDeviceDefinition; 1] = [&facts];
        let registry = NativeDeviceRegistry::new(&defs).unwrap();
        let quote = crate::AudioCandidateReservationQuote::for_project_with_effect_registry(
            editor.project(),
            1,
            &registry,
        )
        .unwrap();
        assert_eq!(quote.prepared_pcm_frames, 16);
        let mut inputs = Some(bank);
        let mut custody = crate::AudioPreparationCustody::try_new_on_control().unwrap();
        let token = custody
            .begin_effect_revision_in_place_on_control(
                editor.project(),
                11,
                crate::AudioSourceContext {
                    project_generation: 1,
                    editor_revision: 1,
                },
                &mut inputs,
                None,
                &registry,
            )
            .unwrap();
        assert!(inputs.is_none());
        let mut owner = crate::AudioCandidateOwner::try_new_on_control(custody, token)
            .ok()
            .unwrap();
        owner
            .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
                data_rows: 8192,
                handle_rows: 2048,
                root_rows: 8,
                max_retained_bytes: usize::MAX,
                max_construction_peak_bytes: usize::MAX,
            })
            .unwrap();
        owner
            .test_apply_action(crate::PlaybackAction::Transport(
                crate::PlaybackCommand::Play,
            ))
            .unwrap();
        let mut actual = [[0.0; 2]; 64];
        let mut foreign = Borrowed {
            expected: &facts,
            configuration_address: None,
            ring: [[0.0; 2]; 19],
            cursor: 0,
            visited: 0,
        };
        for block in actual.chunks_mut(8) {
            owner.test_render_with_foreign(block, &mut foreign).unwrap();
        }
        let mut expected = [[0.0; 2]; 64];
        for i in 0..16 {
            let fade = if i < 8 {
                (i as f32 / 2.0).min(1.0) * ((7 - i) as f32 / 2.0).min(1.0)
            } else {
                1.0
            };
            let route = if location == 1 { 1.25 } else { 0.25 };
            expected[19 + i] = [0.25 * fade * route; 2];
        }
        assert_eq!(
            actual, expected,
            "grouped processed source remains upstream of original effect owner {location}"
        );
        owner.dispose_on_control().unwrap();
        assert_eq!(
            owner.recover_inputs_on_control().unwrap().sources.as_ptr(),
            original_bank
        );
    }
}
