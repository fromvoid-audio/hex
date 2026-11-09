// Original fixed-capacity SDK parameter queues. Native-helper storage only;
// no SDK pointer crosses IPC and no queue owns an instance or a graph.
#pragma once
#include "macos_host_messages.hpp"
#include "pluginterfaces/vst/ivstparameterchanges.h"
#include <algorithm>
#include <limits>
#include <thread>

namespace hex_instance {
inline constexpr size_t PARAMETER_POINT_COUNT=64;
inline constexpr size_t DENSE_PARAMETER_SAMPLE_COUNT=256;
struct ParameterPoint {
    int32 offset=0;
    ParamID id=0;
    ParamValue normalized=0;
};

// One instance permanently retains its input and output containers. SDK access
// is confined to the serialized processing/flush call. Host loading/extraction
// uses that same thread before/after the call. Retained references prevent
// reset. Structural misuse and overflow remain sticky. Invalid normalized
// values in plug-in output are individually refused without poisoning PCM.
class ParameterChanges final : public IParameterChanges {
    struct Queue final : public IParamValueQueue {
        ParameterChanges* owner=nullptr;
        ParamID id=kNoParamId;
        std::atomic<uint32> references{1};
        tresult PLUGIN_API queryInterface(const TUID iid,void** out) override {
            if(!out) return kInvalidArgument;
            *out=nullptr;
            if(!owner || !owner->sdk_access()) return kResultFalse;
            if(!same(iid,INLINE_UID_OF(FUnknown)) && !same(iid,INLINE_UID_OF(IParamValueQueue)))
                return kNoInterface;
            *out=static_cast<IParamValueQueue*>(this);addRef();return kResultOk;
        }
        uint32 PLUGIN_API addRef() override { return owner->retain(references); }
        uint32 PLUGIN_API release() override { return owner->release_reference(references); }
        ParamID PLUGIN_API getParameterId() override {
            return owner->sdk_access()?id:kNoParamId;
        }
        int32 PLUGIN_API getPointCount() override {
            return owner->sdk_access()?owner->point_count(id):0;
        }
        tresult PLUGIN_API getPoint(int32 index,int32& offset,ParamValue& value) override {
            if(!owner->sdk_access() || index<0) return kInvalidArgument;
            if(id==owner->dense_id) {
                if(static_cast<size_t>(index)>=owner->dense_count) return kInvalidArgument;
                offset=index;value=owner->dense_values[static_cast<size_t>(index)];return kResultOk;
            }
            for(size_t n=0;n<owner->points_used;++n) {
                const auto& point=owner->values[n];
                if(point.id==id && index--==0) { offset=point.offset;value=point.normalized;return kResultOk; }
            }
            return kInvalidArgument;
        }
        tresult PLUGIN_API addPoint(int32 offset,ParamValue value,int32& index) override {
            if(!owner->sdk_access() || !owner->writable) {
                owner->bad.store(true);return kResultFalse;
            }
            return owner->append(id,offset,value,index)?kResultOk:kResultFalse;
        }
    };
    // One additional ID is reserved exclusively for a dense input lane.
    std::array<Queue,PARAMETER_POINT_COUNT+1> queues{};
    std::array<ParameterPoint,PARAMETER_POINT_COUNT> values{};
    std::array<ParamValue,DENSE_PARAMETER_SAMPLE_COUNT> dense_values{};
    std::atomic<uint32> references{1};
    std::atomic<bool> bad{false};
    std::thread::id thread;
    size_t queues_used=0,points_used=0,dense_count=0;
    ParamID dense_id=kNoParamId;
    int32 sample_frames=0;
    bool writable=false,sdk_active=false;

    bool on_thread() const { return thread==std::this_thread::get_id(); }
    bool sdk_access() {
        if(!on_thread() || !sdk_active) { bad.store(true);return false; }
        return !bad.load();
    }
    uint32 retain(std::atomic<uint32>& refs) {
        auto current=refs.load();
        while(current<std::numeric_limits<uint32>::max())
            if(refs.compare_exchange_weak(current,current+1)) return current+1;
        bad.store(true);return current;
    }
    uint32 release_reference(std::atomic<uint32>& refs) {
        auto current=refs.load();
        while(current>1)
            if(refs.compare_exchange_weak(current,current-1)) return current-1;
        // The container retains its base reference for the complete instance
        // lifetime. A borrowed SDK pointer cannot release that custody.
        bad.store(true);return current;
    }
    int32 point_count(ParamID id) const {
        if(id==dense_id) return static_cast<int32>(dense_count);
        int32 count=0;
        for(size_t n=0;n<points_used;++n) count+=values[n].id==id;
        return count;
    }
    Queue* queue(ParamID id,int32& index,bool reserve_dense=false) {
        index=-1;
        if(id>0x7FFFFFFFu) { bad.store(true);return nullptr; }
        for(size_t n=0;n<queues_used;++n) if(queues[n].id==id) {
            index=static_cast<int32>(n);return &queues[n];
        }
        const auto limit=(reserve_dense || dense_count)?queues.size():PARAMETER_POINT_COUNT;
        if(queues_used==limit) { bad.store(true);return nullptr; }
        auto& result=queues[queues_used];result.id=id;
        index=static_cast<int32>(queues_used++);return &result;
    }
    bool append(ParamID id,int32 offset,ParamValue value,int32& index) {
        index=-1;
        if(offset<0 || offset>=std::max(sample_frames,int32{1}) || id==dense_id) {
            bad.store(true);return false;
        }
        if(!std::isfinite(value) || value<0 || value>1) {
            // Output feedback is optional. Preserve the valid points and the
            // worker's independent PCM and queue-lifetime checks.
            if(!writable) bad.store(true);
            return false;
        }
        int32 parameter_index=0;
        if(!queue(id,parameter_index)) return false;
        index=0;
        for(size_t n=0;n<points_used;++n) {
            auto& previous=values[n];
            if(previous.id!=id) continue;
            if(previous.offset<offset) ++index;
            else if(previous.offset==offset) { previous.normalized=value;return true; }
        }
        if(points_used==values.size()) { bad.store(true);index=-1;return false; }
        // SDK producers may add out of order. Keep each queue sorted, and keep
        // existing equal-offset ID order stable. Duplicate ID/offset is last-wins.
        size_t at=0;
        while(at<points_used && values[at].offset<=offset) ++at;
        for(size_t n=points_used;n>at;--n) values[n]=values[n-1];
        values[at]={offset,id,value};++points_used;return true;
    }
public:
    ParameterChanges() { for(auto& item:queues) item.owner=this; }
    bool exclusive() const {
        if(references.load()!=1) return false;
        for(const auto& item:queues) if(item.references.load()!=1) return false;
        return true;
    }
    // Zero frames is specifically the SDK's stopped parameter-only flush:
    // its only legal offset is zero, not an invented audible sample.
    bool reset(int32 frames,bool output) {
        if(thread==std::thread::id{}) thread=std::this_thread::get_id();
        if(!on_thread() || sdk_active || bad.load() || !exclusive() || frames<0 || frames>8192)
            return false;
        queues_used=points_used=dense_count=0;dense_id=kNoParamId;
        sample_frames=frames;writable=output;return true;
    }
    bool input(ParamID id,int32 offset,ParamValue value) {
        if(!on_thread() || sdk_active || writable || bad.load()) return false;
        int32 index=0;return append(id,offset,value,index);
    }
    // Dense input occupies one point at every local SDK frame. Validate the
    // complete lane before reserving its ID or changing the existing queues.
    bool input_dense(ParamID id,const ParamValue* normalized,size_t count) {
        if(!on_thread() || sdk_active || writable || bad.load() || !normalized ||
            !count || count!=static_cast<size_t>(sample_frames) ||
            count>dense_values.size() || dense_count || id>0x7FFFFFFFu || point_count(id))
            return false;
        for(size_t n=0;n<count;++n)
            if(!std::isfinite(normalized[n]) || normalized[n]<0 || normalized[n]>1) return false;
        int32 index=0;
        if(!queue(id,index,true)) return false;
        std::copy_n(normalized,count,dense_values.begin());
        dense_id=id;dense_count=count;return true;
    }
    // Editing an input before it is loaned to the SDK. Keep the order and
    // bounded backing of every other parameter, including offset-zero points.
    bool erase_input(ParamID id) {
        if(!on_thread() || sdk_active || writable || bad.load() || !exclusive()) return false;
        if(id==dense_id) { dense_id=kNoParamId;dense_count=0; }
        size_t values_kept=0,queues_kept=0;
        for(size_t n=0;n<points_used;++n)
            if(values[n].id!=id) values[values_kept++]=values[n];
        for(size_t n=0;n<queues_used;++n)
            if(queues[n].id!=id) queues[queues_kept++].id=queues[n].id;
        points_used=values_kept;queues_used=queues_kept;return true;
    }
    bool begin_sdk() {
        if(!on_thread() || sdk_active || bad.load() || !exclusive()) return false;
        if(!writable && !dense_count && queues_used==1 && sample_frames>1 &&
            points_used==static_cast<size_t>(sample_frames)) {
            // One sorted ID with exactly as many points as frames covers every
            // offset. Convert existing full-lane requests before SDK access.
            std::array<ParamValue,PARAMETER_POINT_COUNT> normalized;
            const auto count=points_used;
            for(size_t n=0;n<count;++n) normalized[n]=values[n].normalized;
            points_used=0;
            if(!input_dense(queues[0].id,normalized.data(),count)) return false;
        }
        sdk_active=true;return true;
    }
    bool end_sdk() {
        if(!on_thread() || !sdk_active) return false;
        sdk_active=false;return !bad.load() && exclusive();
    }
    // Copied host extraction outside the SDK loan. Sparse points keep their
    // original order; optional dense input follows in local sample order.
    // These views never escape the exchange or constitute an audio ACK.
    size_t size() const { return points_used+dense_count; }
    bool point(size_t index,ParameterPoint& result) const {
        if(!on_thread() || sdk_active || bad.load() || index>=size()) return false;
        if(index<points_used) result=values[index];
        else {
            const auto frame=index-points_used;
            result={static_cast<int32>(frame),dense_id,dense_values[frame]};
        }
        return true;
    }
    tresult PLUGIN_API queryInterface(const TUID iid,void** out) override {
        if(!out) return kInvalidArgument;
        *out=nullptr;
        if(!sdk_access()) return kResultFalse;
        if(!same(iid,INLINE_UID_OF(FUnknown)) && !same(iid,INLINE_UID_OF(IParameterChanges)))
            return kNoInterface;
        *out=static_cast<IParameterChanges*>(this);addRef();return kResultOk;
    }
    uint32 PLUGIN_API addRef() override { return retain(references); }
    uint32 PLUGIN_API release() override { return release_reference(references); }
    int32 PLUGIN_API getParameterCount() override {
        return sdk_access()?static_cast<int32>(queues_used):0;
    }
    IParamValueQueue* PLUGIN_API getParameterData(int32 index) override {
        return sdk_access() && index>=0 && static_cast<size_t>(index)<queues_used?&queues[index]:nullptr;
    }
    IParamValueQueue* PLUGIN_API addParameterData(const ParamID& id,int32& index) override {
        if(!sdk_access() || !writable) { bad.store(true);index=-1;return nullptr; }
        return queue(id,index);
    }
};
static_assert(sizeof(ParameterChanges)<8192,"parameter queues have fixed bounded native storage");
} // namespace hex_instance
