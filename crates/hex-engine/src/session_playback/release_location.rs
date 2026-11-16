//! Lossless private route/tail indices; checked before release ownership commits.
use super::{MAX_TRACKS, SESSION_TAIL_GROUP_CAPACITY, SessionPlaybackError};

const NO_TAIL: u16 = 256;
const _: () = {
    assert!(MAX_TRACKS < u16::MAX as usize);
    assert!(SESSION_TAIL_GROUP_CAPACITY <= NO_TAIL as usize);
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ReleaseLocation(u32);

impl ReleaseLocation {
    pub(super) fn new(
        track_index: usize,
        tail_group: Option<u16>,
    ) -> Result<Self, SessionPlaybackError> {
        let track = if track_index == usize::MAX {
            u16::MAX
        } else if track_index < MAX_TRACKS {
            u16::try_from(track_index).map_err(|_| SessionPlaybackError::ReleaseBankFull)?
        } else {
            return Err(SessionPlaybackError::ReleaseBankFull);
        };
        let tail = match tail_group {
            Some(group) if usize::from(group) < SESSION_TAIL_GROUP_CAPACITY => group,
            // 256 is outside the validated tail domain, leaving the original
            // corrupt u16::MAX test value distinguishable from absence.
            None => NO_TAIL,
            Some(_) => return Err(SessionPlaybackError::ReleaseBankFull),
        };
        Ok(Self(u32::from(track) | (u32::from(tail) << 16)))
    }

    #[cfg(test)]
    pub(super) fn corrupt_detached_tail_for_test() -> Self {
        Self(u32::MAX)
    }

    pub(super) fn track_index(self) -> usize {
        let track = (self.0 & u32::from(u16::MAX)) as u16;
        if track == u16::MAX {
            usize::MAX
        } else {
            usize::from(track)
        }
    }

    pub(super) fn tail_group(self) -> Option<u16> {
        let tail = (self.0 >> 16) as u16;
        (tail != NO_TAIL).then_some(tail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_playback::{ReleaseEntry, SessionContinuation};
    use std::mem::{align_of, size_of};

    // Uses the actual current payload and source metadata; only location
    // storage differs.
    #[allow(dead_code)]
    struct LegacyEntry {
        source_origin: Option<super::super::ReleaseSourceOrigin>,
        monitor_offset: u64,
        track_index: usize,
        tail_group: Option<u16>,
        track_id: hex_project::TrackId,
        order: u64,
        mix: crate::PreparedTrackMix,
        continuation: Option<SessionContinuation>,
        inline_tuning_owned: bool,
        clip_gain: f64,
        voice: crate::midi_template::MidiReleaseVoice,
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn compiled_layout_saves_eight_bytes_without_changing_payload_or_capacity() {
        assert_eq!(usize::BITS, 64);
        assert_eq!(MAX_TRACKS, 256);
        assert_eq!(SESSION_TAIL_GROUP_CAPACITY, 256);
        assert_eq!(size_of::<ReleaseLocation>(), 4);
        assert_eq!(align_of::<ReleaseLocation>(), 4);
        let old = size_of::<LegacyEntry>();
        let new = size_of::<ReleaseEntry>();
        eprintln!(
            "release location proof: old={old} new={new} old_align={} new_align={} banks=2 capacity={}",
            align_of::<LegacyEntry>(),
            align_of::<ReleaseEntry>(),
            super::super::SESSION_RELEASE_CAPACITY
        );
        assert_eq!(align_of::<LegacyEntry>(), align_of::<ReleaseEntry>());
        assert_eq!(old - new, 8);
        assert_eq!(
            2 * super::super::SESSION_RELEASE_CAPACITY * (old - new),
            262_144
        );
        let combined =
            30_990_312 - 2 * super::super::SESSION_RELEASE_CAPACITY * (old - new) + 2_639_456;
        assert_eq!(combined, 33_367_624);
        assert!(combined < super::super::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES);
    }

    #[test]
    fn all_independent_locations_roundtrip_and_sort_in_original_order() {
        let tracks = (0..MAX_TRACKS).chain(std::iter::once(usize::MAX));
        let tails: Vec<_> = std::iter::once(None)
            .chain((0..SESSION_TAIL_GROUP_CAPACITY).map(|n| Some(u16::try_from(n).unwrap())))
            .collect();
        let mut original = Vec::new();
        let mut compact = Vec::new();
        for track in tracks {
            for &tail in &tails {
                let location = ReleaseLocation::new(track, tail).unwrap();
                assert_eq!(
                    (location.track_index(), location.tail_group()),
                    (track, tail)
                );
                for order in [0, u64::MAX] {
                    original.push((track, tail, order));
                    compact.push((location, order));
                }
                for remapped in [0, MAX_TRACKS - 1, usize::MAX] {
                    let relocated = ReleaseLocation::new(remapped, location.tail_group()).unwrap();
                    assert_eq!(
                        (relocated.track_index(), relocated.tail_group()),
                        (remapped, tail)
                    );
                }
            }
        }
        original.reverse();
        compact.reverse();
        original.sort_unstable();
        compact.sort_unstable_by_key(|(location, order)| {
            (location.track_index(), location.tail_group(), *order)
        });
        let decoded: Vec<_> = compact
            .iter()
            .map(|(location, order)| (location.track_index(), location.tail_group(), *order))
            .collect();
        assert_eq!(original, decoded);
        assert_eq!(original.len(), 2 * 257 * 257);
    }

    #[test]
    fn malformed_original_tail_sentinel_remains_distinct_from_absence() {
        // Preserve the existing rollback fixture's deliberately corrupt pair.
        let corrupt = ReleaseLocation(u32::MAX);
        assert_eq!(corrupt.track_index(), usize::MAX);
        assert_eq!(corrupt.tail_group(), Some(u16::MAX));
        assert_ne!(corrupt, ReleaseLocation::new(usize::MAX, None).unwrap());
    }

    #[test]
    fn invalid_locations_are_refused_without_truncation() {
        for index in [MAX_TRACKS, usize::from(u16::MAX), usize::MAX - 1] {
            assert_eq!(
                ReleaseLocation::new(index, None),
                Err(SessionPlaybackError::ReleaseBankFull)
            );
        }
        for tail in [256, u16::MAX] {
            assert_eq!(
                ReleaseLocation::new(0, Some(tail)),
                Err(SessionPlaybackError::ReleaseBankFull)
            );
        }
    }
}
