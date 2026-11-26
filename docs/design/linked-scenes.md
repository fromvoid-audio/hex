# Linked Arrangement & Console Scenes Contract (`#3441`)

Source baseline: `origin/main` (`01c2ed732`).
Owning prerequisites and consumers: `#3441` -> `#3442` (`hex-project`, `hex-storage`, `hex-session`) -> `#3443` & `#3444` (`hex-ui`) -> `#3104` -> `#3414`.

## 1. Identity Model: Definition vs Placement

- **Shared definition (`SceneId`, owned by `hex-project/src/session.rs`)**:
  - A Console `Scene` is the single canonical definition for a scene's `id: SceneId`, `name: String`, optional `follow_action: SceneFollowAction`, and sparse Session cell clips (`(TrackId, SceneId) -> SessionClip`).
- **Arrangement placement (`SectionId`, owned by `hex-project/src/arrangement_sections.rs`)**:
  - An `ArrangementSection` represents one disjoint half-open beat interval `[start, end)` on the Arrangement Main track, with its own `id: SectionId`, `name: String`, `color: ElementColor`, and an explicit optional link `linked_scene: Option<SceneId>`.
  - Standalone Arrangement sections have `linked_scene: None`.
  - Linked Arrangement scenes have `linked_scene: Some(scene_id)` referencing an existing `SceneId` in `Project::scenes()`.

## 2. Repeated Placements

- Multiple `ArrangementSection` placements may reference the same `SceneId` (e.g., `chorus` placed at bars 9–17 and bars 25–33).
- Each placement retains its own unique `SectionId`, `start`, `end`, and `color`, while sharing the canonical `SceneId` link.
- Dragging a Console scene onto the Arrangement Main track (`#3444`) creates a new `ArrangementSection` with `linked_scene: Some(scene_id)` at the target beat span (defaulting to the longest active cell length in that scene, or 4 bars / 16 quarter notes when empty, clamped/validated against existing disjoint-section rules).

## 3. Edit Propagation Rules

| Edit | Propagation | History / Undo |
| --- | --- | --- |
| **Rename Console `Scene`** | Propagates to every `ArrangementSection` with `linked_scene == Some(scene_id)` in the same canonical command. | One Undo step restores prior scene name and all linked section names. |
| **Rename linked `ArrangementSection`** | Updates the linked Console `Scene`'s name AND all sibling placements referencing that same `SceneId`. | One Undo step restores prior names atomically. |
| **Recolor linked `ArrangementSection`** | Local to the placement (`SectionId`) unless explicitly unified later; preserves existing section color reuse laws. | One Undo step. |
| **Resize / Move / Ripple linked `ArrangementSection`** | Uses existing `arrangement_sections` cut/insert/ripple laws; preserves `linked_scene` on surviving moved/resized/split left fragments; splitting assigns a fresh `SectionId` retaining `linked_scene` on both halves. | One Undo step. |
| **Delete linked `ArrangementSection`** | Deletes only that Arrangement placement (`SectionId`); the Console `SceneId` definition and any sibling placements remain intact. | One Undo step restores placement and link. |
| **Delete Console `Scene`** | Unlinks every `ArrangementSection` whose `linked_scene == Some(scene_id)` (`linked_scene = None`), converting them to standalone Arrangement sections without deleting Arrangement clips, audio, or markers. | One Undo step restores the Console scene, its cells, and all section links. |
| **Explicit unlink (`Command::UnlinkSectionScene`)** | Clears `linked_scene` on one `SectionId` while retaining its name, range, color, and Arrangement content. | One Undo step. |
| **Edit Session cell content in Console** | Modifies canonical `Scene` cells owned by `hex-project::session`. Live Session playback follows `hex-session` prepare-before-commit; Arrangement audio/MIDI clips remain independent until explicitly bounced/placed so destructive overwrites never occur silently. | One Undo step per canonical cell edit. |

## 4. Reciprocal UI Workflows (`#3443` & `#3444`)

1. **Right-click Arrangement scene -> `edit scene in console` (`#3443`)**:
   - Enabled only when the targeted `SectionId` has `linked_scene: Some(scene_id)` and that `scene_id` exists in the current `Project` snapshot.
   - Switches view focus to Console and sets `selected_scene = Some(scene_id)` without launching transport, firing clips, mutating project state, or creating/duplicating scenes.
   - Missing, unlinked, stale-revision, or replaced-project targets refuse truthfully and preserve current focus/selection.
2. **Drag Console scene -> Arrangement Main track (`#3444`)**:
   - Hit-tests only the Arrangement Main track lane (explicitly NOT the Main mix bus, return tracks, or ordinary audio/MIDI tracks).
   - Captures `(project_generation, revision, scene_id)` at drag start; holds frozen placement preview via `pending_edit` until exact canonical acknowledgement or refusal.
   - Dispatches `Command::PlaceLinkedScene { scene, start, end, color }` as one atomic undoable command; cancel, invalid target, overlap refusal, or project replacement restores prior UI and project state untouched.

## 5. Persistence & Migration Policy (`#3442`)

- Assign a fresh schema version in `hex-storage` (and capability entry) storing optional `linked_scene_id: Option<u64>` on each serialized Arrangement section.
- Historical projects without `linked_scene_id` decode losslessly with `linked_scene: None`.
- Reject dangling `linked_scene_id` (pointing to a non-existent `SceneId`) during `Project::adopt_snapshot` / storage decode.
- Preserve original project bytes on disk until deliberate save; surface save-upgrade confirmation when saving a newly linked project over an older schema file.

## 6. Explicit Non-Goals & Unsupported Combinations

- Does not implicitly overwrite existing Arrangement audio/MIDI clips when editing a Console cell.
- Does not create linked placements on non-Main tracks or on the Main mix bus.
- Does not alter launched Session playback routes merely by navigating via `edit scene in console`.
- Native drag/interaction and listening acceptance remain separate user-run verification.
