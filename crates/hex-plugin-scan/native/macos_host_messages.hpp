// Original bounded SDK messages. Native-helper memory only, never a Rust ABI.
#pragma once
#include <Foundation/Foundation.h>
#include "pluginterfaces/vst/ivstmessage.h"
#include "pluginterfaces/vst/ivstattributes.h"
#include <array>
#include <atomic>
#include <cmath>
#include <cstring>
#include <pthread.h>

namespace hex_instance {
using namespace Steinberg;
using namespace Steinberg::Vst;
inline constexpr size_t HOST_MESSAGE_COUNT=16;
inline constexpr size_t HOST_ATTRIBUTE_COUNT=16;
inline constexpr size_t HOST_IDENTIFIER_BYTES=64;
inline constexpr size_t HOST_TEXT_UNITS=256;
inline constexpr size_t HOST_BINARY_BYTES=1024;
inline bool same(const TUID a,const TUID b) { return a && b && std::memcmp(a,b,sizeof(TUID))==0; }

template<typename Char> bool text_length(const Char* value,size_t maximum,size_t& length) {
    if (!value) return false;
    for (length=0;length<=maximum;++length) if (value[length]==0) return true;
    return false;
}

// Opt-in counters shared by the host's message leases. Each number is
// cumulative and sampled independently. No identifiers or payload are copied.
struct HostMessageDiagnostics {
    std::atomic<uint64_t> messages{0}, binary_attempts{0}, binary_writes{0}, binary_oversize{0};
    std::atomic<uint64_t> binary_reads{0}, binary_misses{0}, wrong_thread_reads{0};
    std::atomic<uint32> max_binary_bytes{0};

    void note_binary_write_attempt(uint32 bytes) {
        binary_attempts.fetch_add(1,std::memory_order_relaxed);
        auto largest=max_binary_bytes.load(std::memory_order_relaxed);
        while(bytes>largest && !max_binary_bytes.compare_exchange_strong(
            largest,bytes,std::memory_order_relaxed)) {}
    }
};

// Permanently constructed in the owning Host pool. Each lease confines payload
// access to its creating thread, including a synchronous receiver on that lane.
// References can be released on any thread. Last release returns the slot and
// its Host lease. Host retains its control lease through instance retirement.
class HostMessage final : public IMessage, public IAttributeList {
    enum class Kind { Empty, Integer, Floating, Text, Binary };
    struct Attribute {
        Kind kind=Kind::Empty;
        char id[HOST_IDENTIFIER_BYTES+1]{};
        int64 integer=0;
        double floating=0;
        TChar text[HOST_TEXT_UNITS+1]{};
        std::array<unsigned char,HOST_BINARY_BYTES> binary{};
        uint32 length=0;
    };
    std::array<Attribute,HOST_ATTRIBUTE_COUNT> values{};
    std::atomic<uint32> references{0};
    FUnknown* owner=nullptr;
    HostMessageDiagnostics* diagnostics=nullptr;
    pthread_t payload_thread=nullptr;
    bool attributes_only=false, named=false;
    char message_id[HOST_IDENTIFIER_BYTES+1]{};
    bool on_payload_thread() const { return pthread_equal(payload_thread,pthread_self()); }
    bool can_read_payload() const {
        if(on_payload_thread()) return true;
        if(diagnostics) diagnostics->wrong_thread_reads.fetch_add(1,std::memory_order_relaxed);
        return false;
    }
    Attribute* find(AttrID id,bool create=false) {
        size_t length=0;
        if (!on_payload_thread() || !text_length(id,HOST_IDENTIFIER_BYTES,length)) return nullptr;
        Attribute* empty=nullptr;
        for (auto& value:values) {
            if (value.kind!=Kind::Empty && std::strcmp(value.id,id)==0) return &value;
            if (!empty && value.kind==Kind::Empty) empty=&value;
        }
        if (create && empty) { std::memmove(empty->id,id,length+1); return empty; }
        return nullptr;
    }
public:
    void bind(FUnknown* host,HostMessageDiagnostics* trace) { owner=host;diagnostics=trace; }
    bool acquire(bool attributes) {
        uint32 available=0;
        if (!owner || !references.compare_exchange_strong(available,1,std::memory_order_acq_rel)) return false;
        owner->addRef();
        payload_thread=pthread_self();
        for (auto& value:values) value.kind=Kind::Empty;
        attributes_only=attributes; named=false; message_id[0]=0;
        if(diagnostics && !attributes) diagnostics->messages.fetch_add(1,std::memory_order_relaxed);
        return true;
    }
    uint32 PLUGIN_API addRef() override { return ++references; }
    uint32 PLUGIN_API release() override {
        const auto remaining=--references;
        if (!remaining) owner->release();
        return remaining;
    }
    tresult PLUGIN_API queryInterface(const TUID iid,void** out) override {
        if (!out) return kInvalidArgument;
        *out=nullptr;
        if (!iid) return kInvalidArgument;
        if (same(iid,INLINE_UID_OF(FUnknown)))
            *out=attributes_only?static_cast<FUnknown*>(static_cast<IAttributeList*>(this)):
                                 static_cast<FUnknown*>(static_cast<IMessage*>(this));
        else if (!attributes_only && same(iid,INLINE_UID_OF(IMessage))) *out=static_cast<IMessage*>(this);
        else if (same(iid,INLINE_UID_OF(IAttributeList))) *out=static_cast<IAttributeList*>(this);
        else return kNoInterface;
        addRef(); return kResultOk;
    }
    FIDString PLUGIN_API getMessageID() override {
        return can_read_payload() && named?message_id:nullptr;
    }
    void PLUGIN_API setMessageID(FIDString id) override {
        if (!on_payload_thread()) return;
        size_t length=0;
        named=text_length(id,HOST_IDENTIFIER_BYTES,length);
        if (named) std::memmove(message_id,id,length+1);
    }
    IAttributeList* PLUGIN_API getAttributes() override {
        return can_read_payload()?static_cast<IAttributeList*>(this):nullptr;
    }
    tresult PLUGIN_API setInt(AttrID id,int64 number) override {
        auto* value=find(id,true); if (!value) return kResultFalse;
        value->integer=number; value->kind=Kind::Integer; return kResultOk;
    }
    tresult PLUGIN_API getInt(AttrID id,int64& number) override {
        auto* value=find(id); if (!value || value->kind!=Kind::Integer) return kResultFalse;
        number=value->integer; return kResultOk;
    }
    tresult PLUGIN_API setFloat(AttrID id,double number) override {
        if (!std::isfinite(number)) return kInvalidArgument;
        auto* value=find(id,true); if (!value) return kResultFalse;
        value->floating=number; value->kind=Kind::Floating; return kResultOk;
    }
    tresult PLUGIN_API getFloat(AttrID id,double& number) override {
        auto* value=find(id); if (!value || value->kind!=Kind::Floating) return kResultFalse;
        number=value->floating; return kResultOk;
    }
    tresult PLUGIN_API setString(AttrID id,const TChar* text) override {
        size_t length=0; if (!text_length(text,HOST_TEXT_UNITS,length)) return kInvalidArgument;
        auto* value=find(id,true); if (!value) return kResultFalse;
        std::memmove(value->text,text,(length+1)*sizeof(TChar));
        value->length=static_cast<uint32>(length); value->kind=Kind::Text; return kResultOk;
    }
    tresult PLUGIN_API getString(AttrID id,TChar* text,uint32 bytes) override {
        auto* value=find(id);
        if (!text || !value || value->kind!=Kind::Text || bytes/sizeof(TChar)<=value->length) return kResultFalse;
        std::memmove(text,value->text,(value->length+1)*sizeof(TChar)); return kResultOk;
    }
    tresult PLUGIN_API setBinary(AttrID id,const void* bytes,uint32 size) override {
        if(diagnostics) {
            diagnostics->note_binary_write_attempt(size);
            if(size>HOST_BINARY_BYTES) diagnostics->binary_oversize.fetch_add(1,std::memory_order_relaxed);
        }
        if ((!bytes && size) || size>HOST_BINARY_BYTES) return kInvalidArgument;
        auto* value=find(id,true); if (!value) return kResultFalse;
        if (size) std::memmove(value->binary.data(),bytes,size);
        value->length=size; value->kind=Kind::Binary;
        if(diagnostics) diagnostics->binary_writes.fetch_add(1,std::memory_order_relaxed);
        return kResultOk;
    }
    tresult PLUGIN_API getBinary(AttrID id,const void*& bytes,uint32& size) override {
        if(!can_read_payload()) return kResultFalse;
        auto* value=find(id);
        if(!value || value->kind!=Kind::Binary) {
            if(diagnostics) diagnostics->binary_misses.fetch_add(1,std::memory_order_relaxed);
            return kResultFalse;
        }
        bytes=value->binary.data(); size=value->length;
        if(diagnostics) diagnostics->binary_reads.fetch_add(1,std::memory_order_relaxed);
        return kResultOk;
    }
};
} // namespace hex_instance
