//! Existing ordinary-instance wire; actual direct reap belongs to the client.
//! Control/worker only. Bounds precede copies; copied facts never authorize a graph.
use crate::{Error, Result};
use hex_audio::plugin_transport::{Binding, Block, MAX_FRAMES};
use hex_lattice_contract::NativeDeviceState;
use hex_plugin_state::{Identity, STATE_VERSION, TYPE_ID};
use std::io::Cursor;

pub const CONFIG_LIMIT: usize = 8448;
pub const CONTROL_LIMIT: usize = 8192;
pub const PACKET_LIMIT: usize = CONFIG_LIMIT + 12;

#[derive(Clone, Debug)]
pub struct Configuration {
    pub binding: Binding,
    pub identity: Identity,
    pub sample_rate: f64,
    pub quantum: u16,
    pub input_channels: u8,
    pub output_channels: u8,
    /// Real caller-supplied per-direction presentation contributions, never
    /// a claim that physical or unknown latency has been removed/measured.
    pub input_presentation_frames: u32,
    pub output_presentation_frames: u32,
    pub bundle: String,
    pub executable: String,
}
impl Configuration {
    pub fn validate(&self) -> Result<()> {
        if [
            self.binding.root,
            self.binding.instance,
            self.binding.graph,
            self.binding.epoch,
        ]
        .contains(&0)
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
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut out = Vec::with_capacity(146 + self.bundle.len() + self.executable.len());
        stamp(&mut out, self.binding);
        out.extend(self.identity.class.bytes());
        out.extend(self.identity.binary.executable_sha256);
        out.extend(self.identity.binary.info_sha256);
        out.extend(self.sample_rate.to_le_bytes());
        out.extend(self.quantum.to_le_bytes());
        out.extend([self.input_channels, self.output_channels, 0, 0, 0, 0]); // Float32 + reserved
        out.extend(self.input_presentation_frames.to_le_bytes());
        out.extend(self.output_presentation_frames.to_le_bytes());
        for path in [&self.bundle, &self.executable] {
            out.extend((path.len() as u32).to_le_bytes());
            out.extend(path.as_bytes());
        }
        control(*b"HXVICF01", &out)
    }
    pub fn validate_block(&self, block: &Block, sequence: u64) -> Result<()> {
        self.validate()?;
        if block.binding != self.binding
            || block.frames != self.quantum
            || block.sequence != sequence
            || sequence.checked_mul(u64::from(self.quantum)) != Some(block.first_frame)
            || !block.events.as_slice().is_empty()
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presentation {
    Absent,
    Applied,
}
#[derive(Clone, Debug)]
pub struct Ready {
    pub initial_state: NativeDeviceState,
    /// Untrusted raw SDK report. The graph owner must reserve/validate its delay
    /// capacity before admission; no allocation is sized from it here.
    pub algorithmic_latency_frames: u32,
    pub presentation: Presentation,
}
impl Ready {
    pub fn decode(body: &[u8], config: &Configuration) -> Result<Self> {
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
        let presentation = match r.byte()? {
            0 => Presentation::Absent,
            1 => Presentation::Applied,
            _ => return Err(Error::Protocol("refused presentation cannot be ready")),
        };
        let component = match r.byte()? {
            0 => false,
            1 => true,
            _ => return Err(Error::Protocol("invalid ready state presence")),
        };
        let count = usize::from(u16::from_le_bytes(r.array()?));
        if count > 4096 {
            return Err(Error::Protocol("ready state exceeds envelope bound"));
        }
        let initial_state = NativeDeviceState::new(TYPE_ID, STATE_VERSION, r.take(count)?.to_vec())
            .map_err(|_| Error::Protocol("invalid ready state envelope"))?;
        r.end()?;
        let state = hex_plugin_state::decode(&initial_state).map_err(Error::State)?;
        if state.identity != config.identity || state.component.is_some() != component {
            return Err(Error::Protocol("ready state source identity mismatch"));
        }
        Ok(Self {
            initial_state,
            algorithmic_latency_frames,
            presentation,
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
    let limit = if &magic == b"HXVICF01" {
        CONFIG_LIMIT
    } else {
        CONTROL_LIMIT
    };
    if body.len() > limit {
        return Err(Error::Protocol("control frame exceeds bound"));
    }
    let mut out = Vec::with_capacity(12 + body.len());
    out.extend(magic);
    out.extend((body.len() as u32).to_le_bytes());
    out.extend(body);
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
pub fn pcm(bytes: &[u8], config: &Configuration, sequence: u64) -> Result<Block> {
    config.validate()?;
    let mut cursor = Cursor::new(bytes);
    let block = hex_audio::plugin_transport::wire::read(&mut cursor)
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
