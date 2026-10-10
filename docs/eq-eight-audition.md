# eq eight band audition kernel

Pulse2292 adds a transient DSP primitive for monitoring one enabled EQ band in
isolation. The selection is an ephemeral `(band, edit lane)` value with checked
ranges; it is not saved in native device state, project schema, or parameter
descriptors, and no UI control is exposed by this change. Selecting a disabled
band is refused. The prepared runtime retains the authored enabled flags
separately from coefficient identity, so an enabled neutral-gain band remains
auditionable regardless of whether its prepared filter is mathematically neutral.

Each frame continues to run the complete normal EQ cascade with its existing
coefficient and output-gain smoothing. A second fixed 128-byte history processes
only the selected band in parallel from the pre-EQ stereo or channel-mode input.
In stereo mode the selected band processes both channels. In left/right and
mid/side modes the selected edit lane is processed and the other lane remains
dry before the mid/side pair is converted back to stereo. The latter is Hex's
explicit audition interpretation, not a measured Ableton Live behavior.

The normal and audition outputs crossfade over 256 frames. Releasing audition
fades back to the always-running normal cascade. Switching bands or edit lanes
fades the current audition down, clears the audition history at the switch, then
fades the new selection up. A selection change can therefore take at most 512
frames from a fully audible old band to a fully audible new band. Reset clears
selection, pending selection, mix and parallel history. Same-rate, same-mode
compatible coefficient handoff copies the normal and audition histories and
crossfade state together. A mode change or
a target that disables either active or pending selection cancels audition and
clears its history; the new normal EQ remains publishable. A fresh runtime starts
with no audition unless history is explicitly inherited.

The tests compare each band shape against an independent selected-band prepared
runtime, verify both stereo lanes and left/right and mid/side edit lanes, confirm
other enabled bands are excluded, exercise neutral-enabled and disabled bands,
and check reset, mode/enable handoff, partitioned crossfade and coefficient
handoff. This validates the fixed standalone kernel. It does not add graph or
project control plumbing, runtime callback allocation instrumentation, user
access or UI, Live behavior claims, audible host acceptance or performance
measurements; those remain separate integration and acceptance work.
