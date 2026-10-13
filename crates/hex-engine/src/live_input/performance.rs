//! Audio-owned typing state. Application receipts remain speculative until the
//! complete render transaction commits, including the instrument and suffix.
use super::PreparedTypingArm;
use crate::{
    Invalid,
    typing_protocol::{Action, Owner, Refusal, Token},
};
use hex_lattice::{NativeInstrumentEvent, NoteGate};

const VOICES: usize = 64;
const COMMANDS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypingApplication {
    pub ticket: u64,
    pub action: Action,
    pub performance_frame: u64,
    pub result: Result<(), Refusal>,
}

#[derive(Clone, Copy)]
struct Voice {
    token: Token,
    key: u8,
    onset: u64,
    gate: NoteGate,
    native_id: Option<u64>,
}

/// Constructed on control, moved into playback, and sampled without allocation.
/// Its owner is captured at arm time and never follows editor selection.
pub struct TypingPerformance {
    arm: PreparedTypingArm,
    owner: Owner,
    epoch: u64,
    through_epoch: u64,
    armed: bool,
    primary: [Option<Voice>; VOICES],
    scratch: [Option<Voice>; VOICES],
    pending: [Option<(u64, Action)>; COMMANDS],
    pending_len: usize,
    last_ticket: u64,
    receipts: [Option<TypingApplication>; COMMANDS],
    receipt_len: usize,
    applied: [Option<TypingApplication>; COMMANDS],
    native_events: [Option<NativeInstrumentEvent>; COMMANDS],
    event_len: usize,
    transaction: bool,
    block_frame: u64,
    live_limit: usize,
    authored_reservation: usize,
    authored_events: usize,
    inherited_authored: usize,
    inherited_until: u64,
    inherited_bound_known: bool,
    stable_authored: usize,
    inherited_before: (usize, u64, bool),
    admitted_source: Option<std::sync::Weak<crate::PreparedRevision>>,
    serial_primary: u64,
    serial_scratch: u64,
    continuation: Option<hex_lattice::AnalyticContinuation>,
    bridge_bytes: usize,
}

impl TypingPerformance {
    pub fn new(arm: PreparedTypingArm, epoch: u64) -> Result<Self, Invalid> {
        if epoch == 0 {
            return Err(Invalid("typing epoch is invalid"));
        }
        let target = arm.target();
        let live_limit = arm.voice_limit();
        Ok(Self {
            arm,
            epoch,
            owner: target.owner()?,
            through_epoch: 0,
            armed: true,
            primary: [None; VOICES],
            scratch: [None; VOICES],
            pending: [None; COMMANDS],
            pending_len: 0,
            last_ticket: 0,
            receipts: [None; COMMANDS],
            receipt_len: 0,
            applied: [None; COMMANDS],
            native_events: [None; COMMANDS],
            event_len: 0,
            transaction: false,
            block_frame: 0,
            live_limit,
            authored_reservation: 0,
            authored_events: 0,
            inherited_authored: 0,
            inherited_until: 0,
            inherited_bound_known: false,
            stable_authored: 0,
            inherited_before: (0, 0, false),
            admitted_source: None,
            serial_primary: 0,
            serial_scratch: 0,
            continuation: None,
            bridge_bytes: 0,
        })
    }
    pub fn arm(&self) -> &PreparedTypingArm {
        &self.arm
    }
    pub fn owner(&self) -> Owner {
        self.owner
    }
    pub fn is_armed(&self) -> bool {
        self.armed && self.live_limit != 0
    }
    pub fn active_voices(&self) -> usize {
        self.primary.iter().flatten().count()
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn cut_through_epoch(&self) -> u64 {
        self.through_epoch
    }
    pub(crate) fn cancel_unapplied_onsets(&mut self, frame: u64) {
        let mut retained = 0;
        for index in 0..self.pending_len {
            let Some((ticket, action)) = self.pending[index] else {
                continue;
            };
            if matches!(action, Action::On { .. }) {
                self.serial_primary = self.serial_primary.max(action.token().serial);
                self.serial_scratch = self.serial_primary;
                if let Some(slot) = self.applied.iter_mut().find(|slot| slot.is_none()) {
                    *slot = Some(TypingApplication {
                        ticket,
                        action,
                        performance_frame: frame,
                        result: Err(Refusal::NotPlaying),
                    });
                }
            } else {
                self.pending[retained] = Some((ticket, action));
                retained += 1;
            }
        }
        self.pending[retained..].fill(None);
        self.pending_len = retained;
    }
    pub(crate) fn reserve_authored_voices(&mut self, authored: usize) -> Result<(), Invalid> {
        self.live_limit = self
            .arm
            .voice_limit()
            .checked_sub(authored)
            .filter(|remaining| *remaining != 0)
            .ok_or(Invalid("authored voices leave no typing capacity"))?;
        self.stable_authored = authored;
        Ok(())
    }
    pub(crate) fn set_authored_reservation(&mut self, voices: usize, events: usize) {
        self.authored_reservation = voices;
        self.authored_events = events;
    }
    pub(crate) fn authored_reservation(&self) -> (usize, usize) {
        (self.authored_reservation, self.authored_events)
    }
    pub(crate) fn inherited_authored(&self, frame: u64) -> (usize, u64) {
        if frame < self.inherited_until {
            (self.inherited_authored, self.inherited_until)
        } else {
            (0, 0)
        }
    }
    pub(crate) fn reserve_inherited_authored(&mut self, count: usize, until: u64) {
        self.inherited_authored = count;
        self.inherited_until = until;
        self.inherited_bound_known = true;
    }
    pub(crate) fn authored_budget_at(&self, frame: u64, pending_route: bool) -> Option<usize> {
        let known = self
            .authored_reservation
            .checked_add(self.inherited_authored(frame).0)?;
        Some(if pending_route {
            self.stable_authored.max(known)
        } else if self.inherited_bound_known {
            known
        } else {
            self.stable_authored
        })
    }
    pub(crate) fn speculative_active_voices(&self) -> usize {
        self.scratch.iter().flatten().count()
    }
    pub(crate) fn refresh_live_limit(&mut self, authored: usize) -> Result<(), Invalid> {
        let remaining = self
            .arm
            .voice_limit()
            .checked_sub(authored)
            .ok_or(Invalid("retained input exceeds combined voice capacity"))?;
        if self.active_voices() > remaining {
            return Err(Invalid(
                "retained input cannot preserve all original voices",
            ));
        }
        self.live_limit = remaining;
        Ok(())
    }
    pub(crate) fn can_reserve_authored(&self, authored: usize) -> bool {
        self.arm
            .voice_limit()
            .checked_sub(authored)
            .is_some_and(|remaining| {
                remaining != 0
                    && self.active_voices()
                        + self
                            .pending
                            .iter()
                            .flatten()
                            .filter(|(_, action)| matches!(action, Action::On { .. }))
                            .count()
                        <= remaining
            })
    }
    pub(crate) fn admit_source(&mut self, source: &std::sync::Arc<crate::PreparedRevision>) {
        self.admitted_source = Some(std::sync::Arc::downgrade(source));
    }
    pub(crate) fn source_matches(&self, source: &std::sync::Arc<crate::PreparedRevision>) -> bool {
        self.admitted_source
            .as_ref()
            .is_some_and(|admitted| admitted.as_ptr() == std::sync::Arc::as_ptr(source))
    }
    pub fn declared_storage_bytes(&self) -> Option<usize> {
        // The inline arm is already included in Self; include its captured
        // payload, context and ownership bookkeeping exactly once.
        std::mem::size_of::<Self>()
            .checked_add(self.bridge_bytes)?
            .checked_add(
                self.arm
                    .declared_storage_bytes()?
                    .checked_sub(std::mem::size_of::<PreparedTypingArm>())?,
            )
    }
    pub(crate) fn charge_bridge(&mut self, bytes: usize) {
        self.bridge_bytes = bytes;
    }
    /// This acknowledges only admission into this bounded pending prefix.
    /// Native application is reported by take_application after render commit.
    pub fn queue(&mut self, ticket: u64, action: Action, playing: bool) -> Result<(), Refusal> {
        if self.transaction || ticket == 0 || ticket == u64::MAX || ticket <= self.last_ticket {
            return Err(Refusal::Invalid);
        }
        self.last_ticket = ticket;
        let (owner, token) = match action {
            Action::On { owner, token, .. } | Action::Off { owner, token } => (owner, token),
        };
        if owner != self.owner || token.epoch != self.epoch || token.epoch <= self.through_epoch {
            return Err(Refusal::Stale);
        }
        if matches!(action, Action::On { .. }) {
            if !playing {
                return Err(Refusal::NotPlaying);
            }
            if !self.armed {
                return Err(Refusal::NotAdmitted);
            }
        }
        if self.pending_len + self.applied.iter().flatten().count() == COMMANDS {
            return Err(Refusal::Capacity);
        }
        self.pending[self.pending_len] = Some((ticket, action));
        self.pending_len += 1;
        Ok(())
    }
    /// Watermark is observed before input admission/render prefix. Releasing
    /// voices retain their owner; a cut never rewrites their onset or phase.
    pub fn cut(&mut self, through_epoch: u64) {
        self.through_epoch = self.through_epoch.max(through_epoch);
        if self.epoch <= self.through_epoch {
            self.armed = false;
        }
    }
    pub(crate) fn begin_block(
        &mut self,
        frame: u64,
        mut allocate: impl FnMut() -> Result<u64, Invalid>,
    ) -> Result<(), Invalid> {
        self.block_frame = frame;
        self.inherited_before = (
            self.inherited_authored,
            self.inherited_until,
            self.inherited_bound_known,
        );
        self.scratch = self.primary;
        self.serial_scratch = self.serial_primary;
        self.receipts.fill(None);
        self.receipt_len = 0;
        self.native_events.fill(None);
        self.event_len = 0;
        self.transaction = true;
        let release = self.arm.release_frames();
        for slot in &mut self.scratch {
            if slot.is_some_and(|voice| match voice.gate {
                NoteGate::Held => false,
                NoteGate::Released { at_age_frames } => voice
                    .onset
                    .checked_add(at_age_frames)
                    .and_then(|off| off.checked_add(release))
                    .is_some_and(|end| end <= frame),
            }) {
                *slot = None;
            }
        }
        // At most 64 cut releases can coexist with a 32-command prefix. Native
        // releases are generated into the existing 256-event host vector later.
        for voice in self.scratch.iter_mut().flatten() {
            if voice.token.epoch <= self.through_epoch && voice.gate == NoteGate::Held {
                voice.gate = NoteGate::Released {
                    at_age_frames: frame
                        .checked_sub(voice.onset)
                        .ok_or(Invalid("typing onset exceeds performance frame"))?,
                };
            }
        }
        for index in 0..self.pending_len {
            let (ticket, action) =
                self.pending[index].ok_or(Invalid("typing prefix is incomplete"))?;
            let zero_span = matches!(action, Action::On { .. }) && self.pending[index + 1..self.pending_len].iter().flatten().any(|(_, later)| matches!(later, Action::Off { token, .. } if *token == action.token()));
            let result = if zero_span {
                self.serial_scratch = self.serial_scratch.max(action.token().serial);
                Err(Refusal::Stale)
            } else {
                self.apply_at(action, frame, &mut allocate)?
            };
            self.receipts[index] = Some(TypingApplication {
                ticket,
                action,
                performance_frame: frame,
                result,
            });
            self.receipt_len += 1;
        }
        Ok(())
    }
    fn apply_at(
        &mut self,
        action: Action,
        frame: u64,
        allocate: &mut impl FnMut() -> Result<u64, Invalid>,
    ) -> Result<Result<(), Refusal>, Invalid> {
        let token = action.token();
        if token.epoch <= self.through_epoch {
            return Ok(Err(Refusal::Stale));
        }
        match action {
            Action::On {
                key,
                pitch,
                velocity,
                ..
            } => {
                if token.serial <= self.serial_scratch {
                    return Ok(Err(Refusal::Stale));
                }
                self.serial_scratch = token.serial;
                if !self.armed {
                    return Ok(Err(Refusal::NotAdmitted));
                }
                if key >= 12
                    || pitch != 60 + key
                    || velocity != super::TYPING_VELOCITY
                    || token.serial == 0
                    || self.arm.allowed_keys() & (1 << key) == 0
                {
                    return Ok(Err(Refusal::Invalid));
                }
                if self
                    .scratch
                    .iter()
                    .flatten()
                    .any(|voice| voice.token == token)
                {
                    return Ok(Err(Refusal::Stale));
                }
                if self.scratch.iter().flatten().count() >= self.live_limit {
                    return Ok(Err(Refusal::Capacity));
                }
                let Some(slot) = self.scratch.iter_mut().find(|slot| slot.is_none()) else {
                    return Ok(Err(Refusal::Capacity));
                };
                let native_id = if self.arm.is_mutable() {
                    Some(allocate()?)
                } else {
                    None
                };
                *slot = Some(Voice {
                    token,
                    key,
                    onset: frame,
                    gate: NoteGate::Held,
                    native_id,
                });
                if let Some(id) = native_id {
                    self.native_events[self.event_len] = Some(NativeInstrumentEvent::NoteOn {
                        frame_offset: 0,
                        occurrence_id: id,
                        pitch,
                        velocity,
                    });
                    self.event_len += 1;
                }
                Ok(Ok(()))
            }
            Action::Off { .. } => {
                let Some(voice) = self
                    .scratch
                    .iter_mut()
                    .flatten()
                    .find(|voice| voice.token == token)
                else {
                    return Ok(Err(Refusal::Stale));
                };
                if voice.gate != NoteGate::Held {
                    return Ok(Err(Refusal::Stale));
                }
                let age = frame
                    .checked_sub(voice.onset)
                    .ok_or(Invalid("typing release precedes onset"))?;
                frame
                    .checked_add(self.arm.release_frames())
                    .ok_or(Invalid("typing release deadline overflow"))?;
                voice.gate = NoteGate::Released { at_age_frames: age };
                Ok(Ok(()))
            }
        }
    }
    pub(crate) fn speculative_voices(&self) -> usize {
        self.scratch.iter().flatten().count()
    }
    pub(crate) fn append_native_events(
        &self,
        frame: u64,
        events: &mut Vec<NativeInstrumentEvent>,
    ) -> Result<(), Invalid> {
        if frame != self.block_frame {
            return Ok(());
        }
        for event in self.native_events[..self.event_len].iter().flatten() {
            if events.len() == events.capacity() {
                return Err(Invalid("typing native event budget exhausted"));
            }
            events.push(*event);
        }
        for voice in self.scratch.iter().flatten() {
            if let (Some(id), NoteGate::Released { at_age_frames }) = (voice.native_id, voice.gate)
                && voice.onset.checked_add(at_age_frames) == Some(frame)
            {
                if events.len() == events.capacity() {
                    return Err(Invalid("typing native release budget exhausted"));
                }
                events.push(NativeInstrumentEvent::NoteOff {
                    frame_offset: 0,
                    occurrence_id: id,
                });
            }
        }
        Ok(())
    }
    pub(crate) fn sample_additive(
        &self,
        frame: u64,
        output: &mut [[f32; 2]],
    ) -> Result<(), Invalid> {
        if self.arm.is_mutable() {
            return Ok(());
        }
        for voice in self.scratch.iter().flatten() {
            for (offset, output) in output.iter_mut().enumerate() {
                let age = frame
                    .checked_add(offset as u64)
                    .and_then(|frame| frame.checked_sub(voice.onset))
                    .ok_or(Invalid("typing sample age overflow"))?;
                let sample = self.arm.sample_continued(
                    usize::from(voice.key),
                    frame + offset as u64,
                    age,
                    voice.gate,
                    self.continuation,
                )?;
                output[0] += sample[0];
                output[1] += sample[1];
            }
        }
        Ok(())
    }
    pub(crate) fn commit_block(&mut self) {
        self.primary = self.scratch;
        self.serial_primary = self.serial_scratch;
        // Unobserved applications cannot be silently overwritten by a later
        // callback. Queue admission reserved each receipt slot in advance.
        for receipt in self.receipts.iter().flatten() {
            if let Some(slot) = self.applied.iter_mut().find(|slot| slot.is_none()) {
                *slot = Some(*receipt);
            }
        }
        self.pending.fill(None);
        self.pending_len = 0;
        self.transaction = false;
    }
    pub(crate) fn rollback_block(&mut self) {
        self.transaction = false;
        (
            self.inherited_authored,
            self.inherited_until,
            self.inherited_bound_known,
        ) = self.inherited_before;
    }
    pub(crate) fn preflight_inherit(
        &self,
        source: &Self,
        frame: u64,
    ) -> Result<Option<hex_lattice::AnalyticContinuation>, Invalid> {
        if self.owner != source.owner
            || self.epoch != source.epoch
            || self.arm.is_mutable() != source.arm.is_mutable()
            || source.primary.iter().flatten().count() > self.live_limit
            || source.primary.iter().flatten().count()
                + source
                    .pending
                    .iter()
                    .flatten()
                    .filter(|(_, action)| matches!(action, Action::On { .. }))
                    .count()
                > self.live_limit
            || source.transaction
            || source.pending_len + source.applied.iter().flatten().count() > COMMANDS
        {
            return Err(Invalid(
                "typing owner or capacity cannot inherit this source",
            ));
        }
        for voice in source.primary.iter().flatten() {
            if self.arm.allowed_keys() & (1 << voice.key) == 0 {
                return Err(Invalid("typing held pitch is no longer admitted"));
            }
        }
        if source.primary.iter().all(Option::is_none) {
            // No physical sampler state exists to retarget. Queued onsets
            // remain prospective and use the actually published palette.
            return Ok(None);
        }
        self.arm
            .transition_from(&source.arm, source.continuation, frame)
    }
    pub(crate) fn inherit(
        &mut self,
        source: &Self,
        continuation: Option<hex_lattice::AnalyticContinuation>,
    ) {
        self.primary = source.primary;
        self.scratch = source.primary;
        self.pending = source.pending;
        self.pending_len = source.pending_len;
        self.applied = source.applied;
        self.last_ticket = source.last_ticket;
        self.serial_primary = source.serial_primary;
        self.serial_scratch = source.serial_primary;
        self.through_epoch = self.through_epoch.max(source.through_epoch);
        self.armed &= source.armed;
        self.continuation = continuation;
        self.inherited_authored = self.inherited_authored.max(source.inherited_authored);
        self.inherited_until = self.inherited_until.max(source.inherited_until);
        self.inherited_bound_known |= source.inherited_bound_known;
    }
    pub(crate) fn reset(&mut self) {
        self.cut(self.epoch);
        self.primary.fill(None);
        self.scratch.fill(None);
        self.pending.fill(None);
        self.pending_len = 0;
        self.native_events.fill(None);
        self.event_len = 0;
        self.receipts.fill(None);
        self.applied.fill(None);
        self.transaction = false;
        self.inherited_authored = 0;
        self.inherited_until = 0;
        self.inherited_bound_known = true;
    }
    pub fn take_application(&mut self) -> Option<TypingApplication> {
        let index = self.applied.iter().position(Option::is_some)?;
        self.applied[index].take()
    }
}
