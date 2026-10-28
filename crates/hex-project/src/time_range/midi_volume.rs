//! Frame-anchored MIDI-volume clipboard metadata. Split at both source and
//! destination tempo changes so frame-based range operations preserve ramps.
use crate::{
    BeatTime, Invalid, MAX_MIDI_VOLUME_POINTS_PER_CLIP, MidiClip, MidiClipGain, MidiClipVolume,
    MidiClipVolumePoint, TempoMap,
};
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
struct FramePoint {
    frame: u64,
    incoming: MidiClipGain,
    value: MidiClipGain,
}
#[derive(Clone, Debug, Default)]
pub(super) struct FrameVolume {
    points: Vec<FramePoint>,
    clip_gain: crate::ClipGain,
}
fn boundary(points: &mut BTreeSet<u64>, value: u64) -> Result<(), Invalid> {
    if !points.contains(&value) && points.len() == MAX_MIDI_VOLUME_POINTS_PER_CLIP {
        return Err(Invalid("time range clip volume point limit reached"));
    }
    points.insert(value);
    Ok(())
}
impl FrameVolume {
    pub(super) fn capture(
        clip: &MidiClip,
        tempo: &TempoMap,
        start_frame: u64,
        end_frame: u64,
    ) -> Result<Self, Invalid> {
        if clip.volume().points().is_empty() {
            return Ok(Self {
                clip_gain: clip.volume().clip_gain(),
                ..Self::default()
            });
        }
        let clip_end = BeatTime::from_raw(
            clip.start()
                .raw()
                .checked_add(clip.length().raw())
                .ok_or(Invalid("midi clip volume end overflow"))?,
        );
        // A raw-Q32 clip edge can round to the same frame as a slightly
        // earlier beat. Preserve the authored edge when cutting at its exact
        // frame instead of rejecting that valid full-clip range as negative.
        let start = if start_frame == tempo.beat_to_frame(clip.start())? {
            clip.start()
        } else {
            tempo.frame_to_beat(start_frame)?
        };
        let end = if end_frame == tempo.beat_to_frame(clip_end)? {
            clip_end
        } else {
            tempo.frame_to_beat(end_frame)?
        };
        let elapsed_start = start
            .raw()
            .checked_sub(clip.start().raw())
            .ok_or(Invalid("volume range starts before midi clip"))?;
        let elapsed_end = end
            .raw()
            .checked_sub(clip.start().raw())
            .ok_or(Invalid("volume range ends before midi clip"))?;
        let volume = clip.volume().materialized(
            clip.source_playback()?,
            BeatTime::from_raw(elapsed_start),
            BeatTime::from_raw(elapsed_end),
        )?;
        let mut beats = BTreeSet::new();
        for point in volume.points() {
            boundary(&mut beats, point.beat.raw())?;
        }
        for point in tempo.points() {
            if start < point.beat() && point.beat() < end {
                boundary(&mut beats, point.beat().raw() - start.raw())?;
            }
        }
        let mut points: Vec<FramePoint> = Vec::new();
        points
            .try_reserve_exact(beats.len())
            .map_err(|_| Invalid("could not reserve frame volume points"))?;
        for raw in beats {
            let beat = BeatTime::from_raw(raw);
            let frame = if raw == 0 {
                0
            } else if raw == end.raw() - start.raw() {
                end_frame - start_frame
            } else {
                tempo
                    .beat_to_frame(BeatTime::from_raw(
                        start
                            .raw()
                            .checked_add(raw)
                            .ok_or(Invalid("volume frame projection overflow"))?,
                    ))?
                    .checked_sub(start_frame)
                    .ok_or(Invalid("volume frame precedes range"))?
            };
            let point = FramePoint {
                frame,
                incoming: volume.gain_before(beat),
                value: volume.gain_at(beat),
            };
            if let Some(previous) = points.last_mut()
                && previous.frame == frame
            {
                // The last boundary in a rounded frame owns both limits,
                // including the preceding ramp's incoming endpoint.
                *previous = point;
            } else {
                points.push(point);
            }
        }
        Ok(Self {
            points,
            clip_gain: clip.volume().clip_gain(),
        })
    }
    pub(super) fn paste(
        &self,
        tempo: &TempoMap,
        start_frame: u64,
        end_frame: u64,
    ) -> Result<MidiClipVolume, Invalid> {
        if self.points.is_empty() {
            return Ok(MidiClipVolume::default().with_clip_gain(self.clip_gain));
        }
        let start = tempo.frame_to_beat(start_frame)?;
        let end = tempo.frame_to_beat(end_frame)?;
        let duration = end_frame
            .checked_sub(start_frame)
            .filter(|n| *n > 0)
            .ok_or(Invalid("volume frame range must be positive"))?;
        let length = end
            .raw()
            .checked_sub(start.raw())
            .filter(|n| *n > 0)
            .ok_or(Invalid("volume beat range collapsed"))?;
        let mut frames = BTreeSet::new();
        for point in &self.points {
            if point.frame > duration {
                return Err(Invalid("volume point exceeds pasted range"));
            }
            boundary(&mut frames, point.frame)?;
        }
        for point in tempo.points() {
            let frame = tempo.beat_to_frame(point.beat())?;
            if start_frame < frame && frame < end_frame {
                boundary(&mut frames, frame - start_frame)?;
            }
        }
        let mut points: Vec<MidiClipVolumePoint> = Vec::new();
        points
            .try_reserve_exact(frames.len())
            .map_err(|_| Invalid("could not reserve pasted volume points"))?;
        for frame in frames {
            let beat = if frame == 0 {
                BeatTime::ZERO
            } else if frame == duration {
                BeatTime::from_raw(length)
            } else {
                BeatTime::from_raw(
                    tempo
                        .frame_to_beat(
                            start_frame
                                .checked_add(frame)
                                .ok_or(Invalid("pasted volume frame overflow"))?,
                        )?
                        .raw()
                        .checked_sub(start.raw())
                        .ok_or(Invalid("pasted volume precedes clip"))?,
                )
            };
            let point = MidiClipVolumePoint {
                beat,
                incoming: self.gain(frame, true)?,
                value: self.gain(frame, false)?,
            };
            if let Some(previous) = points.last_mut()
                && previous.beat == beat
            {
                *previous = point;
            } else {
                points.push(point);
            }
        }
        MidiClipVolume::new(points, BeatTime::from_raw(length))
            .map(|v| v.with_clip_gain(self.clip_gain))
    }
    fn gain(&self, frame: u64, before: bool) -> Result<MidiClipGain, Invalid> {
        let index = self.points.partition_point(|point| point.frame < frame);
        if let Some(point) = self.points.get(index)
            && point.frame == frame
        {
            return Ok(if before { point.incoming } else { point.value });
        }
        let Some(previous) = index.checked_sub(1).and_then(|i| self.points.get(i)) else {
            return Ok(self
                .points
                .first()
                .map_or(MidiClipGain::UNITY, |point| point.incoming));
        };
        let Some(next) = self.points.get(index) else {
            return Ok(previous.value);
        };
        let fraction = (frame - previous.frame) as f64 / (next.frame - previous.frame) as f64;
        MidiClipGain::new(
            previous.value.linear() + (next.incoming.linear() - previous.value.linear()) * fraction,
        )
    }
}
