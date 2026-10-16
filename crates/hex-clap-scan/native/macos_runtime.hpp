// ORIGINAL bounded loaded-image UUID provenance. Not image hashes or containment.
#pragma once
#if defined(__APPLE__) && defined(__aarch64__)
#include <mach-o/dyld.h>
#include <mach-o/loader.h>
#include <sys/utsname.h>
#include <array>
#include <string>
#include <vector>
#include <stdexcept>
#include <cstring>
namespace HexMacRuntime {
inline std::string text(const char* s) {
  if(!s||strnlen(s,1025)>1024)throw std::runtime_error("runtime string bound");
  std::string out="\"";
  for(const unsigned char* p=reinterpret_cast<const unsigned char*>(s);*p;p++) {
    if(*p<32||*p>=127)throw std::runtime_error("runtime path encoding refused");
    if(*p=='"'||*p=='\\'){out+='\\';}
    out+=char(*p);
  }return out+'"';
}
// Called only before module load and after worker join/destruction/unload. No
// concurrent dlopen, enumeration callback, foreign pointer or thread retention.
inline std::string snapshot() {
  utsname os{};if(uname(&os))throw std::runtime_error("runtime OS identity unavailable");
  auto n=_dyld_image_count();if(n<2||n>64)throw std::runtime_error("runtime image bound");
  std::string out="{\"kind\":\"loaded-image-uuid-v1\",\"shared_cache_uuid\":null,\"os_release\":"+text(os.release)+",\"images\":[";
  for(uint32_t i=1;i<n;i++) {
    auto* raw=_dyld_get_image_header(i);auto* path=_dyld_get_image_name(i);
    if(!raw||!path||raw->magic!=MH_MAGIC_64)throw std::runtime_error("runtime image unavailable");
    auto* h=reinterpret_cast<const mach_header_64*>(raw);
    if(h->cputype!=CPU_TYPE_ARM64||h->ncmds>4096||h->sizeofcmds>1024*1024)throw std::runtime_error("runtime image architecture/commands");
    const auto* bytes=reinterpret_cast<const uint8_t*>(h)+sizeof(*h);size_t at=0;std::string uuid;
    for(uint32_t j=0;j<h->ncmds;j++) {
      if(at>h->sizeofcmds||h->sizeofcmds-at<sizeof(load_command))throw std::runtime_error("runtime command range");
      const auto* c=reinterpret_cast<const load_command*>(bytes+at);
      if(c->cmdsize<sizeof(load_command)||c->cmdsize>h->sizeofcmds-at)throw std::runtime_error("runtime command size");
      if(c->cmd==LC_UUID) {
        if(!uuid.empty()||c->cmdsize!=sizeof(uuid_command))throw std::runtime_error("runtime UUID duplicate/size");
        const auto* u=reinterpret_cast<const uuid_command*>(c);const char* hex="0123456789abcdef";
        for(auto v:u->uuid){uuid+=hex[v>>4];uuid+=hex[v&15];}
      }at+=c->cmdsize;
    }
    if(uuid.size()!=32||at!=h->sizeofcmds)throw std::runtime_error("runtime UUID missing");
    if(i>1){out+=',';}
    out+="{\"path\":"+text(path)+",\"uuid\":\""+uuid+"\"}";
  }out+="]}";if(out.size()>32768)throw std::runtime_error("runtime snapshot capacity");return out;
}
inline void append(std::vector<uint8_t>& out,uint32_t n){for(unsigned i=0;i<4;i++)out.push_back(uint8_t(n>>(8*i)));}
inline std::vector<uint8_t> envelope(const std::vector<uint8_t>& packet,const std::string& before) {
  if(snapshot()!=before)throw std::runtime_error("loaded runtime changed/unload incomplete");
  if(packet.size()>1024*1024-32768-20)throw std::runtime_error("Mac envelope capacity");
  std::vector<uint8_t> b{'H','C','M','A','C','R','0','1'};append(b,uint32_t(8+before.size()+packet.size()));append(b,uint32_t(before.size()));b.insert(b.end(),before.begin(),before.end());append(b,uint32_t(packet.size()));b.insert(b.end(),packet.begin(),packet.end());return b;
}
}
#endif
