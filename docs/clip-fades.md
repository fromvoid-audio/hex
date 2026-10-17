# Clip fade contract

This is an original Hex policy. The installed Live fade-controller analysis has
not established a rendered curve or its units; it does not prove compatibility.
Implementation and acceptance status are tracked in `plan.md` and `../todo.md`.

All positions and lengths below are project-rate frames. A clip envelope stores
`span`, `offset`, `fade_in`, `fade_out`, two silent endpoint offsets and two integer curve bends. The span must be positive, each fade
length must be at most the span, and checked `offset + clip.duration` must fit
within it. A clip-local sample at `x` evaluates the original domain at
`q = offset + x`. The source interval and its looping phase are independent.

For a fade length of zero, its factor is unity. For a fade-in length `n >= 2`,
the factor is `q / (n - 1)` within its first `n` samples and unity afterwards.
A one-frame fade-in is unity. For a fade-out length `n >= 2`, the factor is
`(span - 1 - q) / (n - 1)` within the final `n` samples and unity beforehand.
A one-frame fade-out zeros the final sample. Multiply the two factors when
they overlap. This provides bounded linear endpoints without a stateful
smoother, a source-media rewrite. With zero curve bends and edge-aligned lower handles, this legacy law remains sample-exact.

The legacy `SetClipFades` command establishes a fresh span equal to the current clip duration,
offset zero, and the requested lengths. Invalid requests fail transactionally.
Move and duplicate retain the envelope. Splitting at local frame `cut` retains
the span and lengths on both children and increases the right offset by `cut`.
Thus samples retain their exact coordinate through recursive splits. A child's
inherited fade lengths may exceed its own duration; they describe the retained
domain rather than a newly fitted ramp.

Trimming is an explicit envelope edit: reanchor to the new duration and offset
zero, clamping each silent endpoint to the new duration minus one, then each fade length to the remaining span; curve bends are retained.
This can reshape an inherited partial fade. Do not describe trim as preserving
the prior sample gain. Loop splits retain the source interval and advance the right child's source
phase modulo its length, independently of the retained fade domain. An unchanged
edge trim is a true no-op and retains the inherited envelope.

Compilation carries only validated envelope metadata into prepared playback.
Evaluate gains without allocation or mutable ramp state so seeking, render block
partitioning and project-loop wrapping use the same sample coordinates. A neutral
envelope bypasses gain multiplication to retain the legacy PCM path. Schema 28
persists a bounded, exact Arrangement-audio-ID bijection in `fade_shapes`,
augmenting the frozen legacy fade DTO and retained musical domains. Schemas 25–27
migrate with edge-aligned endpoints and linear curves; schemas 1–5 migrate to
neutral envelopes. Earlier readers reject the new field, and explicit legacy
encoders refuse shaped fades.

The UI displays the clipped local envelope and uses canonical edit commands.
When an inherited domain is edited, the readout must make reanchoring apparent.
Repeated absolute edits must not race a pending canonical update. Small icon
controls and bounded visible-clip geometry preserve the compact workspace.

Acceptance includes boundary and one-frame samples, overlap, malformed domain
and overflow rejection, history, move/duplicate/trim, recursive split continuity,
seek and block partition independence, source/project loops, strict migration,
saved export PCM and the unchanged neutral export. Profiling is an in-memory
CPU check; native interactions, listening and equivalent Live fixtures remain
separate gates. Automatic fades and crossfades remain later work.


## Pointer handles and curves — 17 October 2026 / Pulse2693

In Arrangement, hold unmodified `f` with workspace focus to reveal neutral audio
clip handles. Authored lengths, endpoint offsets or nonzero curve settings keep
handles visible after release, including on unselected clips. Text inputs retain
normal typing; the enabled computer MIDI keyboard retains `f` as a note key.
Releasing the key during a pointer gesture leaves that gesture visible until its
release or cancellation. MIDI clips and Session sources do not expose audio fades.

Each side has an upper length handle, a lower silent endpoint handle and, when
its fade contains at least three samples, a middle curve handle. Upper handles
move horizontally. Lower handles move horizontally while preserving the upper
endpoint. The middle handle moves vertically, bending from convex through linear
to concave. Geometry stays bounded to visible clips and viewport coordinates;
short curves receive their own interior samples even on long clips. The curve and
handles occupy the waveform body below the shared name strip. Existing
inspector step controls retain endpoint/curve settings; reset clears the envelope.

The inward offsets `in_start` and `out_end_offset` default to zero. Before
`in_start`, and at or after `span - out_end_offset`, gain is zero. Between those
endpoints the previous ramp lengths apply, translated to the new endpoints.
Each length must fit the span remaining after its respective offset. Existing
overlap multiplication remains supported. For a linear factor `t` and signed
bend `c` in -1000..=1000, the independent Hex curve is
`t * (1001 - c) / (1001 + c - 2*c*t)`. This monotonic bounded rational law has
exact zero/unity endpoints, needs no transcendental call or mutable smoother,
and bypasses the bend arithmetic at zero. This is not an experimentally matched
Ableton transfer function; the installed Live reference is unavailable on this
Linux cloud executor.

A drag retains one clip snapshot and a bounded metadata preview; it never
compiles audio or queues project work on pointer motion. Release submits one
`SetClipFadeEnvelope` command and therefore one undo step. The preview is visual;
audio applies after the normal prepared revision is accepted. A click without movement creates no edit. Escape, focus/window loss, resize,
project-generation/revision changes, pending work and changed lane/viewport
geometry cancel the gesture rather than committing a stale edit. Editing an
inherited split/musical fade explicitly reanchors to the current clip, as the
inspector and handle tooltips disclose. Holding `f` exposes those local edit
handles even when the inherited endpoints lie outside a split; the stored curve
continues to display its actual gain until the pointer changes it. Splits retain shaped domains and musical projections.

Native user checklist (pending; source tests do not establish native acceptance):

1. Hold/release `f` over default audio clips; drag both top handles; release `f`.
   Edited curves and their handles should remain visible on unselected clips.
2. Move both lower handles inward, then bend each middle handle up/down. Verify
   the endpoint stays fixed and playback/export follows the displayed envelope.
3. Drag outside the clip, cancel with Escape, and switch focus/windows while held.
   Verify no clip move/trim or stuck handle mode, and one Undo per completed drag.
4. Split a shaped/musical clip, change tempo, save/reopen and compare playback;
   verify Reset clears the envelope. Compare interaction and listening with Live.
