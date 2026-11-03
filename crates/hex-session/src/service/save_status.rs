//! Status text for a save whose project bytes were published successfully.
//!
//! A replacement save may be unable to carry some extended attributes (such as
//! Finder tags) from the replaced file onto the new one; hex-storage reports
//! them as [`MetadataWarning`]s. They are non-fatal: the project is saved and
//! the document stays clean, so the warning is part of the success status rather
//! than an error. The status leads with `project saved`, names the loss in one
//! short clause for the truncated status bar, and appends a bounded detail of
//! attribute names and causes that the status tooltip shows in full. Attribute
//! names are case-sensitive OS identifiers and are wrapped as verbatim text.
//! Durability warnings take precedence and are handled by the caller.
use hex_storage::MetadataWarning;

/// The status of a clean, completed save.
pub(super) const SAVED: &str = "project saved";

/// The macOS extended attribute that stores Finder tags.
const FINDER_TAGS: &str = "com.apple.metadata:_kMDItemUserTags";

/// Detailed entries listed before the remainder is summarized as a count.
const MAX_DETAILED: usize = 3;

/// Returns the status for a successful save with these metadata warnings.
pub(super) fn saved_status(metadata_warnings: &[MetadataWarning]) -> String {
    if metadata_warnings.is_empty() {
        return SAVED.to_owned();
    }
    let finder_tags = metadata_warnings
        .iter()
        .any(|warning| warning.attribute.as_deref() == Some(FINDER_TAGS));
    let headline = if finder_tags {
        "finder tags could not be kept"
    } else {
        "some file attributes could not be kept"
    };
    let mut details = metadata_warnings
        .iter()
        .take(MAX_DETAILED)
        .map(|warning| match &warning.attribute {
            Some(name) => format!(
                "could not {} {}: {}",
                warning.operation,
                hex_storage::text::verbatim(name),
                warning.source
            ),
            None => format!("could not {}: {}", warning.operation, warning.source),
        })
        .collect::<Vec<_>>();
    let remaining = metadata_warnings.len().saturating_sub(MAX_DETAILED);
    if remaining > 0 {
        details.push(format!("and {remaining} more"));
    }
    format!("{SAVED} · {headline} · {}", details.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_storage::text;
    use std::io;

    fn warning(attribute: Option<&str>, operation: &'static str) -> MetadataWarning {
        MetadataWarning {
            attribute: attribute.map(str::to_owned),
            operation,
            source: io::Error::other("Permission denied"),
        }
    }

    #[test]
    fn no_warnings_keep_the_plain_saved_status() {
        assert_eq!(saved_status(&[]), "project saved");
    }

    #[test]
    fn finder_tag_loss_is_named_with_its_attribute_and_cause() {
        let status = saved_status(&[warning(Some(FINDER_TAGS), "write a file attribute")]);
        assert!(status.starts_with("project saved · finder tags could not be kept · "));
        assert_eq!(
            text::plain(&text::lowercase_outside_verbatim(&status)),
            "project saved · finder tags could not be kept · could not write a file \
             attribute com.apple.metadata:_kMDItemUserTags: permission denied"
        );
    }

    #[test]
    fn other_attributes_and_list_failures_use_the_generic_headline() {
        let status = saved_status(&[
            warning(Some("user.Custom"), "read a file attribute"),
            warning(None, "list file attributes"),
        ]);
        assert_eq!(
            text::plain(&status),
            "project saved · some file attributes could not be kept · could not read a file \
             attribute user.Custom: Permission denied; could not list file attributes: \
             Permission denied"
        );
    }

    #[test]
    fn detail_is_bounded_and_counts_the_remainder() {
        let warnings = (0..5)
            .map(|_| warning(Some("user.a"), "write a file attribute"))
            .collect::<Vec<_>>();
        let status = text::plain(&saved_status(&warnings));
        assert_eq!(status.matches("could not write").count(), MAX_DETAILED);
        assert!(status.ends_with("; and 2 more"), "{status}");
    }
}
