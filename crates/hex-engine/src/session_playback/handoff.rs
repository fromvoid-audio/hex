use super::*;
use super::{instrument_move::InstrumentMoveKind, return_effect_tails::DeviceEditTailPolicy};
use crate::playback::HostedMoveRequest;
use crate::session_inventory::EffectiveSessionMidi;
use crate::signal_graph::FreshHistory;
use crate::{Invalid, MidiTiming};
use std::sync::Weak;

mod commit;
mod compensation_plan;
mod continuation_plan;
mod loop_gate_continuation;
mod preflight;
mod shared_sources;
mod tempo_anchor;

pub(super) use compensation_plan::validate_foreign_tail_sources;
pub(crate) use compensation_plan::{COMPENSATED_TAILS_CHANGED, COMPENSATED_TIMING_CHANGED};
pub(crate) use continuation_plan::MIDI_SOURCE_LAYOUT_CHANGED;
pub(super) use preflight::configuration_matches_continuation;
pub(crate) use tempo_anchor::ClaimedTempoPair;
pub use tempo_anchor::PreparedTempoAnchor;

use compensation_plan::{
    appended_foreign_effect, cold_added_track, direct_return_tail_graph, fresh_added_audio_track,
    fresh_unconnected_track, retained_path_roles_match, single_track_latency_edit,
    tail_runtime_suffix_start, validate_tail_suffix,
};
use continuation_plan::{
    ArrangementInstrumentHandoff, ArrangementQueryCut, RemapCandidate, Restart,
    SessionMidiRouteHandoff, add_or_extend_suppression, arrangement_only_proofs,
    candidate_arrangement_cut, cut_arrangement_voices_for_remap,
    first_pass_arrangement_loop_continuity, handoff_arrangement_instrument_changes,
    handoff_session_midi_route_changes, inline_tuning_owner_matches, is_suppressed,
    map_arrangement_cut, mutable_key_has_live_proof, mutable_prefix_restoration,
    mutable_prefix_shortening, mutable_proof_deadline, mutable_session_clip_matches,
    remap_release_gate, restartable, retune_inline_release,
};
use shared_sources::SharedSourceHandoff;

const GROUP_ROUTE_HISTORY_TRANSFER: u8 = 4;
const HOSTED_MOVE_HISTORY_TRANSFER: u8 = 5;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_revision_handoff_preserves_transport_state() {
        let project = hex_project::Project::new(48_000).unwrap();
        let media = std::collections::BTreeMap::new();
        let sine = Arc::new(crate::SineTable::new());
        let first =
            Arc::new(PreparedRevision::prepare(&project, &media, Arc::clone(&sine), None).unwrap());
        let second = Arc::new(PreparedRevision::prepare(&project, &media, sine, None).unwrap());
        let mut previous = SessionPlayback::new(first).unwrap();
        previous.play_from(128).unwrap();
        let mut candidate = SessionPlayback::new(second).unwrap();
        candidate.inherit_state(&mut previous).unwrap();
        assert!(candidate.playing());
        assert_eq!(candidate.arrangement_frame(), 128);
    }
}
