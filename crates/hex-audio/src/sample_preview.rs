//! Independent one-shot browser PCM on the existing output stream.
//! One incoming owner, two retirement slots and one latest control-side pending
//! owner bound memory and callback work. Only control reclaims PCM.
use hex_engine::{Audio, Frame, Invalid};
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::sync::{
    Arc,
    atomic::{
        AtomicU64,
        Ordering::{Acquire, Release},
    },
};

struct Preview {
    generation: u64,
    audio: Audio,
    position: usize,
}

pub struct Control {
    incoming: Producer<Preview>,
    retired: Consumer<Preview>,
    pending: Option<Preview>,
    generation: Arc<AtomicU64>,
    rate: u32,
    last_generation: u64,
    closing: bool,
}

pub(crate) struct Callback {
    incoming: Consumer<Preview>,
    retired: Producer<Preview>,
    active: Option<Preview>,
    generation: Arc<AtomicU64>,
}

pub(crate) fn channel(rate: u32) -> (Control, Callback) {
    let (incoming, consumer) = RingBuffer::new(1);
    let (returning, retired) = RingBuffer::new(2);
    let generation = Arc::new(AtomicU64::new(0));
    (
        Control {
            incoming,
            retired,
            pending: None,
            generation: generation.clone(),
            rate,
            last_generation: 0,
            closing: false,
        },
        Callback {
            incoming: consumer,
            retired: returning,
            active: None,
            generation,
        },
    )
}

impl Control {
    /// Silence the previous request, including queued PCM, before decode starts.
    /// The returned token is scoped to this output instance.
    pub fn request(&mut self) -> Result<u64, Invalid> {
        if self.closing {
            return Err(Invalid("sample preview output is closing"));
        }
        let generation = self
            .last_generation
            .checked_add(1)
            .ok_or(Invalid("sample preview request counter exhausted"))?;
        self.last_generation = generation;
        self.generation.store(generation, Release);
        self.pending = None;
        self.reclaim();
        Ok(generation)
    }

    /// Admit immutable, validated PCM. Stale completions are disposed on control.
    pub fn publish(&mut self, generation: u64, audio: Audio) -> Result<(), Invalid> {
        if self.closing {
            return Err(Invalid("sample preview output is closing"));
        }
        if generation != self.generation.load(Acquire) {
            return Ok(());
        }
        if audio.sample_rate() != self.rate {
            return Err(Invalid("sample preview rate does not match audio output"));
        }
        self.pending = Some(Preview {
            generation,
            audio,
            position: 0,
        });
        self.reclaim();
        Ok(())
    }

    pub(crate) fn reclaim(&mut self) {
        for _ in 0..2 {
            let Ok(preview) = self.retired.pop() else {
                break;
            };
            drop(preview);
        }
        if let Some(preview) = self.pending.take()
            && let Err(PushError::Full(preview)) = self.incoming.push(preview)
        {
            self.pending = Some(preview);
        }
    }

    pub(crate) fn shutdown(&mut self) {
        self.cancel();
        self.closing = true;
    }

    /// Silence and invalidate playback without disturbing project transport.
    pub fn cancel(&mut self) {
        self.generation.store(0, Release);
        self.pending = None;
        self.reclaim();
    }
}

impl Callback {
    pub(crate) fn begin_block(&mut self, closing: bool) {
        let generation = self.generation.load(Acquire);
        let retire_active = self.active.as_ref().is_some_and(|preview| {
            closing
                || preview.generation != generation
                || preview.position == preview.audio.frames().len()
        });
        if retire_active
            && !self.retired.is_full()
            && let Some(preview) = self.active.take()
        {
            // This callback is the sole producer; the checked slot cannot vanish.
            if let Err(PushError::Full(preview)) = self.retired.push(preview) {
                self.active = Some(preview);
            }
        }
        if self.active.is_none()
            && !self.retired.is_full()
            && let Ok(preview) = self.incoming.pop()
        {
            self.active = Some(preview);
            if (closing
                || self
                    .active
                    .as_ref()
                    .is_some_and(|p| p.generation != generation))
                && let Some(preview) = self.active.take()
                && let Err(PushError::Full(preview)) = self.retired.push(preview)
            {
                self.active = Some(preview);
            }
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.active.is_none() && self.incoming.is_empty()
    }

    /// Unity preview is added after the project's master stage. It never moves
    /// project transport and is not captured by project-node spectrum taps.
    pub(crate) fn mix(&mut self, output: &mut [Frame]) {
        let Some(preview) = self.active.as_mut() else {
            return;
        };
        if preview.generation != self.generation.load(Acquire) {
            return;
        }
        let frames = preview.audio.frames();
        let count = output.len().min(frames.len() - preview.position);
        for (target, source) in output.iter_mut().zip(&frames[preview.position..][..count]) {
            target[0] += source[0];
            target[1] += source[1];
        }
        preview.position += count;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(value: f32, frames: usize) -> Audio {
        Audio::new(48_000, vec![[value, -value]; frames]).unwrap()
    }

    #[test]
    fn one_shot_restarts_newest_request_and_retires_finished_pcm() {
        let (mut control, mut callback) = channel(48_000);
        let first = control.request().unwrap();
        control.publish(first, pcm(0.25, 3)).unwrap();
        callback.begin_block(false);
        let mut output = [[0.1; 2]; 2];
        callback.mix(&mut output);
        assert_eq!(output, [[0.35, -0.15]; 2]);
        let newest = control.request().unwrap();
        output.fill([0.0; 2]);
        callback.mix(&mut output);
        assert_eq!(output, [[0.0; 2]; 2], "decode immediately silences old PCM");
        control.publish(first, pcm(0.9, 2)).unwrap();
        control.publish(newest, pcm(0.5, 3)).unwrap();
        callback.begin_block(false);
        callback.mix(&mut output);
        assert_eq!(output, [[0.5, -0.5]; 2]);
        output.fill([0.0; 2]);
        callback.mix(&mut output);
        assert_eq!(output, [[0.5, -0.5], [0.0; 2]]);
        callback.begin_block(false);
        assert!(callback.is_empty());
        control.reclaim();
        assert!(control.retired.is_empty());
    }

    #[test]
    fn saturated_retirement_preserves_pcm_until_control_reclaims_and_latest_wins() {
        let (mut control, mut callback) = channel(48_000);
        // Fill the real return queue, without allowing control reclamation.
        for value in [0.1, 0.2] {
            callback
                .retired
                .push(Preview {
                    generation: 0,
                    audio: pcm(value, 2),
                    position: 0,
                })
                .ok()
                .unwrap();
        }
        let old = control.request().unwrap();
        control.publish(old, pcm(0.3, 4)).unwrap();
        callback.begin_block(false);
        let next = control.request().unwrap();
        control.publish(next, pcm(0.4, 4)).unwrap();
        // Saturate again with callback-owned old PCM and no room to hand back.
        for value in [0.1, 0.2] {
            callback
                .retired
                .push(Preview {
                    generation: 0,
                    audio: pcm(value, 2),
                    position: 0,
                })
                .ok()
                .unwrap();
        }
        callback.begin_block(false);
        let mut output = [[0.0; 2]; 2];
        callback.mix(&mut output);
        assert_eq!(output, [[0.0; 2]; 2]);
        assert!(callback.active.is_some());
        control.reclaim();
        callback.begin_block(false);
        callback.mix(&mut output);
        assert_eq!(output, [[0.4, -0.4]; 2]);
        control.cancel();
        callback.begin_block(true);
        assert!(callback.is_empty());
        control.reclaim();
    }

    #[test]
    fn mismatched_rate_and_cancelled_completions_do_not_play() {
        let (mut control, mut callback) = channel(48_000);
        let token = control.request().unwrap();
        assert!(
            control
                .publish(token, Audio::new(44_100, vec![[0.2; 2]; 2]).unwrap())
                .is_err()
        );
        control.cancel();
        control.publish(token, pcm(0.5, 4)).unwrap();
        callback.begin_block(false);
        assert!(callback.is_empty());
        let later = control.request().unwrap();
        assert!(later > token);
    }

    #[test]
    fn preview_mixes_while_stopped_playing_paused_and_after_project_stop() {
        use hex_engine::{Clip, PlaybackCommand, Project, Track, playback::Playback};
        let audio = Audio::new(48_000, vec![[0.25; 2]]).unwrap();
        let clip = Clip::new(audio, 0, 100, true).unwrap();
        let track = Track::new(vec![clip], 1.0, false).unwrap();
        let project = Arc::new(Project::new(48_000, vec![track]).unwrap());
        let (mut control, mut callback) = crate::prepared::channel(48_000).unwrap();
        control.publish(Playback::from_shared(project)).unwrap();
        let token = control.sample_preview.request().unwrap();
        control.sample_preview.publish(token, pcm(0.5, 20)).unwrap();
        let mut output = [[0.0; 2]; 2];
        callback.begin_block();
        callback.render(&mut output).unwrap();
        assert_eq!(output, [[0.5, -0.5]; 2]);
        assert_eq!(control.status.position.load(Acquire), 0);
        assert!(!control.status.playing.load(Acquire));
        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        callback.render(&mut output).unwrap();
        assert_eq!(output, [[0.75, -0.25]; 2]);
        assert_eq!(control.status.position.load(Acquire), 2);
        control.command(PlaybackCommand::Pause).unwrap();
        callback.begin_block();
        callback.render(&mut output).unwrap();
        assert_eq!(output, [[0.5, -0.5]; 2]);
        assert_eq!(control.status.position.load(Acquire), 2);
        control.stop();
        callback.begin_block();
        callback.render(&mut output).unwrap();
        assert_eq!(output, [[0.5, -0.5]; 2]);
        assert_eq!(control.status.position.load(Acquire), 0);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn full_incoming_coalesces_pending_to_latest_and_shutdown_returns_both_owners() {
        let (mut control, mut callback) = channel(48_000);
        for value in [0.1, 0.2, 0.3] {
            let token = control.request().unwrap();
            control.publish(token, pcm(value, 2)).unwrap();
        }
        assert!(control.pending.is_some());
        callback.begin_block(false); // reject queued 0.1
        control.reclaim(); // admit only latest 0.3
        callback.begin_block(false);
        let mut output = [[0.0; 2]; 1];
        callback.mix(&mut output);
        assert_eq!(output, [[0.3, -0.3]]);
        let token = control.request().unwrap();
        control.publish(token, pcm(0.4, 2)).unwrap();
        control.cancel();
        callback.begin_block(true);
        assert!(callback.is_empty());
        assert_eq!(control.retired.slots(), 2);
        control.reclaim();
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn sample_replacement_completion_and_shutdown_allocate_and_free_nothing() {
        let (mut control, mut callback) = crate::prepared::channel(48_000).unwrap();
        let token = control.sample_preview.request().unwrap();
        control.sample_preview.publish(token, pcm(0.25, 3)).unwrap();
        let mut output = [[0.0; 2]; 2];
        let measure = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        assert_eq!(
            (
                measure.count_total,
                measure.bytes_total,
                measure.count_current,
                measure.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(output, [[0.25, -0.25]; 2]);
        let token = control.sample_preview.request().unwrap();
        control.sample_preview.publish(token, pcm(0.5, 1)).unwrap();
        let measure = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        assert_eq!(
            (
                measure.count_total,
                measure.bytes_total,
                measure.count_current,
                measure.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(output, [[0.0; 2]; 2]);
        control.reclaim();
        let token = control.sample_preview.request().unwrap();
        control.sample_preview.publish(token, pcm(0.5, 1)).unwrap();
        callback.begin_block();
        control.request_shutdown();
        assert!(control.sample_preview.request().is_err());
        let measure = allocation_counter::measure(|| callback.begin_block());
        assert_eq!(
            (
                measure.count_total,
                measure.bytes_total,
                measure.count_current,
                measure.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(control.status.shutdown_complete.load(Acquire));
        control.reclaim();
    }
}
