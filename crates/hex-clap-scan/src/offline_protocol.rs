//! Owned controlled offline fixture messages; no engine or plugin pointer API.
use crate::{MAX_PACKET, SDK};
use serde::{Deserialize, Serialize};
pub const GAIN_ID: u32 = 0x200001;
pub const PLUGIN_ID: &str = "org.hex.controlled.offline-gain";
pub const MODES: [&str; 12] = [
    "process",
    "process-hang",
    "process-crash",
    "process-nonfinite",
    "process-error",
    "process-descendant",
    "instance-init-fail",
    "state-fail",
    "bus-refuse",
    "restart",
    "sleep",
    "tail",
];
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Point {
    pub offset: u32,
    pub value: f64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Block {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    pub points: Vec<Point>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u16,
    pub request: u64,
    pub sample_rate: u32,
    pub start_frame: u64,
    pub initial_gain: f64,
    pub state: Vec<u8>,
    pub blocks: Vec<Block>,
}
impl Request {
    pub fn validate(&self) -> Result<usize, String> {
        if self.version != 1
            || self.request == 0
            || ![44100, 48000].contains(&self.sample_rate)
            || !plain(self.initial_gain)
            || self.state.len() > 65536
            || self.blocks.is_empty()
            || self.blocks.len() > 32
        {
            return Err("offline request header/capacity refused".into());
        }
        if self.state.is_empty() && self.start_frame != 0 {
            return Err("fresh controlled fixture requires frame zero".into());
        }
        let mut frames = 0usize;
        let mut points = 0usize;
        for b in &self.blocks {
            if b.left.is_empty()
                || b.left.len() > 1024
                || b.left.len() != b.right.len()
                || b.points.len() > 128
                || b.left.iter().chain(&b.right).any(|v| !v.is_finite())
            {
                return Err("offline PCM/block capacity refused".into());
            }
            frames = frames.checked_add(b.left.len()).ok_or("frame overflow")?;
            points = points.checked_add(b.points.len()).ok_or("point overflow")?;
            if frames > 8192 || points > 1024 {
                return Err("offline aggregate capacity refused".into());
            }
            let mut prior = None;
            for p in &b.points {
                if p.offset as usize >= b.left.len()
                    || !plain(p.value)
                    || prior.is_some_and(|v| v > p.offset)
                {
                    return Err("offline point order/value refused".into());
                }
                prior = Some(p.offset);
            }
        }
        if self
            .start_frame
            .checked_add(frames as u64)
            .is_none_or(|n| n > i64::MAX as u64)
        {
            return Err("offline clock overflow refused".into());
        }
        Ok(frames)
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut w = Writer::default();
        w.u16(1);
        w.u64(self.request);
        w.blob(PLUGIN_ID.as_bytes());
        w.u32(self.sample_rate);
        w.u64(self.start_frame);
        w.f64(self.initial_gain);
        w.blob(&self.state);
        w.u32(self.blocks.len() as u32);
        for b in &self.blocks {
            w.u32(b.left.len() as u32);
            w.u32(b.points.len() as u32);
            for p in &b.points {
                w.u32(p.offset);
                w.f64(p.value);
            }
            for v in b.left.iter().chain(&b.right) {
                w.u32(v.to_bits());
            }
        }
        packet(b"HCOFFQ01", w.bytes)
    }
}
fn plain(v: f64) -> bool {
    v.is_finite() && (0.0..=2.0).contains(&v)
}
#[derive(Clone, Debug, Serialize)]
pub struct Parameter {
    pub id: u32,
    pub flags: u32,
    pub min: f64,
    pub max: f64,
    pub default_plain: f64,
    pub title: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct ResultRecord {
    pub sdk: String,
    pub platform: String,
    pub plugin_id: String,
    pub sample_rate: u32,
    pub start_frame: u64,
    pub end_frame: u64,
    pub latency: u32,
    pub tail: u32,
    pub parameters: Vec<Parameter>,
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    pub state: Vec<u8>,
    pub lifecycle: Vec<String>,
    pub callbacks: Vec<String>,
    pub statuses: Vec<u32>,
}
pub fn decode(bytes: &[u8], request: &Request) -> Result<ResultRecord, String> {
    let frames = request.validate()?;
    let mut r = Reader::packet(bytes, b"HCOFFR01")?;
    if r.u16()? != 1 || r.u64()? != request.request {
        return Err("offline response version/request refused".into());
    }
    let sdk = r.text(40)?;
    let platform = r.text(32)?;
    let plugin_id = r.text(128)?;
    let sample_rate = r.u32()?;
    let start_frame = r.u64()?;
    let end_frame = r.u64()?;
    let latency = r.u32()?;
    let tail = r.u32()?;
    // This protocol is explicitly the original zero-latency stereo fixture profile.
    if sdk != SDK
        || platform != "linux-x86_64"
        || plugin_id != PLUGIN_ID
        || sample_rate != request.sample_rate
        || start_frame != request.start_frame
        || end_frame != request.start_frame + frames as u64
        || latency != 0
        || ![0, 2].contains(&tail)
        || r.u32()? != 2
        || r.u32()? != 2
        || r.u32()? != 0
        || r.u32()? != 0
    {
        return Err("offline identity/capability refused".into());
    }
    let count = r.u32()? as usize;
    if count == 0 || count > 128 {
        return Err("parameter count refused".into());
    }
    let mut parameters = Vec::with_capacity(count);
    let mut ids = std::collections::BTreeSet::new();
    for _ in 0..count {
        let id = r.u32()?;
        let flags = r.u32()?;
        let min = r.f64()?;
        let max = r.f64()?;
        let default_plain = r.f64()?;
        let title = r.text(512)?;
        if !ids.insert(id) || !plain(default_plain) || min != 0.0 || max != 2.0 {
            return Err("parameter metadata refused".into());
        }
        parameters.push(Parameter {
            id,
            flags,
            min,
            max,
            default_plain,
            title,
        });
    }
    if parameters.len() != 1
        || parameters[0].id != GAIN_ID
        || parameters[0].flags != 32
        || parameters[0].default_plain != 1.0
    {
        return Err("controlled gain profile refused".into());
    }
    if r.u32()? as usize != frames {
        return Err("output frame count refused".into());
    }
    let mut pcm = Vec::with_capacity(frames * 2);
    for _ in 0..frames * 2 {
        let v = f32::from_bits(r.u32()?);
        if !v.is_finite() {
            return Err("non-finite processed PCM refused".into());
        }
        pcm.push(v);
    }
    let right = pcm.split_off(frames);
    let left = pcm;
    let state = r.blob(65536)?;
    let lifecycle = r.strings(128)?;
    let callbacks = r.strings(128)?;
    let n = r.u32()? as usize;
    if n != request.blocks.len() {
        return Err("status count refused".into());
    }
    let statuses = (0..n).map(|_| r.u32()).collect::<Result<Vec<_>, _>>()?;
    if statuses.iter().any(|s| !(1..=5).contains(s)) {
        return Err("process status refused".into());
    }
    for (i, status) in statuses.iter().enumerate() {
        if *status == 5
            && (i == 0
                || ![4, 5].contains(&statuses[i - 1])
                || !request.blocks[i].points.is_empty()
                || request.blocks[i]
                    .left
                    .iter()
                    .chain(&request.blocks[i].right)
                    .any(|v| *v != 0.0))
        {
            return Err("invalid sleep skip refused".into());
        }
    }
    if r.at != r.bytes.len() {
        return Err("trailing offline bytes refused".into());
    }
    Ok(ResultRecord {
        sdk,
        platform,
        plugin_id,
        sample_rate,
        start_frame,
        end_frame,
        latency,
        tail,
        parameters,
        left,
        right,
        state,
        lifecycle,
        callbacks,
        statuses,
    })
}
#[derive(Default)]
struct Writer {
    bytes: Vec<u8>,
}
impl Writer {
    fn u16(&mut self, v: u16) {
        self.bytes.extend(v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.bytes.extend(v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.bytes.extend(v.to_le_bytes());
    }
    fn f64(&mut self, v: f64) {
        self.u64(v.to_bits());
    }
    fn blob(&mut self, v: &[u8]) {
        self.u32(v.len() as u32);
        self.bytes.extend(v);
    }
}
fn packet(magic: &[u8; 8], bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    if bytes.len() > MAX_PACKET {
        return Err("offline packet capacity refused".into());
    }
    let mut p = magic.to_vec();
    p.extend((bytes.len() as u32).to_le_bytes());
    p.extend(bytes);
    Ok(p)
}
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn packet(bytes: &'a [u8], magic: &[u8; 8]) -> Result<Self, String> {
        if bytes.len() < 12
            || &bytes[..8] != magic
            || bytes.len() > MAX_PACKET + 12
            || u32::from_le_bytes(bytes[8..12].try_into().map_err(|_| "length")?) as usize
                != bytes.len() - 12
        {
            return Err("offline packet header/length refused".into());
        }
        Ok(Self {
            bytes: &bytes[12..],
            at: 0,
        })
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).ok_or("offset overflow")?;
        let b = self
            .bytes
            .get(self.at..end)
            .ok_or("truncated offline packet")?;
        self.at = end;
        Ok(b)
    }
    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().map_err(|_| "u16")?,
        ))
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().map_err(|_| "u32")?,
        ))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().map_err(|_| "u64")?,
        ))
    }
    fn f64(&mut self) -> Result<f64, String> {
        Ok(f64::from_bits(self.u64()?))
    }
    fn blob(&mut self, max: usize) -> Result<Vec<u8>, String> {
        let n = self.u32()? as usize;
        if n > max {
            return Err("offline field capacity refused".into());
        }
        Ok(self.take(n)?.to_vec())
    }
    fn text(&mut self, max: usize) -> Result<String, String> {
        let value =
            String::from_utf8(self.blob(max)?).map_err(|_| "offline UTF8 refused".to_owned())?;
        if value.contains('\0') {
            return Err("offline NUL refused".into());
        }
        Ok(value)
    }
    fn strings(&mut self, max: usize) -> Result<Vec<String>, String> {
        let n = self.u32()? as usize;
        if n > max {
            return Err("offline record capacity refused".into());
        }
        (0..n).map(|_| self.text(512)).collect()
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildReceipt {
    pub version: u32,
    pub scope: String,
    pub scanner_receipt_sha256: String,
    pub source_sha256: String,
    pub helper_sha256: String,
    pub modules: Vec<crate::Module>,
}
pub fn source_hash() -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for (name, bytes) in [
        (
            "offline.cpp",
            include_bytes!("../native/offline.cpp").as_slice(),
        ),
        (
            "offline_host.hpp",
            include_bytes!("../native/offline_host.hpp").as_slice(),
        ),
        (
            "offline_wire.hpp",
            include_bytes!("../native/offline_wire.hpp").as_slice(),
        ),
        (
            "processing_fixture.cpp",
            include_bytes!("../native/processing_fixture.cpp").as_slice(),
        ),
    ] {
        h.update(name);
        h.update([0]);
        h.update(bytes);
        h.update([0]);
    }
    format!("{h:x}", h = h.finalize())
}
impl BuildReceipt {
    pub fn validate(&self, scanner: &[u8]) -> Result<(), String> {
        if self.version != 1
            || self.scope != "original-controlled-offline-fixtures"
            || self.scanner_receipt_sha256 != crate::hash(scanner)
            || self.source_sha256 != source_hash()
            || !crate::hex_digest(&self.helper_sha256)
            || self.modules.len() != MODES.len()
        {
            return Err("offline build identity refused".into());
        }
        let mut names = std::collections::BTreeSet::new();
        for m in &self.modules {
            if !MODES.contains(&m.name.as_str())
                || !names.insert(m.name.as_str())
                || !crate::hex_digest(&m.sha256)
                || m.binary != format!("{}.clap", m.name)
            {
                return Err("only original offline fixtures allowed".into());
            }
        }
        Ok(())
    }
}
