# Return tracks and sends

The editable project model owns an ordered, bounded vector of return tracks.
Each has a project-wide `ReturnId`, a validated name, linear gain in `0..=2`,
mute state and up to 64 generic native effect envelopes. Regular tracks own
sparse send levels keyed by `ReturnId`; zero removes a send, targets are unique,
and each track can address at most the 16 supported returns. Send levels use the
same finite `0..=2` linear gain domain as track and return gain.

Return IDs and effect `DeviceId`s share the project's monotonic global ID space
with tracks, clips, scenes, notes and assets. Candidate edits preserve atomic
history behavior. Removing a return removes every send that targets it in the
same edit; removing a regular track drops its owned sends. Snapshot validation
checks limits, ID uniqueness/high-water, names, gain ranges, duplicate/missing
send targets and effect limits before project construction.

Schema 17 requires `return_tracks` and a `sends` array on each regular track.
Return effect state uses the same opaque generic envelope as regular effects.
Schemas 1–16 strictly decode with empty return and send collections. Unknown
device types and state payloads remain preserved without a dependency from
`hex-project` or `hex-storage` on the DSP registry.

Playback now implements the bounded acyclic topology regular track → Return →
Main. Each regular source and effect chain is rendered once. A send taps the
post-track-fader signal; each Return sums its input, processes its registered
effect chain and matrix, applies Return gain/mute, then joins Main before the
master gain is applied once. Returns have no source, instrument, Session cells,
send targets or feedback path. Arrangement, SessionPlayback and prepared linear
export share this compiled routing contract. A zero-send project keeps exact
pre-routing Arrangement PCM.

Return effects are compiled through the native effect registry. Stateful return
processors must explicitly report `Some(0)` algorithmic latency; unknown and
nonzero latency refuse candidate compilation before editor history is committed.
Matrix-only effects are memoryless zero-latency transforms. This is a narrow
admission rule, not general path compensation. Return chains and fixed 256-frame
scratch are constructed on control. Compatible bus IDs inherit processor
history after full preflight; new buses start fresh, seek/stop reset chains just
like regular effects, and loop wrap keeps their history continuous. Filter
outputs continue to render on zero input so tails decay.

Prepared revisions count stateful regular-track and Return effects together
with registered mutable-instrument runtime declarations against the shared
active graph count and paired-byte caps. This check runs before a candidate can
commit editor history, and SessionPlayback repeats it defensively before
constructing runtimes.

At a revision boundary, sends from a removed regular track stop immediately;
its existing release tails keep the current Main-only behavior. An audible
removed or changed Return retains its whole old effect chain and captured bus
matrix/gain/mute as a Main-only residual tail; old sends are not replayed. An
unchanged chain transfers its history once, including for a gain/mute-only bus
edit. The 64-slot Return tail bank shares processor, paired-runtime-byte and
chain-entry limits with Arrangement effect tails. See
[Return effect residual tails](return-effect-tails.md) for the handoff, reset,
capacity and transaction contract. Allocation probes cover effectful Return
rendering, replacement and control-side retirement; they do not establish
physical-device deadlines or listening acceptance. Pinned return controls and
device interactions are described below.

The Pulse2178 UI puts a compact, internally scrollable return section directly
above the pinned Main row and below the independently scrolling Arrangement
tracks. It caps the section at 67 logical pixels (header plus two rows), so a
16-return project preserves usable Arrangement space at 880×540 with the detail
pane open; all returns remain reachable by scrolling inside the section. Return
focus selects its device view without changing the selected regular track or
project-wide time-selection scope. Returns can be added, removed, muted and
gain-adjusted. Regular-track send menus list named returns and expose one compact numeric slider
and value field per return, with bounded 0..2 linear gain and three displayed
fractional digits. Pointer movement and valid text entry share one session-wide,
typed send-gesture target with device parameter controls: updates are transient
prepared previews and a successful release/commit creates one canonical history
edit. The menu finalizes only its source track's send drafts when that source scrolls
out of view; valid drafts commit once, invalid drafts cancel back to the canonical
value with a lowercase error, and editor keyboard focus is restored. If the
terminal edit is refused, the draft stays accessible and the source row is scrolled
back into view. Removing a source/return cancels and retires matching cached send
controls. These controls remain bounded to 0..2; setting zero removes the sparse
send. This continuous drag/type lifecycle is Pulse2179. Automated UI coverage
checks popup bounds at 880×540, input geometry after scroll, valid/invalid offscreen
draft handling, focus restoration and stale-owner retirement; native pointer,
keyboard and audible acceptance remains open.

Return device view accepts native effects, uses the same descriptor-driven
parameter controls and device-gesture finalization/acknowledgement as regular
effects, and supports bypass, removal and same-return reordering. Return-only
arrangement controls and device-rail rendering live in focused editor modules,
keeping the shared device panel and Arrangement modules from accumulating another
large return-specific branch. Browser
instrument drops and cross-owner device moves are refused with a visible reason.
The audio dependency is integrated into this branch. Session service acceptance
now covers add/send, undo/redo, Utility insertion and parameter patches, bypass,
same-return reorder/removal, stale gesture cancellation, and schema-17
save/reopen with PCM comparisons. UI fixtures cover stale numeric drafts when a
return is removed and ensure clip/sample drops over the pinned return area clear
their target and preview rather than resolving a regular track. The regular
track's single instrument remains freely movable in its chain. Focused UI/session
tests, affected strict Clippy, formatting and source guards pass. Full clean-head
local CI and native UI/listening acceptance remain open; the user performs the
native interaction and audition checks.

The initial three-run 100-track/16-return routing profile was repeated at clean
audio commit `5c8d3e5`; all runs retain PCM digest
`2d913f742d1d6255`. The repeated p50 values are 211.1–226.0 µs, p99 values
267.5–295.3 µs and maxima 308.2–335.6 µs. These are short release microbench
samples and do not establish physical audio deadline or listening acceptance.
Logs and manifest are retained at
`artifacts/profiles/return-routing-20261010/committed-5c8d3e5/` in the primary
checkout; they supplement, rather than replace, the original dirty-source
profile evidence.
