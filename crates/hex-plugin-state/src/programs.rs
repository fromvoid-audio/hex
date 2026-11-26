//! Bounded control-side descriptors for VST3 `IUnitInfo` units, factory
//! program lists, stable program selector `ParamID`s, and user-preset vs
//! factory-program provenance.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Steinberg root unit identifier (`kRootUnitId = 0`).
pub const ROOT_UNIT_ID: UnitId = UnitId(0);
/// Steinberg sentinel for units without a program list (`kNoProgramListId = -1`).
pub const NO_PROGRAM_LIST_ID: ProgramListId = ProgramListId(-1);

pub const MAX_UNITS: usize = 256;
pub const MAX_PROGRAM_LISTS: usize = 64;
pub const MAX_PROGRAMS_PER_LIST: usize = 1024;
pub const MAX_NAME_BYTES: usize = 128;

const WIRE_MAGIC: &[u8; 4] = b"HXUP";
const WIRE_VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnitId(pub i32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProgramListId(pub i32);

/// One VST3 unit exposed by `IUnitInfo::getUnitInfo`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitInfo {
    pub id: UnitId,
    pub name: String,
    pub parent_id: UnitId,
    pub program_list_id: ProgramListId,
}

/// One factory program entry within a VST3 `ProgramList`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramEntry {
    pub index: u32,
    pub name: String,
}

/// One VST3 program list (`IUnitInfo::getProgramListInfo`) and its optional
/// `kIsProgramChange` selector `ParamID`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramList {
    pub id: ProgramListId,
    pub name: String,
    pub selector_param_id: Option<u32>,
    pub programs: Vec<ProgramEntry>,
}

/// Explicit distinction between plugin factory programs and host user presets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProgramOrigin {
    FactoryProgram {
        unit_id: UnitId,
        list_id: ProgramListId,
        program_index: u32,
    },
    UserPreset {
        name: String,
    },
}

/// Validated selection target for activating a factory program via its stable
/// `kIsProgramChange` `ParamID`.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgramSelection {
    pub origin: ProgramOrigin,
    pub selector_param_id: u32,
    pub normalized_value: f64,
    pub program_name: String,
}

/// Result of refreshing unit/program metadata while retaining stable selector `ParamID`s.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramRefreshReport {
    pub retained_lists: Vec<ProgramListId>,
    pub added_lists: Vec<ProgramListId>,
    pub removed_lists: Vec<ProgramListId>,
    pub changed_selectors: Vec<(ProgramListId, Option<u32>, Option<u32>)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramCatalogError {
    TooManyUnits,
    TooManyProgramLists,
    TooManyPrograms,
    NameTooLong,
    DuplicateUnitId,
    DuplicateProgramListId,
    DuplicateSelectorParamId,
    InvalidParentUnit,
    UnitHierarchyCycle,
    UnknownProgramList,
    InvalidProgramIndex,
    UnknownUnit,
    UnitHasNoProgramList,
    MissingSelectorParameter,
    InvalidNormalizedValue,
    MalformedWire,
}

impl fmt::Display for ProgramCatalogError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(match self {
            Self::TooManyUnits => "vst3 unit count exceeds the bounded limit",
            Self::TooManyProgramLists => "vst3 program list count exceeds the bounded limit",
            Self::TooManyPrograms => "vst3 program count exceeds the bounded limit",
            Self::NameTooLong => "vst3 unit or program name exceeds the bounded limit",
            Self::DuplicateUnitId => "vst3 unit id is duplicated",
            Self::DuplicateProgramListId => "vst3 program list id is duplicated",
            Self::DuplicateSelectorParamId => "vst3 program selector parameter id is duplicated",
            Self::InvalidParentUnit => "vst3 unit references an unknown parent unit",
            Self::UnitHierarchyCycle => "vst3 unit hierarchy contains a cycle",
            Self::UnknownProgramList => "vst3 unit references an unknown program list",
            Self::InvalidProgramIndex => "vst3 program index is out of range",
            Self::UnknownUnit => "vst3 unit does not exist",
            Self::UnitHasNoProgramList => "vst3 unit does not expose a program list",
            Self::MissingSelectorParameter => {
                "vst3 program list has no program-change selector parameter"
            }
            Self::InvalidNormalizedValue => "vst3 normalized program value is invalid",
            Self::MalformedWire => "vst3 program catalog wire payload is malformed",
        })
    }
}

impl std::error::Error for ProgramCatalogError {}

/// Steinberg VST3 discrete parameter normalization for a program list of `count` entries.
pub fn program_index_to_normalized(index: u32, count: u32) -> Result<f64, ProgramCatalogError> {
    if count == 0 || index >= count {
        return Err(ProgramCatalogError::InvalidProgramIndex);
    }
    if count == 1 {
        return Ok(0.0);
    }
    Ok(f64::from(index) / f64::from(count - 1))
}

/// Steinberg VST3 normalized-to-discrete index conversion (`min(stepCount, floor(norm * (stepCount + 1)))`).
pub fn normalized_to_program_index(
    normalized: f64,
    count: u32,
) -> Result<u32, ProgramCatalogError> {
    if count == 0 {
        return Err(ProgramCatalogError::InvalidProgramIndex);
    }
    if !normalized.is_finite() || !(0.0..=1.0).contains(&normalized) {
        return Err(ProgramCatalogError::InvalidNormalizedValue);
    }
    if count == 1 {
        return Ok(0);
    }
    let step_count = count - 1;
    let raw = (normalized * f64::from(count)).floor() as u32;
    Ok(raw.min(step_count))
}

/// Validated, bounded VST3 unit and factory program catalog.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProgramCatalog {
    units: Vec<UnitInfo>,
    program_lists: Vec<ProgramList>,
}

impl ProgramCatalog {
    pub fn new(
        units: Vec<UnitInfo>,
        program_lists: Vec<ProgramList>,
    ) -> Result<Self, ProgramCatalogError> {
        if units.len() > MAX_UNITS {
            return Err(ProgramCatalogError::TooManyUnits);
        }
        if program_lists.len() > MAX_PROGRAM_LISTS {
            return Err(ProgramCatalogError::TooManyProgramLists);
        }

        let mut list_ids = BTreeSet::new();
        let mut selector_ids = BTreeSet::new();
        for list in &program_lists {
            if list.id == NO_PROGRAM_LIST_ID || !list_ids.insert(list.id) {
                return Err(ProgramCatalogError::DuplicateProgramListId);
            }
            if list.name.len() > MAX_NAME_BYTES {
                return Err(ProgramCatalogError::NameTooLong);
            }
            if let Some(param_id) = list.selector_param_id
                && !selector_ids.insert(param_id)
            {
                return Err(ProgramCatalogError::DuplicateSelectorParamId);
            }
            if list.programs.len() > MAX_PROGRAMS_PER_LIST {
                return Err(ProgramCatalogError::TooManyPrograms);
            }
            for (expected, entry) in list.programs.iter().enumerate() {
                if entry.index != expected as u32 {
                    return Err(ProgramCatalogError::InvalidProgramIndex);
                }
                if entry.name.len() > MAX_NAME_BYTES {
                    return Err(ProgramCatalogError::NameTooLong);
                }
            }
        }

        let mut unit_parents = BTreeMap::new();
        for unit in &units {
            if unit.name.len() > MAX_NAME_BYTES {
                return Err(ProgramCatalogError::NameTooLong);
            }
            if unit_parents.insert(unit.id, unit.parent_id).is_some() {
                return Err(ProgramCatalogError::DuplicateUnitId);
            }
            if unit.program_list_id != NO_PROGRAM_LIST_ID
                && !list_ids.contains(&unit.program_list_id)
            {
                return Err(ProgramCatalogError::UnknownProgramList);
            }
        }

        // Validate parent links and cycle freedom. Root unit (id == 0) may point
        // to itself or -1 (no parent).
        for unit in &units {
            let mut cursor = unit.id;
            let mut visited = BTreeSet::new();
            loop {
                visited.insert(cursor);
                let Some(&parent) = unit_parents.get(&cursor) else {
                    return Err(ProgramCatalogError::InvalidParentUnit);
                };
                if parent.0 == -1 || (cursor == ROOT_UNIT_ID && parent == ROOT_UNIT_ID) {
                    break;
                }
                if !unit_parents.contains_key(&parent) {
                    return Err(ProgramCatalogError::InvalidParentUnit);
                }
                if visited.contains(&parent) {
                    return Err(ProgramCatalogError::UnitHierarchyCycle);
                }
                cursor = parent;
            }
        }

        Ok(Self {
            units,
            program_lists,
        })
    }

    pub fn units(&self) -> &[UnitInfo] {
        &self.units
    }

    pub fn program_lists(&self) -> &[ProgramList] {
        &self.program_lists
    }

    pub fn unit(&self, id: UnitId) -> Option<&UnitInfo> {
        self.units.iter().find(|unit| unit.id == id)
    }

    pub fn program_list(&self, id: ProgramListId) -> Option<&ProgramList> {
        self.program_lists.iter().find(|list| list.id == id)
    }

    pub fn program_list_for_unit(&self, unit_id: UnitId) -> Option<&ProgramList> {
        let unit = self.unit(unit_id)?;
        if unit.program_list_id == NO_PROGRAM_LIST_ID {
            return None;
        }
        self.program_list(unit.program_list_id)
    }

    pub fn program_list_for_selector_param(&self, param_id: u32) -> Option<&ProgramList> {
        self.program_lists
            .iter()
            .find(|list| list.selector_param_id == Some(param_id))
    }

    /// Resolve a factory program selection on `unit_id` into its exact stable
    /// `ParamID` and Steinberg normalized parameter value.
    pub fn select_program(
        &self,
        unit_id: UnitId,
        program_index: u32,
    ) -> Result<ProgramSelection, ProgramCatalogError> {
        let unit = self.unit(unit_id).ok_or(ProgramCatalogError::UnknownUnit)?;
        if unit.program_list_id == NO_PROGRAM_LIST_ID {
            return Err(ProgramCatalogError::UnitHasNoProgramList);
        }
        let list = self
            .program_list(unit.program_list_id)
            .ok_or(ProgramCatalogError::UnknownProgramList)?;
        let selector_param_id = list
            .selector_param_id
            .ok_or(ProgramCatalogError::MissingSelectorParameter)?;
        let entry = list
            .programs
            .get(program_index as usize)
            .ok_or(ProgramCatalogError::InvalidProgramIndex)?;
        let normalized_value =
            program_index_to_normalized(program_index, list.programs.len() as u32)?;
        Ok(ProgramSelection {
            origin: ProgramOrigin::FactoryProgram {
                unit_id,
                list_id: list.id,
                program_index,
            },
            selector_param_id,
            normalized_value,
            program_name: entry.name.clone(),
        })
    }

    /// Compare an updated catalog against `self` while preserving stable
    /// `selector_param_id`s when the updated list omits an already-known selector.
    pub fn refresh_preserving_selectors(
        &self,
        mut updated: ProgramCatalog,
    ) -> Result<(ProgramCatalog, ProgramRefreshReport), ProgramCatalogError> {
        let previous_by_id: BTreeMap<ProgramListId, Option<u32>> = self
            .program_lists
            .iter()
            .map(|list| (list.id, list.selector_param_id))
            .collect();
        let mut used_selectors: BTreeSet<u32> = updated
            .program_lists
            .iter()
            .filter_map(|list| list.selector_param_id)
            .collect();

        let mut retained_lists = Vec::new();
        let mut added_lists = Vec::new();
        let mut changed_selectors = Vec::new();
        let mut updated_ids = BTreeSet::new();

        for list in &mut updated.program_lists {
            updated_ids.insert(list.id);
            match previous_by_id.get(&list.id).copied() {
                Some(prev_selector) => {
                    retained_lists.push(list.id);
                    if list.selector_param_id.is_none()
                        && let Some(stable_id) = prev_selector
                        && used_selectors.insert(stable_id)
                    {
                        list.selector_param_id = Some(stable_id);
                    }
                    if prev_selector != list.selector_param_id {
                        changed_selectors.push((list.id, prev_selector, list.selector_param_id));
                    }
                }
                None => added_lists.push(list.id),
            }
        }

        let removed_lists = self
            .program_lists
            .iter()
            .map(|list| list.id)
            .filter(|id| !updated_ids.contains(id))
            .collect();

        let validated = ProgramCatalog::new(updated.units, updated.program_lists)?;
        Ok((
            validated,
            ProgramRefreshReport {
                retained_lists,
                added_lists,
                removed_lists,
                changed_selectors,
            },
        ))
    }

    /// Deterministic bounded wire encoding for IPC transport.
    pub fn encode_wire(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(WIRE_MAGIC);
        out.push(WIRE_VERSION);
        out.extend_from_slice(&(self.units.len() as u16).to_le_bytes());
        out.extend_from_slice(&(self.program_lists.len() as u16).to_le_bytes());
        for unit in &self.units {
            out.extend_from_slice(&unit.id.0.to_le_bytes());
            out.extend_from_slice(&unit.parent_id.0.to_le_bytes());
            out.extend_from_slice(&unit.program_list_id.0.to_le_bytes());
            write_str(&mut out, &unit.name);
        }
        for list in &self.program_lists {
            out.extend_from_slice(&list.id.0.to_le_bytes());
            match list.selector_param_id {
                Some(param_id) => {
                    out.push(1);
                    out.extend_from_slice(&param_id.to_le_bytes());
                }
                None => {
                    out.push(0);
                    out.extend_from_slice(&0_u32.to_le_bytes());
                }
            }
            write_str(&mut out, &list.name);
            out.extend_from_slice(&(list.programs.len() as u16).to_le_bytes());
            for program in &list.programs {
                out.extend_from_slice(&program.index.to_le_bytes());
                write_str(&mut out, &program.name);
            }
        }
        out
    }

    /// Strict bounded wire decoding; rejects truncation, trailing bytes, invalid UTF-8, or invalid hierarchy.
    pub fn decode_wire(bytes: &[u8]) -> Result<Self, ProgramCatalogError> {
        let mut cursor = 0usize;
        let magic = take_bytes(bytes, &mut cursor, 4)?;
        if magic != WIRE_MAGIC {
            return Err(ProgramCatalogError::MalformedWire);
        }
        let version = take_bytes(bytes, &mut cursor, 1)?[0];
        if version != WIRE_VERSION {
            return Err(ProgramCatalogError::MalformedWire);
        }
        let unit_count = usize::from(read_u16(bytes, &mut cursor)?);
        let list_count = usize::from(read_u16(bytes, &mut cursor)?);
        if unit_count > MAX_UNITS {
            return Err(ProgramCatalogError::TooManyUnits);
        }
        if list_count > MAX_PROGRAM_LISTS {
            return Err(ProgramCatalogError::TooManyProgramLists);
        }
        let mut units = Vec::with_capacity(unit_count);
        for _ in 0..unit_count {
            let id = UnitId(read_i32(bytes, &mut cursor)?);
            let parent_id = UnitId(read_i32(bytes, &mut cursor)?);
            let program_list_id = ProgramListId(read_i32(bytes, &mut cursor)?);
            let name = read_str(bytes, &mut cursor)?;
            units.push(UnitInfo {
                id,
                name,
                parent_id,
                program_list_id,
            });
        }
        let mut program_lists = Vec::with_capacity(list_count);
        for _ in 0..list_count {
            let id = ProgramListId(read_i32(bytes, &mut cursor)?);
            let has_selector = take_bytes(bytes, &mut cursor, 1)?[0];
            let raw_selector = read_u32(bytes, &mut cursor)?;
            let selector_param_id = match has_selector {
                0 if raw_selector == 0 => None,
                1 => Some(raw_selector),
                _ => return Err(ProgramCatalogError::MalformedWire),
            };
            let name = read_str(bytes, &mut cursor)?;
            let program_count = usize::from(read_u16(bytes, &mut cursor)?);
            if program_count > MAX_PROGRAMS_PER_LIST {
                return Err(ProgramCatalogError::TooManyPrograms);
            }
            let mut programs = Vec::with_capacity(program_count);
            for _ in 0..program_count {
                let index = read_u32(bytes, &mut cursor)?;
                let pname = read_str(bytes, &mut cursor)?;
                programs.push(ProgramEntry { index, name: pname });
            }
            program_lists.push(ProgramList {
                id,
                name,
                selector_param_id,
                programs,
            });
        }
        if cursor != bytes.len() {
            return Err(ProgramCatalogError::MalformedWire);
        }
        Self::new(units, program_lists)
    }
}

fn write_str(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&(value.len() as u16).to_le_bytes());
    out.extend_from_slice(value.as_bytes());
}

fn take_bytes<'a>(
    bytes: &'a [u8],
    cursor: &mut usize,
    count: usize,
) -> Result<&'a [u8], ProgramCatalogError> {
    let end = cursor
        .checked_add(count)
        .ok_or(ProgramCatalogError::MalformedWire)?;
    let slice = bytes
        .get(*cursor..end)
        .ok_or(ProgramCatalogError::MalformedWire)?;
    *cursor = end;
    Ok(slice)
}

fn read_u16(bytes: &[u8], cursor: &mut usize) -> Result<u16, ProgramCatalogError> {
    let raw = take_bytes(bytes, cursor, 2)?;
    Ok(u16::from_le_bytes(
        raw.try_into()
            .map_err(|_| ProgramCatalogError::MalformedWire)?,
    ))
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, ProgramCatalogError> {
    let raw = take_bytes(bytes, cursor, 4)?;
    Ok(u32::from_le_bytes(
        raw.try_into()
            .map_err(|_| ProgramCatalogError::MalformedWire)?,
    ))
}

fn read_i32(bytes: &[u8], cursor: &mut usize) -> Result<i32, ProgramCatalogError> {
    let raw = take_bytes(bytes, cursor, 4)?;
    Ok(i32::from_le_bytes(
        raw.try_into()
            .map_err(|_| ProgramCatalogError::MalformedWire)?,
    ))
}

fn read_str(bytes: &[u8], cursor: &mut usize) -> Result<String, ProgramCatalogError> {
    let len = usize::from(read_u16(bytes, cursor)?);
    if len > MAX_NAME_BYTES {
        return Err(ProgramCatalogError::NameTooLong);
    }
    let slice = take_bytes(bytes, cursor, len)?;
    std::str::from_utf8(slice)
        .map(str::to_owned)
        .map_err(|_| ProgramCatalogError::MalformedWire)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_catalog() -> ProgramCatalog {
        ProgramCatalog::new(
            vec![
                UnitInfo {
                    id: ROOT_UNIT_ID,
                    name: "root".into(),
                    parent_id: UnitId(-1),
                    program_list_id: ProgramListId(10),
                },
                UnitInfo {
                    id: UnitId(1),
                    name: "filter".into(),
                    parent_id: ROOT_UNIT_ID,
                    program_list_id: NO_PROGRAM_LIST_ID,
                },
            ],
            vec![ProgramList {
                id: ProgramListId(10),
                name: "factory bank".into(),
                selector_param_id: Some(777),
                programs: vec![
                    ProgramEntry {
                        index: 0,
                        name: "init".into(),
                    },
                    ProgramEntry {
                        index: 1,
                        name: "warm pad".into(),
                    },
                    ProgramEntry {
                        index: 2,
                        name: "bright lead".into(),
                    },
                ],
            }],
        )
        .unwrap()
    }

    #[test]
    fn unit_program_selection_normalization_and_wire_round_trip() {
        let catalog = sample_catalog();
        let sel = catalog.select_program(ROOT_UNIT_ID, 1).unwrap();
        assert_eq!(sel.selector_param_id, 777);
        assert_eq!(sel.normalized_value, 0.5);
        assert_eq!(sel.program_name, "warm pad");
        assert_eq!(
            sel.origin,
            ProgramOrigin::FactoryProgram {
                unit_id: ROOT_UNIT_ID,
                list_id: ProgramListId(10),
                program_index: 1,
            }
        );
        assert_ne!(
            sel.origin,
            ProgramOrigin::UserPreset {
                name: "warm pad".into()
            }
        );

        for count in [1_u32, 2, 3, 16, 128] {
            for idx in 0..count {
                let norm = program_index_to_normalized(idx, count).unwrap();
                assert_eq!(normalized_to_program_index(norm, count).unwrap(), idx);
            }
        }
        assert!(normalized_to_program_index(f64::NAN, 4).is_err());
        assert!(normalized_to_program_index(-0.01, 4).is_err());
        assert!(normalized_to_program_index(1.01, 4).is_err());

        let wire = catalog.encode_wire();
        assert_eq!(ProgramCatalog::decode_wire(&wire).unwrap(), catalog);
        for len in 0..wire.len() {
            assert!(ProgramCatalog::decode_wire(&wire[..len]).is_err());
        }
        let mut extra = wire;
        extra.push(0);
        assert_eq!(
            ProgramCatalog::decode_wire(&extra),
            Err(ProgramCatalogError::MalformedWire)
        );
    }

    #[test]
    fn refresh_retains_stable_selector_param_ids_and_rejects_absent_programs() {
        let initial = sample_catalog();
        let updated = ProgramCatalog::new(
            vec![UnitInfo {
                id: ROOT_UNIT_ID,
                name: "root".into(),
                parent_id: UnitId(-1),
                program_list_id: ProgramListId(10),
            }],
            vec![ProgramList {
                id: ProgramListId(10),
                name: "factory bank".into(),
                selector_param_id: None,
                programs: vec![ProgramEntry {
                    index: 0,
                    name: "init".into(),
                }],
            }],
        )
        .unwrap();

        let (merged, report) = initial.refresh_preserving_selectors(updated).unwrap();
        assert_eq!(
            merged
                .program_list(ProgramListId(10))
                .unwrap()
                .selector_param_id,
            Some(777)
        );
        assert_eq!(report.retained_lists, vec![ProgramListId(10)]);
        assert!(report.changed_selectors.is_empty());

        assert_eq!(
            merged.select_program(ROOT_UNIT_ID, 5),
            Err(ProgramCatalogError::InvalidProgramIndex)
        );
        assert_eq!(
            merged.select_program(UnitId(99), 0),
            Err(ProgramCatalogError::UnknownUnit)
        );
    }

    #[test]
    fn rejects_unit_cycles_and_unknown_program_lists() {
        let cyclic = ProgramCatalog::new(
            vec![
                UnitInfo {
                    id: UnitId(1),
                    name: "a".into(),
                    parent_id: UnitId(2),
                    program_list_id: NO_PROGRAM_LIST_ID,
                },
                UnitInfo {
                    id: UnitId(2),
                    name: "b".into(),
                    parent_id: UnitId(1),
                    program_list_id: NO_PROGRAM_LIST_ID,
                },
            ],
            vec![],
        );
        assert_eq!(cyclic, Err(ProgramCatalogError::UnitHierarchyCycle));

        let dangling = ProgramCatalog::new(
            vec![UnitInfo {
                id: ROOT_UNIT_ID,
                name: "root".into(),
                parent_id: UnitId(-1),
                program_list_id: ProgramListId(999),
            }],
            vec![],
        );
        assert_eq!(dangling, Err(ProgramCatalogError::UnknownProgramList));
    }
}
