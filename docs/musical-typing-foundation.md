# musical typing: finite native input stage

Pulse #26 / canonical draft PR190. The original private protocol foundation is
now consumed by the engine, the existing audio command channel, the session
control worker and GPUI keyboard capture. This is the approved finite computer
keyboard stage. The original recording and MIDI I/O issue remains incomplete.
Native optimized macOS keyboard/CoreAudio, listening, device delivery and
hardware deadline acceptance are separate pending requirements.

## user contract

The transport toolbar has an explicit `typing · off / waiting / ready` arm
control. Select a track with an admitted native instrument, arm it and start
Play explicitly. Readiness here reports actual audio admission, not queue
submission or issue/PR review readiness. The lowercase canvas keys
`a w s e d f t g y h u j` map to pitches 60–71 (c4–b4), fixed velocity 100.
Sine, FM4 and Operator opt into explicit held/released note sampling; other
providers default to unsupported. A bounded palette validates all twelve keys
on the control worker and exposes the actual supported-key bitmap.

New onsets require a playing transport, active window, admitted original owner
and canvas/MIDI focus. Text editing, modals, modifiers and a pending seek block
new notes. Owned key-up is captured before child widgets, even after focus or
selection changes. Autorepeat/duplicate downs cannot start another occurrence.
Disarm/window deactivation/audio failure cut the typing epoch. Native keyboard
unplug and hardware MIDI notifications are not provided by this stage.

Arming/disarming never edits project state, adds an Undo step, creates authored
clips or implicitly starts transport. Full presets, oscillator enable payloads,
untouched numeric drafts, freely positioned hybrid-track instruments and shared
lattice controls retain their existing contracts and original PR histories.

## ownership and application

Canonical project generation zero is valid; transient owner encoding uses its
checked `generation + 1` incarnation. Track/device IDs are the original IDs.
Physical press tokens retain their original owner/epoch/serial. Authored source
signatures retain real clip/note IDs, musical values and loop pass. Mutable
native live and authored occurrences use the SAME checked host occurrence
allocator and separate provenance ledgers; a press serial is not a native ID.

`NoteGate::Held` is explicit. Release carries the original note age at actual
NoteOff. No fake infinite finite gate, invented clip, authored note or synthetic
clock horizon represents a physical hold. The existing authored sampler ABI
remains 344 bytes and its finite-note law remains unchanged.

The real 32-command audio ring returns queue tickets. Consumption is not
application. Receipts are staged until the complete instrument/suffix render
transaction commits, then published only after final channel/format validation
and output copy. The bounded atomic snapshot reports original owner, actual
performance frame, action and refusal. Mono overflow and nonfinite output use
the existing fault Stop/cut path; failed output cannot publish an old successful
On later. This adds no recording transaction or rollback claim.

A separate atomic cancellation watermark survives a full release queue and is
observed before publication and the captured command prefix. A paused transport
freezes age and holds pending releases until BEFORE its first resumed sample.
An unapplied On cancelled by Pause cannot resurrect on resume. Stop/reset and
successful Seek cut the epoch; refused Seek retains prior ownership. New input
stays blocked while a seek is pending. Rearm requires the previous cut receipt,
including after replacing an audio channel. Counter exhaustion refuses safely.

## complete render and publication path

Audio uses the real track prefix once; the instrument adds authored and typed
sources at its canonical gap; the suffix runs once. Memoryless chains use their
validated prefix/suffix matrices, while stateful chains use the actual native
runtime. Track automation/mix, sends, Return feeds and Main then run once.
Silent Arrangement/Session sources can still carry live input. Mutable authored
route changes retain their existing active-gate refusal; typing-only runtime
ownership does not block source navigation. Linear export
refuses transient arming and does not invent recorded notes.

Compatible publication preserves typed phase/age/feedback/LFO and original
physical authored gates. Pre-cut candidate chase is suppressed against retained
source identity; changed future authored signatures enter only at their real
eligible boundary. Analytic retained voices use the existing bounded release
bank with full authored signatures; mutable original proofs retain their native
IDs and absolute original gate/release deadlines. A preflight failure leaves
all old owners intact. Compatible mutable handoff retains the established
provider/topology proof, including unchanged effect configuration requirements.
Instrument type or processing-class changes explicitly cut/disarm and require
rearming; this stage does not invent an analytic-to-mutable voice import.

Structural disarm retains the original instrument palette/runtime, suffix, mix
and stable Return send IDs through release. An absent original Return refuses
atomically rather than redirecting to another bus at the same index. Existing
safe explicit Stop remains available before such a structural edit. This does
not add removed-bus source-input hosting to the Return effect tail bank.

Before owner movement, admission checks combined authored/live voice capacity
(64 analytic, 16 Operator mutable), the 256-event limit including releases,
checked IDs, release/suffix/context resources, ACK capacity and source fences.
Source/global density and retained-voice bounds are conservative; insufficient
proof refuses instead of stealing voices or widening caps. Queued actions can
still receive an explicit application refusal when capacity is reached.

## storage and verification boundaries

The control-prepared palette charges captured state, exact effect topology,
contexts and ownership bookkeeping. The actual performance owner charges both
voice ledgers, pending actions, native event scratch, receipts and admission
stamp. Retained typing owners, original runtime ledgers and suffix chains are
charged before publication. Shared configuration stays within the existing
2 MiB context bound; complete playback runtime stays within 32 MiB. No cap or
accounting exclusion is changed.

The audio bridge charge includes Control/Callback containers, status atomics,
all mandatory publication/retirement/command rings and auxiliary channel
allocations, including Arc header alignment. A Linux allocation probe compares
that charge with actual owned allocations; callback tests cover total and net
allocation/deallocation across Sine, FM4, static Operator, LFO, feedback and
combined LFO/feedback. The original reference Ledger remains a mathematical
model; its tests do not replace actual channel/PCM/GPUI tests.

Meaningful regressions cover independent Sine hold/release math, all twelve
pitches, mixed authored/typed continuity and future edits, capacity/ID refusal,
Pause releases, output rejection, queue-full recovery, loops, Session routing,
hybrid placement/effects/automation/Return feeds, retained release ownership,
export isolation, worker/history stamps and rendered toolbar/key capture with
a real bounded headless audio channel. The latter is a test-only injection;
production has no headless output fallback. A separate ignored optimized Linux
timing probe measures the changed callback/receipt path, not native deadlines.

## pending original acceptance

Actual optimized macOS keyboard/CoreAudio delivery, held-note editing/listening,
shutdown/device lifecycle and deadline acceptance remain pending. The original
#26 audio input/monitoring/recording, recoverable capture, hardware MIDI input,
zero-velocity MIDI NoteOff, expression/MPE, MIDI output and
Session-to-Arrangement capture are not implemented by this finite stage.
#2372/#2384 can now use a real source input path for their native review, but
neither their native criteria nor broad #26 completion is claimed here.

The coordination HOLD keeps Pulse blocked, PR190 draft, and gates/verification
receipts/readiness unregistered. No merge, deployment, delivery or Done occurs.
