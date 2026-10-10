# Return effect residual tails

SessionPlayback preserves an audible Return's complete previous processor chain
across a revision that changes its device order, bypass state, configuration or
removes the Return. It does not retain only a changed suffix: state in an earlier
processor may still feed the old later processors. The candidate chain remains
fresh when its configuration changed. If the full old chain's configuration is
unchanged, its history is copied once into the candidate and no parallel tail is
created. A Return gain or mute edit alone does not invalidate processor history;
the copied history runs under the candidate bus controls.

At a handoff, an audible old bus is eligible when it is unmuted, has nonzero
linear gain, and its committed full-chain state is not quiescent. The retained
owner captures the old Return ID for diagnosis, bus-level stereo matrix, gain,
mute state and the complete `NativeRuntimeChain`, including each device's own
matrix and runtime. The old chain receives zero input once per render chunk of at
most 256 frames. Its output is added once to the candidate Main after the
candidate's current Return buses and before the candidate master stage. Old
sends are not replayed, and retained Returns cannot send to or feed other
Returns. New sends go only through the candidate bus.

The separate tail bank has 64 preallocated slots. Every pending handoff first
preflights all transfers and captures together with Arrangement release tails:
at most 256 stateful tail processors, 2 MiB of paired declared runtime payload,
and 4,096 aggregate chain entries. A declared finite residual longer than ten
seconds at its prepared sample rate is refused. Unbounded declarations are
admitted within the same processor, byte and entry bounds and retained until
the complete chain reports quiescent. If a limit or the 64-slot bank would be
exceeded, handoff returns `ReleaseBankFull` before moving any runtime owner, so
the publication can be retried after a later Stop or Seek clears residual
state.

The bank and its 256-frame scratch are allocated at SessionPlayback
construction. Processing is limited to the existing render chunk, and no old
chain is cloned or dropped in the callback. Speculative active flags commit only
after the full requested render block succeeds; a failed block restores those
flags and the next `begin_block` restores processor scratch from committed
state. Stop and Seek reset all retained chains and flags. Pause freezes them,
and Arrangement loop wrap does not reset them. Quiescent chains remain owned by
their Playback until control-side reclamation.

Focused engine tests cover old-chain continuation after Return removal, old
matrix/gain with the current master, changed later-device configuration while
retaining the old prefix state, bypass/reorder changes, fresh candidate history,
gain/mute-only inheritance, inaudible removal, tail propagation, exact shared
budget boundaries, finite-horizon/overflow refusal and retry, and render rollback.
They also cover exact-chain history inheritance without duplication and full-bank
refusal/retry. The allocation
probe `stateful_utility_return_render_and_retirement_are_callback_allocation_free`
passes for a stateful Utility Return revision handoff, requiring zero total and
net callback allocations and deallocations. These checks do not establish
physical-device deadlines, audible acceptance in Live, or behavior for every
third-party processor. Return processors still must report zero algorithmic
latency before compilation; this work does not add plugin hosting or path delay
compensation.
