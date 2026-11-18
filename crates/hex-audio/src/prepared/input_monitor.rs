//! One exact-source, single-track live input monitor for resident playback.
//!
//! Control publishes at most one owner; a callback reads at most 256 frames
//! and returns the owner through a one-slot queue. A full or abandoned return
//! queue retains it until control-side callback destruction. Stop bypasses the
//! queue; a block already being rendered may finish. Priming and terminal
//! errors leave ordinary playback intact. The playing mode borrows the active
//! renderer. The paused mode carries separate processor history and is bound
//! to a transport epoch as well as an exact resident source. Input and output
//! rates must match; independent device-clock drift is not corrected.
use super::AppliedAudioSource;
use crate::input_capture::{MonitorBlock, MonitorStream, MonitorStreamError};
use hex_engine::{Frame, OutputCaptureId, PausedTrackMonitor, playback::Playback};
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::sync::{
    Arc,
    atomic::{
        AtomicBool, AtomicU8,
        Ordering::{Acquire, Release},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MonitorStatus {
    Idle = 0,
    Pending = 1,
    Priming = 2,
    Running = 3,
    Stopped = 4,
    SourceChanged = 5,
    Unsupported = 6,
    InputEnded = 7,
    Underrun = 8,
    Overrun = 9,
    Discontinuity = 10,
    OutputFault = 11,
}
impl MonitorStatus {
    fn decode(value: u8) -> Self {
        match value {
            1 => Self::Pending,
            2 => Self::Priming,
            3 => Self::Running,
            4 => Self::Stopped,
            5 => Self::SourceChanged,
            6 => Self::Unsupported,
            7 => Self::InputEnded,
            8 => Self::Underrun,
            9 => Self::Overrun,
            10 => Self::Discontinuity,
            11 => Self::OutputFault,
            _ => Self::Idle,
        }
    }
}
struct Binding {
    stream: Box<MonitorStream>,
    source: AppliedAudioSource,
    mode: Mode,
}
enum Mode {
    Playing(OutputCaptureId),
    Paused {
        monitor: PausedTrackMonitor,
        transport_epoch: u64,
    },
}
/// One-slot monitor publisher. Rejected publications retain their owner.
pub struct Control {
    incoming: Producer<Box<Binding>>,
    retired: Consumer<Box<Binding>>,
    stop: Arc<AtomicBool>,
    state: Arc<AtomicU8>,
    occupied: bool,
    rate: u32,
}
pub(super) struct Callback {
    incoming: Consumer<Box<Binding>>,
    retired: Producer<Box<Binding>>,
    active: Option<Box<Binding>>,
    stop: Arc<AtomicBool>,
    state: Arc<AtomicU8>,
}
/// An installation refusal returns the original monitor stream on control.
pub struct Refusal {
    pub stream: Box<MonitorStream>,
}

/// A paused installation refusal returns both original owners on control.
pub struct PausedRefusal {
    pub stream: Box<MonitorStream>,
    pub monitor: PausedTrackMonitor,
}

pub(super) fn channel(rate: u32) -> (Control, Callback) {
    let (incoming, receiver) = RingBuffer::new(1);
    let (returns, retired) = RingBuffer::new(1);
    let stop = Arc::new(AtomicBool::new(false));
    let state = Arc::new(AtomicU8::new(MonitorStatus::Idle as u8));
    (
        Control {
            incoming,
            retired,
            stop: Arc::clone(&stop),
            state: Arc::clone(&state),
            occupied: false,
            rate,
        },
        Callback {
            incoming: receiver,
            retired: returns,
            active: None,
            stop,
            state,
        },
    )
}
pub(super) fn constructor_storage_quote() -> Option<usize> {
    use crate::native_callback_owner::ChannelStorage;
    ChannelStorage::ring_bytes::<Box<Binding>>(1)?
        .checked_mul(2)?
        .checked_add(std::mem::size_of::<AtomicBool>())?
        .checked_add(std::mem::size_of::<AtomicU8>())
}

impl Control {
    /// Install one endpoint for an exact applied graph. Call only on control.
    pub fn publish(
        &mut self,
        stream: Box<MonitorStream>,
        source: AppliedAudioSource,
        track: OutputCaptureId,
    ) -> Result<(), Refusal> {
        if !matches!(track, OutputCaptureId::Track(_)) {
            return Err(Refusal { stream });
        }
        self.publish_binding(stream, source, track, Mode::Playing)
            .map_err(|(stream, _)| Refusal { stream })
    }

    /// Install a control-prepared monitor for a settled pause. Use the source and
    /// transport epoch of that activation. The callback revalidates both and
    /// confirms the monitor's actual prepared revision before producing audio.
    /// The caller must budget the monitor runtime and keep its prepared data
    /// accounted for until reclaim or control-side shutdown.
    pub fn publish_paused(
        &mut self,
        stream: Box<MonitorStream>,
        monitor: PausedTrackMonitor,
        source: AppliedAudioSource,
        transport_epoch: u64,
    ) -> Result<(), PausedRefusal> {
        self.publish_binding(stream, source, monitor, |monitor| Mode::Paused {
            monitor,
            transport_epoch,
        })
        .map_err(|(stream, monitor)| PausedRefusal { stream, monitor })
    }

    fn publish_binding<T>(
        &mut self,
        stream: Box<MonitorStream>,
        source: AppliedAudioSource,
        owner: T,
        mode: impl FnOnce(T) -> Mode,
    ) -> Result<(), (Box<MonitorStream>, T)> {
        if self.occupied
            || self.incoming.is_abandoned()
            || stream.source().1.sample_rate != self.rate
        {
            return Err((stream, owner));
        }
        let Ok(slot) = self.incoming.write_chunk_uninit(1) else {
            return Err((stream, owner));
        };
        let binding = Box::new(Binding {
            stream,
            source,
            mode: mode(owner),
        });
        self.stop.store(false, Release);
        self.state.store(MonitorStatus::Pending as u8, Release);
        slot.fill_from_iter(std::iter::once(binding));
        self.occupied = true;
        Ok(())
    }
    /// Independent of queue pressure; the callback stops using the owner.
    pub fn stop(&self) {
        self.stop.store(true, Release);
    }
    pub fn status(&self) -> MonitorStatus {
        MonitorStatus::decode(self.state.load(Acquire))
    }
    /// Reclaim the original stream and dispose of any paused renderer on control.
    pub fn reclaim(&mut self) -> Option<Box<MonitorStream>> {
        let binding = self.retired.pop().ok()?;
        self.occupied = false;
        Some(binding.stream)
    }
}
impl Drop for Control {
    fn drop(&mut self) {
        self.stop();
    }
}
impl Callback {
    pub(super) fn constructor_channel_storage(
        &self,
    ) -> Option<crate::native_callback_owner::ChannelStorage> {
        use crate::native_callback_owner::ChannelStorage;
        ChannelStorage::ring(self.incoming.buffer())?
            .plus(ChannelStorage::ring(self.retired.buffer())?)?
            .plus(ChannelStorage::paired_payload(self.stop.as_ref()))?
            .plus(ChannelStorage::paired_payload(self.state.as_ref()))
    }
    fn retire(&mut self) {
        if self.retired.is_abandoned() || self.retired.is_full() {
            return;
        }
        if let Some(binding) = self.active.take()
            && let Err(PushError::Full(binding)) = self.retired.push(binding)
        {
            self.active = Some(binding);
        }
    }
    pub(super) fn begin_block(&mut self, closing: bool, fault: bool) {
        if closing || fault || self.stop.load(Acquire) {
            if matches!(
                MonitorStatus::decode(self.state.load(Acquire)),
                MonitorStatus::Pending | MonitorStatus::Priming | MonitorStatus::Running
            ) {
                self.state.store(
                    if fault {
                        MonitorStatus::OutputFault
                    } else {
                        MonitorStatus::Stopped
                    } as u8,
                    Release,
                );
            }
            self.retire();
        }
        if self.active.is_none()
            && !self.retired.is_abandoned()
            && !self.retired.is_full()
            && let Ok(binding) = self.incoming.pop()
        {
            self.active = Some(binding);
            if closing || fault || self.stop.load(Acquire) {
                self.retire();
            }
        }
    }
    pub(super) fn reject(&mut self, reason: MonitorStatus) {
        if self.active.is_some() {
            self.state.store(reason as u8, Release);
            self.stop.store(true, Release);
        }
    }
    pub(super) fn target(&self) -> Option<OutputCaptureId> {
        match self.active.as_ref()?.mode {
            Mode::Playing(track) => Some(track),
            Mode::Paused { .. } => None,
        }
    }
    pub(super) fn reject_paused(&mut self, reason: MonitorStatus) {
        if !self.stop.load(Acquire) && self.has_paused_binding() {
            self.reject(reason);
        }
    }
    pub(super) fn has_paused_binding(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|binding| matches!(binding.mode, Mode::Paused { .. }))
    }
    fn source_matches(&mut self, source: Option<AppliedAudioSource>) -> bool {
        if self.stop.load(Acquire) {
            return false;
        }
        if self
            .active
            .as_ref()
            .is_some_and(|binding| Some(binding.source) != source)
        {
            self.reject(MonitorStatus::SourceChanged);
            return false;
        }
        true
    }
    /// Return a borrowed track and a complete block, or keep ordinary playback.
    pub(super) fn read(
        &mut self,
        source: Option<AppliedAudioSource>,
        supported: bool,
        playing: bool,
        output: &mut [Frame],
    ) -> Option<OutputCaptureId> {
        let track = self.target()?;
        if !self.source_matches(source) {
            return None;
        }
        if !supported || !playing {
            self.reject(MonitorStatus::Unsupported);
            return None;
        }
        if !self.read_stream(output) {
            return None;
        }
        self.state.store(MonitorStatus::Running as u8, Release);
        Some(track)
    }

    /// On success only the independently owned monitor rendered this block.
    /// A refusal leaves the ordinary runtime free to render its own output.
    pub(super) fn render_paused(
        &mut self,
        source: Option<AppliedAudioSource>,
        epoch: u64,
        playback: Option<&Playback>,
        output: &mut [Frame],
    ) -> bool {
        let Some(Mode::Paused {
            transport_epoch, ..
        }) = self.active.as_ref().map(|binding| &binding.mode)
        else {
            return false;
        };
        let bound_epoch = *transport_epoch;
        if !self.source_matches(source) {
            return false;
        }
        if bound_epoch != epoch {
            self.reject(MonitorStatus::Stopped);
            return false;
        }
        let Some(playback) = playback.filter(|p| output.len() <= 256 && !p.playing()) else {
            self.reject(MonitorStatus::Unsupported);
            return false;
        };
        if output.is_empty() {
            return false;
        }
        let mut input = [[0.0; 2]; 256];
        let input = &mut input[..output.len()];
        if !self.read_stream(input) {
            return false;
        }
        let Some(Mode::Paused { monitor, .. }) =
            self.active.as_mut().map(|binding| &mut binding.mode)
        else {
            return false;
        };
        if playback
            .render_with_paused_track_monitor(monitor, input, output)
            .is_err()
        {
            self.reject(MonitorStatus::Unsupported);
            return false;
        }
        self.state.store(MonitorStatus::Running as u8, Release);
        true
    }

    fn read_stream(&mut self, output: &mut [Frame]) -> bool {
        let Some(binding) = self.active.as_mut() else {
            return false;
        };
        match binding.stream.read_block(output) {
            Ok(MonitorBlock::Priming) => {
                self.state.store(MonitorStatus::Priming as u8, Release);
                false
            }
            Ok(MonitorBlock::Ready) => true,
            Err(error) => {
                let reason = match error {
                    MonitorStreamError::InvalidBlock => MonitorStatus::Unsupported,
                    MonitorStreamError::Discontinuity => MonitorStatus::Discontinuity,
                    MonitorStreamError::Underrun => MonitorStatus::Underrun,
                    MonitorStreamError::Input(crate::input_capture::InputMonitorEnd::Overrun) => {
                        MonitorStatus::Overrun
                    }
                    MonitorStreamError::Input(_) => MonitorStatus::InputEnded,
                };
                self.reject(reason);
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input_capture::test_monitor_source;
    use hex_project::{Command, Created, Editor, Project};

    fn target() -> OutputCaptureId {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "input".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        OutputCaptureId::Track(track)
    }
    fn source() -> AppliedAudioSource {
        AppliedAudioSource {
            context: hex_engine::AudioSourceContext {
                project_generation: 1,
                editor_revision: 2,
            },
            audio_revision: 3,
            output_identity: 4,
        }
    }

    #[test]
    fn constructor_quote_matches_actual_channels() {
        let (_control, callback) = channel(48_000);
        let actual = callback.constructor_channel_storage().unwrap();
        assert_eq!(Some(actual.data_bytes), constructor_storage_quote());
        assert_eq!((actual.shared_groups, actual.shared_handles), (4, 8));
    }

    #[test]
    fn refusal_stop_and_source_change_preserve_exact_owners_and_terminal() {
        let (mut control, mut callback) = channel(48_000);
        let (_input, stream) = test_monitor_source();
        let original = &*stream as *const _;
        assert!(control.publish(stream, source(), target()).is_ok());
        let (_other_input, other) = test_monitor_source();
        let other_ptr = &*other as *const _;
        let refused = control.publish(other, source(), target()).err().unwrap();
        assert_eq!(other_ptr, &*refused.stream as *const _);
        callback.begin_block(false, false);
        assert_eq!(callback.read(None, true, true, &mut [[0.0; 2]; 64]), None);
        assert_eq!(control.status(), MonitorStatus::SourceChanged);
        callback.begin_block(false, false);
        assert_eq!(control.status(), MonitorStatus::SourceChanged);
        assert_eq!(original, &*control.reclaim().unwrap() as *const _);
        assert!(control.publish(refused.stream, source(), target()).is_ok());
        control.stop();
        callback.begin_block(false, false);
        assert_eq!(control.status(), MonitorStatus::Stopped);
        assert_eq!(other_ptr, &*control.reclaim().unwrap() as *const _);
    }

    #[test]
    fn priming_and_unsupported_paths_leave_owner_for_control() {
        let (mut control, mut callback) = channel(48_000);
        let (mut input, stream) = test_monitor_source();
        assert!(control.publish(stream, source(), target()).is_ok());
        callback.begin_block(false, false);
        let mut block = [[1.0; 2]; 64];
        assert!(
            callback
                .read(Some(source()), true, true, &mut block)
                .is_none()
        );
        assert_eq!(control.status(), MonitorStatus::Priming);
        input.submit(&[0.25; 512]);
        assert!(
            callback
                .read(Some(source()), false, true, &mut block)
                .is_none()
        );
        assert_eq!(control.status(), MonitorStatus::Unsupported);
        callback.begin_block(false, false);
        assert!(control.reclaim().is_some());
        assert_eq!(input.submit(&[0.25; 64]), 1);
    }

    #[test]
    fn fault_and_underrun_are_terminal_and_return_owner() {
        let (mut control, mut callback) = channel(48_000);
        let (mut input, stream) = test_monitor_source();
        input.submit(&[0.25; 512]);
        assert!(control.publish(stream, source(), target()).is_ok());
        callback.begin_block(false, false);
        assert!(
            callback
                .read(Some(source()), true, true, &mut [[0.0; 2]; 256])
                .is_some()
        );
        assert!(
            callback
                .read(Some(source()), true, true, &mut [[0.0; 2]; 256])
                .is_some()
        );
        assert!(
            callback
                .read(Some(source()), true, true, &mut [[0.0; 2]; 1])
                .is_none()
        );
        assert_eq!(control.status(), MonitorStatus::Underrun);
        callback.begin_block(false, false);
        assert_eq!(control.status(), MonitorStatus::Underrun);
        let stream = control.reclaim().unwrap();
        assert!(control.publish(stream, source(), target()).is_ok());
        callback.begin_block(false, true);
        assert_eq!(control.status(), MonitorStatus::OutputFault);
        assert!(control.reclaim().is_some());
    }
}
