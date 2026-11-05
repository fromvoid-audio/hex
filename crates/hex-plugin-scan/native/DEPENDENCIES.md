# native dependency notices

VST3 SDK 3.8.1 superproject `3cdf9ca5d1f5b1b21e0a86832aa4abe55607bd96`
and its exact submodule pins are checked by `build.py`: base `fcf9da0b`,
cmake `054c9143`, pluginterfaces `4f547e8e`, public.sdk `586dc5e6`.
Provide the pinned clean SDK separately; no SDK copy/binary or automatic fetch
is included.

Controlled build receipts retain root/submodule MIT notices, compiler/header/
runtime identities and system notices, including libstdc++6, libgcc-s1, libc6
and linux-libc-dev. SDK licensing does not grant a license for third-party
plugin content or choose Hex's distribution license. Locked Rust dependencies
retain their own notices in their source distributions.

For commands use `python3 crates/hex-plugin-scan/native/build.py --help` and
`cargo run -p hex-plugin-scan --locked` (prints usage and exits nonzero without arguments). Protocol and ownership
contracts belong beside their original Rust/C++ implementations.
