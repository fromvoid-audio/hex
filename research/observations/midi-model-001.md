# midi-model-001: musical note editing reference

Date: 8 October 2026. Scope: public documentation study for Hex's first canonical
MIDI model, not installed-app behavior or executable analysis.

Source: [Live 12 manual, Editing MIDI](https://www.ableton.com/en/live-manual/12/editing-midi/).
The earlier `editing-midi-notes-and-velocities` URL did not fetch; the current
Live 12 chapter is `editing-midi`.

Documented behavior: MIDI clips carry notes and controller data. Notes describe
pitch, position, length and velocity for an instrument. The note editor presents
pitch vertically and musical time horizontally, supports note creation/movement,
and exposes velocity and chance lanes. Edits are undoable and do not mutate an
imported source MIDI file.

Hex direction: retain canonical beat-based note data with validated ranges and
undoable project commands; resolve note timing outside the audio callback. Keep
persistence independent of a piano-roll view or particular instrument. This is
an original implementation choice informed by the documented workflow.

Unknowns: exact Live internal representation, overlap/retrigger ordering,
seek/chase and loop-boundary note-off behavior, tuning representation, per-note
expression, controller state, chance evaluation and serialization. No proprietary
MIDI clip, factory content or decompiled implementation was copied. No claim of
observed installed-Live MIDI behavior or exact MIDI parity.

Acceptance must include valid/invalid note data, stable identities, history,
strict persistence, deterministic frame-event timing and discontinuity handling.
Controller/expression/chance/MPE, external MIDI I/O and device/plugin hosting stay
explicitly required in the larger feature plan.
