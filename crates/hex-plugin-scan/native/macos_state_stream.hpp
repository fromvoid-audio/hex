// Helper-control-only bounded SDK stream. The instance retains a control lease;
// foreign release never finally destroys this storage on a processing thread.
#pragma once
#include <Foundation/Foundation.h>
#include "pluginterfaces/base/ibstream.h"
#include <algorithm>
#include <array>
#include <atomic>
#include <cstring>
#include <cstdint>
#include <limits>
#include <vector>

namespace hex_instance {
class StateStream final : public Steinberg::IBStream,public Steinberg::ISizeableStream {
    using int32=Steinberg::int32;using int64=Steinberg::int64;using uint32=Steinberg::uint32;
    std::atomic<uint32> references{1};
    std::atomic<bool> refused{false};
    uint32_t overflow_bytes=0;
    size_t limit=0,position=0,length=0;
    bool read_only=false;
    bool control() { if ([NSThread isMainThread]) return true;refused.store(true);return false; }
    Steinberg::tresult invalid() { refused.store(true);return Steinberg::kInvalidArgument; }
    ~StateStream() = default;
public:
    static constexpr size_t envelope_capacity=512*1024;
    static constexpr size_t capacity=envelope_capacity-96;
    std::array<unsigned char,capacity> bytes{};
    explicit StateStream(size_t maximum):limit(std::min(maximum,capacity)) {}
    // Restore owns a fixed copied extent. Foreign code may read/seek it but
    // cannot overwrite the caller's captured state or manufacture more bytes.
    StateStream(const unsigned char* source,size_t count):read_only(true) {
        if (count>capacity || (count && !source)) { refused.store(true);return; }
        limit=length=count;
        if (count) std::copy_n(source,count,bytes.begin());
    }
    Steinberg::tresult PLUGIN_API queryInterface(const Steinberg::TUID iid,void** out) override {
        if (!out) return invalid();*out=nullptr;
        if (!iid) return invalid();
        if (!std::memcmp(iid,INLINE_UID_OF(Steinberg::FUnknown),16) ||
            !std::memcmp(iid,INLINE_UID_OF(Steinberg::IBStream),16)) *out=static_cast<Steinberg::IBStream*>(this);
        else if (!std::memcmp(iid,INLINE_UID_OF(Steinberg::ISizeableStream),16)) *out=static_cast<Steinberg::ISizeableStream*>(this);
        else return Steinberg::kNoInterface;
        addRef();return Steinberg::kResultOk;
    }
    uint32 PLUGIN_API addRef() override {
        auto count=references.load();
        do { if (count==std::numeric_limits<uint32>::max()) { refused.store(true);return count; } }
        while (!references.compare_exchange_weak(count,count+1));
        return count+1;
    }
    uint32 PLUGIN_API release() override {
        auto count=references.load();
        while (count>1) if (references.compare_exchange_weak(count,count-1)) return count-1;
        refused.store(true);return 1; // no foreign caller owns the control lease
    }
    bool exclusive() const { return references.load()==1; }
    bool valid() const { return !refused.load(); }
    uint32_t overflow() const { return overflow_bytes; }
    size_t size() const { return length; }
    bool reset_write_on_control(size_t maximum) {
        if (!control() || !valid() || !exclusive() || maximum>capacity) return false;
        bytes.fill(0);limit=maximum;position=length=0;read_only=false;overflow_bytes=0;return true;
    }
    bool rewind_read_only_on_control() {
        if (!control() || !valid() || !exclusive()) return false;
        read_only=true;position=0;limit=length;return true;
    }
    bool dispose_on_control() {
        if (!control() || !exclusive()) return false;
        delete this;return true;
    }
    Steinberg::tresult PLUGIN_API read(void* buffer,int32 count,int32* done) override {
        if (done) *done=0;
        if (!control() || count<0 || (count && !buffer)) return invalid();
        const auto available=position<length?length-position:0;
        const auto amount=std::min(static_cast<size_t>(count),available);
        if (amount) std::memcpy(buffer,bytes.data()+position,amount);
        position+=amount;if (done) *done=static_cast<int32>(amount);
        return amount==static_cast<size_t>(count)?Steinberg::kResultOk:Steinberg::kResultFalse;
    }
    Steinberg::tresult PLUGIN_API write(void* buffer,int32 count,int32* done) override {
        if (done) *done=0;
        if (!control() || read_only || count<0 || (count && !buffer)) return invalid();
        if (static_cast<size_t>(count)>limit-position) {
            // Include the component's reserved prefix for controller captures.
            // This is a lower bound, not a claim that the SDK finished writing.
            const auto observed=capacity-limit+position+static_cast<size_t>(count);
            overflow_bytes=std::max(overflow_bytes,static_cast<uint32_t>(std::min(observed,static_cast<size_t>(std::numeric_limits<uint32_t>::max()))));
            return Steinberg::kInvalidArgument;
        }
        if (count) std::memcpy(bytes.data()+position,buffer,static_cast<size_t>(count));
        position+=static_cast<size_t>(count);length=std::max(length,position);
        if (done) *done=count;return Steinberg::kResultOk;
    }
    Steinberg::tresult PLUGIN_API seek(int64 offset,int32 mode,int64* result) override {
        if (!control()) return invalid();
        size_t base=0;
        if (mode==kIBSeekCur) base=position;
        else if (mode==kIBSeekEnd) base=length;
        else if (mode!=kIBSeekSet) return invalid();
        if (offset>=0) {
            if (static_cast<Steinberg::uint64>(offset)>limit-base) return invalid();
            position=base+static_cast<size_t>(offset);
        } else {
            const auto distance=static_cast<Steinberg::uint64>(-(offset+1))+1;
            if (distance>base) return invalid();position=base-static_cast<size_t>(distance);
        }
        if (result) *result=static_cast<int64>(position);return Steinberg::kResultOk;
    }
    Steinberg::tresult PLUGIN_API tell(int64* result) override {
        if (!control() || !result) return invalid();*result=static_cast<int64>(position);return Steinberg::kResultOk;
    }
    Steinberg::tresult PLUGIN_API getStreamSize(int64& size) override {
        if (!control()) return invalid();size=static_cast<int64>(length);return Steinberg::kResultOk;
    }
    Steinberg::tresult PLUGIN_API setStreamSize(int64 size) override {
        if (!control() || read_only || size<0 || static_cast<Steinberg::uint64>(size)>limit) return invalid();
        const auto target=static_cast<size_t>(size);
        if (target>length) std::fill(bytes.begin()+length,bytes.begin()+target,0);
        length=target;position=std::min(position,length);return Steinberg::kResultOk;
    }
};
struct CapturedState {
    std::vector<unsigned char> bytes;
    uint32_t component_bytes=0,controller_bytes=0;
    bool component=false,controller=false;
};
} // namespace hex_instance
