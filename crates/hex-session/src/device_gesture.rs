//! One worker-owned typed mixer/native-device overlay. No callback ownership.
use crate::{Session, SessionError};
use hex_engine::{NativeDeviceClass, PreparedRevision};
use hex_project::{DeviceId, Project, ReturnId, TrackId};
use std::sync::Arc;
mod client;
#[cfg(test)]
mod client_tests;
mod eq_creation;
mod machine;
mod targets;
pub use client::GestureClient;
use machine::{Active, ActiveKind};
pub(crate) use machine::{GestureMachine, GestureOutcome, GesturePreparation, PreparedGesture};
use targets::*;
mod plugin_parameter;
mod preparation;
pub use eq_creation::EqNodeCreationSnapshot;
pub use plugin_parameter::PluginGesturePreparation;
pub use plugin_parameter::{PluginParameterOrigin, PluginParameterValue};
#[cfg(test)]
mod group_tests;
#[cfg(test)]
mod manual_tests;
#[cfg(test)]
mod snapshot_tests;
pub use preparation::reconcile_command_for_project;
pub(crate) use preparation::{command_for_snapshot, existing_device_command, manual_device_edit};

pub use hex_lattice_contract::{
    DeviceParameterSnapshot, DeviceTypeId, ParameterId, ParameterValue,
};

/// Tokens are monotonic within a project-open generation; zero is invalid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GestureKey {
    pub generation: u64,
    pub token: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GestureTarget {
    MainGain,
    TrackGain {
        track: TrackId,
    },
    /// One frozen cohort; the anchor supplies the scalar relative gesture value.
    TrackGains {
        track: TrackId,
    },
    TrackBalance {
        track: TrackId,
    },
    ReturnGain {
        return_track: ReturnId,
    },
    DeviceParameter {
        device: DeviceId,
        parameter: ParameterId,
    },
    DeviceParameterPair {
        device: DeviceId,
        first: ParameterId,
        second: ParameterId,
    },
    DeviceParameterSnapshot {
        device: DeviceId,
    },
    EqNodeCreation {
        device: DeviceId,
    },
    /// Raw provider identity, including zero; never a built-in ParameterId.
    PluginParameter {
        device: DeviceId,
        parameter: u32,
    },
    ReturnSend {
        track: TrackId,
        return_track: ReturnId,
    },
}

impl GestureTarget {
    pub fn is_mixer_gain(self) -> bool {
        matches!(
            self,
            Self::TrackGain { .. }
                | Self::TrackGains { .. }
                | Self::MainGain
                | Self::ReturnGain { .. }
        )
    }

    pub fn track_gain_anchor(self) -> Option<TrackId> {
        match self {
            Self::TrackGain { track } | Self::TrackGains { track } => Some(track),
            _ => None,
        }
    }
}

/// Snapshot-edit intent indexed by the gesture's immutable ordered parameter domain.
/// The client and worker require `same_parameters` before interpreting a mask;
/// validation-only entries stay clear, and the union survives coalesced move-back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TouchedParameters {
    bits: u64,
}
const _: () = assert!(hex_lattice_contract::MAX_PARAMETER_SNAPSHOT_ENTRIES <= u64::BITS as usize);
impl TouchedParameters {
    pub fn is_empty(self) -> bool {
        self.bits == 0
    }
    pub fn len(self) -> usize {
        self.bits.count_ones() as usize
    }
    /// Resolve only against the same ordered domain validated for this gesture.
    pub fn ids(
        self,
        domain: &DeviceParameterSnapshot,
    ) -> Result<impl Iterator<Item = ParameterId> + '_, &'static str> {
        self.validate_domain(domain)?;
        Ok(domain
            .entries()
            .iter()
            .enumerate()
            .filter_map(move |(index, &(id, _))| (self.bits & (1_u64 << index) != 0).then_some(id)))
    }
    fn validate_domain(self, domain: &DeviceParameterSnapshot) -> Result<(), &'static str> {
        let count = domain.entries().len();
        if count < u64::BITS as usize && self.bits >> count != 0 {
            return Err("snapshot touched parameter changed");
        }
        Ok(())
    }
    fn include_changed(
        &mut self,
        origin: DeviceParameterSnapshot,
        value: DeviceParameterSnapshot,
    ) -> Result<(), &'static str> {
        if !origin.same_parameters(&value) {
            return Err("snapshot gesture parameters changed");
        }
        for (index, (&(_, old), &(_, next))) in
            origin.entries().iter().zip(value.entries()).enumerate()
        {
            if old != next {
                self.bits |= 1_u64 << index;
            }
        }
        Ok(())
    }
    fn merge(&mut self, other: Self, domain: DeviceParameterSnapshot) -> Result<(), &'static str> {
        other.validate_domain(&domain)?;
        self.bits |= other.bits;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParameterPairValue {
    pub first: ParameterValue,
    pub second: ParameterValue,
}

// The largest payload is a bounded 256-track control Begin (3,088 bytes).
// Copy retains the existing coalescer/retry/terminal ownership without another
// heap owner. The 32-slot worker FIFO bounds storage; callbacks never see it.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GestureRequest {
    BeginTrackGains {
        key: GestureKey,
        group: hex_project::RelativeTrackGains,
    },
    BeginPlugin {
        key: GestureKey,
        origin: PluginParameterOrigin,
        recording: bool,
    },
    ChangePlugin {
        key: GestureKey,
        sequence: u64,
        value: PluginParameterValue,
    },
    CommitPlugin {
        key: GestureKey,
        sequence: u64,
        value: PluginParameterValue,
    },
    Begin {
        key: GestureKey,
        target: GestureTarget,
    },
    Change {
        key: GestureKey,
        sequence: u64,
        value: ParameterValue,
    },
    ChangePair {
        key: GestureKey,
        sequence: u64,
        value: ParameterPairValue,
    },
    Commit {
        key: GestureKey,
        sequence: u64,
        /// An explicit update occurred, even if its value equals the base.
        manual_edit: bool,
        value: ParameterValue,
    },
    CommitPair {
        key: GestureKey,
        sequence: u64,
        /// An explicit update occurred, even if its value equals the base.
        manual_edit: bool,
        value: ParameterPairValue,
    },
    BeginSnapshot {
        key: GestureKey,
        device: DeviceId,
        snapshot: DeviceParameterSnapshot,
    },
    ChangeSnapshot {
        touched_parameters: TouchedParameters,
        key: GestureKey,
        sequence: u64,
        snapshot: DeviceParameterSnapshot,
    },
    CommitSnapshot {
        touched_parameters: TouchedParameters,
        key: GestureKey,
        sequence: u64,
        snapshot: DeviceParameterSnapshot,
    },
    BeginEqCreation {
        key: GestureKey,
        device: DeviceId,
        snapshot: EqNodeCreationSnapshot,
    },
    ChangeEqCreation {
        key: GestureKey,
        sequence: u64,
        snapshot: EqNodeCreationSnapshot,
    },
    CommitEqCreation {
        key: GestureKey,
        sequence: u64,
        snapshot: EqNodeCreationSnapshot,
    },
    Cancel {
        key: GestureKey,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GesturePhase {
    Begin,
    Change,
    Commit,
    Cancel,
}

#[derive(Clone, Debug)]
pub struct GestureAck {
    /// A refused native terminal kept the worker's last accepted overlay audible. Reconcile
    /// with the exact active snapshot from the same Event; never infer restoration or retry.
    pub preview_retained: bool,
    /// Retire exact plugin display/input custody without claiming restoration.
    /// Never combine with accepted or preview_retained.
    pub source_lost: bool,
    pub key: GestureKey,
    pub phase: GesturePhase,
    pub sequence: u64,
    pub accepted: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GestureSnapshot {
    pub touched_parameters: TouchedParameters,
    /// Accepted/explicit update intent; Begin alone never selects manual read.
    pub manual_edit: bool,
    pub key: GestureKey,
    pub target: GestureTarget,
    pub sequence: u64,
    pub value: ParameterValue,
    pub secondary_value: Option<ParameterValue>,
    pub parameters: Option<DeviceParameterSnapshot>,
    pub creation: Option<EqNodeCreationSnapshot>,
    /// Frozen starting values only; current targets derive from `value`.
    /// Control metadata, never a callback request or saved project field.
    pub track_gains: Option<hex_project::RelativeTrackGains>,
    /// Exact normalized plugin intent. The built-in scalar is unused for this
    /// target, as it is for compound snapshots; never round this through f32.
    pub plugin_value: Option<PluginParameterValue>,
}
