use super::*;
use crate::{
    AudioStorageLedgerReservation, PreparedRevision, SessionPlayback, SessionPlaybackCommand,
    SineTable,
};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn control_sampler_normal_preparation_and_render_borrow_exact_captured_source() {
    normal_preparation_and_render(false);
}

/// Saved sampler states keep their saved version in the project (v2, or v3
/// with a crossfade, Pulse #4088); both prepare and render without migration.
/// A dormant one-shot crossfade forces v3 with identical output.
#[test]
fn control_sampler_prepares_and_renders_a_saved_v3_state() {
    normal_preparation_and_render(true);
}

fn normal_preparation_and_render(saved_v3: bool) {
    let mut editor = Editor::new(Project::new(16000).unwrap(), 8).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            path: "original.wav".into(),
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
    let mut state = hex_devices::sampler::state::SamplerState::for_source_frames(4).unwrap();
    state.mode = hex_devices::sampler::state::AuthoredMode::OneShot {
        gate: hex_devices::sampler::GateMode::Trigger,
    };
    state.envelope = hex_devices::sampler::Envelope {
        attack_seconds: 0.0,
        decay_seconds: 0.0,
        sustain: 1.0,
        release_seconds: 0.0,
    };
    if saved_v3 {
        state.loop_crossfade_ms = 5.0;
    }
    let native = state
        .to_native()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(asset.value()).unwrap(),
        }])
        .unwrap();
    assert_eq!(native.state_version(), if saved_v3 { 3 } else { 2 });
    editor
        .execute(Command::InsertInstrument {
            track,
            state: native,
        })
        .unwrap();
    let mut store = ResidentDeviceAssets::default();
    let cancel = AtomicBool::new(false);
    let limits = AudioStorageLedgerReservation {
        data_rows: 256,
        handle_rows: 32,
        root_rows: 32,
        max_retained_bytes: 20_000_000,
        max_construction_peak_bytes: 20_000_000,
    };
    let source = crate::OwnedAudioSource::try_resident(
        8000,
        vec![[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0, 0.0]],
    )
    .ok()
    .unwrap();
    store
        .insert_on_control((asset, source), 0, limits, 4, &cancel, || {
            Ok::<_, ResourcePreparationError>(())
        })
        .unwrap();
    let scope =
        ControlSamplerSources::prepare_on_control(&store, editor.project(), 20_000_000, 4, &cancel)
            .unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare_control_sampler_export(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            &crate::audio_transform::AudioTransformPreparation::default(),
            &scope,
        )
        .unwrap(),
    );
    let factory = revision
        .session()
        .mutable_instrument_factory(track)
        .unwrap();
    let hex_lattice::InstrumentHistoryRetargetPreparation::Prepared(proof) =
        factory.prepare_history_retarget_target().unwrap()
    else {
        panic!("sampler scalar retarget proof")
    };
    assert_eq!(
        scope.compiler_factory_allowance().unwrap(),
        std::mem::size_of_val(factory) + std::mem::size_of_val(proof.as_ref()),
        "the future generic factory quote covers its actual scalar proof owner"
    );
    drop(proof);
    assert!(SessionPlayback::new(Arc::clone(&revision)).is_err());
    assert!(SessionPlayback::new_linear_arrangement_export(Arc::clone(&revision)).is_err());
    dispatched_owner_accounting(&store, &scope, &revision, limits);
    let foreign =
        ControlSamplerSources::prepare_on_control(&store, editor.project(), 20_000_000, 4, &cancel)
            .unwrap();
    assert!(SessionPlayback::new_control_sampler_export(Arc::clone(&revision), &foreign).is_err());
    drop(foreign);
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let Created::Track(other) = editor
        .execute(Command::AddMidiTrack {
            name: "other".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::MoveTrackDevice {
            device,
            track: other,
            before: None,
        })
        .unwrap();
    assert!(
        PreparedRevision::prepare_control_sampler_export(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            &crate::audio_transform::AudioTransformPreparation::default(),
            &scope,
        )
        .is_err(),
        "same device/state on a different track requires a fresh scope"
    );
    let mut playback =
        SessionPlayback::new_control_sampler_export(Arc::clone(&revision), &scope).unwrap();
    let allowance =
        SessionPlayback::control_sampler_export_runtime_allowance(&revision, &scope).unwrap();
    let actual = playback.accounted_runtime_storage_bytes().unwrap();
    assert_eq!(playback.release_storage_capacity(), 0);
    assert!(actual <= allowance, "actual={actual}, quote={allowance}");
    assert!(allowance < limits.max_retained_bytes);
    assert!(playback.apply(SessionPlaybackCommand::Pause).is_err());
    assert!(
        playback
            .apply(SessionPlaybackCommand::StopTrack {
                track,
                quantization: crate::LaunchQuantization::Immediate,
            })
            .is_err()
    );
    let mut other_export =
        SessionPlayback::new_control_sampler_export(Arc::clone(&revision), &scope).unwrap();
    assert!(other_export.try_inherit_state_from(&mut playback).is_err());
    drop(other_export);
    let weak = Arc::downgrade(scope.data.configurations[0].source.owner.arc());
    drop(store);
    drop(editor);
    drop(state);
    assert!(weak.upgrade().is_some());
    playback.apply(SessionPlaybackCommand::Seek(0)).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 12];
    playback
        .render_control_sampler_export(&mut output, &scope)
        .unwrap();
    assert_eq!(
        output,
        [
            [0.125, -0.25],
            [0.3125, -0.125],
            [0.5, 0.0],
            [0.1875, 0.125],
            [-0.125, 0.25],
            [-0.0625, 0.125],
            [0.0, 0.0],
            [0.0, 0.0],
            [0.0, 0.0],
            [0.0, 0.0],
            [0.0, 0.0],
            [0.0, 0.0]
        ]
    );
    assert!(playback.apply(SessionPlaybackCommand::Seek(1)).is_err());
    drop(scope);
    assert!(
        weak.upgrade().is_none(),
        "public scalar prepared/runtime references must not own source PCM"
    );
    assert!(playback.render(&mut output).is_err());
}

fn dispatched_owner_accounting(
    store: &ResidentDeviceAssets,
    scope: &ControlSamplerSources,
    revision: &PreparedRevision,
    limits: AudioStorageLedgerReservation,
) {
    let anchor = scope.capture_on_control().unwrap();
    let (job, token) = anchor.capture_for_dispatch_on_control(7).unwrap();
    let selections = |extra| {
        let mut selections = vec![
            ControlSamplerSelection {
                sources: Some(scope),
                prepared: Some(revision),
            },
            ControlSamplerSelection {
                sources: Some(&anchor),
                prepared: Some(revision),
            },
        ];
        if extra {
            selections.push(ControlSamplerSelection {
                sources: Some(&job),
                prepared: Some(revision),
            });
        }
        selections
    };
    let inspect = |actual_job, tokens: &[ControlSamplerOutbound<'_>], limits| {
        let selected = selections(actual_job);
        let stores = [store];
        let phase = DeviceResourcePhase {
            registered: &[],
            stores: &stores,
            selections: &selected,
            outbound: tokens,
            legacy: &[],
            checked_runtime_bytes: std::mem::size_of::<DispatchedSourceObligation>(),
            construction_bytes: 0,
        };
        inspect_device_resource_union(&phase, &phase, limits)
    };
    let outbound = ControlSamplerOutbound {
        anchor: &anchor,
        obligation: &token,
        job: 7,
    };
    let before = inspect(true, &[], limits).unwrap();
    let after = inspect(false, &[outbound], limits).unwrap();
    assert_eq!(before.retained.data_bytes, after.retained.data_bytes);
    assert_eq!(
        before.retained.retained_bytes,
        after.retained.retained_bytes
    );
    assert_eq!(
        before.retained.unresolved_handles,
        after.retained.unresolved_handles
    );
    assert_eq!(
        before.construction.construction_peak_bytes,
        after.construction.construction_peak_bytes
    );
    assert_eq!(after.retained_frames, 4);
    let exact = AudioStorageLedgerReservation {
        max_retained_bytes: after.retained.retained_bytes,
        max_construction_peak_bytes: after.construction.construction_peak_bytes,
        ..limits
    };
    inspect(false, &[outbound], exact).unwrap();
    assert!(
        inspect(
            false,
            &[outbound],
            AudioStorageLedgerReservation {
                max_retained_bytes: exact.max_retained_bytes - 1,
                ..exact
            }
        )
        .is_err()
    );
    assert!(
        inspect(
            false,
            &[outbound],
            AudioStorageLedgerReservation {
                max_construction_peak_bytes: exact.max_construction_peak_bytes - 1,
                ..exact
            }
        )
        .is_err()
    );
    assert!(
        inspect(true, &[outbound], limits).is_err(),
        "actual owner plus token is duplicate custody"
    );
    assert!(inspect(false, &[outbound, outbound], limits).is_err());
    assert!(
        inspect(
            false,
            &[ControlSamplerOutbound { job: 8, ..outbound }],
            limits
        )
        .is_err()
    );
    let foreign = DispatchedSourceObligation {
        snapshot: token.snapshot + 1,
        job: token.job,
        handle: token.handle,
    };
    assert!(
        inspect(
            false,
            &[ControlSamplerOutbound {
                obligation: &foreign,
                ..outbound
            }],
            limits
        )
        .is_err()
    );
    drop(job);
    let selected = [
        ControlSamplerSelection {
            sources: Some(scope),
            prepared: Some(revision),
        },
        ControlSamplerSelection {
            sources: Some(&anchor),
            prepared: Some(revision),
        },
    ];
    let outbound = [outbound];
    let phase = DeviceResourcePhase {
        registered: &[],
        stores: &[store],
        selections: &selected,
        outbound: &outbound,
        legacy: &[],
        checked_runtime_bytes: std::mem::size_of::<DispatchedSourceObligation>(),
        construction_bytes: 0,
    };
    let delayed = inspect_device_resource_union(&phase, &phase, limits).unwrap();
    assert_eq!(
        delayed.retained.retained_bytes, after.retained.retained_bytes,
        "a dropped job stays conservatively charged until its terminal is observed"
    );
}
