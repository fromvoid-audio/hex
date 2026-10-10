# live effect residuals after Session source stop

Pulse2211 reproduced a source-stop bug: SessionPlayback skipped a live track's
processing when its route was Silence and no MIDI release existed, even while
its stateful effect chain retained filter history. Stopping an exhausted audio
one-shot cut the remaining DC/bass-mono residual to zero; leaving its one-shot
owner in place advanced the same history correctly.

A silent route now keeps processing actual zero input while the committed live
effect chain reports nonquiescent. It runs through the existing ordered track
path and post-fader sends, once per chunk, without reintroducing clip audio.
This does not create a detached owner or change revision handoff policy. Pause
freezes history; global Stop and Seek reset it. Quiescence inspects committed
state, never just PCM silence. Existing speculative twins and whole-block commit
continue to provide rollback.

The original32-frame audio fixture reproduces the cut before the fix and exactly
matches the exhausted-one-shot reference afterward across mixed partitions.
A second test covers Pause and global Stop/Seek. A native prepared callback
probe compares the stopped audio residual to the naturally exhausted one-shot
and checks zero total and signed-net allocation count/bytes during source stop,
residual render, global Stop and shutdown. Construction and control reclamation
are outside measurements. Native listening and device deadlines remain open.
