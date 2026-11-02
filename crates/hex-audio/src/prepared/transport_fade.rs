//! Host effects of a declicked queued or streamed Pause/Stop (Pulse #4151)
//! and a queued resident Seek (Pulse #4140).
//!
//! `hex_engine::Playback` owns the fade itself: a Pause/Stop/Seek admitted
//! while playing is reported Applied when consumed and takes effect F frames
//! later; a Seek then ramps in at its target. Streamed seeks still jump
//! immediately when their target pages are ready.
//! This module owns only the callback effects that must follow the audible
//! end rather than the command: the output transport revocation and transport
//! epoch advance, the sampler keyboard revocation for Stop, and, for streamed
//! transports, the active page roles, which stay requested through the fade.
//! MIDI and gain takes and native parameter gestures are still aborted when
//! the command is consumed.
//!
//! Priority, fault, handoff and closing Stops stay immediate, as does a
//! streamed Pause/Stop while the stream is buffering silence. A pending fade
//! settles before the next command or priority Stop applies, after the render
//! that completes it, and is dropped when the fault Stop or a replacement graph
//! takes over its transport transitions.
use super::*;

/// A consumed Pause/Stop whose fade is still rendering.
pub(super) struct PendingFade {
    ticket: u64,
    revision: u64,
    stop: bool,
    stream: bool,
}

impl Callback {
    pub(super) fn begin_transport_fade(&mut self, ticket: u64, stop: bool, stream: bool) {
        self.transport_fade = Some(PendingFade {
            ticket,
            revision: self.status.applied_revision.load(Relaxed),
            stop,
            stream,
        });
    }

    pub(super) fn transport_fade_pending(&mut self) -> bool {
        self.active
            .as_mut()
            .and_then(ActivePublication::current_mut)
            .is_some_and(|active| active.playback.pending_transport_fade().is_some())
    }

    /// Complete a pending fade now, then apply its deferred effects. A refusal
    /// at the fade end is published under the command's ticket.
    pub(super) fn settle_transport_fade(&mut self) {
        let Some(fade) = self.transport_fade.take() else {
            return;
        };
        if fade.revision != self.status.applied_revision.load(Relaxed) {
            // A replacement graph owns its own transport transitions.
            return;
        }
        let failure = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
            .and_then(|mut active| {
                active.playback.finish_transport_fade();
                active.playback.take_transport_fade_failure()
            });
        if let Some(reason) = failure {
            self.status.publish_rejected_action(fade.ticket, reason);
        }
        if fade.stop {
            self.revoke_current_sampler_keyboard();
        }
        if fade.stream {
            self.clear_active_stream_roles();
        }
        self.invalidate_gain_recording(crate::gain_recording::AbortReason::TransportChanged);
    }
}

#[cfg(test)]
mod tests {
    use crate::prepared::{self, Control};
    use hex_engine::{
        Audio, Clip, Frame, Project, Track,
        playback::{Playback, PlaybackCommand},
    };
    use std::sync::atomic::Ordering::Acquire;

    const F: u64 = 128;

    fn dc_callback() -> (Control, prepared::Callback) {
        let audio = Audio::new(48_000, vec![[1.0, -1.0]; 8192]).unwrap();
        let clip = Clip::new(audio, 0, 8192, false).unwrap();
        let track = Track::new(vec![clip], 1.0, false).unwrap();
        let (mut control, callback) = prepared::channel(48_000).unwrap();
        control
            .publish(Playback::new(Project::new(48_000, vec![track]).unwrap()))
            .unwrap();
        (control, callback)
    }

    fn render(callback: &mut prepared::Callback, frames: usize, block: usize) -> Vec<Frame> {
        let mut output = Vec::with_capacity(frames);
        let mut chunk = vec![[0.0; 2]; block];
        while output.len() < frames {
            let len = block.min(frames - output.len());
            callback.begin_block();
            callback.render(&mut chunk[..len]).unwrap();
            output.extend_from_slice(&chunk[..len]);
        }
        output
    }

    fn largest_step(output: &[Frame]) -> f32 {
        output
            .windows(2)
            .map(|pair| (pair[1][0] - pair[0][0]).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn queued_pause_and_stop_fade_a_dc_clip_for_every_block_size() {
        let position = |control: &Control| control.status.position.load(Acquire);
        for block in [1, 64, 100, 512] {
            let (mut control, mut callback) = dc_callback();
            control.command(PlaybackCommand::Play).unwrap();
            let mut output = render(&mut callback, 1000, block);
            control.command(PlaybackCommand::Pause).unwrap();
            output.extend(render(&mut callback, 1000, block));
            assert!(!control.status.playing.load(Acquire), "block {block}");
            assert_eq!(position(&control), 1000 + F, "block {block}: audible end");
            control.command(PlaybackCommand::Play).unwrap();
            output.extend(render(&mut callback, 1000, block));
            assert_eq!(position(&control), 2000 + F, "block {block}");
            control.command(PlaybackCommand::Stop).unwrap();
            output.extend(render(&mut callback, 1000, block));
            assert!(!control.status.playing.load(Acquire), "block {block}");
            assert_eq!(position(&control), 0, "block {block}: Stop still rewinds");

            // Frame zero keeps its transient; every later step is one ramp
            // step of a unit DC clip.
            assert_eq!(output[999], [1.0, -1.0], "block {block}");
            let step = largest_step(&output[1..]);
            assert!(step <= 1.0 / F as f32 + 1e-6, "block {block}: step {step}");
            let fade_end = 1000 + F as usize;
            assert_eq!(output[fade_end - 1], [0.0; 2], "block {block}");
            assert!(output[fade_end..2000].iter().all(|f| *f == [0.0; 2]));
            assert_eq!(output[2000][0], 1.0 / F as f32, "block {block}: ramp in");
            assert!(output[3000 + F as usize..].iter().all(|f| *f == [0.0; 2]));
            control.request_shutdown();
            callback.begin_block();
        }
    }

    #[test]
    fn a_queued_seek_while_playing_fades_out_jumps_and_ramps_in() {
        let position = |control: &Control| control.status.position.load(Acquire);
        let mut reference = None;
        for block in [1, 64, 100, 512] {
            let (mut control, mut callback) = dc_callback();
            control.command(PlaybackCommand::Play).unwrap();
            let mut output = render(&mut callback, 1000, block);
            control.command(PlaybackCommand::Seek(5000)).unwrap();
            output.extend(render(&mut callback, 1000, block));
            assert!(control.status.playing.load(Acquire), "block {block}");
            assert_eq!(
                position(&control),
                5000 + 1000 - F,
                "block {block}: the seek target follows the fade"
            );
            let step = largest_step(&output[1..]);
            assert!(step <= 1.0 / F as f32 + 1e-6, "block {block}: step {step}");
            let fade_end = 1000 + F as usize;
            assert_eq!(output[fade_end - 1], [0.0; 2], "block {block}");
            assert_eq!(output[fade_end][0], 1.0 / F as f32, "block {block}");
            match &reference {
                None => reference = Some(output),
                Some(reference) => assert_eq!(&output, reference, "block {block}"),
            }
            control.request_shutdown();
            callback.begin_block();
        }
    }

    #[test]
    fn a_full_queue_still_stops_at_once_through_the_priority_stop() {
        let (mut control, mut callback) = dc_callback();
        control.command(PlaybackCommand::Play).unwrap();
        render(&mut callback, 256, 256);
        let mut admitted = 0;
        while control.command(PlaybackCommand::Pause).is_ok() {
            admitted += 1;
            assert!(admitted <= super::super::COMMANDS, "the queue is bounded");
        }
        control.stop();
        let output = render(&mut callback, 64, 64);
        assert!(!control.status.playing.load(Acquire));
        assert_eq!(control.status.position.load(Acquire), 0);
        assert!(
            output.iter().all(|f| *f == [0.0; 2]),
            "priority Stop stays an immediate cut"
        );
        control.request_shutdown();
        callback.begin_block();
    }
}
