//! Track creation, naming, mixer, transport and project-level edit helpers and dispatch.
use super::{Command, Created, TrackDeviceSlot};
use crate::{
    Invalid, LoopRegion, MAX_NAME_BYTES, MAX_TRACKS, MeterPoint, Project, TempoPoint, Track,
    TrackContent, TrackId,
};

pub(crate) fn name_valid(name: &str) -> Result<(), Invalid> {
    if name.trim().is_empty() || name.len() > MAX_NAME_BYTES || name.chars().any(char::is_control) {
        Err(Invalid(
            "name must be nonempty, without control characters, and at most 1024 bytes",
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn insert_track_before(
    project: &mut Project,
    name: String,
    before_track: Option<TrackId>,
) -> Result<TrackId, Invalid> {
    name_valid(&name)?;
    if project.tracks.len() >= MAX_TRACKS {
        return Err(Invalid("track limit reached"));
    }
    let index = match before_track {
        Some(track) => project
            .tracks
            .iter()
            .position(|candidate| candidate.id == track)
            .ok_or(Invalid("track insertion anchor no longer exists"))?,
        None => project.tracks.len(),
    };
    let parent_group = project.tracks.get(index).and_then(Track::parent_group);
    let track = new_track(project, name, crate::TrackKind::Ordinary, parent_group)?;
    let id = track.id();
    project.tracks.insert(index, track);
    Ok(id)
}

pub(crate) fn insert_track_before_with_gain(
    project: &mut Project,
    name: String,
    before_track: Option<TrackId>,
    gain: Option<f32>,
) -> Result<TrackId, Invalid> {
    if gain.is_some_and(|value| !value.is_finite() || !(0.0..=2.0).contains(&value)) {
        return Err(Invalid("gain must be finite and within 0..2"));
    }
    let track = insert_track_before(project, name, before_track)?;
    if let Some(gain) = gain {
        project.track_mut(track)?.gain = gain;
    }
    Ok(track)
}

pub(crate) fn new_track(
    project: &mut Project,
    name: String,
    kind: crate::TrackKind,
    parent_group: Option<TrackId>,
) -> Result<Track, Invalid> {
    name_valid(&name)?;
    if project.tracks.len() >= MAX_TRACKS {
        return Err(Invalid("track limit reached"));
    }
    if parent_group.is_some_and(|id| !project.track(id).is_some_and(Track::is_group)) {
        return Err(Invalid("track parent must be an existing group"));
    }
    let id = TrackId(project.allocate_id()?);
    let color = crate::appearance::new_track_color(project);
    Ok(Track {
        group_output_mutes: Default::default(),
        tags: Vec::new(),
        kind,
        parent_group,
        color,
        gain_lane: crate::GainLane::default(),
        id,
        name,
        automatic_name: false,
        gain: 1.0,
        balance: 0.0,
        muted: false,
        soloed: false,
        devices: Vec::new(),
        instrument: None,
        retained_source_path: false,
        instrument_position: None,
        content: TrackContent::default(),
        sends: Vec::new(),
    })
}

impl Project {
    pub(super) fn apply_track_command(&mut self, command: Command) -> Result<Created, Invalid> {
        match command {
            Command::SetTuning(tuning) => {
                self.tuning = tuning;
                Ok(Created::None)
            }
            Command::SpliceGainTouchBatch {
                track,
                expected_epoch,
                spans,
            } => {
                if spans.is_empty() || spans.len() > crate::MAX_GAIN_TOUCH_SPANS_PER_TRACK {
                    return Err(Invalid("gain touch take span count is outside its bound"));
                }
                if self
                    .track(track)
                    .ok_or(Invalid("track no longer exists"))?
                    .gain_lane
                    .recording_epoch
                    != expected_epoch
                {
                    return Err(Invalid("gain touch replacement epoch is stale"));
                }
                let mut epoch = expected_epoch;
                for span in &spans {
                    span.validate()?;
                    if span.context.recording_epoch <= epoch {
                        return Err(Invalid("gain touch take epochs must strictly increase"));
                    }
                    if span.context.sample_rate != self.sample_rate {
                        return Err(Invalid("gain touch sample rate differs from project"));
                    }
                    epoch = span.context.recording_epoch;
                }
                // Project::apply runs only on Editor's isolated candidate. A
                // later refusal discards every earlier pass and all its IDs.
                let mut epoch = expected_epoch;
                for span in spans {
                    let next = span.context.recording_epoch;
                    self.apply(Command::SpliceGainTouch {
                        track,
                        expected_epoch: epoch,
                        span,
                    })?;
                    epoch = next;
                }
                Ok(Created::None)
            }
            Command::SpliceGainTouch {
                track,
                expected_epoch,
                span,
            } => {
                let owner = self.track(track).ok_or(Invalid("track no longer exists"))?;
                if span.context.sample_rate != self.sample_rate {
                    return Err(Invalid("gain touch sample rate differs from project"));
                }
                let proposed_units = owner.gain_lane.touch_units(expected_epoch, &span)?;
                let other = self
                    .tracks()
                    .iter()
                    .filter(|owner| owner.id() != track)
                    .try_fold(0_usize, |count, owner| {
                        count
                            .checked_add(owner.gain_lane.units()?)
                            .ok_or(Invalid("gain lane count overflow"))
                    })?;
                if other
                    .checked_add(proposed_units)
                    .is_none_or(|count| count > crate::MAX_GAIN_POINTS)
                {
                    return Err(Invalid("project gain lane limit reached"));
                }
                let proposed = owner.gain_lane.with_touch(expected_epoch, span)?;
                self.track_mut(track)?.gain_lane = proposed;
                Ok(Created::None)
            }
            Command::ReplaceTrackGainEnvelope { track, points } => {
                self.replace_gain_envelope(track, points)
            }
            Command::UpsertTrackGainPoint { track, point } => self.upsert_gain_point(track, point),
            Command::MoveTrackGainPoint { track, from, to } => {
                self.move_gain_point(track, from, to)
            }
            Command::RemoveTrackGainPoint { track, beat } => self.remove_gain_point(track, beat),
            Command::ClearTrackGainEnvelope { track } => {
                let lane = &mut self.track_mut(track)?.gain_lane;
                lane.base.clear();
                lane.spans.clear();
                Ok(Created::None)
            }
            Command::SetTrackColor { track, color } => {
                self.track_mut(track)?.color = color;
                Ok(Created::None)
            }
            Command::GainBy { track, amount } => {
                if !amount.is_finite() {
                    return Err(Invalid("gain adjustment must be finite"));
                }
                let gain = (self
                    .track(track)
                    .ok_or(Invalid("track no longer exists"))?
                    .gain
                    + amount)
                    .clamp(0.0, 2.0);
                self.apply(Command::Gain { track, gain })
            }
            Command::BalanceBy { track, amount } => {
                if !amount.is_finite() {
                    return Err(Invalid("balance adjustment must be finite"));
                }
                let balance = (self
                    .track(track)
                    .ok_or(Invalid("track no longer exists"))?
                    .balance
                    + amount)
                    .clamp(-1.0, 1.0);
                self.apply(Command::Balance { track, balance })
            }
            Command::MasterGainBy { amount } => {
                if !amount.is_finite() {
                    return Err(Invalid("master gain adjustment must be finite"));
                }
                let gain = (self.master_gain + amount).clamp(0.0, 2.0);
                self.apply(Command::MasterGain { gain })
            }
            Command::ToggleTracksMute(tracks) => {
                let mut muted = false;
                for id in tracks.ids() {
                    muted |= !self
                        .track(*id)
                        .ok_or(Invalid("track selection no longer exists"))?
                        .muted;
                }
                for track in &mut self.tracks {
                    if tracks.ids().contains(&track.id()) {
                        track.muted = muted;
                    }
                }
                Ok(Created::None)
            }
            Command::ToggleMute(track) => {
                let muted = !self
                    .track(track)
                    .ok_or(Invalid("track no longer exists"))?
                    .muted;
                self.apply(Command::Mute { track, muted })
            }
            Command::InsertTrackWithDevice {
                name,
                before_track,
                slot,
                state,
            } => {
                let track = insert_track_before(self, name, before_track)?;
                match slot {
                    TrackDeviceSlot::Instrument => {
                        self.apply(Command::InsertInstrument { track, state })?;
                    }
                    TrackDeviceSlot::Effect => {
                        self.apply(Command::InsertDevice {
                            track,
                            before: None,
                            state,
                        })?;
                    }
                }
                Ok(Created::Track(track))
            }
            Command::InsertDefaultTrack { before_track } => Ok(Created::Track(
                crate::track_name::insert_default_track(self, before_track)?,
            )),
            Command::InsertDefaultTrackWithGain { before_track, gain } => {
                if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
                    return Err(Invalid("gain must be finite and within 0..2"));
                }
                let track = crate::track_name::insert_default_track(self, before_track)?;
                self.track_mut(track)?.gain = gain;
                Ok(Created::Track(track))
            }
            Command::InsertDefaultTrackWithDevice {
                before_track,
                slot,
                state,
                device_name,
            } => crate::track_name::insert_default_track_with_device(
                self,
                before_track,
                slot,
                state,
                device_name,
            ),
            Command::InsertDefaultTrackWithDeviceAndGain {
                before_track,
                slot,
                state,
                device_name,
                gain,
            } => {
                if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
                    return Err(Invalid("gain must be finite and within 0..2"));
                }
                let Created::Track(track) = crate::track_name::insert_default_track_with_device(
                    self,
                    before_track,
                    slot,
                    state,
                    device_name,
                )?
                else {
                    return Err(Invalid("device track creation returned no track"));
                };
                self.track_mut(track)?.gain = gain;
                Ok(Created::Track(track))
            }
            Command::LoadInstrument {
                track,
                replacing,
                state,
                device_name,
            } => crate::track_name::load_instrument(self, track, replacing, state, device_name),
            Command::AddTrack { name } => {
                Ok(Created::Track(insert_track_before(self, name, None)?))
            }
            Command::InsertTrack { name, before_track } => Ok(Created::Track(insert_track_before(
                self,
                name,
                before_track,
            )?)),
            Command::MoveTrackBefore { track, before } => self.move_track_before(track, before),
            Command::MoveTracksBefore { tracks, before } => {
                self.move_tracks_before(&tracks, before)
            }
            Command::GroupTracks { tracks, name } => self.group_tracks(&tracks, name),
            Command::UngroupTrack(group) => self.ungroup_track(group),
            Command::ReparentTracks {
                tracks,
                destination,
            } => self.reparent_tracks(&tracks, destination),
            Command::DuplicateTracks {
                tracks,
                without_clips,
            } => {
                let destination = self.track_destination_after_selection(&tracks)?;
                let clipboard = self.capture_tracks(&tracks, without_clips)?;
                crate::track_clipboard::paste(self, &clipboard, destination)
            }
            Command::RemoveTracks(ids) => self.remove_track_subtrees(&ids),
            Command::RemoveTrack(id) => self.remove_track_subtrees(&[id]),
            Command::SetTrackTags { track, tags } => {
                let tags = crate::track_tags::canonical_list(tags)?;
                self.track_mut(track)?.tags = tags;
                Ok(Created::None)
            }
            Command::RenameTrack { track, name } => {
                name_valid(&name)?;
                let target = self.track_mut(track)?;
                target.name = name;
                target.automatic_name = false;
                Ok(Created::None)
            }
            Command::Gain { track, gain } => crate::track_gain_group::apply(self, &[(track, gain)]),
            Command::SetTrackGains { gains } => crate::track_gain_group::apply(self, &gains),
            Command::Balance { track, balance } => {
                if !balance.is_finite() || !(-1.0..=1.0).contains(&balance) {
                    return Err(Invalid("balance must be finite and within -1..1"));
                }
                self.track_mut(track)?.balance = balance;
                Ok(Created::None)
            }
            Command::MasterGain { gain } => {
                if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
                    return Err(Invalid("master gain must be finite and within 0..2"));
                }
                self.master_gain = gain;
                Ok(Created::None)
            }
            Command::SetProjectTempo { tempo } => {
                self.set_project_tempo(tempo)?;
                Ok(Created::None)
            }
            Command::SetTempoPoint { beat, tempo } => {
                self.set_tempo_point(TempoPoint::new(beat, tempo))?;
                Ok(Created::None)
            }
            Command::SetMeterPoint { beat, meter } => {
                self.meter_map = self.meter_map.replace_point(MeterPoint::new(beat, meter))?;
                Ok(Created::None)
            }
            Command::Mute { track, muted } => {
                self.track_mut(track)?.muted = muted;
                Ok(Created::None)
            }
            Command::ToggleSolo { track, exclusive } => {
                let currently_soloed = self
                    .track(track)
                    .ok_or(Invalid("track no longer exists"))?
                    .soloed;
                if currently_soloed {
                    self.track_mut(track)?.soloed = false;
                } else {
                    if exclusive {
                        for item in &mut self.tracks {
                            item.soloed = false;
                        }
                    }
                    self.track_mut(track)?.soloed = true;
                }
                Ok(Created::None)
            }
            Command::ToggleSoloTracks { tracks, exclusive } => {
                let ids = tracks.ids();
                if ids.iter().any(|id| self.track(*id).is_none()) {
                    return Err(Invalid("solo selection no longer exists"));
                }
                let enable = !self
                    .tracks
                    .iter()
                    .filter(|track| ids.contains(&track.id()))
                    .all(|track| track.soloed);
                for track in &mut self.tracks {
                    if ids.contains(&track.id()) {
                        track.soloed = enable;
                    } else if enable && exclusive {
                        track.soloed = false;
                    }
                }
                Ok(Created::None)
            }
            Command::SetLoopRegion {
                start,
                end,
                enabled,
            } => {
                self.loop_region = Some(LoopRegion::new(start, end, enabled)?);
                Ok(Created::None)
            }
            Command::ToggleLoopRegion => {
                let region = self.loop_region.ok_or(Invalid("no loop region is set"))?;
                self.loop_region = Some(LoopRegion::new(
                    region.start(),
                    region.end(),
                    !region.enabled(),
                )?);
                Ok(Created::None)
            }
            Command::ClearLoopRegion => {
                self.loop_region = None;
                Ok(Created::None)
            }
            Command::PasteTracks {
                clipboard,
                destination,
            } => crate::track_clipboard::paste(self, &clipboard, destination),
            _ => Err(Invalid("not a track edit command")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_valid_rejects_empty_whitespace_and_control_characters() {
        assert!(name_valid("lead").is_ok());
        assert!(name_valid("   ").is_err());
        assert!(name_valid("bad\nname").is_err());
    }

    #[test]
    fn insert_track_before_rejects_unknown_anchor_without_mutating_project() {
        let mut project = Project::new(48_000).expect("project");
        let before = project.clone();
        assert!(insert_track_before(&mut project, "audio".into(), Some(TrackId(999))).is_err());
        assert_eq!(project, before);
    }
}
