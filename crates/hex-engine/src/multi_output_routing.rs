//! Bounded multi-output VST3 instrument routing to distinct Hex destinations.
//!
//! Preserves exact SDK output bus indices through bus activation, deactivation,
//! renaming, and I/O restart without shifting sibling routes or silently
//! downmixing inactive buses. Preallocated per-bus delay lines keep all routed
//! outputs phase-aligned under uniform PDC latency and compute per-bus peak meters.

use crate::Frame;
use std::collections::BTreeSet;
use std::fmt;

/// Matches `max_instrument_outputs` in the native macOS VST3 helper.
pub const MAX_MULTI_OUTPUT_BUSES: usize = 16;
/// Bounded per-bus PDC alignment ceiling (matching `MAX_MAIN_PASS_DELAY_FRAMES` scale).
pub const MAX_MULTI_OUTPUT_PDC_FRAMES: usize = 8_192;

const SNAPSHOT_MAGIC: &[u8; 4] = b"HXMO";
const SNAPSHOT_VERSION: u8 = 1;

/// Distinct Hex destination receiving one VST3 audio output bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MultiOutputDestination {
    Track(u64),
    Return(u64),
    Main,
}

/// Stable authored binding from one plugin instance's output `bus_index` to a Hex destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MultiOutputRoute {
    pub device_id: u64,
    pub bus_index: u16,
    pub destination: MultiOutputDestination,
    pub active: bool,
}

/// Peak stereo meter observation for one routed multi-output bus.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MultiOutputBusMeter {
    pub bus_index: u16,
    pub peak_left: f32,
    pub peak_right: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MultiOutputError {
    BusIndexOutOfRange,
    DuplicateBusRoute,
    TooManyRoutes,
    PdcExceedsLimit,
    BlockLengthMismatch,
    MissingActiveBus,
    MalformedSnapshot,
}

impl fmt::Display for MultiOutputError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(match self {
            Self::BusIndexOutOfRange => "vst3 output bus index exceeds multi-output capacity",
            Self::DuplicateBusRoute => "vst3 output bus already has an assigned destination",
            Self::TooManyRoutes => "vst3 multi-output route count exceeds capacity",
            Self::PdcExceedsLimit => "vst3 multi-output pdc delay exceeds bounded limit",
            Self::MissingActiveBus => "active vst3 output bus has no block buffer",
            Self::BlockLengthMismatch => "vst3 multi-output bus block length mismatch",
            Self::MalformedSnapshot => "vst3 multi-output route snapshot is malformed",
        })
    }
}

impl std::error::Error for MultiOutputError {}

/// Validated table of multi-output routes for one or more hosted instruments.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MultiOutputRouteTable {
    routes: Vec<MultiOutputRoute>,
}

impl MultiOutputRouteTable {
    pub fn new(routes: Vec<MultiOutputRoute>) -> Result<Self, MultiOutputError> {
        if routes.len() > MAX_MULTI_OUTPUT_BUSES * 16 {
            return Err(MultiOutputError::TooManyRoutes);
        }
        let mut seen = BTreeSet::new();
        for route in &routes {
            if usize::from(route.bus_index) >= MAX_MULTI_OUTPUT_BUSES {
                return Err(MultiOutputError::BusIndexOutOfRange);
            }
            if !seen.insert((route.device_id, route.bus_index)) {
                return Err(MultiOutputError::DuplicateBusRoute);
            }
        }
        let mut sorted = routes;
        sorted.sort_by_key(|route| (route.device_id, route.bus_index));
        Ok(Self { routes: sorted })
    }

    pub fn routes(&self) -> &[MultiOutputRoute] {
        &self.routes
    }

    pub fn route_for(&self, device_id: u64, bus_index: u16) -> Option<MultiOutputRoute> {
        self.routes
            .iter()
            .copied()
            .find(|route| route.device_id == device_id && route.bus_index == bus_index)
    }

    /// Transactionally set or clear the destination for `(device_id, bus_index)`
    /// without shifting or mutating any sibling bus indices.
    pub fn with_route(
        &self,
        device_id: u64,
        bus_index: u16,
        destination: Option<MultiOutputDestination>,
        active: bool,
    ) -> Result<Self, MultiOutputError> {
        if usize::from(bus_index) >= MAX_MULTI_OUTPUT_BUSES {
            return Err(MultiOutputError::BusIndexOutOfRange);
        }
        let mut next = self.routes.clone();
        next.retain(|route| !(route.device_id == device_id && route.bus_index == bus_index));
        if let Some(destination) = destination {
            next.push(MultiOutputRoute {
                device_id,
                bus_index,
                destination,
                active,
            });
        }
        Self::new(next)
    }

    /// Toggle bus activation for `(device_id, bus_index)` while keeping all sibling
    /// bus indices and destinations intact.
    pub fn with_bus_active(&self, device_id: u64, bus_index: u16, active: bool) -> Self {
        let mut next = self.routes.clone();
        for route in &mut next {
            if route.device_id == device_id && route.bus_index == bus_index {
                route.active = active;
            }
        }
        Self { routes: next }
    }

    /// Reconcile routes against the set of active bus indices exposed by `device_id`
    /// after an I/O restart or bus rename: buses still exposed stay bound to their
    /// exact destinations; absent/inactive buses become inactive without shifting
    /// other buses or downmixing.
    pub fn reconcile_device_buses(&self, device_id: u64, active_bus_indices: &[u16]) -> Self {
        let active_set: BTreeSet<u16> = active_bus_indices.iter().copied().collect();
        let mut next = self.routes.clone();
        for route in &mut next {
            if route.device_id == device_id {
                route.active = active_set.contains(&route.bus_index);
            }
        }
        Self { routes: next }
    }

    /// Deterministic binary snapshot encoding for project save/reopen round trips.
    pub fn encode_snapshot(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(7 + self.routes.len() * 20);
        out.extend_from_slice(SNAPSHOT_MAGIC);
        out.push(SNAPSHOT_VERSION);
        out.extend_from_slice(&(self.routes.len() as u16).to_le_bytes());
        for route in &self.routes {
            out.extend_from_slice(&route.device_id.to_le_bytes());
            out.extend_from_slice(&route.bus_index.to_le_bytes());
            out.push(u8::from(route.active));
            match route.destination {
                MultiOutputDestination::Main => {
                    out.push(0);
                    out.extend_from_slice(&0_u64.to_le_bytes());
                }
                MultiOutputDestination::Track(id) => {
                    out.push(1);
                    out.extend_from_slice(&id.to_le_bytes());
                }
                MultiOutputDestination::Return(id) => {
                    out.push(2);
                    out.extend_from_slice(&id.to_le_bytes());
                }
            }
        }
        out
    }

    /// Strict snapshot decoder rejecting truncated, trailing, duplicate, or invalid routes.
    pub fn decode_snapshot(bytes: &[u8]) -> Result<Self, MultiOutputError> {
        let header = bytes.get(..7).ok_or(MultiOutputError::MalformedSnapshot)?;
        if &header[..4] != SNAPSHOT_MAGIC || header[4] != SNAPSHOT_VERSION {
            return Err(MultiOutputError::MalformedSnapshot);
        }
        let count = usize::from(u16::from_le_bytes([header[5], header[6]]));
        let expected_len = 7usize
            .checked_add(
                count
                    .checked_mul(20)
                    .ok_or(MultiOutputError::MalformedSnapshot)?,
            )
            .ok_or(MultiOutputError::MalformedSnapshot)?;
        if bytes.len() != expected_len {
            return Err(MultiOutputError::MalformedSnapshot);
        }
        let mut routes = Vec::with_capacity(count);
        for index in 0..count {
            let start = 7 + index * 20;
            let rec = &bytes[start..start + 20];
            let device_id = u64::from_le_bytes(
                rec[0..8]
                    .try_into()
                    .map_err(|_| MultiOutputError::MalformedSnapshot)?,
            );
            let bus_index = u16::from_le_bytes([rec[8], rec[9]]);
            let active = match rec[10] {
                0 => false,
                1 => true,
                _ => return Err(MultiOutputError::MalformedSnapshot),
            };
            let target_raw = u64::from_le_bytes(
                rec[12..20]
                    .try_into()
                    .map_err(|_| MultiOutputError::MalformedSnapshot)?,
            );
            let destination = match rec[11] {
                0 if target_raw == 0 => MultiOutputDestination::Main,
                1 => MultiOutputDestination::Track(target_raw),
                2 => MultiOutputDestination::Return(target_raw),
                _ => return Err(MultiOutputError::MalformedSnapshot),
            };
            routes.push(MultiOutputRoute {
                device_id,
                bus_index,
                destination,
                active,
            });
        }
        Self::new(routes)
    }
}

/// Preallocated per-bus PDC delay ring for one multi-output instrument instance.
#[derive(Clone, Debug)]
struct BusDelayLine {
    ring: Vec<Frame>,
    cursor: usize,
    delay_frames: usize,
}

impl BusDelayLine {
    fn new(delay_frames: usize) -> Result<Self, MultiOutputError> {
        if delay_frames > MAX_MULTI_OUTPUT_PDC_FRAMES {
            return Err(MultiOutputError::PdcExceedsLimit);
        }
        Ok(Self {
            ring: vec![[0.0, 0.0]; delay_frames.max(1)],
            cursor: 0,
            delay_frames,
        })
    }

    fn process_in_place(&mut self, frames: &mut [Frame]) {
        if self.delay_frames == 0 {
            return;
        }
        let len = self.delay_frames;
        for sample in frames {
            let delayed = self.ring[self.cursor];
            self.ring[self.cursor] = *sample;
            self.cursor += 1;
            if self.cursor >= len {
                self.cursor = 0;
            }
            *sample = delayed;
        }
    }
}

/// Control-prepared, allocation-free per-block router for one multi-output VST3
/// instrument. Routes each active bus to its exact destination with uniform PDC
/// alignment and per-bus peak metering; inactive/unrouted buses are never downmixed.
#[derive(Clone, Debug)]
pub struct MultiOutputRouter {
    device_id: u64,
    pdc_frames: usize,
    routes: [Option<MultiOutputRoute>; MAX_MULTI_OUTPUT_BUSES],
    required_bus_buffers: usize,
    delays: Vec<BusDelayLine>,
    meters: [MultiOutputBusMeter; MAX_MULTI_OUTPUT_BUSES],
}

impl MultiOutputRouter {
    pub fn prepare(
        device_id: u64,
        table: &MultiOutputRouteTable,
        pdc_frames: usize,
    ) -> Result<Self, MultiOutputError> {
        if pdc_frames > MAX_MULTI_OUTPUT_PDC_FRAMES {
            return Err(MultiOutputError::PdcExceedsLimit);
        }
        let mut routes = [None; MAX_MULTI_OUTPUT_BUSES];
        for route in table
            .routes()
            .iter()
            .copied()
            .filter(|r| r.device_id == device_id)
        {
            routes[usize::from(route.bus_index)] = Some(route);
        }
        let required_bus_buffers = routes
            .iter()
            .rposition(|route| route.is_some_and(|route| route.active))
            .map_or(0, |index| index + 1);
        let mut delays = Vec::with_capacity(MAX_MULTI_OUTPUT_BUSES);
        for _ in 0..MAX_MULTI_OUTPUT_BUSES {
            delays.push(BusDelayLine::new(pdc_frames)?);
        }
        let mut meters = [MultiOutputBusMeter::default(); MAX_MULTI_OUTPUT_BUSES];
        for (index, meter) in meters.iter_mut().enumerate() {
            meter.bus_index = index as u16;
        }
        Ok(Self {
            device_id,
            pdc_frames,
            routes,
            required_bus_buffers,
            delays,
            meters,
        })
    }

    pub const fn device_id(&self) -> u64 {
        self.device_id
    }

    pub const fn pdc_frames(&self) -> usize {
        self.pdc_frames
    }

    pub fn meter(&self, bus_index: u16) -> Option<MultiOutputBusMeter> {
        self.meters.get(usize::from(bus_index)).copied()
    }

    /// Route one block of per-bus stereo PCM into caller-supplied destination
    /// buffers via `sink(destination, frames)`.
    ///
    /// A missing active routed bus refuses the whole block before delivery,
    /// metering or delay advancement. Trailing inactive buses may be omitted.
    /// Performs zero heap allocations. Inactive or unrouted buses are ignored
    /// (never downmixed into Main or sibling buses).
    pub fn route_block(
        &mut self,
        bus_buffers: &mut [&mut [Frame]],
        expected_frames: usize,
        mut sink: impl FnMut(MultiOutputDestination, &[Frame]),
    ) -> Result<(), MultiOutputError> {
        if bus_buffers.len() > MAX_MULTI_OUTPUT_BUSES {
            return Err(MultiOutputError::BusIndexOutOfRange);
        }
        if bus_buffers.len() < self.required_bus_buffers {
            return Err(MultiOutputError::MissingActiveBus);
        }
        for buffer in bus_buffers.iter() {
            if buffer.len() != expected_frames {
                return Err(MultiOutputError::BlockLengthMismatch);
            }
        }
        for (bus_index, buffer) in bus_buffers.iter_mut().enumerate() {
            let Some(route) = self.routes[bus_index] else {
                self.meters[bus_index].peak_left = 0.0;
                self.meters[bus_index].peak_right = 0.0;
                continue;
            };
            if !route.active {
                self.meters[bus_index].peak_left = 0.0;
                self.meters[bus_index].peak_right = 0.0;
                continue;
            }
            self.delays[bus_index].process_in_place(buffer);
            let mut peak_l = 0.0_f32;
            let mut peak_r = 0.0_f32;
            for frame in buffer.iter() {
                peak_l = peak_l.max(frame[0].abs());
                peak_r = peak_r.max(frame[1].abs());
            }
            self.meters[bus_index].peak_left = peak_l;
            self.meters[bus_index].peak_right = peak_r;
            sink(route.destination, buffer);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn multi_output_routes_distinct_signals_with_pdc_meters_and_no_silent_downmix() {
        let device = 100_u64;
        let kick_track = 10_u64;
        let snare_track = 20_u64;
        let room_return = 30_u64;

        let table = MultiOutputRouteTable::default()
            .with_route(device, 0, Some(MultiOutputDestination::Main), true)
            .unwrap()
            .with_route(
                device,
                1,
                Some(MultiOutputDestination::Track(kick_track)),
                true,
            )
            .unwrap()
            .with_route(
                device,
                2,
                Some(MultiOutputDestination::Track(snare_track)),
                false, // Inactive bus must NOT bleed or downmix into Main or siblings.
            )
            .unwrap()
            .with_route(
                device,
                3,
                Some(MultiOutputDestination::Return(room_return)),
                true,
            )
            .unwrap();

        // Save/reopen snapshot round trip retains exact device, bus_index, destination, and active flags.
        let encoded = table.encode_snapshot();
        assert_eq!(
            MultiOutputRouteTable::decode_snapshot(&encoded).unwrap(),
            table
        );
        for len in 0..encoded.len() {
            assert!(MultiOutputRouteTable::decode_snapshot(&encoded[..len]).is_err());
        }

        let mut router = MultiOutputRouter::prepare(device, &table, 2).unwrap();
        let mut bus0 = [[0.1, -0.1]; 4];
        let mut bus1 = [[0.5, 0.25]; 4];
        let mut bus2 = [[0.9, -0.9]; 4]; // Inactive
        let mut bus3 = [[-0.3, 0.4]; 4];

        let mut delivered: BTreeMap<MultiOutputDestination, Vec<Frame>> = BTreeMap::new();
        {
            let mut slices: [&mut [Frame]; 4] = [&mut bus0, &mut bus1, &mut bus2, &mut bus3];
            router
                .route_block(&mut slices, 4, |dest, pcm| {
                    delivered.entry(dest).or_default().extend_from_slice(pcm);
                })
                .unwrap();
        }

        // Exact 2-frame PDC alignment on all active destinations; inactive bus 2 absent.
        assert_eq!(
            delivered.get(&MultiOutputDestination::Main).unwrap(),
            &vec![[0.0, 0.0], [0.0, 0.0], [0.1, -0.1], [0.1, -0.1]]
        );
        assert_eq!(
            delivered
                .get(&MultiOutputDestination::Track(kick_track))
                .unwrap(),
            &vec![[0.0, 0.0], [0.0, 0.0], [0.5, 0.25], [0.5, 0.25]]
        );
        assert!(
            !delivered.contains_key(&MultiOutputDestination::Track(snare_track)),
            "inactive bus must not emit or downmix"
        );
        assert_eq!(
            delivered
                .get(&MultiOutputDestination::Return(room_return))
                .unwrap(),
            &vec![[0.0, 0.0], [0.0, 0.0], [-0.3, 0.4], [-0.3, 0.4]]
        );

        // Accurate per-bus peak meters.
        assert_eq!(router.meter(0).unwrap().peak_left, 0.1);
        assert_eq!(router.meter(1).unwrap().peak_left, 0.5);
        assert_eq!(router.meter(2).unwrap().peak_left, 0.0);
        assert_eq!(router.meter(3).unwrap().peak_right, 0.4);
    }

    #[test]
    fn missing_active_bus_refuses_before_pcm_meters_or_delay_advance() {
        let device = 19;
        let table = MultiOutputRouteTable::default()
            .with_route(device, 0, Some(MultiOutputDestination::Main), true)
            .unwrap()
            .with_route(device, 2, Some(MultiOutputDestination::Track(7)), true)
            .unwrap()
            .with_route(device, 3, Some(MultiOutputDestination::Return(8)), true)
            .unwrap()
            .with_bus_active(device, 3, false);
        let mut router = MultiOutputRouter::prepare(device, &table, 1).unwrap();
        fn render(
            router: &mut MultiOutputRouter,
            marker: f32,
        ) -> Vec<(MultiOutputDestination, Vec<Frame>)> {
            let mut main = [[marker, marker]; 2];
            let mut skipped = [[0.0; 2]; 2];
            let mut auxiliary = [[-marker, marker]; 2];
            let mut delivered = Vec::new();
            // The inactive trailing bus has no supplied buffer.
            router
                .route_block(
                    &mut [&mut main, &mut skipped, &mut auxiliary],
                    2,
                    |destination, pcm| delivered.push((destination, pcm.to_vec())),
                )
                .unwrap();
            delivered
        }
        assert_eq!(render(&mut router, 0.5).len(), 2);
        let mut untouched = router.clone();
        let mut main = [[0.875, -0.875]; 2];
        let original = main;
        let mut published = false;
        let result = router.route_block(&mut [&mut main], 2, |_, _| published = true);
        assert_eq!(result, Err(MultiOutputError::MissingActiveBus));
        assert!(!published);
        assert_eq!(main, original);
        for bus in [0, 2] {
            assert_eq!(router.meter(bus), untouched.meter(bus));
        }
        assert_eq!(render(&mut router, 0.25), render(&mut untouched, 0.25));
    }

    #[test]
    fn restart_reconciliation_and_bus_deactivation_preserve_sibling_bus_identities() {
        let device = 55_u64;
        let t1 = 1_u64;
        let t2 = 2_u64;
        let table = MultiOutputRouteTable::default()
            .with_route(device, 0, Some(MultiOutputDestination::Main), true)
            .unwrap()
            .with_route(device, 1, Some(MultiOutputDestination::Track(t1)), true)
            .unwrap()
            .with_route(device, 2, Some(MultiOutputDestination::Track(t2)), true)
            .unwrap();

        // Deactivating bus 1 leaves bus 2 anchored at bus_index 2 -> Track(2).
        let after_disable = table.with_bus_active(device, 1, false);
        assert!(!after_disable.route_for(device, 1).unwrap().active);
        let bus2 = after_disable.route_for(device, 2).unwrap();
        assert!(bus2.active);
        assert_eq!(bus2.destination, MultiOutputDestination::Track(t2));

        // After I/O restart where only buses 0 and 2 remain active, bus 2 keeps its exact route.
        let reconciled = table.reconcile_device_buses(device, &[0, 2]);
        assert!(!reconciled.route_for(device, 1).unwrap().active);
        assert!(reconciled.route_for(device, 2).unwrap().active);
        assert_eq!(
            reconciled.route_for(device, 2).unwrap().destination,
            MultiOutputDestination::Track(t2)
        );
    }
}
