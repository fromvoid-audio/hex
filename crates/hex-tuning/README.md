# hex-tuning

## Purpose

Safe, bounded control-side repeating scale math, harmonic analysis and validated
project tuning/audition values. Adapts the user's Freqs source; provenance and the
full requested editor checklist are in [project tuning](../../docs/project-tuning.md).
The neutral project default retains A440 12-TET; Freqs's seven-degree 220 Hz scale
is available as a deliberate authored scale. A table has exactly 128 frequencies.

## Dependencies

Standard library only. Project/storage own canonical edits and persistence.

## Does not own

GPUI, audio processing, project history, files/preset storage, plugin hosting,
MTS-ESP native loading, global master registration or callback publication.
Analysis and scale edits run on control, never in the audio callback.
