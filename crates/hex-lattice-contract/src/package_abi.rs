//! Neutral fixed-width C-compatible ABI and wire contract for installable
//! Hex-native device packages (`#2990`).
//!
//! # Purpose
//! Define versioned, fixed-width `#[repr(C)]` ABI records, capability flags,
//! status codes, bounded wire frames, and validation contracts shared between
//! Hex control/host code and detached native device packages without exposing
//! Rust ABI layout, trait objects, allocator ownership, or unwinding across the
//! boundary.
//!
//! # Independent Versioning
//! Every package declares four independent version identifiers:
//! - `package_version` (`major.minor.patch`): distribution and update generation
//!   of the package artifact itself.
//! - `state_version` (`u32`): authored state payload format owned by the device
//!   for deterministic migration and project compatibility.
//! - `abi_version` (`u32`): binary call/wire contract version negotiated with
//!   the host ([`PACKAGE_ABI_VERSION_V1`]).
//! - `ui_version` (`u32`): declarative control/panel schema version consumed by
//!   the UI layer independently of DSP ABI or state format.
//!
//! # Thread Affinity, Allocation Ownership, and No-Unwind Rules
//! 1. **Control-thread operations** (`describe`, `parameter_descriptor`,
//!    `create`, `destroy`, `set_state`, `get_state`, `reset`) are serialized on
//!    a single control/preparation thread per instance. They may allocate only
//!    through the module's own allocator and must never return pointers that the
//!    host is expected to `free`.
//! 2. **Audio-thread operations** (`process`) run on a single realtime audio
//!    thread per active instance without concurrent control calls on that
//!    instance. `process` must perform **zero** heap allocations, **zero**
//!    frees, **zero** locks/syscalls, and **zero** blocking I/O. Every audio,
//!    parameter-event, and note-event buffer is host-owned, caller-borrowed for
//!    the duration of the call only, and bounded by [`PACKAGE_ABI_MAX_FRAMES`],
//!    [`PACKAGE_ABI_MAX_PARAM_EVENTS`], and [`PACKAGE_ABI_MAX_NOTE_EVENTS`].
//! 3. **No unwind crossing**: all exported C ABI entry points are `extern "C"`
//!    (never `C-unwind`). Any panic or C++ exception inside a module is a hard
//!    ABI violation; modules must catch/avoid unwinds and return a non-zero
//!    [`AbiStatus`] code instead.
//! 4. **Output validation**: the host validates every rendered stereo frame for
//!    finiteness after `process`; any NaN/Inf sample is rejected as
//!    [`AbiStatus::InvalidOutput`] and replaced with silence.
//!
//! # Execution Boundary Architecture Decision (`#2990`)
//! Two candidate execution boundaries were qualified on macOS arm64 against a
//! same-source static Rust baseline using detached C99 native modules
//! (`hex.pkg.utility` and `hex.pkg.sine`, compiled independently via Apple
//! `clang` with zero shared Rust ABI):
//!
//! - **Pre-run acceptance thresholds** (derived from Hex's existing host timing
//!   and callback budgets):
//!   - Declared added latency: **0 frames** (zero deliberate added wait/latency;
//!     never weakened to make IPC pass).
//!   - Audio callback allocations/frees: **0 total count, 0 total bytes, 0 net**.
//!   - Worst-case 96 kHz / 32-frame quantum (`333.3 µs`): 100-instance parallel
//!     batch and 8-device sequential chain must remain well below the quantum
//!     budget and within `<= 2.0x` of the same-source static baseline.
//!   - Failure handling: malformed ABI, NaN/Inf output, crashes, hangs, and
//!     teardown stalls must be detected and handled deterministically without
//!     corrupting host state.
//!
//! - **Selected two-tier boundary decision** (measured on `arm64-apple-darwin`
//!   release build, retained raw measurements in
//!   `research/observations/installed-devices-abi-qualification-2026-11-26.json`):
//!   1. **Active realtime audio execution (trusted/admitted factory & verified
//!      native packages)**: narrowly audited external C ABI loader crate
//!      (`third_party/hex-device-abi-*`, excluded from workspace membership so
//!      workspace `unsafe_code = "forbid"` stays intact, pinned `libc = "=0.2.190"`,
//!      `unsafe_op_in_unsafe_fn = "deny"`). Achieves **0 frames added latency**,
//!      **0 callback allocations/frees** (total and signed-net), and near-static
//!      throughput across 44.1/48/96 kHz and 32/64/128/512 frames: at 96 kHz /
//!      32 frames (`333.3 µs` quantum), 100 Utility instances took `4.05 µs` vs
//!      `3.36 µs` static (`1.20x`, `1.2%` of quantum), and an 8-device sequential
//!      chain (Sine + 7 Utilities) took `0.55 µs` vs `0.39 µs` static.
//!   2. **Untrusted discovery, validation, probing, and optional isolated
//!      execution**: contained-runner subprocess over fixed-width wire framing
//!      ([`AbiWireRequestHeader`] / [`AbiWireReplyHeader`]). Isolates crashes,
//!      hangs, and teardown stalls during package inspection/admission, while
//!      empirically demonstrating that per-block subprocess IPC cannot satisfy
//!      zero-latency 100-instance realtime budgets (`866.0 µs` = `260%` of the
//!      `333.3 µs` quantum at 96 kHz / 32 frames; `1,085.5 µs` = `150%` of the
//!      `725.6 µs` quantum at 44.1 kHz / 32 frames; `68.1 µs` for the 8-device
//!      chain = `124x` in-process C ABI).
//!
//! - **Unresolved platform limits explicitly recorded**:
//!   - Neither code-signature verification nor subprocess separation alone
//!     constitutes OS sandboxing (filesystem/network/entitlement confinement
//!     requires explicit OS sandbox policy and remains a separate trust gate).
//!   - An in-process native module that faults (`SIGSEGV`/`SIGABRT`) or hangs
//!     during active realtime processing can still fault or stall the host audio
//!     process; therefore arbitrary untrusted production loading is NOT enabled
//!     by this qualification contract and requires explicit trust/admission
//!     policy in follow-up milestones (`#2991`–`#3005`).

use crate::{DeviceContractError, DeviceTypeId, MAX_DEVICE_TYPE_ID_BYTES};

/// ASCII `"HEXD"` magic tag validating every C ABI descriptor and wire header.
pub const PACKAGE_ABI_MAGIC: u32 = 0x4845_5844;
/// Current negotiated C ABI version.
pub const PACKAGE_ABI_VERSION_V1: u32 = 1;
/// Hard per-block frame bound matching Hex's bounded audio processing blocks.
pub const PACKAGE_ABI_MAX_FRAMES: u32 = 512;
/// Maximum parameters declared by a v1 native device package.
pub const PACKAGE_ABI_MAX_PARAMS: u32 = 64;
/// Maximum sample-offset parameter events per process call.
pub const PACKAGE_ABI_MAX_PARAM_EVENTS: u32 = 64;
/// Maximum sample-offset MIDI/note events per process call.
pub const PACKAGE_ABI_MAX_NOTE_EVENTS: u32 = 64;
/// Maximum UTF-8 bytes in a parameter display label.
pub const PACKAGE_ABI_MAX_PARAM_NAME_BYTES: usize = 64;
/// Maximum opaque state bytes per package instance in v1.
pub const PACKAGE_ABI_MAX_STATE_BYTES: u32 = 4_096;
/// Maximum wire payload bytes for contained-runner IPC messages.
pub const PACKAGE_ABI_MAX_WIRE_PAYLOAD_BYTES: u32 = 65_536;

/// Device category carried across the C ABI.
pub const DEVICE_KIND_AUDIO_EFFECT: u32 = 1;
pub const DEVICE_KIND_INSTRUMENT: u32 = 2;

/// Capability flags negotiated in [`AbiDescriptor::capabilities`].
pub const CAP_AUDIO_EFFECT: u64 = 1 << 0;
pub const CAP_INSTRUMENT: u64 = 1 << 1;
pub const CAP_PARAMETER_EVENTS: u64 = 1 << 2;
pub const CAP_NOTE_EVENTS: u64 = 1 << 3;
pub const CAP_ZERO_LATENCY: u64 = 1 << 4;
pub const CAP_STATE_PERSISTENCE: u64 = 1 << 5;
pub const CAP_NOTE_EXPRESSION: u64 = 1 << 6;
pub const CAP_CHANNEL_EXPRESSION: u64 = 1 << 7;
/// Bitmask of all capability bits defined in ABI v1.
pub const CAP_KNOWN_MASK_V1: u64 = CAP_AUDIO_EFFECT
    | CAP_INSTRUMENT
    | CAP_PARAMETER_EVENTS
    | CAP_NOTE_EVENTS
    | CAP_ZERO_LATENCY
    | CAP_STATE_PERSISTENCE
    | CAP_NOTE_EXPRESSION
    | CAP_CHANNEL_EXPRESSION;

/// Note & expressive event kinds in [`AbiNoteEvent::kind`].
pub const NOTE_EVENT_ON: u32 = 1;
pub const NOTE_EVENT_OFF: u32 = 2;
pub const NOTE_EVENT_PITCH_BEND: u32 = 3;
pub const NOTE_EVENT_PRESSURE: u32 = 4;
pub const NOTE_EVENT_CC: u32 = 5;
pub const NOTE_EVENT_ALL_NOTES_OFF: u32 = 6;

/// Wire command opcodes for contained-runner IPC transport.
pub const WIRE_OP_DESCRIBE: u32 = 1;
pub const WIRE_OP_CREATE: u32 = 2;
pub const WIRE_OP_PROCESS: u32 = 3;
pub const WIRE_OP_SET_STATE: u32 = 4;
pub const WIRE_OP_GET_STATE: u32 = 5;
pub const WIRE_OP_DESTROY: u32 = 6;
pub const WIRE_OP_SHUTDOWN: u32 = 7;

/// Independent four-axis version tuple carried by every native device package.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ManifestVersions {
    pub package_major: u16,
    pub package_minor: u16,
    pub package_patch: u16,
    pub state_version: u32,
    pub abi_version: u32,
    pub ui_version: u32,
}

/// Fixed-width C ABI status codes returned across the boundary.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbiStatus {
    Ok = 0,
    InvalidArg = 1,
    UnsupportedAbi = 2,
    SizeMismatch = 3,
    InvalidRate = 4,
    InvalidFrames = 5,
    InvalidState = 6,
    InvalidOutput = 7,
    AllocationFailed = 8,
    InternalFault = 9,
}

impl AbiStatus {
    pub const fn from_i32(raw: i32) -> Self {
        match raw {
            0 => Self::Ok,
            1 => Self::InvalidArg,
            2 => Self::UnsupportedAbi,
            3 => Self::SizeMismatch,
            4 => Self::InvalidRate,
            5 => Self::InvalidFrames,
            6 => Self::InvalidState,
            7 => Self::InvalidOutput,
            8 => Self::AllocationFailed,
            _ => Self::InternalFault,
        }
    }
}

/// Fixed-width C ABI device descriptor returned by `describe`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AbiDescriptor {
    pub struct_size: u32,
    pub magic: u32,
    pub abi_version: u32,
    pub device_kind: u32,
    pub state_version: u32,
    pub ui_version: u32,
    pub package_major: u16,
    pub package_minor: u16,
    pub package_patch: u16,
    pub reserved0: u16,
    pub capabilities: u64,
    pub reported_latency_frames: u32,
    pub tail_frames: u32,
    pub parameter_count: u32,
    pub max_state_bytes: u32,
    pub type_id_len: u32,
    pub reserved1: u32,
    pub type_id: [u8; MAX_DEVICE_TYPE_ID_BYTES],
}

impl AbiDescriptor {
    /// Validate magic, struct size, ABI version, independent version fields,
    /// capability negotiation bits, bounds, and canonical `DeviceTypeId`.
    pub fn validate(&self) -> Result<(DeviceTypeId, ManifestVersions), AbiStatus> {
        if self.magic != PACKAGE_ABI_MAGIC {
            return Err(AbiStatus::InvalidArg);
        }
        if self.struct_size as usize != std::mem::size_of::<Self>() {
            return Err(AbiStatus::SizeMismatch);
        }
        if self.abi_version != PACKAGE_ABI_VERSION_V1 {
            return Err(AbiStatus::UnsupportedAbi);
        }
        if self.reserved0 != 0 || self.reserved1 != 0 {
            return Err(AbiStatus::InvalidArg);
        }
        if self.state_version == 0 || self.ui_version == 0 {
            return Err(AbiStatus::InvalidArg);
        }
        if (self.capabilities & !CAP_KNOWN_MASK_V1) != 0 {
            return Err(AbiStatus::UnsupportedAbi);
        }
        match self.device_kind {
            DEVICE_KIND_AUDIO_EFFECT if (self.capabilities & CAP_AUDIO_EFFECT) != 0 => {}
            DEVICE_KIND_INSTRUMENT if (self.capabilities & CAP_INSTRUMENT) != 0 => {}
            _ => return Err(AbiStatus::InvalidArg),
        }
        if (self.capabilities & CAP_ZERO_LATENCY) != 0 && self.reported_latency_frames != 0 {
            return Err(AbiStatus::InvalidArg);
        }
        if self.parameter_count > PACKAGE_ABI_MAX_PARAMS
            || self.max_state_bytes > PACKAGE_ABI_MAX_STATE_BYTES
        {
            return Err(AbiStatus::InvalidArg);
        }
        let len = self.type_id_len as usize;
        if len == 0 || len > MAX_DEVICE_TYPE_ID_BYTES {
            return Err(AbiStatus::InvalidArg);
        }
        if self.type_id[len..].iter().any(|&b| b != 0) {
            return Err(AbiStatus::InvalidArg);
        }
        let text = std::str::from_utf8(&self.type_id[..len]).map_err(|_| AbiStatus::InvalidArg)?;
        let id = DeviceTypeId::new(text).map_err(|_: DeviceContractError| AbiStatus::InvalidArg)?;
        Ok((
            id,
            ManifestVersions {
                package_major: self.package_major,
                package_minor: self.package_minor,
                package_patch: self.package_patch,
                state_version: self.state_version,
                abi_version: self.abi_version,
                ui_version: self.ui_version,
            },
        ))
    }
}

/// Fixed-width C ABI parameter metadata record.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AbiParameterDescriptor {
    pub struct_size: u32,
    pub id: u32,
    pub min: f32,
    pub max: f32,
    pub default_value: f32,
    pub flags: u32,
    pub name_len: u32,
    pub reserved: u32,
    pub name: [u8; PACKAGE_ABI_MAX_PARAM_NAME_BYTES],
}

impl AbiParameterDescriptor {
    pub fn validate(&self) -> Result<&str, AbiStatus> {
        if self.struct_size as usize != std::mem::size_of::<Self>() {
            return Err(AbiStatus::SizeMismatch);
        }
        if self.id == 0 || self.reserved != 0 {
            return Err(AbiStatus::InvalidArg);
        }
        if !self.min.is_finite()
            || !self.max.is_finite()
            || !self.default_value.is_finite()
            || self.min > self.max
            || self.default_value < self.min
            || self.default_value > self.max
        {
            return Err(AbiStatus::InvalidArg);
        }
        let len = self.name_len as usize;
        if len == 0 || len > PACKAGE_ABI_MAX_PARAM_NAME_BYTES {
            return Err(AbiStatus::InvalidArg);
        }
        if self.name[len..].iter().any(|&b| b != 0) {
            return Err(AbiStatus::InvalidArg);
        }
        std::str::from_utf8(&self.name[..len]).map_err(|_| AbiStatus::InvalidArg)
    }
}

/// Sample-offset parameter event delivered to `process`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AbiParamEvent {
    pub sample_offset: u32,
    pub parameter_id: u32,
    pub value: f32,
    pub reserved: u32,
}

/// Sample-offset note event delivered to instrument `process`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AbiNoteEvent {
    pub sample_offset: u32,
    pub kind: u32,
    pub note: u8,
    pub channel: u8,
    pub reserved0: u16,
    pub velocity: f32,
    pub detune_cents: f32,
    pub reserved1: u32,
}

/// Per-block C ABI process context header.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AbiProcessContext {
    pub struct_size: u32,
    pub abi_version: u32,
    pub sample_rate: u32,
    pub frames: u32,
    pub param_event_count: u32,
    pub note_event_count: u32,
    pub flags: u32,
    pub reserved: u32,
}

impl AbiProcessContext {
    pub const fn new(
        sample_rate: u32,
        frames: u32,
        param_event_count: u32,
        note_event_count: u32,
    ) -> Self {
        Self {
            struct_size: std::mem::size_of::<Self>() as u32,
            abi_version: PACKAGE_ABI_VERSION_V1,
            sample_rate,
            frames,
            param_event_count,
            note_event_count,
            flags: 0,
            reserved: 0,
        }
    }

    pub fn validate(&self) -> Result<(), AbiStatus> {
        if self.struct_size as usize != std::mem::size_of::<Self>() {
            return Err(AbiStatus::SizeMismatch);
        }
        if self.abi_version != PACKAGE_ABI_VERSION_V1 {
            return Err(AbiStatus::UnsupportedAbi);
        }
        if !(8_000..=384_000).contains(&self.sample_rate) {
            return Err(AbiStatus::InvalidRate);
        }
        if self.frames == 0 || self.frames > PACKAGE_ABI_MAX_FRAMES {
            return Err(AbiStatus::InvalidFrames);
        }
        if self.param_event_count > PACKAGE_ABI_MAX_PARAM_EVENTS
            || self.note_event_count > PACKAGE_ABI_MAX_NOTE_EVENTS
            || self.flags != 0
            || self.reserved != 0
        {
            return Err(AbiStatus::InvalidArg);
        }
        Ok(())
    }
}

/// Fixed-width little-endian wire request header for contained-runner IPC.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AbiWireRequestHeader {
    pub magic: u32,
    pub abi_version: u32,
    pub opcode: u32,
    pub instance_id: u32,
    pub payload_bytes: u32,
    pub reserved: u32,
}

impl AbiWireRequestHeader {
    pub const BYTE_LEN: usize = 24;

    pub fn encode(self) -> [u8; Self::BYTE_LEN] {
        let mut out = [0u8; Self::BYTE_LEN];
        out[0..4].copy_from_slice(&self.magic.to_le_bytes());
        out[4..8].copy_from_slice(&self.abi_version.to_le_bytes());
        out[8..12].copy_from_slice(&self.opcode.to_le_bytes());
        out[12..16].copy_from_slice(&self.instance_id.to_le_bytes());
        out[16..20].copy_from_slice(&self.payload_bytes.to_le_bytes());
        out[20..24].copy_from_slice(&self.reserved.to_le_bytes());
        out
    }

    pub fn decode(bytes: &[u8; Self::BYTE_LEN]) -> Result<Self, AbiStatus> {
        let header = Self {
            magic: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            abi_version: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            opcode: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            instance_id: u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
            payload_bytes: u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
            reserved: u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
        };
        if header.magic != PACKAGE_ABI_MAGIC {
            return Err(AbiStatus::InvalidArg);
        }
        if header.abi_version != PACKAGE_ABI_VERSION_V1 {
            return Err(AbiStatus::UnsupportedAbi);
        }
        if header.reserved != 0 || header.payload_bytes > PACKAGE_ABI_MAX_WIRE_PAYLOAD_BYTES {
            return Err(AbiStatus::InvalidArg);
        }
        Ok(header)
    }
}

/// Fixed-width little-endian wire reply header for contained-runner IPC.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AbiWireReplyHeader {
    pub magic: u32,
    pub abi_version: u32,
    pub status: i32,
    pub instance_id: u32,
    pub payload_bytes: u32,
    pub reserved: u32,
}

impl AbiWireReplyHeader {
    pub const BYTE_LEN: usize = 24;

    pub fn encode(self) -> [u8; Self::BYTE_LEN] {
        let mut out = [0u8; Self::BYTE_LEN];
        out[0..4].copy_from_slice(&self.magic.to_le_bytes());
        out[4..8].copy_from_slice(&self.abi_version.to_le_bytes());
        out[8..12].copy_from_slice(&self.status.to_le_bytes());
        out[12..16].copy_from_slice(&self.instance_id.to_le_bytes());
        out[16..20].copy_from_slice(&self.payload_bytes.to_le_bytes());
        out[20..24].copy_from_slice(&self.reserved.to_le_bytes());
        out
    }

    pub fn decode(bytes: &[u8; Self::BYTE_LEN]) -> Result<Self, AbiStatus> {
        let header = Self {
            magic: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            abi_version: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            status: i32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            instance_id: u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
            payload_bytes: u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
            reserved: u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
        };
        if header.magic != PACKAGE_ABI_MAGIC {
            return Err(AbiStatus::InvalidArg);
        }
        if header.abi_version != PACKAGE_ABI_VERSION_V1 {
            return Err(AbiStatus::UnsupportedAbi);
        }
        if header.reserved != 0 || header.payload_bytes > PACKAGE_ABI_MAX_WIRE_PAYLOAD_BYTES {
            return Err(AbiStatus::InvalidArg);
        }
        Ok(header)
    }
}

/// Validate that every stereo sample produced by a native package is finite.
/// On failure the caller must silence output rather than propagating NaN/Inf.
#[inline]
pub fn validate_pcm_output(frames: &[[f32; 2]]) -> Result<(), AbiStatus> {
    for &[l, r] in frames {
        if !l.is_finite() || !r.is_finite() {
            return Err(AbiStatus::InvalidOutput);
        }
    }
    Ok(())
}

/// Magic 4-byte prefix for canonical package-state envelopes stored inside
/// `NativeDeviceState::payload()`.
pub const PACKAGE_STATE_ENVELOPE_MAGIC: [u8; 4] = *b"HXPS";
/// Envelope format version.
pub const PACKAGE_STATE_ENVELOPE_VERSION: u8 = 1;

/// Exact immutable package identity pinned alongside opaque device state
/// bytes in project/preset/history payloads (`#2992`, `#2993`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageIdentityEnvelope {
    pub publisher_id: String,
    pub type_id: String,
    pub package_major: u16,
    pub package_minor: u16,
    pub package_patch: u16,
    pub state_version: u32,
    pub abi_version: u32,
    pub ui_version: u32,
    pub package_sha256: [u8; 32],
}

impl PackageIdentityEnvelope {
    /// Encode this package identity together with raw device-owned state bytes
    /// into a bounded deterministic payload suitable for `NativeDeviceState`.
    pub fn encode_with_raw_state(&self, raw_state: &[u8]) -> Result<Vec<u8>, DeviceContractError> {
        if self.publisher_id.is_empty()
            || self.publisher_id.len() > 64
            || self.type_id.is_empty()
            || self.type_id.len() > MAX_DEVICE_TYPE_ID_BYTES
        {
            return Err(DeviceContractError::MalformedState);
        }
        DeviceTypeId::new(&self.type_id)?;
        if raw_state.len() > PACKAGE_ABI_MAX_STATE_BYTES as usize {
            return Err(DeviceContractError::StateTooLarge);
        }
        let total = 4
            + 1
            + 2
            + 2
            + 2
            + 4
            + 4
            + 4
            + 32
            + 1
            + self.publisher_id.len()
            + 1
            + self.type_id.len()
            + 4
            + raw_state.len();
        if total > crate::MAX_NATIVE_DEVICE_STATE_BYTES {
            return Err(DeviceContractError::StateTooLarge);
        }
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(&PACKAGE_STATE_ENVELOPE_MAGIC);
        out.push(PACKAGE_STATE_ENVELOPE_VERSION);
        out.extend_from_slice(&self.package_major.to_le_bytes());
        out.extend_from_slice(&self.package_minor.to_le_bytes());
        out.extend_from_slice(&self.package_patch.to_le_bytes());
        out.extend_from_slice(&self.state_version.to_le_bytes());
        out.extend_from_slice(&self.abi_version.to_le_bytes());
        out.extend_from_slice(&self.ui_version.to_le_bytes());
        out.extend_from_slice(&self.package_sha256);
        out.push(self.publisher_id.len() as u8);
        out.extend_from_slice(self.publisher_id.as_bytes());
        out.push(self.type_id.len() as u8);
        out.extend_from_slice(self.type_id.as_bytes());
        out.extend_from_slice(&(raw_state.len() as u32).to_le_bytes());
        out.extend_from_slice(raw_state);
        Ok(out)
    }

    /// Decode a canonical package state envelope, returning the pinned package
    /// identity and borrowed raw device state slice without executing module code.
    pub fn decode_with_raw_state(bytes: &[u8]) -> Result<(Self, &[u8]), DeviceContractError> {
        if bytes.len() < 4 + 1 + 6 + 12 + 32 + 2 + 4 {
            return Err(DeviceContractError::MalformedState);
        }
        if bytes[0..4] != PACKAGE_STATE_ENVELOPE_MAGIC {
            return Err(DeviceContractError::MalformedState);
        }
        if bytes[4] != PACKAGE_STATE_ENVELOPE_VERSION {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        let package_major = u16::from_le_bytes([bytes[5], bytes[6]]);
        let package_minor = u16::from_le_bytes([bytes[7], bytes[8]]);
        let package_patch = u16::from_le_bytes([bytes[9], bytes[10]]);
        let state_version = u32::from_le_bytes([bytes[11], bytes[12], bytes[13], bytes[14]]);
        let abi_version = u32::from_le_bytes([bytes[15], bytes[16], bytes[17], bytes[18]]);
        let ui_version = u32::from_le_bytes([bytes[19], bytes[20], bytes[21], bytes[22]]);
        let mut package_sha256 = [0u8; 32];
        package_sha256.copy_from_slice(&bytes[23..55]);
        let mut cursor = 55usize;

        let pub_len = usize::from(
            *bytes
                .get(cursor)
                .ok_or(DeviceContractError::MalformedState)?,
        );
        cursor += 1;
        if pub_len == 0 || pub_len > 64 || cursor + pub_len > bytes.len() {
            return Err(DeviceContractError::MalformedState);
        }
        let publisher_id = std::str::from_utf8(&bytes[cursor..cursor + pub_len])
            .map_err(|_| DeviceContractError::MalformedState)?
            .to_owned();
        cursor += pub_len;

        let type_len = usize::from(
            *bytes
                .get(cursor)
                .ok_or(DeviceContractError::MalformedState)?,
        );
        cursor += 1;
        if type_len == 0 || type_len > MAX_DEVICE_TYPE_ID_BYTES || cursor + type_len > bytes.len() {
            return Err(DeviceContractError::MalformedState);
        }
        let type_id = std::str::from_utf8(&bytes[cursor..cursor + type_len])
            .map_err(|_| DeviceContractError::MalformedState)?
            .to_owned();
        DeviceTypeId::new(&type_id)?;
        cursor += type_len;

        if cursor + 4 > bytes.len() {
            return Err(DeviceContractError::MalformedState);
        }
        let raw_len = u32::from_le_bytes([
            bytes[cursor],
            bytes[cursor + 1],
            bytes[cursor + 2],
            bytes[cursor + 3],
        ]) as usize;
        cursor += 4;
        if cursor + raw_len != bytes.len() {
            return Err(DeviceContractError::MalformedState);
        }
        Ok((
            Self {
                publisher_id,
                type_id,
                package_major,
                package_minor,
                package_patch,
                state_version,
                abi_version,
                ui_version,
                package_sha256,
            },
            &bytes[cursor..],
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_descriptor() -> AbiDescriptor {
        let mut type_id = [0u8; MAX_DEVICE_TYPE_ID_BYTES];
        let name = b"hex.pkg.utility";
        type_id[..name.len()].copy_from_slice(name);
        AbiDescriptor {
            struct_size: std::mem::size_of::<AbiDescriptor>() as u32,
            magic: PACKAGE_ABI_MAGIC,
            abi_version: PACKAGE_ABI_VERSION_V1,
            device_kind: DEVICE_KIND_AUDIO_EFFECT,
            state_version: 2,
            ui_version: 3,
            package_major: 1,
            package_minor: 4,
            package_patch: 7,
            reserved0: 0,
            capabilities: CAP_AUDIO_EFFECT | CAP_PARAMETER_EVENTS | CAP_ZERO_LATENCY,
            reported_latency_frames: 0,
            tail_frames: 0,
            parameter_count: 5,
            max_state_bytes: 256,
            type_id_len: name.len() as u32,
            reserved1: 0,
            type_id,
        }
    }

    #[test]
    fn fixed_width_c_abi_struct_sizes_are_explicit_and_stable() {
        assert_eq!(std::mem::size_of::<AbiDescriptor>(), 192);
        assert_eq!(std::mem::size_of::<AbiParameterDescriptor>(), 96);
        assert_eq!(std::mem::size_of::<AbiParamEvent>(), 16);
        assert_eq!(std::mem::size_of::<AbiNoteEvent>(), 24);
        assert_eq!(std::mem::size_of::<AbiProcessContext>(), 32);
        assert_eq!(
            std::mem::size_of::<AbiWireRequestHeader>(),
            AbiWireRequestHeader::BYTE_LEN
        );
        assert_eq!(
            std::mem::size_of::<AbiWireReplyHeader>(),
            AbiWireReplyHeader::BYTE_LEN
        );
    }

    #[test]
    fn descriptor_validation_accepts_independent_versions_and_rejects_malformed_abi() {
        let desc = valid_descriptor();
        let (id, versions) = desc.validate().expect("valid descriptor");
        assert_eq!(id.as_str().unwrap(), "hex.pkg.utility");
        assert_eq!(
            versions,
            ManifestVersions {
                package_major: 1,
                package_minor: 4,
                package_patch: 7,
                state_version: 2,
                abi_version: 1,
                ui_version: 3,
            }
        );

        let mut bad_magic = desc;
        bad_magic.magic = 0xDEAD_BEEF;
        assert_eq!(bad_magic.validate(), Err(AbiStatus::InvalidArg));

        let mut bad_size = desc;
        bad_size.struct_size -= 4;
        assert_eq!(bad_size.validate(), Err(AbiStatus::SizeMismatch));

        let mut future_abi = desc;
        future_abi.abi_version = PACKAGE_ABI_VERSION_V1 + 1;
        assert_eq!(future_abi.validate(), Err(AbiStatus::UnsupportedAbi));

        let mut unknown_cap = desc;
        unknown_cap.capabilities |= 1 << 63;
        assert_eq!(unknown_cap.validate(), Err(AbiStatus::UnsupportedAbi));

        let mut conflicting_latency = desc;
        conflicting_latency.reported_latency_frames = 16;
        assert_eq!(conflicting_latency.validate(), Err(AbiStatus::InvalidArg));

        let mut trailing_garbage = desc;
        trailing_garbage.type_id[desc.type_id_len as usize] = b'x';
        assert_eq!(trailing_garbage.validate(), Err(AbiStatus::InvalidArg));
    }

    #[test]
    fn wire_headers_and_pcm_validation_reject_invalid_frames_and_non_finite_samples() {
        let req = AbiWireRequestHeader {
            magic: PACKAGE_ABI_MAGIC,
            abi_version: PACKAGE_ABI_VERSION_V1,
            opcode: WIRE_OP_PROCESS,
            instance_id: 7,
            payload_bytes: 1024,
            reserved: 0,
        };
        let encoded = req.encode();
        assert_eq!(AbiWireRequestHeader::decode(&encoded), Ok(req));

        let mut oversized = req;
        oversized.payload_bytes = PACKAGE_ABI_MAX_WIRE_PAYLOAD_BYTES + 1;
        assert_eq!(
            AbiWireRequestHeader::decode(&oversized.encode()),
            Err(AbiStatus::InvalidArg)
        );

        assert_eq!(validate_pcm_output(&[[0.25, -0.5], [1.0, 0.0]]), Ok(()));
        assert_eq!(
            validate_pcm_output(&[[0.0, f32::NAN]]),
            Err(AbiStatus::InvalidOutput)
        );
        assert_eq!(
            validate_pcm_output(&[[f32::INFINITY, 0.0]]),
            Err(AbiStatus::InvalidOutput)
        );

        let env = PackageIdentityEnvelope {
            publisher_id: "hex.factory".to_owned(),
            type_id: "hex.pkg.utility".to_owned(),
            package_major: 1,
            package_minor: 2,
            package_patch: 3,
            state_version: 1,
            abi_version: PACKAGE_ABI_VERSION_V1,
            ui_version: 2,
            package_sha256: [0xAB; 32],
        };
        let raw = [1u8, 2, 3, 4, 5];
        let encoded_env = env.encode_with_raw_state(&raw).expect("encode envelope");
        let (decoded_env, decoded_raw) =
            PackageIdentityEnvelope::decode_with_raw_state(&encoded_env).expect("decode envelope");
        assert_eq!(decoded_env, env);
        assert_eq!(decoded_raw, &raw);
        assert_eq!(
            PackageIdentityEnvelope::decode_with_raw_state(&encoded_env[..10]),
            Err(DeviceContractError::MalformedState)
        );
    }
}
