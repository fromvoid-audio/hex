// Original factory-only failure fixtures. Advertised classes are metadata traps:
// createInstance must never be called by discovery. No production DSP is claimed.
#include "pluginterfaces/base/ipluginbase.h"
#include <CoreFoundation/CoreFoundation.h>
#include <atomic>
#include <csignal>
#include <cstdio>
#include <cstring>
#include <unistd.h>

using namespace Steinberg;
#ifndef HEX_FACTORY_MODE
#define HEX_FACTORY_MODE 0
#endif
static void hang() { for(;;) pause(); }
static void crash() { std::raise(SIGSEGV); }
__attribute__((constructor)) static void load() {
    if(HEX_FACTORY_MODE==14) hang();
    if(HEX_FACTORY_MODE==15) crash();
}

class Factory final : public IPluginFactory {
    std::atomic<uint32> refs{0};
public:
    uint32 PLUGIN_API addRef() override { return ++refs; }
    uint32 PLUGIN_API release() override { return --refs; }
    uint32 count() const { return refs.load(); }
    tresult PLUGIN_API queryInterface(const TUID iid,void** out) override {
        if(!out) return kInvalidArgument;
        *out=nullptr;
        if(!FUnknownPrivate::iidEqual(iid,INLINE_UID_OF(FUnknown)) &&
           !FUnknownPrivate::iidEqual(iid,INLINE_UID_OF(IPluginFactory))) return kNoInterface;
        *out=this; addRef(); return kResultOk;
    }
    tresult PLUGIN_API getFactoryInfo(PFactoryInfo* out) override {
        if(!out) return kInvalidArgument;
        *out=PFactoryInfo("hex original factory fixtures","https://invalid.example","fixture@invalid.example",0);
        return kResultOk;
    }
    int32 PLUGIN_API countClasses() override {
        if(HEX_FACTORY_MODE==3) hang();
        if(HEX_FACTORY_MODE==13) crash();
        if(HEX_FACTORY_MODE==4) return 1025;
        return 2;
    }
    tresult PLUGIN_API getClassInfo(int32 index,PClassInfo* out) override {
        if(!out || index<0 || index>1) return kInvalidArgument;
        TUID cid={0x48,0x45,0x58,0x46,0x41,0x43,0x54,0x31,0x01,0x02,0x03,0x04,0x05,0x06,0x07,0};
        cid[15]=static_cast<char>(HEX_FACTORY_MODE==6?1:index+1);
        *out=PClassInfo(cid,PClassInfo::kManyInstances,index?"Component Controller Class":"Audio Module Class",
                       index?"hex original factory controller":"hex original factory processor");
        if(HEX_FACTORY_MODE==5) std::memset(out->name,'x',sizeof out->name);
        return kResultOk;
    }
    tresult PLUGIN_API createInstance(FIDString,FIDString,void** out) override {
        if(out) *out=nullptr;
        std::fprintf(stderr,"forbidden ordinary instance creation\n"); return kNotImplemented;
    }
};
static Factory factory;
extern "C" __attribute__((visibility("default"))) bool bundleEntry(CFBundleRef) {
    if(HEX_FACTORY_MODE==1) hang();
    if(HEX_FACTORY_MODE==2) crash();
    if(HEX_FACTORY_MODE==10) {
        char block[4096]; std::memset(block,'d',sizeof block);
        for(int n=0;n<1024;++n) if(write(STDOUT_FILENO,block,sizeof block)<0) break;
    }
    if(HEX_FACTORY_MODE==11) {
        // Same-group child holds inherited channels open. Parent discovery must
        // not await EOF or claim Darwin adopted/reaped this descendant.
        pid_t child=fork();
        if(child<0) return false;
        if(!child) { sleep(5); _exit(0); }
    }
    if(HEX_FACTORY_MODE==12) {
        // Leave the original owned group; only the direct unreaped Child is a
        // safe fallback target. Never signal the newly joined parent group.
        if(setpgid(0,getpgid(getppid()))!=0) return false;
        hang();
    }
    return true;
}
extern "C" __attribute__((visibility("default"))) IPluginFactory* GetPluginFactory() { factory.addRef(); return &factory; }
extern "C" __attribute__((visibility("default"))) bool bundleExit() {
    if(HEX_FACTORY_MODE==7) return false;
    if(HEX_FACTORY_MODE==8) hang();
    if(HEX_FACTORY_MODE==9) crash();
    return factory.count()==0;
}
