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
        if effect.state().type_id() != DeviceTypeId::from_static("hex.compressor") {
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

    /// Deleted sources disconnect. Deleted destinations own no remaining route.
    /// Apply only to an edited candidate. Imported dangling routes must refuse.
    pub(crate) fn prune_deleted_sidechains(&mut self) {
        if self.sidechains.is_empty() {
            return;
        }
        let mut routes = std::mem::take(&mut self.sidechains);
        routes.retain(|route| {
            self.track(route.source).is_some() && self.device(route.device).is_some()
        });
        self.sidechains = routes;
    }
}
