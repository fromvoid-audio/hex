# paired project exports

Pulse2783 decision21741 selects the existing restricted `SharedMetadata<Project>`
facade, exported as `hex_session::SharedProject`. Public raw Arc<Project> APIs
could not supply a closed canonical constructor invariant. This adds no owner,
cache, registry or provenance family and no engine dependency in the model.

Canonical ViewState, CanonicalAudioSourceSnapshot and normal/tuning/device
preview and gesture producers now consume fresh owned Project values once at
the existing paired constructor. Handle clones retain its receipt. Consumers
borrow immutable Project values or clone the whole paired handle. No raw Arc,
Weak, adoption, mutation or detachable receipt escapes. `shares_owner_with` is
allocation equality for cache/correlation only; `PartialEq` still compares
contents as the previous Arc consumers did, without declaring identity.

Outer header/body and each actual paired field are observed separately. A source
snapshot's field aliases the canonical view's Project allocation. Independent
canonical views and previews allocate independent outer bodies even when values
compare equal. Repeated fields do not substitute for missing exported aliases.
Actual exclusive model buffers, canonical MIDI and native-state children use
existing typed traversals; repeated visits to one body deduplicate the SAME
physical MIDI fields, while a separately cloned Project adds real child aliases.
Inline Undo Project values belong to their history buffer rather than this Arc
body charge. Source/loop/waveform geometry and UI gesture behavior are unchanged.

`visit_shared_project_selected_storage_on_control` is selected storage only.
It refuses import-provenance and edited-audio shared domains pending their own
qualification; an outer wrapper does not make those nested allocations known.
This is a prospective closed-graph restriction, not a new processing behavior:
referenced registry admission remains closed. No compatibility reader or
historical-alpha conversion is required. Existing projects/media are untouched.

Fresh Project cloning still allocates owned model buffers and shares canonical
children. Neither the facade nor final capacity observations recover old/new,
input/history/temporary construction peak. Complete live root inventory/custody,
atomic stale/cancel/refusal publication and saturated control/reaper retirement
remain mandatory. These selected exports do not qualify enclosing ViewState,
source-snapshot/preview/renderer owners or the full graph by themselves.

New oracles cover actual canonical view/snapshot plus two real preview producers,
independent equal outer bodies, omitted outer alias and whole-body totals,
exact/minus-one retained budgets, unknown nested ownership, old exports after
Session retirement, and nested actual MIDI/state-field deduplication. Compile-fail
docs reject raw adoption and mutation. An existing audio allocation harness probes
128 repeated paired-body/model/MIDI/state inspections after ledger reservation;
its result must be executed on a supported constructor build before claiming
zero allocation/free. No native visual/listening or playable Sampler claim follows.
