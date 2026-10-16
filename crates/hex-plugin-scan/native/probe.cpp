// Original controlled scanner. SDK interfaces used under retained MIT notices.
#include "pluginterfaces/base/ipluginbase.h"
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
using namespace Steinberg;
static constexpr auto sdk="3cdf9ca5d1f5b1b21e0a86832aa4abe55607bd96";
static constexpr auto public_sdk="586dc5e6c8012c3e4b01c79389375cbe96bdb1da";
static void number(std::vector<uint8_t>& b,uint64_t value,unsigned bytes) {for(unsigned i=0;i<bytes;++i)b.push_back(uint8_t(value>>(8*i)));}
static void text(std::vector<uint8_t>& b,const char* str,size_t bound) {size_t size=strnlen(str,bound);if(size==bound)throw std::runtime_error("unterminated metadata");number(b,size,4);b.insert(b.end(),str,str+size);}
static void write_all(int fd,const uint8_t* p,size_t n) {while(n){auto count=write(fd,p,n);if(count<=0)throw std::runtime_error("protocol write failed");p+=count;n-=size_t(count);}}
struct NativeModule {
 void* library=nullptr; IPluginFactory* factory=nullptr; bool entered=false;
 using Entry=bool(*)(void*);using Exit=bool(*)();using Factory=IPluginFactory*(*)();
 Exit exit=nullptr;
 bool close() {
  if(factory){factory->release();factory=nullptr;}
  bool ok=true;if(entered){ok=exit();entered=false;}
  if(library){ok=(dlclose(library)==0)&&ok;library=nullptr;}
  return ok;
 }
 ~NativeModule(){close();}
 void load(const char* path) {
  library=dlopen(path,RTLD_NOW|RTLD_LOCAL);if(!library)throw std::runtime_error("module load failed");
  auto entry=reinterpret_cast<Entry>(dlsym(library,"ModuleEntry"));exit=reinterpret_cast<Exit>(dlsym(library,"ModuleExit"));
  auto get=reinterpret_cast<Factory>(dlsym(library,"GetPluginFactory"));
  if(!entry||!exit||!get)throw std::runtime_error("required SDK entry missing");
  if(!entry(library))throw std::runtime_error("module entry failed");
  entered=true;
  factory=get();if(!factory)throw std::runtime_error("factory missing");
 }
};
int main(int argc,char** argv) {
 // All stdout/stderr belong to plugin diagnostics. Dedicated protocol descriptor
 // is inherited from stdin, which is deliberately a writable pipe in this child.
 if(argc!=3)return 2;
 struct rlimit core_limit{};if(setrlimit(RLIMIT_CORE,&core_limit)!=0)return 2;
 try {
  char* end=nullptr;auto request=std::strtoull(argv[2],&end,10);if(!request||!end||*end)return 2;
  NativeModule module;module.load(argv[1]);
  PFactoryInfo info{};if(module.factory->getFactoryInfo(&info)!=kResultOk)throw std::runtime_error("factory info failed");
  auto count=module.factory->countClasses();if(count<0||count>1024)throw std::runtime_error("factory class bound");
  std::vector<uint8_t> payload;number(payload,1,2);number(payload,request,8);text(payload,sdk,41);text(payload,public_sdk,41);
#if defined(__x86_64__)
  text(payload,"linux-x86_64",32);
#elif defined(__aarch64__)
  text(payload,"linux-aarch64",32);
#else
#error Unsupported scanner helper architecture
#endif
  number(payload,uint32_t(info.flags),4);text(payload,info.vendor,sizeof info.vendor);text(payload,info.url,sizeof info.url);text(payload,info.email,sizeof info.email);number(payload,uint32_t(count),4);
  // Do not create any component/controller/processor/editor, including dynamic
  // classes. Basic class information suffices; extended capabilities stay unknown.
  for(int32 i=0;i<count;++i){PClassInfo ci{};if(module.factory->getClassInfo(i,&ci)!=kResultOk)throw std::runtime_error("class info failed");payload.insert(payload.end(),ci.cid,ci.cid+16);number(payload,uint32_t(ci.cardinality),4);text(payload,ci.category,sizeof ci.category);text(payload,ci.name,sizeof ci.name);}
  if(payload.size()>1024*1024)throw std::runtime_error("packet bound");
  if(!module.close())throw std::runtime_error("native unload refused");
  std::vector<uint8_t> packet{'H','V','S','C','A','N','0','1'};number(packet,payload.size(),4);packet.insert(packet.end(),payload.begin(),payload.end());
  write_all(STDIN_FILENO,packet.data(),packet.size());return 0;
 }catch(const std::exception& e){std::fprintf(stderr,"native probe: %s\n",e.what());return 2;}
}
