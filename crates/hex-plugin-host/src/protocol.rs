//! Existing ordinary-instance wire; actual direct reap belongs to the client.
//! Control/worker only. Bounds precede copies; copied facts never authorize a graph.
use crate::transport::{Block, MAX_FRAMES};
use crate::{Error, Result};
use hex_lattice_contract::{ForeignProcessorBinding as Binding, NativeDeviceState};
use hex_plugin_state::{Identity, STATE_VERSION, TYPE_ID};
use std::io::Cursor;
mod capture;
mod edits;
mod parameter_page;
pub use edits::{
    MAX_PARAMETER_EDITS, ParameterEdit, ParameterEditKind, ParameterEdits, RecoveryEdits,
    edit_request, idle_editor_edit_request, parameter_edits, recovery_edit_request, recovery_edits,
};
pub use parameter_page::{
    ControllerParameter, ControllerRequest, MAX_CONTROLLER_PARAMETERS, PARAMETER_PAGE_SIZE,
    ParameterConversion, ParameterConversionResult, ParameterConversionStatus, ParameterPage,
    ParameterPageProgress, ParameterPageQuery, ParameterPlainValue, ParameterText,
    PluginEditorState, parameter_page, parameter_page_request,
};
mod flush;
pub use capture::{
    CAPTURE_REFUSAL_MAGIC, StateCaptureRefusal, capture_refusal, capture_request, captured_state,
    live_capture_reply, live_capture_request, live_captured_state,
};
pub use flush::{
    FlushDisposition, FlushedParameter, FlushedParameters, MAX_FLUSHED_PARAMETERS, flush_request,
    flushed_parameters,
};

pub const CONFIG_LIMIT: usize = 8448;
pub const CONTROL_LIMIT: usize = 8192;
pub const PACKET_LIMIT: usize = 12 + hex_plugin_state::MAX_ENVELOPE_BYTES + 166;
pub(crate) fn control_limit(magic: &[u8]) -> usize {
    let prefix = match magic {
        b"HXVICF03" => return CONFIG_LIMIT,
        b"HXVIST02" => 39,
        b"HXVIFA04" => 140,
        b"HXVIQF03" => 156,
        b"HXVIAR02" => 164,
        b"HXVIRR03" => 166,
        b"HXVIRD05" => 86,
        b"HXVICS02" => 62,
        b"HXVLCA02" => 63,
        _ => return CONTROL_LIMIT,
    };
    prefix + hex_plugin_state::MAX_ENVELOPE_BYTES
}
/// Explicit protocol DATA during preparation/completion and quiescent capture:
/// at most two outgoing frames, or a presentation frame plus both bounded
/// Vec/Arc state payloads during capture. Heap-backed incoming Packet storage belongs
/// to its enclosing owner. Source/configuration, file/inventory workspaces,
/// errors and allocator/Arc/OS implementation overhead are separate.
pub const MAX_WORKING_DATA_BYTES: usize = 2 * PACKET_LIMIT + CONFIG_LIMIT;

#[derive(Clone, Debug)]
pub struct Configuration {
    pub binding: Binding,
    pub identity: Identity,
    pub sample_rate: f64,
    pub quantum: u16,
    pub input_channels: u8,
    pub output_channels: u8,
    pub bundle: String,
    pub executable: String,
    /// Copied canonical startup state, never authority to publish a graph.
    pub restore: Option<NativeDeviceState>,
}
impl Configuration {
    pub fn validate(&self) -> Result<()> {
        if !self.binding.is_valid()
            || !self.sample_rate.is_finite()
            || self.sample_rate <= 0.0
            || self.sample_rate > 768000.0
            || self.quantum == 0
            || usize::from(self.quantum) > MAX_FRAMES
            || self.input_channels > 2
            || !(1..=2).contains(&self.output_channels)
            || [&self.bundle, &self.executable]
                .iter()
                .any(|p| p.is_empty() || p.len() > 4096 || p.contains('\0'))
        {
            return Err(Error::Protocol("unsupported instance configuration"));
        }
        if let Some(state) = &self.restore {
            state
                .require_asset_free_processing()
                .map_err(|_| Error::Protocol("startup state must be asset free"))?;
            hex_plugin_state::available(state, Some(self.identity)).map_err(Error::State)?;
        }
        Ok(())
    }
    /// A separate bounded frame keeps the existing path/configuration bound.
    /// The worker validates both frames before entering the selected module.
    pub fn startup_request(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut out = control_prefix(
            *b"HXVIST02",
            39 + self
                .restore
                .as_ref()
                .map_or(0, |state| state.payload().len()),
        )?;
        stamp(&mut out, self.binding);
        self.append_restore_ack(&mut out);
        finish_control(out)
    }
    fn append_restore_ack(&self, out: &mut Vec<u8>) {
        let payload = self
            .restore
            .as_ref()
            .map_or(&[][..], |state| state.payload());
        out.push(u8::from(self.restore.is_some()));
        out.extend((payload.len() as u32).to_le_bytes());
        out.extend(payload);
    }
    fn check_restore_ack(&self, r: &mut Reader<'_>) -> Result<bool> {
        let restored = match r.byte()? {
            0 => false,
            1 => true,
            _ => {
                return Err(Error::Protocol(
                    "invalid startup restoration acknowledgement",
                ));
            }
        };
        let count = u32::from_le_bytes(r.array()?) as usize;
        let expected = self
            .restore
            .as_ref()
            .map_or(&[][..], |state| state.payload());
        if restored != self.restore.is_some()
            || count != expected.len()
            || r.take(count)? != expected
        {
            return Err(Error::Protocol(
                "startup restoration acknowledgement differs from the requested state",
            ));
        }
        Ok(restored)
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        // Version/binding, identity, format and two path lengths occupy138
        // bytes. The independent wire fixture verifies this framing contract.
        let mut out = control_prefix(
            *b"HXVICF03",
            138 + self.bundle.len() + self.executable.len(),
        )?;
        stamp(&mut out, self.binding);
        out.extend(self.identity.class.bytes());
        out.extend(self.identity.binary.executable_sha256);
        out.extend(self.identity.binary.info_sha256);
        out.extend(self.sample_rate.to_le_bytes());
        out.extend(self.quantum.to_le_bytes());
        out.extend([self.input_channels, self.output_channels, 0, 0, 0, 0]); // Float32 + reserved
        for path in [&self.bundle, &self.executable] {
            out.extend((path.len() as u32).to_le_bytes());
            out.extend(path.as_bytes());
        }
        finish_control(out)
    }
    pub fn validate_block(&self, block: &Block, sequence: u64) -> Result<()> {
        self.validate()?;
        if block.binding != self.binding
            || block.frames != self.quantum
            || block.sequence != sequence
            || sequence.checked_mul(u64::from(self.quantum)) != Some(block.first_frame)
            || block.events.as_slice().iter().any(|event| {
                self.input_channels != 0
                    || event.len != 3
                    || !matches!(event.bytes[0] & 0xf0, 0x80 | 0x90)
            })
            || !block.pcm[..usize::from(self.quantum)]
                .iter()
                .flatten()
                .all(|v| v.is_finite())
            || block.pcm[..usize::from(self.quantum)].iter().any(|v| {
                (self.input_channels < 2 && v[1] != 0.0)
                    || (self.input_channels == 0 && v[0] != 0.0)
            })
        {
            return Err(Error::Protocol(
                "instance request identity or samples refused",
            ));
        }
        Ok(())
    }
}

/// Actual caller-derived per-direction contributions. Unknown uses a separate
/// plan, so its SDK sentinel cannot be mistaken for measured zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentationLatency {
    pub input_frames: u32,
    pub output_frames: u32,
}
/// Unknown is a deliberate compatibility mode, distinct from a known zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentationPlan {
    RequireKnown,
    Known(PresentationLatency),
    AllowUnknown,
}
impl From<Option<PresentationLatency>> for PresentationPlan {
    fn from(value: Option<PresentationLatency>) -> Self {
        value.map_or(Self::RequireKnown, Self::Known)
    }
}
impl PresentationPlan {
    fn flag(self) -> u8 {
        match self {
            Self::RequireKnown => 0,
            Self::Known(_) => 1,
            Self::AllowUnknown => 2,
        }
    }
    fn known(self) -> Option<PresentationLatency> {
        match self {
            Self::Known(value) => Some(value),
            _ => None,
        }
    }
    pub(super) fn expected(self, available: bool) -> Result<Presentation> {
        match (self, available) {
            (_, false) => Ok(Presentation::Absent),
            (Self::Known(_), true) => Ok(Presentation::Applied),
            (Self::AllowUnknown, true) => Ok(Presentation::Unknown),
            (Self::RequireKnown, true) => Err(Error::Protocol(
                "actual presentation contributions are unavailable",
            )),
        }
    }
}
pub fn presentation_request(
    binding: Binding,
    plan: impl Into<PresentationPlan>,
) -> Result<Vec<u8>> {
    let plan = plan.into();
    let value = plan.known();
    let mut out = control_prefix(*b"HXVIPR01", 35 + usize::from(value.is_some()) * 8)?;
    stamp(&mut out, binding);
    out.push(plan.flag());
    if let Some(value) = value {
        out.extend(value.input_frames.to_le_bytes());
        out.extend(value.output_frames.to_le_bytes());
    }
    finish_control(out)
}
/// Raw source-bound preparation facts. No state capture, processing start,
/// canonical/graph admission or audio application is acknowledged by this type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Facts {
    pub algorithmic_latency_frames: u32,
    /// Source-reported residual tail in SDK sample frames. u32::MAX is the SDK
    /// infinite-tail sentinel. This raw report is not a quiescence proof or a
    /// host/bridge drain bound and never sizes an allocation.
    pub tail_frames: u32,
    pub presentation_available: bool,
    pub restored: bool,
}
impl Facts {
    pub fn decode(body: &[u8], config: &Configuration) -> Result<Self> {
        config.validate()?;
        let mut r = Reader::new(body);
        if r.binding()? != config.binding
            || r.array::<16>()? != config.identity.class.bytes()
            || r.array::<32>()? != config.identity.binary.executable_sha256
            || r.array::<32>()? != config.identity.binary.info_sha256
            || f64::from_le_bytes(r.array()?) != config.sample_rate
            || u16::from_le_bytes(r.array()?) != config.quantum
            || r.byte()? != config.input_channels
            || r.byte()? != config.output_channels
        {
            return Err(Error::Protocol(
                "instance preparation facts differ from selected source/configuration",
            ));
        }
        let algorithmic_latency_frames = u32::from_le_bytes(r.array()?);
        let tail_frames = u32::from_le_bytes(r.array()?);
        let presentation_available = match r.byte()? {
            0 => false,
            1 => true,
            _ => return Err(Error::Protocol("invalid presentation interface fact")),
        };
        let restored = config.check_restore_ack(&mut r)?;
        r.end()?;
        Ok(Self {
            algorithmic_latency_frames,
            tail_frames,
            presentation_available,
            restored,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presentation {
    Absent,
    Applied,
    Unknown,
}
#[derive(Clone, Debug)]
pub struct Ready {
    pub initial_state: NativeDeviceState,
    /// Untrusted raw SDK report. The graph owner must reserve/validate its delay
    /// capacity before admission; no allocation is sized from it here.
    pub algorithmic_latency_frames: u32,
    /// Source-reported residual tail in SDK sample frames. u32::MAX is the SDK
    /// infinite-tail sentinel. This raw report is not a quiescence proof or a
    /// host/bridge drain bound and never sizes an allocation.
    pub tail_frames: u32,
    pub presentation: Presentation,
    /// Exact startup request was acknowledged; canonical state remains caller owned.
    pub restored: bool,
}
impl Ready {
    pub fn decode(
        body: &[u8],
        config: &Configuration,
        expected: impl Into<PresentationPlan>,
    ) -> Result<Self> {
        let expected = expected.into();
        config.validate()?;
        let mut r = Reader::new(body);
        if r.binding()? != config.binding
            || r.array::<16>()? != config.identity.class.bytes()
            || f64::from_le_bytes(r.array()?) != config.sample_rate
            || u16::from_le_bytes(r.array()?) != config.quantum
            || r.byte()? != config.input_channels
            || r.byte()? != config.output_channels
        {
            return Err(Error::Protocol("instance ready configuration mismatch"));
        }
        let algorithmic_latency_frames = u32::from_le_bytes(r.array()?);
        let tail_frames = u32::from_le_bytes(r.array()?);
        let presentation = match r.byte()? {
            0 => Presentation::Absent,
            1 => Presentation::Applied,
            2 => Presentation::Unknown,
            _ => return Err(Error::Protocol("refused presentation cannot be ready")),
        };
        let supplied = match r.byte()? {
            0 => None,
            1 => Some(PresentationLatency {
                input_frames: u32::from_le_bytes(r.array()?),
                output_frames: u32::from_le_bytes(r.array()?),
            }),
            _ => return Err(Error::Protocol("invalid presentation acknowledgement")),
        };
        if supplied != expected.known()
            || (presentation != Presentation::Absent && presentation != expected.expected(true)?)
        {
            return Err(Error::Protocol(
                "presentation acknowledgement differs from the requested contributions",
            ));
        }
        let restored = match r.byte()? {
            0 => false,
            1 => true,
            _ => return Err(Error::Protocol("invalid final restoration acknowledgement")),
        };
        if restored != config.restore.is_some() {
            return Err(Error::Protocol(
                "final restoration acknowledgement differs from startup request",
            ));
        }
        let component = match r.byte()? {
            0 => false,
            1 => true,
            _ => return Err(Error::Protocol("invalid ready state presence")),
        };
        let count = u32::from_le_bytes(r.array()?) as usize;
        if count > hex_plugin_state::MAX_ENVELOPE_BYTES {
            return Err(Error::Protocol("ready state exceeds envelope bound"));
        }
        let payload = r.take(count)?;
        r.end()?;
        let mut copied = buffer(count)?;
        copied.extend_from_slice(payload);
        let initial_state = NativeDeviceState::new(TYPE_ID, STATE_VERSION, copied)
            .map_err(|_| Error::Protocol("invalid ready state envelope"))?;
        let state = hex_plugin_state::decode(&initial_state).map_err(Error::State)?;
        if state.identity != config.identity || state.component.is_some() != component {
            return Err(Error::Protocol("ready state source identity mismatch"));
        }
        Ok(Self {
            initial_state,
            algorithmic_latency_frames,
            tail_frames,
            presentation,
            restored,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Terminal {
    pub blocks: u64,
    pub frames: u64,
    pub failure: u32,
    pub closed: bool,
    pub unloaded: bool,
}
impl Terminal {
    pub fn decode(body: &[u8], binding: Binding) -> Result<Self> {
        let mut r = Reader::new(body);
        if r.binding()? != binding {
            return Err(Error::Protocol("terminal identity mismatch"));
        }
        let blocks = u64::from_le_bytes(r.array()?);
        let frames = u64::from_le_bytes(r.array()?);
        let failure = u32::from_le_bytes(r.array()?);
        let boolean = |v| match v {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::Protocol("invalid terminal flag")),
        };
        let closed = boolean(r.byte()?)?;
        let unloaded = boolean(r.byte()?)?;
        r.end()?;
        if unloaded && !closed {
            return Err(Error::Protocol("unload without closed instance"));
        }
        Ok(Self {
            blocks,
            frames,
            failure,
            closed,
            unloaded,
        })
    }
}

pub fn control(magic: [u8; 8], body: &[u8]) -> Result<Vec<u8>> {
    let mut out = control_prefix(magic, body.len())?;
    out.extend_from_slice(body);
    finish_control(out)
}

pub(crate) fn buffer(capacity: usize) -> Result<Vec<u8>> {
    if capacity > PACKET_LIMIT {
        return Err(Error::Protocol("protocol allocation exceeds frame bound"));
    }
    let mut out = Vec::new();
    out.try_reserve_exact(capacity)
        .map_err(|_| Error::Protocol("protocol allocation refused"))?;
    if out.capacity() != capacity {
        return Err(Error::Protocol("protocol allocation bound differs"));
    }
    Ok(out)
}

fn control_prefix(magic: [u8; 8], body_len: usize) -> Result<Vec<u8>> {
    if body_len > control_limit(&magic) {
        return Err(Error::Protocol("control frame exceeds bound"));
    }
    let mut out = buffer(12 + body_len)?;
    out.extend(magic);
    out.extend((body_len as u32).to_le_bytes());
    Ok(out)
}
fn finish_control(out: Vec<u8>) -> Result<Vec<u8>> {
    let size = out
        .get(8..12)
        .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
        .map(u32::from_le_bytes)
        .and_then(|size| usize::try_from(size).ok())
        .and_then(|size| size.checked_add(12));
    if size != Some(out.len()) || out.capacity() != out.len() {
        return Err(Error::Protocol("control construction extent differs"));
    }
    Ok(out)
}
pub fn stop(binding: Binding, sequence: u64, frames: u64) -> [u8; 64] {
    let mut out = [0; 64];
    out[..4].copy_from_slice(b"HXVE");
    out[4..6].copy_from_slice(&1_u16.to_le_bytes());
    for (n, value) in [
        binding.root,
        binding.instance,
        binding.graph,
        binding.epoch,
        sequence,
        frames,
    ]
    .iter()
    .enumerate()
    {
        out[16 + n * 8..24 + n * 8].copy_from_slice(&value.to_le_bytes());
    }
    out
}

/// Quiesce only after the exact acknowledged PCM prefix. This is not a final
/// retirement request, nor authority to reuse an old callback/ring owner.
pub fn quiesce(config: &Configuration, sequence: u64) -> Result<[u8; 64]> {
    config.validate()?;
    let frames = sequence
        .checked_mul(u64::from(config.quantum))
        .ok_or(Error::Protocol("instance position exhausted"))?;
    let mut out = stop(config.binding, sequence, frames);
    out[..4].copy_from_slice(b"HXVQ");
    Ok(out)
}

/// Stopped restoration discards the old graph's instrument gates. The helper
/// releases its accepted notes without extending the acknowledged PCM prefix.
pub(crate) fn quiesce_for_stopped_restore(
    config: &Configuration,
    sequence: u64,
) -> Result<[u8; 64]> {
    let mut out = quiesce(config, sequence)?;
    if config.input_channels == 0 {
        out[4..6].copy_from_slice(&2_u16.to_le_bytes());
    }
    Ok(out)
}

/// Explicit stopped component reactivation, distinct from ordinary quiescence.
pub fn reactivate(config: &Configuration, sequence: u64, operation: u64) -> Result<[u8; 72]> {
    if operation == 0 {
        return Err(Error::Protocol("reactivation operation must be nonzero"));
    }
    let mut out = [0; 72];
    out[..64].copy_from_slice(&quiesce(config, sequence)?);
    out[..4].copy_from_slice(b"HXVA");
    out[64..].copy_from_slice(&operation.to_le_bytes());
    Ok(out)
}

pub fn validate_resume(previous: Binding, next: Binding) -> Result<()> {
    if [
        previous.root,
        previous.instance,
        previous.graph,
        previous.epoch,
    ]
    .contains(&0)
        || next.root != previous.root
        || next.instance != previous.instance
        || next.graph < previous.graph
        || next.epoch <= previous.epoch
    {
        return Err(Error::Protocol(
            "resume requires the same source owner and a newer epoch",
        ));
    }
    Ok(())
}

/// Resume keeps the original source/rate/buses/quantum. Only the two requested
/// binding words differ; the header still names the acknowledged old binding.
pub fn resume_request(
    config: &Configuration,
    next: Binding,
    plan: impl Into<PresentationPlan>,
) -> Result<Vec<u8>> {
    let plan = plan.into();
    let presentation = plan.known();
    config.validate()?;
    validate_resume(config.binding, next)?;
    let mut header = stop(config.binding, next.graph, next.epoch);
    header[..4].copy_from_slice(b"HXVR");
    header[10] = plan.flag();
    let mut out = buffer(64 + usize::from(presentation.is_some()) * 8)?;
    out.extend(header);
    if let Some(value) = presentation {
        out.extend(value.input_frames.to_le_bytes());
        out.extend(value.output_frames.to_le_bytes());
    }
    Ok(out)
}

fn epoch_facts<'a>(
    body: &'a [u8],
    config: &Configuration,
    sequence: u64,
) -> Result<(Facts, Reader<'a>)> {
    // Current fixed facts plus the exact optional original startup envelope.
    config.validate()?;
    let size = 140_usize
        .checked_add(
            config
                .restore
                .as_ref()
                .map_or(0, |state| state.payload().len()),
        )
        .ok_or(Error::Protocol("epoch facts extent overflow"))?;
    let facts = Facts::decode(
        body.get(..size)
            .ok_or(Error::Protocol("truncated epoch facts"))?,
        config,
    )?;
    let mut rest = Reader::new(
        body.get(size..)
            .ok_or(Error::Protocol("truncated epoch facts"))?,
    );
    let frames = sequence
        .checked_mul(u64::from(config.quantum))
        .ok_or(Error::Protocol("instance position exhausted"))?;
    if u64::from_le_bytes(rest.array()?) != sequence || u64::from_le_bytes(rest.array()?) != frames
    {
        return Err(Error::Protocol(
            "quiescent PCM prefix differs from actual completion",
        ));
    }
    Ok((facts, rest))
}

pub fn quiescent_facts(body: &[u8], config: &Configuration, sequence: u64) -> Result<Facts> {
    let (facts, rest) = epoch_facts(body, config, sequence)?;
    rest.end()?;
    Ok(facts)
}

pub fn reactivated_facts(
    body: &[u8],
    config: &Configuration,
    sequence: u64,
    operation: u64,
) -> Result<Facts> {
    let (facts, mut rest) = epoch_facts(body, config, sequence)?;
    if operation == 0 || u64::from_le_bytes(rest.array()?) != operation {
        return Err(Error::Protocol("reactivation operation differs"));
    }
    rest.end()?;
    Ok(facts)
}

pub fn resumed(
    body: &[u8],
    config: &Configuration,
    sequence: u64,
    expected: Facts,
    presentation: impl Into<PresentationPlan>,
) -> Result<Presentation> {
    let presentation = presentation.into();
    let (facts, mut rest) = epoch_facts(body, config, sequence)?;
    if facts != expected {
        return Err(Error::Protocol(
            "native facts changed before resumed acknowledgement",
        ));
    }
    let result = match rest.byte()? {
        0 => Presentation::Absent,
        1 => Presentation::Applied,
        2 => Presentation::Unknown,
        _ => return Err(Error::Protocol("native presentation resume refused")),
    };
    let supplied = match rest.byte()? {
        0 => None,
        1 => Some(PresentationLatency {
            input_frames: u32::from_le_bytes(rest.array()?),
            output_frames: u32::from_le_bytes(rest.array()?),
        }),
        _ => {
            return Err(Error::Protocol(
                "invalid resumed presentation acknowledgement",
            ));
        }
    };
    rest.end()?;
    if supplied != presentation.known()
        || result != presentation.expected(facts.presentation_available)?
    {
        return Err(Error::Protocol(
            "resumed presentation differs from actual plan",
        ));
    }
    Ok(result)
}
pub fn pcm(bytes: &[u8], config: &Configuration, sequence: u64) -> Result<Block> {
    config.validate()?;
    let mut cursor = Cursor::new(bytes);
    let block = crate::transport::wire::read(&mut cursor)
        .map_err(Error::Io)?
        .ok_or(Error::Protocol("missing instance PCM reply"))?;
    if cursor.position() != bytes.len() as u64
        || block.binding != config.binding
        || block.sequence != sequence
        || block.frames != config.quantum
        || sequence.checked_mul(u64::from(config.quantum)) != Some(block.first_frame)
        || !block.events.as_slice().is_empty()
        || (config.output_channels == 1
            && block.pcm[..usize::from(config.quantum)]
                .iter()
                .any(|v| v[1] != 0.0))
    {
        return Err(Error::Protocol(
            "instance PCM reply identity or extent mismatch",
        ));
    }
    Ok(block)
}
pub fn error(body: &[u8], binding: Binding) -> Result<u32> {
    let mut r = Reader::new(body);
    if u16::from_le_bytes(r.array()?) != 1 {
        return Err(Error::Protocol("error frame version"));
    }
    match r.byte()? {
        0 => {}
        1 => {
            if r.ids()? != binding {
                return Err(Error::Protocol("error frame identity"));
            }
        }
        _ => return Err(Error::Protocol("error frame presence")),
    }
    let code = u32::from_le_bytes(r.array()?);
    r.end()?;
    Ok(code)
}
pub fn stamp(out: &mut Vec<u8>, binding: Binding) {
    out.extend(1_u16.to_le_bytes());
    for n in [binding.root, binding.instance, binding.graph, binding.epoch] {
        out.extend(n.to_le_bytes());
    }
}
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(n)
            .ok_or(Error::Protocol("frame offset overflow"))?;
        let bytes = self
            .bytes
            .get(self.at..end)
            .ok_or(Error::Protocol("truncated control frame"))?;
        self.at = end;
        Ok(bytes)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.take(N)?
            .try_into()
            .map_err(|_| Error::Protocol("truncated field"))
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }
    fn binding(&mut self) -> Result<Binding> {
        if u16::from_le_bytes(self.array()?) != 1 {
            return Err(Error::Protocol("instance protocol version"));
        }
        self.ids()
    }
    fn ids(&mut self) -> Result<Binding> {
        let b = Binding {
            root: u64::from_le_bytes(self.array()?),
            instance: u64::from_le_bytes(self.array()?),
            graph: u64::from_le_bytes(self.array()?),
            epoch: u64::from_le_bytes(self.array()?),
        };
        if [b.root, b.instance, b.graph, b.epoch].contains(&0) {
            return Err(Error::Protocol("missing instance binding"));
        }
        Ok(b)
    }
    fn end(&self) -> Result<()> {
        if self.at == self.bytes.len() {
            Ok(())
        } else {
            Err(Error::Protocol("trailing control bytes"))
        }
    }
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
