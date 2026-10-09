# Installed reference inventory

Inspected 8 October 2026 using bundle metadata, `file`, `shasum`, and `otool -L`.

| Field | Observed value |
| --- | --- |
| App | `/Applications/Ableton Live 12 Suite.app` |
| Bundle version | `12.4.6 (2026-09-10_0de5c8fa9a)` |
| Executable | `Contents/MacOS/Live` |
| Format | Mach-O universal executable |
| Architectures | x86_64 and arm64 |
| Initial analysis slice | arm64 |
| Executable SHA-256 | `5e8832e06fe92d42ceebac4fad153e44422c90923f426dcb05c760c6d80b3949` |

The arm64 import listing includes CoreAudio, AudioToolbox, CoreMIDI, AppKit,
Metal, Accelerate, and bundled `libonnxruntime_abl.dylib`, `libusb-1.0.dylib`,
and REX. Imports are evidence of linkage, not proof of any particular execution
path or internal architecture. The x86_64 listing additionally includes USFLIB.

The initial discovery found no Ghidra, Rizin, radare2, or llvm-objdump on PATH; no
Ghidra/Hopper/IDA app name was found directly under /Applications. This is a
bounded discovery check, not proof the tools are absent elsewhere.
Apple `otool` is available. Subsequently, Homebrew Ghidra 12.1.4 and OpenJDK
21.0.12.1 were installed for bounded headless research. An ignored arm64 analysis
project now exists, and one fade-controller callback/target was decompiled with
unresolved types/control-flow warnings. See
[FADE-CONTROLLER-001](../../research/observations/fade-controller-001.md); this is
tool/path evidence, not confirmed fade behavior or DSP parity.

Run `sh scripts/inventory-live.sh` after a Live update, record the new hash,
and retain separate fixture provenance for each build. Keep original app files
and analysis databases outside source distribution.
