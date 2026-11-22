//! UI-only adaptive spacing presets; these never select a fixed musical interval.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AdaptiveGridDensity {
    Fine,
    #[default]
    Medium,
    Coarse,
}

impl AdaptiveGridDensity {
    pub const ALL: [Self; 3] = [Self::Fine, Self::Medium, Self::Coarse];

    pub const fn stepped(self, coarser: bool) -> Self {
        match (self, coarser) {
            (Self::Fine, true) | (Self::Coarse, false) => Self::Medium,
            (Self::Medium | Self::Coarse, true) => Self::Coarse,
            (Self::Fine | Self::Medium, false) => Self::Fine,
        }
    }

    pub const fn minimum_spacing(self) -> f32 {
        match self {
            Self::Fine => 9.0,
            Self::Medium => 18.0,
            Self::Coarse => 36.0,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Fine => "adaptive grid · fine",
            Self::Medium => "adaptive grid · medium",
            Self::Coarse => "adaptive grid · coarse",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_step_monotonically_and_clamp_at_both_ends() {
        assert_eq!(AdaptiveGridDensity::default(), AdaptiveGridDensity::Medium);
        assert_eq!(
            AdaptiveGridDensity::Fine.stepped(false),
            AdaptiveGridDensity::Fine
        );
        assert_eq!(
            AdaptiveGridDensity::Fine.stepped(true),
            AdaptiveGridDensity::Medium
        );
        assert_eq!(
            AdaptiveGridDensity::Medium.stepped(true),
            AdaptiveGridDensity::Coarse
        );
        assert_eq!(
            AdaptiveGridDensity::Coarse.stepped(true),
            AdaptiveGridDensity::Coarse
        );
        assert_eq!(
            AdaptiveGridDensity::Coarse.stepped(false),
            AdaptiveGridDensity::Medium
        );
        assert!(
            AdaptiveGridDensity::Fine.minimum_spacing()
                < AdaptiveGridDensity::Medium.minimum_spacing()
        );
        assert!(
            AdaptiveGridDensity::Medium.minimum_spacing()
                < AdaptiveGridDensity::Coarse.minimum_spacing()
        );
        for preset in AdaptiveGridDensity::ALL {
            assert_eq!(preset.label(), preset.label().to_lowercase());
        }
    }
}
