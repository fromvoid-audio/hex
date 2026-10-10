# Persistent four-color theme settings

Pulse ticket 15 owns this focused change. Four opaque RGB roles configure the
native editor: background (`#2a2828`), text (`#c3c5c0`), primary accent
(`#f06449`) and secondary accent (`#70e8e3`). Derived surfaces, dividers, grid,
selection, hover and device-control colors follow these roles. Existing black
clip borders and filled waveform silhouettes remain intentional exceptions.
Lowercase text, bundled Ioskeley Mono and accepted compact geometry are preserved.

## Controls and ownership

The native macOS hex > settings… menu (Command-comma) opens a compact modal.
Repeated activation focuses the existing draft. The toolbar has no settings button. Each
color has a swatch, hex readout and RGB sliders with direct numeric channel
editing. Values are bounded to 0–255. Valid changes preview immediately; Reset
restores shipped defaults, Cancel/Escape restores the palette present when the
panel opened, and Save persists the validated draft. Focus returns to the editor
after cancellation or successful asynchronous saving. Keyboard routing follows
actual modal state, including when a numeric field owns focus.

Background/text require 4.5:1 contrast; accents require 3:1 against background.
An invalid draft remains editable with a visible explanation while the last
legible preview remains active. Derived clip/button foregrounds choose readable
black or white when the preferred text role lacks contrast; saved roles are not
rewritten. These checks are source-level usability policy, not complete
accessibility certification.

`hex-preferences` owns bounded, versioned serialization and atomic publication
at `~/Library/Application Support/hex/theme.json`, independent of project files,
browser preferences and recent projects. UI load/save runs on a background
executor with generation checks and at most one save in flight. Failed saves
retain an open editable draft and an actionable error. Window close/quit refuses
while saving; closing an unsaved preview cancels that preview before normal
project-close handling. Theme edits do not enter project history or dirty state.

Missing settings use defaults. Malformed/future settings remain untouched and
cannot be silently overwritten. A contrast-invalid saved palette remains on
disk and becomes an editable draft while usable default chrome is displayed.

`hex-ui::theme` replaces a complete immutable, UI-thread-local token snapshot
when preferences change. Paint accesses cached colors without locks or file I/O;
audio/project workers have no theme dependency. Existing device widget entities
receive changed styles without replacing active numeric drafts or gestures.
`hex-widgets` receives caller-owned colors and has no application theme dependency.

## Evidence and remaining acceptance

Focused verification passed: 317 UI tests, 19 widget tests, 16 preference tests,
strict affected Clippy, formatting and architecture/UI-style/GPUI-patch guards.
Regressions cover preview/cancel/focus, successful persistence, failed-save
recovery, contrast-invalid startup preservation, close during save, Escape from
focused numeric controls and injected text-selection tint. Full clean local CI
follows integration; no native macOS interaction has been claimed.

The user's native checklist for the combined build:

- Open settings from the native menu or Command-comma; adjust all four roles and check tracks,
  selections, browser, status and an open Utility device update immediately.
- Type an RGB channel, drag another, then press Escape; previous colors and
  editor shortcuts should return. Invalid contrast should leave controls readable.
- Save a valid palette, quit and relaunch; colors should persist independently
  of the project. Reset then Save restores the shipped palette.


Menu access follow-up (Pulse2115): removes the in-window settings button and adds
the native application-menu action and global Command-comma binding. Existing
preview/persistence tests exercise that action through keyboard dispatch. The user
accepted theming on 9 October 2026; native menu access remains a separate check.
