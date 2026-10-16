// ORIGINAL factory-only CLAP fixture. No instance/processing/editor exists.
#include <clap/entry.h>
#include <clap/factory/plugin-factory.h>
#include <cstdio>
#include <cstring>
#include <csignal>
#include <unistd.h>
#ifndef FIXTURE_MODE
#define FIXTURE_MODE 0
#endif
static unsigned initialized=0;
static const char* features[]= {
  "audio-effect","stereo","hex-original-unknown",nullptr
};
static const char* unclosed[65];
static char malformed[513];
static const char bad_utf8[]= {
  char(0xff),0
};
static clap_plugin_descriptor_t descriptor= {
  CLAP_VERSION_INIT,"org.hex.controlled.gain","original \"descriptor\" fixture","hex","https://invalid.example",nullptr,nullptr,"fixture-v1","metadata only",features
};
static uint32_t count(const clap_plugin_factory_t*) {
  return FIXTURE_MODE==11?1025:FIXTURE_MODE==3?2:1;
}
static const clap_plugin_descriptor_t* get(const clap_plugin_factory_t*,uint32_t index) {
  if(index>=count(nullptr))return nullptr;
  return &descriptor;
}
static const clap_plugin_t* create(const clap_plugin_factory_t*,const clap_host_t*,const char*) {
  std::fputs("FORBIDDEN create_plugin\n",stderr);
  return nullptr;
}
static const clap_plugin_factory_t factory= {
  count,get,create
};
static bool init(const char*) {
  std::puts("original CLAP diagnostic, never IPC");
  std::fflush(stdout);
  if(FIXTURE_MODE==1)return false;
  if(FIXTURE_MODE==6) {
    std::raise(SIGSEGV);
    return false;
  }
  if(FIXTURE_MODE==7) {
    for(;;)pause();
  }
  if(FIXTURE_MODE==8) {
    auto child=fork();
    if(child==0) {
      signal(SIGTERM,SIG_IGN);
      for(;;)pause();
    }std::fprintf(stderr,"fixture descendant=%ld\n",long(child));
  }
  if(FIXTURE_MODE==4) {
    std::memset(malformed,'x',sizeof malformed);
    descriptor.name=malformed;
  }
  if(FIXTURE_MODE==10)descriptor.name=bad_utf8;
  if(FIXTURE_MODE==12) {
    for(auto& p:unclosed)p="unterminated-feature-array";
    descriptor.features=unclosed;
  }
  ++initialized;
  std::fprintf(stderr,"fixture init count=%u\n",initialized);
  return true;
}
static void deinit() {
  if(FIXTURE_MODE==9) {
    for(;;)pause();
  }--initialized;
  std::fprintf(stderr,"fixture deinit count=%u\n",initialized);
}
static const void* get_factory(const char* id) {
  if(FIXTURE_MODE==2)return nullptr;
  return std::strcmp(id,CLAP_PLUGIN_FACTORY_ID)==0?&factory:nullptr;
}
extern "C" CLAP_EXPORT const clap_plugin_entry_t clap_entry= {
  FIXTURE_MODE==5?clap_version_t {
    0,9,0
  }:CLAP_VERSION,init,deinit,get_factory
};
__attribute__((destructor)) static void unloaded() {
  std::fprintf(stderr,"fixture unload initialized=%u\n",initialized);
}
