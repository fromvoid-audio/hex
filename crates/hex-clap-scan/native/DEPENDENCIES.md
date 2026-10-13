# controlled native boundary and closure

Official free-audio/clap1.2.10 commit195b42a004144fab0b3cf95e9c067187d15365b7
supplies MIT interface headers only. CLAP-LICENSE.txt retains the exact notice.
No SDK implementation, Clack, Rust CLAP FFI, GUI framework or audio provider is
linked. The original C++ probe owns dlopen/entry init/borrowed factory and
copied descriptor values/deinit/dlclose. Descriptor/feature pointer arrays remain
inside the child; create_plugin is never called. Failed entry init never gets a
deinit or further CLAP call; instance lifecycle remains unimplemented.

Builder requires an already supplied exact clean SDK checkout, performs no fetch,
and records C++17/O2/Wall/Wextra/Werror commands, compiler binary/version,
all emitted original/CLAP/system headers and runtime closure for every original
artifact. System header roots are checked; libstdc++6/libgcc-s1/libc6/linux-libc-dev
copyright notices and hashes are retained. No distribution license is chosen.
No SDK copy or generated binaries are checked in.

Standalone safe Rust process ownership derives from frozen PR1934262556b,
without editing/extracting that code or adopting a cross-format public API.
Scoped rustix1.1.5 and signal-hook0.3.18(default features off) provide safe APIs;
signal-hook-registry1.4.8's audited action is SA_RESTART|SA_SIGINFO with no
inherited SA_NOCLDWAIT. Its handler only stores an atomic flag. The original
launcher exercises ignored SIGCHLD auto-reap as a negative control. Subreaper
and signals exist only in this standalone executable, not primary/audio code.

Caller-owned receipts are identity/provenance, not arbitrary-code authenticity.
Processes do not establish a security sandbox. Existing UID-map namespace refusal
remains an explicit third-party containment blocker. Native Mac packaging/signing,
installed module discovery and full CLAP hosting acceptance remain unsupported.
