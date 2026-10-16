// ORIGINAL one-request non-realtime controlled VST3 processing helper.
#include "offline_host.hpp"
#include <dlfcn.h>
#include <unistd.h>
#include <sys/resource.h>
#include <cstdio>
#include <cstdlib>
#include <thread>
#include <exception>
using namespace Controlled;
static void read_all(void* p,size_t n) {
  auto b=static_cast<uint8_t*>(p);
  while(n) {
    auto k=read(0,b,n);
    check(k>0,"request read failed");
    b+=k;
    n-=size_t(k);
  }
}
static void write_all(const std::vector<uint8_t>& b) {
  size_t at=0;
  while(at<b.size()) {
    auto n=write(0,b.data()+at,b.size()-at);
    check(n>0,"response write failed");
    at+=size_t(n);
  }
}
struct Block  {
  uint32 frames;
  std::vector<Point> points;
  std::vector<float> left,right;
};
struct Request  {
  uint64 request,start;
  uint32 rate;
  double controller;
  std::vector<uint8_t> component_state,controller_state;
  std::vector<Block> blocks;
  uint32 frames=0;
};
static Request read_request(uint64 expected) {
  std::array<uint8_t,12> header {};
  read_all(header.data(),header.size());
  check(std::memcmp(header.data(),"HVOFFQ01",8)==0,"request magic");
  uint32 n=0;
  for(unsigned i=0;i<4;i++)n|=uint32(header[8+i])<<(8*i);
  check(n<=1024*1024,"request capacity");
  std::vector<uint8_t>b(n);
  read_all(b.data(),b.size());
  uint8_t extra;
  check(read(0,&extra,1)==0,"request trailing bytes");
  Reader r {
    b
  };
  Request q {};
  check(r.u(2)==1,"request version");
  q.request=r.u(8);
  check(q.request==expected&&expected!=0,"request identity");
  auto cid=r.bytes(16);
  check(std::memcmp(cid.data(),component_id,16)==0,"component identity");
  q.rate=uint32(r.u(4));
  q.start=r.u(8);
  q.controller=r.f64();
  check((q.rate==44100||q.rate==48000)&&norm(q.controller),"request rate/value");
  q.component_state=r.blob(65536);
  q.controller_state=r.blob(65536);
  check(!q.component_state.empty()||q.start==0,"fresh fixture requires frame zero");
  auto blocks=r.u(4);
  check(blocks>=1&&blocks<=32,"block capacity");
  uint32 points=0;
  for(size_t i=0;i<blocks;i++) {
    Block block {};
    block.frames=uint32(r.u(4));
    auto count=r.u(4);
    check(block.frames>=1&&block.frames<=1024&&count<=128,"block capacity");
    q.frames+=block.frames;
    points+=uint32(count);
    check(q.frames<=8192&&points<=1024,"aggregate capacity");
    int32 last=-1;
    for(size_t j=0;j<count;j++) {
      auto offset=r.u(4);
      double v=r.f64();
      check(offset<block.frames&&int64(offset)>last&&norm(v),"parameter offset/order/value");
      block.points.push_back( {
        int32(offset),v
      });
      last=int32(offset);
    }for(int channel=0;channel<2;channel++) {
      auto& out=channel?block.right:block.left;
      for(size_t j=0;j<block.frames;j++) {
        float v=r.f32();
        check(std::isfinite(v),"nonfinite input");
        out.push_back(v);
      }
    }q.blocks.push_back(std::move(block));
  }
  check(r.at==b.size()&&q.start<=uint64(INT64_MAX)-q.frames,"trailing request/clock overflow");
  return q;
}
struct Owner  {
  Host host;
  void* library=nullptr;
  bool entered=false;
  using Exit=bool(*)();
  Exit exit=nullptr;
  IPluginFactory* factory=nullptr;
  IComponent* component=nullptr;
  IEditController* controller=nullptr;
  IAudioProcessor* processor=nullptr;
  IConnectionPoint* component_cp=nullptr;
  IConnectionPoint* controller_cp=nullptr;
  IProcessContextRequirements* context_requirements=nullptr;
  bool ci=false,ei=false,handler=false,cconnect=false,econnect=false,in=false,out=false,active=false,processing=false;
  bool failed=false;
  std::vector<std::string> trace;
  Stream component_in,controller_in,sync,component_out,controller_out;
  void record(const char* s) {
    check(trace.size()<128,"lifecycle capacity");
    trace.emplace_back(s);
  }
  void step(tresult result,const char* s) {
    ok(result,s);
    record(s);
  }
  template<class T> void release(T*& p,const char* s) {
    if(p) {
      p->release();
      p=nullptr;
      record(s);
    }
  }
  void cleanup() {
    auto stop=[&](tresult v,const char* s) {
      if(v!=kResultOk)failed=true;
      record(s);
    };
    if(processing) {
      stop(processor->setProcessing(false),"stop-processing");
      processing=false;
    }
    if(active) {
      stop(component->setActive(false),"deactivate");
      active=false;
    }
    if(in) {
      stop(component->activateBus(kAudio,kInput,0,false),"deactivate-input");
      in=false;
    }
    if(out) {
      stop(component->activateBus(kAudio,kOutput,0,false),"deactivate-output");
      out=false;
    }
    if(handler) {
      stop(controller->setComponentHandler(nullptr),"clear-handler");
      handler=false;
    }
    if(cconnect) {
      stop(component_cp->disconnect(controller_cp),"disconnect-component");
      cconnect=false;
    }
    if(econnect) {
      stop(controller_cp->disconnect(component_cp),"disconnect-controller");
      econnect=false;
    }
    if(ci) {
      stop(component->terminate(),"terminate-component");
      ci=false;
    }
    if(ei) {
      stop(controller->terminate(),"terminate-controller");
      ei=false;
    }
    release(context_requirements,"release-context");
    release(component_cp,"release-component-connection");
    release(controller_cp,"release-controller-connection");
    release(processor,"release-processor");
    release(component,"release-component");
    release(controller,"release-controller");
    release(factory,"release-factory");
    if(entered) {
      if(!exit())failed=true;
      entered=false;
      record("module-exit");
    }
    if(library) {
      if(dlclose(library)!=0)failed=true;
      library=nullptr;
      record("module-unload");
    }
    check(host.refs==1&&host.messages==0&&host.balanced()&&!host.fault,"host ownership/callback refusal");
    for(auto stream: {
      &component_in,&controller_in,&sync,&component_out,&controller_out
    })check(stream->refs==1&&!stream->fault,"state adapter ownership/capacity");
    check(!failed,"native cleanup failed");
  }
  void load(const char* path) {
    library=dlopen(path,RTLD_NOW|RTLD_LOCAL);
    check(library,"module load failed");
    using Entry=bool(*)(void*);
    using Factory=IPluginFactory*(*)();
    auto entry=reinterpret_cast<Entry>(dlsym(library,"ModuleEntry"));
    exit=reinterpret_cast<Exit>(dlsym(library,"ModuleExit"));
    auto get=reinterpret_cast<Factory>(dlsym(library,"GetPluginFactory"));
    check(entry&&exit&&get,"module exports");
    check(entry(library),"module entry");
    entered=true;
    record("module-entry");
    factory=get();
    check(factory,"factory");
    record("factory");
  }
  void prepare(const Request& q) {
    PFactoryInfo info{};
    step(factory->getFactoryInfo(&info),"factory-info");
    check(info.flags==PFactoryInfo::kUnicode&&factory->countClasses()==2,"unsupported factory profile");
    for(int32 i=0;i<2;++i) {
      PClassInfo klass{};
      step(factory->getClassInfo(i,&klass),"factory-class");
      check(equal(klass.cid,i?controller_id:component_id)&&klass.cardinality==PClassInfo::kManyInstances,"factory class identity");
      check(strnlen(klass.category,sizeof klass.category)<sizeof klass.category&&strnlen(klass.name,sizeof klass.name)<sizeof klass.name,"unterminated factory metadata");
      check(std::strcmp(klass.category,i?"Component Controller Class":"Audio Module Class")==0,"factory class category");
    }
    void* p=nullptr;
    step(factory->createInstance(component_id,INLINE_UID_OF(IComponent),&p),"create-component");
    component=static_cast<IComponent*>(p);
    check(component,"null component");
    TUID cid;
    step(component->getControllerClassId(cid),"controller-id");
    check(equal(cid,controller_id),"controller identity");
    void* combined=nullptr;
    auto combined_result=component->queryInterface(INLINE_UID_OF(IEditController),&combined);
    if(combined) {
      static_cast<IEditController*>(combined)->release();
      throw std::runtime_error("combined component/controller unsupported");
    }check(combined_result==kNoInterface,"unexpected controller query");
    step(component->setIoMode(kOfflineProcessing),"offline-io");
    step(component->initialize(static_cast<IHostApplication*>(&host)),"initialize-component");
    ci=true;
    p=nullptr;
    step(factory->createInstance(cid,INLINE_UID_OF(IEditController),&p),"create-controller");
    controller=static_cast<IEditController*>(p);
    check(controller,"null controller");
    step(controller->initialize(static_cast<IHostApplication*>(&host)),"initialize-controller");
    ei=true;
    step(controller->setComponentHandler(&host),"install-handler");
    handler=true;
    check(!host.fault,"host restart refused");
    p=nullptr;
    step(component->queryInterface(INLINE_UID_OF(IConnectionPoint),&p),"query-component-connection");
    component_cp=static_cast<IConnectionPoint*>(p);
    p=nullptr;
    step(controller->queryInterface(INLINE_UID_OF(IConnectionPoint),&p),"query-controller-connection");
    controller_cp=static_cast<IConnectionPoint*>(p);
    check(component_cp&&controller_cp,"null connection");
    step(component_cp->connect(controller_cp),"connect-component");
    cconnect=true;
    step(controller_cp->connect(component_cp),"connect-controller");
    econnect=true;
    component_in.bytes=q.component_state;
    if(!q.component_state.empty())step(component->setState(&component_in),"restore-component");
    step(component->getState(&sync),"sync-component-state");
    sync.at=0;
    step(controller->setComponentState(&sync),"sync-controller-state");
    controller_in.bytes=q.controller_state;
    if(!q.controller_state.empty())step(controller->setState(&controller_in),"restore-controller");
    step(controller->setParamNormalized(gain_id,q.controller),"controller-display-value");
    p=nullptr;
    step(component->queryInterface(INLINE_UID_OF(IAudioProcessor),&p),"query-processor");
    processor=static_cast<IAudioProcessor*>(p);
    check(processor,"null processor");
    check(component->getBusCount(kAudio,kInput)==1&&component->getBusCount(kAudio,kOutput)==1&&component->getBusCount(kEvent,kInput)==0&&component->getBusCount(kEvent,kOutput)==0,"unsupported bus counts");
    SpeakerArrangement stereo=SpeakerArr::kStereo;
    step(processor->setBusArrangements(&stereo,1,&stereo,1),"stereo-arrangement");
    for(auto dir: {
      kInput,kOutput
    }) {
      BusInfo bus {};
      SpeakerArrangement arrangement=0;
      step(component->getBusInfo(kAudio,dir,0,bus),dir==kInput?"input-info":"output-info");
      step(processor->getBusArrangement(dir,0,arrangement),"bus-arrangement");
      check(bus.channelCount==2&&bus.busType==kMain&&bus.mediaType==kAudio&&bus.direction==dir&&bus.flags==BusInfo::kDefaultActive&&arrangement==SpeakerArr::kStereo,"unsupported bus capability");
      utf8(bus.name);
    }
    step(processor->canProcessSampleSize(kSample32),"float32");
    ProcessSetup setup {};
    setup.processMode=kOffline;
    setup.symbolicSampleSize=kSample32;
    setup.maxSamplesPerBlock=1024;
    setup.sampleRate=q.rate;
    step(processor->setupProcessing(setup),"setup-processing");
    check(processor->getLatencySamples()==0&&processor->getTailSamples()==0,"latency/tail refused");
    p=nullptr;
    step(component->queryInterface(INLINE_UID_OF(IProcessContextRequirements),&p),"query-context");
    context_requirements=static_cast<IProcessContextRequirements*>(p);
    check(context_requirements&&context_requirements->getProcessContextRequirements()==(IProcessContextRequirements::kNeedContinousTimeSamples|IProcessContextRequirements::kNeedTransportState),"context requirements refused");
  }
  std::vector<ParameterInfo> parameters() {
    auto count=controller->getParameterCount();
    check(count>0&&count<=128,"parameter metadata capacity");
    std::vector<ParameterInfo> params;
    for(int32 i=0;i<count;i++) {
      ParameterInfo p {};
      step(controller->getParameterInfo(i,p),"parameter-info");
      check(norm(p.defaultNormalizedValue)&&p.stepCount>=0,"parameter metadata invalid");
      for(auto prior:params)check(prior.id!=p.id,"duplicate parameter ID");
      utf8(p.title);
      params.push_back(p);
    }check(params.size()==1&&params[0].id==gain_id&&params[0].flags==ParameterInfo::kCanAutomate&&params[0].stepCount==0,"unsupported parameter profile");
    return params;
  }
  void process_blocks(const Request& q,std::vector<float>& left,std::vector<float>& right) {
    step(processor->setProcessing(true),"start-processing");
    processing=true;
    size_t at=0;
    for(const auto& block:q.blocks) {
      Changes changes;
      changes.queue.points=block.points;
      // Child-owned mutable inputs, separate from the immutable request value.
      auto input_left=block.left;
      auto input_right=block.right;
      float* input_channels[2]={input_left.data(),input_right.data()};
      float* output_channels[2]={left.data()+at,right.data()+at};
      AudioBusBuffers inputs,outputs;
      inputs.numChannels=2;
      inputs.channelBuffers32=input_channels;
      outputs.numChannels=2;
      outputs.channelBuffers32=output_channels;
      ProcessContext context{};
      context.sampleRate=q.rate;
      context.projectTimeSamples=int64(q.start+at);
      context.continousTimeSamples=context.projectTimeSamples;
      context.state=ProcessContext::kPlaying|ProcessContext::kContTimeValid;
      ProcessData data;
      data.processMode=kOffline;
      data.symbolicSampleSize=kSample32;
      data.numSamples=int32(block.frames);
      data.numInputs=1;
      data.numOutputs=1;
      data.inputs=&inputs;
      data.outputs=&outputs;
      data.inputParameterChanges=&changes;
      data.processContext=&context;
      step(processor->process(data),"process");
      check(changes.refs==1&&changes.queue.refs==1,"parameter adapter ownership");
      for(size_t i=at;i<at+block.frames;i++) {
        check(std::isfinite(left[i])&&std::isfinite(right[i]),"nonfinite output refused");
      }
      at+=block.frames;
    }
    step(processor->setProcessing(false),"stop-processing");
    processing=false;
  }
  void render(const Request& q,std::vector<float>& left,std::vector<float>& right) {
    step(component->activateBus(kAudio,kInput,0,true),"activate-input");
    in=true;
    step(component->activateBus(kAudio,kOutput,0,true),"activate-output");
    out=true;
    step(component->setActive(true),"activate");
    active=true;
    left.resize(q.frames);
    right.resize(q.frames);
    std::exception_ptr failure;
    std::thread worker([&] {
      try {
        process_blocks(q,left,right);
      } catch(...) {
        failure=std::current_exception();
        if(processing) {
          if(processor->setProcessing(false)!=kResultOk)failed=true;
          processing=false;
          record("stop-processing");
        }
      }
    });
    worker.join();
    if(failure)std::rethrow_exception(failure);
    step(component->setActive(false),"deactivate");
    active=false;
    step(component->getState(&component_out),"save-component");
    step(controller->getState(&controller_out),"save-controller");
  }
};
int main(int argc,char** argv) {
  if(argc!=3)return 2;
  rlimit core {};
  if(setrlimit(RLIMIT_CORE,&core)!=0)return 2;
  Owner owner;
  try {
    char* end=nullptr;
    auto id=std::strtoull(argv[2],&end,10);
    check(id&&end&&!*end,"request ID");
    auto q=read_request(id);
    owner.load(argv[1]);
    owner.prepare(q);
    auto params=owner.parameters();
    std::vector<float> left,right;
    owner.render(q,left,right);
    owner.cleanup();
    Writer w;
    w.u(1,2);
    w.u(q.request,8);
    w.text("3cdf9ca5d1f5b1b21e0a86832aa4abe55607bd96");
    w.text("586dc5e6c8012c3e4b01c79389375cbe96bdb1da");
    w.text("linux-x86_64");
    w.raw(component_id,16);
    w.raw(controller_id,16);
    w.u(q.rate,4);
    w.u(0,4);
    w.u(0,4);
    w.u(2,4);
    w.u(2,4);
    w.u(0,4);
    w.u(0,4);
    w.u(params.size(),4);
    for(auto p:params) {
      w.u(p.id,4);
      w.u(uint32(p.flags),4);
      w.u(uint32(p.stepCount),4);
      w.f64(p.defaultNormalizedValue);
      w.text(utf8(p.title));
    }w.u(q.frames,4);
    for(auto v:left)w.f32(v);
    for(auto v:right)w.f32(v);
    w.blob(owner.component_out.bytes);
    w.blob(owner.controller_out.bytes);
    w.u(owner.trace.size(),4);
    for(auto s:owner.trace)w.text(s);
    w.u(owner.host.callbacks.size(),4);
    for(auto s:owner.host.callbacks)w.text(s);
    Writer packet;
    packet.raw("HVOFFR01",8);
    packet.u(w.b.size(),4);
    packet.raw(w.b.data(),w.b.size());
    write_all(packet.b);
    return 0;
  }catch(const std::exception& e) {
    std::fprintf(stderr,"offline helper: %s\n",e.what());
    try {
      owner.cleanup();
    }catch(const std::exception& cleanup) {
      std::fprintf(stderr,"offline cleanup: %s\n",cleanup.what());
    }return 2;
  }
}
