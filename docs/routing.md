# routing topology and first Return feeds

Pulse28 / PR184 admits one bounded audio stage: additive Return-to-Return feeds.
The full routing, automation editing/recording, sample-offset playback, cumulative
latency and native acceptance ticket remains incomplete. No route UI is exposed.

`RoutingOwner` reuses canonical TrackId and ReturnId with one Main sink. Existing
post-fader sends and implicit track/Return-to-Main paths participate in graph
validation. Current regular tracks expose stereo output, Returns stereo
input/output, and Main stereo input. Track inputs, Main outputs, MIDI buses,
device ports and sidechains remain refused; opaque state implies no capability.

## canonical Return feeds

One Project-owned `ReturnFeed` record contains a global monotonic `RouteId`,
source/destination `ReturnId`, and finite positive gain up to2. Gain edits retain
identity, zero removes, and recreation allocates above the retained high-water
mark. The unique DAG on16 Returns admits at most120 feeds. Missing endpoints,
global ID collisions, duplicate pairs, malformed gains and all cycles refuse
atomically. Removing a Return cleans both incident directions in the same edit;
undo restores identities. `Snapshot.return_feeds` is the same canonical state
used by history and persistence, with no parallel routing envelope.

`SetReturnFeed` and validated snapshots admit only this typed stereo contract.
A feed adds post-source-chain/matrix/gain/mute samples to destination input
before its chain. Source output still contributes Main. Each Return chain runs
once, dependency-ready ties use pinned Return order, and Main sums current
Return outputs in pinned order before its one master stage. Feed serialization
order cannot change execution or input summation. With no feeds the original
render loops and arithmetic order remain intact in both consumers.

Arrangement rendering and prepared Session/linear-export playback share a
control-prepared plan. Return processors still require known zero algorithmic
latency; regular processor admission retains its established policy. This stage
adds no compensation, automation, MIDI or sidechain behavior. Edge/gain edits
retain compatible Return-chain history. Removed/changed eligible chains retain
zero-input Main-only residuals using their old bus coefficients, never replaying
old feeds. Surviving destinations decay naturally after feed removal. Pause
freezes history, Stop/Seek reset it, and transport wraps preserve it. Whole-block
failure keeps committed histories/routes/clocks unchanged and silences output.
Borrowed pages and post-node observations retain their existing speculative
transaction and ownership contract.

## persistence and unsupported proposals

Schema20 requires strict `routing` records tagged `return_feed`, each with
required id/source/destination/gain, and rejects unknown fields/ports. It retains
every project field, bounded exact import provenance and opaque device payload.
The ordinary encoder selects20 for nonempty feeds and keeps the schema19 byte
path for legacy projects. Explicit `encode_schema20` also handles empty feeds.
Older envelope writers refuse nonempty canonical feeds instead of discarding
them. All encoding remains under the existing8MiB document ceiling.

Schemas1–18 retain their established migrations to legacy sends/Main paths;
schema19 still accepts only its empty structural proposal array. Nonempty old
schema19 proposals remain unsupported rather than receiving guessed gains/IDs.
Operator statev9/v10 and preset envelope1 are separate formats.

The generic `SetRouting` proposal API checks up to4096 additional edges against
canonical sends, admitted feeds and implicit Main paths. Missing owners,
duplicate edges, incompatible roles/channels and all cycles refuse. Nonempty
structural proposals still return `authored routing requires playback support`
before mutation; only the explicit ReturnFeed command adopts playable new edges.
Empty proposals remain no-ops. This preserves truthful refusal for other ports.

Source tests cover PCM, dependency order/permutations, partitioning, transport,
history/IDs/removal/undo, strict save/load, opaque/provenance preservation,
rollback, borrowed observation, callback allocation and retirement. Linux does
not establish native routing interaction, CoreAudio deadlines, optimized macOS
packaging, listening, Live reference behavior or full-ticket acceptance.
