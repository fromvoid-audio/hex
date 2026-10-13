# controlled VST3 discovery

## Purpose

Own a Linux x86_64 standalone safe-Rust supervisor for explicit original SDK
fixture modules and a separately built native factory probe. This is discovery,
not an audio host. Installed-directory and production third-party scanning remain
refused pending a reviewed OS containment policy. Processes isolate ordinary
crashes; they are not a security sandbox against arbitrary same-user code.

## Dependencies

No workspace crate dependencies. Exact existing serde/serde_json encode bounded
receipts/results. sha2 hashes controlled binary/build identity only. rustix1.1.5
process/event safe APIs implement scoped Linux child ownership, WNOWAIT
observation, subreaping, termination and readiness polling. Hex Rust still forbids
unsafe. Scoped signal-hook0.3.18 (default features off, MIT/Apache2.0) installs
a safe atomic SIGCHLD handler in this standalone executable before spawn,
removing inherited SIG_IGN/SA_NOCLDWAIT auto-reaping. Its existing
signal-hook-registry1.4.8 action flags were audited; the controlled native launcher
proves the negative control and repaired child/descendant ownership. Native SDK-interface loading occurs only in the separate C++ executable,
never in this supervisor, engine or audio process. No Rust VST3 wrapper, audio
backend, GUI toolkit or component/processor/editor interface is adopted.

## Does not own

Plugin processing, realtime queues, buses, automation, latency compensation,
canonical project state, state recovery, editors, provider/audio/root UI,
third-party security containment, Mac builds/signing or full Pulse21 acceptance.

## Controlled build and invocation

`native/build.py --sdk <already-fetched-pinned-sdk> --output <owned-fixture-folder>`
checks the SDK and four submodule commits, rejects tracked source changes, checks
MIT licenses and records compiler, exact commands, all emitted SDK/system headers, compiler binary identity, loaded
helper runtime-library hashes and retained SDK/system license notices. The
controlled receipt ceiling is512KiB (measured notices alone occupy200969B here).
It performs no fetch, SDK installation or installed-plugin scan. SDKroot
3cdf9ca5/public.sdk586dc5e6 are exact pinned3.8.1 inputs. SDK notices are retained
in native/SDK-LICENSE.txt; no Hex distribution license is chosen.

`hex-plugin-scan <owned-folder>/controlled-fixtures.json <good|hang|crash|descendant|discardable> [deadline-ms] [previous-report.json]`
returns bounded JSON. Optional stdin `cancel` cancels the request. Only the
five original factory-only fixtures built from this source are admitted. The
caller-owned build receipt is the explicit trust root, not an authenticity or
security sandbox. Binary hashes bind the attempt, not arbitrary plugin trust.
Pre/post receipt and helper/module identity checks refuse changed inputs. Source
identity/architecture/version checks never load a library. ELF64LE x86_64 ET_DYN
is the only currently supported binary target; no translation fallback.

One fresh helper per request receives minimal environment and its own process
group. Protocol is length-delimited/versioned on a dedicated socket descriptor;
plugin stdout/stderr are separate bounded diagnostics. The probe enumerates
factory class metadata, releases its factory, calls ModuleExit and dlclose before
writing success. It never creates a component/controller/processor/editor.
Factory/class strings are length-delimited, not raw JSON5 interpolation. Unknown
factory flags remain visible; dynamic/discardable or unknown flags never establish
cacheable future class inventory. Vendor strings are self-reported, not provenance.

The direct child is observed with WNOWAIT before termination: its unreaped PID
pins the process-group identity. Scoped TERM/KILL occur before direct reaping;
no later group signal risks a reused PID. The dedicated supervisor subreaps and
reaps controlled descendants. Timeout/cancel/crash/malformed/changed attempts
retain prior valid records. Cleanup exceeding500ms is reported as pending failure
with an owned lease retained by the live supervisor; it continues to drive reaping
without claiming completion. No universal bounded termination for OS-stuck tasks
or protection against escaped hostile processes is promised. This executable
must not be embedded as a process-global subreaper in Hex's primary process.

Real Linux tests require an explicit controlled fixture receipt through
HEX_SCAN_FIXTURE_RECEIPT. Without it native cases are visibly ignored; fake
protocol/identity tests still run. The final exact-source validation must supply
that receipt and retain build plus test provenance. Linux fixture enumeration,
unload and fault containment do not prove production plugin compatibility,
Mac native packaging/signing, processing/state/editor support or physical audio.
