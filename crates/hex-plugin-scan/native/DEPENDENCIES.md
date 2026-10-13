# inspected build and foreign boundary

SDK superproject3cdf9ca5d1f5b1b21e0a86832aa4abe55607bd96 identifies3.8.1.
Pinned basefcf9da0b, cmake054c9143, pluginterfaces4f547e8e and
public.sdk586dc5e6 are checked by the offline builder with tracked changes refused.
Root/submodule MIT notices and hashes are retained in every controlled receipt.
The original C++ helper uses SDK IPluginFactory/PFactoryInfo/PClassInfo interfaces;
it does not link sdk_hosting, VSTGUI, an audio backend or a Rust VST wrapper.
Its original Linux loader owns dlopen -> ModuleEntry -> GetPluginFactory,
factory release -> ModuleExit -> dlclose. Success is emitted only after cleanup.
Factory-only fixtures deliberately implement no component processing/editor.

The builder compiles with C++17/O2 and all warnings as errors. MD dependency
files record every emitted original/SDK/system header digest (211before the
negative-control launcher was added). SDK source and system header roots are
explicitly checked. Compiler binary identity, compiler version, exact commands,
resolved helper runtime-library hashes and system package copyright notices are
retained. Standard compiler/CRT/kernel interfaces are system dependencies, not
newly installed libraries. Runtime/system license texts include libstdc++6,
libgcc-s1, libc6 and linux-libc-dev; MIT SDK licensing is not a claim that these
system notices disappear. No new Hex distribution license or native packaging
policy is chosen. No SDK copy/binary is committed; a separately supplied pinned
checkout is required, with no build-script network fetch.

Rust serde1.0.229/serde_json1.0.151, sha2.0.10.9 and rustix1.1.5 reuse existing
locked sources. Hashing is scoped to scanner identity and rustix process APIs to
this standalone executable. Signal-hook0.3.18/default-features=false is the only
new registry package; its existing signal-hook-registry1.4.8 installs a zeroed
sigaction with SA_RESTART|SA_SIGINFO, without inherited SA_NOCLDWAIT. The audited
safe flag API's handler only stores an AtomicBool. Its MIT/Apache2.0 license
files remain in the pinned crate distribution. A dedicated native launcher proves
actual ignored/NOCLDWAIT auto-reaping, then the scanner's repaired direct-child
and descendant ownership. This is not permission to modify process-global
signals/subreaping inside the primary app.

Process-group signals happen only while the direct PID remains unreaped;
therefore its group identity cannot be recycled between observation and signal.
Successful std process_group(0) spawn verifies child-side group setup.
WNOWAIT preserves the zombie lease; wait/descendant reaping happens after scoped
TERM/KILL. A pending cleanup lease stays owned by the live supervisor; no total
termination guarantee is made for OS-stuck tasks. Production third-party and
installed-directory scanning stay refused pending OS containment review. A
caller-owned controlled build receipt is the trust root, not cryptographic proof
that hostile actors cannot forge files or escape groups. The finite Linux tests
prove the original controlled fixtures only.

The ORIGINAL offline component helper adds only pinned SDK interface headers:
IComponent/IAudioProcessor/IEditController/IConnectionPoint, IHostApplication/
IComponentHandler/IMessage/IAttributeList, IBStream/IParameterChanges and
ProcessContext requirements. All original implementations remain separately built
C++17/O2, with no SDK hosting source, GUI, callback Rust wrapper, audio backend or
new package. A child control thread owns setup/state/teardown; one joined thread
owns processing. Host/interface/state/queue references stay inside this child.
All artifact runtime closures now enter the retained union audit. The existing
safe Rust supervisor adds bounded duplex writes; its process-global subreaper
still runs ONLY in the standalone supervisor, never embedded in primary Hex.
