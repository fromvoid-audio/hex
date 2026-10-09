# Transactional stateful lattice effects

Pulse 16 owns this effect-runtime change; Utility 18 consumes it. Instruments,
Operator and external plugin adapters require separate contracts. This implements
original DC removal and bass-mono processing, not verified Ableton DSP parity.

## Definition and processing boundary

The registered definition optionally prepares a stateful processor using the
project sample rate. Immutable prepared configuration reports a runtime version
and bounded runtime-byte declaration, and creates audio-owned runtime state on
control. The processor receives stereo blocks, resets, and explicitly checks and
copies compatible history without allocation. Device codecs/descriptors remain
owned by their definitions; generic project/storage state needs no device DTO.

Linear memoryless chains retain their fused matrix fast path. Stateful chains
process each track's summed source signal once, in serial order, before track
gain/balance/mute and master mixing. Bypassed processors advance their histories
while preserving the signal. Arrangement, Session and offline paths use the same
registered processing definitions. Runtime objects remain outside canonical
project/history snapshots.

Each chain has preconstructed primary/scratch processor twins. Block rendering
copies committed history into scratch, processes scratch, and commits only after
the entire containing block succeeds. A failed Session block silences output
without advancing clocks, routes, releases or committed histories. Retrying the
same block agrees with an independently advanced reference runtime.

The legacy Arrangement fallback owns a persistent `RendererRuntime` prepared on
control; it never constructs a renderer/device chain per callback span. Session
playback owns its own runtime and does not construct an unused duplicate fallback.
Direct offline `Renderer` constructs its owner once before the render loop.

## Revisions, transport and retirement

Compatible history uses stable track/device IDs, device type, runtime version,
sample rate and the processor's checked compatibility. Parameter changes can
preserve physical histories; sample-rate/type/incompatible changes initialize
fresh state. Seek, PlayFrom and Stop reset histories; Pause freezes them. Loop
wrap and ordinary Session launch preserve defined continuity.

Same-revision releases join the current track chain. Captured outgoing source
releases retain their old chain/settings/mix in fixed tail slots. Handoff
preflights every required owner/slot before mutation; refusal preserves the old
graph and sound for a bounded retry. Only groups referenced by active copied
releases consume transferred tail capacity. Completed owners stay parked until
control-side graph retirement; callbacks never perform their final destruction.

Initial admission caps are 256 active stateful processors, 256 captured-tail
processors, 256 tail groups and 4096 declared runtime bytes per processor twin.
The active-plus-tail paired-history ceiling is 4 MiB; bounded chain/graph metadata,
source media, prepared configuration and scheduler buffers are separate. These
caps are not CPU deadline guarantees or a claim that 100 tracks with 16 stateful
devices each are supported. Raising them requires measured work/memory evidence.

## Utility state

Utility's device-owned version 2 payload adds DC removal, bass mono, its cutoff
and bass audition. Version 1 migrates to neutral filter defaults; generic storage
schema 14 remains unchanged. Cutoff is validated at 50–500 Hz. Existing typed
parameter edits, gestures, Undo and persistence use the codec/descriptor contract.
Known invalid state is refused; no silent reset or bypass substitutes for it.

## Evidence and remaining acceptance

Affected lattice/contract/devices/engine/audio/export tests, strict Clippy,
formatting and architecture checks passed before integration. Tests cover a
second independently registered nonlinear processor, serial order, summed-track
oracle, compatible/incompatible histories, block partitioning, Utility migration
and Undo, offline/Session PCM, rollback, stop/seek/pause/loop, tail saturation and
reuse. Root additionally removed unused duplicate fallback owners before full CI.

Allocation probes verify zero total allocations and zero signed-net count/bytes
for stateful processing, successful handoffs, tail completion and legacy fallback
loop/seek/stop/replacement. Control reclamation measures zero allocations with
expected negative signed-net count/bytes. The full tail-bank refusal is tested
for engine state/PCM atomicity; a separate allocation count for that exact refusal
through the public audio fixture remains open. Native APIs, physical deadlines
and every possible fault/density path are not proven by these fixtures.

One local release microbenchmark on Apple M5 Pro arm64 used 48 kHz, 128-frame
blocks, 100 tracks, one looping 128-frame resident clip and one Utility per track,
100 warmups and 2000 timed renders. Memoryless p50/p95/p99/max were
19/30/75/778 microseconds; DC-enabled stateful processing was
123/299/777/2999 microseconds. The maximum exceeded the 2667-microsecond block
deadline. The temporary benchmark example was removed, so reproducible sustained
profiling and admission scalability remain Pulse 2108; this is not native deadline
or broad performance acceptance.

Full clean local CI follows integration. User listening checks should compare
DC removal, bass mono/cutoff/audition, live parameter changes, bypass, seek/loop,
Stop/restart, save/reopen and Undo. Reference transfer-law/filter response parity,
instrument hosting and larger device workloads remain open.
