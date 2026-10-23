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
    Processing=7,ProtocolFailure=8,Cancelled=9,Teardown=10,HostControl=11,Unexpected=12,InstanceOpen=13 };
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
wire::Bytes ready_body(const wire::Config& config,const hex_instance::CapturedState& state,
    uint32 latency,hex_instance::Instance::PresentationResult presentation) {
    wire::Bytes out;config.stamp(out);out.insert(out.end(),config.cid.begin(),config.cid.end());
    uint64_t rate=0;std::memcpy(&rate,&config.rate,8);wire::number(out,rate,8);
    wire::number(out,config.maximum,2);wire::number(out,config.inputs,1);wire::number(out,config.outputs,1);
    wire::number(out,latency,4);wire::number(out,static_cast<uint32>(presentation),1);
    // State existence is not availability or an application/graph ACK.
    wire::number(out,state.component?1:0,1);auto payload=state_payload(config,state);
    wire::number(out,payload.size(),2);out.insert(out.end(),payload.begin(),payload.end());
    return out;
}
}
int main(int argc,char**) {
    if(argc!=1) return Configuration;
    const int channel=isolate_channel();if(channel<0) return Configuration;
    @autoreleasepool {
        Module module;hex_instance::Instance instance;
        std::optional<wire::Config> config;
        uint32_t failure=Configuration;bool accepted=false,normal_stop=false;
        uint64_t blocks=0,frames=0;
        try {
            auto request=wire::configuration(channel);config.emplace(request);
            failure=Admission;module.open(*config);
            failure=ClassFailure;if(!module.has_class(*config)) throw Refusal{};
            TUID cid{};std::memcpy(cid,config->cid.data(),16);
            failure=InstanceOpen;
            if(!instance.open(module.factory,cid)) throw Refusal{};
            failure=Setup;hex_instance::ProcessingConfiguration setup{};
            setup.sample_rate=config->rate;setup.maximum_frames=config->maximum;
            setup.input_channels=config->inputs;setup.output_channels=config->outputs;
            setup.input_enabled=config->inputs!=0;
            if(instance.configure(setup)!=hex_instance::ConfigurationResult::Prepared) throw Refusal{};
            failure=Presentation;
            const auto presentation=instance.set_presentation_latency(config->input_latency,config->output_latency);
            if(presentation!=hex_instance::Instance::PresentationResult::Applied &&
                presentation!=hex_instance::Instance::PresentationResult::Absent) throw Refusal{};
            failure=Capture;hex_instance::CapturedState state{};
            if(!instance.capture_initial_state_on_control(state)) throw Refusal{};
            failure=HostControl;instance.dispatch_host_callbacks();
            if(instance.host_refusals()) throw Refusal{};
            auto latency=instance.reported_latency_on_control();if(!latency) throw Refusal{};
            const auto hello=ready_body(*config,state,*latency,presentation);
            std::atomic<bool> done{false};std::atomic<uint32_t> worker_failure{0},startup{0},phase{0};
            std::mutex startup_mutex;std::condition_variable startup_changed;bool go=false;
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
                    data.numInputs=config->inputs?1:0;data.numOutputs=1;data.inputs=config->inputs?&input:nullptr;data.outputs=&output;
                    while(started && !worker_failure.load()) {
                        phase.store(1,std::memory_order_release); // receiving a complete frame
                        const auto received=packet.read(channel,*config,blocks,frames);
                        if(received==2) { normal_stop=true;break; }
                        if(received==0) std::_Exit(Cancelled); // EOF never claims cooperative close
                        if(received!=1) { worker_failure.store(ProtocolFailure);break; }
                        bool valid=true;output_right.fill(0);input.silenceFlags=0;output.silenceFlags=0;
                        for(size_t n=0;n<packet.frames;++n) {
                            left[n]=wire::sample(packet.bytes.data()+wire::header_size+n*8);
                            right[n]=wire::sample(packet.bytes.data()+wire::header_size+n*8+4);
                            valid=valid && (config->inputs==2 || right[n]==0) && (config->inputs!=0 || left[n]==0);
                        }
                        if(!valid) { worker_failure.store(ProtocolFailure);break; }
                        data.numSamples=packet.frames;phase.store(2,std::memory_order_release);
                        if(!instance.process(data)) { worker_failure.store(Processing);break; }
                        for(size_t n=0;n<packet.frames;++n) {
                            wire::sample(packet.bytes.data()+wire::header_size+n*8,output_left[n]);
                            wire::sample(packet.bytes.data()+wire::header_size+n*8+4,output_right[n]);
                        }
                        if(!packet.write(channel)) { worker_failure.store(Cancelled);break; }
                        ++blocks;frames+=packet.frames;
                    }
                    if(worker_failure.load()) error(channel,&*config,worker_failure.load());
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
            accepted=startup.load(std::memory_order_acquire)==1 && !worker_failure.load() && wire::send(channel,"HXVIRD01",hello);
            if(!accepted && !worker_failure.load()) worker_failure.store(Cancelled);
            { std::lock_guard<std::mutex> lock(startup_mutex);go=true;startup_changed.notify_all(); }
            // Pump the helper main lane, never the Hex callback. Unsupported
            // restart/control requests cannot be silently acknowledged as applied.
            while(!done.load(std::memory_order_acquire)) {
                try {
                    instance.dispatch_host_callbacks();
                    if(instance.host_refusals()) { error(channel,&*config,HostControl);std::_Exit(HostControl); }
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
