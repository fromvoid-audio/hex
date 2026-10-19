# remembered project directory

Pulse2818 / PR424 remembers one last-successfully-used project folder in the
independent application preference `project-directory.json`. It is separate from
browser Places, recent-project history and sample import/audition directories.
Successful canonical Open or Save completion supplies the actual document parent;
selecting a picker path does not publish a preference. Cancel, declined dirty
confirmation, cancelled Save As, platform errors and failed load/save preserve it.
An actual successful save still counts if a later continuation fails to open.

Open and untitled first Save / explicit Save As use that folder after background
access validation. An unset, missing, non-directory or inaccessible remembered
folder uses the previous picker default. Named Save keeps its actual current
file; suggested names are unchanged. Paths retain native spelling/bytes, without
lowercase display conversion, canonicalization, media moves or folder creation.
The existing preference codec/atomic publication preserves malformed or unsupported
settings with a visible error rather than rewriting them.

The editor loads/validates/persists on background control tasks. One latest pending
value coalesces writes; the owned worker drains it even if the view is removed.
No filesystem I/O happens under its brief control-only mutex or in UI paint/audio.
The fourth preference path is explicit in the existing #414 constructor boundary;
default test paths resolve under its retained disposable environment before jobs
start. Explicit fixtures own their paths. Workers retain the environment through
completion, preserving the same filesystem-job custody contract.

The excluded GPUI patch adds an Open-with-initial-directory adapter with default
ordinary-picker delegation on other platforms. macOS sets NSOpenPanel directoryURL;
Open and Save share an NSURL filesystem-representation helper with NUL refusal.
Original dependency identity, license and prior patches remain. The cumulative
patch is applied to the hash-verified original archive and compared byte-for-byte.

Source checks exercise real Session save/open/history and actual platform request
arguments with injected TestPlatform results. Async directory preparation retains
the existing Open generation/serial and modal admission; stale replacement/closed
window cannot open a late picker, and stale Save responses cannot save the new
document. The real actor replacement probe confirms the existing #421 replacement
cleanup already cancels the old intent; no extra reset or global action change.
A test-only asynchronous gate checks actual pending write completion after view
drop, without blocking executor threads or substituting preference I/O.

These checks do not establish physical native chooser behavior. A Mac-only NSURL
byte/NUL roundtrip shows/configures no panel; actual initial directory presentation,
cancel and overwrite UX still need a coordinator-owned disposable native check.
No user project, running app, permissions, TCC or profile operations are part of
this source task. The sole merger owns final prospective local CI/review/merge;
coordinator owns serial native verification and any later optimized delivery.
