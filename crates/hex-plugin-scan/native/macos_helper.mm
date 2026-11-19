// Original controlled native bootstrap. Installed modules are never accepted.
#import <Foundation/Foundation.h>
#include <CommonCrypto/CommonDigest.h>
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "fixture_identity.hpp"
#include <array>
#include <cstdio>
#include <cstring>
#include <thread>
#include <algorithm>
#include "macos_lifecycle_case.hpp"
#include "macos_host_case.hpp"
#include "macos_stream_case.hpp"
#include "macos_processing_case.hpp"

using namespace Steinberg;
using namespace Steinberg::Vst;

static bool fixture(const char* kind,const char* expected) {
    NSString* name=[NSString stringWithUTF8String:kind];
    NSString* relative=[NSString stringWithFormat:@"fixtures/%@.vst3",name];
    NSString* resources=[[[NSBundle mainBundle] resourcePath] stringByResolvingSymlinksInPath];
    NSString* path=[resources stringByAppendingPathComponent:relative];
    NSString* binary=[path stringByAppendingPathComponent:@"Contents/MacOS/fixture"];
    CFURLRef url=CFURLCreateWithFileSystemPath(nullptr,(__bridge CFStringRef)path,kCFURLPOSIXPathStyle,true);
    if(!url) return false;
    CFBundleRef bundle=CFBundleCreate(nullptr,url); CFRelease(url);
    if(!bundle) return false;
    // CFBundle selects its executable from Info.plist. Bind the actual selected
    // URL before hashing or loading, rather than trusting a parallel fixed path.
    CFURLRef executable=CFBundleCopyExecutableURL(bundle);
    CFURLRef absolute=executable?CFURLCopyAbsoluteURL(executable):nullptr;
    CFStringRef selected=absolute?CFURLCopyFileSystemPath(absolute,kCFURLPOSIXPathStyle):nullptr;
    NSString* identifier=[@"org.portalsurfer.hex.fixture." stringByAppendingString:name];
    bool exact=selected && [(__bridge NSString*)selected isEqualToString:binary] &&
        [(__bridge id)CFBundleGetValueForInfoDictionaryKey(bundle,CFSTR("CFBundleExecutable")) isEqual:@"fixture"] &&
        [(__bridge id)CFBundleGetValueForInfoDictionaryKey(bundle,CFSTR("CFBundleIdentifier")) isEqual:identifier];
    NSString* member=resources;
    for(NSString* component in [relative stringByAppendingPathComponent:@"Contents/MacOS/fixture"].pathComponents) {
        member=[member stringByAppendingPathComponent:component];
        NSDictionary* info=[[NSFileManager defaultManager] attributesOfItemAtPath:member error:nullptr];
        exact=exact && info && ![[info fileType] isEqualToString:NSFileTypeSymbolicLink];
    }
    if(!exact) {
        NSString* actual=selected?(__bridge NSString*)selected:@"missing";
        std::fprintf(stderr,"fixture executable selection refused: %s (expected %s)\n",
                     [actual UTF8String],[binary UTF8String]);
    }
    if(selected) CFRelease(selected);
    if(absolute) CFRelease(absolute);
    if(executable) CFRelease(executable);
    if(!exact) { CFRelease(bundle); return false; }
    NSDictionary* attrs=[[NSFileManager defaultManager] attributesOfItemAtPath:binary error:nullptr];
    if(!attrs || ![[attrs fileType] isEqualToString:NSFileTypeRegular] || [attrs fileSize]>16*1024*1024) {
        CFRelease(bundle); return false;
    }
    NSData* bytes=[NSData dataWithContentsOfFile:binary];
    if(!bytes || [bytes length]>16*1024*1024) { CFRelease(bundle); return false; }
    unsigned char digest[CC_SHA256_DIGEST_LENGTH];
    CC_SHA256([bytes bytes],static_cast<CC_LONG>([bytes length]),digest);
    char hex[65]={};
    for(size_t i=0;i<sizeof(digest);++i) std::snprintf(hex+2*i,3,"%02x",digest[i]);
    if(std::strcmp(hex,expected)) { CFRelease(bundle); return false; }
    bool loaded=CFBundleLoadExecutable(bundle);
    auto entry=reinterpret_cast<bool(*)(CFBundleRef)>(CFBundleGetFunctionPointerForName(bundle,CFSTR("bundleEntry")));
    auto exit=reinterpret_cast<bool(*)()>(CFBundleGetFunctionPointerForName(bundle,CFSTR("bundleExit")));
    auto get=reinterpret_cast<IPluginFactory*(*)()>(CFBundleGetFunctionPointerForName(bundle,CFSTR("GetPluginFactory")));
    bool entered=loaded && entry && exit && get && entry(bundle);
    bool ok=false;
    if(entered) {
        const bool lifecycle=std::strcmp(kind,"lifecycle")==0;
        if (lifecycle) {
            auto configure=reinterpret_cast<bool(*)(int,int)>(CFBundleGetFunctionPointerForName(bundle,CFSTR("HexLifecycleCase")));
            if (!configure || !configure(lifecycle_evidence.mode,lifecycle_evidence.failure)) {
                exit(); CFBundleUnloadExecutable(bundle); CFRelease(bundle); return false;
            }
        }
        auto* factory=get();
        if (lifecycle) {
            ok=hex_stream_fixture::enabled?hex_stream_fixture::run(bundle,factory):
                (host_evidence.scenario>=0?host_case(bundle,factory):lifecycle_case(bundle,factory));
            if (!lifecycle_evidence.closed) {
                // Failed teardown retains the module and both factory references.
                // The standalone process owner must reap this helper; no live unload.
                return ok;
            }
            if (factory) factory->release();
            if (!exit()) return false; // retain refused module exit until process retirement
            if (loaded) CFBundleUnloadExecutable(bundle);
            lifecycle_evidence.unloaded=!CFBundleIsExecutableLoaded(bundle);
            ok=lifecycle_evidence.unloaded && ok;
            CFRelease(bundle);
            return ok;
        }
        if (std::strcmp(kind,"processing")==0) {
            ok=processing_case(bundle,factory);
            if (!processing_evidence.closed) return ok; // retain live custody until process exit
            if (factory) factory->release();
            if (!exit()) return false;
            if (loaded) CFBundleUnloadExecutable(bundle);
            processing_evidence.unloaded=!CFBundleIsExecutableLoaded(bundle);
            ok=processing_evidence.unloaded && ok;
            CFRelease(bundle);
            return ok;
        }
        PClassInfo info{};
        if(factory && factory->countClasses()==1 && factory->getClassInfo(0,&info)==kResultOk) {
            IComponent* component=nullptr;
            bool created=factory->createInstance(info.cid,INLINE_UID_OF(IComponent),reinterpret_cast<void**>(&component))==kResultOk;
            if(created && component) {
                IAudioProcessor* processor=nullptr;
                bool initialized=component->initialize(nullptr)==kResultOk;
                bool queried=component->queryInterface(INLINE_UID_OF(IAudioProcessor),reinterpret_cast<void**>(&processor))==kResultOk;
                if(initialized && queried && processor) {
                    const bool instrument=std::strcmp(kind,"instrument")==0;
                    SpeakerArrangement stereo=SpeakerArr::kStereo;
                    ProcessSetup setup{}; setup.processMode=kRealtime; setup.symbolicSampleSize=kSample32;
                    setup.maxSamplesPerBlock=64; setup.sampleRate=48000;
                    bool configured=processor->setBusArrangements(instrument?nullptr:&stereo,instrument?0:1,&stereo,1)==kResultOk &&
                        processor->setupProcessing(setup)==kResultOk;
                    bool buses=configured && component->activateBus(kAudio,kOutput,0,true)==kResultOk &&
                        (instrument || component->activateBus(kAudio,kInput,0,true)==kResultOk);
                    bool active=buses && component->setActive(true)==kResultOk;
                    if(active) {
                        std::array<float,64> left{},right{},out_left{},out_right{};
                        left.fill(0.75f); right.fill(-0.5f);
                        float* inputs[]={left.data(),right.data()}; float* outputs[]={out_left.data(),out_right.data()};
                        AudioBusBuffers input{},output{};
                        input.numChannels=output.numChannels=2; input.channelBuffers32=inputs; output.channelBuffers32=outputs;
                        ProcessData data{}; data.processMode=kRealtime; data.symbolicSampleSize=kSample32; data.numSamples=64;
                        data.numInputs=instrument?0:1; data.inputs=instrument?nullptr:&input;
                        data.numOutputs=1; data.outputs=&output;
                        bool processed=false;
                        // Original fixture only: this is not a realtime deadline test.
                        std::thread worker([&] {
                            bool started=processor->setProcessing(true)==kResultOk;
                            processed=started && processor->process(data)==kResultOk;
                            if(started) processed=(processor->setProcessing(false)==kResultOk) && processed;
                        });
                        worker.join();
                        ok=processed;
                        for(size_t i=0;i<64;++i)
                            ok=ok && out_left[i]==(instrument?0.125f:0.375f) && out_right[i]==(instrument?0.125f:-0.25f);
                        ok=(component->setActive(false)==kResultOk) && ok;
                    }
                }
                if(initialized) {
                    component->activateBus(kAudio,kOutput,0,false);
                    if(std::strcmp(kind,"instrument")) component->activateBus(kAudio,kInput,0,false);
                }
                if(processor) processor->release();
                if(initialized) ok=(component->terminate()==kResultOk) && ok;
                component->release();
            }
        }
        if(factory) factory->release();
        ok=exit() && ok;
    }
    if(loaded) CFBundleUnloadExecutable(bundle);
    CFRelease(bundle);
    return ok;
}

int main(int argc,char** argv) {
    @autoreleasepool {
        if (argc==3 && !std::strcmp(argv[1],"--processing-self-test")) {
            const size_t length=std::strlen(argv[2]);
            if (!length || length>2) return 2;
            int scenario=0;
            for (size_t n=0;n<length;++n) { if (argv[2][n]<'0' || argv[2][n]>'9') return 2; scenario=scenario*10+(argv[2][n]-'0'); }
            if (scenario>31) return 2;
            processing_evidence.scenario=scenario;
            try { const bool valid=fixture("processing",HEX_PROCESSING_SHA256); print_processing(); return valid?0:1; }
            catch (...) { std::fprintf(stderr,"processing fault; module custody retained until process exit\n"); return 1; }
        }
        if (argc==2 && !std::strcmp(argv[1],"--stream-fixture")) {
            hex_stream_fixture::enabled=true; lifecycle_evidence.mode=1; lifecycle_evidence.failure=0;
            try { return fixture("lifecycle",HEX_LIFECYCLE_SHA256)?0:1; }
            catch (...) { std::fprintf(stderr,"stream fault; module custody retained until process exit\n"); return 1; }
        }
        if (argc==3 && !std::strcmp(argv[1],"--host-self-test")) {
            const size_t length=std::strlen(argv[2]);
            if (!length || length>2) return 2;
            int scenario=0;
            for (size_t n=0;n<length;++n) { if (argv[2][n]<'0' || argv[2][n]>'9') return 2; scenario=scenario*10+(argv[2][n]-'0'); }
            if (scenario>15) return 2;
            host_evidence.scenario=scenario; lifecycle_evidence.mode=1; lifecycle_evidence.failure=0;
            try { const bool valid=fixture("lifecycle",HEX_LIFECYCLE_SHA256); print_host(); return valid?0:1; }
            catch (...) { std::fprintf(stderr,"host-control fault; module custody retained until process exit\n"); return 1; }
        }
        if (argc==4 && !std::strcmp(argv[1],"--lifecycle-self-test")) {
            const auto number=[](const char* value,int maximum)->int {
                if (!value || !*value || std::strlen(value)>2) return -1;
                int result=0;
                for (const char* p=value;*p;++p) { if (*p<'0' || *p>'9') return -1; result=result*10+(*p-'0'); }
                return result<=maximum?result:-1;
            };
            lifecycle_evidence.mode=number(argv[2],2);
            lifecycle_evidence.failure=number(argv[3],26);
            if (lifecycle_evidence.mode<0 || lifecycle_evidence.failure<0) return 2;
            try {
                const bool valid=fixture("lifecycle",HEX_LIFECYCLE_SHA256);
                print_lifecycle();
                return valid?0:1;
            } catch (...) { std::fprintf(stderr,"lifecycle fault; module custody retained until process exit\n"); return 1; }
        }
        if(argc!=2) { std::fprintf(stderr,"controlled helper requires --identity or --self-test\n"); return 2; }
        if(!std::strcmp(argv[1],"--identity")) {
            std::puts("{\"version\":1,\"scope\":\"original-macos-bootstrap\",\"platform\":\"darwin-arm64\",\"sdk\":\"" HEX_SDK_PIN "\"}");
            return 0;
        }
        if(std::strcmp(argv[1],"--self-test")) { std::fprintf(stderr,"installed modules and arbitrary paths refused\n"); return 2; }
        try {
            if(!fixture("effect",HEX_EFFECT_SHA256) || !fixture("instrument",HEX_INSTRUMENT_SHA256)) {
                std::fprintf(stderr,"original fixture smoke failed\n"); return 1;
            }
        } catch(...) { std::fprintf(stderr,"bootstrap failed\n"); return 1; }
        std::puts("{\"version\":1,\"scope\":\"original-macos-bootstrap\",\"effect_frames\":64,\"instrument_frames\":64}");
        return 0;
    }
}
