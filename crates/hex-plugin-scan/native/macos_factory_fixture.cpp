// Original factory-only failure fixtures. Advertised classes are metadata traps:
// createInstance must never be called by discovery. No production DSP is claimed.
#include "pluginterfaces/base/ipluginbase.h"
#include <CoreFoundation/CoreFoundation.h>
#include <atomic>
#include <cerrno>
#include <csignal>
#include <cstdio>
#include <cstring>
#include <fcntl.h>
#include <limits.h>
#include <unistd.h>

using namespace Steinberg;
#ifndef HEX_FACTORY_MODE
#define HEX_FACTORY_MODE 0
#endif
static void hang() { for(;;) pause(); }
static void crash() { std::raise(SIGSEGV); }

// Opt-in, per-copy test signal. Both files are siblings of the copied bundle,
// so this never modifies its signed contents. Without the request, existing
// entry-hang fixtures retain their original behavior.
static bool signal_entry_if_requested(CFBundleRef bundle) {
    CFURLRef url=CFBundleCopyBundleURL(bundle);
    if(!url) return false;
    UInt8 path[PATH_MAX];
    bool converted=CFURLGetFileSystemRepresentation(url,true,path,sizeof path);
    CFRelease(url);
    if(!converted) return false;
    size_t length=std::strlen(reinterpret_cast<const char*>(path));
    while(length>1 && path[length-1]=='/') path[--length]=0;
    char request[PATH_MAX],ready[PATH_MAX];
    const char* bundle_path=reinterpret_cast<const char*>(path);
    int a=std::snprintf(request,sizeof request,"%s.entry-wait",bundle_path);
    int b=std::snprintf(ready,sizeof ready,"%s.entry-ready",bundle_path);
    if(a<0 || b<0 || static_cast<size_t>(a)>=sizeof request || static_cast<size_t>(b)>=sizeof ready) return false;
    int requested=open(request,O_RDONLY|O_CLOEXEC|O_NOFOLLOW);
    if(requested<0) return errno==ENOENT;
    if(close(requested)!=0) return false;
    int marker=open(ready,O_WRONLY|O_CREAT|O_EXCL|O_CLOEXEC|O_NOFOLLOW,0600);
    if(marker<0) return false;
    constexpr char message[]="entry-hang\n";
    bool written=write(marker,message,sizeof message-1)==static_cast<ssize_t>(sizeof message-1);
    return close(marker)==0 && written;
}
__attribute__((constructor)) static void load() {
    if(HEX_FACTORY_MODE==14) hang();
    if(HEX_FACTORY_MODE==15) crash();
}

class Factory final : public IPluginFactory2 {
    std::atomic<uint32> refs{0};
public:
    uint32 PLUGIN_API addRef() override { return ++refs; }
    uint32 PLUGIN_API release() override { return --refs; }
    uint32 count() const { return refs.load(); }
    tresult PLUGIN_API queryInterface(const TUID iid,void** out) override {
        if(!out) return kInvalidArgument;
        *out=nullptr;
        if(FUnknownPrivate::iidEqual(iid,INLINE_UID_OF(IPluginFactory2))) {
            if(HEX_FACTORY_MODE==22) return kResultFalse;
            if(HEX_FACTORY_MODE==25) return kResultOk;
            if(HEX_FACTORY_MODE<16) return kNoInterface;
        } else if(!FUnknownPrivate::iidEqual(iid,INLINE_UID_OF(FUnknown)) &&
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
    tresult PLUGIN_API getClassInfo2(int32 index,PClassInfo2* out) override {
        if(HEX_FACTORY_MODE==24) return kResultFalse;
        PClassInfo basic{};
        if(!out || getClassInfo(index,&basic)!=kResultOk) return kInvalidArgument;
        const char* kind=index?"":HEX_FACTORY_MODE==16?"Instrument|Synth":
            HEX_FACTORY_MODE==17?"Fx|Delay":HEX_FACTORY_MODE==18?"Fx|Instrument":"MyInstrument|CustomFx";
        *out=PClassInfo2(basic.cid,basic.cardinality,basic.category,basic.name,0x80000000u,
                        kind,"original extended vendor","1.2.3","VST 3.8");
        if(HEX_FACTORY_MODE==20) out->cid[0]=0;
        if(HEX_FACTORY_MODE==21) std::memset(out->subCategories,'x',sizeof out->subCategories);
        if(HEX_FACTORY_MODE==23) { out->subCategories[0]=static_cast<char>(0xff); out->subCategories[1]=0; }
        return kResultOk;
    }
    tresult PLUGIN_API createInstance(FIDString,FIDString,void** out) override {
        if(out) *out=nullptr;
        std::fprintf(stderr,"forbidden ordinary instance creation\n"); return kNotImplemented;
    }
};
static Factory factory;
extern "C" __attribute__((visibility("default"))) bool bundleEntry(CFBundleRef bundle) {
    if(HEX_FACTORY_MODE==1) { if(!signal_entry_if_requested(bundle)) return false; hang(); }
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
