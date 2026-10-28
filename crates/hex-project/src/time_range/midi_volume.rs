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
        let (start, end) = clip_range_beats(clip, tempo, start_frame, end_frame)?;
        let elapsed_start = start.raw() - clip.start().raw();
        let elapsed_end = end.raw() - clip.start().raw();
        let volume = clip.volume().materialized(
            clip.source_playback()?,
            BeatTime::from_raw(elapsed_start),
            BeatTime::from_raw(elapsed_end),
        )?;
        Self::capture_projected(&volume, tempo, start, end, start_frame, end_frame)
    }

    fn capture_voice_layer(
        layer: &crate::MidiVoiceGainLayer,
        clip: &MidiClip,
        tempo: &TempoMap,
        start_frame: u64,
        end_frame: u64,
    ) -> Result<Self, Invalid> {
        if layer.volume().points().is_empty() {
            return Ok(Self {
                clip_gain: layer.volume().clip_gain(),
                ..Self::default()
            });
        }
        let (start, end) = clip_range_beats(clip, tempo, start_frame, end_frame)?;
        let elapsed_start = BeatTime::from_raw(start.raw() - clip.start().raw());
        let elapsed_end = BeatTime::from_raw(end.raw() - clip.start().raw());
        let length = BeatTime::from_raw(
            end.raw()
                .checked_sub(start.raw())
                .filter(|n| *n > 0)
                .ok_or(Invalid("voice gain frame range collapsed"))?,
        );
        let projected = layer.materialized_range(
            clip.source_playback()?,
            elapsed_start,
            elapsed_end,
            BeatTime::ZERO,
            length,
        )?;
        Self::capture_projected(
            projected.volume(),
            tempo,
            start,
            end,
            start_frame,
            end_frame,
        )
    }

    fn capture_projected(
        volume: &MidiClipVolume,
        tempo: &TempoMap,
        start: BeatTime,
        end: BeatTime,
        start_frame: u64,
        end_frame: u64,
    ) -> Result<Self, Invalid> {
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
            clip_gain: volume.clip_gain(),
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

fn clip_range_beats(
    clip: &MidiClip,
    tempo: &TempoMap,
    start_frame: u64,
    end_frame: u64,
) -> Result<(BeatTime, BeatTime), Invalid> {
    let clip_end = BeatTime::from_raw(
        clip.start()
            .raw()
            .checked_add(clip.length().raw())
            .ok_or(Invalid("midi clip volume end overflow"))?,
    );
    // Preserve authored clip edges when frame rounding maps adjacent beats to
    // the same frame; otherwise use the captured frame's canonical beat.
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
    if start < clip.start() || end <= start || end > clip_end {
        return Err(Invalid("volume range is outside midi clip"));
    }
    Ok((start, end))
}

/// Frame-anchored projection keeps each inherited law independent across a
/// tempo-changing copy or insert; empty owners do not allocate.
#[derive(Clone, Debug, Default)]
pub(super) struct FrameVoiceGain(Option<std::sync::Arc<[FrameVolume]>>);
impl FrameVoiceGain {
    pub(super) fn capture(
        gain: &crate::MidiVoiceGain,
        clip: &MidiClip,
        tempo: &TempoMap,
        start_frame: u64,
        end_frame: u64,
    ) -> Result<Self, Invalid> {
        if gain.layers().is_empty() {
            return Ok(Self::default());
        }
        let mut layers = Vec::new();
        layers
            .try_reserve_exact(gain.layers().len())
            .map_err(|_| Invalid("could not reserve frame voice gains"))?;
        for layer in gain.layers() {
            layers.push(FrameVolume::capture_voice_layer(
                layer,
                clip,
                tempo,
                start_frame,
                end_frame,
            )?);
        }
        Ok(Self(Some(layers.into())))
    }
    pub(super) fn paste(
        &self,
        tempo: &TempoMap,
        start_frame: u64,
        end_frame: u64,
    ) -> Result<crate::MidiVoiceGain, Invalid> {
        let Some(source) = &self.0 else {
            return Ok(crate::MidiVoiceGain::default());
        };
        let start = tempo.frame_to_beat(start_frame)?;
        let end = tempo.frame_to_beat(end_frame)?;
        let length = BeatTime::from_raw(
            end.raw()
                .checked_sub(start.raw())
                .filter(|n| *n > 0)
                .ok_or(Invalid("pasted voice gain interval collapsed"))?,
        );
        let mut layers = Vec::new();
        layers
            .try_reserve_exact(source.len())
            .map_err(|_| Invalid("could not reserve pasted voice gains"))?;
        for layer in source.iter() {
            layers.push(crate::MidiVoiceGainLayer::new(
                BeatTime::ZERO,
                length,
                length,
                layer.paste(tempo, start_frame, end_frame)?,
            )?);
        }
        crate::MidiVoiceGain::new(layers)
    }
}

#[cfg(test)]
mod voice_tests {
    use super::*;
    use crate::{Command, Created, MidiVoiceGain, MidiVoiceGainLayer, Project, Tempo};
    #[test]
    fn selected_voice_law_follows_frames_across_a_tempo_change() {
        let q = |n| BeatTime::from_quarters(n).unwrap();
        let mut project = Project::new(48_000).unwrap();
        let Created::Track(track) = project
            .apply(Command::AddMidiTrack {
                name: "midi".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(clip) = project
            .apply(Command::PlaceMidiClip {
                track,
                start: q(0),
                length: q(4),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        project
            .apply(Command::SetTempoPoint {
                beat: q(4),
                tempo: Tempo::new(60_000_000).unwrap(),
            })
            .unwrap();
        let curve = MidiClipVolume::new(
            vec![
                MidiClipVolumePoint::linear(q(0), MidiClipGain::new(0.25).unwrap()),
                MidiClipVolumePoint::linear(q(4), MidiClipGain::new(1.0).unwrap()),
            ],
            q(4),
        )
        .unwrap();
        let gain = MidiVoiceGain::new(vec![
            MidiVoiceGainLayer::new(q(0), q(4), q(4), curve).unwrap(),
        ])
        .unwrap();
        let captured = FrameVoiceGain::capture(
            &gain,
            project.midi_clip(clip).unwrap(),
            project.tempo_map(),
            24_000,
            72_000,
        )
        .unwrap();
        let pasted = captured
            .paste(project.tempo_map(), 96_000, 144_000)
            .unwrap();
        let layer = &pasted.layers()[0];
        let midpoint = BeatTime::from_raw(q(1).raw() / 2);
        assert_eq!(layer.curve_gain_at(BeatTime::ZERO).linear(), 0.4375);
        assert_eq!(layer.curve_gain_at(midpoint).linear(), 0.625);
        assert_eq!(layer.curve_gain_at(q(1)).linear(), 0.8125);
    }
}
