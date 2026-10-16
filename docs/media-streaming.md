# Large-project media streaming

The planning-only [source/provider support matrix](streaming-source-support-matrix.md)
maps the full Pulse #24 scope, exact constructor/accounting gaps and remaining
native tests for #2536. Its initial audio-only class does not complete #24.

## Required outcome

Long projects with 100 active audio tracks must play, seek, loop, launch Session
sources and edit clips without retaining every complete source as resident PCM.
Current native import/open limits are 64 MiB/source, 8,000,000 source frames and
8,000,000 aggregate converted frames. Those remain enforced until a replacement
media/worker/playback architecture passes its acceptance. Raising them alone is
not streaming or a scalability solution.

Current ownership: hex-media reads complete files and decodes complete buffers;
hex-session owns full project-rate Audio objects and waveforms; compiled engine
Arrangement and Session sources share immutable resident PCM. Preserve that usable
path while adding the required streaming alternative.

## First dependency: bounded seekable source decoding

Size large, risk high across parser/API/error contracts; one coherent media writer.
Add a control-side WavFrameReader for supported classic RIFF/WAVE sources. Inspect
headers by seeking over payloads, with checked u64 offsets/lengths and explicit
file/source-frame/chunk-header/window ceilings. No full-file byte buffer and no
full-source decoded allocation. Preserve lexical PathBuf spelling and use the
same supported formats, sample scaling, mono duplication and finite-float rules
as resident import. Share format/sample decoding and header topology validation;
avoid independent drifting parsers. Public resident APIs and their limits remain
unchanged. No file I/O or decoding belongs on the callback.

Chosen API direction: WavReaderLimits separates whole-file byte/source-frame
ceilings from maximum header count and decoded-window frames. WavFrameReader owns
one read-only File, lexical path, validated header/layout and captured metadata.
Expose metadata and `read_frames_into(start, output)` using caller-owned stereo
Frame storage, bounded scratch and zero-filled suffix at EOF. `read_frames(start,
count)` is a fallibly allocated control-side convenience over the same core. At
EOF return zero frames; a start beyond EOF or overflowing requested end is an
error. A failed read zeroes the complete supplied output so partial decoded samples
are never usable. The Vec convenience returns no partial result on error.

Opening validates the entire RIFF chunk topology, duplicates, padding, supported
format and aligned/nonempty data, but does not scan every PCM sample. Nonfinite
float samples are rejected when the affected window is decoded. Metadata-only
opening must not be described as validating the complete audio payload. Classic
u32 RIFF/data size fields remain a format limit; RF64 support is required separately.

Capture the open descriptor's length and modification time, and check them before
and after reads. Path replacement does not retarget that descriptor; ordinary
in-place modifications/truncation are refused. Metadata checks are not content
identity: same-size edits with unchanged/coarsely resolved/restored timestamps can
escape detection. Full immutable cache identity/signatures and invalidation are
required before source pages participate in production playback. Do not promise
an atomic source snapshot from a mutable file or its metadata.

Acceptance: all PCM16/24/32 and float32 mono/stereo windows match the existing
whole-file decoder bit-for-bit, including random seeks and irregular partitions.
Test data-before-format, unknown/odd chunks and trailing bytes; duplicate/missing/
truncated/unpadded/unsupported/unaligned input and nonfinite samples; bounded header
scans, oversized windows, checked offsets, EOF and failure output; mutation/truncate
and descriptor/path behavior. A sparse original source above64 MiB and8M frames
must open and return exact small far-offset windows without materializing its full
payload. Keep fixtures original, bounded in physical storage and recoverably named.
Run media tests/strict Clippy and affected Session import/open regressions; inspect
actual read/allocation bounds and retained resident behavior before acceptance.

## Dependent integration, still required

1. Build bounded waveform summaries and source metadata in worker chunks; introduce
   canonical media providers while preserving asset IDs, conversion metadata,
   undo/history, save/open/relink and missing-media recovery.
2. Add incremental project-rate conversion using the existing global output-index
   phase and exact source halo. Page edges must reproduce resident conversion;
   never restart sinc phase or edge padding for each page.
3. Prepare a bounded cache and read/decode/resample scheduler with page identity,
   stale-request cancellation and immutable source verification. Account for total
   buffers, waveforms, worker concurrency, outstanding requests and retained graphs.
4. Hand prepared pages to playback through fixed bounded queues/pools. Callback
   access must allocate, wait, lock and finally destroy no objects. Define starvation,
   silence/error reporting, seek readiness and off-callback reclamation explicitly.
5. Integrate streamed sources into Arrangement/Session render, loops, handoff,
   export and project lifecycle. Prepare known playback before Play; do not hide
   disk setup/decoding or arbitrary wait time in the transport handler.
6. Stress 100 independent long sources, mixed rates, seeks, loops and launch bursts
   while editing, saving and swapping graphs. Record cache misses, first-audible
   timing, sustained callback deadlines and native input/listening. Compare Live
   only under equivalent conditions; a source reader alone proves none of this.


## Incremental conversion contract

Prepare one control-side converter from source rate/count and project rate with
explicit source/output metadata ceilings and a separate output-window cap. Keep
resident resample API behavior and identity shared PCM intact; both conversion
paths use the same v1 weights and integer global-index phase math. Freeze original
v1 fixture outputs before the refactor to prevent two newly shared paths from
agreeing on an accidental behavior change.

Query the required half-open source interval for a global output interval using
the first/last rounded centers plus the sinc radius, clipped to the whole source.
The rounded phase can carry from phase 1024 to the next source center. Missing
real halo samples are errors; only taps outside the complete source are zero.
Never restart phase or normalize edges at page boundaries. Render into caller
storage with no per-window allocation; errors zero the full output. Output-end
requests are strict rather than implicitly padded. Empty output at EOF is valid.

A control-side WAV helper can decode the queried span into caller scratch and
then convert; validate source metadata, scratch capacity and reader limits before
reading. Reader and output-window caps are distinct, especially at extreme rate
ratios. Existing kernel tables have a 13 MiB hard cap; total page memory is input
scratch plus output storage, each eight bytes per stereo frame. Sharing identical
immutable tables across multiple assets remains required before 100-track cache
integration; duplicating large tables per track is not acceptable architecture.

Acceptance includes frozen pre-refactor golden samples/digests, exact irregular
and random-access windows, global phase carry, source/halo edges, identity signed
zero, tiny/extreme-rate sources, missing halo and invalid spans, capacity/overflow
refusal and zero-on-error, and WAV-to-converted far windows. No production media
limit or callback access changes until cache/lifecycle integration passes.


## Required cross-crate integration decisions

The ownership trace confirms `Session::media` and compiled Arrangement/Session
clips retain complete Audio buffers today; successful Open eagerly converts
all assets. Existing audio Publication retirement and control-side reclamation
must also retain a streamed media-generation lease until every active, deferred,
retired and export revision is finished. Dropping a UI/service generation cannot
close the worker sources still referenced by audio.

Introduce generation-scoped source identity independent of canonical AssetId:
a new project can reuse AssetId(1) without sharing old pages. Page keys require
source generation, source identity, converter version and page index; request
cancellation additionally needs a request generation. Disk decoding needs a
separate bounded worker from serialized edit/open operations so slow reads do
not block interactive edits. Engine rendering borrows prepared project-rate page
views, with no File or media-codec dependency. Page buffers travel through fixed
request/ready/return queues; a full return queue retains buffers in bounded held
slots rather than destroying them in the callback.

Before implementing live delivery, settle and document these contracts:

- Immutable source identity beyond descriptor mtime/length, including external
  mutation, persisted metadata/schema requirements and reuse across Open.
- Fixed page/slot budgets and maximum unique active demand per block; account for
  overlapping clips, halo work, startup pins and lookahead, not just track count.
- Mandatory warmed pages before graph publication so Play needs no deliberate
  disk/compile wait. Cold seek and Session launch readiness must preserve initial
  transients and expose pending state rather than silently dropping source starts.
- Real misses produce zero plus bounded underrun telemetry and coalesced requests;
  stale pages must never substitute for the requested source/frame range.
- Shutdown ordering: stop requests/audio, drain page/publication ownership, then
  close workers and descriptors; do not rely on detached Service worker lifetime.
- Offline export retains its original generation and uses the same random-access
  converter even if UI edits or opens another project concurrently.

The first coherent integration must exercise real worker/page transport and
Arrangement PCM, followed by source-preserving loops/seek/edits/history/Open,
Session one-shots and export. Keep existing resident playback usable through the
transition; do not publish metadata-only projects that cannot yet play their
sources. Acceptance requires bounded footprint and work, warmed 100-long-track
playback, deliberate starvation without callback stalls, stale-generation
rejection, ownership retirement and audible/native measurements.


## Source acceptance — 9 October

Bounded source reading and incremental conversion are now implemented in
hex-media. The shared parser and 32 KiB scratch were inspected; 33 media tests,
strict media Clippy, full Session regressions, normal desktop build, formatting
and architecture/UI guards pass. Root independently compared 2,700 windows in
each debug/release build to the frozen original v1 implementation across nine
rate pairs, with exact stereo sample bits. Source/compiler/lockfile and log
provenance is retained in artifacts/window-conversion-acceptance-manifest.json.
These results accept source access/conversion prerequisites, not live streaming.
The next concrete dependency is sharing prepared rate-pair kernels and choosing
verified source/page identity before bounded transport/cache integration.


## Shared converter preparation — active next dependency

Expose an immutable, cloneable ResampleKernel prepared once per exact source/
project rate pair. Source plans own a shared handle and independent frame counts,
limits and requested intervals. Keep ResamplePlan::new and resident resample as
conveniences; explicit with_kernel construction derives rates from the shared
kernel to prevent mismatches. There is no global cache, lock or callback ownership
in this step. The future media worker owns bounded rate-pair reuse and counts
unique table payloads against its budget. Last shared-handle drops stay on the
control side; window conversion borrows the kernel without refcount churn.

Acceptance: 100 plans with independent counts share the same immutable table,
keep it alive after the original handle drops, and reproduce original v1 PCM.
Keep invalid source/output/window contracts and identity sharing unchanged;
identity kernels have zero weight payload. Record worst table payload, run all
media tests/Clippy and affected Session/desktop checks, then inspect ownership.
No hidden caching or production streaming claim follows from shared preparation.


## Verified source bytes — chosen implementation contract

Prepare a control-side VerifiedWavReader with 64 KiB raw-file hash pages, a
SHA-256 whole-file fingerprint and a bounded in-memory ordered digest manifest.
Use the pinned RustCrypto sha2 0.10.9 already resolved in Cargo.lock; its stable
incremental API is documented at https://docs.rs/sha2/0.10.9/sha2/ . This is a new
explicit media dependency edge, not a handwritten hash or a callback dependency.
All byte, source, header, window and manifest-capacity limits apply before
unbounded work/allocation. Cancellation is checked between scan pages and reads.
Sidecar persistence and aggregate cache/generation ownership remain dependent work.

One full scan captures a byte sequence using bounded buffers. Length/mtime checks
before/after reject ordinary concurrent changes, but this is not an atomic source
image: coordinated same-metadata mutation during scanning can yield a captured
hybrid sequence. Do not call it an immutable filesystem snapshot or adversarial
storage guarantee. The fingerprint names the captured bytes. A managed copy is
not the first choice because it duplicates large media and also cannot establish
an instantaneous source image without a stronger filesystem snapshot contract.

After hashing, parse authoritative WAV layout through a Read/Seek adapter that
verifies complete fixed raw pages before returning their bytes. Arbitrary reads,
frames and halos may cross hash-page boundaries; split them correctly. Decode
from those same verified buffers; a second file read after verification would
reintroduce a time-of-check/time-of-use bug. Invalidate loaded page scratch before
each public window operation, and verify the bounded header topology again before
reading PCM so changed header/layout pages cannot silently reinterpret audio.
Digest mismatch/changed source and cancellation have typed errors; every failed
window clears its entire destination. EOF suffix zeroing and existing reader
limits remain unchanged. Resampling uses the same global halo queries and shared
kernel. No verified bytes may be published partially on an error.

Acceptance: independently known whole-file SHA-256, supported encoding PCM bits,
hash-page/frame/halo crossings, same-size edits with restored mtime, changed
headers/layouts, descriptor replacement, truncation, cancellation, all resource
caps, EOF/overflow/nonfinite zero-on-error, sparse far-offset conversion. Run full
media checks and dependent Session/desktop acceptance. File identity, canonical
AssetId and fresh runtime/project generation remain distinct future cache keys.


## Verified reader acceptance — 9 October

VerifiedWavReader and shared-kernel verified conversion are now implemented.
All 47 media tests/strict Clippy pass. Root independently verified an original
Python-generated 24-bit stereo WAV: whole-file SHA-256, scalar decoded samples
across a raw-page boundary, exact converted halo samples, and a byte mutation
with restored mtime rejected with completely zeroed output. Debug and release
both pass. Full Session suite, normal desktop build, workspace all-targets check,
format and policy guards pass. Provenance is retained in
artifacts/verified-source-acceptance-manifest.json. No running app was replaced.

Full-file hashing is not decoded-sample validation. The reader rejects nonfinite
float values when decoded; the subsequent stream worker now scans all float
samples before catalog admission. Aggregate manifest, waveform and cache budgets
must eventually include retained history/export/retired graphs, beyond the
implemented per-runtime caps. Reader acceptance alone does not enable native
playback of long projects or discharge the complete 100-track gate.


The next actual worker/engine delivery contract is in
[page-delivery.md](page-delivery.md), including explicit source generations,
32 MiB initial PCM pool, cold-action readiness, coalesced misses and ownership
retirement. These requirements preserve the full long-project goal beyond the
accepted source primitives.

## Headless worker acceptance and canonical integration

The real `hex-stream` worker/cache now delivers verified project-rate pages to
the engine's borrowed `MediaPages` interface. Eleven focused tests cover bounded
ownership and pressured shutdown, cache/request turnover, generation refusal,
source changes, signal validation and full-cache forward progress. Root debug
and release harnesses compare actual worker pages and rendering against the
original signed24 fixture and frozen v1 conversion. A 100-active-track sparse
long-source workload passes its warmed-window PCM oracle and accounts for all
1,024 buffers at joined shutdown. Exact timings and workload limitations are in
[performance.md](performance.md); checkpoint provenance is in
`artifacts/stream-delivery-final-manifest.json`.

The canonical Arrangement compiler now resolves resident audio or verified
streamed metadata through one compile loop and preview helper. Referenced bindings match the
canonical asset and expected project-open generation; unused retained entries
remain permitted. Native Session inventory continues to use resident audio
until its source and lifecycle contracts are integrated. `StreamRuntime` owns a
joining worker and cannot move into the callback. Split control ownership from
the bounded callback page endpoint, retain catalog leases across revisions, and
prewarm mandatory pages before native publication. Persisted source identity,
sustained advancing IO, cold-action readiness and physical device stress remain
open. The resident import budget stays enforced.

Five focused compiler tests and full engine regressions/Clippy pass. Root actual
worker debug/release matches 60,452 resident PCM frames and identical MIDI
previews, with three explicit loop-boundary seeks and no warmed misses. Renderer
is linear; these seeks do not establish streamed Playback transport scheduling.
See `artifacts/stream-canonical-acceptance-manifest.json` for provenance. The
control/page ownership split is now source accepted in debug/release, including
real-worker PCM, wrong-pair recovery and allocation probes. Pages contain no
worker wake/join; destruction remains off realtime. Actual descriptors stay
worker-owned, so control must outlive referencing graphs. Dynamic generation
leases, native endpoint retirement and separate prewarm readiness remain open.
Next extend canonical PreparedRevision/Session inventory and route rendering
through the same resident/streamed source resolver. See page-delivery.md for
the ownership contract and acceptance scope.

### Real catalog ownership accepted; implementation paused — 9 October

StreamControl and callback pages now retain the actual shared catalog owner.
Ready catalogs provide opaque CatalogLease handles with verified sorted full
metadata membership; fingerprints/generations must match and separate runtime
identities remain distinct. Control Drop no longer cancels a worker retained by
pages/leases. Matching shutdown refuses outstanding leases before mutation and
returns both halves intact; dropping leases permits retry and exact pool
accounting. Final owner release stops/wakes/joins off rendering.

Debug/release stream suites pass nine unit, one allocation integration and nine
worker integration tests. Strict Clippy, workspace formatting and architecture/
UI style guards pass. Independent debug/release checks match 110,758 Session PCM
frames, preserve cache/stats through Busy recovery, reclaim eight buffers, reject
wrong full metadata and distinguish identical-key catalogs; an uncached real page
read succeeds after controller Drop. Private test observation confirms final lease
release joins on the named control reclamation thread after endpoint/controller
release. Provenance: `artifacts/stream-catalog-lease-acceptance-manifest.json`.

This is headless ownership acceptance. Native graph/export lease attachment,
callback endpoint retirement, device-fault teardown and non-UI final reclamation
remain required; prewarm installed-page acknowledgment and sustained native
streaming are also open. Ordinary endpoint/lease final Drop is not realtime-safe.
The user requested a pause after this unit to redirect work toward quicker
practical usefulness. No further implementation or prewarm/native admission
work was started; preserve the full roadmap for that discussion.
