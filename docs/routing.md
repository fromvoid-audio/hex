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
same check to a newly constructed candidate. Canonical Snapshot now carries ReturnFeed records: every admitted Project still
has empty authored routing. Any future opening of admission must enforce complete
engine preparation/rendering; changing only the adoption guard would ignore
routing semantics.
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

## Return-feed preparation — admission closed

The next prepared model owns one `ReturnFeed` record per directed Return pair:
global monotonic `RouteId`, source/destination `ReturnId`, and finite positive
gain up to2. Gain belongs to that record only. Gain edits retain identity, zero
removes, and recreation allocates above the retained high-water mark. A maximum
of120 feeds matches the unique DAG bound for16 Returns. Missing endpoints,
global ID collisions, duplicate pairs, invalid gains and all cycles refuse.
Removing a Return cleans both incoming and outgoing feed records in the same edit.

Canonical `Snapshot.return_feeds` carries the same Project-owned records through
snapshot/history/storage, with no parallel production envelope. `Snapshot::with_return_feed`
prepares a new validated canonical metadata candidate without mutating its source.
`SetReturnFeed` and `Project::from_snapshot` still refuse any nonempty result before
canonical adoption. All publicly admitted Projects have no feeds. Tests may
construct private unadmitted metadata to check ordinary snapshots, cloning/history
and removal; that does not authorize rendering or production adoption. Necessary
legacy snapshot constructors explicitly initialize an empty feed list.

Reserved schema20 requires strict `routing` records tagged `return_feed`, each
with required id/source/destination/gain. It retains bounded original provenance,
opaque state and every existing project field. Nonempty decode/load validates then
refuses. The ordinary encoder remains schema19 during this preparation phase;
explicit `encode_schema20` only encodes admitted legacy Projects, under the same
document ceiling. Schemas1–19 retain their established migrations; nonempty old
schema19 proposals remain unsupported rather than receiving guessed IDs/gains.
Operator state v9/v10 and preset envelope1 are separate formats.

Approved future audio semantics are additive post-source-chain/matrix/gain/mute
into destination input before its chain. The source still contributes Main.
Each source runs once, master runs once, and captured old Return residuals stay
zero-input Main-only without replaying old feeds. Actual adoption must wait for
both render consumers, prepared capability checks, schema20 writer handoff and
verified canonical snapshot/history against the frozen combined pages/observer
API. No MIDI/sidechain/latency capability is added by this metadata preparation.
