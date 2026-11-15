//! Copied observations from one original SDK editor and the compensated Main
//! reader. One batch is at most one physical quantum; no callback allocation.
use super::automation_draft::{
    DraftClock, DraftProvenance, GroupDraftEvent, Vst3GroupCaptureTarget, Vst3GroupTarget,
};
use super::{DeviceAutomationSource, DeviceAutomationTakeScope, DeviceCaptureTarget};
use crate::device_gesture::GestureKey;
use hex_engine::AudioSourceContext;

pub const MAX_VST3_EDITOR_GROUP_MEMBERS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginEditorRecordingTarget {
    Single {
        device: hex_project::DeviceId,
        class: [u8; 16],
        parameter: u32,
        reading: bool,
    },
    Group(Vst3GroupTarget),
}

impl PluginEditorRecordingTarget {
    pub fn device(self) -> hex_project::DeviceId {
        match self {
            Self::Single { device, .. } => device,
            Self::Group(group) => group.device(),
        }
    }
    pub fn class(self) -> [u8; 16] {
        match self {
            Self::Single { class, .. } => class,
            Self::Group(group) => group.class(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vst3EditorGroupRows {
    rows: [Option<GroupDraftEvent>; MAX_VST3_EDITOR_GROUP_MEMBERS],
    delivery_performance_frame: u64,
}

impl Vst3EditorGroupRows {
    pub fn new(
        rows: [Option<GroupDraftEvent>; MAX_VST3_EDITOR_GROUP_MEMBERS],
        delivery_performance_frame: u64,
    ) -> Result<Self, &'static str> {
        let first = rows[0].ok_or("grouped vst3 Main witness is empty")?;
        if first.ordinal == 0 || first.clock.performance_frame > delivery_performance_frame {
            return Err("grouped vst3 Main witness has an invalid clock or ordinal");
        }
        let mut previous = None;
        let mut ended = false;
        for row in rows {
            match row {
                Some(row)
                    if !ended
                        && row.clock == first.clock
                        && previous
                            .is_none_or(|old: u64| old.checked_add(1) == Some(row.ordinal)) =>
                {
                    previous = Some(row.ordinal);
                }
                None => ended = true,
                _ => return Err("grouped vst3 Main witness is not one ordered quantum"),
            }
        }
        Ok(Self {
            rows,
            delivery_performance_frame,
        })
    }

    pub fn rows(&self) -> impl Iterator<Item = GroupDraftEvent> + '_ {
        self.rows.iter().flatten().copied()
    }

    pub fn delivery_performance_frame(&self) -> u64 {
        self.delivery_performance_frame
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)] // One bounded copy, in control custody.
pub enum Vst3EditorGroupReceipt {
    Ready,
    Applied(Vst3EditorGroupRows),
    Finished(DraftClock),
    Interrupted,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vst3EditorGroupObservation {
    pub key: GestureKey,
    pub context: AudioSourceContext,
    pub source: DeviceAutomationSource,
    pub receipt: Vst3EditorGroupReceipt,
}

impl Vst3EditorGroupObservation {
    pub(crate) fn target(self) -> Result<Vst3GroupCaptureTarget, &'static str> {
        let DeviceCaptureTarget::Vst3Group(target) = self.source.target else {
            return Err("vst3 editor group observation has a single-target source");
        };
        if self.key.generation != self.context.project_generation
            || self.key.token == 0
            || self.source.sample_rate == 0
            || !target.binding.is_valid()
            || target.binding.root != self.source.output_identity
        {
            return Err("vst3 editor group observation source changed");
        }
        Ok(target)
    }
    pub(crate) fn scope(self) -> Result<DeviceAutomationTakeScope, &'static str> {
        self.target()?;
        Ok(DeviceAutomationTakeScope {
            target: self.source.target,
            stamp: DraftProvenance {
                sample_rate: self.source.sample_rate,
                output_identity: self.source.output_identity,
                project_generation: self.context.project_generation,
                editor_revision: self.context.editor_revision,
                audio_revision: self.source.audio_revision,
                transport_epoch: self.source.transport_epoch,
                gesture_serial: self.key.token,
            },
        })
    }

    pub(crate) fn matches(
        self,
        scope: DeviceAutomationTakeScope,
        source: DeviceAutomationSource,
    ) -> bool {
        self.source == source && self.scope().is_ok_and(|actual| actual == scope)
    }
}
