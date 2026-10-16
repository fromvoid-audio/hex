# CLAP foreign-module admission and containment contract

Design revision 1, 15 October 2026. Owning work: Pulse #2523, PR218, ordinary
member of the **safe CLAP hosting on macOS arm64** goal. This is a design for
review, not an execution permit or evidence that containment exists.

## Decision and scope

Foreign means any executable not built as an enrolled ORIGINAL fixture, including
an installed, signed, notarized or user-selected CLAP bundle. Discovery executes
foreign code too: `dlopen` may run constructors before `clap_entry` is inspected.
Admission must therefore finish **before any foreign instruction**, not merely
before instance creation or processing. Unknown capability means refusal.

The first foreign profile is bounded, headless, offline discovery and legacy gain
processing in a disposable macOS guest. The proposed security boundary is an
entire VM, with no host filesystem sharing or network adapter. A fresh process
inside the ordinary desktop user's account is insufficient. App Sandbox/XPC alone
is not selected: neither an XPC connection nor a code signature proves the byte
binding, descendant lifetime and resource contract below. `sandbox-exec`, private
Seatbelt interfaces, SIP changes and a privileged custom kernel component are not
fallbacks. A VM is a proposal requiring a separate implementation/boundary decision;
this document approves no entitlement, policy, dependency or FFI change.

The current foreign execution allowlist is **empty**. The profile below is the
finite qualification target; it becomes supported only for exact measured tuples
after its implementation, security review and every required canary pass. There
is no implicit support for an OS version range. This is independently actionable
while compiler/SDK notices and native fixture acceptance remain blocked.

Production engine, audio, UI, realtime scheduling, native editors, persistent
plugin instances, arbitrary buses/events/state and full #20 acceptance are outside
this profile. Preserve Rust `unsafe_code = "forbid"` and the separately built
native-helper boundary. A future Virtualization.framework adapter belongs outside
the Rust workspace FFI boundary and needs its own explicit decision.

## Initial macOS arm64 profile and prerequisites

`foreign-macos-arm64-offline-v1` targets Apple Silicon hardware, native arm64 host
and guest, macOS 15 or later as an API floor, and thin arm64 Mach-O helpers/modules.
The floor is not an approved build: an admission record must name exact host and
guest product versions, build numbers and Darwin releases, hardware model,
restore-image SHA-256, guest hardware-model data, helper/tool source and executable
digests, configuration digest and canary report digest. Every tuple is initially
unqualified; an update of any field requires requalification. No Rosetta, Intel,
universal/fat binaries, Linux or macOS host-native foreign loading is admitted.

Required public APIs and their limited roles:

| API/prerequisite | Required use; refusal condition |
| --- | --- |
| `VZVirtualMachine.isSupported`, `VZVirtualMachineConfiguration.validate()` | Actual Apple Silicon host support and valid configuration; absence/error refuses before boot. |
| `com.apple.security.virtualization` | Required by Apple for the separate VM owner; absent approval/signing/enrollment refuses. This design does not grant it. |
| `VZMacOSRestoreImage`, `VZMacOSConfigurationRequirements`, `VZMacHardwareModel.isSupported` | Legitimately obtained, pinned restore image and supported hardware model; no download/install during admission. Honor reported minimum CPU/memory; if above this profile's ceilings, refuse. |
| `VZMacOSBootLoader`, `VZMacPlatformConfiguration`, `VZMacMachineIdentifier`, `VZMacAuxiliaryStorage` | Guest boot identity and per-attempt auxiliary storage. No reuse of mutated guest or auxiliary state. |
| `VZVirtualMachineConfiguration.cpuCount`, `.memorySize`, `.storageDevices`, `.networkDevices`, `.directorySharingDevices` | Explicit device inventory, fixed CPU/RAM/storage; network and directory-sharing arrays empty. Never rely on defaults. |
| `VZDiskImageStorageDeviceAttachment(url:readOnly:)`, `VZVirtioBlockDeviceConfiguration` | Read-only input disk plus private fixed-capacity writable guest disk; no raw host device. |
| `VZVirtioSocketDeviceConfiguration`, `VZVirtioSocketDevice.connect(toPort:)` | Candidate host-initiated, attempt-local byte channel to guest port 2523. Actual macOS guest endpoint support must be demonstrated; class availability alone is not proof. Unsupported guest transport refuses; do not substitute TCP/NAT. |
| `VZVirtualMachine.start`, `.stop(completionHandler:)`, `VZVirtualMachineDelegate` | Observe actual start/stop/error on the owner's serial queue. Guest shutdown acknowledgment alone does not prove stop. |
| `openat`, `fstat`, `O_NOFOLLOW`, exclusive create; SHA-256 | Bounded regular-file ingestion and private staging. These alone do not provide immutable custody. |
| `uname`, `sysctlbyname` (`kern.osversion`, `kern.osproductversion`) | Record actual runtime tuple; missing/mismatched values refuse. |
| `dlopen`/`dlsym`/`dlclose`, `_dyld_image_count`, `_dyld_get_image_header`, `_dyld_get_image_name`, `LC_UUID` | Native guest helper only, after admission. UUID enumeration is provenance, not containment or an executable hash. |

Apple's [Virtualization framework](https://developer.apple.com/documentation/virtualization),
[VM configuration](https://developer.apple.com/documentation/virtualization/vzvirtualmachineconfiguration),
[macOS guest installation](https://developer.apple.com/documentation/virtualization/installing-macos-on-a-virtual-machine),
[virtio sockets](https://developer.apple.com/documentation/virtualization/vzvirtiosocketdeviceconfiguration)
and [stop API](https://developer.apple.com/documentation/virtualization/vzvirtualmachine/stop(completionhandler:))
describe the candidate mechanisms. Availability does not establish this security
contract. Guest licensing/notices, exact matching compiler/SDK terms, signing,
storage provisioning and API/transport behavior need actual attributable evidence.
No infrastructure purchase, credential copy or entitlement change follows.

## Executable-byte identity and custody

Trust assumptions are an uncompromised host kernel/hypervisor and trusted VM owner,
broker and enrolled base guest. A hostile module, its threads and all guest
descendants are untrusted. Host administrator compromise is outside the claim;
ordinary third-party module behavior, path replacement and malicious IPC are inside.

1. The trusted ingestion worker reads user-selected bytes as data, never executes
   them, and never recursively follows links. Cap a bundle at 256 regular files,
   256 MiB total and 64 MiB per file; reject special files, hard links, symlinks,
   traversal, case-colliding names, malformed Mach-O and changed ingestion inputs.
   Record the exact relative-path/length/SHA-256 manifest. A digest identifies
   bytes; it does not certify safety, license or publisher trust.
2. Create a new broker-owned input disk from those bytes and enrolled helper
   closure. Hash the final disk and attach it read-only. Keep it inaccessible for
   writes by the user/module throughout VM boot, load and stop. Advisory mode bits,
   a pathname, pre/post hashes, an open descriptor or an operator declaration
   cannot substitute for enforced custody. The proposed custody mechanism is a
   dedicated non-login host service UID, distinct from the desktop UID, with
   broker-owned 0700 staging and no group/other access. The desktop supplies bounded
   input bytes, never a backing-image pathname or writable descriptor. Base images,
   helpers and configuration are enrolled by a trusted provisioning step and are
   not writable through the ingestion interface. The service has no administrator
   authority and exposes only the bounded request protocol below. Its peer
   authentication/service registration needs separate implementation review.
   If another untrusted same-UID writer exists or a hostile writer can alter any
   backing image between validation and consumption, refuse. This account/service
   policy is a proposal, not a claim that it exists or permission to install it.
3. The guest copies no executable onto writable storage for loading. The enrolled
   loader resolves the manifest's exact read-only module/dependency paths and
   verifies hashes before loading. Reject non-system dependencies outside the
   manifest, unresolved `@rpath`/`@loader_path` escapes, environment injection,
   writable executable search paths and unknown executable mappings. System images
   must match the qualified guest baseline. A post-load check alone cannot undo
   a constructor's execution; static closure and enforced custody precede load.
4. The receipt binds module manifest, input disk, helpers, base guest, runtime
   tuple, policy/configuration and attempt nonce. Signature/Team ID/notarization
   metadata is supplementary. No disable-library-validation/JIT entitlement is
   assumed; any required relaxation is a separate decision or this profile refuses.

Original-fixture exclusive custody in PR199 is a different, narrower trust model:
the operator promises no same-UID writer. Its read-only sealing can be reversed by
the owner, who can replace and restore paths between matching hashes. Its original
no-execution byte-swap canary demonstrates that limitation. Do not promote those
receipts or that attestation into hostile-plugin protection.

## Filesystem, network and IPC authority

Foreign code can read the disposable guest OS/runtime, enrolled helpers and exact
read-only module bundle. It can write only disposable guest storage. Treat even
guest-root compromise as contained by the VM device boundary. Expose no host home,
projects, media, preferences, credentials, keychain, clipboard, drag-and-drop,
shared directory, host raw disk, USB, audio device or host GPU passthrough. A minimal
virtual display required for boot conveys no host editor integration. Guest state
and guest OS writes never flow back into the base image or the next attempt.

Network denial means no virtual NIC, NAT, bridge, DNS proxy or network forwarding;
IPv4/IPv6, TCP/UDP, DNS and local-network/loopback access to host services must all
fail. Guest loopback may exist but has no host route. The virtio channel is IPC,
not network authority. No guest-requested connection or host listener is accepted.

The trusted broker connects once to port 2523 on its owned VM socket device. Bind
the channel to that VM instance and nonce, never to a plugin-supplied PID or path.
Allow one request and one terminal reply, versioned length framing, at most 1 MiB
per packet and 4 MiB aggregate per direction. Diagnostics use separate logical
frames, capped at 64 KiB each for stdout/stderr, charged to the aggregate limit.
No path-opening, shell command, URL fetch, file-descriptor/Mach-right transfer,
object deserialization or arbitrary host RPC is exposed. Foreign pointers never
cross the channel. Invalid versions, duplicate terminal replies, unsolicited frames,
overflow, disconnect and nonce mismatch discard all output and trigger cleanup.

All guest responses are hostile, including reports of successful containment or
shutdown. The host validates bounded owned descriptor/PCM/state data; it never
trusts guest assertions as proof of policy enforcement. This profile has no live
audio callback channel, shared host memory or persistent capability token.

## Descendants, resource ceilings and cleanup

One attempt owns one VM and one host VM-owner process. Foreign `fork`, double-fork,
`setsid`, exec and guest services may create descendants **inside** that VM; none
may gain a host execution endpoint. Every guest process dies when actual VM stop
is observed. No reliance on a guest process tree, PGID, cooperative plugin unload,
XPC disconnect or Linux subreaper establishes that guarantee.

| Resource | Initial ceiling and enforcement requirement |
| --- | --- |
| Concurrency | One VM/attempt globally; refuse busy, never silently queue unbounded work. |
| Guest CPU | 2 vCPUs maximum, no overcommit within this profile. A vCPU count is not a host CPU-percentage quota. |
| Guest RAM | 4 GiB fixed, no balloon expansion. This does not bound framework/host overhead. |
| Guest writable storage | Fresh 64 GiB fixed logical disk, no dynamically growing external backing or host sharing; preflight space plus 8 GiB host reserve. Disk-full fails the attempt. |
| Host broker buffers | 8 MiB aggregate IPC/diagnostics storage; packet bounds checked before allocation; no unbounded output accumulation. |
| VM owner resources | 6 GiB resident footprint, 128 threads, 128 open FDs; measured at intervals no longer than 50 ms by a trusted external supervisor. Crossing stops the VM. These are monitored stop thresholds, not hard kernel quotas; overshoot is possible. Qualification must record peak/overshoot and host headroom. A consumer requiring hard host RAM/CPU quotas is unsupported. |
| Time | Boot/handshake 120 s; foreign request 30 s; cleanup observation 5 s. Host monotonic deadlines cover hangs, log floods and guest clock manipulation. |

The 4 GiB guest cap is enforced by VM configuration; any OS minimum above it is
unsupported. Disk creation is separately provisioned, with no automatic purchase
or global cleanup. No claim that `setrlimit(RLIMIT_AS)`, a CLAP buffer charge or a
polling RSS measurement is a hard macOS memory sandbox is permitted.

On success, keep all returned data provisional until protocol validation and actual
VM stop, host-owner exit/reap, handle closure and final custody verification finish.
On timeout/cancel/fault, invalidate the nonce and output first, close IPC, request
VM stop and observe its delegate/state/completion. A graceful guest shutdown is
optional; it never extends the deadline. If stop fails, the trusted supervisor
terminates the owned host owner while preserving PID identity, then must establish
that no VM remains. This fallback itself needs a canary; process death alone is
not assumed to prove VM destruction. A 5 s cleanup overrun enters `cleanup_pending`,
retains the owner/VM lease, blocks new admission and drives observation/reaping.
Do not promise finite completion of an OS-stuck task or delete uncertain evidence.

After verified stop, discard the private writable disk and auxiliary storage, close
all host endpoints, and remove only attempt-owned staging with checked identity.
Cleanup failure remains visible and retryable; preserve existing host documents,
fixture roots, backups and logs. Never reuse a failed guest. Guest plugin teardown
is best effort; it cannot authorize host result publication or resource reuse.

Host process-group TERM/KILL, NOWAIT PID reservation and actual wait/reap are
**supervision**, useful for timeout/crash ownership. They confer no filesystem,
network, same-user process or descendant security. Retain PR199's Linux experiment
as history: UID mapping in the namespace probe failed with a read-only filesystem.
No Linux sandbox was established; no policy change or weaker fallback was approved.

## Admission states and refusal behavior

`closed -> qualifying -> ready -> staging -> booting -> running -> stopping ->
reaped -> publish` is the only success path. `ready` requires the exact approved
tuple, enforced custody and a complete current qualification report. Revalidate
the tuple/configuration/custody at every attempt; qualification is not a caller flag.

| Condition | Required outcome |
| --- | --- |
| No implementation/review/canary evidence; unknown API or policy capability | `closed / unsupported_containment`; no boot or foreign load. |
| Notices, byte custody, closure, signature requirements or runtime identity missing/changed | `refused_identity`; no foreign load, retain prior valid catalog/state. |
| Wrong platform, profile, architecture, OS build, transport or quota prerequisites | `unsupported_profile`; no host-native fallback. |
| Busy, insufficient disk/memory reserve, invalid input or packet budget | `refused_resource` or `refused_input`; preserve original data. |
| Constructor/entry/process failure, timeout, cancellation, malformed result or policy canary failure | `failed`; discard whole descriptor/PCM/state result, stop attempt, invalidate qualification on containment failure. |
| Stop/reap/storage cleanup uncertain | `cleanup_pending`; retain lease/evidence, no new attempt or success publication. |

Errors must name the failing prerequisite and exact attempt/profile without leaking
paths containing credentials or guest output as trusted text. Preserve unknown
saved plugin state as opaque data, never execute it automatically. Re-enabling a
profile requires a new approved report after a security/identity failure; no
retry with widened permissions, stale receipts or inferred capability is allowed.

## Finite qualification and evidence matrix

Run these only in a separately authorized implementation validation effort using
original canary programs first. This issue runs none of them and loads no plugin.
Every row below is **not run / not qualified** for the proposed VM profile.

| ID | Actual experiment required | Pass oracle |
| --- | --- | --- |
| Q1 platform | Exact host/guest build and hardware tuple; missing entitlement/API, unsupported model/architecture and invalid configuration controls | Supported tuple boots without foreign code; every negative refuses before load. |
| Q2 byte binding | Race rename/chmod/replace/truncate backing image and bundle, hardlink/symlink/rpath escape and dependency injection, including constructors | Enforced writes fail or admission refuses before any marker constructor; consumed module/helper bytes match manifest. Pre/post matching hashes alone fail this test. |
| Q3 filesystem | Guest user and guest-root canaries attempt host-home/project/keychain sentinel reads and writes, shared-directory/raw-device discovery; fill guest disk | Host sentinels untouched/unread; no unintended device; bounded disk-full failure. |
| Q4 network | Guest/descendants attempt IPv4/IPv6 TCP, UDP, DNS, LAN, internet and known host-loopback listener | Host-side listeners/capture see no packets/connections; no NIC/forwarder configured. |
| Q5 descendants | Fork, double-fork, setsid, exec and guest-service child persist after loader exit/crash/cancel | Whole VM actually stops; no host process executes canary code and no guest heartbeat survives stop. |
| Q6 resource | CPU/thread/fork/FD/log/IPC floods, memory exhaustion, disk-full and host-owner threshold crossing | Configured caps/broker bounds hold; record peaks, monitor overshoot, host reserve and actual stop. No host OOM or uncontrolled storage growth. |
| Q7 IPC | Guest tries other ports, unsolicited channels, replay/nonces, malformed lengths, duplicates, FD/Mach-right payloads and truncation | Only one bounded owned exchange; all negatives discard result and clean up. Transport unavailable on macOS is refusal. |
| Q8 lifecycle | Boot/request hang, crash at every state, owner crash, stop error, cancellation, host restart and cleanup interruption | No prefix published; retained lease blocks reuse until observed VM death/reap; retry removes only owned staging. |
| Q9 runtime | Compare enrolled/actual runtime images, extra dylib, missing UUID and incomplete unload; mutate SDK stub independently | Distinguish build and runtime evidence; mismatched runtime refuses success, SDK hashes never stand in for loaded images. |
| Q10 original semantics | All eight PR199 `macos_native` ignored tests on legitimate enrolled ORIGINAL artifacts, separately from VM canaries | Actual Mac command/log/result attribution; no ignore removal or inferred pass. |
| Q11 publication | Late plugin/state/teardown/stop failure after otherwise valid PCM/descriptors, previous valid catalog present | Zero new result/state publication; previous catalog/project intact. |

Each report records exact clean source/tree/digest, command, toolchain, host and
guest tuple, VM/helper/config/image/module identities, canary source hashes,
timestamps, exit results, retained log digests, host-side observations and reviewer.
Missing/skipped/unsupported is not pass. Q1–Q9/Q11 prove only the measured security
profile; Q10 proves only original native behavior. Full required local CI remains
separate exact-source implementation evidence. Neither proves realtime deadlines,
CoreAudio/listening, editor lifecycle, optimized delivery or full hosting acceptance.

## Runtime identity and canonical history

PR199 is frozen at `d14fbfc18203d47e2d27df2bc820ef070c9b473d` (tree
`24845a67de6e233bb3f28f8211e81d77810519a0`). Its native runtime snapshot records
the Darwin release and ordered loaded system-image paths/`LC_UUID` values before
load and after joined teardown/unload. UUIDs identify linked images; they are not
SHA-256 hashes of loaded memory or shared-cache files. `shared_cache_uuid` is
explicitly null. Enumeration assumes original non-concurrent-loader phases; it is
not a general hostile-loader attestation API.

SDK `.tbd` files are link-time text stubs, not the system dylib bytes consumed at
runtime. Their hashes belong to build provenance. The controlled module's content
SHA-256 identifies its captured bytes, distinct from both a system-image UUID and
the proof that those exact bytes were loaded. Retained `otool` output, depfiles and
notice hashes do not establish complete hashed linker/tool/SDK closure or the
applicability/completeness of license terms.

Canonical source and original implementation history remain in
[PR199](https://github.com/PORTALSURFER/hex/pull/199) / Pulse #20 and
[PR193](https://github.com/PORTALSURFER/hex/pull/193) / #21. This specification
does not copy, replace, merge or reopen their frozen implementation, remove ignored
tests, release a review hold, or approve module execution. Qualification and any
boundary implementation remain separate work; the foreign allowlist stays empty.
