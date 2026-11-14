//! Case-preserving presentation of export recovery paths in session messages.

/// Format an export failure for app display while preserving a staging path's case.
pub fn export_error_text(error: &hex_export::ExportError) -> String {
    match error {
        hex_export::ExportError::StagingRetained { path, cause } => format!(
            "{}; inspect the staging path at {} before removing anything",
            export_error_text(cause),
            hex_storage::text::verbatim(path.display())
        ),
        _ => error.to_string(),
    }
}
