//! Canonical `.vstpreset` import/export through the single-Undo `Editor` history.
//!
//! Host user `.vstpreset` files remain explicitly distinct from plugin factory
//! programs (`hex_plugin_state::programs::ProgramOrigin`). Validating a preset
//! checks class identity and chunk integrity before returning or executing one
//! `Command::ReplaceDeviceState`; malformed, incompatible, or wrong-class
//! presets leave the project, revision, and Undo stack untouched.

use crate::{Session, SessionError};
use hex_plugin_state::vstpreset::{apply_vstpreset_to_state, export_state_to_vstpreset};
use hex_project::{Command, DeviceId};

impl Session {
    /// Export a canonical VST3 device's component and controller state as a
    /// Steinberg `.vstpreset` binary container.
    pub fn export_vst3_preset(
        &self,
        device: DeviceId,
        info_xml: Option<&str>,
    ) -> Result<Vec<u8>, SessionError> {
        let entry = self
            .project()
            .device(device)
            .ok_or(SessionError::Invalid("device no longer exists"))?;
        export_state_to_vstpreset(entry.state(), info_xml)
            .map_err(|error| SessionError::NativeAudio(error.to_string()))
    }

    /// Validate a Steinberg `.vstpreset` binary payload against an existing
    /// VST3 device and return the single canonical `Command::ReplaceDeviceState`
    /// transaction without mutating session state.
    ///
    /// Restores both component and controller chunks when present, retains the
    /// device's verified binary identity and host gain metadata, and refuses
    /// wrong-class, missing-component, or malformed payloads.
    pub fn prepare_vst3_preset_command(
        &self,
        device: DeviceId,
        preset_bytes: &[u8],
    ) -> Result<Command, SessionError> {
        let current = self
            .project()
            .device(device)
            .ok_or(SessionError::Invalid("device no longer exists"))?
            .state();
        let next_state = apply_vstpreset_to_state(current, preset_bytes)
            .map_err(|error| SessionError::NativeAudio(error.to_string()))?;
        Ok(Command::ReplaceDeviceState {
            device,
            state: next_state,
        })
    }

    /// Apply a Steinberg `.vstpreset` payload directly to canonical `Editor`
    /// history as one undoable step (for control-side/offline preset loading
    /// without invoking resident native-only graph compilation).
    pub fn apply_vst3_preset_canonical(
        &mut self,
        device: DeviceId,
        preset_bytes: &[u8],
    ) -> Result<(), SessionError> {
        let command = self.prepare_vst3_preset_command(device, preset_bytes)?;
        self.editor.execute(command)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_plugin_state::{
        BinaryIdentity, ClassId, Identity, State, decode, encode,
        vstpreset::{VstPreset, encode_vstpreset},
    };
    use hex_project::Created;

    fn identity(class_byte: u8) -> Identity {
        Identity {
            class: ClassId::new([class_byte; 16]).unwrap(),
            binary: BinaryIdentity {
                executable_sha256: [0x11; 32],
                info_sha256: [0x22; 32],
            },
        }
    }

    fn vst3_state(
        class_byte: u8,
        component: &[u8],
        controller: Option<&[u8]>,
    ) -> hex_lattice_contract::NativeDeviceState {
        encode(State {
            identity: identity(class_byte),
            component: Some(component),
            controller,
        })
        .unwrap()
    }

    fn session_with_vst3(
        class_byte: u8,
        component: &[u8],
        controller: Option<&[u8]>,
    ) -> (Session, DeviceId) {
        let mut session = Session::new(48_000, 16, 48_000 * 60).unwrap();
        let Created::Track(track) = session
            .editor
            .execute(Command::AddTrack {
                name: "synth".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let initial_state = vst3_state(class_byte, component, controller);
        let Created::Device(device) = session
            .editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: initial_state,
            })
            .unwrap()
        else {
            panic!("device");
        };
        (session, device)
    }

    #[test]
    fn vstpreset_apply_and_export_round_trip_in_one_undo_step() {
        let (mut session, device) = session_with_vst3(0x42, b"init-comp", Some(b"init-cont"));

        let exported_initial = session
            .export_vst3_preset(device, Some("<Info name=\"initial\"/>"))
            .unwrap();

        let new_preset = encode_vstpreset(VstPreset {
            class: identity(0x42).class,
            component: Some(b"lead-comp"),
            controller: Some(b"lead-cont"),
            info_xml: Some("<Info name=\"lead\"/>"),
        })
        .unwrap();

        let rev_before = session.history_revision();
        session
            .apply_vst3_preset_canonical(device, &new_preset)
            .unwrap();
        assert_eq!(session.history_revision(), rev_before + 1);

        let after_apply = decode(session.project().device(device).unwrap().state()).unwrap();
        assert_eq!(after_apply.component, Some(&b"lead-comp"[..]));
        assert_eq!(after_apply.controller, Some(&b"lead-cont"[..]));

        // Single Undo restores both processor and controller chunks.
        assert!(session.editor.undo());
        let after_undo = decode(session.project().device(device).unwrap().state()).unwrap();
        assert_eq!(after_undo.component, Some(&b"init-comp"[..]));
        assert_eq!(after_undo.controller, Some(&b"init-cont"[..]));

        // Redo restores the applied preset.
        assert!(session.editor.redo());
        let after_redo = decode(session.project().device(device).unwrap().state()).unwrap();
        assert_eq!(after_redo.component, Some(&b"lead-comp"[..]));
        assert_eq!(after_redo.controller, Some(&b"lead-cont"[..]));

        // Re-applying exported initial preset restores initial chunks.
        session
            .apply_vst3_preset_canonical(device, &exported_initial)
            .unwrap();
        let round_tripped = decode(session.project().device(device).unwrap().state()).unwrap();
        assert_eq!(round_tripped.component, Some(&b"init-comp"[..]));
        assert_eq!(round_tripped.controller, Some(&b"init-cont"[..]));
    }

    #[test]
    fn malformed_and_wrong_class_vstpresets_preserve_current_state_and_history() {
        let (mut session, device) = session_with_vst3(0x33, b"good-comp", Some(b"good-cont"));
        let snapshot_before = session.project().clone();
        let rev_before = session.history_revision();

        // Wrong plugin class.
        let wrong_class = encode_vstpreset(VstPreset {
            class: identity(0x99).class,
            component: Some(b"other"),
            controller: None,
            info_xml: None,
        })
        .unwrap();
        assert!(
            session
                .apply_vst3_preset_canonical(device, &wrong_class)
                .is_err()
        );
        assert_eq!(session.project(), &snapshot_before);
        assert_eq!(session.history_revision(), rev_before);

        // Truncated/malformed bytes.
        assert!(
            session
                .apply_vst3_preset_canonical(device, b"VST3bad")
                .is_err()
        );
        assert_eq!(session.project(), &snapshot_before);
        assert_eq!(session.history_revision(), rev_before);

        // Missing component chunk.
        let missing_comp = encode_vstpreset(VstPreset {
            class: identity(0x33).class,
            component: None,
            controller: Some(b"only-cont"),
            info_xml: None,
        })
        .unwrap();
        assert!(
            session
                .apply_vst3_preset_canonical(device, &missing_comp)
                .is_err()
        );
        assert_eq!(session.project(), &snapshot_before);
        assert_eq!(session.history_revision(), rev_before);
    }
}
