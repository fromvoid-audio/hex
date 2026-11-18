//! Audio import members that place an existing project asset.
//!
//! Purpose: let an import batch reuse an `AssetId` already owned by the project
//! instead of minting a duplicate asset for media that is already resident.
//! A reused member follows the same placement law as a new prepared import:
//! Arrangement placement (the overwrite flag keeps alias inspection parity),
//! new-track insertion, Console cells, captured auto-warp and batch-wide
//! footprint resolution through `Editor::execute_audio_imports_with_warp_prepared`.
//!
//! Dependencies: the canonical `Project` model and its existing placement
//! commands. It does not own media residency, file identity matching or
//! resident PCM accounting; hex-session decides which members to reuse.
//!
//! Only ordinary, online file assets may be reused. Missing, offline and device
//! assets (device-original sources or assets referenced by a device) are refused,
//! and refusal inside a batch leaves the project and history untouched.
use crate::{AssetId, Command, Created, Invalid, Project, SceneId, TrackId};

/// Destination of an import member that reuses an existing project asset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExistingAudioPlacement {
    /// Insert a new hybrid track before `before_track` (append with `None`) and
    /// place a full-length clip at `start`.
    NewTrack {
        name: String,
        before_track: Option<TrackId>,
        start: u64,
    },
    /// Place a full-length clip on an existing track. `overwrite` mirrors the
    /// distinction between the new-asset clip import commands.
    Clip {
        track: TrackId,
        start: u64,
        overwrite: bool,
    },
    /// Place a one-shot source slice in a Console (Session) cell.
    Session {
        track: TrackId,
        scene: SceneId,
        name: String,
        source_start: u64,
        source_end: u64,
    },
}

impl ExistingAudioPlacement {
    /// Whether this member belongs to an Arrangement import plan.
    pub(crate) fn is_arrangement(&self) -> bool {
        !matches!(self, Self::Session { .. })
    }

    /// Mirrors the new-asset commands that alias inspection must examine.
    pub(crate) fn may_replace_clips(&self) -> bool {
        matches!(
            self,
            Self::Clip {
                overwrite: true,
                ..
            } | Self::Session { .. }
        )
    }
}

impl Project {
    /// Validate that `asset` may be placed by an import member and return its
    /// project-rate playback frame count. Session owners use this read-only
    /// check to decide whether a member may reuse the asset before building a
    /// `Command::ImportExistingAudio`; the command repeats it when applied.
    pub fn existing_import_frames(&self, asset: AssetId) -> Result<u64, Invalid> {
        let media = self
            .asset(asset)
            .ok_or(Invalid("imported media no longer exists"))?;
        if self.asset_offline(asset) {
            return Err(Invalid("missing media cannot be placed by an import"));
        }
        if media.original_source().is_some() || self.device_uses_asset(asset) {
            return Err(Invalid("device media cannot be placed by an import"));
        }
        Ok(media.playback_frames)
    }

    /// Single-command path; batches route Arrangement members through
    /// `apply_audio_imports` so footprints resolve together.
    pub(crate) fn apply_existing_audio_import(
        &mut self,
        asset: AssetId,
        placement: ExistingAudioPlacement,
    ) -> Result<Created, Invalid> {
        let frames = self.existing_import_frames(asset)?;
        let (track, start) = match placement {
            ExistingAudioPlacement::Session {
                track,
                scene,
                name,
                source_start,
                source_end,
            } => {
                return self.apply(Command::CreateSessionAudioClip {
                    track,
                    scene,
                    name,
                    asset,
                    source_start,
                    source_end,
                });
            }
            ExistingAudioPlacement::NewTrack {
                name,
                before_track,
                start,
            } => (
                crate::edit::insert_track_before(self, name, before_track)?,
                start,
            ),
            ExistingAudioPlacement::Clip { track, start, .. } => {
                self.track(track).ok_or(Invalid("track no longer exists"))?;
                (track, start)
            }
        };
        let clip = match self.apply(Command::PlaceClip {
            track,
            asset,
            start,
            duration: frames,
            source_offset: 0,
            source_length: frames,
            looping: false,
        })? {
            Created::Clip(id) => id,
            _ => return Err(Invalid("audio import did not create its clip")),
        };
        Ok(Created::Imported { track, asset, clip })
    }
}

#[cfg(test)]
#[path = "import_existing_tests.rs"]
mod tests;
