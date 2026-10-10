# mutable instrument source loops

Status: bounded source-loop admission is implemented in the shared SessionPlayback
renderer, including enabled Arrangement transport loops for mutable instruments.

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
track. Arrangement transport loops now wrap mutable instruments after control-
side density admission. Pass zero is the initial timeline traversal: notes
already sounding when playback first reaches `loop_start` continue uninterrupted
and are not chased or duplicated. At an actual wrap, notes authored before
`loop_start` whose gates still cover that position receive one fresh attack,
using their authored remaining gate through the exclusive `loop_end`. Notes
authored inside the loop repeat at their normal phase. A monotonic transport pass
is part of each occurrence key. Linear Arrangement export retains its monotonic
clock and existing mutable seek/chase limitations; it continues to refuse
mutable Arrangement source loops.

## bounds and recovery

The source/template compilers continue to enforce the existing maximum tempo,
minimum advancing source-loop period, query width, repeat-window work ceiling,
edge and onset density, simultaneous-voice, span, event, project occurrence and
beat-domain limits. Arrangement transport-loop admission additionally proves a
256-frame onset bound and periodic voice/release overlap for each mutable track,
including the first-wrap seam where intro releases meet chased loop notes. It
uses a fixed maximum span count and fallible collection growth; dense or
unsupported arrangements fail before runtime construction. No arbitrary minimum
period is imposed beyond advancing frame boundaries. Query and mutable event
storage are fixed and reused. A failed callback block zeros its output, rolls
back mutable runtime twins and occurrence ledgers, and leaves the committed
performance and Arrangement clocks available for a retry. Editing enabled
transport-loop bounds is refused while an Arrangement-owned mutable gate remains
active; safe handoff preserves the monotonic pass counter.

## evidence

`session_playback::tests::source_loop_tests` checks trimmed Session intro and
repeats against authored frame positions, source-end gate clipping, distinct
repeat identity, 1/17/127/256-frame partition equivalence, finite Arrangement
spans, transport-wrap PCM oracles, density rejection, transport-pass overflow
rollback and continued linear-export refusal. The audio `allocation-probe`
checks both mutable Session source repetition and an Arrangement transport loop
through an actual wrap, requiring zero total allocations, bytes, signed-net
allocation count and bytes. These checks do not establish native device timing,
sustained callback deadlines, Live numerical parity, or mutable chase behavior
after seek/preroll.
