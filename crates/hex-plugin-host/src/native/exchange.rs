//! Shared non-RT control/worker exchange helpers used by both [`ControlClient`]
//! (out-of-band control socket) and [`Prepared`] (serialized worker socket).
//! Consolidates edit polling, controller parameter-page begin/poll, and live
//! state capture begin/poll without changing wire framing or error semantics.
use super::*;
use crate::protocol::{ControllerRequest, ParameterPageProgress};

pub(super) struct ExchangeChannel<'a> {
    pub channel: &'a mut Channel,
    pub packet: &'a mut Packet,
    pub config: &'a Configuration,
    pub cancellation: &'a Cancellation,
    pub exchange_limit: Duration,
}

#[derive(Clone, Copy)]
pub(super) enum EditSequenceMode {
    /// Serialized audio lane requires the exact current sequence.
    Exact(u64),
    /// Out-of-band control lane accepts any sequence `>= min_sequence`.
    AtLeast(u64),
}

#[allow(clippy::too_many_arguments)]
pub(super) fn poll_edits_exchange(
    io: &mut ExchangeChannel<'_>,
    sequence_mode: EditSequenceMode,
    operation: u64,
    previous: (u64, u64),
    recovery: bool,
    flush_idle: bool,
) -> Result<(protocol::RecoveryEdits, u64)> {
    let request_sequence = match sequence_mode {
        EditSequenceMode::Exact(seq) | EditSequenceMode::AtLeast(seq) => seq,
    };
    let request = if flush_idle {
        protocol::idle_editor_edit_request(io.config, request_sequence, operation)?
    } else if recovery {
        protocol::recovery_edit_request(io.config, request_sequence, operation)?
    } else {
        protocol::edit_request(io.config, request_sequence, operation)?
    };
    let until = end_after(io.exchange_limit)?;
    let token = io.cancellation.clone();
    io.channel.send(&request, until, &mut || token.check())?;
    io.channel.read(io.packet, until, &mut || token.check())?;
    if io.packet.magic() == b"HXVIER01" {
        return Err(Error::Worker(protocol::error(
            io.packet.body()?,
            io.config.binding,
        )?));
    }
    let magic = if recovery { b"HXVIGE02" } else { b"HXVIGE01" };
    if io.packet.magic() != magic {
        return Err(Error::Protocol("missing parameter edit acknowledgement"));
    }
    let body = io.packet.body()?;
    match (sequence_mode, recovery) {
        (EditSequenceMode::Exact(seq), true) => Ok((
            protocol::recovery_edits(body, io.config, seq, operation, previous)?,
            seq,
        )),
        (EditSequenceMode::Exact(seq), false) => Ok((
            protocol::RecoveryEdits {
                edits: protocol::parameter_edits(body, io.config, seq, operation, previous)?,
                pending_latency_restart: None,
            },
            seq,
        )),
        (EditSequenceMode::AtLeast(min_seq), true) => {
            protocol::recovery_edits_at_least(body, io.config, min_seq, operation, previous)
        }
        (EditSequenceMode::AtLeast(min_seq), false) => {
            let (edits, observed_sequence) =
                protocol::parameter_edits_at_least(body, io.config, min_seq, operation, previous)?;
            Ok((
                protocol::RecoveryEdits {
                    edits,
                    pending_latency_restart: None,
                },
                observed_sequence,
            ))
        }
    }
}

pub(super) fn parameter_page_exchange(
    io: &mut ExchangeChannel<'_>,
    sequence: u64,
    operation: u64,
    query: ControllerRequest,
    deadline: Instant,
    poll: bool,
) -> Result<ParameterPageProgress> {
    let request = protocol::parameter_page_request(io.config, sequence, operation, query, poll)?;
    let until = end_after(io.exchange_limit)?.min(deadline);
    let token = io.cancellation.clone();
    io.channel.send(&request, until, &mut || token.check())?;
    io.channel.read(io.packet, until, &mut || token.check())?;
    if io.packet.magic() == b"HXVIER01" {
        return Err(Error::Worker(protocol::error(
            io.packet.body()?,
            io.config.binding,
        )?));
    }
    if io.packet.magic() != b"HXVIMD03" {
        return Err(Error::Protocol("missing controller page acknowledgement"));
    }
    protocol::parameter_page(io.packet.body()?, io.config, sequence, operation, query)
}

pub(super) fn live_capture_exchange(
    io: &mut ExchangeChannel<'_>,
    sequence: u64,
    operation: u64,
    deadline: Instant,
    poll: bool,
) -> Result<Option<NativeDeviceState>> {
    let request = protocol::live_capture_request(io.config, sequence, operation, poll)?;
    let until = end_after(io.exchange_limit)?.min(deadline);
    let token = io.cancellation.clone();
    io.channel.send(&request, until, &mut || token.check())?;
    io.channel.read(io.packet, until, &mut || token.check())?;
    let reply = protocol::live_capture_reply(
        io.packet.magic(),
        io.packet.body()?,
        io.config,
        sequence,
        operation,
    );
    io.packet.shrink_to_pcm();
    // Cancellation wins over a decoded reply or refusal; a worker error or
    // malformed frame stays the terminal cause.
    if matches!(reply, Ok(_) | Err(Error::CaptureRefused(_))) {
        token.check()?;
    }
    reply
}
