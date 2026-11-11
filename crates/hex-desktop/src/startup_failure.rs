//! Fatal startup failure classification for the desktop entry point.
//!
//! Each fallible startup step is tagged with the stage that failed, so the
//! native alert shown by `hex_ui::open_or_report` pairs the verbatim error
//! (which may carry case-sensitive paths) with a lowercase recovery hint for
//! that stage. Failures before the editor opens (update custody, arguments)
//! use the same alert instead of exiting silently.

/// The startup step that failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    /// Startup options could not be parsed.
    Arguments,
    /// Shared update custody of an installed test bundle was unavailable.
    UpdateCustody,
    /// Static audio/source reservations were refused.
    AudioPolicy,
    /// The untitled recovery location could not be resolved.
    RecoveryLocation,
    /// The canonical session service could not be constructed.
    Session,
    /// Embedded assets/fonts or the editor window failed.
    Editor,
}

impl Stage {
    /// Lowercase recovery hint shown beneath the error.
    pub(crate) fn recovery(self) -> &'static str {
        match self {
            Self::Arguments => "check the command-line options, then start hex again.",
            Self::UpdateCustody => {
                "wait for the running hex update to finish, then start hex again."
            }
            Self::AudioPolicy => {
                "this hex build is inconsistent. reinstall hex or rebuild it with scripts/bundle-macos.sh."
            }
            Self::RecoveryLocation => {
                "set the home folder for your account and make sure it is writable, then start hex again."
            }
            Self::Session => {
                "make sure hex's application support folder is writable and the disk has free space, then start hex again."
            }
            Self::Editor => "reinstall hex or rebuild it with scripts/bundle-macos.sh.",
        }
    }
}

/// A fatal startup error and the step that produced it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct StartupFailure {
    stage: Stage,
    error: String,
}

impl StartupFailure {
    pub(crate) fn new(stage: Stage, error: impl Into<String>) -> Self {
        Self {
            stage,
            error: error.into(),
        }
    }

    /// Tags a step's error with its stage.
    pub(crate) fn at(stage: Stage) -> impl FnOnce(String) -> Self {
        move |error| Self::new(stage, error)
    }

    /// The verbatim error and its recovery hint, as `hex_ui::open_or_report` takes them.
    pub(crate) fn into_report(self) -> (String, &'static str) {
        (self.error, self.stage.recovery())
    }
}

impl std::fmt::Display for StartupFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}; {}", self.error, self.stage.recovery())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAGES: [Stage; 6] = [
        Stage::Arguments,
        Stage::UpdateCustody,
        Stage::AudioPolicy,
        Stage::RecoveryLocation,
        Stage::Session,
        Stage::Editor,
    ];

    #[test]
    fn every_stage_has_a_lowercase_actionable_hint() {
        for stage in STAGES {
            let hint = stage.recovery();
            assert!(!hint.is_empty(), "{stage:?}");
            // scripts/bundle-macos.sh is a case-sensitive path and already lowercase.
            assert_eq!(hint, hint.to_lowercase(), "{stage:?}");
            assert!(hint.ends_with('.'), "{stage:?}");
        }
    }

    #[test]
    fn errors_stay_verbatim_and_map_to_their_stage_hint() {
        // A missing HOME is a recovery-location failure, not a reinstall problem.
        let home = StartupFailure::new(Stage::RecoveryLocation, "home directory is unavailable");
        let (error, hint) = home.into_report();
        assert_eq!(error, "home directory is unavailable");
        assert!(hint.contains("home folder"));

        let asset = StartupFailure::at(Stage::Editor)(
            "could not load embedded icon: icons/Play.svg".to_owned(),
        );
        assert_eq!(
            asset.to_string(),
            "could not load embedded icon: icons/Play.svg; reinstall hex or rebuild it with scripts/bundle-macos.sh."
        );
        let (error, _) = asset.into_report();
        assert!(error.ends_with("icons/Play.svg"), "paths keep their case");

        let custody = StartupFailure::new(Stage::UpdateCustody, "custody busy").into_report();
        assert!(custody.1.contains("update to finish"));
        let hints: std::collections::BTreeSet<_> = STAGES.iter().map(|s| s.recovery()).collect();
        assert_eq!(hints.len(), STAGES.len(), "each stage has its own hint");
    }
}
