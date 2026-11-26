//! Bounded typed VST3 audio/event bus descriptors and route mappings.
//!
//! Preserves exact SDK bus indices (including inactive and disconnected buses)
//! so enabling, disabling, renaming, or removing one bus never shifts the
//! identity of sibling main/auxiliary input or multi-output routes.

use std::collections::BTreeSet;
use std::fmt;

/// Bounded per-direction audio bus inventory (matches native helper capacity).
pub const MAX_AUDIO_BUSES: usize = 32;
/// Bounded per-direction event bus inventory.
pub const MAX_EVENT_BUSES: usize = 8;
/// Maximum UTF-8 bytes for a bus display name.
pub const MAX_BUS_NAME_BYTES: usize = 64;
/// Maximum channel width admitted per typed bus descriptor.
pub const MAX_BUS_CHANNELS: u8 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BusMedia {
    Audio,
    Event,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BusDirection {
    Input,
    Output,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BusKind {
    Main,
    Auxiliary,
}

/// Stable identifier for one VST3 bus: `(media, direction, index)`.
/// Inactive or disconnected buses keep their exact `index` and never renumber.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BusAddress {
    pub media: BusMedia,
    pub direction: BusDirection,
    pub index: u16,
}

/// One validated VST3 bus descriptor reported by `IComponent::getBusInfo`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusDescriptor {
    pub address: BusAddress,
    pub kind: BusKind,
    pub channel_count: u8,
    pub default_active: bool,
    pub active: bool,
    pub name: String,
}

/// Typed external route attached to a plugin bus (either an auxiliary/sidechain
/// input tap or a multi-output destination route).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BusEndpoint {
    /// Post-mixer stereo tap from a Hex track feeding an input bus.
    TrackTap(u64),
    /// Dedicated Hex destination track receiving a multi-output audio/event bus.
    DestinationTrack(u64),
    /// Return bus destination receiving an output bus.
    DestinationReturn(u64),
    /// Main bus destination.
    Main,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BusRoute {
    pub bus: BusAddress,
    pub endpoint: BusEndpoint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusLayoutError {
    TooManyBuses,
    NameTooLong,
    InvalidBusIndex,
    InvalidChannelCount,
    DuplicateBusAddress,
    UnknownBus,
    InactiveBus,
    IncompatibleEndpoint,
    DuplicateRoute,
    LayoutMismatch,
    RateMismatch,
}

impl fmt::Display for BusLayoutError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(match self {
            Self::TooManyBuses => "vst3 bus count exceeds the bounded limit",
            Self::NameTooLong => "vst3 bus name exceeds the bounded limit",
            Self::InvalidBusIndex => "vst3 bus indices must be contiguous from zero per direction",
            Self::InvalidChannelCount => "vst3 bus channel count is unsupported",
            Self::DuplicateBusAddress => "vst3 bus address is duplicated",
            Self::UnknownBus => "vst3 bus does not exist in the plugin layout",
            Self::InactiveBus => "vst3 bus is inactive and cannot carry a live route",
            Self::IncompatibleEndpoint => "route endpoint does not match vst3 bus direction",
            Self::DuplicateRoute => "vst3 bus already has an assigned route",
            Self::LayoutMismatch => "vst3 channel layout does not match the negotiated route",
            Self::RateMismatch => "vst3 sample rate does not match the active audio route",
        })
    }
}

impl std::error::Error for BusLayoutError {}

/// Validated, immutable snapshot of a VST3 plugin's audio and event bus layout
/// plus its active typed routes. All mutations return a fresh candidate so
/// refused changes preserve the prior active graph untouched.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginBusLayout {
    sample_rate: u32,
    buses: Vec<BusDescriptor>,
    routes: Vec<BusRoute>,
}

impl PluginBusLayout {
    pub fn new(sample_rate: u32, buses: Vec<BusDescriptor>) -> Result<Self, BusLayoutError> {
        if sample_rate == 0 {
            return Err(BusLayoutError::RateMismatch);
        }
        let mut seen = BTreeSet::new();
        let mut counts = [0usize; 4];
        for bus in &buses {
            if bus.name.len() > MAX_BUS_NAME_BYTES {
                return Err(BusLayoutError::NameTooLong);
            }
            if !seen.insert(bus.address) {
                return Err(BusLayoutError::DuplicateBusAddress);
            }
            let bucket = match (bus.address.media, bus.address.direction) {
                (BusMedia::Audio, BusDirection::Input) => 0,
                (BusMedia::Audio, BusDirection::Output) => 1,
                (BusMedia::Event, BusDirection::Input) => 2,
                (BusMedia::Event, BusDirection::Output) => 3,
            };
            if bus.address.media == BusMedia::Audio
                && (bus.channel_count == 0 || bus.channel_count > MAX_BUS_CHANNELS)
            {
                return Err(BusLayoutError::InvalidChannelCount);
            }
            if bus.address.media == BusMedia::Event && bus.channel_count > 16 {
                return Err(BusLayoutError::InvalidChannelCount);
            }
            counts[bucket] += 1;
            let limit = if bus.address.media == BusMedia::Audio {
                MAX_AUDIO_BUSES
            } else {
                MAX_EVENT_BUSES
            };
            if counts[bucket] > limit {
                return Err(BusLayoutError::TooManyBuses);
            }
        }
        // Verify contiguous zero-based indices per (media, direction) so inactive
        // descriptors stay anchored at their true SDK bus indices.
        for (media, direction, count) in [
            (BusMedia::Audio, BusDirection::Input, counts[0]),
            (BusMedia::Audio, BusDirection::Output, counts[1]),
            (BusMedia::Event, BusDirection::Input, counts[2]),
            (BusMedia::Event, BusDirection::Output, counts[3]),
        ] {
            for index in 0..count {
                let address = BusAddress {
                    media,
                    direction,
                    index: index as u16,
                };
                if !seen.contains(&address) {
                    return Err(BusLayoutError::InvalidBusIndex);
                }
            }
        }
        Ok(Self {
            sample_rate,
            buses,
            routes: Vec::new(),
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn buses(&self) -> &[BusDescriptor] {
        &self.buses
    }

    pub fn routes(&self) -> &[BusRoute] {
        &self.routes
    }

    pub fn bus(&self, address: BusAddress) -> Option<&BusDescriptor> {
        self.buses.iter().find(|bus| bus.address == address)
    }

    pub fn route_for(&self, address: BusAddress) -> Option<BusEndpoint> {
        self.routes
            .iter()
            .find(|route| route.bus == address)
            .map(|route| route.endpoint)
    }

    /// Auxiliary audio input buses suitable for external sidechain taps,
    /// preserving each bus's exact SDK index even when earlier buses are inactive.
    pub fn auxiliary_audio_inputs(&self) -> impl Iterator<Item = &BusDescriptor> {
        self.buses.iter().filter(|bus| {
            bus.address.media == BusMedia::Audio
                && bus.address.direction == BusDirection::Input
                && bus.kind == BusKind::Auxiliary
        })
    }

    /// Additional audio output buses beyond main output 0 (multi-output instruments).
    pub fn additional_audio_outputs(&self) -> impl Iterator<Item = &BusDescriptor> {
        self.buses.iter().filter(|bus| {
            bus.address.media == BusMedia::Audio
                && bus.address.direction == BusDirection::Output
                && bus.address.index > 0
        })
    }

    /// Transactionally set a route on `address`, returning a new candidate layout.
    /// Validates direction, active state, sample rate, and channel count without
    /// mutating `self` on refusal.
    pub fn with_route(
        &self,
        address: BusAddress,
        endpoint: Option<BusEndpoint>,
        expected_rate: u32,
        expected_channels: Option<u8>,
    ) -> Result<Self, BusLayoutError> {
        if expected_rate != self.sample_rate {
            return Err(BusLayoutError::RateMismatch);
        }
        let descriptor = self.bus(address).ok_or(BusLayoutError::UnknownBus)?;
        let mut next = self.clone();
        next.routes.retain(|route| route.bus != address);
        let Some(endpoint) = endpoint else {
            return Ok(next);
        };
        if !descriptor.active {
            return Err(BusLayoutError::InactiveBus);
        }
        if let Some(channels) = expected_channels
            && descriptor.address.media == BusMedia::Audio
            && descriptor.channel_count != channels
        {
            return Err(BusLayoutError::LayoutMismatch);
        }
        let direction_ok = matches!(
            (address.direction, endpoint),
            (BusDirection::Input, BusEndpoint::TrackTap(_))
                | (
                    BusDirection::Output,
                    BusEndpoint::DestinationTrack(_)
                        | BusEndpoint::DestinationReturn(_)
                        | BusEndpoint::Main,
                )
        );
        if !direction_ok {
            return Err(BusLayoutError::IncompatibleEndpoint);
        }
        next.routes.push(BusRoute {
            bus: address,
            endpoint,
        });
        next.routes.sort_by_key(|route| route.bus);
        Ok(next)
    }

    /// Transactionally toggle bus activation without shifting any bus index.
    /// Deactivating a bus disconnects only that bus's route and retains all
    /// sibling routes intact.
    pub fn with_bus_active(
        &self,
        address: BusAddress,
        active: bool,
    ) -> Result<Self, BusLayoutError> {
        let mut next = self.clone();
        let bus = next
            .buses
            .iter_mut()
            .find(|bus| bus.address == address)
            .ok_or(BusLayoutError::UnknownBus)?;
        bus.active = active;
        if !active {
            next.routes.retain(|route| route.bus != address);
        }
        Ok(next)
    }

    /// Prune routes whose target track was deleted without disturbing unrelated
    /// active buses or shifting bus indices.
    pub fn without_deleted_track(&self, deleted_track_id: u64) -> Self {
        let mut next = self.clone();
        next.routes.retain(|route| {
            !matches!(
                route.endpoint,
                BusEndpoint::TrackTap(id) | BusEndpoint::DestinationTrack(id) if id == deleted_track_id
            )
        });
        next
    }

    /// Reconcile routes against a refreshed plugin bus layout (for example after
    /// I/O restart or bus rename): preserves every route whose `(media, direction,
    /// index)` still exists, remains active, and matches channel count.
    pub fn reconcile_after_restart(&self, refreshed: &Self) -> Result<Self, BusLayoutError> {
        let mut next = refreshed.clone();
        for route in &self.routes {
            if let Some(bus) = refreshed.bus(route.bus)
                && bus.active
                && self
                    .bus(route.bus)
                    .is_some_and(|old| old.channel_count == bus.channel_count)
            {
                next = next.with_route(
                    route.bus,
                    Some(route.endpoint),
                    refreshed.sample_rate,
                    Some(bus.channel_count),
                )?;
            }
        }
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio_bus(
        direction: BusDirection,
        index: u16,
        kind: BusKind,
        channels: u8,
        active: bool,
        name: &str,
    ) -> BusDescriptor {
        BusDescriptor {
            address: BusAddress {
                media: BusMedia::Audio,
                direction,
                index,
            },
            kind,
            channel_count: channels,
            default_active: active,
            active,
            name: name.into(),
        }
    }

    #[test]
    fn inactive_auxiliary_buses_keep_exact_indices_and_refusals_preserve_active_layout() {
        let layout = PluginBusLayout::new(
            48_000,
            vec![
                audio_bus(BusDirection::Input, 0, BusKind::Main, 2, true, "main in"),
                audio_bus(
                    BusDirection::Input,
                    1,
                    BusKind::Auxiliary,
                    2,
                    false,
                    "aux 1",
                ),
                audio_bus(
                    BusDirection::Input,
                    2,
                    BusKind::Auxiliary,
                    2,
                    true,
                    "sidechain",
                ),
                audio_bus(BusDirection::Output, 0, BusKind::Main, 2, true, "main out"),
            ],
        )
        .unwrap();

        let aux1 = BusAddress {
            media: BusMedia::Audio,
            direction: BusDirection::Input,
            index: 1,
        };
        let aux2 = BusAddress {
            media: BusMedia::Audio,
            direction: BusDirection::Input,
            index: 2,
        };

        // Inactive bus refuses live route without mutating layout.
        assert_eq!(
            layout.with_route(aux1, Some(BusEndpoint::TrackTap(7)), 48_000, Some(2)),
            Err(BusLayoutError::InactiveBus)
        );
        // Active bus at index 2 keeps exact index 2 despite inactive bus 1.
        let routed = layout
            .with_route(aux2, Some(BusEndpoint::TrackTap(7)), 48_000, Some(2))
            .unwrap();
        assert_eq!(routed.route_for(aux2), Some(BusEndpoint::TrackTap(7)));
        assert_eq!(routed.route_for(aux1), None);

        // Rate/layout/direction mismatches refuse without mutating `routed`.
        assert_eq!(
            routed.with_route(aux2, Some(BusEndpoint::TrackTap(8)), 44_100, Some(2)),
            Err(BusLayoutError::RateMismatch)
        );
        assert_eq!(
            routed.with_route(aux2, Some(BusEndpoint::TrackTap(8)), 48_000, Some(1)),
            Err(BusLayoutError::LayoutMismatch)
        );
        assert_eq!(
            routed.with_route(
                aux2,
                Some(BusEndpoint::DestinationTrack(8)),
                48_000,
                Some(2)
            ),
            Err(BusLayoutError::IncompatibleEndpoint)
        );
        assert_eq!(routed.route_for(aux2), Some(BusEndpoint::TrackTap(7)));

        // Deleting track 7 prunes only that route.
        let pruned = routed.without_deleted_track(7);
        assert_eq!(pruned.route_for(aux2), None);
    }

    #[test]
    fn multi_output_routes_survive_bus_rename_and_deactivating_sibling_never_shifts_indices() {
        let initial = PluginBusLayout::new(
            48_000,
            vec![
                audio_bus(BusDirection::Output, 0, BusKind::Main, 2, true, "main"),
                audio_bus(BusDirection::Output, 1, BusKind::Auxiliary, 2, true, "kick"),
                audio_bus(
                    BusDirection::Output,
                    2,
                    BusKind::Auxiliary,
                    2,
                    true,
                    "snare",
                ),
            ],
        )
        .unwrap();

        let out1 = BusAddress {
            media: BusMedia::Audio,
            direction: BusDirection::Output,
            index: 1,
        };
        let out2 = BusAddress {
            media: BusMedia::Audio,
            direction: BusDirection::Output,
            index: 2,
        };

        let routed = initial
            .with_route(
                out1,
                Some(BusEndpoint::DestinationTrack(10)),
                48_000,
                Some(2),
            )
            .unwrap()
            .with_route(
                out2,
                Some(BusEndpoint::DestinationTrack(20)),
                48_000,
                Some(2),
            )
            .unwrap();

        // Deactivating bus 1 drops only bus 1's route; bus 2 keeps exact index 2 -> track 20.
        let after_disable = routed.with_bus_active(out1, false).unwrap();
        assert_eq!(after_disable.route_for(out1), None);
        assert_eq!(
            after_disable.route_for(out2),
            Some(BusEndpoint::DestinationTrack(20))
        );

        // After I/O restart with renamed buses and bus 1 inactive, bus 2 stays bound to track 20.
        let refreshed = PluginBusLayout::new(
            48_000,
            vec![
                audio_bus(
                    BusDirection::Output,
                    0,
                    BusKind::Main,
                    2,
                    true,
                    "stereo 1-2",
                ),
                audio_bus(
                    BusDirection::Output,
                    1,
                    BusKind::Auxiliary,
                    2,
                    false,
                    "aux a",
                ),
                audio_bus(
                    BusDirection::Output,
                    2,
                    BusKind::Auxiliary,
                    2,
                    true,
                    "aux b",
                ),
            ],
        )
        .unwrap();
        let reconciled = routed.reconcile_after_restart(&refreshed).unwrap();
        assert_eq!(reconciled.route_for(out1), None);
        assert_eq!(
            reconciled.route_for(out2),
            Some(BusEndpoint::DestinationTrack(20))
        );
    }
}
