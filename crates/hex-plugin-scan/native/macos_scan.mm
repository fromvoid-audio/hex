// Original factory-only child. Metadata inspection does not load foreign code.
#import <Foundation/Foundation.h>
#include <CommonCrypto/CommonDigest.h>
#include "pluginterfaces/base/ipluginbase.h"
#include "fixture_identity.hpp"
#include "macos_scan_wire.hpp"
#include "macos_bundle.hpp"
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <memory>

using namespace Steinberg;
using namespace MacScan;
using hex_bundle::utf8;using hex_bundle::string;using hex_bundle::canonical;
using hex_bundle::member;using hex_bundle::digest;using hex_bundle::executable;
#ifndef HEX_SCAN_HANG_AFTER_REPLY
#define HEX_SCAN_HANG_AFTER_REPLY 0
#endif

struct Module {
    CFBundleRef bundle=nullptr;
    IPluginFactory* factory=nullptr;
    IPluginFactory2* extended=nullptr;
    bool loaded=false,entered=false;
    bool (*exit)()=nullptr;
    bool close() {
        if(extended) { auto* owned=extended; extended=nullptr; owned->release(); }
        if(factory) { auto* owned=factory; factory=nullptr; owned->release(); }
        bool ok=entered;
        if(entered) { entered=false; ok=exit && exit(); }
        if(loaded) { loaded=false; CFBundleUnloadExecutable(bundle); }
        if(bundle) { CFRelease(bundle); bundle=nullptr; }
        return ok;
    }
    ~Module() { try { close(); } catch(...) {} }
};
static Bytes discover(Module& module,const Attempt& attempt) {
    PFactoryInfo info{};
    if(module.factory->getFactoryInfo(&info)!=kResultOk) throw std::runtime_error("factory information failed");
    int32 count=module.factory->countClasses();
    if(count<0 || count>1024) throw std::runtime_error("factory class count refused");
    void* extended=nullptr;
    auto extension=module.factory->queryInterface(INLINE_UID_OF(IPluginFactory2),&extended);
    if(extension==kResultOk && extended) module.extended=static_cast<IPluginFactory2*>(extended);
    else if(extension!=kNoInterface || extended) throw std::runtime_error("optional factory interface refused");
    Bytes body; number(body,2,2); number(body,attempt.request,8);
    text(body,HEX_SDK_PIN,40); text(body,HEX_PUBLIC_SDK_PIN,40); text(body,"darwin-arm64",32);
    number(body,static_cast<uint32>(info.flags),4);
    text(body,field(info.vendor,sizeof info.vendor),64);
    text(body,field(info.url,sizeof info.url),256);
    text(body,field(info.email,sizeof info.email),128); number(body,static_cast<uint32>(count),4);
    for(int32 index=0;index<count;++index) {
        PClassInfo item{};
        if(module.factory->getClassInfo(index,&item)!=kResultOk) throw std::runtime_error("factory class query failed");
        body.insert(body.end(),item.cid,item.cid+16);
        number(body,static_cast<uint32>(item.cardinality),4);
        text(body,field(item.category,sizeof item.category),32);
        text(body,field(item.name,sizeof item.name),64);
        number(body,module.extended?1:0,1);
        if(module.extended) {
            PClassInfo2 detail{};
            if(module.extended->getClassInfo2(index,&detail)!=kResultOk) throw std::runtime_error("extended factory class query failed");
            if(std::memcmp(item.cid,detail.cid,sizeof item.cid) || item.cardinality!=detail.cardinality ||
               field(item.category,sizeof item.category)!=field(detail.category,sizeof detail.category))
                throw std::runtime_error("extended factory class identity differs");
            // Basic class name remains the display identity; still validate all
            // bounded extended strings before accepting the copied class record.
            field(detail.name,sizeof detail.name);
            number(body,detail.classFlags,4);
            text(body,field(detail.subCategories,sizeof detail.subCategories),128);
            text(body,field(detail.vendor,sizeof detail.vendor),64);
            text(body,field(detail.version,sizeof detail.version),64);
            text(body,field(detail.sdkVersion,sizeof detail.sdkVersion),64);
        }
    }
    Bytes result{'H','V','S','C','A','N','0','1'};
    number(result,body.size(),4); result.insert(result.end(),body.begin(),body.end());
    return result;
}

int main(int argc,char**) {
    @autoreleasepool {
        if(argc!=1) { std::fprintf(stderr,"factory scan requires its owned protocol channel\n"); return 2; }
        try {
            Reader request=receive("HXMACRQ1"); Attempt attempt(request);
            std::string root_path=request.text(path_limit),info_hash=request.text(64); request.end();
            if(info_hash.size()!=64) throw std::runtime_error("Info.plist identity required");
            NSString* root=string(root_path);
            // Match Rust fs::canonicalize's POSIX filesystem semantics.
            // NSString standardization may collapse /private/var back to /var.
            if(canonical(root_path)!=root_path) throw std::runtime_error("canonical bundle root required");
            NSString* info_path=member(root,@"Contents/Info.plist");
            if(digest(info_path,65536)!=info_hash) throw std::runtime_error("Info.plist changed before metadata inspection");
            Module module;
            CFURLRef url=CFURLCreateWithFileSystemPath(nullptr,(__bridge CFStringRef)root,kCFURLPOSIXPathStyle,true);
            if(!url) throw std::runtime_error("bundle URL unavailable");
            module.bundle=CFBundleCreate(nullptr,url); CFRelease(url);
            if(!module.bundle) throw std::runtime_error("bundle metadata unavailable");
            std::string selected=executable(module.bundle,root);
            std::string identifier=utf8((__bridge NSString*)CFBundleGetIdentifier(module.bundle),256);
            if(identifier.empty()) throw std::runtime_error("bundle identifier required");
            Bytes preflight; attempt.write(preflight); text(preflight,selected,path_limit);
            text(preflight,identifier,256); text(preflight,info_hash,64); send("HXMACPF1",std::move(preflight));

            // No function lookup, module entry or executable load before ACK.
            Reader ack=receive("HXMACAK1"); attempt.check(ack);
            std::string admitted=ack.text(path_limit),admitted_info=ack.text(64),binary_hash=ack.text(64);
            auto image=ack.number(4); ack.end();
            if(admitted!=selected || admitted_info!=info_hash || binary_hash.size()!=64 || (image!=1 && image!=2) ||
               digest(member(root,@"Contents/Info.plist"),65536)!=info_hash || executable(module.bundle,root)!=selected ||
               digest(member(root,string(selected)))!=binary_hash)
                throw std::runtime_error("stale or changed executable admission");
            if(!CFBundleLoadExecutable(module.bundle)) throw std::runtime_error("native module load failed");
            module.loaded=true;
            auto entry=reinterpret_cast<bool(*)(CFBundleRef)>(CFBundleGetFunctionPointerForName(module.bundle,CFSTR("bundleEntry")));
            module.exit=reinterpret_cast<bool(*)()>(CFBundleGetFunctionPointerForName(module.bundle,CFSTR("bundleExit")));
            auto get=reinterpret_cast<IPluginFactory*(*)()>(CFBundleGetFunctionPointerForName(module.bundle,CFSTR("GetPluginFactory")));
            if(!entry || !module.exit || !get || !entry(module.bundle)) throw std::runtime_error("Mac module entry failed");
            module.entered=true; module.factory=get();
            if(!module.factory) throw std::runtime_error("module factory missing");
            Bytes classes=discover(module,attempt);
            if(!module.close()) throw std::runtime_error("module exit refused");
            // Unload is void: this records its completed request, not proof that
            // dyld unmapped every image. Parent still requires actual exit/reap.
            Bytes result; attempt.write(result); number(result,7,4);
            result.insert(result.end(),classes.begin(),classes.end()); send("HXMACRS1",std::move(result));
            // Separate original test build only; never a runtime-selectable mode.
            if(HEX_SCAN_HANG_AFTER_REPLY) for(;;) pause();
            return 0;
        } catch(const std::exception& error) {
            std::fprintf(stderr,"factory scan failed: %.256s\n",error.what()); return 1;
        } catch(...) { std::fprintf(stderr,"factory scan failed\n"); return 1; }
    }
}
