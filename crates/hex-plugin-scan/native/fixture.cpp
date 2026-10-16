// Original factory-only SDK fixture: no audio component/processor/editor exists.
#include "pluginterfaces/base/ipluginbase.h"
#include <atomic>
#include <cstring>
#include <csignal>
#include <cstdio>
#include <unistd.h>
using namespace Steinberg;
#ifndef FIXTURE_MODE
#define FIXTURE_MODE 0
#endif
class Factory final:public IPluginFactory {
public:
 std::atomic<uint32> refs{0};
 tresult PLUGIN_API queryInterface(const TUID iid,void** object) override {
  *object=nullptr;if(FUnknownPrivate::iidEqual(iid,INLINE_UID_OF(FUnknown))||FUnknownPrivate::iidEqual(iid,INLINE_UID_OF(IPluginFactory))){*object=this;addRef();return kResultOk;}return kNoInterface;
 }
 uint32 PLUGIN_API addRef() override {return ++refs;}
 uint32 PLUGIN_API release() override {auto value=--refs;std::fprintf(stderr,"fixture factory release=%u\n",value);return value;}
 tresult PLUGIN_API getFactoryInfo(PFactoryInfo* info) override {
  *info=PFactoryInfo("hex original \"SDK\" fixture","https://invalid.example/fixture","fixture@invalid.example",FIXTURE_MODE==4?PFactoryInfo::kClassesDiscardable:0);return kResultOk;
 }
 int32 PLUGIN_API countClasses() override {return 1;}
 tresult PLUGIN_API getClassInfo(int32 index,PClassInfo* info) override {
  if(index!=0)return kInvalidArgument;
  static const TUID cid={0x21,0x01,0x10,0x02,0x03,0x04,0x05,0x06,0x07,0x08,0x09,0x10,0x11,0x12,0x13,0x14};
  *info=PClassInfo(cid,PClassInfo::kManyInstances,"Audio Module Class","factory only \"fixture\"");return kResultOk;
 }
 tresult PLUGIN_API createInstance(FIDString,FIDString,void** object) override {*object=nullptr;std::fprintf(stderr,"FORBIDDEN component creation\n");return kNotImplemented;}
};
static Factory factory;
extern "C" __attribute__((visibility("default"))) bool ModuleEntry(void*) {
 std::puts("plugin stdout is a diagnostic, never metadata");std::fflush(stdout);
 if(FIXTURE_MODE==1){for(;;)pause();}
 if(FIXTURE_MODE==2){std::raise(SIGSEGV);return false;}
 if(FIXTURE_MODE==3){auto pid=fork();if(pid==0){signal(SIGTERM,SIG_IGN);for(;;)pause();}std::fprintf(stderr,"fixture descendant=%ld\n",long(pid));}
 return true;
}
extern "C" __attribute__((visibility("default"))) IPluginFactory* GetPluginFactory() {factory.addRef();return &factory;}
extern "C" __attribute__((visibility("default"))) bool ModuleExit() {std::fprintf(stderr,"fixture module exit refs=%u\n",factory.refs.load());return factory.refs.load()==0;}
