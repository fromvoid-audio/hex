//! Finite probe scope; fixtures and measurements retain their existing owners.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Selection {
    All,
    Audio100Dense,
    ProjectDense,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Case {
    pub density: &'static str,
    pub divisor: u64,
    pub action: &'static str,
}

impl Selection {
    pub(super) fn parse(value: Option<&str>) -> Result<Self, &'static str> {
        match value {
            None | Some("all") => Ok(Self::All),
            Some("audio-100-dense") => Ok(Self::Audio100Dense),
            Some("project-dense") => Ok(Self::ProjectDense),
            _ => Err("navigation scope must be all, audio-100-dense or project-dense"),
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Audio100Dense => "audio-100-dense",
            Self::ProjectDense => "project-dense",
        }
    }

    pub(super) fn validate_inputs(
        self,
        project: bool,
        mixed: bool,
        audio: bool,
    ) -> Result<(), &'static str> {
        match self {
            Self::All if project || mixed => Ok(()),
            Self::Audio100Dense if audio => Ok(()),
            Self::ProjectDense if project => Ok(()),
            _ => Err("selected navigation scope requires its project or fixture path"),
        }
    }

    pub(super) fn cases(self, content: &str, tracks: usize, single_row: bool) -> Vec<Case> {
        if tracks == 0 || !matches!(content, "mixed" | "audio" | "project") {
            return Vec::new();
        }
        let dense_only = match self {
            Self::All => false,
            Self::Audio100Dense if content == "audio" && tracks == 100 && !single_row => true,
            Self::ProjectDense if content == "project" && !single_row => true,
            _ => return Vec::new(),
        };
        [("dense", 1), ("zoomed", 16)]
            .into_iter()
            .filter(|(_, divisor)| !dense_only || *divisor == 1)
            .flat_map(|(density, divisor)| {
                ["pan", "zoom"].map(|action| Case {
                    density,
                    divisor,
                    action,
                })
            })
            .collect()
    }
}

/// Default remains one current-layout trial; comparison balances temporal order.
pub(super) fn comparison_trials(value: Option<&str>) -> Result<&'static [bool], &'static str> {
    match value {
        None | Some("0") => Ok(&[false]),
        Some("1") => Ok(&[true, false, false, true]),
        _ => Err("navigation layout comparison must be 0 or 1"),
    }
}

pub(super) fn iterations(smoke: bool) -> (usize, usize) {
    if smoke { (4, 1) } else { (8 + 40, 8) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_retains_default_and_balances_reference_current_trials() {
        assert_eq!(comparison_trials(None).unwrap(), &[false]);
        assert_eq!(comparison_trials(Some("0")).unwrap(), &[false]);
        assert_eq!(
            comparison_trials(Some("1")).unwrap(),
            &[true, false, false, true]
        );
        for value in ["", " ", "true", "2", "1 "] {
            assert!(comparison_trials(Some(value)).is_err());
        }
        let (rounds, warmup) = iterations(false);
        assert_eq!(
            comparison_trials(Some("1")).unwrap().len() * 2 * rounds,
            384
        );
        assert_eq!((rounds, warmup), (48, 8));
    }

    #[test]
    fn default_keeps_complete_generated_and_project_case_inventories() {
        let full = Selection::parse(None).unwrap();
        assert_eq!(full, Selection::parse(Some("all")).unwrap());
        let mut generated = Vec::new();
        for content in ["mixed", "audio"] {
            for (tracks, rows) in [(1, &[false][..]), (100, &[false, true][..])] {
                for &row in rows {
                    let cases = full.cases(content, tracks, row);
                    assert_eq!(
                        cases
                            .iter()
                            .map(|c| (c.density, c.action))
                            .collect::<Vec<_>>(),
                        [
                            ("dense", "pan"),
                            ("dense", "zoom"),
                            ("zoomed", "pan"),
                            ("zoomed", "zoom")
                        ]
                    );
                    generated.extend(cases);
                }
            }
        }
        assert_eq!(generated.len(), 24);
        assert_eq!(
            [false, true]
                .into_iter()
                .map(|row| full.cases("project", 71, row).len())
                .sum::<usize>(),
            8
        );
        assert_eq!(iterations(false), (48, 8));
        assert_eq!(iterations(false).0 - iterations(false).1, 40);
        assert_eq!(iterations(true), (4, 1));
    }

    #[test]
    fn bounded_scopes_select_only_their_normal_dense_pan_and_zoom() {
        for (scope, content, tracks) in [
            ("audio-100-dense", "audio", 100),
            ("project-dense", "project", 71),
        ] {
            let selection = Selection::parse(Some(scope)).unwrap();
            assert_eq!(selection.label(), scope);
            assert_eq!(
                selection.cases(content, tracks, false),
                [
                    Case {
                        density: "dense",
                        divisor: 1,
                        action: "pan"
                    },
                    Case {
                        density: "dense",
                        divisor: 1,
                        action: "zoom"
                    },
                ]
            );
            assert!(selection.cases(content, tracks, true).is_empty());
            assert!(selection.cases("mixed", tracks, false).is_empty());
        }
        assert!(Selection::Audio100Dense.cases("audio", 1, false).is_empty());
        assert!(
            Selection::Audio100Dense
                .cases("project", 100, false)
                .is_empty()
        );
        assert!(
            Selection::ProjectDense
                .cases("audio", 100, false)
                .is_empty()
        );
    }

    #[test]
    fn invalid_or_incompatible_selections_refuse_instead_of_running_nothing() {
        for value in ["", " ", "audio", "all,project-dense", "project-dense "] {
            assert!(Selection::parse(Some(value)).is_err());
        }
        for project in [false, true] {
            for mixed in [false, true] {
                for audio in [false, true] {
                    assert_eq!(
                        Selection::All
                            .validate_inputs(project, mixed, audio)
                            .is_ok(),
                        project || mixed
                    );
                    assert_eq!(
                        Selection::Audio100Dense
                            .validate_inputs(project, mixed, audio)
                            .is_ok(),
                        audio
                    );
                    assert_eq!(
                        Selection::ProjectDense
                            .validate_inputs(project, mixed, audio)
                            .is_ok(),
                        project
                    );
                }
            }
        }
        assert!(Selection::All.cases("unknown", 1, false).is_empty());
        assert!(
            Selection::ProjectDense
                .cases("project", 0, false)
                .is_empty()
        );
    }
}
