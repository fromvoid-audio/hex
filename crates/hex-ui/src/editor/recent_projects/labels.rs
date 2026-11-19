//! Stable, lowercase labels for recent project menu entries.
use std::path::{Path, PathBuf};

pub(in crate::editor) fn recent_project_label(path: &Path, all_paths: &[PathBuf]) -> String {
    let raw_file_name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    let file_name = raw_file_name.to_lowercase();
    let duplicate_name = all_paths
        .iter()
        .filter(|other| {
            other
                .file_name()
                .unwrap_or(other.as_os_str())
                .to_string_lossy()
                .eq_ignore_ascii_case(&raw_file_name)
        })
        .count()
        > 1;
    if duplicate_name {
        let parent = path
            .parent()
            .unwrap_or(Path::new(""))
            .to_string_lossy()
            .to_lowercase();
        format!("{file_name} · {parent}")
    } else {
        file_name
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::recent_project_label;
    use std::path::PathBuf;

    #[test]
    fn duplicate_filename_labels_include_lowercase_parent_path() {
        let paths = [
            PathBuf::from("/Work/Live/Session.HEX"),
            PathBuf::from("/Archive/Session.HEX"),
        ];
        assert_eq!(
            recent_project_label(&paths[0], &paths),
            "session.hex · /work/live"
        );
    }

    #[test]
    fn duplicate_unicode_filenames_include_each_parent() {
        let paths = [
            PathBuf::from("/Work/Ärger.hex"),
            PathBuf::from("/Archive/Ärger.hex"),
        ];
        assert_eq!(recent_project_label(&paths[0], &paths), "ärger.hex · /work");
        assert_eq!(
            recent_project_label(&paths[1], &paths),
            "ärger.hex · /archive"
        );
    }
}
