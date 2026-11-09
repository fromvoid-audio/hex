// Native-helper host context. Bounded metadata transactions are not graph/DSP ACKs.
#pragma once
#include "macos_host_messages.hpp"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivsthostapplication.h"
#include <CommonCrypto/CommonDigest.h>
#include <algorithm>
#include <limits>
#include <memory>
#include <new>
#include <utility>

namespace hex_instance {
// Rich metadata lives in the helper. This bound admits controllers exposing
// full per-channel MIDI mappings alongside their ordinary parameter inventory.
inline constexpr size_t HOST_PARAMETER_COUNT=2624;
inline constexpr size_t HOST_FEEDBACK_COUNT=128;
inline constexpr size_t HOST_RESTART_COUNT=8;
// A single native editor event can synchronously report a group of gestures.
// Reserve the burst on main without changing PCM handoff or receipt sizes.
inline constexpr size_t HOST_REQUEST_COUNT=64;
struct ParameterSnapshot {
    struct Parameter { ParameterInfo info{}; ParamValue value=0; uint64 editor_sequence=0; };
    // Rich strings belong to the main SDK lane. Allocate only the validated
    // count, retaining the previous inventory until a full refresh succeeds.
    std::unique_ptr<Parameter[]> parameters;
    uint32 count=0;
    uint64 revision=0;
    std::array<unsigned char,32> descriptor_signature{};
};
// Main-lane display-cache identity only. Encode validated semantic fields,
// including enumeration order, rather than SDK struct padding. Runtime values,
// edit sequences and observation revisions deliberately do not participate.
inline std::array<unsigned char,32> parameter_descriptor_signature(const ParameterSnapshot& snapshot) {
    CC_SHA256_CTX hash{};CC_SHA256_Init(&hash);
    const auto number=[&](uint64 value,size_t bytes) {
        std::array<unsigned char,8> encoded{};
        for(size_t n=0;n<bytes;++n) encoded[n]=static_cast<unsigned char>(value>>(8*n));
        CC_SHA256_Update(&hash,encoded.data(),static_cast<CC_LONG>(bytes));
    };
    const auto text=[&](const String128& value) {
        for(size_t n=0;n<128 && value[n];++n) number(static_cast<uint16>(value[n]),2);
        number(0,2);
    };
    number(1,4); // Signature encoding version, separate from the saved format.
    number(snapshot.count,4);
    for(uint32 n=0;n<snapshot.count;++n) {
        const auto& info=snapshot.parameters[n].info;
        number(info.id,4);number(static_cast<uint32>(info.flags),4);
        number(static_cast<uint32>(info.stepCount),4);number(static_cast<uint32>(info.unitId),4);
        uint64 bits=0;std::memcpy(&bits,&info.defaultNormalizedValue,8);number(bits,8);
        text(info.title);text(info.shortTitle);text(info.units);
    }
    std::array<unsigned char,32> result{};CC_SHA256_Final(result.data(),&hash);return result;
}
// Copied DSP-facing facts contain no controller pointers or strings. Main owns
// the rich snapshot; its one processing reader uses these published copies.
struct ProcessingParameterSnapshot {
    struct Parameter {
        ParamID id=0;
        int32 flags=0;
        int32 steps=0;
        bool accepts_authored(uint32 expected_steps) const {
            constexpr uint32 supported=ParameterInfo::kCanAutomate|ParameterInfo::kIsWrapAround|
                ParameterInfo::kIsList|ParameterInfo::kIsBypass;
            const auto bits=static_cast<uint32>(flags);
            return (bits&ParameterInfo::kCanAutomate) && !(bits&~supported) &&
                static_cast<uint32>(steps)==expected_steps;
        }
    };
    std::array<Parameter,HOST_PARAMETER_COUNT> parameters{};
    uint32 count=0;
    uint64 revision=0;
    const Parameter* find(ParamID id) const {
        const auto end=parameters.begin()+count;
        const auto found=std::lower_bound(parameters.begin(),end,id,
            [](const Parameter& value,ParamID target) { return value.id<target; });
        return found!=end && found->id==id?&*found:nullptr;
    }
};
// Latest processor observations, coalesced by stable ID. This is controller
// synchronization, not an authored gesture or callback-delivery acknowledgement.
struct ParameterFeedback {
    struct Value { ParamID id=0; ParamValue normalized=0; uint64 editor_sequence=0; };
    // The per-epoch feedback obligation is independent of the inventory.
    std::array<Value,HOST_FEEDBACK_COUNT> values{};
    uint32 count=0;
    Value at(uint32 index) const { return values[index]; }
    bool put(ParamID id,ParamValue value,uint64 editor_sequence=0) {
        if(id>0x7FFFFFFFu || !std::isfinite(value) || value<0 || value>1) return false;
        const Value next{id,value,editor_sequence};
        for(uint32 n=0;n<count;++n) if(values[n].id==id) { values[n]=next;return true; }
        if(count==values.size()) return false;
        values[count++]=next;return true;
    }
};
inline constexpr size_t HOST_EDIT_COUNT=8;
enum EditKind : uint32 { Restart=0, Begin=1, Perform=2, End=3, GroupBegin=4, GroupEnd=5, GroupAborted=6 };
inline bool is_group_marker(uint32 kind) { return kind>=GroupBegin && kind<=GroupAborted; }
// sequence orders all main callbacks. gesture names the exact Begin sequence;
// normalized is meaningful only for Perform. A queued record is not a DSP ACK.
struct EditEvent {
    uint64 sequence=0,gesture=0;
    ParamValue normalized=0;
    ParamID id=0;
    uint32 kind=Restart;
    bool standalone_begin=false;
    // Main-only grouping before handoff. Wire receivers recover membership
    // from the explicit ordered marker records instead of a fake ParamID.
    uint64 cohort=0;
    // Inclusive unread width in the atomically published worker batch.
    size_t batch_remaining=1;
};
enum class HostDisposition { Idle, Refreshed, InvalidMetadata, NoController, Recursive, Retiring, WrongThread, Capacity, Pending, EditQueued };
struct HostDispatch {
    uint64 sequence=0, metadata_revision=0;
    int32 flags=0;
    HostDisposition disposition=HostDisposition::Idle;
    EditEvent edit{};
};
enum HostRefusal : uint32 { WrongThread=1, UnknownRestart=2, GraphControlRequired=4,
    QueueFull=8, RecursiveRestart=16, InvalidMetadata=32, Retiring=64,
    UnsupportedCapability=128, UnsupportedEdit=256, MessagePoolFull=512, MetadataCapacity=1024, InvalidEdit=2048,
    UnknownEditParameter=1u<<12, ReadOnlyEditParameter=1u<<13, InvalidEditValue=1u<<14,
    DuplicateEditBegin=1u<<15, MissingPerformBegin=1u<<16, MissingEndBegin=1u<<17,
    FeedbackParameterMissing=1u<<18, FeedbackSetterRefused=1u<<19, FeedbackValueInvalid=1u<<20 };

// One instance's control owner. Title/value refresh and ordered parameter edits
// are admitted here. IO,
// latency/reload/routing require actual graph preparation and remain explicitly
// unavailable until the product's owning runtime/bridge/PDC contracts exist.
// Callbacks acknowledge queuing rather than DSP application. When a native
// drag saturates the main FIFO, replace a superseded, undispatched value for
// that gesture. A standalone value can supersede another complete standalone
// edit. Explicit gesture boundaries and restart records are never replaced.
class Host final : public IHostApplication, public IComponentHandler, public IComponentHandler2 {
    std::atomic<uint32> references{1}, refusals{0};
    HostMessageDiagnostics message_trace{};
    std::array<HostMessage,HOST_MESSAGE_COUNT> messages{};
    std::array<EditEvent,HOST_REQUEST_COUNT> requests{};
    std::array<EditEvent,HOST_EDIT_COUNT> edits{};
    std::atomic<uint64> edits_read{0},edits_written{0};
    std::atomic<uint32> pending_edits{0};
    // The serialized reader opts this instance in. Only the main SDK lane
    // admits and covers restart intent. The ordinal outlives receipt reads.
    std::atomic<bool> observes_latency_restarts{false};
    std::atomic<uint64> pending_latency_restart{0};
    std::array<ParamID,HOST_EDIT_COUNT> active_ids{};
    std::array<uint64,HOST_EDIT_COUNT> active_tokens{};
    size_t first=0, count=0, restart_count=0, active_count=0;
    uint64 sequence=0;
    const bool advertises_editor_groups;
    uint64 cohort=0;
    bool refused_cohort=false;
    std::array<uint64,HOST_EDIT_COUNT> cohort_gestures{};
    size_t end_reservations() const { return active_count+(cohort?1:0); }
    std::atomic<bool> accepting{true};
    std::atomic<pthread_t> processing_thread{nullptr};
    bool dispatching=false, recursive=false, applying_feedback=false, applying_saved_state=false;
    ParameterSnapshot metadata{};
    // Two fixed copies, one main producer and one serialized processing reader.
    // Main may coalesce an unread Ready snapshot, never a Reading one. A raced
    // scan defers the existing restart request; neither lane waits or allocates.
    enum SnapshotPhase : uint8_t { Free, Writing, Ready, Reading };
    struct ProcessingSlot {
        std::atomic<uint8_t> phase{Free};
        ProcessingParameterSnapshot snapshot{};
    };
    std::array<ProcessingSlot,2> processing_snapshots{};
    struct FeedbackSlot {
        std::atomic<uint8_t> phase{Free};
        ParameterFeedback snapshot{};
    } feedback;
    static ProcessingParameterSnapshot compact(const ParameterSnapshot& source) {
        ProcessingParameterSnapshot result{};result.count=source.count;result.revision=source.revision;
        for(uint32 n=0;n<source.count;++n) {
            const auto& parameter=source.parameters[n];
            result.parameters[n]={parameter.info.id,parameter.info.flags,parameter.info.stepCount};
        }
        std::sort(result.parameters.begin(),result.parameters.begin()+result.count,
            [](const auto& left,const auto& right) { return left.id<right.id; });
        return result;
    }
    bool publish_processing_snapshot(const ProcessingParameterSnapshot& next) {
        for(auto& slot:processing_snapshots) {
            auto phase=slot.phase.load(std::memory_order_acquire);
            if((phase!=Free && phase!=Ready) ||
                !slot.phase.compare_exchange_strong(phase,Writing,std::memory_order_acq_rel)) continue;
            slot.snapshot=next;
            slot.phase.store(Ready,std::memory_order_release);return true;
        }
        return false;
    }
    void refuse(uint32 why) { refusals.fetch_or(why,std::memory_order_relaxed); }
    static bool valid_text(const String128 value) {
        for (size_t n=0;n<128;++n) {
            const auto c=static_cast<uint16>(value[n]);
            if (!c) return true;
            if (c>=0xD800 && c<=0xDBFF) {
                if (++n>=128) return false;
                const auto next=static_cast<uint16>(value[n]);
                if (next<0xDC00 || next>0xDFFF) return false;
            } else if (c>=0xDC00 && c<=0xDFFF) return false;
        }
        return false;
    }
public:
    static constexpr int32 supported_restarts=kParamValuesChanged|kParamTitlesChanged;
    explicit Host(bool trace_messages=false,bool editor_groups=false)
        : advertises_editor_groups(editor_groups) {
        for (auto& message:messages)
            message.bind(static_cast<IHostApplication*>(this),trace_messages?&message_trace:nullptr);
    }
    const HostMessageDiagnostics& message_diagnostics() const { return message_trace; }
    uint32 reference_count() const { return references.load(); }
    uint32 refusal_flags() const { return refusals.load(); }
    void bind_processing_thread() { processing_thread.store(pthread_self(),std::memory_order_release); }
    // A synchronous SDK state setter may echo parameter edits. The saved
    // bytes already define the processor state and the controller inventory
    // is refreshed after the setters. Do not stage these echoes as live edits.
    template <typename Setter>
    tresult apply_saved_state(Setter set) {
        struct StateApplication {
            bool& active;
            explicit StateApplication(bool& flag):active(flag) { active=true; }
            ~StateApplication() { active=false; }
        } scope(applying_saved_state);
        return set();
    }
    void observe_latency_restarts() {
        observes_latency_restarts.store(true,std::memory_order_release);
    }
    // Only main invokes this after sampling final startup facts, before any
    // processing lane exists. Live observation remains explicitly opt-in.
    void finish_startup_latency_observation_on_control() {
        observes_latency_restarts.store(false,std::memory_order_release);
        pending_latency_restart.store(0,std::memory_order_release);
    }
    uint64 latency_restart() const {
        return pending_latency_restart.load(std::memory_order_acquire);
    }
    void cover_latency_restart(uint64 covered) {
        // Activation may have synchronously caused a newer restart callback.
        pending_latency_restart.compare_exchange_strong(covered,0,std::memory_order_acq_rel);
    }
    uint32 PLUGIN_API addRef() override { return ++references; }
    uint32 PLUGIN_API release() override {
        const auto remaining=--references;
        if (!remaining) delete this;
        return remaining;
    }
    tresult PLUGIN_API queryInterface(const TUID iid,void** out) override {
        if (!out) return kInvalidArgument;
        *out=nullptr;
        if (!iid) return kInvalidArgument;
        if (same(iid,INLINE_UID_OF(FUnknown)) || same(iid,INLINE_UID_OF(IHostApplication)))
            *out=static_cast<IHostApplication*>(this);
        else if (same(iid,INLINE_UID_OF(IComponentHandler))) *out=static_cast<IComponentHandler*>(this);
        else if (advertises_editor_groups && same(iid,INLINE_UID_OF(IComponentHandler2)))
            *out=static_cast<IComponentHandler2*>(this);
        else return kNoInterface;
        addRef(); return kResultOk;
    }
    tresult PLUGIN_API getName(String128 name) override {
        if (![NSThread isMainThread] || !name) return kInvalidArgument;
        std::memset(name,0,sizeof(String128));
        const char16_t label[]=u"hex"; std::memcpy(name,label,sizeof(label)); return kResultOk;
    }
    tresult PLUGIN_API createInstance(TUID cid,TUID iid,void** out) override {
        if (!out) return kInvalidArgument;
        *out=nullptr;
        const bool message=same(cid,INLINE_UID_OF(IMessage)) && same(iid,INLINE_UID_OF(IMessage));
        if (![NSThread isMainThread]) {
            const auto processing=processing_thread.load(std::memory_order_acquire);
            if(!message || (processing && pthread_equal(processing,pthread_self()))) {
                refuse(WrongThread); return kResultFalse;
            }
        }
        if (!accepting) { refuse(Retiring); return kResultFalse; }
        if (!cid || !iid) return kInvalidArgument;
        const bool attributes=same(cid,INLINE_UID_OF(IAttributeList)) && same(iid,INLINE_UID_OF(IAttributeList));
        if (!message && !attributes) return kNoInterface;
        for (auto& slot:messages) if (slot.acquire(attributes)) {
            *out=attributes?static_cast<void*>(static_cast<IAttributeList*>(&slot)):
                            static_cast<void*>(static_cast<IMessage*>(&slot));
            return kResultOk;
        }
        refuse(MessagePoolFull); return kOutOfMemory;
    }
    tresult PLUGIN_API beginEdit(ParamID id) override { return admit_grouped_edit(id,Begin,0); }
    tresult PLUGIN_API performEdit(ParamID id,ParamValue value) override { return admit_grouped_edit(id,Perform,value); }
    tresult PLUGIN_API endEdit(ParamID id) override { return admit_grouped_edit(id,End,0); }
    tresult PLUGIN_API setDirty(TBool) override { return kNotImplemented; }
    tresult PLUGIN_API requestOpenEditor(FIDString) override { return kNotImplemented; }
    tresult PLUGIN_API startGroupEdit() override {
        if(!advertises_editor_groups) return kNotImplemented;
        if(![NSThread isMainThread]) {refuse(WrongThread);return kResultFalse;}
        if(!accepting) {refuse(Retiring);return kResultFalse;}
        if(dispatching) {recursive=true;refuse(RecursiveRestart);return kResultFalse;}
        if(applying_saved_state) return kResultOk;
        if(cohort) {poison_cohort();refuse(InvalidEdit);return kResultFalse;}
        if(sequence>std::numeric_limits<uint64>::max()-2 || !make_edit_room(2)) {
            refuse(QueueFull);return kResultFalse;
        }
        cohort=++sequence;refused_cohort=false;cohort_gestures.fill(0);
        requests[(first+count++)%requests.size()]={sequence,cohort,0,0,GroupBegin,false,cohort};
        pending_edits.fetch_add(1,std::memory_order_release);
        return kResultOk;
    }
    tresult PLUGIN_API finishGroupEdit() override {
        if(!advertises_editor_groups) return kNotImplemented;
        if(![NSThread isMainThread]) {refuse(WrongThread);return kResultFalse;}
        if(!accepting) {refuse(Retiring);return kResultFalse;}
        if(dispatching) {recursive=true;refuse(RecursiveRestart);return kResultFalse;}
        if(applying_saved_state) return kResultOk;
        if(!cohort) {refuse(InvalidEdit);return kResultFalse;}
        // Start reserved this terminal's record and one ordinal. Parameter
        // admission cannot spend either reservation.
        ++sequence;
        const auto terminal=refused_cohort?GroupAborted:GroupEnd;
        if(terminal==GroupAborted) {
            for(auto& active:active_tokens)
                if(active && std::find(cohort_gestures.begin(),cohort_gestures.end(),active)!=cohort_gestures.end()) {
                    active=0;--active_count;
                }
        }
        requests[(first+count++)%requests.size()]={sequence,cohort,0,0,terminal,false,cohort};
        cohort=0;refused_cohort=false;
        pending_edits.fetch_add(1,std::memory_order_release);
        return terminal==GroupEnd?kResultOk:kResultFalse;
    }
private:
    tresult admit_grouped_edit(ParamID id,uint32 kind,ParamValue value) {
        if([NSThread isMainThread] && cohort && refused_cohort && kind!=End) return kResultFalse;
        const auto result=admit_edit(id,kind,value);
        if(result!=kResultOk && [NSThread isMainThread] && cohort) poison_cohort();
        return result;
    }
    void poison_cohort() {
        refused_cohort=true;
        // Refuse the pending cohort prefix before main can publish it. Any
        // older worker batches keep their exact delivered receipts.
        for(size_t n=0;n<count;) {
            const auto& request=requests[(first+n)%requests.size()];
            if(request.cohort==cohort && request.kind==Perform) {
                erase_request(n);
                pending_edits.fetch_sub(1,std::memory_order_acq_rel);
            } else ++n;
        }
    }
    bool cohort_room(size_t needed) const {
        if(!cohort) return true;
        size_t width=0;
        for(size_t n=count;n>0;--n) {
            if(requests[(first+n-1)%requests.size()].cohort!=cohort) break;
            ++width;
        }
        // One record remains available for finish/abort even at saturation.
        return width+needed+1<=edits.size();
    }
    bool remember_cohort_gesture(uint64 gesture) {
        if(!cohort) return true;
        for(auto& member:cohort_gestures) {
            if(member==gesture) return true;
            if(!member) {member=gesture;return true;}
        }
        return false;
    }
    void erase_request(size_t index) {
        for(size_t next=index+1;next<count;++next)
            requests[(first+next-1)%requests.size()]=requests[(first+next)%requests.size()];
        --count;
    }
    bool replace_queued_perform(const EditEvent& next) {
        // Main owns this FIFO even while the worker drains its separate
        // handoff. Append at the tail to retain increasing receipt sequences.
        for(size_t n=count;n>0;--n) {
            const auto& previous=requests[(first+n-1)%requests.size()];
            if(previous.kind==Restart || is_group_marker(previous.kind) || previous.cohort!=next.cohort) break;
            if(previous.kind!=Perform || previous.gesture!=next.gesture) continue;
            erase_request(n-1);
            requests[(first+count++)%requests.size()]=next;
            return true;
        }
        return false;
    }
    bool make_edit_room(size_t needed) {
        // An older value can be dropped only if its replacement is still in
        // this main-owned FIFO and precedes any intervening restart.
        for(size_t n=0;count+end_reservations()+needed>requests.size();) {
            if(n>=count) return false;
            const auto current=requests[(first+n)%requests.size()];
            bool superseded=false;
            if(current.kind==Perform) for(size_t later=n+1;later<count;++later) {
                const auto& next=requests[(first+later)%requests.size()];
                if(next.kind==Restart || is_group_marker(next.kind) || next.cohort!=current.cohort) break;
                if(next.kind==Perform && next.gesture==current.gesture) {
                    superseded=true;break;
                }
            }
            if(superseded) {
                erase_request(n);
                pending_edits.fetch_sub(1,std::memory_order_acq_rel);
            } else ++n;
        }
        return true;
    }
    bool replace_standalone_edit(ParamID id) {
        // Only replace a complete triplet still owned by the main FIFO, in
        // the same restart segment and after the last explicit edit for id.
        for(size_t n=count;n>0;--n) {
            const auto& end=requests[(first+n-1)%requests.size()];
            if(end.kind==Restart || is_group_marker(end.kind) || end.cohort!=cohort) break;
            if(end.id!=id) continue;
            if(end.kind!=End || n<3) break;
            const auto& begin=requests[(first+n-3)%requests.size()];
            if(!begin.standalone_begin || begin.gesture!=end.gesture) break;
            for(size_t record=0;record<3;++record) erase_request(n-3);
            return true;
        }
        return false;
    }
    tresult admit_standalone_edit(ParameterSnapshot::Parameter& info,ParamValue value) {
        constexpr size_t records=3;
        if(sequence>std::numeric_limits<uint64>::max()-records-(cohort?1:0)) {
            refuse(QueueFull);return kResultFalse;
        }
        // Some controllers send a valid value without beginEdit. Present it
        // as a complete one-shot gesture to the existing DSP/receipt path.
        const auto id=info.info.id;
        if(!cohort_room(records)) {refuse(QueueFull);return kResultFalse;}
        const bool has_room=make_edit_room(records);
        if(!has_room && !replace_standalone_edit(id)) return kResultFalse;
        const auto gesture=sequence+1;
        requests[(first+count++)%requests.size()]={gesture,gesture,0,id,Begin,true,cohort};
        requests[(first+count++)%requests.size()]={gesture+1,gesture,value,id,Perform,false,cohort};
        requests[(first+count++)%requests.size()]={gesture+2,gesture,0,id,End,false,cohort};
        sequence=gesture+2;
        info.editor_sequence=gesture+1;
        if(has_room) pending_edits.fetch_add(records,std::memory_order_release);
        return kResultOk;
    }
    tresult admit_edit(ParamID id,uint32 kind,ParamValue value) {
        if(![NSThread isMainThread]) {refuse(WrongThread);return kResultFalse;}
        if(!accepting) {refuse(Retiring);return kResultFalse;}
        if(dispatching) {recursive=true;refuse(RecursiveRestart);return kResultFalse;}
        const bool valid_value=kind!=Perform || (std::isfinite(value) && value>=0 && value<=1);
        if(applying_saved_state && id<=0x7FFFFFFFu && valid_value) return kResultOk;
        ParameterSnapshot::Parameter* info=nullptr;
        for(uint32 n=0;n<metadata.count;++n) if(metadata.parameters[n].info.id==id) info=&metadata.parameters[n];
        if(!info) {refuse(InvalidEdit|UnknownEditParameter);return kInvalidArgument;}
        if(kind!=End && (info->info.flags&(ParameterInfo::kIsReadOnly|ParameterInfo::kIsHidden))) {
            refuse(InvalidEdit|ReadOnlyEditParameter);return kInvalidArgument;
        }
        if(!valid_value) {refuse(InvalidEdit|InvalidEditValue);return kInvalidArgument;}
        size_t active=active_tokens.size(),vacant=active_tokens.size();
        for(size_t n=0;n<active_tokens.size();++n) {
            if(active_tokens[n] && active_ids[n]==id) active=n;
            if(!active_tokens[n]) vacant=n;
        }
        if(kind==Begin && active!=active_tokens.size()) {refuse(InvalidEdit|DuplicateEditBegin);return kResultFalse;}
        if(kind!=Begin && active==active_tokens.size()) {
            if(kind==Perform) return admit_standalone_edit(*info,value);
            refuse(InvalidEdit|MissingEndBegin);return kResultFalse;
        }
        if(sequence>=std::numeric_limits<uint64>::max()-(cohort?1:0)) {refuse(QueueFull);return kResultFalse;}
        const auto next=sequence+1;
        const auto gesture=kind==Begin?next:active_tokens[active];
        const EditEvent event{next,gesture,value,id,kind,false,cohort};
        // Reserve one slot for each admitted gesture's End. A new Begin
        // consumes a record and adds a reservation. End spends its own.
        const auto available=requests.size()-count-end_reservations();
        if(kind!=Begin && !remember_cohort_gesture(gesture)) {refuse(QueueFull);return kResultFalse;}
        if(kind==Begin && (vacant==active_tokens.size() || !make_edit_room(2))) return kResultFalse;
        if(kind!=Perform && !cohort_room(1)) {refuse(QueueFull);return kResultFalse;}
        if(kind==Begin && !remember_cohort_gesture(gesture)) {refuse(QueueFull);return kResultFalse;}
        if(kind==Perform && (!available || !cohort_room(1))) {
            if(replace_queued_perform(event)) {
                info->editor_sequence=next;
                sequence=next;
                return kResultOk;
            }
            if(!cohort_room(1) || !make_edit_room(1)) {refuse(QueueFull);return kResultFalse;}
        }
        requests[(first+count)%requests.size()]=event;
        ++count;sequence=next;pending_edits.fetch_add(1,std::memory_order_release);
        if(kind==Perform) info->editor_sequence=next;
        if(kind==Begin) {active_ids[vacant]=id;active_tokens[vacant]=gesture;++active_count;}
        if(kind==End) {active_tokens[active]=0;--active_count;}
        return kResultOk;
    }
public:
    tresult PLUGIN_API restartComponent(int32 flags) override {
        if (![NSThread isMainThread]) { refuse(WrongThread); return kResultFalse; }
        if (!accepting) { refuse(Retiring); return kResultFalse; }
        const bool latency_enabled=observes_latency_restarts.load(std::memory_order_acquire);
        // A setter may report changed latency while applying host feedback.
        // This records intent only. Metadata getter recursion still refuses.
        if (dispatching && !(applying_feedback && latency_enabled && (flags & kLatencyChanged))) {
            recursive=true; refuse(RecursiveRestart); return kResultFalse;
        }
        const uint32 bits=static_cast<uint32>(flags);
        if (bits & ~uint32{0xFFF}) { refuse(UnknownRestart); return kInvalidArgument; }
        // State setters run before the first processing epoch. The current
        // host profile has no saved vendor parameter bindings to remap, and
        // startup refreshes the inventory after applying the state.
        if (applying_saved_state) flags &= ~kParamIDMappingChanged;
        if (!flags) return kResultOk;
        if(cohort) {
            poison_cohort();refuse(UnsupportedCapability);return kResultFalse;
        }
        const auto supported=supported_restarts |
            (latency_enabled?kLatencyChanged:0);
        if (flags & ~supported) {
            constexpr int32 graph=kReloadComponent|kIoChanged|kLatencyChanged|kPrefetchableSupportChanged|
                                  kRoutingInfoChanged|kParamIDMappingChanged;
            refuse(flags & graph?GraphControlRequired:UnsupportedCapability);
            return kNotImplemented;
        }
        const auto refresh=flags & supported_restarts;
        if ((refresh && (count+end_reservations()==requests.size() || restart_count==HOST_RESTART_COUNT)) ||
            sequence>=std::numeric_limits<uint64>::max()-(cohort?1:0)) {
            refuse(QueueFull); return kResultFalse;
        }
        const auto ordinal=++sequence;
        if(refresh) {
            requests[(first+count)%requests.size()]={ordinal,0,0,static_cast<ParamID>(refresh),Restart};
            ++count;++restart_count;
        }
        if(flags & kLatencyChanged) pending_latency_restart.store(ordinal,std::memory_order_release);
        return kResultOk;
    }
    // At most one request and 128 info/value reads per call. A complete staged
    // snapshot publishes once on main; malformed/reentrant reads keep the prior
    // accepted snapshot. Stable ParamIDs cannot be silently remapped by a refresh.
    HostDispatch dispatch(IEditController* controller) {
        HostDispatch result{}; result.metadata_revision=metadata.revision;
        if (![NSThread isMainThread]) { refuse(WrongThread); result.disposition=HostDisposition::WrongThread; return result; }
        if (!accepting) { result.disposition=HostDisposition::Retiring; return result; }
        if (dispatching) { recursive=true; refuse(RecursiveRestart); result.disposition=HostDisposition::Recursive; return result; }
        if (!count) return result;
        const auto request=requests[first];
        result.sequence=request.sequence;
        if(request.kind!=Restart) {
            const auto written=edits_written.load(std::memory_order_relaxed);
            const auto read=edits_read.load(std::memory_order_acquire);
            size_t width=1;
            if(request.cohort) while(width<count &&
                    requests[(first+width)%requests.size()].cohort==request.cohort) ++width;
            if(width>edits.size() || written>std::numeric_limits<uint64>::max()-width ||
                    width>edits.size()-(written-read)) {
                result.disposition=HostDisposition::Pending;return result;
            }
            for(size_t n=0;n<width;++n) {
                auto next=requests[(first+n)%requests.size()];
                next.batch_remaining=width-n;
                edits[(written+n)%edits.size()]=next;
            }
            // One frontier makes the whole producer cohort visible to PCM.
            edits_written.store(written+width,std::memory_order_release);
            first=(first+width)%requests.size();count-=width;
            result.edit=request;result.disposition=HostDisposition::EditQueued;return result;
        }
        result.flags=static_cast<int32>(request.id);
        result=refresh_controller_on_control(controller,result);
        if(result.disposition!=HostDisposition::Pending) {
            first=(first+1)%requests.size();--count;--restart_count;
        }
        return result;
    }
    // Startup restoration must rescan actual controller values without
    // fabricating a plug-in restart request or IComponentHandler edit.
    HostDispatch refresh_on_control(IEditController* controller) {
        HostDispatch result{};result.metadata_revision=metadata.revision;
        return refresh_controller_on_control(controller,result);
    }
private:
    HostDispatch refresh_controller_on_control(IEditController* controller,HostDispatch result) {
        if (![NSThread isMainThread]) { refuse(WrongThread);result.disposition=HostDisposition::WrongThread;return result; }
        if (!accepting) { result.disposition=HostDisposition::Retiring;return result; }
        if (dispatching) { recursive=true;refuse(RecursiveRestart);result.disposition=HostDisposition::Recursive;return result; }
        if (!controller) { refuse(UnsupportedCapability); result.disposition=HostDisposition::NoController; return result; }
        dispatching=true; recursive=false;
        struct Leave { bool& flag; ~Leave() { flag=false; } } leave{dispatching};
        ParameterSnapshot staged{};
        const auto total=controller->getParameterCount();
        if (total>static_cast<int32>(HOST_PARAMETER_COUNT)) {
            refuse(MetadataCapacity); result.disposition=HostDisposition::Capacity; return result;
        }
        bool valid=total>=0;uint32 bypasses=0;
        if (valid) staged.count=static_cast<uint32>(total);
        if (staged.count) {
            staged.parameters.reset(new(std::nothrow) ParameterSnapshot::Parameter[staged.count]);
            if(!staged.parameters) {
                refuse(MetadataCapacity); result.disposition=HostDisposition::Capacity; return result;
            }
        }
        for (uint32 i=0;valid && i<staged.count;++i) {
            auto& parameter=staged.parameters[i];
            valid=controller->getParameterInfo(static_cast<int32>(i),parameter.info)==kResultOk;
            if (!valid) break;
            const auto& info=parameter.info;
            valid=valid_text(info.title) && valid_text(info.shortTitle) && valid_text(info.units) &&
                  info.id<=0x7FFFFFFFu && info.stepCount>=0 && std::isfinite(info.defaultNormalizedValue) &&
                  info.defaultNormalizedValue>=0 && info.defaultNormalizedValue<=1;
            valid=valid && !((info.flags&ParameterInfo::kIsReadOnly) && (info.flags&ParameterInfo::kCanAutomate)) &&
                (!(info.flags&ParameterInfo::kIsHidden) || (info.flags&ParameterInfo::kIsReadOnly));
            if(info.flags&ParameterInfo::kIsBypass) valid=valid && ++bypasses==1;
            if (valid) {
                parameter.value=controller->getParamNormalized(info.id);
                valid=std::isfinite(parameter.value) && parameter.value>=0 && parameter.value<=1;
                if(i<metadata.count && metadata.parameters[i].info.id==info.id)
                    parameter.editor_sequence=metadata.parameters[i].editor_sequence;
                else for(uint32 prior=0;prior<metadata.count;++prior) if(metadata.parameters[prior].info.id==info.id) {
                    parameter.editor_sequence=metadata.parameters[prior].editor_sequence;break;
                }
            }
        }
        if (metadata.revision && staged.count!=metadata.count) valid=false;
        if (recursive) { result.disposition=HostDisposition::Recursive; return result; }
        if (!valid) { refuse(InvalidMetadata); result.disposition=HostDisposition::InvalidMetadata; return result; }
        if (metadata.revision==std::numeric_limits<uint64>::max()) {
            refuse(MetadataCapacity); result.disposition=HostDisposition::Capacity; return result;
        }
        staged.revision=metadata.revision+1;
        const auto next=compact(staged);
        const auto same_id=[](const auto& left,const auto& right) { return left.id==right.id; };
        const auto end=next.parameters.begin()+next.count;
        bool valid_ids=std::adjacent_find(next.parameters.begin(),end,same_id)==end;
        if (valid_ids && metadata.revision) {
            const auto prior=compact(metadata);
            valid_ids=std::equal(next.parameters.begin(),end,prior.parameters.begin(),same_id);
        }
        if (!valid_ids) {
            refuse(InvalidMetadata); result.disposition=HostDisposition::InvalidMetadata; return result;
        }
        if(!publish_processing_snapshot(next)) {
            result.disposition=HostDisposition::Pending;return result;
        }
        staged.descriptor_signature=parameter_descriptor_signature(staged);
        metadata=std::move(staged);
        result.metadata_revision=metadata.revision; result.disposition=HostDisposition::Refreshed;
        return result;
    }
public:
    // Native-control borrowed view only, valid until next dispatch or retirement.
    // A transport consumer must copy/validate its bounded data before crossing IPC.
    const ParameterSnapshot* snapshot() const { return [NSThread isMainThread]?&metadata:nullptr; }
    ProcessingParameterSnapshot processing_snapshot_on_control() const {
        return [NSThread isMainThread]?compact(metadata):ProcessingParameterSnapshot{};
    }
    void refresh_processing_snapshot(ProcessingParameterSnapshot& current) {
        // The caller is the same single worker that invokes process/flush.
        // Release a slot only after copying; no SDK or channel I/O spans it.
        for(auto& slot:processing_snapshots) {
            uint8_t phase=Ready;
            if(!slot.phase.compare_exchange_strong(phase,Reading,std::memory_order_acq_rel)) continue;
            if(slot.snapshot.revision>current.revision) current=slot.snapshot;
            slot.phase.store(Free,std::memory_order_release);
        }
    }
    // Single serialized producer (worker, or main after its epoch/join barrier);
    // busy control reads leave pending
    // observations with that worker. Replacing Ready merges unread IDs first.
    bool publish_parameter_feedback(ParameterFeedback& pending) {
        if(!pending.count) return true;
        auto phase=feedback.phase.load(std::memory_order_acquire);
        if((phase!=Free && phase!=Ready) ||
            !feedback.phase.compare_exchange_strong(phase,Writing,std::memory_order_acq_rel)) return false;
        auto staged=phase==Ready?feedback.snapshot:ParameterFeedback{};
        for(uint32 n=0;n<pending.count;++n) {
            const auto value=pending.at(n);
            if(!staged.put(value.id,value.normalized,value.editor_sequence)) {
                feedback.phase.store(phase,std::memory_order_release);return false;
            }
        }
        feedback.snapshot=staged;pending.count=0;
        feedback.phase.store(Ready,std::memory_order_release);return true;
    }
    bool synchronize_parameter_feedback(IEditController* controller) {
        if(![NSThread isMainThread] || !accepting || dispatching) return false;
        uint8_t phase=Ready;
        if(!feedback.phase.compare_exchange_strong(phase,Reading,std::memory_order_acq_rel)) return true;
        const auto staged=feedback.snapshot;
        // Never hold a data slot across foreign code. Slow/refused SDK setters
        // cannot stall the producer, and no SDK pointer crosses this handoff.
        feedback.phase.store(Free,std::memory_order_release);
        if(!controller || metadata.revision==std::numeric_limits<uint64>::max()) return false;
        struct Observation { uint32 index=0; ParamValue normalized=0; };
        std::array<Observation,HOST_FEEDBACK_COUNT> observations{};
        uint32 observed=0;
        dispatching=true;recursive=false;
        struct Leave {
            bool& dispatching;bool& feedback;
            ~Leave(){dispatching=false;feedback=false;}
        } leave{dispatching,applying_feedback};
        for(uint32 n=0;n<staged.count;++n) {
            const auto value=staged.at(n);
            uint32 index=0;
            while(index<metadata.count && metadata.parameters[index].info.id!=value.id) ++index;
            if(index==metadata.count) {refuse(FeedbackParameterMissing);return false;}
            // DSP can finish long after the controller has moved
            // on to a newer drag position. Its DSP receipt remains valid.
            if(value.editor_sequence && metadata.parameters[index].editor_sequence>value.editor_sequence) continue;
            auto normalized=value.editor_sequence?controller->getParamNormalized(value.id):-1.0;
            if(normalized!=value.normalized) {
                applying_feedback=true;
                const auto updated=controller->setParamNormalized(value.id,value.normalized);
                applying_feedback=false;
                if(updated!=kResultOk) {refuse(FeedbackSetterRefused);return false;}
                normalized=controller->getParamNormalized(value.id);
            }
            if(!std::isfinite(normalized) || normalized<0 || normalized>1) {
                refuse(FeedbackValueInvalid);return false;
            }
            if(recursive) return false;
            observations[observed++]={index,normalized};
        }
        for(uint32 n=0;n<observed;++n)
            metadata.parameters[observations[n].index].value=observations[n].normalized;
        if(observed) ++metadata.revision;
        return true;
    }
    // One serialized processing consumer; main never overwrites an unread
    // record. Full handoff defers dispatch, preserving main request FIFO.
    bool peek_edit(EditEvent& result,size_t offset=0) const {
        const auto read=edits_read.load(std::memory_order_relaxed);
        if(offset>=edits_written.load(std::memory_order_acquire)-read) return false;
        result=edits[(read+offset)%edits.size()];return true;
    }
    void consume_edit() {
        const auto read=edits_read.load(std::memory_order_relaxed);
        if(read!=edits_written.load(std::memory_order_acquire)) edits_read.store(read+1,std::memory_order_release);
    }
    uint32 ready_parameter_edits() const {
        const auto read=edits_read.load(std::memory_order_relaxed);
        return static_cast<uint32>(edits_written.load(std::memory_order_acquire)-read);
    }
    uint32 pending_parameter_edits() const { return pending_edits.load(std::memory_order_acquire); }
    // Main-only monotonic admission revision also detects an edit that the
    // worker has already completed during a foreign state-read callback.
    uint64 callback_revision_on_control() const { return sequence; }
    void complete_parameter_edit() { pending_edits.fetch_sub(1,std::memory_order_acq_rel); }
    void stop_accepting() {
        if (![NSThread isMainThread]) { refuse(WrongThread); return; }
        accepting=false;first=0;count=0;restart_count=0;active_count=0;cohort=0;refused_cohort=false;
        active_tokens.fill(0);pending_edits.store(0,std::memory_order_release);
    }
};
// Include retained large attributes plus one replacement allocation in each
// concurrently leased message. The small message pool is inline in Host.
static_assert(sizeof(Host)+2*HOST_MESSAGE_COUNT*HOST_BINARY_BYTES+
    2*HOST_PARAMETER_COUNT*sizeof(ParameterSnapshot::Parameter)<6*1024*1024,
    "native host, binary attributes and both inventories stay below six MiB");
} // namespace hex_instance
