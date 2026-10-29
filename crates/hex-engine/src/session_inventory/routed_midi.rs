//! Control-side expansion of one selected MIDI bus into destination templates.
use super::*;
use hex_project::{SessionCell, SessionMidiClip};

/// Weighted counts are checked before reserving expanded templates or note keys.
pub(super) struct RoutedMidiPlan {
    pub(super) destinations: Vec<(TrackId, TrackId)>,
    pub(super) template_count: usize,
}
impl RoutedMidiPlan {
    pub(super) fn prepare(project: &Project) -> Result<Self, crate::Invalid> {
        if project.midi_routes().is_empty() {
            return Ok(Self {
                destinations: Vec::new(),
                template_count: 0,
            });
        }
        let mut counts = BTreeMap::<TrackId, (usize, usize)>::new();
        let mut templates = 0usize;
        let mut notes = 0usize;
        for cell in project.session_cells() {
            if let SessionCellContent::Midi(clip) = cell.content() {
                let count = counts.entry(cell.track()).or_default();
                count.0 = count
                    .0
                    .checked_add(1)
                    .ok_or(crate::Invalid("Session MIDI template count overflow"))?;
                count.1 = count
                    .1
                    .checked_add(if clip.active() {
                        clip.source().notes().len()
                    } else {
                        0
                    })
                    .ok_or(crate::Invalid("Session MIDI note key count overflow"))?;
                templates = templates
                    .checked_add(1)
                    .ok_or(crate::Invalid("Session MIDI template count overflow"))?;
                notes = notes
                    .checked_add(if clip.active() {
                        clip.source().notes().len()
                    } else {
                        0
                    })
                    .ok_or(crate::Invalid("Session MIDI note key count overflow"))?;
            }
        }
        let canonical_templates = templates;
        // There are at most MAX_TRACKS destinations, independently of cell count.
        let mut destinations = Vec::new();
        destinations
            .try_reserve_exact(project.midi_routes().len())
            .map_err(|_| crate::Invalid("could not reserve Session MIDI source index"))?;
        for track in project.tracks() {
            let root = project.midi_source_track(track.id());
            if root == track.id() {
                continue;
            }
            let (root_templates, root_notes) = counts.get(&root).copied().unwrap_or_default();
            templates = templates.checked_add(root_templates).ok_or(crate::Invalid(
                "routed Session MIDI template count overflow",
            ))?;
            notes = notes.checked_add(root_notes).ok_or(crate::Invalid(
                "routed Session MIDI note key count overflow",
            ))?;
            if templates > hex_project::MAX_SESSION_CELLS {
                return Err(crate::Invalid("routed Session MIDI template limit reached"));
            }
            if notes > hex_project::MAX_NOTES {
                return Err(crate::Invalid("routed Session MIDI note key limit reached"));
            }
            destinations.push((track.id(), root));
        }
        if notes > hex_project::MAX_NOTES {
            return Err(crate::Invalid("Session MIDI note bank key limit reached"));
        }
        destinations.sort_unstable_by_key(|&(destination, _)| destination);
        Ok(Self {
            destinations,
            template_count: templates - canonical_templates,
        })
    }
    pub(super) fn sources<'a>(
        &'a self,
        project: &'a Project,
    ) -> impl Iterator<Item = (TrackId, &'a SessionCell, &'a SessionMidiClip)> + 'a {
        self.destinations
            .iter()
            .flat_map(move |&(destination, root)| {
                project
                    .session_cells()
                    .iter()
                    .filter_map(move |cell| match cell.content() {
                        SessionCellContent::Midi(clip) if cell.track() == root => {
                            Some((destination, cell, clip))
                        }
                        _ => None,
                    })
            })
    }
}

pub(super) fn prepare_clip(
    destination: TrackId,
    cell: &SessionCell,
    clip: &SessionMidiClip,
    config: &PreparedTrackInstrumentConfig,
    source_timing: &SessionTimingView<'_>,
    shared_contexts: &Arc<crate::shared_analytic::SharedAnalyticContextTable>,
    cached: Option<&PreparedSessionMidiClip>,
) -> Result<PreparedSessionMidiClip, crate::Invalid> {
    let source_identity = clip.source_identity();
    let mutable_limits = config.mutable_limits;
    if !clip.volume().is_unity()
        && mutable_limits.is_some_and(|limits| !limits.supports_clip_volume)
    {
        return Err(crate::Invalid(
            "clip volume is unsupported by this mutable instrument mode",
        ));
    }
    if clip.active()
        && mutable_limits.is_some()
        && let Some(instrument) = config.config.as_deref()
    {
        for note in clip.source().notes().iter().filter(|note| note.active()) {
            instrument
                .validate_event_note(note.pitch(), note.velocity())
                .map_err(|_| crate::Invalid("mutable Session instrument note is unsupported"))?;
        }
    }
    let instrument_id = config.device_id;
    let instrument_state = config.state.clone();
    let instrument_enabled = clip.active() && config.config.is_some() && !config.bypassed;
    let cached = cached.filter(|old| {
        old.track == destination
            && old.source_identity == source_identity
            && old.active == clip.active()
            && old.playback_offset == clip.playback_offset()
            && old.instrument_id == instrument_id
            && old.instrument_state == instrument_state
            && old.tuning_identity == config.tuning_identity
            && old.instrument_enabled == instrument_enabled
    });
    let source = if let Some(old) = cached {
        Arc::clone(&old.source)
    } else {
        Arc::new(
            PreparedMidiSourceTemplate::prepare_source_window_active(
                destination,
                clip.id(),
                clip.source(),
                clip.playback_offset(),
                clip.active(),
                source_timing,
            )
            .map_err(template_error)?,
        )
    };
    // Source/instrument reuse above and below intentionally excludes volume:
    // changing only this clip's curve must not manufacture new note custody.
    // Revalidate volume admission on every preparation: effective tempo loops
    // can change break density without changing the canonical map/max BPM.
    let volume =
        crate::midi_clip_volume::PreparedMidiClipVolume::prepare_session(clip, source_timing)?;
    let bank = config
        .note_bank
        .as_ref()
        .ok_or(crate::Invalid("Session MIDI note bank is missing"))?;
    let cached_instrument = cached.filter(|old| {
        Arc::ptr_eq(old.instrument.note_bank(), bank)
            && old.instrument.is_event_source() == mutable_limits.is_some()
            && old.mutable_limits == mutable_limits
    });
    let instrument = if let Some(old) = cached_instrument {
        Arc::clone(&old.instrument)
    } else if let Some(limits) = mutable_limits {
        Arc::new(
            PreparedMidiInstrumentTemplate::prepare_events(&source, limits)
                .map_err(template_error)?,
        )
    } else {
        Arc::new(
            PreparedMidiInstrumentTemplate::prepare_with_bank_and_contexts(
                &source,
                Arc::clone(bank),
                Arc::clone(shared_contexts),
            )
            .map_err(template_error)?,
        )
    };
    let work = if let Some(old) = cached_instrument {
        old.work
    } else {
        work_for(&source, &instrument)?
    };
    Ok(PreparedSessionMidiClip {
        active: clip.active(),
        playback_offset: clip.playback_offset(),
        id: clip.id(),
        track: destination,
        scene: cell.scene(),
        name: clone_string(clip.name())?,
        source_identity,
        source,
        volume,
        instrument,
        instrument_id,
        instrument_state,
        tuning_identity: config.tuning_identity,
        instrument_enabled,
        mutable_limits,
        work,
    })
}

pub(super) fn account_template(
    clip: &PreparedSessionMidiClip,
    bytes: &mut usize,
    references: &mut usize,
) -> Result<(), crate::Invalid> {
    *references = references
        .checked_add(
            clip.instrument
                .index_reference_count()
                .map_err(template_error)?,
        )
        .ok_or(crate::Invalid(
            "Session MIDI note bank reference count overflow",
        ))?;
    *bytes = bytes
        .checked_add(std::mem::size_of::<
            Arc<crate::midi_note_bank::PreparedMidiNoteBank>,
        >())
        .ok_or(crate::Invalid(
            "Session MIDI bank handle byte count overflow",
        ))?;
    // This is conservative control metadata accounting, once per prepared
    // row even when routed owners share the immutable curve's Arc. It is not
    // an allocation identity or a CURRENT closed-storage custody declaration.
    *bytes = bytes
        .checked_add(
            clip.volume
                .as_ref()
                .map_or(0, |program| program.retained_control_metadata_bytes()),
        )
        .ok_or(crate::Invalid(
            "session midi clip volume byte count overflow",
        ))?;
    crate::midi_note_bank::PreparedMidiNoteBank::validate_project_budget(
        *bytes,
        *references,
        crate::midi_note_bank::MAX_SESSION_NOTE_BANK_REFERENCES,
    )
}

#[cfg(test)]
mod tests;
