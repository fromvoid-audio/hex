
#[test]
fn place_correctness_listing_uses_indexed_terms_and_full_native_paths() {
    let fixture = Fixture::new();
    let root = fixture.0.join("kick");
    let wave = fixture.file("kick/drums.wav");
    let project = fixture.file("kick/drums.hex");
    let foreign = fixture.file("kick-other/drums.wav");
    let catalog = scan_samples(std::slice::from_ref(&fixture.0)).catalog;
    let listing = crate::scan_directory(&root).unwrap();
    for query in ["kick drums", "DRUMS  KICK", "  kick\t drums  "] {
        let scoped = catalog.search_with_listing_in(query, Some(&root), Some(&listing));
        assert_eq!(scoped.len(), 2, "indexed and current-folder semantics differ for {query:?}");
        assert!(scoped.iter().any(|entry| entry.path == wave));
        assert!(scoped.iter().any(|entry| entry.path == project));
        assert!(!scoped.iter().any(|entry| entry.path == foreign));
        let all = catalog.search_with_listing(query, Some(&listing));
        assert_eq!(all.len(), 2, "existing All index semantics must remain intact");
        assert!(all.iter().any(|entry| entry.path == wave));
        assert!(all.iter().any(|entry| entry.path == foreign));
    }
}
