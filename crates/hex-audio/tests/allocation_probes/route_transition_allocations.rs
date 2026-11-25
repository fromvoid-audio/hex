//! Zero-allocation/free probe for callback Session route transition
//! observation delivery and overflow handling (Pulse #3455).
#![cfg(feature = "allocation-probe")]

use hex_audio::prepared::{self, ROUTE_TRANSITION_CAPACITY, SessionRouteTransitionObservation};
use hex_engine::playback::{Playback, PlaybackCommand};
use hex_engine::{
    AppliedSessionRouteTarget, AppliedSessionRouteTransition, LaunchQuantization, PreparedRevision,
    SessionPlaybackCommand, SineTable,
};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::collections::BTreeMap;
use std::sync::Arc;

#[test]
fn callback_route_transition_delivery_and_overflow_allocate_and_free_nothing() {
    const RATE: u32 = 8_000;
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
    let sine_state = hex_devices::native_instrument_registry()
        .unwrap()
        .default_state(hex_devices::SINE_INSTRUMENT_TYPE_ID)
        .unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "probe".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: sine_state,
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Scene(scene) = editor
        .execute(Command::AddScene { name: "s1".into() })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(clip_a) = editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "ca".into(),
            source_length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip a")
    };
    editor
        .execute(Command::AddMidiNote {
            clip: clip_a,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let Created::Scene(scene_2) = editor
        .execute(Command::AddScene { name: "s2".into() })
        .unwrap()
    else {
        panic!("scene 2")
    };
    let Created::SessionClip(clip_b) = editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene: scene_2,
            name: "cb".into(),
            source_length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip b")
    };
    editor
        .execute(Command::AddMidiNote {
            clip: clip_b,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 67,
            channel: 0,
            velocity: 100,
        })
        .unwrap();

    let rev = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let playback = Playback::try_from_prepared_with_session(rev).unwrap();
    let (mut control, mut callback) = prepared::channel(RATE).unwrap();
    control.publish(playback).unwrap();
    control.command(PlaybackCommand::Play).unwrap();

    let mut warmup = [[0.0; 2]; 100];
    callback.begin_block();
    callback.render(&mut warmup).unwrap();

    // Schedule quantized scene launch at beat 1 (frame 4000).
    control
        .session_command_with_ticket(SessionPlaybackCommand::LaunchScene {
            scene,
            quantization: LaunchQuantization::Quarter,
        })
        .unwrap();

    let mut cross = [[0.0; 2]; 4_000];
    let counts = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut cross).unwrap();
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current,
        ),
        (0, 0, 0, 0)
    );

    let mut out = [SessionRouteTransitionObservation {
        output_identity: 0,
        audio_revision: 0,
        control_generation: 0,
        transport_epoch: 0,
        transition: AppliedSessionRouteTransition {
            track,
            scene: None,
            previous: AppliedSessionRouteTarget::Arrangement,
            target: AppliedSessionRouteTarget::Silence,
            performance_frame: 0,
            content_frame: 0,
            loop_pass: 0,
            beat: BeatTime::ZERO,
        },
    }; 4];
    let drained = control.drain_session_route_transitions(&mut out);
    assert_eq!(drained.count, 1);
    assert!(!drained.overflowed);
    assert_eq!(out[0].transition.scene, Some(scene));
    assert_eq!(out[0].transition.performance_frame, 4_000);

    // Fill the ring to overflow and verify callback stays zero-allocation/free.
    for i in 0..ROUTE_TRANSITION_CAPACITY {
        let clip = if i % 2 == 0 { clip_b } else { clip_a };
        control
            .session_command_with_ticket(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        callback.begin_block();
        callback.render(&mut warmup[..4]).unwrap();
    }
    control
        .session_command_with_ticket(SessionPlaybackCommand::LaunchClip {
            clip: clip_b,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    let overflow_counts = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut warmup[..4]).unwrap();
    });
    assert_eq!(
        (
            overflow_counts.count_total,
            overflow_counts.bytes_total,
            overflow_counts.count_current,
            overflow_counts.bytes_current,
        ),
        (0, 0, 0, 0)
    );
    assert!(control.session_route_transitions_overflowed());
    assert!(control.session_route_transitions_dropped() >= 1);
}
