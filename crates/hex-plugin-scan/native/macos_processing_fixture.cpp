// Original flexible-bus SDK oracle. No third-party DSP or installed module.
#define HEX_LIFECYCLE_FIXTURE 1
#include "macos_fixture.cpp"
#include "pluginterfaces/vst/ivstparameterchanges.h"
#include <cmath>
#include <algorithm>
#include <array>
#include <stdexcept>
#include <limits>
#include <pthread.h>
#include <cstdio>
#include <cstdlib>
#include "pluginterfaces/base/ibstream.h"
#ifndef HEX_INSTANCE_VARIANT
#define HEX_INSTANCE_VARIANT 0
#endif

namespace {
struct Observation {
    int violations=0, created=0, initialized=0, terminated=0;
    int arrangements=0, setups=0, activations=0, starts=0, stops=0, blocks=0, flushes=0;
} observed;
int next_mode=0,next_fault=HEX_INSTANCE_VARIANT;
bool control() { const bool valid=pthread_main_np()!=0; if (!valid) ++observed.violations; return valid; }
const TUID processing_id={0x48,0x45,0x58,0x50,1,2,3,4,5,6,7,8,9,10,11,12};

static void trace(int stage,int factory_refs=-1) {
    const auto* path=std::getenv("HEX_ORIGINAL_INSTANCE_TRACE");
    if (!path || std::strlen(path)>4096) return;
    // Only owned SDK test modules implement this opt-in trace, on helper control
    // before/after processing. Production helper code never reads this variable.
    auto* output=std::fopen(path,"wb");if (!output) return;
    const int values[]={stage,observed.violations,observed.created,observed.initialized,observed.terminated,
        observed.arrangements,observed.setups,observed.activations,observed.starts,observed.stops,
        observed.blocks,observed.flushes,static_cast<int>(objects.load()),factory_refs};
    std::fwrite(values,sizeof(int),14,output);std::fclose(output);
}
class ProcessingProcessor final : public Processor,public IAudioPresentationLatency {
    const int mode=next_mode,fault=next_fault;
    bool initialized=false, arranged=false, setup=false, active=false, processing=false;
    bool input=false,output=false,thread_bound=false;
    int32 inputs=2,outputs=2,maximum=0,block=0,precision=kSample32;
    double sample_rate=0;
    float gain=1;
    uint32 input_latency=0,output_latency=0;
    IBStream* retained_stream=nullptr;
    pthread_t worker{};
    bool same_worker() const { return !thread_bound || pthread_equal(worker,pthread_self()); }
public:
    ProcessingProcessor() { ++observed.created; }
    ~ProcessingProcessor() {
        if (initialized || active || processing) ++observed.violations;
        if (retained_stream && fault!=16) retained_stream->release();
    }
    uint32 PLUGIN_API addRef() override { return Processor::addRef(); }
    uint32 PLUGIN_API release() override { return Processor::release(); }
    tresult PLUGIN_API queryInterface(const TUID iid,void** out) override {
        if (!out || !iid) return kInvalidArgument;
        if (same(iid,INLINE_UID_OF(IAudioPresentationLatency))) {
            *out=static_cast<IAudioPresentationLatency*>(this);addRef();return kResultOk;
        }
        return Processor::queryInterface(iid,out);
    }
    tresult PLUGIN_API setAudioPresentationLatencySamples(BusDirection direction,int32 index,uint32 value) override {
        if (!control() || !active || processing || index || (direction!=kInput && direction!=kOutput) ||
            (direction==kInput && !inputs) || fault==17) return kResultFalse;
        (direction==kInput?input_latency:output_latency)=value;return kResultOk;
    }
    tresult PLUGIN_API getState(IBStream* stream) override {
        if (!control() || !active || processing || !stream) return kResultFalse;
        if (fault==26) { trace(7);throw std::runtime_error("original SDK state exception"); }
        if (fault==15) { std::array<unsigned char,4001> huge{};return stream->write(huge.data(),4001,nullptr); }
        if (fault==23) {
            stream->seek(std::numeric_limits<int64>::min(),IBStream::kIBSeekCur,nullptr);
            stream->seek(std::numeric_limits<int64>::max(),IBStream::kIBSeekSet,nullptr);
            return kResultOk; // rejected stream operations must not masquerade as captured state
        }
        if (fault==24) stream->release(); // original invalid-reference sensitivity control
        if (fault==16) { stream->addRef();retained_stream=stream; }
        std::array<unsigned char,32> bytes{};std::memcpy(bytes.data(),"HXPS",4);
        const uint32 count=static_cast<uint32>(block);std::memcpy(bytes.data()+4,&count,4);
        std::memcpy(bytes.data()+8,&gain,4);std::memcpy(bytes.data()+12,&sample_rate,8);
        bytes[20]=static_cast<unsigned char>(inputs);bytes[21]=static_cast<unsigned char>(outputs);
        std::memcpy(bytes.data()+24,&input_latency,4);std::memcpy(bytes.data()+28,&output_latency,4);
        ISizeableStream* sized=nullptr;
        if (stream->queryInterface(INLINE_UID_OF(ISizeableStream),reinterpret_cast<void**>(&sized))!=kResultOk || !sized)
            return kResultFalse;
        const bool sized_ok=sized->setStreamSize(32)==kResultOk;sized->release();
        int32 written=0;
        const bool ok=sized_ok && stream->seek(4,IBStream::kIBSeekSet,nullptr)==kResultOk &&
            stream->write(bytes.data()+4,28,&written)==kResultOk && written==28 &&
            stream->seek(0,IBStream::kIBSeekSet,nullptr)==kResultOk && stream->write(bytes.data(),4,&written)==kResultOk && written==4;
        trace(4);return ok?kResultOk:kResultFalse;
    }
    tresult PLUGIN_API initialize(FUnknown* host) override {
        if (!control() || initialized || !host) return kResultFalse;
        const auto result=Processor::initialize(host);
        if (result==kResultOk) { initialized=true; ++observed.initialized; }
        return result;
    }
    tresult PLUGIN_API terminate() override {
        if (!control() || !initialized || active || processing) return kResultFalse;
        const auto result=Processor::terminate();
        if (result==kResultOk) { initialized=false; ++observed.terminated;trace(5); }
        return result;
    }
    int32 PLUGIN_API getBusCount(MediaType media,BusDirection direction) override {
        if (!control() || !initialized || media!=kAudio) return 0;
        if (fault==5) return 2;
        return direction==kInput && mode==1?0:1;
    }
    tresult PLUGIN_API getBusInfo(MediaType media,BusDirection direction,int32 index,BusInfo& info) override {
        if (!control() || index || !getBusCount(media,direction)) return kInvalidArgument;
        info={};info.mediaType=kAudio;info.direction=direction;info.busType=kMain;
        info.channelCount=direction==kInput?inputs:outputs;
        if (fault==12 && arranged) ++info.channelCount;
        info.flags=BusInfo::kDefaultActive;
        const char16_t name[]=u"main";std::memcpy(info.name,name,sizeof(name));
        return kResultOk;
    }
    tresult PLUGIN_API getBusArrangement(BusDirection direction,int32 index,SpeakerArrangement& value) override {
        if (!control() || index || !getBusCount(kAudio,direction)) return kInvalidArgument;
        value=(direction==kInput?inputs:outputs)==1?SpeakerArr::kMono:SpeakerArr::kStereo;
        return kResultOk;
    }
    tresult PLUGIN_API canProcessSampleSize(int32 size) override {
        if (!control() || !initialized || fault==6) return kResultFalse;
        return size==kSample32 || (mode==2 && size==kSample64)?kResultOk:kResultFalse;
    }
    tresult PLUGIN_API setBusArrangements(SpeakerArrangement* in,int32 ni,SpeakerArrangement* out,int32 no) override {
        if (!control() || !initialized || active || processing || ni!=(mode==1?0:1) || no!=1 ||
            !out || (ni && !in)) return kInvalidArgument;
        const auto count=[](SpeakerArrangement value)->int32 {
            return value==SpeakerArr::kMono?1:(value==SpeakerArr::kStereo?2:0);
        };
        const int32 requested_input=ni?count(in[0]):0,requested_output=count(out[0]);
        if (!requested_output || (ni && !requested_input)) return kResultFalse;
        ++observed.arrangements;
        inputs=requested_input;outputs=requested_output;arranged=true;
        if (fault==1 || fault==2) { inputs=1;outputs=1; }
        return fault==1?kResultFalse:kResultOk; // real refusal can mutate arrangement
    }
    tresult PLUGIN_API setupProcessing(ProcessSetup& request) override {
        if (!control() || !arranged || active || processing || request.processMode!=kRealtime ||
            (request.symbolicSampleSize!=kSample32 && !(mode==2 && request.symbolicSampleSize==kSample64)) ||
            request.maxSamplesPerBlock<1 || request.maxSamplesPerBlock>256 ||
            (request.sampleRate!=44100 && request.sampleRate!=48000 && request.sampleRate!=96000)) return kResultFalse;
        ++observed.setups;
        sample_rate=request.sampleRate;maximum=request.maxSamplesPerBlock;precision=request.symbolicSampleSize;
        if (fault==3) { sample_rate=12345;return kResultFalse; }
        setup=true;return kResultOk;
    }
    tresult PLUGIN_API activateBus(MediaType media,BusDirection direction,int32 index,TBool enabled) override {
        if (!control() || !setup || active || processing || index || !getBusCount(media,direction)) return kResultFalse;
        ++observed.activations;
        if (enabled && fault==7) return kResultFalse;
        (direction==kInput?input:output)=enabled;return kResultOk;
    }
    tresult PLUGIN_API setActive(TBool enabled) override {
        if (!enabled && fault==25) trace(7); // sensitivity: teardown after unknown startup
        if (!control() || !setup || processing) return kResultFalse;
        active=enabled;return kResultOk;
    }
    tresult PLUGIN_API setProcessing(TBool enabled) override {
        if (!active || (enabled?processing:(!processing || !same_worker()))) return kResultFalse;
        if (pthread_main_np()) { ++observed.violations;return kResultFalse; }
        if (enabled && fault==18) return kResultFalse;
        if (enabled) { worker=pthread_self();thread_bound=true;++observed.starts; }
        else ++observed.stops;
        processing=enabled;
        if (enabled && fault==25) throw std::runtime_error("original SDK startup exception");
        return kResultOk;
    }
    tresult PLUGIN_API process(ProcessData& data) override {
        if (pthread_main_np() || !same_worker() || !active || !std::isfinite(sample_rate) || data.processMode!=kRealtime ||
            data.symbolicSampleSize!=precision) { ++observed.violations;return kResultFalse; }
        if (!data.numSamples && !data.numInputs && !data.numOutputs && !data.inputs && !data.outputs) {
            if (processing || !data.inputParameterChanges || data.inputParameterChanges->getParameterCount()!=1)
                return kInvalidArgument;
            auto* queue=data.inputParameterChanges->getParameterData(0);
            int32 offset=-1;ParamValue value=-1;
            if (!queue || queue->getParameterId()!=0x1234 || queue->getPointCount()!=1 ||
                queue->getPoint(0,offset,value)!=kResultOk || offset || !std::isfinite(value) || value<0 || value>1)
                return kInvalidArgument;
            ++observed.flushes;
            if (fault==11) return kResultFalse;
            gain=static_cast<float>(value);return kResultOk;
        }
        if (!processing || data.numSamples<1 || data.numSamples>maximum || data.inputEvents || data.inputParameterChanges ||
            data.numInputs!=(mode==1 || (!input && data.numInputs==0)?0:1) ||
            data.numOutputs!=(!output && data.numOutputs==0?0:1)) return kInvalidArgument;
        if (data.numInputs && (!data.inputs || data.inputs[0].numChannels!=inputs ||
            (precision==kSample32?!data.inputs[0].channelBuffers32:!data.inputs[0].channelBuffers64)))
            return kInvalidArgument;
        if (data.numOutputs && (!data.outputs || data.outputs[0].numChannels!=outputs ||
            (precision==kSample32?!data.outputs[0].channelBuffers32:!data.outputs[0].channelBuffers64)))
            return kInvalidArgument;
        ++observed.blocks; // count attempted foreign DSP, including a refusal
        if (fault==10) return kResultFalse;
        const float scale=0.5f+static_cast<float>(block)*0.125f;
        bool silent=true;
        for (int32 frame=0;frame<data.numSamples;++frame) {
            // Read all inputs for this sample before writing any aliased output.
            const double left=mode==1?0.25:(input?(precision==kSample32?data.inputs[0].channelBuffers32[0][frame]:data.inputs[0].channelBuffers64[0][frame]):0.0);
            const double right=mode==1?0.25:(input && inputs==2?(precision==kSample32?data.inputs[0].channelBuffers32[1][frame]:data.inputs[0].channelBuffers64[1][frame]):left);
            if (output) for (int32 channel=0;channel<outputs;++channel) {
                const double value=(channel?right:left)*scale*gain;
                if (precision==kSample32) {
                    auto* destination=data.outputs[0].channelBuffers32[channel];if (!destination) return kInvalidArgument;
                    destination[frame]=fault==4?std::numeric_limits<float>::quiet_NaN():static_cast<float>(value);
                } else {
                    auto* destination=data.outputs[0].channelBuffers64[channel];if (!destination) return kInvalidArgument;
                    destination[frame]=fault==4?std::numeric_limits<double>::quiet_NaN():value;
                }
                silent=silent && value==0;
            }
        }
        if (output) data.outputs[0].silenceFlags=silent || fault==9?((uint64(1)<<outputs)-1):0;
        if (fault==8 && output) data.outputs[0].numChannels=3;
        if (fault==13) data.numSamples=8192;
        ++block;return kResultOk;
    }
};

class ProcessingFactory final : public IPluginFactory {
    std::atomic<uint32> refs{0};
public:
    uint32 count() const { return refs.load(); }
    tresult PLUGIN_API queryInterface(const TUID iid,void** out) override {
        if (!out) return kInvalidArgument;*out=nullptr;
        if (!same(iid,INLINE_UID_OF(FUnknown)) && !same(iid,INLINE_UID_OF(IPluginFactory))) return kNoInterface;
        *out=static_cast<IPluginFactory*>(this);addRef();return kResultOk;
    }
    uint32 PLUGIN_API addRef() override { return ++refs; }
    uint32 PLUGIN_API release() override { return --refs; }
    tresult PLUGIN_API getFactoryInfo(PFactoryInfo* out) override {
        if (!out) return kInvalidArgument;
        *out=PFactoryInfo("hex original fixtures","https://invalid.example","fixture@invalid.example",0);return kResultOk;
    }
    int32 PLUGIN_API countClasses() override { return 1; }
    tresult PLUGIN_API getClassInfo(int32 index,PClassInfo* out) override {
        if (index || !out) return kInvalidArgument;
        *out=PClassInfo(processing_id,PClassInfo::kManyInstances,"Audio Module Class","hex original processing fixture");return kResultOk;
    }
    tresult PLUGIN_API createInstance(FIDString cid,FIDString iid,void** out) override {
        if (!out) return kInvalidArgument;*out=nullptr;
        if (!cid || !iid || !same(cid,processing_id)) return kInvalidArgument;
        auto* object=new(std::nothrow) ProcessingProcessor();if (!object) return kOutOfMemory;
        const auto result=object->queryInterface(iid,out);object->release();return result;
    }
} processing_factory;
} // namespace

extern "C" __attribute__((visibility("default"))) bool HexProcessingConfigure(int mode,int fault) {
    if (!control() || mode<0 || mode>2 || fault<0 || fault>13) return false;
    next_mode=mode;next_fault=fault;return true;
}
extern "C" __attribute__((visibility("default"))) int HexProcessingObservation(int* out,int capacity) {
    if (!control() || !out || capacity<13) return -1;
    const int values[]={observed.violations,observed.created,observed.initialized,observed.terminated,
        observed.arrangements,observed.setups,observed.activations,observed.starts,observed.stops,
        observed.blocks,observed.flushes,static_cast<int>(objects.load()),static_cast<int>(processing_factory.count())};
    std::copy(std::begin(values),std::end(values),out);return 13;
}
extern "C" __attribute__((visibility("default"))) bool bundleEntry(CFBundleRef) {
    // Supported fault control belongs only to this original SDK fixture.
    // The production helper never reads this environment variable.
    const auto* selected=std::getenv("HEX_ORIGINAL_INSTANCE_VARIANT");
    if (selected) {
        const auto length=std::strlen(selected);if (!length || length>2) return false;
        int value=0;for (size_t n=0;n<length;++n) { if (selected[n]<'0' || selected[n]>'9') return false;value=value*10+selected[n]-'0'; }
        if (value>26) return false;next_fault=value;
    }
    trace(1,0);
    if (next_fault==21) for (int n=0;n<4096;++n) { std::fputs("owned SDK stdout noise\n",stdout);std::fputs("owned SDK stderr noise\n",stderr); }
    if (next_fault==22 && std::fgetc(stdin)!=EOF) return false;
    return next_fault!=19;
}
extern "C" __attribute__((visibility("default"))) bool bundleExit() {
    trace(6,static_cast<int>(processing_factory.count()));
    return next_fault!=20 && objects.load()==0 && processing_factory.count()==0;
}
extern "C" __attribute__((visibility("default"))) IPluginFactory* GetPluginFactory() {
    processing_factory.addRef();return &processing_factory;
}
