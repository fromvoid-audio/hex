//! One post-mixer stereo track tap per native Compressor instance.
use crate::{Created, DeviceId, Invalid, Project, RoutingOwner, TrackId};
use hex_lattice_contract::DeviceTypeId;

/// Bounded authored keys. Each destination uses its device ID as its identity.
pub const MAX_SIDECHAIN_ROUTES: usize = crate::MAX_TRACKS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SidechainRoute {
    pub device: DeviceId,
    pub source: TrackId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SidechainRouteSnapshot {
    pub device: u64,
    pub source: u64,
}

impl Project {
    pub fn sidechains(&self) -> &[SidechainRoute] {
        &self.sidechains
    }

    /// Resolve the actual effect chain. Retained source devices can render in a
    /// different owner context and require a separate routing contract.
    pub fn sidechain_destination(&self, device: DeviceId) -> Result<RoutingOwner, Invalid> {
        let effect = self
            .device(device)
            .ok_or(Invalid("device no longer exists"))?;
        let device_type = effect.state().type_id();
        if device_type != DeviceTypeId::from_static("hex.compressor")
            && device_type != DeviceTypeId::from_static("hex.gate")
            && device_type != hex_lattice_contract::VST3_DEVICE_TYPE_ID
        {
            return Err(Invalid("device does not support a sidechain"));
        }
        if self
            .native_sources()
            .iter()
            .any(|source| source.devices().any(|effect| effect.id() == device))
        {
            return Err(Invalid(
                "retained source devices do not support a sidechain",
            ));
        }
        if let Some(track) = self
            .tracks()
            .iter()
            .find(|track| track.devices().iter().any(|effect| effect.id() == device))
        {
            Ok(RoutingOwner::Track(track.id()))
        } else if let Some(bus) = self
            .return_tracks()
            .iter()
            .find(|bus| bus.devices().iter().any(|effect| effect.id() == device))
        {
            Ok(RoutingOwner::Return(bus.id()))
        } else if self
            .main_devices()
            .iter()
            .any(|effect| effect.id() == device)
        {
            Ok(RoutingOwner::Main)
        } else {
            Err(Invalid(
                "sidechain requires a track, return, or main effect",
            ))
        }
    }

    /// Tracks that can supply a new key without closing a path back from the
    /// destination. Use the same authored graph as validation and replace the
    /// current key instead of considering it an additional route.
    pub fn eligible_sidechain_sources(&self, device: DeviceId) -> Result<Vec<TrackId>, Invalid> {
        use std::collections::{BTreeMap, BTreeSet};

        let destination = self.sidechain_destination(device)?;
        let mut outgoing = BTreeMap::<_, Vec<_>>::new();
        for (from, to) in self.playable_audio_edges(Some(device))? {
            outgoing.entry(from).or_default().push(to);
        }
        let mut reachable = BTreeSet::new();
        let mut pending = vec![destination];
        while let Some(owner) = pending.pop() {
            if reachable.insert(owner)
                && let Some(children) = outgoing.get(&owner)
            {
                pending.extend(children.iter().copied());
            }
        }
        Ok(self
            .tracks()
            .iter()
            .map(crate::Track::id)
            .filter(|track| !reachable.contains(&RoutingOwner::Track(*track)))
            .collect())
    }

    pub(crate) fn set_sidechain(
        &mut self,
        device: DeviceId,
        source: Option<TrackId>,
    ) -> Result<Created, Invalid> {
        self.sidechain_destination(device)?;
        let index = self
            .sidechains
            .binary_search_by_key(&device, |route| route.device);
        match (index, source) {
            (Ok(index), Some(source)) => self.sidechains[index].source = source,
            (Ok(index), None) => {
                self.sidechains.remove(index);
            }
            (Err(index), Some(source)) => {
                if self.sidechains.len() >= MAX_SIDECHAIN_ROUTES {
                    return Err(Invalid("sidechain route limit reached"));
                }
                self.sidechains
                    .insert(index, SidechainRoute { device, source });
            }
            (Err(_), None) => {}
        }
        self.validate_playable_routes()?;
        Ok(Created::None)
    }

    pub(crate) fn adopt_sidechains(
        &mut self,
        routes: Vec<SidechainRouteSnapshot>,
    ) -> Result<(), Invalid> {
        if routes.len() > MAX_SIDECHAIN_ROUTES {
            return Err(Invalid("sidechain route limit reached"));
        }
        if routes
            .windows(2)
            .any(|pair| pair[0].device >= pair[1].device)
        {
            return Err(Invalid("sidechains must have unique ascending device ids"));
        }
        self.sidechains = routes
            .into_iter()
            .map(|route| SidechainRoute {
                device: DeviceId(route.device),
                source: TrackId(route.source),
            })
            .collect();
        self.validate_playable_routes()
    }

    /// Deleted sources disconnect. Deleted or replaced destinations that no
    /// longer support a sidechain own no remaining route.
    /// Apply only to an edited candidate. Imported dangling routes must refuse.
    pub(crate) fn prune_deleted_sidechains(&mut self) {
        if self.sidechains.is_empty() {
            return;
        }
        let mut routes = std::mem::take(&mut self.sidechains);
        routes.retain(|route| {
            self.track(route.source).is_some() && self.sidechain_destination(route.device).is_ok()
        });
        self.sidechains = routes;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Created, DeviceId, Editor};

    #[test]
    fn sidechain_queries_reject_unknown_device_and_start_empty() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        assert!(editor.project().sidechains().is_empty());
        assert!(
            editor
                .project()
                .sidechain_destination(DeviceId(999))
                .is_err()
        );
        assert!(
            editor
                .project()
                .eligible_sidechain_sources(DeviceId(999))
                .is_err()
        );
        let Created::Track(track) = editor
            .execute(Command::AddTrack { name: "a".into() })
            .unwrap()
        else {
            panic!("track")
        };
        assert!(
            editor
                .execute(Command::SetSidechain {
                    device: DeviceId(999),
                    source: Some(track),
                })
                .is_err()
        );
    }

    #[test]
    fn replacing_sidechained_compressor_with_other_effect_prunes_sidechain_and_undoes() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(source) = editor
            .execute(Command::AddTrack {
                name: "kick".into(),
            })
            .unwrap()
        else {
            panic!("source track")
        };
        let Created::Track(dest) = editor
            .execute(Command::AddTrack {
                name: "bass".into(),
            })
            .unwrap()
        else {
            panic!("dest track")
        };
        let comp_state = hex_lattice_contract::NativeDeviceState::new(
            DeviceTypeId::from_static("hex.compressor"),
            1,
            vec![1],
        )
        .unwrap();
        let Created::Device(device) = editor
            .execute(Command::InsertDevice {
                track: dest,
                before: None,
                state: comp_state.clone(),
            })
            .unwrap()
        else {
            panic!("compressor")
        };
        editor
            .execute(Command::SetSidechain {
                device,
                source: Some(source),
            })
            .unwrap();
        assert_eq!(
            editor.project().sidechains(),
            &[SidechainRoute { device, source }]
        );

        let utility_state = hex_lattice_contract::NativeDeviceState::new(
            DeviceTypeId::from_static("hex.utility"),
            1,
            vec![2],
        )
        .unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: utility_state.clone(),
            })
            .unwrap();
        assert!(
            editor.project().sidechains().is_empty(),
            "obsolete sidechain route must be pruned when compressor is replaced by another effect"
        );
        assert_eq!(
            editor.project().device(device).unwrap().state(),
            &utility_state
        );

        assert!(editor.undo());
        assert_eq!(
            editor.project().device(device).unwrap().state(),
            &comp_state
        );
        assert_eq!(
            editor.project().sidechains(),
            &[SidechainRoute { device, source }],
            "undo must restore both the compressor and its sidechain route"
        );

        assert!(editor.redo());
        assert!(editor.project().sidechains().is_empty());
        assert_eq!(
            editor.project().device(device).unwrap().state(),
            &utility_state
        );
    }

    #[test]
    fn vst3_effect_supports_sidechain_routing_with_cycle_detection_and_undo() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(source) = editor
            .execute(Command::AddTrack {
                name: "kick".into(),
            })
            .unwrap()
        else {
            panic!("source track")
        };
        let Created::Track(dest) = editor
            .execute(Command::AddTrack {
                name: "synth".into(),
            })
            .unwrap()
        else {
            panic!("dest track")
        };
        let vst3_state = hex_lattice_contract::NativeDeviceState::new(
            hex_lattice_contract::VST3_DEVICE_TYPE_ID,
            1,
            vec![1, 2, 3, 4],
        )
        .unwrap();
        let Created::Device(device) = editor
            .execute(Command::InsertDevice {
                track: dest,
                before: None,
                state: vst3_state,
            })
            .unwrap()
        else {
            panic!("vst3 device")
        };
        editor
            .execute(Command::SetSidechain {
                device,
                source: Some(source),
            })
            .unwrap();
        assert_eq!(
            editor.project().sidechains(),
            &[SidechainRoute { device, source }]
        );
        assert!(
            editor
                .project()
                .eligible_sidechain_sources(device)
                .unwrap()
                .contains(&source)
        );
        // Self-sidechain cycle must still refuse.
        assert!(
            editor
                .execute(Command::SetSidechain {
                    device,
                    source: Some(dest),
                })
                .is_err()
        );
        assert!(editor.undo());
        assert!(editor.project().sidechains().is_empty());
        assert!(editor.redo());
        assert_eq!(
            editor.project().sidechains(),
            &[SidechainRoute { device, source }]
        );
    }
}
