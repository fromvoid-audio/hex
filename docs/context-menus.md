# context menus

The reusable GPUI popup in `crates/hex-ui/src/context_menu.rs` owns presentation:
compact 24px rows, rectangular 210px panel, fine border, dark drop shadow,
window-edge clamping, typed item actions and keyboard/pointer routing. The
caller owns the command implementation and menu state. No native assets copied.

The first caller is Arrangement track insertion. Right-click anywhere in a
track row to insert a hybrid track below it; right-click the arranger empty area
to append. There is one lowercase item: `insert track`. No audio/MIDI type menu.
Arrow keys select rows; Enter activates; Escape or an outside click dismisses
and restores the prior focus. Editor shortcut context is suspended while open;
underlying timeline wheel, pan and seek capture must not leak through.

`hex-project::Command::InsertTrack` validates a stable optional before-track ID,
name and track limits, and creates a default empty hybrid track. Session's
existing candidate compilation/history pipeline remains authoritative. One
successful insertion is one undo step; no schema fields were added. Menus bind
the project generation at opening and refuse removed insertion anchors.

Automated checks cover click activation, dismissal, keyboard selection, screen
clamping, ordering, append, stale anchors and undo/redo. Native visual acceptance
is pending. Track-header color swatches and clip-flavor toggles now also serve Arrangement
and Session; see [element colors](element-colors.md). Other callers can reuse
the popup without duplicating presentation.

Verification: 747 workspace tests passed, 1 ignored; 239 UI tests
passed, 1 ignored. Strict UI/project lint, desktop build, formatting and
architecture/UI-style/GPUI guards passed. Separate review bundle:
`artifacts/hex-waveform-menu-20261009.app`; source/binary/check identity:
`artifacts/waveform-menu-manifest.json`. User checks: visible waveform while
zooming/resizing; right-click track/empty space; insert position; Undo/Redo;
Escape and outside-click dismissal. Running apps and unsaved work preserved.
