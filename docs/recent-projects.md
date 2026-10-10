# Recent projects

Pulse ticket 39 tracks this focused request. The native File menu lists up to ten
successfully opened project paths, most recent first. Selecting an entry routes
through the same project-replacement confirmation and open worker as Open Project.
Opening a missing or invalid file reports the existing error without promoting or
removing its entry. Merely saving a new project does not add it to this list.

Menu actions carry the native path, so reordering the list cannot redirect an
existing action to a different project. Display labels are lowercase through the
UI text boundary; the original path bytes are retained. Duplicate filenames show
parent context. No project metadata, history or schema changes are required.

`hex-preferences` owns a separate bounded, versioned recent-projects document.
The UI loads and saves it through the background executor, coalescing newer states
behind an in-flight save. Atomic temporary-file publication and malformed-file
refusal protect existing preferences. A failed preferences operation remains
visible; it must not silently discard project work or overwrite malformed data.

Source review and targeted checks passed: 13 preference tests, focused recent-menu
and worker-response regressions, strict preference/UI Clippy, formatting and
architecture/GPUI/style guards. Startup merging evicts oldest entries within both
the ten-path and 8 KiB native-path budget; new opens take precedence over loaded
entries. Full local CI evidence belongs to the focused PR. Native acceptance
remains open.
The user checks File > recent projects after opening two projects, confirms MRU
ordering and reopening, restarts the review app and checks persistence, then tries
a missing project and confirms the existing open error and retained list. With an
edited project, a recent-project click must retain replacement protection.
