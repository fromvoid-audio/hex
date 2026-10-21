//! Ordinary graph custody stays on the canonical worker. Original-bank leases are inventoried
//! directly; legacy clip capacity and existing runtime buffers use separate conservative bounds.
//! A retired book survives endpoint relinquishment until a fresh exclusive native lease opens.
use super::*;
use hex_engine::{
    native_device_resources::{
        DeviceResourceAdmission, DeviceResourcePublication, PreparedDeviceResources,
    },
    playback::Playback,
};
use hex_session::service::{
    ResidentPublicationAcceptance, ResidentPublicationMetadata, ResidentPublicationPolicy,
    ResidentPublicationRefusal,
};

pub(super) struct Pending {
    playback: Playback,
    resources: Option<PreparedDeviceResources>,
    metadata: ResidentPublicationMetadata,
    policy: ResidentPublicationPolicy,
}
#[derive(Default)]
pub(super) struct Resident {
    pub(super) pending: Option<Pending>,
    pub(super) retired: Option<(DeviceResourcePublication, u64)>,
}
impl NativeAudioSink {
    pub(super) fn resident_runtime_hold(
        &self,
    ) -> Option<hex_session::service::NativeSamplerReservationHold> {
        let active = self
            .output
            .as_ref()
            .and_then(|output| output.publication.unreturned_resident_graphs_on_control());
        if active.is_none() && self.resident.retired.is_none() && self.resident.pending.is_none() {
            return None;
        }
        let bytes = (|| {
            let policy = self.policy.as_ref().ok()?;
            let retired = self
                .resident
                .retired
                .as_ref()
                .map_or(0, |(book, _)| book.unreturned_graphs_on_control());
            let owners = active.unwrap_or(0).checked_add(retired)?;
            let per_graph = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES
                .checked_add(policy.ordinary_graph_metadata_bytes)?;
            let books = usize::from(active.is_some())
                .checked_add(usize::from(self.resident.retired.is_some()))?;
            let mut bytes = owners
                .checked_mul(per_graph)?
                .checked_add(books.checked_mul(policy.control_backing_quote)?)?;
            if let Some(pending) = &self.resident.pending {
                bytes = bytes
                    .checked_add(
                        pending
                            .playback
                            .accounted_session_runtime_bytes_on_control()?,
                    )?
                    .checked_add(policy.ordinary_graph_metadata_bytes)?;
            }
            Some(bytes)
        })()
        .unwrap_or(usize::MAX);
        // Original banks and legacy clip capacities are selected by resident_inventory. These
        // scalars cover existing runtime/immutable graph budgets only, with no source projection.
        Some(hex_session::service::NativeSamplerReservationHold {
            retained_bytes: bytes,
            construction_bytes: bytes,
            frames: 0,
            current_context: None,
        })
    }
    pub(super) fn retain_resident_output(&mut self) -> bool {
        let Some(control) = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
        else {
            return false;
        };
        if control.unreturned_resident_graphs_on_control().is_none() {
            return false;
        }
        let frames = control
            .suspend_resident_forwarding_pcm()
            .unwrap_or(u64::MAX);
        if let Some(book) = control.take_device_resource_publication_on_shutdown() {
            // opened_output clears the prior hold only after its constructor acquires the
            // exclusive lease, before any new publication can create this retirement book.
            self.resident.retired = Some((book, frames));
        }
        self.output_hold = Default::default();
        self.ordinary_pcm_frames = 0;
        self.verified_pcm_frames = 0;
        true
    }
    pub(super) fn resident_inventory(&self) -> [DeviceResourceAdmission<'_>; 3] {
        [
            self.output
                .as_ref()
                .and_then(|output| output.publication.device_resource_admission_on_control())
                .unwrap_or_default(),
            self.resident
                .retired
                .as_ref()
                .map_or(DeviceResourceAdmission::default(), |(book, frames)| {
                    book.admission_on_control(*frames)
                }),
            self.resident
                .pending
                .as_ref()
                .map_or(DeviceResourceAdmission::default(), |pending| {
                    DeviceResourceAdmission::unpublished_on_control(
                        &pending.playback,
                        pending.resources.as_ref(),
                    )
                }),
        ]
    }
    #[allow(clippy::too_many_arguments, clippy::result_large_err)] // Explicit admission inputs; refusal returns the exact graph.
    pub(super) fn publish_resident_graph(
        &mut self,
        playback: Playback,
        resources: Option<&PreparedDeviceResources>,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        metadata_limit: usize,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
    ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
        // Active-output refusal never enters the offline slot. The prospective view already
        // includes the active book, so it must not also be selected from resident_inventory.
        if let Some(control) = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
        {
            control.reclaim();
            let output_identity = match control.output_identity() {
                Some(identity) => identity,
                None => {
                    return Err(ResidentPublicationRefusal {
                        playback,
                        error: "audio output identity unavailable".into(),
                    });
                }
            };
            let policy = match policy {
                ResidentPublicationPolicy::InheritRuntime => {
                    hex_audio::prepared::PublicationPolicy::InheritRuntime
                }
                ResidentPublicationPolicy::FreshIfUnchanged(expected) => {
                    let Some(actual) = control.capture_fresh_runtime_fence().filter(|actual| {
                        expected.output_identity == output_identity
                            && expected.active_audio_revision == actual.expected_active_revision()
                            && expected.route_generation == actual.route_state_generation()
                            && expected.admitted_command_ticket == actual.admitted_command_ticket()
                    }) else {
                        return Err(ResidentPublicationRefusal {
                            playback,
                            error: "audio runtime changed while preparing graph".into(),
                        });
                    };
                    hex_audio::prepared::PublicationPolicy::InstallFreshRuntimeIfUnchanged(actual)
                }
            };
            let retired = self
                .resident
                .retired
                .as_ref()
                .map_or(DeviceResourceAdmission::default(), |(book, frames)| {
                    book.admission_on_control(*frames)
                });
            let pending = self.resident.pending.as_ref().map_or(
                DeviceResourceAdmission::default(),
                |pending| {
                    DeviceResourceAdmission::unpublished_on_control(
                        &pending.playback,
                        pending.resources.as_ref(),
                    )
                },
            );
            let result = control.publish_with_admission_on_control(
                playback,
                policy,
                metadata.context.project_generation,
                |prospective| admit(&[prospective, retired, pending]),
            );
            return match result {
                Ok(audio_revision) => {
                    self.observe(
                        metadata.context,
                        "submitted",
                        EditorPublicationState::Submitted { audio_revision },
                    );
                    Ok(ResidentPublicationAcceptance::Submitted {
                        output_identity,
                        audio_revision,
                    })
                }
                Err(hex_audio::prepared::AdmittedPublishError::Admission { reason, playback }) => {
                    Err(ResidentPublicationRefusal {
                        playback,
                        error: reason,
                    })
                }
                Err(hex_audio::prepared::AdmittedPublishError::Publication(error)) => {
                    Err(ResidentPublicationRefusal {
                        playback: error.playback,
                        error: error.reason.into(),
                    })
                }
            };
        }
        if self.output.is_some() {
            return Err(ResidentPublicationRefusal {
                playback,
                error: "active audio publication endpoint is closed".into(),
            });
        }
        if matches!(policy, ResidentPublicationPolicy::FreshIfUnchanged(_)) {
            return Err(ResidentPublicationRefusal {
                playback,
                error: "fresh runtime publication requires its original active output".into(),
            });
        }
        // Only explicit unavailable/opening output state reaches here. The old pending owner
        // stays selected until the exact new owner has passed full control-union admission.
        let captured = match resources
            .map(|resources| resources.capture_on_control(metadata_limit))
            .transpose()
        {
            Ok(captured) => captured,
            Err(error) => {
                return Err(ResidentPublicationRefusal {
                    playback,
                    error: format!("resource capture refused: {error:?}"),
                });
            }
        };
        let candidate =
            DeviceResourceAdmission::unpublished_on_control(&playback, captured.as_ref());
        let current = self.resident_inventory();
        if let Err(error) = admit(&[candidate, current[0], current[1], current[2]]) {
            return Err(ResidentPublicationRefusal { playback, error });
        }
        self.resident.pending = Some(Pending {
            playback,
            resources: captured,
            metadata,
            policy,
        });
        Ok(ResidentPublicationAcceptance::OfflineRetained)
    }
    pub(super) fn retry_resident_pending(
        &mut self,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
    ) {
        if self.output.is_none() {
            return;
        }
        let Some(pending) = self.resident.pending.take() else {
            return;
        };
        let Pending {
            playback,
            resources,
            metadata,
            policy,
        } = pending;
        if let Err(refused) =
            self.publish_resident_graph(playback, resources.as_ref(), metadata, policy, 0, admit)
        {
            self.resident.pending = Some(Pending {
                playback: refused.playback,
                resources,
                metadata,
                policy,
            });
        }
    }
}
