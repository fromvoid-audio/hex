# eq eight band audition kernel and host

Pulse2292 adds a transient DSP primitive for monitoring one enabled EQ band in
isolation. The selection is an ephemeral `(band, edit lane)` value with checked
ranges; it is not saved in native device state, project schema, or parameter
descriptors, and no UI control is exposed by this change. Selecting a disabled
band is refused. The prepared runtime retains the authored enabled flags
separately from coefficient identity, so an enabled neutral-gain band remains
auditionable regardless of whether its prepared filter is mathematically neutral.

Each frame continues to run the complete normal EQ cascade with its existing
coefficient and output-gain smoothing. A second fixed 128-byte history processes
only the selected band in parallel from the pre-EQ stereo or channel-mode input.
In stereo mode the selected band processes both channels. In left/right and
mid/side modes the selected edit lane is processed and the other lane remains
dry before the mid/side pair is converted back to stereo. The latter is Hex's
explicit audition interpretation, not a measured Ableton Live behavior.

The normal and audition outputs crossfade over 256 frames. Releasing audition
fades back to the always-running normal cascade. Switching bands or edit lanes
fades the current audition down, clears the audition history at the switch, then
fades the new selection up. A selection change can therefore take at most 512
frames from a fully audible old band to a fully audible new band. Reset clears
selection, pending selection, mix and parallel history. Same-rate, same-mode
compatible coefficient handoff copies the normal and audition histories and
crossfade state together. A mode change or
a target that disables either active or pending selection cancels audition and
clears its history; the new normal EQ remains publishable. A fresh runtime starts
with no audition unless history is explicitly inherited.

The tests compare each band shape against an independent selected-band prepared
runtime, verify both stereo lanes and left/right and mid/side edit lanes, confirm
other enabled bands are excluded, exercise neutral-enabled and disabled bands,
and check reset, mode/enable handoff, partitioned crossfade and coefficient
handoff. This validates the fixed standalone kernel. It does not add graph or
project control plumbing, runtime callback allocation instrumentation, user
access or UI, Live behavior claims or audible host acceptance; those remain separate
integration and acceptance work. Registered standalone allocation and release
measurements are recorded below; they do not establish callback lease behavior.

## local kernel evidence (11 october)

The unchanged DSP implementation at commit
`410e99069a5387cd76dc5c28d1c8aa7222b4f411` was compared with main
`140ef2b6efc589c6024171507a9104121da781b5`. A local ignored Rust fixture uses
`EQ8_DEFINITION` through `NativeDeviceDefinition::prepare_processor`, constructs
and clones the registered concrete runtime outside measurement, then measures
processing, checked selection changes, compatible history transfers and reset
with pinned allocation-counter 0.8.1. Total and signed-net allocation counts and
bytes were all zero. This is a standalone registered kernel probe, not a CPAL
callback or the future host audition lease.

On Apple M5 Pro, macOS 26.6.2, rustc 1.98.1, nine optimized runs interleaved three
old-normal/new-normal/new-audition rounds at 48 khz/128 frames, eight enabled
bands, 100 warm-up and 2000 measured blocks. Runtime size increased from 3632 to
3792 bytes, retaining the 4096-byte per-twin limit. Old normal p50 ranged
21.54–21.67 μs, new normal 21.63–21.67 μs, and audition 23.54 μs; normal-output
checksums matched across versions and repeats. Raw p95/max timings are retained,
including scheduling outliers; these figures do not establish physical device
deadlines or Live parity. No Cargo or other profile ran during the timed window
2026-10-10 22:05:16–22:05:17 UTC; OS scheduling was uncontrolled.

Ignored evidence `artifacts/profiles/eq8-audition-2292/` retains the fixture,
manifest/lock, both executables, per-run JSON, allocation probe and `summary.json`
with source, hardware and binary hashes. Reproduce the standalone fixture with
`cargo build --release --offline --features audition --manifest-path
artifacts/profiles/eq8-audition-2292/fixture/Cargo.toml`, then run its executable
with `probe`, `normal` or `solo`. The retained baseline executable was built
without the audition feature before applying this change. Source unit tests cover
all shapes/lanes, non-neutral output gain, whole-versus-partitioned release,
latest-request supersession, reset and compatible/incompatible handoff.

## transient host contract (pulse2253)

The host integration builds on the kernel above. A headphone button uses the
shared compact Lattice toggle widget and arms a UI-local mode; arming alone never
changes sound. Physical pointer press on an enabled filter dot requests audition
before the parameter-edit drag threshold. Holding still creates no authored edit;
dragging retains the normal frequency/gain/Q editing gesture. Keyboard selection
never starts an audition. Inside release fades to the normal cascade; outside
release, focus loss, disarm and device/panel/project lifecycle changes hard-cancel.
Disabled bands and globally bypassed devices refuse activation rather than
changing their saved enable flags. LR/MS lane behavior remains the explicit Hex
interpretation above, pending reference listening evidence.

`hex-lattice` owns a device-neutral optional `NativeProcessorAudition` capability;
its default is absent. A checked selector, requested-selection inspection, smooth
release and immediate transient cancellation form the contract. Both transactional
processor twins must accept the exact owner/device/type/selector before either is
mutated. EQ8 implements the capability; unrelated devices need no storage branch,
new authored parameter or special host construction path.

`hex-audio` owns a separate bounded target lease, not an analyzer-data command.
The target binds the typed Track/Return identity, project control-context generation,
actual publication activation floor, sample rate and selector. An eight-command
SPSC ring drains at most eight changes per block; a coherent atomic intent and
hard-cancel watermark invalidate old Begin requests even when the ring is full.
Only the latest pending command retries. Epoch exhaustion is terminal and fails
closed. Priority Stop retains its shared-reference API and atomically fences
already-admitted requests without needing ring space.

Activation checks the actual audio Publication generation and rate as well as
its revision floor. Waiting for a future publication is distinct from refusing a
removed, wrong-type, bypassed or disabled target. Begin applies once; compatible
parameter publications inherit the requested selection and filter histories.
A reset or incompatible mode/enable change clears the request and latches the
rejected epoch, so continuing to hold cannot reactivate it. A fresh physical press
can request a new epoch. Audition does not start transport or route a hardware cue.

Only current Arrangement/Session track and Return chains resolve Begin commands.
When an instrument's old note-release owner retains its effect suffix, ordinary
filter history stays exclusively with that owner. A held audition may move to
the candidate's matching current entry through a separate transient-only bridge:
both old twins must agree, both candidate twins must accept the selector and
certify parameter compatibility, and the actual publication context must match.
The candidate starts a fresh audition history and fade; it does not copy the
ordinary suffix history or replay Begin every block.
Transferred residual track/return tails are hard-cleared after a successful,
no-longer-fallible graph handoff and before the candidate's first output. Refused
or deferred candidates leave the active graph untouched. Clearing transient state
never resets the ordinary EQ histories. Queue objects, graphs and assets retain
the existing control-side reclamation ownership.

This contract is source integration, not proof of Ableton parity or physical-device
performance. Native hold/drag/release, focus and listening acceptance remain open
in Pulse2253. Automated callback and release-profile evidence is recorded below, separately
from that acceptance.

## local host evidence (11 october)

Source `3e13a01ef1361c6249ee59ccb6826e047f3c96c3` includes the bounded
lease, native headphone/physical-press plumbing and transient release-tail bridge.
Focused Lattice, device, engine, audio and response-graph tests pass. Repeated
Track callback publications preserve the held selector; engine Return gain/Q/type
publications preserve it without duplicating the current Return chain. A retiring
mutable-instrument suffix retains its independently checked ordinary EQ PCM after
hard-cancellation; an older same-track tail cannot mask the current held owner.
The pointer-up geometry regression distinguishes actual graph bounds from clamped
drag coordinates. Physical outside release is separate from the authored Commit,
so shared Operator envelope drags retain their accepted outside-release behavior.
The actual UI-feature outside-envelope reproducer passes with this separation. No native interaction or listening test was performed.

The actual Track callback allocation fixture exercises Begin, repeated parameter
publications, instrument-release-tail transfer, smooth release, hard cancellation
and shutdown. Measured scopes have zero total allocations and zero signed-net
counts/bytes. Separate source tests cover priority Stop, seek, render faults,
publication generation/rate/floor fences, queue saturation, idle boundaries and
both-twin refusal. These do not establish that every fault or native API path is
allocation-free. Return audition continuity is engine-tested; a separate Return
audition callback allocation probe remains unmeasured. PR125 separately measures
non-audition Return parameter-history handoff.

The checked-in `hex-audio` example `profile_eq8_audition` times actual prepared
callback Begin/render/spectrum-finish calls. It renders an original held sine
note through two non-neutral EQ bands, compares every block exactly with a
separate Playback oracle outside timing, and checks that held audition differs
from normal output. Release and cancellation occur halfway through warm-up;
all measured post-warm blocks equal an independently advanced normal oracle.
Construction, control reclamation, checksums and destruction are outside timing.

Three runs each at 48 khz and 128/256 frames use 200 warm-up and 4000 measured
blocks on Apple M5 Pro/macOS 26.6.2/rustc 1.98.1. Normal/released/cancelled p50
ranges are 5.042–5.083 μs at 128 frames and 9.083–9.125 μs at 256 frames; held
audition is 6.459–6.500 μs and 11.917–12.000 μs respectively. These are short,
unpaced headless runs, not physical device deadlines or reference listening.
No other Hex Cargo/profile ran during the timed window; an external Yield app
was active and OS scheduling was uncontrolled. Raw p95/max outliers are retained.

`artifacts/profiles/eq8-audition-host-2253/3e13a01/` retains three executables,
18 raw logs, 60 scenario rows, source/tree/hardware/process metadata and SHA256
hashes in `summary.json`. The separate one-band spectrum comparison retains
exact old/new PCM across all scenarios; its older baseline includes fewer
prerequisites and is not an isolated measurement of just this lease. See
[performance](performance.md#eq8-transient-audition-host--11-october-2026).
Full final-tree local CI and final diff review remain the PR merge gates.
