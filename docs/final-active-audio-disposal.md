# final-active audio candidate disposal

Pulse2540 / canonical PR235 extends PR234's real constructor custody. It retains
original PR177 `8d444c06c295e52a8aa0dde275a89ecec6f454e2` as provenance for the
owner-location/Pending-before-take approach. Original Pulse24 / PR177 full graph,
transfer, native and listening acceptance remains open.

## one actual owner and active slot

The private `ActivePublication` enum has one slot for either the existing legacy
publication or `ActiveCandidate`. The latter contains the same constructor-owned
`AudioCandidateOwner` and exact root/slot/incarnation/runtime Stamp accepted by
the real publication queue. One private installation boundary requires a vacant
active/deferred slot and matching constructor identity. It adopts no raw Playback,
Arc or caller-generated installed receipt. Legacy publication behavior remains.

Production admission still always refuses this variant before installation.
A cfg(test) override exercises the actual slot and runtime. Non-default engine
`instrument-test-support` exposes bounded test actions/render and scalar transport
only; default code exposes no renderable constructor escape. The test fixture
sets the private Control revision scalar to enable its existing bounded command
queue. This is not applied-revision, install-ACK or native admission evidence.
PR236 must supply real production revision/accept/install/ACK metadata and satisfy
its separate readiness, accounting and admission contract.

## exact terminal return

StreamOwner anchors native custody before backend exposure. The same leased
retirement bundle holds its actual stream handle and terminal root. Lifecycle
must normally complete the stream-handle take/Drop frontier before it can provide
the private, root-bound `CompletedStreamHandle`. A missing handle after exposure,
a pause refusal or pending/panicking Drop cannot supply it. Root identities are
never reused; exact slot/runtime incarnations are independently rechecked when
returning the owner.

Both data and error guards must release their final terminal anchors before
control extracts either original Box. Publication into a terminal cell alone is
insufficient. The sealed root-bound release authority plus completed handle and
exclusive exact Callback extraction permits the same active owner to move into a
distinct `FinalActiveReturn`. Queue rejection and legacy Box destruction cannot
mint that type. Pause, silence and shutdown ACK alone prove none of this.

The extracted owner is secured in persistent NativeRetirement storage before
any callback/error carrier destructor. It stays there until both actual Box
Drops return normally. A destructor panic leaves Pending, the first outer
retirement payload and the remaining original owner retained without retry.
The production lifecycle loop retains failed retirement and its exclusive output
lease, so ordinary field Drop cannot release this custody or admit a new output.

Only then does the exact Root accept FinalActiveReturn. PR234's existing worker
execution performs original runtime/group destruction, preparation cleanup,
actual catalog-lease release, stream join and complete page-pool conservation.
Busy external leases retain occupancy. Incomplete pool/worker/destructor failure
remains sticky and charged. READY classification `FinalActive` is published only
after normal cleanup and metadata destruction, together with the actual original
bank and engine disposal fact. READY extraction is one-shot; the terminal root
never reopens. Control abandonment instead completes all original-bank Drop
phases on control without requiring another UI poll.

This establishes permanent nonrenderability through valid application callback
paths under the pinned safe CPAL dependency contract. It is **not** independently
observed OS AudioUnit stop/uninitialize/dispose success. CPAL 0.17.3 may start the
AudioUnit before returning a stream; coreaudio-rs 0.14.2 ignores several native
teardown errors and can retain callback guards after failure. Missing releases or
exposed failed-build uncertainty remain pinned. No unsafe boundary, callback
panic recovery, forced stop, timeout or new worker/registry was introduced.

## closed audio-only ownership class

The existing constructor rejects devices, instruments, MIDI and signal graphs.
Its exhaustive Playback/Session visitor now reserves a row for every PRESENT
and ABSENT independently owning location. Real present Project/prepared-revision,
Session, metronome and empty Track/Return-chain handles remain owned during
supported launch/play/Pause/Stop operations. Only normal actual runtime Drop
closes their constructor-bound groups.

Mutable owners, tails, retained analytic contexts, compensation and linear
fallback owners must remain absent. Actual unsupported population refuses and
pins the original runtime, with no group closure or recovered input claim.
An absent location is never counted as an owner or a disposed group. Supported
launch/inheritance/Stop changes routes and DSP state but does not transfer any
child owning handle between runtimes. The actual transfer here is the whole
constructor owner through queue → active slot → terminal custody.

This does not certify independently populated stateful tail/context fanout.
Full transfer hooks and descendant-owner accounting remain part of Pulse24 /
PR177 before any eligibility expansion; tests never substitute unsupported DSP
fixtures for a constructor proof. PR233's Unknown costs, complete-cost refusals,
32 MiB runtime ceiling and domain-count policy remain unchanged. New owner-row
backing is checked against actual runtime storage; opaque runtime/carrier/OS
costs stay Unknown. Physical destruction is not a PCM-free or admission claim.

## evidence boundaries

Focused controls exercise the actual constructor and active slot, Session audio
source-offset PCM, Pause/Stop and full-command priority Stop, both release orders,
delayed aliases, exact callback/error Box identities and control destructor thread,
completed-handle and pause refusal, retained owner after an actual carrier Drop
panic, real catalog Busy, abandoned Control autonomous multiphase cleanup,
original input conservation and one-shot/reuse barriers. Installation and actual
bounded command/render paths are measured for zero total and signed-net
allocation count/bytes. Engine controls include resident and streamed original
roots, exact actual Runtime Box destructor identity/thread, dormant handles and
an actual unsupported tail-population refusal. Existing PR234/stream controls
cover build/play/drop frontiers, worker panic and incomplete pool conservation.

Headless backend fixtures invoke the production lifecycle take/Drop helper;
they do not call a fake terminal-proof constructor. Native CPAL/device failures,
physical deadlines, listening and OS-stop observations remain separate. Exact
full repository local CI is recorded with PR235; prior PR validation is not this
candidate's evidence. No native user-test build or installation is implied.
