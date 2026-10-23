// Current two-phase ordinary-instance protocol. One privately retained duplex descriptor;
// plug-in stdio is disconnected before loading. No native pointer is serialized.
#pragma once
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
    if(read_exact(fd,header.data(),header.size())!=1 || std::memcmp(header.data(),"HXVICF02",8))
        throw std::runtime_error("instance configuration frame refused");
    const auto size=number(header.data()+8,4);
    if(size>config_limit) throw std::runtime_error("instance configuration byte bound");
    Reader result{Bytes(static_cast<size_t>(size)),0};
    if(read_exact(fd,result.bytes.data(),result.bytes.size())!=1) throw std::runtime_error("truncated instance configuration");
    return result;
}
inline bool send(int fd,const char* magic,const Bytes& body) {
    if(body.size()>8192) return false;
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
struct Presentation {
    uint32_t input=0,output=0;
};
// No SDK call occurs inside this parser. Unknown presentation is explicit and
// carries no invented numeric values. All bounds/identity precede application.
inline bool presentation(int fd,const Config& config,std::optional<Presentation>& result) {
    try {
        std::array<uint8_t,12> header{};
        if(read_exact(fd,header.data(),header.size())!=1 || std::memcmp(header.data(),"HXVIPR01",8)) return false;
        const auto size=number(header.data()+8,4);if(size!=35 && size!=43) return false;
        Reader in{Bytes(static_cast<size_t>(size)),0};
        if(read_exact(fd,in.bytes.data(),in.bytes.size())!=1 || in.number(2)!=1) return false;
        for(auto id:config.binding) if(in.number(8)!=id) return false;
        const auto known=in.number(1);if(known>1) return false;
        if(known) result=Presentation{static_cast<uint32_t>(in.number(4)),static_cast<uint32_t>(in.number(4))};
        else result.reset();
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
    std::array<uint8_t,header_size+max_frames*8> bytes{};
    uint16_t frames=0;bool stop=false;
    // 0 EOF/cancellation, 1 PCM, 2 exact terminal request, -1 malformed.
    int read(int fd,const Config& config,uint64_t sequence,uint64_t frame) {
        auto* p=bytes.data();const auto received=read_exact(fd,p,header_size);
        if(received!=1) return received;
        stop=!std::memcmp(p,"HXVE",4);
        if((!stop && std::memcmp(p,"HXVB",4)) || number(p+4,2)!=1 || number(p+8,2)) return -1;
        for(size_t n=10;n<16;++n) if(p[n]) return -1;
        for(size_t n=0;n<4;++n) if(number(p+16+n*8,8)!=config.binding[n]) return -1;
        if(number(p+48,8)!=sequence || number(p+56,8)!=frame) return -1;
        frames=static_cast<uint16_t>(number(p+6,2));
        if(stop) return frames? -1:2;
        if(!frames || frames>config.maximum || frame>std::numeric_limits<uint64_t>::max()-frames || sequence==std::numeric_limits<uint64_t>::max()) return -1;
        if(read_exact(fd,p+header_size,frames*8)!=1) return -1;
        for(size_t n=0;n<frames*2u;++n) if(!std::isfinite(sample(p+header_size+n*4))) return -1;
        return 1;
    }
    bool write(int fd) const { return write_exact(fd,bytes.data(),header_size+frames*8); }
};
} // namespace hex_instance_wire
