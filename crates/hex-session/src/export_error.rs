//! Case-preserving presentation of export recovery paths in session messages.

/// Format an export failure for app display while preserving a staging path's case.
pub fn export_error_text(error: &hex_export::ExportError) -> String {
    match error {
        hex_export::ExportError::StagingRetained { path, cause } => format!(
            "{}; inspect the staging path at {} before removing anything",
            export_error_text(cause),
            hex_storage::text::verbatim(path.display())
        ),
        hex_export::ExportError::IncompleteDestination { path, source } => format!(
            "could not complete the wav copy: {source}; inspect the destination path at {} before removing anything",
            hex_storage::text::verbatim(path.display())
        ),
        _ => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io, path::PathBuf};

    #[test]
    fn incomplete_export_copy_preserves_both_path_cases_and_cause() {
        let error = hex_export::ExportError::StagingRetained {
            path: PathBuf::from("/MixedCase/Stage.wav"),
            cause: Box::new(hex_export::ExportError::IncompleteDestination {
                path: PathBuf::from("/MixedCase/Output.wav"),
                source: io::Error::other("write failed"),
            }),
        };
        let text = hex_storage::text::plain(&hex_storage::text::lowercase_outside_verbatim(
            &export_error_text(&error),
        ));
        assert!(text.contains("/MixedCase/Stage.wav"));
        assert!(text.contains("/MixedCase/Output.wav"));
        assert!(text.contains("write failed"));
    }
}
