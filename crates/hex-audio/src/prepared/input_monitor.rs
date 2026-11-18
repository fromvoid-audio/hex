//! One exact-source, single-track live input monitor for resident playback.
//!
//! Control publishes at most one owner; a callback reads at most 256 frames
//! and returns the owner through a one-slot queue. A full or abandoned return
//! queue retains it until control-side callback destruction. Stop bypasses the
//! queue; a block already being rendered may finish. Priming and terminal
//! errors leave ordinary playback intact. The playing mode borrows the active
//! renderer. The paused mode carries separate processor history and is bound
//! to a transport epoch as well as an exact resident source. A release stops
//! reading input and renders retained PCM through that private history. A
//! priority Stop or invalid binding also revokes the release. Input and output
//! rates must match; independent device-clock drift is not corrected.
use super::AppliedAudioSource;
use crate::input_capture::{InputMonitorEnd, MonitorBlock, MonitorStream, MonitorStreamError};
use hex_engine::{Frame, OutputCaptureId, PausedTrackMonitor, playback::Playback};
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::sync::{
    Arc,
    atomic::{
        AtomicU8,
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
    source: AppliedAudioSource,
    mode: Mode,
}
enum Mode {
    Playing {
        track: OutputCaptureId,
        stream: Box<MonitorStream>,
    },
    Paused {
        stream: Box<MonitorStream>,
        monitor: PausedTrackMonitor,
        transport_epoch: u64,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        foreign: bool,
    },
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    PausedSource {
        monitor: hex_engine::PausedSourceMonitor,
        transport_epoch: u64,
    },
}
#[repr(u8)]
enum Request {
    Feed,
    Release,
    Stop,
}

/// One-slot monitor publisher. Rejected publications retain their owner.
pub struct Control {
    incoming: Producer<Box<Binding>>,
    retired: Consumer<Box<Binding>>,
    request: Arc<AtomicU8>,
    state: Arc<AtomicU8>,
    occupied: bool,
    rate: u32,
}
pub(super) struct Callback {
    incoming: Consumer<Box<Binding>>,
    retired: Producer<Box<Binding>>,
    active: Option<Box<Binding>>,
    request: Arc<AtomicU8>,
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
    let request = Arc::new(AtomicU8::new(Request::Feed as u8));
    let state = Arc::new(AtomicU8::new(MonitorStatus::Idle as u8));
    (
        Control {
            incoming,
            retired,
            request: Arc::clone(&request),
            state: Arc::clone(&state),
            occupied: false,
            rate,
        },
        Callback {
            incoming: receiver,
            retired: returns,
            active: None,
            request,
            state,
        },
    )
}
pub(super) fn constructor_storage_quote() -> Option<usize> {
    use crate::native_callback_owner::ChannelStorage;
    ChannelStorage::ring_bytes::<Box<Binding>>(1)?
        .checked_mul(2)?
        .checked_add(2 * std::mem::size_of::<AtomicU8>())
}

impl Control {
    /// Install one endpoint for an exact applied graph. Call only on control.
    pub fn publish(
        &mut self,
        stream: Box<MonitorStream>,
        source: AppliedAudioSource,
        track: OutputCaptureId,
    ) -> Result<(), Refusal> {
        if !matches!(track, OutputCaptureId::Track(_)) || stream.source().1.sample_rate != self.rate
        {
            return Err(Refusal { stream });
        }
        self.publish_binding(source, stream, |stream| Mode::Playing { track, stream })
            .map_err(|stream| Refusal { stream })
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
        if stream.source().1.sample_rate != self.rate {
            return Err(PausedRefusal { stream, monitor });
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let foreign = monitor.foreign_effect_bindings().next().is_some();
        self.publish_binding(source, (stream, monitor), |(stream, monitor)| {
            Mode::Paused {
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                foreign,
                stream,
                monitor,
                transport_epoch,
            }
        })
        .map_err(|(stream, monitor)| PausedRefusal { stream, monitor })
    }

    /// Publish one private note monitor for the exact settled pause. Control
    /// retains its engine storage hold and owns matching endpoint publication.
    /// This shares the physical input monitor's single private ownership slot.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub fn publish_paused_source(
        &mut self,
        monitor: hex_engine::PausedSourceMonitor,
        source: AppliedAudioSource,
        transport_epoch: u64,
    ) -> Result<(), hex_engine::PausedSourceMonitor> {
        self.publish_binding(source, monitor, |monitor| Mode::PausedSource {
            monitor,
            transport_epoch,
        })
    }

    fn publish_binding<T>(
        &mut self,
        source: AppliedAudioSource,
        owner: T,
        mode: impl FnOnce(T) -> Mode,
    ) -> Result<(), T> {
        if self.occupied || self.incoming.is_abandoned() {
            return Err(owner);
        }
        let Ok(slot) = self.incoming.write_chunk_uninit(1) else {
            return Err(owner);
        };
        let binding = Box::new(Binding {
            source,
            mode: mode(owner),
        });
        self.request.store(Request::Feed as u8, Release);
        self.state.store(MonitorStatus::Pending as u8, Release);
        slot.fill_from_iter(std::iter::once(binding));
        self.occupied = true;
        Ok(())
    }
    /// Independent of queue pressure; the callback stops using the owner.
    pub fn stop(&self) {
        self.request.store(Request::Stop as u8, Release);
    }

    /// Detach a paused monitor from physical input and keep its retained audio
    /// until drained. Request this before stopping the input worker. A hard
    /// stop always wins. Reclaim is still needed to return both owners.
    pub fn release_paused(&self) {
        let _ = self.request.compare_exchange(
            Request::Feed as u8,
            Request::Release as u8,
            Release,
            std::sync::atomic::Ordering::Relaxed,
        );
    }
    pub fn status(&self) -> MonitorStatus {
        MonitorStatus::decode(self.state.load(Acquire))
    }
    /// Dispose of a returned renderer on control and return its input stream
    /// if one was published. The note monitor's hold witnesses destruction.
    pub fn reclaim(&mut self) -> Option<Box<MonitorStream>> {
        let binding = self.retired.pop().ok()?;
        self.occupied = false;
        match binding.mode {
            Mode::Playing { stream, .. } | Mode::Paused { stream, .. } => Some(stream),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            Mode::PausedSource { .. } => None,
        }
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
            .plus(ChannelStorage::paired_payload(self.request.as_ref()))?
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
    fn stop_requested(&self) -> bool {
        self.request.load(Acquire) == Request::Stop as u8
    }
    fn release_requested(&self) -> bool {
        self.request.load(Acquire) == Request::Release as u8
    }
    pub(super) fn begin_block(&mut self, closing: bool, fault: bool) {
        if closing || fault || self.stop_requested() {
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
            if closing || fault || self.stop_requested() {
                self.retire();
            }
        }
    }
    pub(super) fn reject(&mut self, reason: MonitorStatus) {
        if self.active.is_some() {
            self.state.store(reason as u8, Release);
            self.request.store(Request::Stop as u8, Release);
        }
    }
    pub(super) fn target(&self) -> Option<OutputCaptureId> {
        match self.active.as_ref()?.mode {
            Mode::Playing { track, .. } => Some(track),
            _ => None,
        }
    }
    pub(super) fn reject_paused(&mut self, reason: MonitorStatus) {
        if !self.stop_requested() && self.active.is_some() && self.target().is_none() {
            self.reject(reason);
        }
    }
    pub(super) fn has_paused_binding(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|binding| matches!(binding.mode, Mode::Paused { .. }))
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn has_paused_source_binding(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|binding| matches!(binding.mode, Mode::PausedSource { .. }))
    }
    fn source_matches(&mut self, source: Option<AppliedAudioSource>) -> bool {
        if self.stop_requested() {
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

    /// Claim only the exact paused interval. A missing or refused private
    /// bank leaves the opted-in resident silent, with its history intact.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn render_paused_source(
        &mut self,
        source: Option<AppliedAudioSource>,
        epoch: u64,
        playback: Option<&mut Playback>,
        output: &mut [Frame],
        observer: Option<&mut hex_engine::NodeSignalObserver<'_>>,
        banks: Option<super::native_effects::PausedNoteBanks<'_>>,
    ) -> bool {
        if !self.source_matches(source) {
            return false;
        }
        let Some(Mode::PausedSource {
            monitor,
            transport_epoch,
        }) = self.active.as_deref_mut().map(|binding| &mut binding.mode)
        else {
            return false;
        };
        if *transport_epoch != epoch || playback.as_ref().is_some_and(|p| p.playing()) {
            self.reject(MonitorStatus::Stopped);
            return false;
        }
        if output.is_empty() {
            return false;
        }
        let rendered =
            if let (Some(playback), Some(mut banks), Some(source)) = (playback, banks, source) {
                banks.render((source, epoch), playback, monitor, output, observer)
            } else {
                false
            };
        if rendered {
            self.state.store(MonitorStatus::Running as u8, Release);
        } else {
            output.fill([0.0; 2]);
            self.reject(MonitorStatus::Unsupported);
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
        if self.release_requested() {
            self.reject(MonitorStatus::Stopped);
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
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))] native: Option<(
            &mut crate::candidate_custody::NativeEndpointBank,
            usize,
        )>,
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
        if bound_epoch != epoch || playback.is_some_and(Playback::playing) {
            self.reject(MonitorStatus::Stopped);
            return false;
        }
        let Some(playback) = playback.filter(|_| output.len() <= 256) else {
            self.reject(MonitorStatus::Unsupported);
            return false;
        };
        if output.is_empty() {
            return false;
        }
        let mut input = [[0.0; 2]; 256];
        let input = &mut input[..output.len()];
        let mut releasing = self.release_requested();
        if !releasing {
            let ready = self.read_stream(input);
            releasing = self.release_requested();
            if !ready && !releasing {
                return false;
            }
        }
        if self.stop_requested() {
            return false;
        }
        let Some(Binding {
            mode:
                Mode::Paused {
                    monitor,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    foreign,
                    ..
                },
            ..
        }) = self.active.as_deref_mut()
        else {
            return false;
        };
        let render_builtin = |monitor: &mut PausedTrackMonitor, output: &mut [Frame]| {
            if releasing {
                playback.drain_paused_track_monitor(monitor, output)
            } else {
                playback
                    .render_with_paused_track_monitor(monitor, input, output)
                    .map(|()| true)
            }
        };
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let rendered = if *foreign {
            Self::render_foreign_paused(
                playback,
                monitor,
                source.map(|source| (source, epoch)),
                (!releasing).then_some(&*input),
                output,
                native,
            )
        } else {
            render_builtin(monitor, output)
        };
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        let rendered = render_builtin(monitor, output);
        match rendered {
            Ok(true) => self.state.store(MonitorStatus::Running as u8, Release),
            Ok(false) => self.reject(MonitorStatus::Stopped),
            Err(_) => {
                self.reject(MonitorStatus::Unsupported);
                return false;
            }
        }
        true
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn render_foreign_paused(
        playback: &Playback,
        monitor: &mut PausedTrackMonitor,
        source: Option<(AppliedAudioSource, u64)>,
        input: Option<&[Frame]>,
        output: &mut [Frame],
        native: Option<(&mut crate::candidate_custody::NativeEndpointBank, usize)>,
    ) -> Result<bool, hex_engine::Invalid> {
        let Some((bank, frames)) = native.filter(|(_, frames)| *frames == output.len()) else {
            return Err(hex_engine::Invalid(
                "paused native monitor requires a physical callback",
            ));
        };
        if source.is_none_or(|source| {
            !bank.matches_paused_monitor(source, monitor.foreign_effect_bindings())
        }) {
            return Err(hex_engine::Invalid(
                "paused native monitor endpoints differ",
            ));
        }
        bank.begin_rendering(frames)
            .map_err(|_| hex_engine::Invalid("paused native monitor callback refused"))?;
        let result = if let Some(input) = input {
            playback
                .render_paused_track_monitor_with_foreign(monitor, input, output, bank)
                .map(|()| true)
        } else {
            playback.drain_paused_track_monitor_with_foreign(monitor, output, bank)
        };
        if result.is_err() || !bank.transports_are_live() {
            bank.reject_rendering();
            output.fill([0.0; 2]);
            return Err(hex_engine::Invalid(
                "paused native monitor processing failed",
            ));
        }
        result
    }

    fn read_stream(&mut self, output: &mut [Frame]) -> bool {
        let Some(binding) = self.active.as_mut() else {
            return false;
        };
        let stream = match &mut binding.mode {
            Mode::Playing { stream, .. } | Mode::Paused { stream, .. } => stream,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            Mode::PausedSource { .. } => return false,
        };
        match stream.read_block(output) {
            Ok(MonitorBlock::Priming) => {
                self.state.store(MonitorStatus::Priming as u8, Release);
                false
            }
            Ok(MonitorBlock::Ready) => true,
            // The control thread publishes release before stopping the input.
            // If that races the read, the paused renderer can still drain.
            Err(MonitorStreamError::Input(InputMonitorEnd::StopRequested))
                if self.release_requested() =>
            {
                false
            }
            Err(error) => {
                let reason = match error {
                    MonitorStreamError::InvalidBlock => MonitorStatus::Unsupported,
                    MonitorStreamError::Discontinuity => MonitorStatus::Discontinuity,
                    MonitorStreamError::Underrun => MonitorStatus::Underrun,
                    MonitorStreamError::Input(InputMonitorEnd::Overrun) => MonitorStatus::Overrun,
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
