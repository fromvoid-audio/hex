//! Read-only replacement accents for already validated event previews. Positions
//! stay in the retained root clock until the view projects visible intersections.
//! At most four endpoints per admitted event plus one destination interval; reuse
//! Arrangement's scalar sweep rather than comparing every pair of events.
use hex_project::{AudioEventEdit, AudioEvents};

pub(super) fn replacement_spans(
    events: &AudioEvents,
    operation: &AudioEventEdit,
) -> Vec<(u64, u64)> {
    let mut excluded_id = None;
    let mut excluded_range = None;
    let move_target = match *operation {
        AudioEventEdit::Move { event, start } => Some((event, start)),
        AudioEventEdit::Place { event, start } => {
            u64::try_from(start).ok().map(|start| (event, start))
        }
        _ => None,
    };
    let target = if let Some((event, start)) = move_target {
        let Some(source) = events.events.iter().find(|e| e.id == event) else {
            return Vec::new();
        };
        if source.start == start {
            return Vec::new();
        }
        excluded_id = Some(event);
        start.checked_add(source.length()).map(|end| (start, end))
    } else {
        match *operation {
            AudioEventEdit::Trim { event, start, end } => {
                excluded_id = Some(event);
                Some((start, end))
            }
            AudioEventEdit::MoveRange {
                start,
                end,
                destination,
            } => {
                if start == destination {
                    return Vec::new();
                }
                excluded_range = Some((start, end));
                end.checked_sub(start)
                    .and_then(|span| destination.checked_add(span))
                    .map(|end| (destination, end))
            }
            AudioEventEdit::Paste { at, length, .. } => at.checked_add(length).map(|end| (at, end)),
            // Insert moves existing content instead of replacing it.
            _ => None,
        }
    };
    let Some((start, end)) = target.filter(|(a, b)| a < b) else {
        return Vec::new();
    };
    let mut endpoints = Vec::with_capacity(events.events.len() * 4 + 2);
    endpoints.extend([(start, 1, 0), (end, -1, 0)]);
    for event in &events.events {
        if excluded_id == Some(event.id) {
            continue;
        }
        let mut victim = |a: u64, b: u64| {
            if a < b {
                endpoints.extend([(a, 0, 1), (b, 0, -1)]);
            }
        };
        if let Some((a, b)) = excluded_range {
            victim(event.start, event.end.min(a));
            victim(event.start.max(b), event.end);
        } else {
            victim(event.start, event.end);
        }
    }
    super::super::tracks::group_overlap_spans(endpoints)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::AudioEvent;

    #[test]
    fn replacement_accents_follow_occupied_victims_and_range_overwrite_semantics() {
        let events = AudioEvents {
            sources: Vec::new(),
            next_id: 4,
            events: [(1, 0, 20), (2, 30, 40), (3, 60, 100)]
                .into_iter()
                .map(|(id, start, end)| AudioEvent {
                    active: true,
                    gain: hex_project::ClipGain::UNITY,
                    source: 0,
                    id,
                    start,
                    end,
                    source_start: start,
                    fade: None,
                })
                .collect(),
        };
        for (operation, expected) in [
            (
                AudioEventEdit::Move {
                    event: 1,
                    start: 25,
                },
                vec![(30, 40)],
            ),
            (AudioEventEdit::Move { event: 1, start: 0 }, vec![]),
            (
                AudioEventEdit::Trim {
                    event: 1,
                    start: 0,
                    end: 70,
                },
                vec![(30, 40), (60, 70)],
            ),
            // The canonical range move carves its whole destination, including
            // source gaps, but does not overwrite the removed source selection.
            (
                AudioEventEdit::MoveRange {
                    start: 0,
                    end: 40,
                    destination: 25,
                },
                vec![(60, 65)],
            ),
            (
                AudioEventEdit::MoveRange {
                    start: 10,
                    end: 70,
                    destination: 0,
                },
                vec![(0, 10)],
            ),
            (
                AudioEventEdit::Paste {
                    at: 15,
                    length: 60,
                    events: vec![],
                },
                vec![(15, 20), (30, 40), (60, 75)],
            ),
            (
                AudioEventEdit::Insert {
                    at: 0,
                    length: 20,
                    events: vec![],
                },
                vec![],
            ),
        ] {
            assert_eq!(
                replacement_spans(&events, &operation),
                expected,
                "{operation:?}"
            );
        }
    }
}
