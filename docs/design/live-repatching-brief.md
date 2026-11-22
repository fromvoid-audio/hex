# hex live track and device editing brief

This is the agreed product target for 0.2, not a description of shipped behavior.
It inherits the [hex overview](../../README.md), the [device-host brief](device-host-brief.md)
and the shared product design philosophy. Pulse owns implementation scope,
dependencies and acceptance. Source doc comments own technical contracts.

## purpose and working surface

People composing and performing should be able to repatch a playing set quickly
and predictably. Prepare edits away from the audio callback while the old graph
plays. A signal routing change is a hard cut with a short fade to prevent
clicks. Stop the old affected route and install one new route at the cut.
The cut can clear live DSP history, including voices, tails and buffers. Keep
unaffected routes and transport going. Preserve authored settings and apply
the output timing rules below.

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
3. **insert fresh DSP when ready.** Prepare the device while the existing chain
   plays. Apply the routing cut when ready so subsequent input runs through the
   updated chain.
4. **reorder devices within a track at a routing cut.** Keep the devices'
   authored settings and apply the new order with a short de-click fade.
5. **move the running device across tracks.** Prepare a fresh device for the
   destination with the current preset and authored settings, including
   parameter values, while the source keeps playing. At the routing cut, use a
   short de-click fade and switch to the prepared device. Sounding voices,
   tails and buffered audio end with the outgoing instance. Release affected
   voices and retire the original safely. Failure or cancellation leaves the
   source in place. The destination uses its normal playback and monitoring
   rules.
6. **preserve sound through visual track reorder.** Moving rows without changing
   signal routing keeps playback continuous. Moving a track into, out of or
   between groups changes its routing and follows the de-click cut policy.
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
9. **keep unaffected routes playing.** Apply the de-click cut on changed paths,
   including input changes to a shared return. Removing a vocal feed stops its
   input at the cut, while remaining feeds continue through the return. The cut
   can clear decay buffered by the return. Deleting the return itself follows
   the removal rule.
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
    safe retirement instead of retaining warm workers for history. Undo and
    redo of a cross-track device move follow rule 5. Restore placement and
    authored settings in a fresh instance. When undo/redo changes signal
    routing, use the same de-click cut and output timing rules.
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

This is an independent hex policy: device edits should work without interruption
and turning off devices retains latency.

Fade lengths, cut mechanics and compensation capacity need bounded
implementation decisions and evidence; this brief does not choose them or
claim they are already qualified.
