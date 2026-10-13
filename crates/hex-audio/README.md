# hex-audio

The opt-in streaming bridge prepares movable shared-context handles on control.
Its retiring handoff consumes the predecessor's existing playback box and sends
an opaque, nonrevivable `RetiredPlayback` through the same bounded return path.
Candidate-only staging may consume a refused candidate; that candidate must be
rebuilt on control. A refusal before owner commit returns the usable predecessor.
Validated context and Return-tail guards hold exclusive borrows until their
infallible moves. Actual reserve capacities and the tagged return-ring entries
are included in existing storage reporting; existing ceilings are retained.
Resident borrowed handoff APIs remain available through the legacy path.

## Purpose

Own device output, stream lifetime, bounded real-time control delivery, and backend telemetry.

The desktop's nonblocking initial open uses `request_initial_open()` and polls
its one-result handle; the lifecycle worker uses the device's actual default
rate without assuming a project rate. Recovery uses
`request_recovery_open(expected_rate)` and checks the default device rate before
constructing a stream. `PreparedOutput::open` remains available as a synchronous
startup path for existing callers.
Only one recovery open may be pending. Its reservation remains held until the
result is consumed, or until an abandoned late output and any retained stream
lease have retired. Device setup and stream disposal stay off the audio callback
and caller thread.

`prepared::Control` publishes a fixed-size `Playback` wrapper through a one-slot
queue and keeps publication admission closed until the callback accepts or
returns that candidate. Session route commands share the bounded command queue
with transport actions; failed actions have a separate ticket/reason snapshot.
Priority Stop uses a ticket marker independent of queue capacity: at callback
entry it stops immediately, discards queued commands admitted through that
marker, and applies later commands from the captured queue prefix in FIFO order.
Commands admitted after the callback's entry cut remain for the next block;
ordinary actions are refused before ticket wraparound. The callback examines at
most the fixed 32-command queue and never waits for control admission.
The callback retries transient Session handoff capacity refusals while the old
graph continues playing, and permanently rejected candidates return through the
retirement queue. Read rejection tuples through `Status` snapshot methods so the
reason and ticket/revision remain paired. Session capability status is reported
separately from render faults. Graph construction and final destruction remain
off the callback.

`Control::capture_fresh_runtime_fence` returns a control-side snapshot of the
coherent Session route header followed by an Acquire read of the admitted-command
high-water mark. A caller may publish a separately prepared candidate with
`InstallFreshRuntimeIfUnchanged(fence)`. Immediately before transfer, the callback
compares the fence with its actual active revision (zero when empty), active route
generation, current admitted ticket, and pending priority Stop marker. A mismatch
rejects and returns the candidate, reports `RuntimeFenceChanged`, and releases
the publication gate. The callback does not clear or consume Stop on rejection;
the ordinary command cut applies Stop to the previous graph. Commands admitted
after validation linearize after installation and are consumed by the new graph.
If retirement storage is full, the old graph continues rendering/consuming bounded
controls; the fence is checked only after capacity returns. Unconditional fresh
installation and inherited-runtime publication retain their existing policies.
The fence is audio-runtime freshness only; it does not establish caller project
identity, compilation provenance, or active UI/session-route admission.

`Status::session_routes` copies one coherent route snapshot into caller-owned
storage with at most three reader attempts. A short buffer or concurrent writer
returns `None`; callers must ignore the whole output buffer on `None`. Allocate
at least `SESSION_ROUTE_CAPACITY` entries for complete snapshots. The fixed
256-slot atomic table is allocated with the channel. Each accepted graph revision
and committed route-generation change writes a complete snapshot, including
clearing unused slots; unchanged render blocks compare only the revision and
opaque engine generation. The snapshot's `performance_at_update` is its cut
frame, not a live cursor. Read `Status::performance_frame` for current transport.
Legacy and unavailable Session capability snapshots have zero route entries and
retain their explicit capability status. After shutdown retires the active graph,
the route snapshot publishes revision `0`, zero entries and Legacy capability;
`applied_revision` remains the separate accepted-revision high-water mark.

`Control::launch_and_play_with_ticket` queues one atomic Session action. Only
`LaunchClip` and `LaunchScene` payloads are accepted; transport starts after the
route command succeeds. An invalid target or non-launch command leaves the
current transport state unchanged and reports through the ordinary action
rejection snapshot.

The development-only `profile_session_routes` example constructs canonical
100/256-track fixtures using the existing dev dependency. Run with
`cargo run --release -p hex-audio --example profile_session_routes -- 100`.
It compares callback PCM/routes to the headless kernel and times changed and
unchanged control entries plus snapshot reads. Render/validation checks stay
outside timing; no CPAL stream is opened. See docs/performance.md for methodology,
retained outliers and the separate physical deadline gate.

## Output spectrum analysis

`spectrum` provides the fixed 4096-frame stereo FFT kernel; `spectrum_worker`
provides a bounded, off-callback consumer for already-rendered stereo output.
Create both sides on a control thread with `spawn_spectrum_worker`, move only
`SpectrumCapture` into a future callback tap, and submit finite batches of 1–256
frames. The capture queue holds 32 batches. A full queue or invalid batch drops
only analysis data and consumes its sequence, so the next accepted batch exposes
the discontinuity. Unsupported sample rates are refused at submission and also
consume a sequence. Sequence numbers never wrap; exhaustion requires a new
capture producer and generation. Callback submission copies into inline storage
and performs one nonblocking SPSC push; it does not wake the worker.

The worker drains at most 32 batches per poll, assembles a fixed 4096-frame
rolling window, and publishes no faster than 30 Hz through a one-result bounded
channel. Target, capture generation, sample-rate changes and sequence gaps clear
the rolling window. Ordinary applied-revision changes keep contiguous output
samples and update the published revision. Sample-rate changes reconstruct the
kernel on the worker. Results expire after 100 ms without accepted capture. The
worker thread owns FFT construction/reconstruction and analysis. The prepared
CPAL callback only stages the selected EQ8 post-node output into fixed storage
and submits bounded batches; it performs no FFT.

The callback owns the capture producer. After a successful stream pause, the
lifecycle owner drops the stream/callback first, joins the worker reaper second,
and releases the exclusive output lease last. Failed pauses retain the stream,
worker and lease. Dropping the reaper only requests worker exit; it does not join.
See [spectrum analysis](../../docs/spectrum-analysis.md) for the numeric
contract, post-node selection and limits.

Run `cargo run -p hex-audio --release --example profile_eq8_spectrum -- 128 4000`
to compare prepared callback timing with no worker, an attached inactive worker,
and the EQ8 tap enabled. The example checks matching PCM against an independent
prepared renderer and reports per-scenario percentiles and checksums. It is a
headless microbenchmark, not a physical-device deadline or native UI test.

## Dependencies

hex-engine; CPAL for CoreAudio output; rtrb for SPSC commands. Tests use
hex-project as a dev-dependency to build canonical prepared revisions for the
callback boundary. The non-default `allocation-probe` feature also activates
hex-engine's hidden `instrument-test-support` preparation seam and uses
hex-lattice as a dev-dependency. This injects a fixed-storage mutable proof
instrument into the real prepared callback path; neither dependency edge exists
in production audio.

## Does not own

GPUI views, project editing/history, file parsing, or musical device DSP algorithms.

Read ../../AGENTS.md and ../../docs/architecture.md before extending this crate.

## Opt-in allocation probes

`cargo test -p hex-audio --features allocation-probe` enables headless integration
probes over the real prepared callback API. The pinned dev-only allocation-counter
0.8.1 supplies a per-thread allocator counter; Hex uses its safe API and adds no
unsafe code. Production never references this dependency. The architecture policy
rejects it outside audio dev dependencies. Sensitivity tests must detect both
allocate/free pairs and freeing objects allocated before a measured scope, so
callback checks require zero total allocation count/bytes and zero signed net
count/bytes. Construction, control admission, independent PCM oracles and control
reclamation remain outside callback scopes. This is allocator evidence, not timing,
other-thread/native-API allocation, locking, CPAL or physical deadline acceptance.

## Observational spectrum kernel

`spectrum::SpectrumKernel` owns reusable fixed-window stereo analysis for
backend telemetry. This observational FFT is separate from musical device DSP
in `hex-devices`; construction and analysis belong on control/worker threads.
It does not capture audio, own a rolling stream window, schedule a worker or
render a UI. See [spectrum analysis](../../docs/spectrum-analysis.md).

## Opt-in streaming bridge

Production audio has a narrow, acyclic dependency on `hex-stream` for verified
catalog/page ownership, installed readiness and control reclamation. Stream has
no audio/CPAL/GPUI dependency. `request_streaming_open` creates the explicit bridge;
ordinary opens remain resident. `PreparedOutput::publish_streamed` takes the
complete owning request; synchronous errors retain it and expose retryability.
Use the actual `streaming_device_epoch` and exclusive
`input_bridge_storage_bytes` for preparation. Use `stream_catalog_lease` only on
control for same-context successors; discard abandoned loans before retirement.
Taking actual `take_stream_pool_report` outcomes frees reusable slots. Partial
`stream_pool_reports` and cleanup phase remain observable without fabricating a
successful join. Getter contention is unavailable data, not readiness.

The existing lifecycle worker owns native graph reclamation/joins. Successful
pause precedes callback closure disposal; media cleanup precedes device-lease
release. Source faults/incomplete pools retain roots and prevent reopen. Read
`docs/page-delivery.md` for BUFFERING semantics, N=2 refusal, exact-demand tokens,
controller handoff and remaining full native acceptance. Source/harness checks
cannot prove CoreAudio, listening, deadlines or sustained native100-track behavior.
