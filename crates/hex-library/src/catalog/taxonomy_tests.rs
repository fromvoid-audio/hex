//! Focused tests for hierarchical tag taxonomy, hexsmp4 persistence, and group filtering (#2665).

use super::*;
use crate::boundary::{CatalogLibraryAdapter, LibraryBoundary};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hex-taxonomy-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn hexsmp4_persists_hierarchy_migrates_v3_and_evaluates_group_and_descendant_filters() {
    let dir = temp_dir("hierarchy");
    let kick = dir.join("kick.wav");
    let snare = dir.join("snare.wav");
    let pad = dir.join("pad.wav");
    let mut catalog = SampleCatalog {
        entries: vec![
            LibraryEntry {
                name: "kick.wav".into(),
                path: kick.clone(),
                kind: EntryKind::WaveAudio,
            },
            LibraryEntry {
                name: "snare.wav".into(),
                path: snare.clone(),
                kind: EntryKind::WaveAudio,
            },
            LibraryEntry {
                name: "pad.wav".into(),
                path: pad.clone(),
                kind: EntryKind::WaveAudio,
            },
        ],
        ..SampleCatalog::default()
    };
    catalog.rebuild_search_index();

    // Save v3 without taxonomy, verify v3 magic and lossless reopen.
    catalog.add_tag(&kick, "808").unwrap();
    catalog.add_tag(&kick, "punchy").unwrap();
    catalog.add_tag(&snare, "acoustic").unwrap();
    catalog.add_tag(&snare, "punchy").unwrap();
    catalog.add_tag(&pad, "warm").unwrap();
    let v3_path = dir.join("v3.db");
    catalog.save(&v3_path).unwrap();
    assert_eq!(&std::fs::read(&v3_path).unwrap()[..7], b"hexsmp3");
    let mut upgraded = SampleCatalog::load(&v3_path).unwrap();
    assert_eq!(upgraded, catalog);

    // Define hierarchy: drums -> {808, acoustic}; character -> {punchy, warm}.
    upgraded.define_tag("drums", None).unwrap();
    upgraded.define_tag("808", Some("drums")).unwrap();
    upgraded.define_tag("acoustic", Some("drums")).unwrap();
    upgraded.define_tag("character", None).unwrap();
    upgraded.define_tag("punchy", Some("character")).unwrap();
    upgraded.define_tag("warm", Some("character")).unwrap();

    // Reject self-parent, cycle, missing parent, and depth > 8.
    assert!(upgraded.define_tag("drums", Some("drums")).is_err());
    assert!(upgraded.define_tag("drums", Some("808")).is_err());
    assert!(upgraded.define_tag("orphan", Some("missing")).is_err());
    let mut deep_parent = "drums".to_string();
    for depth in 1..=crate::boundary::MAX_TAXONOMY_DEPTH {
        let name = format!("level{depth}");
        upgraded.define_tag(&name, Some(&deep_parent)).unwrap();
        deep_parent = name;
    }
    assert!(upgraded.define_tag("too_deep", Some(&deep_parent)).is_err());
    for depth in (1..=crate::boundary::MAX_TAXONOMY_DEPTH).rev() {
        upgraded.delete_tag(&format!("level{depth}"));
    }

    // Save as hexsmp4 and reopen cleanly, including adapter hydration.
    let v4_path = dir.join("v4.db");
    upgraded.save(&v4_path).unwrap();
    assert_eq!(&std::fs::read(&v4_path).unwrap()[..7], b"hexsmp4");
    let reopened = SampleCatalog::load(&v4_path).unwrap();
    assert_eq!(reopened, upgraded);

    let adapter =
        CatalogLibraryAdapter::from_catalog(reopened.clone(), std::slice::from_ref(&dir), None);
    let drums_id = adapter.tag_id_for_name("drums").unwrap();
    let tag_808_id = adapter.tag_id_for_name("808").unwrap();
    assert!(adapter.descendant_tags(drums_id).contains(&tag_808_id));

    // Hierarchy descendant query: `tag: "drums"` matches BOTH kick (808) and snare (acoustic)!
    let drums_matches = reopened.query(&CatalogQuery {
        tag: Some("drums"),
        ..CatalogQuery::default()
    });
    assert_eq!(drums_matches.len(), 2);

    // Group query: (drums OR warm) AND (punchy) -> matches kick & snare only (excludes pad).
    let groups = vec![
        vec!["drums".to_owned(), "warm".to_owned()],
        vec!["punchy".to_owned()],
    ];
    let combined = reopened.query(&CatalogQuery {
        tag_groups: &groups,
        ..CatalogQuery::default()
    });
    assert_eq!(combined.len(), 2);
    assert!(combined.iter().all(|e| e.path != pad));

    // Rename, merge, delete, and collection rename preserve integrity.
    let mut edited = reopened;
    edited.rename_tag("808", "sub808").unwrap();
    assert!(edited.tags_for(&kick).unwrap().contains("sub808"));
    assert_eq!(
        edited
            .tag_taxonomy()
            .get("sub808")
            .and_then(|p| p.as_deref()),
        Some("drums")
    );
    assert!(edited.merge_tag("drums", "sub808").is_err());
    edited.merge_tag("sub808", "acoustic").unwrap();
    assert!(edited.tags_for(&kick).unwrap().contains("acoustic"));
    edited.delete_tag("drums");
    assert_eq!(
        edited
            .tag_taxonomy()
            .get("acoustic")
            .and_then(|p| p.as_deref()),
        None
    );

    edited.add_to_collection("favs", &kick).unwrap();
    edited.rename_collection("favs", "gold").unwrap();
    assert_eq!(edited.collections(), vec!["gold".to_string()]);
    assert!(edited.collection_members("gold").unwrap().contains(&kick));

    // Malformed / truncated hexsmp4 is refused without touching original bytes.
    let mut corrupt = std::fs::read(&v4_path).unwrap();
    corrupt.truncate(corrupt.len() - 3);
    let corrupt_path = dir.join("corrupt.db");
    std::fs::write(&corrupt_path, &corrupt).unwrap();
    assert!(SampleCatalog::load(&corrupt_path).is_err());
    assert_eq!(std::fs::read(&corrupt_path).unwrap(), corrupt);

    let _ = std::fs::remove_dir_all(&dir);
}
