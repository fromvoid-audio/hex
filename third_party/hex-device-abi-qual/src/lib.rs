//! Qualification harness for Hex installable native device packages (`#2990`).
//!
//! Excluded from the main Hex workspace so workspace `unsafe_code = "forbid"`
//! remains untouched and arbitrary production loading is NOT enabled.
//! Compares three execution modes over the exact same fixed-width C ABI contract:
//! 1. Same-source static Rust baselines (`StaticUtility`, `StaticSine`).
//! 2. Narrowly audited external in-process C ABI loader (`InProcessModule`).
//! 3. Contained-runner subprocess transport (`ContainedRunner`) over fixed-width
//!    `AbiWireRequestHeader`/`AbiWireReplyHeader` frames with hard timeouts and
//!    crash/hang/teardown isolation.
#![deny(unsafe_code)]

mod raw;

use hex_lattice_contract::package_abi::{
    AbiDescriptor, AbiNoteEvent, AbiParamEvent, AbiParameterDescriptor, AbiStatus,
    AbiWireReplyHeader, AbiWireRequestHeader, NOTE_EVENT_OFF, NOTE_EVENT_ON, PACKAGE_ABI_MAGIC,
    PACKAGE_ABI_MAX_FRAMES, PACKAGE_ABI_VERSION_V1, WIRE_OP_CREATE, WIRE_OP_DESCRIBE,
    WIRE_OP_DESTROY, WIRE_OP_PROCESS, WIRE_OP_SHUTDOWN, validate_pcm_output,
};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Narrowly audited external in-process C ABI module handle.
pub struct InProcessModule(raw::RawModule);

/// Narrowly audited external in-process C ABI device instance.
pub struct InProcessInstance(raw::RawInstance);

impl InProcessModule {
    pub fn load(path: &Path) -> Result<Self, AbiStatus> {
        raw::RawModule::load(path).map(Self)
    }

    pub fn descriptor(&self) -> AbiDescriptor {
        self.0.descriptor()
    }

    pub fn parameter_descriptor(&self, index: u32) -> Result<AbiParameterDescriptor, AbiStatus> {
        self.0.parameter_descriptor(index)
    }

    pub fn create_instance(&self, sample_rate: u32) -> Result<InProcessInstance, AbiStatus> {
        self.0.create_instance(sample_rate).map(InProcessInstance)
    }
}

impl InProcessInstance {
    pub fn set_state(&mut self, bytes: &[u8]) -> Result<(), AbiStatus> {
        self.0.set_state(bytes)
    }

    pub fn get_state(&self, out: &mut [u8]) -> Result<usize, AbiStatus> {
        self.0.get_state(out)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &mut self,
        input: &[[f32; 2]],
        output: &mut [[f32; 2]],
        param_events: &[AbiParamEvent],
        note_events: &[AbiNoteEvent],
    ) -> Result<(), AbiStatus> {
        self.0.process(input, output, param_events, note_events)
    }
}

/// Errors reported by the contained-runner subprocess boundary.
#[derive(Debug, PartialEq, Eq)]
pub enum ContainedError {
    Spawn(String),
    Abi(AbiStatus),
    Timeout,
    Crashed,
    Protocol(&'static str),
}

/// Contained subprocess host executing a detached C ABI `.dylib` behind
/// fixed-width wire framing with bounded timeouts and forced kill on hang.
pub struct ContainedRunner {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    reply_rx: mpsc::Receiver<Result<(AbiWireReplyHeader, Vec<u8>), ContainedError>>,
}

impl ContainedRunner {
    pub fn spawn(runner_bin: &Path, dylib_path: &Path) -> Result<Self, ContainedError> {
        let mut child = Command::new(runner_bin)
            .arg(dylib_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| ContainedError::Spawn(e.to_string()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or(ContainedError::Protocol("missing stdin"))?;
        let mut stdout: ChildStdout = child
            .stdout
            .take()
            .ok_or(ContainedError::Protocol("missing stdout"))?;

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            loop {
                let mut hdr_bytes = [0u8; AbiWireReplyHeader::BYTE_LEN];
                if stdout.read_exact(&mut hdr_bytes).is_err() {
                    let _ = tx.send(Err(ContainedError::Crashed));
                    break;
                }
                let hdr = match AbiWireReplyHeader::decode(&hdr_bytes) {
                    Ok(h) => h,
                    Err(status) => {
                        let _ = tx.send(Err(ContainedError::Abi(status)));
                        break;
                    }
                };
                let mut payload = vec![0u8; hdr.payload_bytes as usize];
                if !payload.is_empty() && stdout.read_exact(&mut payload).is_err() {
                    let _ = tx.send(Err(ContainedError::Crashed));
                    break;
                }
                if tx.send(Ok((hdr, payload))).is_err() {
                    break;
                }
            }
        });

        Ok(Self {
            child: Some(child),
            stdin: Some(stdin),
            reply_rx: rx,
        })
    }

    fn exchange(
        &mut self,
        opcode: u32,
        instance_id: u32,
        payload: &[u8],
        timeout: Duration,
    ) -> Result<(AbiWireReplyHeader, Vec<u8>), ContainedError> {
        let hdr = AbiWireRequestHeader {
            magic: PACKAGE_ABI_MAGIC,
            abi_version: PACKAGE_ABI_VERSION_V1,
            opcode,
            instance_id,
            payload_bytes: u32::try_from(payload.len())
                .map_err(|_| ContainedError::Protocol("payload overflow"))?,
            reserved: 0,
        };
        let stdin = self
            .stdin
            .as_mut()
            .ok_or(ContainedError::Protocol("runner closed"))?;
        if stdin.write_all(&hdr.encode()).is_err()
            || (!payload.is_empty() && stdin.write_all(payload).is_err())
            || stdin.flush().is_err()
        {
            self.force_kill();
            return Err(ContainedError::Crashed);
        }
        match self.reply_rx.recv_timeout(timeout) {
            Ok(Ok((reply, body))) => {
                let status = AbiStatus::from_i32(reply.status);
                if status != AbiStatus::Ok {
                    return Err(ContainedError::Abi(status));
                }
                Ok((reply, body))
            }
            Ok(Err(err)) => {
                self.force_kill();
                Err(err)
            }
            Err(_) => {
                self.force_kill();
                Err(ContainedError::Timeout)
            }
        }
    }

    pub fn describe(&mut self, timeout: Duration) -> Result<String, ContainedError> {
        let (_hdr, body) = self.exchange(WIRE_OP_DESCRIBE, 0, &[], timeout)?;
        String::from_utf8(body).map_err(|_| ContainedError::Protocol("non-utf8 type_id"))
    }

    pub fn create(&mut self, sample_rate: u32, timeout: Duration) -> Result<u32, ContainedError> {
        let (hdr, _body) = self.exchange(WIRE_OP_CREATE, 0, &sample_rate.to_le_bytes(), timeout)?;
        Ok(hdr.instance_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &mut self,
        instance_id: u32,
        sample_rate: u32,
        input: &[[f32; 2]],
        output: &mut [[f32; 2]],
        param_events: &[AbiParamEvent],
        note_events: &[AbiNoteEvent],
        timeout: Duration,
    ) -> Result<(), ContainedError> {
        if input.len() != output.len()
            || output.is_empty()
            || output.len() > PACKAGE_ABI_MAX_FRAMES as usize
        {
            return Err(ContainedError::Abi(AbiStatus::InvalidFrames));
        }
        let mut req = Vec::with_capacity(
            16 + input.len() * 8 + param_events.len() * 12 + note_events.len() * 20,
        );
        req.extend_from_slice(&sample_rate.to_le_bytes());
        req.extend_from_slice(&(output.len() as u32).to_le_bytes());
        req.extend_from_slice(&(param_events.len() as u32).to_le_bytes());
        req.extend_from_slice(&(note_events.len() as u32).to_le_bytes());
        for &[l, r] in input {
            req.extend_from_slice(&l.to_le_bytes());
            req.extend_from_slice(&r.to_le_bytes());
        }
        for ev in param_events {
            req.extend_from_slice(&ev.sample_offset.to_le_bytes());
            req.extend_from_slice(&ev.parameter_id.to_le_bytes());
            req.extend_from_slice(&ev.value.to_le_bytes());
        }
        for ev in note_events {
            req.extend_from_slice(&ev.sample_offset.to_le_bytes());
            req.extend_from_slice(&ev.kind.to_le_bytes());
            req.push(ev.note);
            req.push(ev.channel);
            req.extend_from_slice(&0u16.to_le_bytes());
            req.extend_from_slice(&ev.velocity.to_le_bytes());
            req.extend_from_slice(&ev.detune_cents.to_le_bytes());
        }

        let (_hdr, body) = self.exchange(WIRE_OP_PROCESS, instance_id, &req, timeout)?;
        if body.len() != output.len() * 8 {
            output.fill([0.0; 2]);
            return Err(ContainedError::Protocol("reply frame byte count mismatch"));
        }
        for (i, chunk) in body.chunks_exact(8).enumerate() {
            let l = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            let r = f32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
            output[i] = [l, r];
        }
        if let Err(status) = validate_pcm_output(output) {
            output.fill([0.0; 2]);
            return Err(ContainedError::Abi(status));
        }
        Ok(())
    }

    pub fn destroy(&mut self, instance_id: u32, timeout: Duration) -> Result<(), ContainedError> {
        let _ = self.exchange(WIRE_OP_DESTROY, instance_id, &[], timeout)?;
        Ok(())
    }

    pub fn shutdown_bounded(&mut self, timeout: Duration) -> Result<(), ContainedError> {
        let start = Instant::now();
        let res = self.exchange(WIRE_OP_SHUTDOWN, 0, &[], timeout);
        self.stdin.take();
        if let Some(mut child) = self.child.take() {
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if start.elapsed() < timeout => {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(ContainedError::Timeout);
                    }
                }
            }
        }
        res.map(|_| ())
    }

    pub fn force_kill(&mut self) {
        self.stdin.take();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for ContainedRunner {
    fn drop(&mut self) {
        self.force_kill();
    }
}

/// Same-source static Rust Utility baseline matching `fixtures/utility_v1.c`.
#[derive(Clone, Copy)]
pub struct StaticUtility {
    gain: f32,
    balance: f32,
    width: f32,
    invert_l: bool,
    mute: bool,
}

impl Default for StaticUtility {
    fn default() -> Self {
        Self {
            gain: 1.0,
            balance: 0.0,
            width: 1.0,
            invert_l: false,
            mute: false,
        }
    }
}

impl StaticUtility {
    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &mut self,
        input: &[[f32; 2]],
        output: &mut [[f32; 2]],
        param_events: &[AbiParamEvent],
    ) {
        let mut ev_idx = 0usize;
        for (i, &[mut l, mut r]) in input.iter().enumerate() {
            while ev_idx < param_events.len() && param_events[ev_idx].sample_offset as usize <= i {
                let ev = param_events[ev_idx];
                if ev.value.is_finite() {
                    match ev.parameter_id {
                        1 if (0.0..=4.0).contains(&ev.value) => self.gain = ev.value,
                        2 if (-1.0..=1.0).contains(&ev.value) => self.balance = ev.value,
                        3 if (0.0..=2.0).contains(&ev.value) => self.width = ev.value,
                        4 => self.invert_l = ev.value >= 0.5,
                        5 => self.mute = ev.value >= 0.5,
                        _ => {}
                    }
                }
                ev_idx += 1;
            }
            if self.mute {
                output[i] = [0.0, 0.0];
                continue;
            }
            if self.invert_l {
                l = -l;
            }
            let mid = 0.5 * (l + r);
            let side = 0.5 * (l - r) * self.width;
            l = mid + side;
            r = mid - side;
            let gain_l = self.gain
                * if self.balance > 0.0 {
                    1.0 - self.balance
                } else {
                    1.0
                };
            let gain_r = self.gain
                * if self.balance < 0.0 {
                    1.0 + self.balance
                } else {
                    1.0
                };
            output[i] = [l * gain_l, r * gain_r];
        }
    }
}

#[derive(Clone, Copy, Default)]
struct StaticSineVoice {
    active: bool,
    note: u8,
    channel: u8,
    velocity: f32,
    phase: f64,
    phase_inc: f64,
}

/// Same-source static Rust Sine baseline matching `fixtures/sine_v1.c`.
#[derive(Clone, Copy)]
pub struct StaticSine {
    sample_rate: u32,
    gain: f32,
    voices: [StaticSineVoice; 16],
}

impl StaticSine {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            gain: 0.5,
            voices: [StaticSineVoice::default(); 16],
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &mut self,
        output: &mut [[f32; 2]],
        param_events: &[AbiParamEvent],
        note_events: &[AbiNoteEvent],
    ) {
        const TWO_PI: f64 = std::f64::consts::TAU;
        let mut p_idx = 0usize;
        let mut n_idx = 0usize;
        for (i, frame) in output.iter_mut().enumerate() {
            while p_idx < param_events.len() && param_events[p_idx].sample_offset as usize <= i {
                let ev = param_events[p_idx];
                if ev.parameter_id == 1 && ev.value.is_finite() && (0.0..=2.0).contains(&ev.value) {
                    self.gain = ev.value;
                }
                p_idx += 1;
            }
            while n_idx < note_events.len() && note_events[n_idx].sample_offset as usize <= i {
                let ev = note_events[n_idx];
                if ev.kind == NOTE_EVENT_ON {
                    let slot = self.voices.iter().position(|v| !v.active).unwrap_or(0);
                    let detune = if ev.detune_cents.is_finite() {
                        f64::from(ev.detune_cents)
                    } else {
                        0.0
                    };
                    let semitones = (f64::from(ev.note) - 69.0) + detune / 100.0;
                    let hz = 440.0 * 2.0_f64.powf(semitones / 12.0);
                    self.voices[slot] = StaticSineVoice {
                        active: true,
                        note: ev.note,
                        channel: ev.channel,
                        velocity: if ev.velocity.is_finite() {
                            ev.velocity
                        } else {
                            0.0
                        },
                        phase: 0.0,
                        phase_inc: (TWO_PI * hz) / f64::from(self.sample_rate),
                    };
                } else if ev.kind == NOTE_EVENT_OFF {
                    for v in &mut self.voices {
                        if v.active && v.note == ev.note && v.channel == ev.channel {
                            v.active = false;
                        }
                    }
                }
                n_idx += 1;
            }
            let mut sum = 0.0_f64;
            for v in &mut self.voices {
                if v.active {
                    sum += v.phase.sin() * f64::from(v.velocity);
                    v.phase += v.phase_inc;
                    if v.phase >= TWO_PI {
                        v.phase -= TWO_PI;
                    }
                }
            }
            let s = (sum * f64::from(self.gain)) as f32;
            *frame = [s, s];
        }
    }
}

/// Compile a standalone C99 fixture `.c` into an independently built native
/// `.dylib` outside Cargo's host build.
pub fn compile_c_fixture(
    c_source: &Path,
    include_dir: &Path,
    out_dylib: &Path,
    extra_defines: &[&str],
) -> Result<PathBuf, String> {
    let mut cmd = Command::new("clang");
    cmd.args([
        "-std=c99", "-O2", "-Wall", "-Wextra", "-Werror", "-shared", "-fPIC",
    ])
    .arg("-I")
    .arg(include_dir);
    for def in extra_defines {
        cmd.arg(format!("-D{def}"));
    }
    cmd.arg(c_source).arg("-o").arg(out_dylib).arg("-lm");
    let output = cmd
        .output()
        .map_err(|e| format!("clang spawn failed: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "clang failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(out_dylib.to_path_buf())
}
