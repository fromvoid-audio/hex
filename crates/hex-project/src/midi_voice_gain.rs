//! Immutable source-local gain layers retained by a grouped MIDI voice.
//!
//! Each layer keeps the original finite clip interval and its materialized
//! curve. The half-open end holds its left limit for an outgoing release.
//! Source coordinates remain independent of the enclosing clip's editable
//! volume and of a note's velocity. Rendering prepares these control values on
//! the existing timing owner; no callback conversion or ownership is implied.
use crate::{BeatTime, Invalid, MidiClipGain, MidiClipVolume, MidiSourcePlayback};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Leave one slot in the host's eight-factor product for the enclosing clip.
pub const MAX_MIDI_VOICE_GAIN_LAYERS: usize = 7;

/// One original finite clip law, positioned in a MIDI source. The curve's
/// local origin is `start`; `end` is exclusive and must remain in that source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MidiVoiceGainLayer {
    start: BeatTime,
    end: BeatTime,
    volume: MidiClipVolume,
}

impl MidiVoiceGainLayer {
    pub fn new(
        start: BeatTime,
        end: BeatTime,
        source_length: BeatTime,
        volume: MidiClipVolume,
    ) -> Result<Self, Invalid> {
        let length = end
            .raw()
            .checked_sub(start.raw())
            .filter(|n| *n > 0)
            .map(BeatTime::from_raw)
            .ok_or(Invalid("voice gain interval must be positive"))?;
        if end > source_length {
            return Err(Invalid("voice gain interval exceeds midi source"));
        }
        volume.validate_source_length(length)?;
        Ok(Self { start, end, volume })
    }
    pub fn start(&self) -> BeatTime {
        self.start
    }
    pub fn end(&self) -> BeatTime {
        self.end
    }
    pub fn volume(&self) -> &MidiClipVolume {
        &self.volume
    }
    /// Translate a materialized finite layer into a larger source without
    /// re-evaluating its curve or changing its held endpoint.
    pub fn shifted(&self, offset: BeatTime, source_length: BeatTime) -> Result<Self, Invalid> {
        let shift = |beat: BeatTime| {
            beat.raw()
                .checked_add(offset.raw())
                .map(BeatTime::from_raw)
                .ok_or(Invalid("voice gain shift overflow"))
        };
        Self::new(
            shift(self.start)?,
            shift(self.end)?,
            source_length,
            self.volume.clone(),
        )
    }
    /// Expand this finite owner to source coordinates. Before its start the
    /// first value is held; at and after its exclusive end the left limit is
    /// held. The resulting curve can use the existing playback projection and
    /// timing preparation without transferring ownership to the audio thread.
    pub fn source_volume(&self, source_length: BeatTime) -> Result<MidiClipVolume, Invalid> {
        if self.end > source_length {
            return Err(Invalid("voice gain interval exceeds midi source"));
        }
        if self.volume.points().is_empty() {
            return Ok(MidiClipVolume::default().with_clip_gain(self.volume.clip_gain()));
        }
        let mut points = Vec::new();
        points
            .try_reserve_exact(self.volume.points().len().saturating_add(3))
            .map_err(|_| Invalid("could not reserve voice gain projection"))?;
        let initial = self.volume.gain_at(BeatTime::ZERO);
        points.push(crate::MidiClipVolumePoint::linear(BeatTime::ZERO, initial));
        if self.start != BeatTime::ZERO {
            points.push(crate::MidiClipVolumePoint::linear(self.start, initial));
        }
        for point in self.volume.points() {
            if point.beat == BeatTime::ZERO || point.beat.raw() >= self.end.raw() - self.start.raw()
            {
                continue;
            }
            let beat = self
                .start
                .raw()
                .checked_add(point.beat.raw())
                .map(BeatTime::from_raw)
                .ok_or(Invalid("voice gain projection boundary overflow"))?;
            points.push(crate::MidiClipVolumePoint { beat, ..*point });
        }
        let held = self
            .volume
            .gain_before(BeatTime::from_raw(self.end.raw() - self.start.raw()));
        points.push(crate::MidiClipVolumePoint::linear(self.end, held));
        Ok(MidiClipVolume::new(points, source_length)?.with_clip_gain(self.volume.clip_gain()))
    }
    /// Project the retained source law through a finite clip view into a new
    /// grouped source. The law holds outside its original half-open interval;
    /// existing playback projection owns repeat seams and directed windows.
    pub fn materialized(
        &self,
        playback: MidiSourcePlayback,
        offset: BeatTime,
        destination_length: BeatTime,
    ) -> Result<Self, Invalid> {
        self.materialized_range(
            playback,
            BeatTime::ZERO,
            playback.arrangement_duration(),
            offset,
            destination_length,
        )
    }
    /// Project a finite subrange, retaining the curve's original phase while
    /// translating the selected interval to `offset` in the destination.
    pub fn materialized_range(
        &self,
        playback: MidiSourcePlayback,
        start: BeatTime,
        stop: BeatTime,
        offset: BeatTime,
        destination_length: BeatTime,
    ) -> Result<Self, Invalid> {
        if self.end > playback.source_length() {
            return Err(Invalid("voice gain interval exceeds midi source"));
        }
        let duration = stop
            .raw()
            .checked_sub(start.raw())
            .filter(|duration| *duration > 0)
            .map(BeatTime::from_raw)
            .ok_or(Invalid("voice gain projection interval must be positive"))?;
        if stop > playback.arrangement_duration() {
            return Err(Invalid("voice gain projection exceeds finite playback"));
        }
        let end = offset
            .raw()
            .checked_add(duration.raw())
            .map(BeatTime::from_raw)
            .ok_or(Invalid("voice gain projection overflow"))?;
        let full = self.source_volume(playback.source_length())?;
        let projected = full.materialized(playback, start, stop)?;
        Self::new(offset, end, destination_length, projected)
    }
    /// Scale absolute source boundaries using the same floor mapping as MIDI
    /// edge stretch. Refuse collapsed curve boundaries before changing data.
    pub fn scaled(
        &self,
        numerator: u64,
        denominator: u64,
        source_length: BeatTime,
    ) -> Result<Self, Invalid> {
        if numerator == 0 || denominator == 0 {
            return Err(Invalid("voice gain scale must be positive"));
        }
        let scale = |raw: u64| -> Result<u64, Invalid> {
            u64::try_from(u128::from(raw) * u128::from(numerator) / u128::from(denominator))
                .map_err(|_| Invalid("voice gain scale overflow"))
        };
        let start = scale(self.start.raw())?;
        let end = scale(self.end.raw())?;
        let length = end
            .checked_sub(start)
            .filter(|length| *length > 0)
            .map(BeatTime::from_raw)
            .ok_or(Invalid("voice gain scale collapses interval"))?;
        let mut points = Vec::new();
        points
            .try_reserve_exact(self.volume.points().len())
            .map_err(|_| Invalid("could not reserve scaled voice gain"))?;
        for point in self.volume.points() {
            let absolute = self
                .start
                .raw()
                .checked_add(point.beat.raw())
                .ok_or(Invalid("voice gain boundary overflow"))?;
            let local = scale(absolute)?
                .checked_sub(start)
                .ok_or(Invalid("voice gain boundary precedes scaled interval"))?;
            points.push(crate::MidiClipVolumePoint {
                beat: BeatTime::from_raw(local),
                ..*point
            });
        }
        let volume = MidiClipVolume::new(points, length)?.with_clip_gain(self.volume.clip_gain());
        Self::new(
            BeatTime::from_raw(start),
            BeatTime::from_raw(end),
            source_length,
            volume,
        )
    }

    /// Curve-only value for control-side validation. At and beyond the
    /// exclusive end, retain the exact left limit, including authored jumps.
    pub fn curve_gain_at(&self, source_beat: BeatTime) -> MidiClipGain {
        if source_beat >= self.end {
            return self
                .volume
                .gain_before(BeatTime::from_raw(self.end.raw() - self.start.raw()));
        }
        let local = BeatTime::from_raw(source_beat.raw().saturating_sub(self.start.raw()));
        self.volume.gain_at(local)
    }
}

/// Bounded ordered product; clones share immutable layer storage.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MidiVoiceGain {
    layers: Option<Arc<[MidiVoiceGainLayer]>>,
}
/// Owning identity of a nonempty immutable gain law. Holding it prevents
/// allocation-address reuse while a control-side preparation cache uses it.
/// Equal values allocated independently have distinct identities. This token
/// does not transfer audio-thread custody or prove exclusive ownership.
#[derive(Clone, Debug)]
pub struct MidiVoiceGainIdentity(Arc<[MidiVoiceGainLayer]>);
impl PartialEq for MidiVoiceGainIdentity {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for MidiVoiceGainIdentity {}
impl PartialOrd for MidiVoiceGainIdentity {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for MidiVoiceGainIdentity {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.as_ptr().cmp(&other.0.as_ptr())
    }
}
impl Hash for MidiVoiceGainIdentity {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.as_ptr().hash(state);
    }
}
impl MidiVoiceGainIdentity {
    /// Borrow immutable layers while this identity keeps their owner alive.
    pub fn layers(&self) -> &[MidiVoiceGainLayer] {
        &self.0
    }
}
impl MidiVoiceGain {
    pub fn new(layers: Vec<MidiVoiceGainLayer>) -> Result<Self, Invalid> {
        if layers.len() > MAX_MIDI_VOICE_GAIN_LAYERS {
            return Err(Invalid("voice gain layer limit reached"));
        }
        Ok(Self {
            layers: (!layers.is_empty()).then(|| layers.into()),
        })
    }
    pub fn layers(&self) -> &[MidiVoiceGainLayer] {
        self.layers.as_deref().unwrap_or(&[])
    }
    /// The empty unity law has no backing allocation or owning identity.
    pub fn identity(&self) -> Option<MidiVoiceGainIdentity> {
        self.layers
            .as_ref()
            .map(|layers| MidiVoiceGainIdentity(Arc::clone(layers)))
    }
    // Callers borrow the owning project for the complete identity comparison.
    pub(crate) fn owner_identity(&self) -> Option<*const MidiVoiceGainLayer> {
        self.layers.as_ref().map(|layers| layers.as_ptr())
    }
    /// Add one finite owner without changing the shared predecessor. The
    /// enclosing clip's independently editable gain uses the remaining slot.
    pub fn with_layer(&self, layer: MidiVoiceGainLayer) -> Result<Self, Invalid> {
        let count = self
            .layers()
            .len()
            .checked_add(1)
            .filter(|count| *count <= MAX_MIDI_VOICE_GAIN_LAYERS)
            .ok_or(Invalid("voice gain layer limit reached"))?;
        let mut layers = Vec::new();
        layers
            .try_reserve_exact(count)
            .map_err(|_| Invalid("could not reserve voice gain layers"))?;
        layers.extend_from_slice(self.layers());
        layers.push(layer);
        Self::new(layers)
    }
    /// Validate the layers against the current source after source edits or
    /// decoding; a shorter source must never silently discard an owner.
    pub fn validate_source_length(&self, source_length: BeatTime) -> Result<(), Invalid> {
        if self.layers().iter().any(|layer| layer.end > source_length) {
            return Err(Invalid("voice gain interval exceeds midi source"));
        }
        Ok(())
    }
    pub fn shifted(&self, offset: BeatTime, source_length: BeatTime) -> Result<Self, Invalid> {
        let mut layers = Vec::new();
        layers
            .try_reserve_exact(self.layers().len())
            .map_err(|_| Invalid("could not reserve shifted voice gains"))?;
        for layer in self.layers() {
            layers.push(layer.shifted(offset, source_length)?);
        }
        Self::new(layers)
    }
    pub fn materialized(
        &self,
        playback: MidiSourcePlayback,
        offset: BeatTime,
        destination_length: BeatTime,
    ) -> Result<Self, Invalid> {
        let mut layers = Vec::new();
        layers
            .try_reserve_exact(self.layers().len())
            .map_err(|_| Invalid("could not reserve projected voice gains"))?;
        for layer in self.layers() {
            layers.push(layer.materialized(playback, offset, destination_length)?);
        }
        Self::new(layers)
    }
    pub fn materialized_range(
        &self,
        playback: MidiSourcePlayback,
        start: BeatTime,
        stop: BeatTime,
        offset: BeatTime,
        destination_length: BeatTime,
    ) -> Result<Self, Invalid> {
        if start >= stop || stop > playback.arrangement_duration() {
            return Err(Invalid("voice gain projection interval is invalid"));
        }
        if offset
            .raw()
            .checked_add(stop.raw() - start.raw())
            .is_none_or(|end| end > destination_length.raw())
        {
            return Err(Invalid("voice gain projection exceeds destination"));
        }
        let mut layers = Vec::new();
        layers
            .try_reserve_exact(self.layers().len())
            .map_err(|_| Invalid("could not reserve projected voice gains"))?;
        for layer in self.layers() {
            layers.push(layer.materialized_range(
                playback,
                start,
                stop,
                offset,
                destination_length,
            )?);
        }
        Self::new(layers)
    }
    pub fn scaled(
        &self,
        numerator: u64,
        denominator: u64,
        source_length: BeatTime,
    ) -> Result<Self, Invalid> {
        if numerator == 0 || denominator == 0 {
            return Err(Invalid("voice gain scale must be positive"));
        }
        let mut layers = Vec::new();
        layers
            .try_reserve_exact(self.layers().len())
            .map_err(|_| Invalid("could not reserve scaled voice gains"))?;
        for layer in self.layers() {
            layers.push(layer.scaled(numerator, denominator, source_length)?);
        }
        Self::new(layers)
    }
    /// Conservative control-side retained bytes, charging shared curves for
    /// each layer rather than claiming allocation identity or exclusive use.
    pub fn retained_control_metadata_bytes(&self) -> Option<usize> {
        let Some(layers) = &self.layers else {
            return Some(0);
        };
        let mut bytes = std::mem::size_of_val(layers.as_ref())
            .checked_add(2 * std::mem::size_of::<usize>())?
            .checked_add(
                std::mem::align_of::<usize>().max(std::mem::align_of::<MidiVoiceGainLayer>()) - 1,
            )?;
        for layer in layers.iter() {
            bytes = bytes.checked_add(layer.volume.retained_control_metadata_bytes()?)?;
        }
        Some(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn b(n: u64) -> BeatTime {
        BeatTime::from_raw(n)
    }
    #[test]
    fn project_volume_budget_counts_shared_laws_once_and_distinct_laws_conservatively() {
        use crate::{Command, Created, Editor, MidiClipVolumePoint, Project};
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "midi".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        for pitch in 0..65 {
            editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch,
                    channel: 0,
                    velocity: 100,
                })
                .unwrap();
        }
        let points = (0..crate::MAX_MIDI_VOLUME_POINTS_PER_CLIP)
            .map(|index| MidiClipVolumePoint::linear(b(index as u64), MidiClipGain::UNITY))
            .collect();
        let volume = MidiClipVolume::new(points, BeatTime::ONE).unwrap();
        let layer =
            MidiVoiceGainLayer::new(BeatTime::ZERO, BeatTime::ONE, BeatTime::ONE, volume).unwrap();
        let shared = MidiVoiceGain::new(vec![layer.clone()]).unwrap();
        let mut project = editor.project().clone();
        for index in 0..65 {
            project
                .midi_source_mut(clip)
                .unwrap()
                .set_test_voice_gain(index, shared.clone());
        }
        assert!(project.validate_midi_volumes().is_ok());
        for index in 0..64 {
            project
                .midi_source_mut(clip)
                .unwrap()
                .set_test_voice_gain(index, MidiVoiceGain::new(vec![layer.clone()]).unwrap());
        }
        // Sixty-four distinct laws plus the remaining shared law exceed the project budget.
        assert!(project.validate_midi_volumes().is_err());
        project
            .midi_source_mut(clip)
            .unwrap()
            .set_test_voice_gain(64, MidiVoiceGain::default());
        assert!(project.validate_midi_volumes().is_ok());
    }
    #[test]
    fn incomplete_consumers_refuse_nonempty_note_owners_without_altering_project() {
        use crate::{
            AliasContentSnapshot, Command, Created, Editor, Project, SessionCellContentSnapshot,
            TrackContentSnapshot,
        };
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "midi".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        let layer = MidiVoiceGainLayer::new(
            BeatTime::ZERO,
            BeatTime::ONE,
            BeatTime::ONE,
            MidiClipVolume::default(),
        )
        .unwrap();
        let law = MidiVoiceGain::new(vec![layer]).unwrap();
        let mut injected = editor.project().clone();
        injected
            .midi_source_mut(clip)
            .unwrap()
            .set_test_voice_gain(0, law.clone());
        assert!(injected.validate_midi_volumes().is_ok());
        let longer = BeatTime::from_raw(BeatTime::ONE.raw() * 2);
        let out_of_bounds =
            MidiVoiceGainLayer::new(BeatTime::ZERO, longer, longer, MidiClipVolume::default())
                .unwrap();
        injected
            .midi_source_mut(clip)
            .unwrap()
            .set_test_voice_gain(0, MidiVoiceGain::new(vec![out_of_bounds]).unwrap());
        assert!(injected.validate_midi_volumes().is_err());
        let before = editor.project().snapshot();
        let mut snapshot = before.clone();
        let clips = match &mut snapshot.tracks[0].content {
            TrackContentSnapshot::Instrument(clips) => clips,
            TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
            _ => panic!("midi track"),
        };
        clips[0].notes[0].voice_gain = law.clone();
        assert!(Project::from_snapshot(snapshot).is_err());
        assert_eq!(editor.project().snapshot(), before);

        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        let Created::SessionClip(session) = editor
            .execute(Command::CreateSessionMidiClip {
                track,
                scene,
                name: "cell".into(),
                source_length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("session")
        };
        editor
            .execute(Command::AddMidiNote {
                clip: session,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        let mut snapshot = editor.project().snapshot();
        let SessionCellContentSnapshot::Midi(session_clip) = &mut snapshot.session.cells[0].content
        else {
            panic!("cell")
        };
        session_clip.source.notes[0].voice_gain = law.clone();
        assert!(Project::from_snapshot(snapshot).is_err());

        editor
            .execute(Command::DuplicateAliasClips(vec![clip]))
            .unwrap();
        let mut snapshot = editor.project().snapshot();
        let AliasContentSnapshot::Midi { notes, .. } = &mut snapshot.aliases.sources[0].content
        else {
            panic!("alias")
        };
        notes[0].voice_gain = law;
        assert!(Project::from_snapshot(snapshot).is_err());
    }
    #[test]
    fn finite_layer_holds_the_left_limit_and_rejects_invalid_source_intervals() {
        use crate::MidiClipVolumePoint;
        let curve = MidiClipVolume::new(
            vec![
                MidiClipVolumePoint::linear(b(0), MidiClipGain::new(0.25).unwrap()),
                MidiClipVolumePoint {
                    beat: b(8),
                    incoming: MidiClipGain::new(1.0).unwrap(),
                    value: MidiClipGain::new(3.0).unwrap(),
                },
            ],
            b(8),
        )
        .unwrap();
        let layer = MidiVoiceGainLayer::new(b(4), b(12), b(16), curve.clone()).unwrap();
        assert_eq!(layer.curve_gain_at(b(0)).linear(), 0.25);
        assert_eq!(layer.curve_gain_at(b(8)).linear(), 0.625);
        assert_eq!(layer.curve_gain_at(b(12)).linear(), 1.0);
        assert_eq!(layer.curve_gain_at(b(16)).linear(), 1.0);
        assert!(MidiVoiceGainLayer::new(b(4), b(4), b(16), curve.clone()).is_err());
        assert!(MidiVoiceGainLayer::new(b(4), b(12), b(11), curve.clone()).is_err());
        assert!(MidiVoiceGainLayer::new(b(4), b(11), b(16), curve).is_err());
    }
    #[test]
    fn ordered_layers_are_bounded_and_share_immutable_storage() {
        let layer = MidiVoiceGainLayer::new(b(0), b(8), b(8), MidiClipVolume::default()).unwrap();
        let law = MidiVoiceGain::new(vec![layer.clone(); MAX_MIDI_VOICE_GAIN_LAYERS]).unwrap();
        assert_eq!(law.layers().len(), MAX_MIDI_VOICE_GAIN_LAYERS);
        assert!(law.retained_control_metadata_bytes().unwrap() > 0);
        assert_eq!(law, law.clone());
        assert!(law.with_layer(layer.clone()).is_err());
        assert!(MidiVoiceGain::new(vec![layer; MAX_MIDI_VOICE_GAIN_LAYERS + 1]).is_err());
        assert_eq!(
            MidiVoiceGain::default().retained_control_metadata_bytes(),
            Some(0)
        );
    }
    #[test]
    fn owning_identity_reuses_shared_laws_without_equating_separate_allocations() {
        use std::collections::HashMap;
        let layer = MidiVoiceGainLayer::new(b(0), b(8), b(8), MidiClipVolume::default()).unwrap();
        let law = MidiVoiceGain::new(vec![layer.clone()]).unwrap();
        let shared = law.clone();
        let equal_but_distinct = MidiVoiceGain::new(vec![layer]).unwrap();
        assert_eq!(law, equal_but_distinct);
        let first = law.identity().unwrap();
        let same = shared.identity().unwrap();
        let other = equal_but_distinct.identity().unwrap();
        assert_eq!(first, same);
        assert_ne!(first, other);
        let mut sorted = vec![same.clone(), other.clone(), first.clone()];
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 2);
        let mut owners = HashMap::new();
        owners.insert(first, 7);
        drop(law);
        drop(shared);
        assert_eq!(owners.get(&same), Some(&7));
        assert_eq!(owners.get(&other), None);
        assert!(MidiVoiceGain::default().identity().is_none());
    }
    #[test]
    fn shifting_and_stretching_preserve_layer_law_and_refuse_collapsed_boundaries() {
        use crate::MidiClipVolumePoint;
        let curve = MidiClipVolume::new(
            vec![
                MidiClipVolumePoint::linear(b(0), MidiClipGain::new(0.25).unwrap()),
                MidiClipVolumePoint::linear(b(2), MidiClipGain::new(0.5).unwrap()),
                MidiClipVolumePoint::linear(b(8), MidiClipGain::new(1.0).unwrap()),
            ],
            b(8),
        )
        .unwrap();
        let layer = MidiVoiceGainLayer::new(b(4), b(12), b(16), curve).unwrap();
        let moved = layer.shifted(b(3), b(19)).unwrap();
        for at in [4, 6, 8, 12, 15] {
            assert_eq!(layer.curve_gain_at(b(at)), moved.curve_gain_at(b(at + 3)));
        }
        let stretched = layer.scaled(2, 1, b(32)).unwrap();
        for at in [4, 6, 8, 12, 15] {
            assert_eq!(
                layer.curve_gain_at(b(at)),
                stretched.curve_gain_at(b(at * 2))
            );
        }
        assert!(layer.scaled(1, 8, b(2)).is_err());
        assert!(layer.shifted(b(u64::MAX), b(u64::MAX)).is_err());
        assert!(layer.scaled(0, 1, b(16)).is_err());
    }
    #[test]
    fn materialized_voice_law_preserves_pre_interval_hold_and_repeated_source_phase() {
        use crate::MidiClipVolumePoint;
        let curve = MidiClipVolume::new(
            vec![
                MidiClipVolumePoint::linear(b(0), MidiClipGain::new(0.25).unwrap()),
                MidiClipVolumePoint::linear(b(4), MidiClipGain::new(1.0).unwrap()),
                MidiClipVolumePoint::linear(b(8), MidiClipGain::new(2.0).unwrap()),
            ],
            b(8),
        )
        .unwrap();
        let law = MidiVoiceGainLayer::new(b(4), b(12), b(16), curve).unwrap();
        let playback = MidiSourcePlayback::new(b(16), b(0), Some((b(2), b(14))), b(40)).unwrap();
        let projected = law.materialized(playback, b(3), b(43)).unwrap();
        for elapsed in 0..40 {
            let phase = playback.source_phase_at_elapsed(b(elapsed)).unwrap();
            let expected = law.curve_gain_at(phase).linear();
            let actual = projected.curve_gain_at(b(elapsed + 3)).linear();
            assert!(
                (actual - expected).abs() < 1e-12,
                "elapsed {elapsed}: {actual} != {expected}"
            );
        }
        assert_eq!(projected.start(), b(3));
        assert_eq!(projected.end(), b(43));
        let cropped = law
            .materialized_range(playback, b(5), b(25), b(2), b(22))
            .unwrap();
        for elapsed in 5..25 {
            let phase = playback.source_phase_at_elapsed(b(elapsed)).unwrap();
            let expected = law.curve_gain_at(phase).linear();
            let actual = cropped.curve_gain_at(b(elapsed - 5 + 2)).linear();
            assert!(
                (actual - expected).abs() < 1e-12,
                "cropped {elapsed}: {actual} != {expected}"
            );
        }
        assert!(
            law.materialized_range(playback, b(5), b(5), b(0), b(1))
                .is_err()
        );
        assert!(
            law.materialized(playback, b(u64::MAX), b(u64::MAX))
                .is_err()
        );
    }
    #[test]
    fn materialized_voice_law_reflects_reversed_repeated_source() {
        use crate::{ContentWindow, MidiClipVolumePoint};
        let q = |n: u64| BeatTime::from_raw(n << 32);
        let curve = MidiClipVolume::new(
            vec![
                MidiClipVolumePoint::linear(q(0), MidiClipGain::new(0.25).unwrap()),
                MidiClipVolumePoint::linear(q(4), MidiClipGain::new(1.0).unwrap()),
                MidiClipVolumePoint::linear(q(8), MidiClipGain::new(2.0).unwrap()),
            ],
            q(8),
        )
        .unwrap();
        let law = MidiVoiceGainLayer::new(q(4), q(12), q(16), curve).unwrap();
        let playback = MidiSourcePlayback::new(q(16), q(0), Some((q(2), q(14))), q(40))
            .unwrap()
            .with_content_window(ContentWindow::new(0, q(40).raw(), true).unwrap())
            .unwrap();
        let projected = law.materialized(playback, q(3), q(43)).unwrap();
        for quarter in 0..40 {
            let elapsed = BeatTime::from_raw(q(quarter).raw() + (1_u64 << 31));
            let phase = playback.source_phase_at_elapsed(elapsed).unwrap();
            let expected = law.curve_gain_at(phase).linear();
            let actual = projected
                .curve_gain_at(BeatTime::from_raw(elapsed.raw() + q(3).raw()))
                .linear();
            assert!(
                (actual - expected).abs() < 1e-9,
                "quarter {quarter}: {actual} != {expected}"
            );
        }
    }
    #[test]
    fn nested_gain_layers_keep_order_and_independent_clip_scalars_on_projection() {
        let first = MidiVoiceGainLayer::new(
            b(0),
            b(8),
            b(16),
            MidiClipVolume::default().with_clip_gain(crate::ClipGain::new(-6_000).unwrap()),
        )
        .unwrap();
        let second = MidiVoiceGainLayer::new(
            b(4),
            b(12),
            b(16),
            MidiClipVolume::default().with_clip_gain(crate::ClipGain::new(6_000).unwrap()),
        )
        .unwrap();
        let original = MidiVoiceGain::new(vec![first, second]).unwrap();
        let playback = MidiSourcePlayback::new(b(16), b(0), Some((b(2), b(14))), b(20)).unwrap();
        let projected = original.materialized(playback, b(3), b(23)).unwrap();
        assert_eq!(projected.layers().len(), 2);
        assert_eq!(projected.layers()[0].volume().clip_gain().millidb(), -6_000);
        assert_eq!(projected.layers()[1].volume().clip_gain().millidb(), 6_000);
        assert!(original.validate_source_length(b(11)).is_err());
        assert!(original.validate_source_length(b(16)).is_ok());
    }
    #[test]
    fn splitting_a_crossing_note_projects_its_original_gain_into_the_right_source() {
        use crate::{Command, Created, MidiClipVolumePoint, Project};
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
            .apply(Command::AddMidiNote {
                clip,
                start: q(1),
                duration: q(2),
                pitch: 60,
                channel: 0,
                velocity: 100,
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
        let original = MidiVoiceGainLayer::new(q(0), q(4), q(4), curve).unwrap();
        project
            .midi_source_mut(clip)
            .unwrap()
            .set_test_voice_gain(0, MidiVoiceGain::new(vec![original.clone()]).unwrap());
        let fragments = crate::midi::materialize_overwrite_ranges(
            project.midi_clip(clip).unwrap(),
            &[(q(2).raw(), q(4).raw())],
        )
        .unwrap();
        assert!(fragments[0].source().content.may_have_voice_gains);
        let fragment_note = &fragments[0].notes()[0];
        for t in 0..=2 {
            assert_eq!(
                fragment_note.voice_gain().layers()[0].curve_gain_at(q(t)),
                original.curve_gain_at(q(t + 2))
            );
        }
        let Created::Clip(right) = project
            .apply(Command::SplitMidiClip { clip, beat: q(2) })
            .unwrap()
        else {
            panic!("split")
        };
        assert!(
            project
                .midi_clip(clip)
                .unwrap()
                .source()
                .content
                .may_have_voice_gains
        );
        assert!(
            project
                .midi_clip(right)
                .unwrap()
                .source()
                .content
                .may_have_voice_gains
        );
        assert!(project.validate_midi_volumes().is_ok());
        let left = &project.midi_clip(clip).unwrap().notes()[0];
        let right = &project.midi_clip(right).unwrap().notes()[0];
        assert_eq!(left.voice_gain().layers()[0], original);
        assert_eq!(right.start(), q(0));
        assert_eq!(right.duration(), q(1));
        for t in 0..=2 {
            assert_eq!(
                right.voice_gain().layers()[0].curve_gain_at(q(t)),
                original.curve_gain_at(q(t + 2))
            );
        }
    }
    #[test]
    fn source_range_duplication_keeps_the_selected_gain_phase() {
        use crate::{Command, Created, Editor, MidiClipVolumePoint, Project};
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
            .apply(Command::AddMidiNote {
                clip,
                start: q(1),
                duration: q(2),
                pitch: 60,
                channel: 0,
                velocity: 100,
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
        let law = MidiVoiceGainLayer::new(q(0), q(4), q(4), curve).unwrap();
        project
            .midi_source_mut(clip)
            .unwrap()
            .set_test_voice_gain(0, MidiVoiceGain::new(vec![law.clone()]).unwrap());
        let mut editor = Editor::new(project, 16).unwrap();
        let source = editor.project().clone();
        let before = editor.project().snapshot();
        editor
            .execute_midi_prepared_transition::<(), Invalid>(
                Command::DuplicateMidiSourceRange {
                    clip,
                    start: q(0),
                    end: q(2),
                },
                |_, _| Ok(()),
            )
            .unwrap();
        let notes = editor.project().midi_clip(clip).unwrap().notes();
        assert!(
            editor
                .project()
                .midi_clip(clip)
                .unwrap()
                .source()
                .content
                .may_have_voice_gains
        );
        assert!(editor.project().validate_midi_volumes().is_ok());
        let copied = notes.iter().find(|note| note.start() == q(3)).unwrap();
        assert_eq!(copied.duration(), q(1));
        for t in 0..=2 {
            assert_eq!(
                copied.voice_gain().layers()[0].curve_gain_at(q(t + 2)),
                law.curve_gain_at(q(t))
            );
        }
        let effect = editor.next_midi_history_effect(false).unwrap();
        let mut tampered = editor.project().clone();
        let altered = tampered
            .midi_source_mut(clip)
            .unwrap()
            .notes
            .iter_mut()
            .find(|note| note.start() == q(3))
            .unwrap();
        altered.voice_gain = MidiVoiceGain::default();
        assert!(!effect.validates_forward(&source, &tampered));
        let after = editor.project().snapshot();
        assert!(editor.undo());
        let mut restored = editor.project().snapshot();
        restored.next_id = before.next_id;
        assert_eq!(restored, before);
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
    }
}
