//! Private cache encoding only; no reinterpretation of production metadata.
use super::super::layout::{Handle, Metadata};
use super::enable::CompactRecord;
use std::mem::size_of;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Location {
    Current(u32),
    Retained(u32),
    Absent,
    Detached,
}
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct RootRef(pub u16);
impl RootRef {
    pub fn encode(location: Location) -> Option<Self> {
        Some(Self(match location {
            Location::Current(i) if i <= 255 => u16::try_from(i).ok()?,
            Location::Retained(i) if i <= 16383 => 0x8000 | u16::try_from(i).ok()?,
            Location::Absent => 0xffff,
            Location::Detached => 0xfffe,
            _ => return None,
        }))
    }
    pub fn decode(self) -> Option<Location> {
        Some(match self.0 {
            0..=255 => Location::Current(u32::from(self.0)),
            0x8000..=0xbfff => Location::Retained(u32::from(self.0 & 0x3fff)),
            0xffff => Location::Absent,
            0xfffe => Location::Detached,
            _ => return None,
        })
    }
    pub fn original_slot(self) -> Option<u32> {
        Some(match self.decode()? {
            Location::Current(i) | Location::Retained(i) => i,
            Location::Absent => u32::MAX,
            Location::Detached => u32::MAX - 1,
        })
    }
    pub fn usable(self) -> bool {
        matches!(
            self.decode(),
            Some(Location::Current(_) | Location::Retained(_))
        )
    }
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct CachedRoots {
    pub target: RootRef,
    pub enable: RootRef,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct CachedMetadata {
    pub generation: u64,
    pub committed: u32,
    pub staged: u32,
    pub flags: u32,
    pub free_next: u32,
    pub roots: CachedRoots,
    pub retirement_next: u32,
    pub retirement_epoch: u64,
    pub retained_charge: u64,
}
impl CachedMetadata {
    pub fn from_original(m: Metadata, roots: CachedRoots) -> Option<Self> {
        if m.target_context_slot != roots.target.original_slot()? {
            return None;
        }
        roots.enable.decode()?;
        Some(Self {
            generation: m.generation,
            committed: m.committed,
            staged: m.staged,
            flags: m.flags,
            free_next: m.free_next,
            roots,
            retirement_next: m.retirement_next,
            retirement_epoch: m.retirement_epoch,
            retained_charge: m.retained_charge,
        })
    }
    pub fn original(self) -> Option<Metadata> {
        self.roots.enable.decode()?;
        Some(Metadata {
            generation: self.generation,
            committed: self.committed,
            staged: self.staged,
            flags: self.flags,
            free_next: self.free_next,
            target_context_slot: self.roots.target.original_slot()?,
            retirement_next: self.retirement_next,
            retirement_epoch: self.retirement_epoch,
            retained_charge: self.retained_charge,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct CachedSlot {
    pub record: CompactRecord,
    pub metadata: CachedMetadata,
}
#[repr(C)]
pub(super) struct CachedPool {
    pub slots: Vec<CachedSlot>,
    pub current: Vec<Handle>,
    pub staging: Vec<Handle>,
    pub free_head: u32,
    pub live: u32,
    pub next_generation: u64,
    pub retained_bytes: usize,
    pub namespace: u64,
    pub ownership_epoch: u64,
}
pub(super) fn pool_bytes() -> usize {
    let mut slots = Vec::<CachedSlot>::new();
    slots.try_reserve_exact(1142).unwrap();
    let mut current = Vec::<Handle>::new();
    current.try_reserve_exact(256).unwrap();
    let mut staging = Vec::<Handle>::new();
    staging.try_reserve_exact(256).unwrap();
    assert_eq!(
        (slots.capacity(), current.capacity(), staging.capacity()),
        (1142, 256, 256)
    );
    size_of::<CachedPool>()
        + slots.capacity() * size_of::<CachedSlot>()
        + (current.capacity() + staging.capacity()) * size_of::<Handle>()
}
