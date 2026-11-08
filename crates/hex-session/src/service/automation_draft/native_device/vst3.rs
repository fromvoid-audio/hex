//! Matching final-output scalar observations use the existing device draft lane.
//! The caller must first match the actual audio receipt (including its private
//! serial/source) to the admitted operation. These copied data are not endpoint
//! authority. Clocks retain the receipt's compensated input origin; the later
//! final Main processing frame confirms delivery.
use super::*;
use hex_lattice_contract::ForeignProcessorBinding;
use hex_project::{AutomationValue, DeviceAutomationTarget, DeviceId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vst3CaptureTarget {
    pub device: DeviceId,
    pub class: [u8; 16],
    pub parameter: u32,
    pub binding: ForeignProcessorBinding,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Vst3ApplicationValue {
    Value(f64),
    /// Actual exclusive restoration confirmed by final-output receipt.
    Restored,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vst3ParameterApplication {
    pub binding: ForeignProcessorBinding,
    pub parameter: u32,
    pub origin: DraftClock,
    pub value: Vst3ApplicationValue,
}
impl Vst3CaptureTarget {
    fn validate(self, source: &hex_project::Project, stamp: DraftProvenance) -> Result<(), String> {
        if !self.binding.is_valid() || self.binding.root != stamp.output_identity {
            return Err("vst3 capture binding differs from its output".into());
        }
        let class = source
            .device(self.device)
            .ok_or("vst3 capture device is absent")?
            .automation_target_class(DeviceAutomationTarget::Vst3 {
                parameter: self.parameter,
            })
            .map_err(|error| error.0)?;
        if class != Some(self.class) {
            return Err("vst3 capture class differs from its source".into());
        }
        Ok(())
    }
    pub(in crate::service) fn value(
        self,
        application: Vst3ParameterApplication,
    ) -> Result<Option<AutomationValue>, &'static str> {
        if application.binding != self.binding || application.parameter != self.parameter {
            return Err("vst3 capture application target differs");
        }
        match application.value {
            Vst3ApplicationValue::Restored => Ok(None),
            Vst3ApplicationValue::Value(value) => {
                if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                    return Err("vst3 capture value must be finite and within 0..1");
                }
                // Existing canonical authoring is f32. IEEE conversion rounds
                // once, with absolute error <=2^-25 on0..1; clocks are exact.
                AutomationValue::new(value as f32)
                    .map(Some)
                    .map_err(|error| error.0)
            }
        }
    }
}
impl DeviceDraftInput {
    /// Source/class/binding validation is canonical data validation only. The
    /// caller still owns actual Root/SDK admission and matching receipt custody.
    pub fn start_vst3_on_control(
        session: &Session,
        document_base: Option<&Path>,
        path: PathBuf,
        target: Vst3CaptureTarget,
        stamp: DraftProvenance,
        clock: CaptureClock,
    ) -> Result<(Self, DraftWorker), String> {
        let source = recovery::project_for_save(session, document_base)?;
        target.validate(&source, stamp)?;
        let path = recovery::absolute_document_path(&path)?;
        let writer = DraftWriter::create_with_clock(
            path,
            &source,
            DraftTarget::Device {
                device: target.device,
                target: DeviceAutomationTarget::Vst3 {
                    parameter: target.parameter,
                },
            },
            stamp,
            clock,
        )
        .map_err(|error| error.to_string())?;
        let (input, worker) = spawn_writer_on_control(writer)?;
        Ok((
            Self {
                input,
                capture: Capture::Vst3 { target },
                stamp,
                last_sequence: 0,
            },
            worker,
        ))
    }
    /// None is bounded backpressure: the caller retains every application and
    /// the sequence watermark advances only after the whole batch is admitted.
    pub fn append_vst3_delivered(
        &mut self,
        target: Vst3CaptureTarget,
        stamp: DraftProvenance,
        applications: &[(u64, Vst3ParameterApplication)],
    ) -> Result<Option<u64>, &'static str> {
        self.require_vst3_scope(target, stamp)?;
        append_applications(
            &mut self.input,
            &mut self.last_sequence,
            applications,
            |application| {
                let value = target
                    .value(application)?
                    .ok_or("vst3 capture application restored the lane")?;
                Ok((application.origin, value))
            },
        )
    }
    pub fn finish_vst3_delivered(
        &mut self,
        target: Vst3CaptureTarget,
        stamp: DraftProvenance,
        restoration: Vst3ParameterApplication,
    ) -> Result<(), &'static str> {
        self.require_vst3_scope(target, stamp)?;
        if target.value(restoration)?.is_some() {
            return Err("vst3 capture terminal did not restore the lane");
        }
        self.input
            .finish(DraftTerminal::Finished(restoration.origin))
    }
    pub fn abort_vst3_delivered(
        &mut self,
        target: Vst3CaptureTarget,
        stamp: DraftProvenance,
        reason: DraftAbortReason,
    ) -> Result<(), &'static str> {
        self.require_vst3_scope(target, stamp)?;
        self.input.finish(DraftTerminal::Aborted(reason))
    }
    fn require_vst3_scope(
        &self,
        target: Vst3CaptureTarget,
        stamp: DraftProvenance,
    ) -> Result<(), &'static str> {
        if !matches!(&self.capture,Capture::Vst3 {target:current} if *current==target)
            || self.stamp != stamp
        {
            return Err("vst3 capture receipt scope differs from its draft");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
