# canonical verified source plan

Pulse2544 / PR239 connects the existing Session worker to the native streamed
publication path. The first source/service checkpoint supplies the neutral
preparation described here; desktop composition, nonowning UI clients and native
publication remain unfinished until the rest of that PR is implemented.

`DocumentAudioMode` is selected explicitly for a fresh New/Open before media
preparation. Resident remains the default. `VerifiedStreaming` imports and opens
use `VerifiedWavReader` to verify bounded headers, source format, complete file
fingerprint and page manifests without decoding/resampling full PCM. Playback
frame counts use the canonical checked conversion function. No resident frame
charge is fabricated and the resident duration ceiling is unchanged. Each source
still obeys 64 MiB / 8,000,000 source-frame ceilings. Caller reservations also
bound retained source count, actual path capacity, verification manifest capacity,
header count and read window. Retained history consumes those reservations even
when Undo removes all current clips. Verification/readiness failure never adopts
a partial canonical edit; valid canonical edits do not certify native admission.

Verified source facts contain an immutable resolved path, fingerprint, source and
project rates, source/playback frame counts, conversion version and reader bounds.
They contain no reader, descriptor, page endpoint, catalog or native lease.
Private shared immutable payloads keep Session/history, temporary semantic banks
and read-only snapshots from duplicating verified path backing. Source-count
bounds cover payload count; each plan's bounded map carries its own handles.
The reservation is source metadata, not a process RSS or allocator estimate.
Verification manifest sizes are retained as conservative reservation charges even
though the temporary reader/manifest is dropped before canonical commit.

Relative media resolve against the opened document's retained base; successful
import/open captures that resolved binding. Save As changes the document handle,
not source identity or its base. Ordinary edits and Undo/Redo preserve captured
facts. Reopen/relink verifies a fresh plan rather than silently refreshing bytes.
A checked private semantic source epoch changes for a fresh verified document or
verified binding-set addition. Ordinary edits/history reuse the same epoch.
Semantic keys never grant native catalog identity: the native facade must match
complete facts against its actual retained domain and independently reverify the
expected fingerprint through the worker.

The existing canonical snapshot pairs the exact Project, immutable source plan
and actual worker document generation/editor revision. Source catalog generation,
semantic epoch and document generation are separate. Preview views cannot mint a
canonical snapshot. Semantic compilation uses the existing mixed-source prepared
revision compiler for edit/import/history transactions; it is display/control
preparation, not native constructor custody. Verified-mode `Event.playback` is
absent, including empty and source-free revisions, so it cannot fall back to the
legacy publication lane. The later desktop facade must use fresh consuming
constructor banks and the existing sealed Root/domain/readiness APIs.

Waveform summaries are unavailable for verified-only sources at this checkpoint;
no zero-filled peaks pretend to be measured audio. Resident waveforms remain as
before. Offline export and gain recording explicitly refuse verified-mode
sources. Streamed offline export is deferred to Pulse2650 / PR266; it must not
run the current prepared NoPages renderer. No device eligibility, ownership,
readiness, physical deadline or native UI/listening acceptance is established by
this source plan. The native admitted audio-only/Utility class and original
Pulse24 / PR177 acceptance remain unchanged.
