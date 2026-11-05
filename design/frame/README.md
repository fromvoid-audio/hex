# hex / orbital workstation — Frame study

## Continue editing in Frame

[hex-orbital.frame.json](hex-orbital.frame.json) is the current editable prototype,
saved at r133. It includes the structured design, prototype actions, and orbital
styleguide in one portable project.

In Frame, choose **Project → Import project**, select the file, and open the
imported copy. If Frame already has a project with the same ID, use the separate
copy proposed by the import preview. Turn on **Interact** to use the top-right
Arrangement/Console switch and bottom-right Clip/Device switch.
Choose `group-track-selected` in the State picker to see the selected group track.

To save a later design revision back to Hex, use **Project → Download project** in
Frame and update the portable project on the design branch. The older artifacts
below document earlier iterations.

## Earlier design and implementation reference

Two editable 1900 × 1120 desktop concepts closely follow the user-supplied Live
screenshots' layout and widgets with a neutral charcoal / sci-fi accent theme and Hex's
lowercase Ioskeley Mono typography. Arrangement has the overview, timeline,
right-side track controls and device dock. Session has twelve track columns,
six scenes, routing, six sends per channel, faders, solo/arm indicators, narrow
returns, master column and bottom MIDI clip editor. Both share the two-column
browser, transport and groove pool. Controls are compact, panels contiguous.

## Accepted implementation reference

[UI iteration 3](iterations/03/README.md) is the current user-approved native
implementation reference: r70, saved 8 October 2026, with softer cyan #70e8e3
and six state views including Arrangement time selection. It includes the portable
Frame project, theme, sources, previews, validation results and artifact hashes.
[Iteration 2](iterations/02/README.md) remains preserved as the previous baseline.

## Open and edit

```sh
python3 design/frame/serve.py --frame /Users/portalsurfer/dev/frame --port 8780
```

Open http://127.0.0.1:8780/. Frame remains unmodified. Use the State picker or
the two top-right prototype controls to switch arrangement/session. View switching,
play/pause/stop and scene launching change illustrative prototype state only.

- `hex-live-r70.json`: canonical structured Frame document.
- `build_live_study.py`: reproducible geometry generator for the canonical document.
- `live-layout-styleguide.json`: unified theme tokens, custom-color policy and density rules.
- `hex.frame.json`: portable Frame project; import through Frame's Project menu.
- `arrangement.png`, `session.png`, and `palette.png`: full-resolution Frame exports.
- `clip-colors.json`: custom-picker policy and example saved swatches.
- `clip_palette.py`: preserved earlier palette generator, no longer used for concept paint.
- `review.json`: palette-board and five-state structural checks and eight declared action scenarios.
- `load.js`: bundled font registration, loading, checks and exports.
- `hex-yield-r8.json` / `yield-styleguide.json`: preserved earlier concept.

To revise geometry, edit the generator, bump its revision/file name and the loader's
reference together, generate the JSON, then reload Frame. Loading writes exports
through the existing restricted loopback export endpoint. Generated previews and
portable handoff reflect the loaded revision.

## Theme and references

The current beauty pass is deliberately independent of Yield's exact palette,
authorized by the user's 8 October request. It retains the layout and typography.
The current **orbital** theme follows the user's supplied Yield concept
(clipboard `6a41c277-d18b-447d-aaa7-daffc44c31d5`): deep charcoal `#2a2828`,
panels `#2f2d2d`, wells `#242222`, soft off-white `#c3c5c0` and red-orange
`#f06449`. Cyan `#00e8df` is secondary, soft violet `#9994bd` is tertiary, and
clearer blue `#719cce` supports parameters. Concept clips share darker gray `#454c49` with off-white waveform detail.
Groove timing and browser/action indicators use the bright red-orange accent.
These are independently chosen tokens inspired by the supplied pixels.

The r67 concept prioritizes harmonious artwork over assigning a unique hue to
every clip. Readable text remains important. One smoky gray clip fill complements bright orange controls and secondary violet,
blue and mint accents; they are concept paint
choices, independent of the user's personal palette.

The fixed clip library is replaced by a free color-picker design: saturation/value
plane, hue strip, hex/RGB fields and personal saved swatches, capped at 64. Eight
example saved swatches demonstrate the concept; users are not restricted to those
colors. Opening/closing the picker is interactive in Frame. Color manipulation,
applying a color, saving/removing swatches and persistence are design specifications,
not implemented native or prototype behavior. Earlier palettes remain preserved.

Separate semantic tokens remain green `#65ffad` for success, yellow-orange
`#ffd35e` for warning and red `#ff7086` for error. Bright accents are reserved for
focus, primary actions, parameters, editing and deliberate status signals.

`theme_palette.py` is the palette authority. `theme-palette.json` contains every
UI role and clip color; `theme-palette.frame.json` and `theme-palette.png` make
these relationships reviewable in Frame. Previous Dragon and earth-tone trials
remain in the earlier revision JSON and preserved `r25-*` through `r31-*` files.
The earlier Kanagawa research used its published palette and theme mapping at
https://github.com/rebelot/kanagawa.nvim (8 October 2026).

Browser tags are now individual outlined rectangles, replacing continuous dark
bars in content, type, sounds, drums and the footer. Each uses a 15 px height,
dark `#211f1f` border and existing 17 px row positions. This implements the user's
supplied cropped Live browser reference (clipboard ID
`bd692965-d265-4e7b-9876-d914691ce444`); tag behavior remains illustrative.

The current preview uses 127.0.0.1:8780. Its accumulated browser revision history
now reaches the storage quota; the portable project is saved on disk with the
current r67 revision and complete archived history. Previous
localhost origins retain their accumulated history; no browser data was deleted.
The exported portable project is confirmed at r67.

Historical references remain Yield's default theme source at
`/Users/portalsurfer/dev/yield/crates/yield-design/src/lib.rs`, revision
`144cfaf329c66482e6d0640bc24828a9f08e3c8f`, and the supplied original concept at
`/Users/portalsurfer/dev/yield-legacy/reference/wavecrate-editorial-terminal-clean-master.png`.
The current unified colors are Hex design choices, not claimed Yield tokens.

`clip-colors.json` now specifies custom picker controls, the maximum of 64 saved
swatches and example personal colors. It is not a fixed available-color library.
`theme-palette.json` separates UI tokens, concept artwork colors and picker policy.
Arbitrary future custom colors require adaptive waveform/text ink and focus outlines.

Layout references are the two screenshots supplied on 8 October 2026 (clipboard
IDs `cccf5176-f323-4e58-8cd7-0d1aa0175bb7` and
`5e091356-2820-429d-8fa3-8a03ddecf539`). Reference pixels/media are not embedded;
waveform envelopes and notes are synthetic. This is a visual concept, not a new
reference-app behavior experiment or a measured pixel-parity result.

## Validation and limits

Revision r67 is inspected at its actual export size. Frame checks text bounds,
clipping, unexpected overlap and contrast across default/playing/session/
session-playing/clip-colors; intentional piano-grid underlays and scrollbar overlays are
declared explicitly. Eight declared prototype action scenarios pass.

The design has 4,625 elements after reducing the clip picker. The main workspace
geometry and prototype actions remain unchanged; the color picker becomes a custom-color design.
The theme board shows the four accents in context and separates custom swatches.
Both final views, tag rows and palette were visually inspected. Perceived beauty
remains a user acceptance judgment.

The r67 prototype checks pass. The separate native UI style guard currently flags
`crates/hex-ui/src/editor/arrangement.rs:126–127`; this design pass did not edit that file.

The native GPUI application is unchanged. Routing, sends, arm indicators, browser,
clip parameters and devices are illustrative design widgets. This does not establish
DAW behavior, native interaction, screen-reader support, responsive resizing or
small-window acceptance. Detailed icon paths and full widget interaction are future
implementation work; some icons are font glyph stand-ins.

Frame r67 implements all four notes preserved in `feedback-r54.json`: bright red-orange
groove timing indicators, vivid orange browser/action controls, one smoky gray
clip fill, and gray/orange as the core. Other accents remain subordinate.

Frame r67 brightens background layers slightly and adds subtle red warmth, with
darker dividers; accent/text/clip colors and layout remain unchanged.

Frame r67 adds luminous green `#00f5c6` for secondary highlights and lifts warm
backgrounds three RGB steps. Tags have dark fills/outlines. Library selection
follows Yield: faint orange row fill and a 2 px left orange marker, based on
`yield/crates/yield-ui/src/browser/view.rs` and `yield-design/src/lib.rs`.

Frame r67 replaces violet with raspberry `#d12c6c` for tertiary details. Orange
is primary; luminous green `#00f5c6` is secondary. The notes tab and unfocused
note outline demonstrate tertiary use. Background lift and dark tag chips remain.

Frame r67 nudges the secondary accent toward cyan-blue; other colors stay exact.

Frame r67 adds solid simulated playback meters in Arrangement and Session,
including stereo channel/main levels, return levels, peak markers and orange upper levels and red
peak caps. Play/scene launch shows the fills; Stop clears them. Levels
are static illustrative playback states, not continuous animation or audio input.

Frame r67 replaces the strawberry tertiary accent with soft violet `#9994bd`
and shifts secondary mint toward cyan `#00e8df`. Orange and charcoal stay
unchanged. Solid meters use orange upper levels and red peak caps.

The iteration 2 [state gallery](iterations/02/states/index.html) saves all five
interface states as individual full-resolution views. The loader exports them
using state-*.png names in addition to the primary previews.

Frame r68 adds a time-selection visual state: a translucent cyan range across
three Arrangement track lanes, strong range edges and a matching ruler strip.
This is selection artwork, not implemented drag/edit behavior. Iteration 2 r67
is preserved; r68 is saved as a state-study extension.

Frame r69 slightly desaturates secondary cyan to #35e8e1, preserving its
brightness, all other theme tokens and geometry. The time-selection tint now
derives from this token, keeping meters, focus and range artwork consistent.
The accepted iteration 2 snapshot remains unchanged.

Frame r70 further desaturates cyan to #70e8e3 while preserving peak brightness
and all other theme tokens and geometry. Shared selection, focus and meter
artwork follow the token.
