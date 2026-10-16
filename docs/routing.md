# validated audio and MIDI routing

Pulse28 / PR184 uses canonical typed routes with prepared playback plans.
Existing post-fader sends remain track-owned. Additional route records own
monotonic global RouteIds, participate in snapshots/history, and survive strict
schema24 save/load. There is no parallel routing project or duplicated send gain.

## stereo audio

`SetAudioFeed` adds a Track or Return post-fader stereo output to a destination
track's input, before its effect prefix. `SetReturnFeed` adds a Return output to
another Return input before its chain. Source outputs still contribute to Main;
these are additive feeds. Feed gains are finite0..2: zero removes, gain edits
retain identity, and recreation allocates above the retained ID high-water mark.

Validation includes all sends, Return feeds, track feeds and implicit Main paths.
Missing owners, self/duplicate pairs, wrong port directions/channels, malformed
gains, ID collisions, capacity excess and every cycle refuse the complete
candidate. Adding a send or Return feed must also preserve this combined DAG.
Track/Return removal cleans incident routes atomically; Undo restores identities.
Main exposes no source port, and Returns expose no MIDI port. No device sidechain
or arbitrary channel capability is inferred from opaque device state.

The control-prepared owner schedule runs each track/Return once in dependency
order. Contributions use source post-device gain/mute/balance before the target
chain. Canonical ties and Main summation order are deterministic. No-route,
zero-latency projects retain the existing block path. Authored track feeds and
nonzero latency use the shared graph and [compensation](latency-compensation.md).
Compatible coefficient edits retain DSP and in-flight history; topology changes
that cannot transfer history safely require Stop before adoption.

Detached old effect/source residuals retain their frozen transition mix and
contribute Main only, never replaying removed feeds. Surviving target chains
decay naturally after feed removal. Pause freezes state; Stop/Seek reset it;
transport loops retain it. Whole-block failure silences output and preserves
committed histories/routes/clocks. Borrowed pages, capture and observations keep
their existing transaction and ownership boundaries.

## MIDI sources

`SetMidiSource` selects one source track for a destination instrument. `None`
restores destination-local authored MIDI. Fanout is supported; the source need
not contain an instrument. Source chains are acyclic and resolve to their root
MIDI event bus, so A→B→C sends A's events to both destinations. The destination
owns instrument construction, note bank, effects and fader; audio is not forwarded
as a substitute for MIDI. Destination audio ownership remains independent.

Arrangement and launched Session source clips prepare against the destination's
actual instrument through the normal registry. Source clip/note identity and
destination runtime identity remain distinct. Fanout contributes to complete
revision template, note-bank, event and work admission before publication.
Source launches/stops drive all mapped destinations and retain existing bounded
release/chase rules, including explicit refusal where mutable continuation is
unsupported. MIDI topology edits while playing or paused require Stop. Priority
transport Stop remains effective under full queues and clears active voices.

## controls and persistence

The mixer strip's `routing` control opens a bounded scrollable panel in Arrangement
or Session. For a selected track it edits additive input gains and selects local
or upstream MIDI. A selected Return exposes Return input feeds; regular sends
retain their shared numeric controls. All actions dispatch canonical worker
commands and surface validation/preparation errors. Gain recording fences project
edits until finish/cancel. No UI-local scheduler or routing state is authoritative.

Schema24 strictly requires audio-feed and MIDI-route arrays alongside the complete
schema23 document. It preserves sends, Return feeds, device input/output gains,
opaque payloads, import provenance, gain points/spans/epochs and media identity.
Older schemas migrate empty typed routes. Older writers refuse nonempty routes
rather than lose them. Historical main/feature schema18/19 shapes dispatch by key
presence to strict readers; malformed mixed forms never fall through by parse
failure. All serialization retains the8MiB document ceiling and existing
recoverable/create-new publication.

The generic `SetRouting` structural proposal API remains an explicit unsupported
port proposal boundary; playable changes use the typed commands above. It does
not silently adopt graphs that playback ignores. Source tests cover canonical
validation/history, strict persistence, actual PCM/event routing, partitioning,
transport, handoff, rollback and callback ownership. Exact optimized native
interaction/listening checks remain separate from automated local CI; physical
deadlines and Live reference parity are not established by synthetic tests.

Track gain editing and recording are specified in [track gain automation](track-gain-automation.md)
and [gain touch recording](gain-touch-spans.md).
