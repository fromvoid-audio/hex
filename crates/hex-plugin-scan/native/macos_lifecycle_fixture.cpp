// Original hostile-order fixture: actual SDK objects, not a host-state mock.
#define HEX_LIFECYCLE_FIXTURE 1
#include "macos_fixture.cpp"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivsthostapplication.h"
#include "pluginterfaces/vst/ivstmessage.h"
#include <pthread.h>

namespace {
enum Step { Create=1, Initialize, ProcessorQuery, ControllerCreate, ControllerInitialize,
    Handler, ComponentConnect, ControllerConnect, Arrangement, Setup, OutputOn, InputOn,
    Activate, Start, Process, Stop, Deactivate, InputOff, OutputOff, ControllerDisconnect,
    ComponentDisconnect, ClearHandler, ControllerTerminate, Terminate };
struct Observation {
    int mode=0, failure=0, violations=0, created=0, controllers=0, blocks=0;
    int length=0, trace[128]{};
} observation;
const TUID controller_id = {0x48,0x45,0x58,0x43,1,2,3,4,5,6,7,8,9,10,11,12};
bool event(int step, bool valid, bool audio=false) {
    if (observation.length < 128) observation.trace[observation.length++]=step;
    else ++observation.violations;
    if (!valid || (pthread_main_np()!=0)==audio) ++observation.violations;
    return valid && observation.failure!=step;
}

class Node;
Node* active_component=nullptr;
class Node final : public Processor, public IEditController, public IConnectionPoint {
    std::atomic<uint32> references{1};
    bool controller_only, initialized=false, arranged=false, setup=false;
    bool input=false, output=false, active=false, processing=false, connected=false;
    IComponentHandler* handler=nullptr;
    IConnectionPoint* peer=nullptr;
    FUnknown* context=nullptr;
    pthread_t audio_thread{};
public:
    explicit Node(bool controller) : controller_only(controller) {
        if (controller) ++observation.controllers; else ++observation.created;
    }
    ~Node() {
        if (active_component==this) active_component=nullptr;
        if (initialized || handler || context || connected || active || processing)
            ++observation.violations;
    }
    IEditController* alias_controller() { addRef(); return static_cast<IEditController*>(this); }
    uint32 PLUGIN_API addRef() override { return ++references; }
    uint32 PLUGIN_API release() override {
        const auto count=--references; if (!count) delete this; return count;
    }
    tresult PLUGIN_API queryInterface(const TUID iid,void** out) override {
        if (!out) return kInvalidArgument;
        *out=nullptr;
        if (same(iid,INLINE_UID_OF(FUnknown)) || same(iid,INLINE_UID_OF(IPluginBase)))
            *out=controller_only?static_cast<FUnknown*>(static_cast<IEditController*>(this)):
                                 static_cast<FUnknown*>(static_cast<IComponent*>(this));
        else if (!controller_only && same(iid,INLINE_UID_OF(IComponent)))
            *out=static_cast<IComponent*>(this);
        else if (!controller_only && same(iid,INLINE_UID_OF(IAudioProcessor))) {
            if (!event(ProcessorQuery,initialized)) return kResultFalse;
            *out=static_cast<IAudioProcessor*>(this);
        } else if ((controller_only || observation.mode==2) && same(iid,INLINE_UID_OF(IEditController)))
            *out=static_cast<IEditController*>(this);
        else if (observation.mode!=0 && same(iid,INLINE_UID_OF(IConnectionPoint)))
            *out=static_cast<IConnectionPoint*>(this);
        else return kNoInterface;
        addRef(); return kResultOk;
    }
    tresult PLUGIN_API initialize(FUnknown* supplied) override {
        const int step=controller_only?ControllerInitialize:Initialize;
        if (!event(step,!initialized && supplied)) return kResultFalse;
        IHostApplication* application=nullptr;
        if (supplied->queryInterface(INLINE_UID_OF(IHostApplication),reinterpret_cast<void**>(&application))!=kResultOk || !application)
            return kResultFalse;
        String128 name{};
        const bool named=application->getName(name)==kResultOk && name[0]==u'h' && name[1]==u'e' && name[2]==u'x' && name[3]==0;
        application->release();
        if (!named) { ++observation.violations; return kResultFalse; }
        if (!controller_only && Processor::initialize(supplied)!=kResultOk) return kResultFalse;
        supplied->addRef(); context=supplied; initialized=true; return kResultOk;
    }
    tresult PLUGIN_API terminate() override {
        if (!event(controller_only?ControllerTerminate:Terminate,
                   initialized && !active && !processing && !connected && !handler && !input && !output)) return kResultFalse;
        if (!controller_only && Processor::terminate()!=kResultOk) return kResultFalse;
        context->release(); context=nullptr; initialized=false; return kResultOk;
    }
    tresult PLUGIN_API getControllerClassId(TUID out) override {
        if (observation.mode!=1) return kResultFalse;
        std::memcpy(out,controller_id,sizeof(TUID)); return kResultOk;
    }
    tresult PLUGIN_API setBusArrangements(SpeakerArrangement* a,int32 n,SpeakerArrangement* b,int32 m) override {
        if (!event(Arrangement,initialized && !active)) return kResultFalse;
        const auto result=Processor::setBusArrangements(a,n,b,m);
        arranged=result==kResultOk; return result;
    }
    tresult PLUGIN_API setupProcessing(ProcessSetup& value) override {
        if (!event(Setup,arranged && !active)) return kResultFalse;
        const auto result=Processor::setupProcessing(value);
        setup=result==kResultOk; return result;
    }
    tresult PLUGIN_API activateBus(MediaType media,BusDirection dir,int32 index,TBool value) override {
        const int step=dir==kInput?(value?InputOn:InputOff):(value?OutputOn:OutputOff);
        if (!event(step,setup && !active)) return kResultFalse;
        const auto result=Processor::activateBus(media,dir,index,value);
        if (result==kResultOk) (dir==kInput?input:output)=value;
        return result;
    }
    tresult PLUGIN_API setActive(TBool value) override {
        if (!event(value?Activate:Deactivate,setup && !processing && (value?(input&&output&&!active):active))) return kResultFalse;
        const auto result=Processor::setActive(value);
        if (result==kResultOk) active=value;
        return result;
    }
    tresult PLUGIN_API setProcessing(TBool value) override {
        const bool valid=active && (value?!processing:(processing&&pthread_equal(audio_thread,pthread_self())));
        if (!event(value?Start:Stop,valid,true)) return kResultFalse;
        const auto result=Processor::setProcessing(value);
        if (result==kResultOk) { processing=value; if (value) audio_thread=pthread_self(); }
        return result;
    }
    tresult PLUGIN_API process(ProcessData& data) override {
        if (!event(Process,processing && pthread_equal(audio_thread,pthread_self()),true)) return kResultFalse;
        const auto result=Processor::process(data);
        if (result==kResultOk) {
            const float persistence=observation.blocks==0?1.0f:0.5f;
            for (int c=0;c<2;++c) for (int n=0;n<data.numSamples;++n)
                data.outputs[0].channelBuffers32[c][n]*=persistence;
            ++observation.blocks;
        }
        return result;
    }
    tresult PLUGIN_API connect(IConnectionPoint* other) override {
        if (!event(controller_only?ControllerConnect:ComponentConnect,initialized && !connected && other && other!=static_cast<IConnectionPoint*>(this))) return kResultFalse;
        other->addRef(); peer=other; connected=true; return kResultOk;
    }
    tresult PLUGIN_API disconnect(IConnectionPoint* other) override {
        if (!event(controller_only?ControllerDisconnect:ComponentDisconnect,initialized && connected && peer==other)) return kResultFalse;
        peer->release(); peer=nullptr; connected=false; return kResultOk;
    }
    tresult PLUGIN_API notify(IMessage*) override { return kNotImplemented; }
    tresult PLUGIN_API setComponentHandler(IComponentHandler* supplied) override {
        if (!event(supplied?Handler:ClearHandler,initialized && (supplied?!handler:handler!=nullptr))) return kResultFalse;
        if (supplied) supplied->addRef();
        if (handler) handler->release();
        handler=supplied; return kResultOk;
    }
    tresult PLUGIN_API setComponentState(IBStream*) override { return kNotImplemented; }
    tresult PLUGIN_API setState(IBStream*) override { return kNotImplemented; }
    tresult PLUGIN_API getState(IBStream*) override { return kNotImplemented; }
    int32 PLUGIN_API getParameterCount() override { return 0; }
    tresult PLUGIN_API getParameterInfo(int32,ParameterInfo&) override { return kInvalidArgument; }
    tresult PLUGIN_API getParamStringByValue(ParamID,ParamValue,String128) override { return kInvalidArgument; }
    tresult PLUGIN_API getParamValueByString(ParamID,TChar*,ParamValue&) override { return kInvalidArgument; }
    ParamValue PLUGIN_API normalizedParamToPlain(ParamID,ParamValue value) override { return value; }
    ParamValue PLUGIN_API plainParamToNormalized(ParamID,ParamValue value) override { return value; }
    ParamValue PLUGIN_API getParamNormalized(ParamID) override { return 0; }
    tresult PLUGIN_API setParamNormalized(ParamID,ParamValue) override { return kNotImplemented; }
    IPlugView* PLUGIN_API createView(FIDString) override { return nullptr; }
};

class LifecycleFactory final : public IPluginFactory {
    std::atomic<uint32> references{0};
public:
    uint32 PLUGIN_API addRef() override { return ++references; }
    uint32 PLUGIN_API release() override { return --references; }
    uint32 count() const { return references.load(); }
    tresult PLUGIN_API queryInterface(const TUID iid,void** out) override {
        if (!out) return kInvalidArgument; *out=nullptr;
        if (!same(iid,INLINE_UID_OF(FUnknown)) && !same(iid,INLINE_UID_OF(IPluginFactory))) return kNoInterface;
        *out=this; addRef(); return kResultOk;
    }
    tresult PLUGIN_API getFactoryInfo(PFactoryInfo* out) override {
        if (!out) return kInvalidArgument;
        *out=PFactoryInfo("hex original lifecycle","","",0); return kResultOk;
    }
    int32 PLUGIN_API countClasses() override { return observation.mode==1?2:1; }
    tresult PLUGIN_API getClassInfo(int32 index,PClassInfo* out) override {
        if (!out || index<0 || index>=countClasses()) return kInvalidArgument;
        *out=PClassInfo(index?controller_id:class_id,PClassInfo::kManyInstances,
                        index?"Component Controller Class":"Audio Module Class","hex original lifecycle");
        return kResultOk;
    }
    tresult PLUGIN_API createInstance(FIDString cid,FIDString iid,void** out) override {
        if (!out) return kInvalidArgument; *out=nullptr;
        const bool controller=cid && same(cid,controller_id);
        if (!cid || !iid || (!controller && !same(cid,class_id)) || (controller && observation.mode!=1)) return kInvalidArgument;
        if (!event(controller?ControllerCreate:Create,objects.load()==(controller?1u:0u))) return kResultFalse;
        if (controller && observation.failure==25 && active_component) {
            *out=active_component->alias_controller(); return kResultOk;
        }
        auto* object=new(std::nothrow) Node(controller);
        if (!object) return kOutOfMemory;
        if (!controller) active_component=object;
        const auto result=object->queryInterface(iid,out); object->release(); return result;
    }
} factory;
} // namespace

extern "C" __attribute__((visibility("default"))) bool bundleEntry(CFBundleRef) { return true; }
extern "C" __attribute__((visibility("default"))) bool bundleExit() { return observation.failure!=26 && objects.load()==0 && factory.count()==0 && observation.violations==0; }
extern "C" __attribute__((visibility("default"))) IPluginFactory* GetPluginFactory() { factory.addRef(); return &factory; }
extern "C" __attribute__((visibility("default"))) bool HexLifecycleCase(int mode,int failure) {
    if (objects.load() || factory.count() || mode<0 || mode>2 || failure<0 || failure>26) return false;
    observation={}; observation.mode=mode; observation.failure=failure; return true;
}
// Copy bounded observations only. This private original-fixture export is not a host ABI.
extern "C" __attribute__((visibility("default"))) int HexLifecycleObservation(int* out,int capacity) {
    const int needed=7+observation.length;
    if (!out || capacity<needed) return -1;
    out[0]=observation.violations; out[1]=observation.created; out[2]=observation.controllers;
    out[3]=observation.blocks; out[4]=static_cast<int>(objects.load()); out[5]=static_cast<int>(factory.count()); out[6]=observation.length;
    std::memcpy(out+7,observation.trace,sizeof(int)*observation.length); return needed;
}
