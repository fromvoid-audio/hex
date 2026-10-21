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
metadata without executing code. A catalog holds at most64 modules/2048 classes
and4MiB of JSON; paths are bounded to4096 bytes. The worker drains at most4MiB of
report output and64KiB of outer diagnostics in fixed polling batches, with one
active operation per browser. Its cancellation token has an atomic publication
boundary: cancellation wins until catalog publication begins; later requests
report finishing instead of falsely claiming cancellation. Native objects have
already retired before that boundary. A stuck native cleanup retains its outer
scanner/control owner rather than authorizing a process-group kill.

Rows classify copied optional Factory2 subcategory tokens exactly: `Instrument`,
`Fx`, or both (the SDK explicitly permits `Fx|Instrument`). Missing or unrecognized
type tokens remain unknown; names, substring matches and flags never infer a type.
Nonempty class vendor metadata overrides the factory vendor. Type/vendor search
and restart retain original metadata bytes. These are reported discovery labels;
no row is a playable or insertable device. Standard/custom roots remain separate.
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
