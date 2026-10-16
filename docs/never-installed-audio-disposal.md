# never-installed audio candidate disposal

Pulse2539 / canonical PR234 adds control-side custody for the constructor-owned
private roots from PR232. It follows the owner-location and Pending-before-take
ideas in original PR177 source `8d444c06c295e52a8aa0dde275a89ecec6f454e2`;
it does not merge that PR or establish its outstanding native/full-review gates.

## accepted custody is not installed playback

`AudioCandidateOwner` constructs one real engine `Playback` from the private
prepared revision. Root, preparation incarnation and runtime identities have no
public scalar constructor. It exposes borrowed preparation/source metadata, not
a render method, mutable graph, raw Arc, or transferable device owner. The
existing runtime preflight and 32 MiB accounted runtime ceiling remain in force;
this does not turn that partial accounting into a complete-cost declaration.

`Control::stage_audio_candidate` accepts custody into the actual one-slot
publication queue only when its native terminal receiver is anchored. A naked
headless channel refuses with the original owner and original domain Vec.
The same private queue envelope distinguishes legacy `Publication` from a
constructor-owned candidate. The candidate variant is admission-pending: the
current callback always rejects it before inheritance, active assignment or
rendering. It never updates applied revision or Session routes. Legacy publish,
fresh-runtime fencing, commands, Stop and rendering preserve their existing API.
No streamed/native admission, readiness or final-active proof is enabled.

Two persistent candidate slots share the existing two-domain retention policy.
This is a domain-count bound, not a new byte budget. Reservation, queue push,
pre-install rejection, return and physical disposal are distinct transitions.
A synchronous refusal restores the exact original owner/domain Vec; an accepted
nonowning ticket cannot reclaim anything by being dropped. Actual root, slot,
slot incarnation and runtime identity must all match. Stale/foreign/duplicate
returns cannot overwrite occupied destinations. A full return queue retains the
actual owner in the callback's existing held-return slot. No shutdown ACK,
silence, timeout or request manufactures a physical receipt.

## normal destruction and retained failure

UI-side `Control::reclaim` only ingests candidate returns and requests coalesced
cleanup. The existing lifecycle worker secures the returned owner and actual
catalog custody in a persistent execution slot, then releases the ordinary bank
lock before any provider, destructor or join. The constructor inventory exhaustively matches
Playback and Session fields; independently movable device/context locations are
separate groups, while the owning Session container also retains its buffers.
No inheritance/transfer occurs for this candidate. Only normal return from the
actual runtime Box destructor closes those groups. Existing preparation cleanup
then restores the original input bank. A physical engine receipt describes this
normal destruction; it is neither a PCM-free claim (the original bank still owns
PCM) nor a catalog, final-active or complete-storage receipt.

Actual catalog leases and matching control/pages pairs are retained. Full
metadata coverage is checked against those real leases; equal metadata never
establishes catalog identity. After engine root cleanup, leases are released and
`PreparedStreamDisposal` takes the exact original pair through a pre-reserved
borrowed cleanup capsule. An external lease yields Busy without stop/join or
ownership mutation. The actual JoinHandle leaves the worker mutex before join.
The first worker panic payload and remaining endpoints stay in the capsule.
All fixed pool pages must be conserved in persistent worker/endpoint vectors
before physical completion is reported. A pool mismatch is sticky failure.
The old public consuming `StreamControl::shutdown` behavior is unchanged.

Every potentially destructive frontier records Pending before taking the owner.
Control-side catches retain the first payload and remaining custody; failed
frontiers are never retried or treated as closure. An irreversible cleanup
failure also closes further staging in that root, returning new inputs unchanged;
already accepted owners remain retained. Busy external leases do not close it.
Slots/domain occupancy remain charged until normal cleanup. A live Control polls a READY original bank/receipt; recovery itself only moves
those values under a nonblocking bank lock. Original-bank extraction and empty
Execution metadata destruction finish on lifecycle before READY is published.
Control abandonment instead lets lifecycle destroy that bank normally, with
persistent Pending-before-take storage and no further UI polling.
A failed native retirement remains in the already-exclusive lifecycle lane with
its output lease, preventing a replacement output from escaping the barrier.
There is no timeout, new worker registry, admission bypass or retry protocol.

The existing lifecycle queue has three reserved roles: at most one Open, one
coalesced Cleanup and one leased Retire. The worker retains one pending cleanup
root through Busy and subsequent physical phases. Synchronous cleanup enqueue,
coalescing and the worker's idle decision use the same short root-bank lock.
StreamOwner announces retirement under that lock before enqueueing Retire;
no later live cleanup can trail it. FIFO dispatch adopts/clears the exact pending
root before retiring the output and releasing its lease, so no stale old job can
overlap a replacement root. The sender/worker exist before native enablement and
backend exposure. Queue refusal preserves all owners/charges; genuine internal
mismatch retains the lane rather than releasing an output. This is an audio
lifecycle handoff, not a general job service.

## native terminal anchoring

The data and error callback Boxes each have a move-only backend guard and a
control-owned terminal cell. Both terminal receivers and candidate-root custody
exist before `build_output_stream`. Backend guard Drop only publishes the Box;
control cannot extract either until both guards have released their final cell
anchors. Publication while a guard still exists is insufficient. Control-first
Drop moves the real return consumer/held carrier into persistent root custody so
backend destruction cannot become the last owner of an undrained return queue.
No candidate-owning callback local crosses a fallible/destructive operation.

The CPAL 0.17.3 macOS builder may start its AudioUnit before returning a Stream.
A build error after exposure, failed play, stream Drop or terminal publication
alone does not prove backend stop. Missing returned handles after exposure stay
Unknown and pin custody. Returned streams are paused/disposed on lifecycle
control; both guard-release orders remain required for physical extraction.
The safe dependency boundary does not independently verify OS stop: underlying
coreaudio-rs 0.14.2 teardown ignores native stop/uninitialize/dispose failures.
Final-active application nonrenderability and native stop evidence belong to
Pulse2540 / PR235. No native callback unwind recovery was added; ordinary
Result-based render faults remain supported. Survival of a panic crossing the
native FFI callback boundary is not claimed.

## storage and acceptance limits

PR233's sealed DATA rows and `require_complete` refusal are unchanged. New
runtime and terminal/carrier costs remain explicitly Unknown. The exact control mutexes are initialized during channel construction, preserving
legacy allocation-free control reclaim even on std implementations with lazy mutex
backings. Their opaque backing costs remain Unknown. The named
`AudioCandidateCustodyReport::metadata_bytes` observes only bank/slot and selected
catalog-capsule container DATA; it is not a total for runtime, carrier Boxes,
Arc control blocks, channels, worker/OS bookkeeping, stack, TLS or allocator RSS.
The new endpoint-page collection backing is reported separately; the joined
worker Vec transfers an existing factory backing and is not added twice.
Physical closure does not upgrade any unresolved declaration or admission cap.

Focused headless controls cover original-bank conservation, actual nested runtime
and carrier destructor failures, actual factory worker panic, missing pool pages,
exact identities and slot reuse, full publication/return pressure, domain pressure,
external leases, control-first abandonment, both final guard-release orders and
stream-build/pause/destruction frontiers. Opt-in per-thread probes measure actual
candidate rejection/held-return turnover and guard release for zero total and
signed-net allocation count/bytes. They do not measure worker allocations,
native API callbacks, OS stop, device deadlines or listening. Full exact-source
local CI remains required before merge; PR234 records its actual verification
status and evidence. Original Pulse24 / PR177 acceptance
and downstream PR235/236/241 work remain open.
