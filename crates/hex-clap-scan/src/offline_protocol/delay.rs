//! Protocol3 ORIGINAL finite FIFO, not a plugin/graph admission API.
use super::*;
pub const DELAY_PROFILE: &str = "finite-delay-gain-v1";
pub const DELAY_PLUGIN_ID: &str = "org.hex.controlled.offline-finite-delay";
pub const DELAY_MODES: [&str; 24] = [
    "delay-0",
    "delay-1",
    "delay-16",
    "delay-31",
    "delay-32",
    "delay-inactive-callback",
    "delay-thread-callback",
    "delay-drift",
    "delay-restart",
    "delay-infinite-tail",
    "delay-shrink-tail",
    "delay-late-error",
    "delay-drain-hang",
    "delay-drain-crash",
    "delay-descendant",
    "delay-late-nonfinite",
    "delay-active-callback",
    "delay-main-tail",
    "delay-large-tail",
    "delay-stop-tail",
    "delay-late-callback",
    "delay-ignored-output",
    "delay-late-output",
    "delay-foreign-output",
];
fn allowed(n: u32) -> bool {
    [0, 1, 16, 31, 32].contains(&n)
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DelayOwner {
    pub profile: String,
    pub module_sha256: String,
    pub state_version: u32,
    pub sample_rate: u32,
    pub delay: u32,
    pub next_frame: u64,
    pub max_tail: u32,
    pub drained: u32,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DelayBlock {
    pub drain: bool,
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    pub points: Vec<Point>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DelayRequest {
    pub version: u16,
    pub profile: String,
    pub request: u64,
    pub sample_rate: u32,
    pub start_frame: u64,
    pub initial_gain: f64,
    pub delay: u32,
    pub state: Vec<u8>,
    pub state_owner: Option<DelayOwner>,
    pub blocks: Vec<DelayBlock>,
}
impl DelayRequest {
    fn base(&self) -> Request {
        Request {
            version: 1,
            request: self.request,
            sample_rate: self.sample_rate,
            start_frame: self.start_frame,
            initial_gain: self.initial_gain,
            state: self.state.clone(),
            blocks: self
                .blocks
                .iter()
                .map(|b| Block {
                    left: b.left.clone(),
                    right: b.right.clone(),
                    points: b.points.clone(),
                })
                .collect(),
        }
    }
    fn ledger(&self) -> Result<(u32, u32), String> {
        let (tail, mut drained) = self
            .state_owner
            .as_ref()
            .map_or((self.delay, 0), |o| (o.max_tail, o.drained));
        let mut suffix = false;
        for b in &self.blocks {
            if suffix && !b.drain {
                return Err("drain suffix order refused".into());
            }
            suffix |= b.drain;
            if b.drain {
                if !b.points.is_empty() || b.left.iter().chain(&b.right).any(|v| *v != 0.0) {
                    return Err("drain must be event-free zero PCM".into());
                }
                drained = drained
                    .checked_add(b.left.len() as u32)
                    .ok_or("drain overflow")?;
                if drained > 32 {
                    return Err("cumulative drain ceiling refused".into());
                }
            } else {
                drained = 0;
            }
        }
        Ok((tail, drained))
    }
    pub fn validate_bound(&self, module: &str) -> Result<usize, String> {
        let frames = self.base().validate()?;
        if self.version != 3
            || self.profile != DELAY_PROFILE
            || !allowed(self.delay)
            || !crate::hex_digest(module)
        {
            return Err("finite-delay profile refused".into());
        }
        match (&self.state_owner, self.state.is_empty()) {
            (None, true) => {}
            (Some(o), false)
                if o.profile == DELAY_PROFILE
                    && o.module_sha256 == module
                    && o.state_version == 3
                    && o.sample_rate == self.sample_rate
                    && o.delay == self.delay
                    && o.next_frame == self.start_frame
                    && o.max_tail >= self.delay
                    && o.max_tail <= 32
                    && o.drained <= 32 =>
            {
                validate_state(&self.state, o)?;
            }
            _ => return Err("finite-delay state owner refused".into()),
        }
        self.ledger()?;
        Ok(frames)
    }
    pub fn encode_bound(&self, module: &str) -> Result<Vec<u8>, String> {
        self.validate_bound(module)?;
        let mut w = Writer::default();
        w.u16(3);
        w.u64(self.request);
        w.blob(DELAY_PLUGIN_ID.as_bytes());
        w.u32(self.sample_rate);
        w.u64(self.start_frame);
        w.f64(self.initial_gain);
        w.blob(DELAY_PROFILE.as_bytes());
        w.u32(3);
        w.blob(module.as_bytes());
        w.u32(self.delay);
        let (tail, drained) = self
            .state_owner
            .as_ref()
            .map_or((self.delay, 0), |o| (o.max_tail, o.drained));
        w.u32(tail);
        w.u32(drained);
        w.blob(&self.state);
        w.u32(self.blocks.len() as u32);
        for b in &self.blocks {
            w.u8(u8::from(b.drain));
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
        packet(b"HCOFFQ03", w.bytes)
    }
}
fn validate_state(bytes: &[u8], o: &DelayOwner) -> Result<(), String> {
    if bytes.len() != 360 {
        return Err("finite FIFO state size refused".into());
    }
    let mut r = Reader { bytes, at: 0 };
    if r.u32()? != 0x444c5903 || r.u32()? != o.sample_rate || r.u32()? != o.delay {
        return Err("finite FIFO state identity refused".into());
    }
    let cursor = r.u32()?;
    if cursor >= o.delay.max(1)
        || u64::from(cursor)
            != (if o.delay == 0 {
                0
            } else {
                o.next_frame % u64::from(o.delay)
            })
        || u64::from(o.drained) > o.next_frame
        || r.u64()? != o.next_frame
        || !plain(r.f64()?)
    {
        return Err("finite FIFO cursor/frame/gain refused".into());
    }
    for _ in 0..2 {
        for i in 0..32 {
            let v = f32::from_bits(r.u32()?);
            if !v.is_finite() || ((i >= o.delay || o.drained >= o.max_tail) && v != 0.0) {
                return Err("finite FIFO queue refused".into());
            }
        }
    }
    if r.u32()? != o.max_tail || r.u32()? != o.drained || r.take(64)? != o.module_sha256.as_bytes()
    {
        return Err("finite FIFO continuation ledger/module refused".into());
    }
    Ok(())
}
#[derive(Debug, Serialize)]
pub struct DelayResult {
    pub audio: ResultRecord,
    pub profile: String,
    pub state_owner: DelayOwner,
    pub pending: u32,
    pub sample_bytes: u32,
    pub metadata_bytes: u32,
    pub input_reserved_bytes: u64,
    pub isolation_overhead: &'static str,
}
pub fn decode_delay(
    bytes: &[u8],
    request: &DelayRequest,
    module: &str,
) -> Result<DelayResult, String> {
    request.validate_bound(module)?;
    let mut r = Reader::packet(bytes, b"HCOFFR03")?;
    let audio = decode_record(&mut r, &request.base(), 3, DELAY_PLUGIN_ID, 0)?;
    let profile = r.text(64)?;
    let version = r.u32()?;
    let identity = r.text(64)?;
    let drained = r.u32()?;
    let pending = r.u32()?;
    let sample_bytes = r.u32()?;
    let metadata_bytes = r.u32()?;
    let input_reserved_bytes = r.u64()?;
    let (old_tail, expected_drained) = request.ledger()?;
    if profile != DELAY_PROFILE
        || version != 3
        || identity != module
        || audio.latency != request.delay
        || audio.tail < old_tail
        || audio.tail > 32
        || drained != expected_drained
        || pending != audio.tail.saturating_sub(drained)
        || sample_bytes != 256
        || metadata_bytes != 32
        || input_reserved_bytes != request.blocks.len() as u64 * 128 * 56
        || r.at != r.bytes.len()
    {
        return Err("finite-delay response owner/demand refused".into());
    }
    let state_owner = DelayOwner {
        profile: profile.clone(),
        module_sha256: identity,
        state_version: version,
        sample_rate: request.sample_rate,
        delay: request.delay,
        next_frame: audio.end_frame,
        max_tail: audio.tail,
        drained,
    };
    validate_state(&audio.state, &state_owner)?;
    Ok(DelayResult {
        audio,
        profile,
        state_owner,
        pending,
        sample_bytes,
        metadata_bytes,
        input_reserved_bytes,
        isolation_overhead: "unknown; not processing latency",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fresh() -> DelayRequest {
        DelayRequest {
            version: 3,
            profile: DELAY_PROFILE.into(),
            request: 9,
            sample_rate: 48000,
            start_frame: 0,
            initial_gain: 1.0,
            delay: 16,
            state: vec![],
            state_owner: None,
            blocks: vec![DelayBlock {
                drain: false,
                left: vec![1.0],
                right: vec![-1.0],
                points: vec![],
            }],
        }
    }
    fn owner() -> DelayOwner {
        DelayOwner {
            profile: DELAY_PROFILE.into(),
            module_sha256: "a".repeat(64),
            state_version: 3,
            sample_rate: 48000,
            delay: 16,
            next_frame: 1,
            max_tail: 16,
            drained: 0,
        }
    }
    fn state(o: &DelayOwner) -> Vec<u8> {
        let mut w = Writer::default();
        w.u32(0x444c5903);
        w.u32(o.sample_rate);
        w.u32(o.delay);
        w.u32((o.next_frame % u64::from(o.delay.max(1))) as u32);
        w.u64(o.next_frame);
        w.f64(1.0);
        for _ in 0..64 {
            w.u32(0);
        }
        w.u32(o.max_tail);
        w.u32(o.drained);
        w.bytes.extend(o.module_sha256.as_bytes());
        w.bytes
    }
    fn response(req: &DelayRequest) -> Vec<u8> {
        let mut w = Writer::default();
        w.u16(3);
        w.u64(req.request);
        for s in [SDK, "linux-x86_64", DELAY_PLUGIN_ID] {
            w.blob(s.as_bytes());
        }
        w.u32(48000);
        w.u64(0);
        w.u64(1);
        w.u32(16);
        w.u32(16);
        for n in [2, 2, 0, 0, 1, GAIN_ID, 32] {
            w.u32(n);
        }
        for n in [0.0, 2.0, 1.0] {
            w.f64(n);
        }
        w.blob(b"gain");
        w.u32(1);
        w.u32(0);
        w.u32(0);
        w.blob(&state(&owner()));
        w.u32(0);
        w.u32(0);
        w.u32(1);
        w.u32(1);
        w.blob(DELAY_PROFILE.as_bytes());
        w.u32(3);
        w.blob("a".repeat(64).as_bytes());
        for n in [0, 16, 256, 32] {
            w.u32(n);
        }
        w.u64(128 * 56);
        packet(b"HCOFFR03", w.bytes).unwrap()
    }
    #[test]
    fn bounded_framing_identity_and_all_truncations() {
        let req = fresh();
        let p = response(&req);
        assert!(decode_delay(&p, &req, &"a".repeat(64)).is_ok());
        for n in 0..p.len() {
            assert!(decode_delay(&p[..n], &req, &"a".repeat(64)).is_err());
        }
        let mut extra = p.clone();
        extra.push(0);
        assert!(decode_delay(&extra, &req, &"a".repeat(64)).is_err());
        assert!(decode_delay(&p, &req, &"b".repeat(64)).is_err());
        let encoded = req.encode_bound(&"a".repeat(64)).unwrap();
        assert_eq!(&encoded[..8], b"HCOFFQ03");
    }
    #[test]
    fn cumulative_drain_owner_bytes_cannot_reset_ledger() {
        let mut req = fresh();
        req.start_frame = 31;
        let mut o = owner();
        o.drained = 31;
        o.next_frame = 31;
        req.state = state(&o);
        req.state_owner = Some(o);
        req.blocks[0].drain = true;
        req.blocks[0].left = vec![0.0];
        req.blocks[0].right = vec![0.0];
        assert!(req.validate_bound(&"a".repeat(64)).is_ok());
        req.blocks[0].left.push(0.0);
        req.blocks[0].right.push(0.0);
        assert!(req.validate_bound(&"a".repeat(64)).is_err());
        req.state_owner.as_mut().unwrap().drained = 0;
        assert!(req.validate_bound(&"a".repeat(64)).is_err());
    }
    #[test]
    fn queue_cursor_nonfinite_version_and_unused_cells_refused() {
        let o = owner();
        let bytes = state(&o);
        assert!(validate_state(&bytes, &o).is_ok());
        for (at, word) in [
            (0, 1),
            (12, 16),
            (32, f32::INFINITY.to_bits()),
            (32 + 16 * 4, 1),
        ] {
            let mut bad = bytes.clone();
            bad[at..at + 4].copy_from_slice(&u32::to_le_bytes(word));
            assert!(validate_state(&bad, &o).is_err());
        }
    }
    #[test]
    fn drain_suffix_zero_events_and_aggregate_capacity_refused() {
        let mut req = fresh();
        req.blocks[0].drain = true;
        assert!(req.validate_bound(&"a".repeat(64)).is_err());
        req.blocks[0].left[0] = 0.0;
        req.blocks[0].right[0] = 0.0;
        assert!(req.validate_bound(&"a".repeat(64)).is_ok());
        req.blocks.push(fresh().blocks.remove(0));
        assert!(req.validate_bound(&"a".repeat(64)).is_err());
        let mut req = fresh();
        req.blocks = vec![req.blocks[0].clone(); 33];
        assert!(req.validate_bound(&"a".repeat(64)).is_err());
        let mut req = fresh();
        req.blocks[0].points = vec![
            Point {
                offset: 0,
                value: 1.0
            };
            128
        ];
        req.blocks = vec![req.blocks[0].clone(); 9];
        assert!(req.validate_bound(&"a".repeat(64)).is_err());
    }
}
