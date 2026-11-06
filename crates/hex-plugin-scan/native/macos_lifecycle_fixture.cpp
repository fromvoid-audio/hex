// Original hostile-order fixture: actual SDK objects, not a host-state mock.
#define HEX_LIFECYCLE_FIXTURE 1
#include "macos_fixture.cpp"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivsthostapplication.h"
#include "pluginterfaces/vst/ivstmessage.h"
#include <pthread.h>
#include "pluginterfaces/vst/ivstattributes.h"
#include <array>
#include <cmath>
#include <cstdio>
#include <limits>
#include <thread>

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
Node* active_controller=nullptr;
bool host_testing=false, process_callback=false;
int metadata_fault=0, received_messages=0, processing_callback_result=kNotImplemented, processing_message_result=kNotImplemented;
ParamValue metadata_value=0.25;
IMessage* retained_message=nullptr;
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
        if (controller) { ++observation.controllers; active_controller=this; }
        else { ++observation.created; if (observation.mode==2) active_controller=this; }
    }
    ~Node() {
        if (active_component==this) active_component=nullptr;
        if (active_controller==this) active_controller=nullptr;
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
        if (host_testing && process_callback && active_controller) {
            processing_callback_result=active_controller->send_restart(kParamValuesChanged);
            IHostApplication* app=nullptr;
            if (active_controller->host_context()->queryInterface(INLINE_UID_OF(IHostApplication),reinterpret_cast<void**>(&app))==kResultOk && app) {
                TUID id{}; std::memcpy(id,INLINE_UID_OF(IMessage),sizeof(TUID));
                IMessage* message=nullptr;
                processing_message_result=app->createInstance(id,id,reinterpret_cast<void**>(&message));
                if (message) { ++observation.violations; message->release(); }
                app->release();
            }
        }
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
    tresult PLUGIN_API notify(IMessage* message) override {
        if (!host_testing) return kNotImplemented;
        if (!pthread_main_np() || !initialized || !connected || !message) return kResultFalse;
        auto* attributes=message->getAttributes();
        int64 integer=0; double floating=0; String128 text{}; const void* binary=nullptr; uint32 bytes=0;
        const char16_t expected[]=u"Mixed-Case";
        const unsigned char expected_binary[]={0,1,255};
        const bool valid=message->getMessageID() && std::strcmp(message->getMessageID(),"OriginalCase")==0 && attributes &&
            attributes->getInt("integer",integer)==kResultOk && integer==7 &&
            attributes->getFloat("floating",floating)==kResultOk && floating==0.25 &&
            attributes->getString("text",text,sizeof(text))==kResultOk && std::memcmp(text,expected,sizeof(expected))==0 &&
            attributes->getBinary("binary",binary,bytes)==kResultOk && bytes==3 && binary && std::memcmp(binary,expected_binary,3)==0;
        if (!valid) return kResultFalse;
        ++received_messages; return kResultOk;
    }
    tresult send_restart(int32 flags) { return handler?handler->restartComponent(flags):kNotImplemented; }
    tresult send_edit(int kind,ParamID id,ParamValue value) {
        if(!handler) return kNotImplemented;
        if(kind==1) return handler->beginEdit(id);
        if(kind==2) {metadata_value=value;return handler->performEdit(id,value);}
        if(kind==3) return handler->endEdit(id);
        return kInvalidArgument;
    }
    FUnknown* host_context() const { return context; }
    IConnectionPoint* connected_peer() const { return peer; }
    tresult PLUGIN_API setComponentHandler(IComponentHandler* supplied) override {
        if (!event(supplied?Handler:ClearHandler,initialized && (supplied?!handler:handler!=nullptr))) return kResultFalse;
        if (supplied) supplied->addRef();
        if (handler) handler->release();
        handler=supplied; return kResultOk;
    }
    tresult PLUGIN_API setComponentState(IBStream*) override { return kNotImplemented; }
    tresult PLUGIN_API setState(IBStream*) override { return kNotImplemented; }
    tresult PLUGIN_API getState(IBStream*) override { return kNotImplemented; }
    int32 PLUGIN_API getParameterCount() override {
        if (!host_testing) return 0;
        if (!pthread_main_np() || !initialized) { ++observation.violations; return -1; }
        return metadata_fault==5?129:2;
    }
    tresult PLUGIN_API getParameterInfo(int32 index,ParameterInfo& info) override {
        if (!host_testing || !pthread_main_np() || !initialized || index<0 || index>1) return kInvalidArgument;
        info={}; info.id=index?11:7; info.defaultNormalizedValue=0.5;
        const char16_t title[]=u"Original Parameter"; std::memcpy(info.title,title,sizeof(title));
        const char16_t short_title[]=u"Original"; std::memcpy(info.shortTitle,short_title,sizeof(short_title));
        const char16_t units[]=u"units"; std::memcpy(info.units,units,sizeof(units));
        if (metadata_fault==1 && index) info.id=7;
        if (metadata_fault==3) for (auto& c:info.title) c=u'x';
        if (metadata_fault==4) info.stepCount=-1;
        if (metadata_fault==6) send_restart(kParamValuesChanged);
        if (metadata_fault==7) info.id=index?12:7;
        return kResultOk;
    }
    tresult PLUGIN_API getParamStringByValue(ParamID,ParamValue,String128) override { return kInvalidArgument; }
    tresult PLUGIN_API getParamValueByString(ParamID,TChar*,ParamValue&) override { return kInvalidArgument; }
    ParamValue PLUGIN_API normalizedParamToPlain(ParamID,ParamValue value) override { return value; }
    ParamValue PLUGIN_API plainParamToNormalized(ParamID,ParamValue value) override { return value; }
    ParamValue PLUGIN_API getParamNormalized(ParamID id) override {
        if (!host_testing) return 0;
        if (!pthread_main_np() || !initialized || (id!=7 && id!=11 && id!=12)) { ++observation.violations; return 0; }
        return metadata_fault==2?std::numeric_limits<double>::quiet_NaN():metadata_value;
    }
    tresult PLUGIN_API setParamNormalized(ParamID,ParamValue) override { return kNotImplemented; }
    IPlugView* PLUGIN_API createView(FIDString) override {
        if (host_testing) {
            if (!pthread_main_np() || !initialized) ++observation.violations;
            else send_restart(kParamTitlesChanged|kParamValuesChanged);
        }
        return nullptr;
    }
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

// Real controller/host ABI actions, private to the original native fixture.
extern "C" __attribute__((visibility("default"))) bool HexHostConfigure() {
    if (objects.load() || factory.count()!=1 || retained_message) return false;
    host_testing=true; process_callback=false; metadata_fault=0; metadata_value=0.25;
    received_messages=0; processing_callback_result=processing_message_result=kNotImplemented; return true;
}
extern "C" __attribute__((visibility("default"))) int HexHostAction(int action,int detail) {
    if (!host_testing || !active_controller) return kNotImplemented;
    if (action==0) return active_controller->send_restart(detail);
    if (action==1) { metadata_value=0.75; return kResultOk; }
    if (action==2) { metadata_fault=detail; return kResultOk; }
    if (action==3) { process_callback=true; return kResultOk; }
    if (action==4) return processing_callback_result;
    if (action==5) { active_controller->createView("editor"); return kResultOk; }
    if (action==6) return received_messages;
    if (action==7) return processing_message_result;
    return kInvalidArgument;
}
extern "C" __attribute__((visibility("default"))) int HexHostEdit(int kind,uint32 id,double value) {
    return host_testing && active_controller?active_controller->send_edit(kind,id,value):kNotImplemented;
}
extern "C" __attribute__((visibility("default"))) int HexHostMessages(int retain) {
    if (!host_testing || !active_controller || (!pthread_main_np() && retain!=2)) return kInvalidArgument;
    IHostApplication* host=nullptr;
    if (active_controller->host_context()->queryInterface(INLINE_UID_OF(IHostApplication),reinterpret_cast<void**>(&host))!=kResultOk || !host) return kResultFalse;
    IMessage* message=nullptr; TUID message_id{}; std::memcpy(message_id,INLINE_UID_OF(IMessage),sizeof(TUID));
    bool ok=host->createInstance(message_id,message_id,reinterpret_cast<void**>(&message))==kResultOk && message;
    if (retain==2) {
        if (ok) {
            message->setMessageID("Background");
            auto* attributes=message->getAttributes();
            int64 value=0;
            ok=attributes && attributes->setInt("sequence",42)==kResultOk &&
                attributes->getInt("sequence",value)==kResultOk && value==42 &&
                message->getMessageID() && !std::strcmp(message->getMessageID(),"Background");
            message->release();
        }
        host->release(); return ok?kResultOk:kResultFalse;
    }
    if (ok && retain) { retained_message=message; host->release(); return kResultOk; }
    if (ok) {
        message->setMessageID("OriginalCase"); auto* attributes=message->getAttributes();
        const char16_t text[]=u"Mixed-Case"; const unsigned char data[]={0,1,255};
        ok=attributes && attributes->setInt("integer",7)==kResultOk && attributes->setFloat("floating",0.25)==kResultOk &&
            attributes->setString("text",text)==kResultOk && attributes->setBinary("binary",data,3)==kResultOk;
        double before=0; int64 unchanged=99; String128 small{}; small[0]=u'!';
        std::array<TChar,258> oversized{}; oversized.fill(u'x'); oversized.back()=0;
        ok=ok && attributes->setFloat("floating",std::numeric_limits<double>::infinity())!=kResultOk &&
            attributes->getFloat("floating",before)==kResultOk && before==0.25 &&
            attributes->getInt("floating",unchanged)!=kResultOk && unchanged==99 &&
            attributes->getString("text",small,2)!=kResultOk && small[0]==u'!' &&
            attributes->setString("text",oversized.data())!=kResultOk &&
            attributes->setBinary("binary",data,1025)!=kResultOk;
        const void* borrowed=nullptr; uint32 borrowed_size=0;
        ok=ok && attributes->getBinary("missing",borrowed,borrowed_size)==kResultFalse;
        ok=attributes->getBinary("binary",borrowed,borrowed_size)==kResultOk && borrowed_size==3 && ok;
        ok=attributes->setBinary("binary",borrowed,borrowed_size)==kResultOk && ok;
        message->setMessageID(message->getMessageID());
        for (int n=0;n<12 && ok;++n) { char key[8]{}; std::snprintf(key,sizeof(key),"key%d",n); ok=attributes->setInt(key,n)==kResultOk; }
        ok=ok && attributes->setInt("overflow",1)!=kResultOk && active_controller->connected_peer() &&
            active_controller->connected_peer()->notify(message)==kResultOk;
        bool wrong_thread_refused=false;
        std::thread wrong_thread([&] {
            const void* value=nullptr; uint32 size=0;
            wrong_thread_refused=!message->getMessageID() && !message->getAttributes() &&
                attributes->getBinary("binary",value,size)==kResultFalse;
        });
        wrong_thread.join();
        ok=ok && wrong_thread_refused;
        message->release();
    }
    std::array<IMessage*,16> live{};
    for (auto& item:live) ok=(host->createInstance(message_id,message_id,reinterpret_cast<void**>(&item))==kResultOk && item) && ok;
    IMessage* excess=nullptr;
    ok=host->createInstance(message_id,message_id,reinterpret_cast<void**>(&excess))!=kResultOk && !excess && ok;
    // Reference release from a worker returns pool slots, never frees Host storage.
    std::thread release([&] { for (auto* item:live) if (item) item->release(); }); release.join();
    IMessage* reused=nullptr;
    ok=(host->createInstance(message_id,message_id,reinterpret_cast<void**>(&reused))==kResultOk && reused) && ok;
    if (reused) { int64 stale=0; ok=!reused->getMessageID() && reused->getAttributes()->getInt("integer",stale)!=kResultOk && ok; reused->release(); }
    IAttributeList* list=nullptr; TUID attribute_id{}; std::memcpy(attribute_id,INLINE_UID_OF(IAttributeList),sizeof(TUID));
    ok=(host->createInstance(attribute_id,attribute_id,reinterpret_cast<void**>(&list))==kResultOk && list) && ok;
    if (list) { int64 value=0; ok=list->setInt("only",42)==kResultOk && list->getInt("only",value)==kResultOk && value==42 && ok; list->release(); }
    void* unknown=reinterpret_cast<void*>(1);
    ok=host->createInstance(message_id,attribute_id,&unknown)==kNoInterface && !unknown && ok;
    unknown=reinterpret_cast<void*>(1);
    ok=host->createInstance(nullptr,message_id,&unknown)!=kResultOk && !unknown && ok;
    host->release(); return ok?kResultOk:kResultFalse;
}
