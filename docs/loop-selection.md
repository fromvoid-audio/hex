# loop from selection

In the focused Arrangement, **l** sets and enables the transport loop to the
explicit time selection, or to the outer bounds of all selected audio/MIDI clips
when no non-empty time selection exists. Reverse time selections normalize their
endpoints; the main all-track proxy uses the same range helper. Gaps between
selected clips are included. Header selection already clears a time rectangle.
MIDI endpoints are converted through the current tempo map, including the end
beat, rather than treating a beat length as an absolute frame position.

This deliberately replaces Arrangement plain-l loop toggling. Repeated l
reasserts the range; it neither toggles nor starts/stops transport. Session l,
the loop toolbar action, and the existing Command-l single-audio action retain
their existing behavior. The musical computer keyboard keeps l as a note.
Text, numeric, rename, browser and other child editors retain focus ownership.
Empty selection is a no-op. Stale non-empty ranges/clips refuse rather than
falling back; active edits finish before a loop can be set.

The existing SetLoopRegion command owns validation, one Undo/Redo and current
project-format persistence. This feature does not edit clips or selection,
change loop-bar gestures, or introduce playback/focus modes.

Native acceptance remains open: on an optimized combined candidate, select one
audio clip, one MIDI clip, then additive clips with gaps; l must show exact outer
bounds and an enabled loop. Drag a reverse time selection while a clip remains
selected; l must use the time bounds. Repeated l stays enabled, one Undo restores
the prior loop, Redo restores the new loop. Save/reopen preserves it. Check no
selection, browser search/results, numeric and rename focus, lower MIDI editor,
and musical-keyboard l do not change the loop. Verify Session/toolbar toggle and
Command-l audio behavior. Do not replace or launch the user's installed app from
cloud work; the coordinator owns native dispatch and delivery.
