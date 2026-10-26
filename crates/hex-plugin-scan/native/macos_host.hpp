// Native-helper host context. Bounded metadata transactions are not graph/DSP ACKs.
#pragma once
#include "macos_host_messages.hpp"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivsthostapplication.h"
#include <limits>

namespace hex_instance {
inline constexpr size_t HOST_PARAMETER_COUNT=128;
inline constexpr size_t HOST_RESTART_COUNT=8;
struct ParameterSnapshot {
    struct Parameter { ParameterInfo info{}; ParamValue value=0; };
    std::array<Parameter,HOST_PARAMETER_COUNT> parameters{};
    uint32 count=0;
    uint64 revision=0;
};
// Copied DSP-facing facts contain no controller pointers or strings. Main owns
// the rich snapshot; its one processing reader uses these published copies.
struct ProcessingParameterSnapshot {
    struct Parameter { ParamID id=0; int32 steps=0,flags=0; };
    std::array<Parameter,HOST_PARAMETER_COUNT> parameters{};
    uint32 count=0;
    uint64 revision=0;
    const Parameter* find(ParamID id) const {
        for(uint32 n=0;n<count;++n) if(parameters[n].id==id) return &parameters[n];
        return nullptr;
    }
};
enum class HostDisposition { Idle, Refreshed, InvalidMetadata, NoController, Recursive, Retiring, WrongThread, Capacity, Pending };
struct HostDispatch {
    uint64 sequence=0, metadata_revision=0;
    int32 flags=0;
    HostDisposition disposition=HostDisposition::Idle;
};
enum HostRefusal : uint32 { WrongThread=1, UnknownRestart=2, GraphControlRequired=4,
    QueueFull=8, RecursiveRestart=16, InvalidMetadata=32, Retiring=64,
    UnsupportedCapability=128, UnsupportedEdit=256, MessagePoolFull=512, MetadataCapacity=1024 };

// One instance's control owner. Only title/value refresh is admitted here. IO,
// latency/reload/routing require actual graph preparation and remain explicitly
// unavailable until the product's owning runtime/bridge/PDC contracts exist.
// Each accepted callback queues one ordered request, never recursively calls the
// controller or graph, and acknowledges queuing rather than DSP application.
class Host final : public IHostApplication, public IComponentHandler {
    std::atomic<uint32> references{1}, refusals{0};
    std::array<HostMessage,HOST_MESSAGE_COUNT> messages{};
    struct Request { uint64 sequence=0; int32 flags=0; };
    std::array<Request,HOST_RESTART_COUNT> requests{};
    size_t first=0, count=0;
    uint64 sequence=0;
    bool accepting=true, dispatching=false, recursive=false;
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
    static ProcessingParameterSnapshot compact(const ParameterSnapshot& source) {
        ProcessingParameterSnapshot result{};result.count=source.count;result.revision=source.revision;
        for(uint32 n=0;n<source.count;++n) {
            const auto& parameter=source.parameters[n];
            result.parameters[n]={parameter.info.id,parameter.info.stepCount,parameter.info.flags};
        }
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
        if (![NSThread isMainThread]) { refuse(WrongThread); return kResultFalse; }
        if (!accepting) { refuse(Retiring); return kResultFalse; }
        if (!cid || !iid) return kInvalidArgument;
        const bool message=same(cid,INLINE_UID_OF(IMessage)) && same(iid,INLINE_UID_OF(IMessage));
        const bool attributes=same(cid,INLINE_UID_OF(IAttributeList)) && same(iid,INLINE_UID_OF(IAttributeList));
        if (!message && !attributes) return kNoInterface;
        for (auto& slot:messages) if (slot.acquire(attributes)) {
            *out=attributes?static_cast<void*>(static_cast<IAttributeList*>(&slot)):
                            static_cast<void*>(static_cast<IMessage*>(&slot));
            return kResultOk;
        }
        refuse(MessagePoolFull); return kOutOfMemory;
    }
    tresult PLUGIN_API beginEdit(ParamID) override { refuse(UnsupportedEdit); return kNotImplemented; }
    tresult PLUGIN_API performEdit(ParamID,ParamValue) override { refuse(UnsupportedEdit); return kNotImplemented; }
    tresult PLUGIN_API endEdit(ParamID) override { refuse(UnsupportedEdit); return kNotImplemented; }
    tresult PLUGIN_API restartComponent(int32 flags) override {
        if (![NSThread isMainThread]) { refuse(WrongThread); return kResultFalse; }
        if (!accepting) { refuse(Retiring); return kResultFalse; }
        if (dispatching) { recursive=true; refuse(RecursiveRestart); return kResultFalse; }
        const uint32 bits=static_cast<uint32>(flags);
        if (!bits || (bits & ~uint32{0xFFF})) { refuse(UnknownRestart); return kInvalidArgument; }
        if (flags & ~supported_restarts) {
            constexpr int32 graph=kReloadComponent|kIoChanged|kLatencyChanged|kPrefetchableSupportChanged|
                                  kRoutingInfoChanged|kParamIDMappingChanged;
            refuse(flags & graph?GraphControlRequired:UnsupportedCapability);
            return kNotImplemented;
        }
        if (count==requests.size() || sequence==std::numeric_limits<uint64>::max()) {
            refuse(QueueFull); return kResultFalse;
        }
        requests[(first+count)%requests.size()]={++sequence,flags};
        ++count;
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
        result.sequence=request.sequence; result.flags=request.flags;
        result=refresh_controller_on_control(controller,result);
        if(result.disposition!=HostDisposition::Pending) { first=(first+1)%requests.size();--count; }
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
        if (total>static_cast<int32>(staged.parameters.size())) {
            refuse(MetadataCapacity); result.disposition=HostDisposition::Capacity; return result;
        }
        bool valid=total>=0;uint32 bypasses=0;
        if (valid) staged.count=static_cast<uint32>(total);
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
            for (uint32 n=0;n<i;++n) if (staged.parameters[n].info.id==info.id) valid=false;
            if (valid) {
                parameter.value=controller->getParamNormalized(info.id);
                valid=std::isfinite(parameter.value) && parameter.value>=0 && parameter.value<=1;
            }
            if (valid && metadata.revision) {
                bool known=false;
                for (uint32 n=0;n<metadata.count;++n) if (metadata.parameters[n].info.id==info.id) known=true;
                valid=known;
            }
        }
        if (metadata.revision && staged.count!=metadata.count) valid=false;
        if (recursive) { result.disposition=HostDisposition::Recursive; return result; }
        if (!valid) { refuse(InvalidMetadata); result.disposition=HostDisposition::InvalidMetadata; return result; }
        if (metadata.revision==std::numeric_limits<uint64>::max()) {
            refuse(MetadataCapacity); result.disposition=HostDisposition::Capacity; return result;
        }
        staged.revision=metadata.revision+1;
        if(!publish_processing_snapshot(compact(staged))) {
            result.disposition=HostDisposition::Pending;return result;
        }
        metadata=staged;
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
    void stop_accepting() {
        if (![NSThread isMainThread]) { refuse(WrongThread); return; }
        accepting=false; first=0; count=0;
    }
};
static_assert(sizeof(Host)+HOST_MESSAGE_COUNT*sizeof(HostMessage)+sizeof(ParameterSnapshot)<1024*1024,
              "native host and staged metadata stay below the declared one MiB bound");
} // namespace hex_instance
