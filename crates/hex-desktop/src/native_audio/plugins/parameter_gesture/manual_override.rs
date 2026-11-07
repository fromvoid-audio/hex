//! Actual applied-worker hold and isolated canonical capture for one reading
//! processor. Release drains on the ordinary control poll even after UI refusal.
use super::*;

pub(super) struct LiveOverride {
    pub(super) source: AppliedAudioSource,
    pub(super) transport_epoch: u64,
    pub(super) class: [u8; 16],
    pub(super) reading: bool,
    pub(super) holds_program: bool,
    pub(super) playing: bool,
    pub(super) hold: Option<NativeWorkerParameterChange>,
    release: Option<NativeWorkerParameterChange>,
    scratch: Option<ManualBaseCapture>,
    pub(super) invalidated: bool,
    pub(super) discard: bool,
}

impl LiveOverride {
    pub(super) fn new(
        source: AppliedAudioSource,
        transport_epoch: u64,
        class: [u8; 16],
        reading: bool,
        holds_program: bool,
        playing: bool,
    ) -> Self {
        Self {
            source,
            transport_epoch,
            class,
            reading,
            holds_program,
            playing,
            hold: None,
            release: None,
            scratch: None,
            invalidated: false,
            discard: false,
        }
    }

    fn poll_release(&mut self, control: &mut PublicationControl, binding: Binding) -> bool {
        if let Some(token) = self.release.as_ref() {
            match poll_input(control, binding, token) {
                Ok(NativeWorkerParameterProgress::Pending) | Err("native preparation is busy") => {
                    return false;
                }
                Ok(_) => {}
                Err(reason) if source_gone(reason) => {}
                Err(_) => return false,
            }
            self.release = None;
            self.hold = None;
        }
        let Some(hold) = self.hold.as_ref() else {
            return true;
        };
        match control.native_source_parameter_hold_on_control(
            self.source,
            binding,
            NativeParameterHold::Release { hold },
        ) {
            Ok(token) => self.release = Some(token),
            Err(reason) if source_gone(reason) => {
                self.hold = None;
                return true;
            }
            Err(_) => {}
        }
        false
    }
}

impl Plugins {
    pub(in crate::native_audio) fn override_publication_source(
        &self,
        successor: AudioSourceContext,
    ) -> Option<(AppliedAudioSource, u64, bool)> {
        let gesture = self.parameter_gesture.as_ref()?;
        let (predecessor, _) = gesture.commit?;
        let live = gesture.live.as_ref()?;
        (successor.project_generation == predecessor.project_generation
            && predecessor.editor_revision.checked_add(1) == Some(successor.editor_revision))
        .then_some((live.source, live.transport_epoch, live.playing))
    }

    pub(in crate::native_audio::plugins) fn parameter_override_cleanup_pending(&self) -> bool {
        self.parameter_gesture.as_ref().is_some_and(|gesture| {
            gesture.live.as_ref().is_some_and(|live| {
                (live.invalidated || gesture.commit_accepted)
                    && (gesture.pending.is_some() || live.hold.is_some() || live.scratch.is_some())
            })
        })
    }

    pub(in crate::native_audio::plugins) fn poll_parameter_override(
        &mut self,
        control: &mut PublicationControl,
        context: Option<AudioSourceContext>,
    ) {
        let Some(gesture) = self.parameter_gesture.as_mut() else {
            return;
        };
        let Some(live) = gesture.live.as_mut() else {
            return;
        };
        if gesture.commit_accepted {
            let processed = gesture.next_program.is_some_and(|expected| {
                control
                    .native_source_processed_program_on_control(gesture.binding)
                    .filter(|received| received.binding == gesture.binding)
                    .and_then(|received| received.program)
                    .is_some_and(|actual| actual >= expected)
            });
            let quiescent = control
                .native_source_endpoint_status(gesture.binding)
                .is_some_and(|(_, live)| !live);
            if (processed || quiescent) && live.poll_release(control, gesture.binding) {
                self.parameter_gesture = None;
            }
            return;
        }
        if gesture.commit.is_none() {
            let source_changed = context != Some(live.source.context)
                || control.status.recording_source() != Some(live.source);
            live.discard |= source_changed
                || (!live.playing
                    && live.reading
                    && control
                        .status
                        .playing
                        .load(std::sync::atomic::Ordering::Acquire));
            live.invalidated |= control.status.native_parameter_source(live.playing)
                != Some((live.source, live.transport_epoch));
        }
        let discard = live.discard;
        if live.invalidated && gesture.poll_override_cleanup(control) && discard {
            self.parameter_gesture = None;
        }
    }
}

impl Gesture {
    pub(super) fn restore_override(
        &mut self,
        control: &mut PublicationControl,
    ) -> Result<Prepared, Error> {
        if let Some(live) = self.live.as_mut() {
            live.invalidated = true;
        }
        let deadline = Instant::now() + Duration::from_millis(150);
        loop {
            if self.poll_override_cleanup(control) {
                return Ok(Prepared::Applied);
            }
            if Instant::now() >= deadline {
                return Err(refused("vst3 automation read is still being restored"));
            }
            edits::poll_control(control);
        }
    }

    fn poll_override_cleanup(&mut self, control: &mut PublicationControl) -> bool {
        if let Some(pending) = self.pending.as_ref() {
            if pending.value != self.origin.initial {
                let _ = control.withdraw_native_source_parameter_change_on_control(
                    self.binding,
                    &pending.token,
                );
            }
            let _ = self.poll_point(control);
        }
        let initial = self.sdk_value(self.origin.initial);
        let Some(live) = self.live.as_mut() else {
            return true;
        };
        if let Some(scratch) = live.scratch.as_mut()
            && scratch.poll_on_control(control, &mut || true).is_some()
        {
            live.scratch = None;
        }
        // A target without a reading lane uses ordinary points. Cancel restores
        // its original effective value on the same worker before releasing UI
        // custody. Idle reading targets also restore their actual Begin value.
        // A running reading target releases its authored hold.
        if !(live.discard || live.holds_program && live.playing)
            && self.pending.is_none()
            && self.last != self.origin.initial
        {
            match control.begin_native_source_parameter_change_on_control(
                self.binding,
                self.origin.parameter,
                initial,
            ) {
                Ok(token) => {
                    self.pending = Some(PendingPoint {
                        token,
                        value: self.origin.initial,
                        sequence: self.sequence,
                        opening_hold: false,
                    })
                }
                Err(reason) if source_gone(reason) => live.discard = true,
                Err(_) => {}
            }
            return false;
        }
        let released = self.pending.is_none() && live.poll_release(control, self.binding);
        released && live.scratch.is_none()
    }

    pub(super) fn capture_override(
        &mut self,
        control: &mut PublicationControl,
        host: &Hosting,
        canonical: &NativeDeviceState,
        cancellation: Option<&RequestCancellation>,
    ) -> Result<Prepared, Error> {
        let actual = self.sdk_value(self.last);
        let live = self.live.as_mut().ok_or_else(lost)?;
        if (live.playing && live.holds_program && live.hold.is_none()) || live.invalidated {
            return Err(refused("vst3 automation hold is unavailable"));
        }
        let status = std::sync::Arc::clone(&control.status);
        let expected = (live.source, live.transport_epoch);
        let mut cancelled = || {
            cancellation.is_some_and(RequestCancellation::is_cancelled)
                || status.native_parameter_source(live.playing) != Some(expected)
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if cancelled() {
                return Err(lost());
            }
            if live.scratch.is_none() {
                let beginning = if live.playing {
                    host.begin_manual_base_capture_on_control(
                        control,
                        live.source,
                        self.binding,
                        canonical,
                        self.origin.parameter,
                        actual,
                    )
                } else {
                    host.begin_idle_manual_base_capture_on_control(
                        control,
                        live.source,
                        self.binding,
                        canonical,
                        self.origin.parameter,
                        actual,
                    )
                };
                match beginning {
                    Ok(scratch) => live.scratch = Some(scratch),
                    Err(reason)
                        if matches!(
                            reason.as_str(),
                            "native preparation is busy" | "native source is busy"
                        ) => {}
                    Err(reason) => return Err(refused(reason)),
                }
            }
            if let Some(scratch) = live.scratch.as_mut()
                && let Some(terminal) = scratch.poll_on_control(control, &mut cancelled)
            {
                live.scratch = None;
                return terminal.map_err(refused).map(|base| {
                    Prepared::OverrideCaptured(Box::new(PluginParameterOverride {
                        source: live.source.context,
                        class: live.class,
                        parameter: self.origin.parameter,
                        base,
                    }))
                });
            }
            if Instant::now() >= deadline {
                return Err(refused("vst3 manual base capture is still settling"));
            }
            edits::poll_control(control);
        }
    }
}
