# mutable instrument source loops

Status: bounded source-loop admission is implemented in the shared SessionPlayback
renderer. The runtime does not wrap an enabled Arrangement transport loop for a
mutable instrument.

## occurrence and gate contract

A prepared MIDI source owns one intro template and one repeat template. A source
note in the intro is emitted at most once. A note whose onset lies inside the
loop interval is emitted once per repeat, with a monotonically increasing
`iteration` on its prepared span and event. Source start trimming offsets the
intro timeline; the first repeat begins after the intro reaches loop end. The
current Hex contract does not chase an authored onset before the initial
`source_start`, even if that note's gate overlaps the trim point. A note authored
at or after `source_start` but before `loop_start` is intro-only: it is not
repeated, and its gate may be clipped at `loop_end`.

A held authored note is clipped at the exclusive source loop end on every
occurrence. The event key combines track, clip, note, source iteration and
Arrangement transport-loop pass. Session routes use pass zero. The mutable
ledger assigns a distinct checked runtime occurrence ID to each delivered key,
then pairs its NoteOff with that exact ID. This keeps repeated gates independent
through nonuniform callback partitions.

Arrangement clips remain finite: their source-loop repetitions stop at the
Arrangement clip end. Session source loops continue while their cell owns the
track. Arrangement transport-loop wrapping remains refused for mutable
instruments because this admission does not provide a discontinuity history
contract for a voice that crosses that wrap. Linear Arrangement export retains
its monotonic clock and existing mutable seek/chase limitations; it continues
to refuse mutable Arrangement source loops.

## bounds and recovery

The source/template compilers continue to enforce the existing maximum tempo,
minimum advancing loop period, query width, repeat-window work ceiling, edge and
onset density, simultaneous-voice, span, event, project occurrence and beat-domain
limits. Invalid or dense sources fail during prepared revision construction;
there is no callback-side source expansion. Query and mutable event storage are
fixed and reused. A failed callback block zeros its output, rolls back mutable
runtime twins and occurrence ledgers, and leaves the committed performance and
Arrangement clocks available for a retry.

## evidence

`session_playback::tests::source_loop_tests` checks trimmed Session intro and
repeats against authored frame positions, source-end gate clipping, distinct
repeat identity, 1/17/127/256-frame partition equivalence, finite Arrangement
spans and continued transport-loop refusal. The audio `allocation-probe`
callback test renders a mutable looping Session source across multiple seams and
requires zero total allocations, bytes, signed-net allocation count and bytes.
These checks do not establish native device timing, sustained callback deadlines,
loop-wrapped transport behavior or Live numerical parity.
