//! Persisted reversible track freeze metadata.
//! Freezing attaches a verified rendered audio asset to an ordinary track while
//! keeping its original clips, MIDI, instrument, devices, and automation intact.
//! Unfreezing clears the freeze state and restores live rendering immediately.
use crate::{Asset, AssetId, Created, IDENTITY_CONVERSION, Invalid, MAX_ASSETS, Project, TrackId};
use std::{collections::BTreeMap, path::PathBuf};

/// Persisted reversible freeze attachment owned by a single ordinary track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrackFreezeState {
    pub asset: AssetId,
    /// Project-rate timeline start frame of the frozen render.
    pub start: u64,
    /// Project-rate duration in frames of the frozen render.
    pub duration: u64,
    /// Verified playback frame length of `asset` at the project sample rate.
    pub source_frames: u64,
}

impl TrackFreezeState {
    pub fn validate(&self, assets: &BTreeMap<AssetId, Asset>) -> Result<(), Invalid> {
        if self.duration == 0 {
            return Err(Invalid("frozen track render duration must be nonzero"));
        }
        if self.start.checked_add(self.duration).is_none() {
            return Err(Invalid("frozen track render interval overflow"));
        }
        let asset = assets
            .get(&self.asset)
            .ok_or(Invalid("frozen track asset is missing"))?;
        if asset.playback_frames() != self.source_frames {
            return Err(Invalid(
                "frozen track source frames differ from referenced asset",
            ));
        }
        if self.duration > self.source_frames {
            return Err(Invalid(
                "frozen track duration exceeds referenced asset frames",
            ));
        }
        Ok(())
    }
}

/// Snapshot transport for one frozen track entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrozenTrackSnapshot {
    pub track: u64,
    pub asset: u64,
    pub start: u64,
    pub duration: u64,
    pub source_frames: u64,
}

impl Project {
    pub fn has_frozen_tracks(&self) -> bool {
        self.tracks.iter().any(|track| track.frozen.is_some())
    }

    pub(crate) fn freeze_track(
        &mut self,
        track: TrackId,
        path: PathBuf,
        start: u64,
        duration: u64,
        frames: u64,
    ) -> Result<Created, Invalid> {
        let owner = self.track(track).ok_or(Invalid("track no longer exists"))?;
        if owner.is_group() {
            return Err(Invalid("group tracks cannot be frozen directly"));
        }
        if owner.frozen.is_some() {
            return Err(Invalid("track is already frozen"));
        }
        if path.as_os_str().is_empty() || frames == 0 {
            return Err(Invalid("frozen track media needs a path and frames"));
        }
        if duration == 0 || duration > frames || start.checked_add(duration).is_none() {
            return Err(Invalid("frozen track render interval is invalid"));
        }
        if self.assets.len() >= MAX_ASSETS {
            return Err(Invalid("asset limit reached"));
        }
        let asset_id = AssetId(self.allocate_id()?);
        self.assets.insert(
            asset_id,
            Asset {
                original_source: None,
                id: asset_id,
                path,
                source_sample_rate: self.sample_rate,
                source_frames: frames,
                playback_frames: frames,
                conversion_version: IDENTITY_CONVERSION,
            },
        );
        self.track_mut(track)?.frozen = Some(TrackFreezeState {
            asset: asset_id,
            start,
            duration,
            source_frames: frames,
        });
        Ok(Created::Asset(asset_id))
    }

    pub(crate) fn freeze_track_with_asset(
        &mut self,
        track: TrackId,
        asset: AssetId,
        start: u64,
        duration: u64,
    ) -> Result<Created, Invalid> {
        let owner = self.track(track).ok_or(Invalid("track no longer exists"))?;
        if owner.is_group() {
            return Err(Invalid("group tracks cannot be frozen directly"));
        }
        if owner.frozen.is_some() {
            return Err(Invalid("track is already frozen"));
        }
        let media = self
            .assets
            .get(&asset)
            .ok_or(Invalid("media no longer exists"))?;
        let state = TrackFreezeState {
            asset,
            start,
            duration,
            source_frames: media.playback_frames(),
        };
        state.validate(&self.assets)?;
        self.track_mut(track)?.frozen = Some(state);
        Ok(Created::None)
    }

    pub(crate) fn unfreeze_track(&mut self, track: TrackId) -> Result<Created, Invalid> {
        let owner = self.track_mut(track)?;
        if owner.frozen.is_none() {
            return Err(Invalid("track is not frozen"));
        }
        owner.frozen = None;
        Ok(Created::None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BeatTime, Command, Created, Editor};

    #[test]
    fn track_freeze_preserves_midi_devices_and_supports_undo_redo_and_snapshot() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "lead".into(),
            })
            .unwrap()
        else {
            panic!("expected track");
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("expected clip");
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(2).unwrap(),
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();

        let Created::Asset(freeze_asset) = editor
            .execute(Command::FreezeTrack {
                track,
                path: PathBuf::from("frozen/lead.wav"),
                start: 0,
                duration: 96_000,
                frames: 96_000,
            })
            .unwrap()
        else {
            panic!("expected asset");
        };

        let frozen_track = editor.project().track(track).unwrap();
        assert_eq!(
            frozen_track.frozen(),
            Some(TrackFreezeState {
                asset: freeze_asset,
                start: 0,
                duration: 96_000,
                source_frames: 96_000,
            })
        );
        // Original MIDI clip and note remain untouched while frozen.
        assert_eq!(frozen_track.midi_clips().len(), 1);
        assert_eq!(frozen_track.midi_clips()[0].source().notes().len(), 1);

        // Frozen asset cannot be deleted while referenced.
        assert!(editor.execute(Command::RemoveAsset(freeze_asset)).is_err());
        // Double-freeze is refused.
        assert!(
            editor
                .execute(Command::FreezeTrackWithAsset {
                    track,
                    asset: freeze_asset,
                    start: 0,
                    duration: 96_000,
                })
                .is_err()
        );

        // Snapshot round-trip preserves freeze state and untouched MIDI.
        let restored = Project::from_snapshot(editor.project().snapshot()).unwrap();
        assert_eq!(&restored, editor.project());

        // Unfreeze restores live state cleanly; Undo re-freezes; Redo unfreezes.
        editor.execute(Command::UnfreezeTrack { track }).unwrap();
        assert!(!editor.project().track(track).unwrap().is_frozen());
        assert!(editor.undo());
        assert!(editor.project().track(track).unwrap().is_frozen());
        assert!(editor.redo());
        assert!(!editor.project().track(track).unwrap().is_frozen());
    }
}
