//! Lossless transient plugin intent through the existing keyed gesture owner.
//! Raw IDs/cursors are observations, not host admission. The native adapter
//! must validate current context, original source, parameter metadata and SDK
//! application before returning from a preparation. No codec or DSP lives here.
use super::*;
#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PluginParameterValue(f64);
impl PluginParameterValue {
    pub fn new(normalized: f64) -> Result<Self, &'static str> {
        if !normalized.is_finite() || !(0.0..=1.0).contains(&normalized) {
            return Err("plugin parameter value must be finite and within 0..1");
        }
        Ok(Self(if normalized == 0.0 { 0.0 } else { normalized }))
    }
    pub const fn normalized(self) -> f64 {
        self.0
    }
}

/// The UI's observed starting point. The host must revalidate it against the
/// actual parameter page; a caller's equal numbers cannot authorize a source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PluginParameterOrigin {
    pub context: hex_engine::AudioSourceContext,
    pub device: DeviceId,
    pub parameter: u32,
    pub page_first: u16,
    pub metadata_revision: u64,
    pub initial: PluginParameterValue,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PluginGesturePreparation {
    Begin {
        origin: PluginParameterOrigin,
        snapshot: GestureSnapshot,
    },
    /// Repeating the accepted sequence during a canonical rebase revalidates
    /// the actual source without enqueueing the parameter point again.
    Preview(GestureSnapshot),
    Commit(GestureSnapshot),
    Restore {
        snapshot: GestureSnapshot,
        initial: PluginParameterValue,
    },
}

impl GestureClient {
    pub fn begin_plugin(
        &mut self,
        origin: PluginParameterOrigin,
    ) -> Result<GestureKey, &'static str> {
        if self.recovery_pending {
            if !self.has_pending_terminal() {
                self.cancel()?;
            }
            return Err("previous device edit is canceling · retry after it finishes");
        }
        if self.active.is_some() {
            return Err("another device gesture is active");
        }
        self.next_token = self.next_token.saturating_add(1).max(1);
        let key = GestureKey {
            generation: origin.context.project_generation,
            token: self.next_token,
        };
        self.active = Some(plugin_snapshot(key, origin));
        self.pending_begin = Some(GestureRequest::BeginPlugin { key, origin });
        Ok(key)
    }
    pub fn change_plugin(&mut self, value: PluginParameterValue) -> Result<u64, &'static str> {
        self.update_plugin(value, false)
    }
    pub fn finish_plugin(&mut self, value: PluginParameterValue) -> Result<(), &'static str> {
        self.update_plugin(value, true).map(|_| ())
    }
    fn update_plugin(
        &mut self,
        value: PluginParameterValue,
        terminal: bool,
    ) -> Result<u64, &'static str> {
        if self.has_pending_terminal() {
            return Err("device gesture is finishing");
        }
        let active = self.active.as_mut().ok_or("no device gesture is active")?;
        if !matches!(active.target, GestureTarget::PluginParameter { .. })
            || active.plugin_value.is_none()
        {
            return Err("active gesture does not accept plugin values");
        }
        active.sequence = active.sequence.saturating_add(1).max(1);
        active.plugin_value = Some(value);
        let request = if terminal {
            GestureRequest::CommitPlugin {
                key: active.key,
                sequence: active.sequence,
                value,
            }
        } else {
            GestureRequest::ChangePlugin {
                key: active.key,
                sequence: active.sequence,
                value,
            }
        };
        if terminal {
            self.latest_change = None;
            self.terminal = Some(request);
        } else {
            self.latest_change = Some(request);
        }
        Ok(active.sequence)
    }
}

fn plugin_snapshot(key: GestureKey, origin: PluginParameterOrigin) -> GestureSnapshot {
    GestureSnapshot {
        key,
        target: GestureTarget::PluginParameter {
            device: origin.device,
            parameter: origin.parameter,
        },
        sequence: 0,
        value: ParameterValue::Float(0.0),
        secondary_value: None,
        parameters: None,
        creation: None,
        plugin_value: Some(origin.initial),
    }
}

impl GestureMachine {
    pub(super) fn apply_plugin(
        &mut self,
        session: &mut Session,
        request: GestureRequest,
        generation: u64,
        outcome: &mut GestureOutcome,
        prepare: &mut impl FnMut(&mut Session, GesturePreparation) -> PreparedGesture,
    ) -> Result<(), SessionError> {
        let key = request.key();
        match request {
            GestureRequest::BeginPlugin { origin, .. } => {
                if origin.context.project_generation != generation
                    || self.active.is_some()
                    || self.last_key.is_some_and(|last| {
                        last.generation == generation && key.token <= last.token
                    })
                {
                    return Err(SessionError::Invalid("plugin gesture is active or stale"));
                }
                let state = session
                    .project()
                    .device(origin.device)
                    .filter(|device| {
                        device.state().type_id() == DeviceTypeId::from_static("hex.vst3")
                    })
                    .ok_or(SessionError::Invalid(
                        "plugin gesture target is unavailable",
                    ))?
                    .state()
                    .clone();
                let snapshot = plugin_snapshot(key, origin);
                prepare(
                    session,
                    GesturePreparation::Plugin(PluginGesturePreparation::Begin {
                        origin,
                        snapshot,
                    }),
                )?;
                self.active = Some(Active {
                    snapshot,
                    kind: ActiveKind::Plugin,
                    state_origin: Some(state),
                    plugin_initial: Some(origin.initial),
                });
                self.last_key = Some(key);
            }
            GestureRequest::ChangePlugin {
                sequence, value, ..
            }
            | GestureRequest::CommitPlugin {
                sequence, value, ..
            } => {
                let active = self.checked_active(session, key)?;
                let terminal = request.phase() == GesturePhase::Commit;
                if !matches!(active.target, GestureTarget::PluginParameter { .. })
                    || active.plugin_value.is_none()
                    || sequence == 0
                    || sequence < active.sequence
                    || (!terminal && sequence == active.sequence)
                {
                    return Err(SessionError::Invalid(
                        "plugin gesture update is stale or changed kind",
                    ));
                }
                let draft = GestureSnapshot {
                    sequence,
                    plugin_value: Some(value),
                    ..active
                };
                // A final point is an actual preview first. If canonical commit
                // later refuses, the retained snapshot describes that accepted
                // SDK preview instead of claiming restoration to an older value.
                if sequence > active.sequence || active.plugin_value != Some(value) {
                    let prepared = prepare(
                        session,
                        GesturePreparation::Plugin(PluginGesturePreparation::Preview(draft)),
                    )?;
                    if let Some(current) = self.active.as_mut() {
                        current.snapshot = draft;
                    }
                    // A parameter point needs neither a cloned Project nor a
                    // replacement graph. A stopped host may separately return
                    // its real unchanged-canonical restoration revision.
                    outcome.playback = prepared.map(|(_, prepared)| prepared);
                }
                if terminal {
                    let initial = self
                        .active
                        .as_ref()
                        .and_then(|active| active.plugin_initial)
                        .ok_or(SessionError::Invalid(
                            "plugin gesture initial value is missing",
                        ))?;
                    if value == initial {
                        let prepared = prepare(
                            session,
                            GesturePreparation::Plugin(PluginGesturePreparation::Restore {
                                snapshot: draft,
                                initial,
                            }),
                        )?;
                        outcome.playback = prepared.map(|(_, prepared)| prepared);
                    } else {
                        let before = session.project().clone();
                        let (_, prepared) = prepare(
                            session,
                            GesturePreparation::Plugin(PluginGesturePreparation::Commit(draft)),
                        )?
                        .ok_or(SessionError::Invalid("plugin commit produced no revision"))?;
                        outcome.canonical_changed = session.project() != &before;
                        outcome.playback = Some(prepared);
                    }
                    self.active = None;
                }
            }
            _ => return Err(SessionError::Invalid("invalid plugin gesture request")),
        }
        Ok(())
    }
}
