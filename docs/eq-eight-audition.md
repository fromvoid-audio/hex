# eq eight band audition kernel

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
