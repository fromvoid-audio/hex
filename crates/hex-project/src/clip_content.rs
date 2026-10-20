//! Exact derived-Eq metadata, with no model/source owner or identity witness.
use crate::*;

const MAX_ATOMS: usize = MAX_NAME_BYTES
    + MAX_NOTES
    + MAX_AUDIO_EVENTS
    + 2 * MAX_AUDIO_POINTS
    + 2 * MAX_TEMPO_POINTS
    + 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContentAtom {
    Word(u64),
    Wide(u128),
    Signed(i128),
    Byte(u8),
    Bool(bool),
    Fade(ClipFade),
    Gain(ClipGain),
    Flavor(ClipFlavor),
    Clock(AudioDomainClock),
    Tempo(Tempo),
    Placement(AudioViewPlacement),
    Mode(WarpMode),
    Marker(WarpMarker),
    GainPoint(SourceGainPoint),
    TempoPoint(TempoPoint),
    Event(AudioEvent),
    Note {
        id: NoteId,
        start: BeatTime,
        duration: BeatTime,
        pitch: u8,
        channel: u8,
        velocity: u8,
    },
}

/// A private, exact sequence of Copy metadata values and name bytes. This is
/// neither a model clone nor source/admission authority. Reserve outside any
/// retained read. Capture and comparison never grow or free the backing.
pub struct ClipContentToken {
    atoms: Vec<ContentAtom>,
    captured: bool,
}
impl std::fmt::Debug for ClipContentToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipContentToken")
            .field("capacity", &self.atoms.capacity())
            .field("captured", &self.captured)
            .finish_non_exhaustive()
    }
}
impl ClipContentToken {
    pub fn reserve_on_control(capacity: usize) -> Result<Self, Invalid> {
        if capacity > MAX_ATOMS {
            return Err(Invalid("clip content exceeds bounded metadata"));
        }
        let mut atoms = Vec::new();
        atoms
            .try_reserve_exact(capacity)
            .map_err(|_| Invalid("could not reserve clip content metadata"))?;
        Ok(Self {
            atoms,
            captured: false,
        })
    }
    pub fn capacity(&self) -> usize {
        self.atoms.capacity()
    }
}

type Emit<'a> = dyn FnMut(ContentAtom) -> Result<(), Invalid> + 'a;
fn word(emit: &mut Emit<'_>, n: u64) -> Result<(), Invalid> {
    emit(ContentAtom::Word(n))
}
fn length(emit: &mut Emit<'_>, n: usize) -> Result<(), Invalid> {
    word(
        emit,
        u64::try_from(n).map_err(|_| Invalid("clip content length overflow"))?,
    )
}
fn name(emit: &mut Emit<'_>, s: &str) -> Result<(), Invalid> {
    length(emit, s.len())?;
    for b in s.bytes() {
        emit(ContentAtom::Byte(b))?;
    }
    Ok(())
}
fn audio(clip: &Clip, emit: &mut Emit<'_>) -> Result<(), Invalid> {
    // Exhaustive owning-field matches deliberately fail compilation when the
    // canonical derived Eq gains a field. None/Some tags and order are exact.
    let Clip {
        name: label,
        flavor,
        id,
        asset,
        start,
        duration,
        source_offset,
        source_length,
        looping,
        source_phase,
        fade,
        gain,
        edit,
    } = clip;
    word(emit, 0)?;
    name(emit, label)?;
    emit(ContentAtom::Flavor(*flavor))?;
    for n in [
        id.value(),
        asset.value(),
        *start,
        *duration,
        *source_offset,
        *source_length,
        *source_phase,
    ] {
        word(emit, n)?;
    }
    emit(ContentAtom::Bool(*looping))?;
    emit(ContentAtom::Fade(*fade))?;
    emit(ContentAtom::Gain(*gain))?;
    emit(ContentAtom::Bool(edit.is_some()))?;
    if let Some(edit) = edit {
        let AudioClipEdit {
            domain,
            view_start,
            view_end,
            musical_start,
            placement,
        } = edit.as_ref();
        word(emit, *view_start)?;
        word(emit, *view_end)?;
        emit(ContentAtom::Bool(musical_start.is_some()))?;
        if let Some(start) = musical_start {
            word(emit, start.raw())?;
        }
        emit(ContentAtom::Bool(placement.is_some()))?;
        if let Some(placement) = placement {
            emit(ContentAtom::Placement(*placement))?;
        }
        let AudioClipDomain {
            events,
            source_offset,
            source_length,
            source_phase,
            looping,
            clock,
            pitch_cents,
            sample_bpm,
            warp,
            warp_enabled,
            source_gain,
            fade,
            fade_start,
            fade_projection,
        } = domain.as_ref();
        for n in [*source_offset, *source_length, *source_phase, *fade_start] {
            word(emit, n)?;
        }
        emit(ContentAtom::Bool(*looping))?;
        emit(ContentAtom::Clock(*clock))?;
        emit(ContentAtom::Signed(i128::from(*pitch_cents)))?;
        emit(ContentAtom::Bool(sample_bpm.is_some()))?;
        if let Some(tempo) = sample_bpm {
            emit(ContentAtom::Tempo(*tempo))?;
        }
        emit(ContentAtom::Bool(*warp_enabled))?;
        emit(ContentAtom::Bool(warp.is_some()))?;
        if let Some(WarpSettings { mode, markers }) = warp {
            emit(ContentAtom::Mode(*mode))?;
            length(emit, markers.len())?;
            for m in markers {
                emit(ContentAtom::Marker(*m))?;
            }
        }
        length(emit, source_gain.len())?;
        for point in source_gain {
            emit(ContentAtom::GainPoint(*point))?;
        }
        emit(ContentAtom::Fade(*fade))?;
        emit(ContentAtom::Bool(events.is_some()))?;
        if let Some(events) = events {
            let AudioEvents { next_id, events } = events.as_ref();
            word(emit, *next_id)?;
            length(emit, events.len())?;
            for event in events {
                emit(ContentAtom::Event(*event))?;
            }
        }
        emit(ContentAtom::Bool(fade_projection.is_some()))?;
        if let Some(AudioFadeProjection {
            tempo,
            origin_beat_raw,
            local_start_raw,
        }) = fade_projection
        {
            emit(ContentAtom::Signed(*origin_beat_raw))?;
            word(emit, *local_start_raw)?;
            tempo.visit_content_eq(emit)?;
        }
    }
    Ok(())
}
fn midi(clip: &MidiClip, emit: &mut Emit<'_>) -> Result<(), Invalid> {
    let MidiClip {
        name: label,
        flavor,
        id,
        start,
        length: span,
        source,
    } = clip;
    word(emit, 1)?;
    name(emit, label)?;
    emit(ContentAtom::Flavor(*flavor))?;
    for n in [id.value(), start.raw(), span.raw()] {
        word(emit, n)?;
    }
    let MidiSource {
        source_length,
        source_start,
        loop_start,
        loop_end,
        looping,
        notes,
    } = source.as_ref();
    for n in [
        source_length.raw(),
        source_start.raw(),
        loop_start.raw(),
        loop_end.raw(),
    ] {
        word(emit, n)?;
    }
    emit(ContentAtom::Bool(*looping))?;
    length(emit, notes.len())?;
    for note in notes {
        let MidiNote {
            id,
            start,
            duration,
            pitch,
            channel,
            velocity,
        } = note;
        emit(ContentAtom::Note {
            id: *id,
            start: *start,
            duration: *duration,
            pitch: *pitch,
            channel: *channel,
            velocity: *velocity,
        })?;
    }
    Ok(())
}
fn required(mut visit: impl FnMut(&mut Emit<'_>) -> Result<(), Invalid>) -> Result<usize, Invalid> {
    let mut count = 0_usize;
    visit(&mut |_| {
        count = count
            .checked_add(1)
            .filter(|n| *n <= MAX_ATOMS)
            .ok_or(Invalid("clip content exceeds bounded metadata"))?;
        Ok(())
    })?;
    Ok(count)
}
fn capture(
    token: &mut ClipContentToken,
    mut visit: impl FnMut(&mut Emit<'_>) -> Result<(), Invalid>,
) -> Result<(), Invalid> {
    let count = required(&mut visit)?;
    if count > token.atoms.capacity() {
        return Err(Invalid("clip content scratch capacity is insufficient"));
    }
    token.atoms.clear(); // Copy values only: retains all backing, no destructors.
    token.captured = false;
    visit(&mut |atom| {
        token.atoms.push(atom);
        Ok(())
    })?;
    token.captured = true;
    Ok(())
}
fn matches(
    token: &ClipContentToken,
    mut visit: impl FnMut(&mut Emit<'_>) -> Result<(), Invalid>,
) -> bool {
    if !token.captured {
        return false;
    }
    let mut index = 0;
    let result = visit(&mut |atom| {
        if token.atoms.get(index) != Some(&atom) {
            return Err(Invalid("clip content changed"));
        }
        index += 1;
        Ok(())
    });
    result.is_ok() && index == token.atoms.len()
}
macro_rules! content_api {
    ($ty:ty, $visit:ident) => {
        impl $ty {
            pub fn content_token_capacity(&self) -> Result<usize, Invalid> {
                required(|emit| $visit(self, emit))
            }
            pub fn capture_content(&self, token: &mut ClipContentToken) -> Result<(), Invalid> {
                capture(token, |emit| $visit(self, emit))
            }
            pub fn matches_content(&self, token: &ClipContentToken) -> bool {
                matches(token, |emit| $visit(self, emit))
            }
        }
    };
}
content_api!(Clip, audio);
content_api!(MidiClip, midi);

#[cfg(test)]
mod tests;
