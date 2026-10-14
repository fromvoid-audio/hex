# storage gain compatibility

Schema18 has two published strict shapes. Provenance18 requires
`import_provenance` (including null) and forbids device host-gain fields. Gain18
forbids `import_provenance` and admits validated integer `output_gain_millidb`,
defaulting an absent value to zero. Both retain unknown-field and duplicate-key
refusal. Admit exactly one complete shape; refuse hybrids rather than stripping
fields. A removed provenance key can leave a byte-identical valid gain18 shape:
no reader can recover missing evidence or prove its lineage from those bytes.

Combined schema22 carries all schema21 provenance/routing/feed/gain-lane state
and required host-gain integers for every regular effect, instrument and Return
effect. Original18–21 shapes stay strict. Nonunity host gain selects22, including
bypassed devices; unity preserves existing19/20/21 encoder selection. Explicit
older encoders refuse nonunity gain. Values remain bounded -60000..24000
millidecibels; original IDs, opaque payloads and bypass state are preserved.
The8MiB document ceiling, bounded writer and canonical structural limits remain.

The real host-gain contract prerequisite was three-way reconciled from published
main715e40e into the retained neutral contract, without engine/runtime changes.
Focused contract/storage/session persistence tests and strict affected lint
validate this bounded storage unit only. The unchanged engine is not asserted
to apply new trim/meter semantics; coordinated engine and full-main integration,
prepared PCM parity, exact-full CI and native checks remain required. No meter,
input-capture, Main observer or accessibility implementation is added here.
