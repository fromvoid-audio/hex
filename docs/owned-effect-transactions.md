# owned effect transactions

Pulse #2673 / PR289 establishes an additive transaction capability for long-history
effects. This is the W04 bootstrap, not permission to increase production budgets
or publish a seconds-long device. Production definitions still select their
existing legacy twin capability; the registered original delay is an opt-in
`conformance` fixture and never appears in the device browser.

## preparation and ownership

`NativeDeviceDefinition::prepare_runtime_factory` defaults to wrapping the existing
`prepare_processor` result. `PreparedProcessorFactory` selects exactly one legacy
or owned capability. `PreparedRuntimeInstance`, registry `prepare_runtime_device`
and chain `new_prepared` carry it without positional sidecars. Old factory methods,
`PreparedProcessorInstance` literals, registry `prepare_device` and chain `new`
remain supported. An owned-only provider explicitly refuses the legacy interface.

A `PreparedOwnedProcessorRuntime` constructs one exclusive owner on control with a
fallible factory. Its byte declaration includes the owner, both history banks,
all buffers and reserved capacity. Actual fixed storage must be known and no
larger than declared. General host accounting charges this total exactly once;
legacy factory declarations remain per twin. The chain getter is now explicitly
`declared_pair_bytes`, and engine preflight, retained histories and handoff use
that unit. Existing effective caps remain 131,072 bytes per pair and 2,097,152
bytes per bank, corresponding to the unchanged legacy per-twin constants.
Factories exceeding those limits refuse before construction.

Owned runtime begin/process/commit/rollback/reset and optional inheritance must
allocate and free nothing, block on nothing, and perform no I/O. Transactions can
span arbitrary borrowed segment lengths and multiple ring wraps. The initial
processing capability is infallible after admission, including `None` clocks;
providers needing fallible per-segment resources or clock admission need a future
explicit capability. Default bypass preserves dry samples while forwarding every
sample's shifted borrowed clock to private processing.

The fixture owns committed/scratch rings. Begin abandons any open transaction;
processing tracks the written circular interval. Commit copies that interval to
committed storage; rollback copies it back from committed storage. Each terminal
copy is bounded by `min(transaction writes, ring length)`, with at most two
contiguous copies, and cursors/quiescence counts travel with the history. There
is no write journal that silently overflows on a long transaction. Reset clears
both banks. This ring algorithm is not a universal transaction scheme for FFT,
convolution or arbitrary random-access state.

## host errors and adoption

Shared Session rendering rolls back current Track/Return chains, detached Track
chains and retained Return chains on a failed render. The legacy Arrangement
renderer also rolls back after processing starts, including compensated paths.
Preflight refusal retains its earlier output policy; a late error clears output.
A failed block never becomes committed effect history. Stopped monitoring still
owns the runtime on the callback and follows the same transaction contract.

Inheritance is an explicit provider capability, false by default. The fixture
permits only closed, matching histories within the existing small live-copy cap;
large-history copies remain refused. A stopped transport does not transfer
callback ownership to control. The current Stop/Restore/Install mechanism resets
history; it is not preserved-history custody. Full W04 still requires a bounded
quiescent ownership transfer for large state, resource-accounted admission,
production consumers and physical native publication/retirement evidence.

Native physical storage verification remains the existing Utility allowlist with
its actual legacy allocation identities. Owned runtimes expose no counterfeit
legacy box identities and cannot satisfy that proof. No native eligibility,
callback ownership budget, storage provenance rule or app installation is relaxed.

## evidence and limits

`cargo test -p hex-lattice --features conformance --locked` covers a real registered
owned factory, shared codec/effect harness, arbitrary partitions and multiple
wraps, abandoned work, drain/reset/bypass, open-transaction inheritance refusal,
factory errors and understated storage. A clock fixture checks default bypass
through loop/frame/performance offsets and explicit absent clocks.

Engine fixtures cover late failure after speculative Track/Return rendering in
compensated and uncompensated sessions, retained Return history, immediate
handoff readiness and exact retry PCM. Removing the host abort calls makes the
regression fail at immediate inheritance; restoring them passes. Stopped monitor
fixtures retain frozen transport clocks, drain histories and close transactions.
Detached Track error paths and legacy Arrangement error paths are inspected;
not every fault frontier has an injected regression fixture yet.

`cargo test -p hex-audio --features allocation-probe` includes the borrowed owned
chain's active, commit, abort, bypass, bounded inheritance and reset operations.
Construction, assertions/oracles and final destruction remain outside the scope;
require zero total and signed-net allocation counts and bytes. This does not
establish physical callback deadlines, native API allocation or large live custody.

The control-side release spike is reproducible with:

```sh
cargo run --release -p hex-lattice --features conformance --example owned_history_profile --locked
```

It directly constructs the fixture outside native admission: two seconds of
stereo history at 44.1/48/96/192 kHz, 128-frame blocks, 100 warmup and 4,000 measured
transactions, `Instant` timing. Each output equals an independent whole-copy shift
oracle. At 192 kHz the dirty copy is 1,024 bytes/block versus 6,144,000 whole-copy
bytes/block; large live inheritance is explicitly false. Retained raw results and
source identity are under `research/observations/owned-effects-2673/`. Apple M5 Pro,
arm64, macOS 26.6.2 (25G83), rustc/Cargo 1.94.0. Concurrent machine load is
uncontrolled, and the whole-copy comparator is a fixture, not a production
before/after benchmark or convolution proof.

Full final-source local CI and its unchanged source digest are recorded in PR289
and Pulse. Native/listening/reference evidence and later W04 acceptance remain
open independently of this focused bootstrap merging.
