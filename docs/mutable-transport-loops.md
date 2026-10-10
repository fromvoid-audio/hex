# mutable instrument Arrangement transport loops

## contract

Interactive `SessionPlayback` admits an enabled Arrangement transport loop for
tracks with mutable native instruments only after control-side density proofs
pass. The proof checks every first-traversal source span and the repeating loop
period, including release and ordered-event overlap from intro voices into the
first wrapped pass. Admission enforces the 256-frame onset bound, the smaller of
the device-declared and engine 64-voice capacities, the source-span ceiling, and
the smaller of the device-declared and engine 256-event capacities before a
runtime is constructed. Ordered NoteOn/NoteOff edges are bounded for intro,
steady periodic windows and the first-wrap seam. Span collection
is bounded and fallible. An empty short loop is valid; there is no arbitrary
minimum transport-loop duration beyond a positive advancing frame period.

Pass zero is the first traversal from the transport origin. Reaching
`loop_start` for the first time does not increment the pass or issue a chase:
already-sounding gates continue as authored. At an actual wrap, the checked
monotonic pass increments. A note authored before `loop_start` is attacked
again at `loop_start` only if its gate was still active there on the first
traversal. That fresh attack uses the authored remaining gate, ending at
`min(authored gate end, loop end)`. A note authored in the loop is scheduled at
its ordinary phase. Every occurrence key combines track, clip, note, source
iteration and transport pass, and receives a fresh checked runtime ID. The
exclusive loop end emits NoteOff before the following pass's NoteOn.

The mutable engine is not reset at wrap, so device feedback and effect tails
continue. Render partitions are independent. Each callback block commits the
clock, loop pass, instrument runtime and occurrence ledger together; a late
failure silences the complete requested output and keeps the committed state
available for retry.

Changing enabled loop bounds during an active Arrangement mutable gate is
refused at revision handoff. Tempo-map edits are also refused while a mutable
Arrangement loop gate is active because the new frame deadline cannot yet be
proven from the carried runtime; the old runtime and PCM remain unchanged.
Handoff with unchanged timing and loop bounds retains the monotonic pass and
mutable gate. Seek/preroll chase remains unsupported and keeps its existing
refusal. Linear Arrangement export remains a monotonic renderer and
continues to refuse mutable Arrangement source loops. These choices preserve the
existing clock and export contracts; they do not claim Ableton Live parity.

## evidence

`session_playback::tests::source_loop_tests` compares finite source-loop output
against an independently expanded Arrangement and checks source iteration IDs.
`session_playback::tests::transport_loop_tests` checks a note that starts before
`loop_start` but ends inside the loop (the wrapped gate keeps only its remaining
authored duration), compares whole-block with 1/17/127/256-frame partitions
across multiple wraps, and verifies a release-only pre-loop candidate does not
become a false chase at handoff. Other checks cover the first natural loop entry,
first-wrap fresh attack, per-device event/voice limits, aggregate event-edge
density, first-wrap release seam, bounded short-loop admission, linear-export
refusal, late pass-overflow rollback/retry, active bound-edit refusal, active
tempo-edit refusal with old PCM continuity, and pass/gate preservation across
unchanged handoff. The callback allocation
probe measures a real prepared Arrangement loop through a wrap and requires
zero total and signed-net allocation count/bytes. This evidence does not prove
native-device timing, physical callback deadlines, or audible acceptance.
