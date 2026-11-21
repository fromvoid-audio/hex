use super::*;
use crate::native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets};
use crate::playback::HostedInstrumentMove;
use hex_project::DeviceSelectionIds;
use std::sync::atomic::AtomicBool;

struct ResidentRevision {
    prepared: Arc<PreparedRevision>,
    resources: PreparedDeviceResources,
}

fn prepare_move(duration: BeatTime) -> ([ResidentRevision; 2], HostedInstrumentMove) {
    let (mut editor, original) = fixture_with_duration(duration);
    let donor = editor.project().tracks()[0].id();
    let device = editor
        .project()
        .track(donor)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let Created::Track(receiver) = editor
        .execute(Command::AddMidiTrack {
            name: "receiver".into(),
        })
        .unwrap()
    else {
        panic!("MIDI receiver");
    };
    let target = ProcessorInstanceIdentity::new(
        ProcessorChainOwner::Track(receiver.value()),
        device.value(),
    )
    .unwrap();
    let definition = InstrumentDefinition {
        identity: original,
        second: None,
        retained_receiver: Some(target),
        instrument_profile: true,
        epoch: 1,
    };
    let definitions: &[&dyn NativeDeviceDefinition] = &[&definition];
    let registry = NativeDeviceRegistry::new(definitions).unwrap();
    let prepare = |project: &hex_project::Project| {
        let cancel = AtomicBool::new(false);
        let resources = PreparedDeviceResources::prepare_on_control(
            &ResidentDeviceAssets::default(),
            project,
            1_000_000,
            &cancel,
        )
        .unwrap();
        let prepared = PreparedRevision::prepare_with_device_resources_and_effect_registry(
            project,
            &Default::default(),
            Arc::new(SineTable::new()),
            None,
            &Default::default(),
            &resources,
            resources.compiler_scan_work_on_control(project).unwrap(),
            &cancel,
            &registry,
            None,
        )
        .unwrap();
        ResidentRevision {
            prepared: Arc::new(prepared),
            resources,
        }
    };
    let old = prepare(editor.project());
    editor
        .execute(Command::MoveTrackDevices {
            source: donor,
            destination: receiver,
            selected: DeviceSelectionIds::new(&[device]).unwrap(),
        })
        .unwrap();
    let new = prepare(editor.project());
    let [old_donor, old_receiver] = old.prepared.arrangement().tracks() else {
        panic!("two tracks");
    };
    let [new_donor, new_receiver] = new.prepared.arrangement().tracks() else {
        panic!("two tracks");
    };
    let original_node = old_donor.foreign_instrument.as_ref().unwrap();
    let moved_node = new_receiver.foreign_instrument.as_ref().unwrap();
    assert!(old_receiver.foreign_instrument.is_none());
    assert!(new_donor.foreign_instrument.is_none());
    assert_eq!(old_donor.canonical_track_id(), Some(donor));
    assert_eq!(new_receiver.canonical_track_id(), Some(receiver));
    assert_eq!(original_node.owner, moved_node.owner);
    assert_eq!(
        original_node.foreign_processor(),
        moved_node.foreign_processor()
    );
    assert_eq!(
        original_node
            .foreign_processor()
            .unwrap()
            .bridge_latency_frames(),
        512
    );
    let binding = original_node
        .foreign_processor()
        .unwrap()
        .binding()
        .unwrap();
    (
        [old, new],
        HostedInstrumentMove {
            donor,
            receiver,
            device,
            binding,
        },
    )
}

#[test]
fn two_bare_midi_routes_share_clock_and_refuse_dirty_moved_history() {
    let ([old, new], movement) = prepare_move(BeatTime::from_quarters(1).unwrap());
    crate::signal_graph::tests::verify_held_move_clock(
        Arc::clone(old.prepared.arrangement().signal_graph.as_ref().unwrap()),
        Arc::clone(new.prepared.arrangement().signal_graph.as_ref().unwrap()),
        movement.donor,
        movement.receiver,
    );
}

#[test]
fn hosted_track_fade_refuses_wrong_epoch_or_an_ended_donor_gate() {
    for note_is_held in [true, false] {
        let duration = if note_is_held {
            BeatTime::from_quarters(1).unwrap()
        } else {
            BeatTime::from_ratio(1, 32).unwrap()
        };
        let ([old, new], mut movement) = prepare_move(duration);
        let mut original =
            SessionPlayback::new_with_device_resources(old.prepared, &old.resources).unwrap();
        let mut hosted = NoteEndpoint {
            instrument_sample: 0.25,
            ..NoteEndpoint::default()
        };
        original.apply(SessionPlaybackCommand::Play).unwrap();
        for _ in 0..4 {
            hosted.render(&mut original, &mut [[0.0; 2]; 256]).unwrap();
        }
        let mut candidate =
            SessionPlayback::new_with_device_resources(new.prepared, &new.resources).unwrap();
        candidate.require_continuous_insert = true;
        candidate.hosted_instrument_move = Some(movement);
        assert_eq!(
            candidate.direct_hosted_track_cut_from(&original).is_some(),
            note_is_held
        );
        if note_is_held {
            movement.binding.epoch += 1;
            candidate.hosted_instrument_move = Some(movement);
        }
        let clock = original.performance_frame();
        let gain = original.track_faders[0].current();
        assert!(matches!(
            candidate.advance_native_removal_from(&mut original),
            Err(crate::session_playback::foreign_arrangement::INCOMPATIBLE_HANDOFF)
        ));
        assert_eq!(original.performance_frame(), clock);
        assert_eq!(original.track_faders[0].current(), gain);
        assert!(!original.track_faders[0].active());
    }
}
