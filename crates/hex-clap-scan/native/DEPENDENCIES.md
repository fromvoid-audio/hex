# native dependency notices

Official free-audio/clap 1.2.10, commit
`195b42a004144fab0b3cf95e9c067187d15365b7`, supplies MIT interface headers.
`CLAP-LICENSE.txt` retains the exact notice. No SDK copy or generated binaries
are checked in; provide the pinned clean SDK separately. The builder performs
no network fetch.

Controlled build receipts retain compiler/header/runtime identities and system
notices, including libstdc++6, libgcc-s1, libc6 and linux-libc-dev. The MIT SDK
notice does not choose Hex's distribution license or license plugin content.
Rust dependencies retain their own notices in the locked crate distributions.

For commands use `python3 crates/hex-clap-scan/native/build.py --help` and
`cargo run -p hex-clap-scan --locked` (prints usage and exits nonzero without arguments). Protocol and ownership contracts
belong beside their original Rust/C++ implementations.
