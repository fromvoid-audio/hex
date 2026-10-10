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

Return tracks have no clips, instrument, Session cells or return-to-return route.
Playback compilation explicitly refuses projects containing a return track or
nonzero send until the Pulse2177 audio graph integration owns routing, class
validation, path latency, bounded processing and lifecycle retirement.

The Pulse2178 UI puts a compact, internally scrollable return section directly
above the pinned Main row and below the independently scrolling Arrangement
tracks. It caps the section at 67 logical pixels (header plus two rows), so a
16-return project preserves usable Arrangement space at 880×540 with the detail
pane open; all returns remain reachable by scrolling inside the section. Return
focus selects its device view without changing the selected regular track or
project-wide time-selection scope. Returns can be added, removed, muted and
gain-adjusted. Regular-track send menus list named returns and expose bounded
0..2 linear gain steps with the current linear value; setting zero removes the
sparse send. Continuous drag/type send editing with one undo step is tracked
separately as Pulse2179, because the current project command is one history edit
per value and must not be called for every pointer update. The bounded step helper
snaps floating-point roundoff near 0 and 2 to exact endpoints while preserving
interior values such as 0.375 without quantization.

Return device view accepts native effects, uses the same descriptor-driven
parameter controls and device-gesture finalization/acknowledgement as regular
effects, and supports bypass, removal and same-return reordering. Return-only
arrangement controls and device-rail rendering live in focused editor modules,
keeping the shared device panel and Arrangement modules from accumulating another
large return-specific branch. Browser
instrument drops and cross-owner device moves are refused with a visible reason.
The playback compiler in the current UI branch still refuses return edits until
the independent Pulse2177 audio integration is merged. Accordingly this source
work does not establish end-to-end add/send/effect playback, callback behavior,
save/reopen integration, or native UI/listening acceptance. Those remain open
for integration and user testing.
