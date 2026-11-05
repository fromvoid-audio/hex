//! UI-only adaptive spacing presets; these never select a fixed musical interval.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum AdaptiveGridDensity {
    Fine,
    #[default]
    Medium,
    Coarse,
}

impl AdaptiveGridDensity {
    pub(crate) const ALL: [Self; 3] = [Self::Fine, Self::Medium, Self::Coarse];

    pub(crate) const fn stepped(self, coarser: bool) -> Self {
        match (self, coarser) {
            (Self::Fine, true) | (Self::Coarse, false) => Self::Medium,
            (Self::Medium | Self::Coarse, true) => Self::Coarse,
            (Self::Fine | Self::Medium, false) => Self::Fine,
        }
    }

    pub(crate) const fn minimum_spacing(self) -> f32 {
        match self {
            Self::Fine => 9.0,
            Self::Medium => 18.0,
            Self::Coarse => 36.0,
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Fine => "adaptive grid · fine",
            Self::Medium => "adaptive grid · medium",
            Self::Coarse => "adaptive grid · coarse",
        }
    }
}
