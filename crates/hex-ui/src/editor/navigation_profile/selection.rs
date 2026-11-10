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
