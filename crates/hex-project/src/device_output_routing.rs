//! Canonical multi-output VST3 instrument bus routing to distinct Hex destinations.
//!
//! Each route is keyed by `(device, bus_index)` (`bus_index < MAX_DEVICE_OUTPUT_BUSES`)
//! and binds one audio output bus of a hosted track instrument/device to a validated
//! `Track`, `Return`, or `Main` destination without shifting sibling indices or
//! introducing audio graph cycles.

use crate::{Created, DeviceId, Invalid, Project, ReturnId, RoutingOwner, TrackId};

/// Matches `max_instrument_outputs` in the native macOS VST3 helper and `hex-engine`.
pub const MAX_DEVICE_OUTPUT_BUSES: u16 = 16;
/// Bounded canonical multi-output route count across one project.
pub const MAX_DEVICE_OUTPUT_ROUTES: usize = 256;

/// Distinct Hex destination receiving one hosted device output bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeviceOutputDestination {
    Track(TrackId),
    Return(ReturnId),
    Main,
}

impl DeviceOutputDestination {
    pub fn into_routing_owner(self) -> RoutingOwner {
        match self {
            Self::Track(id) => RoutingOwner::Track(id),
            Self::Return(id) => RoutingOwner::Return(id),
            Self::Main => RoutingOwner::Main,
        }
    }
}

/// Canonical authored route from one device's output `bus_index` to a Hex destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceOutputRoute {
    pub device: DeviceId,
    pub bus_index: u16,
    pub destination: DeviceOutputDestination,
}

/// Snapshot representation for versioned project persistence (Schema 133).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceOutputRouteSnapshot {
    pub device: u64,
    pub bus_index: u16,
    pub destination: RoutingOwner,
}

impl Project {
    /// Canonical multi-output device bus routes, sorted by `(device, bus_index)`.
    pub fn device_output_routes(&self) -> &[DeviceOutputRoute] {
        &self.device_output_routes
    }

    /// Resolve the owning track of a multi-output route source device.
    pub fn device_output_source_owner(&self, device: DeviceId) -> Result<RoutingOwner, Invalid> {
        let Some(track) = self.tracks().iter().find(|track| {
            track.instrument().is_some_and(|inst| inst.id() == device)
                || track.devices().iter().any(|dev| dev.id() == device)
        }) else {
            return Err(Invalid("device output route requires a track device"));
        };
        Ok(RoutingOwner::Track(track.id()))
    }

    pub(crate) fn set_device_output_route(
        &mut self,
        device: DeviceId,
        bus_index: u16,
        destination: Option<DeviceOutputDestination>,
    ) -> Result<Created, Invalid> {
        if bus_index >= MAX_DEVICE_OUTPUT_BUSES {
            return Err(Invalid("device output bus index exceeds capacity"));
        }
        let source_owner = self.device_output_source_owner(device)?;
        if let Some(target) = destination {
            match target {
                DeviceOutputDestination::Track(id) => {
                    self.track(id)
                        .ok_or(Invalid("output destination track no longer exists"))?;
                    if RoutingOwner::Track(id) == source_owner {
                        return Err(Invalid("routing cycles are unsupported"));
                    }
                }
                DeviceOutputDestination::Return(id) => {
                    if !self.returns.iter().any(|bus| bus.id() == id) {
                        return Err(Invalid("output destination return no longer exists"));
                    }
                }
                DeviceOutputDestination::Main => {}
            }
        }
        let key = (device, bus_index);
        let pos = self
            .device_output_routes
            .binary_search_by_key(&key, |route| (route.device, route.bus_index));
        match (pos, destination) {
            (Ok(index), Some(destination)) => {
                self.device_output_routes[index].destination = destination;
            }
            (Ok(index), None) => {
                self.device_output_routes.remove(index);
            }
            (Err(index), Some(destination)) => {
                if self.device_output_routes.len() >= MAX_DEVICE_OUTPUT_ROUTES {
                    return Err(Invalid("device output route limit reached"));
                }
                self.device_output_routes.insert(
                    index,
                    DeviceOutputRoute {
                        device,
                        bus_index,
                        destination,
                    },
                );
            }
            (Err(_), None) => {}
        }
        self.validate_playback_routing()?;
        Ok(Created::None)
    }

    pub(crate) fn adopt_device_output_routes(
        &mut self,
        routes: Vec<DeviceOutputRouteSnapshot>,
    ) -> Result<(), Invalid> {
        if routes.len() > MAX_DEVICE_OUTPUT_ROUTES {
            return Err(Invalid("device output route limit reached"));
        }
        if routes
            .windows(2)
            .any(|pair| (pair[0].device, pair[0].bus_index) >= (pair[1].device, pair[1].bus_index))
        {
            return Err(Invalid(
                "device output routes must have unique ascending (device, bus_index) keys",
            ));
        }
        let mut adopted = Vec::with_capacity(routes.len());
        for route in routes {
            if route.bus_index >= MAX_DEVICE_OUTPUT_BUSES {
                return Err(Invalid("device output bus index exceeds capacity"));
            }
            let destination = match route.destination {
                RoutingOwner::Track(id) => DeviceOutputDestination::Track(id),
                RoutingOwner::Return(id) => DeviceOutputDestination::Return(id),
                RoutingOwner::Main => DeviceOutputDestination::Main,
            };
            adopted.push(DeviceOutputRoute {
                device: DeviceId(route.device),
                bus_index: route.bus_index,
                destination,
            });
        }
        self.device_output_routes = adopted;
        self.validate_playback_routing()
    }

    pub(crate) fn prune_deleted_device_output_routes(&mut self) {
        if self.device_output_routes.is_empty() {
            return;
        }
        let mut routes = std::mem::take(&mut self.device_output_routes);
        routes.retain(|route| {
            let source_ok = self.device_output_source_owner(route.device).is_ok();
            let dest_ok = match route.destination {
                DeviceOutputDestination::Track(id) => self.track(id).is_some(),
                DeviceOutputDestination::Return(id) => self.returns.iter().any(|b| b.id() == id),
                DeviceOutputDestination::Main => true,
            };
            source_ok && dest_ok
        });
        self.device_output_routes = routes;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Created, Editor, TrackDeviceSlot};

    #[test]
    fn device_output_routes_support_undo_redo_cycle_refusal_and_pruning() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(inst_track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "drums".into(),
                before_track: None,
                slot: TrackDeviceSlot::Instrument,
                state: crate::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("instrument track")
        };
        let Created::Track(kick_track) = editor
            .execute(Command::AddTrack {
                name: "kick".into(),
            })
            .unwrap()
        else {
            panic!("kick track")
        };
        let Created::ReturnTrack(room) = editor
            .execute(Command::AddReturnTrack {
                name: "room".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        let device = editor
            .project()
            .track(inst_track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();

        // Out-of-range bus index and self-route refuse cleanly.
        assert!(
            editor
                .execute(Command::SetDeviceOutputRoute {
                    device,
                    bus_index: MAX_DEVICE_OUTPUT_BUSES,
                    destination: Some(DeviceOutputDestination::Main),
                })
                .is_err()
        );
        assert!(
            editor
                .execute(Command::SetDeviceOutputRoute {
                    device,
                    bus_index: 1,
                    destination: Some(DeviceOutputDestination::Track(inst_track)),
                })
                .is_err()
        );

        // Route bus 1 -> kick_track and bus 2 -> room return.
        editor
            .execute(Command::SetDeviceOutputRoute {
                device,
                bus_index: 1,
                destination: Some(DeviceOutputDestination::Track(kick_track)),
            })
            .unwrap();
        editor
            .execute(Command::SetDeviceOutputRoute {
                device,
                bus_index: 2,
                destination: Some(DeviceOutputDestination::Return(room)),
            })
            .unwrap();
        assert_eq!(editor.project().device_output_routes().len(), 2);

        // Closing a cycle from kick_track back to inst_track must refuse.
        assert!(
            editor
                .execute(Command::SetAudioFeed {
                    source: RoutingOwner::Track(kick_track),
                    destination: inst_track,
                    gain: 0.5,
                })
                .is_err()
        );

        // Removing kick_track prunes bus 1 while keeping bus 2 intact; Undo restores both.
        editor.execute(Command::RemoveTrack(kick_track)).unwrap();
        assert_eq!(
            editor.project().device_output_routes(),
            &[DeviceOutputRoute {
                device,
                bus_index: 2,
                destination: DeviceOutputDestination::Return(room),
            }]
        );
        assert!(editor.undo());
        assert_eq!(editor.project().device_output_routes().len(), 2);
        assert!(editor.redo());
        assert_eq!(editor.project().device_output_routes().len(), 1);

        // Clearing bus 2 leaves empty routes.
        editor
            .execute(Command::SetDeviceOutputRoute {
                device,
                bus_index: 2,
                destination: None,
            })
            .unwrap();
        assert!(editor.project().device_output_routes().is_empty());
    }
}
