# pulse2881 / pr483 — held device preview playback

The confirmed mouse-node regression is that Space reaches the transport handler
but stateful Arrangement seek refuses an active device gesture, including an
already callback-applied preview. Stationary node hold succeeds. The original
report's node-versus-keyboard-note interpretation is still unconfirmed; native
framebuffer flicker is not reproduced or declared fixed by these source checks.

Candidate31cedf7398e68832b38f5052682a32fa7fad70ed,
treea542dc040be7eec13e9539ce07efbfa595c139d6, canonical483 continuation from
faf65f33418e55660a474d7dc53e51ae43d21716 (zero-diff mainc1875321).
Source history includes c70 pointer/routing diagnostics,557738f1 stable-output
negative,369fed76 intermediate pending-edit failure and0ff57b28 lifecycle
checkpoint. No history/main chase/whole-file imports. Nine files,988 additions,
25 removals relativefaf65; only arrangement_seek.rs/controller.rs production.

Private SeekIntent/PendingSeek retain existing DevicePreviewIdentity, prepared
Arc and exact audible preview ViewState into PendingPlayback/callback accounting.
Canonical None excludes both preview types. Admission/result/publication/callback
observation preserve generation/revision/key/sequence/output/route/fresh-runtime
fences. A sequence change retains only the latest play intent for the SAME held
key while its new preview settles; terminal/replacement/removal retires stale
candidates. Pause cancels pending play without committing/releasing the gesture.
An already-forwarded runtime retains publication custody until the response yields
its revision for the existing cancellation path. Session/Engine/audio public API,
callback policy, schema, native output and parameter DSP remain unchanged.

The exact clean13-stage final receipt is the authority for source qualification;
metadata.json records its hash, duration and counts. Runtime groups:60EQ+10seek+
20device comparison+5shortcut text=95unique/0ignored. Four new tests include12
actual preparation/published races:sequence/cancel/commit/project/device/Pause.
They use actual pointer/key delivery,Service preparation,publication and callback,
without fake Event/ACK/canonical install. Model node positions remain stable
through48 tick/repaint cycles and real parameter Undo/Redo. Native pixels, actual
output/listening/latency and full21/current-main qualification remain open.

Negative provenance: authoritative original output-v3 is2PASS1FAIL after matching
preview identity and callback revision apply. Its dirtyc70 test bytes were committed
unchanged as557738f1; productionfaf65. Early closed-pane and E0593/E0502 failures
are setup/compile-only. First implementation still used edit_pending()'s indirect
gesture guard and failed; preserved369. Lifecycle-v2 was3PASS1FAIL because its new
project fixture incorrectly required a replacement output not supplied by the
fixture. Dedicated diagnostic proves generation1/requests0/gestureNone/seekNone/
latchedfalse/candidates0/audiofalse/tracks0 with NEW canonical playback retained.
Corrected oracle verifies new source/generation/no old seek rather than discarding
that playback. Pause intentionally retains its held gesture. All raw distinctions
are retained; no failure is relabeled native proof. Earlier8f809 evidence is
referenced rather than recursively repacked.

Coordinator native checklist: qualified build/real output, held node stationary
and dragged Space play/pause, move during preparation, release/Undo/Redo, Escape,
device/project replacement and numeric text Space. Independently observe the
reported flicker with actual build/source and confirm whether original 'note'
means graph node or keyboard MIDI note. No native/app operation is performed here.

Pulse remainsInProgress. GitHub source readiness/review request is separate from
approval,merge,installation or user acceptance. Sole merger composes currentmain,
runs prospective required full checks and fresh review; no full21/selfmerge claim.
