//! Exhaustive privacy-safe projections of canonical request/command kinds.
//! New variants require an explicit projection. Text/paths/blob contents are
//! never Debug-formatted; unsupported fields are named as omitted. Collections
//! report counts, with at most eight IDs where the typed selection allows it.
use super::{Description, FirstIds, Parameter, Snapshot};
use crate::service::Request;
use hex_project::{Command, ExistingAudioPlacement};
macro_rules! describe {
    ($kind:literal; [$($key:literal => $value:expr),*]; omitted=$omitted:literal) => {{
        let mut description = Description::new($kind);
        $(description.field($key, format_args!("{:?}", $value));)*
        description.omitted($omitted);
        description
    }};
}
pub(super) fn clip_edit(value: &hex_project::ClipEdit) -> Description {
    match value {
        hex_project::ClipEdit::Active { active } => {
            describe!("active"; ["active" => active]; omitted="")
        }
        hex_project::ClipEdit::Gain { millidb } => {
            describe!("gain"; ["millidb" => millidb]; omitted="")
        }
        hex_project::ClipEdit::Pitch { cents } => {
            describe!("pitch"; ["cents" => cents]; omitted="")
        }
        hex_project::ClipEdit::GainBy { millidb } => {
            describe!("gain_by"; ["millidb" => millidb]; omitted="")
        }
        hex_project::ClipEdit::PitchBy { cents } => {
            describe!("pitch_by"; ["cents" => cents]; omitted="")
        }
        hex_project::ClipEdit::Musical { enabled } => {
            describe!("musical"; ["enabled" => enabled]; omitted="")
        }
        hex_project::ClipEdit::SampleBpm { .. } => describe!("sample_bpm"; []; omitted="tempo"),
        hex_project::ClipEdit::WarpEnabled { enabled } => {
            describe!("warp_enabled"; ["enabled" => enabled]; omitted="")
        }
        hex_project::ClipEdit::WarpMode { .. } => describe!("warp_mode"; []; omitted="mode"),
        hex_project::ClipEdit::WarpAdjust { .. } => {
            describe!("warp_adjust"; []; omitted="adjustment")
        }
        hex_project::ClipEdit::Looping { looping } => {
            describe!("looping"; ["looping" => looping]; omitted="")
        }
        hex_project::ClipEdit::Flavor { .. } => describe!("flavor"; []; omitted="flavor"),
        hex_project::ClipEdit::Rename { name } => {
            describe!("rename"; ["name_redacted_bytes" => name.len()]; omitted="")
        }
        hex_project::ClipEdit::Delete => describe!("delete"; []; omitted=""),
    }
}

pub(super) fn track_clipboard(value: &crate::service::TrackClipboardAction) -> Description {
    match value {
        crate::service::TrackClipboardAction::Copy(value0) => {
            describe!("copy"; ["value0_count" => value0.ids().len(), "value0_first_ids" => FirstIds(value0.ids())]; omitted="")
        }
        crate::service::TrackClipboardAction::Cut(value0) => {
            describe!("cut"; ["value0_count" => value0.ids().len(), "value0_first_ids" => FirstIds(value0.ids())]; omitted="")
        }
        crate::service::TrackClipboardAction::Duplicate {
            tracks,
            without_clips,
        } => {
            describe!("duplicate"; ["tracks_count" => tracks.ids().len(), "tracks_first_ids" => FirstIds(tracks.ids()), "without_clips" => without_clips]; omitted="")
        }
        crate::service::TrackClipboardAction::Paste { before } => {
            describe!("paste"; ["before" => before.map(|id| id.value())]; omitted="")
        }
    }
}

pub(super) fn gain_lane(value: &crate::service::GainLaneEdit) -> Description {
    match value {
        crate::service::GainLaneEdit::SpliceTake {
            expected_epoch,
            spans,
        } => {
            describe!("splice_take"; ["expected_epoch" => expected_epoch, "spans_count" => spans.len()]; omitted="")
        }
        crate::service::GainLaneEdit::Replace(value0) => {
            describe!("replace"; ["value0_count" => value0.len()]; omitted="")
        }
        crate::service::GainLaneEdit::Upsert(_) => describe!("upsert"; []; omitted="value0"),
        crate::service::GainLaneEdit::Move { from, to } => {
            describe!("move"; ["from_beats_q32" => from.raw(), "to_beats_q32" => to.raw()]; omitted="")
        }
        crate::service::GainLaneEdit::Remove(value0) => {
            describe!("remove"; ["value0_beats_q32" => value0.raw()]; omitted="")
        }
        crate::service::GainLaneEdit::Clear => describe!("clear"; []; omitted=""),
    }
}

pub(super) fn tuning_gesture(value: &crate::tuning_gesture::TuningGestureRequest) -> Description {
    match value {
        crate::tuning_gesture::TuningGestureRequest::Begin { key } => {
            describe!("begin"; ["key_generation" => key.generation, "key_token" => key.token]; omitted="")
        }
        crate::tuning_gesture::TuningGestureRequest::Change { key, sequence, .. } => {
            describe!("change"; ["key_generation" => key.generation, "key_token" => key.token, "sequence" => sequence]; omitted="tuning")
        }
        crate::tuning_gesture::TuningGestureRequest::Commit { key, sequence, .. } => {
            describe!("commit"; ["key_generation" => key.generation, "key_token" => key.token, "sequence" => sequence]; omitted="tuning")
        }
        crate::tuning_gesture::TuningGestureRequest::Cancel { key } => {
            describe!("cancel"; ["key_generation" => key.generation, "key_token" => key.token]; omitted="")
        }
    }
}

pub(super) fn tuning_library(value: &crate::tuning_gesture::TuningLibraryOperation) -> Description {
    match value {
        crate::tuning_gesture::TuningLibraryOperation::List => describe!("list"; []; omitted=""),
        crate::tuning_gesture::TuningLibraryOperation::Save { slot, name } => {
            describe!("save"; ["slot" => slot, "name_redacted_bytes" => name.len()]; omitted="")
        }
        crate::tuning_gesture::TuningLibraryOperation::Load { slot } => {
            describe!("load"; ["slot" => slot]; omitted="")
        }
        crate::tuning_gesture::TuningLibraryOperation::Rename { slot, name } => {
            describe!("rename"; ["slot" => slot, "name_redacted_bytes" => name.len()]; omitted="")
        }
        crate::tuning_gesture::TuningLibraryOperation::Duplicate { slot } => {
            describe!("duplicate"; ["slot" => slot]; omitted="")
        }
        crate::tuning_gesture::TuningLibraryOperation::Import { path } => {
            describe!("import"; ["path_redacted_bytes" => path.as_os_str().len()]; omitted="")
        }
        crate::tuning_gesture::TuningLibraryOperation::Export { path, name } => {
            describe!("export"; ["path_redacted_bytes" => path.as_os_str().len(), "name_redacted_bytes" => name.len()]; omitted="")
        }
    }
}

pub(super) fn device_drop_source(value: &crate::service::NativeDeviceDropSource) -> Description {
    match value {
        crate::service::NativeDeviceDropSource::Browser { type_id, .. } => {
            describe!("browser"; ["type_id" => type_id.as_str()]; omitted="class")
        }
        crate::service::NativeDeviceDropSource::Existing {
            device, type_id, ..
        } => {
            describe!("existing"; ["device" => device.value(), "type_id" => type_id.as_str()]; omitted="class")
        }
    }
}

pub(super) fn effect_bus_destination(value: &crate::service::EffectBusDestination) -> Description {
    match value {
        crate::service::EffectBusDestination::Main => describe!("main"; []; omitted=""),
        crate::service::EffectBusDestination::Return(value0) => {
            describe!("return"; ["value0" => value0.value()]; omitted="")
        }
    }
}

pub(super) fn created(value: &hex_project::Created) -> Description {
    match value {
        hex_project::Created::NativeSource(value0) => {
            describe!("native_source"; ["value0" => value0.value()]; omitted="")
        }
        hex_project::Created::ArrangementSection(value0) => {
            describe!("arrangement_section"; ["value0" => value0.value()]; omitted="")
        }
        hex_project::Created::Track(value0) => {
            describe!("track"; ["value0" => value0.value()]; omitted="")
        }
        hex_project::Created::Tracks { first, last } => {
            describe!("tracks"; ["first" => first.value(), "last" => last.value()]; omitted="")
        }
        hex_project::Created::ReturnTrack(value0) => {
            describe!("return_track"; ["value0" => value0.value()]; omitted="")
        }
        hex_project::Created::Device(value0) => {
            describe!("device"; ["value0" => value0.value()]; omitted="")
        }
        hex_project::Created::Asset(value0) => {
            describe!("asset"; ["value0" => value0.value()]; omitted="")
        }
        hex_project::Created::Clip(value0) => {
            describe!("clip"; ["value0" => value0.value()]; omitted="")
        }
        hex_project::Created::Clips { first, last } => {
            describe!("clips"; ["first" => first.value(), "last" => last.value()]; omitted="")
        }
        hex_project::Created::Note(value0) => {
            describe!("note"; ["value0" => value0.value()]; omitted="")
        }
        hex_project::Created::Scene(value0) => {
            describe!("scene"; ["value0" => value0.value()]; omitted="")
        }
        hex_project::Created::SessionClip(value0) => {
            describe!("session_clip"; ["value0" => value0.value()]; omitted="")
        }
        hex_project::Created::Notes { clip, first, last } => {
            describe!("notes"; ["clip" => clip.value(), "first" => first.value(), "last" => last.value()]; omitted="")
        }
        hex_project::Created::Imported { track, asset, clip } => {
            describe!("imported"; ["track" => track.value(), "asset" => asset.value(), "clip" => clip.value()]; omitted="")
        }
        hex_project::Created::None => describe!("none"; []; omitted=""),
    }
}

pub(super) fn command(value: &Command) -> Description {
    match value {
        Command::DuplicateAliasClips(value0) => {
            describe!("duplicate_alias_clips"; ["value0_count" => value0.len()]; omitted="")
        }
        Command::DuplicateAliasClipsAt {
            clips,
            anchor,
            track,
            ..
        } => {
            describe!("duplicate_alias_clips_at"; ["clips_count" => clips.len(), "anchor" => anchor.value(), "track" => track.value()]; omitted="position")
        }
        Command::DuplicateAliasClipToNewTrack {
            clip,
            name,
            before_track,
            initial_gain,
            ..
        } => {
            describe!("duplicate_alias_clip_to_new_track"; ["clip" => clip.value(), "name_bytes" => name.len(), "has_before_track" => before_track.is_some(), "initial_gain" => initial_gain]; omitted="position")
        }
        Command::MakeClipsUnique(value0) => {
            describe!("make_clips_unique"; ["value0_count" => value0.len()]; omitted="")
        }
        Command::ApplyClipSettingsToAliases(value0) => {
            describe!("apply_clip_settings_to_aliases"; ["value0" => value0.value()]; omitted="")
        }
        Command::DuplicateSessionCell {
            track,
            scene,
            alias,
        } => {
            describe!("duplicate_session_cell"; ["track" => track.value(), "scene" => scene.value(), "alias" => alias]; omitted="")
        }
        Command::CreateNativeSource {
            name,
            devices,
            instrument_index,
        } => {
            describe!("create_native_source"; ["name_redacted_bytes" => name.len(), "devices_count" => devices.len(), "instrument_index" => instrument_index]; omitted="")
        }
        Command::BindTrackSource { track, source } => {
            describe!("bind_track_source"; ["track" => track.value(), "source" => source.map(|id| id.value())]; omitted="")
        }
        Command::SetNativeSourceReceiver { source, track } => {
            describe!("set_native_source_receiver"; ["source" => source.value(), "track" => track.map(|id| id.value())]; omitted="")
        }
        Command::RenameNativeSource { source, name } => {
            describe!("rename_native_source"; ["source" => source.value(), "name_redacted_bytes" => name.len()]; omitted="")
        }
        Command::RemoveNativeSource(value0) => {
            describe!("remove_native_source"; ["value0" => value0.value()]; omitted="")
        }
        Command::GroupTracks { tracks, name } => {
            describe!("group_tracks"; ["tracks_count" => tracks.ids().len(), "tracks_first_ids" => FirstIds(tracks.ids()), "name_redacted_bytes" => name.len()]; omitted="")
        }
        Command::UngroupTrack(value0) => {
            describe!("ungroup_track"; ["value0" => value0.value()]; omitted="")
        }
        Command::ReparentTracks { tracks, .. } => {
            describe!("reparent_tracks"; ["tracks_count" => tracks.ids().len(), "tracks_first_ids" => FirstIds(tracks.ids())]; omitted="destination")
        }
        Command::DuplicateTracks {
            tracks,
            without_clips,
        } => {
            describe!("duplicate_tracks"; ["tracks_count" => tracks.ids().len(), "tracks_first_ids" => FirstIds(tracks.ids()), "without_clips" => without_clips]; omitted="")
        }
        Command::CommitArrangementBounce {
            in_place,
            adoption,
            files,
            ..
        } => {
            describe!("commit_arrangement_bounce"; ["in_place" => in_place, "adoption" => adoption, "files_count" => files.len()]; omitted="target")
        }
        Command::SetClipActive { clip, active } => {
            describe!("set_clip_active"; ["clip" => clip.value(), "active" => active]; omitted="")
        }
        Command::SetTimeRangeActive { active, .. } => {
            describe!("set_time_range_active"; ["active" => active]; omitted="range")
        }
        Command::EditClips { clips, edit } => {
            let mut description =
                describe!("edit_clips"; ["clips_count" => clips.len()]; omitted="");
            description.nested("edit", self::clip_edit(edit));
            description
        }
        Command::AddArrangementSection {
            name, start, end, ..
        } => {
            describe!("add_arrangement_section"; ["name_redacted_bytes" => name.len(), "start_beats_q32" => start.raw(), "end_beats_q32" => end.raw()]; omitted="color")
        }
        Command::SetArrangementSectionRange {
            section,
            start,
            end,
        } => {
            describe!("set_arrangement_section_range"; ["section" => section.value(), "start_beats_q32" => start.raw(), "end_beats_q32" => end.raw()]; omitted="")
        }
        Command::MoveArrangementSectionMarker { section, start } => {
            describe!("move_arrangement_section_marker"; ["section" => section.value(), "start_beats_q32" => start.raw()]; omitted="")
        }
        Command::ResizeArrangementSectionMarker {
            section, boundary, ..
        } => {
            describe!("resize_arrangement_section_marker"; ["section" => section.value(), "boundary_beats_q32" => boundary.raw()]; omitted="edge")
        }
        Command::RenameArrangementSection { section, name } => {
            describe!("rename_arrangement_section"; ["section" => section.value(), "name_redacted_bytes" => name.len()]; omitted="")
        }
        Command::SetArrangementSectionColor { section, .. } => {
            describe!("set_arrangement_section_color"; ["section" => section.value()]; omitted="color")
        }
        Command::RemoveArrangementSection(value0) => {
            describe!("remove_arrangement_section"; ["value0" => value0.value()]; omitted="")
        }
        Command::ResizeArrangementSection {
            section, length, ..
        } => {
            describe!("resize_arrangement_section"; ["section" => section.value(), "length_beats_q32" => length.raw()]; omitted="edge")
        }
        Command::ResizeArrangementSectionEdge {
            section, boundary, ..
        } => {
            describe!("resize_arrangement_section_edge"; ["section" => section.value(), "boundary_beats_q32" => boundary.raw()]; omitted="edge")
        }
        Command::ResizeArrangementSectionBoundary {
            left,
            right,
            boundary,
            marker_only,
        } => {
            describe!("resize_arrangement_section_boundary"; ["left" => left.value(), "right" => right.value(), "boundary_beats_q32" => boundary.raw(), "marker_only" => marker_only]; omitted="")
        }
        Command::DeleteArrangementSection(value0) => {
            describe!("delete_arrangement_section"; ["value0" => value0.value()]; omitted="")
        }
        Command::DuplicateArrangementSection(value0) => {
            describe!("duplicate_arrangement_section"; ["value0" => value0.value()]; omitted="")
        }
        Command::DuplicateArrangementSectionAt {
            section,
            target,
            after,
        } => {
            describe!("duplicate_arrangement_section_at"; ["section" => section.value(), "target" => target.value(), "after" => after]; omitted="")
        }
        Command::SplitArrangementSection { section, at } => {
            describe!("split_arrangement_section"; ["section" => section.value(), "at_beats_q32" => at.raw()]; omitted="")
        }
        Command::MergeArrangementSections(value0) => {
            describe!("merge_arrangement_sections"; ["value0_count" => value0.len()]; omitted="")
        }
        Command::ReorderSectionBefore { section, before } => {
            describe!("reorder_section_before"; ["section" => section.value(), "before" => before.value()]; omitted="")
        }
        Command::ReorderSectionAfter { section, after } => {
            describe!("reorder_section_after"; ["section" => section.value(), "after" => after.value()]; omitted="")
        }
        Command::SetTuning(_) => describe!("set_tuning"; []; omitted="value0"),
        Command::EditAudioEvents { clip, .. } => {
            describe!("edit_audio_events"; ["clip" => clip.value()]; omitted="operation")
        }
        Command::RenameClip { clip, name } => {
            describe!("rename_clip"; ["clip" => clip.value(), "name_redacted_bytes" => name.len()]; omitted="")
        }
        Command::SetTrackColor { track, .. } => {
            describe!("set_track_color"; ["track" => track.value()]; omitted="color")
        }
        Command::SetClipFlavor { clip, .. } => {
            describe!("set_clip_flavor"; ["clip" => clip.value()]; omitted="flavor")
        }
        Command::ConvertAudioClipToMidi(value0) => {
            describe!("convert_audio_clip_to_midi"; ["value0" => value0.value()]; omitted="")
        }
        Command::ConvertMidiClipToAudio { clip, path, frames } => {
            describe!("convert_midi_clip_to_audio"; ["clip" => clip.value(), "path_redacted_bytes" => path.as_os_str().len(), "frames" => frames]; omitted="")
        }
        Command::InsertTrackWithDevice {
            name,
            before_track,
            state,
            ..
        } => {
            describe!("insert_track_with_device"; ["name_redacted_bytes" => name.len(), "before_track" => before_track.map(|id| id.value()), "state_type" => state.type_id().as_str(), "state_state_version" => state.state_version(), "state_payload_bytes" => state.payload().len()]; omitted="slot")
        }
        Command::InsertDefaultTrackWithDevice {
            before_track,
            state,
            device_name,
            ..
        } => {
            describe!("insert_default_track_with_device"; ["before_track" => before_track.map(|id| id.value()), "state_type" => state.type_id().as_str(), "state_state_version" => state.state_version(), "state_payload_bytes" => state.payload().len(), "device_name_redacted_bytes" => device_name.len()]; omitted="slot")
        }
        Command::InsertDefaultTrackWithDeviceAndGain {
            before_track,
            state,
            device_name,
            gain,
            ..
        } => {
            describe!("insert_default_track_with_device_and_gain"; ["before_track" => before_track.map(|id| id.value()), "state_type" => state.type_id().as_str(), "device_name_redacted_bytes" => device_name.len(), "gain" => gain]; omitted="slot,state")
        }
        Command::SpliceGainTouch {
            track,
            expected_epoch,
            ..
        } => {
            describe!("splice_gain_touch"; ["track" => track.value(), "expected_epoch" => expected_epoch]; omitted="span")
        }
        Command::SpliceGainTouchBatch {
            track,
            expected_epoch,
            spans,
        } => {
            describe!("splice_gain_touch_batch"; ["track" => track.value(), "expected_epoch" => expected_epoch, "spans_count" => spans.len()]; omitted="")
        }
        Command::ReplaceTrackGainEnvelope { track, points } => {
            describe!("replace_track_gain_envelope"; ["track" => track.value(), "points_count" => points.len()]; omitted="")
        }
        Command::UpsertTrackGainPoint { track, .. } => {
            describe!("upsert_track_gain_point"; ["track" => track.value()]; omitted="point")
        }
        Command::MoveTrackGainPoint { track, from, to } => {
            describe!("move_track_gain_point"; ["track" => track.value(), "from_beats_q32" => from.raw(), "to_beats_q32" => to.raw()]; omitted="")
        }
        Command::RemoveTrackGainPoint { track, beat } => {
            describe!("remove_track_gain_point"; ["track" => track.value(), "beat_beats_q32" => beat.raw()]; omitted="")
        }
        Command::ClearTrackGainEnvelope { track } => {
            describe!("clear_track_gain_envelope"; ["track" => track.value()]; omitted="")
        }
        Command::SetRouting(value0) => {
            describe!("set_routing"; ["value0_count" => value0.len()]; omitted="")
        }
        Command::SetAudioFeed {
            destination, gain, ..
        } => {
            describe!("set_audio_feed"; ["destination" => destination.value(), "gain" => gain]; omitted="source")
        }
        Command::SetSidechain { device, source } => {
            describe!("set_sidechain"; ["device" => device.value(), "source" => source.map(|id| id.value())]; omitted="")
        }
        Command::SetMidiSource {
            destination,
            source,
        } => {
            describe!("set_midi_source"; ["destination" => destination.value(), "source" => source.map(|id| id.value())]; omitted="")
        }
        Command::SetReturnFeed {
            source,
            destination,
            gain,
        } => {
            describe!("set_return_feed"; ["source" => source.value(), "destination" => destination.value(), "gain" => gain]; omitted="")
        }
        Command::InsertMainDevice { before, state } => {
            describe!("insert_main_device"; ["before" => before.map(|id| id.value()), "state_type" => state.type_id().as_str(), "state_state_version" => state.state_version(), "state_payload_bytes" => state.payload().len()]; omitted="")
        }
        Command::MoveMainDevice { device, before } => {
            describe!("move_main_device"; ["device" => device.value(), "before" => before.map(|id| id.value())]; omitted="")
        }
        Command::InsertDevice {
            track,
            before,
            state,
        } => {
            describe!("insert_device"; ["track" => track.value(), "before" => before.map(|id| id.value()), "state_type" => state.type_id().as_str(), "state_state_version" => state.state_version(), "state_payload_bytes" => state.payload().len()]; omitted="")
        }
        Command::LoadInstrument {
            track,
            replacing,
            state,
            device_name,
        } => {
            describe!("load_instrument"; ["track" => track.value(), "replacing" => replacing.map(|id| id.value()), "state_type" => state.type_id().as_str(), "state_state_version" => state.state_version(), "state_payload_bytes" => state.payload().len(), "device_name_redacted_bytes" => device_name.len()]; omitted="")
        }
        Command::InsertInstrument { track, state } => {
            describe!("insert_instrument"; ["track" => track.value(), "state_type" => state.type_id().as_str(), "state_state_version" => state.state_version(), "state_payload_bytes" => state.payload().len()]; omitted="")
        }
        Command::RemoveInstrument { track } => {
            describe!("remove_instrument"; ["track" => track.value()]; omitted="")
        }
        Command::RemoveDevice(value0) => {
            describe!("remove_device"; ["value0" => value0.value()]; omitted="")
        }
        Command::MoveDevice {
            device,
            track,
            before,
        } => {
            describe!("move_device"; ["device" => device.value(), "track" => track.value(), "before" => before.map(|id| id.value())]; omitted="")
        }
        Command::MoveTrackDevice {
            device,
            track,
            before,
        } => {
            describe!("move_track_device"; ["device" => device.value(), "track" => track.value(), "before" => before.map(|id| id.value())]; omitted="")
        }
        Command::SpliceDeviceAutomationTouch {
            device,
            target,
            spans,
            ..
        } => {
            describe!("splice_device_automation_touch"; ["device" => device.value(), "target" => target, "span_count" => spans.len()]; omitted="")
        }
        Command::ReplaceDeviceAutomationTarget {
            device,
            target,
            points,
        } => {
            describe!("replace_device_automation_target"; ["device" => device.value(), "target" => target, "points_count" => points.len()]; omitted="")
        }
        Command::ReplaceDeviceAutomation {
            device,
            parameter,
            points,
        } => {
            describe!("replace_device_automation"; ["device" => device.value(), "parameter" => parameter.value(), "points_count" => points.len()]; omitted="")
        }
        Command::ManualDeviceEdit {
            device,
            state,
            targets,
        } => {
            describe!("manual_device_edit"; ["device" => device.value(), "state_type" => state.type_id().as_str(), "state_payload_bytes" => state.payload().len(), "targets_count" => targets.len()]; omitted="")
        }
        Command::SetDeviceAutomationReadSelection {
            device,
            target,
            selection,
        } => {
            describe!("device_automation_read_selection"; ["device" => device.value(), "target" => target, "selection" => format!("{selection:?}")]; omitted="")
        }
        Command::ReenableDeviceAutomation => {
            describe!("reenable_device_automation"; []; omitted="")
        }
        Command::ReplaceDeviceState { device, state } => {
            describe!("replace_device_state"; ["device" => device.value(), "state_type" => state.type_id().as_str(), "state_state_version" => state.state_version(), "state_payload_bytes" => state.payload().len()]; omitted="")
        }
        Command::ReplaceDeviceStateWithIdentityReservation {
            device,
            expected_state,
            state,
            expected_next_id,
            reserved_ids,
            ..
        } => {
            describe!("replace_device_state_with_identity_reservation"; ["device" => device.value(), "expected_state_type" => expected_state.type_id().as_str(), "expected_state_state_version" => expected_state.state_version(), "expected_state_payload_bytes" => expected_state.payload().len(), "state_type" => state.type_id().as_str(), "state_state_version" => state.state_version(), "state_payload_bytes" => state.payload().len(), "expected_next_id" => expected_next_id, "reserved_ids" => reserved_ids]; omitted="expected_variant")
        }
        Command::SetDeviceBypass { device, bypassed } => {
            describe!("set_device_bypass"; ["device" => device.value(), "bypassed" => bypassed]; omitted="")
        }
        Command::ToggleDeviceBypass { device } => {
            describe!("toggle_device_bypass"; ["device" => device.value()]; omitted="")
        }
        Command::CompareDevice { device, .. } => {
            describe!("compare_device"; ["device" => device.value()]; omitted="expected,action")
        }
        Command::ReplaceDeviceComparison {
            device,
            state,
            comparison,
        } => {
            describe!("replace_device_comparison"; ["device" => device.value(), "state_type" => state.type_id().as_str(), "state_state_version" => state.state_version(), "state_payload_bytes" => state.payload().len(), "comparison_present" => comparison.is_some()]; omitted="")
        }
        Command::AddReturnTrack { name } => {
            describe!("add_return_track"; ["name_redacted_bytes" => name.len()]; omitted="")
        }
        Command::AddReturnTrackWithGain { name, gain } => {
            describe!("add_return_track_with_gain"; ["name_redacted_bytes" => name.len(), "gain" => gain]; omitted="")
        }
        Command::RemoveReturnTrack(value0) => {
            describe!("remove_return_track"; ["value0" => value0.value()]; omitted="")
        }
        Command::RenameReturnTrack { return_track, name } => {
            describe!("rename_return_track"; ["return_track" => return_track.value(), "name_redacted_bytes" => name.len()]; omitted="")
        }
        Command::ReturnGain { return_track, gain } => {
            describe!("return_gain"; ["return_track" => return_track.value(), "gain" => gain]; omitted="")
        }
        Command::ReturnMute {
            return_track,
            muted,
        } => {
            describe!("return_mute"; ["return_track" => return_track.value(), "muted" => muted]; omitted="")
        }
        Command::SetReturnSend {
            track,
            return_track,
            gain,
        } => {
            describe!("set_return_send"; ["track" => track.value(), "return_track" => return_track.value(), "gain" => gain]; omitted="")
        }
        Command::InsertReturnDevice {
            return_track,
            before,
            state,
        } => {
            describe!("insert_return_device"; ["return_track" => return_track.value(), "before" => before.map(|id| id.value()), "state_type" => state.type_id().as_str(), "state_state_version" => state.state_version(), "state_payload_bytes" => state.payload().len()]; omitted="")
        }
        Command::MoveReturnDevice { device, before } => {
            describe!("move_return_device"; ["device" => device.value(), "before" => before.map(|id| id.value())]; omitted="")
        }
        Command::DeleteTimeRange { .. } => describe!("delete_time_range"; []; omitted="range"),
        Command::PasteTimeRange { track, frame, .. } => {
            describe!("paste_time_range"; ["track" => track.value(), "frame" => frame]; omitted="clipboard")
        }
        Command::RemoveDevices { .. } => describe!("remove_devices"; []; omitted="owner,selected"),
        Command::PasteDevices { .. } => {
            describe!("paste_devices"; []; omitted="clipboard,owner,before")
        }
        Command::PasteTracks { .. } => {
            describe!("paste_tracks"; []; omitted="clipboard,destination")
        }
        Command::PasteClips {
            track,
            frame,
            insert,
            ..
        } => {
            describe!("paste_clips"; ["track" => track.value(), "frame" => frame, "insert" => insert]; omitted="clipboard")
        }
        Command::PasteTimeRangeOnSourceTracks { track, frame, .. } => {
            describe!("paste_time_range_on_source_tracks"; ["track" => track.value(), "frame" => frame]; omitted="clipboard")
        }
        Command::InsertTimeRange { track, frame, .. } => {
            describe!("insert_time_range"; ["track" => track.value(), "frame" => frame]; omitted="clipboard")
        }
        Command::InsertTimeRangeOnSourceTracks { track, frame, .. } => {
            describe!("insert_time_range_on_source_tracks"; ["track" => track.value(), "frame" => frame]; omitted="clipboard")
        }
        Command::MoveTimeRange { track, frame, .. } => {
            describe!("move_time_range"; ["track" => track.value(), "frame" => frame]; omitted="range")
        }
        Command::CopyTimeRangeTo { track, frame, .. } => {
            describe!("copy_time_range_to"; ["track" => track.value(), "frame" => frame]; omitted="range")
        }
        Command::AddTrack { name } => {
            describe!("add_track"; ["name_redacted_bytes" => name.len()]; omitted="")
        }
        Command::InsertDefaultTrack { before_track } => {
            describe!("insert_default_track"; ["before_track" => before_track.map(|id| id.value())]; omitted="")
        }
        Command::InsertDefaultTrackWithGain { before_track, gain } => {
            describe!("insert_default_track_with_gain"; ["before_track" => before_track.map(|id| id.value()), "gain" => gain]; omitted="")
        }
        Command::InsertTrack { name, before_track } => {
            describe!("insert_track"; ["name_redacted_bytes" => name.len(), "before_track" => before_track.map(|id| id.value())]; omitted="")
        }
        Command::ImportAudioTrack {
            name,
            path,
            sample_rate,
            frames,
        } => {
            describe!("import_audio_track"; ["name_redacted_bytes" => name.len(), "path_redacted_bytes" => path.as_os_str().len(), "sample_rate" => sample_rate, "frames" => frames]; omitted="")
        }
        Command::ImportPreparedAudioTrack {
            name,
            path,
            source_sample_rate,
            source_frames,
            playback_frames,
            conversion_version,
        } => {
            describe!("import_prepared_audio_track"; ["name_redacted_bytes" => name.len(), "path_redacted_bytes" => path.as_os_str().len(), "source_sample_rate" => source_sample_rate, "source_frames" => source_frames, "playback_frames" => playback_frames, "conversion_version" => conversion_version]; omitted="")
        }
        Command::ImportPreparedAudioTrackAt {
            name,
            before_track,
            start,
            path,
            source_sample_rate,
            source_frames,
            playback_frames,
            conversion_version,
        } => {
            describe!("import_prepared_audio_track_at"; ["name_redacted_bytes" => name.len(), "before_track" => before_track.map(|id| id.value()), "start" => start, "path_redacted_bytes" => path.as_os_str().len(), "source_sample_rate" => source_sample_rate, "source_frames" => source_frames, "playback_frames" => playback_frames, "conversion_version" => conversion_version]; omitted="")
        }
        Command::ImportPreparedAudioClip {
            track,
            start,
            path,
            source_sample_rate,
            source_frames,
            playback_frames,
            conversion_version,
        } => {
            describe!("import_prepared_audio_clip"; ["track" => track.value(), "start" => start, "path_redacted_bytes" => path.as_os_str().len(), "source_sample_rate" => source_sample_rate, "source_frames" => source_frames, "playback_frames" => playback_frames, "conversion_version" => conversion_version]; omitted="")
        }
        Command::ImportPreparedAudioClipWithOverwrite {
            track,
            start,
            path,
            source_sample_rate,
            source_frames,
            playback_frames,
            conversion_version,
        } => {
            describe!("import_prepared_audio_clip_with_overwrite"; ["track" => track.value(), "start" => start, "path_redacted_bytes" => path.as_os_str().len(), "source_sample_rate" => source_sample_rate, "source_frames" => source_frames, "playback_frames" => playback_frames, "conversion_version" => conversion_version]; omitted="")
        }
        Command::ImportPreparedSessionAudioClip {
            track,
            scene,
            name,
            path,
            source_sample_rate,
            source_frames,
            playback_frames,
            conversion_version,
            source_start,
            source_end,
        } => {
            describe!("import_prepared_session_audio_clip"; ["track" => track.value(), "scene" => scene.value(), "name_redacted_bytes" => name.len(), "path_redacted_bytes" => path.as_os_str().len(), "source_sample_rate" => source_sample_rate, "source_frames" => source_frames, "playback_frames" => playback_frames, "conversion_version" => conversion_version, "source_start" => source_start, "source_end" => source_end]; omitted="")
        }
        Command::ImportExistingAudio { asset, placement } => match placement {
            ExistingAudioPlacement::NewTrack {
                name,
                before_track,
                start,
            } => {
                describe!("import_existing_audio_track_at"; ["asset" => asset.value(), "name_redacted_bytes" => name.len(), "before_track" => before_track.map(|id| id.value()), "start" => start]; omitted="")
            }
            ExistingAudioPlacement::Clip {
                track,
                start,
                overwrite,
            } => {
                describe!("import_existing_audio_clip"; ["asset" => asset.value(), "track" => track.value(), "start" => start, "overwrite" => overwrite]; omitted="")
            }
            ExistingAudioPlacement::Session {
                track,
                scene,
                name,
                source_start,
                source_end,
            } => {
                describe!("import_existing_session_audio_clip"; ["asset" => asset.value(), "track" => track.value(), "scene" => scene.value(), "name_redacted_bytes" => name.len(), "source_start" => source_start, "source_end" => source_end]; omitted="")
            }
        },
        Command::MoveTrackBefore { track, before } => {
            describe!("move_track_before"; ["track" => track.value(), "before" => before.map(|id| id.value())]; omitted="")
        }
        Command::MoveTracksBefore { tracks, before } => {
            describe!("move_tracks_before"; ["tracks_count" => tracks.ids().len(), "tracks_first_ids" => FirstIds(tracks.ids()), "before" => before.map(|id| id.value())]; omitted="")
        }
        Command::RemoveTrack(value0) => {
            describe!("remove_track"; ["value0" => value0.value()]; omitted="")
        }
        Command::RemoveTracks(value0) => {
            describe!("remove_tracks"; ["value0_count" => value0.len()]; omitted="")
        }
        Command::SetTrackTags { track, tags } => {
            describe!("set_track_tags"; ["track" => track.value(), "tags_count" => tags.len()]; omitted="tag_text")
        }
        Command::RenameTrack { track, name } => {
            describe!("rename_track"; ["track" => track.value(), "name_redacted_bytes" => name.len()]; omitted="")
        }
        Command::Gain { track, gain } => {
            describe!("gain"; ["track" => track.value(), "gain" => gain]; omitted="")
        }
        Command::SetTrackGains { gains } => {
            describe!("set_track_gains"; ["tracks_count" => gains.len()]; omitted="gains")
        }
        Command::Balance { track, balance } => {
            describe!("balance"; ["track" => track.value(), "balance" => balance]; omitted="")
        }
        Command::MasterGain { gain } => describe!("master_gain"; ["gain" => gain]; omitted=""),
        Command::SetProjectTempo { .. } => describe!("set_project_tempo"; []; omitted="tempo"),
        Command::SetTempoPoint { beat, .. } => {
            describe!("set_tempo_point"; ["beat_beats_q32" => beat.raw()]; omitted="tempo")
        }
        Command::SetMeterPoint { beat, .. } => {
            describe!("set_meter_point"; ["beat_beats_q32" => beat.raw()]; omitted="meter")
        }
        Command::Mute { track, muted } => {
            describe!("mute"; ["track" => track.value(), "muted" => muted]; omitted="")
        }
        Command::ToggleSoloTracks { tracks, exclusive } => {
            describe!("toggle_solo_tracks"; ["tracks_count" => tracks.ids().len(), "tracks_first_ids" => FirstIds(tracks.ids()), "exclusive" => exclusive]; omitted="")
        }
        Command::ToggleSolo { track, exclusive } => {
            describe!("toggle_solo"; ["track" => track.value(), "exclusive" => exclusive]; omitted="")
        }
        Command::SetLoopRegion {
            start,
            end,
            enabled,
        } => {
            describe!("set_loop_region"; ["start" => start, "end" => end, "enabled" => enabled]; omitted="")
        }
        Command::ToggleLoopRegion => describe!("toggle_loop_region"; []; omitted=""),
        Command::ClearLoopRegion => describe!("clear_loop_region"; []; omitted=""),
        Command::AddAsset {
            path,
            sample_rate,
            frames,
        } => {
            describe!("add_asset"; ["path_redacted_bytes" => path.as_os_str().len(), "sample_rate" => sample_rate, "frames" => frames]; omitted="")
        }
        Command::ImportDeviceAsset {
            path,
            sample_rate,
            frames,
        } => {
            describe!("import_device_asset"; ["path_redacted_bytes" => path.as_os_str().len(), "sample_rate" => sample_rate, "frames" => frames]; omitted="")
        }
        Command::ImportDeviceAssetIntoEmptyInstrument {
            track,
            path,
            sample_rate,
            frames,
            state,
            ..
        } => {
            describe!("import_device_asset_into_empty_instrument"; ["track" => track.value(), "path_redacted_bytes" => path.as_os_str().len(), "sample_rate" => sample_rate, "frames" => frames, "state_type" => state.type_id().as_str(), "state_state_version" => state.state_version(), "state_payload_bytes" => state.payload().len()]; omitted="source_slot")
        }
        Command::ImportDeviceAssetReplacingInstrument {
            track,
            device,
            expected_state,
            path,
            sample_rate,
            frames,
            state,
            ..
        } => {
            describe!("import_device_asset_replacing_instrument"; ["track" => track.value(), "device" => device.value(), "expected_state_type" => expected_state.type_id().as_str(), "expected_state_state_version" => expected_state.state_version(), "expected_state_payload_bytes" => expected_state.payload().len(), "path_redacted_bytes" => path.as_os_str().len(), "sample_rate" => sample_rate, "frames" => frames, "state_type" => state.type_id().as_str(), "state_state_version" => state.state_version(), "state_payload_bytes" => state.payload().len()]; omitted="source_slot")
        }
        Command::RelinkAssets(relinks) => {
            describe!("relink_assets"; ["count" => relinks.len()]; omitted="paths")
        }
        Command::RemoveAsset(value0) => {
            describe!("remove_asset"; ["value0" => value0.value()]; omitted="")
        }
        Command::PlaceClip {
            track,
            asset,
            start,
            duration,
            source_offset,
            source_length,
            looping,
        } => {
            describe!("place_clip"; ["track" => track.value(), "asset" => asset.value(), "start" => start, "duration" => duration, "source_offset" => source_offset, "source_length" => source_length, "looping" => looping]; omitted="")
        }
        Command::DuplicateClip(value0) => {
            describe!("duplicate_clip"; ["value0" => value0.value()]; omitted="")
        }
        Command::DuplicateClips(value0) => {
            describe!("duplicate_clips"; ["value0_count" => value0.len()]; omitted="")
        }
        Command::GroupClips(value0) => {
            describe!("group_clips"; ["value0_count" => value0.len()]; omitted="")
        }
        Command::GroupTimeRange(_) => describe!("group_time_range"; []; omitted="value0"),
        Command::GroupAudioEventRange { clip, start, end } => {
            describe!("group_audio_event_range"; ["clip" => clip.value(), "start" => start, "end" => end]; omitted="")
        }
        Command::ReverseClips(value0) => {
            describe!("reverse_clips"; ["value0_count" => value0.len()]; omitted="")
        }
        Command::SplitClip { clip, frame } => {
            describe!("split_clip"; ["clip" => clip.value(), "frame" => frame]; omitted="")
        }
        Command::SplitTracksAtFrame { tracks, frame } => {
            describe!("split_tracks_at_frame"; ["tracks_count" => tracks.len(), "frame" => frame]; omitted="")
        }
        Command::SplitTimeRange { .. } => describe!("split_time_range"; []; omitted="range"),
        Command::TrimClipEdge { clip, frame, .. } => {
            describe!("trim_clip_edge"; ["clip" => clip.value(), "frame" => frame]; omitted="edge")
        }
        Command::TrimClipFadeEdge { clip, frame, .. } => {
            describe!("trim_clip_fade_edge"; ["clip" => clip.value(), "frame" => frame]; omitted="edge")
        }
        Command::MoveClip { clip, track, start } => {
            describe!("move_clip"; ["clip" => clip.value(), "track" => track.value(), "start" => start]; omitted="")
        }
        Command::MoveClipWithOverwrite { clip, track, start } => {
            describe!("move_clip_with_overwrite"; ["clip" => clip.value(), "track" => track.value(), "start" => start]; omitted="")
        }
        Command::MoveClipsWithOverwrite {
            clips,
            anchor,
            track,
            copy,
            ..
        } => {
            describe!("move_clips_with_overwrite"; ["clips_count" => clips.len(), "anchor" => anchor.value(), "track" => track.value(), "copy" => copy]; omitted="position")
        }
        Command::MoveClipToNewTrack {
            clip,
            name,
            before_track,
            start,
            initial_gain,
        } => {
            describe!("move_clip_to_new_track"; ["clip" => clip.value(), "name_redacted_bytes" => name.len(), "before_track" => before_track.map(|id| id.value()), "start" => start, "initial_gain" => initial_gain]; omitted="")
        }
        Command::CopyClipTo { clip, track, start } => {
            describe!("copy_clip_to"; ["clip" => clip.value(), "track" => track.value(), "start" => start]; omitted="")
        }
        Command::CopyClipToNewTrack {
            clip,
            name,
            before_track,
            start,
            initial_gain,
        } => {
            describe!("copy_clip_to_new_track"; ["clip" => clip.value(), "name_redacted_bytes" => name.len(), "before_track" => before_track.map(|id| id.value()), "start" => start, "initial_gain" => initial_gain]; omitted="")
        }
        Command::TrimClip {
            clip,
            start,
            duration,
            source_offset,
            source_length,
        } => {
            describe!("trim_clip"; ["clip" => clip.value(), "start" => start, "duration" => duration, "source_offset" => source_offset, "source_length" => source_length]; omitted="")
        }
        Command::CreateArrangementFades { .. } => {
            describe!("create_arrangement_fades"; []; omitted="range")
        }
        Command::CreateAudioEventFades { clip, start, end } => {
            describe!("create_audio_event_fades"; ["clip" => clip.value(), "start" => start, "end" => end]; omitted="")
        }
        Command::SetClipFades {
            clip,
            fade_in,
            fade_out,
        } => {
            describe!("set_clip_fades"; ["clip" => clip.value(), "fade_in" => fade_in, "fade_out" => fade_out]; omitted="")
        }
        Command::SetClipFadeEnvelope { clip, .. } => {
            describe!("set_clip_fade_envelope"; ["clip" => clip.value()]; omitted="fade")
        }
        Command::SetAudioClipGain { clip, millidb } => {
            describe!("set_audio_clip_gain"; ["clip" => clip.value(), "millidb" => millidb]; omitted="")
        }
        Command::SetAudioClipPitch { clip, cents } => {
            describe!("set_audio_clip_pitch"; ["clip" => clip.value(), "cents" => cents]; omitted="")
        }
        Command::SetAudioClipMusical { clip, enabled } => {
            describe!("set_audio_clip_musical"; ["clip" => clip.value(), "enabled" => enabled]; omitted="")
        }
        Command::SetAudioClipSampleBpm { clip, .. } => {
            describe!("set_audio_clip_sample_bpm"; ["clip" => clip.value()]; omitted="tempo")
        }
        Command::StretchAudioClipEdge { clip, frame, .. } => {
            describe!("stretch_audio_clip_edge"; ["clip" => clip.value(), "frame" => frame]; omitted="edge")
        }
        Command::ResizeAudioClipView { clip, .. } => {
            describe!("resize_audio_clip_view"; ["clip" => clip.value()]; omitted="operation")
        }
        Command::SetAudioClipWarp { clip, settings } => {
            describe!("set_audio_clip_warp"; ["clip" => clip.value(), "settings_present" => settings.is_some()]; omitted="")
        }
        Command::SetAudioClipSourceGain { clip, points } => {
            describe!("set_audio_clip_source_gain"; ["clip" => clip.value(), "points_count" => points.len()]; omitted="")
        }
        Command::SetClipLooping { clip, looping } => {
            describe!("set_clip_looping"; ["clip" => clip.value(), "looping" => looping]; omitted="")
        }
        Command::SetClipSourcePhase { clip, phase } => {
            describe!("set_clip_source_phase"; ["clip" => clip.value(), "phase" => phase]; omitted="")
        }
        Command::RemoveClip(value0) => {
            describe!("remove_clip"; ["value0" => value0.value()]; omitted="")
        }
        Command::NudgeClip { clip, frames } => {
            describe!("nudge_clip"; ["clip" => clip.value(), "frames" => frames]; omitted="")
        }
        Command::TrimClipEndBy { clip, frames } => {
            describe!("trim_clip_end_by"; ["clip" => clip.value(), "frames" => frames]; omitted="")
        }
        Command::GainBy { track, amount } => {
            describe!("gain_by"; ["track" => track.value(), "amount" => amount]; omitted="")
        }
        Command::BalanceBy { track, amount } => {
            describe!("balance_by"; ["track" => track.value(), "amount" => amount]; omitted="")
        }
        Command::MasterGainBy { amount } => {
            describe!("master_gain_by"; ["amount" => amount]; omitted="")
        }
        Command::ToggleMute(value0) => {
            describe!("toggle_mute"; ["value0" => value0.value()]; omitted="")
        }
        Command::ToggleTracksMute(value0) => {
            describe!("toggle_tracks_mute"; ["value0_count" => value0.ids().len(), "value0_first_ids" => FirstIds(value0.ids())]; omitted="")
        }
        Command::AddMidiTrack { name } => {
            describe!("add_midi_track"; ["name_redacted_bytes" => name.len()]; omitted="")
        }
        Command::AddScene { name } => {
            describe!("add_scene"; ["name_redacted_bytes" => name.len()]; omitted="")
        }
        Command::InsertScene { name, before } => {
            describe!("insert_scene"; ["name_redacted_bytes" => name.len(), "before" => before.map(|id| id.value())]; omitted="")
        }
        Command::DuplicateScene(value0) => {
            describe!("duplicate_scene"; ["value0" => value0.value()]; omitted="")
        }
        Command::MoveScene { scene, before } => {
            describe!("move_scene"; ["scene" => scene.value(), "before" => before.map(|id| id.value())]; omitted="")
        }
        Command::RenameScene { scene, name } => {
            describe!("rename_scene"; ["scene" => scene.value(), "name_redacted_bytes" => name.len()]; omitted="")
        }
        Command::RemoveScene(value0) => {
            describe!("remove_scene"; ["value0" => value0.value()]; omitted="")
        }
        Command::CreateSessionMidiClip {
            track,
            scene,
            name,
            source_length,
        } => {
            describe!("create_session_midi_clip"; ["track" => track.value(), "scene" => scene.value(), "name_redacted_bytes" => name.len(), "source_length_beats_q32" => source_length.raw()]; omitted="")
        }
        Command::CreateSessionAudioClip {
            track,
            scene,
            name,
            asset,
            source_start,
            source_end,
        } => {
            describe!("create_session_audio_clip"; ["track" => track.value(), "scene" => scene.value(), "name_redacted_bytes" => name.len(), "asset" => asset.value(), "source_start" => source_start, "source_end" => source_end]; omitted="")
        }
        Command::CopyArrangementAudioClipToSession { source, scene } => {
            describe!("copy_arrangement_audio_clip_to_session"; ["source" => source.value(), "scene" => scene.value()]; omitted="")
        }
        Command::CopyArrangementMidiToSession { source, scene } => {
            describe!("copy_arrangement_midi_to_session"; ["source" => source.value(), "scene" => scene.value()]; omitted="")
        }
        Command::CopyArrangementAudioClipToNewSessionTrack {
            source,
            before_track,
            initial_gain,
            ..
        } => {
            describe!("copy_arrangement_audio_clip_to_new_session_track"; ["source" => source.value(), "before_track" => before_track.map(|id| id.value()), "initial_gain" => initial_gain]; omitted="scene")
        }
        Command::CopyArrangementMidiToNewSessionTrack {
            source,
            before_track,
            initial_gain,
            ..
        } => {
            describe!("copy_arrangement_midi_to_new_session_track"; ["source" => source.value(), "before_track" => before_track.map(|id| id.value()), "initial_gain" => initial_gain]; omitted="scene")
        }
        Command::MoveSessionCell {
            from_track,
            from_scene,
            to_track,
            to_scene,
        } => {
            describe!("move_session_cell"; ["from_track" => from_track.value(), "from_scene" => from_scene.value(), "to_track" => to_track.value(), "to_scene" => to_scene.value()]; omitted="")
        }
        Command::DuplicateSessionMidiCell {
            track,
            from_scene,
            to_scene,
        } => {
            describe!("duplicate_session_midi_cell"; ["track" => track.value(), "from_scene" => from_scene.value(), "to_scene" => to_scene.value()]; omitted="")
        }
        Command::DuplicateSessionAudioCell {
            track,
            from_scene,
            to_scene,
        } => {
            describe!("duplicate_session_audio_cell"; ["track" => track.value(), "from_scene" => from_scene.value(), "to_scene" => to_scene.value()]; omitted="")
        }
        Command::SetSessionAudioClipLoop { clip, loop_range } => {
            describe!("set_session_audio_clip_loop"; ["clip" => clip.value(), "loop_range_present" => loop_range.is_some()]; omitted="")
        }
        Command::SetSessionAudioClipSource {
            clip,
            asset,
            source_start,
            source_end,
        } => {
            describe!("set_session_audio_clip_source"; ["clip" => clip.value(), "asset" => asset.value(), "source_start" => source_start, "source_end" => source_end]; omitted="")
        }
        Command::ClearSessionCell { track, scene } => {
            describe!("clear_session_cell"; ["track" => track.value(), "scene" => scene.value()]; omitted="")
        }
        Command::SetSessionStopCell { track, scene } => {
            describe!("set_session_stop_cell"; ["track" => track.value(), "scene" => scene.value()]; omitted="")
        }
        Command::SetSessionMidiClipSource {
            clip,
            source_length,
            source_start,
            loop_start,
            loop_end,
            looping,
        } => {
            describe!("set_session_midi_clip_source"; ["clip" => clip.value(), "source_length_beats_q32" => source_length.raw(), "source_start_beats_q32" => source_start.raw(), "loop_start_beats_q32" => loop_start.raw(), "loop_end_beats_q32" => loop_end.raw(), "looping" => looping]; omitted="")
        }
        Command::RenameSessionMidiClip { clip, name } => {
            describe!("rename_session_midi_clip"; ["clip" => clip.value(), "name_redacted_bytes" => name.len()]; omitted="")
        }
        Command::PlaceMidiClip {
            track,
            start,
            length,
        } => {
            describe!("place_midi_clip"; ["track" => track.value(), "start_beats_q32" => start.raw(), "length_beats_q32" => length.raw()]; omitted="")
        }
        Command::PlaceMidiClipWithOverwrite {
            track,
            start,
            length,
        } => {
            describe!("place_midi_clip_with_overwrite"; ["track" => track.value(), "start_beats_q32" => start.raw(), "length_beats_q32" => length.raw()]; omitted="")
        }
        Command::TrimMidiClipEdge { clip, beat, .. } => {
            describe!("trim_midi_clip_edge"; ["clip" => clip.value(), "beat_beats_q32" => beat.raw()]; omitted="edge")
        }
        Command::StretchMidiClipEdge { clip, beat, .. } => {
            describe!("stretch_midi_clip_edge"; ["clip" => clip.value(), "beat_beats_q32" => beat.raw()]; omitted="edge")
        }
        Command::SplitMidiClip { clip, beat } => {
            describe!("split_midi_clip"; ["clip" => clip.value(), "beat_beats_q32" => beat.raw()]; omitted="")
        }
        Command::SetMidiClipSource {
            clip,
            source_length,
            source_start,
            loop_start,
            loop_end,
            looping,
            length,
        } => {
            describe!("set_midi_clip_source"; ["clip" => clip.value(), "source_length_beats_q32" => source_length.raw(), "source_start_beats_q32" => source_start.raw(), "loop_start_beats_q32" => loop_start.raw(), "loop_end_beats_q32" => loop_end.raw(), "looping" => looping, "length_beats_q32" => length.raw()]; omitted="")
        }
        Command::ResizeMidiClipView { clip, .. } => {
            describe!("resize_midi_clip_view"; ["clip" => clip.value()]; omitted="operation")
        }
        Command::ReplaceMidiClipVolume { clip, .. } => {
            describe!("replace_midi_clip_volume"; ["clip" => clip.value()]; omitted="volume")
        }
        Command::SetMidiClipGain { clip, gain } => {
            describe!("set_midi_clip_gain"; ["clip" => clip.value(), "millidb" => gain.millidb()]; omitted="")
        }
        Command::AddMidiNote {
            clip,
            start,
            duration,
            pitch,
            channel,
            velocity,
        } => {
            describe!("add_midi_note"; ["clip" => clip.value(), "start_beats_q32" => start.raw(), "duration_beats_q32" => duration.raw(), "pitch" => pitch, "channel" => channel, "velocity" => velocity]; omitted="")
        }
        Command::AddMidiNotes { clip, notes } => {
            describe!("add_midi_notes"; ["clip" => clip.value(), "notes_count" => notes.len()]; omitted="")
        }
        Command::EditMidiNotes {
            clip,
            remove,
            insert,
        } => {
            describe!("edit_midi_notes"; ["clip" => clip.value(), "remove_count" => remove.len(), "insert_count" => insert.len()]; omitted="")
        }
        Command::EditMidiNotesWithSourceExtent {
            clip,
            remove,
            insert,
        } => {
            describe!("edit_midi_notes_with_source_extent"; ["clip" => clip.value(), "remove_count" => remove.len(), "insert_count" => insert.len()]; omitted="")
        }
        Command::DuplicateMidiSourceRange { clip, start, end } => {
            describe!("duplicate_midi_source_range"; ["clip" => clip.value(), "start_beats_q32" => start.raw(), "end_beats_q32" => end.raw()]; omitted="")
        }
        Command::UpdateMidiNote {
            note,
            start,
            duration,
            pitch,
            channel,
            velocity,
        } => {
            describe!("update_midi_note"; ["note" => note.value(), "start_beats_q32" => start.raw(), "duration_beats_q32" => duration.raw(), "pitch" => pitch, "channel" => channel, "velocity" => velocity]; omitted="")
        }
        Command::UpdateMidiNotesWithSourcePrefix {
            clip,
            prefix,
            notes,
        } => {
            describe!("update_midi_notes_with_source_prefix"; ["clip" => clip.value(),"prefix_beats_q32" => prefix.raw(),"notes_count" => notes.len()]; omitted="note_data")
        }
        Command::UpdateMidiNotesWithSourceExtent { clip, notes } => {
            describe!("update_midi_notes_with_source_extent"; ["clip" => clip.value(), "notes_count" => notes.len()]; omitted="")
        }
        Command::UpdateMidiNotes { clip, notes } => {
            describe!("update_midi_notes"; ["clip" => clip.value(), "notes_count" => notes.len()]; omitted="")
        }
        Command::SetMidiNotesActive {
            clip,
            notes,
            active,
        } => {
            describe!("set_midi_notes_active"; ["clip" => clip.value(), "notes_count" => notes.len(), "active" => active]; omitted="")
        }
        Command::RemoveMidiNote(value0) => {
            describe!("remove_midi_note"; ["value0" => value0.value()]; omitted="")
        }
        Command::RemoveMidiNotes { clip, notes } => {
            describe!("remove_midi_notes"; ["clip" => clip.value(), "notes_count" => notes.len()]; omitted="")
        }
        Command::MoveMidiClip { clip, track, start } => {
            describe!("move_midi_clip"; ["clip" => clip.value(), "track" => track.value(), "start_beats_q32" => start.raw()]; omitted="")
        }
        Command::MoveMidiClipWithOverwrite { clip, track, start } => {
            describe!("move_midi_clip_with_overwrite"; ["clip" => clip.value(), "track" => track.value(), "start_beats_q32" => start.raw()]; omitted="")
        }
        Command::MoveMidiClipToNewTrack {
            clip,
            name,
            before_track,
            start,
            initial_gain,
        } => {
            describe!("move_midi_clip_to_new_track"; ["clip" => clip.value(), "name_redacted_bytes" => name.len(), "before_track" => before_track.map(|id| id.value()), "start_beats_q32" => start.raw(), "initial_gain" => initial_gain]; omitted="")
        }
        Command::CopyMidiClipTo { clip, track, start } => {
            describe!("copy_midi_clip_to"; ["clip" => clip.value(), "track" => track.value(), "start_beats_q32" => start.raw()]; omitted="")
        }
        Command::CopyMidiClipToNewTrack {
            clip,
            name,
            before_track,
            start,
            initial_gain,
        } => {
            describe!("copy_midi_clip_to_new_track"; ["clip" => clip.value(), "name_redacted_bytes" => name.len(), "before_track" => before_track.map(|id| id.value()), "start_beats_q32" => start.raw(), "initial_gain" => initial_gain]; omitted="")
        }
        Command::RemoveMidiClip(value0) => {
            describe!("remove_midi_clip"; ["value0" => value0.value()]; omitted="")
        }
        Command::DuplicateMidiClip(value0) => {
            describe!("duplicate_midi_clip"; ["value0" => value0.value()]; omitted="")
        }
    }
}

pub(super) fn request(value: &Request) -> Description {
    match value {
        Request::PaintMidiClip(intent) => {
            describe!("paint_midi_clip"; ["project_generation" => intent.project_generation, "revision" => intent.revision, "track" => intent.track.value(), "start" => intent.start.raw(), "length" => intent.length.raw()]; omitted="")
        }
        Request::DeviceClipboard {
            project_generation,
            revision,
            ..
        } => {
            describe!("device_clipboard"; ["project_generation" => project_generation, "revision" => revision]; omitted="action")
        }
        Request::TrackClipboard {
            project_generation,
            revision,
            action,
        } => {
            let mut description = describe!("track_clipboard"; ["project_generation" => project_generation, "revision" => revision]; omitted="");
            description.nested("action", self::track_clipboard(action));
            description
        }
        Request::ReorderTracks {
            project_generation,
            revision,
            tracks,
            before,
        } => {
            describe!("reorder_tracks"; ["project_generation" => project_generation, "revision" => revision, "tracks_count" => tracks.ids().len(), "tracks_first_ids" => FirstIds(tracks.ids()), "before" => before.map(|id| id.value())]; omitted="")
        }
        Request::ReparentTracks {
            project_generation,
            revision,
            tracks,
            destination,
        } => {
            describe!("reparent_tracks"; ["project_generation" => project_generation, "revision" => revision, "tracks_count" => tracks.ids().len(), "tracks_first_ids" => FirstIds(tracks.ids()), "parent" => destination.parent_group.map(|id| id.value()), "before" => destination.before.map(|id| id.value())]; omitted="")
        }
        Request::EditArrangementSection {
            project_generation,
            revision,
            command,
        } => {
            let mut description = describe!("edit_arrangement_section"; ["project_generation" => project_generation, "revision" => revision]; omitted="");
            description.nested("command", self::command(command));
            description
        }
        Request::MoveExistingClips {
            project_generation,
            revision,
            command,
        } => {
            let mut description = describe!("move_existing_clips"; ["project_generation" => project_generation, "revision" => revision]; omitted="");
            description.nested("command", self::command(command));
            description
        }
        Request::DuplicateClips {
            project_generation,
            revision,
            clips,
        } => {
            describe!("duplicate_clips"; ["project_generation" => project_generation, "revision" => revision, "clips_count" => clips.len()]; omitted="")
        }
        Request::DuplicateAliasClips {
            project_generation,
            revision,
            clips,
        } => {
            describe!("duplicate_alias_clips"; ["project_generation" => project_generation, "revision" => revision, "clips_count" => clips.len()]; omitted="")
        }
        Request::DuplicateSessionCell {
            project_generation,
            revision,
            track,
            scene,
            alias,
        } => {
            describe!("duplicate_session_cell"; ["project_generation" => project_generation, "revision" => revision, "track" => track.value(), "scene" => scene.value(), "alias" => alias]; omitted="")
        }
        Request::GroupClips {
            project_generation,
            revision,
            clips,
        } => {
            describe!("group_clips"; ["project_generation" => project_generation, "revision" => revision, "clips_count" => clips.len()]; omitted="")
        }
        Request::GroupTimeRange {
            project_generation,
            revision,
            ..
        } => {
            describe!("group_time_range"; ["project_generation" => project_generation, "revision" => revision]; omitted="range")
        }
        Request::GroupAudioEvents {
            project_generation,
            revision,
            clip,
            start,
            end,
        } => {
            describe!("group_audio_events"; ["project_generation" => project_generation, "revision" => revision, "clip" => clip.value(), "start" => start, "end" => end]; omitted="")
        }
        Request::RestartRecovery(_) => describe!("restart_recovery"; []; omitted="value0"),
        Request::ResidentArrangementSeek(_) => {
            describe!("resident_arrangement_seek"; []; omitted="value0")
        }
        Request::PrepareTuningAudition { revision, .. } => {
            describe!("prepare_tuning_audition"; ["revision" => revision]; omitted="input")
        }
        Request::PrepareLiveExpression { revision, .. } => {
            describe!("prepare_live_expression"; ["revision" => revision]; omitted="input")
        }
        Request::RestartAudio {
            project_generation,
            request_id,
        } => {
            describe!("restart_audio"; ["project_generation" => project_generation, "request_id" => request_id]; omitted="")
        }
        Request::PrepareComputerKeyboard {
            project_generation,
            revision,
            request_id,
            track,
            ..
        } => {
            describe!("prepare_computer_keyboard"; ["project_generation" => project_generation, "revision" => revision, "request_id" => request_id, "track" => track.value()]; omitted="input")
        }
        Request::SaveDevicePreset { path, .. } => {
            describe!("save_device_preset"; ["path_redacted_bytes" => path.as_os_str().len()]; omitted="owner")
        }
        Request::LoadDevicePreset { path, .. } => {
            describe!("load_device_preset"; ["path_redacted_bytes" => path.as_os_str().len()]; omitted="owner")
        }
        Request::InsertBrowserInstrumentTrack {
            project_generation,
            revision,
            type_id,
            new_track_gain,
            ..
        } => {
            describe!("insert_browser_instrument_track"; ["project_generation" => project_generation, "revision" => revision, "type_id" => type_id.as_str(), "new_track_gain" => new_track_gain]; omitted="class")
        }
        Request::InsertPluginEffect(_) => describe!("insert_plugin_effect"; []; omitted="value0"),
        Request::ContinuePluginEffect => describe!("continue_plugin_effect"; []; omitted=""),
        Request::TogglePluginEditor {
            project_generation,
            device,
        } => {
            describe!("toggle_plugin_editor"; ["project_generation" => project_generation, "device" => device.value()]; omitted="")
        }
        Request::ContinuePluginEditor => describe!("continue_plugin_editor"; []; omitted=""),
        Request::QueryPluginParameters(query) => {
            describe!("query_plugin_parameters"; ["device" => query.device.value(), "request_id" => query.request_id]; omitted="")
        }
        Request::ContinuePluginParameters => {
            describe!("continue_plugin_parameters"; []; omitted="")
        }
        Request::ContinuePluginStateRestore => {
            describe!("continue_plugin_state_restore"; []; omitted="")
        }
        Request::InsertEffectBusDevice {
            project_generation,
            revision,
            destination,
            type_id,
            before,
            ..
        } => {
            let mut description = describe!("insert_effect_bus_device"; ["project_generation" => project_generation, "revision" => revision, "type_id" => type_id.as_str(), "before" => before.map(|id| id.value())]; omitted="class");
            description.nested("destination", self::effect_bus_destination(destination));
            description
        }
        Request::DropBrowserDeviceOnArrangement {
            project_generation,
            revision,
            type_id,
            new_track_gain,
            ..
        } => {
            describe!("drop_browser_device_on_arrangement"; ["project_generation" => project_generation, "revision" => revision, "type_id" => type_id.as_str(), "new_track_gain" => new_track_gain]; omitted="class,destination")
        }
        Request::CopyArrangementClipToConsoleTrack {
            project_generation,
            revision,
            source,
            before_track,
            initial_gain,
            ..
        } => {
            describe!("copy_arrangement_clip_to_console_track"; ["project_generation" => project_generation, "revision" => revision, "source" => source.value(), "before_track" => before_track.map(|id| id.value()), "initial_gain" => initial_gain]; omitted="scene")
        }
        Request::CommitMidiTake {
            request_id,
            project_generation,
            revision,
            notes,
            stop_frame,
            ..
        } => {
            describe!("commit_midi_take"; ["request_id" => request_id, "project_generation" => project_generation, "revision" => revision, "notes_count" => notes.len(), "stop_frame" => stop_frame]; omitted="binding")
        }
        Request::DeviceAutomationTake {
            request_id,
            scope,
            action: _,
        } => {
            describe!("device_automation_take"; ["request_id" => request_id, "gesture_serial" => scope.stamp.gesture_serial]; omitted="delivery data")
        }
        Request::RecoverDeviceAutomationTake {
            request_id,
            context,
            expected,
            ..
        } => {
            describe!("recover_device_automation_take"; ["request_id" => request_id, "project_generation" => context.project_generation, "revision" => context.editor_revision, "gesture_serial" => expected.gesture_serial]; omitted="path and captured source")
        }
        Request::ArmNativeParameterRecording {
            project_generation,
            revision,
            target,
        } => {
            describe!("arm_native_parameter_recording"; ["project_generation" => project_generation, "revision" => revision, "armed" => target.is_some()]; omitted="")
        }
        Request::ArmGainRecording {
            project_generation,
            revision,
            track,
        } => {
            describe!("arm_gain_recording"; ["project_generation" => project_generation, "revision" => revision, "track" => track.value()]; omitted="")
        }
        Request::GainLane {
            project_generation,
            revision,
            track,
            edit,
        } => {
            let mut description = describe!("gain_lane"; ["project_generation" => project_generation, "revision" => revision, "track" => track.value()]; omitted="");
            description.nested("edit", self::gain_lane(edit));
            description
        }
        Request::DeviceAutomation {
            project_generation,
            revision,
            device,
            target,
            points,
            ..
        } => {
            describe!("device_automation"; ["project_generation" => project_generation, "revision" => revision, "device" => device.value(), "target" => target, "points_count" => points.len()]; omitted="")
        }
        Request::DeviceAutomationControl {
            project_generation,
            revision,
            edit,
        } => {
            describe!("device_automation_control"; ["project_generation" => project_generation, "revision" => revision, "edit" => format!("{edit:?}")]; omitted="")
        }
        Request::DeviceGesture(value0) => {
            let mut description = describe!("device_gesture"; []; omitted="");
            description.nested("value0", self::gesture(value0));
            description
        }
        Request::TuningGesture(value0) => {
            let mut description = describe!("tuning_gesture"; []; omitted="");
            description.nested("value0", self::tuning_gesture(value0));
            description
        }
        Request::TuningLibrary {
            project_generation,
            revision,
            operation_id,
            operation,
        } => {
            let mut description = describe!("tuning_library"; ["project_generation" => project_generation, "revision" => revision, "operation_id" => operation_id]; omitted="");
            description.nested("operation", self::tuning_library(operation));
            description
        }
        Request::DropNativeDevice {
            project_generation,
            revision,
            source,
            track,
            before,
            replace,
        } => {
            let mut description = describe!("drop_native_device"; ["project_generation" => project_generation, "revision" => revision, "track" => track.value(), "before" => before.map(|id| id.value()), "replace" => replace.map(|id| id.value())]; omitted="");
            description.nested("source", self::device_drop_source(source));
            description
        }
        Request::AddNativeDevice {
            track,
            type_id,
            before,
            ..
        } => {
            describe!("add_native_device"; ["track" => track.value(), "type_id" => type_id.as_str(), "before" => before.map(|id| id.value())]; omitted="class")
        }
        Request::ReplaceNativeDevice {
            device, type_id, ..
        } => {
            describe!("replace_native_device"; ["device" => device.value(), "type_id" => type_id.as_str()]; omitted="class")
        }
        Request::CompareDevice { .. } => {
            describe!("compare_device"; []; omitted="owner,expected,action")
        }
        Request::SamplerEdit(edit) => {
            let mut description = Description::new("sampler_edit");
            let (generation, revision) = match edit {
                crate::service::SamplerEdit::Parameter(e) => {
                    description.operation = Some("parameter");
                    description.field("track", format_args!("{}", e.track.value()));
                    description.field("device", format_args!("{}", e.device.value()));
                    description.field("parameter", format_args!("{:?}", e.parameter));
                    description.field("value", format_args!("{}", e.value));
                    (e.project_generation, e.revision)
                }
                crate::service::SamplerEdit::Replacement(e) => {
                    description.operation = Some("replacement");
                    description.field("track", format_args!("{}", e.track.value()));
                    description.field("device", format_args!("{}", e.device.value()));
                    description.field(
                        "path_redacted_bytes",
                        format_args!("{}", e.path.as_os_str().len()),
                    );
                    (e.project_generation, e.revision)
                }
                crate::service::SamplerEdit::Midi(e) => {
                    description.nested("command", self::command(&e.command));
                    (e.project_generation, e.revision)
                }
                crate::service::SamplerEdit::History(e) => {
                    description.operation = Some("history");
                    description.field("redo", format_args!("{}", e.redo));
                    (e.project_generation, e.revision)
                }
            };
            description.field("request_id", format_args!("{}", edit.request_id()));
            description.field("generation", format_args!("{generation}"));
            description.field("revision", format_args!("{revision}"));
            description.field(
                "cancelled",
                format_args!(
                    "{}",
                    edit.cancel().load(std::sync::atomic::Ordering::Acquire)
                ),
            );
            description
        }
        Request::ContinueSamplerEdit => describe!("continue_sampler_edit"; []; omitted=""),
        Request::SetDeviceParameter {
            device,
            parameter,
            value,
        } => {
            describe!("set_device_parameter"; ["device" => device.value(), "parameter" => parameter.value(), "value" => Parameter(value)]; omitted="")
        }
        Request::RelinkMedia(relinks) => {
            describe!("relink_media"; ["count" => relinks.len()]; omitted="paths")
        }
        Request::RecoverTruncatedImport(_) => {
            describe!("recover_truncated_import"; []; omitted="request")
        }
        Request::Import(value0) => {
            describe!("import"; ["value0_redacted_bytes" => value0.as_os_str().len()]; omitted="")
        }
        Request::ImportDeviceAsset { path, .. } => {
            describe!("import_device_asset"; ["path_redacted_bytes" => path.as_os_str().len()]; omitted="cancel")
        }
        Request::LoadFirstSampler {
            request_id,
            project_generation,
            revision,
            track,
            empty_sampler,
            path,
            ..
        } => {
            describe!("load_first_sampler"; ["request_id" => request_id, "project_generation" => project_generation, "revision" => revision, "track" => track.value(), "empty_sampler_present" => empty_sampler.is_some(), "path_redacted_bytes" => path.as_os_str().len()]; omitted="cancel")
        }
        Request::AssignSampler { track, asset } => {
            describe!("assign_sampler"; ["track" => track.value(), "asset" => asset.value()]; omitted="")
        }
        Request::ImportAudioBatch { paths, .. } => {
            describe!("import_audio_batch"; ["paths_count" => paths.len()]; omitted="destination,context,control,options")
        }
        Request::ImportToTrack { path, track, start } => {
            describe!("import_to_track"; ["path_redacted_bytes" => path.as_os_str().len(), "track" => track.value(), "start" => start]; omitted="")
        }
        Request::ImportToTrackWithOverwrite { path, track, start } => {
            describe!("import_to_track_with_overwrite"; ["path_redacted_bytes" => path.as_os_str().len(), "track" => track.value(), "start" => start]; omitted="")
        }
        Request::ImportToNewTrack {
            path,
            before_track,
            start,
        } => {
            describe!("import_to_new_track"; ["path_redacted_bytes" => path.as_os_str().len(), "before_track" => before_track.map(|id| id.value()), "start" => start]; omitted="")
        }
        Request::ImportToNewTrackWithGain {
            path,
            before_track,
            start,
            gain,
        } => {
            describe!("import_to_new_track_with_gain"; ["path_redacted_bytes" => path.as_os_str().len(), "before_track" => before_track.map(|id| id.value()), "start" => start, "gain" => gain]; omitted="")
        }
        Request::ImportToSession { path, track, scene } => {
            describe!("import_to_session"; ["path_redacted_bytes" => path.as_os_str().len(), "track" => track.value(), "scene" => scene.value()]; omitted="")
        }
        Request::ChangeProjectSampleRate {
            project_generation,
            revision,
            choice,
        } => {
            let fixed = match choice {
                super::super::ProjectSampleRate::Fixed(rate) => Some(rate),
                super::super::ProjectSampleRate::FollowInterface => None,
            };
            describe!("change_project_sample_rate"; ["project_generation" => project_generation, "revision" => revision, "fixed_rate" => fixed]; omitted="")
        }
        Request::NewWithAudioMode { reservation, .. } => {
            describe!("new_with_audio_mode"; ["reservation_present" => reservation.is_some()]; omitted="mode")
        }
        Request::OpenWithAudioMode { reservation, .. } => {
            describe!("open_with_audio_mode"; ["reservation_present" => reservation.is_some()]; omitted="source,relinks,mode")
        }
        Request::Open(value0) => {
            describe!("open"; ["value0_redacted_bytes" => value0.as_os_str().len()]; omitted="")
        }
        Request::OpenWith { .. } => describe!("open_with"; []; omitted="source,relinks"),
        Request::Edit(value0) => {
            let mut description = describe!("edit"; []; omitted="");
            description.nested("value0", self::command(value0));
            description
        }
        Request::BounceArrangement {
            project_generation,
            revision,
            in_place,
            adoption,
            ..
        } => {
            describe!("bounce_arrangement"; ["project_generation" => project_generation, "revision" => revision, "in_place" => in_place, "adoption" => adoption]; omitted="target,control")
        }
        Request::ConvertClip {
            project_generation,
            revision,
            clip,
            to_audio,
        } => {
            describe!("convert_clip"; ["project_generation" => project_generation, "revision" => revision, "clip" => clip.value(), "to_audio" => to_audio]; omitted="")
        }
        Request::CopyTimeRange(_) => describe!("copy_time_range"; []; omitted="value0"),
        Request::CopyClips(value0) => {
            describe!("copy_clips"; ["value0_count" => value0.len()]; omitted="")
        }
        Request::CutTimeRange(_) => describe!("cut_time_range"; []; omitted="value0"),
        Request::PasteTimeRange { track, frame } => {
            describe!("paste_time_range"; ["track" => track.value(), "frame" => frame]; omitted="")
        }
        Request::InsertTimeRange { track, frame } => {
            describe!("insert_time_range"; ["track" => track.value(), "frame" => frame]; omitted="")
        }
        Request::Undo => describe!("undo"; []; omitted=""),
        Request::Redo => describe!("redo"; []; omitted=""),
        Request::SaveNew(value0) => {
            describe!("save_new"; ["value0_redacted_bytes" => value0.as_os_str().len()]; omitted="")
        }
        Request::SaveCurrent => describe!("save_current"; []; omitted=""),
        Request::SaveCurrentWithFormatUpgrade {
            project_generation,
            path,
        } => {
            describe!("save_current_with_format_upgrade"; ["project_generation" => project_generation, "path_redacted_bytes" => path.as_os_str().len()]; omitted="")
        }
        Request::RestorePlayback { generation } => {
            describe!("restore_playback"; ["generation" => generation]; omitted="")
        }
        Request::RepublishPausedPlayback => describe!("republish_paused_playback"; []; omitted=""),
        Request::RestoreStoppedPlayback => describe!("restore_stopped_playback"; []; omitted=""),
        Request::ContinueStoppedPlaybackRestore => {
            describe!("continue_stopped_playback_restore"; []; omitted="")
        }
        Request::Export(value0) => {
            describe!("export"; ["value0_redacted_bytes" => value0.as_os_str().len()]; omitted="")
        }
        Request::ExportRange {
            path,
            start,
            frames,
            format,
            overwrite,
            ..
        } => {
            describe!("export_range"; ["path_redacted_bytes" => path.as_os_str().len(), "start" => start, "frames" => frames, "format" => format, "overwrite" => overwrite]; omitted="signal,control")
        }
    }
}

pub(super) fn gesture(value: &crate::device_gesture::GestureRequest) -> Description {
    match value {
        crate::device_gesture::GestureRequest::BeginPlugin { key, origin, .. } => {
            describe!("begin_plugin"; ["key_generation" => key.generation, "key_token" => key.token,
                "device" => origin.device.value(), "parameter" => origin.parameter,
                "normalized" => origin.initial.normalized(),
                "project_generation" => origin.context.project_generation,
                "editor_revision" => origin.context.editor_revision,
                "page_first" => origin.page_first, "metadata_revision" => origin.metadata_revision]; omitted="")
        }
        crate::device_gesture::GestureRequest::ChangePlugin {
            key,
            sequence,
            value,
        } => {
            describe!("change_plugin"; ["key_generation" => key.generation, "key_token" => key.token,
                "sequence" => sequence, "normalized" => value.normalized()]; omitted="")
        }
        crate::device_gesture::GestureRequest::CommitPlugin {
            key,
            sequence,
            value,
        } => {
            describe!("commit_plugin"; ["key_generation" => key.generation, "key_token" => key.token,
                "sequence" => sequence, "normalized" => value.normalized()]; omitted="")
        }
        crate::device_gesture::GestureRequest::Begin { key, target } => {
            let mut description = describe!("begin"; ["key_generation" => key.generation, "key_token" => key.token]; omitted="");
            description.nested("target", self::gesture_target(target));
            description
        }
        crate::device_gesture::GestureRequest::Change {
            key,
            sequence,
            value,
        } => {
            describe!("change"; ["key_generation" => key.generation, "key_token" => key.token, "sequence" => sequence, "value" => Parameter(value)]; omitted="")
        }
        crate::device_gesture::GestureRequest::ChangePair {
            key,
            sequence,
            value,
        } => {
            describe!("change_pair"; ["key_generation" => key.generation, "key_token" => key.token, "sequence" => sequence, "value_first" => Parameter(&value.first), "value_second" => Parameter(&value.second)]; omitted="")
        }
        crate::device_gesture::GestureRequest::Commit {
            manual_edit,
            key,
            sequence,
            value,
        } => {
            describe!("commit"; ["key_generation" => key.generation, "key_token" => key.token, "sequence" => sequence, "manual_edit" => manual_edit, "value" => Parameter(value)]; omitted="")
        }
        crate::device_gesture::GestureRequest::CommitPair {
            manual_edit,
            key,
            sequence,
            value,
        } => {
            describe!("commit_pair"; ["key_generation" => key.generation, "key_token" => key.token, "sequence" => sequence, "manual_edit" => manual_edit, "value_first" => Parameter(&value.first), "value_second" => Parameter(&value.second)]; omitted="")
        }
        crate::device_gesture::GestureRequest::BeginTrackGains { key, group } => {
            describe!("begin_track_gains"; ["key_generation" => key.generation, "key_token" => key.token, "anchor" => group.anchor().value(), "tracks_count" => group.len()]; omitted="gains")
        }
        crate::device_gesture::GestureRequest::BeginSnapshot {
            key,
            device,
            snapshot,
        } => {
            describe!("begin_snapshot"; ["key_generation" => key.generation, "key_token" => key.token, "device" => device.value(), "snapshot_count" => snapshot.entries().len(), "snapshot" => Snapshot(snapshot)]; omitted="")
        }
        crate::device_gesture::GestureRequest::ChangeSnapshot {
            touched_parameters,
            key,
            sequence,
            snapshot,
        } => {
            describe!("change_snapshot"; ["key_generation" => key.generation, "key_token" => key.token, "sequence" => sequence, "snapshot_count" => snapshot.entries().len(), "touched_count" => touched_parameters.len(), "snapshot" => Snapshot(snapshot)]; omitted="")
        }
        crate::device_gesture::GestureRequest::CommitSnapshot {
            touched_parameters,
            key,
            sequence,
            snapshot,
        } => {
            describe!("commit_snapshot"; ["key_generation" => key.generation, "key_token" => key.token, "sequence" => sequence, "snapshot_count" => snapshot.entries().len(), "touched_count" => touched_parameters.len(), "snapshot" => Snapshot(snapshot)]; omitted="")
        }
        crate::device_gesture::GestureRequest::BeginEqCreation { key, device, .. } => {
            describe!("begin_eq_creation"; ["key_generation" => key.generation, "key_token" => key.token, "device" => device.value()]; omitted="snapshot")
        }
        crate::device_gesture::GestureRequest::ChangeEqCreation { key, sequence, .. } => {
            describe!("change_eq_creation"; ["key_generation" => key.generation, "key_token" => key.token, "sequence" => sequence]; omitted="snapshot")
        }
        crate::device_gesture::GestureRequest::CommitEqCreation { key, sequence, .. } => {
            describe!("commit_eq_creation"; ["key_generation" => key.generation, "key_token" => key.token, "sequence" => sequence]; omitted="snapshot")
        }
        crate::device_gesture::GestureRequest::Cancel { key } => {
            describe!("cancel"; ["key_generation" => key.generation, "key_token" => key.token]; omitted="")
        }
    }
}

pub(super) fn gesture_target(value: &crate::device_gesture::GestureTarget) -> Description {
    match value {
        crate::device_gesture::GestureTarget::PluginParameter { device, parameter } => {
            describe!("plugin_parameter"; ["device" => device.value(), "parameter" => parameter]; omitted="")
        }
        crate::device_gesture::GestureTarget::TrackGain { track } => {
            describe!("track_gain"; ["track" => track.value()]; omitted="")
        }
        crate::device_gesture::GestureTarget::TrackGains { track } => {
            describe!("track_gains"; ["anchor" => track.value()]; omitted="")
        }
        crate::device_gesture::GestureTarget::MainGain => describe!("main_gain"; []; omitted=""),
        crate::device_gesture::GestureTarget::TrackBalance { track } => {
            describe!("track_balance"; ["track" => track.value()]; omitted="")
        }
        crate::device_gesture::GestureTarget::ReturnGain { return_track } => {
            describe!("return_gain"; ["return_track" => return_track.value()]; omitted="")
        }
        crate::device_gesture::GestureTarget::DeviceParameter { device, parameter } => {
            describe!("device_parameter"; ["device" => device.value(), "parameter" => parameter.value()]; omitted="")
        }
        crate::device_gesture::GestureTarget::DeviceParameterPair {
            device,
            first,
            second,
        } => {
            describe!("device_parameter_pair"; ["device" => device.value(), "first" => first.value(), "second" => second.value()]; omitted="")
        }
        crate::device_gesture::GestureTarget::DeviceParameterSnapshot { device } => {
            describe!("device_parameter_snapshot"; ["device" => device.value()]; omitted="")
        }
        crate::device_gesture::GestureTarget::EqNodeCreation { device } => {
            describe!("eq_node_creation"; ["device" => device.value()]; omitted="")
        }
        crate::device_gesture::GestureTarget::ReturnSend {
            track,
            return_track,
        } => {
            describe!("return_send"; ["track" => track.value(), "return_track" => return_track.value()]; omitted="")
        }
    }
}
