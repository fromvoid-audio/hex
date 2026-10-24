// Original Hex SDK bootstrap fixtures. No third-party implementation or assets.
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include <CoreFoundation/CoreFoundation.h>
#include <atomic>
#include <cstring>
#include <new>

using namespace Steinberg;
using namespace Steinberg::Vst;
#ifndef HEX_INSTRUMENT
#define HEX_INSTRUMENT 0
#endif

static std::atomic<uint32> objects{0};
static const TUID class_id = {0x48,0x45,0x58,0x33,0x01,0x02,0x03,0x04,
                             0x05,0x06,0x07,0x08,0x09,0x10,0x11,HEX_INSTRUMENT};
static bool same(const TUID a, const TUID b) { return std::memcmp(a,b,16)==0; }

class Processor : public IComponent, public IAudioProcessor {
    std::atomic<uint32> refs{1};
    bool initialized=false, configured=false, active=false, processing=false;
    bool output=false, input=false;
    int32 maximum=0;
public:
    Processor() { ++objects; }
    virtual ~Processor() { --objects; }
    uint32 PLUGIN_API addRef() override { return ++refs; }
    uint32 PLUGIN_API release() override {
        auto n=--refs; if(!n) delete this; return n;
    }
    tresult PLUGIN_API queryInterface(const TUID iid, void** out) override {
        if(!out) return kInvalidArgument;
        *out=nullptr;
        if(same(iid,INLINE_UID_OF(FUnknown)) || same(iid,INLINE_UID_OF(IPluginBase)) ||
           same(iid,INLINE_UID_OF(IComponent))) *out=static_cast<IComponent*>(this);
        else if(same(iid,INLINE_UID_OF(IAudioProcessor))) *out=static_cast<IAudioProcessor*>(this);
        else return kNoInterface;
        addRef(); return kResultOk;
    }
    tresult PLUGIN_API initialize(FUnknown*) override {
        if(initialized) return kResultFalse;
        initialized=true; return kResultOk;
    }
    tresult PLUGIN_API terminate() override {
        if(!initialized || active || processing) return kResultFalse;
        initialized=false; return kResultOk;
    }
    tresult PLUGIN_API getControllerClassId(TUID) override { return kResultFalse; }
    tresult PLUGIN_API setIoMode(IoMode) override { return initialized?kResultFalse:kResultOk; }
    int32 PLUGIN_API getBusCount(MediaType media, BusDirection dir) override {
        return initialized && media==kAudio && (dir==kOutput || (dir==kInput && !HEX_INSTRUMENT))?1:0;
    }
    tresult PLUGIN_API getBusInfo(MediaType media, BusDirection dir, int32 index, BusInfo& b) override {
        if(index || !getBusCount(media,dir)) return kInvalidArgument;
        b={}; b.mediaType=media; b.direction=dir; b.channelCount=2;
        b.busType=kMain; b.flags=BusInfo::kDefaultActive;
        const char16_t name[]=u"main"; std::memcpy(b.name,name,sizeof(name));
        return kResultOk;
    }
    tresult PLUGIN_API getRoutingInfo(RoutingInfo&,RoutingInfo&) override { return kNotImplemented; }
    tresult PLUGIN_API activateBus(MediaType media,BusDirection dir,int32 index,TBool value) override {
        if(!configured || index || !getBusCount(media,dir)) return kInvalidArgument;
        (dir==kOutput?output:input)=value; return kResultOk;
    }
    tresult PLUGIN_API setActive(TBool value) override {
        if(!configured || processing || (value && (!output || (!HEX_INSTRUMENT && !input)))) return kResultFalse;
        active=value; return kResultOk;
    }
    tresult PLUGIN_API setState(IBStream*) override { return kNotImplemented; }
    tresult PLUGIN_API getState(IBStream*) override { return kNotImplemented; }
    tresult PLUGIN_API setBusArrangements(SpeakerArrangement* ins,int32 ni,SpeakerArrangement* outs,int32 no) override {
        if(!initialized || active || ni!=(HEX_INSTRUMENT?0:1) || no!=1 || !outs || outs[0]!=SpeakerArr::kStereo ||
           (!HEX_INSTRUMENT && (!ins || ins[0]!=SpeakerArr::kStereo))) return kResultFalse;
        return kResultOk;
    }
    tresult PLUGIN_API getBusArrangement(BusDirection dir,int32 index,SpeakerArrangement& out) override {
        if(index || !getBusCount(kAudio,dir)) return kInvalidArgument;
        out=SpeakerArr::kStereo; return kResultOk;
    }
    tresult PLUGIN_API canProcessSampleSize(int32 size) override { return size==kSample32?kResultOk:kResultFalse; }
    uint32 PLUGIN_API getLatencySamples() override { return 0; }
    tresult PLUGIN_API setupProcessing(ProcessSetup& s) override {
        if(!initialized || active || s.processMode!=kRealtime || s.symbolicSampleSize!=kSample32 ||
           s.sampleRate!=48000 || s.maxSamplesPerBlock<1 || s.maxSamplesPerBlock>64) return kResultFalse;
        maximum=s.maxSamplesPerBlock; configured=true; return kResultOk;
    }
    tresult PLUGIN_API setProcessing(TBool value) override {
        if(!active) return kResultFalse;
        processing=value; return kResultOk;
    }
    tresult PLUGIN_API process(ProcessData& d) override {
        if(!processing || d.processMode!=kRealtime || d.symbolicSampleSize!=kSample32 ||
           d.numSamples<1 || d.numSamples>maximum || d.numOutputs!=1 || !d.outputs ||
           d.outputs[0].numChannels!=2 || !d.outputs[0].channelBuffers32 ||
           d.numInputs!=(HEX_INSTRUMENT?0:1) || d.inputEvents || d.inputParameterChanges) return kInvalidArgument;
        if(!HEX_INSTRUMENT && (!d.inputs || d.inputs[0].numChannels!=2 || !d.inputs[0].channelBuffers32)) return kInvalidArgument;
        for(int c=0;c<2;++c) {
            auto* out=d.outputs[0].channelBuffers32[c];
            if(!out || (!HEX_INSTRUMENT && !d.inputs[0].channelBuffers32[c])) return kInvalidArgument;
            for(int32 n=0;n<d.numSamples;++n)
                out[n]=HEX_INSTRUMENT?0.125f:d.inputs[0].channelBuffers32[c][n]*0.5f;
        }
        d.outputs[0].silenceFlags=0; return kResultOk;
    }
    uint32 PLUGIN_API getTailSamples() override { return kNoTail; }
};

// The lifecycle fixture reuses the original DSP, with independent lifecycle oracles.
#ifndef HEX_LIFECYCLE_FIXTURE
class Factory final : public IPluginFactory {
    std::atomic<uint32> refs{0};
public:
    uint32 PLUGIN_API addRef() override { return ++refs; }
    uint32 PLUGIN_API release() override { return --refs; }
    uint32 count() const { return refs.load(); }
    tresult PLUGIN_API queryInterface(const TUID iid,void** out) override {
        if(!out) return kInvalidArgument;
        *out=nullptr;
        if(!same(iid,INLINE_UID_OF(FUnknown)) && !same(iid,INLINE_UID_OF(IPluginFactory))) return kNoInterface;
        *out=this; addRef(); return kResultOk;
    }
    tresult PLUGIN_API getFactoryInfo(PFactoryInfo* out) override {
        if(!out) return kInvalidArgument;
        *out=PFactoryInfo("hex original fixtures","https://invalid.example","fixture@invalid.example",0);
        return kResultOk;
    }
    int32 PLUGIN_API countClasses() override { return 1; }
    tresult PLUGIN_API getClassInfo(int32 index,PClassInfo* out) override {
        if(index || !out) return kInvalidArgument;
        *out=PClassInfo(class_id,PClassInfo::kManyInstances,"Audio Module Class",
                       HEX_INSTRUMENT?"hex original constant instrument fixture":"hex original gain effect fixture");
        return kResultOk;
    }
    tresult PLUGIN_API createInstance(FIDString cid,FIDString iid,void** out) override {
        if(!out) return kInvalidArgument;
        *out=nullptr;
        if(!cid || !iid || !same(cid,class_id)) return kInvalidArgument;
        auto* p=new(std::nothrow) Processor();
        if(!p) return kOutOfMemory;
        auto result=p->queryInterface(iid,out); p->release(); return result;
    }
};
static Factory factory;
extern "C" __attribute__((visibility("default"))) bool bundleEntry(CFBundleRef) { return true; }
extern "C" __attribute__((visibility("default"))) bool bundleExit() { return objects.load()==0 && factory.count()==0; }
extern "C" __attribute__((visibility("default"))) IPluginFactory* GetPluginFactory() { factory.addRef(); return &factory; }

#endif // HEX_LIFECYCLE_FIXTURE
