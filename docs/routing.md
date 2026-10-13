# routing topology proposals

Pulse28 / PR184 adds a control-side structural validator, not general routing
playback. The entire ticket's routing, automation editing/recording, sample-offset
playback, cumulative latency and native acceptance remain incomplete.

`RoutingOwner` reuses canonical TrackId and ReturnId with one Main sink. There
is no independent node allocator, persisted send gain or editing model. Existing
post-fader sends and implicit track/Return-to-Main connections participate in
the graph check. Current regular tracks expose stereo output, Returns stereo
input/output, and Main stereo input. Track inputs and Main outputs are refused.
MIDI bus, device and sidechain capabilities are not established by current
canonical notes or opaque device state and are explicitly refused.

At most4096 additional edges can be proposed. Missing or stale owners,
duplicate edges (including an existing send/Main connection), incompatible
channels/roles and every cycle refuse the complete proposal. Validation uses a
bounded control-side topological traversal. A structurally valid Return-to-Return
proposal is not an admitted send or a feedback capability.

Non-legacy adoption is closed. `Command::SetRouting` validates first, then
returns `authored routing requires playback support` for any nonempty list
before project/history mutation. An empty list is a legacy no-op. Failed edits
preserve IDs, undo and redo. `Project::from_snapshot_with_routing` applies the
same check to a newly constructed candidate. The existing Snapshot API remains
unchanged: every admitted Project has empty authored routing. Any future opening
of admission must first extend snapshots/history and enforce engine compilation;
changing only the adoption guard would lose metadata or ignore routing semantics.
No route UI is exposed.

Schema19 requires an explicit `routing` array and retains every schema18 field,
including bounded exact original import bytes. Typed edge objects strictly
reject unknown fields/roles and bounded decoding rejects excess edges. Decode
checks the candidate graph and refuses even structurally valid nonempty lists;
they never reach engine preparation. Encode only writes admitted legacy routing
as an empty array. Older encoders must not discard authored routing, and older
schemas reject a routing backport. Schemas1–17 migrate to legacy routing with
absent provenance; schema18 retains its provenance and existing sends/Main
semantics. Device state versions/payloads remain opaque, including Operator's
separate reserved v9/v10. No Operator codec change belongs here.

Tests cover malformed/unsupported proposals, cycles, duplicate implicit edges,
exact edge capacity, atomic refusal and redo retention, stale owner removal and
undo, schema18/19 round trips, strict migration and exact opaque state/provenance
retention. Linux evidence does not establish native routing, listening or latency
acceptance. Later graph scheduling/publication changes must be sequenced with
the streaming owner; this stage changes no engine/session/audio behavior.
