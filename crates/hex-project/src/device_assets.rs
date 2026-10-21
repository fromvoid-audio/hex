//! Control-side catalog resolution for neutral device asset references.
use crate::{AssetId, Device, Invalid, Project};

impl Project {
    fn asset_devices(&self) -> impl Iterator<Item = &Device> {
        self.tracks()
            .iter()
            .flat_map(|track| track.devices().iter().chain(track.instrument()))
            .chain(
                self.return_tracks()
                    .iter()
                    .flat_map(|track| track.devices()),
            )
            .chain(self.main_devices())
    }

    /// Resolve identities only; this performs no file access or PCM admission.
    pub fn validate_device_asset_references(&self) -> Result<(), Invalid> {
        for device in self.asset_devices() {
            for reference in device
                .authored_states()
                .flat_map(|state| state.asset_references())
            {
                if self.asset(AssetId(reference.asset.value())).is_none() {
                    return Err(Invalid("device references missing project media"));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn device_uses_asset(&self, asset: AssetId) -> bool {
        self.asset_devices().any(|device| {
            device
                .authored_states()
                .flat_map(|state| state.asset_references())
                .any(|reference| reference.asset.value() == asset.value())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Created, Editor};
    use hex_lattice_contract::{DeviceAssetSlot, DeviceTypeId, NativeDeviceState};
    #[test]
    fn prepared_source_and_empty_instrument_binding_share_one_commit_and_undo() {
        let mut editor = Editor::new(Project::new(16000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "sample".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let before = editor.project().snapshot();
        let command = || Command::ImportDeviceAssetIntoEmptyInstrument {
            track,
            path: "source.wav".into(),
            sample_rate: 8000,
            frames: 4,
            state: NativeDeviceState::new(
                DeviceTypeId::new("test.instrument.resource").unwrap(),
                1,
                vec![],
            )
            .unwrap(),
            source_slot: DeviceAssetSlot::new(1).unwrap(),
        };
        let rejected: Result<_, Invalid> = editor.execute_prepared(command(), |candidate| {
            assert_eq!(candidate.assets().count(), 1);
            let target = candidate.track(track).unwrap().instrument().unwrap();
            assert_eq!(target.state().asset_references().len(), 1);
            Err::<(), _>(Invalid("prepared source refused"))
        });
        assert!(rejected.is_err());
        assert_eq!(editor.project().snapshot(), before);
        let (Created::Asset(asset), ()) = editor
            .execute_prepared(command(), |_| Ok::<_, Invalid>(()))
            .unwrap()
        else {
            panic!("asset")
        };
        assert_eq!(
            editor
                .project()
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .state()
                .asset_references()[0]
                .asset
                .value(),
            asset.value()
        );
        let loaded = editor.project().snapshot();
        assert!(editor.execute(command()).is_err());
        assert_eq!(editor.project().snapshot(), loaded);
        assert!(editor.undo());
        assert_eq!(editor.project().assets().count(), 0);
        assert!(
            editor
                .project()
                .track(track)
                .unwrap()
                .instrument()
                .is_none()
        );
        assert!(editor.undo());
        assert!(
            editor.project().track(track).is_none(),
            "one source+binding command makes one undo entry"
        );
    }
}
