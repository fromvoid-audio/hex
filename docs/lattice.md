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

`hex-project` remains independent of DSP crates. The next integration introduces
stable device IDs, ordered per-track effects, validated settings and typed
insert/remove/reorder/update/bypass commands. Canonical snapshots and storage
must retain every instance; a fused processing matrix is derived data, never the
saved editable chain. Schema 14 will explicitly migrate older projects to empty
effect chains. Candidate compilation must succeed before history commits.

The engine prepares Utility matrices on the control side. Because these effects
are linear and memoryless, their ordered composition can process each source
contribution before summation without a track scratch buffer. Effects run before
track gain/balance/mute and master gain. Arrangement audio/MIDI, Session sources,
resident and streamed paths must share this transform. Captured Session release
tails retain the prepared mix belonging to their source revision, consistent
with the existing release handoff policy. A neutral chain has an exact identity
path; preparation rejects non-finite coefficients or composition overflow.

This first implementation declares zero algorithmic latency. It does not remove
interface latency or implement general device latency compensation. Processing
does not introduce clipping or a limiter; extreme finite inputs and gains can
still overflow, as in the existing renderer.

## Delivery and acceptance

Pulse owns the active tickets. The first focused prerequisite is ticket 42:
lattice stereo primitives and original linear Utility DSP, independently tested.
It does not yet expose desktop device processing. Ticket 16 owns canonical
device model/runtime integration; ticket 17 owns the real compact device panel;
ticket 18 owns complete Utility functionality and reference acceptance; ticket
19 owns Operator. The user prioritized these before main/return-track layout.

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
