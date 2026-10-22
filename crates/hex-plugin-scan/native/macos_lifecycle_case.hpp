// Bounded copied evidence for the original fixture only; not product IPC/admission.
#pragma once
#include "macos_instance.hpp"
#include <array>
#include <algorithm>
#include <cstdio>

struct LifecycleEvidence {
    int mode=0, failure=0, observed[135]{}, observed_length=0;
    bool opened=false, configured=false, started=false, processed=false, stopped=false;
    bool closed=false, pcm=true, stable_close=false, unloaded=false;
} lifecycle_evidence;

static bool lifecycle_case(CFBundleRef bundle, Steinberg::IPluginFactory* factory) {
    using namespace Steinberg;
    using namespace Steinberg::Vst;
    auto configure=reinterpret_cast<bool(*)(int,int)>(CFBundleGetFunctionPointerForName(bundle,CFSTR("HexLifecycleCase")));
    auto observe=reinterpret_cast<int(*)(int*,int)>(CFBundleGetFunctionPointerForName(bundle,CFSTR("HexLifecycleObservation")));
    // The loader already owns one factory reference. Fixture configuration must
    // precede GetPluginFactory, so its case has been set by the caller.
    if (!configure || !observe || !factory) return false;
    hex_instance::Instance instance;
    PClassInfo info{};
    if (factory->getClassInfo(0,&info)!=kResultOk) return false;
    auto& result=lifecycle_evidence;
    result.opened=instance.open(factory,info.cid);
    if (result.opened) result.configured=instance.configure_stereo(48000,64,false);
    if (result.opened && !result.configured)
        result.pcm=!instance.configure_stereo(48000,64,false); // no replay after partial preparation
    if (result.configured) {
        std::array<float,64> left{},right{},out_left{},out_right{};
        left.fill(0.75f); right.fill(-0.5f);
        float* inputs[]={left.data(),right.data()}; float* outputs[]={out_left.data(),out_right.data()};
        AudioBusBuffers input{},output{};
        input.numChannels=output.numChannels=2; input.channelBuffers32=inputs; output.channelBuffers32=outputs;
        ProcessData data{}; data.processMode=kRealtime; data.symbolicSampleSize=kSample32;
        data.numSamples=64; data.numInputs=1; data.inputs=&input; data.numOutputs=1; data.outputs=&output;
        // Wrong-thread operations must refuse before reaching the foreign object.
        result.pcm=!instance.start_processing() && !instance.process(data);
        std::thread worker([&] {
            result.started=instance.start_processing();
            if (!result.started) result.pcm=result.pcm && !instance.start_processing();
            result.processed=result.started;
            for (int block=0;block<2 && result.processed;++block) {
                result.processed=instance.process(data);
                if (result.processed) for (size_t n=0;n<64;++n) {
                    const float scale=block==0?1.0f:0.5f;
                    result.pcm=result.pcm && out_left[n]==0.375f*scale && out_right[n]==-0.25f*scale;
                }
            }
            if (result.started && !result.processed)
                result.pcm=result.pcm && !instance.process(data); // rejected before foreign replay
            if (result.started) result.stopped=instance.stop_processing();
        });
        worker.join();
    }
    result.closed=instance.close();
    result.observed_length=observe(result.observed,135);
    const int previous_length=result.observed_length;
    std::array<int,135> previous{};
    std::copy(std::begin(result.observed),std::end(result.observed),previous.begin());
    const bool again=instance.close();
    const int after=observe(result.observed,135);
    result.stable_close=again==result.closed && after==previous_length &&
        std::equal(previous.begin(),previous.end(),std::begin(result.observed));
    return result.observed_length>=7 && result.pcm && result.stable_close && result.observed[0]==0;
}

static void print_lifecycle() {
    const auto& r=lifecycle_evidence;
    std::printf("{\"version\":1,\"scope\":\"original-macos-instance-lifecycle\",\"mode\":%d,\"failure\":%d,"
                "\"opened\":%s,\"configured\":%s,\"started\":%s,\"processed\":%s,\"stopped\":%s,"
                "\"closed\":%s,\"pcm\":%s,\"stable_close\":%s,\"unloaded\":%s,\"observation\":[",
        r.mode,r.failure,r.opened?"true":"false",r.configured?"true":"false",r.started?"true":"false",
        r.processed?"true":"false",r.stopped?"true":"false",r.closed?"true":"false",r.pcm?"true":"false",
        r.stable_close?"true":"false",r.unloaded?"true":"false");
    for (int i=0;i<r.observed_length && i<135;++i) std::printf("%s%d",i?",":"",r.observed[i]);
    std::puts("]}");
}
