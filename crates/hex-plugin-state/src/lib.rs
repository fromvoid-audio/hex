//! # Purpose
//! Own the control-side VST3 provider codec inside the existing generic device
//! envelope. A project `DeviceId` is the instance identity; neither class identity,
//! browser rows, paths nor parameter indices identify an instance. Cloning state
//! does not mint or reuse a project identity.
//!
//! # Dependencies
//! Uses only std and hex-lattice-contract's bounded immutable NativeDeviceState.
//! Storage integration tests consume the unchanged generic project envelope.
//!
//! # Does not own
//! No SDK ABI, foreign objects, discovery, filesystem, DSP, callback work, state
//! capture/restore or graph admission. Copied binary identities are byte identities,
//! not authenticity or permission to load code. A host must independently resolve
//! and validate the current module/class before using these bytes.
//!
//! Version1 uses `hex.vst3`, the generic state version and a 96-byte fixed header:
//! HXVS, platform byte, presence bits, two reserved zero bytes, SDK class UID16,
//! executable SHA25632, Info.plist SHA25632, two little-endian u32 chunk lengths,
//! then component and controller bytes. UIDs are exact scanner SDK bytes; no GUID
//! byte swapping or case folding occurs. Initially only macOS arm64 is supported.
//! Paths are deliberately absent: moving identical bytes cannot change identity.
//!
//! The live codec admits a 512 KiB envelope with a combined component and
//! controller limit. This is a bounded profile for opaque vendor state. Oversize capture refuses before allocation/copy without consuming
//! caller buffers. Empty captured chunks differ from missing chunks. Unknown or
//! malformed envelopes remain generic project state and must be retained intact;
//! decoding/availability errors never reconstruct, normalize or overwrite them.
use hex_lattice_contract::{DeviceTypeId, MAX_VST3_DEVICE_STATE_BYTES, NativeDeviceState};
use std::fmt;

pub const TYPE_ID: DeviceTypeId = hex_lattice_contract::VST3_DEVICE_TYPE_ID;
pub const STATE_VERSION: u32 = 1;
const HEADER_BYTES: usize = 96;
/// Maximum complete envelope the live native worker protocol admits.
pub const MAX_ENVELOPE_BYTES: usize = MAX_VST3_DEVICE_STATE_BYTES;
pub const MAX_OPAQUE_BYTES: usize = MAX_ENVELOPE_BYTES - HEADER_BYTES;

/// SDK factory class identity, independent of module path and project instance ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClassId([u8; 16]);
impl ClassId {
    pub fn new(bytes: [u8; 16]) -> Result<Self, Error> {
        if bytes == [0; 16] {
            return Err(Error::InvalidClass);
        }
        Ok(Self(bytes))
    }
    pub const fn bytes(self) -> [u8; 16] {
        self.0
    }
}

/// Exact copied discovery identities. These do not resolve a file or authorize it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BinaryIdentity {
    pub executable_sha256: [u8; 32],
    pub info_sha256: [u8; 32],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub class: ClassId,
    pub binary: BinaryIdentity,
}

/// Borrowed control-side projection; opaque bytes are never interpreted as DSP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State<'a> {
    pub identity: Identity,
    pub component: Option<&'a [u8]>,
    pub controller: Option<&'a [u8]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    WrongType,
    UnsupportedVersion(u32),
    UnsupportedPlatform(u8),
    InvalidClass,
    Malformed,
    TooLarge,
    MissingComponent,
    MissingBinary,
    ClassChanged,
    BinaryChanged,
}
impl fmt::Display for Error {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(match self {
            Self::WrongType => "device is not a vst3 state envelope",
            Self::UnsupportedVersion(_) => "vst3 state version is unsupported",
            Self::UnsupportedPlatform(_) => "vst3 state platform is unsupported",
            Self::InvalidClass => "vst3 class identity is invalid",
            Self::Malformed => "vst3 state envelope is malformed",
            Self::TooLarge => "vst3 state exceeds the bounded capture limit",
            Self::MissingComponent => "vst3 component state is missing",
            Self::MissingBinary => "vst3 binary or class is unavailable",
            Self::ClassChanged => "vst3 class identity changed",
            Self::BinaryChanged => "vst3 binary identity changed",
        })
    }
}
impl std::error::Error for Error {}

/// Capture copied chunks on control. No caller-owned buffer is consumed on refusal.
/// `None` retains missing state; `Some(&[])` is an actual empty captured stream.
/// The caller publishes this envelope only through its ordinary project edit.
pub fn encode(state: State<'_>) -> Result<NativeDeviceState, Error> {
    let component = state.component.unwrap_or(&[]);
    let controller = state.controller.unwrap_or(&[]);
    let count = component
        .len()
        .checked_add(controller.len())
        .ok_or(Error::TooLarge)?;
    if count > MAX_OPAQUE_BYTES {
        return Err(Error::TooLarge);
    }
    let mut payload = Vec::with_capacity(HEADER_BYTES + count);
    payload.extend_from_slice(b"HXVS");
    payload.push(1); // macOS arm64; no inferred host-platform conversion
    payload.push(u8::from(state.component.is_some()) | (u8::from(state.controller.is_some()) << 1));
    payload.extend_from_slice(&[0, 0]);
    payload.extend_from_slice(&state.identity.class.bytes());
    payload.extend_from_slice(&state.identity.binary.executable_sha256);
    payload.extend_from_slice(&state.identity.binary.info_sha256);
    // The combined bound fits the two u32 chunk lengths.
    payload.extend_from_slice(&(component.len() as u32).to_le_bytes());
    payload.extend_from_slice(&(controller.len() as u32).to_le_bytes());
    payload.extend_from_slice(component);
    payload.extend_from_slice(controller);
    NativeDeviceState::new(TYPE_ID, STATE_VERSION, payload).map_err(|_| Error::TooLarge)
}

/// Inspect without mutating the retained generic envelope. Unknown versions and
/// platforms remain recoverable project bytes, not defaults or automatic migrations.
pub fn decode(state: &NativeDeviceState) -> Result<State<'_>, Error> {
    if state.type_id() != TYPE_ID {
        return Err(Error::WrongType);
    }
    if state.state_version() != STATE_VERSION {
        return Err(Error::UnsupportedVersion(state.state_version()));
    }
    let bytes = state.payload();
    if bytes.len() > MAX_ENVELOPE_BYTES {
        return Err(Error::TooLarge);
    }
    let header = bytes.get(..HEADER_BYTES).ok_or(Error::Malformed)?;
    if header.get(..4) != Some(b"HXVS") || header[5] & !3 != 0 || header[6..8] != [0, 0] {
        return Err(Error::Malformed);
    }
    if header[4] != 1 {
        return Err(Error::UnsupportedPlatform(header[4]));
    }
    let array = |range: std::ops::Range<usize>| header.get(range).ok_or(Error::Malformed);
    let class = ClassId::new(array(8..24)?.try_into().map_err(|_| Error::Malformed)?)?;
    let identity = Identity {
        class,
        binary: BinaryIdentity {
            executable_sha256: array(24..56)?.try_into().map_err(|_| Error::Malformed)?,
            info_sha256: array(56..88)?.try_into().map_err(|_| Error::Malformed)?,
        },
    };
    let component =
        u32::from_le_bytes(array(88..92)?.try_into().map_err(|_| Error::Malformed)?) as usize;
    let controller =
        u32::from_le_bytes(array(92..96)?.try_into().map_err(|_| Error::Malformed)?) as usize;
    let count = component.checked_add(controller).ok_or(Error::TooLarge)?;
    if count > MAX_OPAQUE_BYTES {
        return Err(Error::TooLarge);
    }
    if bytes.len() != HEADER_BYTES + count
        || (header[5] & 1 == 0 && component != 0)
        || (header[5] & 2 == 0 && controller != 0)
    {
        return Err(Error::Malformed);
    }
    let split = HEADER_BYTES + component;
    Ok(State {
        identity,
        component: (header[5] & 1 != 0).then_some(&bytes[HEADER_BYTES..split]),
        controller: (header[5] & 2 != 0).then_some(&bytes[split..]),
    })
}

/// Compare with independently resolved, currently validated identity on control.
/// Passing a cached browser row is insufficient admission evidence. Success means
/// only these copied identities/chunks match, never that foreign state was applied
/// or that a processor/graph is admitted. Missing controller state is legal for a
/// component without separately captured controller state.
pub fn available<'a>(
    state: &'a NativeDeviceState,
    resolved: Option<Identity>,
) -> Result<State<'a>, Error> {
    let state = decode(state)?;
    if state.component.is_none() {
        return Err(Error::MissingComponent);
    }
    let resolved = resolved.ok_or(Error::MissingBinary)?;
    if resolved.class != state.identity.class {
        return Err(Error::ClassChanged);
    }
    if resolved.binary != state.identity.binary {
        return Err(Error::BinaryChanged);
    }
    Ok(state)
}

#[cfg(test)]
mod tests;
