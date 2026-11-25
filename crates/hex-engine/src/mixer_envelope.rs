//! Control-prepared track mixer automation envelopes (balance/pan, mute, and
//! return sends). Queries are pure binary searches with zero callback allocation.
use crate::Invalid;
use hex_project::{AutomationInterpolation, MixerAutomationLane, TempoMap};

#[derive(Clone, Copy, Debug)]
struct PreparedMixerKnot {
    frame: u64,
    value: f64,
    outgoing: AutomationInterpolation,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedMixerLane {
    knots: Vec<PreparedMixerKnot>,
}

impl PreparedMixerLane {
    pub(crate) fn prepare(
        lane: &MixerAutomationLane,
        tempo: &TempoMap,
    ) -> Result<Option<Self>, Invalid> {
        let points = lane.points();
        if points.is_empty() {
            return Ok(None);
        }
        let tempo_points = tempo.points();
        let mut capacity = points.len();
        for pair in points.windows(2) {
            if pair[0].outgoing != AutomationInterpolation::Step {
                let start = tempo_points.partition_point(|m| m.beat() <= pair[0].beat);
                let end = tempo_points.partition_point(|m| m.beat() < pair[1].beat);
                capacity = capacity
                    .checked_add(end.saturating_sub(start))
                    .ok_or(Invalid("prepared mixer knot count overflow"))?;
            }
        }
        let mut knots = Vec::<PreparedMixerKnot>::new();
        knots
            .try_reserve_exact(capacity)
            .map_err(|_| Invalid("could not reserve prepared mixer lane"))?;
        let mut push = |beat, value, outgoing| -> Result<(), Invalid> {
            let frame = tempo.beat_to_frame(beat).map_err(|e| Invalid(e.0))?;
            let knot = PreparedMixerKnot {
                frame,
                value,
                outgoing,
            };
            if let Some(last) = knots.last_mut().filter(|k| k.frame == frame) {
                *last = knot;
            } else {
                knots.push(knot);
            }
            Ok(())
        };
        for (index, point) in points.iter().enumerate() {
            let next = points.get(index + 1);
            let markers = if point.outgoing != AutomationInterpolation::Step
                && let Some(next) = next
            {
                let start = tempo_points.partition_point(|m| m.beat() <= point.beat);
                let end = tempo_points.partition_point(|m| m.beat() < next.beat);
                &tempo_points[start..end]
            } else {
                &[]
            };
            let outgoing = if markers.is_empty() {
                point.outgoing
            } else {
                AutomationInterpolation::Linear
            };
            push(point.beat, f64::from(point.value), outgoing)?;
            if let Some(next) = next {
                for marker in markers {
                    let fraction = (marker.beat().raw() - point.beat.raw()) as f64
                        / (next.beat.raw() - point.beat.raw()) as f64;
                    let value = point.outgoing.interpolate(
                        f64::from(point.value),
                        f64::from(next.value),
                        fraction,
                    );
                    push(marker.beat(), value, AutomationInterpolation::Linear)?;
                }
            }
        }
        Ok(Some(Self { knots }))
    }

    #[inline]
    pub(crate) fn value_at(&self, frame: u64) -> f32 {
        let Some(first) = self.knots.first() else {
            return 0.0;
        };
        let next = self.knots.partition_point(|knot| knot.frame <= frame);
        if next == 0 {
            return first.value as f32;
        }
        let left = self.knots[next - 1];
        if frame == left.frame || left.outgoing == AutomationInterpolation::Step {
            return left.value as f32;
        }
        let Some(right) = self.knots.get(next) else {
            return left.value as f32;
        };
        let span = right.frame.saturating_sub(left.frame);
        if span == 0 {
            return left.value as f32;
        }
        let fraction = (frame - left.frame) as f64 / span as f64;
        left.outgoing.interpolate(left.value, right.value, fraction) as f32
    }
}

#[cfg(test)]
mod tests {
    use crate::{Audio, Renderer, compile};
    use hex_devices::utility::UTILITY_DEFINITION;
    use hex_lattice_contract::{DeviceAutomationTarget, NativeDeviceMetadata};
    use hex_project::{
        AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created,
        CurveTension, Editor, MixerAutomationPoint, MixerLaneTarget, Project,
    };
    use std::collections::BTreeMap;

    fn q(beats: u64) -> BeatTime {
        BeatTime::from_quarters(beats).unwrap()
    }

    #[test]
    fn mixer_pan_mute_and_send_lanes_render_deterministically() {
        let sr = 48_000;
        let mut editor = Editor::new(Project::new(sr).unwrap(), 32).unwrap();
        // 120 BPM => 1 quarter = 24_000 frames.
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "tone".into(),
                path: "tone.wav".into(),
                sample_rate: sr,
                frames: 96_000,
            })
            .unwrap()
        else {
            panic!("expected imported track");
        };
        let Created::ReturnTrack(ret) = editor
            .execute(Command::AddReturnTrack { name: "bus".into() })
            .unwrap()
        else {
            panic!("expected return");
        };

        // Pan sweeps from hard left (-1) at beat 0 to hard right (+1) at beat 2.
        editor
            .execute(Command::SetTrackMixerAutomation {
                track,
                target: MixerLaneTarget::Balance,
                points: vec![
                    MixerAutomationPoint {
                        beat: q(0),
                        value: -1.0,
                        outgoing: AutomationInterpolation::Linear,
                    },
                    MixerAutomationPoint {
                        beat: q(2),
                        value: 1.0,
                        outgoing: AutomationInterpolation::Step,
                    },
                ],
            })
            .unwrap();
        // Mute turns ON at beat 3.
        editor
            .execute(Command::SetTrackMixerAutomation {
                track,
                target: MixerLaneTarget::Mute,
                points: vec![
                    MixerAutomationPoint {
                        beat: q(0),
                        value: 0.0,
                        outgoing: AutomationInterpolation::Step,
                    },
                    MixerAutomationPoint {
                        beat: q(3),
                        value: 1.0,
                        outgoing: AutomationInterpolation::Step,
                    },
                ],
            })
            .unwrap();
        // Return send ramps from 0.0 at beat 0 to 1.0 at beat 2 (no static send set!).
        editor
            .execute(Command::SetTrackMixerAutomation {
                track,
                target: MixerLaneTarget::Send(ret),
                points: vec![
                    MixerAutomationPoint {
                        beat: q(0),
                        value: 0.0,
                        outgoing: AutomationInterpolation::Linear,
                    },
                    MixerAutomationPoint {
                        beat: q(2),
                        value: 1.0,
                        outgoing: AutomationInterpolation::Step,
                    },
                ],
            })
            .unwrap();

        let dc = Audio::new(sr, vec![[0.5, 0.5]; 96_000]).unwrap();
        let media = BTreeMap::from([(asset, dc)]);
        let compiled = compile::project(editor.project(), &media).unwrap();
        let mut renderer = Renderer::new(&compiled);
        renderer.play();
        let mut out = vec![[0.0_f32; 2]; 96_000];
        renderer.render(&mut out).unwrap();

        // At frame 0 (beat 0): balance = -1 (L=0.5, R=0.0), send = 0 => dry only [0.5, 0.0].
        assert!((out[0][0] - 0.5).abs() < 1e-4, "frame 0: {:?}", out[0]);
        assert!(out[0][1].abs() < 1e-4, "frame 0: {:?}", out[0]);

        // At frame 24_000 (beat 1): balance = 0 (L=0.5, R=0.5), send = 0.5 => dry + send = 0.75.
        assert!(
            (out[24_000][0] - 0.75).abs() < 1e-3,
            "beat 1: {:?}",
            out[24_000]
        );
        assert!(
            (out[24_000][1] - 0.75).abs() < 1e-3,
            "beat 1: {:?}",
            out[24_000]
        );

        // At frame 48_000 (beat 2): balance = +1 (L=0.0, R=0.5), send = 1.0 => dry + send = [0.0, 1.0].
        assert!(out[48_000][0].abs() < 1e-4, "beat 2: {:?}", out[48_000]);
        assert!(
            (out[48_000][1] - 1.0).abs() < 1e-3,
            "beat 2: {:?}",
            out[48_000]
        );

        // At frame 72_000 (beat 3): mute lane = 1.0 => silence [0.0, 0.0]!
        assert_eq!(out[72_000], [0.0, 0.0]);
        assert_eq!(out[80_000], [0.0, 0.0]);
    }

    #[test]
    fn curved_device_automation_diverges_from_linear_and_matches_oracle() {
        let sr = 48_000;
        let render_with = |interp: AutomationInterpolation| {
            let mut editor = Editor::new(Project::new(sr).unwrap(), 16).unwrap();
            let Created::Imported { track, asset, .. } = editor
                .execute(Command::ImportAudioTrack {
                    name: "src".into(),
                    path: "src.wav".into(),
                    sample_rate: sr,
                    frames: 50_000,
                })
                .unwrap()
            else {
                panic!("expected track");
            };
            let state = UTILITY_DEFINITION.default_state().unwrap();
            let Created::Device(device) = editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state,
                })
                .unwrap()
            else {
                panic!("expected device");
            };
            editor
                .execute(Command::ReplaceDeviceAutomationTarget {
                    device,
                    target: DeviceAutomationTarget::HostOutputGain,
                    points: vec![
                        AutomationPoint {
                            beat: q(0),
                            value: AutomationValue::new(0.0).unwrap(),
                            outgoing: interp,
                        },
                        AutomationPoint {
                            beat: q(2),
                            value: AutomationValue::new(1.0).unwrap(),
                            outgoing: AutomationInterpolation::Step,
                        },
                    ],
                })
                .unwrap();
            let dc = Audio::new(sr, vec![[0.25, 0.25]; 50_000]).unwrap();
            let media = BTreeMap::from([(asset, dc)]);
            let compiled = compile::project(editor.project(), &media).unwrap();
            let mut renderer = Renderer::new(&compiled);
            renderer.play();
            let mut out = vec![[0.0_f32; 2]; 50_000];
            renderer.render(&mut out).unwrap();
            out
        };

        let linear = render_with(AutomationInterpolation::Linear);
        let convex = render_with(AutomationInterpolation::Curve(
            CurveTension::from_millis(700).unwrap(),
        ));
        let concave = render_with(AutomationInterpolation::Curve(
            CurveTension::from_millis(-700).unwrap(),
        ));

        // Midpoint at frame 24_000 (beat 1): convex < linear < concave.
        assert!(convex[24_000][0] < linear[24_000][0] - 0.01);
        assert!(concave[24_000][0] > linear[24_000][0] + 0.01);
        // Endpoints at beat 0 (frame 0) and beat 2 (frame 48_000) agree!
        assert!((convex[0][0] - linear[0][0]).abs() < 1e-4);
        assert!((convex[48_000][0] - linear[48_000][0]).abs() < 1e-4);
        assert!((concave[48_000][0] - linear[48_000][0]).abs() < 1e-4);
    }

    #[test]
    fn frozen_track_plays_frozen_audio_bypasses_live_chain_and_unfreezes_cleanly() {
        let sr = 48_000;
        let mut editor = Editor::new(Project::new(sr).unwrap(), 16).unwrap();
        let Created::Imported {
            track,
            asset: live_asset,
            ..
        } = editor
            .execute(Command::ImportAudioTrack {
                name: "synth".into(),
                path: "live.wav".into(),
                sample_rate: sr,
                frames: 4_096,
            })
            .unwrap()
        else {
            panic!("expected track");
        };
        // Set track gain to 0.5 — must remain LIVE when frozen!
        editor.execute(Command::Gain { track, gain: 0.5 }).unwrap();

        let live_audio = Audio::new(sr, vec![[0.2, 0.2]; 4_096]).unwrap();
        let frozen_audio = Audio::new(sr, vec![[0.8, 0.4]; 4_096]).unwrap();

        // Freeze the track with the verified rendered asset.
        let Created::Asset(frozen_asset) = editor
            .execute(Command::FreezeTrack {
                track,
                path: "frozen/synth.wav".into(),
                start: 0,
                duration: 4_096,
                frames: 4_096,
            })
            .unwrap()
        else {
            panic!("expected frozen asset");
        };

        let media = BTreeMap::from([
            (live_asset, live_audio.clone()),
            (frozen_asset, frozen_audio),
        ]);
        let compiled_frozen = compile::project(editor.project(), &media).unwrap();
        let mut r_frozen = Renderer::new(&compiled_frozen);
        r_frozen.play();
        let mut out_frozen = vec![[0.0_f32; 2]; 4_096];
        r_frozen.render(&mut out_frozen).unwrap();
        // Frozen audio [0.8, 0.4] * live track gain 0.5 == [0.4, 0.2]!
        assert!((out_frozen[100][0] - 0.4).abs() < 1e-5);
        assert!((out_frozen[100][1] - 0.2).abs() < 1e-5);

        // Unfreeze restores live audio [0.2, 0.2] * 0.5 == [0.1, 0.1]!
        editor.execute(Command::UnfreezeTrack { track }).unwrap();
        let compiled_live = compile::project(editor.project(), &media).unwrap();
        let mut r_live = Renderer::new(&compiled_live);
        r_live.play();
        let mut out_live = vec![[0.0_f32; 2]; 4_096];
        r_live.render(&mut out_live).unwrap();
        assert!((out_live[100][0] - 0.1).abs() < 1e-5);
        assert!((out_live[100][1] - 0.1).abs() < 1e-5);
    }
}
