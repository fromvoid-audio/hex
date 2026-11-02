//! Decides when a confirmed default-route change moves the open output
//! (#4131). Pure control state on the Session worker: the route watch thread
//! confirms routes, this compares each newly confirmed route once with the
//! route the delivered output opened on, and the UI performs the move through
//! its existing restart-audio recovery.
//!
//! Only a route confirmed after the previous check is compared, so an output
//! that opened on the new route during the watch's confirmation delay is not
//! reopened. Each output identity is reported at most once.
use hex_audio::OutputRoute;

#[derive(Debug, Default)]
pub(super) struct RouteFollow {
    confirmed: Option<OutputRoute>,
    due: bool,
    reported: Option<u64>,
}
impl RouteFollow {
    /// Records a newly confirmed default route.
    pub(super) fn confirm(&mut self, route: OutputRoute) {
        self.confirmed = Some(route);
        self.due = true;
    }

    /// Identity of the delivered output to move, if the newest confirmed
    /// route differs from the route it opened on. A due check waits while no
    /// output is delivered. An output without a reported route never moves.
    pub(super) fn output_to_move(
        &mut self,
        output: Option<(u64, Option<&OutputRoute>)>,
    ) -> Option<u64> {
        let (identity, opened) = output?;
        if !std::mem::take(&mut self.due) {
            return None;
        }
        let confirmed = self.confirmed.as_ref()?;
        if self.reported == Some(identity) || opened.is_none_or(|opened| opened == confirmed) {
            return None;
        }
        self.reported = Some(identity);
        Some(identity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(device: &str, nominal_hz: u32) -> OutputRoute {
        OutputRoute {
            device: device.into(),
            nominal_hz,
        }
    }

    #[test]
    fn a_confirmed_route_change_moves_the_open_output_once() {
        let speakers = route("speakers", 48_000);
        let headphones = route("headphones", 48_000);
        let mut follow = RouteFollow::default();
        follow.confirm(speakers.clone());
        assert_eq!(follow.output_to_move(Some((1, Some(&speakers)))), None);
        follow.confirm(headphones.clone());
        assert_eq!(follow.output_to_move(Some((1, Some(&speakers)))), Some(1));
        // Not repeated for the same output, even after another change.
        assert_eq!(follow.output_to_move(Some((1, Some(&speakers)))), None);
        follow.confirm(route("dock", 48_000));
        assert_eq!(follow.output_to_move(Some((1, Some(&speakers)))), None);
        // The reopened output on the current route stays.
        follow.confirm(headphones.clone());
        assert_eq!(follow.output_to_move(Some((2, Some(&headphones)))), None);
        // A nominal-rate change on the same device moves it.
        follow.confirm(route("headphones", 44_100));
        assert_eq!(follow.output_to_move(Some((2, Some(&headphones)))), Some(2));
    }

    #[test]
    fn a_change_waits_for_a_delivered_output_and_ignores_unknown_routes() {
        let speakers = route("speakers", 48_000);
        let headphones = route("headphones", 48_000);
        let mut follow = RouteFollow::default();
        follow.confirm(headphones.clone());
        // Confirmed while opening: compared once the output is delivered.
        assert_eq!(follow.output_to_move(None), None);
        assert_eq!(follow.output_to_move(Some((3, Some(&speakers)))), Some(3));
        // An output that opened on the confirmed route during the watch's
        // confirmation delay is not moved by that stale confirmation.
        follow.confirm(headphones.clone());
        assert_eq!(follow.output_to_move(Some((4, Some(&headphones)))), None);
        // Without a newer confirmation nothing is compared again.
        assert_eq!(follow.output_to_move(Some((5, Some(&speakers)))), None);
        // An output whose route the backend did not report never moves.
        follow.confirm(speakers);
        assert_eq!(follow.output_to_move(Some((6, None))), None);
    }
}
