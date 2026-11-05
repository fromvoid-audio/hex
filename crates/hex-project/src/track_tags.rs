//! Canonical, bounded track labels. Tags own no routing, audio or view state.
//! Text entry normalizes whitespace/case once; snapshots must already be canonical.
//! Lists are sorted and unique. Renaming a tag edits that track's value only;
//! equal labels on other tracks remain independent assignments, with no tag IDs.
use crate::{Invalid, Track};

pub const MAX_TRACK_TAGS: usize = 16;
pub const MAX_TRACK_TAG_BYTES: usize = 64;
pub const MAX_TRACK_TAG_CHARS: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TrackTag(String);
impl TrackTag {
    /// Human entry accepts surrounding/repeated whitespace and case variants.
    /// Commas separate tags in the editor and cannot occur inside one label.
    pub fn new(value: &str) -> Result<Self, Invalid> {
        if value.len() > MAX_TRACK_TAG_BYTES * 4
            || value.chars().any(|c| c.is_control() || c == ',')
        {
            return Err(Invalid(
                "tag must be short text without control characters or commas",
            ));
        }
        let normalized = value
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        if normalized.is_empty()
            || normalized.len() > MAX_TRACK_TAG_BYTES
            || normalized.chars().count() > MAX_TRACK_TAG_CHARS
        {
            return Err(Invalid("tag must contain 1..32 characters within 64 bytes"));
        }
        Ok(Self(normalized))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl Track {
    pub fn tags(&self) -> &[TrackTag] {
        &self.tags
    }
    pub fn has_tag(&self, tag: &TrackTag) -> bool {
        self.tags.binary_search(tag).is_ok()
    }
}

pub(crate) fn canonical_list(mut tags: Vec<TrackTag>) -> Result<Vec<TrackTag>, Invalid> {
    if tags.len() > MAX_TRACK_TAGS {
        return Err(Invalid("a track supports at most 16 tags"));
    }
    tags.sort_unstable();
    tags.dedup();
    // A caller may supply a short Vec with arbitrarily large spare capacity.
    // Retain only a fresh bounded list, not that external allocation envelope.
    let mut canonical = Vec::new();
    canonical
        .try_reserve_exact(tags.len())
        .map_err(|_| Invalid("could not reserve track tags"))?;
    canonical.extend(tags);
    Ok(canonical)
}
pub(crate) fn snapshot_list(values: Vec<String>) -> Result<Vec<TrackTag>, Invalid> {
    if values.len() > MAX_TRACK_TAGS {
        return Err(Invalid("a track supports at most 16 tags"));
    }
    let mut tags = Vec::with_capacity(values.len());
    for value in values {
        let tag = TrackTag::new(&value)?;
        if tag.as_str() != value || tags.last().is_some_and(|last| last >= &tag) {
            return Err(Invalid(
                "saved track tags must be canonical, sorted and unique",
            ));
        }
        tags.push(tag);
    }
    Ok(tags)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Created, Editor, Project};
    #[test]
    fn labels_are_bounded_and_snapshot_validation_never_normalizes_saved_data() {
        assert_eq!(TrackTag::new("  LOW   End  ").unwrap().as_str(), "low end");
        for bad in ["", "   ", "a,b", "a\nb", "\0"] {
            assert!(TrackTag::new(bad).is_err());
        }
        assert!(TrackTag::new(&"a".repeat(33)).is_err());
        assert!(TrackTag::new(&"界".repeat(22)).is_err());
        let mut overreserved = Vec::with_capacity(4096);
        overreserved.push(TrackTag::new("drums").unwrap());
        let bounded = canonical_list(overreserved).unwrap();
        assert_eq!(bounded[0].as_str(), "drums");
        assert!(bounded.capacity() <= MAX_TRACK_TAGS);
        for bad in [
            vec!["Drums".into()],
            vec!["drums".into(), "drums".into()],
            vec!["z".into(), "a".into()],
        ] {
            assert!(snapshot_list(bad).is_err());
        }
    }
    #[test]
    fn assignments_survive_history_snapshot_and_duplication_without_changing_other_tracks() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "drums".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let original = editor.project().clone();
        editor
            .execute(Command::SetTrackTags {
                track,
                tags: vec![
                    TrackTag::new("Drums").unwrap(),
                    TrackTag::new("lowend").unwrap(),
                    TrackTag::new("drums").unwrap(),
                ],
            })
            .unwrap();
        let tagged = editor.project().clone();
        assert_eq!(tagged.track(track).unwrap().tags().len(), 2);
        assert_eq!(Project::from_snapshot(tagged.snapshot()).unwrap(), tagged);
        assert!(editor.undo());
        assert_eq!(editor.project(), &original);
        assert!(editor.redo());
        assert_eq!(editor.project(), &tagged);
        editor
            .execute(Command::DuplicateTracks {
                tracks: crate::TrackSelectionIds::new(&[track]).unwrap(),
                without_clips: true,
            })
            .unwrap();
        assert_eq!(
            editor.project().tracks()[1].tags(),
            tagged.track(track).unwrap().tags()
        );
        let copy = editor.project().tracks()[1].id();
        editor
            .execute(Command::SetTrackTags {
                track: copy,
                tags: Vec::new(),
            })
            .unwrap();
        assert_eq!(
            editor.project().track(track).unwrap().tags(),
            tagged.track(track).unwrap().tags()
        );
        let before = editor.project().clone();
        assert!(
            editor
                .execute(Command::SetTrackTags {
                    track: copy,
                    tags: vec![TrackTag::new("x").unwrap(); 17]
                })
                .is_err()
        );
        assert_eq!(editor.project(), &before);
    }
}
