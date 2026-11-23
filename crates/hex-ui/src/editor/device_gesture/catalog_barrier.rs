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
        }
        | Request::InsertBrowserInstrumentTrack {
            project_generation,
            revision,
            ..
        }
        | Request::DropBrowserDeviceOnArrangement {
            project_generation,
            revision,
            ..
        }
        | Request::DropNativeDevice {
            project_generation,
            revision,
            ..
        }
        | Request::MoveTrackDevices {
            project_generation,
            revision,
            ..
        }
        | Request::ReorderDevices {
            project_generation,
            revision,
            ..
        }
        | Request::DeviceClipboard {
            project_generation,
            revision,
            ..
        } => Some((*project_generation, *revision)),
        Request::InsertPluginEffect(insertion) => Some((
            insertion.context.project_generation,
            insertion.context.editor_revision,
        )),
        _ => None,
    }
}

fn owner_exists(project: &hex_project::Project, owner: hex_project::DeviceChainOwner) -> bool {
    match owner {
        hex_project::DeviceChainOwner::Track(id) => project.track(id).is_some(),
        hex_project::DeviceChainOwner::Return(id) => {
            project.return_tracks().iter().any(|bus| bus.id() == id)
        }
        hex_project::DeviceChainOwner::Main => true,
    }
}

fn chain_contains(
    project: &hex_project::Project,
    owner: hex_project::DeviceChainOwner,
    device: hex_project::DeviceId,
) -> bool {
    match owner {
        hex_project::DeviceChainOwner::Track(id) => project.track(id).is_some_and(|track| {
            track.ordered_device_chain().any(|node| match node {
                hex_project::TrackDeviceRef::Instrument(candidate)
                | hex_project::TrackDeviceRef::Effect(candidate) => candidate.id() == device,
            })
        }),
        hex_project::DeviceChainOwner::Return(id) => project
            .return_tracks()
            .iter()
            .find(|bus| bus.id() == id)
            .is_some_and(|bus| {
                bus.devices()
                    .iter()
                    .any(|candidate| candidate.id() == device)
            }),
        hex_project::DeviceChainOwner::Main => project
            .main_devices()
            .iter()
            .any(|candidate| candidate.id() == device),
    }
}

fn valid_intent(project: &hex_project::Project, request: &Request) -> bool {
    match request {
        Request::LoadTrackCatalogDevice {
            track,
            class,
            type_id,
            placement,
            ..
        } => {
            project
                .track(*track)
                .is_some_and(|track| placement.matches_track(*class, track))
                && registered(*class, *type_id)
        }
        Request::InsertEffectBusDevice {
            destination,
            class,
            type_id,
            before,
            ..
        } => {
            if *class != hex_engine::NativeDeviceClass::Effect {
                return false;
            }
            let devices = match destination {
                hex_session::service::EffectBusDestination::Main => project.main_devices(),
                hex_session::service::EffectBusDestination::Return(id) => {
                    let Some(bus) = project.return_tracks().iter().find(|bus| bus.id() == *id)
                    else {
                        return false;
                    };
                    bus.devices()
                }
            };
            before.is_none_or(|anchor| devices.iter().any(|device| device.id() == anchor))
                && registered(*class, *type_id)
        }
        Request::InsertBrowserInstrumentTrack { class, type_id, .. } => {
            registered(*class, *type_id)
        }
        Request::DropBrowserDeviceOnArrangement {
            class,
            type_id,
            destination,
            ..
        } => {
            registered(*class, *type_id)
                && match destination {
                    hex_session::service::ArrangementDeviceDestination::NewTrack {
                        before_track,
                    } => before_track.is_none_or(|track| project.track(track).is_some()),
                    hex_session::service::ArrangementDeviceDestination::ExistingTrack {
                        track,
                        instrument,
                    } => project.track(*track).is_some_and(|target| {
                        target.instrument().map(hex_project::Device::id) == *instrument
                    }),
                }
        }
        Request::DropNativeDevice {
            source,
            track,
            before,
            replace,
            ..
        } => {
            let Some(target) = project.track(*track) else {
                return false;
            };
            let source_valid = match source {
                hex_session::service::NativeDeviceDropSource::Browser { class, type_id } => {
                    registered(*class, *type_id)
                }
                hex_session::service::NativeDeviceDropSource::Existing { device, .. } => {
                    project.device(*device).is_some()
                }
            };
            let before_valid = before.is_none_or(|anchor| {
                chain_contains(
                    project,
                    hex_project::DeviceChainOwner::Track(*track),
                    anchor,
                )
            });
            let replace_valid = replace.is_none_or(|instrument| {
                target.instrument().map(hex_project::Device::id) == Some(instrument)
            });
            source_valid && before_valid && replace_valid
        }
        Request::MoveTrackDevices {
            source,
            selected,
            destination,
            ..
        } => {
            project.track(*source).is_some()
                && project.track(*destination).is_some()
                && selected.ids().iter().all(|id| {
                    chain_contains(project, hex_project::DeviceChainOwner::Track(*source), *id)
                })
        }
        Request::ReorderDevices {
            owner,
            selected,
            before,
            ..
        } => {
            owner_exists(project, *owner)
                && selected
                    .ids()
                    .iter()
                    .all(|id| chain_contains(project, *owner, *id))
                && before.is_none_or(|anchor| chain_contains(project, *owner, anchor))
        }
        Request::DeviceClipboard { action, .. } => match action {
            hex_session::service::DeviceClipboardAction::Copy { owner, selected }
            | hex_session::service::DeviceClipboardAction::Cut { owner, selected } => {
                owner_exists(project, *owner)
                    && selected
                        .ids()
                        .iter()
                        .all(|id| chain_contains(project, *owner, *id))
            }
            hex_session::service::DeviceClipboardAction::Paste { owner, before } => {
                owner_exists(project, *owner)
                    && before.is_none_or(|anchor| chain_contains(project, *owner, anchor))
            }
            hex_session::service::DeviceClipboardAction::Duplicate {
                source,
                selected,
                destination,
                before,
            } => {
                owner_exists(project, *source)
                    && selected
                        .ids()
                        .iter()
                        .all(|id| chain_contains(project, *source, *id))
                    && owner_exists(project, *destination)
                    && before.is_none_or(|anchor| chain_contains(project, *destination, anchor))
            }
        },
        Request::InsertPluginEffect(insertion) => {
            insertion.validate(insertion.context, project).is_ok()
        }
        _ => false,
    }
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
        let controls: Vec<_> = self
            .device_gesture_barriers
            .iter()
            .filter(|request| stamps(request).is_some())
            .filter_map(super::insertion_control)
            .collect();
        let had_clipboard = self
            .device_gesture_barriers
            .iter()
            .any(|request| matches!(request, Request::DeviceClipboard { .. }));
        self.device_gesture_barriers
            .retain(|request| stamps(request).is_none());
        if self.device_gesture_barriers.len() != before {
            self.catalog_barrier.refusal = Some(reason);
            self.error = Some(reason.into());
        }
        if had_clipboard {
            self.device_panel.pending_duplicate = None;
        }
        for control in controls {
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
        if let Some(adopted) = proof.adopted_commit {
            for request in &mut self.device_gesture_barriers {
                match request {
                    Request::InsertEffectBusDevice { revision, .. }
                    | Request::LoadTrackCatalogDevice { revision, .. }
                    | Request::InsertBrowserInstrumentTrack { revision, .. }
                    | Request::DropBrowserDeviceOnArrangement { revision, .. }
                    | Request::DropNativeDevice { revision, .. }
                    | Request::MoveTrackDevices { revision, .. }
                    | Request::ReorderDevices { revision, .. }
                    | Request::DeviceClipboard { revision, .. } => {
                        *revision = adopted;
                    }
                    Request::InsertPluginEffect(insertion) => {
                        insertion.context.editor_revision = adopted;
                    }
                    _ => {}
                }
            }
            self.device_panel
                .rebase_pending_duplicate(proof.generation, proof.revision, adopted);
        }
        self.catalog_barrier.custody = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_engine::NativeDeviceClass;
    use hex_lattice_contract::DeviceTypeId;
    use hex_project::{Command, Created, DeviceChainOwner, DeviceSelectionIds, Editor, Project};
    use hex_session::service::{
        ArrangementDeviceDestination, DeviceClipboardAction, NativeDeviceDropSource,
        PluginEffectControl, PluginEffectDestination, PluginEffectInsertion, PluginEffectSelection,
    };
    use std::sync::Arc;

    #[test]
    fn stamps_and_valid_intent_cover_all_revision_stamped_device_requests() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "audio".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let utility = DeviceTypeId::from_static("hex.utility");
        let utility_state = hex_engine::default_native_device_state(utility).unwrap();
        let Created::Device(device) = editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: utility_state,
            })
            .unwrap()
        else {
            panic!("device");
        };
        let project = editor.project();
        let selected = DeviceSelectionIds::new(&[device]).unwrap();
        let plugin_insertion = PluginEffectInsertion {
            context: hex_engine::AudioSourceContext {
                project_generation: 2,
                editor_revision: 7,
            },
            selection: PluginEffectSelection {
                module: std::path::PathBuf::from("/tmp/test.vst3"),
                executable: "Contents/MacOS/test".into(),
                cid: [0x11; 16],
                executable_sha256: "a".repeat(64),
                info_sha256: "b".repeat(64),
            },
            destination: PluginEffectDestination::Track(track),
            before: None,
            track_device_name: None,
            control: Arc::new(PluginEffectControl::default()),
        };

        let requests = [
            Request::InsertBrowserInstrumentTrack {
                project_generation: 2,
                revision: 7,
                class: NativeDeviceClass::Instrument,
                type_id: DeviceTypeId::from_static("hex.instrument.sine"),
                new_track_gain: None,
                control: None,
            },
            Request::DropBrowserDeviceOnArrangement {
                project_generation: 2,
                revision: 7,
                class: NativeDeviceClass::Effect,
                type_id: utility,
                destination: ArrangementDeviceDestination::ExistingTrack {
                    track,
                    instrument: None,
                },
                new_track_gain: None,
                control: None,
            },
            Request::DropNativeDevice {
                project_generation: 2,
                revision: 7,
                source: NativeDeviceDropSource::Browser {
                    class: NativeDeviceClass::Effect,
                    type_id: utility,
                },
                track,
                before: Some(device),
                replace: None,
                control: None,
            },
            Request::MoveTrackDevices {
                project_generation: 2,
                revision: 7,
                source: track,
                selected: selected.clone(),
                destination: track,
            },
            Request::ReorderDevices {
                project_generation: 2,
                revision: 7,
                owner: DeviceChainOwner::Track(track),
                selected: selected.clone(),
                before: Some(device),
            },
            Request::DeviceClipboard {
                project_generation: 2,
                revision: 7,
                action: DeviceClipboardAction::Duplicate {
                    source: DeviceChainOwner::Track(track),
                    selected,
                    destination: DeviceChainOwner::Track(track),
                    before: None,
                },
            },
            Request::InsertPluginEffect(plugin_insertion),
        ];

        for request in &requests {
            assert_eq!(
                stamps(request),
                Some((2, 7)),
                "missing stamp for {request:?}"
            );
            assert!(
                valid_intent(project, request),
                "expected valid_intent for {request:?}"
            );
        }
    }
}
