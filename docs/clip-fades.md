# Clip fade contract

This is an original Hex policy. The installed Live fade-controller analysis has
not established a rendered curve or its units; it does not prove compatibility.
Implementation and acceptance status are tracked in `plan.md` and `../todo.md`.

All positions and lengths below are project-rate frames. A clip envelope stores
`span`, `offset`, `fade_in` and `fade_out`. The span must be positive, each fade
length must be at most the span, and checked `offset + clip.duration` must fit
within it. A clip-local sample at `x` evaluates the original domain at
`q = offset + x`. The source interval and its looping phase are independent.

For a fade length of zero, its factor is unity. For a fade-in length `n >= 2`,
the factor is `q / (n - 1)` within its first `n` samples and unity afterwards.
A one-frame fade-in is unity. For a fade-out length `n >= 2`, the factor is
`(span - 1 - q) / (n - 1)` within the final `n` samples and unity beforehand.
A one-frame fade-out zeros the final sample. Multiply the two factors when
they overlap. This provides bounded linear endpoints without a stateful
smoother, arbitrary curve storage or a source-media rewrite.

Setting fades establishes a fresh span equal to the current clip duration,
offset zero, and the requested lengths. Invalid requests fail transactionally.
Move and duplicate retain the envelope. Splitting at local frame `cut` retains
the span and lengths on both children and increases the right offset by `cut`.
Thus samples retain their exact coordinate through recursive splits. A child's
inherited fade lengths may exceed its own duration; they describe the retained
domain rather than a newly fitted ramp.

Trimming is an explicit envelope edit: reanchor to the new duration and offset
zero, clamping each previous fade length independently to the new duration.
This can reshape an inherited partial fade. Do not describe trim as preserving
the prior sample gain. Loop splits retain the source interval and advance the right child's source
phase modulo its length, independently of the retained fade domain. An unchanged
edge trim is a true no-op and retains the inherited envelope.

Compilation carries only validated envelope metadata into prepared playback.
Evaluate gains without allocation or mutable ramp state so seeking, render block
partitioning and project-loop wrapping use the same sample coordinates. A neutral
envelope bypasses gain multiplication to retain the legacy PCM path. Schema 6
must persist the complete domain; schemas 1–5 migrate to neutral envelopes.

The UI displays the clipped local envelope and uses canonical edit commands.
When an inherited domain is edited, the readout must make reanchoring apparent.
Repeated absolute edits must not race a pending canonical update. Small icon
controls and bounded visible-clip geometry preserve the compact workspace.

Acceptance includes boundary and one-frame samples, overlap, malformed domain
and overflow rejection, history, move/duplicate/trim, recursive split continuity,
seek and block partition independence, source/project loops, strict migration,
saved export PCM and the unchanged neutral export. Profiling is an in-memory
CPU check; native interactions, listening and equivalent Live fixtures remain
separate gates. Automatic fades, crossfades and adjustable curves are later work.
