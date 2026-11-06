// Current source-bound startup and quiescence protocol. One retained duplex descriptor;
// plug-in stdio is disconnected before loading. No native pointer is serialized.
#pragma once
#include "macos_state_stream.hpp"
#include "macos_input_notes.hpp"
#include "macos_parameters.hpp"
#include "macos_parameter_page.hpp"
#include "pluginterfaces/vst/ivstprocesscontext.h"
#include <array>
#include <cerrno>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <limits>
#include <optional>
#include <stdexcept>
#include <string>
#include <vector>
#include <sys/socket.h>
#include <unistd.h>

namespace hex_instance_wire {
constexpr size_t config_limit=2*4096+256, max_frames=256, header_size=64;
constexpr size_t timing_size=24;
constexpr size_t extended_timing_size=40;
using Bytes=std::vector<uint8_t>;
inline uint64_t number(const uint8_t* p,size_t n) {
    uint64_t value=0;for(size_t i=0;i<n;++i) value|=uint64_t(p[i])<<(i*8);return value;
}
inline void number(Bytes& out,uint64_t value,size_t n) {
    for(size_t i=0;i<n;++i) out.push_back(static_cast<uint8_t>(value>>(i*8)));
}
// Clean EOF differs from a partial frame. Only the processing lane reads PCM.
inline int read_exact(int fd,uint8_t* p,size_t n) {
    size_t at=0;
    while(at<n) {
        auto count=::read(fd,p+at,n-at);
        if(count<0 && errno==EINTR) continue;
        if(count<0) return -1;
        if(!count) return at? -1:0;
        at+=static_cast<size_t>(count);
    }
    return 1;
}
inline bool write_exact(int fd,const uint8_t* p,size_t n) {
    size_t at=0;
    while(at<n) {
        auto count=::write(fd,p+at,n-at);
        if(count<0 && errno==EINTR) continue;
        if(count<=0) return false;at+=static_cast<size_t>(count);
    }
    return true;
}
struct Reader {
    Bytes bytes;size_t at=0;
    uint64_t number(size_t n) {
        if(n>8 || at>bytes.size() || n>bytes.size()-at) throw std::runtime_error("truncated instance configuration");
        auto value=hex_instance_wire::number(bytes.data()+at,n);at+=n;return value;
    }
    template<size_t N> std::array<uint8_t,N> array() {
        std::array<uint8_t,N> result{};
        for(auto& v:result) v=static_cast<uint8_t>(number(1));return result;
    }
    std::string text() {
        const auto n=number(4);
        if(n>4096 || at>bytes.size() || n>bytes.size()-at) throw std::runtime_error("instance path bound");
        std::string result(reinterpret_cast<const char*>(bytes.data()+at),static_cast<size_t>(n));at+=static_cast<size_t>(n);
        if(result.empty() || result.find('\0')!=std::string::npos) throw std::runtime_error("invalid instance path");return result;
    }
};
inline Reader configuration(int fd) {
    std::array<uint8_t,12> header{};
    if(read_exact(fd,header.data(),header.size())!=1 || std::memcmp(header.data(),"HXVICF03",8))
        throw std::runtime_error("instance configuration frame refused");
    const auto size=number(header.data()+8,4);
    if(size>config_limit) throw std::runtime_error("instance configuration byte bound");
    Reader result{Bytes(static_cast<size_t>(size)),0};
    if(read_exact(fd,result.bytes.data(),result.bytes.size())!=1) throw std::runtime_error("truncated instance configuration");
    return result;
}
inline bool send(int fd,const char* magic,const Bytes& body) {
    size_t limit=8192;
    if(!std::memcmp(magic,"HXVIQF03",8)) limit=156+hex_instance::StateStream::envelope_capacity;
    if(!std::memcmp(magic,"HXVIAR02",8)) limit=164+hex_instance::StateStream::envelope_capacity;
    if(!std::memcmp(magic,"HXVIRR03",8)) limit=166+hex_instance::StateStream::envelope_capacity;
    if(!std::memcmp(magic,"HXVIFA04",8)) limit=140+hex_instance::StateStream::envelope_capacity;
    if(!std::memcmp(magic,"HXVIRD05",8)) limit=86+hex_instance::StateStream::envelope_capacity;
    if(!std::memcmp(magic,"HXVICS02",8)) limit=62+hex_instance::StateStream::envelope_capacity;
    if(!std::memcmp(magic,"HXVLCA02",8)) limit=63+hex_instance::StateStream::envelope_capacity;
    if(body.size()>limit) return false;
    std::array<uint8_t,12> header{};std::memcpy(header.data(),magic,8);
    for(size_t i=0;i<4;++i) header[8+i]=static_cast<uint8_t>(body.size()>>(i*8));
    return write_exact(fd,header.data(),header.size()) && write_exact(fd,body.data(),body.size());
}
inline std::string hex(const uint8_t* p,size_t n) {
    constexpr char digits[]="0123456789abcdef";std::string value;value.reserve(n*2);
    for(size_t i=0;i<n;++i) { value.push_back(digits[p[i]>>4]);value.push_back(digits[p[i]&15]); }return value;
}
struct Config {
    uint64_t binding[4]{};
    std::array<uint8_t,16> cid{};
    std::array<uint8_t,32> executable_hash{},info_hash{};
    double rate=0;uint16_t maximum=0;uint8_t inputs=0,outputs=0,precision=0;
    std::string root,executable;
    explicit Config(Reader& in) {
        if(in.number(2)!=1) throw std::runtime_error("instance protocol version refused");
        for(auto& id:binding) { id=in.number(8);if(!id) throw std::runtime_error("instance binding missing"); }
        cid=in.array<16>();executable_hash=in.array<32>();info_hash=in.array<32>();
        auto bits=in.number(8);std::memcpy(&rate,&bits,8);
        maximum=static_cast<uint16_t>(in.number(2));inputs=static_cast<uint8_t>(in.number(1));outputs=static_cast<uint8_t>(in.number(1));
        precision=static_cast<uint8_t>(in.number(1));
        if(in.number(3)) throw std::runtime_error("instance reserved bytes refused");
        root=in.text();executable=in.text();
        bool nonzero=false;for(auto v:cid) nonzero=nonzero || v;
        if(in.at!=in.bytes.size() || !nonzero || !std::isfinite(rate) || rate<=0 || rate>768000 ||
            !maximum || maximum>max_frames || inputs>2 || !outputs || outputs>2 || precision!=0)
            throw std::runtime_error("instance configuration unsupported");
    }
    void stamp(Bytes& out) const { number(out,1,2);for(auto id:binding) number(out,id,8); }
};
// Startup is a separate bounded control frame, fully validated before module
// entry. Echoing its exact copied envelope in Facts binds restoration to this
// unique operation without treating an SDK capture as canonical state.
struct Startup {
    Bytes payload;
    hex_instance::CapturedState state{};
    bool restored() const { return !payload.empty(); }
    void acknowledge(Bytes& out) const {
        number(out,restored()?1:0,1);number(out,payload.size(),4);
        out.insert(out.end(),payload.begin(),payload.end());
    }
};
inline Startup startup(int fd,const Config& config) {
    std::array<uint8_t,12> header{};
    if(read_exact(fd,header.data(),header.size())!=1 || std::memcmp(header.data(),"HXVIST02",8))
        throw std::runtime_error("startup state frame refused");
    const auto size=number(header.data()+8,4);
    if(size<39 || size>39+hex_instance::StateStream::envelope_capacity) throw std::runtime_error("startup state frame bound");
    Reader in{Bytes(static_cast<size_t>(size)),0};
    if(read_exact(fd,in.bytes.data(),in.bytes.size())!=1 || in.number(2)!=1)
        throw std::runtime_error("truncated startup state frame");
    for(auto id:config.binding) if(in.number(8)!=id) throw std::runtime_error("startup state binding mismatch");
    const auto known=in.number(1),count=in.number(4);
    if(known>1 || count>hex_instance::StateStream::envelope_capacity || count!=in.bytes.size()-in.at || (!known && count) || (known && count<96))
        throw std::runtime_error("startup state extent refused");
    Startup result{};
    if(!known) return result;
    result.payload.assign(in.bytes.begin()+in.at,in.bytes.end());
    const auto* p=result.payload.data();
    if(std::memcmp(p,"HXVS",4) || p[4]!=1 || (p[5]&~3) || !(p[5]&1) || p[6] || p[7] ||
        std::memcmp(p+8,config.cid.data(),16) || std::memcmp(p+24,config.executable_hash.data(),32) ||
        std::memcmp(p+56,config.info_hash.data(),32)) throw std::runtime_error("startup state source envelope refused");
    const auto component=number(p+88,4),controller=number(p+92,4);
    if(component>hex_instance::StateStream::capacity || controller>hex_instance::StateStream::capacity-component || (!(p[5]&2) && controller) || count!=96+component+controller)
        throw std::runtime_error("startup state chunks refused");
    result.state.component=true;result.state.controller=(p[5]&2)!=0;
    result.state.component_bytes=static_cast<uint32_t>(component);
    result.state.controller_bytes=static_cast<uint32_t>(controller);
    result.state.bytes.assign(p+96,p+count);
    return result;
}
struct Presentation {
    uint32_t input=0,output=0;
};
struct PresentationPlan {
    std::optional<Presentation> known;
    bool allow_unknown=false;
};
// No SDK call occurs inside this parser. Unknown presentation is explicit and
// carries no invented numeric values. All bounds/identity precede application.
inline bool presentation(int fd,const Config& config,PresentationPlan& result) {
    try {
        std::array<uint8_t,12> header{};
        if(read_exact(fd,header.data(),header.size())!=1 || std::memcmp(header.data(),"HXVIPR01",8)) return false;
        const auto size=number(header.data()+8,4);if(size!=35 && size!=43) return false;
        Reader in{Bytes(static_cast<size_t>(size)),0};
        if(read_exact(fd,in.bytes.data(),in.bytes.size())!=1 || in.number(2)!=1) return false;
        for(auto id:config.binding) if(in.number(8)!=id) return false;
        const auto mode=in.number(1);if(mode>2) return false;
        result={};
        result.allow_unknown=mode==2;
        if(mode==1) result.known=Presentation{static_cast<uint32_t>(in.number(4)),static_cast<uint32_t>(in.number(4))};
        return in.at==in.bytes.size();
    } catch(const std::runtime_error&) { return false; }
}
inline float sample(const uint8_t* p) {
    const uint32_t bits=static_cast<uint32_t>(number(p,4));float value=0;std::memcpy(&value,&bits,4);return value;
}
inline void sample(uint8_t* p,float value) {
    uint32_t bits=0;std::memcpy(&bits,&value,4);for(size_t n=0;n<4;++n) p[n]=static_cast<uint8_t>(bits>>(n*8));
}
struct Packet {
    static constexpr size_t timing_change_limit=7, timing_change_bytes=8+extended_timing_size;
    struct TimingChange { uint16_t offset=0; Steinberg::Vst::ProcessContext context{}; };
    std::array<uint8_t,header_size+max_frames*8+hex_instance::INPUT_NOTE_COUNT*12+hex_instance::PARAMETER_POINT_COUNT*16+extended_timing_size+timing_change_limit*timing_change_bytes> bytes{};
    std::array<hex_instance::ParameterPoint,hex_instance::PARAMETER_POINT_COUNT> parameters{};
    std::array<hex_instance::InputNote,hex_instance::INPUT_NOTE_COUNT> notes{};
    size_t note_count=0;
    size_t parameter_count=0;
    size_t reply_parameter_count=0,timing_change_count=0;
    std::array<TimingChange,timing_change_limit> timing_changes{};
    std::optional<Steinberg::Vst::ProcessContext> context;
    uint16_t frames=0;bool stop=false,observe_latency=false,flush_idle_edits=false,release_gates=false;
    uint64_t next_graph=0,next_epoch=0;
    uint64_t capture_operation=0;
    hex_instance::ControllerQuery parameter_query{};
    PresentationPlan resumed_presentation;
    // 0 EOF/cancellation, 1 PCM, 2 terminal, 3 quiesce, 4 resume,
    // 5 quiescent capture, 6 live capture start, 7 live capture poll, 8 parameter-only flush,
    // 9 edit receipts, 10 controller page start, 11 controller page poll,
    // 12 explicit stopped component reactivation, -1 malformed.
    // Only the serialized processing lane reads these requests. Resume changes
    // no source/format facts, and is legal only in an acknowledged quiescence.
    int read(int fd,const Config& config,uint64_t sequence,uint64_t frame) {
        auto* p=bytes.data();const auto received=read_exact(fd,p,header_size);
        if(received!=1) return received;
        stop=!std::memcmp(p,"HXVE",4);
        const bool quiesce=!std::memcmp(p,"HXVQ",4),resume=!std::memcmp(p,"HXVR",4),capture=!std::memcmp(p,"HXVC",4);
        const bool edit_query=!std::memcmp(p,"HXVG",4);
        const bool reactivate=!std::memcmp(p,"HXVA",4);
        const bool page_start=!std::memcmp(p,"HXVM",4),page_poll=!std::memcmp(p,"HXVN",4);
        const bool live_start=!std::memcmp(p,"HXVL",4),live_poll=!std::memcmp(p,"HXVP",4);
        const bool pcm=!std::memcmp(p,"HXVB",4),flush=!std::memcmp(p,"HXVF",4);
        const auto version=number(p+4,2);
        release_gates=quiesce && version==2 && !config.inputs;
        observe_latency=edit_query && (version==2 || version==3);
        flush_idle_edits=edit_query && version==3;
        // Version 6 adds one to seven changes after the initial extended
        // context. Byte 12 is their count. Each record carries a u16 sample
        // offset, six reserved zero bytes and the 40-byte context. Offsets
        // strictly increase and each record changes the playing flag.
        const bool sliced_timing=pcm && version==6;
        const bool extended_timing=pcm && (version==5 || sliced_timing);
        const bool timed=pcm && (version==4 || extended_timing);
        const bool identified_notes=pcm && (version==3 || timed);
        if((!stop && !quiesce && !resume && !capture && !live_start && !live_poll && !pcm && !flush && !edit_query && !page_start && !page_poll && !reactivate) ||
            ((pcm || flush || edit_query)?(version!=1 && version!=2 && !flush_idle_edits && !identified_notes):
                (version!=1 && !release_gates))) return -1;
        note_count=static_cast<size_t>(number(p+8,2));
        const bool dense_notes=note_count>hex_instance::LEGACY_INPUT_NOTE_COUNT;
        if (note_count>notes.size() || (dense_notes && !identified_notes) ||
            (note_count && (!pcm || config.inputs)) || (version==3 && pcm && !note_count)) return -1;
        const bool has_parameter_field=((pcm || flush) && version==2) || identified_notes;
        parameter_count=has_parameter_field?static_cast<size_t>(number(p+10,2)):0;
        if(parameter_count>parameters.size()) return -1;
        timing_change_count=sliced_timing?p[12]:0;
        if(sliced_timing && (!timing_change_count || timing_change_count>timing_changes.size())) return -1;
        for(size_t n=sliced_timing?13:(resume?11:(has_parameter_field?12:10));n<16;++n) if(p[n]) return -1;
        for(size_t n=0;n<4;++n) if(number(p+16+n*8,8)!=config.binding[n]) return -1;
        frames=static_cast<uint16_t>(number(p+6,2));
        if(resume) {
            next_graph=number(p+48,8);next_epoch=number(p+56,8);
            if(frames || p[10]>2 || next_graph<config.binding[2] || next_epoch<=config.binding[3]) return -1;
            resumed_presentation={};
            resumed_presentation.allow_unknown=p[10]==2;
            if(p[10]==1) {
                if(read_exact(fd,p+header_size,8)!=1) return -1;
                resumed_presentation.known=Presentation{static_cast<uint32_t>(number(p+header_size,4)),static_cast<uint32_t>(number(p+header_size+4,4))};
            }
            return 4;
        }
        if(number(p+48,8)!=sequence || number(p+56,8)!=frame) return -1;
        if(stop) return frames? -1:2;
        if(quiesce) return frames? -1:3;
        if(page_start || page_poll) {
            if(frames || read_exact(fd,p+header_size,287)!=1) return -1;
            capture_operation=number(p+header_size,8);
            parameter_query.first=static_cast<uint16_t>(number(p+header_size+8,2));
            parameter_query.revision=number(p+header_size+10,8);
            parameter_query.kind=p[header_size+18];
            parameter_query.id=static_cast<uint32_t>(number(p+header_size+19,4));
            parameter_query.bits=number(p+header_size+23,8);
            for(size_t n=0;n<128;++n) parameter_query.text[n]=static_cast<Steinberg::Vst::TChar>(number(p+header_size+31+n*2,2));
            if(!capture_operation || !hex_instance::valid_controller_query(parameter_query)) return -1;
            return page_start?10:11;
        }
        if(flush) {
            if(version!=2 || frames || read_exact(fd,p+header_size,8+parameter_count*16)!=1) return -1;
            capture_operation=number(p+header_size,8);
            return capture_operation && parse_parameters(p+header_size+8,1)?8:-1;
        }
        if(capture || live_start || live_poll || edit_query || reactivate) {
            if(frames || read_exact(fd,p+header_size,8)!=1) return -1;
            capture_operation=number(p+header_size,8);return capture_operation?(reactivate?12:(edit_query?9:(live_start?6:(live_poll?7:5)))):-1;
        }
        if(!frames || frames>config.maximum || frame>std::numeric_limits<uint64_t>::max()-frames || sequence==std::numeric_limits<uint64_t>::max()) return -1;
        const size_t note_bytes=identified_notes?12:8;
        const size_t clock_bytes=extended_timing?extended_timing_size:(timed?timing_size:0);
        if(read_exact(fd,p+header_size,frames*8+note_count*note_bytes+parameter_count*16+clock_bytes+
            timing_change_count*timing_change_bytes)!=1) return -1;
        for(size_t n=0;n<frames*2u;++n) if(!std::isfinite(sample(p+header_size+n*4))) return -1;
        const auto* payload=p+header_size+frames*8;
        for (size_t n=0;n<note_count;++n) {
            const auto* raw=payload+n*note_bytes;
            const auto offset=number(raw,2);
            const auto status=raw[3]&0xf0;
            const auto note_id=identified_notes?number(raw+8,4):0;
            if (offset>=frames || raw[2]!=3 || (status!=0x80 && status!=0x90) ||
                raw[4]>=128 || raw[5]>=128 || raw[6] || raw[7] ||
                (dense_notes && !note_id) ||
                note_id>static_cast<uint64_t>(std::numeric_limits<int32_t>::max()) ||
                (n && notes[n-1].offset>static_cast<int32_t>(offset))) return -1;
            notes[n]={static_cast<int32_t>(offset),static_cast<int16_t>(raw[3]&0x0f),
                static_cast<int16_t>(raw[4]),raw[5],status==0x90 && raw[5]!=0,
                note_id?static_cast<int32_t>(note_id):-1};
        }
        const auto* points=payload+note_count*note_bytes;
        if(!parse_parameters(points,frames)) return -1;
        context.reset();
        if(!timed) return 1;
        const auto* clock=points+parameter_count*16;
        if(!parse_timing(clock,config.rate,extended_timing,version==5,context.emplace())) return -1;
        const auto playing=[](const Steinberg::Vst::ProcessContext& timing) {
            return (timing.state&Steinberg::Vst::ProcessContext::kPlaying)!=0;
        };
        bool prior_playing=playing(*context);
        for(size_t index=0;index<timing_change_count;++index) {
            const auto* record=clock+clock_bytes+index*timing_change_bytes;
            const auto offset=number(record,2);
            auto& change=timing_changes[index];
            if(!offset || offset>=frames || number(record+2,6) ||
                (index && offset<=timing_changes[index-1].offset) ||
                !parse_timing(record+8,config.rate,true,false,change.context) ||
                playing(change.context)==prior_playing) return -1;
            change.offset=static_cast<uint16_t>(offset);
            prior_playing=playing(change.context);
        }
        return 1;
    }
    bool parse_timing(const uint8_t* start,double rate,bool extended,bool require_extended_fields,
                      Steinberg::Vst::ProcessContext& value) {
        using Steinberg::Vst::ProcessContext;
        const auto frame=number(start,8),beat=number(start+8,8),tempo=number(start+16,4);
        const auto flags=start[20];
        const auto numerator=start[21],denominator=start[22];
        const auto cycle_start=extended?number(start+24,8):0;
        const auto cycle_end=extended?number(start+32,8):0;
        if(frame>static_cast<uint64_t>(std::numeric_limits<int64_t>::max()) ||
            (flags&~15) || (!extended && (flags&12)) || (require_extended_fields && !(flags&12)) ||
            start[23] || (!(flags&2) && beat) ||
            (!(flags&4) && (numerator || denominator)) ||
            ((flags&4) && (!numerator || numerator>99 || !denominator || denominator>64 ||
                          (denominator&(denominator-1)))) ||
            (!(flags&8) && (cycle_start || cycle_end)) ||
            ((flags&8) && cycle_start>=cycle_end)) return false;
        value={};
        value.sampleRate=rate;
        value.projectTimeSamples=static_cast<int64_t>(frame);
        if(flags&1) value.state|=ProcessContext::kPlaying;
        if(flags&2) {
            value.state|=ProcessContext::kProjectTimeMusicValid;
            value.projectTimeMusic=static_cast<double>(beat)/4294967296.0;
        }
        if(tempo) {
            value.state|=ProcessContext::kTempoValid;
            value.tempo=static_cast<double>(tempo)/1000000.0;
        }
        if(flags&4) {
            value.state|=ProcessContext::kTimeSigValid;
            value.timeSigNumerator=numerator;
            value.timeSigDenominator=denominator;
        }
        if(flags&8) {
            const auto start_music=static_cast<double>(cycle_start)/4294967296.0;
            const auto end_music=static_cast<double>(cycle_end)/4294967296.0;
            // Very distant raw endpoints can round to one SDK quarter-note value.
            if(start_music<end_music) {
                value.state|=ProcessContext::kCycleActive|ProcessContext::kCycleValid;
                value.cycleStartMusic=start_music;
                value.cycleEndMusic=end_music;
            }
        }
        return true;
    }
    bool parse_parameters(const uint8_t* start,int32_t limit) {
        for(size_t n=0;n<parameter_count;++n) {
            const auto* point=start+n*16;
            auto& value=parameters[n];value.offset=static_cast<int32_t>(number(point,2));
            value.id=static_cast<uint32_t>(number(point+4,4));
            const auto bits=number(point+8,8);std::memcpy(&value.normalized,&bits,8);
            if(number(point+2,2) || value.offset>=limit || value.id>0x7FFFFFFFu ||
                !std::isfinite(value.normalized) || value.normalized<0 || value.normalized>1 ||
                (n && parameters[n-1].offset>value.offset)) return false;
        }
        return true;
    }
    bool append_parameter_reply(size_t offset,const hex_instance::ParameterChanges& output) {
        if(output.size()>parameters.size()-reply_parameter_count) return false;
        for(size_t n=0;n<output.size();++n) {
            hex_instance::ParameterPoint value{};if(!output.point(n,value)) return false;
            value.offset+=static_cast<int32_t>(offset);
            auto* point=bytes.data()+header_size+frames*8+reply_parameter_count++*16;
            point[0]=static_cast<uint8_t>(value.offset);point[1]=static_cast<uint8_t>(value.offset>>8);
            point[2]=point[3]=0;
            for(size_t byte=0;byte<4;++byte) point[4+byte]=static_cast<uint8_t>(value.id>>(byte*8));
            uint64_t bits=0;std::memcpy(&bits,&value.normalized,8);
            for(size_t byte=0;byte<8;++byte) point[8+byte]=static_cast<uint8_t>(bits>>(byte*8));
        }
        return true;
    }
    bool write(int fd) {
        bytes[4]=reply_parameter_count?2:1;bytes[5]=0;
        bytes[8]=bytes[9]=0; // Input notes are never echoed as processor output.
        bytes[10]=static_cast<uint8_t>(reply_parameter_count);bytes[11]=bytes[12]=0;
        return write_exact(fd,bytes.data(),header_size+frames*8+reply_parameter_count*16);
    }
};
} // namespace hex_instance_wire
