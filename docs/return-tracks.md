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

This stage defines metadata and transactions only. Return tracks have no clips,
instrument, Session cells or return-to-return route. Playback compilation
explicitly refuses projects containing a return track or nonzero send until the
Pulse2177 audio graph integration owns routing, class validation, path latency,
bounded processing and lifecycle retirement. UI2178 must render return rows
pinned above the Main row and expose their actual model operations. Neither
audio routing nor native UI/listening acceptance is established here.
