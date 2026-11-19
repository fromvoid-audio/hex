//! A catalog addition may follow only the exact local terminal it was admitted behind.
use super::super::EditorView;
use hex_session::{
    device_gesture::{GestureAck, GestureKey, GesturePhase},
    service::{Event, Request},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::editor) struct CatalogCustody {
    key: GestureKey,
    generation: u64,
    revision: u64,
    adopted_commit: Option<u64>,
}

#[derive(Default)]
pub(in crate::editor) struct CatalogBarrierState {
    pub(super) custody: Option<CatalogCustody>,
    refusal: Option<&'static str>,
}

fn stamps(request: &Request) -> Option<(u64, u64)> {
    match request {
        Request::InsertEffectBusDevice {
            project_generation,
            revision,
            ..
        }
        | Request::LoadTrackCatalogDevice {
            project_generation,
            revision,
            ..
        } => Some((*project_generation, *revision)),
        _ => None,
    }
}

fn valid_intent(project: &hex_project::Project, request: &Request) -> bool {
    if let Request::LoadTrackCatalogDevice {
        track,
        class,
        type_id,
        placement,
        ..
    } = request
    {
        return project
            .track(*track)
            .is_some_and(|track| placement.matches_track(*class, track))
            && registered(*class, *type_id);
    }
    let Request::InsertEffectBusDevice {
        destination,
        class,
        type_id,
        before,
        ..
    } = request
    else {
        return false;
    };
    if *class != hex_engine::NativeDeviceClass::Effect {
        return false;
    }
    let devices = match destination {
        hex_session::service::EffectBusDestination::Main => project.main_devices(),
        hex_session::service::EffectBusDestination::Return(id) => {
            let Some(bus) = project.return_tracks().iter().find(|bus| bus.id() == *id) else {
                return false;
            };
            bus.devices()
        }
    };
    before.is_none_or(|anchor| devices.iter().any(|device| device.id() == anchor))
        && registered(*class, *type_id)
}

fn registered(
    class: hex_engine::NativeDeviceClass,
    type_id: hex_lattice_contract::DeviceTypeId,
) -> bool {
    hex_engine::native_device_catalog_for_class(class)
        .is_ok_and(|catalog| catalog.iter().any(|entry| entry.type_id == type_id))
}

impl EditorView {
    pub(in crate::editor) fn take_catalog_barrier_error(&mut self) -> Option<&'static str> {
        self.catalog_barrier.refusal.take()
    }

    pub(in crate::editor) fn catalog_custody(
        &self,
        request: &Request,
    ) -> Result<Option<CatalogCustody>, &'static str> {
        let Some((project_generation, revision)) = stamps(request) else {
            return Ok(None);
        };
        let snapshot = self
            .device_gesture
            .snapshot()
            .ok_or("device load lost its device terminal")?;
        let state = self
            .state
            .as_ref()
            .ok_or("device load requires a current project")?;
        if project_generation != self.project_generation
            || snapshot.key.generation != self.project_generation
            || revision != state.revision
            || snapshot.key.token == u64::MAX
            || revision.checked_add(1).is_none()
            || !valid_intent(&state.project, request)
        {
            return Err("device load canceled · destination or project changed");
        }
        let proof = CatalogCustody {
            key: snapshot.key,
            generation: project_generation,
            revision,
            adopted_commit: None,
        };
        if self
            .catalog_barrier
            .custody
            .is_some_and(|current| current != proof)
        {
            return Err("device load canceled · device terminal changed");
        }
        Ok(Some(proof))
    }

    pub(in crate::editor) fn refuse_catalog_barriers(&mut self, reason: &'static str) {
        self.catalog_barrier.custody = None;
        let before = self.device_gesture_barriers.len();
        let control = self
            .device_gesture_barriers
            .iter()
            .filter(|request| stamps(request).is_some())
            .find_map(|request| request.builtin_device_insert_control().cloned());
        self.device_gesture_barriers
            .retain(|request| stamps(request).is_none());
        if self.device_gesture_barriers.len() != before {
            self.catalog_barrier.refusal = Some(reason);
            self.error = Some(reason.into());
        }
        if let Some(control) = control {
            self.retire_plugin_effect(&control, false, Some(reason));
        }
    }

    // Called before adopting this exact event. Other canonical transitions cannot
    // become permission to refresh an already stale addition.
    pub(in crate::editor) fn observe_catalog_barrier(&mut self, event: &Event) {
        let Some(mut proof) = self.catalog_barrier.custody else {
            return;
        };
        let lineage = !event.project_replaced
            && self.project_generation == proof.generation
            && self
                .state
                .as_ref()
                .is_some_and(|state| state.revision == proof.revision)
            && self
                .device_gesture
                .snapshot()
                .is_some_and(|snapshot| snapshot.key == proof.key);
        let commit = event.gesture_protocol_ack
            && event.gesture_ack.as_ref().is_some_and(|ack| {
                ack.key == proof.key && ack.phase == GesturePhase::Commit && ack.accepted
            });
        let revision = event.state.revision;
        let terminal_refusal = event.gesture_protocol_ack
            && event.gesture_ack.as_ref().is_some_and(|ack| {
                ack.key != proof.key || !ack.accepted || ack.phase == GesturePhase::Cancel
            });
        if !lineage
            || terminal_refusal
            || (revision != proof.revision
                && !(commit && proof.revision.checked_add(1) == Some(revision)))
        {
            self.refuse_catalog_barriers(
                "device load canceled · its device terminal or project changed",
            );
        } else if commit {
            proof.adopted_commit = Some(revision);
            self.catalog_barrier.custody = Some(proof);
        }
    }

    // The ordinary GestureClient has already matched the actual key, phase and
    // sequence. The current state is the canonical state adopted from that ACK.
    pub(in crate::editor) fn settle_catalog_barriers(&mut self, ack: &GestureAck) {
        let Some(proof) = self.catalog_barrier.custody else {
            self.refuse_catalog_barriers("device load canceled · device terminal proof is missing");
            return;
        };
        let valid = ack.accepted
            && ack.phase == GesturePhase::Commit
            && ack.key == proof.key
            && self.project_generation == proof.generation
            && self.state.as_ref().is_some_and(|state| {
                proof.adopted_commit == Some(state.revision)
                    && self.device_gesture_barriers.iter().all(|request| {
                        stamps(request).is_none_or(|context| {
                            context == (proof.generation, proof.revision)
                                && valid_intent(&state.project, request)
                        })
                    })
            });
        if !valid {
            self.refuse_catalog_barriers(
                "device load canceled · its device edit was not committed",
            );
            return;
        }
        for request in &mut self.device_gesture_barriers {
            if let Request::InsertEffectBusDevice { revision, .. }
            | Request::LoadTrackCatalogDevice { revision, .. } = request
            {
                // The Some value was verified against the actual adopted state above.
                if let Some(adopted) = proof.adopted_commit {
                    *revision = adopted;
                }
            }
        }
        self.catalog_barrier.custody = None;
    }
}
