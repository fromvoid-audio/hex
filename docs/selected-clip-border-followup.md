# Selected clip bottom border follow-up

Pulse #2823 / canonical PR #429 follows the user's screenshot of selected
Arrangement audio clips with red top and side edges but no bottom edge at the row
separator. The original #2719 / PR #335 history remains intact; its manual
four-edge acceptance was not established by source tests.

The actual pre-fix completed scene placed the row separator after the selected
clip border at the same bottom pixel strip. The fix paints that unchanged
separator before clip content and overlays. Geometry, durations, row heights and
hit areas are unchanged.

The focused regression uses a real Service, prepared audio/MIDI state and a GPUI
window. Its private browser/catalog startup uses disposable paths before workers
start. It selects five mixed clips through actual header clicks/Shift-click,
checks selected and deselected scene borders at the minimum and enlarged row
heights, and checks visible viewport edges. Actual completed quad fragments and
masks (including retained replay) establish edge coverage and order relative to
the real row separator; this is not native framebuffer or GPU readback.

Native/visual acceptance remains open and coordinator-owned. On the eventual
qualified optimized candidate, select one and several audio and MIDI clips. At
minimum and enlarged track heights, inspect all four border edges, including the
bottom row boundary and viewport-clipped ends; deselect and compare. Check title,
waveform/notes, fade/trim affordances and header/body input still behave normally.
Do not alter or replace the currently installed app from this source lane.
