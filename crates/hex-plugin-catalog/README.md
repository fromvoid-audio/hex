# hex-plugin-catalog

## Purpose

Bounded control-side persistence and selected-bundle discovery coordination for
VST3 browser metadata. Owns a recoverable current-format catalog, copied records,
last-good/error projection and the outer standalone scanner's control lifetime.
All filesystem/process work runs on a background control worker.

## Dependencies

`hex-plugin-scan` supplies copied metadata, fixed package identity and admission
checks. Pinned serde/serde_json encode bounded current-format records. Pinned
fs2 supplies nonblocking advisory writer exclusion, with locks held only during
catalog read-modify-publication, never across foreign-code execution.

## Does not own

No DSP, ordinary plugin instances, native ABI, process-global signal handlers,
GPUI, project edits or audio callback work. Does not launch Hex, load foreign
libraries in the application, scan folders automatically or authenticate plugins.
The standalone scanner retains its own native helper; outer cancellation uses
stdin and keeps draining/reaping, never a process-group kill.


The first workflow scans one explicitly selected bundle. Startup restores saved
metadata without executing code. A catalog holds at most512 modules/2048 classes
and4MiB of JSON; paths are bounded to4096 bytes. The worker drains at most4MiB of
report output and64KiB of outer diagnostics in fixed polling batches, with one
active operation per browser. Its cancellation token has an atomic publication
boundary: cancellation wins until catalog publication begins; later requests
report finishing instead of falsely claiming cancellation. Accepted native metadata requires direct helper retirement before that boundary. A stuck native cleanup retains its outer
scanner/control owner rather than authorizing a process-group kill.

The explicit standard-folder action uses the user's `Library/Audio/Plug-Ins/VST3`
and the local `/Library/Audio/Plug-Ins/VST3`. Missing roots are empty; invalid,
linked root components and traversal errors refuse the batch before code runs.
Traversal never enters bundles or follows encountered links; skipped links have a
visible count. Bounds are two roots,4096 entries,1024 directories,8 levels of
folder recursion and512 bundles, with4096-byte/64-component paths. The larger
module bound covers an observed318 installed bundles; class/JSON/output limits
remain fixed, with a320-module/640-class save/reload fixture. Enumeration alone
does not load vendor code or establish compatibility.

A batch retains at most one owned outer scanner/native helper chain at a time.
Normal per-module failures are recorded alongside successful modules; missing or
unconfirmed direct retirement stops the batch before another launch. This does
not claim cleanup of foreign descendants, which remains unknown. Pending copied
reports are validated against class/JSON limits and diagnostics are discarded.
Cancellation discards all pending batch results; publication has one atomic token
boundary followed by a fresh writer-locked merge. Capacity/write/validation errors
preserve the prior file. No missing/stale/duplicate record is deleted here, and
no scan runs at startup. The UI has one busy/cancel owner for either scan action.

Rows classify copied optional Factory2 subcategory tokens exactly: `Instrument`,
`Fx`, or both (the SDK explicitly permits `Fx|Instrument`). Missing or unrecognized
type tokens remain unknown; names, substring matches and flags never infer a type.
Nonempty class vendor metadata overrides the factory vendor. Type/vendor search
and restart retain original metadata bytes. These are reported discovery labels;
no row is a playable or insertable device. Custom roots remain separate.
Cache data never authorizes future plugin instance admission; the processing host
must verify current source and resources. Catalog version2 is the current format;
unsupported older alpha files remain intact with a visible error, without migration.
The native integration tests require an explicit signed original package through
`HEX_VST3_CATALOG_TEST_APP` and original fixture directory through
`HEX_VST3_CATALOG_FIXTURES`; ordinary runs keep these tests ignored. They do not
start the Hex application or establish physical device/listening acceptance.

Dynamic/discardable or unknown factory flags retain historical metadata only;
rows explicitly require a rescan. Stored inventory never establishes future
cacheable class availability or authorizes processing admission.
