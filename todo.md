## stable waveform resize sampling — pulse2764 / PR372, 17 october 2026

Arrangement clips and ordinary/lower-fade resize previews use a bounded shared
viewport sampling lattice instead of redistributing all peak columns across
each changing clip width. Actual interior stereo outline geometry fails under
the former sampling and passes with fixed lane/time density across fractional
edges, zoom scales and the column cap; canonical loop/one-shot trims are covered.
Project/source mapping, fade envelopes, one Undo and audio paths remain owned by
the existing model. Native reproduction/acceptance is pending; the running
source label is0f99bfbc, with no captured resize. See docs/waveform-display.md.
All20 full cloud checks passed on8a787a2b; final main integration is revalidated
and recorded in the PR/Pulse. Launcher368 is merged; no app delivery performed.

## tuning review precision and cyclic gaps — #2739 / PR350, 17 october 2026

Review found preset float rounding and a missing period-seam gap check. Four
regressions fail before repair and pass afterward: exact authored-float portable
round trips, construction/drag seam refusal, safe clicked insertion, and wrapped
insertion/count changes retaining the root. All23 tuning tests and strict scoped
Clippy pass. Existing Rust1.85 baseline stays unchanged. The previous full-run
checkpoint is superseded; final exact-source integration checks remain required.
Extended existing fractional-root and combined device-manifest fixtures also
reproduced a one-ulp JSON parser error. The pinned serde_json dependency now
enables float_roundtrip, preserving current-format authored root precision.
Both owning storage fixtures pass after the repair, including all device owners.

## combined current project format — #2739 / PR350, 17 october 2026

After asset PR297 landed, cloud5 integrates both active capabilities in one
schema31: required complete tuning and device-asset manifest for every project,
including empty/default values. Prior alpha30 refuses explicitly without
conversion, rewriting or deletion. The manifest is a reusable bounded current
payload, not a historical30 dispatch reader. Existing older readers stay intact;
legacy writers continue to refuse data loss. Combined non-octave tuning/assets,
Undo/Redo, strict malformed/duplicate manifests and untouched-file refusal are
covered by the owning storage fixtures. All storage/project/tuning tests and
strict affected all-target Clippy pass. Source20cd1de3 passed all20 local checks
with stale=false; checks for the current main integration remain required. No historical30 reader or automatic user-file conversion was added.

## track header width and row height — pulse2714 / PR330

User prioritized launcher cloud1 takeover after spinner acceptance. Canonical
cloud6 checkpoint49eedce9 and its history are retained. Track-header width now
shares one bounded view value across timeline consumers; individual regular
bottom edges resize aligned header/lane rows with captured displacement and
cancel semantics. Main width works with no tracks. View-only sizing preserves
project/audio/Undo state; variable rows retain logical scrolling and reset on a
new project generation. Five rendered gesture/bounds/control/scroll regressions cover the new behavior.
Required exact final source checks and remaining native acceptance are tracked in
Pulse2714; no app delivery is claimed. See docs/ui-design.md for derived layout bounds.
