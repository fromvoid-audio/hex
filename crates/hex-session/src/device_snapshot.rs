//! Bounded control-side snapshots use the existing candidate/history boundary.
use crate::{Session, SessionError};
use hex_engine::PreparedRevision;
use hex_lattice_contract::DeviceParameterSnapshot;
use hex_project::{Command, DeviceId, Editor, Project};
use std::sync::Arc;

impl Session {
    pub fn preview_device_snapshot(
        &self,
        device: DeviceId,
        snapshot: &DeviceParameterSnapshot,
        previous: Option<&PreparedRevision>,
    ) -> Result<(Arc<Project>, Arc<PreparedRevision>), SessionError> {
        let current = self
            .project()
            .device(device)
            .ok_or(SessionError::Invalid("device no longer exists"))?;
        let state = hex_engine::patch_native_device_snapshot_for_class(
            self.native_device_class(device)?,
            current.state(),
            snapshot,
        )?;
        let mut preview = Editor::new(self.project().clone(), 1)?;
        let media = self.media_bank();
        let table = self.sine_table();
        let (_, prepared) = preview.execute_prepared(
            Command::ReplaceDeviceState { device, state },
            |candidate| {
                PreparedRevision::prepare(
                    candidate,
                    &media,
                    Arc::clone(&table),
                    previous.map(|artifact| artifact.session().as_ref()),
                )
                .map(Arc::new)
                .map_err(SessionError::from)
            },
        )?;
        Ok((Arc::new(preview.project().clone()), prepared))
    }

    /// Patch the latest state, preserving every parameter outside the snapshot.
    /// The editor validates/prepares before history; identical states are no-ops.
    pub fn commit_device_snapshot(
        &mut self,
        device: DeviceId,
        snapshot: &DeviceParameterSnapshot,
        previous: Option<&PreparedRevision>,
    ) -> Result<Arc<PreparedRevision>, SessionError> {
        let current = self
            .project()
            .device(device)
            .ok_or(SessionError::Invalid("device no longer exists"))?;
        let state = hex_engine::patch_native_device_snapshot_for_class(
            self.native_device_class(device)?,
            current.state(),
            snapshot,
        )?;
        self.command_prepared_revision(Command::ReplaceDeviceState { device, state }, previous)
            .map(|(_, prepared)| prepared)
    }
}
