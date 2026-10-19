# native device presets

Pulse #2375 / canonical draft PR187 introduces save/load controls in each supported
native device header. Hover the down arrow for `save device preset` and the up
arrow for `load device preset (.hex-device-preset.json)`. These files contain the
complete device state, distinct from built-in Operator routing choices and project
documents. External plugin hosting is outside this workflow.

Project-scoped device asset references require a portable manifest and remapping
before they can be saved as presets. The W10 reference slice explicitly refuses
such preset encoding before file publication; it never saves unusable project
asset IDs or silently strips bindings. Project schema30 can preserve them in
place. See [device assets](device-assets.md) for the current boundary.

Saving suggests `untitled.hex-device-preset.json`; choosing another name appends
the suffix when absent. Save creates a new complete file and refuses any existing
destination, including a publication race. Choose another filename on collision.
If publication succeeds but directory durability or temporary cleanup fails, the
error explicitly says the complete preset exists; it does not claim the file is
absent. Dialog cancellation performs no preset I/O or preset history operation.

The current GPUI path-prompt API exposes a descriptive prompt but no extension
filter. The load prompt names `.hex-device-preset.json`; selected contents are
strictly validated on the control worker regardless of filename. Native dialog
filtering would require a separately coordinated platform API change.

## format and compatibility

The independent JSON envelope has `format: "hex-device-preset"`, `schema: 2`,
`class` (`instrument` or `effect`), `type_id`, `state_version`, and a complete
opaque byte-array `payload`, and required nullable `comparison`. A comparison
records active slot `a`/`b` and one inactive type id, state version, opaque payload
and empty asset-reference list. Both slots use the same provider; mismatched
device-global host trims refuse before encoding. It contains no project identity, device identity,
neighbor ordering, bypass, host input/output trims, or external assets. Unknown envelope fields, formats,
schemas, classes, malformed type identifiers, version zero, and oversized payloads
are rejected. The existing 4096-byte native-state and 128-byte type-id limits are
unchanged. The file limit derives from worst-case compact byte-array JSON plus
bounded metadata (33536 bytes for both bounded states); extra whitespace is allowed within that bound.

Loading requires the target's exact native class and type. Its existing provider
must validate the state and support its version; newer/unknown/malformed native
states refuse before mutation. Existing provider-supported legacy migrations
remain authoritative. Presets preserve original payload bytes and native state
version rather than re-encoding a migration or stripping unsupported content. Host-owned
input/output trims stay at the target's exact millidB values during load and Undo/Redo;
they are not part of the native opaque-payload envelope.
Envelope schema 2 is the current format; other envelope versions refuse explicitly.

Dialogs capture project generation, canonical revision, stable device identity,
class and type. A changed owner or revision cancels the request with a retry
message before file I/O. Finish any pending edits before opening the dialog;
invalid numeric drafts refuse. If a valid draft is still applying, wait for it
and open the dialog again. The worker owns all preset I/O. A compatible load uses
one `ReplaceDeviceComparison` prepared transaction and existing live admission; failed
admission preserves canonical state, history and audio. Undo restores the previous
complete device state; Redo restores the preset. Save alone does not dirty or
change the canonical project. Deferred preset requests are discarded if the active device gesture fails to finalize; the visible refusal remains and no preset I/O is submitted.

## original acceptance retained

- Discoverable Operator and supported generic native-device save/load workflow.
- Exact oscillator enable flags, levels, tuning, envelopes, routing and supported
  LFO state preservation; one canonical Undo/Redo and existing live ownership and
  admission guards.
- Strict compatibility; actionable malformed/missing/incompatible/oversized file,
  stale-owner and save-collision errors; cancellation and no silent overwrite.
- Exact checked source identity, optimized native build and user save/reload/Undo
  confirmation. Linux local verification does not establish macOS dialogs,
  CoreAudio behavior, listening or hardware deadlines.

PR187 retains original-history integrations of frozen PR164
`25c1cdf00d1ab22f8fe6743aa4bc5f43cb0f1971` (complete Operator v8 enable state)
and PR186 `e00397a2cc4647806dd4c2f4e8035a5692e4f2b3` (untouched numeric value
finalization and retired preview authority). These fixes are consumed from their
canonical histories, with no replacement codec or duplicate implementation.
Preset regressions retain mixed enable flags alongside non-default levels,
tuning, envelopes, signed routing and supported LFO state, including a freely
positioned instrument between two effects. Native acceptance remains pending.
This draft remains blocked under the readiness hold; no passing gate or receipt
is registered.

## current-main integration

The current candidate retains original PR164 enable-state and PR186 preview/draft
histories while integrating PR186 `4286997a495b857123874ca59578e8fa99506313`
(including main `5785bf7c47c5c8a168cfec33bfe1ecbe3aa72c1c`). Header actions
share the current drag-title surface; there is no second drag grip. Oscillator
badges retain canonical enable toggles and current controls-first focus. Invalid
current previews still refuse, and untouched numeric display rounding stays out
of preset finalization and project history.

Capture and device metering retain separate ownership. One shared predicate
identifies the meter-only analytic path whose output already includes track mix;
both Main and capture use that decision, avoiding a second gain/balance pass.
Independent asymmetric-channel, partition, hybrid prefix/suffix, nonlinear send/
return, inherited-release and late-refusal regressions cover the integration.
This does not add host musical typing; PR190 runtime evidence remains separate
and is not a prerequisite invented for the preset workflow. Optimized macOS
preset acceptance must use the complete integrated candidate, not an older bundle.

## current-main integration — 16 october 2026

The focused service/storage/dialog/header wiring is restored after current main
excluded the inherited draft preset workflow. Current routing, compensation,
shared widget styling, input/output metering and capture mix predicates remain
authoritative. Service regression state uses the current Operator version-11
416-byte payload, retaining the filter, enable and LFO state. Both host gain trims
remain target-owned through load and one Undo/Redo.

The nonzero input-trim regression reproduced a reset to unity before repair.
The actual worker compile-refusal fixture reproduced preset requests remaining
queued after failed finalization; repaired retirement submits no preset I/O.
Focused storage/service/rendered-widget results and final exact-source full CI
are recorded with PR187. Historical Mac evidence applies to its earlier candidate;
final-source native dialog, input and listening acceptance remain open.

### Comparison-only authoring and audio correlation

Copy and an identical-state Switch still change canonical comparison history
when the authored pair changes. The Session worker classifies that exact typed
transition (and its exact Undo/Redo inverse) inside the existing prepared Editor
transaction. Only when the complete active provider state, asset bindings, host
metadata and every other playback field remain equal does it retain the same
prepared revision and canonical audio-source snapshot. An audible Switch uses
ordinary graph preparation and admission.

`canonical_audio_source` describes the actual preparation source, whose captured
Project and context can precede the current authoring revision. A private worker
binding correlates comparison-only authoring revisions to that exact retained
source; it does not grant constructor custody or pretend that an audio ACK
occurred. Save/status reconstruction retains this binding. Pending audio stays
pending until its real terminal result, and refused audio stays refused. UI
correlation also requires the actual current output/candidate association before
an applied authoring revision can advance. Later audio edits establish a new
source and leave late old ACKs historical. Existing keyboard input can continue
without replacing its lease; stale device-slot reservations still refuse.
Every comparison enqueue invalidates arrangement seek through the existing path.
