// ORIGINAL controlled VST3 effect, not installed/third-party plugin support.
#include "offline_wire.hpp"
#include <cstdio>
#include <csignal>
#include <unistd.h>
#include <limits>
using namespace Controlled;
#ifndef FIXTURE_MODE
#define FIXTURE_MODE 0
#endif
static std::atomic<uint32> live {
  0
};
#define PLUGIN_REFS std::atomic<uint32> refs{1};uint32 PLUGIN_API addRef()override{return ++refs;}uint32 PLUGIN_API release()override{uint32 n=--refs;if(!n)delete this;return n;}
class Component final:public IComponent,public IAudioProcessor,public IConnectionPoint,public IProcessContextRequirements  {
  IHostApplication* host=nullptr;
  IConnectionPoint* peer=nullptr;
  bool initialized=false,setup=false,active=false,processing=false;
  bool input=false,output=false;
  double gain=.5;
  uint64_t frames=0;
  int32 maximum=0;
  double rate=0;
  public:
  PLUGIN_REFS
  Component() {
    ++live;
  }~Component() {
    --live;
    std::fprintf(stderr,"component destroyed\n");
  }
  tresult PLUGIN_API queryInterface(const TUID iid,void** o)override {
    *o=nullptr;
    if(equal(iid,INLINE_UID_OF(FUnknown))||equal(iid,INLINE_UID_OF(IComponent))||equal(iid,INLINE_UID_OF(IPluginBase)))*o=static_cast<IComponent*>(this);
    else if(equal(iid,INLINE_UID_OF(IAudioProcessor)))*o=static_cast<IAudioProcessor*>(this);
    else if(equal(iid,INLINE_UID_OF(IConnectionPoint)))*o=static_cast<IConnectionPoint*>(this);
    else if(equal(iid,INLINE_UID_OF(IProcessContextRequirements)))*o=static_cast<IProcessContextRequirements*>(this);
    else return kNoInterface;
    addRef();
    return kResultOk;
  }
  tresult PLUGIN_API initialize(FUnknown* context)override {
    if(initialized||FIXTURE_MODE==6)return kResultFalse;
    void* h=nullptr;
    if(context->queryInterface(INLINE_UID_OF(IHostApplication),&h)!=kResultOk)return kResultFalse;
    host=static_cast<IHostApplication*>(h);
    initialized=true;
    std::fprintf(stderr,"component initialized\n");
    return kResultOk;
  }
  tresult PLUGIN_API terminate()override {
    if(!initialized||active||processing||peer)return kResultFalse;
    if(host) {
      host->release();
      host=nullptr;
    }initialized=false;
    std::fprintf(stderr,"component terminated\n");
    return kResultOk;
  }
  tresult PLUGIN_API getControllerClassId(TUID out)override {
    std::memcpy(out,controller_id,16);
    return kResultOk;
  }
  tresult PLUGIN_API setIoMode(IoMode mode)override {
    return !initialized&&mode==kOfflineProcessing?kResultOk:kResultFalse;
  }
  int32 PLUGIN_API getBusCount(MediaType type,BusDirection)override {
    return initialized&&type==kAudio?1:0;
  }
  tresult PLUGIN_API getBusInfo(MediaType type,BusDirection dir,int32 index,BusInfo& b)override {
    if(!initialized||type!=kAudio||index)return kInvalidArgument;
    b= {};
    b.mediaType=kAudio;
    b.direction=dir;
    b.channelCount=2;
    b.busType=kMain;
    b.flags=BusInfo::kDefaultActive;
    utf16(b.name,"main");
    return kResultOk;
  }
  tresult PLUGIN_API getRoutingInfo(RoutingInfo&,RoutingInfo&)override {
    return kNotImplemented;
  }
  tresult PLUGIN_API activateBus(MediaType type,BusDirection dir,int32 index,TBool state)override {
    if(!setup||type!=kAudio||index)return kResultFalse;
    (dir==kInput?input:output)=state;
    return kResultOk;
  }
  tresult PLUGIN_API setActive(TBool state)override {
    if(!setup||processing||(state&&(!input||!output)))return kResultFalse;
    active=state;
    return kResultOk;
  }
  tresult PLUGIN_API setState(IBStream* s)override {
    if(!initialized||active||FIXTURE_MODE==9)return kResultFalse;
    std::array<uint8_t,28>b {};
    int32 n=0;
    if(s->read(b.data(),int32(b.size()),&n)!=kResultOk||n!=int32(b.size()))return kResultFalse;
    std::vector<uint8_t> data(b.begin(),b.end());
    Reader r {
      data
    };
    if(r.u(4)!=0x314e4748||r.u(8)!=1)return kResultFalse;
    double v=r.f64();
    auto f=r.u(8);
    uint8_t extra;int32 remaining=0;
    if(!norm(v)||f>uint64(INT64_MAX)||s->read(&extra,1,&remaining)!=kResultOk||remaining!=0)return kResultFalse;
    gain=v;
    frames=f;
    return kResultOk;
  }
  tresult PLUGIN_API getState(IBStream* s)override {
    if(!initialized||active)return kResultFalse;
    Writer w;
    w.u(0x314e4748,4);
    w.u(1,8);
    w.f64(gain);
    w.u(frames,8);
    int32 n=0;
    return s->write(w.b.data(),int32(w.b.size()),&n)==kResultOk&&n==int32(w.b.size())?kResultOk:kResultFalse;
  }
  tresult PLUGIN_API connect(IConnectionPoint* other)override {
    if(peer||!initialized)return kResultFalse;
    peer=other;
    peer->addRef();
    return kResultOk;
  }
  tresult PLUGIN_API disconnect(IConnectionPoint* other)override {
    if(peer!=other)return kResultFalse;
    peer->release();
    peer=nullptr;
    return kResultOk;
  }
  tresult PLUGIN_API notify(IMessage* message)override {
    if(!message||std::strcmp(message->getMessageID(),"fixture-connected")!=0)return kResultFalse;
    int64 attribute=0;
    return message->getAttributes()->getInt("fixture",attribute)==kResultOk&&attribute==7?kResultOk:kResultFalse;
  }
  tresult PLUGIN_API setBusArrangements(SpeakerArrangement* ins,int32 ni,SpeakerArrangement* outs,int32 no)override {
    return initialized&&!active&&FIXTURE_MODE!=10&&ni==1&&no==1&&ins[0]==SpeakerArr::kStereo&&outs[0]==SpeakerArr::kStereo?kResultTrue:kResultFalse;
  }
  tresult PLUGIN_API getBusArrangement(BusDirection,int32 i,SpeakerArrangement& out)override {
    if(i)return kResultFalse;
    out=SpeakerArr::kStereo;
    return kResultOk;
  }
  tresult PLUGIN_API canProcessSampleSize(int32 size)override {
    return size==kSample32?kResultTrue:kResultFalse;
  }
  uint32 PLUGIN_API getLatencySamples()override {
    return 0;
  }uint32 PLUGIN_API getTailSamples()override {
    return 0;
  }uint32 PLUGIN_API getProcessContextRequirements()override {
    return kNeedContinousTimeSamples|kNeedTransportState;
  }
  tresult PLUGIN_API setupProcessing(ProcessSetup& s)override {
    if(!initialized||active||s.processMode!=kOffline||s.symbolicSampleSize!=kSample32||s.maxSamplesPerBlock<1||s.maxSamplesPerBlock>1024||(s.sampleRate!=44100&&s.sampleRate!=48000))return kResultFalse;
    maximum=s.maxSamplesPerBlock;
    rate=s.sampleRate;
    setup=true;
    return kResultOk;
  }
  tresult PLUGIN_API setProcessing(TBool state)override {
    if(!active)return kResultFalse;
    processing=state;
    return kResultOk;
  }
  tresult PLUGIN_API process(ProcessData& d)override {
    if(FIXTURE_MODE==1) {
      for(;;)pause();
    }if(FIXTURE_MODE==2) {
      std::raise(SIGSEGV);
      return kResultFalse;
    }if(FIXTURE_MODE==4&&frames>0)return kResultFalse;
    if(FIXTURE_MODE==5) {
      auto pid=fork();
      if(pid==0) {
        signal(SIGTERM,SIG_IGN);
        for(;;)pause();
      }std::fprintf(stderr,"fixture descendant=%ld\n",long(pid));
    }
    if(!processing||d.processMode!=kOffline||d.symbolicSampleSize!=kSample32||d.numSamples<1||d.numSamples>maximum||d.numInputs!=1||d.numOutputs!=1||d.inputs[0].numChannels!=2||d.outputs[0].numChannels!=2||!d.processContext||d.processContext->sampleRate!=rate||d.processContext->projectTimeSamples!=int64(frames))return kResultFalse;
    auto q=d.inputParameterChanges?d.inputParameterChanges->getParameterData(0):nullptr;
    int32 count=q?q->getPointCount():0;
    int32 previous=-1;
    double value=gain;
    int32 next=0;
    double target=gain;
    int32 qi=0;
    if(count&&q->getPoint(0,next,target)!=kResultOk)return kResultFalse;
    for(int32 i=0;i<d.numSamples;i++) {
      while(qi<count&&i>next) {
        previous=next;
        value=target;
        ++qi;
        if(qi<count&&q->getPoint(qi,next,target)!=kResultOk)return kResultFalse;
      }
      double v=qi<count?value+(target-value)*double(i-previous)/double(next-previous):value;
      for(int c=0;c<2;c++)d.outputs[0].channelBuffers32[c][i]=FIXTURE_MODE==3?std::numeric_limits<float>::infinity():float(double(d.inputs[0].channelBuffers32[c][i])*(2*v));
      gain=v;
    }
    frames+=uint64(d.numSamples);
    return kResultOk;
  }
};
class Controller final:public IEditController,public IConnectionPoint  {
  IHostApplication* host=nullptr;
  IComponentHandler* handler=nullptr;
  IConnectionPoint* peer=nullptr;
  bool initialized=false;
  double display=.5;
  uint32 preference=7;
  public:
  PLUGIN_REFS
  Controller() {
    ++live;
  }~Controller() {
    --live;
    std::fprintf(stderr,"controller destroyed\n");
  }
  tresult PLUGIN_API queryInterface(const TUID iid,void** o)override {
    *o=nullptr;
    if(equal(iid,INLINE_UID_OF(FUnknown))||equal(iid,INLINE_UID_OF(IEditController))||equal(iid,INLINE_UID_OF(IPluginBase)))*o=static_cast<IEditController*>(this);
    else if(equal(iid,INLINE_UID_OF(IConnectionPoint)))*o=static_cast<IConnectionPoint*>(this);
    else return kNoInterface;
    addRef();
    return kResultOk;
  }
  tresult PLUGIN_API initialize(FUnknown* context)override {
    if(initialized||FIXTURE_MODE==7)return kResultFalse;
    void* h=nullptr;
    if(context->queryInterface(INLINE_UID_OF(IHostApplication),&h)!=kResultOk)return kResultFalse;
    host=static_cast<IHostApplication*>(h);
    initialized=true;
    std::fprintf(stderr,"controller initialized\n");
    return kResultOk;
  }
  tresult PLUGIN_API terminate()override {
    if(!initialized||handler||peer)return kResultFalse;
    host->release();
    host=nullptr;
    initialized=false;
    std::fprintf(stderr,"controller terminated\n");
    return kResultOk;
  }
  tresult PLUGIN_API setComponentState(IBStream* s)override {
    uint8_t bytes[28];
    int32 n=0;
    if(s->read(bytes,28,&n)!=kResultOk||n!=28)return kResultFalse;
    std::vector<uint8_t> b(bytes,bytes+28);
    Reader r {
      b
    };
    if(r.u(4)!=0x314e4748||r.u(8)!=1)return kResultFalse;
    double v=r.f64();
    if(!norm(v))return kResultFalse;
    display=v;
    return kResultOk;
  }
  tresult PLUGIN_API setState(IBStream* s)override {
    uint8_t bytes[8];
    int32 n=0;
    if(s->read(bytes,8,&n)!=kResultOk||n!=8)return kResultFalse;
    std::vector<uint8_t>b(bytes,bytes+8);
    Reader r {
      b
    };
    if(r.u(4)!=0x31435448)return kResultFalse;
    auto v=r.u(4);
    uint8_t extra;int32 remaining=0;
    if(v>31||s->read(&extra,1,&remaining)!=kResultOk||remaining!=0)return kResultFalse;
    preference=uint32(v);
    return kResultOk;
  }
  tresult PLUGIN_API getState(IBStream* s)override {
    Writer w;
    w.u(0x31435448,4);
    w.u(preference,4);
    int32 n=0;
    return s->write(w.b.data(),8,&n)==kResultOk&&n==8?kResultOk:kResultFalse;
  }
  int32 PLUGIN_API getParameterCount()override {
    return 1;
  }
  tresult PLUGIN_API getParameterInfo(int32 i,ParameterInfo& p)override {
    if(i)return kInvalidArgument;
    p= {};
    p.id=gain_id;
    p.flags=ParameterInfo::kCanAutomate;
    p.defaultNormalizedValue=.5;
    p.stepCount=0;
    p.unitId=0;
    utf16(p.title,"gain");
    utf16(p.shortTitle,"gain");
    utf16(p.units,"linear");
    return kResultOk;
  }
  tresult PLUGIN_API getParamStringByValue(ParamID id,double v,String128 out)override {
    if(id!=gain_id||!norm(v))return kInvalidArgument;
    auto s=std::to_string(v*2);
    utf16(out,s.c_str());
    return kResultOk;
  }
  tresult PLUGIN_API getParamValueByString(ParamID,TChar*,double&)override {
    return kNotImplemented;
  }
  double PLUGIN_API normalizedParamToPlain(ParamID id,double v)override {
    return id==gain_id?v*2:0;
  }
  double PLUGIN_API plainParamToNormalized(ParamID id,double v)override {
    return id==gain_id?v/2:0;
  }
  double PLUGIN_API getParamNormalized(ParamID id)override {
    return id==gain_id?display:0;
  }
  tresult PLUGIN_API setParamNormalized(ParamID id,double v)override {
    if(id!=gain_id||!norm(v))return kInvalidArgument;
    display=v;
    return kResultOk;
  }
  tresult PLUGIN_API setComponentHandler(IComponentHandler* h)override {
    if(handler)handler->release();
    handler=h;
    if(h) {
      h->addRef();
      if(FIXTURE_MODE==11)h->restartComponent(kLatencyChanged);
      else {
        h->beginEdit(gain_id);
        h->performEdit(gain_id,.25);
        h->endEdit(gain_id);
      }
    }return kResultOk;
  }
  IPlugView* PLUGIN_API createView(FIDString)override {
    return nullptr;
  }
  tresult PLUGIN_API connect(IConnectionPoint* other)override {
    if(peer||!initialized||FIXTURE_MODE==8)return kResultFalse;
    peer=other;
    peer->addRef();
    TUID iid;
    std::memcpy(iid,INLINE_UID_OF(IMessage),16);
    void* out=nullptr;
    if(host->createInstance(iid,iid,&out)!=kResultOk)return kResultFalse;
    auto m=static_cast<IMessage*>(out);
    m->setMessageID("fixture-connected");
    m->getAttributes()->setInt("fixture",7);
    auto status=peer->notify(m);
    m->release();
    return status;
  }
  tresult PLUGIN_API disconnect(IConnectionPoint* other)override {
    if(peer!=other)return kResultFalse;
    peer->release();
    peer=nullptr;
    return kResultOk;
  }
  tresult PLUGIN_API notify(IMessage*)override {
    return kNotImplemented;
  }
};
class Factory final:public IPluginFactory  {
  public:
  std::atomic<uint32> refs {
    0
  };
  uint32 PLUGIN_API addRef()override {
    return ++refs;
  }uint32 PLUGIN_API release()override {
    return --refs;
  }
  tresult PLUGIN_API queryInterface(const TUID iid,void** o)override {
    *o=nullptr;
    if(equal(iid,INLINE_UID_OF(FUnknown))||equal(iid,INLINE_UID_OF(IPluginFactory))) {
      *o=this;
      addRef();
      return kResultOk;
    }return kNoInterface;
  }
  tresult PLUGIN_API getFactoryInfo(PFactoryInfo* p)override {
    *p=PFactoryInfo("hex original offline fixture","https://invalid.example","fixture@invalid.example",0);
    return kResultOk;
  }
  int32 PLUGIN_API countClasses()override {
    return 2;
  }
  tresult PLUGIN_API getClassInfo(int32 i,PClassInfo* p)override {
    if(i<0||i>1)return kInvalidArgument;
    *p=PClassInfo(i?controller_id:component_id,PClassInfo::kManyInstances,i?"Component Controller Class":"Audio Module Class",i?"controlled gain controller":"controlled gain processor");
    return kResultOk;
  }
  tresult PLUGIN_API createInstance(FIDString cid,FIDString iid,void** p)override {
    *p=nullptr;
    if(equal(cid,component_id)&&equal(iid,INLINE_UID_OF(IComponent))) {
      *p=static_cast<IComponent*>(new Component);
      return kResultOk;
    }if(equal(cid,controller_id)&&equal(iid,INLINE_UID_OF(IEditController))) {
      *p=static_cast<IEditController*>(new Controller);
      return kResultOk;
    }return kNoInterface;
  }
};
static Factory factory;
extern "C" __attribute__((visibility("default"))) bool ModuleEntry(void*) {
  std::puts("offline plugin diagnostic");
  return true;
}
extern "C" __attribute__((visibility("default"))) IPluginFactory* GetPluginFactory() {
  factory.addRef();
  return &factory;
}
extern "C" __attribute__((visibility("default"))) bool ModuleExit() {
  if(FIXTURE_MODE==12) {
    for(;;)pause();
  }std::fprintf(stderr,"offline module exit objects=%u factory=%u\n",live.load(),factory.refs.load());
  return live.load()==0&&factory.refs.load()==0;
}
