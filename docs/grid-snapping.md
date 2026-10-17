# arrangement grid snapping and append preview

Implemented source contract, 9 October 2026; native acceptance pending. The latest correction keeps the
grid across tracks AND blank Arrangement space, including zero tracks. It
supersedes the briefly requested track-only grid. Snapping is enabled by
default and shares its adaptive spacing/meter origins with the displayed grid;
the toolbar toggles adaptive snapping on/off. There is one adaptive size: an
18 px target spacing, bounded to 256 grid lines. Zooming resolves musical
subdivisions from this shared target for painting and snapping. Fixed sizes
are no longer offered by the toolbar. The control fits its adaptive label.

Pulse #2694 (Hex 0.1) tracks a future arranger context menu for choosing
adaptive size/density. Native review of the simplified toggle remains pending.

On macOS, Command temporarily inverts snapping while dragging: enabled becomes
free, disabled becomes enabled. The Windows reference uses Alt. Modifier changes
during a held gesture update its preview, including at a stationary pointer;
release uses the actual release modifier and the same destination resolver.
Audio free moves preserve frame anchors; MIDI moves preserve beat precision and
original grab offset. Apply these rules to audio/MIDI moves, browser WAV imports
and inserted/appended tracks.

Blank-area drops preview a clip in the prospective new-track row: immediately
below the final row, or at the body top with zero tracks. Its horizontal position
and extent reflect the actual proposed placement. Dropping creates the track
and clip at that location as one compiled edit. Only the last-track bottom
insertion line and clip preview highlight the append destination; the user
removed the blank-area tint on 9 October 2026. Between-track insertion stays line-only, and cursor-following ghosts
remain outside eligible Arrangement destinations. Media preparation stays
bounded/background; unknown duration must remain visibly pending.

## reference and limits

Ableton Live 12's official Arrangement manual, section 6.10, documents adaptive
and fixed grid modes and temporary Command inversion (Alt on Windows):
https://www.ableton.com/en/manual/arrangement-view/#using-the-editing-grid
Its shortcut table also documents Command ignoring quantization while dragging:
https://www.ableton.com/en/manual/live-keyboard-shortcuts/

Installed reference rechecked: 12.4.6 (2026-09-10_0de5c8fa9a), universal arm64/
x86_64 executable. Executable SHA-256:
`5e8832e06fe92d42ceebac4fad153e44422c90923f426dcb05c760c6d80b3949`.
This is a documentation-based independent implementation. Live also documents
object-edge/locator/time-signature snapping; those additional magnetic targets,
full grid shortcuts/context menus, off-grid offset behavior and exact physical
thresholds are not proven by this work. No native Live experiment or decompilation
is needed to implement the documented modifier rule; do not claim full parity.

## acceptance

Verify visual/snap spacing agrees across zoom and tempo/meter changes, with
Command pressed/released mid-drag and at release. Test audio/MIDI, browser
imports, dividers and blank append/zero-track previews, original grab offsets,
precision when free, one Undo and stale/invalid cancellation. Source tests cover
geometry/event routing; user performs native checks and supplies the verdict.
No audio callback/schema change or publication authorization.

## native review checklist

1. Move audio and MIDI clips at several zoom levels: placement should agree
   with visible grid lines. Hold Command before pickup and toggle it during a
   held drag, including while stationary; release should match the preview.
2. Drop a browser WAV below the last track and into a zero-track project: see
   a snapped clip preview on the prospective row and confirm the created clip
   lands there. Repeat while holding Command for a free horizontal position.
3. Move an existing clip into blank space and try between-track insertion.
   Blank space has a locked clip preview; dividers remain line-only.
4. Check one Undo/redo and normal overwrite behavior. Wheel zoom and middle
   panning should retain their previous behavior.

Native results are pending.

## source verification and review build

220 UI tests pass, with one existing ignored. Strict UI Clippy, desktop build,
fmt, architecture/style guards and diff checks pass. Rendered tests cover
audio/MIDI/browser Command held before pickup, changed while stationary and at
release; adaptive visual/snap alignment across tempo/meter/zoom; locked append
and zero-track previews; preserved line-only divider feedback; and navigation
regressions. Modifier state remains separate from stale config guards. Nearest
landmark lookup is bounded across neighboring meter sections and handles seams.
Append previews are clipped to the Arrangement body.

Review app: `artifacts/hex-grid-snapping-20261009.app`. Evidence:
`artifacts/grid-snapping-ui-tests.log` and `grid-snapping-manifest.json`. Source
remains uncommitted; prior running apps and unsaved projects were preserved.
User native interaction/feel and exact reference comparison remain pending.
