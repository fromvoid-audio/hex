//! `SubmitError::Busy` names the admission rule that refused a request (#3860).
use super::*;

#[test]
fn an_empty_bounce_target_is_refused_as_invalid_not_export_pending() {
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    let refused = service.submit(Request::BounceArrangement {
        project_generation: 0,
        revision: 0,
        target: hex_project::BounceTarget::Clips(Vec::new()),
        in_place: false,
        adoption: hex_project::BounceAdoption::NewTracks,
        control: hex_export::ExportControl::default(),
    });
    assert!(matches!(
        refused,
        Err(SubmitError::Busy(
            Request::BounceArrangement { .. },
            BusyReason::InvalidBounceTarget
        ))
    ));
}
