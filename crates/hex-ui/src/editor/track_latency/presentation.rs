//! Pure presentation of acknowledged track latency.
use crate::theme;
use hex_engine::OutputLatency;
use hex_project::{ReturnId, TrackId};

#[derive(Clone, Copy)]
pub(crate) enum Owner {
    Track(TrackId),
    Return(ReturnId),
    Main,
}

pub(super) enum Reading {
    Unavailable,
    Pending,
    Applied(OutputLatency, u32, Option<u32>),
}

fn duration(frames: u32, rate: u32) -> String {
    let milliseconds = 1000.0 * f64::from(frames) / f64::from(rate);
    format!("{frames} samples ({milliseconds:.3} ms)")
}

pub(super) fn present(
    owner: Owner,
    reading: Reading,
) -> (&'static str, theme::PaletteRole, String) {
    match reading {
        Reading::Unavailable => (
            "·",
            theme::PaletteRole::Muted,
            "track latency unavailable".into(),
        ),
        Reading::Pending => (
            "…",
            theme::PaletteRole::Muted,
            "waiting for applied track latency".into(),
        ),
        Reading::Applied(facts, rate, main) => {
            let color = if facts.local_frames > 0 {
                theme::PaletteRole::Primary
            } else {
                theme::PaletteRole::Muted
            };
            let local = duration(facts.local_frames, rate);
            let arrival = duration(facts.arrival_frames, rate);
            let detail = if matches!(owner, Owner::Main) {
                format!("main devices · {local}\nmain output arrival · {arrival}")
            } else {
                let compensation = duration(facts.default_route_delay_frames, rate);
                let main = main.map_or_else(|| "unknown".into(), |frames| duration(frames, rate));
                format!(
                    "local devices · {local}\npost-fader arrival · {arrival}\ndefault route compensation · {compensation}\nmain output arrival · {main}"
                )
            };
            (
                "lat",
                color,
                detail
                    + "\ndeclared processing delay only. sends can have separate alignment delays.",
            )
        }
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;

    #[test]
    fn zero_pending_and_unavailable_are_distinct_and_use_the_applied_rate() {
        let facts = OutputLatency {
            local_frames: 0,
            arrival_frames: 48,
            default_route_delay_frames: 0,
        };
        let (label, _, detail) = present(Owner::Main, Reading::Applied(facts, 96_000, Some(48)));
        assert_eq!(label, "lat");
        assert!(detail.contains("main devices · 0 samples (0.000 ms)"));
        assert!(detail.contains("main output arrival · 48 samples (0.500 ms)"));
        assert_eq!(present(Owner::Main, Reading::Pending).0, "…");
        assert_eq!(present(Owner::Main, Reading::Unavailable).0, "·");
    }
}
