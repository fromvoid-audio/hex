// Ordinary one-module/one-instance native worker. ABI/foreign process lifetime
// stays here. It is crash isolation for user-selected code, not a security sandbox.
#import <Foundation/Foundation.h>
#include "macos_bundle.hpp"
#include "macos_instance.hpp"
#include "macos_instance_wire.hpp"
#include <atomic>
#include <condition_variable>
#include <mutex>
#include <fcntl.h>
#include <optional>
#include <thread>

namespace wire=hex_instance_wire;
using namespace Steinberg;using namespace Steinberg::Vst;
namespace {
enum Failure : uint32_t { Configuration=1,Admission=2,ClassFailure=3,Setup=4,Presentation=5,Capture=6,
    Processing=7,ProtocolFailure=8,Cancelled=9,Teardown=10,HostControl=11,Unexpected=12,InstanceOpen=13,Restore=14,
    Quiescence=15,Resume=16 };
struct Refusal {}; // Expected local/SDK return-code refusal, never an exception from foreign code.
struct Module {
    CFBundleRef bundle=nullptr;IPluginFactory* factory=nullptr;
    bool native_attempted=false,loaded=false,entered=false,opened=false;bool (*exit)()=nullptr;
    // No destructor foreign calls. Refused instance/module teardown keeps all
    // remaining references until this process has actually retired.
    void open(const wire::Config& request) {
        using namespace hex_bundle;
        if(canonical(request.root)!=request.root || request.root.size()>bundle_path_limit)
            throw Refusal{};
        NSString* root=string(request.root);
        const auto info=hex(request.info_hash.data(),request.info_hash.size());
        const auto binary=hex(request.executable_hash.data(),request.executable_hash.size());
        if(digest(member(root,@"Contents/Info.plist"),65536)!=info) throw Refusal{};
        CFURLRef url=CFURLCreateWithFileSystemPath(nullptr,(__bridge CFStringRef)root,kCFURLPOSIXPathStyle,true);
        if(!url) throw Refusal{};
        bundle=CFBundleCreate(nullptr,url);CFRelease(url);
        if(!bundle || executable(bundle,root)!=request.executable ||
            digest(member(root,string(request.executable)),16*1024*1024)!=binary ||
            digest(member(root,@"Contents/Info.plist"),65536)!=info)
            throw Refusal{};
        native_attempted=true;
        if(!CFBundleLoadExecutable(bundle)) throw Refusal{};loaded=true;
        auto entry=reinterpret_cast<bool(*)(CFBundleRef)>(CFBundleGetFunctionPointerForName(bundle,CFSTR("bundleEntry")));
        exit=reinterpret_cast<bool(*)()>(CFBundleGetFunctionPointerForName(bundle,CFSTR("bundleExit")));
        auto get=reinterpret_cast<IPluginFactory*(*)()>(CFBundleGetFunctionPointerForName(bundle,CFSTR("GetPluginFactory")));
        if(!entry || !exit || !get || !entry(bundle)) throw Refusal{};
        entered=true;factory=get();if(!factory) throw Refusal{};opened=true;
    }
    bool has_class(const wire::Config& request) {
        const auto count=factory->countClasses();if(count<1 || count>256) return false;
        bool matched=false;
        for(int32 n=0;n<count;++n) {
            PClassInfo info{};
            if(factory->getClassInfo(n,&info)!=kResultOk || !std::memchr(info.category,0,sizeof info.category)) return false;
            if(!std::memcmp(info.cid,request.cid.data(),16)) {
                if(matched || std::strcmp(info.category,kVstAudioEffectClass)) return false;matched=true;
            }
        }
        return matched;
    }
    bool close() {
        if(loaded && !opened) return false; // no successful module-entry/factory contract
        if(factory) { factory->release();factory=nullptr; }
        if(entered) { if(!exit || !exit()) return false;entered=false; }
        if(loaded) { CFBundleUnloadExecutable(bundle);if(CFBundleIsExecutableLoaded(bundle)) return false;loaded=false; }
        if(bundle) { CFRelease(bundle);bundle=nullptr; }
        return true;
    }
    static std::string hex(const uint8_t* p,size_t count) { return wire::hex(p,count); }
};
int isolate_channel() {
    const auto channel=::dup(STDIN_FILENO);if(channel<0) return -1;
    const int quiet=::open("/dev/null",O_RDWR|O_CLOEXEC);
    const int enabled=1;
    if(quiet<0 || ::fcntl(channel,F_SETFD,FD_CLOEXEC)<0 ||
        ::setsockopt(channel,SOL_SOCKET,SO_NOSIGPIPE,&enabled,sizeof enabled)!=0 ||
        ::dup2(quiet,STDIN_FILENO)<0 || ::dup2(quiet,STDOUT_FILENO)<0 || ::dup2(quiet,STDERR_FILENO)<0) {
        if(quiet>=0) ::close(quiet);::close(channel);return -1;
    }
    ::close(quiet);return channel;
}
// Supervised clients provide an independent cancellation socket on stdout.
// Start before any foreign entry: even a hung bundleEntry/unload cannot stop
// this lane from retiring the entire isolated process. It never unloads SDK
// code, signals another PID or touches the Hex callback. EOF/any byte cancels.
class CancellationGuardian {
    int channel=-1;
    std::atomic<bool> complete{false};
    std::thread watch;
public:
    bool start() {
        channel=::dup(STDOUT_FILENO);if(channel<0) return false;
        int kind=0;socklen_t size=sizeof kind;
        if(::fcntl(channel,F_SETFD,FD_CLOEXEC)<0 ||
            ::getsockopt(channel,SOL_SOCKET,SO_TYPE,&kind,&size)!=0 || kind!=SOCK_STREAM) {
            ::close(channel);channel=-1;return false;
        }
        try {
            watch=std::thread([this] {
                uint8_t byte=0;ssize_t count=0;
                do { count=::read(channel,&byte,1); } while(count<0 && errno==EINTR);
                if(!complete.load(std::memory_order_acquire)) std::_Exit(Cancelled);
            });
        } catch(...) { ::close(channel);channel=-1;return false; }
        return true;
    }
    ~CancellationGuardian() {
        if(channel<0) return;
        complete.store(true,std::memory_order_release);
        ::shutdown(channel,SHUT_RDWR);
        if(watch.joinable()) watch.join();
        ::close(channel);
    }
};
void error(int channel,const wire::Config* config,uint32_t reason) {
    wire::Bytes out;wire::number(out,1,2);wire::number(out,config?1:0,1);
    if(config) for(auto id:config->binding) wire::number(out,id,8);
    wire::number(out,reason,4);wire::send(channel,"HXVIER01",out);
}
wire::Bytes state_payload(const wire::Config& config,const hex_instance::CapturedState& state) {
    wire::Bytes out{'H','X','V','S',1,static_cast<uint8_t>((state.component?1:0)|(state.controller?2:0)),0,0};
    out.insert(out.end(),config.cid.begin(),config.cid.end());
    out.insert(out.end(),config.executable_hash.begin(),config.executable_hash.end());
    out.insert(out.end(),config.info_hash.begin(),config.info_hash.end());
    wire::number(out,state.component_bytes,4);wire::number(out,state.controller_bytes,4);
    out.insert(out.end(),state.bytes.begin(),state.bytes.begin()+state.component_bytes+state.controller_bytes);return out;
}
wire::Bytes facts_body(const wire::Config& config,uint32 latency,uint32 tail,bool presentation_available,const wire::Startup& startup) {
    wire::Bytes out;config.stamp(out);out.insert(out.end(),config.cid.begin(),config.cid.end());
    out.insert(out.end(),config.executable_hash.begin(),config.executable_hash.end());
    out.insert(out.end(),config.info_hash.begin(),config.info_hash.end());
    uint64_t rate=0;std::memcpy(&rate,&config.rate,8);wire::number(out,rate,8);
    wire::number(out,config.maximum,2);wire::number(out,config.inputs,1);wire::number(out,config.outputs,1);
    wire::number(out,latency,4);wire::number(out,tail,4);wire::number(out,presentation_available?1:0,1);startup.acknowledge(out);return out;
}
wire::Bytes ready_body(const wire::Config& config,const hex_instance::CapturedState& state,
    uint32 latency,uint32 tail,hex_instance::Instance::PresentationResult presentation,const std::optional<wire::Presentation>& supplied,bool restored) {
    wire::Bytes out;config.stamp(out);out.insert(out.end(),config.cid.begin(),config.cid.end());
    uint64_t rate=0;std::memcpy(&rate,&config.rate,8);wire::number(out,rate,8);
    wire::number(out,config.maximum,2);wire::number(out,config.inputs,1);wire::number(out,config.outputs,1);
    wire::number(out,latency,4);wire::number(out,tail,4);wire::number(out,static_cast<uint32>(presentation),1);
    wire::number(out,supplied?1:0,1);
    if(supplied) { wire::number(out,supplied->input,4);wire::number(out,supplied->output,4); }
    wire::number(out,restored?1:0,1);
    // State existence is not availability or an application/graph ACK.
    wire::number(out,state.component?1:0,1);auto payload=state_payload(config,state);
    wire::number(out,payload.size(),2);out.insert(out.end(),payload.begin(),payload.end());
    return out;
}
}
int main(int argc,char** argv) {
    // No SDK entry or protocol/guardian is used by the actual waitable-child
    // preflight. The caller must observe normal direct reap before loading code.
    if(argc==2 && !std::strcmp(argv[1],"--child-custody-probe")) return 0;
    const bool supervised=argc==2 && !std::strcmp(argv[1],"--supervised");
    if(argc!=1 && !supervised) return Configuration;
    // Static lifetime keeps cancellation live through main-thread TLS/atexit
    // cleanup registered by subsequently loaded SDK modules, not just main.
    static CancellationGuardian guardian;
    if(supervised && !guardian.start()) return Configuration;
    const int channel=isolate_channel();if(channel<0) return Configuration;
    @autoreleasepool {
        Module module;hex_instance::Instance instance;
        std::optional<wire::Config> config;
        uint32_t failure=Configuration;bool accepted=false,normal_stop=false;
        uint64_t blocks=0,frames=0;
        try {
            auto request=wire::configuration(channel);config.emplace(request);
            const auto restoration=wire::startup(channel,*config);
            failure=Admission;module.open(*config);
            failure=ClassFailure;if(!module.has_class(*config)) throw Refusal{};
            TUID cid{};std::memcpy(cid,config->cid.data(),16);
            failure=InstanceOpen;
            if(!instance.open(module.factory,cid)) throw Refusal{};
            if(restoration.restored()) {
                failure=Restore;if(!instance.restore_initial_state_on_control(restoration.state)) throw Refusal{};
            }
            failure=Setup;hex_instance::ProcessingConfiguration setup{};
            setup.sample_rate=config->rate;setup.maximum_frames=config->maximum;
            setup.input_channels=config->inputs;setup.output_channels=config->outputs;
            setup.input_enabled=config->inputs!=0;
            if(instance.configure(setup)!=hex_instance::ConfigurationResult::Prepared) throw Refusal{};
            failure=HostControl;instance.dispatch_host_callbacks();
            if(instance.host_refusals()) throw Refusal{};
            auto latency=instance.reported_latency_on_control();
            auto tail=instance.reported_tail_on_control();
            auto available=instance.presentation_latency_available_on_control();
            if(!latency || !tail || !available) throw Refusal{};
            failure=ProtocolFailure;
            if(!wire::send(channel,"HXVIFA03",facts_body(*config,*latency,*tail,*available,restoration))) throw Refusal{};
            std::optional<wire::Presentation> supplied;
            if(!wire::presentation(channel,*config,supplied)) throw Refusal{};
            failure=Presentation;
            auto presentation=hex_instance::Instance::PresentationResult::Absent;
            if(supplied) presentation=instance.set_presentation_latency(supplied->input,supplied->output);
            else if(*available) throw Refusal{}; // unknown is never substituted with zero
            if((presentation==hex_instance::Instance::PresentationResult::Applied)!=*available ||
                (presentation!=hex_instance::Instance::PresentationResult::Applied &&
                 presentation!=hex_instance::Instance::PresentationResult::Absent)) throw Refusal{};
            failure=Capture;hex_instance::CapturedState state{};
            if(!instance.capture_initial_state_on_control(state)) throw Refusal{};
            failure=HostControl;instance.dispatch_host_callbacks();
            if(instance.host_refusals()) throw Refusal{};
            const auto final_latency=instance.reported_latency_on_control();
            const auto final_tail=instance.reported_tail_on_control();
            failure=Presentation;
            if(!final_latency || !final_tail || *final_latency!=*latency || *final_tail!=*tail) throw Refusal{}; // caller's plan became stale
            const auto hello=ready_body(*config,state,*latency,*tail,presentation,supplied,restoration.restored());
            auto processing_parameters=instance.processing_parameters_on_control();
            hex_instance::ParameterChanges parameter_input,parameter_output;
            hex_instance::ParameterFeedback parameter_feedback;
            std::atomic<bool> done{false};std::atomic<uint32_t> worker_failure{0},startup{0},phase{0};
            std::mutex startup_mutex;std::condition_variable startup_changed;bool go=false;
            // One serialized epoch mailbox, never a second request reader. The
            // processing lane is stopped and waiting at every epoch SDK step.
            // Live capture below has its own bounded nonwaiting slot in this
            // same worker. Mutex/condition waits live only in the epoch path.
            enum EpochStage { Idle,FactsRequested,FactsSent,CaptureRequested,CaptureSent,ResumeRequested,ResumeConfigured,Restarted,ResumeSent };
            EpochStage epoch_stage=Idle;
            uint64_t next_graph=0,next_epoch=0;
            uint64_t capture_operation=0;
            std::optional<wire::Presentation> next_presentation;
            // Single live capture slot. Main exclusively writes its result
            // after Requested/acquire, then Complete/release transfers it back
            // to the original processing reader. No SDK wait, extra reader or
            // concurrent socket writer is added to the PCM path.
            enum LiveCaptureStage { LiveIdle,LiveFeedbackPending,LiveRequested,LiveComplete,LiveFailed };
            std::atomic<LiveCaptureStage> live_stage{LiveIdle};
            hex_instance::CapturedState live_state{};
            uint32_t live_failure=0;
            std::thread worker([&] {
                bool started=false;
                try {
                    started=instance.start_processing();if(!started) worker_failure.store(Processing);
                    startup.store(started?1:2,std::memory_order_release);
                    { std::unique_lock<std::mutex> lock(startup_mutex);startup_changed.wait(lock,[&]{return go;}); }
                    wire::Packet packet{};
                    std::array<float,wire::max_frames> left{},right{},output_left{},output_right{};
                    float* input_pointers[]={left.data(),right.data()};float* output_pointers[]={output_left.data(),output_right.data()};
                    AudioBusBuffers input{},output{};input.numChannels=config->inputs;output.numChannels=config->outputs;
                    input.channelBuffers32=input_pointers;output.channelBuffers32=output_pointers;
                    ProcessData data{};data.processMode=kRealtime;data.symbolicSampleSize=kSample32;
                    uint64_t live_operation=0;
                    data.numInputs=config->inputs?1:0;data.numOutputs=1;data.inputs=config->inputs?&input:nullptr;data.outputs=&output;
                    // A live SDK failure waits only for the next serialized
                    // request to report its error, without another DSP call.
                    // This keeps the failure reply paired with that exchange
                    // instead of racing a later write against child exit.
                    while(started && (!worker_failure.load() || live_stage.load(std::memory_order_acquire)==LiveFailed)) {
                        phase.store(1,std::memory_order_release); // receiving a complete frame
                        const auto received=packet.read(channel,*config,blocks,frames);
                        if(worker_failure.load()) break;
                        instance.publish_parameter_feedback(parameter_feedback);
                        if(live_stage.load(std::memory_order_acquire)==LiveFeedbackPending && !parameter_feedback.count)
                            live_stage.store(LiveRequested,std::memory_order_release);
                        if(received==6 || received==7) {
                            const auto stage=live_stage.load(std::memory_order_acquire);
                            if(received==6) {
                                if(stage!=LiveIdle || live_operation==std::numeric_limits<uint64_t>::max() ||
                                    packet.capture_operation!=live_operation+1) { worker_failure.store(ProtocolFailure);break; }
                                live_operation=packet.capture_operation;
                                live_stage.store(parameter_feedback.count?LiveFeedbackPending:LiveRequested,std::memory_order_release);
                            } else if(stage==LiveIdle || packet.capture_operation!=live_operation) {
                                worker_failure.store(ProtocolFailure);break;
                            }
                            if(received==7 && stage==LiveFailed) { worker_failure.store(live_failure);break; }
                            const bool complete=received==7 && stage==LiveComplete;
                            wire::Bytes body;config->stamp(body);
                            wire::number(body,blocks,8);wire::number(body,frames,8);wire::number(body,live_operation,8);
                            wire::number(body,complete?1:0,1);
                            const auto payload=complete?state_payload(*config,live_state):wire::Bytes{};
                            wire::number(body,payload.size(),2);body.insert(body.end(),payload.begin(),payload.end());
                            if(!wire::send(channel,"HXVLCA01",body)) std::_Exit(Cancelled);
                            if(complete) live_stage.store(LiveIdle,std::memory_order_release);
                            continue;
                        }
                        if((received==2 || received==3) && live_stage.load(std::memory_order_acquire)!=LiveIdle) {
                            worker_failure.store(ProtocolFailure);break;
                        }
                        if(received==2) { normal_stop=true;break; }
                        if(received==0) std::_Exit(Cancelled); // EOF never claims cooperative close
                        if(received==3) {
                            // Main's epoch handshake drains the last worker-owned
                            // observations before state capture or resume.
                            phase.store(3,std::memory_order_release);
                            if(!instance.stop_processing()) { error(channel,&*config,Quiescence);std::_Exit(Quiescence); }
                            started=false;
                            {
                                std::unique_lock<std::mutex> lock(startup_mutex);
                                epoch_stage=FactsRequested;startup_changed.notify_all();
                                startup_changed.wait(lock,[&]{return epoch_stage==FactsSent;});
                            }
                            phase.store(1,std::memory_order_release);
                            auto resumed=packet.read(channel,*config,blocks,frames);
                            uint64_t captures=0;
                            while(resumed==5) {
                                if(captures==std::numeric_limits<uint64_t>::max() || packet.capture_operation!=captures+1) {
                                    worker_failure.store(ProtocolFailure);break;
                                }
                                {
                                    std::unique_lock<std::mutex> lock(startup_mutex);
                                    capture_operation=packet.capture_operation;epoch_stage=CaptureRequested;
                                    startup_changed.notify_all();
                                    startup_changed.wait(lock,[&]{return epoch_stage==CaptureSent;});
                                }
                                ++captures;resumed=packet.read(channel,*config,blocks,frames);
                            }
                            if(worker_failure.load()) break;
                            if(resumed==2) { normal_stop=true;break; }
                            if(resumed==0) std::_Exit(Cancelled);
                            if(resumed!=4) { worker_failure.store(ProtocolFailure);break; }
                            {
                                std::unique_lock<std::mutex> lock(startup_mutex);
                                next_graph=packet.next_graph;next_epoch=packet.next_epoch;
                                next_presentation=packet.resumed_presentation;
                                epoch_stage=ResumeRequested;startup_changed.notify_all();
                                startup_changed.wait(lock,[&]{return epoch_stage==ResumeConfigured;});
                            }
                            phase.store(2,std::memory_order_release);
                            started=instance.start_processing();
                            if(!started) { worker_failure.store(Resume);break; }
                            {
                                std::unique_lock<std::mutex> lock(startup_mutex);
                                epoch_stage=Restarted;startup_changed.notify_all();
                                startup_changed.wait(lock,[&]{return epoch_stage==ResumeSent;});
                            }
                            continue;
                        }
                        if(received!=1) { worker_failure.store(ProtocolFailure);break; }
                        bool valid=true;output_right.fill(0);input.silenceFlags=0;output.silenceFlags=0;
                        for(size_t n=0;n<packet.frames;++n) {
                            left[n]=wire::sample(packet.bytes.data()+wire::header_size+n*8);
                            right[n]=wire::sample(packet.bytes.data()+wire::header_size+n*8+4);
                            valid=valid && (config->inputs==2 || right[n]==0) && (config->inputs!=0 || left[n]==0);
                        }
                        if(!valid) { worker_failure.store(ProtocolFailure);break; }
                        instance.refresh_processing_parameters(processing_parameters);
                        if(!parameter_input.reset(packet.frames,false) || !parameter_output.reset(packet.frames,true)) {
                            error(channel,&*config,Processing);std::_Exit(Processing);
                        }
                        for(size_t n=0;n<packet.parameter_count;++n) {
                            const auto& point=packet.parameters[n];
                            const auto* info=processing_parameters.find(point.id);
                            if(!info || (info->flags&(ParameterInfo::kIsReadOnly|ParameterInfo::kIsHidden)) ||
                                !parameter_input.input(point.id,point.offset,point.normalized)) valid=false;
                        }
                        if(!valid) { worker_failure.store(ProtocolFailure);break; }
                        if(!parameter_input.begin_sdk() || !parameter_output.begin_sdk()) {
                            error(channel,&*config,Processing);std::_Exit(Processing);
                        }
                        data.inputParameterChanges=parameter_input.size()?&parameter_input:nullptr;
                        data.outputParameterChanges=&parameter_output;
                        data.numSamples=packet.frames;phase.store(2,std::memory_order_release);
                        const bool processed=instance.process(data);
                        // No SDK stop/unload beneath retained or malformed
                        // borrowed queues. Their existing process owner retires
                        // the isolated instance without claiming rollback.
                        const bool input_returned=parameter_input.end_sdk();
                        const bool output_returned=parameter_output.end_sdk();
                        if(!input_returned || !output_returned) {
                            error(channel,&*config,Processing);std::_Exit(Processing);
                        }
                        if(!processed) { worker_failure.store(Processing);break; }
                        for(size_t n=0;n<parameter_output.size();++n) {
                            hex_instance::ParameterPoint point{};
                            if(!parameter_output.point(n,point) || !processing_parameters.find(point.id) ||
                                !parameter_feedback.put(point.id,point.normalized)) valid=false;
                        }
                        if(!valid) { worker_failure.store(ProtocolFailure);break; }
                        for(size_t n=0;n<packet.frames;++n) {
                            wire::sample(packet.bytes.data()+wire::header_size+n*8,output_left[n]);
                            wire::sample(packet.bytes.data()+wire::header_size+n*8+4,output_right[n]);
                        }
                        instance.publish_parameter_feedback(parameter_feedback);
                        if(!packet.write(channel,parameter_output)) { worker_failure.store(Cancelled);break; }
                        ++blocks;frames+=packet.frames;
                    }
                    if(worker_failure.load()) error(channel,&*config,worker_failure.load());
                    // Never stop/deactivate/unload beneath a main-lane capture,
                    // even on malformed PCM, EOF or a foreign processing fault.
                    if(live_stage.load(std::memory_order_acquire)!=LiveIdle)
                        std::_Exit(worker_failure.load()?worker_failure.load():Cancelled);
                    phase.store(3,std::memory_order_release); // foreign stopping may also refuse/hang
                    if(started && !instance.stop_processing()) worker_failure.store(Teardown);
                } catch(...) {
                    // A foreign exception gives no successful-stage evidence.
                    // Retire this process without stop/deactivate/release/unload.
                    error(channel,&*config,Unexpected);std::_Exit(Unexpected);
                }
                done.store(true,std::memory_order_release);
            });
            // Ready means the actual worker's fallible setProcessing succeeded,
            // not merely that a control-side configuration was copied.
            while(!startup.load(std::memory_order_acquire)) {
                uint8_t byte=0;const auto peer=::recv(channel,&byte,1,MSG_PEEK|MSG_DONTWAIT);
                if(peer==0) std::_Exit(Cancelled);
                CFRunLoopRunInMode(kCFRunLoopDefaultMode,0.001,true);
            }
            if(instance.host_refusals()) worker_failure.store(HostControl);
            accepted=startup.load(std::memory_order_acquire)==1 && !worker_failure.load() && wire::send(channel,"HXVIRD04",hello);
            if(!accepted && !worker_failure.load()) worker_failure.store(Cancelled);
            { std::lock_guard<std::mutex> lock(startup_mutex);go=true;startup_changed.notify_all(); }
            // Pump the helper main lane, never the Hex callback. Unsupported
            // restart/control requests cannot be silently acknowledged as applied.
            while(!done.load(std::memory_order_acquire)) {
                try {
                    if(worker_failure.load()) {
                        // The joining worker/guardian owns terminal progress.
                        // Do not dispatch more foreign callbacks after failure.
                        std::this_thread::yield();continue;
                    }
                    if(!instance.synchronize_parameter_feedback_on_control()) { error(channel,&*config,HostControl);std::_Exit(HostControl); }
                    instance.dispatch_host_callbacks();
                    if(instance.host_refusals()) { error(channel,&*config,HostControl);std::_Exit(HostControl); }
                    if(live_stage.load(std::memory_order_acquire)==LiveRequested) {
                        try {
                            const bool captured=instance.synchronize_parameter_feedback_on_control() && instance.capture_current_state_on_control(live_state);
                            instance.dispatch_host_callbacks();
                            live_failure=captured && !instance.host_refusals()?0:Capture;
                        } catch(...) { live_failure=Unexpected; }
                        live_stage.store(live_failure?LiveFailed:LiveComplete,std::memory_order_release);
                        if(live_failure) {
                            // Refusal/exception cannot admit another DSP call.
                            // The original worker reports the failure at its
                            // current exchange boundary, then exits without
                            // stop/unload beneath the failed capture owner.
                            uint32_t clear=0;worker_failure.compare_exchange_strong(clear,live_failure);
                        }
                    }
                    EpochStage stage;
                    { std::lock_guard<std::mutex> lock(startup_mutex);stage=epoch_stage; }
                    if(stage==FactsRequested) {
                        // The worker waits on this acknowledged epoch barrier;
                        // its pending value buffer is exclusively borrowed here.
                        if(!instance.publish_parameter_feedback(parameter_feedback) ||
                            !instance.synchronize_parameter_feedback_on_control()) {
                            error(channel,&*config,HostControl);std::_Exit(HostControl);
                        }
                        latency=instance.reported_latency_on_control();
                        tail=instance.reported_tail_on_control();
                        available=instance.presentation_latency_available_on_control();
                        if(!latency || !tail || !available) { error(channel,&*config,Quiescence);std::_Exit(Quiescence); }
                        auto body=facts_body(*config,*latency,*tail,*available,restoration);
                        wire::number(body,blocks,8);wire::number(body,frames,8);
                        if(!wire::send(channel,"HXVIQF02",body)) std::_Exit(Cancelled);
                        std::lock_guard<std::mutex> lock(startup_mutex);
                        epoch_stage=FactsSent;startup_changed.notify_all();
                    } else if(stage==CaptureRequested) {
                        hex_instance::CapturedState current{};
                        if(!instance.capture_current_state_on_control(current)) { error(channel,&*config,Capture);std::_Exit(Capture); }
                        instance.dispatch_host_callbacks();
                        const auto current_latency=instance.reported_latency_on_control();
                        const auto current_tail=instance.reported_tail_on_control();
                        if(instance.host_refusals() || !current_latency || !current_tail || *current_latency!=*latency || *current_tail!=*tail) {
                            error(channel,&*config,Capture);std::_Exit(Capture);
                        }
                        wire::Bytes body;config->stamp(body);
                        wire::number(body,blocks,8);wire::number(body,frames,8);wire::number(body,capture_operation,8);
                        const auto payload=state_payload(*config,current);wire::number(body,payload.size(),2);
                        body.insert(body.end(),payload.begin(),payload.end());
                        if(!wire::send(channel,"HXVICS01",body)) std::_Exit(Cancelled);
                        std::lock_guard<std::mutex> lock(startup_mutex);
                        epoch_stage=CaptureSent;startup_changed.notify_all();
                    } else if(stage==ResumeRequested) {
                        supplied=next_presentation;
                        presentation=hex_instance::Instance::PresentationResult::Absent;
                        if(supplied) presentation=instance.set_presentation_latency(supplied->input,supplied->output);
                        else if(*available) { error(channel,&*config,Presentation);std::_Exit(Presentation); }
                        const auto current_latency=instance.reported_latency_on_control();
                        const auto current_tail=instance.reported_tail_on_control();
                        if(!current_latency || !current_tail || *current_latency!=*latency || *current_tail!=*tail || instance.host_refusals() ||
                            (presentation==hex_instance::Instance::PresentationResult::Applied)!=*available ||
                            (presentation!=hex_instance::Instance::PresentationResult::Applied &&
                             presentation!=hex_instance::Instance::PresentationResult::Absent)) {
                            error(channel,&*config,Presentation);std::_Exit(Presentation);
                        }
                        // The old input/reply sequence is quiescent. Only this
                        // acknowledged transition changes its wire binding;
                        // no old queued request is accepted under the new one.
                        config->binding[2]=next_graph;config->binding[3]=next_epoch;
                        std::lock_guard<std::mutex> lock(startup_mutex);
                        epoch_stage=ResumeConfigured;startup_changed.notify_all();
                    } else if(stage==Restarted) {
                        auto body=facts_body(*config,*latency,*tail,*available,restoration);
                        wire::number(body,blocks,8);wire::number(body,frames,8);
                        wire::number(body,static_cast<uint32>(presentation),1);
                        wire::number(body,supplied?1:0,1);
                        if(supplied) { wire::number(body,supplied->input,4);wire::number(body,supplied->output,4); }
                        if(!wire::send(channel,"HXVIRR02",body)) std::_Exit(Cancelled);
                        std::lock_guard<std::mutex> lock(startup_mutex);
                        epoch_stage=ResumeSent;startup_changed.notify_all();
                    }
                    uint8_t byte=0;const auto peer=::recv(channel,&byte,1,MSG_PEEK|MSG_DONTWAIT);
                    if(peer==0 && phase.load(std::memory_order_acquire)!=1 && !done.load(std::memory_order_acquire)) {
                        // EOF is cancellation, not the graceful HXVE terminal.
                        // Retire the isolated process rather than wait on hung DSP
                        // or unload code beneath its processing thread.
                        std::_Exit(Cancelled);
                    }
                    CFRunLoopRunInMode(kCFRunLoopDefaultMode,0.001,true);
                } catch(...) { error(channel,&*config,Unexpected);std::_Exit(Unexpected); }
            }
            worker.join();failure=worker_failure.load();
            if(!failure && (!instance.publish_parameter_feedback(parameter_feedback) ||
                !instance.synchronize_parameter_feedback_on_control() || instance.host_refusals())) failure=HostControl;
        } catch(const Refusal&) {
            error(channel,config?&*config:nullptr,failure);
        } catch(...) {
            if(module.native_attempted) {
                error(channel,config?&*config:nullptr,Unexpected);std::_Exit(Unexpected);
            }
            error(channel,config?&*config:nullptr,failure);
        }
        bool closed=false,unloaded=false;
        try { closed=instance.close();if(closed) unloaded=module.close(); } catch(...) {}
        if(!closed || !unloaded) failure=Teardown;
        if(config) {
            wire::Bytes terminal;config->stamp(terminal);wire::number(terminal,blocks,8);wire::number(terminal,frames,8);
            wire::number(terminal,failure,4);wire::number(terminal,closed?1:0,1);wire::number(terminal,unloaded?1:0,1);
            wire::send(channel,"HXVITR01",terminal);
        }
        ::close(channel);
        return failure==0 && normal_stop && closed && unloaded?0:static_cast<int>(failure?failure:ProtocolFailure);
    }
}
