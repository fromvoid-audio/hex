# bounded native parameter gestures

Pulse ticket 65 owns the prepared-preview protocol and grouped history. It is
under source acceptance; native panel interaction and listening remain open.

One project has one active gesture, identified by project-open generation and a
monotonic token. Each transient carries an increasing sequence. Begin validates
the target, Change prepares an overlay on the latest canonical project, Commit
includes its final value and performs one prepared canonical edit, and Cancel
restores the latest canonical playback. Previews never change dirty state or
history. Independent canonical edits are retained and the overlay is rebased;
invalid or deleted targets retire it. Project replacement invalidates old tokens.

Canonical prepared state remains separate from preview state. A preview carries
its exact read-only source view and generation/token/sequence/base-revision
identity through publication. The existing callback publication and retirement
ownership are unchanged. Audio application and canonical revision acknowledgements
are distinct; a transient is not a new canonical project revision.

The UI coalescer retains one in-flight protocol request, one replaceable latest
transient and one terminal request. Full service queues retain exact ownership
for retry. The service FIFO holds 32 requests and its event channel holds two;
ordinary UI pending plus deferred post-terminal requests share a total capacity
of 32. Only transient changes coalesce. Save/export/undo/redo finalize before
capture; post-terminal edits cannot overtake the commit or earlier barriers.
Rejected final values must not silently publish old-value saves or exports.

Close and Quit include active drafts and retained terminal work in unsaved
protection even when the canonical project was clean at Begin. Cancel keeps the
work; Discard explicitly permits closing. Panel bindings must also finalize or
protect local typed input before close, and retain the local value during a
pending commit rather than synchronizing an old canonical value into the widget.

## worker roundtrip profile

`cargo run --release -p hex-session --example profile_device_gestures` uses the
public Service and GestureClient protocol at 48 kHz. Fixtures contain 1 or 100
tracks, one Utility and one empty four-beat Arrangement MIDI clip per track;
construction/initial compilation are outside timing. Five warmups precede 100
timed complete Begin/coalesced-Change/Commit roundtrips. Every iteration checks
the prepared preview and canonical final value. Bounded active event polling
uses `yield_now`, deliberately unlike the paced native UI event loop.

On Apple M5 Pro, arm64, Darwin 25.6.0, 9 October 2026:

| tracks | p50 | p95 | maximum |
| --- | --- | --- | --- |
| 1 | 33 µs | 116 µs | 234 µs |
| 100 | 148 µs | 202 µs | 266 µs |

The original 1 ms polling sleeps dominated a roughly 4.5 ms measurement and were
removed from the benchmark wait. This is a light metadata/preparation workload,
not dense notes/media, widget input-to-paint, audio-applied latency, callback
allocation evidence or physical-device deadlines. The retained root log is
`artifacts/device-gesture-worker-profile-20261009.log`; timings are provisional.
