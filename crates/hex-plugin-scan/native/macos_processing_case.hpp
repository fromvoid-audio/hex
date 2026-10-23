// Original SDK processing evidence, never a product admission/physical test.
#pragma once
#include "macos_instance.hpp"
#include <array>
#include <condition_variable>
#include <mutex>

struct ProcessingEvidence {
    int scenario=-1,configuration=-1,flush=-1,observed[13]{};
    bool pcm=true,descriptors=true,active_preserved=true,closed=false,unloaded=false;
} processing_evidence;

class ProcessingParameter final : public Steinberg::Vst::IParamValueQueue,public Steinberg::Vst::IParameterChanges {
    double value;
public:
    explicit ProcessingParameter(double requested):value(requested) {}
    Steinberg::tresult PLUGIN_API queryInterface(const Steinberg::TUID,void** out) override {
        if (out) *out=nullptr;return Steinberg::kNoInterface;
    }
    Steinberg::uint32 PLUGIN_API addRef() override { return 1; }
    Steinberg::uint32 PLUGIN_API release() override { return 1; }
    Steinberg::Vst::ParamID PLUGIN_API getParameterId() override { return 0x1234; }
    Steinberg::int32 PLUGIN_API getPointCount() override { return 1; }
    Steinberg::tresult PLUGIN_API getPoint(Steinberg::int32 index,Steinberg::int32& offset,Steinberg::Vst::ParamValue& out) override {
        if (index) return Steinberg::kInvalidArgument;offset=0;out=value;return Steinberg::kResultOk;
    }
    Steinberg::tresult PLUGIN_API addPoint(Steinberg::int32,Steinberg::Vst::ParamValue,Steinberg::int32&) override {
        return Steinberg::kNotImplemented;
    }
    Steinberg::int32 PLUGIN_API getParameterCount() override { return 1; }
    Steinberg::Vst::IParamValueQueue* PLUGIN_API getParameterData(Steinberg::int32 index) override { return index?nullptr:this; }
    Steinberg::Vst::IParamValueQueue* PLUGIN_API addParameterData(const Steinberg::Vst::ParamID&,Steinberg::int32&) override { return nullptr; }
};

static bool processing_case(CFBundleRef bundle,Steinberg::IPluginFactory* factory) {
    using namespace Steinberg;
    using namespace Steinberg::Vst;
    using namespace hex_instance;
    auto configure=reinterpret_cast<bool(*)(int,int)>(CFBundleGetFunctionPointerForName(bundle,CFSTR("HexProcessingConfigure")));
    auto observe=reinterpret_cast<int(*)(int*,int)>(CFBundleGetFunctionPointerForName(bundle,CFSTR("HexProcessingObservation")));
    if (!configure || !observe || !factory) return false;
    auto& e=processing_evidence;
    const int scenario=e.scenario;
    const int mode=scenario==3?1:(scenario==11?2:0);
    int fault=0;
    switch (scenario) {
        case 15:fault=2;break;case 16:fault=5;break;case 17:fault=6;break;case 18:fault=7;break;
        case 19:fault=4;break;case 20:fault=8;break;case 21:fault=9;break;case 22:fault=10;break;case 26:fault=13;break;case 30:fault=11;break;case 31:fault=12;break;
        default:break;
    }
    if (!configure(mode,fault)) return false;
    PClassInfo info{};if (factory->getClassInfo(0,&info)!=kResultOk) return false;
    Instance instance;
    ProcessingConfiguration setup{};
    setup.input_channels=scenario==0 || scenario==2?1:(scenario==3?0:2);
    setup.input_enabled=scenario!=3 && scenario!=8 && scenario!=23;
    setup.output_channels=scenario==0 || scenario==3?1:2;
    setup.output_enabled=scenario!=9 && scenario!=23;
    if (scenario==24) setup.sample_rate=44100;
    if (scenario==25) setup.sample_rate=96000;
    if (scenario==27) setup.maximum_frames=8;
    if (scenario==28) setup.maximum_frames=256;
    if (!instance.open(factory,info.cid)) { e.closed=instance.close();return false; }
    if (scenario==11) {
        auto unsupported=setup;unsupported.precision=kSample64;
        e.descriptors=instance.configure(unsupported)==ConfigurationResult::UnsupportedPrecision;
    }
    if (scenario==29) {
        for (int n=0;n<7;++n) {
            auto invalid=setup;
            switch (n) {
                case 0:invalid.sample_rate=std::numeric_limits<double>::quiet_NaN();break;
                case 1:invalid.sample_rate=0;break;case 2:invalid.maximum_frames=0;break;
                case 3:invalid.maximum_frames=8193;break;case 4:invalid.input_channels=3;break;
                case 5:invalid.output_channels=0;break;case 6:invalid.input_channels=0;break;
            }
            e.descriptors=instance.configure(invalid)==ConfigurationResult::InvalidConfiguration && e.descriptors;
        }
    }
    const auto result=instance.configure(setup);e.configuration=static_cast<int>(result);
    const bool expected_preparation_failure=(scenario>=15 && scenario<=18) || scenario==31;
    if (expected_preparation_failure || result!=ConfigurationResult::Prepared) {
        e.pcm=expected_preparation_failure && result!=ConfigurationResult::Prepared;
        e.closed=instance.close();
        const bool observed_ok=observe(e.observed,13)==13 && e.observed[0]==0;
        return e.pcm && e.closed && observed_ok;
    }
    std::mutex mutex;std::condition_variable changed;bool reached=false,resume=false;
    bool started=false,stopped=false,candidate_closed=true;
    std::thread worker([&] {
        std::array<float,257> left{},right{},out_left{},out_right{};
        float* in[]={left.data(),right.data()};float* out[]={out_left.data(),out_right.data()};
        AudioBusBuffers input{},output{};input.numChannels=setup.input_channels;output.numChannels=setup.output_channels;
        input.channelBuffers32=in;output.channelBuffers32=out;
        ProcessData data{};data.processMode=kRealtime;data.symbolicSampleSize=kSample32;
        data.numInputs=setup.input_channels?1:0;data.inputs=setup.input_channels?&input:nullptr;
        data.numOutputs=1;data.outputs=&output;
        ProcessingParameter quarter(0.25),half(0.5);
        ProcessData flush{};flush.processMode=kRealtime;
        if (scenario==30) {
            flush.inputParameterChanges=&quarter;
            e.flush=static_cast<int>(instance.flush_stopped(flush));
            e.pcm=e.flush==static_cast<int>(FlushResult::Refused) && !instance.start_processing() &&
                instance.flush_stopped(flush)==FlushResult::InvalidState;
            stopped=true;return;
        }
        if (scenario==10) {
            flush.numSamples=1;
            e.descriptors=instance.flush_stopped(flush)==FlushResult::InvalidData && e.descriptors;
            flush.numSamples=0;
            e.descriptors=instance.flush_stopped(flush)==FlushResult::Empty && e.descriptors;
            flush.inputParameterChanges=&quarter;
            e.flush=static_cast<int>(instance.flush_stopped(flush));
            e.descriptors=e.descriptors && e.flush==static_cast<int>(FlushResult::Applied);
        }
        started=instance.start_processing();e.pcm=started;
        if (scenario==10) e.descriptors=e.descriptors && instance.flush_stopped(flush)==FlushResult::InvalidState;
        for (int block=0;block<3 && e.pcm;++block) {
            left.fill(0.75f);right.fill(-0.5f);out_left.fill(1234);out_right.fill(1234);
            in[0]=left.data();in[1]=right.data();out[0]=out_left.data();out[1]=out_right.data();
            input.silenceFlags=0;output.silenceFlags=0;
            if (scenario==4) { out[0]=in[0];out[1]=in[1]; }
            if (scenario==5) { out[0]=in[1];out[1]=in[0]; }
            if (scenario==6) { in[1]=in[0];out[0]=out[1]=in[0]; }
            if (scenario==7) { left.fill(0);right.fill(0);input.silenceFlags=(uint64_t(1)<<setup.input_channels)-1; }
            if (scenario==8) in[0]=in[1]=nullptr;
            if (scenario==9) out[0]=out[1]=nullptr;
            data.numSamples=block==0?1:(block==1?7:setup.maximum_frames);
            if (scenario==23) { data.numInputs=0;data.inputs=nullptr;data.numOutputs=0;data.outputs=nullptr; }
            if (scenario==12 && !block) {
                auto bad=data;bad.numSamples=0;e.descriptors=!instance.process(bad) && e.descriptors;
                bad=data;bad.numSamples=65;e.descriptors=!instance.process(bad) && e.descriptors;
                bad=data;bad.symbolicSampleSize=kSample64;e.descriptors=!instance.process(bad) && e.descriptors;
                auto saved=in[0];in[0]=nullptr;input.silenceFlags=1;
                e.descriptors=!instance.process(data) && e.descriptors;in[0]=saved;input.silenceFlags=0;
                left[0]=std::numeric_limits<float>::quiet_NaN();e.descriptors=!instance.process(data) && e.descriptors;left[0]=0.75f;
                input.silenceFlags=1;e.descriptors=!instance.process(data) && e.descriptors;input.silenceFlags=0;
                data.numSamples=8;out[0]=in[0]+1;e.descriptors=!instance.process(data) && e.descriptors;
                out[0]=out_left.data();data.numSamples=1;
            }
            const bool processed=instance.process(data);
            if ((scenario>=19 && scenario<=22) || scenario==26) {
                e.pcm=!processed && !instance.process(data);break; // no foreign replay after failure
            }
            e.pcm=processed;
            const float scale=block==0?0.5f:(block==1?0.625f:0.75f);
            const float gain=scenario==10?0.25f:1.0f;
            for (int channel=0;channel<setup.output_channels && setup.output_enabled;++channel) {
                const float source=scenario==3?0.25f:((scenario==7 || scenario==8)?0.0f:
                    (channel && setup.input_channels==2 && scenario!=6?-0.5f:0.75f));
                for (int n=0;n<data.numSamples;++n) e.pcm=e.pcm && out[channel][n]==source*scale*gain;
                const float tail=scenario==4 || scenario==5 || scenario==6?(scenario==6 || (scenario==4?channel==0:channel==1)?0.75f:-0.5f):1234;
                e.pcm=e.pcm && out[channel][data.numSamples]==tail;
            }
            if (!block && (scenario==13 || scenario==14)) {
                std::unique_lock<std::mutex> lock(mutex);reached=true;changed.notify_all();changed.wait(lock,[&]{return resume;});
            }
        }
        if ((scenario==13 || scenario==14) && !reached) {
            std::unique_lock<std::mutex> lock(mutex);reached=true;changed.notify_all();changed.wait(lock,[&]{return resume;});
        }
        if (started) stopped=instance.stop_processing();
        if (scenario==10 && stopped) {
            flush.inputParameterChanges=&half;
            e.descriptors=instance.flush_stopped(flush)==FlushResult::Applied && e.descriptors;
            flush.inputParameterChanges=nullptr;
            e.descriptors=instance.flush_stopped(flush)==FlushResult::Empty && e.descriptors;
            // The second stopped flush changes actual subsequent DSP, rather than
            // merely returning an acknowledgement while the processor is stopped.
            left.fill(0.75f);right.fill(-0.5f);out_left.fill(1234);out_right.fill(1234);
            data.numSamples=7;output.silenceFlags=0;
            const bool restarted=instance.start_processing();
            e.pcm=restarted && instance.process(data) && e.pcm;
            for (int n=0;n<7;++n) e.pcm=e.pcm && out_left[n]==0.75f*0.875f*0.5f && out_right[n]==-0.5f*0.875f*0.5f;
            stopped=restarted && instance.stop_processing() && stopped;
        }
    });
    if (scenario==13 || scenario==14) {
        { std::unique_lock<std::mutex> lock(mutex);changed.wait(lock,[&]{return reached;}); }
        auto changed_setup=setup;changed_setup.sample_rate=96000;changed_setup.maximum_frames=32;changed_setup.input_channels=1;
        e.active_preserved=instance.configure(changed_setup)==ConfigurationResult::InvalidState;
        const auto* live=instance.configuration();
        e.active_preserved=e.active_preserved && live && live->sample_rate==setup.sample_rate &&
            live->maximum_frames==setup.maximum_frames && live->input_channels==setup.input_channels;
        Instance candidate;
        const int failure=scenario==13?1:3;
        e.active_preserved=configure(0,failure) && candidate.open(factory,info.cid) && e.active_preserved;
        const auto rejected=candidate.configure(changed_setup);
        e.active_preserved=e.active_preserved && rejected==(scenario==13?ConfigurationResult::ArrangementRefused:ConfigurationResult::SetupRefused);
        candidate_closed=candidate.close();e.active_preserved=candidate_closed && e.active_preserved;
        { std::lock_guard<std::mutex> lock(mutex);resume=true;changed.notify_all(); }
    }
    worker.join();e.closed=instance.close() && candidate_closed;
    const bool observed_ok=observe(e.observed,13)==13 && e.observed[0]==0;
    return e.pcm && e.descriptors && e.active_preserved && stopped && e.closed && observed_ok;
}

static void print_processing() {
    const auto& e=processing_evidence;
    std::printf("{\"version\":1,\"scope\":\"original-macos-processing\",\"scenario\":%d,\"configuration\":%d,\"flush\":%d,\"pcm\":%s,\"descriptors\":%s,\"active_preserved\":%s,\"closed\":%s,\"unloaded\":%s,\"observation\":[",
        e.scenario,e.configuration,e.flush,e.pcm?"true":"false",e.descriptors?"true":"false",
        e.active_preserved?"true":"false",e.closed?"true":"false",e.unloaded?"true":"false");
    for (int n=0;n<13;++n) std::printf("%s%d",n?",":"",e.observed[n]);
    std::puts("]}");
}
