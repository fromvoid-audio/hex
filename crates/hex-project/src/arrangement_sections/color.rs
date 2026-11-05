//! Control-side section color draws; color usage never restricts a valid edit.
use super::{ElementColor, Project, SectionId};
use std::{collections::hash_map::RandomState, hash::BuildHasher, sync::OnceLock};

pub(super) fn duplicate_color(project: &Project, section: SectionId) -> ElementColor {
    // One randomly seeded hasher per process makes the non-security draw stable
    // across preview clones and candidate preparation, without saving RNG state.
    // A new identity gets a new draw. Undo/Redo and reopen restore the snapshot's
    // chosen color rather than calling this selector again.
    static DRAW: OnceLock<RandomState> = OnceLock::new();
    let ticket = DRAW
        .get_or_init(RandomState::new)
        .hash_one((project.next_id, section));
    let [first, rest @ ..] = ElementColor::ALL;
    draw(first, &rest, ticket)
}

// A nonempty palette by construction, including a single-color palette. Every
// ticket selects in bounded work without scanning existing sections or retrying.
fn draw(first: ElementColor, rest: &[ElementColor], ticket: u64) -> ElementColor {
    let index = (ticket % (rest.len() as u64 + 1)) as usize;
    rest.get(index).copied().unwrap_or(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controlled_draws_reach_all_colors_and_allow_single_color_reuse() {
        let [first, rest @ ..] = ElementColor::ALL;
        let draws: Vec<_> = (0..ElementColor::ALL.len() as u64)
            .map(|ticket| draw(first, &rest, ticket))
            .collect();
        assert!(ElementColor::ALL.iter().all(|color| draws.contains(color)));
        for ticket in [0, 1, 9, u64::MAX] {
            assert_eq!(draw(ElementColor::Cyan, &[], ticket), ElementColor::Cyan);
            assert!(ElementColor::ALL.contains(&draw(first, &rest, ticket)));
        }
    }
}
