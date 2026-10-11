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
`hex-devices` implements and registers Utility and eq eight. A device definition supplies
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

Analytic instrument definitions may optionally prepare an immutable shared
sampling context with compact per-note data. Arrangement and Session share the
same context table within a prepared revision; compatible state/type/rate
configurations reuse owners across revisions, and old copied releases keep
their originating context through bounded playback leases. The existing
41-word inline sampler stays unchanged for Sine and FM4. Contexts are generic
and do not admit Operator or mutable voices; exact byte bounds and control-side
retirement are specified in [instrument slots](lattice-instruments.md).

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

Memoryless `StereoMatrix` transforms declare exact zero algorithmic latency.
Prepared stateful processors declare `Option<u32>` frames: the default `None`
means unknown, while registered Utility and eq eight filters report `Some(0)`
because they use no fixed buffering or lookahead. Their IIR phase/group delay is separate.
This metadata does not remove interface latency or implement general device
latency compensation. Return effect admission now requires a registered
processor's explicit zero-latency declaration; unknown and nonzero values are
refused before candidate history is committed. Processing does not introduce
clipping or a limiter; extreme finite inputs and gains can still overflow, as in
the existing renderer.

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

DC removal, bass mono and Operator require mutable DSP state. `hex-lattice` now
defines a default-off mutable instrument runtime with fixed committed/scratch
twins, bounded caller event slices, preflight, a finite declared release/output
tail and committed-state quiescence query, plus hard reset. Engine admission
still needs to gate analytic versus mutable preparation, transfer compatible
state by stable device identity and instrument configuration, map source events
to occurrence IDs, define route/tail ownership and retire runtime owners on
control. Native and offline paths must share those processing rules.

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
gates are recorded in [stateful lattice](stateful-lattice.md). Instruments now
have a separate optional paired mutable-runtime contract with ordered caller-owned
events, bounded per-definition capacity and declared tail quiescence. It is a
test-only foundation; engine event generation, Arrangement/Session integration,
outgoing tails, sustained chase and callback retirement/probes remain open. See
[instrument slots](lattice-instruments.md).

## eq eight

The registered original `hex.eq8` implementation and compact regular/Return
device panel are documented in [eq eight](eq-eight.md). Its stateful coefficient
bank uses compatible same-rate history transfer; channel-mode changes reset the
incompatible lane history while remaining publishable. The panel's frequency
response is a bounded control-side query of the prepared coefficients, not a
live analyzer or a claim of Live response parity. Native visual and listening
acceptance remain with the user.

## direct instrument replacement

Browser replacement2262: dragging a browser instrument directly over the current
instrument card explicitly targets that stable instrument ID and replaces its
state in place. A single replacement edit preserves chain position, bypass,
effects and Arrangement clips and is undone as one operation. Drops over effect
cards, empty rail space or stale targets do not imply replacement and remain
subject to the occupied-slot refusal. When the instrument type changes, the UI
retires controls, pending values and type-specific page state keyed to the former
device type before rendering the new controls. Session and GPUI regressions cover
replacement, invalid/stale targets, preservation and Undo; full local CI and
native pointer acceptance remain open until delivery.

## compatible effect parameter history

An optional default-false runtime proof allows certified EQ8 parameter changes
to adopt ordinary filter history in the effect-only Arrangement handoff. Both
candidate twins must certify compatibility after ordered identity/topology checks;
uncertified effects retain existing tail behavior. This does not transfer ordinary
history out of a separately owned instrument-release suffix. Return adoption uses the same proof and keeps one owner for the old current
chain, while independently carried tails stay separate. See
[eq8 parameter history](eq-eight-parameter-history.md).

### optional transient processor audition

The native runtime contract may opt into `NativeProcessorAudition`; its default
implementation exposes no capability. This controls audio-owned temporary
monitoring and does not add a descriptor, authored parameter or serialized state.
The runtime chain validates typed owner/device/type and both transactional twins
before activating a selector. Smooth release and immediate transient cancellation
are distinct operations; normal DSP histories remain intact. EQ8 is the first
implementation. See [the EQ8 host contract](eq-eight-audition.md#transient-host-contract-pulse2253)
for the bounded audio lease, publication fences, pointer lifecycle and remaining
native acceptance gates. Adding another capable device does not require a storage
special case or a mutable type-erased escape hatch.
