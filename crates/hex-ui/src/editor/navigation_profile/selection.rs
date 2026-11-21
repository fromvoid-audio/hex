//! Finite probe scope; fixtures and measurements retain their existing owners.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Selection {
    All,
    Audio100Dense,
    ProjectDense,
    ProjectFitPan,
    ProjectFitPanSweep,
    ProjectFitZoom,
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
            Some("project-fit-pan") => Ok(Self::ProjectFitPan),
            Some("project-fit-pan-sweep") => Ok(Self::ProjectFitPanSweep),
            Some("project-fit-zoom") => Ok(Self::ProjectFitZoom),
            _ => Err(
                "navigation scope must be all, audio-100-dense, project-dense, project-fit-pan, project-fit-pan-sweep or project-fit-zoom",
            ),
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Audio100Dense => "audio-100-dense",
            Self::ProjectDense => "project-dense",
            Self::ProjectFitPan => "project-fit-pan",
            Self::ProjectFitPanSweep => "project-fit-pan-sweep",
            Self::ProjectFitZoom => "project-fit-zoom",
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
            Self::ProjectDense
            | Self::ProjectFitPan
            | Self::ProjectFitPanSweep
            | Self::ProjectFitZoom
                if project =>
            {
                Ok(())
            }
            _ => Err("selected navigation scope requires its project or fixture path"),
        }
    }

    pub(super) fn fits_project_rows(self) -> bool {
        matches!(
            self,
            Self::ProjectFitPan | Self::ProjectFitPanSweep | Self::ProjectFitZoom
        )
    }

    pub(super) fn cases(self, content: &str, tracks: usize, single_row: bool) -> Vec<Case> {
        if tracks == 0 || !matches!(content, "mixed" | "audio" | "project") {
            return Vec::new();
        }
        let dense_only = match self {
            Self::All => false,
            Self::Audio100Dense if content == "audio" && tracks == 100 && !single_row => true,
            Self::ProjectDense
            | Self::ProjectFitPan
            | Self::ProjectFitPanSweep
            | Self::ProjectFitZoom
                if content == "project" && !single_row =>
            {
                true
            }
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
            .filter(|case| match self {
                Self::ProjectFitPan | Self::ProjectFitPanSweep => case.action == "pan",
                Self::ProjectFitZoom => case.action == "zoom",
                _ => true,
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

/// A short pan with alternating legs and a quarter-point phase per leg.
/// Every step in a fitted trial is distinct while staying close to the fit span.
pub(super) fn fitted_pan_sweep_offset(iteration: usize) -> f32 {
    const LEG: usize = 12;
    let phase = iteration / LEG % 4;
    let index = iteration % LEG;
    let step = if phase.is_multiple_of(2) {
        index + 1
    } else {
        LEG - index
    };
    -(step as f32 * 1.5 + phase as f32 * 0.25)
}
