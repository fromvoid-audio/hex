# Private harmonic root-cache proof

This checkpoint extends the frozen b5cb2de private test proposal. No production
metadata is reinterpreted, and no store, provider, engine, lattice, codec or
painter is installed. The prior26 proof tests and negative fixtures remain.

The approved contract separates store-handle namespace from immutable root-owner
namespace. A host must lease the actual source, target and enable owners, dedup
by actual owner identity, and retain them through staged, committed, detached
and retired references until exclusive control reclamation. Existing release
context collection does not establish this new recipe closure.

The private uncompressed location oracle has Current(u32), Retained(u32),
Absent and Detached. The latter two are distinct proof sentinels, not new engine
SharedContextRef variants. Current0..255 encodes0..255; Retained0..16383 encodes
0x8000..0xbfff; Absent/Detached encode0xffff/0xfffe. Other words and out-of-domain
indices refuse. All65536 words are checked. Sentinels round-trip for inactive
metadata but refuse live root resolution. Full namespace/generation/context
keys, receipt, clocks and raw bank/enable words stay separate and unchanged.

Two independent u16 fields replace the four-byte target-location field only in
a NEW test metadata type. Original index/sentinel equivalence is checked before
conversion; uncompressed role and both cached roles are separately checked.
Metadata remains48/slot2456/alignment8. An ownership epoch makes header112.
Actual reserved capacities1142/256/256 charge2817152. The previous measured EQ
30728168 plus this proposed pool is33545320, leaving9112 under33554432. This is
not production installation or universal admission. Original8024 pair and all
prior8312/33581856 overflow controls remain unchanged.

The separate test host catalog explicitly owns Arc<Root> entries. Entry32 and
catalog32 are measured; actual Vec capacity is charged. Each unique newly
allocated Root owner charges its real48-byte Root and16-byte Arc header, plus
the complete real provider declaration (1468 bytes for this v8 fixture).
Same bytes or reused numeric keys do not substitute for actual owner identity.
The declared closure counts each actual provider owner once and every alias
handle. Provider declarations conservatively preserve existing charges even when
canonical input-state Arcs share bytes across distinct provider owners.
It excludes unrelated graphs/assets/static tables and is not a whole playback
physical-memory claim. This model allocates temporary control-side Vecs and Arc
fixture handles; it is NOT a callback-zero-allocation implementation.

For a NEW catalog with capacity256 and THREE actual roots, additional persistent
bookkeeping8224 + owner headers192 leaves only696 of the9112 EQ margin. Capacity
277 leaves24;278 exceeds32MiB by8, before independently charged provider payloads.
With256 distinct new owners, bookkeeping+headers alone cost24608 and exceed the
margin. Reuse of existing host ownership/tables is therefore a required next
interface proposal, not an assumed free map or hidden pointer payload.

All three-root acquisition is preflighted before count/epoch mutation. Actual
owner identity dedup retains separate owners with identical canonical bytes.
Actual real-provider retained owners fill the2MiB declaration budget; acquiring
the three additional roots refuses without mutation, including retired owners.
The proposed2564-byte v9 context gives417*3*2564 >2MiB for fully independent
three-root recipes. The old417 single-target recipe arithmetic is not universal.
Context payload and runtime slot/header/map ledgers remain distinct; no cap or
production admission policy changes here.

Tests cover domain/sentinel boundaries, metadata/full-key equivalence, actual
owner-preserving remap and recreated-owner refusal, wrong format/cache/root,
stale epochs and handles, overflow, store namespace distinct from root namespace,
three-root staged/committed/detached/retired lifetime and alias retention, and
unchanged legacy enable clock/frame/PCM. Root remap does not mutate records or
restart the independent enable transition. Existing interrupted-bank, signedzero,
builtin512-versus64 and rollback/saturation controls remain. No kernel, cross-kind
import, concurrency fence, source v9, runtime root closure or native behavior is
proved. Existing fullCI allocation probes cover existing production routes.

Next: review the actual typed root-lease interface, complete host request capacity,
root ownership transfer/rollback and control-reclamation fencing before a private
production arena or engine hookup. Native amplitude/Normalize/Repeat/range,
painter, persistence, musical/reference and hardware-deadline gaps remain.
Blocked/draft HOLD forbids gate registration/readiness/merge/Done/deploy.
