//! Canonical acyclic four-operator topologies, independent of oscillator levels.
//!
//! The order and node arrangements follow the Live 12 manual's algorithm
//! illustration. The modulation scale remains Hex's original 0.25 turns;
//! this catalog makes no numerical or sonic compatibility claim.
use hex_lattice_contract::{
    DeviceContractError, ParameterDescriptor, ParameterKind, ParameterPresentation, ParameterValue,
};

/// Operator indices are zero-based: a, b, c, d.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Algorithm {
    pub edges: &'static [(usize, usize)],
    pub carriers: &'static [usize],
}

/// Left-to-right algorithm order; all edges flow from higher to lower indices.
pub const CATALOG: [Algorithm; 11] = [
    Algorithm {
        edges: &[(3, 2), (2, 1), (1, 0)],
        carriers: &[0],
    },
    Algorithm {
        edges: &[(3, 1), (2, 1), (1, 0)],
        carriers: &[0],
    },
    Algorithm {
        edges: &[(2, 1), (1, 0), (3, 0)],
        carriers: &[0],
    },
    Algorithm {
        edges: &[(3, 2), (3, 1), (2, 0), (1, 0)],
        carriers: &[0],
    },
    Algorithm {
        edges: &[(3, 2), (2, 1), (2, 0)],
        carriers: &[0, 1],
    },
    Algorithm {
        edges: &[(3, 2), (2, 1)],
        carriers: &[0, 1],
    },
    Algorithm {
        edges: &[(3, 0), (2, 0), (1, 0)],
        carriers: &[0],
    },
    Algorithm {
        edges: &[(3, 2), (1, 0)],
        carriers: &[0, 2],
    },
    Algorithm {
        edges: &[(3, 0), (3, 1), (3, 2)],
        carriers: &[0, 1, 2],
    },
    Algorithm {
        edges: &[(3, 2)],
        carriers: &[0, 1, 2],
    },
    Algorithm {
        edges: &[],
        carriers: &[0, 1, 2, 3],
    },
];

const LABELS: [&str; 12] = [
    "custom",
    "algorithm 1",
    "algorithm 2",
    "algorithm 3",
    "algorithm 4",
    "algorithm 5",
    "algorithm 6",
    "algorithm 7",
    "algorithm 8",
    "algorithm 9",
    "algorithm 10",
    "algorithm 11",
];
pub(super) const DESCRIPTOR: ParameterDescriptor = ParameterDescriptor {
    id: super::parameter::ALGORITHM,
    label: "algorithm",
    kind: ParameterKind::Choice { labels: &LABELS },
    default: ParameterValue::Choice(1),
    presentation: ParameterPresentation::Linear,
};

impl Algorithm {
    pub(super) fn routing_fields(self) -> [f32; 20] {
        let mut fields = [0.0; 20];
        for &(source, target) in self.edges {
            fields[source * 4 + target] = 0.25;
        }
        for &carrier in self.carriers {
            fields[16 + carrier] = 1.0;
        }
        fields
    }
}

pub(super) fn choice(fields: &[f32]) -> u32 {
    CATALOG
        .iter()
        .position(|algorithm| fields == algorithm.routing_fields())
        .map_or(0, |index| index as u32 + 1)
}

pub(super) fn from_choice(choice: u32) -> Result<Option<Algorithm>, DeviceContractError> {
    if choice == 0 {
        return Ok(None);
    }
    CATALOG
        .get(choice as usize - 1)
        .copied()
        .map(Some)
        .ok_or(DeviceContractError::InvalidParameterValue)
}

#[cfg(test)]
mod tests;
