# gain touch recording

Pulse28 / PR184 records the native track fader through delivered audio receipts.
The canonical lane contains an unchanged base curve, disjoint half-open touch
spans, and a durable recording-epoch high-water mark. Track snapshots, storage,
Undo and Redo share this value; there is no second recording project model.

A span owns `[start, end)`. Its first step holds from start; later steps replace
the held value. At end, independently evaluated base automation or static gain
owns the fader again. Base ramps outside the span are never flattened or
reanchored. Preparation maps beats to nearest frames, with later beats winning
coincident frames. A collapsed span owns no audible interval. Arrangement,
Session and linear export use the same prepared envelope law. The preview
visits effective segments while retaining base points as selectable metadata.

Overlapping replacement preserves left/right fragments, original event/context
identity and origin beat; a right fragment carries its previously held event.
The expected lane epoch must match and new recording epochs increase. Undo and
Redo preserve an existing owner's epoch high-water mark. A completed take,
including multiple loop passes, is one atomic history edit. Later passes replace
earlier spans over the same content. Failed normalization or preparation leaves
the original project and history intact.

Bounds are 128 spans per track, 1022 steps per span, and 1024 weighted units per
track: base points plus retained steps plus two endpoints per span. The project
limit is 65536 units. Normalization counts fragments and endpoints before
reserving candidates. Editing base points or time transforms on recorded spans
is explicitly refused; clear removes the complete lane while retaining its
epoch and restores manual ownership.

## native command and receipt boundary

The gain pane's record control prepares one transient raw-fader capability for
its TrackId, then starts playback. Arming does not create saved points, change
history or replace the canonical preparation cache. The shared numeric touch
control sends an absolute gain in finite linear0..2. Release returns to read;
cancel discards the entire uncommitted take.

A target binds project generation, canonical/audio revisions, stable TrackId,
recording/transport epochs, gesture serial and sample rate. Audio validates the
active graph and transport before applying commands. Each callback quantum
consumes a finite prefix; only its latest value is applied. Superseded commands
return credits without inventing applied samples. Two capacity66 rings admit at
most64 outstanding ordinary events and reserve terminal capacity independently.
A take accepts at most1022 value events. Backpressure retries the latest UI value;
a full take releases rather than overwriting or silently dropping recorded data.

Audio applies the override after devices, before sends and Main, preserving
balance and mute even when saved static gain is zero. A receipt records its
actual performance/content frame and loop pass. The native adapter confirms
only after conversion and writing the output quantum, at most256 frames. A
compensated receipt waits until that fader sample reaches Main. Rendering alone
is not a delivery acknowledgment. Prior successful quantums remain delivered if
a later quantum fails; any failure discards the complete uncommitted take.
Receipts do not establish physical interface or loudspeaker delivery.

Stop, Seek, Pause, graph/source changes, device failure and explicit cancellation
abort pending takes. Priority Stop remains independent of full command queues.
Device loss can stop callbacks entirely, so the UI discards its uncommitted state
and releases its edit fence without waiting indefinitely for a terminal receipt.
Recovery replaces the output through the existing lifecycle. Late old commands
must drain their receipt credits before another take is admitted.

Control reconstructs bounded loop pieces from actual content/performance history,
checks sample/beat round trips and submits one revision-stamped splice batch.
The worker rechecks generation, canonical revision and lane epoch before history
or graph publication. Project edits remain fenced during arming/touch/release.
Window close and quit visibly require finishing or cancelling a live take.

## persistence and verification

Schema24 writes complete routing, gain points, spans, epoch, device host gains,
opaque payloads and provenance. Strict schemas22/23 can read nonempty spans;
earlier formats migrate empty spans and refuse unsupported backports rather
than discard data. Historical main/feature schema18/19 shapes retain strict
key-presence dispatch. The existing document ceiling and recoverable file
publication remain unchanged.

Deterministic tests cover envelope/sample boundaries, normalization, loop passes,
single-step Undo/Redo, save/load, staleness, bounded command/receipt pressure,
failed delivery, cancellation and priority Stop. Headless allocation probes cover
selected native adapter scopes. Native interaction/listening and physical device
deadlines remain separate evidence; optional tests use the exact optimized build.
General device-parameter recording and sidechain buses are outside this track-gain
contract. See [compensation](latency-compensation.md).
