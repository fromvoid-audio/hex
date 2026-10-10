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
its existing release tails keep the current Main-only behavior. Removing a
Return cuts that bus at the boundary and retires its runtime on control. This
bounded policy does not claim Live-equivalent send-tail routing. Allocation
probes cover effectful Return rendering, compatible replacement and control-side
retirement; they do not establish physical-device deadlines or listening
acceptance. UI2178 still owns the pinned rows and native interactions above Main.
