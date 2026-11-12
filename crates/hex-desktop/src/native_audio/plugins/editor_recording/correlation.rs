//! Join the original ordered editor boundaries to accepted Main receipts.
//! The caller retains one unmatched PCM receipt and owns the read cadence.
//! Values and clocks come only from the compensated Main witness.
use hex_audio::foreign_parameter_delivery::editor::{ConfirmedQuantum, Target};
use hex_audio::plugin_worker::{ParameterEdit, ParameterEditKind};
use hex_session::device_gesture::GestureKey;
use hex_session::service::automation_draft::{
    DraftClock, Vst3ApplicationValue, Vst3CaptureTarget, Vst3ParameterApplication,
};
use hex_session::service::{
    DeviceAutomationSource, DeviceCaptureTarget, PluginParameterDelivery,
    PluginParameterObservation as Observation,
};

use std::collections::VecDeque;

const MAX_ORDERED_GESTURES: usize = 64;

#[derive(Clone, Copy)]
struct Gesture {
    begin: u64,
    end: Option<u64>,
    performed: bool,
}

#[derive(Clone, Copy)]
struct Fact {
    sequence: u64,
    begin: u64,
    value: Vst3ApplicationValue,
    origin: DraftClock,
    main_frame: u64,
}

// This single, copied control result feeds the inline observation slot.
// Boxing it would allocate once per confirmed SDK value.
#[allow(clippy::large_enum_variant)]
pub(super) enum Match {
    Unrelated,
    WaitingForOrder,
    Observed(Observation),
}

/// A one-take scope; once its first nonempty gesture ends, later gestures
/// cannot replace its terminal while its final Main samples are pending.
pub(super) struct Correlation {
    audio: Target,
    key: GestureKey,
    target: Vst3CaptureTarget,
    source: DeviceAutomationSource,
    gestures: VecDeque<Gesture>,
    selected: Option<u64>,
    ordered_through: u64,
    observed_through: u64,
    restored: bool,
}

impl Correlation {
    pub(super) fn new(
        audio: Target,
        key: GestureKey,
        target: Vst3CaptureTarget,
        source: DeviceAutomationSource,
    ) -> Result<Self, &'static str> {
        if audio.binding != target.binding
            || source.target != DeviceCaptureTarget::Vst3(target)
            || source.output_identity != audio.source.output_identity
            || source.audio_revision != audio.source.audio_revision
            || source.transport_epoch != audio.transport_epoch
            || key.generation != audio.source.context.project_generation
            || key.token == 0
            || source.sample_rate == 0
        {
            return Err("vst3 editor recording identity differs");
        }
        Ok(Self {
            audio,
            key,
            target,
            source,
            gestures: VecDeque::with_capacity(MAX_ORDERED_GESTURES),
            selected: None,
            ordered_through: 0,
            observed_through: 0,
            restored: false,
        })
    }

    pub(super) fn key(&self) -> GestureKey {
        self.key
    }

    pub(super) fn target(&self) -> Target {
        self.audio
    }

    pub(super) fn abort(&self) -> Option<Observation> {
        if self.restored {
            return None;
        }
        if self.observed_through == 0 {
            return Some(Observation::Unchanged { key: self.key });
        }
        Some(Observation::Interrupted {
            key: self.key,
            context: self.audio.source.context,
            target: self.target,
            source: self.source,
            sequence: self.observed_through.saturating_add(1),
        })
    }

    pub(super) fn ordered(&mut self, edit: ParameterEdit) -> Result<(), &'static str> {
        if edit.sequence <= self.ordered_through {
            return Err("vst3 editor observations are out of order");
        }
        self.ordered_through = edit.sequence;
        if edit.kind.is_group_marker()
            || edit.id != self.target.parameter
            || self.selected.is_some_and(|begin| begin != edit.gesture)
        {
            return Ok(());
        }
        match (edit.kind, self.gestures.back_mut()) {
            (ParameterEditKind::Begin, previous)
                if edit.sequence == edit.gesture
                    && previous
                        .as_ref()
                        .is_none_or(|gesture| gesture.end.is_some()) =>
            {
                if self.gestures.len() == MAX_ORDERED_GESTURES {
                    return Err("vst3 editor order exceeded its recording capacity");
                }
                self.gestures.push_back(Gesture {
                    begin: edit.sequence,
                    end: None,
                    performed: false,
                });
            }
            (ParameterEditKind::Perform, Some(gesture))
                if edit.gesture == gesture.begin && gesture.end.is_none() =>
            {
                gesture.performed = true;
            }
            (ParameterEditKind::End, Some(gesture))
                if edit.gesture == gesture.begin && gesture.end.is_none() =>
            {
                if gesture.performed {
                    gesture.end = Some(edit.sequence);
                } else {
                    // A tap with no Perform has no clock and no take.
                    self.gestures.pop_back();
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn confirmed(
        &mut self,
        main: ConfirmedQuantum,
        order_closed: bool,
    ) -> Result<Match, &'static str> {
        if main.target != self.audio
            || main.input.binding != self.audio.binding
            || main.input.scope != self.audio.scope
            || main.input.lost_origin
        {
            return Err("vst3 editor main receipt has a stale source");
        }
        if self.restored {
            return Ok(Match::Unrelated);
        }
        let point = main
            .input
            .performs
            .as_slice()
            .iter()
            .find(|point| point.id == self.target.parameter);
        let end = main
            .input
            .restorations
            .as_slice()
            .iter()
            .find(|end| end.parameter == self.target.parameter);
        let (sequence, begin, value) = match (point, end) {
            (Some(point), None) => (
                point.sequence,
                point.gesture,
                Vst3ApplicationValue::Value(point.normalized),
            ),
            (None, Some(end)) => (end.sequence, end.gesture, Vst3ApplicationValue::Restored),
            (None, None) => return Ok(Match::Unrelated),
            (Some(_), Some(_)) => {
                return Err("vst3 editor restored on the same quantum as its input");
            }
        };
        let origin = main
            .input
            .origin
            .filter(|origin| origin.performance_frame <= main.main_performance_frame)
            .ok_or("vst3 editor main receipt has no valid origin")?;
        self.observe_fact(
            Fact {
                sequence,
                begin,
                value,
                origin: DraftClock {
                    performance_frame: origin.performance_frame,
                    content_frame: origin.arrangement_frame,
                    loop_pass: origin.loop_pass,
                },
                main_frame: main.main_performance_frame,
            },
            order_closed,
        )
    }

    fn observe_fact(&mut self, fact: Fact, order_closed: bool) -> Result<Match, &'static str> {
        let Fact {
            sequence,
            begin,
            value,
            origin,
            main_frame,
        } = fact;
        if self.restored {
            return Ok(Match::Unrelated);
        }
        let unread = sequence > self.ordered_through;
        if unread && !order_closed {
            return Ok(Match::WaitingForOrder);
        }
        if self.selected.is_some_and(|selected| selected != begin) {
            return Ok(Match::Unrelated);
        }
        // After output retirement the ordered reader may have gone away, but
        // the already accepted SDK receipt still names its original Begin
        // sequence. Preserve that prefix rather than losing work on teardown.
        let gesture = self
            .gestures
            .iter()
            .find(|gesture| gesture.begin == begin)
            .copied()
            .or_else(|| {
                (order_closed && unread && begin != 0).then_some(Gesture {
                    begin,
                    end: None,
                    performed: true,
                })
            })
            .ok_or("vst3 editor main receipt has no ordered begin")?;
        if sequence <= self.observed_through || sequence <= begin {
            return Err("vst3 editor main receipt repeats an observed edit");
        }
        match value {
            Vst3ApplicationValue::Value(value)
                if !(gesture.performed || order_closed && unread)
                    || gesture.end.is_some_and(|end| sequence >= end)
                    || !(0.0..=1.0).contains(&value) =>
            {
                return Err("vst3 editor main value differs from the ordered gesture");
            }
            Vst3ApplicationValue::Restored
                if (gesture.end != Some(sequence)
                    && (!order_closed || !unread || gesture.end.is_some()))
                    || self.observed_through == 0 =>
            {
                return Err("vst3 editor restoration has no matching delivered gesture");
            }
            _ => {}
        }
        let delivery = PluginParameterDelivery {
            key: self.key,
            context: self.audio.source.context,
            target: self.target,
            source: self.source,
            sequence,
            application: Vst3ParameterApplication {
                binding: self.audio.binding,
                parameter: self.target.parameter,
                origin,
                value,
            },
            delivery_performance_frame: main_frame,
        };
        if self.selected.is_none() {
            self.selected = Some(begin);
            self.gestures.clear();
            self.gestures.push_back(gesture);
        }
        self.observed_through = sequence;
        self.restored = value == Vst3ApplicationValue::Restored;
        Ok(Match::Observed(if self.restored {
            Observation::Restored(delivery)
        } else {
            Observation::Delivered(delivery)
        }))
    }
}

#[cfg(test)]
mod tests;
