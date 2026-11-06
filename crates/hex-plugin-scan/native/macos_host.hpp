// Native-helper host context. Bounded metadata transactions are not graph/DSP ACKs.
#pragma once
#include "macos_host_messages.hpp"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivsthostapplication.h"
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
    struct Parameter { ParameterInfo info{}; ParamValue value=0; };
    // Rich strings belong to the main SDK lane. Allocate only the validated
    // count, retaining the previous inventory until a full refresh succeeds.
    std::unique_ptr<Parameter[]> parameters;
    uint32 count=0;
    uint64 revision=0;
};
// Copied DSP-facing facts contain no controller pointers or strings. Main owns
// the rich snapshot; its one processing reader uses these published copies.
struct ProcessingParameterSnapshot {
    struct Parameter { ParamID id=0; int32 flags=0; };
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
    struct Value { ParamID id=0; ParamValue normalized=0; };
    // The per-epoch feedback obligation is independent of the inventory.
    std::array<ParamID,HOST_FEEDBACK_COUNT> ids{};
    std::array<ParamValue,HOST_FEEDBACK_COUNT> normalized{};
    uint32 count=0;
    Value at(uint32 index) const { return {ids[index],normalized[index]}; }
    bool put(ParamID id,ParamValue value) {
        if(id>0x7FFFFFFFu || !std::isfinite(value) || value<0 || value>1) return false;
        for(uint32 n=0;n<count;++n) if(ids[n]==id) { normalized[n]=value;return true; }
        if(count==ids.size()) return false;
        ids[count]=id;normalized[count++]=value;return true;
    }
};
inline constexpr size_t HOST_EDIT_COUNT=8;
enum EditKind : uint32 { Restart=0, Begin=1, Perform=2, End=3 };
// sequence orders all main callbacks. gesture names the exact Begin sequence;
// normalized is meaningful only for Perform. A queued record is not a DSP ACK.
struct EditEvent {
    uint64 sequence=0,gesture=0;
    ParamValue normalized=0;
    ParamID id=0;
    uint32 kind=Restart;
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
    UnsupportedCapability=128, UnsupportedEdit=256, MessagePoolFull=512, MetadataCapacity=1024, InvalidEdit=2048 };

// One instance's control owner. Title/value refresh and ordered parameter edits
// are admitted here. IO,
// latency/reload/routing require actual graph preparation and remain explicitly
// unavailable until the product's owning runtime/bridge/PDC contracts exist.
// Each accepted callback queues one ordered request, never recursively calls the
// controller or graph, and acknowledges queuing rather than DSP application.
class Host final : public IHostApplication, public IComponentHandler {
    std::atomic<uint32> references{1}, refusals{0};
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
    size_t first=0, count=0, restart_count=0;
    uint64 sequence=0;
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
            result.parameters[n]={parameter.info.id,parameter.info.flags};
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
    Host() {
        for (auto& message:messages) message.bind(static_cast<IHostApplication*>(this));
    }
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
    tresult PLUGIN_API beginEdit(ParamID id) override { return admit_edit(id,Begin,0); }
    tresult PLUGIN_API performEdit(ParamID id,ParamValue value) override { return admit_edit(id,Perform,value); }
    tresult PLUGIN_API endEdit(ParamID id) override { return admit_edit(id,End,0); }
private:
    tresult admit_edit(ParamID id,uint32 kind,ParamValue value) {
        if(![NSThread isMainThread]) {refuse(WrongThread);return kResultFalse;}
        if(!accepting) {refuse(Retiring);return kResultFalse;}
        if(dispatching) {recursive=true;refuse(RecursiveRestart);return kResultFalse;}
        const bool valid_value=kind!=Perform || (std::isfinite(value) && value>=0 && value<=1);
        if(applying_saved_state && id<=0x7FFFFFFFu && valid_value) return kResultOk;
        const ParameterSnapshot::Parameter* info=nullptr;
        for(uint32 n=0;n<metadata.count;++n) if(metadata.parameters[n].info.id==id) info=&metadata.parameters[n];
        if(!info || (kind!=End && (info->info.flags&(ParameterInfo::kIsReadOnly|ParameterInfo::kIsHidden))) ||
            !valid_value) {
            refuse(InvalidEdit);return kInvalidArgument;
        }
        size_t active=active_tokens.size(),vacant=active_tokens.size();
        for(size_t n=0;n<active_tokens.size();++n) {
            if(active_tokens[n] && active_ids[n]==id) active=n;
            if(!active_tokens[n]) vacant=n;
        }
        if((kind==Begin && active!=active_tokens.size()) || (kind!=Begin && active==active_tokens.size())) {
            refuse(InvalidEdit);return kResultFalse;
        }
        if(count==requests.size() || sequence==std::numeric_limits<uint64>::max() ||
            (kind==Begin && vacant==active_tokens.size())) {refuse(QueueFull);return kResultFalse;}
        const auto next=sequence+1;
        const auto gesture=kind==Begin?next:active_tokens[active];
        requests[(first+count)%requests.size()]={next,gesture,value,id,kind};
        ++count;sequence=next;pending_edits.fetch_add(1,std::memory_order_release);
        if(kind==Begin) {active_ids[vacant]=id;active_tokens[vacant]=gesture;}
        if(kind==End) active_tokens[active]=0;
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
        const auto supported=supported_restarts |
            (latency_enabled?kLatencyChanged:0);
        if (flags & ~supported) {
            constexpr int32 graph=kReloadComponent|kIoChanged|kLatencyChanged|kPrefetchableSupportChanged|
                                  kRoutingInfoChanged|kParamIDMappingChanged;
            refuse(flags & graph?GraphControlRequired:UnsupportedCapability);
            return kNotImplemented;
        }
        const auto refresh=flags & supported_restarts;
        if ((refresh && (count==requests.size() || restart_count==HOST_RESTART_COUNT)) ||
            sequence==std::numeric_limits<uint64>::max()) {
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
            if(written==std::numeric_limits<uint64>::max() || written-read==edits.size()) {
                result.disposition=HostDisposition::Pending;return result;
            }
            edits[written%edits.size()]=request;
            edits_written.store(written+1,std::memory_order_release);
            first=(first+1)%requests.size();--count;
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
            if(!staged.put(value.id,value.normalized)) {
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
        dispatching=true;recursive=false;
        struct Leave {
            bool& dispatching;bool& feedback;
            ~Leave(){dispatching=false;feedback=false;}
        } leave{dispatching,applying_feedback};
        for(uint32 n=0;n<staged.count;++n) {
            const auto value=staged.at(n);
            uint32 index=0;
            while(index<metadata.count && metadata.parameters[index].info.id!=value.id) ++index;
            if(index==metadata.count) return false;
            applying_feedback=true;
            const auto updated=controller->setParamNormalized(value.id,value.normalized);
            applying_feedback=false;
            if(updated!=kResultOk) return false;
            const auto normalized=controller->getParamNormalized(value.id);
            if(!std::isfinite(normalized) || normalized<0 || normalized>1 || recursive) return false;
            observations[n]={index,normalized};
        }
        for(uint32 n=0;n<staged.count;++n)
            metadata.parameters[observations[n].index].value=observations[n].normalized;
        ++metadata.revision;return true;
    }
    // One serialized processing consumer; main never overwrites an unread
    // record. Full handoff defers dispatch, preserving main request FIFO.
    bool peek_edit(EditEvent& result) const {
        const auto read=edits_read.load(std::memory_order_relaxed);
        if(read==edits_written.load(std::memory_order_acquire)) return false;
        result=edits[read%edits.size()];return true;
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
        accepting=false;first=0;count=0;restart_count=0;
        active_tokens.fill(0);pending_edits.store(0,std::memory_order_release);
    }
};
static_assert(sizeof(Host)+HOST_MESSAGE_COUNT*sizeof(HostMessage)+
    2*HOST_PARAMETER_COUNT*sizeof(ParameterSnapshot::Parameter)<5*1024*1024,
    "native host and both controller inventories stay below the declared five MiB bound");
} // namespace hex_instance
