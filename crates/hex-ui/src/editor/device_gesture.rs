use super::EditorView;
use hex_session::device_gesture::{GesturePhase, GestureTarget, ParameterValue};
use hex_session::service::{Request, SubmitError};
use std::collections::VecDeque;

fn defer_request(
    pending: &mut VecDeque<Request>,
    deferred: &mut VecDeque<Request>,
    request: Request,
) -> bool {
    if pending.len() + deferred.len() >= super::UI_REQUEST_CAPACITY {
        return false;
    }
    deferred.push_back(request);
    true
}

pub(super) fn release_deferred(pending: &mut VecDeque<Request>, deferred: &mut VecDeque<Request>) {
    pending.append(deferred);
}

pub(super) fn retire_deferred(
    pending: &mut VecDeque<Request>,
    deferred: &mut VecDeque<Request>,
    project_replaced: bool,
) {
    if project_replaced {
        deferred.clear();
    } else {
        release_deferred(pending, deferred);
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
struct FailedBarrierEffects {
    save_refused: bool,
    export_refused: bool,
}

fn retire_failed_finalize_barriers(deferred: &mut VecDeque<Request>) -> FailedBarrierEffects {
    let mut kept = VecDeque::with_capacity(deferred.len());
    let mut effects = FailedBarrierEffects::default();
    while let Some(request) = deferred.pop_front() {
        match request {
            Request::SaveNew(_) | Request::SaveCurrent => effects.save_refused = true,
            Request::Export(_) => effects.export_refused = true,
            Request::Undo | Request::Redo => {}
            Request::Open(_) => kept.push_back(request),
            request => kept.push_back(request),
        }
    }
    *deferred = kept;
    effects
}

pub(super) fn actor_retired_gesture(
    project_replaced: bool,
    actor_active: bool,
    ack: Option<&hex_session::device_gesture::GestureAck>,
    local_key: Option<hex_session::device_gesture::GestureKey>,
) -> bool {
    project_replaced
        || (!actor_active
            && ack.is_some_and(|ack| ack.reason.is_some() && Some(ack.key) == local_key))
}

impl EditorView {
    /// Start the worker-owned native parameter edit used by device controls.
    #[allow(dead_code)] // The native device panel binds this interaction contract in its own request.
    pub fn begin_device_gesture(
        &mut self,
        target: GestureTarget,
        initial: ParameterValue,
    ) -> Result<(), &'static str> {
        self.device_gesture
            .begin(self.project_generation, target, initial)?;
        Ok(())
    }

    #[allow(dead_code)] // The native device panel binds this interaction contract in its own request.
    pub fn change_device_gesture(&mut self, value: ParameterValue) -> Result<(), &'static str> {
        self.device_gesture.change(value).map(|_| ())
    }

    pub(super) fn finish_device_gesture(
        &mut self,
        value: ParameterValue,
    ) -> Result<(), &'static str> {
        self.device_gesture.finish(value)
    }

    #[allow(dead_code)] // The native device panel binds this interaction contract in its own request.
    pub fn cancel_device_gesture(&mut self) -> Result<(), &'static str> {
        self.device_gesture.cancel()
    }

    pub(super) fn device_gesture_pending(&self) -> bool {
        self.device_gesture.snapshot().is_some() || !self.device_gesture_barriers.is_empty()
    }

    pub(super) fn pump_device_gesture(&mut self) -> bool {
        let Some(gesture) = self.device_gesture.next_request() else {
            return false;
        };
        match self.service.submit(Request::DeviceGesture(gesture)) {
            Ok(()) => {
                self.device_gesture.mark_submitted();
                self.worker_requests_pending = self.worker_requests_pending.saturating_add(1);
                true
            }
            Err(SubmitError::Full(_)) => false,
            Err(SubmitError::Busy(_)) => {
                self.device_gesture.reset();
                self.device_gesture_barriers.clear();
                self.error = Some("an export is already pending".into());
                true
            }
            Err(SubmitError::Closed(_)) => {
                self.device_gesture.reset();
                self.device_gesture_barriers.clear();
                self.error = Some("project worker stopped".into());
                true
            }
        }
    }

    pub(super) fn acknowledge_device_gesture(
        &mut self,
        ack: &hex_session::device_gesture::GestureAck,
    ) -> bool {
        let terminal = matches!(ack.phase, GesturePhase::Commit | GesturePhase::Cancel);
        let matched = self.device_gesture.acknowledge(ack);
        if !matched {
            return false;
        }
        if terminal {
            if !ack.accepted {
                let effects = retire_failed_finalize_barriers(&mut self.device_gesture_barriers);
                if effects.save_refused {
                    self.saving_project = false;
                }
                if effects.export_refused {
                    // Deferred exports have not been submitted to Service, so
                    // there is no worker reservation to release here.
                    self.export_pending = false;
                }
                self.error = Some(
                    ack.reason
                        .clone()
                        .unwrap_or_else(|| "device parameter edit could not be committed".into()),
                );
            }
            release_deferred(
                &mut self.pending_requests,
                &mut self.device_gesture_barriers,
            );
        } else if ack.phase == GesturePhase::Begin && !ack.accepted {
            // The worker refused to admit the draft, so release dependent
            // barriers in their accepted order without an edit to finalize.
            release_deferred(
                &mut self.pending_requests,
                &mut self.device_gesture_barriers,
            );
            self.error = Some(
                ack.reason
                    .clone()
                    .unwrap_or_else(|| "device parameter edit could not start".into()),
            );
        }
        true
    }

    pub(super) fn enqueue_device_barrier(&mut self, request: Request) -> bool {
        if self.device_gesture.snapshot().is_none() {
            return self.enqueue(request);
        }
        if self.pending_requests.len() + self.device_gesture_barriers.len()
            >= super::UI_REQUEST_CAPACITY
        {
            self.error = Some("device edit queue full · request was not applied".into());
            return false;
        }
        if !self.device_gesture.has_pending_terminal() {
            let value = self
                .device_gesture
                .snapshot()
                .map(|snapshot| snapshot.value);
            if let Some(value) = value {
                if let Err(error) = self.device_gesture.finish(value) {
                    self.error = Some(error.into());
                    return false;
                }
            }
        }
        if defer_request(
            &mut self.pending_requests,
            &mut self.device_gesture_barriers,
            request,
        ) {
            true
        } else {
            self.error = Some("device edit queue full · request was not applied".into());
            false
        }
    }

    pub(super) fn enqueue_after_device_terminal(
        &mut self,
        request: &mut Option<Request>,
    ) -> Option<bool> {
        if self.device_gesture_barriers.is_empty() && !self.device_gesture.has_pending_terminal() {
            return None;
        }
        let Some(request) = request.take() else {
            return Some(false);
        };
        if defer_request(
            &mut self.pending_requests,
            &mut self.device_gesture_barriers,
            request,
        ) {
            Some(true)
        } else {
            self.error = Some("device edit queue full · request was not applied".into());
            Some(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FailedBarrierEffects, actor_retired_gesture, defer_request, release_deferred,
        retire_deferred, retire_failed_finalize_barriers,
    };
    use hex_session::service::Request;
    use std::collections::VecDeque;

    #[test]
    fn unrelated_event_does_not_retire_a_pending_begin_or_terminal() {
        assert!(!actor_retired_gesture(
            false,
            false,
            None,
            Some(current_key())
        ));
        assert!(!actor_retired_gesture(
            false,
            false,
            Some(&ack_without_reason()),
            Some(current_key())
        ));
        assert!(!actor_retired_gesture(
            false,
            false,
            Some(&ack_with_reason()),
            Some(other_key())
        ));
    }

    #[test]
    fn rejected_rebase_or_project_replacement_retires_the_local_gesture() {
        assert!(actor_retired_gesture(
            false,
            false,
            Some(&ack_with_reason()),
            Some(current_key())
        ));
        assert!(actor_retired_gesture(
            true,
            false,
            None,
            Some(current_key())
        ));
    }

    #[test]
    fn release_save_then_later_edit_stays_fifo_with_bounded_capacity() {
        let mut pending = VecDeque::new();
        let mut deferred = VecDeque::new();
        for _ in 0..30 {
            pending.push_back(Request::Undo);
        }
        assert!(defer_request(
            &mut pending,
            &mut deferred,
            Request::SaveCurrent
        ));
        assert!(defer_request(&mut pending, &mut deferred, Request::Redo));
        assert!(!defer_request(
            &mut pending,
            &mut deferred,
            Request::SaveCurrent
        ));
        release_deferred(&mut pending, &mut deferred);
        assert_eq!(pending.len(), super::super::UI_REQUEST_CAPACITY);
        assert!(matches!(pending.get(30), Some(Request::SaveCurrent)));
        assert!(matches!(pending.get(31), Some(Request::Redo)));
    }

    #[test]
    fn invalid_rebase_releases_retained_barriers_but_project_open_retires_them() {
        let mut pending = VecDeque::new();
        let mut deferred = VecDeque::from([Request::SaveCurrent, Request::Undo]);
        retire_deferred(&mut pending, &mut deferred, false);
        assert_eq!(pending.len(), 2);
        assert!(deferred.is_empty());
        assert!(matches!(pending.pop_front(), Some(Request::SaveCurrent)));
        assert!(matches!(pending.pop_front(), Some(Request::Undo)));

        deferred.push_back(Request::SaveCurrent);
        retire_deferred(&mut pending, &mut deferred, true);
        assert!(deferred.is_empty());
        assert!(pending.is_empty());
    }

    #[test]
    fn failed_terminal_retires_finalize_barriers_and_keeps_open_and_edits_ordered() {
        let mut deferred = VecDeque::from([
            Request::SaveCurrent,
            Request::Edit(hex_project::Command::AddTrack {
                name: "later edit".into(),
            }),
            Request::Export("deferred.wav".into()),
            Request::Undo,
            Request::Open("confirmed.hex".into()),
        ]);
        let effects = retire_failed_finalize_barriers(&mut deferred);
        assert_eq!(
            effects,
            FailedBarrierEffects {
                save_refused: true,
                export_refused: true,
            }
        );
        assert_eq!(deferred.len(), 2);
        assert!(matches!(deferred.pop_front(), Some(Request::Edit(_))));
        assert!(matches!(deferred.pop_front(), Some(Request::Open(_))));
    }

    fn ack_without_reason() -> hex_session::device_gesture::GestureAck {
        hex_session::device_gesture::GestureAck {
            key: current_key(),
            phase: hex_session::device_gesture::GesturePhase::Change,
            sequence: 1,
            accepted: true,
            reason: None,
        }
    }

    fn ack_with_reason() -> hex_session::device_gesture::GestureAck {
        let mut ack = ack_without_reason();
        ack.reason = Some("device gesture target was removed".into());
        ack
    }

    fn current_key() -> hex_session::device_gesture::GestureKey {
        hex_session::device_gesture::GestureKey {
            generation: 0,
            token: 1,
        }
    }

    fn other_key() -> hex_session::device_gesture::GestureKey {
        hex_session::device_gesture::GestureKey {
            generation: 0,
            token: 2,
        }
    }
}
