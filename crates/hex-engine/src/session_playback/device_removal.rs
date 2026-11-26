//! A short fade on the departing native stage or audio track. Preparation
//! previews the existing handoff before changing the donor. Publication owns
//! the outer source fence and restores the donor when it returns a candidate.
use super::hosted_track_effect_move::HostedEffectCut;
use super::{
    RouteOwner, SessionPlayback, SessionPlaybackError, device_move::NativeTrackMove,
    hosted_instrument_move::HostedTrackCut, instrument_move::InstrumentTrackMove,
};
use crate::playback::HostedMoveRequest;
use crate::signal_graph::FreshHistory;
use hex_lattice::{NativeRemovalTransition, NativeRuntimeChain, ProcessorChainOwner};
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{ReturnId, TrackId};
use std::num::NonZeroU64;

#[derive(Clone, Copy)]
pub(super) struct ReturnFade {
    pub(super) id: ReturnId,
    pub(super) fader: super::track_fader::TrackFader,
}

/// Progress of a staged native removal at a render boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeRemovalProgress {
    NotApplicable,
    Fading,
    Ready,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Audio, Frame, LaunchQuantization, PreparedRevision, SessionPlaybackCommand};
    use hex_project::{Command, Created, Editor, Project};
    use std::{collections::BTreeMap, sync::Arc};

    #[test]
    fn session_audio_track_removal_fades_the_live_route_before_handoff() {
        const RATE: u32 = 48_000;
        const FADE: usize = 240;
        const SIGNAL: Frame = [0.25, -0.125];
        let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "session audio".into(),
                path: "session.wav".into(),
                sample_rate: RATE,
                frames: 2048,
            })
            .unwrap()
        else {
            panic!("audio");
        };
        let clip = editor.project().track(track).unwrap().clips()[0].id();
        editor.execute(Command::RemoveClip(clip)).unwrap();
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .unwrap()
        else {
            panic!("scene");
        };
        let Created::SessionClip(clip) = editor
            .execute(Command::CreateSessionAudioClip {
                track,
                scene,
                name: "session".into(),
                asset,
                source_start: 0,
                source_end: 2048,
            })
            .unwrap()
        else {
            panic!("session clip");
        };
        let media = BTreeMap::from([(asset, Audio::new(RATE, vec![SIGNAL; 2048]).unwrap())]);
        let table = Arc::new(crate::SineTable::new());
        let prepare = |project: &Project| {
            Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
        };
        let before = prepare(editor.project());
        editor.execute(Command::RemoveTrack(track)).unwrap();
        let after = prepare(editor.project());
        for cancel in [false, true] {
            let mut old = SessionPlayback::new(Arc::clone(&before)).unwrap();
            old.apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
            old.apply(SessionPlaybackCommand::Play).unwrap();
            let mut prefix = [[0.0; 2]; 32];
            old.render(&mut prefix).unwrap();
            assert_eq!(prefix, [SIGNAL; 32]);
            let mut next = SessionPlayback::new(Arc::clone(&after)).unwrap();
            next.require_continuous_insert = true;
            assert_eq!(
                next.advance_native_removal_from(&mut old).unwrap(),
                NativeRemovalProgress::Fading
            );
            old.render(&mut prefix).unwrap();
            for (index, frame) in prefix.iter().enumerate() {
                let gain = 1.0 - (index + 1) as f32 / FADE as f32;
                assert!((frame[0] - SIGNAL[0] * gain).abs() < 1e-6);
            }
            if cancel {
                next.cancel_native_removal_from(&mut old);
                let mut restored = [[0.0; 2]; FADE];
                old.render(&mut restored).unwrap();
                assert!(restored[0][0] > prefix[31][0]);
                assert_eq!(restored[FADE - 1], SIGNAL);
            } else {
                let mut rest = [[0.0; 2]; FADE - 32];
                old.render(&mut rest).unwrap();
                assert_eq!(rest[FADE - 33], [0.0; 2]);
                assert_eq!(
                    next.advance_native_removal_from(&mut old).unwrap(),
                    NativeRemovalProgress::Ready
                );
                next.try_inherit_state_live_from(&mut old).unwrap();
                let mut output = [[0.0; 2]; 32];
                next.render(&mut output).unwrap();
                assert_eq!(output, [[0.0; 2]; 32]);
                assert_eq!(next.performance_frame(), (32 + FADE + 32) as u64);
            }
        }
    }

    #[test]
    fn live_mute_unmute_solo_cancel_and_compensated_fade_smoothly() {
        use super::super::track_fader::TrackFader;

        fn assert_smooth_steps(samples: &[Frame], prev: Frame, max_step: f32) {
            let mut last = prev;
            for (index, &frame) in samples.iter().enumerate() {
                for ch in 0..2 {
                    let delta = (frame[ch] - last[ch]).abs();
                    assert!(
                        delta <= max_step,
                        "step jump at frame {index} ch {ch}: {} -> {} (delta {delta} > {max_step})",
                        last[ch],
                        frame[ch]
                    );
                }
                last = frame;
            }
        }

        // Verify exact fade length and smoothness across rates and block sizes,
        // including mute, unmute, solo/unsolo, and cancellation rollback.
        for (rate, block) in [(44_100, 31), (48_000, 1), (48_000, 64), (96_000, 256)] {
            let fade = TrackFader::fade_frames(rate) as usize;
            let len = fade * 8 + 512;
            let sig_a: Frame = [0.4, -0.2];
            let sig_b: Frame = [0.2, 0.3];
            let mut editor = Editor::new(Project::new(rate).unwrap(), 16).unwrap();
            let Created::Imported {
                track: track_a,
                asset: asset_a,
                ..
            } = editor
                .execute(Command::ImportAudioTrack {
                    name: "a".into(),
                    path: "a.wav".into(),
                    sample_rate: rate,
                    frames: len as u64,
                })
                .unwrap()
            else {
                panic!("import a");
            };
            let Created::Imported {
                track: track_b,
                asset: asset_b,
                ..
            } = editor
                .execute(Command::ImportAudioTrack {
                    name: "b".into(),
                    path: "b.wav".into(),
                    sample_rate: rate,
                    frames: len as u64,
                })
                .unwrap()
            else {
                panic!("import b");
            };
            let media = BTreeMap::from([
                (asset_a, Audio::new(rate, vec![sig_a; len]).unwrap()),
                (asset_b, Audio::new(rate, vec![sig_b; len]).unwrap()),
            ]);
            let table = Arc::new(crate::SineTable::new());
            let prep = |p: &Project| {
                Arc::new(PreparedRevision::prepare(p, &media, Arc::clone(&table), None).unwrap())
            };
            let sum = [sig_a[0] + sig_b[0], sig_a[1] + sig_b[1]];
            let rev_both = prep(editor.project());
            editor
                .execute(Command::Mute {
                    track: track_b,
                    muted: true,
                })
                .unwrap();
            let rev_b_muted = prep(editor.project());
            editor
                .execute(Command::Mute {
                    track: track_b,
                    muted: false,
                })
                .unwrap();
            editor
                .execute(Command::ToggleSolo {
                    track: track_a,
                    exclusive: true,
                })
                .unwrap();
            let rev_a_solo = prep(editor.project());
            editor
                .execute(Command::ToggleSolo {
                    track: track_a,
                    exclusive: true,
                })
                .unwrap();
            let rev_unsolo = prep(editor.project());

            let mut old = SessionPlayback::new(Arc::clone(&rev_both)).unwrap();
            old.apply(SessionPlaybackCommand::Play).unwrap();
            let mut buf = vec![[0.0; 2]; block];
            old.render(&mut buf).unwrap();
            assert_eq!(buf[block - 1], sum);

            // 1. Live mute track B + cancel halfway -> smooth recovery to sum.
            let mut cancel_cand = SessionPlayback::new(Arc::clone(&rev_b_muted)).unwrap();
            assert_eq!(
                cancel_cand.advance_mute_fade_from(&mut old).unwrap(),
                NativeRemovalProgress::Fading
            );
            let mut mid = vec![[0.0; 2]; (fade / 2).max(1)];
            old.render(&mut mid).unwrap();
            assert_smooth_steps(
                &mid,
                sum,
                (sig_b[0].abs().max(sig_b[1].abs())) / fade as f32 + 1e-4,
            );
            cancel_cand.cancel_native_removal_from(&mut old);
            let mut recovered = vec![[0.0; 2]; fade + 8];
            old.render(&mut recovered).unwrap();
            assert_smooth_steps(
                &recovered,
                *mid.last().unwrap(),
                (sig_b[0].abs().max(sig_b[1].abs())) / fade as f32 + 1e-4,
            );
            assert!((recovered.last().unwrap()[0] - sum[0]).abs() < 1e-5);

            // 2. Live mute track B to completion, then unmute smoothly.
            let mut mute_cand = SessionPlayback::new(Arc::clone(&rev_b_muted)).unwrap();
            let mut rendered = Vec::new();
            loop {
                match mute_cand.advance_mute_fade_from(&mut old).unwrap() {
                    NativeRemovalProgress::Fading => {
                        let mut chunk = vec![[0.0; 2]; block];
                        old.render(&mut chunk).unwrap();
                        rendered.extend_from_slice(&chunk);
                    }
                    NativeRemovalProgress::Ready => break,
                    NativeRemovalProgress::NotApplicable => panic!("expected mute fade"),
                }
            }
            assert!(rendered.len() >= fade);
            assert_smooth_steps(
                &rendered,
                sum,
                (sig_b[0].abs().max(sig_b[1].abs())) / fade as f32 + 1e-4,
            );
            assert!((rendered.last().unwrap()[0] - sig_a[0]).abs() < 1e-5);
            mute_cand.try_inherit_state_live_from(&mut old).unwrap();
            let mut post_mute = vec![[0.0; 2]; 16];
            mute_cand.render(&mut post_mute).unwrap();
            assert_eq!(post_mute[0], sig_a);

            // Unmute track B -> ramps smoothly from sig_a back to sum.
            let mut unmute_cand = SessionPlayback::new(Arc::clone(&rev_both)).unwrap();
            assert_eq!(
                unmute_cand.advance_mute_fade_from(&mut mute_cand).unwrap(),
                NativeRemovalProgress::NotApplicable
            );
            unmute_cand
                .try_inherit_state_live_from(&mut mute_cand)
                .unwrap();
            let mut ramp_in = vec![[0.0; 2]; fade + 8];
            unmute_cand.render(&mut ramp_in).unwrap();
            assert_smooth_steps(
                &ramp_in,
                sig_a,
                (sig_b[0].abs().max(sig_b[1].abs())) / fade as f32 + 1e-4,
            );
            assert!((ramp_in.last().unwrap()[0] - sum[0]).abs() < 1e-5);

            // 3. Solo track A -> track B fades out smoothly while A stays steady; unsolo ramps B in.
            let mut solo_cand = SessionPlayback::new(Arc::clone(&rev_a_solo)).unwrap();
            let mut solo_out = Vec::new();
            loop {
                match solo_cand.advance_mute_fade_from(&mut unmute_cand).unwrap() {
                    NativeRemovalProgress::Fading => {
                        let mut chunk = vec![[0.0; 2]; block];
                        unmute_cand.render(&mut chunk).unwrap();
                        solo_out.extend_from_slice(&chunk);
                    }
                    NativeRemovalProgress::Ready => break,
                    NativeRemovalProgress::NotApplicable => panic!("expected solo fade"),
                }
            }
            assert_smooth_steps(
                &solo_out,
                sum,
                (sig_b[0].abs().max(sig_b[1].abs())) / fade as f32 + 1e-4,
            );
            assert!((solo_out.last().unwrap()[0] - sig_a[0]).abs() < 1e-5);
            solo_cand
                .try_inherit_state_live_from(&mut unmute_cand)
                .unwrap();
            let mut unsolo_cand = SessionPlayback::new(Arc::clone(&rev_unsolo)).unwrap();
            unsolo_cand
                .try_inherit_state_live_from(&mut solo_cand)
                .unwrap();
            let mut unsolo_in = vec![[0.0; 2]; fade + 8];
            unsolo_cand.render(&mut unsolo_in).unwrap();
            assert_smooth_steps(
                &unsolo_in,
                sig_a,
                (sig_b[0].abs().max(sig_b[1].abs())) / fade as f32 + 1e-4,
            );
            assert!((unsolo_in.last().unwrap()[0] - sum[0]).abs() < 1e-5);
        }

        // Compensated routing: latency-bearing Limiter (True Peak) on track A;
        // muting zero-latency track B fades smoothly at Main without stepping.
        let rate = 48_000;
        let fade = TrackFader::fade_frames(rate) as usize;
        let len = 4096;
        let mut editor = Editor::new(Project::new(rate).unwrap(), 16).unwrap();
        let Created::Imported {
            track: track_a,
            asset: asset_a,
            ..
        } = editor
            .execute(Command::ImportAudioTrack {
                name: "comp-a".into(),
                path: "comp-a.wav".into(),
                sample_rate: rate,
                frames: len as u64,
            })
            .unwrap()
        else {
            panic!("comp a");
        };
        let Created::Imported {
            track: track_b,
            asset: asset_b,
            ..
        } = editor
            .execute(Command::ImportAudioTrack {
                name: "comp-b".into(),
                path: "comp-b.wav".into(),
                sample_rate: rate,
                frames: len as u64,
            })
            .unwrap()
        else {
            panic!("comp b");
        };
        let limiter_state = hex_devices::limiter::DEFINITION.default_state().unwrap();
        let limiter_state = crate::patch_native_device_parameter(
            &limiter_state,
            hex_devices::limiter::parameter::MODE,
            hex_lattice_contract::ParameterValue::Choice(2),
        )
        .unwrap();
        editor
            .execute(Command::InsertDevice {
                track: track_a,
                before: None,
                state: limiter_state,
            })
            .unwrap();
        let media = BTreeMap::from([
            (asset_a, Audio::new(rate, vec![[0.15, 0.15]; len]).unwrap()),
            (asset_b, Audio::new(rate, vec![[0.25, -0.25]; len]).unwrap()),
        ]);
        let table = Arc::new(crate::SineTable::new());
        let before = Arc::new(
            PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None).unwrap(),
        );
        assert!(before.arrangement().main_latency_frames() > 0);
        editor
            .execute(Command::Mute {
                track: track_b,
                muted: true,
            })
            .unwrap();
        let after = Arc::new(
            PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None).unwrap(),
        );
        let mut old = SessionPlayback::new(before).unwrap();
        old.apply(SessionPlaybackCommand::Play).unwrap();
        // Warm up past main latency so both tracks are settled at Main output.
        let mut warmup = vec![[0.0; 2]; 512];
        old.render(&mut warmup).unwrap();
        let settled = *warmup.last().unwrap();
        let mut next = SessionPlayback::new(after).unwrap();
        let mut out = Vec::new();
        loop {
            match next.advance_mute_fade_from(&mut old).unwrap() {
                NativeRemovalProgress::Fading => {
                    let mut chunk = [[0.0; 2]; 64];
                    old.render(&mut chunk).unwrap();
                    out.extend_from_slice(&chunk);
                }
                NativeRemovalProgress::Ready => break,
                NativeRemovalProgress::NotApplicable => panic!("expected compensated mute fade"),
            }
        }
        next.try_inherit_state_live_from(&mut old).unwrap();
        let mut tail = vec![[0.0; 2]; 512];
        next.render(&mut tail).unwrap();
        out.extend_from_slice(&tail);
        assert_smooth_steps(&out, settled, 0.25 / fade as f32 + 2e-4);
        let final_frame = *out.last().unwrap();
        assert!((final_frame[0] - 0.15).abs() < 1e-3);
        assert!((final_frame[1] - 0.15).abs() < 1e-3);
    }
}

#[derive(Clone, Copy)]
pub(super) struct PendingNativeRemoval {
    transition: RemovalTransition,
    runtime: u64,
    epoch: u64,
    started_at: Option<NonZeroU64>,
}

#[derive(Clone, Copy, Default)]
struct MuteFadeMask([u64; 4]);

impl MuteFadeMask {
    fn set(&mut self, index: usize) {
        if index < 256 {
            self.0[index / 64] |= 1u64 << (index % 64);
        }
    }
    fn is_empty(self) -> bool {
        self.0 == [0; 4]
    }
    fn iter_indices(self) -> impl Iterator<Item = usize> {
        self.0.into_iter().enumerate().flat_map(|(word, bits)| {
            (0..64usize)
                .filter_map(move |bit| ((bits & (1u64 << bit)) != 0).then_some(word * 64 + bit))
        })
    }
}

#[derive(Clone, Copy)]
enum RemovalTransition {
    Device(NativeRemovalTransition),
    MainPass(NativeRemovalTransition),
    Track(usize),
    Reorder(usize),
    Route {
        leaf: usize,
        outgoing: usize,
        /// The flat boundary fades a different post-device output after the cut.
        incoming: Option<usize>,
    },
    Instrument(usize),
    Move(StagedTrackMove),
    Return {
        id: ReturnId,
        pass_latency: Option<u32>,
    },
    ReturnPowerOn {
        id: ReturnId,
        device: u64,
    },
    Mute(MuteFadeMask),
}

#[derive(Clone, Copy)]
enum StagedTrackMove {
    Effect(NativeTrackMove),
    Instrument(InstrumentTrackMove),
    Hosted(HostedTrackCut),
    HostedEffect(HostedEffectCut),
}

impl StagedTrackMove {
    fn indices(self) -> [usize; 2] {
        match self {
            Self::Effect(NativeTrackMove {
                source,
                destination,
                ..
            })
            | Self::Instrument(InstrumentTrackMove {
                source,
                destination,
                ..
            })
            | Self::Hosted(HostedTrackCut {
                source,
                destination,
                ..
            })
            | Self::HostedEffect(HostedEffectCut {
                source,
                destination,
                ..
            }) => [source, destination],
        }
    }
}

impl PendingNativeRemoval {
    pub(super) fn matches(self, previous: &SessionPlayback) -> bool {
        self.runtime == previous.capture_runtime && self.epoch == previous.capture_epoch
    }

    pub(super) fn group_reroute_track(self, previous: &SessionPlayback) -> Option<TrackId> {
        match self.transition {
            RemovalTransition::Route { leaf, .. } if self.matches(previous) => {
                previous.routes.get(leaf).map(|route| route.id)
            }
            _ => None,
        }
    }

    pub(super) fn verified_flat_group_faders(
        self,
        previous: &SessionPlayback,
    ) -> Option<(usize, usize)> {
        match self.transition {
            RemovalTransition::Route {
                outgoing,
                incoming: Some(incoming),
                ..
            } if self.matches(previous) => Some((outgoing, incoming)),
            _ => None,
        }
    }

    pub(super) fn is_mute_fade(self) -> bool {
        matches!(self.transition, RemovalTransition::Mute(_))
    }

    pub(super) fn device_transition(self) -> Option<NativeRemovalTransition> {
        match self.transition {
            RemovalTransition::Device(transition) | RemovalTransition::MainPass(transition) => {
                Some(transition)
            }
            RemovalTransition::Track(_)
            | RemovalTransition::Reorder(_)
            | RemovalTransition::Route { .. }
            | RemovalTransition::Instrument(_)
            | RemovalTransition::Move(_)
            | RemovalTransition::Return { .. }
            | RemovalTransition::ReturnPowerOn { .. }
            | RemovalTransition::Mute(_) => None,
        }
    }

    pub(super) fn main_pass_transition(self) -> Option<NativeRemovalTransition> {
        match self.transition {
            RemovalTransition::MainPass(transition) => Some(transition),
            _ => None,
        }
    }

    pub(super) fn faded_instrument_track(
        self,
        previous: &SessionPlayback,
    ) -> Option<hex_project::TrackId> {
        match self.transition {
            RemovalTransition::Instrument(index)
            | RemovalTransition::Move(StagedTrackMove::Instrument(InstrumentTrackMove {
                source: index,
                ..
            })) => previous.routes.get(index).map(|route| route.id),
            _ => None,
        }
    }

    pub(super) fn faded_return(self) -> Option<ReturnId> {
        match self.transition {
            RemovalTransition::Return { id, .. } | RemovalTransition::ReturnPowerOn { id, .. } => {
                Some(id)
            }
            _ => None,
        }
    }

    pub(super) fn return_power_on(self) -> Option<(ReturnId, u64)> {
        match self.transition {
            RemovalTransition::ReturnPowerOn { id, device } => Some((id, device)),
            _ => None,
        }
    }

    pub(super) fn return_pass_latency(self) -> Option<u32> {
        match self.transition {
            RemovalTransition::Return { pass_latency, .. } => pass_latency,
            _ => None,
        }
    }

    pub(super) fn track_move(self) -> Option<NativeTrackMove> {
        match self.transition {
            RemovalTransition::Move(StagedTrackMove::Effect(movement)) => Some(movement),
            _ => None,
        }
    }

    pub(super) fn instrument_track_move(self) -> Option<InstrumentTrackMove> {
        match self.transition {
            RemovalTransition::Move(StagedTrackMove::Instrument(movement)) => Some(movement),
            _ => None,
        }
    }

    pub(super) fn hosted_track_move(self) -> Option<HostedTrackCut> {
        match self.transition {
            RemovalTransition::Move(StagedTrackMove::Hosted(movement)) => Some(movement),
            _ => None,
        }
    }

    pub(super) fn hosted_effect_move(self) -> Option<HostedEffectCut> {
        match self.transition {
            RemovalTransition::Move(StagedTrackMove::HostedEffect(movement)) => Some(movement),
            _ => None,
        }
    }

    pub(super) fn faded_native_identity(self) -> Option<hex_lattice::ProcessorInstanceIdentity> {
        self.device_transition()
            .map(|transition| transition.identity())
            .or_else(|| self.track_move().map(|movement| movement.identity))
            .or_else(|| {
                let request = self.hosted_effect_move()?.request;
                hex_lattice::ProcessorInstanceIdentity::new(
                    ProcessorChainOwner::Track(request.donor.value()),
                    request.device.value(),
                )
                .ok()
            })
    }

    pub(super) fn faded_track(self, previous: &SessionPlayback, id: hex_project::TrackId) -> bool {
        matches!(self.transition, RemovalTransition::Route { outgoing, .. }
            if previous.routes.get(outgoing).is_some_and(|route| route.id == id))
            || self.reordered_track(previous) == Some(id)
            || self.faded_instrument_track(previous) == Some(id)
            || self.track_move_indices().is_some_and(|indices| {
                indices.into_iter().any(|index| {
                    previous
                        .routes
                        .get(index)
                        .is_some_and(|route| route.id == id)
                })
            })
    }

    fn track_move_indices(&self) -> Option<[usize; 2]> {
        match self.transition {
            RemovalTransition::Move(movement) => Some(movement.indices()),
            _ => None,
        }
    }

    pub(super) fn reordered_track(
        self,
        previous: &SessionPlayback,
    ) -> Option<hex_project::TrackId> {
        match self.transition {
            RemovalTransition::Reorder(index) => previous.routes.get(index).map(|route| route.id),
            _ => None,
        }
    }

    pub(super) fn staged_track_id(
        self,
        previous: &SessionPlayback,
    ) -> Option<hex_project::TrackId> {
        match self.transition {
            RemovalTransition::Track(index) => previous.routes.get(index).map(|route| route.id),
            RemovalTransition::Device(_)
            | RemovalTransition::MainPass(_)
            | RemovalTransition::Reorder(_)
            | RemovalTransition::Route { .. }
            | RemovalTransition::Instrument(_)
            | RemovalTransition::Move(_)
            | RemovalTransition::Return { .. }
            | RemovalTransition::ReturnPowerOn { .. }
            | RemovalTransition::Mute(_) => None,
        }
    }

    pub(super) fn ready_on(self, previous: &SessionPlayback) -> bool {
        match self.transition {
            RemovalTransition::Mute(mask) => {
                !previous.playing
                    || mask.iter_indices().all(|index| {
                        previous
                            .track_faders
                            .get(index)
                            .is_some_and(|fader| fader.mute_ready())
                    })
            }
            RemovalTransition::Move(movement) => {
                movement.indices().into_iter().all(|index| {
                    previous
                        .track_faders
                        .get(index)
                        .is_some_and(|fader| fader.removal_ready())
                }) && self.hosted_effect_move().is_none_or(|effect| {
                    previous
                        .compensation
                        .as_ref()
                        .is_some_and(|runtime| runtime.hosted_effect_clock_ready(effect.clock))
                })
            }
            RemovalTransition::Device(transition) | RemovalTransition::MainPass(transition) => {
                previous.native_removal_ready(transition)
            }
            RemovalTransition::Return { id, .. } => previous
                .return_fade
                .is_some_and(|fade| fade.id == id && fade.fader.removal_ready()),
            RemovalTransition::ReturnPowerOn { id, device } => {
                previous
                    .return_fade
                    .is_some_and(|fade| fade.id == id && fade.fader.removal_ready())
                    && previous.compensation.as_ref().is_some_and(|compensation| {
                        compensation.owner_history_is_silent(FreshHistory::ChangedReturn {
                            bus: id,
                            device,
                            cut_predecessor_pcm: false,
                        })
                    })
            }
            RemovalTransition::Track(index)
            | RemovalTransition::Reorder(index)
            | RemovalTransition::Route {
                outgoing: index, ..
            }
            | RemovalTransition::Instrument(index) => {
                previous
                    .track_faders
                    .get(index)
                    .is_some_and(|fader| fader.removal_ready())
                    && previous.compensation.as_ref().is_none_or(|compensation| {
                        // A zero fader can still have its last audible samples
                        // waiting in the old route's alignment ring.
                        previous.routes.get(index).is_some_and(|route| {
                            compensation.owner_history_is_silent(FreshHistory::RemovedTrack {
                                track: route.id,
                                staged: true,
                            })
                        })
                    })
            }
        }
    }
}

impl SessionPlayback {
    /// Preview a fitting resident removal and start its stage-local fade.
    /// Call again at render boundaries and install only at `Ready`. This
    /// never changes the transport or transfers the removed stage's custody.
    pub fn advance_native_removal_from(
        &mut self,
        previous: &mut Self,
    ) -> Result<NativeRemovalProgress, SessionPlaybackError> {
        if self.native_removal.is_some() {
            return self.advance_planned_native_removal_from(previous);
        }
        if self.revision.contains_streamed_sources()
            || previous.revision.contains_streamed_sources()
            || !self.plan_native_removal_from(previous)?
        {
            return Ok(NativeRemovalProgress::NotApplicable);
        }
        let preview = self.preview_stream_handoff_with_policy_and_report(
            previous,
            1,
            None,
            &mut |_| Err(crate::Invalid("native removal requires resident audio")),
            true,
            false,
        );
        match preview {
            Ok((_, report)) if !report.running_audio_restart => {}
            result => {
                self.native_removal = None;
                return Err(result
                    .err()
                    .unwrap_or(SessionPlaybackError::DeviceInsertNeedsConfirmation));
            }
        }
        self.advance_planned_native_removal_from(previous)
    }

    /// Classify a candidate before preview, without touching the running stage.
    /// For a streamed graph the caller must prove page readiness at this plan
    /// before advancing it. The exact successor still goes through full handoff.
    pub(crate) fn plan_native_removal_from(
        &mut self,
        previous: &Self,
    ) -> Result<bool, SessionPlaybackError> {
        if let Some(pending) = self.native_removal {
            return if pending.matches(previous) {
                Ok(true)
            } else {
                Err(SessionPlaybackError::InvalidCommand)
            };
        }
        if !self.require_continuous_insert || !previous.playing || self.live_restart {
            return Ok(false);
        }
        let before_main = previous.revision.arrangement().main_latency_frames();
        let after_main = self.revision.arrangement().main_latency_frames();
        let old_floor = previous.main_output_latency_frames();
        let same_latency = after_main == before_main;
        let fits_existing_pass_floor = after_main <= old_floor && after_main >= before_main;
        let transition = if matches!(self.hosted_move, Some(HostedMoveRequest::Instrument(_))) {
            let movement = self
                .direct_hosted_track_cut_from(previous)
                .ok_or(super::foreign_arrangement::INCOMPATIBLE_HANDOFF)?;
            Some(RemovalTransition::Move(StagedTrackMove::Hosted(movement)))
        } else if matches!(self.hosted_move, Some(HostedMoveRequest::Effect(_))) {
            let movement = self
                .direct_hosted_effect_cut_from(previous)
                .ok_or(SessionPlaybackError::DeviceInsertNeedsConfirmation)?;
            Some(RemovalTransition::Move(StagedTrackMove::HostedEffect(
                movement,
            )))
        } else if matches!(self.hosted_move, Some(HostedMoveRequest::Reorder(_))) {
            let (index, _) = self
                .direct_hosted_effect_reorder_from(previous)
                .ok_or(SessionPlaybackError::DeviceInsertNeedsConfirmation)?;
            Some(RemovalTransition::Reorder(index))
        } else if same_latency || fits_existing_pass_floor {
            self.native_removal_transition_from(previous)
                .map(RemovalTransition::Device)
                .or_else(|| {
                    self.direct_audio_track_removal_from(previous)
                        .map(RemovalTransition::Track)
                })
                .or_else(|| {
                    self.direct_audio_track_reorder_from(previous)
                        .map(RemovalTransition::Reorder)
                })
                .or_else(|| {
                    self.group_reroute_from(previous).map(|leaf| {
                        let (outgoing, incoming) = self
                            .verified_flat_group_faders(previous, leaf)
                            .map_or((leaf, None), |(outgoing, incoming)| {
                                (outgoing, Some(incoming))
                            });
                        RemovalTransition::Route {
                            leaf,
                            outgoing,
                            incoming,
                        }
                    })
                })
                .or_else(|| {
                    self.direct_audio_track_move_from(previous)
                        .map(|movement| RemovalTransition::Move(StagedTrackMove::Effect(movement)))
                })
                .or_else(|| {
                    self.direct_instrument_move_from(previous).map(|movement| {
                        RemovalTransition::Move(StagedTrackMove::Instrument(movement))
                    })
                })
                .or_else(|| {
                    self.direct_instrument_removal_from(previous)
                        .map(RemovalTransition::Instrument)
                })
                .or_else(|| {
                    self.flat_return_removal_from(previous)
                        .map(|id| RemovalTransition::Return {
                            id,
                            pass_latency: None,
                        })
                })
                .or_else(|| {
                    self.same_pass_hosted_return_power_on_from(previous, true)
                        .map(|(id, device)| RemovalTransition::ReturnPowerOn { id, device })
                })
        } else {
            self.short_main_pass_removal_from(previous)
                .or_else(|| self.short_hosted_return_off_from(previous))
                .map(RemovalTransition::MainPass)
                .or_else(|| {
                    self.short_hosted_return_pass_from(previous)
                        .map(|(id, latency)| RemovalTransition::Return {
                            id,
                            pass_latency: Some(latency),
                        })
                })
        };
        let Some(transition) = transition else {
            return Ok(false);
        };
        self.native_removal = Some(PendingNativeRemoval {
            transition,
            runtime: previous.capture_runtime,
            epoch: previous.capture_epoch,
            started_at: None,
        });
        Ok(true)
    }

    pub(crate) fn advance_planned_native_removal_from(
        &mut self,
        previous: &mut Self,
    ) -> Result<NativeRemovalProgress, SessionPlaybackError> {
        let Some(pending) = self.native_removal.as_mut() else {
            return Ok(NativeRemovalProgress::NotApplicable);
        };
        if !pending.matches(previous) {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        if pending.started_at.is_none() {
            let started_at = NonZeroU64::MIN
                .checked_add(previous.performance_frame)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
            let started = match pending.transition {
                RemovalTransition::Move(movement) => {
                    let indices = movement.indices();
                    let valid = indices
                        .into_iter()
                        .all(|index| previous.track_faders.get(index).is_some());
                    if valid {
                        let rate = previous.revision.arrangement().sample_rate();
                        for index in indices {
                            if let Some(fader) = previous.track_faders.get_mut(index) {
                                fader.begin_removal(rate);
                            }
                        }
                    }
                    valid
                }
                RemovalTransition::Device(transition) | RemovalTransition::MainPass(transition) => {
                    previous
                        .native_removal_chain_mut(transition.identity().owner())
                        .is_some_and(|chain| chain.begin_native_removal(transition))
                }
                RemovalTransition::Return { id, .. }
                | RemovalTransition::ReturnPowerOn { id, .. } => {
                    let rate = previous.revision.arrangement().sample_rate();
                    let fade = previous.return_fade.get_or_insert_with(|| ReturnFade {
                        id,
                        fader: super::track_fader::TrackFader::new([1.0; 2]),
                    });
                    if fade.id == id {
                        fade.fader.begin_removal(rate);
                        true
                    } else {
                        false
                    }
                }
                RemovalTransition::Track(index)
                | RemovalTransition::Reorder(index)
                | RemovalTransition::Route {
                    outgoing: index, ..
                }
                | RemovalTransition::Instrument(index) => {
                    let rate = previous.revision.arrangement().sample_rate();
                    previous.track_faders.get_mut(index).is_some_and(|fader| {
                        fader.begin_removal(rate);
                        true
                    })
                }
                RemovalTransition::Mute(mask) => {
                    let rate = previous.revision.arrangement().sample_rate();
                    let mut started_any = false;
                    for index in mask.iter_indices() {
                        let Some(fader) = previous.track_faders.get_mut(index) else {
                            return Err(SessionPlaybackError::DeviceRuntime);
                        };
                        fader.begin_mute(rate);
                        started_any = true;
                    }
                    started_any
                }
            };
            if !started {
                self.native_removal = None;
                return Err(SessionPlaybackError::DeviceRuntime);
            }
            pending.started_at = Some(started_at);
        }
        Ok(if pending.ready_on(previous) {
            NativeRemovalProgress::Ready
        } else {
            NativeRemovalProgress::Fading
        })
    }

    pub(crate) fn native_removal_started(&self) -> bool {
        self.native_removal
            .is_some_and(|pending| pending.started_at.is_some())
    }

    pub(crate) fn native_removal_pending(&self) -> bool {
        self.native_removal.is_some()
    }

    pub(crate) fn pending_hosted_track_effect_deadline(&self) -> Option<u64> {
        let pending = self.native_removal?;
        pending.hosted_effect_move()?;
        let started = pending.started_at?.get() - 1;
        let fade = u64::from(super::track_fader::TrackFader::fade_frames(
            self.revision.arrangement().sample_rate(),
        ));
        // Bound the fade and the old dry alignment drain, including callback
        // subdivision. Expiry restores both predecessor faders.
        Some(started.saturating_add(fade + 512 + 2 * 1024))
    }

    pub(crate) fn pending_foreign_main_pass(
        &self,
    ) -> Option<(hex_lattice_contract::ForeignProcessorBinding, u32)> {
        self.native_removal
            .filter(|pending| pending.started_at.is_some())?
            .main_pass_transition()?
            .foreign_main_pass()
    }

    pub(crate) fn stage_foreign_main_pass_delay(
        &mut self,
        binding: hex_lattice_contract::ForeignProcessorBinding,
        delay: hex_lattice::MainPassDelay,
    ) -> bool {
        if self.pending_foreign_main_pass() != Some((binding, delay.latency_frames())) {
            return false;
        }
        self.main_pass_delay = Some(delay);
        true
    }

    pub(crate) fn pending_foreign_return_pass(
        &self,
    ) -> Option<(
        hex_lattice::ProcessorInstanceIdentity,
        hex_lattice_contract::ForeignProcessorBinding,
    )> {
        let transition = self
            .native_removal
            .filter(|pending| pending.started_at.is_some())?
            .main_pass_transition()?;
        Some((transition.identity(), transition.foreign_return_pass()?))
    }

    pub(crate) fn stage_foreign_return_pass_delay(
        &mut self,
        previous: &Self,
        identity: hex_lattice::ProcessorInstanceIdentity,
        binding: hex_lattice_contract::ForeignProcessorBinding,
    ) -> bool {
        if self.pending_foreign_return_pass() != Some((identity, binding))
            || !self
                .native_removal
                .is_some_and(|pending| pending.matches(previous) && pending.ready_on(previous))
        {
            return false;
        }
        let Some(needed_floor) = previous
            .main_output_latency_frames()
            .checked_sub(self.revision.arrangement().main_latency_frames())
            .filter(|&frames| frames > 0)
        else {
            return false;
        };
        let Some(delay) = previous.committed_return_off_main_pass(identity.owner(), needed_floor)
        else {
            return false;
        };
        self.main_pass_delay = Some(delay);
        true
    }

    /// Return a pending removal and restore the original stage from its
    /// current wet/dry mix, including after a transport command invalidates it.
    pub fn cancel_native_removal_from(&mut self, previous: &mut Self) {
        let Some(pending) = self.native_removal.take() else {
            return;
        };
        if pending.main_pass_transition().is_some() {
            // A refused candidate must discard its uninstalled PCM snapshot.
            self.main_pass_delay = None;
        }
        if pending.started_at.is_some() && pending.runtime == previous.capture_runtime {
            match pending.transition {
                RemovalTransition::Move(movement) => {
                    let arrangement = previous.revision.arrangement();
                    for index in movement.indices() {
                        if let Some(track) = arrangement.tracks().get(index)
                            && let Some(fader) = previous.track_faders.get_mut(index)
                        {
                            fader.cancel_removal(
                                track.mix().channel_gain(),
                                arrangement.sample_rate(),
                            );
                        }
                    }
                }
                RemovalTransition::Device(transition) | RemovalTransition::MainPass(transition) => {
                    if let Some(chain) =
                        previous.native_removal_chain_mut(transition.identity().owner())
                    {
                        chain.cancel_native_removal(transition);
                    }
                }
                RemovalTransition::Return { id, .. }
                | RemovalTransition::ReturnPowerOn { id, .. } => {
                    if let Some(fade) = previous.return_fade.as_mut().filter(|fade| fade.id == id) {
                        fade.fader.cancel_removal(
                            [1.0; 2],
                            previous.revision.arrangement().sample_rate(),
                        );
                    }
                }
                RemovalTransition::Track(index)
                | RemovalTransition::Reorder(index)
                | RemovalTransition::Route {
                    outgoing: index, ..
                }
                | RemovalTransition::Instrument(index) => {
                    let arrangement = previous.revision.arrangement();
                    if let Some(track) = arrangement.tracks().get(index)
                        && let Some(fader) = previous.track_faders.get_mut(index)
                    {
                        fader.cancel_removal(track.mix().channel_gain(), arrangement.sample_rate());
                    }
                }
                RemovalTransition::Mute(mask) => {
                    let rate = previous.revision.arrangement().sample_rate();
                    for index in mask.iter_indices() {
                        if let Some(fader) = previous.track_faders.get_mut(index) {
                            fader.cancel_mute(rate);
                        }
                    }
                }
            }
        }
    }

    /// Stage a short pre-commit fade-out on predecessor tracks transitioning
    /// from unmuted to muted during live playback (including solo changes).
    /// Excludes shared-source contributors so receiver routes stay unaffected.
    pub(crate) fn plan_mute_fade_from(
        &mut self,
        previous: &Self,
    ) -> Result<bool, SessionPlaybackError> {
        if let Some(pending) = self.native_removal {
            if !pending.is_mute_fade() {
                return Ok(false);
            }
            return if pending.matches(previous) {
                Ok(true)
            } else {
                Err(SessionPlaybackError::InvalidCommand)
            };
        }
        if !previous.playing || self.live_restart || previous.return_fade.is_some() {
            return Ok(false);
        }
        let old = previous.revision.arrangement();
        let new = self.revision.arrangement();
        if old.sample_rate() != new.sample_rate()
            || old.main_latency_frames() != new.main_latency_frames()
            || old.tracks().len() != new.tracks().len()
            || old.tracks().len() > 256
            || previous.routes.len() != old.tracks().len()
            || self.routes.len() != new.tracks().len()
            || !old
                .tracks()
                .iter()
                .map(|track| track.canonical_track_id())
                .eq(new.tracks().iter().map(|track| track.canonical_track_id()))
            || !old
                .return_buses()
                .iter()
                .map(|bus| bus.id)
                .eq(new.return_buses().iter().map(|bus| bus.id))
            || previous.track_faders[..previous.routes.len()]
                .iter()
                .any(|fader| fader.is_removing())
        {
            return Ok(false);
        }
        let mut mask = MuteFadeMask::default();
        for (index, (old_track, new_track)) in old.tracks().iter().zip(new.tracks()).enumerate() {
            if !old_track.mix().muted()
                && new_track.mix().muted()
                && old_track.source_id.is_none()
                && new_track.source_id.is_none()
                && previous.revision.gain_recording_track() != old_track.canonical_track_id()
            {
                mask.set(index);
            }
        }
        if mask.is_empty() {
            return Ok(false);
        }
        self.native_removal = Some(PendingNativeRemoval {
            transition: RemovalTransition::Mute(mask),
            runtime: previous.capture_runtime,
            epoch: previous.capture_epoch,
            started_at: None,
        });
        Ok(true)
    }

    /// Plan and advance a short pre-commit fade-out on predecessor tracks that
    /// transition from unmuted to muted during live playback.
    pub fn advance_mute_fade_from(
        &mut self,
        previous: &mut Self,
    ) -> Result<NativeRemovalProgress, SessionPlaybackError> {
        if let Some(pending) = self.native_removal {
            if !pending.is_mute_fade() {
                return Ok(NativeRemovalProgress::NotApplicable);
            }
            return self.advance_planned_native_removal_from(previous);
        }
        if !self.plan_mute_fade_from(previous)? {
            return Ok(NativeRemovalProgress::NotApplicable);
        }
        self.advance_planned_native_removal_from(previous)
    }

    fn native_removal_ready(&self, transition: NativeRemovalTransition) -> bool {
        let chain = match transition.identity().owner() {
            ProcessorChainOwner::Track(id) => self
                .routes
                .iter()
                .position(|route| route.id.value() == id)
                .and_then(|index| self.device_chains.get(index)),
            ProcessorChainOwner::Return(id) => self
                .revision
                .arrangement()
                .return_buses()
                .iter()
                .position(|bus| bus.id.value() == id)
                .and_then(|index| self.return_chains.get(index)),
            ProcessorChainOwner::Main => Some(&self.main_chain),
            ProcessorChainOwner::Source(source) => self
                .source_effect_removal_receiver(source)
                .and_then(|receiver| self.source_effects.chain_for(receiver, source)),
        };
        chain.is_some_and(|chain| chain.native_removal_ready(transition))
    }

    fn native_removal_chain_mut(
        &mut self,
        owner: ProcessorChainOwner,
    ) -> Option<&mut NativeRuntimeChain> {
        match owner {
            ProcessorChainOwner::Track(id) => self
                .routes
                .iter()
                .position(|route| route.id.value() == id)
                .and_then(|index| self.device_chains.get_mut(index)),
            ProcessorChainOwner::Return(id) => self
                .revision
                .arrangement()
                .return_buses()
                .iter()
                .position(|bus| bus.id.value() == id)
                .and_then(|index| self.return_chains.get_mut(index)),
            ProcessorChainOwner::Main => Some(&mut self.main_chain),
            ProcessorChainOwner::Source(source) => {
                let receiver = self.source_effect_removal_receiver(source)?;
                self.source_effects.chain_for_mut(receiver, source)
            }
        }
    }

    fn source_effect_removal_receiver(&self, source: u64) -> Option<usize> {
        self.revision
            .arrangement()
            .tracks()
            .iter()
            .position(|track| {
                track.receives_source_audio()
                    && track.source_id().is_some_and(|id| id.value() == source)
            })
    }

    /// A direct audio track's post-fader output feeds Main and ordinary sends.
    /// One native zero-latency Delay can keep processing through that fade
    /// and retire with the donor without attaching a separate tail owner.
    fn direct_audio_track_removal_from(&self, previous: &Self) -> Option<usize> {
        let old = previous.revision.arrangement().tracks();
        let new = self.revision.arrangement().tracks();
        if old.len().checked_sub(1) != Some(new.len()) {
            return None;
        }
        let remaining = new.iter().map(|track| track.canonical_track_id());
        let mut new_ids = remaining.clone();
        let index = old
            .iter()
            .position(|track| new_ids.next() != Some(track.canonical_track_id()))?;
        if !old
            .iter()
            .enumerate()
            .filter_map(|(i, track)| (i != index).then_some(track.canonical_track_id()))
            .eq(remaining)
        {
            return None;
        }
        let track = old.get(index)?;
        let id = track.canonical_track_id()?;
        let route = previous.routes.get(index)?;
        let direct_audio = match route.owner {
            RouteOwner::Arrangement => !track.clips.is_empty(),
            RouteOwner::SessionAudio { clip, .. } => previous
                .revision
                .session()
                .audio_clip(clip)
                .is_some_and(|clip| clip.track == id),
            RouteOwner::Session { .. } | RouteOwner::Silence => false,
        };
        let can_retire_chain = match track.device_chain.as_ref() {
            [] => true,
            [device] => {
                matches!(route.owner, RouteOwner::Arrangement)
                    && route.pending.is_none()
                    && route.audio_release.is_none()
                    && self.compensation.is_none()
                    && previous.compensation.is_none()
                    && !self.revision.contains_streamed_sources()
                    && !previous.revision.contains_streamed_sources()
                    && device.device_type == hex_devices::delay::DEFINITION.type_id()
                    && device.foreign_processor().is_none()
                    && device.algorithmic_latency_frames() == Some(0)
            }
            _ => false,
        };
        (route.id == id
            && direct_audio
            && can_retire_chain
            && track.instrument_id.is_none()
            && track.source_id.is_none()
            && track.midi_source_index.is_none()
            && !track.has_gain_automation()
            && previous.revision.gain_recording_track() != Some(id)
            && self.direct_track_removal_keeps_timing(previous, id))
        .then_some(index)
    }

    fn direct_track_removal_keeps_timing(&self, previous: &Self, id: hex_project::TrackId) -> bool {
        match (&self.compensation, &previous.compensation) {
            (None, None) => true,
            (Some(next), Some(old)) => next.can_copy_from(
                old,
                Some(FreshHistory::RemovedTrack {
                    track: id,
                    staged: true,
                }),
            ),
            _ => false,
        }
    }

    /// Fade a flat Return after its native effects while the complete chain
    /// stays in the predecessor until the cut.
    fn flat_return_removal_from(&self, previous: &Self) -> Option<ReturnId> {
        if self.compensation.is_some()
            || previous.compensation.is_some()
            || self.revision.contains_streamed_sources()
            || previous.revision.contains_streamed_sources()
        {
            return None;
        }
        let old = previous.revision.arrangement();
        let new = self.revision.arrangement();
        if old.return_buses().len().checked_sub(1) != Some(new.return_buses().len())
            || !old
                .tracks()
                .iter()
                .map(|track| track.canonical_track_id())
                .eq(new.tracks().iter().map(|track| track.canonical_track_id()))
        {
            return None;
        }
        let mut remaining = new.return_buses().iter().map(|bus| bus.id);
        let (removed_index, removed) = old
            .return_buses()
            .iter()
            .enumerate()
            .find(|(_, bus)| remaining.next() != Some(bus.id))?;
        if !old
            .return_buses()
            .iter()
            .filter(|bus| bus.id != removed.id)
            .map(|bus| bus.id)
            .eq(new.return_buses().iter().map(|bus| bus.id))
            || !new
                .return_feed_plan
                .keeps_feeds_after_removal(&old.return_feed_plan, removed_index)
            || previous
                .return_fade
                .is_some_and(|fade| fade.id != removed.id)
            || previous
                .return_effect_tails
                .slots()
                .iter()
                .flatten()
                .any(|tail| tail.output_active && tail.return_id() == Some(removed.id))
        {
            return None;
        }
        // The compiled Return has at most MAX_DEVICES_PER_TRACK stages and
        // keeps the same prepared instances through the fade. A hosted device
        // stays outside this path even when its prepared state has no endpoint.
        removed
            .device_chain
            .iter()
            .all(|device| {
                device.device_type != hex_lattice_contract::VST3_DEVICE_TYPE_ID
                    && device.foreign_processor().is_none()
                    && device.algorithmic_latency_frames() == Some(0)
            })
            .then_some(removed.id)
    }

    /// Fade one workerless Off hosted Return and drain its alignment ring when
    /// the running Main pass already covers the enabled hosted Return's latency.
    pub(super) fn same_pass_hosted_return_power_on_from(
        &self,
        previous: &Self,
        cut_predecessor_pcm: bool,
    ) -> Option<(ReturnId, u64)> {
        let (next, old) = self
            .compensation
            .as_ref()
            .zip(previous.compensation.as_ref())?;
        let before = previous.revision.arrangement();
        let after = self.revision.arrangement();
        let ([old_bus], [new_bus]) = (before.return_buses(), after.return_buses()) else {
            return None;
        };
        let ([old_dev], [new_dev]) = (old_bus.device_chain.as_ref(), new_bus.device_chain.as_ref())
        else {
            return None;
        };
        let main_latency = before.main_latency_frames();
        if !previous.playing
            || main_latency == 0
            || main_latency != after.main_latency_frames()
            || self.revision.contains_streamed_sources()
            || previous.revision.contains_streamed_sources()
            || !before.main_device_chain.is_empty()
            || !after.main_device_chain.is_empty()
            || before.master_mix() != after.master_mix()
            || !before.return_feed_plan.is_empty()
            || !after.return_feed_plan.is_empty()
            || previous.main_pass_delay.is_some()
            || self.main_pass_delay.is_some()
            || previous.stopped_input.is_some()
            || self.stopped_input.is_some()
            || previous.live_gain.audible().is_some()
            || previous
                .return_fade
                .is_some_and(|fade| fade.id != old_bus.id)
            || previous.track_faders[..previous.routes.len()]
                .iter()
                .any(|fader| fader.active())
            || previous
                .return_effect_tails
                .slots()
                .iter()
                .flatten()
                .any(|tail| tail.output_active)
            || previous.tail_output_active.iter().any(|active| *active)
            || !previous.releases.is_empty()
            || previous.foreign_runtime_bindings().next().is_some()
            || old_bus.id != new_bus.id
            || old_bus.muted
            || new_bus.muted
            || old_bus.gain <= 0.0
            || old_bus.gain != new_bus.gain
            || old_bus.matrix != new_bus.matrix
            || old_dev.device_id != new_dev.device_id
            || old_dev.device_type != hex_lattice_contract::VST3_DEVICE_TYPE_ID
            || new_dev.device_type != hex_lattice_contract::VST3_DEVICE_TYPE_ID
            || !old_dev.bypassed
            || old_dev.foreign_processor().is_some()
            || old_dev.algorithmic_latency_frames() != Some(0)
            || new_dev.bypassed
            || new_dev.foreign_processor().is_none()
            || !new_dev
                .algorithmic_latency_frames()
                .is_some_and(|frames| frames > 0 && frames <= main_latency)
            || before.tracks().len() != after.tracks().len()
            || !(1..=2).contains(&before.tracks().len())
            || previous.routes.len() != before.tracks().len()
            || self.routes.len() != after.tracks().len()
            || previous.revision.gain_recording_track().is_some()
            || self.revision.gain_recording_track().is_some()
        {
            return None;
        }
        let mut fed_return = false;
        for (index, (left, right)) in before.tracks().iter().zip(after.tracks()).enumerate() {
            let direct_arrangement = |playback: &SessionPlayback, track: &crate::Track| {
                playback.routes.get(index).is_some_and(|route| {
                    matches!(route.owner, RouteOwner::Arrangement)
                        && route.pending.is_none()
                        && route.audio_release.is_none()
                }) && !playback.source_effects.has_chain(index)
                    && track.clips.len() == 1
                    && track.instrument_id.is_none()
                    && track.source_id.is_none()
                    && track.midi_source_index.is_none()
                    && !track.has_gain_automation()
            };
            let same_sends = left.sends().len() == right.sends().len()
                && left.sends().iter().zip(right.sends()).all(|(a, b)| {
                    a.return_index == b.return_index && a.gain.to_bits() == b.gain.to_bits()
                });
            let same_chain = left.device_chain.len() == right.device_chain.len()
                && left.device_chain.iter().all(|dev| {
                    dev.foreign_processor().is_none()
                        && dev.device_type != hex_lattice_contract::VST3_DEVICE_TYPE_ID
                })
                && self
                    .device_chains
                    .get(index)
                    .zip(previous.device_chains.get(index))
                    .is_some_and(|(next_chain, old_chain)| {
                        next_chain.len() == old_chain.len()
                            && next_chain.configuration_matches_range(old_chain, 0..old_chain.len())
                    });
            if left.canonical_track_id().is_none()
                || left.canonical_track_id() != right.canonical_track_id()
                || left.mix != right.mix
                || !direct_arrangement(previous, left)
                || !direct_arrangement(self, right)
                || !super::device_move::same_clip(&left.clips[0], &right.clips[0])
                || !same_sends
                || !same_chain
            {
                return None;
            }
            if let [send] = left.sends() {
                if send.return_index != 0
                    || send.gain <= 0.0
                    || !left.device_chain.is_empty()
                    || fed_return
                {
                    return None;
                }
                fed_return = true;
            } else if !left.sends().is_empty() {
                return None;
            }
        }
        if !fed_return {
            return None;
        }
        let history = FreshHistory::ChangedReturn {
            bus: old_bus.id,
            device: old_dev.device_id,
            cut_predecessor_pcm,
        };
        next.can_copy_from(old, Some(history))
            .then_some((old_bus.id, old_dev.device_id))
    }

    fn native_removal_transition_from(&self, previous: &Self) -> Option<NativeRemovalTransition> {
        let old = previous.revision.arrangement();
        let new = self.revision.arrangement();
        if !old
            .tracks()
            .iter()
            .map(|track| track.canonical_track_id())
            .eq(new.tracks().iter().map(|track| track.canonical_track_id()))
            || !old
                .return_buses()
                .iter()
                .map(|bus| bus.id)
                .eq(new.return_buses().iter().map(|bus| bus.id))
        {
            return None;
        }
        let mut removal = None;
        for (current, old) in self
            .device_chains
            .iter()
            .zip(&previous.device_chains)
            .chain(self.return_chains.iter().zip(&previous.return_chains))
            .chain(std::iter::once((&self.main_chain, &previous.main_chain)))
        {
            if current.len() != old.len() {
                let transition = current.native_removal_transition_from(old)?;
                if removal.replace(transition).is_some() {
                    return None;
                }
            }
        }
        let nested = self
            .source_effects
            .native_removal_transition_from(&previous.source_effects);
        match (removal, nested) {
            (Some(_), Some(_)) => None,
            (main, nested) => main.or(nested),
        }
    }
}
