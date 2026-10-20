//! Ephemeral display metadata prepared from the same retained head twice.
//! This never claims/ACKs a head or proves source correlation/backend admission.
use hex_session::{
    service::Service,
    snapshot_exports::{SnapshotRead, SnapshotTransportError, SourceEventFacts, SourceEventHead},
};
use std::path::PathBuf;

#[derive(Debug)]
pub(super) enum PrepareError {
    Transport(SnapshotTransportError),
    MissingMetadata,
    HeadChanged,
    LengthOverflow,
    Allocation,
    Capacity,
    InvalidText,
}
impl From<SnapshotTransportError> for PrepareError {
    fn from(error: SnapshotTransportError) -> Self {
        Self::Transport(error)
    }
}
#[derive(Clone, Copy, Debug)]
pub(super) struct TextRange {
    start: usize,
    end: usize,
}
#[derive(Clone, Copy)]
pub(super) struct DeviceAck {
    pub key: hex_session::device_gesture::GestureKey,
    pub phase: hex_session::device_gesture::GesturePhase,
    pub sequence: u64,
    pub accepted: bool,
    pub reason: Option<TextRange>,
}
#[derive(Clone, Copy)]
pub(super) struct TuningAck {
    pub key: hex_session::tuning_gesture::TuningGestureKey,
    pub phase: hex_session::tuning_gesture::TuningGesturePhase,
    pub sequence: u64,
    pub accepted: bool,
    pub reason: Option<TextRange>,
}
#[derive(Clone, Copy)]
pub(super) struct Slot {
    pub number: usize,
    pub name: Option<TextRange>,
    pub error: Option<TextRange>,
}
#[derive(Clone, Copy)]
struct LibraryHeader {
    operation_id: u64,
    selected_slot: Option<usize>,
    loaded_name: Option<TextRange>,
    error: Option<TextRange>,
}
pub(super) struct Library {
    pub operation_id: u64,
    pub selected_slot: Option<usize>,
    pub loaded_name: Option<TextRange>,
    pub error: Option<TextRange>,
    pub slots: Vec<Slot>,
}
pub(super) struct BatchFailure {
    pub member: usize,
    pub path: PathBuf,
    pub explanation: TextRange,
}
#[derive(Clone, Copy)]
struct Header {
    facts: SourceEventFacts,
    revision: u64,
    timeline_frames: u64,
    sample_rate: u32,
    unsaved: bool,
    can_undo: bool,
    can_redo: bool,
    document_path: bool,
    status: Option<TextRange>,
    error: Option<TextRange>,
    batch: Option<(usize, TextRange)>,
    device_ack: Option<DeviceAck>,
    tuning_ack: Option<TuningAck>,
    library: Option<LibraryHeader>,
}
/// Plain display strings/paths, Copy response facts and copied slot labels only.
/// No project, source, PCM, compiled/input owner or additional capability.
pub(super) struct Notice {
    pub head: SourceEventHead,
    pub facts: SourceEventFacts,
    pub revision: u64,
    pub timeline_frames: u64,
    pub sample_rate: u32,
    pub unsaved: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub document_path: Option<PathBuf>,
    pub status: Option<TextRange>,
    pub error: Option<TextRange>,
    pub batch: Option<BatchFailure>,
    pub device_ack: Option<DeviceAck>,
    pub tuning_ack: Option<TuningAck>,
    pub library: Option<Library>,
    text: String,
}
impl Notice {
    pub fn text(&self, range: TextRange) -> Result<&str, PrepareError> {
        self.text
            .get(range.start..range.end)
            .ok_or(PrepareError::InvalidText)
    }
}
#[derive(Clone, Copy)]
struct Quote {
    head: SourceEventHead,
    text_bytes: usize,
    slots: usize,
    document_path_bytes: usize,
    batch_path_bytes: usize,
}
fn quote(head: SourceEventHead, snapshot: SnapshotRead<'_>) -> Result<Quote, PrepareError> {
    let event = snapshot
        .event_metadata()
        .ok_or(PrepareError::MissingMetadata)?;
    let mut text_bytes = 0usize;
    let mut add = |text: Option<&str>| -> Result<(), PrepareError> {
        if let Some(text) = text {
            text_bytes = text_bytes
                .checked_add(text.len())
                .ok_or(PrepareError::LengthOverflow)?;
        }
        Ok(())
    };
    add(event.status())?;
    add(event.error())?;
    add(event
        .audio_batch_failure()
        .map(|failure| failure.explanation()))?;
    add(event.gesture_ack().and_then(|ack| ack.reason()))?;
    add(event.tuning_gesture_ack().and_then(|ack| ack.reason()))?;
    let mut slots = 0;
    if let Some(library) = event.tuning_library() {
        add(library.loaded_name())?;
        add(library.error())?;
        slots = library.slots().len();
        for slot in library.slots() {
            add(slot.name())?;
            add(slot.error())?;
        }
    }
    Ok(Quote {
        head,
        text_bytes,
        slots,
        document_path_bytes: snapshot
            .document_path()
            .map_or(0, |path| path.as_os_str().len()),
        batch_path_bytes: event
            .audio_batch_failure()
            .map_or(0, |failure| failure.path().as_os_str().len()),
    })
}
struct Scratch {
    text: Vec<u8>,
    slots: Vec<Slot>,
    document_path: PathBuf,
    batch_path: PathBuf,
}
impl Scratch {
    fn reserve(quote: Quote) -> Result<Self, PrepareError> {
        let mut scratch = Self {
            text: Vec::new(),
            slots: Vec::new(),
            document_path: PathBuf::new(),
            batch_path: PathBuf::new(),
        };
        scratch
            .text
            .try_reserve_exact(quote.text_bytes)
            .map_err(|_| PrepareError::Allocation)?;
        scratch
            .slots
            .try_reserve_exact(quote.slots)
            .map_err(|_| PrepareError::Allocation)?;
        scratch
            .document_path
            .try_reserve_exact(quote.document_path_bytes)
            .map_err(|_| PrepareError::Allocation)?;
        scratch
            .batch_path
            .try_reserve_exact(quote.batch_path_bytes)
            .map_err(|_| PrepareError::Allocation)?;
        Ok(scratch)
    }
    fn text(&mut self, value: Option<&str>) -> Result<Option<TextRange>, PrepareError> {
        let Some(value) = value else {
            return Ok(None);
        };
        let start = self.text.len();
        let end = start
            .checked_add(value.len())
            .ok_or(PrepareError::LengthOverflow)?;
        if end > self.text.capacity() {
            return Err(PrepareError::Capacity);
        }
        self.text.extend_from_slice(value.as_bytes());
        Ok(Some(TextRange { start, end }))
    }
    fn fill(&mut self, quote: Quote, snapshot: SnapshotRead<'_>) -> Result<Header, PrepareError> {
        let event = snapshot
            .event_metadata()
            .ok_or(PrepareError::MissingMetadata)?;
        if let Some(path) = snapshot.document_path() {
            if path.as_os_str().len() > self.document_path.capacity() {
                return Err(PrepareError::Capacity);
            }
            self.document_path.push(path);
        }
        let status = self.text(event.status())?;
        let error = self.text(event.error())?;
        let batch = if let Some(failure) = event.audio_batch_failure() {
            if failure.path().as_os_str().len() > self.batch_path.capacity() {
                return Err(PrepareError::Capacity);
            }
            self.batch_path.push(failure.path());
            let explanation = self
                .text(Some(failure.explanation()))?
                .ok_or(PrepareError::MissingMetadata)?;
            Some((failure.member(), explanation))
        } else {
            None
        };
        let device_ack = if let Some(ack) = event.gesture_ack() {
            Some(DeviceAck {
                key: ack.key(),
                phase: ack.phase(),
                sequence: ack.sequence(),
                accepted: ack.accepted(),
                reason: self.text(ack.reason())?,
            })
        } else {
            None
        };
        let tuning_ack = if let Some(ack) = event.tuning_gesture_ack() {
            Some(TuningAck {
                key: ack.key(),
                phase: ack.phase(),
                sequence: ack.sequence(),
                accepted: ack.accepted(),
                reason: self.text(ack.reason())?,
            })
        } else {
            None
        };
        let library = if let Some(library) = event.tuning_library() {
            let header = LibraryHeader {
                operation_id: library.operation_id(),
                selected_slot: library.selected_slot(),
                loaded_name: self.text(library.loaded_name())?,
                error: self.text(library.error())?,
            };
            for slot in library.slots() {
                if self.slots.len() >= quote.slots || self.slots.len() == self.slots.capacity() {
                    return Err(PrepareError::Capacity);
                }
                let slot = Slot {
                    number: slot.number(),
                    name: self.text(slot.name())?,
                    error: self.text(slot.error())?,
                };
                self.slots.push(slot);
            }
            Some(header)
        } else {
            None
        };
        if self.text.len() != quote.text_bytes || self.slots.len() != quote.slots {
            return Err(PrepareError::Capacity);
        }
        Ok(Header {
            facts: event.facts(),
            revision: snapshot.revision(),
            timeline_frames: snapshot.timeline_frames(),
            sample_rate: snapshot.sample_rate(),
            unsaved: snapshot.unsaved(),
            can_undo: snapshot.can_undo(),
            can_redo: snapshot.can_redo(),
            document_path: snapshot.document_path().is_some(),
            status,
            error,
            batch,
            device_ack,
            tuning_ack,
            library,
        })
    }
}
/// Quote the retained head, reserve backing outside its guard, then fill only
/// that same head. Failure leaves its receiver/canonical pending claim untouched.
pub(super) fn prepare(service: &Service) -> Result<Option<Notice>, PrepareError> {
    let Some(quote) = service.inspect_event_head_with(quote)?.transpose()? else {
        return Ok(None);
    };
    let mut scratch = Scratch::reserve(quote)?;
    let header = service
        .inspect_event_head_with(|head, snapshot| {
            if head != quote.head {
                return Err(PrepareError::HeadChanged);
            }
            scratch.fill(quote, snapshot)
        })?
        .ok_or(PrepareError::HeadChanged)??;
    // Conversion, owned payload assembly and all later UI effects are outside read.
    let text = String::from_utf8(scratch.text).map_err(|_| PrepareError::InvalidText)?;
    Ok(Some(Notice {
        head: quote.head,
        facts: header.facts,
        revision: header.revision,
        timeline_frames: header.timeline_frames,
        sample_rate: header.sample_rate,
        unsaved: header.unsaved,
        can_undo: header.can_undo,
        can_redo: header.can_redo,
        document_path: header.document_path.then_some(scratch.document_path),
        status: header.status,
        error: header.error,
        batch: header.batch.map(|(member, explanation)| BatchFailure {
            member,
            path: scratch.batch_path,
            explanation,
        }),
        device_ack: header.device_ack,
        tuning_ack: header.tuning_ack,
        library: header.library.map(|library| Library {
            operation_id: library.operation_id,
            selected_slot: library.selected_slot,
            loaded_name: library.loaded_name,
            error: library.error,
            slots: scratch.slots,
        }),
        text,
    }))
}
