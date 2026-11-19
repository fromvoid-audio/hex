//! Exact, control-prepared raw resident input for one Arrangement mute.
use super::{RouteOwner, SessionPlayback};
use crate::{Clip, PreparedRevision, Track};
use hex_project::TrackId;
use std::sync::{Arc, Weak};

pub(super) struct ResidentMuteBasis {
    previous: Weak<PreparedRevision>,
    index: usize,
}

impl SessionPlayback {
    /// Prepare one outgoing resident source from exact compiled owners. Only the
    /// callback's actual outgoing owner can arm the fader on inheritance.
    pub fn prepare_resident_mute_on_control(
        &mut self,
        previous: &Arc<PreparedRevision>,
        track: TrackId,
    ) -> bool {
        if self.require_fresh_candidate().is_err() {
            return false;
        }
        self.resident_mute = None;
        let next = &self.revision;
        if !previous
            .session()
            .timing()
            .same_map(next.session().timing())
        {
            return false;
        }
        let Some(index) = next
            .arrangement()
            .tracks()
            .iter()
            .position(|entry| entry.canonical_track_id() == Some(track))
        else {
            return false;
        };
        let Some(old) = previous.arrangement().tracks().get(index) else {
            return false;
        };
        let new = &next.arrangement().tracks()[index];
        if old.canonical_track_id() != Some(track)
            || !old.audio_content_enabled()
            || old.mix().muted()
            || !new.audio_content_enabled()
            || !new.mix().muted()
            || !plain_resident_track(old)
            || !plain_resident_track(new)
            || old.clips.len() != new.clips.len()
            || !old
                .clips
                .iter()
                .zip(&new.clips)
                .all(|(a, b)| same_clip(a, b))
        {
            return false;
        }
        self.resident_mute = Some(ResidentMuteBasis {
            previous: Arc::downgrade(previous),
            index,
        });
        true
    }

    pub(super) fn resident_mute_index(&self, previous: &Self) -> Option<usize> {
        let basis = self.resident_mute.as_ref()?;
        let route = previous.routes.get(basis.index)?;
        (std::ptr::eq(basis.previous.as_ptr(), Arc::as_ptr(&previous.revision))
            && previous.playing
            && matches!(route.owner, RouteOwner::Arrangement)
            && route.pending.is_none()
            && route.audio_release.is_none())
        .then_some(basis.index)
    }
}

fn plain_resident_track(track: &Track) -> bool {
    !track.has_streamed_clips
        && !track.clips.is_empty()
        && track.source_id.is_none()
        && track.instrument_id.is_none()
        && track.instrument.is_none()
        && track.midi_source_index.is_none()
        && track.device_chain.is_empty()
        && track.sends().is_empty()
        && !track.has_gain_automation()
}

fn same_clip(old: &Clip, new: &Clip) -> bool {
    old.active == new.active
        && old.start == new.start
        && old.end == new.end
        && old.looping == new.looping
        && old.source_offset == new.source_offset
        && old.source_length == new.source_length
        && old
            .audio
            .as_ref()
            .zip(new.audio.as_ref())
            .is_some_and(|(a, b)| a.shares_pcm_with(b))
        && match (old.metadata.as_deref(), new.metadata.as_deref()) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                a.streamed.is_none()
                    && b.streamed.is_none()
                    && a.sparse.is_none()
                    && b.sparse.is_none()
                    && a.gain == b.gain
                    && a.source_phase == b.source_phase
                    && a.reversed == b.reversed
                    && a.fade == b.fade
            }
            _ => false,
        }
}
