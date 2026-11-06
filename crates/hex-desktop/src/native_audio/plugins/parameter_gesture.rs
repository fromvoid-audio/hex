//! One live VST3 gesture on the exact original output and source. The caller
//! keeps canonical history and the actual native source stays in Root.
use super::*;
use hex_audio::{
    plugin_worker::{
        NativeWorkerCapture, NativeWorkerCaptureProgress, NativeWorkerParameterChange,
        NativeWorkerParameterProgress,
    },
    prepared::PublicationControl,
};
use hex_session::{
    device_gesture::{
        GestureKey, GestureSnapshot, GestureTarget, PluginGesturePreparation,
        PluginParameterOrigin, PluginParameterValue,
    },
    service::{
        PluginParameterCapture, PluginParameterGestureError as Error, PluginParameterGestureInput,
        PluginParameterGestureResult as Prepared,
    },
};
use std::time::{Duration, Instant};

pub(super) struct Gesture {
    key: GestureKey,
    origin: PluginParameterOrigin,
    binding: Binding,
    output: u64,
    steps: u32,
    last: PluginParameterValue,
    sequence: u64,
    pending: Option<(NativeWorkerParameterChange, PluginParameterValue, u64)>,
    capture: Option<NativeWorkerCapture>,
    before: Option<NativeDeviceState>,
    commit: Option<(AudioSourceContext, u64)>,
}

fn refused(reason: impl Into<String>) -> Error {
    Error::Refused(reason.into())
}
fn lost() -> Error {
    Error::SourceLost("vst3 parameter source changed".into())
}

fn admit<T>(
    control: &mut PublicationControl,
    mut begin: impl FnMut(&mut PublicationControl) -> Result<T, &'static str>,
) -> Result<T, Error> {
    let deadline = Instant::now() + Duration::from_millis(150);
    loop {
        match begin(control) {
            Err(
                "native preparation is busy"
                | "native parameter mailbox is busy"
                | "native capture mailbox is busy",
            ) if Instant::now() < deadline => edits::poll_control(control),
            result => return result.map_err(refused),
        }
    }
}

impl Plugins {
    pub(in crate::native_audio) fn parameter_gesture(
        &mut self,
        control: &mut PublicationControl,
        input: PluginParameterGestureInput<'_>,
    ) -> Result<Prepared, Error> {
        if let PluginGesturePreparation::Begin { origin, snapshot } = input.operation() {
            if self.parameter_gesture.is_some()
                || self.parameters.is_some()
                || self.restore.is_some()
            {
                return Err(refused("vst3 controller is busy"));
            }
            let inventory = self
                .observed_parameters
                .as_ref()
                .filter(|inventory| {
                    inventory.query.context == origin.context
                        && inventory.query.device == origin.device
                        && control.output_identity() == Some(inventory.query.output_identity)
                })
                .ok_or_else(|| refused("refresh the vst3 parameter list"))?;
            let row = inventory
                .rows
                .iter()
                .find(|row| {
                    row.id == origin.parameter
                        && row.editable
                        && row.page_first == origin.page_first
                        && row.metadata_revision == origin.metadata_revision
                        && row.normalized == origin.initial.normalized()
                })
                .ok_or_else(|| refused("vst3 parameter metadata changed · refresh the list"))?;
            let gesture = Gesture {
                key: snapshot.key,
                origin,
                binding: inventory.binding,
                output: inventory.query.output_identity,
                steps: row.steps,
                last: origin.initial,
                sequence: 0,
                pending: None,
                capture: None,
                before: None,
                commit: None,
            };
            if !gesture.current(self, control, input.context()) {
                return Err(lost());
            }
            self.parameter_gesture = Some(gesture);
            return Ok(Prepared::Applied);
        }
        let mut gesture = self.parameter_gesture.take().ok_or_else(lost)?;
        if !gesture.current(self, control, input.context()) {
            return Err(lost());
        }
        let result = gesture.apply(control, input.context(), input.operation());
        // Only an actual restore or a confirmed source loss retires the preview.
        if !(matches!(&result, Err(Error::SourceLost(_)))
            || result.is_ok()
                && matches!(input.operation(), PluginGesturePreparation::Restore { .. }))
        {
            self.parameter_gesture = Some(gesture);
        }
        result
    }

    pub(in crate::native_audio) fn finish_parameter_gesture(
        &mut self,
        context: AudioSourceContext,
        key: GestureKey,
        sequence: u64,
        accepted: bool,
    ) {
        if let Some(gesture) = self.parameter_gesture.as_mut()
            && gesture.key == key
            && gesture.commit == Some((context, sequence))
        {
            if accepted {
                self.parameter_gesture = None;
                self.observed_parameters = None;
            } else {
                gesture.commit = None;
            }
        }
    }
}

impl Gesture {
    fn current(
        &self,
        plugins: &Plugins,
        control: &PublicationControl,
        context: AudioSourceContext,
    ) -> bool {
        control.output_identity() == Some(self.output)
            && context.project_generation == self.origin.context.project_generation
            && plugins
                .sources
                .iter()
                .rev()
                .find(|source| {
                    source.accepted
                        && !source.retiring
                        && source.project_generation == context.project_generation
                        && source.device == self.origin.device.value()
                })
                .is_some_and(|source| source.binding == self.binding)
    }

    fn apply(
        &mut self,
        control: &mut PublicationControl,
        context: AudioSourceContext,
        operation: PluginGesturePreparation,
    ) -> Result<Prepared, Error> {
        let (snapshot, value, restore, commit) = match operation {
            PluginGesturePreparation::Preview(snapshot) => {
                (snapshot, snapshot.plugin_value, false, false)
            }
            PluginGesturePreparation::Restore { snapshot, initial } => {
                (snapshot, Some(initial), true, false)
            }
            PluginGesturePreparation::Commit(snapshot) => {
                (snapshot, snapshot.plugin_value, false, true)
            }
            PluginGesturePreparation::Begin { .. } => {
                return Err(refused("vst3 gesture already began"));
            }
        };
        let value = value.ok_or_else(|| refused("vst3 gesture has no value"))?;
        if !self.matches(snapshot) || (!restore && snapshot.sequence < self.sequence) {
            return Err(refused("vst3 parameter gesture changed"));
        }
        if self.capture.is_some() && !commit {
            self.capture(control)?;
        }
        self.drain_point(control)?;
        if commit {
            if snapshot.sequence != self.sequence || value != self.last {
                return Err(refused("vst3 final point has not been processed"));
            }
            let before = self
                .before
                .clone()
                .ok_or_else(|| refused("vst3 has no original capture"))?;
            let after = self.capture(control)?;
            self.commit = Some((context, snapshot.sequence));
            return Ok(Prepared::Captured(Box::new(PluginParameterCapture {
                before,
                after,
            })));
        }
        if value != self.last {
            if self.before.is_none() {
                self.before = Some(self.capture(control)?);
            }
            let actual = if self.steps == 0 {
                value.normalized()
            } else {
                let steps = f64::from(self.steps);
                (value.normalized() * steps).round() / steps
            };
            let token = admit(control, |control| {
                control.begin_native_source_parameter_change_on_control(
                    self.binding,
                    self.origin.parameter,
                    actual,
                )
            })?;
            self.pending = Some((token, value, snapshot.sequence));
            self.drain_point(control)?;
        }
        self.sequence = snapshot.sequence;
        Ok(Prepared::Applied)
    }

    fn matches(&self, snapshot: GestureSnapshot) -> bool {
        snapshot.key == self.key
            && snapshot.target
                == GestureTarget::PluginParameter {
                    device: self.origin.device,
                    parameter: self.origin.parameter,
                }
    }

    fn drain_point(&mut self, control: &mut PublicationControl) -> Result<(), Error> {
        let Some((token, value, sequence)) = self.pending.as_ref() else {
            return Ok(());
        };
        let deadline = Instant::now() + Duration::from_millis(150);
        loop {
            let mut result =
                control.poll_native_source_parameter_change_on_control(self.binding, token);
            if matches!(result, Ok(NativeWorkerParameterProgress::Failed)) {
                result = control
                    .drain_stopped_native_source_parameter_change_on_control(self.binding, token);
            }
            match result {
                Ok(
                    NativeWorkerParameterProgress::Processed(_)
                    | NativeWorkerParameterProgress::Flushed,
                ) => {
                    self.last = *value;
                    self.sequence = *sequence;
                    self.pending = None;
                    return Ok(());
                }
                Ok(
                    NativeWorkerParameterProgress::Refused
                    | NativeWorkerParameterProgress::Withdrawn,
                ) => {
                    self.pending = None;
                    return Err(refused("vst3 input was not applied"));
                }
                Ok(NativeWorkerParameterProgress::Failed) => {
                    self.pending = None;
                    return Err(lost());
                }
                Ok(NativeWorkerParameterProgress::Pending) | Err("native preparation is busy") => {}
                Err(
                    "native parameter output is closing"
                    | "native stopped parameter source differs",
                ) => {
                    return Err(lost());
                }
                Err(reason) => return Err(refused(reason)),
            }
            if Instant::now() >= deadline {
                if control.withdraw_native_source_parameter_change_on_control(self.binding, token)
                    == Ok(true)
                {
                    continue;
                }
                return Err(refused("vst3 parameter input is still pending"));
            }
            edits::poll_control(control);
        }
    }

    fn capture(&mut self, control: &mut PublicationControl) -> Result<NativeDeviceState, Error> {
        if self.capture.is_none() {
            self.capture = Some(admit(control, |control| {
                control.begin_native_source_state_capture_on_control(self.binding)
            })?);
        }
        let request = self
            .capture
            .as_ref()
            .ok_or_else(|| refused("vst3 capture unavailable"))?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match control.poll_native_source_state_capture_on_control(self.binding, request) {
                Ok(NativeWorkerCaptureProgress::Captured(state)) => {
                    self.capture = None;
                    return Ok(state);
                }
                Ok(NativeWorkerCaptureProgress::Pending) | Err("native preparation is busy") => {}
                Ok(NativeWorkerCaptureProgress::Refused(reason)) => {
                    self.capture = None;
                    return Err(refused(reason.to_string()));
                }
                Ok(NativeWorkerCaptureProgress::Failed)
                | Err("native capture output is closing") => {
                    self.capture = None;
                    return Err(lost());
                }
                Err(reason) => return Err(refused(reason)),
            }
            if Instant::now() >= deadline {
                return Err(refused("vst3 state capture is still settling"));
            }
            edits::poll_control(control);
        }
    }
}
