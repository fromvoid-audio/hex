//! Case-preserving presentation of export recovery paths in session messages.

/// An export failure and the exact underlying recovery paths, when provided by
/// the publisher. Display text is kept separately so context can be added
/// without discarding the original failure.
#[derive(Debug)]
pub struct ExportFailure {
    message: String,
    origin: ExportFailureOrigin,
}

#[derive(Debug)]
enum ExportFailureOrigin {
    Publisher(Box<hex_export::ExportError>),
    Published(Box<hex_export::ExportReport>),
    Message,
}

impl ExportFailure {
    /// Display text, with recovery path case preserved.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The publisher failure, including any native recovery paths.
    pub fn source_error(&self) -> Option<&hex_export::ExportError> {
        match &self.origin {
            ExportFailureOrigin::Publisher(error) => Some(error),
            _ => None,
        }
    }

    /// A successful publication whose control-custody notification failed.
    pub fn published_report(&self) -> Option<&hex_export::ExportReport> {
        match &self.origin {
            ExportFailureOrigin::Published(report) => Some(report),
            _ => None,
        }
    }

    pub(super) fn notification_after_publication(
        report: hex_export::ExportReport,
        message: &str,
    ) -> Self {
        let mut displayed = format!(
            "export publication completed; inspect the destination path at {}",
            hex_storage::text::verbatim(report.path.display())
        );
        if let Some(recovery) = &report.recovery_path {
            displayed.push_str(&format!(
                "; inspect the previous-file recovery path at {} before removing anything",
                hex_storage::text::verbatim(recovery.display())
            ));
        }
        displayed.push_str("; ");
        displayed.push_str(message);
        Self {
            message: displayed,
            origin: ExportFailureOrigin::Published(Box::new(report)),
        }
    }

    pub(super) fn append_context(&mut self, context: &str) {
        self.message.push_str("; ");
        self.message.push_str(context);
    }
}

impl From<hex_export::ExportError> for ExportFailure {
    fn from(source: hex_export::ExportError) -> Self {
        Self {
            message: export_error_text(&source),
            origin: ExportFailureOrigin::Publisher(Box::new(source)),
        }
    }
}

impl From<String> for ExportFailure {
    fn from(message: String) -> Self {
        Self {
            message,
            origin: ExportFailureOrigin::Message,
        }
    }
}

impl From<&str> for ExportFailure {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

impl std::fmt::Display for ExportFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ExportFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source_error().map(|error| error as _)
    }
}

/// Format an export failure for app display while preserving a staging path's case.
pub fn export_error_text(error: &hex_export::ExportError) -> String {
    match error {
        hex_export::ExportError::StagingRetained { path, cause } => format!(
            "{}; inspect the staging path at {} before removing anything",
            export_error_text(cause),
            hex_storage::text::verbatim(path.display())
        ),
        hex_export::ExportError::PublicationConflict {
            path,
            recovery_path,
        } => format!(
            "export publication raced at {}; inspect the retained file at {}",
            hex_storage::text::verbatim(path.display()),
            hex_storage::text::verbatim(recovery_path.display())
        ),
        hex_export::ExportError::PublishedButNotDurable {
            path,
            recovery_path,
            operation,
            source,
        } => {
            let mut message = format!(
                "wav was published at {} but could not {operation}: {source}",
                hex_storage::text::verbatim(path.display())
            );
            if let Some(recovery_path) = recovery_path {
                message.push_str(&format!(
                    "; previous file is at {}",
                    hex_storage::text::verbatim(recovery_path.display())
                ));
            }
            message
        }
        hex_export::ExportError::IncompleteDestination { path, source } => format!(
            "could not complete the wav copy: {source}; the destination path at {} may contain a partial export or another file; inspect it before removing anything",
            hex_storage::text::verbatim(path.display())
        ),
        _ => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io, path::PathBuf};

    fn visible(error: &hex_export::ExportError) -> String {
        hex_storage::text::plain(&hex_storage::text::lowercase_outside_verbatim(
            &export_error_text(error),
        ))
    }

    #[test]
    fn publication_conflict_keeps_destination_and_recovery_case() {
        let error = hex_export::ExportError::PublicationConflict {
            path: PathBuf::from("/Mixed\u{2069}Case/Output.WAV"),
            recovery_path: PathBuf::from("/Mixed\u{2063}Case/Previous.WAV"),
        };
        assert_eq!(
            visible(&error),
            "export publication raced at /Mixed\u{2069}Case/Output.WAV; inspect the retained file at /Mixed\u{2063}Case/Previous.WAV"
        );
    }

    #[test]
    fn durability_errors_keep_all_paths_and_cause_in_nested_stage() {
        for has_recovery in [false, true] {
            let error = hex_export::ExportError::StagingRetained {
                path: PathBuf::from("/Mixed\u{2068}Case/Stage.WAV"),
                cause: Box::new(hex_export::ExportError::PublishedButNotDurable {
                    path: PathBuf::from("/Mixed\u{2069}Case/Output.WAV"),
                    recovery_path: has_recovery
                        .then(|| PathBuf::from("/Mixed\u{2063}Case/Previous.WAV")),
                    operation: "synchronize the destination directory",
                    source: io::Error::other("sync failed"),
                }),
            };
            let expected_recovery = if has_recovery {
                "; previous file is at /Mixed\u{2063}Case/Previous.WAV"
            } else {
                ""
            };
            assert_eq!(
                visible(&error),
                format!(
                    "wav was published at /Mixed\u{2069}Case/Output.WAV but could not synchronize the destination directory: sync failed{expected_recovery}; inspect the staging path at /Mixed\u{2068}Case/Stage.WAV before removing anything"
                )
            );
        }
    }

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
        assert!(text.contains("may contain a partial export or another file"));
    }
}
