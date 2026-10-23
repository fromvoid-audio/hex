# Bounded binary research tools

`HexSelectedFunction.java` is an original Ghidra research script, not shipped
application code. Hex's application/runtime remains Rust. This helper disassembles
one selected entry, establishes its function if needed, and asks the decompiler
for a result with a 30-second timeout. It does not run whole-program analysis.
Its output includes language, address, inferred body, entry flow and warnings;
it creates a new file and refuses to overwrite an existing export.

Use only entries tied to the behavior questions in research/README.md.
Pass output paths under ignored `research/private/`, and keep the imported binary,
project database, pseudocode and logs there. Do not copy recovered code into Hex.
A successful export establishes tool execution, not algorithm correctness: types,
function boundaries and control flow may remain ambiguous.

Current tested toolchain: Homebrew Ghidra 12.1.4, OpenJDK 21.0.12.1, macOS arm64.
The headless launcher is `/opt/homebrew/opt/ghidra/libexec/support/analyzeHeadless`;
the JDK is `/opt/homebrew/opt/openjdk@21`. Use its `-import` mode once, then
`-process` on the existing private project; use `-noanalysis`, `-scriptPath
research/tools`, `-postScript HexSelectedFunction.java <address> <private-new-file>`
and `-max-cpu 2`. The local analysis project and exact provenance are recorded in
research/observations/fade-controller-001.md. Never restart an import simply
because an observation times out; inspect the existing process/project first.

Primary sources: [Ghidra 12.1.4 release](https://github.com/NationalSecurityAgency/ghidra/releases/tag/Ghidra_12.1.4_build),
[versioned installation guide](https://github.com/NationalSecurityAgency/ghidra/blob/Ghidra_12.1.4_build/GhidraDocs/GettingStarted.md),
and [Homebrew formula](https://formulae.brew.sh/formula/ghidra).
