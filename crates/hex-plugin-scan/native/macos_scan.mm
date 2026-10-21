// Original factory-only child. Metadata inspection does not load foreign code.
#import <Foundation/Foundation.h>
#include <CommonCrypto/CommonDigest.h>
#include "pluginterfaces/base/ipluginbase.h"
#include "fixture_identity.hpp"
#include "macos_scan_wire.hpp"
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <memory>

using namespace Steinberg;
using namespace MacScan;
#ifndef HEX_SCAN_HANG_AFTER_REPLY
#define HEX_SCAN_HANG_AFTER_REPLY 0
#endif

static std::string utf8(NSString* value,size_t maximum) {
    if(![value isKindOfClass:[NSString class]]) throw std::runtime_error("bundle string missing");
    const char* bytes=[value UTF8String];
    auto size=[value lengthOfBytesUsingEncoding:NSUTF8StringEncoding];
    if(!bytes || size>maximum) throw std::runtime_error("bundle text bound or encoding refused");
    std::string result(bytes,size);
    if(result.find('\0')!=std::string::npos) throw std::runtime_error("bundle text null refused");
    return result;
}
static NSString* string(const std::string& value) {
    NSString* result=[[NSString alloc] initWithBytes:value.data() length:value.size() encoding:NSUTF8StringEncoding];
    if(!result) throw std::runtime_error("bundle path encoding refused");
    return [result autorelease];
}
static std::string canonical(const std::string& path) {
    std::unique_ptr<char,decltype(&std::free)> value(::realpath(path.c_str(),nullptr),&std::free);
    if(!value) throw std::runtime_error("bundle filesystem resolution failed");
    std::string result(value.get());
    if(result.size()>path_limit) throw std::runtime_error("canonical bundle path bound");
    return result;
}
static NSString* member(NSString* root,NSString* relative) {
    NSString* current=root;
    auto components=[relative pathComponents];
    if(![components count] || [components count]>32) throw std::runtime_error("bundle member depth refused");
    for(NSString* part in components) {
        if([part isEqual:@".."] || [part isEqual:@"."] || [part isEqual:@"/"]) throw std::runtime_error("bundle member escape refused");
        current=[current stringByAppendingPathComponent:part];
        NSDictionary* info=[[NSFileManager defaultManager] attributesOfItemAtPath:current error:nullptr];
        if(!info || [[info fileType] isEqual:NSFileTypeSymbolicLink]) throw std::runtime_error("linked/missing bundle member");
    }
    NSDictionary* info=[[NSFileManager defaultManager] attributesOfItemAtPath:current error:nullptr];
    if(![[info fileType] isEqual:NSFileTypeRegular]) throw std::runtime_error("nonregular bundle input");
    return current;
}
static std::string digest(NSString* path,uint64_t maximum) {
    NSDictionary* info=[[NSFileManager defaultManager] attributesOfItemAtPath:path error:nullptr];
    if(!info || ![[info fileType] isEqual:NSFileTypeRegular] || [info fileSize]>maximum) throw std::runtime_error("scan file bound refused");
    std::ifstream input([path fileSystemRepresentation],std::ios::binary);
    if(!input) throw std::runtime_error("scan file open failed");
    CC_SHA256_CTX context{}; CC_SHA256_Init(&context);
    std::array<char,65536> block{};
    uint64_t total=0;
    while(input) {
        input.read(block.data(),block.size());
        auto count=input.gcount();
        if(count<0 || static_cast<uint64_t>(count)>maximum-total) throw std::runtime_error("scan file grew beyond bound");
        total+=static_cast<uint64_t>(count);
        CC_SHA256_Update(&context,block.data(),static_cast<CC_LONG>(count));
    }
    if(!input.eof()) throw std::runtime_error("scan file read failed");
    std::array<unsigned char,CC_SHA256_DIGEST_LENGTH> hash{}; CC_SHA256_Final(hash.data(),&context);
    char result[65]={};
    for(size_t i=0;i<hash.size();++i) std::snprintf(result+2*i,3,"%02x",hash[i]);
    return result;
}
struct Module {
    CFBundleRef bundle=nullptr;
    IPluginFactory* factory=nullptr;
    bool loaded=false,entered=false;
    bool (*exit)()=nullptr;
    bool close() {
        if(factory) { auto* owned=factory; factory=nullptr; owned->release(); }
        bool ok=entered;
        if(entered) { entered=false; ok=exit && exit(); }
        if(loaded) { loaded=false; CFBundleUnloadExecutable(bundle); }
        if(bundle) { CFRelease(bundle); bundle=nullptr; }
        return ok;
    }
    ~Module() { try { close(); } catch(...) {} }
};
static std::string executable(Module& module,NSString* root) {
    CFURLRef selected=CFBundleCopyExecutableURL(module.bundle);
    CFURLRef absolute=selected?CFURLCopyAbsoluteURL(selected):nullptr;
    CFStringRef path=absolute?CFURLCopyFileSystemPath(absolute,kCFURLPOSIXPathStyle):nullptr;
    std::string relative;
    try {
        NSString* prefix=[root stringByAppendingString:@"/"];
        NSString* value=path?(__bridge NSString*)path:nil;
        if(!value || ![value hasPrefix:prefix]) throw std::runtime_error("executable outside selected bundle");
        NSString* result=[value substringFromIndex:[prefix length]];
        if(![result hasPrefix:@"Contents/MacOS/"]) throw std::runtime_error("native executable location refused");
        member(root,result);
        relative=utf8(result,path_limit);
    } catch(...) {
        if(path) CFRelease(path); if(absolute) CFRelease(absolute); if(selected) CFRelease(selected);
        throw;
    }
    if(path) CFRelease(path); if(absolute) CFRelease(absolute); if(selected) CFRelease(selected);
    return relative;
}

static Bytes discover(Module& module,const Attempt& attempt) {
    PFactoryInfo info{};
    if(module.factory->getFactoryInfo(&info)!=kResultOk) throw std::runtime_error("factory information failed");
    int32 count=module.factory->countClasses();
    if(count<0 || count>1024) throw std::runtime_error("factory class count refused");
    Bytes body; number(body,1,2); number(body,attempt.request,8);
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
            std::string selected=executable(module,root);
            std::string identifier=utf8((__bridge NSString*)CFBundleGetIdentifier(module.bundle),256);
            if(identifier.empty()) throw std::runtime_error("bundle identifier required");
            Bytes preflight; attempt.write(preflight); text(preflight,selected,path_limit);
            text(preflight,identifier,256); text(preflight,info_hash,64); send("HXMACPF1",std::move(preflight));

            // No function lookup, module entry or executable load before ACK.
            Reader ack=receive("HXMACAK1"); attempt.check(ack);
            std::string admitted=ack.text(path_limit),admitted_info=ack.text(64),binary_hash=ack.text(64);
            auto image=ack.number(4); ack.end();
            if(admitted!=selected || admitted_info!=info_hash || binary_hash.size()!=64 || (image!=1 && image!=2) ||
               digest(member(root,@"Contents/Info.plist"),65536)!=info_hash || executable(module,root)!=selected ||
               digest(member(root,string(selected)),16*1024*1024)!=binary_hash)
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
