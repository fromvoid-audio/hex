// Original controlled native bootstrap. Installed modules are never accepted.
#import <Foundation/Foundation.h>
#include <CommonCrypto/CommonDigest.h>
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "fixture_identity.hpp"
#include <array>
#include <cstdio>
#include <cstring>
#include <thread>

using namespace Steinberg;
using namespace Steinberg::Vst;

static bool fixture(const char* kind,const char* expected) {
    NSString* name=[NSString stringWithUTF8String:kind];
    NSString* relative=[NSString stringWithFormat:@"fixtures/%@.vst3",name];
    NSString* path=[[[NSBundle mainBundle] resourcePath] stringByAppendingPathComponent:relative];
    NSString* binary=[path stringByAppendingPathComponent:@"Contents/MacOS/fixture"];
    NSDictionary* attrs=[[NSFileManager defaultManager] attributesOfItemAtPath:binary error:nullptr];
    if(!attrs || [attrs fileSize]>16*1024*1024) return false;
    NSData* bytes=[NSData dataWithContentsOfFile:binary];
    if(!bytes || [bytes length]>16*1024*1024) return false;
    unsigned char digest[CC_SHA256_DIGEST_LENGTH];
    CC_SHA256([bytes bytes],static_cast<CC_LONG>([bytes length]),digest);
    char hex[65]={};
    for(size_t i=0;i<sizeof(digest);++i) std::snprintf(hex+2*i,3,"%02x",digest[i]);
    if(std::strcmp(hex,expected)) return false;
    CFURLRef url=CFURLCreateWithFileSystemPath(nullptr,(__bridge CFStringRef)path,kCFURLPOSIXPathStyle,true);
    if(!url) return false;
    CFBundleRef bundle=CFBundleCreate(nullptr,url); CFRelease(url);
    if(!bundle) return false;
    bool loaded=CFBundleLoadExecutable(bundle);
    auto entry=reinterpret_cast<bool(*)(CFBundleRef)>(CFBundleGetFunctionPointerForName(bundle,CFSTR("bundleEntry")));
    auto exit=reinterpret_cast<bool(*)()>(CFBundleGetFunctionPointerForName(bundle,CFSTR("bundleExit")));
    auto get=reinterpret_cast<IPluginFactory*(*)()>(CFBundleGetFunctionPointerForName(bundle,CFSTR("GetPluginFactory")));
    bool entered=loaded && entry && exit && get && entry(bundle);
    bool ok=false;
    if(entered) {
        auto* factory=get();
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
