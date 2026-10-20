# Clip title colors

Pulse2828 / PR432 replaces the Arrangement title strip's hardcoded Raised fill
with the existing track-inherited `clip_fill(color, flavor, selected)` result.
Its foreground uses the shared theme-readable text helper against that exact
fill. Palette identity, flavor, persistence, geometry, selection and input routes
are unchanged. Existing inline name input retains its own focused field style.
Console audio/MIDI cells already use this inherited color resolver and derived
foreground; their production code is unchanged.

The private bounded GPUI test reads actual completed title background quads for
audio/MIDI, Muted/Accent flavors, selected/unselected states and a real inherited
track recolor. It checks geometry, independent selection border and Command-R /
Escape name editing without changing clip content. The prior source paints
neutral gray instead of the requested inherited color. This is scene evidence,
not native pixels, glyph rasterization, physical input or listening acceptance.

The present source tests the existing minimum44px rows. Header-only heights and
H overview are separate PR431; the merger must preserve this fill/foreground
hunk while integrating that geometry and PR429's bottom-border paint order.
Future clip activation semantics belong to their own model/gesture feature;
Muted here is the existing visual flavor, not a newly invented audio mute.

Coordinator-owned optimized visual acceptance: compare audio/MIDI title strips
for ordinary and multi-selection, both flavors and an inherited track recolor;
check readable names/rename focus, all four independent selection edges, body
and header grabs. Repeat at the eventual header-only height after PR431 is
integrated. No app installation or physical action belongs to this source lane.

The title receives the exact fill already resolved for its audio/MIDI clip body.
It performs no additional project, track or clip lookup. Previously each title
called `Project::clip_flavor`, which scans Arrangement audio, then MIDI, then
Session cells; repeated for V visible clips among N clips this introduced
O(V*N) lookup work per redraw. The source call paths establish that cost; no
native latency or benchmark improvement is claimed. Both existing callers now
resolve one body fill and reuse it for the title and readable foreground. The
original rendered color/recolor/selection/geometry/rename regression is unchanged.

Current-main integration preserves the passive preview title route: actual clips
receive the already-resolved body fill, while transient preview titles keep
their existing neutral fill and remain noninteractive. No new project lookup
is added. The private scene fixture now precreates one Environment before
sample/import/UI startup, sharing its root with all four preferences and
catalog jobs. Its original color/selection/rename/geometry assertions remain
unchanged; a held real catalog save checks root retention and cleanup.
Native readability, physical input and user acceptance remain open.
