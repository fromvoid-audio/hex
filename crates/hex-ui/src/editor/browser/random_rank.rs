//! Select an eligible audio entry by rank in the current filtered projection.
use hex_library::{EntryKind, LibraryEntry};

pub(super) fn audio_index(entries: &[LibraryEntry], rank: usize) -> Option<usize> {
    entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.kind == EntryKind::WaveAudio)
        .nth(rank)
        .map(|(index, _)| index)
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::audio_index;
    use hex_library::{EntryKind, LibraryEntry};
    use std::path::PathBuf;

    #[test]
    fn audio_rank_uses_the_filtered_projection_and_excludes_other_kinds() {
        let entries: Vec<_> = [
            EntryKind::Directory,
            EntryKind::WaveAudio,
            EntryKind::Project,
            EntryKind::WaveAudio,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, kind)| LibraryEntry {
            name: i.to_string(),
            path: PathBuf::from(i.to_string()),
            kind,
        })
        .collect();
        assert_eq!(audio_index(&entries, 0), Some(1));
        assert_eq!(audio_index(&entries, 1), Some(3));
        assert_eq!(audio_index(&entries, 2), None);
        assert_eq!(audio_index(&entries[2..], 0), Some(1));
        assert_eq!(audio_index(&entries[..1], 0), None);
        assert_eq!(audio_index(&[], 0), None);
    }
}
