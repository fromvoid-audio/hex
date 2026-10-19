# warp resize admission — pulse2816 / pr422

Shift stretching now admits the exact snapped command frame through the canonical
model preview and the same metadata Warp mapping/stretch predicate used by render.
The retained root, marker/tempo crossings, pitch compensation, mode exemptions,
1/8..8 range and epsilon remain authoritative. Ordinary audio trim (including
cropping an already warped root) and MIDI resize retain their existing semantics.
No PCM decode, render, graph preparation or audio callback work runs on motion.

A valid requested endpoint is displayed directly. In particular, a valid distant
endpoint is accepted even if intermediate endpoints would be invalid. Unsupported
requests retain the last validated preview and its exact command frame. Up to eight
point probes may improve that retained endpoint toward an invalid pointer target;
no further optional probe starts after one millisecond of request processing.
An already started bounded metadata check completes normally.
The initial requested endpoint is always checked, regardless of shape or elapsed
time. This is neither a support limit nor a claim of a mathematically nearest or
first connected boundary. There is no new root/map complexity refusal.

Repeated identical command frame/mode requests reuse the capture-local result.
Changing Shift cannot reuse a trim preview as a stretch command. Capture validity
continues to require the original project generation/revision, track/clip/source
and asset extent. Cancellation retires the local resolution. Pointer preview and
MouseUp use the same resolver: a final valid MouseUp may update the candidate; an
invalid release submits the retained exact command frame, without reverse-mapping
rounded visible geometry or re-snapping that retained frame.

The existing PendingTrim/FIFO owner, actual-submission stamping, canonical install
before owned viewport retirement, unrelated ACK handling, refusal and one Undo/Redo
remain unchanged. The audit reproduced the unsupported Warp preview mismatch;
its other tested audio/MIDI, fast final MouseUp/reversal, browser/outside release,
queue/deferred/own-terminal, replacement, modifier, focus and Escape paths passed.
These finite checks do not establish all physical capture-loss interleavings.

Linux debug instrumentation measured canonical preview plus metadata admission:
32 simple two-marker probes took290.437us total (9.076us/point), and32 actual
8192-raw-boundary/1024-marker/eight-cycle probes took373.290008ms
(11.665312ms/point). Per point total allocations were10/540bytes and21/540684bytes,
respectively; maximum simultaneously live temporary bytes508/475148, with zero
signed-net count/bytes after each batch. These are instrumented control-side costs,
not optimized native timing or callback measurements. Optional precision has a
small time budget to avoid multiplying dense-map cost; requested valid endpoints
are never rejected for taking longer. No new allocator dependency was introduced.

Coordinator-only finite optimized acceptance: both edges, original and nonzero
pitch, permanent-Warp ordinary trim and Shift stretching; request too-short and
too-long extents, reverse immediately, release at a new valid point and at an
invalid point, and release over the browser. Expect only admitted visible geometry,
matching committed waveform/extent, no stretch-range error or pending rollback,
one Undo/Redo, unchanged Escape/focus/stale cancellation and ordinary MIDI resize.
Use affected media without changing pitch/tempo to conceal an invalid mapping.
Physical pointer/audio/listening and delivery remain open until coordinator evidence.

The superseded interval-certificate research is preserved separately at
research/422-interval-prototype-7ed9 (30e3a704), with its raw positive and failed
fixture logs. Its unused APIs and extra Project/timebase changes are not included
in this focused fix. The accepted shared admission extraction remains7ed9debc.
