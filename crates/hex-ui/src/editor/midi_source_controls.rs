//! Grid-stepped, atomic editing for MIDI source and Arrangement regions.
use hex_project::{MidiSource, Project};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SourceField {
    ArrangementLength,
    SourceLength,
    SourceStart,
    LoopStart,
    LoopEnd,
    Looping,
}

impl SourceField {
    pub(super) const fn minus_id(self) -> &'static str {
        match self {
            Self::ArrangementLength => "midi-source-arrangement-minus",
            Self::SourceLength => "midi-source-extent-minus",
            Self::SourceStart => "midi-source-start-minus",
            Self::LoopStart => "midi-source-loop-start-minus",
            Self::LoopEnd => "midi-source-loop-end-minus",
            Self::Looping => "midi-source-loop-toggle",
        }
    }

    pub(super) const fn plus_id(self) -> &'static str {
        match self {
            Self::ArrangementLength => "midi-source-arrangement-plus",
            Self::SourceLength => "midi-source-extent-plus",
            Self::SourceStart => "midi-source-start-plus",
            Self::LoopStart => "midi-source-loop-start-plus",
            Self::LoopEnd => "midi-source-loop-end-plus",
            Self::Looping => "midi-source-loop-toggle",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SourceValues {
    pub(super) arrangement_start: u64,
    pub(super) arrangement_length: u64,
    pub(super) source_length: u64,
    pub(super) source_start: u64,
    pub(super) loop_start: u64,
    pub(super) loop_end: u64,
    pub(super) looping: bool,
}

impl SourceValues {
    pub(super) fn from_source(
        source: &MidiSource,
        arrangement_start: u64,
        arrangement_length: u64,
    ) -> Self {
        Self {
            arrangement_start,
            arrangement_length,
            source_length: source.source_length().raw(),
            source_start: source.source_start().raw(),
            loop_start: source.loop_start().raw(),
            loop_end: source.loop_end().raw(),
            looping: source.looping(),
        }
    }
}

pub(super) fn edit_command(
    project: &Project,
    clip: hex_project::ClipId,
    values: SourceValues,
) -> Option<hex_project::Command> {
    if let Some(arrangement) = project.midi_clip(clip) {
        // A forward one-shot's start marker moves its region, keeping notes at
        // their song beats; every other source edit replaces the source fields.
        let current = SourceValues::from_source(
            project.midi_source(clip)?,
            arrangement.start().raw(),
            arrangement.length().raw(),
        );
        if !arrangement.looping()
            && arrangement.content_view().is_none()
            && values.source_start != current.source_start
            && values
                == (SourceValues {
                    source_start: values.source_start,
                    ..current
                })
        {
            return Some(hex_project::Command::ResizeMidiClipView {
                clip,
                operation: hex_project::MidiViewResizeOperation::PlayStart {
                    source_start: hex_project::BeatTime::from_raw(values.source_start),
                },
            });
        }
        Some(hex_project::Command::SetMidiClipSource {
            clip,
            source_length: hex_project::BeatTime::from_raw(values.source_length),
            source_start: hex_project::BeatTime::from_raw(values.source_start),
            loop_start: hex_project::BeatTime::from_raw(values.loop_start),
            loop_end: hex_project::BeatTime::from_raw(values.loop_end),
            looping: values.looping,
            length: hex_project::BeatTime::from_raw(values.arrangement_length),
        })
    } else if project.session_midi_clip(clip).is_some() {
        Some(hex_project::Command::SetSessionMidiClipSource {
            clip,
            source_length: hex_project::BeatTime::from_raw(values.source_length),
            source_start: hex_project::BeatTime::from_raw(values.source_start),
            loop_start: hex_project::BeatTime::from_raw(values.loop_start),
            loop_end: hex_project::BeatTime::from_raw(values.loop_end),
            looping: values.looping,
        })
    } else {
        None
    }
}

pub(super) fn adjust(
    mut values: SourceValues,
    field: SourceField,
    direction: i8,
    step: u64,
    latest_note_end: u64,
) -> Result<Option<SourceValues>, &'static str> {
    if direction == 0 {
        return Ok(None);
    }
    if !matches!(direction, -1 | 1) || step == 0 {
        return Err("midi source controls require a positive grid step");
    }
    let original = values;

    let update = |value: u64, direction: i8| {
        if direction < 0 {
            value.checked_sub(step)
        } else {
            value.checked_add(step)
        }
    };
    match field {
        SourceField::ArrangementLength => {
            let length = update(values.arrangement_length, direction)
                .ok_or("arrangement length exceeds the beat range")?;
            if length == 0 {
                return Err("arrangement length must stay positive");
            }
            values
                .arrangement_start
                .checked_add(length)
                .ok_or("arrangement clip end exceeds the beat range")?;
            values.arrangement_length = length;
        }
        SourceField::SourceLength => {
            let length = update(values.source_length, direction)
                .ok_or("source length exceeds the beat range")?;
            if length == 0 {
                return Err("source length must stay positive");
            }
            if latest_note_end > length {
                return Err("source length cannot exclude authored notes");
            }
            if values.loop_end > length {
                return Err("source length cannot exclude retained scope markers");
            }
            values.source_length = length;
        }
        SourceField::SourceStart => {
            let start = update(values.source_start, direction)
                .ok_or("source start exceeds the beat range")?;
            let exclusive_end = if values.looping {
                values.loop_end
            } else {
                values.source_length
            };
            if start >= exclusive_end {
                return Err("source start must precede its playback end");
            }
            values.source_start = start;
        }
        SourceField::LoopStart => {
            let start =
                update(values.loop_start, direction).ok_or("scope start exceeds the beat range")?;
            if start >= values.loop_end {
                return Err("scope start must precede scope end");
            }
            values.loop_start = start;
        }
        SourceField::LoopEnd => {
            let end =
                update(values.loop_end, direction).ok_or("scope end exceeds the beat range")?;
            if end <= values.loop_start || end > values.source_length {
                return Err("scope end must stay inside the source after scope start");
            }
            if values.looping && values.source_start >= end {
                return Err("scope end must stay after source start while looping");
            }
            values.loop_end = end;
        }
        SourceField::Looping => values.looping = !values.looping,
    }

    if values.arrangement_length == 0
        || values.source_length == 0
        || values.loop_start >= values.loop_end
        || values.loop_end > values.source_length
        || (values.looping && values.source_start >= values.loop_end)
        || (!values.looping && values.source_start >= values.source_length)
    {
        return Err("midi source settings would be invalid");
    }

    Ok((values != original).then_some(values))
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;

    fn values() -> SourceValues {
        SourceValues {
            arrangement_start: 10,
            arrangement_length: 8,
            source_length: 12,
            source_start: 2,
            loop_start: 4,
            loop_end: 8,
            looping: false,
        }
    }

    #[test]
    fn button_ids_are_stable_and_direction_specific() {
        let fields = [
            SourceField::ArrangementLength,
            SourceField::SourceLength,
            SourceField::SourceStart,
            SourceField::LoopStart,
            SourceField::LoopEnd,
            SourceField::Looping,
        ];
        let mut ids = Vec::new();
        for field in fields {
            ids.push(field.minus_id());
            if field != SourceField::Looping {
                ids.push(field.plus_id());
            }
        }
        for (index, id) in ids.iter().enumerate() {
            assert!(
                !ids[index + 1..].contains(id),
                "duplicate source control id: {id}"
            );
        }
        assert_eq!(
            SourceField::Looping.minus_id(),
            SourceField::Looping.plus_id()
        );
    }

    #[test]
    fn arrangement_length_changes_independently_of_source_extent() {
        let changed = adjust(values(), SourceField::ArrangementLength, 1, 2, 0)
            .unwrap()
            .unwrap();
        assert_eq!(changed.arrangement_length, 10);
        assert_eq!(changed.source_length, 12);
    }

    #[test]
    fn source_shrink_preserves_notes_and_retained_markers() {
        assert_eq!(
            adjust(values(), SourceField::SourceLength, -1, 2, 11),
            Err("source length cannot exclude authored notes")
        );
        assert_eq!(
            adjust(values(), SourceField::SourceLength, -1, 6, 0),
            Err("source length cannot exclude retained scope markers")
        );
    }

    #[test]
    fn loop_markers_and_start_obey_half_open_bounds() {
        assert_eq!(
            adjust(values(), SourceField::LoopStart, 1, 4, 0),
            Err("scope start must precede scope end")
        );
        assert_eq!(
            adjust(values(), SourceField::LoopEnd, -1, 4, 0),
            Err("scope end must stay inside the source after scope start")
        );
        let mut looping = values();
        looping.looping = true;
        looping.source_start = 6;
        assert_eq!(
            adjust(looping, SourceField::LoopEnd, -1, 2, 0),
            Err("scope end must stay after source start while looping")
        );
        assert_eq!(
            adjust(values(), SourceField::SourceStart, 1, 10, 0),
            Err("source start must precede its playback end")
        );
    }

    #[test]
    fn overflow_zero_length_and_invalid_direction_are_refused() {
        let mut end = values();
        end.arrangement_start = u64::MAX - 3;
        end.arrangement_length = 2;
        assert_eq!(
            adjust(end, SourceField::ArrangementLength, 1, 2, 0),
            Err("arrangement clip end exceeds the beat range")
        );
        assert_eq!(
            adjust(values(), SourceField::ArrangementLength, -1, 8, 0),
            Err("arrangement length must stay positive")
        );
        assert_eq!(
            adjust(values(), SourceField::SourceStart, 1, 0, 0),
            Err("midi source controls require a positive grid step")
        );
    }

    #[test]
    fn disabled_loop_toggle_keeps_markers_and_zero_direction_is_a_no_op() {
        let command = adjust(values(), SourceField::Looping, 1, 1, 0)
            .unwrap()
            .unwrap();
        assert!(command.looping);
        assert_eq!(command.loop_start, 4);
        assert_eq!(command.loop_end, 8);
        assert_eq!(
            adjust(values(), SourceField::SourceStart, -1, 2, 0),
            Ok(Some(SourceValues {
                source_start: 0,
                ..values()
            }))
        );
        assert_eq!(adjust(values(), SourceField::Looping, 0, 1, 0), Ok(None));
    }

    #[test]
    fn session_source_edit_and_note_authoring_target_the_session_owner() {
        use hex_project::{BeatTime, Command, Created, Editor};

        let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
        let track = match editor
            .execute(Command::AddMidiTrack {
                name: "instrument".into(),
            })
            .expect("track")
        {
            Created::Track(id) => id,
            other => panic!("unexpected result: {other:?}"),
        };
        let scene = match editor
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .expect("scene")
        {
            Created::Scene(id) => id,
            other => panic!("unexpected result: {other:?}"),
        };
        let clip = match editor
            .execute(Command::CreateSessionMidiClip {
                track,
                scene,
                name: "session phrase".into(),
                source_length: BeatTime::from_quarters(4).expect("length"),
            })
            .expect("session clip")
        {
            Created::SessionClip(id) => id,
            other => panic!("unexpected result: {other:?}"),
        };

        let values = SourceValues {
            arrangement_start: 0,
            arrangement_length: 4 * BeatTime::ONE.raw(),
            source_length: 6 * BeatTime::ONE.raw(),
            source_start: BeatTime::ONE.raw(),
            loop_start: 0,
            loop_end: 6 * BeatTime::ONE.raw(),
            looping: false,
        };
        let command = edit_command(editor.project(), clip, values).expect("session source owner");
        assert!(matches!(command, Command::SetSessionMidiClipSource { .. }));
        editor.execute(command).expect("edit session source");
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .expect("add session note");

        let source = editor
            .project()
            .midi_source(clip)
            .expect("shared source accessor");
        assert_eq!(source.source_length().raw(), values.source_length);
        assert_eq!(source.source_start().raw(), values.source_start);
        assert_eq!(source.notes().len(), 1);
    }
}
