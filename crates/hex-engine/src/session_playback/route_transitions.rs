//! Bounded transactional observation log for applied Session route transitions.
//!
//! `SessionPlayback` records an entry only when an immediate route command
//! commits or a due quantized transition applies inside a committed render
//! block. Refused, superseded, and cancelled pending commands emit nothing;
//! buffer overflow is surfaced explicitly without interrupting audio rendering.
use super::*;

/// Fixed per-runtime bounded capacity for committed applied Session route
/// transitions awaiting callback drain.
pub const MAX_APPLIED_SESSION_ROUTE_TRANSITIONS: usize = 128;

/// Applied owner of a Session route transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppliedSessionRouteTarget {
    Arrangement,
    SessionMidi {
        clip: ClipId,
        origin: SessionMidiOrigin,
    },
    SessionAudio {
        clip: ClipId,
        launch_frame: u64,
    },
    Silence,
}

impl From<RouteOwner> for AppliedSessionRouteTarget {
    fn from(owner: RouteOwner) -> Self {
        match owner {
            RouteOwner::Arrangement => Self::Arrangement,
            RouteOwner::Session { clip, origin, .. } => Self::SessionMidi { clip, origin },
            RouteOwner::SessionAudio { clip, launch_frame } => {
                Self::SessionAudio { clip, launch_frame }
            }
            RouteOwner::Silence => Self::Silence,
        }
    }
}

/// One applied Session route transition committed by `SessionPlayback`.
///
/// Captures the actual applied track, optional originating scene, previous and
/// new owners, monotonic performance frame, content frame, loop pass, and
/// musical beat at the exact transition point. Never records command enqueue
/// time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppliedSessionRouteTransition {
    pub track: TrackId,
    pub scene: Option<hex_project::SceneId>,
    pub previous: AppliedSessionRouteTarget,
    pub target: AppliedSessionRouteTarget,
    pub performance_frame: u64,
    pub content_frame: u64,
    pub loop_pass: u64,
    pub beat: BeatTime,
}

/// Fixed-capacity transactional staging/commit log owned by `SessionPlayback`.
/// Zero callback allocation or drop.
pub(super) struct RouteTransitionLog {
    committed: [Option<AppliedSessionRouteTransition>; MAX_APPLIED_SESSION_ROUTE_TRANSITIONS],
    committed_len: usize,
    committed_overflowed: bool,
    scratch: [Option<AppliedSessionRouteTransition>; MAX_APPLIED_SESSION_ROUTE_TRANSITIONS],
    scratch_len: usize,
    scratch_overflowed: bool,
}

impl Default for RouteTransitionLog {
    fn default() -> Self {
        Self {
            committed: [None; MAX_APPLIED_SESSION_ROUTE_TRANSITIONS],
            committed_len: 0,
            committed_overflowed: false,
            scratch: [None; MAX_APPLIED_SESSION_ROUTE_TRANSITIONS],
            scratch_len: 0,
            scratch_overflowed: false,
        }
    }
}

impl RouteTransitionLog {
    pub(super) fn begin_scratch(&mut self) {
        self.scratch_len = 0;
        self.scratch_overflowed = false;
    }

    pub(super) fn stage(&mut self, transition: AppliedSessionRouteTransition) {
        if let Some(slot) = self.scratch.get_mut(self.scratch_len) {
            *slot = Some(transition);
            self.scratch_len += 1;
        } else {
            self.scratch_overflowed = true;
        }
    }

    pub(super) fn commit_scratch(&mut self) {
        if self.scratch_overflowed {
            self.committed_overflowed = true;
        }
        for index in 0..self.scratch_len {
            let Some(transition) = self.scratch[index].take() else {
                continue;
            };
            if let Some(slot) = self.committed.get_mut(self.committed_len) {
                *slot = Some(transition);
                self.committed_len += 1;
            } else {
                self.committed_overflowed = true;
            }
        }
        self.scratch_len = 0;
        self.scratch_overflowed = false;
    }

    /// Drain every committed transition in FIFO order and clear the overflow
    /// latch. Returns `true` if one or more transitions were dropped due to
    /// bounded capacity before this drain.
    pub(super) fn drain(&mut self, mut visit: impl FnMut(AppliedSessionRouteTransition)) -> bool {
        let len = self.committed_len;
        let overflowed = self.committed_overflowed;
        for slot in &mut self.committed[..len] {
            if let Some(transition) = slot.take() {
                visit(transition);
            }
        }
        self.committed_len = 0;
        self.committed_overflowed = false;
        overflowed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SineTable;
    use hex_project::{Command, Created, Editor, Project, SceneId};
    use std::collections::BTreeMap;

    const RATE: u32 = 8_000;

    fn build_two_track_session() -> (
        Project,
        TrackId,
        TrackId,
        SceneId,
        SceneId,
        ClipId,
        ClipId,
        ClipId,
    ) {
        let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
        let sine_state = hex_devices::native_instrument_registry()
            .unwrap()
            .default_state(hex_devices::SINE_INSTRUMENT_TYPE_ID)
            .unwrap();
        let Created::Track(track_a) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "track a".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: sine_state.clone(),
            })
            .unwrap()
        else {
            panic!("track a")
        };
        let Created::Track(track_b) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "track b".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: sine_state,
            })
            .unwrap()
        else {
            panic!("track b")
        };
        let Created::Clip(arr_clip) = editor
            .execute(Command::PlaceMidiClip {
                track: track_a,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("arr clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip: arr_clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(4).unwrap(),
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();

        let Created::Scene(scene_1) = editor
            .execute(Command::AddScene {
                name: "scene 1".into(),
            })
            .unwrap()
        else {
            panic!("scene 1")
        };
        let Created::Scene(scene_2) = editor
            .execute(Command::AddScene {
                name: "scene 2".into(),
            })
            .unwrap()
        else {
            panic!("scene 2")
        };

        let Created::SessionClip(clip_a1) = editor
            .execute(Command::CreateSessionMidiClip {
                track: track_a,
                scene: scene_1,
                name: "a1".into(),
                source_length: BeatTime::from_quarters(1).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip a1")
        };
        editor
            .execute(Command::AddMidiNote {
                clip: clip_a1,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(1).unwrap(),
                pitch: 72,
                channel: 0,
                velocity: 110,
            })
            .unwrap();

        let Created::SessionClip(clip_b1) = editor
            .execute(Command::CreateSessionMidiClip {
                track: track_b,
                scene: scene_1,
                name: "b1".into(),
                source_length: BeatTime::from_quarters(1).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip b1")
        };
        editor
            .execute(Command::AddMidiNote {
                clip: clip_b1,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(1).unwrap(),
                pitch: 67,
                channel: 0,
                velocity: 110,
            })
            .unwrap();

        let Created::SessionClip(clip_a2) = editor
            .execute(Command::CreateSessionMidiClip {
                track: track_a,
                scene: scene_2,
                name: "a2".into(),
                source_length: BeatTime::from_quarters(1).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip a2")
        };
        editor
            .execute(Command::AddMidiNote {
                clip: clip_a2,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(1).unwrap(),
                pitch: 76,
                channel: 0,
                velocity: 110,
            })
            .unwrap();

        (
            editor.project().clone(),
            track_a,
            track_b,
            scene_1,
            scene_2,
            clip_a1,
            clip_b1,
            clip_a2,
        )
    }

    fn drain_vec(playback: &mut SessionPlayback) -> (Vec<AppliedSessionRouteTransition>, bool) {
        let mut out = Vec::new();
        let overflowed = playback.drain_applied_route_transitions(|event| out.push(event));
        (out, overflowed)
    }

    #[test]
    fn quantized_launch_scene_stop_return_and_supersession_record_only_applied_transitions() {
        let (project, track_a, track_b, scene_1, _scene_2, clip_a1, clip_b1, clip_a2) =
            build_two_track_session();
        let rev = Arc::new(
            PreparedRevision::prepare(&project, &BTreeMap::new(), Arc::new(SineTable::new()), None)
                .unwrap(),
        );
        let mut playback = SessionPlayback::new(rev).unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();

        // Advance 100 frames (at 8 kHz, 120 BPM => 4000 frames/quarter).
        let mut buf = [[0.0; 2]; 100];
        playback.render(&mut buf).unwrap();
        assert_eq!(drain_vec(&mut playback), (vec![], false));

        // Enqueue quantized LaunchClip(clip_a1), then supersede with LaunchClip(clip_a2)
        // before beat 1 (frame 4000). Neither enqueue emits an applied transition!
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip: clip_a1,
                quantization: LaunchQuantization::Quarter,
            })
            .unwrap();
        assert_eq!(drain_vec(&mut playback), (vec![], false));

        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip: clip_a2,
                quantization: LaunchQuantization::Quarter,
            })
            .unwrap();
        assert_eq!(drain_vec(&mut playback), (vec![], false));

        // Render across frame 4000 in one block [100..4200): only clip_a2 applies at frame 4000!
        let mut cross = vec![[0.0; 2]; 4100];
        playback.render(&mut cross).unwrap();
        let (events, overflowed) = drain_vec(&mut playback);
        assert!(!overflowed);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].track, track_a);
        assert_eq!(events[0].scene, None);
        assert_eq!(events[0].previous, AppliedSessionRouteTarget::Arrangement);
        assert_eq!(
            events[0].target,
            AppliedSessionRouteTarget::SessionMidi {
                clip: clip_a2,
                origin: SessionMidiOrigin::launch(BeatTime::ONE),
            }
        );
        assert_eq!(events[0].performance_frame, 4000);
        assert_eq!(events[0].content_frame, 4000);
        assert_eq!(events[0].loop_pass, 0);
        assert_eq!(events[0].beat, BeatTime::ONE);
        assert!(cross[3900..].iter().any(|f| f[0].abs() > 1e-5));

        // Quantized LaunchScene(scene_1) at beat 2 (frame 8000): applies both tracks with scene=Some(scene_1).
        playback
            .apply(SessionPlaybackCommand::LaunchScene {
                scene: scene_1,
                quantization: LaunchQuantization::Quarter,
            })
            .unwrap();
        assert_eq!(drain_vec(&mut playback), (vec![], false));

        let mut to_scene = vec![[0.0; 2]; 3900]; // 4200..8100 crosses 8000
        playback.render(&mut to_scene).unwrap();
        let (events, overflowed) = drain_vec(&mut playback);
        assert!(!overflowed);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].track, track_a);
        assert_eq!(events[0].scene, Some(scene_1));
        assert_eq!(events[0].performance_frame, 8000);
        assert!(matches!(
            events[0].target,
            AppliedSessionRouteTarget::SessionMidi { clip, .. } if clip == clip_a1
        ));
        assert_eq!(events[1].track, track_b);
        assert_eq!(events[1].scene, Some(scene_1));
        assert_eq!(events[1].performance_frame, 8000);
        assert!(matches!(
            events[1].target,
            AppliedSessionRouteTarget::SessionMidi { clip, .. } if clip == clip_b1
        ));

        // Quantized StopTrack(track_b) + CancelPending emits ZERO transition.
        playback
            .apply(SessionPlaybackCommand::StopTrack {
                track: track_b,
                quantization: LaunchQuantization::Quarter,
            })
            .unwrap();
        playback
            .apply(SessionPlaybackCommand::CancelPending {
                track: Some(track_b),
            })
            .unwrap();
        let mut pass = vec![[0.0; 2]; 4000]; // 8100..12100 crosses 12000
        playback.render(&mut pass).unwrap();
        assert_eq!(drain_vec(&mut playback), (vec![], false));

        // Immediate StopTrack(track_a) and BackToArrangement emit exact applied transitions.
        playback
            .apply(SessionPlaybackCommand::StopTrack {
                track: track_a,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        let (events, overflowed) = drain_vec(&mut playback);
        assert!(!overflowed);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].track, track_a);
        assert_eq!(events[0].target, AppliedSessionRouteTarget::Silence);
        assert_eq!(events[0].performance_frame, 12100);

        playback
            .apply(SessionPlaybackCommand::BackToArrangement {
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        let (events, overflowed) = drain_vec(&mut playback);
        assert!(!overflowed);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].track, track_a);
        assert_eq!(events[0].target, AppliedSessionRouteTarget::Arrangement);
        assert_eq!(events[1].track, track_b);
        assert_eq!(events[1].target, AppliedSessionRouteTarget::Arrangement);
    }

    #[test]
    fn loop_wrap_seek_stop_refused_command_and_overflow_preserve_truth_and_audio() {
        let (project, track_a, _track_b, _scene_1, _scene_2, clip_a1, _clip_b1, clip_a2) =
            build_two_track_session();
        let mut editor = Editor::new(project, 4).unwrap();
        editor
            .execute(Command::SetLoopRegion {
                start: 0,
                end: 4000,
                enabled: true,
            })
            .unwrap();
        let project = editor.project().clone();
        let rev = Arc::new(
            PreparedRevision::prepare(&project, &BTreeMap::new(), Arc::new(SineTable::new()), None)
                .unwrap(),
        );
        let mut playback = SessionPlayback::new(rev).unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();

        // Advance past loop wrap (frame 4000 -> content wraps to 0, loop_pass = 1).
        let mut first = vec![[0.0; 2]; 4100];
        playback.render(&mut first).unwrap();

        // Quantized launch at beat 2 (performance frame 8000, content_frame 0, loop_pass 2).
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip: clip_a1,
                quantization: LaunchQuantization::Quarter,
            })
            .unwrap();
        let mut second = vec![[0.0; 2]; 4000]; // 4100..8100 crosses 8000
        playback.render(&mut second).unwrap();
        let (events, overflowed) = drain_vec(&mut playback);
        assert!(!overflowed);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].performance_frame, 8000);
        assert_eq!(events[0].content_frame, 0);
        assert_eq!(events[0].loop_pass, 2);

        // Pending command cleared by Stop emits NO false applied transition.
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip: clip_a2,
                quantization: LaunchQuantization::Quarter,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Stop).unwrap();
        assert_eq!(drain_vec(&mut playback), (vec![], false));

        // Refused unknown clip command (using an Arrangement clip ID not in Session)
        // emits NO false applied transition.
        let unknown_clip = project.tracks()[0].midi_clips()[0].id();
        assert!(
            playback
                .apply(SessionPlaybackCommand::LaunchClip {
                    clip: unknown_clip,
                    quantization: LaunchQuantization::Immediate,
                })
                .is_err()
        );
        assert_eq!(drain_vec(&mut playback), (vec![], false));

        // Overflow (> MAX_APPLIED_SESSION_ROUTE_TRANSITIONS without drain) sets overflow flag
        // while keeping playback rendering cleanly.
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        for i in 0..(MAX_APPLIED_SESSION_ROUTE_TRANSITIONS + 12) {
            let clip = if i % 2 == 0 { clip_a1 } else { clip_a2 };
            playback
                .apply(SessionPlaybackCommand::LaunchClip {
                    clip,
                    quantization: LaunchQuantization::Immediate,
                })
                .unwrap();
        }
        let mut audio = [[0.0; 2]; 64];
        playback.render(&mut audio).unwrap();
        assert!(audio.iter().any(|f| f[0].abs() > 1e-5));
        let (events, overflowed) = drain_vec(&mut playback);
        assert!(overflowed);
        assert_eq!(events.len(), MAX_APPLIED_SESSION_ROUTE_TRANSITIONS);
        assert_eq!(events[0].track, track_a);
        // After draining, overflow latch is cleared.
        assert_eq!(drain_vec(&mut playback), (vec![], false));
    }
}
