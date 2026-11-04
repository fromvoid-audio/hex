//! One audio-owned transient fader; this does not record or acknowledge delivery.
use super::*;
use crate::Invalid;

#[derive(Clone, Copy, Debug, PartialEq)]
enum GainMode {
    Read,
    Touch,
    Rejoin(Option<crate::gain_envelope::TouchTarget>),
}

/// One armed fader, copied before rendering so a failed block cannot advance it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct LiveGain {
    mode: GainMode,
    current: f32,
    from: f32,
    target: f32,
    elapsed: u32,
    duration: u32,
}

impl Default for LiveGain {
    fn default() -> Self {
        Self {
            mode: GainMode::Read,
            current: 0.0,
            from: 0.0,
            target: 0.0,
            elapsed: 0,
            duration: 1,
        }
    }
}

impl LiveGain {
    pub(super) fn audible(&self) -> Option<f32> {
        (!matches!(self.mode, GainMode::Read)).then_some(self.current)
    }
    fn retarget(&mut self, target: f32, force: bool, rate: u32) {
        if force || self.target != target {
            self.from = self.current;
            self.target = target;
            self.elapsed = 0;
            self.duration = rate.div_ceil(200).max(1);
        }
    }

    pub(super) fn next(
        &mut self,
        track: &crate::Track,
        frame: u64,
        desired: Option<f32>,
        rate: u32,
    ) -> Option<f32> {
        if matches!(self.mode, GainMode::Read) && desired.is_none() {
            return None;
        }
        let read = track.read_gain_at(frame)?;
        match (self.mode, desired) {
            (GainMode::Read, None) => return None,
            (GainMode::Read, Some(target)) => {
                self.current = track.read_gain_at(frame.saturating_sub(1)).unwrap_or(read);
                self.retarget(target, true, rate);
                self.mode = GainMode::Touch;
            }
            (_, Some(target)) => {
                self.retarget(target, !matches!(self.mode, GainMode::Touch), rate);
                self.mode = GainMode::Touch;
            }
            (mode, None) => {
                let event = track.gain_touch_target_at(frame);
                let previous = match mode {
                    GainMode::Rejoin(previous) => previous,
                    _ => None,
                };
                let changed = !matches!(mode, GainMode::Rejoin(_)) || event != previous;
                if changed {
                    let target = event
                        .map(|event| event.gain)
                        .or_else(|| {
                            track.base_gain_at(
                                frame.saturating_add(u64::from(rate.div_ceil(200).max(1) - 1)),
                            )
                        })
                        .unwrap_or(read);
                    let force = event.map(|event| event.span) != previous.map(|event| event.span);
                    self.retarget(target, force || !matches!(mode, GainMode::Rejoin(_)), rate);
                    self.mode = GainMode::Rejoin(event);
                }
            }
        }
        self.elapsed = self.elapsed.saturating_add(1).min(self.duration);
        self.current = if self.elapsed == self.duration {
            self.target
        } else {
            (f64::from(self.from)
                + (f64::from(self.target) - f64::from(self.from)) * f64::from(self.elapsed)
                    / f64::from(self.duration)) as f32
        };
        if matches!(self.mode, GainMode::Rejoin(_))
            && self.elapsed == self.duration
            && self.current == read
        {
            self.mode = GainMode::Read;
        }
        Some(self.current)
    }
}
pub(super) fn effective_mix(
    track: &crate::Track,
    frame: u64,
    armed: Option<TrackId>,
    override_gain: Option<f32>,
) -> crate::PreparedTrackMix {
    match override_gain.filter(|_| armed.is_some() && armed == track.canonical_track_id()) {
        Some(gain) => track.recording_mix(gain),
        None => track.mix_at(frame),
    }
}

/// Snapshot the committed audible fader when a current track becomes detached.
pub(super) fn detached_mix(
    playback: &SessionPlayback,
    track: &crate::Track,
    index: usize,
    frame: u64,
) -> crate::PreparedTrackMix {
    if playback.revision.gain_recording_track() == track.canonical_track_id()
        && let Some(gain) = playback.live_gain.audible()
    {
        return track.recording_mix(gain);
    }
    let mix = effective_mix(
        track,
        frame,
        playback.revision.gain_recording_track(),
        playback.gain_recording_override,
    );
    let recording = playback.gain_recording_override.is_some()
        && playback.revision.gain_recording_track() == track.canonical_track_id();
    if !track.has_gain_automation() && !recording && playback.track_faders[index].active() {
        mix.with_channel_gain(playback.track_faders[index].current())
    } else {
        mix
    }
}
impl SessionPlayback {
    pub(crate) fn gain_recording_ready(&self, track: u64) -> bool {
        self.gain_recording_clock(track).is_some()
    }
    pub(crate) fn set_gain_recording_override(
        &mut self,
        track: u64,
        gain: f32,
    ) -> Result<(), Invalid> {
        if !self.gain_recording_ready(track) {
            return Err(Invalid("gain recording target is not armed"));
        }
        if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
            return Err(Invalid(
                "gain recording override must be finite and within 0..2",
            ));
        }
        if self.gain_recording_override == Some(gain) {
            return Ok(());
        }
        self.gain_recording_override = Some(gain);
        Ok(())
    }
    pub(crate) fn clear_gain_recording_override(&mut self) {
        self.gain_recording_override = None;
        self.live_gain = LiveGain::default();
        self.live_gain_scratch = LiveGain::default();
    }
    pub(crate) fn release_gain_recording_override(&mut self) {
        self.gain_recording_override = None;
    }
    pub(crate) fn gain_recording_clock(&self, track: u64) -> Option<(u64, u64, u64)> {
        if self.linear_arrangement_export
            || self
                .revision
                .gain_recording_track()
                .is_none_or(|id| id.value() != track)
        {
            return None;
        }
        // The graph's summing arrival excludes serial Main processors. Receipt
        // delivery must wait through the complete final output path instead.
        let final_arrival = self.revision.arrangement().main_latency_frames();
        if let Some(compensation) = &self.compensation {
            let target = compensation
                .graph
                .tracks
                .iter()
                .find(|plan| plan.id.is_some_and(|id| id.value() == track))?;
            let arrival = compensation
                .graph
                .plan
                .nodes()
                .get(target.fader)?
                .output_arrival_frames;
            let delay = final_arrival.checked_sub(arrival)?;
            let (frame, pass) = if arrival == 0 {
                (self.arrangement_frame, self.arrangement_loop_pass)
            } else {
                let origin = compensation.delays.stamp_at_delay(arrival - 1)?;
                (origin.arrangement_frame, origin.loop_pass)
            };
            Some((frame, u64::from(delay), pass))
        } else {
            // A Main-only delay does not require upstream compensation rings.
            Some((
                self.arrangement_frame,
                u64::from(final_arrival),
                self.arrangement_loop_pass,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Project};
    use std::collections::BTreeMap;
    fn armed() -> (SessionPlayback, TrackId) {
        let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: "record".into(),
                path: "memory.wav".into(),
                source_sample_rate: 48000,
                source_frames: 1024,
                playback_frames: 1024,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })
            .unwrap()
        else {
            panic!("import")
        };
        editor.execute(Command::Gain { track, gain: 0.0 }).unwrap();
        editor
            .execute(Command::Balance {
                track,
                balance: 0.5,
            })
            .unwrap();
        let Created::ReturnTrack(bus) = editor
            .execute(Command::AddReturnTrack { name: "bus".into() })
            .unwrap()
        else {
            panic!("return")
        };
        editor
            .execute(Command::SetReturnSend {
                track,
                return_track: bus,
                gain: 0.5,
            })
            .unwrap();
        let revision = Arc::new(
            PreparedRevision::prepare_for_gain_recording(
                editor.project(),
                &BTreeMap::from([(
                    asset,
                    crate::Audio::new(48000, vec![[0.25, -0.5]; 1024]).unwrap(),
                )]),
                Arc::new(crate::SineTable::new()),
                None,
                track,
            )
            .unwrap(),
        );
        let mut runtime = SessionPlayback::new(revision).unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        (runtime, track)
    }
    #[test]
    fn gain_recording_override_is_once_before_capture_sends_and_master() {
        let (mut runtime, track) = armed();
        runtime
            .set_gain_recording_override(track.value(), 0.8)
            .unwrap();
        let bus = runtime.revision.arrangement().return_buses()[0].id;
        let plan = runtime
            .output_capture_plan(
                &[OutputCaptureId::Track(track), OutputCaptureId::Return(bus)],
                8,
            )
            .unwrap();
        let mut main = [[0.0; 2]; 8];
        let mut track = main;
        let mut returned = main;
        let mut buffers = [&mut track[..], &mut returned[..]];
        let mut capture = OutputCapture::new(&plan, &mut buffers);
        runtime
            .render_with_capture(&mut main, &mut capture)
            .unwrap();
        for (index, frame) in main.iter().enumerate() {
            let progress = (index + 1) as f32 / 240.0;
            let track = capture.output(0).unwrap()[index];
            let returned = capture.output(1).unwrap()[index];
            for (channel, target) in [0.1, -0.4].into_iter().enumerate() {
                assert!((track[channel] - target * progress).abs() < 1e-6);
                assert!((returned[channel] - target * progress * 0.5).abs() < 1e-6);
                assert!((frame[channel] - (track[channel] + returned[channel])).abs() < 1e-6);
            }
        }
    }
    #[test]
    fn group_gain_override_changes_only_its_submix_path() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: "source".into(),
                path: "memory.wav".into(),
                source_sample_rate: 48_000,
                source_frames: 16,
                playback_frames: 16,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })
            .unwrap()
        else {
            panic!("import")
        };
        let Created::Track(group) = editor
            .execute(Command::GroupTracks {
                tracks: hex_project::TrackSelectionIds::new(&[track]).unwrap(),
                name: "group".into(),
            })
            .unwrap()
        else {
            panic!("group")
        };
        let Created::ReturnTrack(bus) = editor
            .execute(Command::AddReturnTrack { name: "bus".into() })
            .unwrap()
        else {
            panic!("return")
        };
        editor
            .execute(Command::SetReturnSend {
                track,
                return_track: bus,
                gain: 0.5,
            })
            .unwrap();
        let revision = Arc::new(
            PreparedRevision::prepare_for_gain_recording(
                editor.project(),
                &BTreeMap::from([(
                    asset,
                    crate::Audio::new(48_000, vec![[0.25, -0.5]; 16]).unwrap(),
                )]),
                Arc::new(crate::SineTable::new()),
                None,
                group,
            )
            .unwrap(),
        );
        let mut runtime = SessionPlayback::new(revision).unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime
            .set_gain_recording_override(group.value(), 0.5)
            .unwrap();
        let mut output = [[0.0; 2]; 8];
        runtime.render(&mut output).unwrap();
        // The submix ramps while the independent send remains unchanged.
        for (index, frame) in output.iter().enumerate() {
            let gain = 1.0 - 0.5 * (index + 1) as f32 / 240.0;
            assert!((frame[0] - (0.25 * gain + 0.125)).abs() < 1e-6);
            assert!((frame[1] - (-0.5 * gain - 0.25)).abs() < 1e-6);
        }
        assert_eq!(runtime.gain_recording_clock(group.value()), Some((8, 0, 0)));
    }
    #[test]
    fn gain_recording_fault_rolls_back_pcm_clock_and_stop_clears_override() {
        let (mut runtime, track) = armed();
        let (mut oracle, oracle_track) = armed();
        runtime
            .set_gain_recording_override(track.value(), 0.8)
            .unwrap();
        oracle
            .set_gain_recording_override(oracle_track.value(), 0.8)
            .unwrap();
        runtime.render(&mut [[0.0; 2]; 8]).unwrap();
        oracle.render(&mut [[0.0; 2]; 8]).unwrap();
        let committed = runtime.live_gain;
        let frame = runtime.arrangement_frame;
        runtime.arrangement_frame = u64::MAX - 300;
        let mut output = [[9.0; 2]; 512];
        assert!(runtime.render(&mut output).is_err());
        assert_eq!(output, [[0.0; 2]; 512]);
        assert_eq!(runtime.performance_frame, 8);
        assert_eq!(runtime.arrangement_frame, u64::MAX - 300);
        assert_eq!(runtime.live_gain, committed);
        runtime.arrangement_frame = frame;
        let mut actual = [[0.0; 2]; 16];
        let mut expected = actual;
        runtime.render(&mut actual).unwrap();
        oracle.render(&mut expected).unwrap();
        assert_eq!(actual, expected);
        // It is an already accepted control command. Native failure cancellation
        // must explicitly terminate the take; a failed render invents no receipt.
        assert_eq!(runtime.gain_recording_override, Some(0.8));
        runtime.apply(SessionPlaybackCommand::Stop).unwrap();
        assert_eq!(runtime.gain_recording_override, None);
        assert_eq!(runtime.gain_recording_clock(track.value()), Some((0, 0, 0)));
    }

    #[test]
    fn detached_mix_freezes_the_audible_recording_level() {
        let (mut runtime, track) = armed();
        runtime
            .set_gain_recording_override(track.value(), 0.8)
            .unwrap();
        runtime.render(&mut [[0.0; 2]; 16]).unwrap();
        let source = &runtime.revision.arrangement().tracks()[0];
        let actual = detached_mix(&runtime, source, 0, runtime.arrangement_frame);
        let gain = 0.8 * 16.0 / 240.0;
        let expected = source.recording_mix(gain);
        for (actual, expected) in actual
            .channel_gain()
            .into_iter()
            .zip(expected.channel_gain())
        {
            assert!((actual - expected).abs() < 1e-6);
        }
    }
}
