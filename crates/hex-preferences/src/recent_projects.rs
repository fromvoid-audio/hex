//! Independent bounded recent-project history preferences.
use crate::storage::{
    DOCUMENT_VERSION, MAX_DOCUMENT_BYTES, MAX_TOTAL_PATH_BYTES, PreferencesError, StoredPath,
    VersionedDocument, app_support_dir_from, load_versioned, native_path_len, save_versioned,
    total_native_path_bytes, validate_saved_path,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

pub const MAX_RECENT_PROJECTS: usize = 10;

/// Stable app-support location for the independent recent-project history.
pub fn recent_projects_path(home: &Path) -> PathBuf {
    app_support_dir_from(home).join("recent-projects.json")
}

/// Recently opened project files, most recent first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecentProjects {
    paths: Vec<PathBuf>,
}

impl RecentProjects {
    /// Build an ordered list, deduplicating exact native paths and rejecting
    /// empty, NUL-bearing, malformed, or excessive saved data rather than truncating it.
    pub fn new(paths: impl IntoIterator<Item = PathBuf>) -> Result<Self, PreferencesError> {
        let mut recent = Vec::new();
        let mut seen = HashSet::new();
        let mut total_path_bytes = 0_usize;
        for path in paths {
            validate_saved_path(&path)?;
            if seen.insert(path.clone()) {
                total_path_bytes = total_path_bytes.checked_add(native_path_len(&path)).ok_or(
                    PreferencesError::DocumentTooLarge {
                        maximum: MAX_DOCUMENT_BYTES,
                    },
                )?;
                if total_path_bytes > MAX_TOTAL_PATH_BYTES {
                    return Err(PreferencesError::DocumentTooLarge {
                        maximum: MAX_DOCUMENT_BYTES,
                    });
                }
                if recent.len() == MAX_RECENT_PROJECTS {
                    return Err(PreferencesError::TooManyRecentProjects {
                        maximum: MAX_RECENT_PROJECTS,
                    });
                }
                recent.push(path);
            }
        }
        Ok(Self { paths: recent })
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// Promote a successfully opened path to the front and evict the oldest
    /// item when the bounded menu is full.
    pub fn record_opened(&mut self, path: PathBuf) -> Result<bool, PreferencesError> {
        validate_saved_path(&path)?;
        if self.paths.first() == Some(&path) {
            return Ok(false);
        }
        self.paths.retain(|item| item != &path);
        self.paths.insert(0, path);
        self.paths.truncate(MAX_RECENT_PROJECTS);
        while total_native_path_bytes(&self.paths) > MAX_TOTAL_PATH_BYTES {
            self.paths.pop();
        }
        Ok(true)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecentProjectsDocument {
    version: u32,
    paths: Vec<StoredPath>,
}

impl VersionedDocument for RecentProjectsDocument {
    fn version(&self) -> u32 {
        self.version
    }
}

/// Load recent project paths. Missing settings are empty; malformed files are
/// returned as errors without modification.
pub fn load_recent_projects(path: &Path) -> Result<RecentProjects, PreferencesError> {
    load_versioned::<RecentProjectsDocument, _>(path, RecentProjects::default, |document| {
        let paths = document
            .paths
            .into_iter()
            .map(StoredPath::into_path)
            .collect::<Result<Vec<_>, _>>()?;
        RecentProjects::new(paths)
    })
}

/// Atomically save the recent-project document without replacing malformed or
/// future-version settings.
pub fn save_recent_projects(path: &Path, recent: &RecentProjects) -> Result<(), PreferencesError> {
    let paths = recent
        .paths
        .iter()
        .map(|path| {
            validate_saved_path(path)?;
            StoredPath::from_path(path)
        })
        .collect::<Result<Vec<_>, PreferencesError>>()?;
    let document = RecentProjectsDocument {
        version: DOCUMENT_VERSION,
        paths,
    };
    save_versioned(
        path,
        |existing| load_recent_projects(existing).map(|_| ()),
        &document,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::TestDirectory;
    use std::fs;

    #[test]
    fn recent_projects_promote_exact_paths_deduplicate_and_evict_oldest() {
        let mut recent = RecentProjects::default();
        for index in 0..=MAX_RECENT_PROJECTS {
            assert!(
                recent
                    .record_opened(PathBuf::from(format!("/project-{index}.hex")))
                    .expect("record opened path")
            );
        }
        assert_eq!(recent.paths().len(), MAX_RECENT_PROJECTS);
        assert_eq!(recent.paths()[0], PathBuf::from("/project-10.hex"));
        assert_eq!(
            recent.paths()[MAX_RECENT_PROJECTS - 1],
            PathBuf::from("/project-1.hex")
        );
        assert!(
            recent
                .record_opened(PathBuf::from("/project-5.hex"))
                .expect("promote existing path")
        );
        assert_eq!(recent.paths()[0], PathBuf::from("/project-5.hex"));
        assert_eq!(recent.paths().len(), MAX_RECENT_PROJECTS);
        assert!(
            !recent
                .record_opened(PathBuf::from("/project-5.hex"))
                .expect("same most-recent path is unchanged")
        );
        assert_eq!(recent.paths()[0], PathBuf::from("/project-5.hex"));
        assert!(matches!(
            RecentProjects::new([PathBuf::from("/bad\0project.hex")]),
            Err(PreferencesError::InvalidDocument(_))
        ));
        assert!(matches!(
            recent.record_opened(PathBuf::from("/bad\0project.hex")),
            Err(PreferencesError::InvalidDocument(_))
        ));
    }

    #[test]
    fn recent_project_promotion_evicts_oldest_to_keep_combined_path_bytes_bounded() {
        let long_a = PathBuf::from(format!("/{}", "a".repeat(4_090)));
        let long_b = PathBuf::from(format!("/{}", "b".repeat(4_090)));
        let mut recent = RecentProjects::new([long_a.clone(), PathBuf::from("/short.hex")])
            .expect("initial recent projects");
        recent
            .record_opened(long_b.clone())
            .expect("promote valid long path");
        assert_eq!(
            recent.paths(),
            &[long_b.clone(), long_a.clone(), PathBuf::from("/short.hex")]
        );
        recent
            .record_opened(PathBuf::from("/new.hex"))
            .expect("promote short path");
        assert_eq!(recent.paths(), &[PathBuf::from("/new.hex"), long_b, long_a]);
    }

    #[test]
    fn recent_projects_round_trip_paths_without_requiring_files_to_exist() {
        let directory = TestDirectory::new("hex-recent-projects");
        let config = directory.0.join("recent-projects.json");
        let recent = RecentProjects::new([
            PathBuf::from("/offline/session.hex"),
            PathBuf::from("/other/session.hex"),
        ])
        .expect("recent projects");
        save_recent_projects(&config, &recent).expect("save recent projects");
        assert_eq!(load_recent_projects(&config).expect("load recents"), recent);
    }

    #[cfg(unix)]
    #[test]
    fn recent_projects_preserve_non_utf8_path_bytes() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let directory = TestDirectory::new("hex-recent-projects");
        let config = directory.0.join("recent-projects.json");
        let path = PathBuf::from(OsString::from_vec(b"/tmp/session-\xff.hex".to_vec()));
        let recent = RecentProjects::new([path]).expect("native path");
        save_recent_projects(&config, &recent).expect("save native path");
        assert_eq!(
            load_recent_projects(&config).expect("load native path"),
            recent
        );
    }

    #[test]
    fn recent_projects_refuse_to_overwrite_malformed_document() {
        let directory = TestDirectory::new("hex-recent-projects");
        let config = directory.0.join("recent-projects.json");
        let corrupt = b"not json";
        fs::write(&config, corrupt).expect("write malformed document");
        let mut recent = RecentProjects::default();
        recent
            .record_opened(PathBuf::from("/opened.hex"))
            .expect("record path");
        assert!(matches!(
            save_recent_projects(&config, &recent),
            Err(PreferencesError::InvalidDocument(_))
        ));
        assert_eq!(fs::read(&config).expect("read preserved document"), corrupt);
    }
}
