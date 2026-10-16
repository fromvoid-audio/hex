//! One audio-owned transient fader; this does not record or acknowledge delivery.
use super::*;
use crate::Invalid;
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
        self.gain_recording_override = Some(gain);
        Ok(())
    }
    pub(crate) fn clear_gain_recording_override(&mut self) {
        self.gain_recording_override = None;
    }
    pub(crate) fn gain_recording_clock(&self, track: u64) -> Option<(u64, u64, u64)> {
        if self.linear_arrangement_export
            || !self
                .revision
                .gain_recording_track()
                .is_some_and(|id| id.value() == track)
        {
            return None;
        }
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
            let delay = compensation.graph.main_latency().checked_sub(arrival)?;
            let (frame, pass) = if arrival == 0 {
                (self.arrangement_frame, self.arrangement_loop_pass)
            } else {
                let origin = compensation.delays.stamp_at_delay(arrival - 1)?;
                (origin.arrangement_frame, origin.loop_pass)
            };
            Some((frame, u64::from(delay), pass))
        } else {
            Some((self.arrangement_frame, 0, self.arrangement_loop_pass))
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
        assert_eq!(capture.output(0).unwrap(), [[0.1, -0.4]; 8]);
        assert_eq!(capture.output(1).unwrap(), [[0.05, -0.2]; 8]);
        assert_eq!(main, [[0.1 + 0.05, -0.4 - 0.2]; 8]);
    }
    #[test]
    fn gain_recording_fault_rolls_back_pcm_clock_and_stop_clears_override() {
        let (mut runtime, track) = armed();
        runtime
            .set_gain_recording_override(track.value(), 0.8)
            .unwrap();
        runtime.arrangement_frame = u64::MAX - 300;
        let mut output = [[9.0; 2]; 512];
        assert!(runtime.render(&mut output).is_err());
        assert_eq!(output, [[0.0; 2]; 512]);
        assert_eq!(runtime.performance_frame, 0);
        assert_eq!(runtime.arrangement_frame, u64::MAX - 300);
        // It is an already accepted control command. Native failure cancellation
        // must explicitly terminate the take; a failed render invents no receipt.
        assert_eq!(runtime.gain_recording_override, Some(0.8));
        runtime.apply(SessionPlaybackCommand::Stop).unwrap();
        assert_eq!(runtime.gain_recording_override, None);
        assert_eq!(runtime.gain_recording_clock(track.value()), Some((0, 0, 0)));
    }
}
