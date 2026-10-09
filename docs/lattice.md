# lattice: native device DSP

The user selected **lattice** on 9 October 2026 as the name of Hex's native
device DSP system. It supports independently implemented native instruments and
effects; CLAP and VST3 hosting remain separate adapters. The name is app-owned
lowercase text. It does not imply measured Ableton compatibility.

## First implementation boundary

`hex-lattice` owns concrete stereo processing primitives: a validated, immutable
2×2 matrix, ordered composition and allocation-free frame/block application.
`hex-devices` owns original device algorithms, starting with Utility's memoryless
linear controls alongside the existing sine instrument. Neither crate owns
project editing, persistence, UI, device I/O or plugin loading.

`hex-lattice-contract` owns neutral device metadata and bounded versioned state:
stable type/parameter identifiers, parameter values/descriptors and opaque state
payloads. `hex-project` deliberately depends on this shared contract while
remaining independent of DSP implementations. It owns instance IDs, ordered
per-track effects and structural envelope/history validation.

Type IDs are bounded lowercase namespace strings such as `hex.utility`;
parameter IDs are stable within each type. Opaque payloads use immutable shared
bytes so project/history clones retain state without copying each payload.
State parsing and replacement allocations belong on control workers.

`hex-lattice` owns the native processing-definition and registry seam;
`hex-devices` implements and registers Utility. A device definition supplies
defaults, validation, state migration, parameter access/patching and preparation.
Its codec interprets its own payload. Adding a device means implementing this
contract and registering it, without adding a per-device storage DTO or engine
type switch. The first admitted processing class is memoryless linear audio
effects; other classes need a real runtime implementation before admission.

Canonical snapshots and storage retain each instance's ID, bypass, stable type
ID, state version and opaque payload. A fused processing matrix is derived data,
never the saved editable chain. Schema 14 explicitly migrates older projects to
empty effect chains. Unknown types and future state versions round-trip without
payload loss; compilation refuses unavailable/unsupported processing explicitly,
including bypassed missing devices. Known malformed payloads never default-reset.
Candidate compilation must succeed before history commits.

Generic project commands insert/remove/reorder/bypass/replace device state.
Parameter requests resolve the registered definition on the session worker and
patch its current canonical state before prepared commit, preventing stale whole
settings from overwriting another edit. Descriptors supply UI control metadata.
Tests register a second independent definition through the same storage and
preparation paths to establish the modular seam.

The engine prepares Utility matrices on the control side. Because these effects
are linear and memoryless, their ordered composition can process each source
contribution before summation without a track scratch buffer. Effects run before
track gain/balance/mute and master gain. Arrangement audio/MIDI, Session sources,
resident and streamed paths must share this transform. Captured Session release
tails retain the prepared mix belonging to their source revision. This makes
the policy consistent for retained and removed tracks: previously only removed
tracks used the captured mix, while retained-track tails used the current mix.
A neutral chain has an exact identity
path; preparation rejects non-finite coefficients or composition overflow.

This first implementation declares zero algorithmic latency. It does not remove
interface latency or implement general device latency compensation. Processing
does not introduce clipping or a limiter; extreme finite inputs and gains can
still overflow, as in the existing renderer.

## Delivery and acceptance

Pulse owns the active tickets. The first focused prerequisite is ticket 42:
lattice stereo primitives and original linear Utility DSP, independently tested.
It does not yet expose desktop device processing. Ticket 16 owns canonical
device model/runtime integration, with canonical chains delivered in ticket 43
and the modular contract tracked by ticket 70. Ticket 44 owns a reusable
Live-style native device-control toolkit;
ticket 17 owns the real compact device panel and depends on both;
ticket 18 owns complete Utility functionality and reference acceptance; ticket
19 owns Operator. The user prioritized these before main/return-track layout.
The toolkit supplies knobs, sliders, editable values, toggles, selectors and
compact device frames without owning canonical device state or DSP.

Source tests cover identity, asymmetric stereo inputs, polarity/channel choices,
mono/width/M-S behavior, serial order, malformed parameters and sample/block
equivalence. Integration must additionally cover history, schema migrations,
round trips, failed preparation, Arrangement/Session/offline agreement and
callback allocation probes. The panel reads canonical state and dispatches
commands; native UI and listening acceptance belong to the user.

Utility's control inventory is informed by the
[Live 12 manual](https://www.ableton.com/en/live-manual/12/live-audio-effect-reference/#utility).
Exact transfer laws, filter designs and control interactions require original
reference measurements; matching control names is not proof of parity. No
proprietary implementation or assets are copied.

## Stateful follow-ups

DC removal, bass mono and Operator require mutable DSP state. Immutable
`PreparedRevision` owns prepared configuration; mutable playback owns processing
state. Before admitting stateful devices, define compatible state transfer by
stable device ID across parameter edits, initialization on insertion/type changes,
seek/loop/stop reset semantics, bounded event scheduling and control-side
retirement. Native and offline paths must use the same processing rules.

Stage an explicit single instrument slot before Operator, preserving the current
sine instrument as the migration default. Do not pretend a matrix represents a
filter, nonlinear effect or instrument, and do not expose decorative unsupported
DC/bass-mono/Operator controls while these dependencies remain open.


Stateful processing runs once on each track's summed signal, before track mix.
Current linear fusion remains valid. Captured outgoing releases retain their
source-revision chain in a fixed audio-owned tail bank, separate from current
track audio. Preflight handoff capacity before any mutation; saturation retains
the old graph and defers publication. Move/swap runtime owners and park completed
tail resources for later control-side retirement, never final-drop in callback.
Count and byte budgets, compatible stable-ID history transfer, reset/pause/loop
semantics, shared offline processing and allocation/handoff/tail tests precede
admission. Standalone original filters are specified in
[Utility filters](utility-filters.md); their source tests do not establish native
processing or exact Live response.

## stateful effect runtime implementation

Generic paired stateful processors, history transfer, transactional rendering and
control retirement are now implemented. Original Utility filters use the same
registered codec and native/offline path. Scope, bounds, probes and open performance
gates are recorded in [stateful lattice](stateful-lattice.md); instruments still
require their own event/voice contract.
