//! UI-side bounded coalescer (`GestureClient`) for device and mixer gestures.
use super::*;

/// UI-side coalescer. It owns one in-flight request, one replaceable transient,
/// and a reserved terminal request which cannot be displaced by more changes.
#[derive(Default)]
pub struct GestureClient {
    pub(super) next_token: u64,
    pub(super) active: Option<GestureSnapshot>,
    pub(super) origin_parameters: Option<DeviceParameterSnapshot>,
    pub(super) accepted_pair: Option<ParameterPairValue>,
    pub(super) in_flight: Option<GestureRequest>,
    pub(super) submitted: bool,
    pub(super) recovery_pending: bool,
    pub(super) stranded_commit: Option<GestureRequest>,
    pub(super) pending_begin: Option<GestureRequest>,
    pub(super) latest_change: Option<GestureRequest>,
    pub(super) terminal: Option<GestureRequest>,
}

impl GestureClient {
    /// Share the local identity space without pretending the original editor
    /// delivered a Hex pointer gesture or owning any of its acknowledgements.
    pub fn reserve_plugin_editor_key(
        &mut self,
        generation: u64,
    ) -> Result<GestureKey, &'static str> {
        if self.active.is_some() || self.in_flight.is_some() || self.recovery_pending {
            return Err("finish the current device gesture first");
        }
        self.next_token = self
            .next_token
            .checked_add(1)
            .ok_or("device gesture limit reached")?;
        Ok(GestureKey {
            generation,
            token: self.next_token,
        })
    }

    pub fn begin(
        &mut self,
        generation: u64,
        target: GestureTarget,
        initial: ParameterValue,
    ) -> Result<GestureKey, &'static str> {
        if matches!(target, GestureTarget::PluginParameter { .. }) {
            return Err("plugin gesture requires its observed source context");
        }
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
            generation,
            token: self.next_token,
        };
        self.active = Some(GestureSnapshot {
            track_gains: None,
            touched_parameters: crate::device_gesture::TouchedParameters::default(),
            manual_edit: false,
            key,
            target,
            sequence: 0,
            value: initial,
            secondary_value: None,
            parameters: None,
            creation: None,
            plugin_value: None,
        });
        self.origin_parameters = None;
        self.accepted_pair = None;
        self.stranded_commit = None;
        self.pending_begin = Some(GestureRequest::Begin { key, target });
        Ok(key)
    }

    pub fn begin_track_gains(
        &mut self,
        generation: u64,
        group: hex_project::RelativeTrackGains,
    ) -> Result<GestureKey, &'static str> {
        let key = self.begin(
            generation,
            GestureTarget::TrackGains {
                track: group.anchor(),
            },
            ParameterValue::Float(group.anchor_gain()),
        )?;
        if let Some(active) = self.active.as_mut() {
            active.track_gains = Some(group);
        }
        self.pending_begin = Some(GestureRequest::BeginTrackGains { key, group });
        Ok(key)
    }

    pub fn begin_pair(
        &mut self,
        generation: u64,
        device: DeviceId,
        first: ParameterId,
        second: ParameterId,
        initial: ParameterPairValue,
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
        if first == second {
            return Err("paired device gesture needs distinct parameters");
        }
        self.next_token = self.next_token.saturating_add(1).max(1);
        let key = GestureKey {
            generation,
            token: self.next_token,
        };
        let target = GestureTarget::DeviceParameterPair {
            device,
            first,
            second,
        };
        self.active = Some(GestureSnapshot {
            track_gains: None,
            touched_parameters: crate::device_gesture::TouchedParameters::default(),
            manual_edit: false,
            key,
            target,
            sequence: 0,
            value: initial.first,
            secondary_value: Some(initial.second),
            parameters: None,
            creation: None,
            plugin_value: None,
        });
        self.origin_parameters = None;
        self.accepted_pair = None;
        self.stranded_commit = None;
        self.pending_begin = Some(GestureRequest::Begin { key, target });
        Ok(key)
    }

    pub fn begin_snapshot(
        &mut self,
        generation: u64,
        device: DeviceId,
        initial: DeviceParameterSnapshot,
    ) -> Result<GestureKey, &'static str> {
        let key = self.begin(
            generation,
            GestureTarget::DeviceParameterSnapshot { device },
            ParameterValue::Float(0.0),
        )?;
        self.origin_parameters = Some(initial);
        self.active
            .as_mut()
            .ok_or("device snapshot gesture did not begin")?
            .parameters = Some(initial);
        self.pending_begin = Some(GestureRequest::BeginSnapshot {
            key,
            device,
            snapshot: initial,
        });
        Ok(key)
    }

    pub fn change_snapshot(&mut self, value: DeviceParameterSnapshot) -> Result<u64, &'static str> {
        self.update_snapshot(value, false)
    }

    pub fn finish_snapshot(&mut self, value: DeviceParameterSnapshot) -> Result<(), &'static str> {
        self.update_snapshot(value, true).map(|_| ())
    }

    fn update_snapshot(
        &mut self,
        value: DeviceParameterSnapshot,
        terminal: bool,
    ) -> Result<u64, &'static str> {
        if self.has_pending_terminal() {
            return Err("device gesture is finishing");
        }
        let active = self.active.as_mut().ok_or("no device gesture is active")?;
        if !active
            .parameters
            .is_some_and(|initial| initial.same_parameters(&value))
        {
            return Err("snapshot gesture parameters changed");
        }
        active.touched_parameters.include_changed(
            self.origin_parameters.ok_or("snapshot origin is missing")?,
            value,
        )?;
        active.manual_edit = !active.touched_parameters.is_empty();
        active.sequence = active.sequence.saturating_add(1).max(1);
        active.parameters = Some(value);
        let request = if terminal {
            GestureRequest::CommitSnapshot {
                touched_parameters: active.touched_parameters,
                key: active.key,
                sequence: active.sequence,
                snapshot: value,
            }
        } else {
            GestureRequest::ChangeSnapshot {
                touched_parameters: active.touched_parameters,
                key: active.key,
                sequence: active.sequence,
                snapshot: value,
            }
        };
        if terminal {
            self.latest_change = None;
            self.terminal = Some(request);
        } else {
            self.latest_change = Some(request);
        }
        self.stranded_commit = None;
        Ok(active.sequence)
    }

    pub fn change(&mut self, value: ParameterValue) -> Result<u64, &'static str> {
        if self.has_pending_terminal() {
            return Err("device gesture is finishing");
        }
        let Some(snapshot) = self.active.as_mut() else {
            return Err("no device gesture is active");
        };
        if matches!(
            snapshot.target,
            GestureTarget::DeviceParameterPair { .. }
                | GestureTarget::DeviceParameterSnapshot { .. }
                | GestureTarget::EqNodeCreation { .. }
                | GestureTarget::PluginParameter { .. }
        ) {
            return Err("active gesture requires paired values");
        }
        snapshot.manual_edit = true;
        snapshot.sequence = snapshot.sequence.saturating_add(1).max(1);
        snapshot.value = value;
        snapshot.secondary_value = None;
        self.stranded_commit = None;
        self.latest_change = Some(GestureRequest::Change {
            key: snapshot.key,
            sequence: snapshot.sequence,
            value,
        });
        Ok(snapshot.sequence)
    }

    pub fn change_pair(&mut self, value: ParameterPairValue) -> Result<u64, &'static str> {
        if self.has_pending_terminal() {
            return Err("device gesture is finishing");
        }
        let Some(snapshot) = self.active.as_mut() else {
            return Err("no device gesture is active");
        };
        if !matches!(snapshot.target, GestureTarget::DeviceParameterPair { .. }) {
            return Err("active gesture does not accept paired values");
        }
        let redundant = match self.latest_change {
            Some(GestureRequest::ChangePair { value: queued, .. }) => queued == value,
            Some(_) => false,
            None => match self.in_flight {
                Some(GestureRequest::ChangePair {
                    value: inflight, ..
                }) => inflight == value,
                None if self.pending_begin.is_none() && !self.recovery_pending => {
                    self.accepted_pair == Some(value)
                }
                _ => false,
            },
        };
        if redundant {
            return Ok(snapshot.sequence);
        }
        snapshot.manual_edit = true;
        snapshot.sequence = snapshot.sequence.saturating_add(1).max(1);
        snapshot.value = value.first;
        snapshot.secondary_value = Some(value.second);
        self.stranded_commit = None;
        self.latest_change = Some(GestureRequest::ChangePair {
            key: snapshot.key,
            sequence: snapshot.sequence,
            value,
        });
        Ok(snapshot.sequence)
    }

    pub fn finish(&mut self, value: ParameterValue) -> Result<(), &'static str> {
        if self.has_pending_terminal() {
            return Err("device gesture is already finishing");
        }
        let Some(snapshot) = self.active.as_mut() else {
            return Err("no device gesture is active");
        };
        if matches!(
            snapshot.target,
            GestureTarget::DeviceParameterPair { .. }
                | GestureTarget::DeviceParameterSnapshot { .. }
                | GestureTarget::EqNodeCreation { .. }
                | GestureTarget::PluginParameter { .. }
        ) {
            return Err("active gesture requires paired values");
        }
        snapshot.manual_edit |= snapshot.value != value;
        snapshot.sequence = snapshot.sequence.saturating_add(1).max(1);
        snapshot.value = value;
        snapshot.secondary_value = None;
        self.stranded_commit = None;
        self.latest_change = None;
        self.terminal = Some(GestureRequest::Commit {
            manual_edit: snapshot.manual_edit,
            key: snapshot.key,
            sequence: snapshot.sequence,
            value,
        });
        Ok(())
    }

    pub fn finish_pair(&mut self, value: ParameterPairValue) -> Result<(), &'static str> {
        if self.has_pending_terminal() {
            return Err("device gesture is already finishing");
        }
        let Some(snapshot) = self.active.as_mut() else {
            return Err("no device gesture is active");
        };
        if !matches!(snapshot.target, GestureTarget::DeviceParameterPair { .. }) {
            return Err("active gesture does not accept paired values");
        }
        snapshot.manual_edit |=
            snapshot.value != value.first || snapshot.secondary_value != Some(value.second);
        snapshot.sequence = snapshot.sequence.saturating_add(1).max(1);
        snapshot.value = value.first;
        snapshot.secondary_value = Some(value.second);
        self.stranded_commit = None;
        self.latest_change = None;
        self.terminal = Some(GestureRequest::CommitPair {
            manual_edit: snapshot.manual_edit,
            key: snapshot.key,
            sequence: snapshot.sequence,
            value,
        });
        Ok(())
    }

    pub fn cancel(&mut self) -> Result<(), &'static str> {
        if self.has_pending_terminal() {
            return Err("device gesture is already finishing");
        }
        let Some(snapshot) = self.active else {
            return Err("no device gesture is active");
        };
        self.stranded_commit = None;
        self.latest_change = None;
        self.terminal = Some(GestureRequest::Cancel { key: snapshot.key });
        Ok(())
    }

    /// Finalize the currently authored overlay without dropping compound values.
    pub fn finish_current(&mut self) -> Result<(), &'static str> {
        let active = self.active.ok_or("no device gesture is active")?;
        if let Some(value) = active.plugin_value {
            self.finish_plugin(value)
        } else if let Some(creation) = active.creation {
            self.finish_eq_creation(creation)
        } else if let Some(parameters) = active.parameters {
            self.finish_snapshot(parameters)
        } else if let Some(second) = active.secondary_value {
            self.finish_pair(ParameterPairValue {
                first: active.value,
                second,
            })
        } else {
            self.finish(active.value)
        }
    }

    /// Reserve the next protocol request before attempting the bounded service
    /// submit. On queue-full, call this again to retrieve the exact same request.
    pub fn next_request(&mut self) -> Option<GestureRequest> {
        if self.submitted {
            return None;
        }
        if let Some(in_flight) = self.in_flight {
            return Some(in_flight);
        }
        let request = self
            .pending_begin
            .take()
            .or_else(|| self.terminal.take())
            .or_else(|| self.latest_change.take())?;
        self.in_flight = Some(request);
        Some(request)
    }

    pub fn mark_submitted(&mut self) {
        if self.in_flight.is_some() {
            self.submitted = true;
        }
    }

    pub fn reset(&mut self) {
        self.origin_parameters = None;
        self.accepted_pair = None;
        self.recovery_pending = false;
        self.stranded_commit = None;
        self.active = None;
        self.in_flight = None;
        self.submitted = false;
        self.pending_begin = None;
        self.latest_change = None;
        self.terminal = None;
    }

    /// Clear cached paired-preview acceptance after an audio output reset or
    /// owner change so the next identical sample reaches the worker.
    pub fn invalidate_accepted_preview(&mut self) {
        self.accepted_pair = None;
    }

    /// A matching acknowledgment releases the protocol slot. Stale or unrelated
    /// responses leave the current gesture and its reserved terminal untouched.
    pub fn acknowledge(&mut self, ack: &GestureAck) -> bool {
        self.acknowledge_with_snapshot(ack, None)
    }

    /// Reconcile an exact terminal refusal with the worker's accepted scalar overlay. Missing
    /// or unrelated snapshots leave the in-flight protocol slot untouched. A retained overlay
    /// is parked without an automatic Commit; callers may retry or explicitly Cancel it.
    pub fn acknowledge_with_snapshot(
        &mut self,
        ack: &GestureAck,
        worker_active: Option<GestureSnapshot>,
    ) -> bool {
        let Some(request) = self.in_flight else {
            return false;
        };
        if request.key() != ack.key
            || request.phase() != ack.phase
            || request.sequence() != ack.sequence
        {
            return false;
        }
        if ack.source_lost {
            if ack.accepted
                || ack.preview_retained
                || !self.active.is_some_and(|active| {
                    matches!(active.target, GestureTarget::PluginParameter { .. })
                })
            {
                return false;
            }
            self.reset();
            return true;
        }
        let retained = if ack.preview_retained {
            if ack.accepted || !matches!(ack.phase, GesturePhase::Commit | GesturePhase::Cancel) {
                return false;
            }
            let Some(snapshot) = worker_active.filter(|snapshot| {
                snapshot.key == ack.key
                    && self
                        .active
                        .is_some_and(|active| active.target == snapshot.target)
            }) else {
                return false;
            };
            Some(snapshot)
        } else {
            None
        };
        self.in_flight = None;
        self.submitted = false;
        if let GestureRequest::ChangePair { value, .. } = request {
            if ack.accepted && retained.is_none() {
                self.accepted_pair = Some(value);
            } else {
                self.accepted_pair = None;
            }
        }
        if let Some(snapshot) = retained {
            self.active = Some(snapshot);
            self.accepted_pair = None;
            self.pending_begin = None;
            self.latest_change = None;
            self.terminal = None;
            self.recovery_pending = true;
            self.stranded_commit = (request.phase() == GesturePhase::Commit).then_some(request);
            return true;
        }
        if !ack.accepted && ack.phase == GesturePhase::Begin {
            self.active = None;
            self.accepted_pair = None;
            self.stranded_commit = None;
            self.pending_begin = None;
            self.latest_change = None;
            self.terminal = None;
        } else if matches!(ack.phase, GesturePhase::Commit | GesturePhase::Cancel) {
            self.recovery_pending = false;
            self.stranded_commit = None;
            self.active = None;
            self.accepted_pair = None;
            self.latest_change = None;
            self.terminal = None;
            self.pending_begin = None;
        }
        true
    }

    /// A refused native terminal left an audible overlay which needs retry or Cancel.
    pub fn is_recovery_pending(&self) -> bool {
        self.recovery_pending
    }

    /// True when any Begin, Change, or terminal gesture request is queued or in flight.
    pub fn has_pending_requests(&self) -> bool {
        self.in_flight.is_some()
            || self.pending_begin.is_some()
            || self.latest_change.is_some()
            || self.terminal.is_some()
    }

    /// True when a released `Commit*` was refused with a retained overlay and
    /// has not been superseded by another drag change or explicit `Cancel`.
    pub fn has_stranded_commit(&self) -> bool {
        self.recovery_pending && self.stranded_commit.is_some()
    }

    /// Exact released commit intent retained when a terminal Commit was refused
    /// with an audible overlay. Returns `None` when no commit is stranded or when
    /// a Cancel was refused instead.
    pub fn reconcilable_snapshot(&self) -> Option<GestureSnapshot> {
        if !self.recovery_pending {
            return None;
        }
        let mut snapshot = self.active?;
        let commit = self.stranded_commit?;
        if commit.key() != snapshot.key {
            return None;
        }
        match commit {
            GestureRequest::Commit {
                sequence,
                manual_edit,
                value,
                ..
            } => {
                snapshot.sequence = sequence;
                snapshot.manual_edit |= manual_edit;
                snapshot.value = value;
                Some(snapshot)
            }
            GestureRequest::CommitPair {
                sequence,
                manual_edit,
                value,
                ..
            } => {
                snapshot.sequence = sequence;
                snapshot.manual_edit |= manual_edit;
                snapshot.value = value.first;
                snapshot.secondary_value = Some(value.second);
                Some(snapshot)
            }
            GestureRequest::CommitSnapshot {
                touched_parameters,
                sequence,
                snapshot: parameters,
                ..
            } => {
                snapshot.sequence = sequence;
                let _ = snapshot
                    .touched_parameters
                    .merge(touched_parameters, parameters);
                snapshot.manual_edit |= !snapshot.touched_parameters.is_empty();
                snapshot.parameters = Some(parameters);
                Some(snapshot)
            }
            GestureRequest::CommitEqCreation {
                sequence,
                snapshot: creation,
                ..
            } => {
                snapshot.sequence = sequence;
                snapshot.manual_edit = true;
                snapshot.creation = Some(creation);
                Some(snapshot)
            }
            GestureRequest::CommitPlugin {
                sequence, value, ..
            } => {
                snapshot.sequence = sequence;
                snapshot.manual_edit = true;
                snapshot.plugin_value = Some(value);
                Some(snapshot)
            }
            _ => None,
        }
    }

    /// Borrowed cohort query: visible faders avoid copying the bounded control
    /// snapshot or traversing project content on every repaint.
    pub fn preview_track_gain(&self, generation: u64, track: TrackId) -> Option<f32> {
        let active = self
            .active
            .as_ref()
            .filter(|active| active.key.generation == generation)?;
        let ParameterValue::Float(value) = active.value else {
            return None;
        };
        match active.target {
            GestureTarget::TrackGain { track: anchor } if anchor == track => Some(value),
            GestureTarget::TrackGains { .. } => active
                .track_gains
                .as_ref()?
                .gain_for(track, value)
                .ok()
                .flatten(),
            _ => None,
        }
    }

    pub fn snapshot(&self) -> Option<GestureSnapshot> {
        self.active
    }

    /// Exact terminal owned by this coalescer, whether reserved or in flight.
    pub fn terminal(&self) -> Option<(GesturePhase, u64)> {
        self.terminal
            .or(self.in_flight.filter(|request| {
                matches!(request.phase(), GesturePhase::Commit | GesturePhase::Cancel)
            }))
            .map(|request| (request.phase(), request.sequence()))
    }

    pub fn has_pending_terminal(&self) -> bool {
        self.terminal.is_some()
            || self.in_flight.is_some_and(|request| {
                matches!(request.phase(), GesturePhase::Commit | GesturePhase::Cancel)
            })
    }
}
