//! Disposable contained-runner child binary for `#2990` ABI qualification.
//! Loads a single detached C ABI `.dylib` and serves bounded wire requests over
//! stdin/stdout so crashes, hangs, and teardown stalls remain process-isolated.
#![deny(unsafe_code)]

use hex_device_abi_qual::{InProcessInstance, InProcessModule};
use hex_lattice_contract::package_abi::{
    AbiNoteEvent, AbiParamEvent, AbiStatus, AbiWireReplyHeader, AbiWireRequestHeader,
    PACKAGE_ABI_MAGIC, PACKAGE_ABI_MAX_FRAMES, PACKAGE_ABI_MAX_NOTE_EVENTS,
    PACKAGE_ABI_MAX_PARAM_EVENTS, PACKAGE_ABI_VERSION_V1, WIRE_OP_CREATE, WIRE_OP_DESCRIBE,
    WIRE_OP_DESTROY, WIRE_OP_PROCESS, WIRE_OP_SHUTDOWN,
};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::PathBuf;

fn send_reply(
    stdout: &mut io::StdoutLock<'_>,
    status: AbiStatus,
    instance_id: u32,
    payload: &[u8],
) -> io::Result<()> {
    let hdr = AbiWireReplyHeader {
        magic: PACKAGE_ABI_MAGIC,
        abi_version: PACKAGE_ABI_VERSION_V1,
        status: status as i32,
        instance_id,
        payload_bytes: payload.len() as u32,
        reserved: 0,
    };
    stdout.write_all(&hdr.encode())?;
    if !payload.is_empty() {
        stdout.write_all(payload)?;
    }
    stdout.flush()
}

fn main() {
    let Some(path_arg) = std::env::args_os().nth(1) else {
        std::process::exit(2);
    };
    let dylib_path = PathBuf::from(path_arg);
    let module = match InProcessModule::load(&dylib_path) {
        Ok(m) => m,
        Err(_) => std::process::exit(3),
    };

    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    let mut instances: HashMap<u32, InProcessInstance> = HashMap::new();
    let mut next_id = 1u32;

    let mut in_buf = vec![[0.0f32; 2]; PACKAGE_ABI_MAX_FRAMES as usize];
    let mut out_buf = vec![[0.0f32; 2]; PACKAGE_ABI_MAX_FRAMES as usize];
    let mut param_events = Vec::with_capacity(PACKAGE_ABI_MAX_PARAM_EVENTS as usize);
    let mut note_events = Vec::with_capacity(PACKAGE_ABI_MAX_NOTE_EVENTS as usize);
    let mut reply_pcm = Vec::with_capacity(PACKAGE_ABI_MAX_FRAMES as usize * 8);

    loop {
        let mut hdr_bytes = [0u8; AbiWireRequestHeader::BYTE_LEN];
        if stdin.read_exact(&mut hdr_bytes).is_err() {
            break;
        }
        let req = match AbiWireRequestHeader::decode(&hdr_bytes) {
            Ok(r) => r,
            Err(status) => {
                let _ = send_reply(&mut stdout, status, 0, &[]);
                break;
            }
        };
        let mut payload = vec![0u8; req.payload_bytes as usize];
        if !payload.is_empty() && stdin.read_exact(&mut payload).is_err() {
            break;
        }

        match req.opcode {
            WIRE_OP_DESCRIBE => {
                let desc = module.descriptor();
                let len = desc.type_id_len as usize;
                let _ = send_reply(&mut stdout, AbiStatus::Ok, 0, &desc.type_id[..len]);
            }
            WIRE_OP_CREATE => {
                if payload.len() != 4 {
                    let _ = send_reply(&mut stdout, AbiStatus::InvalidArg, 0, &[]);
                    continue;
                }
                let rate = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
                match module.create_instance(rate) {
                    Ok(inst) => {
                        let id = next_id;
                        next_id = next_id.saturating_add(1);
                        instances.insert(id, inst);
                        let _ = send_reply(&mut stdout, AbiStatus::Ok, id, &[]);
                    }
                    Err(status) => {
                        let _ = send_reply(&mut stdout, status, 0, &[]);
                    }
                }
            }
            WIRE_OP_PROCESS => {
                if payload.len() < 16 {
                    let _ = send_reply(&mut stdout, AbiStatus::InvalidArg, req.instance_id, &[]);
                    continue;
                }
                let _rate = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
                let frames =
                    u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]) as usize;
                let p_count =
                    u32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]) as usize;
                let n_count =
                    u32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]])
                        as usize;
                let expected = 16 + frames * 8 + p_count * 12 + n_count * 20;
                if frames == 0
                    || frames > PACKAGE_ABI_MAX_FRAMES as usize
                    || p_count > PACKAGE_ABI_MAX_PARAM_EVENTS as usize
                    || n_count > PACKAGE_ABI_MAX_NOTE_EVENTS as usize
                    || payload.len() != expected
                {
                    let _ = send_reply(&mut stdout, AbiStatus::InvalidArg, req.instance_id, &[]);
                    continue;
                }
                let mut cursor = 16;
                for frame in &mut in_buf[..frames] {
                    let l = f32::from_le_bytes([
                        payload[cursor],
                        payload[cursor + 1],
                        payload[cursor + 2],
                        payload[cursor + 3],
                    ]);
                    let r = f32::from_le_bytes([
                        payload[cursor + 4],
                        payload[cursor + 5],
                        payload[cursor + 6],
                        payload[cursor + 7],
                    ]);
                    *frame = [l, r];
                    cursor += 8;
                }
                param_events.clear();
                for _ in 0..p_count {
                    let sample_offset = u32::from_le_bytes([
                        payload[cursor],
                        payload[cursor + 1],
                        payload[cursor + 2],
                        payload[cursor + 3],
                    ]);
                    let parameter_id = u32::from_le_bytes([
                        payload[cursor + 4],
                        payload[cursor + 5],
                        payload[cursor + 6],
                        payload[cursor + 7],
                    ]);
                    let value = f32::from_le_bytes([
                        payload[cursor + 8],
                        payload[cursor + 9],
                        payload[cursor + 10],
                        payload[cursor + 11],
                    ]);
                    param_events.push(AbiParamEvent {
                        sample_offset,
                        parameter_id,
                        value,
                        reserved: 0,
                    });
                    cursor += 12;
                }
                note_events.clear();
                for _ in 0..n_count {
                    let sample_offset = u32::from_le_bytes([
                        payload[cursor],
                        payload[cursor + 1],
                        payload[cursor + 2],
                        payload[cursor + 3],
                    ]);
                    let kind = u32::from_le_bytes([
                        payload[cursor + 4],
                        payload[cursor + 5],
                        payload[cursor + 6],
                        payload[cursor + 7],
                    ]);
                    let note = payload[cursor + 8];
                    let channel = payload[cursor + 9];
                    let velocity = f32::from_le_bytes([
                        payload[cursor + 12],
                        payload[cursor + 13],
                        payload[cursor + 14],
                        payload[cursor + 15],
                    ]);
                    let detune_cents = f32::from_le_bytes([
                        payload[cursor + 16],
                        payload[cursor + 17],
                        payload[cursor + 18],
                        payload[cursor + 19],
                    ]);
                    note_events.push(AbiNoteEvent {
                        sample_offset,
                        kind,
                        note,
                        channel,
                        reserved0: 0,
                        velocity,
                        detune_cents,
                        reserved1: 0,
                    });
                    cursor += 20;
                }

                let Some(inst) = instances.get_mut(&req.instance_id) else {
                    let _ = send_reply(&mut stdout, AbiStatus::InvalidArg, req.instance_id, &[]);
                    continue;
                };
                match inst.process(
                    &in_buf[..frames],
                    &mut out_buf[..frames],
                    &param_events,
                    &note_events,
                ) {
                    Ok(()) => {
                        reply_pcm.clear();
                        for &[l, r] in &out_buf[..frames] {
                            reply_pcm.extend_from_slice(&l.to_le_bytes());
                            reply_pcm.extend_from_slice(&r.to_le_bytes());
                        }
                        let _ = send_reply(&mut stdout, AbiStatus::Ok, req.instance_id, &reply_pcm);
                    }
                    Err(status) => {
                        let _ = send_reply(&mut stdout, status, req.instance_id, &[]);
                    }
                }
            }
            WIRE_OP_DESTROY => {
                if instances.remove(&req.instance_id).is_some() {
                    let _ = send_reply(&mut stdout, AbiStatus::Ok, req.instance_id, &[]);
                } else {
                    let _ = send_reply(&mut stdout, AbiStatus::InvalidArg, req.instance_id, &[]);
                }
            }
            WIRE_OP_SHUTDOWN => {
                instances.clear();
                let _ = send_reply(&mut stdout, AbiStatus::Ok, 0, &[]);
                break;
            }
            _ => {
                let _ = send_reply(&mut stdout, AbiStatus::InvalidArg, 0, &[]);
            }
        }
    }
}
