use super::relinquish_client_matches;

#[test]
fn targeted_release_ignores_newer_authoring_other_output_and_other_document() {
    let context = hex_engine::AudioSourceContext {
        project_generation: 4,
        editor_revision: 7,
    };
    assert!(relinquish_client_matches(4, Some(6), Some(12), 12, context));
    assert!(!relinquish_client_matches(
        4,
        Some(8),
        Some(12),
        12,
        context
    ));
    assert!(!relinquish_client_matches(
        4,
        Some(7),
        Some(13),
        12,
        context
    ));
    assert!(!relinquish_client_matches(
        5,
        Some(7),
        Some(12),
        12,
        context
    ));
}
