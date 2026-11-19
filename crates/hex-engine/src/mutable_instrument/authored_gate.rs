//! Authored gate scheduling in the engine's occurrence ledger. Rendering and
//! worker custody belong to its callers, including rollback for built-in DSP
//! and acceptance of foreign worker input at the physical callback boundary.
use super::*;

/// The copied worker input reserves 64 combined events per quantum.
pub(crate) const MAX_FOREIGN_NOTES: usize = 64;
pub(crate) const FOREIGN_GATE_LIMITS: crate::midi_compile::MutableInstrumentLimits =
    crate::midi_compile::MutableInstrumentLimits {
        max_events_per_query: MAX_FOREIGN_NOTES,
        max_active_occurrences: MAX_FOREIGN_NOTES,
        max_release_tail_frames: 0,
        supports_clip_volume: false,
        steals_releasing_voices: true,
    };

#[derive(Clone, Copy)]
pub(crate) struct AuthoredGate {
    frame_offset: usize,
    occurrence_id: u64,
    pitch: u8,
    channel: u8,
    velocity: u8,
    on: bool,
}

impl AuthoredGate {
    pub(crate) fn release_now(note: MutableOccurrence) -> Self {
        let (_, _, pitch, channel, _) = note.signature.details();
        Self {
            frame_offset: 0,
            occurrence_id: note.id,
            pitch,
            channel,
            velocity: 0,
            on: false,
        }
    }
}

impl From<AuthoredGate> for NativeInstrumentEvent {
    fn from(gate: AuthoredGate) -> Self {
        if gate.on {
            Self::NoteOn {
                frame_offset: gate.frame_offset,
                occurrence_id: gate.occurrence_id,
                pitch: gate.pitch,
                velocity: gate.velocity,
            }
        } else {
            Self::NoteOff {
                frame_offset: gate.frame_offset,
                occurrence_id: gate.occurrence_id,
            }
        }
    }
}

impl TryFrom<AuthoredGate> for hex_lattice::ForeignNote {
    type Error = MutableInstrumentRuntimeError;

    fn try_from(gate: AuthoredGate) -> Result<Self, Self::Error> {
        let id = i32::try_from(gate.occurrence_id)
            .ok()
            .and_then(|id| std::num::NonZeroU32::new(id as u32))
            .ok_or(MutableInstrumentRuntimeError::IdentityExhausted)?;
        Ok(Self {
            frame: u16::try_from(gate.frame_offset)
                .map_err(|_| MutableInstrumentRuntimeError::Arithmetic)?,
            channel: gate.channel,
            pitch: gate.pitch,
            velocity: gate.velocity,
            on: gate.on,
            note_id: Some(id),
        })
    }
}

pub(crate) struct AuthoredGateContext<'a> {
    pub(crate) track: Option<TrackId>,
    pub(crate) performance_start: u64,
    pub(crate) source_start: u64,
    pub(crate) width: u64,
    pub(crate) source_age: Option<u64>,
    pub(crate) contributor_fade_rate: Option<u32>,
    pub(crate) tail_frames: u64,
    pub(crate) max_active: usize,
    pub(crate) steals_releasing: bool,
    pub(crate) live: usize,
    pub(crate) suppressions: &'a [MidiOccurrenceSuppression],
}

/// Emit the segment's authored gates into caller-owned storage. The caller
/// orders them by offset, with releases before onsets at an equal offset, and
/// combines any live events before staging the segment. Errors invalidate the
/// scratch ledger for this callback.
pub(crate) fn stage_authored_gates(
    scratch: &mut Vec<MutableOccurrence>,
    next_id: &mut u64,
    onsets: &mut MutableOnsetBuffer,
    context: AuthoredGateContext<'_>,
    mut emit: impl FnMut(AuthoredGate) -> Result<(), MutableInstrumentRuntimeError>,
) -> Result<(), MutableInstrumentRuntimeError> {
    let AuthoredGateContext {
        track,
        performance_start,
        source_start,
        width,
        source_age,
        contributor_fade_rate,
        tail_frames,
        max_active,
        steals_releasing,
        live,
        suppressions,
    } = context;
    let performance_end = performance_start
        .checked_add(width)
        .ok_or(MutableInstrumentRuntimeError::Arithmetic)?;
    onsets.sort_canonically();

    scratch.retain(|occurrence| {
        !occurrence.note_off_sent
            || match (source_age, occurrence.quiet_deadline_age) {
                (Some(age), Some(quiet)) => quiet > age,
                _ => occurrence.quiet_deadline_frame > performance_start,
            }
    });
    for occurrence in scratch.iter_mut() {
        if !occurrence.note_off_sent && occurrence.gate_end_frame < performance_start {
            return Err(MutableInstrumentRuntimeError::OrphanedGate);
        }
        if !occurrence.note_off_sent && occurrence.gate_end_frame < performance_end {
            let offset = usize::try_from(occurrence.gate_end_frame - performance_start)
                .map_err(|_| MutableInstrumentRuntimeError::Arithmetic)?;
            let (_, _, pitch, channel, _) = occurrence.signature.details();
            emit(AuthoredGate {
                frame_offset: offset,
                occurrence_id: occurrence.id,
                pitch,
                channel,
                velocity: 0,
                on: false,
            })?;
            occurrence.note_off_sent = true;
            occurrence.quiet_deadline_age = source_age.map(|age| {
                age.saturating_add(offset as u64)
                    .saturating_add(tail_frames)
            });
        }
    }

    for (onset_index, onset) in onsets.iter().enumerate() {
        if track.is_some_and(|track| onset.key.track != track) || onset.source_start < source_start
        {
            continue;
        }
        let Some(onset_delta) = onset.source_start.checked_sub(source_start) else {
            continue;
        };
        if onset_delta >= width {
            continue;
        }
        let contributor_fade = if onset.contributor_fade_in {
            if onset_delta != 0 {
                return Err(MutableInstrumentRuntimeError::InvalidSourceSpan);
            }
            let start_age = source_age.ok_or(MutableInstrumentRuntimeError::InvalidSourceSpan)?;
            let fade = contributor_fade_rate
                .and_then(ContributorFadeIn::new)
                .ok_or(MutableInstrumentRuntimeError::InvalidSourceSpan)?;
            Some(ContributorFade {
                start_age,
                frames: fade.frames(),
                start_bits: 0.0_f64.to_bits(),
                muted: false,
            })
        } else {
            None
        };
        let onset_frame = performance_start
            .checked_add(onset_delta)
            .ok_or(MutableInstrumentRuntimeError::Arithmetic)?;
        let signature = MutableOccurrenceSignature::new(onset);
        if has_mutable_suppression(suppressions, signature, onset_frame) {
            continue;
        }
        scratch.retain(|proof| {
            !proof.note_off_sent
                || match (source_age, proof.quiet_deadline_age) {
                    (Some(age), Some(quiet)) => quiet > age.saturating_add(onset_delta),
                    _ => proof.quiet_deadline_frame > onset_frame,
                }
        });
        if scratch
            .iter()
            .any(|active| active.signature.key == onset.key)
            || onsets.iter().take(onset_index).any(|previous| {
                previous.key == onset.key
                    && !has_mutable_suppression(
                        suppressions,
                        MutableOccurrenceSignature::new(previous),
                        performance_start
                            .saturating_add(previous.source_start.saturating_sub(source_start)),
                    )
            })
        {
            return Err(MutableInstrumentRuntimeError::DuplicateOccurrence);
        }
        *next_id = next_id
            .checked_add(1)
            .ok_or(MutableInstrumentRuntimeError::IdentityExhausted)?;
        let gate_delta = onset
            .source_gate_end
            .checked_sub(source_start)
            .ok_or(MutableInstrumentRuntimeError::Arithmetic)?;
        let gate_end_frame = performance_start
            .checked_add(gate_delta)
            .ok_or(MutableInstrumentRuntimeError::Arithmetic)?;
        if gate_end_frame <= onset_frame {
            return Err(MutableInstrumentRuntimeError::InvalidSourceSpan);
        }
        let onset_offset =
            usize::try_from(onset_delta).map_err(|_| MutableInstrumentRuntimeError::Arithmetic)?;
        let id = *next_id;
        let on = AuthoredGate {
            frame_offset: onset_offset,
            occurrence_id: id,
            pitch: onset.pitch,
            channel: onset.channel,
            velocity: onset.velocity,
            on: true,
        };
        emit(on)?;
        let note_off_sent = gate_end_frame < performance_end;
        if note_off_sent {
            let offset = usize::try_from(gate_end_frame - performance_start)
                .map_err(|_| MutableInstrumentRuntimeError::Arithmetic)?;
            emit(AuthoredGate {
                frame_offset: offset,
                velocity: 0,
                on: false,
                ..on
            })?;
        }
        // Voice slots and the fixed proof ledger are bounded separately:
        // stolen releases leave the provider's slots but keep their proofs.
        let occupied = if steals_releasing {
            scratch
                .iter()
                .filter(|proof| !proof.note_off_sent || proof.gate_end_frame > onset_frame)
                .count()
        } else {
            scratch.len()
        };
        if occupied + live >= max_active.min(MAX_MUTABLE_OCCURRENCE_PROOFS)
            || scratch.len() + live >= MAX_MUTABLE_OCCURRENCE_PROOFS
            || scratch.len() == scratch.capacity()
        {
            return Err(MutableInstrumentRuntimeError::OccurrenceCapacity);
        }
        let quiet_deadline_frame = gate_end_frame
            .checked_add(tail_frames)
            .ok_or(MutableInstrumentRuntimeError::Arithmetic)?;
        scratch.push(MutableOccurrence {
            signature,
            id,
            onset_frame,
            gate_end_frame,
            quiet_deadline_frame,
            quiet_deadline_age: if note_off_sent {
                source_age.map(|age| age.saturating_add(gate_delta).saturating_add(tail_frames))
            } else {
                None
            },
            note_off_sent,
            gain_at_end_bits: 1.0_f64.to_bits(),
            gain_frozen: false,
            contributor_fade,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests;
