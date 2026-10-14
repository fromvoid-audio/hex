// ORIGINAL controlled CLAP descriptor scanner; official MIT headers only.
#include <clap/entry.h>
#include <clap/factory/plugin-factory.h>
#include <dlfcn.h>
#include <unistd.h>
#include <sys/resource.h>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cstdint>
#include <stdexcept>
#include <string>
#include <vector>
#include <set>
#include "platform_identity.hpp"
#include "macos_runtime.hpp"
static constexpr size_t maximum=1024*1024;
static void check(bool value,const char* error) {
  if(!value)throw std::runtime_error(error);
}
struct Writer  {
  std::vector<uint8_t> bytes;
  void number(uint64_t value,unsigned count) {
    check(bytes.size()+count<=maximum,"packet capacity");
    for(unsigned i=0;i<count;i++)bytes.push_back(uint8_t(value>>(8*i)));
  }
  void raw(const void* p,size_t count) {
    check(count<=maximum-bytes.size(),"packet capacity");
    auto b=static_cast<const uint8_t*>(p);
    if(count)bytes.insert(bytes.end(),b,b+count);
  }
  std::string text(const char* p,size_t bound,bool mandatory=false) {
    if(!p) {
      check(!mandatory,"mandatory descriptor missing");
      number(0,4);
      return  {};
    }
    auto n=strnlen(p,bound+1);
    check(n<=bound,"unterminated descriptor string");
    check(!mandatory||n>0,"mandatory descriptor blank");
    number(n,4);
    raw(p,n);
    return std::string(p,n);
  }
};
struct Module  {
  void* library=nullptr;
  const clap_plugin_entry_t* entry=nullptr;
  bool initialized=false;
  void load(const char* path) {
    library=dlopen(path,RTLD_NOW|RTLD_LOCAL);
    check(library,"module load failed");
    entry=static_cast<const clap_plugin_entry_t*>(dlsym(library,"clap_entry"));
    check(entry,"entry missing");
    check(entry->clap_version.major==1&&clap_version_is_compatible(entry->clap_version),"unsupported CLAP API");
    check(entry->init&&entry->deinit&&entry->get_factory,"entry callbacks missing");
    check(entry->init(path),"entry initialization failed");
    initialized=true;
  }
  bool close() {
    if(initialized) {
      entry->deinit();
      initialized=false;
    }entry=nullptr;
    bool ok=true;
    if(library) {
      ok=dlclose(library)==0;
      library=nullptr;
    }return ok;
  }
  ~Module() {
    close();
  }
};
static void write_all(const std::vector<uint8_t>& bytes) {
  size_t at=0;
  while(at<bytes.size()) {
    auto n=write(STDIN_FILENO,bytes.data()+at,bytes.size()-at);
    check(n>0,"protocol write failed");
    at+=size_t(n);
  }
}
int main(int argc,char** argv) {
#if defined(__APPLE__) && defined(__aarch64__)
  if(argc==2&&!std::strcmp(argv[1],"--runtime-identity")) {
    try {std::puts(HexMacRuntime::snapshot().c_str());return 0;}catch(const std::exception& e){std::fprintf(stderr,"runtime identity: %s\n",e.what());return 2;}
  }
#endif
  if(argc!=3)return 2;
  rlimit core {};
  if(setrlimit(RLIMIT_CORE,&core)!=0)return 2;
  try  {
    char* end=nullptr;
    auto request=std::strtoull(argv[2],&end,10);
    check(request&&end&&!*end,"request identity");
#if defined(__APPLE__) && defined(__aarch64__)
    const auto runtime_before=HexMacRuntime::snapshot();
#endif
    Module module;
    module.load(argv[1]);
    auto factory=static_cast<const clap_plugin_factory_t*>(module.entry->get_factory(CLAP_PLUGIN_FACTORY_ID));
    check(factory&&factory->get_plugin_count&&factory->get_plugin_descriptor&&factory->create_plugin,"factory missing/incomplete");
    auto count=factory->get_plugin_count(factory);
    check(count<=1024,"descriptor count capacity");
    Writer w;
    w.number(1,2);
    w.number(request,8);
    w.text("195b42a004144fab0b3cf95e9c067187d15365b7",41);
    w.text(HexFixturePlatform::name,32);
    w.number(module.entry->clap_version.major,4);
    w.number(module.entry->clap_version.minor,4);
    w.number(module.entry->clap_version.revision,4);
    w.number(count,4);
    std::set<std::string> ids;
    for(uint32_t i=0;i<count;i++) {
      auto d=factory->get_plugin_descriptor(factory,i);
      check(d,"null descriptor");
      check(d->clap_version.major==1&&clap_version_is_compatible(d->clap_version),"descriptor API refused");
      w.number(d->clap_version.major,4);
      w.number(d->clap_version.minor,4);
      w.number(d->clap_version.revision,4);
      auto id=w.text(d->id,512,true);
      check(ids.insert(id).second,"duplicate descriptor identity");
      w.text(d->name,512,true);
      w.text(d->vendor,512);
      w.text(d->url,512);
      w.text(d->manual_url,512);
      w.text(d->support_url,512);
      w.text(d->version,512);
      w.text(d->description,512);
      size_t n=0;
      if(d->features) {
        while(n<65&&d->features[n])++n;
      }check(n<=64,"feature count/termination capacity");
      w.number(n,4);
      for(size_t j=0;j<n;j++)w.text(d->features[j],128);
    }
    // Never call factory->create_plugin; all descriptor pointers are borrowed.
    check(module.close(),"module unload failed");
    Writer packet;
    packet.raw("HCLSCAN1",8);
    packet.number(w.bytes.size(),4);
    packet.raw(w.bytes.data(),w.bytes.size());
#if defined(__APPLE__) && defined(__aarch64__)
    write_all(HexMacRuntime::envelope(packet.bytes,runtime_before));
#else
    write_all(packet.bytes);
#endif
    return 0;
  }catch(const std::exception& e) {
    std::fprintf(stderr,"clap probe: %s\n",e.what());
    return 2;
  }
}
