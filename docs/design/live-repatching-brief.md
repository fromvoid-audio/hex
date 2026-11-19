# hex live track and device editing brief

This is the agreed product target for 0.2, not a description of shipped behavior.
It inherits the [hex overview](../../README.md), the [device-host brief](device-host-brief.md)
and the shared product design philosophy. Pulse owns implementation scope,
dependencies and acceptance. Source doc comments own technical contracts.

## purpose and working surface

People composing and performing should be able to repatch a playing set quickly
and predictably. Prepare edits away from the audio callback while the old graph
plays. Apply the complete edit at a clean boundary with a short de-click
transition. Keep unrelated tracks and transport going. After that transition,
each running device has one active route. Brief fades and temporary retained
compensation latency are acceptable costs of stability.

Track/device creation, selection, power, placement, loading state, cancellation
and failures belong on the working surface. Keep independent controls usable,
including an already-running effect's parameters. Pending controls must not
pretend their audio has installed. Expose device controls when ready. Keep the
layout compact and calm, using hex's shared controls and lowercase text.
Keyboard navigation must reach these actions and preserve normal text editing;
cancel and interruption choices must be keyboard operable with clear focus.
This policy needs no new long-term settings or configurable tail modes.

## agreed editing behavior

1. **create tracks immediately.** An empty track is silent. A track created with
   a device appears immediately with a visible loading state. Once ready it can
   accept notes or clip audio under normal playback and monitoring rules.
   Creation itself launches nothing. Loading failure is visible on the proposed
   track and leaves the existing set playing.
2. **remove with a short de-click fade.** Deleting a device or instrument
   reconnects the surviving path; deleting a track fades it to silence. The
   removed device's own echoes, releases and held instrument notes end with
   the fade. Retire the instance safely after its callback references end.
   Do not run a separate outgoing DSP tail.
3. **insert fresh DSP when ready.** Preserve still-present processors' history.
   Inserting a delay after a sounding reverb feeds that reverb's continuing
   decay through the new delay at the switch. There is no parallel old reverb
   route beyond the transition.
4. **reorder within a track without restarting devices.** Keep running device
   state and switch routing. Moving distortion after a delay makes the delay's
   existing repeats pass through the distortion.
5. **move the running device across tracks.** Transfer its state to the
   destination, including a delay's existing repeats. Smooth both paths with a
   short fade. Voices already sounding on the source track move with the
   instrument and must receive their proper releases. For destination input,
   wait for note-on events at or after the handoff; do not chase notes whose
   destination onset preceded it. A four-bar destination chord already halfway
   through stays silent on the moved instrument; the next real note-on plays
   normally. This rule is specific to cross-track instrument moves and does
   not change insertion, off/on or other note-chase behavior.
6. **preserve sound through visual track reorder.** Routing edits, including
   moving into a group, retain running track and group processor states and
   switch to the new route with a short fade.
7. **make hex power genuine on/off.** Off smoothly removes the device's
   contribution, stops processing and clears runtime audio history. Retain
   authored settings and automation. On starts fresh, with only a tiny
   transition if needed to de-click and no designed audible fade-in. A
   plug-in's own bypass remains the plug-in's responsibility.
8. **prepare replacement before removing the old device.** Once ready, use
   remove-and-add with a short fade and fresh replacement DSP. Failed
   preparation leaves the old device. Duplicate/paste restores authored
   settings and copied automation into a fresh instance; the original keeps
   running.
9. **keep surviving downstream history.** This includes shared returns.
   Removing a vocal feed stops new vocal input after the fade, while vocal
   and snare decay already inside a surviving return can finish. Deleting
   the return itself follows the removal rule.
10. **keep output timing stable through a playback pass where possible.**
    Apply live when routing fits existing timing and can switch cleanly. On
    latency reductions retain only compensation delay until stop and next
    play; actual device retirement is independent of timing retention. If an
    increase cannot fit, or another unavoidable interruption is required,
    prepare while old audio plays and ask first. Proceed fades down, switches
    and resumes from the same musical position with new timing. Cancel keeps
    the previous graph playing. Do not blend different global time positions.
11. **restore authored state with undo/redo.** Restore project settings,
    placement and automation. Reintroduced devices are fresh DSP instances;
    do not resurrect audio buffers or voices. Release deleted plug-ins after
    safe retirement instead of retaining warm workers for history. Undoing a
    move of a still-running device moves its current DSP state back. Apply the
    same latency rules to undo/redo.
12. **show a cancelable pending insertion.** A loading slot needs cancel, not
    movement or a pending-chain editor. Accepted independent edits, such as
    gain changes on a running EQ, survive insertion success, failure and cancel.
    Failure/cancel preserves a valid old audio graph; explain failure. A
    cancelled load must never install later.

## success and limits

Success means these workflows hold for native and hosted devices, including
rapid edits, failed preparation, cancellation, undo/redo, held-note release,
group/return routing and compensated playback. Source and headless tests must
prove their bounded contracts; optimized native interaction and listening
acceptance remain separate user checks in Pulse.

0.2 is reserved for direct implementation, necessary prerequisites, regression
coverage and acceptance of this policy. General arrangement, recording, export,
browser, device-catalog and engineering improvements belong to later roadmap
versions unless a concrete dependency makes them necessary here. Completed and
cancelled issue history remains unchanged.

This is a hex policy, not an Ableton parity claim. Previously verified Live
documentation says device edits generally work without interruption and turning
off devices retains latency. Deletion timing and a Live interruption dialog were
not verified. No new reference runtime study is asserted here.

Fade lengths, transfer mechanics and compensation capacity need bounded
implementation decisions and evidence; this brief does not choose them or
claim they are already qualified.
