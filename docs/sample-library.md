# Indexed sample library

Requested 17 October 2026; Pulse #2699 / PR317. The sample index is a catalog of WAV
files from Music and all explicitly added browser folders, including subfolders.
Home remains a project-navigation shortcut and is not scanned implicitly. No new
audio codec support is implied: audio decoding/import keeps its existing WAV
validation. Discovery indexes filenames/paths, not decoded audio metadata or tags.

The Samples section browses configured roots as an inline folder hierarchy. A
nonempty query combines all discovered indexed samples. Folder chips select
that Place as the single active source; its nonempty query narrows indexed matches
to the root subtree. Whitespace-separated search terms all match case-insensitively
against the full path. Rows show filenames only; original native paths distinguish duplicate names
internally and remain available in tooltips. Selection auto-audition, manual Play/Stop, waveform-position and right-arrow
playback, explicit Enter import (sample double-click does not import) and existing
waveform-aware Arrangement/Session drag paths remain shared.
Projects retains its project filter within the inline hierarchy. Adding a folder persists the existing browser
place and triggers a catalog scan without navigating away from Samples.

Place results stay unavailable while the current folder availability scan is pending
or has failed, including cached indexed matches and previously rendered audition/import
controls. A successful current scan restores surviving selection by original path;
source/query changes discard that saved selection. Folder refresh cancels queued/active
Place audition. Scoped current-folder additions use the same case-insensitive,
whitespace-separated full-path terms as the index. Global All composition retains its
existing behavior. These are Pulse #2791 / PR397 source guarantees; optimized native
acceptance remains separate.

Global Command-F/All search composes this catalog with the current bounded folder
snapshot and registered devices. It replaces the previous source selection, deduplicates
original paths, and retains current-folder WAVs outside indexed roots. File rows
share one ordered snapshot for rendering, keyboard counts, preview and device row
offsets. Catalog/folder publications remap the selected file path or device identity
through the same BrowserItemKey contract; unchanged catalogs retain the cached
result snapshot and viewport. Folder publication explicitly invalidates combined
results even if the directory/query did not change. Project search stays bounded
to the current folder; this adds no recursive project scan.

hex-library owns discovery and the versioned derived database, independent of
project persistence/history. GPUI runs load, traversal and cache writes on the
background executor. Startup loads the cache after places load, restricts cached
records to configured roots, then reconciles against disk. Refresh runs after folder
addition, from the refresh icon, and every 30 seconds while the editor exists.
There is one worker in flight and one coalesced refresh flag. Results from outdated
root snapshots are discarded and followed by a scan of current roots. A weak view
handle ends the refresh timer after editor destruction. In-progress filesystem
calls are not cancellable; slow/disconnected volumes can delay that worker, but do
not block UI interaction or audio callbacks.

Each complete scan replaces the derived catalog: new/renamed files appear and
removed files disappear. Unavailable folders remain in preferences and report a
scan warning; their files are absent until a successful rescan. Other folders
continue scanning. Hidden children and file/directory symlinks are skipped;
canonical visited-directory identities avoid cycles and duplicate work across
overlapping roots. Original native paths and casing remain import identities.

Limits per scan: 16 roots, 500,000 inspected entries, 16,384 directories, depth 64,
50,000 retained WAV records, and 16 MiB of aggregate retained/visited path bytes.
A path is capped at 16 KiB. Hitting a limit is visible; a capped traversal is not a
claim to contain all files. Results are sorted by name then path. Visible results
are cached by catalog/search/source and rendered through a virtualized row list.

The database sits next to the browser places file with extension `samples-v1.db`.
Its platform-specific schema-1 binary header records native path encoding, followed
by bounded length-prefixed paths. Loading rejects malformed/truncated/trailing or
future-format data, duplicate/nonabsolute/non-WAV paths, and files over 32 MiB.
Unix bytes and Windows UTF-16 roundtrip losslessly. This is disposable derived
state: a failed load reports rebuilding and a fresh scan may replace it. User
preferences and projects are never replaced by that recovery. Publication uses a
same-directory create-new temporary file, sync, rename and directory sync on Unix;
write failures are visible and leave any previous complete database intact.

Automated acceptance covers recursive/overlapping roots, duplicate names,
search/source combinations, hidden files/symlink cycles, rename/delete/add
reconciliation, offline roots, bounds, malformed cache and native-path roundtrips;
the native browser fixture uses actual indexed samples for existing preview/drag
regressions. Native macOS visual/listening/restart acceptance remains user-owned:
add two folders with nested WAVs, select Samples/all folders, expand/collapse and search/filter,
audition and drag, then add/remove a WAV and refresh; restart and confirm places
and samples restore. Verify Projects expands folders inline. No exact Live parity
or large-library interaction timing claim is made.
