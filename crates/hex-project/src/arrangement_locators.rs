//! Frame-anchored Arrangement landmarks. The canonical inventory is sorted by
//! unique frame and shares the global project ID allocator. Locator edits are
//! ordinary history transactions. Empty names are represented by absence.
use crate::{Command, Created, ElementColor, Invalid, LocatorId, Project, TempoMap};
use std::sync::Arc;

pub const MAX_ARRANGEMENT_LOCATORS: usize = 1_024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArrangementLocator {
    id: LocatorId,
    pub(crate) frame: u64,
    name: Option<String>,
}

impl ArrangementLocator {
    pub fn id(&self) -> LocatorId {
        self.id
    }
    pub fn frame(&self) -> u64 {
        self.frame
    }
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArrangementLocatorSnapshot {
    pub id: u64,
    pub frame: u64,
    pub name: Option<String>,
}

impl From<&ArrangementLocator> for ArrangementLocatorSnapshot {
    fn from(locator: &ArrangementLocator) -> Self {
        Self {
            id: locator.id.value(),
            frame: locator.frame,
            name: locator.name.clone(),
        }
    }
}

pub(crate) fn from_snapshot(
    snapshot: ArrangementLocatorSnapshot,
) -> Result<ArrangementLocator, Invalid> {
    if let Some(name) = &snapshot.name {
        crate::edit::name_valid(name)?;
    }
    Ok(ArrangementLocator {
        id: LocatorId(snapshot.id),
        frame: snapshot.frame,
        name: snapshot.name,
    })
}

fn validate_frame(tempo: &TempoMap, frame: u64) -> Result<(), Invalid> {
    if frame > crate::MAX_SPARSE_AUDIO_TIMELINE_FRAMES {
        return Err(Invalid("locator is outside the arrangement timeline"));
    }
    tempo.frame_to_beat(frame).map(|_| ())
}

pub(crate) fn validate(locators: &[ArrangementLocator], tempo: &TempoMap) -> Result<(), Invalid> {
    for locator in locators {
        validate_frame(tempo, locator.frame)?;
    }
    if locators
        .windows(2)
        .any(|pair| pair[0].frame >= pair[1].frame)
    {
        return Err(Invalid(
            "arrangement locator positions must be unique and ordered",
        ));
    }
    Ok(())
}

/// Derived display metadata in canonical frame order, suitable for caching
/// with the project revision. Absence of section color means neutral gray.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArrangementLocatorAppearance {
    pub id: LocatorId,
    pub frame: u64,
    pub name: String,
    pub color: Option<ElementColor>,
}

impl Project {
    pub fn arrangement_locators(&self) -> &[ArrangementLocator] {
        &self.arrangement_locators
    }

    pub fn arrangement_locator(&self, id: LocatorId) -> Option<&ArrangementLocator> {
        self.arrangement_locators
            .iter()
            .find(|locator| locator.id == id)
    }

    /// Walk the two sorted inventories once. Authored names count towards
    /// their section's ordinal, and unsectioned locators have their own count.
    pub fn arrangement_locator_appearances(&self) -> Vec<ArrangementLocatorAppearance> {
        let mut sections = self
            .arrangement_sections()
            .iter()
            .filter_map(|section| {
                let tempo = self.tempo_map();
                Some((
                    tempo.beat_to_frame(section.start()).ok()?,
                    tempo.beat_to_frame(section.end()).ok()?,
                    section,
                ))
            })
            .peekable();
        let mut section_ordinal = 0;
        let mut neutral_ordinal = 0;
        self.arrangement_locators()
            .iter()
            .map(|locator| {
                while sections
                    .peek()
                    .is_some_and(|(_, end, _)| locator.frame >= *end)
                {
                    sections.next();
                    section_ordinal = 0;
                }
                let (name, color) = match sections.peek() {
                    Some((start, _, section)) if locator.frame >= *start => {
                        section_ordinal += 1;
                        (
                            locator
                                .name
                                .clone()
                                .unwrap_or_else(|| format!("{}.{section_ordinal}", section.name())),
                            Some(section.color()),
                        )
                    }
                    _ => {
                        neutral_ordinal += 1;
                        (
                            locator
                                .name
                                .clone()
                                .unwrap_or_else(|| format!("locator {neutral_ordinal}")),
                            None,
                        )
                    }
                };
                ArrangementLocatorAppearance {
                    id: locator.id,
                    frame: locator.frame,
                    name,
                    color,
                }
            })
            .collect()
    }
}

pub(crate) fn apply(project: &mut Project, command: Command) -> Result<Created, Invalid> {
    let id = match command {
        Command::AddArrangementLocator { frame } => {
            validate_frame(project.tempo_map(), frame)?;
            let index = match project
                .arrangement_locators
                .binary_search_by_key(&frame, |locator| locator.frame)
            {
                Ok(_) => return Ok(Created::None),
                Err(index) => index,
            };
            if project.arrangement_locators.len() >= MAX_ARRANGEMENT_LOCATORS {
                return Err(Invalid("arrangement locator limit reached"));
            }
            let id = LocatorId(project.allocate_id()?);
            Arc::make_mut(&mut project.arrangement_locators).insert(
                index,
                ArrangementLocator {
                    id,
                    frame,
                    name: None,
                },
            );
            return Ok(Created::ArrangementLocator(id));
        }
        Command::MoveArrangementLocator { locator, .. }
        | Command::RenameArrangementLocator { locator, .. }
        | Command::RemoveArrangementLocator(locator) => locator,
        _ => return Err(Invalid("unsupported locator edit")),
    };
    let index = project
        .arrangement_locators
        .iter()
        .position(|locator| locator.id == id)
        .ok_or(Invalid("arrangement locator no longer exists"))?;
    match command {
        Command::MoveArrangementLocator { frame, .. } => {
            if project.arrangement_locators[index].frame == frame {
                return Ok(Created::None);
            }
            validate_frame(project.tempo_map(), frame)?;
            let destination = project
                .arrangement_locators
                .binary_search_by_key(&frame, |locator| locator.frame)
                .err()
                .ok_or(Invalid(
                    "an arrangement locator already exists at that position",
                ))?;
            let locators = Arc::make_mut(&mut project.arrangement_locators);
            let mut locator = locators.remove(index);
            locator.frame = frame;
            locators.insert(destination - usize::from(destination > index), locator);
        }
        Command::RenameArrangementLocator { name, .. } => {
            let name =
                name.filter(|name| !name.trim().is_empty() || name.chars().any(char::is_control));
            if let Some(name) = &name {
                crate::edit::name_valid(name)?;
            }
            Arc::make_mut(&mut project.arrangement_locators)[index].name = name;
        }
        Command::RemoveArrangementLocator(_) => {
            Arc::make_mut(&mut project.arrangement_locators).remove(index);
        }
        _ => return Err(Invalid("unsupported locator edit")),
    }
    Ok(Created::None)
}

/// Plan locator custody alongside the exact frame permutation of a section
/// edit. A removed frame retires its locator. A duplicate gets a fresh ID.
pub(crate) fn remap(
    project: &Project,
    tempo: &TempoMap,
    next_id: &mut u64,
    mut frames: impl FnMut(u64) -> Result<(Option<u64>, Option<u64>), Invalid>,
) -> Result<Arc<Vec<ArrangementLocator>>, Invalid> {
    if project.arrangement_locators().is_empty() {
        return Ok(Arc::clone(&project.arrangement_locators));
    }
    let mut locators = Vec::with_capacity(project.arrangement_locators().len());
    for locator in project.arrangement_locators() {
        let (original, copy) = frames(locator.frame)?;
        if let Some(frame) = original {
            locators.push(ArrangementLocator {
                frame,
                ..locator.clone()
            });
        }
        if let Some(frame) = copy {
            let id = LocatorId(*next_id);
            *next_id = next_id
                .checked_add(1)
                .ok_or(Invalid("project identity space exhausted"))?;
            locators.push(ArrangementLocator {
                id,
                frame,
                ..locator.clone()
            });
        }
        if locators.len() > MAX_ARRANGEMENT_LOCATORS {
            return Err(Invalid("arrangement locator limit reached"));
        }
    }
    locators.sort_by_key(|locator| locator.frame);
    validate(&locators, tempo)?;
    Ok(Arc::new(locators))
}

/// Shift locators at or after `start_frame` right by `frame_length` and insert
/// captured locators at `start_frame + offset`, allocating fresh global IDs.
pub(crate) fn splice_captured(
    project: &Project,
    tempo: &TempoMap,
    next_id: &mut u64,
    start_frame: u64,
    frame_length: u64,
    captured: &[(u64, Option<String>)],
) -> Result<Arc<Vec<ArrangementLocator>>, Invalid> {
    if project.arrangement_locators().is_empty() && captured.is_empty() {
        return Ok(Arc::clone(&project.arrangement_locators));
    }
    let total = project
        .arrangement_locators()
        .len()
        .checked_add(captured.len())
        .filter(|&count| count <= MAX_ARRANGEMENT_LOCATORS)
        .ok_or(Invalid("arrangement locator limit reached"))?;
    let mut locators = Vec::new();
    locators
        .try_reserve_exact(total)
        .map_err(|_| Invalid("could not reserve arrangement locators"))?;
    for locator in project.arrangement_locators() {
        let frame = if locator.frame >= start_frame {
            locator
                .frame
                .checked_add(frame_length)
                .ok_or(Invalid("arrangement locator shift overflow"))?
        } else {
            locator.frame
        };
        locators.push(ArrangementLocator {
            frame,
            ..locator.clone()
        });
    }
    for (offset, name) in captured {
        let frame = start_frame
            .checked_add(*offset)
            .ok_or(Invalid("pasted arrangement locator frame overflow"))?;
        let id = LocatorId(*next_id);
        *next_id = next_id
            .checked_add(1)
            .ok_or(Invalid("project identity space exhausted"))?;
        locators.push(ArrangementLocator {
            id,
            frame,
            name: name.clone(),
        });
    }
    locators.sort_by_key(|locator| locator.frame);
    validate(&locators, tempo)?;
    Ok(Arc::new(locators))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Editor;

    #[test]
    fn from_snapshot_validates_names_and_validate_rejects_unsorted_or_duplicate_frames() {
        let valid = from_snapshot(ArrangementLocatorSnapshot {
            id: 1,
            frame: 48_000,
            name: Some("verse".into()),
        })
        .unwrap();
        assert_eq!(valid.id(), LocatorId(1));
        assert_eq!(valid.frame(), 48_000);
        assert_eq!(valid.name(), Some("verse"));
        assert!(
            from_snapshot(ArrangementLocatorSnapshot {
                id: 2,
                frame: 0,
                name: Some("   ".into()),
            })
            .is_err()
        );

        let tempo = TempoMap::constant(48_000, crate::Tempo::new(120_000_000).unwrap()).unwrap();
        let a = ArrangementLocator {
            id: LocatorId(1),
            frame: 48_000,
            name: None,
        };
        let dup = ArrangementLocator {
            id: LocatorId(2),
            frame: 48_000,
            name: None,
        };
        let earlier = ArrangementLocator {
            id: LocatorId(3),
            frame: 24_000,
            name: None,
        };
        assert!(validate(&[earlier.clone(), a.clone()], &tempo).is_ok());
        assert!(validate(&[a.clone(), dup], &tempo).is_err());
        assert!(validate(&[a, earlier], &tempo).is_err());
    }

    #[test]
    fn add_rename_and_move_locators_maintain_sorted_order() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::ArrangementLocator(late) = editor
            .execute(Command::AddArrangementLocator { frame: 96_000 })
            .unwrap()
        else {
            panic!("expected locator")
        };
        let Created::ArrangementLocator(early) = editor
            .execute(Command::AddArrangementLocator { frame: 24_000 })
            .unwrap()
        else {
            panic!("expected locator")
        };
        editor
            .execute(Command::RenameArrangementLocator {
                locator: late,
                name: Some("chorus".into()),
            })
            .unwrap();
        let frames: Vec<u64> = editor
            .project()
            .arrangement_locators()
            .iter()
            .map(ArrangementLocator::frame)
            .collect();
        assert_eq!(frames, vec![24_000, 96_000]);
        assert_eq!(
            editor
                .project()
                .arrangement_locator(late)
                .and_then(ArrangementLocator::name),
            Some("chorus")
        );
        assert!(
            editor
                .execute(Command::MoveArrangementLocator {
                    locator: early,
                    frame: 96_000,
                })
                .is_err()
        );
    }
}
