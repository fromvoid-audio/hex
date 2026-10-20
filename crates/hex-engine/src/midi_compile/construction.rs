//! Private constructor-local bound for mutable/silent tracks without previews.
//! No owning clone, provider, heap scratch, input-union or publication authority.
use super::*;
use crate::owned_shared::OwnedShared;
use std::mem::size_of;

#[derive(Clone, Copy)]
pub(crate) struct Cost {
    pub(crate) retained_bytes: usize,
    pub(crate) construction_peak_bytes: usize,
}
pub(crate) fn quote(track: &Track, audible: bool) -> Result<Cost, Invalid> {
    let overflow = || Invalid("mutable midi construction cost overflow");
    if track.midi_clips().is_empty() {
        return Ok(Cost {
            retained_bytes: 0,
            construction_peak_bytes: 0,
        });
    }
    let mut occurrences = 0usize;
    for clip in track.midi_clips() {
        let playback = clip.source_playback().map_err(|error| Invalid(error.0))?;
        for note in clip.notes() {
            occurrences = usize::try_from(
                playback
                    .note_occurrences(note.start(), note.duration())
                    .map_err(|error| Invalid(error.0))?
                    .count(),
            )
            .ok()
            .and_then(|n| occurrences.checked_add(n))
            .filter(|n| *n <= MAX_COMPILED_MIDI_OCCURRENCES)
            .ok_or(Invalid("compiled midi occurrence limit reached"))?;
        }
    }
    let spans = if audible { occurrences } else { 0 };
    let (body, header) =
        OwnedShared::<crate::midi_note_bank::PreparedMidiNoteBank>::declared_parts().ok_or(
            Invalid("mutable midi bank constructor layout is unsupported"),
        )?;
    let retained_bytes = spans
        .checked_mul(size_of::<CompiledNoteSpan>() + size_of::<SpanNode>())
        .and_then(|n| n.checked_add(body))
        .and_then(|n| n.checked_add(header))
        .ok_or_else(overflow)?;
    // Count each actual requested density backing conservatively. Onset,
    // voice-edge and event-edge scratch retire in sequence, but summing them
    // bounds their overlap without assuming a particular early drop.
    let scratch = spans
        .checked_mul(size_of::<u64>() + 4 * size_of::<(u64, bool)>())
        .and_then(|n| n.checked_add(track.midi_clips().len().checked_mul(size_of::<usize>())?))
        .ok_or_else(overflow)?;
    Ok(Cost {
        retained_bytes,
        construction_peak_bytes: retained_bytes.checked_add(scratch).ok_or_else(overflow)?,
    })
}
impl Cost {
    pub(super) fn validate_actual(
        self,
        spans: &Vec<CompiledNoteSpan>,
        nodes: &Vec<SpanNode>,
        bank: &OwnedShared<crate::midi_note_bank::PreparedMidiNoteBank>,
    ) -> Result<(), Invalid> {
        let (body, header) = bank
            .parts()
            .ok_or(Invalid("mutable midi bank storage is unsupported"))?;
        let actual = spans
            .capacity()
            .checked_mul(size_of::<CompiledNoteSpan>())
            .and_then(|n| n.checked_add(nodes.capacity().checked_mul(size_of::<SpanNode>())?))
            .and_then(|n| n.checked_add(body))
            .and_then(|n| n.checked_add(header))
            .ok_or(Invalid("mutable midi actual storage overflow"))?;
        if actual > self.retained_bytes || actual > self.construction_peak_bytes {
            return Err(Invalid(
                "mutable midi constructor exceeded its backing bound",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created};

    #[test]
    fn sampler_compiler_midi_cost_bounds_real_mutable_note_bank_without_quote_births() {
        let (bank, identity, mut editor) =
            crate::shared_analytic::sampler_tests::canonical_fixture()
                .expect("actual original-resource Sampler constructor must be supported");
        drop(identity);
        let track = editor.project().tracks()[0].id();
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        let step = BeatTime::ONE.raw() / 32;
        for ordinal in 0..17 {
            editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::from_raw(ordinal * step),
                    duration: BeatTime::from_raw(step),
                    pitch: 60,
                    channel: 0,
                    velocity: 127,
                })
                .unwrap();
        }
        let project = editor.project();
        let track = project.track(track).unwrap();
        let slot = track.instrument().unwrap();
        let registry = hex_devices::native_instrument_registry().unwrap();
        let table = crate::shared_analytic::SharedAnalyticContextTable::prepare(
            project,
            &registry,
            None,
            Some(crate::native_device_resources::DeviceResourcePreparation {
                bank: &bank,
                max_scope_metadata_bytes: 65_536,
            }),
        )
        .unwrap();
        let owner = table.owner_for_device(slot.id()).unwrap();
        let factory = owner
            .sampler_prepared_on_control(track.id(), slot.id(), slot.state(), project.sample_rate())
            .unwrap()
            .unwrap();
        let counts = slot.state().payload_storage_observation();
        crate::owned_shared::reset_birth_count();
        crate::owned_shared::reset_clone_count();
        let cost = quote(track, true).unwrap();
        let silent = quote(track, false).unwrap();
        assert_eq!(crate::owned_shared::birth_count(), 0);
        assert_eq!(crate::owned_shared::clone_count(), 0);
        assert_eq!(slot.state().payload_storage_observation(), counts);
        let actual = crate::midi_compile::compile_instrument_track(
            project,
            track,
            Some(factory),
            Some(MutableInstrumentLimits::from_factory(factory).unwrap()),
            Some((table.index_for_device(slot.id()).unwrap(), owner)),
            table.clone_on_control(),
            None,
        )
        .unwrap()
        .unwrap();
        assert!(actual.mutable);
        assert_eq!(actual.spans.len(), 17);
        assert_eq!(actual.nodes.len(), 17);
        let (body, header) = actual.bank.parts().unwrap();
        let retained = actual.spans.capacity() * size_of::<CompiledNoteSpan>()
            + actual.nodes.capacity() * size_of::<SpanNode>()
            + body
            + header;
        assert_eq!(cost.retained_bytes, retained);
        assert_eq!(silent.retained_bytes, body + header);
        assert!(cost.construction_peak_bytes > retained);
        eprintln!(
            "SUPPORTED sampler compiler mutable MIDI actual17 spans/nodes, retained={retained}, peak={}, no quote births",
            cost.construction_peak_bytes
        );
    }
}
