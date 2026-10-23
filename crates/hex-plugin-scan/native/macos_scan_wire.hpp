// Original bounded factory-discovery wire. No VST object crosses this boundary.
#pragma once
#include <array>
#include <cerrno>
#include <cstdint>
#include <cstring>
#include <stdexcept>
#include <string>
#include <vector>
#include <unistd.h>

namespace MacScan {
constexpr size_t packet_limit=1024*1024;
constexpr size_t path_limit=4096;
using Bytes=std::vector<uint8_t>;

inline void number(Bytes& bytes,uint64_t value,size_t width) {
    for(size_t n=0;n<width;++n) bytes.push_back(static_cast<uint8_t>(value>>(8*n)));
}
inline void text(Bytes& bytes,const std::string& value,size_t maximum) {
    if(value.size()>maximum || value.find('\0')!=std::string::npos) throw std::runtime_error("wire text refused");
    number(bytes,value.size(),4); bytes.insert(bytes.end(),value.begin(),value.end());
}
inline std::string field(const char* value,size_t capacity) {
    const void* end=std::memchr(value,0,capacity);
    if(!end) throw std::runtime_error("unterminated factory text");
    return std::string(value,static_cast<const char*>(end));
}
inline void transfer(uint8_t* bytes,size_t count,bool writing) {
    size_t offset=0;
    while(offset<count) {
        auto n=writing ? ::write(STDIN_FILENO,bytes+offset,count-offset) : ::read(STDIN_FILENO,bytes+offset,count-offset);
        if(n<0 && errno==EINTR) continue;
        if(n<=0) throw std::runtime_error("scan wire closed");
        offset+=static_cast<size_t>(n);
    }
}
struct Reader {
    Bytes bytes;
    size_t offset=0;
    uint64_t number(size_t width) {
        if(width>8 || offset>bytes.size() || width>bytes.size()-offset) throw std::runtime_error("truncated scan wire");
        uint64_t value=0;
        for(size_t n=0;n<width;++n) value|=uint64_t(bytes[offset++])<<(8*n);
        return value;
    }
    std::string text(size_t maximum) {
        auto length=number(4);
        if(length>maximum || offset>bytes.size() || length>bytes.size()-offset) throw std::runtime_error("scan text bound");
        std::string result(reinterpret_cast<const char*>(bytes.data()+offset),static_cast<size_t>(length));
        offset+=static_cast<size_t>(length);
        if(result.find('\0')!=std::string::npos) throw std::runtime_error("embedded wire null");
        return result;
    }
    void end() const { if(offset!=bytes.size()) throw std::runtime_error("trailing scan request"); }
};
inline Reader receive(const char* magic) {
    std::array<uint8_t,12> header{}; transfer(header.data(),header.size(),false);
    if(std::memcmp(header.data(),magic,8)) throw std::runtime_error("out-of-order scan frame");
    size_t size=0;
    for(size_t n=0;n<4;++n) size|=size_t(header[8+n])<<(8*n);
    if(size>packet_limit) throw std::runtime_error("scan packet bound");
    Reader result{Bytes(size),0}; transfer(result.bytes.data(),size,false); return result;
}
inline void send(const char* magic,Bytes bytes) {
    if(bytes.size()>packet_limit) throw std::runtime_error("scan result bound");
    Bytes frame(magic,magic+8); number(frame,bytes.size(),4); frame.insert(frame.end(),bytes.begin(),bytes.end());
    transfer(frame.data(),frame.size(),true);
}
struct Attempt {
    uint64_t request=0,generation=0;
    explicit Attempt(Reader& input) {
        if(input.number(2)!=1) throw std::runtime_error("scan version refused");
        request=input.number(8); generation=input.number(8);
        if(!request || !generation) throw std::runtime_error("scan identity refused");
    }
    void write(Bytes& output) const { number(output,1,2); number(output,request,8); number(output,generation,8); }
    void check(Reader& input) const {
        Attempt other(input);
        if(other.request!=request || other.generation!=generation) throw std::runtime_error("stale scan admission");
    }
};
} // namespace MacScan
