# Temporary Takeover Tracks — Architecture & Routing Design Brief (Pulse #3246)

Status: Planning / contract specification (no implementation dispatched by this document).
Related: `#2756` (reusable track sources / independent instrument manager), `#3245` (whole-track bounce), `docs/design/live-repatching-brief.md`.

## 1. Confirmed Product Decisions (Approved, 25 October 2026)

1. **Temporary takeover semantics**: A fork track is a **temporary takeover track**, not a parallel send or destructive clip split. During a fork's active timeline interval, audio from the parent track routes through the fork's alternate processing chain and the parent's direct audible output for that interval is suppressed. There is never sustained dry-signal doubling.
2. **Return to normal path**: Outside active takeover intervals, the parent's direct audible path resumes unchanged. Original clips, instrument, source ownership, and parent processing remain intact.
3. **Single live upstream instrument**: An upstream instrument on the parent track stays one live instance (per `#2756`). Takeover branches audio downstream; it never duplicates or retriggers the instrument and never disrupts unrelated tracks sharing the same source.
4. **Multiple sequential forks**: A parent track may branch to multiple fork tracks across non-overlapping (or explicitly ordered) intervals without duplicating the upstream instrument.
5. **Arranger visual feedback**:
   - Dim/grey the exact timeline intervals on the parent track whose direct audible path is suppressed by an active takeover (not the entire track, and visually distinct from deactivated source clips).
   - Keep clip names, waveforms/notes, boundaries, selection, and focus readable.
   - Render a compact directional routing cue linking parent -> fork -> return to parent, aligned across zoom, scroll, track reorder, collapse, and resize using cached visible geometry and lowercase `theme::text`.
   - Reflect canonical effective routing (after overlap resolution) and update atomically with Undo/Redo, cancellation, removal, and project reload.

## 2. Proposed Canonical Contracts (To Confirm Before Implementation)

### 2.1 Tap Point, Mix & Latency Ownership (`hex-project` + `hex-engine`)
- **Proposed default tap point**: **Post-instrument / pre-parent-effects** (with optional per-fork tap selector: `PreEffects` vs `PostEffects`, default `PreEffects` so the fork applies genuinely alternate processing from the dry instrument/clip output without double-processing parent effects; `PostEffects` allows adding a temporary delay/reverb onto the parent's existing chain).
- **Mix, sends, automation, and mute/solo**:
  - During takeover, the fork owns post-chain gain, pan, sends, and automation for the branched signal.
  - Parent mute suppresses the upstream source feed if the parent is muted (unless the fork is soloed, in which case solo overrides parent mute for the upstream source feed without unmuting the parent's direct path outside the takeover interval).
  - Outside takeover, the parent owns all mix/sends as usual.
- **Latency & PDC**:
  - `hex-engine::signal_graph` compiles a single static worst-case PDC latency across the parent and all attached fork branches (`max(parent_chain_latency, fork_chain_latency)`), aligned via existing `hex-engine::compensation` rings so entering/exiting a takeover interval never shifts timing or requires a live graph restart.

### 2.2 Topology, Identity & Overlap Policy (`hex-project`)
- **Topology**: Sibling forks from one parent track (depth = 1 for v1) with explicit `TakeoverRoute { parent: TrackId, tap: TakeoverTap, intervals: Vec<TakeoverInterval> }` owned by the fork track.
- **Cycle rejection**: `hex-project` validates acyclicity alongside group routing and sidechain routes; a fork cannot target itself, a group ancestor, or another fork in v1 (bounded depth = 1; serial nested forks deferred to a documented v2 extension once depth-1 is proven).
- **Overlapping intervals**: Explicit **refusal on edit** in `hex-project` (`Invalid("takeover intervals overlap on parent track")`) rather than silent precedence, guaranteeing deterministic single-route audibility with zero hidden competing dry paths.
- **Deletion & recovery**:
  - Deleting a fork track removes its takeover intervals and restores the parent's direct path atomically (one Undo step).
  - Deleting a parent track either refuses while forks reference it or converts forks to empty unrouted audio tracks in one undoable transaction (proposed: unlink fork `TakeoverRoute` -> `None` cleanly in one Undo step, matching sidechain pruning on device/track removal).

### 2.3 Boundary Crossfades, Tails & Playback Semantics (`hex-engine` + `hex-audio`)
- **Click-free transitions**: Entering and exiting each `[start, end)` takeover interval applies a deterministic, sample-accurate 5 ms equal-power / linear complementary crossfade (`w_parent + w_fork == 1.0`, reusing `TrackFader::fade_frames(sample_rate)`). A unity-gain, empty-chain fork sums to exact unity (`1.0`) across the entire boundary with zero level bump or dip.
- **Effect tails after `end`**: At `frame >= end`, the fork's input gate closes (`0.0`) while the fork's effect chain continues draining its natural reverb/delay tail (`OutputTail`) into Main/Returns alongside the resumed parent path. Only dry input is exclusively switched; wet tails never cut abruptly.
- **Seek / Loop / Export / Bounce**:
  - Arrangement seeks and loop wraps compute the active takeover state directly from canonical content frames (no stateful drift).
  - Offline export, stem export, and bounce (`hex-export`, `hex-session`) evaluate the exact same compiled `hex-engine` signal graph: Main mix reflects seamless takeover; parent stem contains only non-taken-over intervals; fork stem contains the taken-over audio plus its wet tail.
  - Real-time callback invariant: zero allocations, zero mutexes, zero dynamic graph rebuilds when crossing takeover boundaries during playback.

### 2.4 Persistence & Saved-Project Compatibility (`hex-project` + `hex-storage`)
- Adding `TakeoverRoute` to saved track snapshots requires a **fresh schema version** in `hex-storage` with explicit forward migration from prior supported schemas (defaulting `takeover: None`) and strict rejection of newer/malformed schemas per `AGENTS.md`.
- Round-trip, historical-fixture, and Undo/Redo tests must ship in the same PR as the schema change.

### 2.5 Arranger UI & Visual Cues (`hex-ui` + `hex-ui-kit`)
- Read-only projection from canonical `Project` state (`effective_takeover_intervals(parent_track)`):
  - Dimmed overlay rects over `[start, end)` on the parent track lane using a dedicated theme token (`PaletteRole` takeover dimming) that keeps waveforms/notes/clip labels legible and distinct from muted/deactivated clips.
  - Compact directional routing cue rendered from cached visible track-row Y coordinates and timeline X projection (clipped to viewport; collapses cleanly when either track is offscreen/collapsed).
  - Zero per-frame full-project scans; updates in lockstep with acknowledged editor state and Undo/Redo.

## 3. Recommended Boundary-Scoped Implementation Breakdown

Each slice stays within 1–2 boundaries (`< 1s` unit tests, `<= 3m` merge verification):
1. **Slice 1 (`hex-project` + `hex-storage`)**: `TakeoverRoute` / `TakeoverInterval` model, typed commands (`CreateTakeoverTrack`, `SetTakeoverIntervals`, `RemoveTakeover`), overlap & cycle validation, parent/fork deletion pruning, Undo/Redo, fresh storage schema + migration + historical fixtures.
2. **Slice 2 (`hex-engine`)**: Signal-graph tap compilation, static PDC alignment across parent + fork branches, sample-accurate 5 ms complementary boundary crossfade, wet tail drain past `end`, seek/loop/unity-fork PCM oracles.
3. **Slice 3 (`hex-session` + `hex-audio` + `hex-export`)**: Live playback handoff, zero-allocation callback probe across boundary crossings, Main + parent/fork stem export & bounce verification.
4. **Slice 4 (`hex-ui` + `hex-ui-kit`)**: Parent interval dimming overlay, compact directional routing cue, interval handle editing, zoom/scroll/reorder/collapse/Undo/Redo GPUI tests, plus user native visual/listening acceptance checklist.
