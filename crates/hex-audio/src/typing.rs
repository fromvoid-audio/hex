//! Bounded coherent observations of actual typing application. No queue ticket
//! is promoted to an application merely because it was consumed.
use hex_engine::{
    live_input::{TypingAdmission, TypingApplication},
    typing_protocol::{Action, Owner, Refusal, Token},
};
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypingSnapshot {
    pub owner: Owner,
    pub epoch: u64,
    pub armed: bool,
    pub allowed_keys: u16,
    pub cut_through_epoch: u64,
    pub applications: [Option<TypingApplication>; 12],
    pub last_application: Option<TypingApplication>,
}
#[derive(Default)]
struct Key {
    ticket: AtomicU64,
    generation: AtomicU64,
    track: AtomicU64,
    device: AtomicU64,
    epoch: AtomicU64,
    serial: AtomicU64,
    frame: AtomicU64,
    kind: AtomicU64,
    result: AtomicU64,
    pitch_velocity: AtomicU64,
}
pub struct TypingStatus {
    sequence: AtomicU64,
    generation: AtomicU64,
    track: AtomicU64,
    device: AtomicU64,
    epoch: AtomicU64,
    armed_keys: AtomicU64,
    cut: AtomicU64,
    keys: [Key; 12],
    last: Key,
}
impl Default for TypingStatus {
    fn default() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            track: AtomicU64::new(0),
            device: AtomicU64::new(0),
            epoch: AtomicU64::new(0),
            armed_keys: AtomicU64::new(0),
            cut: AtomicU64::new(0),
            keys: std::array::from_fn(|_| Key::default()),
            last: Key::default(),
        }
    }
}
impl TypingStatus {
    // One callback writer. An exhausted sequence permanently refuses reads;
    // it cannot wrap into a false coherent observation.
    fn begin(&self) -> Option<u64> {
        let sequence = self.sequence.load(SeqCst);
        let Some(end) = sequence.checked_add(2) else {
            self.sequence.store(u64::MAX, SeqCst);
            return None;
        };
        self.sequence.store(sequence + 1, SeqCst);
        Some(end)
    }
    pub(crate) fn publish_admission(&self, admission: Option<TypingAdmission>, cut: u64) {
        let Some(end) = self.begin() else {
            return;
        };
        let (owner, epoch, armed, keys) = admission.map_or(
            (
                Owner {
                    generation: 0,
                    track: 0,
                    device: 0,
                },
                0,
                false,
                0,
            ),
            |admission| {
                (
                    admission.owner,
                    admission.epoch,
                    admission.armed,
                    admission.allowed_keys,
                )
            },
        );
        self.generation.store(owner.generation, SeqCst);
        self.track.store(owner.track, SeqCst);
        self.device.store(owner.device, SeqCst);
        self.epoch.store(epoch, SeqCst);
        self.armed_keys
            .store(u64::from(keys) | (u64::from(armed) << 16), SeqCst);
        self.cut.fetch_max(
            cut.max(admission.map_or(0, |admission| admission.cut_through_epoch)),
            SeqCst,
        );
        self.sequence.store(end, SeqCst);
    }
    pub(crate) fn publish_refusal(
        &self,
        ticket: u64,
        action: Action,
        performance_frame: u64,
        reason: Refusal,
    ) {
        self.publish_application(TypingApplication {
            ticket,
            action,
            performance_frame,
            result: Err(reason),
        });
    }
    pub(crate) fn publish_application(&self, application: TypingApplication) {
        let key = match application.action {
            Action::On { key, .. } => self.keys.get(usize::from(key)),
            Action::Off { owner, token } => self.keys.iter().find(|slot| {
                slot.epoch.load(SeqCst) == token.epoch
                    && slot.serial.load(SeqCst) == token.serial
                    && slot.generation.load(SeqCst) == owner.generation
                    && slot.track.load(SeqCst) == owner.track
                    && slot.device.load(SeqCst) == owner.device
            }),
        };
        let Some(end) = self.begin() else {
            return;
        };
        if application.ticket >= self.last.ticket.load(SeqCst) {
            self.last.write(application);
        }
        if let Some(slot) = key
            && application.ticket >= slot.ticket.load(SeqCst)
        {
            slot.write(application);
        }
        self.sequence.store(end, SeqCst);
    }
    pub fn snapshot(&self) -> Option<TypingSnapshot> {
        for _ in 0..3 {
            let start = self.sequence.load(SeqCst);
            if start & 1 != 0 {
                continue;
            }
            let owner = Owner {
                generation: self.generation.load(SeqCst),
                track: self.track.load(SeqCst),
                device: self.device.load(SeqCst),
            };
            let epoch = self.epoch.load(SeqCst);
            let keys = self.armed_keys.load(SeqCst);
            let cut = self.cut.load(SeqCst);
            let applications = std::array::from_fn(|key| self.keys[key].read());
            let last_application = self.last.read();
            if self.sequence.load(SeqCst) == start {
                return Some(TypingSnapshot {
                    owner,
                    epoch,
                    armed: keys & (1 << 16) != 0,
                    allowed_keys: keys as u16,
                    cut_through_epoch: cut,
                    applications,
                    last_application,
                });
            }
        }
        None
    }
}

impl Key {
    fn write(&self, application: TypingApplication) {
        let (owner, token, kind, pitch_velocity) = match application.action {
            Action::On {
                owner,
                token,
                key,
                pitch,
                velocity,
            } => (
                owner,
                token,
                1,
                u64::from(pitch) | (u64::from(velocity) << 8) | (u64::from(key) << 16),
            ),
            Action::Off { owner, token } => (owner, token, 2, 0),
        };
        self.ticket.store(application.ticket, SeqCst);
        self.generation.store(owner.generation, SeqCst);
        self.track.store(owner.track, SeqCst);
        self.device.store(owner.device, SeqCst);
        self.epoch.store(token.epoch, SeqCst);
        self.serial.store(token.serial, SeqCst);
        self.frame.store(application.performance_frame, SeqCst);
        self.kind.store(kind, SeqCst);
        self.result
            .store(application.result.err().map_or(0, encode_refusal), SeqCst);
        self.pitch_velocity.store(pitch_velocity, SeqCst);
    }
    fn read(&self) -> Option<TypingApplication> {
        let ticket = self.ticket.load(SeqCst);
        if ticket == 0 {
            return None;
        }
        let owner = Owner {
            generation: self.generation.load(SeqCst),
            track: self.track.load(SeqCst),
            device: self.device.load(SeqCst),
        };
        let token = Token {
            epoch: self.epoch.load(SeqCst),
            serial: self.serial.load(SeqCst),
        };
        let packed = self.pitch_velocity.load(SeqCst);
        let action = match self.kind.load(SeqCst) {
            1 => Action::On {
                owner,
                token,
                key: (packed >> 16) as u8,
                pitch: packed as u8,
                velocity: (packed >> 8) as u8,
            },
            2 => Action::Off { owner, token },
            _ => return None,
        };
        Some(TypingApplication {
            ticket,
            action,
            performance_frame: self.frame.load(SeqCst),
            result: decode_result(self.result.load(SeqCst))?,
        })
    }
}

/// Validate the actual floating value delivered to the supported native sample
/// converters, using the same overflow-safe mono mean as the host copy path.
pub fn output_is_finite(frames: &[[f32; 2]], channels: usize) -> bool {
    crate::device::host_samples_are_finite::<f32>(frames, channels)
}
fn encode_refusal(reason: Refusal) -> u64 {
    match reason {
        Refusal::Stale => 1,
        Refusal::Invalid => 2,
        Refusal::NotPlaying => 3,
        Refusal::NotAdmitted => 4,
        Refusal::Capacity => 5,
        Refusal::IdentityExhausted => 6,
    }
}
fn decode_result(value: u64) -> Option<Result<(), Refusal>> {
    Some(match value {
        0 => Ok(()),
        1 => Err(Refusal::Stale),
        2 => Err(Refusal::Invalid),
        3 => Err(Refusal::NotPlaying),
        4 => Err(Refusal::NotAdmitted),
        5 => Err(Refusal::Capacity),
        6 => Err(Refusal::IdentityExhausted),
        _ => return None,
    })
}
