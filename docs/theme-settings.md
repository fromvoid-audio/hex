# Persistent four-color theme settings

Pulse ticket 15 owns this focused change. Four opaque RGB roles configure the
native editor: background (`#2a2828`), text (`#c3c5c0`), primary accent
(`#f06449`) and secondary accent (`#70e8e3`). Derived surfaces, dividers, grid,
selection, hover and device-control colors follow these roles. Existing black
clip borders and filled waveform silhouettes remain intentional exceptions.
Lowercase text, bundled Ioskeley Mono and accepted compact geometry are preserved.

## Controls and ownership

The native macOS hex > settings… menu (Command-comma) opens a compact modal.
Repeated activation focuses the existing draft. The toolbar has no settings button.
Four larger role swatches select background, text, primary or secondary. A shared
saturation/brightness field and rainbow hue strip edit the selected role. Hue,
saturation and brightness each have a slider and direct numeric value; the existing
RGB sliders and hex readout show the same opaque color. Hue is retained while a
color is gray or black, so raising brightness or saturation can recover its chosen
hue. RGB/HSV conversions round to 8-bit channels without repeatedly converting
the saved RGB through HSV. RGB values are bounded to 0–255, hue to 0–359 degrees,
and saturation/brightness to 0–100 percent. Valid changes preview immediately; Reset
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

## Shared device semantic styles — pulse2471

`theme::device_style` resolves Control, Layout, Display and Readout roles used by
production panels and available to native galleries. Gallery migration remains
Pulse2474. All neutral shell/header, section, rail,
display, readout, hover, disabled text, focus marker and dial rail colors follow
the existing palette roles. Inline readout appearance depends on presentation,
never a device type ID. The idle header/surround share the Layout surface;
Pulse2472 resolves selected-header paint separately through
`theme::device_frame_style`, including its contrast-checked foreground. Outer
border/surround and caller-owned body/control palettes remain neutral.

The a/b/c/d oscillator colors remain stable musical identity cues, centralized
by `oscillator_identity_color`: yellow, turquoise, blue and orange. Badges and
routing diagrams use the same mapping (the routing stack reverses its order).
They are intentional identity accents, not device-owned neutral skins. Badge
foregrounds choose a readable color against each identity color; disabled badges
use the active display/muted roles. No extra preference or saved device state is
introduced.

Retained numeric, toggle and selector entities receive style changes in place;
Operator view-only preview fields/toggles, EQ audition and return-send controls
now refresh too. Response graphs already include style in retained model updates;
selector popups paint from their current entity style. Refresh does not recreate
entities, finish drafts, cancel gestures or alter canonical parameter values,
project history, focus ownership or tab order.

Focused source/scene tests contrast dark and light palettes, retain active
canonical and preview numeric drafts and focus, and keep an open selector popup
and its navigation state while refreshing its palette. Automated verification
results are recorded in `todo.md`; optional native visual/input acceptance still
requires the user's optimized build. No automated native computer interaction is
claimed.

## Evidence and remaining acceptance

Focused verification passed: 317 UI tests, 19 widget tests, 16 preference tests,
strict affected Clippy, formatting and architecture/UI-style/GPUI-patch guards.
Regressions cover preview/cancel/focus, successful persistence, failed-save
recovery, contrast-invalid startup preservation, close during save, Escape from
focused numeric controls and injected text-selection tint. Full clean local CI
follows integration; no native macOS interaction has been claimed.

Picker implementation evidence (Pulse2116): HSV conversion tests cover primaries,
hue wrapping, non-finite/range rejection, gray/black hue retention and exact RGB
round trips for representative palette colors. UI tests cover role selection,
dragging the field through release outside its bounds, synchronized HSV/RGB preview,
direct hue entry, contrast-invalid preview retention, Reset, Cancel, Save and
preference reload. All 26 widget tests and 7 focused theme UI tests pass. A rendered
GPUI fixture resized to the editor minimum of 880×540 verifies that each role
swatch, picker surface, HSB/RGB control and modal action fits inside the panel.
Strict widget/UI Clippy and formatting/style/architecture/GPUI-patch guards pass;
native macOS layout and interaction remain for user review.

The user's native checklist for the combined build:

- Open settings from the native menu or Command-comma; adjust all four roles and check tracks,
  selections, browser, status and an open Utility device update immediately.
- Type an RGB channel, drag another, then press Escape; previous colors and
  editor shortcuts should return. Invalid contrast should leave controls readable.
- Save a valid palette, quit and relaunch; colors should persist independently
  of the project. Reset then Save restores the shipped palette.
- Select all four larger swatches; drag the saturation/brightness field and hue
  strip, including release beyond their bounds. Check the HSB and RGB values stay
  aligned, and changing a gray or black color's brightness preserves its chosen hue.
- Type HSB and RGB values, then use Escape to restore the palette that was open
  before editing. Confirm low-contrast drafts keep the last legible preview and
  show the contrast explanation.


Menu access follow-up (Pulse2115): removes the in-window settings button and adds
the native application-menu action and global Command-comma binding. Existing
preview/persistence tests exercise that action through keyboard dispatch. The user
accepted theming on 9 October 2026; native menu access remains a separate check.
