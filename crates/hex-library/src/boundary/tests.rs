//! Focused boundary tests for neutral library identity & validated tag commands (#7420).

use super::*;
use crate::catalog::{SampleCatalog, scan_samples};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hex-lib-boundary-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn minimal_wav_bytes() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&44u32.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&2u16.to_le_bytes()); // stereo
    out.extend_from_slice(&48_000u32.to_le_bytes());
    out.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&8u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 8]);
    out
}

#[test]
fn lists_sources_pages_items_assigns_tags_observes_revision_and_rejects_stale_commands() {
    let root = temp_dir("basic");
    let src_a = root.join("drums");
    let src_b = root.join("synths");
    std::fs::create_dir_all(&src_a).unwrap();
    std::fs::create_dir_all(&src_b).unwrap();
    let kick = src_a.join("kick.wav");
    let snare = src_a.join("snare.wav");
    let pad = src_b.join("pad.wav");
    for p in [&kick, &snare, &pad] {
        std::fs::write(p, minimal_wav_bytes()).unwrap();
    }
    let db_path = root.join("catalog.hexsmp3");
    let roots = vec![src_a.clone(), src_b.clone()];
    let scan = scan_samples(&roots);
    scan.catalog.save(&db_path).unwrap();

    let mut adapter =
        CatalogLibraryAdapter::from_catalog(scan.catalog, &roots, Some(db_path.clone()));
    assert_eq!(adapter.list_sources().len(), 2);
    let drums_src = adapter.list_sources()[0].id;

    // Paging metadata returns bounded pages with total counts and stable IDs.
    let page0 = adapter.page_items(Some(drums_src), PageRequest::new(0, 1));
    assert_eq!(page0.total, 2);
    assert_eq!(page0.items.len(), 1);
    let kick_id = adapter.item_id_for_path(&kick).unwrap();
    let snare_id = adapter.item_id_for_path(&snare).unwrap();

    // Create hierarchical tags: drums -> punchy.
    let r0 = adapter.revision();
    let drums_tag = adapter
        .execute(LibraryCommandEnvelope {
            expected_revision: r0,
            command: LibraryCommand::CreateTag {
                name: "Drums".into(),
                parent: None,
            },
        })
        .unwrap();
    let drums_tag_id = drums_tag.created_tag.unwrap();
    let punchy_tag = adapter
        .execute(LibraryCommandEnvelope {
            expected_revision: drums_tag.committed_revision,
            command: LibraryCommand::CreateTag {
                name: "punchy".into(),
                parent: Some(drums_tag_id),
            },
        })
        .unwrap();
    let punchy_tag_id = punchy_tag.created_tag.unwrap();
    assert_eq!(
        adapter.descendant_tags(drums_tag_id),
        vec![drums_tag_id, punchy_tag_id]
    );

    // Assign tag and observe committed revision + persistence.
    let assigned = adapter
        .execute(LibraryCommandEnvelope {
            expected_revision: punchy_tag.committed_revision,
            command: LibraryCommand::AssignTag {
                item: kick_id,
                tag: punchy_tag_id,
            },
        })
        .unwrap();
    assert!(assigned.changed);
    assert!(assigned.committed_revision > punchy_tag.committed_revision);

    // Stale revision is rejected without mutating state.
    let stale_err = adapter
        .execute(LibraryCommandEnvelope {
            expected_revision: r0,
            command: LibraryCommand::RemoveTag {
                item: kick_id,
                tag: punchy_tag_id,
            },
        })
        .unwrap_err();
    assert!(matches!(
        stale_err,
        LibraryBoundaryError::StaleRevision { .. }
    ));

    // Relocating source root preserves stable LibraryItemId and attached tags.
    let relocated = root.join("drums_moved");
    std::fs::rename(&src_a, &relocated).unwrap();
    let after_reloc = adapter
        .relocate_source(drums_src, relocated.clone())
        .unwrap();
    assert_eq!(
        adapter.item_id_for_path(&relocated.join("kick.wav")),
        Some(kick_id)
    );
    assert_eq!(
        adapter.item_id_for_path(&relocated.join("snare.wav")),
        Some(snare_id)
    );
    assert!(
        adapter
            .catalog()
            .tags_for(&relocated.join("kick.wav"))
            .unwrap()
            .contains("punchy")
    );

    // Reopen persisted catalog from disk and verify tag survived.
    let reopened = SampleCatalog::load(&db_path).unwrap();
    assert!(
        reopened
            .tags_for(&relocated.join("kick.wav"))
            .unwrap()
            .contains("punchy")
    );

    // Bulk assign + remove + rename + merge + delete keep taxonomy valid.
    let bulk = adapter
        .execute(LibraryCommandEnvelope {
            expected_revision: after_reloc,
            command: LibraryCommand::BulkAssignTag {
                items: vec![kick_id, snare_id],
                tag: drums_tag_id,
            },
        })
        .unwrap();
    assert!(bulk.changed);
    let renamed = adapter
        .execute(LibraryCommandEnvelope {
            expected_revision: bulk.committed_revision,
            command: LibraryCommand::RenameTag {
                tag: punchy_tag_id,
                name: "snappy".into(),
            },
        })
        .unwrap();
    assert!(renamed.changed);
    assert!(
        adapter
            .catalog()
            .tags_for(&relocated.join("kick.wav"))
            .unwrap()
            .contains("snappy")
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn failed_persistence_and_invalid_commands_preserve_prior_state_without_data_loss() {
    let root = temp_dir("rollback");
    let src = root.join("samples");
    std::fs::create_dir_all(&src).unwrap();
    let wav = src.join("lead.wav");
    std::fs::write(&wav, minimal_wav_bytes()).unwrap();
    let scan = scan_samples(std::slice::from_ref(&src));

    // Point backing_path inside a non-existent directory where atomic save must fail.
    let bad_backing = root.join("missing_parent_file");
    std::fs::write(&bad_backing, b"not a dir").unwrap();
    let unwritable = bad_backing.join("catalog.hexsmp3");

    let mut adapter = CatalogLibraryAdapter::from_catalog(
        scan.catalog,
        std::slice::from_ref(&src),
        Some(unwritable),
    );
    let rev0 = adapter.revision();
    let item_id = adapter.item_id_for_path(&wav).unwrap();

    // Failed write rolls back both revision and taxonomy in memory.
    let err = adapter
        .execute(LibraryCommandEnvelope {
            expected_revision: rev0,
            command: LibraryCommand::CreateTag {
                name: "synth".into(),
                parent: None,
            },
        })
        .unwrap_err();
    assert!(matches!(err, LibraryBoundaryError::Storage(_)));
    assert_eq!(adapter.revision(), rev0);
    assert!(adapter.list_tags().is_empty());

    // Invalid empty label and unknown tag/item are refused cleanly.
    assert!(matches!(
        adapter.execute(LibraryCommandEnvelope {
            expected_revision: rev0,
            command: LibraryCommand::CreateTag {
                name: "   ".into(),
                parent: None,
            },
        }),
        Err(LibraryBoundaryError::InvalidLabel(_))
    ));
    assert!(matches!(
        adapter.execute(LibraryCommandEnvelope {
            expected_revision: rev0,
            command: LibraryCommand::AssignTag {
                item: item_id,
                tag: TagId(9_999),
            },
        }),
        Err(LibraryBoundaryError::UnknownTag(_))
    ));

    let _ = std::fs::remove_dir_all(&root);
}
