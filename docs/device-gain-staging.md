# Native device gain staging

The first version is manual. Every regular-track effect, instrument and Return
effect has a 25 px strip immediately to the right of its existing device frame.
The left well is input; the right well is output after the host output trim.
Both use the same -60 to 0 dBFS scale, stereo-linked maximum sample amplitude,
a fast attack, a 12 dB/second display decay and a held peak marker. Above-full-
scale peaks remain numerical values and show a red marker; audio is not clipped
by the meter. These are sample peaks, not oversampled true peaks or loudness.

Hover each well to see its highest observed peak in dBFS. Output hover also
shows trim and gesture hints. Drag output vertically to adjust -60 to +24 dB;
Shift gives fine adjustment, double-click resets to unity, and the existing
focused numeric control keyboard/escape lifecycle applies. The input well is
read-only. There is no automatic matcher, capture toggle or lock in this version.
Bypass bypasses both processing and host trim. Device frame/control widths are
preserved; the compact strip adds to the host's total rail width.

## Measurement and ownership

Every effect retains its own runtime entry even when it is a memoryless matrix.
Input is measured before processing; output follows processing and host trim.
The existing finite matrix-composition admission guard remains. Existing limits
on track/device counts, stateful processors and declared runtime bytes remain.

`hex-lattice::DeviceMeter` holds two positive-finite peak atomics. Audio accumulates
maxima until one UI consumer drains them, so short transients are retained between
refreshes. Construction, handles and final reclamation stay on control. Native
publication enables runtime metering; offline rendering/export leaves it off.
The UI resolves meters from the exact audible prepared revision, fences against
project generation and authored-state mismatches, and clears local held values
when that meter source changes. No callback allocation, lock, logging or file I/O
is introduced. Faulted or paused playback clears live fills. Atomic observations
are diagnostics, not part of the transactional musical state.

Instrument boundaries measure incoming audio (zero for MIDI-only generators),
then instrument output and host trim before suffix effects. The existing
analytic no-effect path retains its per-voice fader summation order. Its diagnostic
tap normalizes the already mixed lanes by their known nonzero coefficients;
this carries only f32 rounding, and muted/zero-gain lanes have no measured signal.
Ordered/stateful instrument paths use direct raw taps. Native metering PCM parity
and trim-only live-note handoff have focused coverage. General device-latency
alignment and intersample peak reconstruction are separate features.

## Authored gain

Host output trim is neutral envelope metadata, separate from each device's opaque
DSP payload. It uses integer millidecibels to retain deterministic Eq/history,
validated -60000..24000 inclusive. The reserved host parameter `u32::MAX` uses the
existing engine/session parameter gesture and undo pipeline; device registries
refuse that identifier in device-owned descriptors. Registry migration and
ordinary parameter edits preserve trim. DSP configuration identities explicitly
exclude it so changing host gain does not replace instruments or filter history.

Project schema 18 writes `output_gain_millidb` alongside each device envelope.
Older schemas default the field to zero; invalid gains refuse loading. Native,
Arrangement, Session and prepared export share post-device trim processing.

## Deferred matching

The requested later matcher would use peaks actually sampled while enabled,
with no short capture timeout that misses slow attacks. It would continue
tracking higher peaks and updating gain, turn off on focus loss by default,
and require a deliberate lock to remain enabled across focus changes. No part
of that behavior is implied by these manual controls.

## Native acceptance

Source checks do not establish macOS pointer/paint or listening acceptance. The
user owns native testing, through the supported optimized stable test bundle:

1. Play a transient and a slow attack through an effect; compare adjacent meter
   heights, held marks and hover values, including a signal above 0 dBFS.
2. Drag output, use Shift, reset and Undo/Redo. Verify the next device's input
   follows the adjusted output; check bypass, an instrument and a Return effect.
3. Save/reopen and switch device/track views while playing. Confirm trim persists,
   stale meters disappear, device controls still fit and peak hover stays readable.

This Linux cloud workspace cannot install or validate the macOS test bundle.
