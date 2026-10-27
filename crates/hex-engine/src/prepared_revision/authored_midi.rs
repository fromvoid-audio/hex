//! Authored note identity and activation evidence for safe playback handoffs. Playable-span
//! indexes omit notes outside source markers and cannot prove a stable note was absent or inactive.
use super::*;
use hex_project::{BeatTime, ClipId, MidiSource, NoteId, SceneId, SessionCellContent};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Source {
    pub(super) id: ClipId,
    track: TrackId,
    scene: Option<SceneId>,
    start: BeatTime,
    length: BeatTime,
    markers: [BeatTime; 4],
    looping: bool,
    notes: Vec<NoteState>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct NoteState {
    id: NoteId,
    active: bool,
}

type Input<'a> = (
    ClipId,
    TrackId,
    Option<SceneId>,
    BeatTime,
    BeatTime,
    &'a MidiSource,
);
fn sources(project: &Project) -> impl Iterator<Item = Input<'_>> {
    project
        .tracks()
        .iter()
        .flat_map(|track| {
            track.midi_clips().iter().map(move |clip| {
                (
                    clip.id(),
                    track.id(),
                    None,
                    clip.start(),
                    clip.length(),
                    clip.source(),
                )
            })
        })
        .chain(project.session_cells().iter().filter_map(|cell| {
            if let SessionCellContent::Midi(clip) = cell.content() {
                Some((
                    clip.id(),
                    cell.track(),
                    Some(cell.scene()),
                    BeatTime::ZERO,
                    clip.source().source_length(),
                    clip.source(),
                ))
            } else {
                None
            }
        }))
}

pub(super) fn has_inactive(project: &Project) -> bool {
    sources(project).any(|(_, _, _, _, _, source)| source.notes().iter().any(|note| !note.active()))
}

/// One control scan over all canonical source/notes, bounded by existing authoring laws.
/// Sorting the copied IDs is control-only; callback membership is two binary searches.
fn counts(project: &Project) -> Result<(usize, usize), Invalid> {
    sources(project).try_fold(
        (0usize, 0usize),
        |(clips, notes), (_, _, _, _, _, source)| {
            let clips = clips
                .checked_add(1)
                .filter(|count| *count <= hex_project::MAX_CLIPS)
                .ok_or(Invalid("authored MIDI evidence clip limit exceeded"))?;
            let notes = notes
                .checked_add(source.notes().len())
                .filter(|count| *count <= hex_project::MAX_NOTES)
                .ok_or(Invalid("authored MIDI evidence note limit exceeded"))?;
            Ok((clips, notes))
        },
    )
}

pub(super) fn construction_bytes(project: &Project) -> Result<usize, Invalid> {
    let (clips, notes) = counts(project)?;
    clips
        .checked_mul(std::mem::size_of::<Source>())
        .and_then(|bytes| {
            notes
                .checked_mul(std::mem::size_of::<NoteState>())
                .and_then(|n| bytes.checked_add(n))
        })
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Vec<Source>>()))
        .ok_or(Invalid("authored MIDI evidence byte count overflow"))
}

pub(super) fn prepare(project: &Project) -> Result<Vec<Source>, Invalid> {
    let (clips, _) = counts(project)?;
    let mut evidence = Vec::new();
    evidence
        .try_reserve_exact(clips)
        .map_err(|_| Invalid("could not reserve authored MIDI evidence"))?;
    if evidence.capacity() > hex_project::MAX_CLIPS {
        return Err(Invalid(
            "authored MIDI evidence capacity exceeds clip limit",
        ));
    }
    for (id, track, scene, start, length, source) in sources(project) {
        let mut notes = Vec::new();
        notes
            .try_reserve_exact(source.notes().len())
            .map_err(|_| Invalid("could not reserve authored MIDI note identity index"))?;
        if notes.capacity() > hex_project::MAX_NOTES {
            return Err(Invalid(
                "authored MIDI identity capacity exceeds note limit",
            ));
        }
        notes.extend(source.notes().iter().map(|note| NoteState {
            id: note.id(),
            active: note.active(),
        }));
        notes.sort_unstable_by_key(|note| note.id);
        if notes.windows(2).any(|ids| ids[0].id == ids[1].id) {
            return Err(Invalid("duplicate authored MIDI note identity"));
        }
        evidence.push(Source {
            id,
            track,
            scene,
            start,
            length,
            markers: [
                source.source_length(),
                source.source_start(),
                source.loop_start(),
                source.loop_end(),
            ],
            looping: source.looping(),
            notes,
        });
    }
    evidence.sort_unstable_by_key(|source| source.id);
    if evidence.windows(2).any(|clips| clips[0].id == clips[1].id) {
        return Err(Invalid("duplicate authored MIDI clip identity"));
    }
    Ok(evidence)
}

pub(super) fn retained_bytes(sources: &Vec<Source>) -> Option<usize> {
    sources.iter().try_fold(
        sources
            .capacity()
            .checked_mul(std::mem::size_of::<Source>())?
            .checked_add(std::mem::size_of::<Vec<Source>>())?,
        |bytes, source| {
            bytes.checked_add(
                source
                    .notes
                    .capacity()
                    .checked_mul(std::mem::size_of::<NoteState>())?,
            )
        },
    )
}

pub(super) fn newly_authored(
    old: &[Source],
    new: &[Source],
    clip: ClipId,
    note: NoteId,
    source_track: TrackId,
    session: bool,
) -> bool {
    let find = |sources: &[Source]| sources.binary_search_by_key(&clip, |source| source.id).ok();
    let Some(candidate) = find(new).and_then(|index| new.get(index)) else {
        return false;
    };
    if candidate.track != source_track
        || candidate.scene.is_some() != session
        || !candidate
            .notes
            .binary_search_by_key(&note, |entry| entry.id)
            .ok()
            .and_then(|index| candidate.notes.get(index))
            .is_some_and(|entry| entry.active)
    {
        return false;
    }
    let Some(previous) = find(old).and_then(|index| old.get(index)) else {
        // An absent Session clip cannot establish the old effective owner/launch.
        return !session;
    };
    previous.track == candidate.track
        && previous.scene == candidate.scene
        && previous.start == candidate.start
        && previous.length == candidate.length
        && previous.markers == candidate.markers
        && previous.looping == candidate.looping
        && previous
            .notes
            .binary_search_by_key(&note, |entry| entry.id)
            .ok()
            .and_then(|index| previous.notes.get(index))
            .is_none_or(|entry| !entry.active)
}

/// Exact inactivity evidence for one authored clip/note identity. Missing
/// evidence is not treated as inactivity; it follows the existing handoff path.
pub(super) fn inactive_note(sources: &[Source], clip: ClipId, note: NoteId) -> bool {
    sources
        .binary_search_by_key(&clip, |source| source.id)
        .ok()
        .and_then(|index| sources.get(index))
        .and_then(|source| {
            source
                .notes
                .binary_search_by_key(&note, |entry| entry.id)
                .ok()
                .and_then(|index| source.notes.get(index))
        })
        .is_some_and(|entry| !entry.active)
}
