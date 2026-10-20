# loop from selection

In the focused Arrangement, **command-l** sets and enables the transport loop to the
explicit time selection, or to the outer bounds of all selected audio/MIDI clips
when no non-empty time selection exists. Reverse time selections normalize their
endpoints; the main all-track proxy uses the same range helper. Gaps between
selected clips are included. Header selection already clears a time rectangle.
MIDI endpoints are converted through the current tempo map, including the end
beat, rather than treating a beat length as an absolute frame position.

Command-l replaces the former primary-audio-only loop action with this complete
selection contract. It is a loop command, not a media conversion. Repeated
command-l reasserts the range; it neither toggles nor starts/stops transport.
Plain l remains an alias while musical typing is off; with musical typing on,
unmodified l remains a note and command-l remains the explicit loop shortcut.
Session l and the loop toolbar retain toggle behavior. Command-l is Arrangement-only.
Text, numeric, rename, browser and other child editors retain focus ownership.
Empty selection is a no-op. A retained Session-owned clip after switching to
Arrangement has no Arrangement extent and is ignored; an unknown/deleted ID still
refuses. Active browser-audio and native-device drops keep their edit ownership
even if the editor retains keyboard focus. Stale non-empty ranges/clips refuse rather than
falling back; active edits finish before a loop can be set.

The existing SetLoopRegion command owns validation, one Undo/Redo and current
project-format persistence. This feature does not edit clips or selection,
change loop-bar gestures, or introduce playback/focus modes.

Native acceptance remains open: on an optimized combined candidate, select one
audio clip, one MIDI clip, then additive clips with gaps; command-l must show exact outer
bounds and an enabled loop. Drag a reverse time selection while a clip remains
selected; command-l must use the time bounds. Repeated command-l stays enabled, one Undo restores
the prior loop, Redo restores the new loop. Save/reopen preserves it. Check no
selection, browser search/results, numeric and rename focus, lower MIDI editor,
and musical-keyboard unmodified l do not change the loop. Command-l with musical
typing enabled must still use the full selection. Verify Session/toolbar toggle
and unchanged plain-l behavior outside musical typing. Do not replace or launch the user's installed app from
cloud work; the coordinator owns native dispatch and delivery.

Current integration refuses L and Command-L while a genuine middle-button
Arrangement pan owns the pointer. The existing active-pan field joins the
existing loop-shortcut edit guard; no pan lifecycle or keyboard framework
changes. The real held-pan regression retains project/history/selection and
pointer ownership, then checks the same keys and Undo/Redo after release.
The private selection fixture shares one precreated Environment for samples
and all four preferences. Its original selection/typing/Console/drop tests
remain intact, with central Environment lifecycle coverage reused.
