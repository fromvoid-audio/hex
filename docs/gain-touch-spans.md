# gain touch metadata preparation

PR184 adds schema22 model/storage preparation. Actual touch recording, audible
application acknowledgments, runtime overrides and native recording remain
refused. This stage does not complete Pulse28.

A Track owns one GainLane: its unchanged base GainPoint curve, a flat ordered
list of disjoint half-open touch spans, and a durable recording-epoch high-water
mark. TrackSnapshot and bounded Undo/Redo preserve that canonical value. There
are no nested lanes or copied base curves. Empty spans preserve all prior
prepared playback and preview behavior.

A span owns `[start, end)`. Its first step holds from start; each later strictly
ordered beat replaces the held value. At end the original base/static fallback
owns the read again, including when end coincides with a base point. The model
returns `None` outside spans rather than interpolating a replacement base curve.
It preserves authored beats; no runtime frame quantization is introduced here.

A pure proposal validates before replacing overlapping spans. Surviving left and
right fragments retain their original context, event sequence, gain and original
beat anchor. A right fragment carries its prior held event at the new start.
Adjacent spans are allowed. Replacement requires the current lane epoch and a
strictly newer recording epoch; Undo/Redo cannot rewind a surviving owner's
high-water mark. Owner removal removes the lane and history can restore it.

Structural bounds are 128 spans per track, 1022 steps per individual span and
1024 weighted units per track: base points plus retained steps plus two endpoints
per span. The project limit is 65536 weighted units. Fragment costs are counted
before cloning event/base vectors. These are control-model and decoding limits,
not a proof of callback capacity, process memory, or real-time event reservation.

Each span has structurally validated context metadata: recording epoch, project
generation, canonical/audio revision, transport/gesture epochs, gesture serial,
sample rate and loop pass. Contexts within one recording epoch must agree;
retained event identities cannot change gain or origin beat. These stamps do not
prove that audio applied a value. Runtime counters, the proposed 64 events plus
terminal capacity, and successful whole-output ACKs are not implemented here.

Schema1–21 migration produces empty spans and epoch zero while preserving base
curves, ReturnFeeds, import provenance and opaque native-device payloads. Strict
schema22 requires span/epoch fields and rejects malformed, unknown, unordered,
overlapping, over-capacity or inconsistent context data. Epoch-only projects can
save/load schema22. Older writers refuse touch metadata rather than lose it.
Normal Project adoption, splice commands and schema22 decode refuse nonempty
spans until both prepared Arrangement/Session and export consumers support the
complete value. Private tests serialize proposed metadata to check roundtrip;
this is not an alternate production save/history owner.

Existing lane transforms refuse a hypothetical nonempty span owner; base edits
on admitted epoch-only projects preserve its epoch. No UI recording control or
production override/ACK path is added. Linux model/math tests prove neither
native interaction nor actual recording PCM. Worker1's endpoint-domain ownership
escrow/reaper is not PCM/output transaction escrow or a gain-application ACK.
The existing 256-frame render quantum versus whole hardware callback commit and
rollback boundary remains unresolved for a future actual recording stage.
