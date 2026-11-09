//! Grouped native VST3 records share the ordinary draft's header, lock,
//! fixed records and durable chain. Scope clocks are physical applications,
//! and each raw parameter has its own SDK gesture and exclusive restoration.
use super::*;

pub const MAX_GROUP_TARGETS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vst3GroupTarget {
    device: DeviceId,
    class: [u8; 16],
    ids: [u32; MAX_GROUP_TARGETS],
    len: usize,
}
impl Vst3GroupTarget {
    pub fn new(device: DeviceId, class: [u8; 16], ids: &[u32]) -> Result<Self, StorageError> {
        if class == [0; 16]
            || !(2..=MAX_GROUP_TARGETS).contains(&ids.len())
            || ids.windows(2).any(|pair| pair[0] >= pair[1])
            || ids.iter().any(|&id| id > i32::MAX as u32)
        {
            return Err(invalid());
        }
        let mut result = Self {
            device,
            class,
            ids: [0; MAX_GROUP_TARGETS],
            len: ids.len(),
        };
        result.ids[..result.len].copy_from_slice(ids);
        Ok(result)
    }

    pub fn device(self) -> DeviceId {
        self.device
    }

    pub fn class(self) -> [u8; 16] {
        self.class
    }

    pub fn ids(&self) -> &[u32] {
        &self.ids[..self.len]
    }

    pub(super) fn validate_source(self, source: &Project) -> Result<(), StorageError> {
        for &parameter in self.ids() {
            let target = DraftTarget::Device {
                device: self.device,
                target: DeviceAutomationTarget::Vst3 { parameter },
            };
            if target.vst3_class_in_source(source)? != Some(self.class) {
                return Err(StorageError::Invalid(
                    "grouped automation draft class differs from its source",
                ));
            }
        }
        Ok(())
    }
}

/// Exact SDK normalized value or an actual, exclusive restore on Main.
#[derive(Clone, Copy, Debug)]
pub enum GroupApplication {
    Value(f64),
    Restored,
}

impl PartialEq for GroupApplication {
    fn eq(&self, other: &Self) -> bool {
        match (*self, *other) {
            (Self::Value(left), Self::Value(right)) => left.to_bits() == right.to_bits(),
            (Self::Restored, Self::Restored) => true,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroupDraftEvent {
    /// Contiguous file ordinal, starting at 1. A late restore can refer to an
    /// SDK End older than another lane's already delivered value.
    pub ordinal: u64,
    pub parameter: u32,
    pub sdk_sequence: u64,
    pub sdk_gesture: u64,
    pub clock: DraftClock,
    pub application: GroupApplication,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GroupHeader {
    owner: u64,
    class: [u8; 16],
    ids: Vec<u32>,
    provenance: DraftProvenance,
    capture_clock: ClockMetadata,
}

impl GroupHeader {
    pub(super) fn encode(
        target: Vst3GroupTarget,
        provenance: DraftProvenance,
        capture_clock: CaptureClock,
    ) -> Result<Vec<u8>, StorageError> {
        Ok(serde_json::to_vec(&Self {
            owner: target.device.value(),
            class: target.class,
            ids: target.ids().to_vec(),
            provenance,
            capture_clock: ClockMetadata::from(capture_clock),
        })?)
    }

    pub(super) fn decode(
        bytes: &[u8],
        source: &Project,
    ) -> Result<(Vst3GroupTarget, DraftProvenance, CaptureClock), StorageError> {
        let meta: Self = serde_json::from_slice(bytes)?;
        let device = source
            .tracks()
            .iter()
            .flat_map(|track| track.devices())
            .chain(
                source
                    .return_tracks()
                    .iter()
                    .flat_map(|track| track.devices()),
            )
            .chain(source.main_devices())
            .chain(
                source
                    .native_sources()
                    .iter()
                    .flat_map(|track| track.devices()),
            )
            .find(|device| device.id().value() == meta.owner)
            .ok_or_else(invalid)?;
        let target = Vst3GroupTarget::new(device.id(), meta.class, &meta.ids)?;
        target.validate_source(source)?;
        let clock = CaptureClock::try_from(meta.capture_clock)?;
        validate_capture_clock(source, meta.provenance, clock)?;
        Ok((target, meta.provenance, clock))
    }
}

#[derive(Clone, Copy, Default)]
struct Member {
    last: Option<GroupDraftEvent>,
}

#[derive(Clone, Copy)]
pub(super) struct GroupState {
    target: Vst3GroupTarget,
    capture: CaptureClock,
    first: Option<DraftClock>,
    latest: Option<GroupDraftEvent>,
    members: [Member; MAX_GROUP_TARGETS],
}

impl GroupState {
    pub(super) fn new(target: Vst3GroupTarget, capture: CaptureClock) -> Self {
        Self {
            target,
            capture,
            first: None,
            latest: None,
            members: Default::default(),
        }
    }

    pub(super) fn validate(&mut self, event: GroupDraftEvent) -> Result<(), StorageError> {
        let index = self
            .target
            .ids()
            .binary_search(&event.parameter)
            .map_err(|_| invalid())?;
        let ordinal = self
            .latest
            .map_or(Some(1), |old| old.ordinal.checked_add(1));
        if ordinal != Some(event.ordinal)
            || (self.capture.loop_region.is_none() && event.clock.loop_pass != 0)
            || event.sdk_gesture == 0
            || event.sdk_sequence <= event.sdk_gesture
            || self
                .latest
                .is_some_and(|old| !clock_follows(old.clock, event.clock))
        {
            return Err(invalid());
        }
        let first = self.first.unwrap_or(event.clock);
        super::take::ClockMapping::from_clocks(first, event.clock, self.capture.loop_region)
            .map_err(|_| invalid())?;
        if self.members.iter().enumerate().any(|(slot, member)| {
            slot != index
                && member
                    .last
                    .is_some_and(|row| row.sdk_gesture == event.sdk_gesture)
        }) {
            return Err(invalid());
        }
        let member = &mut self.members[index];
        if member.last.is_some_and(|old| {
            old.sdk_gesture != event.sdk_gesture
                || old.sdk_sequence >= event.sdk_sequence
                || old.application == GroupApplication::Restored
        }) {
            return Err(invalid());
        }
        match event.application {
            GroupApplication::Value(value) if !(0.0..=1.0).contains(&value) => {
                return Err(invalid());
            }
            GroupApplication::Restored
                if member.last.is_none_or(|old| {
                    old.clock.performance_frame >= event.clock.performance_frame
                }) =>
            {
                return Err(invalid());
            }
            _ => {}
        }
        member.last = Some(event);
        self.first = Some(first);
        self.latest = Some(event);
        Ok(())
    }

    pub(super) fn finish(self, terminal: DraftTerminal) -> Result<(), StorageError> {
        if let DraftTerminal::Finished(clock) = terminal
            && (self.latest.is_none_or(|event| event.clock != clock)
                || self.members[..self.target.len].iter().any(|member| {
                    member
                        .last
                        .is_none_or(|event| event.application != GroupApplication::Restored)
                }))
        {
            return Err(StorageError::Invalid(
                "grouped automation draft has an incomplete or mismatched member",
            ));
        }
        Ok(())
    }
}

pub(super) fn encode(event: GroupDraftEvent, chain: [u8; 32]) -> [u8; RECORD_BYTES] {
    let mut record = [0; RECORD_BYTES];
    record[0] = match event.application {
        GroupApplication::Value(_) => 5,
        GroupApplication::Restored => 6,
    };
    record[4..8].copy_from_slice(&event.parameter.to_le_bytes());
    put(&mut record, 1, event.ordinal);
    put(&mut record, 2, event.clock.performance_frame);
    put(&mut record, 3, event.clock.content_frame);
    put(&mut record, 4, event.clock.loop_pass);
    if let GroupApplication::Value(value) = event.application {
        put(&mut record, 5, value.to_bits());
    }
    put(&mut record, 6, event.sdk_sequence);
    put(&mut record, 7, event.sdk_gesture);
    seal(record, chain)
}

pub(super) fn decode(record: &[u8; RECORD_BYTES]) -> Result<GroupDraftEvent, StorageError> {
    if record[1..4] != [0; 3] {
        return Err(invalid());
    }
    let normalized = u64_at(record, 40);
    let application = match record[0] {
        5 => GroupApplication::Value(f64::from_bits(normalized)),
        6 if normalized == 0 => GroupApplication::Restored,
        _ => return Err(invalid()),
    };
    Ok(GroupDraftEvent {
        ordinal: u64_at(record, 8),
        parameter: u32_at(record, 4),
        sdk_sequence: u64_at(record, 48),
        sdk_gesture: u64_at(record, 56),
        clock: DraftClock {
            performance_frame: u64_at(record, 16),
            content_frame: u64_at(record, 24),
            loop_pass: u64_at(record, 32),
        },
        application,
    })
}

fn invalid() -> StorageError {
    StorageError::Invalid("invalid grouped automation draft target, sequence or clock")
}
