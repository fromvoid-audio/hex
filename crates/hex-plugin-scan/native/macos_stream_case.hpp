// Original fixture-only copied stream. PCM uses one real SDK instance/worker.
// MIDI records round-trip as transport metadata, not SDK event processing.
// No installed-module path, graph/device admission or deadline claim.
#pragma once
#include "macos_instance.hpp"
#include <array>
#include <cstdint>
#include <cstdio>
#include <limits>

namespace hex_stream_fixture {
constexpr size_t header_bytes=64, max_frames=256, max_events=64;
constexpr size_t max_packet=header_bytes+max_frames*8+max_events*8;
struct Packet {
    std::array<unsigned char,max_packet> bytes{};
    uint16_t frames=0, events=0;
    uint64_t identity[6]{};
    size_t size=0;
};
inline uint64_t number(const unsigned char* bytes,size_t count) {
    uint64_t value=0;
    for (size_t n=0;n<count;++n) value|=uint64_t(bytes[n])<<(n*8);
    return value;
}
inline float sample(const unsigned char* bytes) {
    const uint32_t bits=static_cast<uint32_t>(number(bytes,4));
    float value=0; std::memcpy(&value,&bits,4); return value;
}
inline void sample(unsigned char* bytes,float value) {
    uint32_t bits=0; std::memcpy(&bits,&value,4);
    for (size_t n=0;n<4;++n) bytes[n]=static_cast<unsigned char>(bits>>(n*8));
}
// 0 clean EOF, 1 complete packet, -1 malformed/truncated. Never allocate a
// peer-selected extent or accept a truncated packet as normal shutdown.
inline int read(Packet& packet) {
    auto* p=packet.bytes.data();
    if (std::fread(p,1,1,stdin)!=1) return std::feof(stdin)?0:-1;
    if (std::fread(p+1,1,header_bytes-1,stdin)!=header_bytes-1) return -1;
    if (std::memcmp(p,"HXVB",4) || number(p+4,2)!=1) return -1;
    for (size_t n=10;n<16;++n) if (p[n]) return -1;
    packet.frames=static_cast<uint16_t>(number(p+6,2));
    packet.events=static_cast<uint16_t>(number(p+8,2));
    // The existing original SDK fixture declares a maximum of 64. Do not
    // silently raise that capability to the transport's 256-frame capacity.
    if (!packet.frames || packet.frames>64 || packet.events>max_events) return -1;
    for (size_t n=0;n<6;++n) packet.identity[n]=number(p+16+n*8,8);
    for (size_t n=0;n<4;++n) if (!packet.identity[n]) return -1;
    const auto sequence=packet.identity[4];
    if (sequence>std::numeric_limits<uint64_t>::max()/packet.frames ||
        packet.identity[5]!=sequence*packet.frames) return -1;
    packet.size=header_bytes+packet.frames*8+packet.events*8;
    if (std::fread(p+header_bytes,1,packet.size-header_bytes,stdin)!=packet.size-header_bytes) return -1;
    for (size_t n=0;n<packet.frames*2u;++n) if (!std::isfinite(sample(p+header_bytes+n*4))) return -1;
    uint16_t previous=0;
    for (size_t n=0;n<packet.events;++n) {
        const auto* event=p+header_bytes+packet.frames*8+n*8;
        const auto frame=number(event,2);
        const auto length=event[2];
        const auto kind=event[3]&0xf0;
        const auto expected=(kind==0xc0 || kind==0xd0)?2:
            ((kind==0x80 || kind==0x90 || kind==0xa0 || kind==0xb0 || kind==0xe0)?3:0);
        if (frame>=packet.frames || (n && frame<previous) || length!=expected || !expected ||
            event[4]>=128 || (length==3?event[5]>=128:event[5]!=0) || event[6] || event[7]) return -1;
        previous=static_cast<uint16_t>(frame);
    }
    return 1;
}
inline bool write(const Packet& packet) {
    return std::fwrite(packet.bytes.data(),1,packet.size,stdout)==packet.size && std::fflush(stdout)==0;
}
inline bool enabled=false;
inline bool run(CFBundleRef bundle,Steinberg::IPluginFactory* factory) {
    using namespace Steinberg;
    using namespace Steinberg::Vst;
    auto observe=reinterpret_cast<int(*)(int*,int)>(CFBundleGetFunctionPointerForName(bundle,CFSTR("HexLifecycleObservation")));
    if (!factory || !observe) return false;
    Packet first{};
    if (read(first)!=1 || first.identity[4]!=0) return false;
    hex_instance::Instance instance;
    PClassInfo info{};
    bool ok=factory->getClassInfo(0,&info)==kResultOk && instance.open(factory,info.cid) &&
        instance.configure_stereo(48000,first.frames,false);
    uint64_t blocks=0;
    if (ok) {
        std::thread worker([&] {
            bool started=instance.start_processing();
            ok=started;
            Packet packet=first;
            std::array<float,64> left{},right{},out_left{},out_right{};
            float* inputs[]={left.data(),right.data()}; float* outputs[]={out_left.data(),out_right.data()};
            AudioBusBuffers input{},output{};
            input.numChannels=output.numChannels=2; input.channelBuffers32=inputs; output.channelBuffers32=outputs;
            ProcessData data{}; data.processMode=kRealtime; data.symbolicSampleSize=kSample32;
            data.numSamples=first.frames; data.numInputs=data.numOutputs=1; data.inputs=&input; data.outputs=&output;
            while (ok && blocks<64) {
                for (size_t n=0;n<4;++n) ok=ok && packet.identity[n]==first.identity[n];
                ok=ok && packet.frames==first.frames && packet.identity[4]==blocks;
                if (!ok) break;
                for (size_t n=0;n<packet.frames;++n) {
                    left[n]=sample(packet.bytes.data()+header_bytes+n*8);
                    right[n]=sample(packet.bytes.data()+header_bytes+n*8+4);
                }
                ok=instance.process(data);
                if (!ok) break; // foreign DSP is never retried/rolled back
                for (size_t n=0;n<packet.frames;++n) {
                    ok=ok && std::isfinite(out_left[n]) && std::isfinite(out_right[n]);
                    sample(packet.bytes.data()+header_bytes+n*8,out_left[n]);
                    sample(packet.bytes.data()+header_bytes+n*8+4,out_right[n]);
                }
                if (!ok || !write(packet)) { ok=false; break; }
                ++blocks;
                const auto next=read(packet);
                if (!next) break;
                if (next!=1 || blocks==64) { ok=false; break; }
            }
            if (started) ok=instance.stop_processing() && ok;
        });
        worker.join();
    }
    lifecycle_evidence.closed=instance.close();
    int observation[135]{};
    const int count=observe(observation,135);
    ok=ok && lifecycle_evidence.closed && count>=7 && observation[0]==0 &&
        observation[1]==1 && observation[3]==static_cast<int>(blocks);
    std::fprintf(stderr,"original stream: blocks=%llu closed=%d violations=%d\n",
        static_cast<unsigned long long>(blocks),lifecycle_evidence.closed?1:0,count>=1?observation[0]:-1);
    return ok;
}
} // namespace hex_stream_fixture
