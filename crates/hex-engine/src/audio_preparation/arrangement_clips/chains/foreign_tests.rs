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
        let mut facts = PreparedForeignProcessor::new(8, 3, 16)?;
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
        calls: std::sync::atomic::AtomicUsize::new(0),
    };
    (editor.project().clone(), bank, facts)
}
struct Borrowed<'a> {
    expected: &'a Facts,
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
            ring: [[0.0; 2]; 19],
            cursor: 0,
            visited: 0,
        };
        let mut output = [[1.0; 2]; 32];
        for frames in output.chunks_mut(8) {
            runtime.begin_block();
            runtime
                .process_range_with_foreign(0..1, frames, 0, None, &mut borrowed)
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
    assert!(custody.cancel(&construction));
    custody.cleanup_on_control();
    let inputs = custody
        .recover_on_control(&construction)
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
