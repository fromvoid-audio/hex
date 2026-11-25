//! Incremental preparation fast-paths for trivial mixer, MIDI-note, and
//! value-only native effect parameter deltas.
//!
//! When an edit touches only scalar mixer fields (Mute, Solo, Track Gain,
//! Balance, Master Gain, Return Gain/Mute, Send/Feed levels), Arrangement MIDI
//! notes on unchanged tracks/instruments, or latency-preserving built-in effect
//! parameters on a single chain, this module produces a `PreparedRevision`
//! semantically identical to full compilation while skipping:
//! - `SignalGraph` / `LatencyPlan` rebuilding
//! - unrelated tracks' audio clip, MIDI, and effect compilation
//! - `PreparedSessionInventory` rebuilding and Sampler PCM factory rescans
//! - `AudioClipWaveform` rebuilding (caller reuses existing waveforms because
//!   `audio_clip_views` is untouched)
//!
//! Any edit outside these proven classes returns `Ok(None)` so the caller falls
//! back to the full compiler with zero loss of validation or fidelity.

use super::*;
use crate::{
    PreparedMasterGain, PreparedTrackMix, TrackSoloSelection, serial_effect_power::EffectPower,
    signal_graph::UpdatedSignalGraph,
};
use hex_lattice_contract::VST3_DEVICE_TYPE_ID;

#[derive(Clone, Copy, PartialEq, Eq)]
enum EffectChainTarget {
    Track(usize),
    Return(usize),
    Main,
}

enum IncrementalDelta {
    /// Only scalar mixer values, mute/solo flags, send gains, or return feed gains changed.
    MixerOnly,
    /// Only Arrangement `midi_clips()` changed on the listed track indices.
    ArrangementMidi { changed_tracks: Vec<usize> },
    /// Only non-VST3 native effect state payloads changed on one chain with identical topology.
    SingleEffectChain(EffectChainTarget),
}

impl PreparedRevision {
    /// Attempt to incrementally derive a new `PreparedRevision` from an already
    /// compiled `(previous_project, previous_revision)` pair.
    ///
    /// Returns `Ok(Some(revision))` when the delta is proven to be a pure mixer
    /// edit, Arrangement MIDI-note edit, or latency-preserving single-chain
    /// native effect parameter edit. The resulting revision leaves
    /// `arrangement().audio_clip_views()` unchanged and preserves
    /// `resource_scope` / `device_resource_scope` so callers can reuse their
    /// retained `PreparedDeviceResources` and `AudioClipWaveform` cache without
    /// rescanning Sampler PCM or rebuilding `SignalGraph`.
    ///
    /// Returns `Ok(None)` for any structural, audio-clip, routing, timing,
    /// tuning, Session, or VST3 change so callers run the full compiler.
    pub fn try_prepare_incremental(
        previous_project: &Project,
        project: &Project,
        previous_revision: &Self,
    ) -> Result<Option<Self>, Invalid> {
        if previous_revision.gain_recording_track.is_some()
            || previous_revision.native_parameter_input.is_some()
            || previous_revision.scalar_automation.is_some()
        {
            return Ok(None);
        }
        let Some(delta) = classify_delta(previous_project, project, previous_revision) else {
            return Ok(None);
        };
        match delta {
            IncrementalDelta::MixerOnly => {
                prepare_mixer_only(previous_project, project, previous_revision).map(Some)
            }
            IncrementalDelta::ArrangementMidi { changed_tracks } => prepare_arrangement_midi(
                previous_project,
                project,
                previous_revision,
                &changed_tracks,
            ),
            IncrementalDelta::SingleEffectChain(target) => {
                prepare_single_effect_chain(project, previous_revision, target)
            }
        }
    }
}

fn classify_delta(
    previous: &Project,
    project: &Project,
    previous_revision: &PreparedRevision,
) -> Option<IncrementalDelta> {
    if previous.sample_rate() != project.sample_rate()
        || previous.tuning() != project.tuning()
        || previous.loop_region() != project.loop_region()
        || previous.tempo_map() != project.tempo_map()
        || previous.meter_map() != project.meter_map()
        || previous.tracks().len() != project.tracks().len()
        || previous.return_tracks().len() != project.return_tracks().len()
        || previous.audio_feeds() != project.audio_feeds()
        || previous.midi_routes() != project.midi_routes()
        || previous.sidechains() != project.sidechains()
        || previous.session() != project.session()
        || previous.native_sources() != project.native_sources()
        || previous.offline_assets().ne(project.offline_assets())
        || previous.arrangement_sections() != project.arrangement_sections()
    {
        return None;
    }
    let arrangement = previous_revision.arrangement();
    if arrangement.sample_rate() != project.sample_rate()
        || arrangement.tracks().len() != project.tracks().len()
        || arrangement.return_buses.len() != project.return_tracks().len()
    {
        return None;
    }
    if previous.assets().ne(project.assets()) {
        return None;
    }
    // Check return-to-return feed topology (identical source/destination order; gains may differ).
    if previous.authored_routing().len() != project.authored_routing().len()
        || previous
            .authored_routing()
            .iter()
            .zip(project.authored_routing())
            .any(|(a, b)| a.source() != b.source() || a.destination() != b.destination())
    {
        return None;
    }
    let feeds_gain_changed = previous
        .authored_routing()
        .iter()
        .zip(project.authored_routing())
        .any(|(a, b)| a.gain() != b.gain());

    let mut mixer_changed = previous.master_gain() != project.master_gain() || feeds_gain_changed;
    let mut midi_changed_tracks = Vec::new();
    let mut effect_target: Option<EffectChainTarget> = None;

    for (index, ((old_track, new_track), compiled_track)) in previous
        .tracks()
        .iter()
        .zip(project.tracks())
        .zip(arrangement.tracks())
        .enumerate()
    {
        if old_track.id() != new_track.id()
            || compiled_track.canonical_track_id() != Some(new_track.id())
            || old_track.kind() != new_track.kind()
            || old_track.parent_group() != new_track.parent_group()
            || old_track.gain_lane() != new_track.gain_lane()
            || old_track.source_id() != new_track.source_id()
            || old_track.retained_source_path() != new_track.retained_source_path()
            || old_track.instrument_position() != new_track.instrument_position()
            || old_track.group_output_mutes() != new_track.group_output_mutes()
            || old_track.instrument() != new_track.instrument()
            || old_track.clips() != new_track.clips()
        {
            return None;
        }
        // Check send target topology (gains may change, send edge list must match).
        if old_track.sends().len() != new_track.sends().len()
            || old_track
                .sends()
                .iter()
                .zip(new_track.sends())
                .any(|(a, b)| a.return_track() != b.return_track())
        {
            return None;
        }
        if old_track.gain() != new_track.gain()
            || old_track.balance() != new_track.balance()
            || old_track.muted() != new_track.muted()
            || old_track.soloed() != new_track.soloed()
            || old_track
                .sends()
                .iter()
                .zip(new_track.sends())
                .any(|(a, b)| a.gain() != b.gain())
        {
            mixer_changed = true;
        }
        if old_track.midi_clips() != new_track.midi_clips() {
            midi_changed_tracks.push(index);
        }
        if old_track.devices() != new_track.devices()
            && (!native_effect_states_only_changed(old_track.devices(), new_track.devices())
                || effect_target
                    .replace(EffectChainTarget::Track(index))
                    .is_some())
        {
            return None;
        }
    }

    for (index, ((old_bus, new_bus), compiled_bus)) in previous
        .return_tracks()
        .iter()
        .zip(project.return_tracks())
        .zip(&arrangement.return_buses)
        .enumerate()
    {
        if old_bus.id() != new_bus.id() || compiled_bus.id != new_bus.id() {
            return None;
        }
        if old_bus.gain() != new_bus.gain() || old_bus.muted() != new_bus.muted() {
            mixer_changed = true;
        }
        if old_bus.devices() != new_bus.devices()
            && (!native_effect_states_only_changed(old_bus.devices(), new_bus.devices())
                || effect_target
                    .replace(EffectChainTarget::Return(index))
                    .is_some())
        {
            return None;
        }
    }

    if previous.main_devices() != project.main_devices()
        && (!native_effect_states_only_changed(previous.main_devices(), project.main_devices())
            || effect_target.replace(EffectChainTarget::Main).is_some())
    {
        return None;
    }

    match (
        mixer_changed,
        !midi_changed_tracks.is_empty(),
        effect_target,
    ) {
        (_, false, None) => Some(IncrementalDelta::MixerOnly),
        (false, true, None) => Some(IncrementalDelta::ArrangementMidi {
            changed_tracks: midi_changed_tracks,
        }),
        (false, false, Some(target)) => Some(IncrementalDelta::SingleEffectChain(target)),
        _ => None,
    }
}

/// True when two effect chains have identical topology, IDs, parents, types,
/// bypass flags, automation/modulation, and no VST3 or asset-bearing devices,
/// differing only in built-in native effect state bytes.
fn native_effect_states_only_changed(old: &[Device], new: &[Device]) -> bool {
    if old.len() != new.len() {
        return false;
    }
    // Any chain containing a VST3 effect requires the host's foreign effect registry.
    if old
        .iter()
        .any(|d| d.state().type_id() == VST3_DEVICE_TYPE_ID)
        || new
            .iter()
            .any(|d| d.state().type_id() == VST3_DEVICE_TYPE_ID)
    {
        return false;
    }
    old.iter().zip(new).all(|(a, b)| {
        if a == b {
            return true;
        }
        a.id() == b.id()
            && a.parent() == b.parent()
            && a.bypassed() == b.bypassed()
            && a.state().type_id() == b.state().type_id()
            && a.state().asset_references().is_empty()
            && b.state().asset_references().is_empty()
            && a.automation() == b.automation()
            && a.modulation() == b.modulation()
    })
}

/// True when `Track::source_mix` must remain identity so the fader/post-fader
/// mix is applied once at the track output rather than baked into source rendering.
/// Mirrors `Track::with_audio_instrument_and_effects`, `with_instrument_config`,
/// `with_mutable_instrument`, `with_sends`, `with_gain_envelope`,
/// `with_midi_source_index`, and `compile::compile`'s upstream-routing override.
fn track_uses_identity_source_mix(compiled: &crate::Track, upstream_routing: bool) -> bool {
    upstream_routing
        || compiled.requires_ordered_processing()
        || !compiled.sends.is_empty()
        || compiled.gain_envelope.is_some()
        || compiled.instrument_output_gain != 1.0
        || compiled.instrument_input_gain != 1.0
}

fn prepare_mixer_only(
    previous_project: &Project,
    project: &Project,
    previous_revision: &PreparedRevision,
) -> Result<PreparedRevision, Invalid> {
    let mut arrangement = previous_revision.arrangement().as_ref().clone();
    arrangement.master_gain = PreparedMasterGain::new(project.master_gain())?;

    let solo = TrackSoloSelection::prepare(project);
    let any_soloed = solo.any;
    let upstream_routing = arrangement
        .signal_graph
        .as_ref()
        .is_some_and(|graph| graph.needs_upstream_frame_routing());

    for (track_index, (old_track, track)) in previous_project
        .tracks()
        .iter()
        .zip(project.tracks())
        .enumerate()
    {
        let compiled = &mut arrangement.tracks[track_index];
        let input_muted = solo.muted(track_index, track)?;
        let shared_source = track.source_id().filter(|source| {
            project.source_contributors(*source).count() > 1
                || project
                    .native_source(*source)
                    .and_then(|definition| definition.receiver())
                    .and_then(|receiver| project.track(receiver))
                    .is_some_and(hex_project::Track::retained_source_path)
        });
        let muted = if track.receives_source_audio()
            && let Some(source) = shared_source
        {
            (track.muted() && !track.soloed())
                || (any_soloed && !solo.source_selected(project, source)?)
        } else {
            input_muted
        };
        let local_content_enabled = shared_source.is_none() || !input_muted;

        let mix_effects = compiled.mix.effects;
        let mix =
            PreparedTrackMix::with_effects(track.gain(), track.balance(), muted, mix_effects)?;
        compiled.mix = mix;
        compiled.source_mix = if track_uses_identity_source_mix(compiled, upstream_routing) {
            PreparedTrackMix::identity()
        } else {
            mix
        };
        compiled.source_input_enabled = local_content_enabled && !track.retained_source_path();
        compiled.audio_content_enabled = local_content_enabled;

        if compiled.gain_envelope.is_some()
            && (track.gain() != old_track.gain()
                || track.balance() != old_track.balance()
                || compiled.mix.muted()
                    != previous_revision.arrangement().tracks()[track_index]
                        .mix
                        .muted())
        {
            let envelope_mix = PreparedTrackMix::with_effects(
                1.0,
                track.balance(),
                compiled.mix.muted(),
                compiled.mix.effects,
            )?;
            compiled.gain_envelope = crate::gain_envelope::PreparedGainEnvelope::prepare(
                track.gain_lane(),
                project.tempo_map(),
                envelope_mix,
                track.gain(),
            )?;
        }

        for (compiled_send, authored_send) in compiled.sends.iter_mut().zip(track.sends()) {
            compiled_send.gain = authored_send.gain();
        }
    }

    for (bus, authored) in arrangement
        .return_buses
        .iter_mut()
        .zip(project.return_tracks())
    {
        if !authored.gain().is_finite() || authored.gain() < 0.0 {
            return Err(Invalid("return gain must be finite and nonnegative"));
        }
        bus.gain = authored.gain();
        bus.muted = authored.muted();
    }

    let feeds_changed = previous_project
        .authored_routing()
        .iter()
        .zip(project.authored_routing())
        .any(|(a, b)| a.gain() != b.gain());
    if feeds_changed {
        crate::return_feed_plan::ReturnFeedPlan::validate_admission(project)?;
        arrangement.return_feed_plan = crate::return_feed_plan::ReturnFeedPlan::prepare(project)?;
    }

    let feeds = project
        .authored_routing()
        .iter()
        .map(|feed| {
            let source = project
                .return_tracks()
                .iter()
                .position(|bus| bus.id() == feed.source())
                .ok_or(Invalid("compensation feed source is missing"))?;
            let target = project
                .return_tracks()
                .iter()
                .position(|bus| bus.id() == feed.destination())
                .ok_or(Invalid("compensation feed target is missing"))?;
            Ok((source, target, feed.gain()))
        })
        .collect::<Result<Vec<_>, Invalid>>()?;

    if let Some(prev_graph) = arrangement.signal_graph.as_ref() {
        match prev_graph.with_updated_send_and_feed_gains(&arrangement, &feeds)? {
            UpdatedSignalGraph::Unchanged => {}
            UpdatedSignalGraph::Updated(updated) => {
                arrangement.signal_graph = Some(updated);
            }
            UpdatedSignalGraph::TopologyChanged => {
                arrangement.signal_graph =
                    crate::signal_graph::SignalGraph::prepare(&arrangement, &feeds, project)?;
            }
        }
    }

    Ok(PreparedRevision {
        source_routing: previous_revision.source_routing.clone(),
        resource_scope: previous_revision.resource_scope,
        device_resource_scope: previous_revision.device_resource_scope,
        tuning_family: TuningContinuationFamily::fresh()?,
        authored_clock_epoch: NonZeroU64::MIN,
        arrangement: crate::owned_shared::OwnedShared::new_owned(arrangement),
        session: previous_revision.session.clone_on_control(),
        midi_preview: previous_revision.midi_preview.clone_on_control(),
        mutable_track_revisions: previous_revision.mutable_track_revisions.clone(),
        authored_midi_sources: previous_revision.authored_midi_sources.clone(),
        gain_recording_track: None,
        native_parameter_input: None,
        scalar_automation: None,
        foreign_automation: Default::default(),
    })
}

fn prepare_arrangement_midi(
    previous_project: &Project,
    project: &Project,
    previous_revision: &PreparedRevision,
    changed_tracks: &[usize],
) -> Result<Option<PreparedRevision>, Invalid> {
    crate::midi_compile::validate_arrangement_occurrences(project)?;
    let mut arrangement = previous_revision.arrangement().as_ref().clone();
    let shared_contexts = Arc::clone(arrangement.shared_contexts());
    let session = previous_revision.session();

    // Find every track whose MIDI output depends on any changed track (either directly
    // or via a MIDI route where `source_index` is in `changed_tracks`).
    let mut affected = Vec::new();
    for (track_index, track) in project.tracks().iter().enumerate() {
        let source_id = project.midi_source_track(track.id());
        let source_index = project
            .tracks()
            .iter()
            .position(|candidate| candidate.id() == source_id)
            .ok_or(Invalid("midi source root is missing"))?;
        if changed_tracks.contains(&track_index) || changed_tracks.contains(&source_index) {
            // Multi-contributor shared sources need cross-track density validation;
            // fall back cleanly to the full compiler for that rare topology.
            if let Some(source) = track.source_id()
                && project.source_contributors(source).count() >= 2
            {
                return Ok(None);
            }
            affected.push((track_index, source_index));
        }
    }

    let instruments_registry = hex_devices::native_instrument_registry()
        .map_err(|_| Invalid("invalid built-in native instrument registry"))?;
    let mut replaced_previews: Vec<crate::MidiClipPreview> = Vec::new();
    let mut replaced_track_clip_ids: Vec<hex_project::ClipId> = Vec::new();

    for &(track_index, source_index) in &affected {
        let track = &project.tracks()[track_index];
        let midi_source = &project.tracks()[source_index];
        let compiled = &mut arrangement.tracks[track_index];

        // Collect existing clip IDs on this track from previous_project so removed or
        // updated clips are replaced cleanly in the preview index.
        if let Some(prev_track) = previous_project.tracks().get(track_index) {
            for prev_clip in prev_track.midi_clips() {
                replaced_track_clip_ids.push(prev_clip.id());
            }
        }

        let foreign_limits = compiled
            .foreign_instrument
            .as_ref()
            .filter(|node| !node.bypassed)
            .map(|_| crate::mutable_instrument::authored_gate::FOREIGN_GATE_LIMITS);

        let (prepared_instrument, mutable_limits): (
            Option<Arc<dyn hex_lattice::PreparedNativeInstrument>>,
            Option<crate::midi_compile::MutableInstrumentLimits>,
        ) = if let Some(device) = track.instrument().filter(|d| !d.bypassed()) {
            if device.state().type_id() == VST3_DEVICE_TYPE_ID {
                (None, foreign_limits)
            } else if !device.state().asset_references().is_empty() {
                // Resource-backed Sampler: reuse the existing mutable runtime factory from
                // `PreparedSessionInventory` with ZERO PCM scans!
                if !compiled.has_mutable_instrument() {
                    return Ok(None);
                }
                let Some(factory) = session.mutable_instrument_factory(track.id()) else {
                    return Ok(None);
                };
                let limits = crate::midi_compile::MutableInstrumentLimits::from_factory(factory)?;
                (None, Some(limits))
            } else {
                // Built-in non-resource instrument (Sine, Operator, FM4): lightweight single-track
                // preparation ensures exact `validate_event_note` semantics (e.g., custom tuning).
                let context = hex_lattice::DevicePreparationContext {
                    sample_rate: project.sample_rate(),
                    tuning: shared_contexts.tuning(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                };
                let config: Arc<dyn hex_lattice::PreparedNativeInstrument> =
                    Arc::from(crate::native_device_resources::prepare_instrument(
                        &instruments_registry,
                        device.id(),
                        device.state(),
                        &context,
                        None,
                    )?);
                let limits = if compiled.has_mutable_instrument() {
                    let Some(factory) = session.mutable_instrument_factory(track.id()) else {
                        return Ok(None);
                    };
                    Some(crate::midi_compile::MutableInstrumentLimits::from_factory(
                        factory,
                    )?)
                } else {
                    None
                };
                if limits.is_some() {
                    for clip in midi_source.midi_clips().iter().filter(|c| c.active()) {
                        for note in clip.source().notes().iter().filter(|n| n.active()) {
                            config
                                .validate_event_note(note.pitch(), note.velocity())
                                .map_err(|_| Invalid("mutable instrument note is unsupported"))?;
                        }
                    }
                }
                (Some(config), limits)
            }
        } else {
            (None, None)
        };

        if source_index != track_index && !track.midi_clips().is_empty() {
            crate::midi_compile::compile_instrument_track(
                project,
                track,
                None,
                None,
                None,
                Arc::clone(&shared_contexts),
                Some(&mut replaced_previews),
            )?;
        }

        let instrument = if midi_source.midi_clips().is_empty() {
            None
        } else {
            let shared_ctx = track
                .instrument()
                .and_then(|device| shared_contexts.index_for_device(device.id()))
                .and_then(|index| shared_contexts.owner(index).map(|owner| (index, owner)));
            Some(
                crate::midi_compile::compile_instrument_track(
                    project,
                    midi_source,
                    prepared_instrument.as_deref(),
                    mutable_limits,
                    shared_ctx,
                    Arc::clone(&shared_contexts),
                    if source_index == track_index {
                        Some(&mut replaced_previews)
                    } else {
                        None
                    },
                )?
                .ok_or(Invalid("instrument content missing after compile"))?,
            )
        };

        compiled.instrument = instrument;
        compiled.mutable_looping_source = if track.receives_source_audio() {
            track.source_id().is_some_and(|source| {
                project.source_contributors(source).any(|contributor| {
                    project
                        .track(project.midi_source_track(contributor.id()))
                        .is_some_and(|root| {
                            root.midi_clips()
                                .iter()
                                .any(|clip| clip.active() && clip.source().looping())
                        })
                })
            })
        } else {
            false
        };
    }

    // Revalidate project-wide MIDI note bank budgets.
    let arr_bytes = arrangement.midi_note_bank_storage_bytes()?;
    let arr_refs = arrangement.midi_note_sampler_reference_count()?;
    let total_arr_refs: usize = arrangement
        .tracks
        .iter()
        .filter_map(|t| t.instrument.as_ref())
        .try_fold(0usize, |acc, inst| {
            acc.checked_add(inst.spans.len())
                .ok_or(Invalid("midi note bank reference count overflow"))
        })?;
    crate::midi_note_bank::PreparedMidiNoteBank::validate_project_budget(
        arr_bytes,
        total_arr_refs,
        crate::midi_note_bank::MAX_ARRANGEMENT_NOTE_BANK_REFERENCES,
    )?;
    let midi_bank_bytes = arr_bytes
        .checked_add(session.midi_note_bank_storage_bytes())
        .ok_or(Invalid("complete revision midi bank byte count overflow"))?;
    let midi_bank_references = arr_refs
        .checked_add(session.midi_note_bank_reference_count())
        .ok_or(Invalid(
            "complete revision midi bank reference count overflow",
        ))?;
    crate::midi_note_bank::PreparedMidiNoteBank::validate_complete_revision_budget(
        midi_bank_bytes,
        midi_bank_references,
    )?;

    // Recompute audible_end_frame across tracks.
    arrangement.audible_end_frame = arrangement
        .tracks
        .iter()
        .flat_map(|track| {
            track
                .clips
                .iter()
                .map(|clip| clip.end)
                .chain(track.instrument.iter().flat_map(|instrument| {
                    instrument
                        .spans
                        .iter()
                        .map(|span| span.end)
                        .chain(instrument.timeline_end)
                }))
        })
        .max();

    // Update MidiArrangementPreview: keep all unaffected clips, replace affected tracks' clips.
    let mut affected_clip_ids: Vec<hex_project::ClipId> = replaced_track_clip_ids;
    for &(track_index, _) in &affected {
        for clip in project.tracks()[track_index].midi_clips() {
            affected_clip_ids.push(clip.id());
        }
    }
    // Also remove any clip IDs that were removed from an affected track.
    let mut live_clip_ids: Vec<hex_project::ClipId> = project
        .tracks()
        .iter()
        .flat_map(|t| t.midi_clips().iter().map(|c| c.id()))
        .collect();
    live_clip_ids.sort_unstable();
    affected_clip_ids.sort_unstable();
    affected_clip_ids.dedup();

    let mut next_previews = Vec::with_capacity(
        previous_revision
            .midi_preview()
            .clips()
            .len()
            .saturating_add(replaced_previews.len()),
    );
    for clip in previous_revision.midi_preview().clips() {
        if affected_clip_ids.binary_search(&clip.clip_id).is_err()
            && live_clip_ids.binary_search(&clip.clip_id).is_ok()
        {
            next_previews.push(clip.clone());
        }
    }
    next_previews.extend(replaced_previews);
    let midi_preview = MidiArrangementPreview::from_clips(next_previews)?;

    let source_routing = if affected
        .iter()
        .any(|&(idx, _)| project.tracks()[idx].source_id().is_some())
    {
        crate::PreparedSourceRouting::prepare_for_playback(project, &arrangement, session)?
    } else {
        previous_revision.source_routing.clone()
    };

    let mutable_track_revisions = prepare_mutable_track_revisions(
        project,
        &arrangement,
        previous_revision.device_resource_scope.is_some(),
    )?;
    let authored_midi_sources = if previous_revision.device_resource_scope.is_some()
        || authored_midi::has_inactive(project)
    {
        authored_midi::prepare(project)?
    } else {
        Vec::new()
    };

    Ok(Some(PreparedRevision {
        source_routing,
        resource_scope: previous_revision.resource_scope,
        device_resource_scope: previous_revision.device_resource_scope,
        tuning_family: TuningContinuationFamily::fresh()?,
        authored_clock_epoch: NonZeroU64::MIN,
        arrangement: crate::owned_shared::OwnedShared::new_owned(arrangement),
        session: previous_revision.session.clone_on_control(),
        midi_preview: crate::owned_shared::OwnedShared::new_owned(midi_preview),
        mutable_track_revisions,
        authored_midi_sources,
        gain_recording_track: None,
        native_parameter_input: None,
        scalar_automation: None,
        foreign_automation: Default::default(),
    }))
}

fn prepare_single_effect_chain(
    project: &Project,
    previous_revision: &PreparedRevision,
    target: EffectChainTarget,
) -> Result<Option<PreparedRevision>, Invalid> {
    if !previous_revision.arrangement().source_effects.is_empty() {
        return Ok(None);
    }
    let effects = hex_devices::native_registry()
        .map_err(|_| Invalid("invalid built-in native device registry"))?;
    let mut arrangement = previous_revision.arrangement().as_ref().clone();
    let device_context = hex_lattice::DevicePreparationContext {
        sample_rate: project.sample_rate(),
        tuning: arrangement.shared_contexts().tuning(),
        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
    };
    let upstream_routing = arrangement
        .signal_graph
        .as_ref()
        .is_some_and(|graph| graph.needs_upstream_frame_routing());

    match target {
        EffectChainTarget::Track(index) => {
            let track = &project.tracks()[index];
            let plan = crate::compile::compile_device_effects(
                track,
                project,
                &effects,
                &device_context,
                None,
                &[],
            )?;
            let compiled = &mut arrangement.tracks[index];
            if !chains_preserve_latency(&compiled.device_chain, &plan.device_chain) {
                return Ok(None);
            }
            let stateful = !plan.device_chain.is_empty();
            let ordered = compiled.instrument_position != 0;
            let segmented =
                stateful || (ordered && (plan.prefix.is_some() || plan.suffix.is_some()));
            let mix_effects = if segmented || compiled.mutable_instrument {
                None
            } else {
                plan.fused
            };
            let mix = PreparedTrackMix::with_effects(
                track.gain(),
                track.balance(),
                compiled.mix.muted(),
                mix_effects,
            )?;
            compiled.ordered_prefix = plan.prefix;
            compiled.ordered_suffix = plan.suffix;
            compiled.device_chain = Arc::from(plan.device_chain);
            compiled.mix = mix;
            if compiled.gain_envelope.is_some() {
                let envelope_mix = PreparedTrackMix::with_effects(
                    1.0,
                    track.balance(),
                    compiled.mix.muted(),
                    compiled.mix.effects,
                )?;
                compiled.gain_envelope = crate::gain_envelope::PreparedGainEnvelope::prepare(
                    track.gain_lane(),
                    project.tempo_map(),
                    envelope_mix,
                    track.gain(),
                )?;
            }
            compiled.source_mix = if track_uses_identity_source_mix(compiled, upstream_routing) {
                PreparedTrackMix::identity()
            } else {
                mix
            };
        }
        EffectChainTarget::Return(index) => {
            let return_track = &project.return_tracks()[index];
            let plan = crate::compile::compile_effect_devices(
                EffectPower::new(return_track.devices())?,
                hex_lattice::ProcessorChainOwner::Return(return_track.id().value()),
                project,
                &device_context,
                &effects,
                None,
                &[],
            )?;
            let bus = &mut arrangement.return_buses[index];
            if !chains_preserve_latency(&bus.device_chain, &plan.device_chain) {
                return Ok(None);
            }
            bus.matrix = plan.fused;
            bus.device_chain = Arc::from(plan.device_chain);
        }
        EffectChainTarget::Main => {
            let plan = crate::compile::compile_effect_devices(
                EffectPower::new(project.main_devices())?,
                hex_lattice::ProcessorChainOwner::Main,
                project,
                &device_context,
                &effects,
                None,
                &[],
            )?;
            if !chains_preserve_latency(&arrangement.main_device_chain, &plan.device_chain) {
                return Ok(None);
            }
            arrangement.main_device_chain = Arc::from(plan.device_chain);
        }
    }

    // Revalidate stateful processor count/bytes and runtime graph budget across all chains.
    let mut stateful_count = 0usize;
    let mut stateful_bytes = 0usize;
    for device in arrangement
        .tracks
        .iter()
        .flat_map(|t| t.device_chain.iter())
        .chain(
            arrangement
                .return_buses
                .iter()
                .flat_map(|b| b.device_chain.iter()),
        )
        .chain(arrangement.main_device_chain.iter())
    {
        if let Some(definition) = &device.definition {
            if !definition.is_powered_off() {
                stateful_count = stateful_count
                    .checked_add(1)
                    .ok_or(Invalid("stateful device count overflow"))?;
            }
            stateful_bytes = stateful_bytes
                .checked_add(
                    definition
                        .declared_pair_bytes()
                        .ok_or(Invalid("stateful pair byte count overflow"))?,
                )
                .ok_or(Invalid("stateful device runtime budget overflow"))?;
            if stateful_count > hex_lattice::MAX_STATEFUL_PROCESSORS
                || stateful_bytes > hex_lattice::MAX_PROCESSOR_BANK_RUNTIME_BYTES * 2
            {
                return Err(Invalid(
                    "stateful device runtime exceeds its bounded budget",
                ));
            }
        }
    }
    crate::mutable_instrument::validate_prepared_graph_budget(
        &arrangement,
        previous_revision.session(),
    )
    .map_err(|_| Invalid("active effect and mutable instrument runtime budget exceeded"))?;

    let mutable_track_revisions = if matches!(target, EffectChainTarget::Track(_)) {
        prepare_mutable_track_revisions(
            project,
            &arrangement,
            previous_revision.device_resource_scope.is_some(),
        )?
    } else {
        previous_revision.mutable_track_revisions.clone()
    };

    Ok(Some(PreparedRevision {
        source_routing: previous_revision.source_routing.clone(),
        resource_scope: previous_revision.resource_scope,
        device_resource_scope: previous_revision.device_resource_scope,
        tuning_family: TuningContinuationFamily::fresh()?,
        authored_clock_epoch: NonZeroU64::MIN,
        arrangement: crate::owned_shared::OwnedShared::new_owned(arrangement),
        session: previous_revision.session.clone_on_control(),
        midi_preview: previous_revision.midi_preview.clone_on_control(),
        mutable_track_revisions,
        authored_midi_sources: previous_revision.authored_midi_sources.clone(),
        gain_recording_track: None,
        native_parameter_input: None,
        scalar_automation: None,
        foreign_automation: Default::default(),
    }))
}

fn chains_preserve_latency(
    old: &[hex_lattice::PreparedRuntimeInstance],
    new: &[hex_lattice::PreparedRuntimeInstance],
) -> bool {
    old.len() == new.len()
        && old.iter().zip(new).all(|(a, b)| {
            a.owner == b.owner
                && a.device_id == b.device_id
                && a.algorithmic_latency_frames() == b.algorithmic_latency_frames()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SessionPlayback, SessionPlaybackCommand};
    use hex_lattice_contract::NativeDeviceMetadata;
    use hex_project::{BeatTime, Command, Created, Editor, TrackDeviceSlot};

    fn render_revision(revision: Arc<PreparedRevision>, frames: usize) -> Vec<[f32; 2]> {
        let mut playback = SessionPlayback::new(revision).unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        let mut out = vec![[0.0; 2]; frames];
        playback.render(&mut out).unwrap();
        out
    }

    #[test]
    fn incremental_mixer_midi_and_effect_deltas_match_full_compile_and_reuse_graph() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track_a) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "lead".into(),
                before_track: None,
                slot: TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("track a");
        };
        let Created::Track(track_b) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "bass".into(),
                before_track: None,
                slot: TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("track b");
        };
        let Created::Device(utility_id) = editor
            .execute(Command::InsertDevice {
                track: track_a,
                before: None,
                state: hex_devices::utility::UTILITY_DEFINITION
                    .default_state()
                    .unwrap(),
            })
            .unwrap()
        else {
            panic!("utility");
        };
        let Created::ReturnTrack(ret) = editor
            .execute(Command::AddReturnTrack { name: "bus".into() })
            .unwrap()
        else {
            panic!("return");
        };
        editor
            .execute(Command::SetReturnSend {
                track: track_a,
                return_track: ret,
                gain: 0.25,
            })
            .unwrap();
        editor
            .execute(Command::SetReturnSend {
                track: track_b,
                return_track: ret,
                gain: 0.35,
            })
            .unwrap();
        let Created::Clip(clip_a) = editor
            .execute(Command::PlaceMidiClip {
                track: track_a,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip a");
        };
        let Created::Note(note_a) = editor
            .execute(Command::AddMidiNote {
                clip: clip_a,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap()
        else {
            panic!("note a");
        };
        let Created::Clip(clip_b) = editor
            .execute(Command::PlaceMidiClip {
                track: track_b,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip b");
        };
        editor
            .execute(Command::AddMidiNote {
                clip: clip_b,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 48,
                channel: 0,
                velocity: 90,
            })
            .unwrap();

        let table = Arc::new(SineTable::new());
        let empty = BTreeMap::new();
        let mut prev_project = editor.project_arc();
        let mut prev_rev = Arc::new(
            PreparedRevision::prepare(&prev_project, &empty, Arc::clone(&table), None).unwrap(),
        );

        // 1. Mute, Solo, Track Gain, Balance, Master Gain, Return Gain, Return Send Gain.
        let mixer_commands = [
            Command::Gain {
                track: track_a,
                gain: 0.4,
            },
            Command::Balance {
                track: track_a,
                balance: -0.35,
            },
            Command::Mute {
                track: track_b,
                muted: true,
            },
            Command::ToggleSolo {
                track: track_a,
                exclusive: true,
            },
            Command::ToggleSolo {
                track: track_a,
                exclusive: true,
            },
            Command::MasterGain { gain: 0.8 },
            Command::ReturnGain {
                return_track: ret,
                gain: 0.6,
            },
            Command::SetReturnSend {
                track: track_a,
                return_track: ret,
                gain: 0.5,
            },
            Command::Gain {
                track: track_b,
                gain: 0.4,
            },
            Command::SetReturnSend {
                track: track_b,
                return_track: ret,
                gain: 0.7,
            },
            Command::Gain {
                track: track_b,
                gain: 1.5,
            },
        ];
        for (step, cmd) in mixer_commands.into_iter().enumerate() {
            let is_send = matches!(cmd, Command::SetReturnSend { .. });
            editor.execute(cmd).unwrap();
            let next_project = editor.project_arc();
            let inc = Arc::new(
                PreparedRevision::try_prepare_incremental(&prev_project, &next_project, &prev_rev)
                    .unwrap()
                    .unwrap_or_else(|| panic!("mixer step {step} should hit incremental path")),
            );
            let full = Arc::new(
                PreparedRevision::prepare(
                    &next_project,
                    &empty,
                    Arc::clone(&table),
                    Some(prev_rev.session()),
                )
                .unwrap(),
            );
            for (inc_track, full_track) in inc
                .arrangement()
                .tracks()
                .iter()
                .zip(full.arrangement().tracks())
            {
                assert_eq!(inc_track.mix(), full_track.mix());
                assert_eq!(inc_track.source_mix(), full_track.source_mix());
            }
            assert!(Arc::ptr_eq(inc.session(), prev_rev.session()));
            assert!(Arc::ptr_eq(inc.midi_preview(), prev_rev.midi_preview()));
            if !is_send {
                assert!(Arc::ptr_eq(
                    inc.arrangement().signal_graph.as_ref().unwrap(),
                    prev_rev.arrangement().signal_graph.as_ref().unwrap(),
                ));
            }
            assert_eq!(
                inc.arrangement().signal_graph,
                full.arrangement().signal_graph
            );
            assert_eq!(
                render_revision(Arc::clone(&inc), 512),
                render_revision(full, 512)
            );
            prev_project = next_project;
            prev_rev = inc;
        }

        // 2. Single-clip MIDI note move/update reuses SignalGraph + session and matches full render.
        editor
            .execute(Command::UpdateMidiNote {
                note: note_a,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 67,
                channel: 0,
                velocity: 110,
            })
            .unwrap();
        let next_project = editor.project_arc();
        let inc_midi = Arc::new(
            PreparedRevision::try_prepare_incremental(&prev_project, &next_project, &prev_rev)
                .unwrap()
                .expect("MIDI note edit should hit incremental path"),
        );
        let full_midi = Arc::new(
            PreparedRevision::prepare(
                &next_project,
                &empty,
                Arc::clone(&table),
                Some(prev_rev.session()),
            )
            .unwrap(),
        );
        assert!(Arc::ptr_eq(inc_midi.session(), prev_rev.session()));
        assert!(Arc::ptr_eq(
            inc_midi.arrangement().signal_graph.as_ref().unwrap(),
            prev_rev.arrangement().signal_graph.as_ref().unwrap(),
        ));
        assert_eq!(inc_midi.midi_preview(), full_midi.midi_preview());
        assert_eq!(
            render_revision(Arc::clone(&inc_midi), 512),
            render_revision(full_midi, 512)
        );
        prev_project = next_project;
        prev_rev = inc_midi;

        // 3. Single-chain built-in effect parameter change reuses SignalGraph + session.
        let current_state = prev_project.device(utility_id).unwrap().state().clone();
        let patched_state = hex_devices::utility::UTILITY_DEFINITION
            .patch_parameter(
                &current_state,
                hex_devices::utility::parameter::GAIN,
                hex_lattice_contract::ParameterValue::Float(0.5),
            )
            .unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device: utility_id,
                state: patched_state,
            })
            .unwrap();
        let next_project = editor.project_arc();
        let inc_fx = Arc::new(
            PreparedRevision::try_prepare_incremental(&prev_project, &next_project, &prev_rev)
                .unwrap()
                .expect("native effect parameter edit should hit incremental path"),
        );
        let full_fx = Arc::new(
            PreparedRevision::prepare(
                &next_project,
                &empty,
                Arc::clone(&table),
                Some(prev_rev.session()),
            )
            .unwrap(),
        );
        assert!(Arc::ptr_eq(inc_fx.session(), prev_rev.session()));
        assert!(Arc::ptr_eq(
            inc_fx.arrangement().signal_graph.as_ref().unwrap(),
            prev_rev.arrangement().signal_graph.as_ref().unwrap(),
        ));
        assert_eq!(
            render_revision(Arc::clone(&inc_fx), 512),
            render_revision(full_fx, 512)
        );
    }
}
