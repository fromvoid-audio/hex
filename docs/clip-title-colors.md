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
