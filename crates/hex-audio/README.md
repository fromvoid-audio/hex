# hex-audio

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

The staged service integration also provides `WorkerPreparedOutput`: its native
factory reserves paired client/publication terminal destinations before CPAL
exposure. Configure its virgin Root before `take_client()`; the returned move-only
`prepared::Client` has bounded scalar/command methods and borrowed observations.
Closing it transfers the complete endpoint bundle for lifecycle destruction and
leaves an inert token. Worker publication and graph cleanup can finish while the
client remains live; final native resource/lease release waits for its actual
endpoint Drop. Session/desktop integration of this factory is still in progress;
see [source/service integration](../../docs/streamed-session-source-plan.md).

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

## Browser sample preview

`prepared::Control::sample_preview` accepts finite, output-rate `Audio` prepared
on a background worker. `request` silences older generations before decoding;
`publish` coalesces a latest pending owner behind the one-slot input queue.
The existing callback adds the one-shot at unity after project master output,
including while project transport is stopped or paused. Project Stop is separate.
Two return slots carry exhausted/stale PCM to the ordinary control reclamation
poll; full return storage retains ownership and stale PCM stays silent. Shutdown
waits for preview owners too. No extra stream, callback allocation, locking or
final PCM drop is needed. The media decoder and browser input remain outside
this crate. See [runtime](../../docs/runtime.md#browser-sample-playback--pulse2637--pr254).

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

The narrow `hex-media` dev dependency opens real verified WAV fixtures for actual
catalog/source association tests. Production media decoding remains in
`hex-stream`; audio gains no media/file I/O dependency or callback decoding.

hex-engine; hex-stream for exact catalog/pair custody and control-side stream
disposal; CPAL for CoreAudio output; rtrb for SPSC commands. This narrow production
stream edge supplies one sealed exact-domain endpoint to the opted-in constructor
renderer under its real readiness transaction. Legacy callers remain unchanged.
Tests use
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


## Delivered gain recording

`gain_recording` owns bounded value/receipt rings:64 outstanding ordinary events,
independent terminal reservation, and1022 events per take. Targets bind stable
track and project/audio/transport/gesture identities. Callback consumption applies
only the latest boundary value; superseded events release credits without an
applied receipt. `finish_gain_recording_quantum` runs after native conversion and
write, deferring acknowledgments until the compensated sample reaches Main.
Failed output, transport reset and cancellation abort the uncommitted take.
Priority Stop stays independent of queue saturation. No record or receipt owns a
graph, allocates, blocks or drops final owners in the callback. Take normalization
and saved history belong to session/project. See
[gain recording](../../docs/gain-touch-spans.md) for quantum and physical-delivery
limits and selected headless allocation coverage.

## never-installed candidate custody

`Control::stage_audio_candidate` accepts an opaque constructor-owned candidate
only with the real native terminal receiver already anchored. The existing queue
returns this admission-pending variant before installation; legacy publication
behavior is unchanged. Persistent slots retain root/runtime identities, actual
catalog leases/pairs and original-bank recovery until normal lifecycle-worker
destruction. UI return ingestion/polling cannot invoke the new worker join or
candidate destructors; READY recovery moves the already-restored bank only.
Coupled callback guards move their exact Boxes to control after both final
releases; build errors after backend exposure remain uncertain. No complete-cost,
native admission or final-active quiescence claim follows. See
[never-installed disposal](../../docs/never-installed-audio-disposal.md).

The initial PR235 final-active constructor path retains the same private
owner/Stamp in the single active-slot enum. Only coupled final native release
plus the exact completed stream-handle frontier can return it for control
disposal. That disposal-only path still refuses production admission; its
headless cfg(test) override exercises actual playback. PR236's separately
opted-in eligible constructor and PR237 readiness supply the bounded native path. A final result follows normal runtime,
carrier, catalog and pool cleanup, never pause/ACK alone or OS-stop inference.
See [final-active disposal](../../docs/final-active-audio-disposal.md) for the
closed audio-only ownership class, retained Unknowns and evidence limits.


## exact streamed chunk readiness

The pre-first-accept streaming opt-in reserves finite actual per-runtime demand
backing. Same-owner/context/predecessor/command-prefix readiness gates initial
and replacement installs and every streamed chunk. Unready chunks BUFFER without
clock/DSP advance, retaining prior committed PCM; real Pause/priority Stop pass
pending Play. Both normal and observed renders share protected-page transactions.
Mutable native runtime metadata reduces the existing 32 MiB runtime allowance as
well as caller DATA budgets. Seek/launch and actual UI composition remain deferred.
See [native streamed readiness](../../docs/streamed-native-readiness.md).
