# native device presets

Pulse #2375 / canonical draft PR187 introduces save/load controls in each supported
native device header. Hover the down arrow for `save device preset` and the up
arrow for `load device preset (.hex-device-preset.json)`. These files contain the
complete device state, distinct from built-in Operator routing choices and project
documents. External plugin hosting is outside this workflow.

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

The independent JSON envelope has `format: "hex-device-preset"`, `schema: 1`,
`class` (`instrument` or `effect`), `type_id`, `state_version`, and a complete
opaque byte-array `payload`. It contains no project identity, device identity,
neighbor ordering, bypass, or external assets. Unknown envelope fields, formats,
schemas, classes, malformed type identifiers, version zero, and oversized payloads
are rejected. The existing 4096-byte native-state and 128-byte type-id limits are
unchanged. The file limit derives from worst-case compact byte-array JSON plus
bounded metadata (16768 bytes); extra whitespace is allowed within that bound.

Loading requires the target's exact native class and type. Its existing provider
must validate the state and support its version; newer/unknown/malformed native
states refuse before mutation. Existing provider-supported legacy migrations
remain authoritative. Presets preserve original payload bytes and native state
version rather than re-encoding a migration or stripping unsupported content.
Envelope schema 1 has no migration to other envelope versions.

Dialogs capture project generation, canonical revision, stable device identity,
class and type. A changed owner or revision cancels the request with a retry
message before file I/O. Finish any pending edits before opening the dialog;
invalid numeric drafts refuse. If a valid draft is still applying, wait for it
and open the dialog again. The worker owns all preset I/O. A compatible load uses
one `ReplaceDeviceState` prepared transaction and existing live admission; failed
admission preserves canonical state, history and audio. Undo restores the previous
complete device state; Redo restores the preset. Save alone does not dirty or
change the canonical project.

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
