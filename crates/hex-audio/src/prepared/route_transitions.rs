//! Bounded callback-to-control observation ring for applied Session route
//! transitions (Pulse #3455).
//!
//! `SessionPlayback` commits an `AppliedSessionRouteTransition` only when an
//! immediate route command commits or a due quantized transition applies inside
//! a successful render block. The callback drains those committed transitions,
//! stamps them with the active output identity, installed audio revision,
//! control generation, and transport epoch, and pushes them into this fixed
//! SPSC ring without allocating or perturbing audio. Overflow is surfaced
//! explicitly via `Status` and `SessionRouteTransitionDrain`.
use super::*;
use crate::native_callback_owner::ChannelStorage;
use hex_engine::AppliedSessionRouteTransition;
use rtrb::{Consumer, Producer, RingBuffer};

/// Fixed callback-to-control ring capacity for applied Session route
/// transitions.
pub const ROUTE_TRANSITION_CAPACITY: usize = 256;

/// One callback-delivered applied Session route transition stamped with its
/// owning output identity, installed audio revision, control generation, and
/// transport epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionRouteTransitionObservation {
    pub output_identity: u64,
    pub audio_revision: u64,
    pub control_generation: u64,
    pub transport_epoch: u64,
    pub transition: AppliedSessionRouteTransition,
}

/// Bounded drain outcome for `Control::drain_session_route_transitions` and
/// `Client::drain_session_route_transitions`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionRouteTransitionDrain {
    /// Number of delivered transitions written into the caller's slice.
    pub count: usize,
    /// True if the engine log or callback-to-control ring overflowed since the
    /// previous drain. Audio playback is preserved intact across overflow.
    pub overflowed: bool,
    /// Monotonic count of transitions dropped due to bounded capacity on this
    /// output.
    pub dropped_total: u64,
}

pub(in crate::prepared) struct RouteTransitionControl {
    consumer: Consumer<SessionRouteTransitionObservation>,
}

pub(in crate::prepared) struct RouteTransitionCallback {
    producer: Producer<SessionRouteTransitionObservation>,
}

pub(in crate::prepared) fn channel() -> (RouteTransitionControl, RouteTransitionCallback) {
    let (producer, consumer) = RingBuffer::new(ROUTE_TRANSITION_CAPACITY);
    (
        RouteTransitionControl { consumer },
        RouteTransitionCallback { producer },
    )
}

pub(crate) fn constructor_storage_quote() -> Option<usize> {
    ChannelStorage::ring_bytes::<SessionRouteTransitionObservation>(ROUTE_TRANSITION_CAPACITY)
}

impl RouteTransitionCallback {
    pub(in crate::prepared) fn constructor_channel_storage(&self) -> Option<ChannelStorage> {
        ChannelStorage::ring(self.producer.buffer())
    }

    pub(in crate::prepared) fn push(
        &mut self,
        status: &Status,
        observation: SessionRouteTransitionObservation,
    ) {
        if self.producer.push(observation).is_err() {
            status.route_transition_dropped.fetch_add(1, Relaxed);
            status.route_transition_overflowed.store(true, Release);
        }
    }

    pub(in crate::prepared) fn record_engine_overflow(&self, status: &Status) {
        status.route_transition_dropped.fetch_add(1, Relaxed);
        status.route_transition_overflowed.store(true, Release);
    }
}

impl RouteTransitionControl {
    pub(in crate::prepared) fn drain(
        &mut self,
        status: &Status,
        out: &mut [SessionRouteTransitionObservation],
    ) -> SessionRouteTransitionDrain {
        let mut count = 0;
        while count < out.len() {
            let Ok(event) = self.consumer.pop() else {
                break;
            };
            out[count] = event;
            count += 1;
        }
        let overflowed = status.route_transition_overflowed.swap(false, AcqRel);
        let dropped_total = status.route_transition_dropped.load(Acquire);
        SessionRouteTransitionDrain {
            count,
            overflowed,
            dropped_total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_engine::playback::{Playback, PlaybackCommand};
    use hex_engine::{
        AppliedSessionRouteTarget, LaunchQuantization, PreparedRevision, SessionMidiOrigin,
        SessionPlaybackCommand, SineTable,
    };
    use hex_project::{BeatTime, ClipId, Command, Created, Editor, Project, SceneId, TrackId};
    use std::collections::BTreeMap;

    const RATE: u32 = 8_000;

    fn make_session_project() -> (Project, TrackId, SceneId, ClipId, ClipId) {
        let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
        let sine_state = hex_devices::native_instrument_registry()
            .unwrap()
            .default_state(hex_devices::SINE_INSTRUMENT_TYPE_ID)
            .unwrap();
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "synth".into(),
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
        let Created::SessionClip(clip_1) = editor
            .execute(Command::CreateSessionMidiClip {
                track,
                scene,
                name: "c1".into(),
                source_length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("clip 1")
        };
        editor
            .execute(Command::AddMidiNote {
                clip: clip_1,
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
        let Created::SessionClip(clip_2) = editor
            .execute(Command::CreateSessionMidiClip {
                track,
                scene: scene_2,
                name: "c2".into(),
                source_length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("clip 2")
        };
        editor
            .execute(Command::AddMidiNote {
                clip: clip_2,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 67,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        (editor.project().clone(), track, scene, clip_1, clip_2)
    }

    #[test]
    fn delivered_transitions_stamp_output_revision_epoch_and_ignore_cancelled_or_refused() {
        let (project, track, scene, clip_1, clip_2) = make_session_project();
        let table = Arc::new(SineTable::new());
        let rev_1 = Arc::new(
            PreparedRevision::prepare(&project, &BTreeMap::new(), Arc::clone(&table), None)
                .unwrap(),
        );
        let playback_1 = Playback::try_from_prepared_with_session(Arc::clone(&rev_1)).unwrap();

        let (mut control, mut callback) = crate::prepared::channel(RATE).unwrap();
        let output_id = control.output_identity().unwrap();
        let applied_rev = control.publish(playback_1).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        let mut block = [[0.0; 2]; 100];
        callback.render(&mut block).unwrap();

        // Publish a stale/refused fenced candidate (mismatched revision):
        // zero transitions may be emitted by a refused publication.
        let refused_playback =
            Playback::try_from_prepared_with_session(Arc::clone(&rev_1)).unwrap();
        let _ = control.publish_with_policy(
            refused_playback,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(FreshRuntimeFence {
                expected_active_revision: applied_rev + 99,
                route_state_generation: 0,
                admitted_command_ticket: 0,
                output_fault_epoch: 0,
                consumed_stop_generation: 0,
            }),
        );
        callback.begin_block();
        callback.render(&mut block).unwrap();
        let mut scratch = [SessionRouteTransitionObservation {
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
        }; 16];
        let drained = control.drain_session_route_transitions(&mut scratch);
        assert_eq!(drained.count, 0);
        assert!(!drained.overflowed);

        // Quantized scene launch at beat 1 (frame 4000): nothing emitted at enqueue time.
        control
            .session_command_with_ticket(SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: LaunchQuantization::Quarter,
            })
            .unwrap();
        callback.begin_block();
        callback.render(&mut block).unwrap(); // 200..300
        assert_eq!(
            control.drain_session_route_transitions(&mut scratch).count,
            0
        );

        // Render across frame 4000: delivered observation carries exact output, revision, epoch, scene.
        let mut cross = vec![[0.0; 2]; 3800]; // 300..4100
        callback.begin_block();
        callback.render(&mut cross).unwrap();
        let drained = control.drain_session_route_transitions(&mut scratch);
        assert!(!drained.overflowed);
        assert_eq!(drained.count, 1);
        let obs = scratch[0];
        assert_eq!(obs.output_identity, output_id);
        assert_eq!(obs.audio_revision, applied_rev);
        assert_eq!(obs.transition.track, track);
        assert_eq!(obs.transition.scene, Some(scene));
        assert_eq!(obs.transition.performance_frame, 4000);
        assert_eq!(obs.transition.content_frame, 4000);
        assert_eq!(obs.transition.beat, BeatTime::ONE);
        assert_eq!(
            obs.transition.target,
            AppliedSessionRouteTarget::SessionMidi {
                clip: clip_1,
                origin: SessionMidiOrigin::launch(BeatTime::ONE),
            }
        );

        // Refused command (unknown clip from a separate project) emits zero transitions.
        let mut other = Editor::new(Project::new(RATE).unwrap(), 4).unwrap();
        let Created::Track(other_track) = other
            .execute(Command::AddMidiTrack { name: "x".into() })
            .unwrap()
        else {
            panic!("other track")
        };
        let Created::Clip(unknown_clip) = other
            .execute(Command::PlaceMidiClip {
                track: other_track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("unknown clip")
        };
        control
            .session_command_with_ticket(SessionPlaybackCommand::LaunchClip {
                clip: unknown_clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        callback.begin_block();
        callback.render(&mut block).unwrap();
        assert_eq!(
            control.drain_session_route_transitions(&mut scratch).count,
            0
        );

        // Fill ring past ROUTE_TRANSITION_CAPACITY without draining: overflow is surfaced,
        // audio still renders cleanly, and priority Stop stays effective!
        for i in 0..(ROUTE_TRANSITION_CAPACITY + 20) {
            let clip = if i % 2 == 0 { clip_2 } else { clip_1 };
            control
                .session_command_with_ticket(SessionPlaybackCommand::LaunchClip {
                    clip,
                    quantization: LaunchQuantization::Immediate,
                })
                .unwrap();
            callback.begin_block();
            let mut tiny = [[0.0; 2]; 4];
            callback.render(&mut tiny).unwrap();
            assert!(tiny.iter().any(|f| f[0].abs() > 1e-5));
        }
        assert!(control.session_route_transitions_overflowed());
        assert!(control.session_route_transitions_dropped() >= 20);

        // Priority Stop still succeeds cleanly even when the observation ring is full.
        control.stop();
        callback.begin_block();
        let mut stopped = [[0.0; 2]; 16];
        callback.render(&mut stopped).unwrap();
        assert!(!control.status.playing.load(Acquire));

        // Drain full ring cleanly and verify overflow latch resets.
        let mut all = vec![scratch[0]; ROUTE_TRANSITION_CAPACITY];
        let drained = control.drain_session_route_transitions(&mut all);
        assert_eq!(drained.count, ROUTE_TRANSITION_CAPACITY);
        assert!(drained.overflowed);
        assert!(drained.dropped_total >= 20);
        assert!(!control.session_route_transitions_overflowed());
    }
}
