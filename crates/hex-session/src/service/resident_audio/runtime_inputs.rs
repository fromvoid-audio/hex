//! Fixed control-side input bindings, independent of any player, factory or source bank.
//! Candidate construction never cancels shared clients. Only accepted canonical replacement
//! retires invalid selections; actual callback installation handles matching client rebinding.
use super::*;
use hex_engine::{
    computer_keyboard::ComputerKeyboardInput, live_expression::LiveExpressionInput,
    tuning_audition::TuningAuditionInput,
};

#[derive(Clone, Copy)]
pub(in crate::service) enum RequiredInput {
    Keyboard,
    Audition,
    Expression,
}

#[derive(Clone, Default)]
pub(in crate::service) struct RuntimeInputs {
    pub(in crate::service) version: u64,
    pub(in crate::service) keyboard: Option<(hex_project::TrackId, Arc<ComputerKeyboardInput>)>,
    pub(in crate::service) audition: Option<Arc<TuningAuditionInput>>,
    pub(in crate::service) expression: Option<Arc<LiveExpressionInput>>,
}
impl RuntimeInputs {
    pub(in crate::service) fn capture_bytes(&self) -> Result<usize, SessionError> {
        let mut bytes = std::mem::size_of::<Self>();
        for extra in [
            self.keyboard
                .as_ref()
                .map_or(0, |(_, input)| input.control_storage_bytes()),
            self.audition.as_ref().map_or(0, |_| {
                std::mem::size_of::<TuningAuditionInput>() + 2 * std::mem::size_of::<usize>()
            }),
            self.expression.as_ref().map_or(0, |_| {
                std::mem::size_of::<LiveExpressionInput>() + 2 * std::mem::size_of::<usize>()
            }),
        ] {
            bytes = bytes
                .checked_add(extra)
                .ok_or(SessionError::Invalid("runtime input capture overflow"))?;
        }
        Ok(bytes)
    }
    pub(in crate::service) fn allowance(&self, snapshot: &Snapshot) -> Result<usize, SessionError> {
        let keyboard = if let Some((track, input)) = &self.keyboard {
            input
                .runtime_storage_limit_on_control()
                .checked_add(Playback::paused_keyboard_allowance_on_control(
                    &snapshot.prepared,
                    Some(&snapshot.resources),
                    *track,
                )?)
                .ok_or(SessionError::Invalid(
                    "resident keyboard allowance overflow",
                ))?
        } else {
            0
        };
        keyboard
            .checked_add(if self.audition.is_some() {
                hex_engine::tuning_audition::MAX_TUNING_AUDITION_STORAGE_BYTES
            } else {
                0
            })
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Self>()))
            .and_then(|bytes| {
                bytes.checked_add(self.expression.as_ref().map_or(0, |_| {
                    std::mem::size_of::<LiveExpressionInput>() + 2 * std::mem::size_of::<usize>()
                }))
            })
            .ok_or(SessionError::Invalid("runtime input allowance overflow"))
    }
    // A selection update, including invalidation by an ordinary edit, must invalidate a captured
    // seek even when the authored project revision did not change. Physical note words do not.
    pub(in crate::service) fn selected_after(
        &mut self,
        previous: &Self,
    ) -> Result<(), SessionError> {
        if let (Some((track, input)), Some((old_track, old_input))) =
            (&self.keyboard, &previous.keyboard)
            && track != old_track
            && Arc::ptr_eq(input, old_input)
        {
            return Err(SessionError::Invalid(
                "a different keyboard track requires a fresh input client",
            ));
        }
        self.version = if self.same_selection(previous) {
            previous.version
        } else {
            previous
                .version
                .checked_add(1)
                .ok_or(SessionError::Invalid("runtime input version exhausted"))?
        };
        Ok(())
    }
    pub(in crate::service) fn same_selection(&self, other: &Self) -> bool {
        let keyboard = match (&self.keyboard, &other.keyboard) {
            (None, None) => true,
            (Some((a_track, a)), Some((b_track, b))) => a_track == b_track && Arc::ptr_eq(a, b),
            _ => false,
        };
        fn same<T>(a: &Option<Arc<T>>, b: &Option<Arc<T>>) -> bool {
            match (a, b) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
        }
        keyboard
            && same(&self.audition, &other.audition)
            && same(&self.expression, &other.expression)
    }
    pub(in crate::service) fn satisfies(&self, requested: &Self, required: RequiredInput) -> bool {
        match required {
            RequiredInput::Keyboard => match (&self.keyboard, &requested.keyboard) {
                (Some((a_track, a)), Some((b_track, b))) => a_track == b_track && Arc::ptr_eq(a, b),
                _ => false,
            },
            RequiredInput::Audition => self
                .audition
                .as_ref()
                .zip(requested.audition.as_ref())
                .is_some_and(|(a, b)| Arc::ptr_eq(a, b)),
            RequiredInput::Expression => self
                .expression
                .as_ref()
                .zip(requested.expression.as_ref())
                .is_some_and(|(a, b)| Arc::ptr_eq(a, b)),
        }
    }
    /// Why `attach` dropped the required input, for an honest refusal.
    pub(in crate::service) fn unbound_reason(
        &self,
        project: &Project,
        playback: &Playback,
        required: RequiredInput,
    ) -> &'static str {
        match required {
            RequiredInput::Expression => match &self.expression {
                None => "controller input was not requested",
                Some(input) if input.is_cancelled() => "controller input was cancelled",
                Some(input)
                    if !project
                        .track(input.track)
                        .and_then(hex_project::Track::instrument)
                        .is_some_and(|device| {
                            device.id() == input.instrument && !device.bypassed()
                        }) =>
                {
                    "controller input track has no active instrument"
                }
                Some(input) if !playback.live_expression_supported_on_control(input.track) => {
                    "this instrument does not accept live controller input"
                }
                Some(_) => "controller input could not bind to the current audio",
            },
            RequiredInput::Keyboard => match &self.keyboard {
                Some((_, input)) if input.is_cancelled() => "computer keyboard input was cancelled",
                Some((track, _))
                    if project
                        .track(*track)
                        .and_then(hex_project::Track::instrument)
                        .is_none_or(hex_project::Device::bypassed) =>
                {
                    "computer keyboard track has no active instrument"
                }
                _ => "computer keyboard input could not bind to the current audio",
            },
            RequiredInput::Audition => "tuning audition input was cancelled",
        }
    }
    pub(in crate::service) fn attach(
        &mut self,
        project: &Project,
        playback: &mut Playback,
        resources: &hex_engine::native_device_resources::PreparedDeviceResources,
    ) -> Result<(), SessionError> {
        if let Some((track, input)) = &self.keyboard {
            let valid = !input.is_cancelled()
                && project
                    .track(*track)
                    .and_then(hex_project::Track::instrument)
                    .is_some_and(|device| !device.bypassed());
            if valid {
                playback.prepare_resident_computer_keyboard(
                    project,
                    Some(resources),
                    *track,
                    Arc::clone(input),
                )?;
            } else {
                self.keyboard = None;
            }
        }
        if let Some(input) = &self.expression {
            let valid = !input.is_cancelled()
                && project
                    .track(input.track)
                    .and_then(hex_project::Track::instrument)
                    .is_some_and(|device| device.id() == input.instrument && !device.bypassed())
                && playback.live_expression_supported_on_control(input.track);
            if valid {
                playback.prepare_live_expression(project, Arc::clone(input))?;
            } else {
                self.expression = None;
            }
        }
        if let Some(input) = &self.audition {
            if input.is_cancelled() {
                self.audition = None;
            } else {
                playback.prepare_tuning_audition(project, Arc::clone(input))?;
            }
        }
        Ok(())
    }
    pub(in crate::service) fn cancel_retired_from(&self, old: &Self) {
        if let Some((_, input)) = &old.keyboard
            && !self
                .keyboard
                .as_ref()
                .is_some_and(|(_, next)| Arc::ptr_eq(input, next))
        {
            input.cancel();
        }
        if let Some(input) = &old.expression
            && !self
                .expression
                .as_ref()
                .is_some_and(|next| Arc::ptr_eq(input, next))
        {
            input.cancel();
        }
        if let Some(input) = &old.audition
            && !self
                .audition
                .as_ref()
                .is_some_and(|next| Arc::ptr_eq(input, next))
        {
            input.cancel();
        }
    }
}
