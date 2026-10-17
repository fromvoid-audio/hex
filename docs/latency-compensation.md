# bounded runtime latency compensation

Pulse28 / PR184 derives latency from actual prepared native processors and
instrument runtimes. Enabled unknown declarations refuse preparation; bypass is
zero audible algorithmic latency. Analytic and mutable instrument declarations
must agree. Built-in instruments declare zero buffering; filter phase is not
misrepresented as removable fixed delay. Compensation aligns signal paths and
cannot remove causal processing, interface, buffer or physical latency.

A typed DAG separates audio input/prefix, independent instrument generation,
merge, suffix/fader, Return chains and Main. At each node, input arrival is the
maximum predecessor output arrival, output arrival adds its checked declared
latency, and each incoming edge delays by target input minus source output.
Audio/instrument reconvergence, sends, Return feeds and track inputs share this
law. Dependency ties and Main summation retain canonical order.

Control validates graph identity, endpoints, cycles, arithmetic and budgets before
publication. Policy caps are4096 nodes,16384 edges,65536 cumulative frames and
16MiB actual per-runtime heap, including committed/speculative delay banks,
transport history and prepared tables. Active/candidate coexistence is capped at
32MiB. These are engine admission limits, not whole-process memory guarantees.
Construction, reservation and destruction belong to control.

Positive-delay edges own preallocated stereo rings. A render transaction changes
only speculative state; commit/rollback copies touched circular intervals.
A late render failure silences the requested block and preserves committed DSP,
delay, route and transport history for an exact retry. Callback scopes do not
allocate, lock, perform I/O or destroy final graph owners. Zero-latency projects
without authored audio feeds retain the existing block path and arithmetic.
Logical MIDI-only routes use that block path with independent audio and MIDI
source ownership; positive declared latency still uses the compensation graph.

Exact arrangement frame, loop pass and distinct Q32.32 performance/Arrangement beat histories travel
with delayed content. Native clock queries and downstream gain evaluation use
the origin at their node, including across loop wraps. Startup history is
explicit silence; it never substitutes current content for unavailable past
content. Pause freezes state, loop wrap retains it, and Stop/Seek reset it.
Recording receipts wait the remaining fader-to-Main delay. Output capture observes
aligned track/Return contributions before the single master stage; processor
observations retain their processing-offset diagnostic contract.

EQ Arrangement automation uses the explicit `arrangement_beat_at_offset` clock,
never the continuously advancing performance beat. Ordinary Track/Return blocks
borrow the authored tempo map; compensated nodes receive the Arrangement beat
stored with their original content frame. Warmup remains unknown and stopped
monitoring does not advance authored automation. Linear export uses the same
clock without transport-loop wrapping. The extra stamp field is included in
the existing capacity-based compensation storage accounting.

Compatible layout handoffs preserve in-flight delay/DSP state while applying
candidate coefficients. Changed latency/topology and incompatible histories while
playing or paused refuse before audible-owner mutation and require Stop. A fully stopped/reset
runtime may adopt a fresh layout. Detached residuals drain intrinsic latency plus
residual tail under existing bounded lifecycle rules and never replay old routes.

Both offline export entry points render the requested extent plus Main latency,
discard causal startup, and publish exactly the requested frame count. They keep
actual future source input during that drain, preserving first/final requested
samples without extending the published range. A nonzero range starts with reset processor
history at its requested source start; it does not reconstruct earlier effect
history implicitly. Existing create-new publication
and interrupted-write recovery remain in force.

Unit/render tests and allocation probes cover specific synthetic normal-registry
fixtures. They do not establish CoreAudio deadlines, listening quality, Live
parity or arbitrary plugin delay. Performance measurements state workload and
hardware separately in [performance](performance.md). Native listening remains
an optional exact-build check, using the supported optimized delivery tooling.

Pulse #2646 extends each transport stamp with the exact source performance frame
and actual running state. Node clocks preserve these through the same delay and
transaction history as content frames, beats and loop passes. Startup history
returns unknown observations. Stopped audition explicitly reports a stopped
transport and no advancing performance origin, even though its musical audition
clock can advance. Stamp storage remains charged against existing compensation
limits using the actual type size and allocated ring capacities; no limit rises.

Arrangement beat stamps are optional when a content frame cannot be represented
in Q32.32. The exact frame/performance clock remains valid; effects observe an
unavailable beat, matching the ordinary clock path. This does not introduce a
new tempo-conversion failure for non-automated audio. EQ uses its documented
authored-base behavior for unavailable beat clocks. Stamp storage accounting uses
the complete optional-field layout.
