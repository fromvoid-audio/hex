// Ordinary one-module/one-instance native worker. ABI/foreign process lifetime
// stays here. It is crash isolation for user-selected code, not a security sandbox.
#import <Foundation/Foundation.h>
#include "macos_bundle.hpp"
#include "macos_editor.hpp"
#include "macos_instance.hpp"
#include "macos_instance_wire.hpp"
#include <atomic>
#include <chrono>
#include <condition_variable>
#include <mutex>
#include <fcntl.h>
#include <poll.h>
#include <optional>
#include <os/log.h>
#include <thread>

namespace wire=hex_instance_wire;
using namespace Steinberg;using namespace Steinberg::Vst;
namespace {
void report_message_activity(const wire::Config& config,bool editor_open,
    const hex_instance::HostMessageDiagnostics& trace,uint64_t pcm_blocks,uint64_t nonzero_input_blocks) {
    static const auto log=os_log_create("org.portalsurfer.hex.vst3","messages");
    const auto count=[](const std::atomic<uint64_t>& value) {
        return static_cast<unsigned long long>(value.load(std::memory_order_relaxed));
    };
    const auto* bundle=config.root.c_str()+config.root.find_last_of('/')+1;
    os_log(log,"vst3 message diagnostic bundle=%{public}s editor=%{public}s "
        "pcm_blocks=%llu nonzero_input_blocks=%llu messages=%llu binary_attempts=%llu "
        "max_binary_bytes=%u binary_limit=%u binary_writes=%llu binary_oversize=%llu "
        "binary_reads=%llu binary_misses=%llu wrong_thread_reads=%llu",
        bundle,editor_open?"open":"closed",
        static_cast<unsigned long long>(pcm_blocks),static_cast<unsigned long long>(nonzero_input_blocks),
        count(trace.messages),count(trace.binary_attempts),trace.max_binary_bytes.load(std::memory_order_relaxed),
        static_cast<unsigned>(hex_instance::HOST_BINARY_BYTES),count(trace.binary_writes),count(trace.binary_oversize),
        count(trace.binary_reads),count(trace.binary_misses),count(trace.wrong_thread_reads));
}

enum Failure : uint32_t { Configuration=1,Admission=2,ClassFailure=3,Setup=4,Presentation=5,Capture=6,
    Processing=7,ProtocolFailure=8,Cancelled=9,Teardown=10,HostControl=11,Unexpected=12,InstanceOpen=13,Restore=14,
    Quiescence=15,Resume=16,SetupState=17,SetupConfiguration=18,SetupPrecision=19,
    SetupTopology=20,SetupArrangementRefused=21,SetupArrangementMismatch=22,
    SetupProcessing=23,SetupActivation=24,ControllerInventoryCapacity=25,
    ControllerInventoryInvalid=26 };
// Keep the worker exit code in the low byte. The error frame also carries
// the main-lane phase and refusal flags for the exact failed exchange.
constexpr uint32_t runtime_host_failure(uint32_t flags,bool feedback) {
    return 0x80000000u | (feedback?0x40000000u:0u) | ((flags&0x003FFFFFu)<<8) | HostControl;
}
// Keep existing frame shape and old stage codes. New setup reasons remain
// ordinary refusal codes for an older supervisor.
Failure setup_failure(hex_instance::ConfigurationResult result) {
    using Result=hex_instance::ConfigurationResult;
    switch(result) {
        case Result::Prepared: return Setup;
        case Result::InvalidState: return SetupState;
        case Result::InvalidConfiguration: return SetupConfiguration;
        case Result::UnsupportedPrecision: return SetupPrecision;
        case Result::UnsupportedTopology: return SetupTopology;
        case Result::ArrangementRefused: return SetupArrangementRefused;
        case Result::ArrangementMismatch: return SetupArrangementMismatch;
        case Result::SetupRefused: return SetupProcessing;
        case Result::ActivationRefused: return SetupActivation;
    }
    return Setup;
}
uint32_t controller_inventory_failure(const hex_instance::Instance& instance,uint32_t fallback) {
    const auto refused=instance.host_refusals();
    if(refused&hex_instance::MetadataCapacity) return ControllerInventoryCapacity;
    if(refused&hex_instance::InvalidMetadata) return ControllerInventoryInvalid;
    return fallback;
}
struct Refusal {}; // Expected local/SDK return-code refusal, never an exception from foreign code.
// Main-lane wait after a published failure: at most `milliseconds` per call,
// never dispatching foreign callbacks. Returns true once the peer has closed
// the socket. Unread request bytes stay for the worker; their readiness
// sleeps rather than turning the wait into a spin.
constexpr int failed_main_wait_ms=5;
// A live capture still waiting on parameter edits after this long is refused
// as still changing (HXVICR02 reason 2). It stays below the host's smallest
// capture deadline (3 s), so continuous edits never fault the lease.
constexpr auto live_settle_limit=std::chrono::milliseconds(1000);
constexpr uint32_t refusal_still_changing=2;
// Non-terminal refusal of exactly one capture operation; the epoch continues.
bool send_still_changing(int channel,const wire::Config& config,uint64_t blocks,uint64_t frames,uint64_t operation) {
    wire::Bytes body;config.stamp(body);
    wire::number(body,blocks,8);wire::number(body,frames,8);wire::number(body,operation,8);
    wire::number(body,refusal_still_changing,1);
    return wire::send(channel,"HXVICR02",body);
}
bool send_state_overflow(int channel,const wire::Config& config,uint64_t blocks,uint64_t frames,uint64_t operation,uint32_t observed) {
    wire::Bytes body;config.stamp(body);
    wire::number(body,blocks,8);wire::number(body,frames,8);wire::number(body,operation,8);
    wire::number(body,1,1);wire::number(body,observed,4);
    return wire::send(channel,"HXVICR02",body);
}
bool wait_for_peer_close(int channel) {
    pollfd descriptor{channel,POLLIN,0};
    const int ready=::poll(&descriptor,1,failed_main_wait_ms);
    if(ready==0) return false;
    if(ready>0) {
        uint8_t byte=0;const auto peer=::recv(channel,&byte,1,MSG_PEEK|MSG_DONTWAIT);
        // A closed peer with unread bytes still reports hangup.
        if(peer==0 || (descriptor.revents&POLLHUP)) return true;
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(failed_main_wait_ms));
    return false;
}
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
            digest(member(root,string(request.executable)))!=binary ||
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
        @autoreleasepool {
            if(factory) { factory->release();factory=nullptr; }
            if(entered) { if(!exit || !exit()) return false;entered=false; }
        }
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
// The SDK defines zero as its sentinel for unknown latency. Only encode it at
// the SDK boundary: the host receipt never reports it as measured frames.
std::optional<uint8_t> apply_presentation(hex_instance::Instance& instance,bool available,const wire::PresentationPlan& plan) {
    using Result=hex_instance::Instance::PresentationResult;
    if(!plan.known && !available) return 0;
    if(!plan.known && !plan.allow_unknown) return std::nullopt;
    const auto values=plan.known.value_or(wire::Presentation{});
    const auto result=instance.set_presentation_latency(values.input,values.output);
    if(!available && result==Result::Absent) return 0;
    if(available && result==Result::Applied) return plan.known?1:2;
    return std::nullopt;
}
void presentation_receipt(wire::Bytes& out,uint8_t mode,const wire::PresentationPlan& plan) {
    wire::number(out,mode,1);wire::number(out,plan.known?1:0,1);
    if(plan.known) { wire::number(out,plan.known->input,4);wire::number(out,plan.known->output,4); }
}
wire::Bytes ready_body(const wire::Config& config,const hex_instance::CapturedState& state,
    uint32 latency,uint32 tail,uint8_t presentation,const wire::PresentationPlan& plan,bool restored) {
    wire::Bytes out;config.stamp(out);out.insert(out.end(),config.cid.begin(),config.cid.end());
    uint64_t rate=0;std::memcpy(&rate,&config.rate,8);wire::number(out,rate,8);
    wire::number(out,config.maximum,2);wire::number(out,config.inputs,1);wire::number(out,config.outputs,1);
    wire::number(out,latency,4);wire::number(out,tail,4);presentation_receipt(out,presentation,plan);
    wire::number(out,restored?1:0,1);
    // State existence is not availability or an application/graph ACK.
    wire::number(out,state.component?1:0,1);auto payload=state_payload(config,state);
    wire::number(out,payload.size(),4);out.insert(out.end(),payload.begin(),payload.end());
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
    const auto* message_diagnostics=std::getenv("HEX_VST3_MESSAGE_DIAGNOSTICS");
    const bool trace_messages=message_diagnostics && !std::strcmp(message_diagnostics,"1");
    Module module;hex_instance::Instance instance;
    hex_instance::EditorWindow editor(supervised?::getppid():0);
    std::optional<wire::Config> config;
    uint32_t failure=Configuration;bool accepted=false,normal_stop=false;
    uint64_t blocks=0,frames=0;
    using CloseResult=hex_instance::Instance::CloseResult;
    auto closing=CloseResult::Refused;
    // SDK startup and teardown may autorelease objects with methods in the
    // bundle. Drain those objects before module close can unmap their code.
    @autoreleasepool {
        try {
            auto request=wire::configuration(channel);config.emplace(request);
            const auto restoration=wire::startup(channel,*config);
            failure=Admission;module.open(*config);
            failure=ClassFailure;if(!module.has_class(*config)) throw Refusal{};
            TUID cid{};std::memcpy(cid,config->cid.data(),16);
            failure=InstanceOpen;
            if(!instance.open(module.factory,cid,trace_messages)) throw Refusal{};
            // State synchronization can issue kLatencyChanged even before
            // processing. Fold it into the facts we already validate here.
            instance.observe_latency_restarts();
            if(restoration.restored()) {
                failure=Restore;
                if(!instance.restore_initial_state_on_control(restoration.state)) {
                    failure=controller_inventory_failure(instance,failure);throw Refusal{};
                }
            }
            failure=Setup;hex_instance::ProcessingConfiguration setup{};
            setup.sample_rate=config->rate;setup.maximum_frames=config->maximum;
            setup.input_channels=config->inputs;setup.output_channels=config->outputs;
            setup.input_enabled=config->inputs!=0;
            const auto configured=instance.configure(setup);
            if(configured!=hex_instance::ConfigurationResult::Prepared) {
                failure=setup_failure(configured);throw Refusal{};
            }
            // The SDK permits latency changes to become observable only after
            // reactivation. Cover startup notifications before publishing facts.
            if(instance.latency_restart()) {
                if(!instance.reactivate_on_control()) throw Refusal{};
                instance.cover_latency_restart(instance.latency_restart());
            }
            failure=HostControl;instance.dispatch_host_callbacks();
            if(instance.host_refusals()) throw Refusal{};
            auto latency=instance.reported_latency_on_control();
            auto tail=instance.reported_tail_on_control();
            auto available=instance.presentation_latency_available_on_control();
            if(!latency || !tail || !available) throw Refusal{};
            failure=ProtocolFailure;
            if(!wire::send(channel,"HXVIFA04",facts_body(*config,*latency,*tail,*available,restoration))) throw Refusal{};
            wire::PresentationPlan supplied;
            if(!wire::presentation(channel,*config,supplied)) throw Refusal{};
            failure=Presentation;
            auto presentation=apply_presentation(instance,*available,supplied);
            if(!presentation) throw Refusal{};
            failure=Capture;hex_instance::CapturedState state{};
            if(!instance.capture_initial_state_on_control(state)) {
                failure=controller_inventory_failure(instance,failure);throw Refusal{};
            }
            failure=HostControl;instance.dispatch_host_callbacks();
            if(instance.host_refusals()) throw Refusal{};
            failure=Presentation;
            if(instance.latency_restart()) {
                if(!instance.reactivate_on_control()) throw Refusal{};
                instance.cover_latency_restart(instance.latency_restart());
                presentation=apply_presentation(instance,*available,supplied);
                if(!presentation || instance.latency_restart() || instance.host_refusals()) throw Refusal{};
            }
            const auto final_latency=instance.reported_latency_on_control();
            const auto final_tail=instance.reported_tail_on_control();
            if(!final_latency || !final_tail || *final_latency!=*latency || *final_tail!=*tail) throw Refusal{}; // caller's plan became stale
            instance.finish_startup_latency_observation_on_control();
            const auto hello=ready_body(*config,state,*latency,*tail,*presentation,supplied,restoration.restored());
            auto processing_parameters=instance.processing_parameters_on_control();
            hex_instance::ParameterChanges parameter_input,parameter_output;
            hex_instance::ParameterFeedback parameter_feedback;
            std::atomic<uint64_t> diagnostic_pcm_blocks{0},diagnostic_nonzero_input_blocks{0};
            std::atomic<bool> done{false};std::atomic<uint32_t> worker_failure{0},startup{0},phase{0};
            std::mutex startup_mutex;std::condition_variable startup_changed;bool go=false;
            const auto control_failure=[&](uint32_t code) {
                // Share the epoch wait mutex so failure publication cannot
                // lose a wake between its predicate check and actual wait.
                { std::lock_guard<std::mutex> lock(startup_mutex);
                  uint32_t clear=0;worker_failure.compare_exchange_strong(clear,code); }
                startup_changed.notify_all();
            };
            // One serialized epoch mailbox, never a second request reader. The
            // processing lane is stopped and waiting at every epoch SDK step.
            // Live capture below has its own bounded nonwaiting slot in this
            // same worker. Mutex/condition waits live only in the epoch path.
            enum EpochStage { Idle,FactsRequested,FactsSent,CaptureRequested,CaptureSent,FlushRequested,FlushSent,ReactivateRequested,ReactivateSent,ResumeRequested,ResumeConfigured,Restarted,ResumeSent };
            EpochStage epoch_stage=Idle;
            uint64_t next_graph=0,next_epoch=0;
            uint64_t capture_operation=0,flush_operation=0,reactivate_operation=0;
            hex_instance::ParameterFeedback flushed_values;
            bool flush_applied=false;
            wire::PresentationPlan next_presentation;
            // Single live capture slot. Main exclusively writes its result
            // after Requested/acquire, then Complete/release transfers it back
            // to the original processing reader. No SDK wait, extra reader or
            // concurrent socket writer is added to the PCM path.
            enum LiveCaptureStage { LiveIdle,LiveFeedbackPending,LiveRequested,LiveComplete,LiveOverflow,LiveFailed };
            std::atomic<LiveCaptureStage> live_stage{LiveIdle};
            hex_instance::CapturedState live_state{};
            uint32_t live_overflow=0;
            uint32_t live_failure=0;
            // One main-produced copied page. Processing only publishes/polls
            // this request and never waits for the SDK getter or shares a
            // controller pointer. This bounded slot is separate from Host.
            enum PageStage {PageIdle,PageFeedbackPending,PageRequested,PageComplete,PageFailed};
            std::atomic<PageStage> page_stage{PageIdle};
            hex_instance::ControllerQuery page_query{};
            hex_instance::ControllerParameterPage parameter_page{};
            hex_instance::EditorState editor_state=hex_instance::EditorState::Closed;
            uint32_t page_failure=0;
            std::thread worker([&] {
                bool started=false;
                try {
                    started=instance.start_processing();if(!started) worker_failure.store(Processing);
                    startup.store(started?1:2,std::memory_order_release);
                    { std::unique_lock<std::mutex> lock(startup_mutex);startup_changed.wait(lock,[&]{return go;}); }
                    struct CompletedEdit {hex_instance::EditEvent edit{};uint64_t first_frame=0;};
                    std::array<CompletedEdit,hex_instance::HOST_EDIT_COUNT> completed_edits{};
                    size_t completed_edit_count=0;uint64_t edit_query_operation=0;
                    const auto complete_edit=[&](const hex_instance::EditEvent& event) {
                        completed_edits[completed_edit_count++]={event,frames};instance.complete_parameter_edit();
                    };
                    const auto drain_edit_boundaries=[&] {
                        // Begin/end alone do not need an invented audio call.
                        hex_instance::EditEvent event{};
                        while(completed_edit_count<completed_edits.size() && instance.peek_parameter_edit(event) && event.kind!=hex_instance::Perform) {
                            instance.consume_parameter_edit();complete_edit(event);
                        }
                    };
                    const auto read_edit_receipts=[&](const wire::Packet& packet) {
                        if(edit_query_operation==std::numeric_limits<uint64_t>::max() || packet.capture_operation!=edit_query_operation+1) return false;
                        if(packet.observe_latency) instance.observe_latency_restarts();
                        drain_edit_boundaries();
                        wire::Bytes body;config->stamp(body);wire::number(body,blocks,8);wire::number(body,frames,8);
                        const auto ready=instance.ready_parameter_edits();
                        const auto pending=instance.pending_parameter_edits();
                        wire::number(body,packet.capture_operation,8);wire::number(body,pending,2);wire::number(body,ready,2);wire::number(body,completed_edit_count,2);
                        for(size_t n=0;n<completed_edit_count;++n) {
                            const auto& record=completed_edits[n];const auto& value=record.edit;
                            wire::number(body,value.sequence,8);wire::number(body,value.gesture,8);
                            uint64_t bits=0;std::memcpy(&bits,&value.normalized,8);wire::number(body,bits,8);
                            wire::number(body,value.id,4);wire::number(body,value.kind,4);wire::number(body,record.first_frame,8);
                        }
                        if(packet.observe_latency) wire::number(body,instance.latency_restart(),8);
                        if(!wire::send(channel,packet.observe_latency?"HXVIGE02":"HXVIGE01",body)) return false;
                        ++edit_query_operation;completed_edit_count=0;return true;
                    };
                    std::array<hex_instance::EditEvent,hex_instance::HOST_EDIT_COUNT> applying_edits{};size_t applying_edit_count=0;
                    const auto last_editor_edit=[&](ParamID id) {
                        for(size_t n=applying_edit_count;n>0;--n)
                            if(applying_edits[n-1].kind==hex_instance::Perform && applying_edits[n-1].id==id) return n-1;
                        return applying_edit_count;
                    };
                    const auto staged_editor_sequence=[&](ParamID id)->uint64 {
                        const auto index=last_editor_edit(id);
                        return index<applying_edit_count?applying_edits[index].sequence:0;
                    };
                    const auto stage_edits=[&](size_t later_points=0) {
                        applying_edit_count=0;hex_instance::EditEvent event{};
                        while(completed_edit_count+applying_edit_count<completed_edits.size() && instance.peek_parameter_edit(event)) {
                            if(event.kind==hex_instance::Perform) {
                                const auto* info=processing_parameters.find(event.id);
                                if(!info || (info->flags&(ParameterInfo::kIsReadOnly|ParameterInfo::kIsHidden))) return false;
                                bool duplicate=false;
                                for(size_t n=0;n<parameter_input.size();++n) {
                                    hex_instance::ParameterPoint point{};if(!parameter_input.point(n,point)) return false;
                                    if(point.id==event.id && point.offset==0) duplicate=true;
                                }
                                if(parameter_input.size()+later_points>=hex_instance::PARAMETER_POINT_COUNT && !duplicate) break;
                                if(!parameter_input.input(event.id,0,event.normalized)) return false;
                            }
                            applying_edits[applying_edit_count++]=event;instance.consume_parameter_edit();
                        }
                        return true;
                    };
                    const auto flush_idle_editor=[&] {
                        // Leave a full receipt batch for its reader to consume.
                        // The next poll can apply another fixed-size batch.
                        if(completed_edit_count==completed_edits.size() || !instance.ready_parameter_edits()) return true;
                        instance.refresh_processing_parameters(processing_parameters);
                        if(!parameter_input.reset(0,false) || !parameter_output.reset(0,true) || !stage_edits()) return false;
                        if(parameter_input.size()) {
                            if(!parameter_input.begin_sdk() || !parameter_output.begin_sdk()) {
                                error(channel,&*config,Processing);std::_Exit(Processing);
                            }
                            ProcessData flush{};flush.processMode=kRealtime;flush.symbolicSampleSize=kSample32;
                            flush.inputParameterChanges=&parameter_input;flush.outputParameterChanges=&parameter_output;
                            phase.store(2,std::memory_order_release);
                            const auto result=instance.flush_idle(flush);
                            const bool input_returned=parameter_input.end_sdk(),output_returned=parameter_output.end_sdk();
                            if(!input_returned || !output_returned) {error(channel,&*config,Processing);std::_Exit(Processing);}
                            if(result!=hex_instance::FlushResult::Applied) return false;
                            // All offsets are zero. Mirror the processor's
                            // returned value over the input value for each ID.
                            for(const auto* changes : {&parameter_input,&parameter_output}) {
                                for(size_t n=0;n<changes->size();++n) {
                                    hex_instance::ParameterPoint point{};
                                    if(!changes->point(n,point) || !processing_parameters.find(point.id)) return false;
                                    if(!parameter_feedback.put(point.id,point.normalized,staged_editor_sequence(point.id))) return false;
                                }
                            }
                            instance.publish_parameter_feedback(parameter_feedback);
                        }
                        for(size_t n=0;n<applying_edit_count;++n) complete_edit(applying_edits[n]);
                        return true;
                    };
                    uint64_t page_operation=0;
                    const auto page_reply=[&](const wire::Packet& request,bool poll) {
                        auto state=page_stage.load(std::memory_order_acquire);
                        if(!poll) {
                            if(state!=PageIdle || page_operation==std::numeric_limits<uint64_t>::max() ||
                                request.capture_operation!=page_operation+1) return false;
                            page_query=request.parameter_query;
                            // A previous PCM result may still be waiting for
                            // the main-lane feedback slot. Publish it before
                            // allowing the controller to answer this query.
                            ++page_operation;page_stage.store(parameter_feedback.count?PageFeedbackPending:PageRequested,std::memory_order_release);
                        } else if(state==PageIdle || request.capture_operation!=page_operation ||
                                  !(request.parameter_query==page_query)) return false;
                        if(state==PageFailed) {worker_failure.store(page_failure);return false;}
                        const bool complete=poll && state==PageComplete;
                        wire::Bytes body;config->stamp(body);wire::number(body,blocks,8);wire::number(body,frames,8);
                        wire::number(body,page_operation,8);
                        if(complete && hex_instance::editor_request(page_query.kind)) {
                            wire::number(body,4,1);
                            wire::number(body,static_cast<uint8_t>(editor_state),1);
                        } else if(complete) {
                            wire::number(body,parameter_page.changed?2:(parameter_page.conversion?3:1),1);
                            wire::number(body,parameter_page.revision,8);wire::number(body,parameter_page.total,2);
                            if(!parameter_page.changed && parameter_page.conversion) {
                                const auto& value=parameter_page.converted;
                                wire::number(body,value.id,4);wire::number(body,value.status,1);
                                uint64_t bits=0;std::memcpy(&bits,&value.normalized,8);wire::number(body,bits,8);
                                const auto kind=std::isnan(value.plain)?3:(std::isinf(value.plain)?(value.plain<0?1:2):0);
                                wire::number(body,kind,1);
                                const double plain=kind?0:value.plain;std::memcpy(&bits,&plain,8);wire::number(body,bits,8);
                                wire::number(body,value.formatted?1:0,1);
                                for(const auto unit:value.display) wire::number(body,static_cast<uint16_t>(unit),2);
                            } else if(!parameter_page.changed) {
                                wire::number(body,parameter_page.first,2);wire::number(body,parameter_page.count,1);
                                const auto number=[&](double value) {
                                    uint64_t bits=0;std::memcpy(&bits,&value,8);wire::number(body,bits,8);
                                };
                                const auto plain=[&](double value) {
                                    const auto kind=std::isnan(value)?3:(std::isinf(value)?(value<0?1:2):0);
                                    wire::number(body,kind,1);number(kind?0:value);
                                };
                                const auto text=[&](const String128& value) {
                                    for(const auto unit:value) wire::number(body,static_cast<uint16>(unit),2);
                                };
                                for(uint16 n=0;n<parameter_page.count;++n) {
                                    const auto& value=parameter_page.parameters[n];const auto& info=value.info;
                                    wire::number(body,info.id,4);wire::number(body,static_cast<uint32>(info.flags),4);
                                    wire::number(body,static_cast<uint32>(info.stepCount),4);wire::number(body,static_cast<uint32>(info.unitId),4);
                                    number(value.normalized);number(info.defaultNormalizedValue);
                                    plain(value.plain);plain(value.default_plain);
                                    text(info.title);text(info.shortTitle);text(info.units);
                                    wire::number(body,(value.formatted?1:0)|(value.default_formatted?2:0),1);
                                    text(value.display);text(value.default_display);
                                }
                            }
                        } else wire::number(body,0,1);
                        if(!wire::send(channel,"HXVIMD01",body)) return false;
                        if(complete) page_stage.store(PageIdle,std::memory_order_release);
                        return true;
                    };
                    wire::Packet packet{};
                    hex_instance::InputNotes input_notes;
                    hex_instance::HeldInputNotes held_notes;
                    std::array<float,wire::max_frames> left{},right{},output_left{},output_right{};
                    float* input_pointers[]={left.data(),right.data()};float* output_pointers[]={output_left.data(),output_right.data()};
                    AudioBusBuffers input{},output{};
                    input.numChannels=instance.configuration()->input_channels;output.numChannels=config->outputs;
                    const auto input_silence=config->inputs?0u:(uint64_t{1}<<input.numChannels)-1;
                    input.channelBuffers32=input_pointers;output.channelBuffers32=output_pointers;
                    ProcessData data{};data.processMode=kRealtime;data.symbolicSampleSize=kSample32;
                    uint64_t live_operation=0;
                    auto live_started=std::chrono::steady_clock::now();
                    data.numInputs=input.numChannels?1:0;data.numOutputs=1;data.inputs=data.numInputs?&input:nullptr;data.outputs=&output;
                    const auto release_held_notes=[&] {
                        const auto count=held_notes.release_into(packet.notes);
                        if(!count) return true;
                        instance.refresh_processing_parameters(processing_parameters);
                        if(!parameter_output.reset(1,true) || !parameter_output.begin_sdk() ||
                            !input_notes.begin(packet.notes,count)) return false;
                        left[0]=0;right[0]=0;output_left[0]=0;output_right[0]=0;
                        input.silenceFlags=input_silence;output.silenceFlags=0;
                        ProcessContext stopped{};stopped.sampleRate=config->rate;
                        auto release=data;
                        release.numSamples=1;release.processContext=&stopped;
                        release.inputEvents=&input_notes;
                        release.inputParameterChanges=nullptr;release.outputParameterChanges=&parameter_output;
                        const bool processed=instance.process(release);
                        const bool parameters_returned=parameter_output.end_sdk(), notes_returned=input_notes.end();
                        if(!processed || !parameters_returned || !notes_returned) return false;
                        for(size_t index=0;index<parameter_output.size();++index) {
                            hex_instance::ParameterPoint point{};
                            if(!parameter_output.point(index,point) || !processing_parameters.find(point.id) ||
                                !parameter_feedback.put(point.id,point.normalized)) return false;
                        }
                        return true;
                    };
                    // A live SDK failure waits only for the next serialized
                    // request to report its error, without another DSP call.
                    // This keeps the failure reply paired with that exchange
                    // instead of racing a later write against child exit.
                    while(started && (!worker_failure.load() || live_stage.load(std::memory_order_acquire)==LiveFailed || page_stage.load(std::memory_order_acquire)==PageFailed)) {
                        phase.store(1,std::memory_order_release); // receiving a complete frame
                        const auto received=packet.read(channel,*config,blocks,frames);
                        if(worker_failure.load()) break;
                        // A late End is not DSP work. Retire its pending
                        // custody even when the next request is save capture;
                        // retain the ordered receipt until its consumer reads.
                        drain_edit_boundaries();
                        instance.publish_parameter_feedback(parameter_feedback);
                        if(page_stage.load(std::memory_order_acquire)==PageFeedbackPending && !parameter_feedback.count)
                            page_stage.store(PageRequested,std::memory_order_release);
                        if(live_stage.load(std::memory_order_acquire)==LiveFeedbackPending && !parameter_feedback.count && !instance.pending_parameter_edits())
                            live_stage.store(LiveRequested,std::memory_order_release);
                        if(received==10 || received==11) {
                            if(!page_reply(packet,received==11)) {if(!worker_failure.load())worker_failure.store(ProtocolFailure);break;}
                            continue;
                        }
                        if(received==9) {
                            if(packet.flush_idle_edits && !flush_idle_editor()) {worker_failure.store(Processing);break;}
                            if(!read_edit_receipts(packet)) {worker_failure.store(ProtocolFailure);break;}
                            continue;
                        }
                        if(received==6 || received==7) {
                            const auto stage=live_stage.load(std::memory_order_acquire);
                            if(received==6) {
                                if(stage!=LiveIdle || live_operation==std::numeric_limits<uint64_t>::max() ||
                                    packet.capture_operation!=live_operation+1) { worker_failure.store(ProtocolFailure);break; }
                                live_operation=packet.capture_operation;live_started=std::chrono::steady_clock::now();
                                live_stage.store(parameter_feedback.count || instance.pending_parameter_edits()?LiveFeedbackPending:LiveRequested,std::memory_order_release);
                            } else if(stage==LiveIdle || packet.capture_operation!=live_operation) {
                                worker_failure.store(ProtocolFailure);break;
                            }
                            if(received==7 && stage==LiveFailed) { worker_failure.store(live_failure);break; }
                            if(received==7 && stage==LiveOverflow) {
                                if(!send_state_overflow(channel,*config,blocks,frames,live_operation,live_overflow)) std::_Exit(Cancelled);
                                live_stage.store(LiveIdle,std::memory_order_release);
                                continue;
                            }
                            // Only this worker leaves FeedbackPending, and main
                            // never touches it, so retiring here cannot race a capture.
                            auto pending=LiveFeedbackPending;
                            if(received==7 && stage==LiveFeedbackPending &&
                                std::chrono::steady_clock::now()-live_started>=live_settle_limit &&
                                live_stage.compare_exchange_strong(pending,LiveIdle,std::memory_order_acq_rel)) {
                                if(!send_still_changing(channel,*config,blocks,frames,live_operation)) std::_Exit(Cancelled);
                                continue;
                            }
                            const bool complete=received==7 && stage==LiveComplete;
                            wire::Bytes body;config->stamp(body);
                            wire::number(body,blocks,8);wire::number(body,frames,8);wire::number(body,live_operation,8);
                            wire::number(body,complete?1:0,1);
                            const auto payload=complete?state_payload(*config,live_state):wire::Bytes{};
                            wire::number(body,payload.size(),4);body.insert(body.end(),payload.begin(),payload.end());
                            if(!wire::send(channel,"HXVLCA02",body)) std::_Exit(Cancelled);
                            if(complete) live_stage.store(LiveIdle,std::memory_order_release);
                            continue;
                        }
                        if((received==2 || received==3) && live_stage.load(std::memory_order_acquire)!=LiveIdle) {
                            worker_failure.store(ProtocolFailure);break;
                        }
                        if(received==2) { normal_stop=true;break; }
                        if(received==0) std::_Exit(Cancelled); // EOF never claims cooperative close
                        if(received==3) {
                            if(packet.release_gates) {
                                // Priority Stop already dropped the old callback's output.
                                // Deliver the accepted gates' releases on this lane, with no
                                // returned audio or change to the copied PCM prefix.
                                phase.store(2,std::memory_order_release);
                                if(!release_held_notes()) { error(channel,&*config,Processing);std::_Exit(Processing); }
                            }
                            // Main's epoch handshake drains the last worker-owned
                            // observations before state capture or resume.
                            phase.store(3,std::memory_order_release);
                            if(!instance.stop_processing()) { error(channel,&*config,Quiescence);std::_Exit(Quiescence); }
                            started=false;
                            {
                                std::unique_lock<std::mutex> lock(startup_mutex);
                                epoch_stage=FactsRequested;startup_changed.notify_all();
                                startup_changed.wait(lock,[&]{return epoch_stage==FactsSent || worker_failure.load();});
                            }
                            if(worker_failure.load()) break;
                            phase.store(1,std::memory_order_release);
                            auto resumed=packet.read(channel,*config,blocks,frames);
                            uint64_t captures=0,flushes=0,reactivations=0;
                            while(resumed==5 || resumed==8 || resumed==9 || resumed==10 || resumed==11 || resumed==12) {
                                if(resumed==12) {
                                    if(reactivations==std::numeric_limits<uint64_t>::max() || packet.capture_operation!=reactivations+1) {
                                        worker_failure.store(ProtocolFailure);break;
                                    }
                                    phase.store(3,std::memory_order_release);
                                    {
                                        std::unique_lock<std::mutex> lock(startup_mutex);
                                        reactivate_operation=packet.capture_operation;epoch_stage=ReactivateRequested;
                                        startup_changed.notify_all();
                                        startup_changed.wait(lock,[&]{return epoch_stage==ReactivateSent || worker_failure.load();});
                                    }
                                    if(worker_failure.load()) break;
                                    ++reactivations;phase.store(1,std::memory_order_release);
                                    resumed=packet.read(channel,*config,blocks,frames);continue;
                                }
                                if(resumed==10 || resumed==11) {
                                    if(!page_reply(packet,resumed==11)) {if(!worker_failure.load())worker_failure.store(ProtocolFailure);break;}
                                    resumed=packet.read(channel,*config,blocks,frames);continue;
                                }
                                if(resumed==9) {
                                    if(packet.flush_idle_edits || !read_edit_receipts(packet)) {worker_failure.store(ProtocolFailure);break;}
                                    resumed=packet.read(channel,*config,blocks,frames);continue;
                                }
                                if(resumed==8) {
                                    if(flushes==std::numeric_limits<uint64_t>::max() || packet.capture_operation!=flushes+1) { worker_failure.store(ProtocolFailure);break; }
                                    instance.refresh_processing_parameters(processing_parameters);
                                    if(!parameter_input.reset(0,false) || !parameter_output.reset(0,true)) { error(channel,&*config,Processing);std::_Exit(Processing); }
                                    bool valid=true;
                                    for(size_t n=0;n<packet.parameter_count;++n) {
                                        const auto& point=packet.parameters[n];const auto* info=processing_parameters.find(point.id);
                                        if(!info || (info->flags&(ParameterInfo::kIsReadOnly|ParameterInfo::kIsHidden)) ||
                                            !parameter_input.input(point.id,0,point.normalized)) valid=false;
                                    }
                                    if(!valid || !stage_edits()) { worker_failure.store(ProtocolFailure);break; }
                                    if(!parameter_input.begin_sdk() || !parameter_output.begin_sdk()) { error(channel,&*config,Processing);std::_Exit(Processing); }
                                    ProcessData flush{};flush.processMode=kRealtime;flush.symbolicSampleSize=kSample32;
                                    flush.inputParameterChanges=parameter_input.size()?&parameter_input:nullptr;flush.outputParameterChanges=&parameter_output;
                                    phase.store(2,std::memory_order_release);
                                    const auto result=instance.flush_stopped(flush);
                                    const bool input_returned=parameter_input.end_sdk(),output_returned=parameter_output.end_sdk();
                                    if(!input_returned || !output_returned) { error(channel,&*config,Processing);std::_Exit(Processing); }
                                    if(result!=hex_instance::FlushResult::Applied && result!=hex_instance::FlushResult::Empty) { worker_failure.store(Processing);break; }
                                    flushed_values={};
                                    for(size_t n=0;n<parameter_input.size();++n) {
                                        hex_instance::ParameterPoint point{};
                                        valid=parameter_input.point(n,point) && flushed_values.put(point.id,point.normalized) && valid;
                                    }
                                    for(size_t n=0;n<parameter_output.size();++n) {
                                        hex_instance::ParameterPoint point{};
                                        if(!parameter_output.point(n,point) || !processing_parameters.find(point.id) || !flushed_values.put(point.id,point.normalized)) valid=false;
                                    }
                                    if(!valid) { worker_failure.store(ProtocolFailure);break; }
                                    for(size_t n=0;n<applying_edit_count;++n) complete_edit(applying_edits[n]);
                                    {
                                        std::unique_lock<std::mutex> lock(startup_mutex);
                                        flush_operation=packet.capture_operation;flush_applied=result==hex_instance::FlushResult::Applied;epoch_stage=FlushRequested;
                                        startup_changed.notify_all();startup_changed.wait(lock,[&]{return epoch_stage==FlushSent || worker_failure.load();});
                                    }
                                    if(worker_failure.load()) break;
                                    ++flushes;phase.store(1,std::memory_order_release);resumed=packet.read(channel,*config,blocks,frames);continue;
                                }
                                if(captures==std::numeric_limits<uint64_t>::max() || packet.capture_operation!=captures+1) {
                                    worker_failure.store(ProtocolFailure);break;
                                }
                                {
                                    std::unique_lock<std::mutex> lock(startup_mutex);
                                    capture_operation=packet.capture_operation;epoch_stage=CaptureRequested;
                                    startup_changed.notify_all();
                                    startup_changed.wait(lock,[&]{return epoch_stage==CaptureSent || worker_failure.load();});
                                }
                                if(worker_failure.load()) break;
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
                                startup_changed.wait(lock,[&]{return epoch_stage==ResumeConfigured || worker_failure.load();});
                            }
                            if(worker_failure.load()) break;
                            phase.store(2,std::memory_order_release);
                            started=instance.start_processing();
                            if(!started) { worker_failure.store(Resume);break; }
                            {
                                std::unique_lock<std::mutex> lock(startup_mutex);
                                epoch_stage=Restarted;startup_changed.notify_all();
                                startup_changed.wait(lock,[&]{return epoch_stage==ResumeSent || worker_failure.load();});
                            }
                            if(worker_failure.load()) break;
                            continue;
                        }
                        if(received!=1) { worker_failure.store(ProtocolFailure);break; }
                        bool valid=true;output_right.fill(0);input.silenceFlags=input_silence;output.silenceFlags=0;
                        bool nonzero_input=false;
                        for(size_t n=0;n<packet.frames;++n) {
                            left[n]=wire::sample(packet.bytes.data()+wire::header_size+n*8);
                            right[n]=wire::sample(packet.bytes.data()+wire::header_size+n*8+4);
                            valid=valid && (config->inputs==2 || right[n]==0) && (config->inputs!=0 || left[n]==0);
                            if(trace_messages) nonzero_input|=left[n]!=0 || right[n]!=0;
                        }
                        const auto event_channels=instance.event_input_channels();
                        // The wire has already validated the notes. A generator
                        // with no event bus still renders with inputEvents=null.
                        if(event_channels) {
                            for (size_t n=0;n<packet.note_count;++n)
                                valid=valid && packet.notes[n].channel<event_channels;
                        }
                        if(!valid) { worker_failure.store(ProtocolFailure);break; }
                        instance.refresh_processing_parameters(processing_parameters);
                        for(size_t n=0;n<packet.parameter_count;++n) {
                            const auto* info=processing_parameters.find(packet.parameters[n].id);
                            if(!info || (info->flags&(ParameterInfo::kIsReadOnly|ParameterInfo::kIsHidden))) valid=false;
                        }
                        if(!valid || (event_channels && !held_notes.stage(packet.notes,packet.note_count))) {
                            worker_failure.store(ProtocolFailure);break;
                        }
                        // Each transport context starts an SDK call, but the
                        // copied exchange and its reply remain one quantum.
                        packet.reply_parameter_count=0;
                        size_t next_note=0,next_parameter=0;
                        std::array<bool,hex_instance::HOST_EDIT_COUNT> editor_value_replaced{};
                        for(size_t slice=0;slice<=packet.timing_change_count;++slice) {
                            const size_t first=slice?packet.timing_changes[slice-1].offset:0;
                            const size_t end=slice<packet.timing_change_count?packet.timing_changes[slice].offset:packet.frames;
                            const auto width=static_cast<int32_t>(end-first);
                            if(!parameter_input.reset(width,false) || !parameter_output.reset(width,true)) {
                                error(channel,&*config,Processing);std::_Exit(Processing);
                            }
                            while(next_parameter<packet.parameter_count && static_cast<size_t>(packet.parameters[next_parameter].offset)<end) {
                                const auto& point=packet.parameters[next_parameter++];
                                if(!parameter_input.input(point.id,point.offset-static_cast<int32_t>(first),point.normalized)) valid=false;
                            }
                            if(!valid || (!slice && !stage_edits(packet.parameter_count-next_parameter))) {
                                worker_failure.store(ProtocolFailure);break;
                            }
                            const size_t first_note=next_note;
                            while(next_note<packet.note_count && static_cast<size_t>(packet.notes[next_note].offset)<end)
                                ++next_note;
                            if(!parameter_input.begin_sdk() || !parameter_output.begin_sdk() ||
                                (event_channels && !input_notes.begin(packet.notes,next_note-first_note,
                                    first_note,static_cast<int32_t>(first)))) {
                                error(channel,&*config,Processing);std::_Exit(Processing);
                            }
                            float* slice_inputs[]={left.data()+first,right.data()+first};
                            float* slice_outputs[]={output_left.data()+first,output_right.data()+first};
                            auto slice_input=input,slice_output=output;
                            slice_input.channelBuffers32=slice_inputs;slice_output.channelBuffers32=slice_outputs;
                            auto window=data;
                            window.inputs=window.numInputs?&slice_input:nullptr;window.outputs=&slice_output;
                            window.inputParameterChanges=parameter_input.size()?&parameter_input:nullptr;
                            window.outputParameterChanges=&parameter_output;
                            window.inputEvents=event_channels?&input_notes:nullptr;
                            window.processContext=slice?&packet.timing_changes[slice-1].context:(packet.context?&*packet.context:nullptr);
                            window.numSamples=width;phase.store(2,std::memory_order_release);
                            const bool processed=instance.process(window);
                            // No SDK stop/unload beneath a retained borrowed queue.
                            const bool input_returned=parameter_input.end_sdk(),output_returned=parameter_output.end_sdk();
                            const bool notes_returned=!event_channels || input_notes.end();
                            if(!input_returned || !output_returned || !notes_returned) {
                                error(channel,&*config,Processing);std::_Exit(Processing);
                            }
                            if(!processed) { worker_failure.store(Processing);break; }
                            // Mirror the latest chronological value per ID. DSP
                            // feedback wins a tie, never a later host point.
                            size_t incoming=0,outgoing=0;
                            while(incoming<parameter_input.size() || outgoing<parameter_output.size()) {
                                hex_instance::ParameterPoint output_point{},input_point{};
                                if(incoming<parameter_input.size() && !parameter_input.point(incoming,input_point)) {valid=false;break;}
                                if(outgoing<parameter_output.size() && !parameter_output.point(outgoing,output_point)) {
                                    valid=false;break;
                                }
                                const bool use_input=incoming<parameter_input.size() &&
                                    (outgoing==parameter_output.size() || input_point.offset<=output_point.offset);
                                const auto point=use_input?input_point:output_point;
                                if(use_input) ++incoming;else ++outgoing;
                                const auto edit=last_editor_edit(point.id);
                                uint64 editor_sequence=0;
                                if(edit<applying_edit_count) {
                                    // Editor input is staged at quantum offset zero. A later
                                    // authored point takes ownership, including across slices.
                                    if(use_input && (first || point.offset)) editor_value_replaced[edit]=true;
                                    if(!editor_value_replaced[edit]) editor_sequence=applying_edits[edit].sequence;
                                }
                                if(!processing_parameters.find(point.id) ||
                                    !parameter_feedback.put(point.id,point.normalized,editor_sequence)) valid=false;
                            }
                            if(!valid || !packet.append_parameter_reply(first,parameter_output)) {
                                worker_failure.store(ProtocolFailure);break;
                            }
                        }
                        if(worker_failure.load()) break;
                        for(size_t n=0;n<packet.frames;++n) {
                            wire::sample(packet.bytes.data()+wire::header_size+n*8,output_left[n]);
                            wire::sample(packet.bytes.data()+wire::header_size+n*8+4,output_right[n]);
                        }
                        for(size_t n=0;n<applying_edit_count;++n) complete_edit(applying_edits[n]);
                        instance.publish_parameter_feedback(parameter_feedback);
                        if(!packet.write(channel)) { worker_failure.store(Cancelled);break; }
                        ++blocks;frames+=packet.frames;
                        if(trace_messages) {
                            diagnostic_pcm_blocks.store(blocks,std::memory_order_relaxed);
                            if(nonzero_input) diagnostic_nonzero_input_blocks.fetch_add(1,std::memory_order_relaxed);
                        }
                    }
                    if(worker_failure.load()) error(channel,&*config,worker_failure.load());
                    // Never stop/deactivate/unload beneath a main-lane capture,
                    // even on malformed PCM, EOF or a foreign processing fault.
                    if(live_stage.load(std::memory_order_acquire)!=LiveIdle || page_stage.load(std::memory_order_acquire)!=PageIdle)
                        std::_Exit(worker_failure.load()?(worker_failure.load()&0xFF):Cancelled);
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
            accepted=startup.load(std::memory_order_acquire)==1 && !worker_failure.load() && wire::send(channel,"HXVIRD05",hello);
            if(!accepted && !worker_failure.load()) worker_failure.store(Cancelled);
            { std::lock_guard<std::mutex> lock(startup_mutex);go=true;startup_changed.notify_all(); }
            // Pump the helper main lane, never the Hex callback. Unsupported
            // restart/control requests cannot be silently acknowledged as applied.
            // Foreign timers and controller calls can also autorelease work
            // on turns that do not visit the editor's AppKit event pump.
            auto next_message_report=std::chrono::steady_clock::time_point::min();
            while(!done.load(std::memory_order_acquire)) @autoreleasepool {
                try {
                    if(trace_messages) {
                        const auto now=std::chrono::steady_clock::now();
                        if(now>=next_message_report) {
                            if(const auto* trace=instance.message_diagnostics_on_control())
                                report_message_activity(*config,editor.is_open(),*trace,
                                    diagnostic_pcm_blocks.load(std::memory_order_relaxed),
                                    diagnostic_nonzero_input_blocks.load(std::memory_order_relaxed));
                            next_message_report=now+std::chrono::seconds(2);
                        }
                    }
                    if(worker_failure.load()) {
                        // The joining worker/guardian owns terminal progress.
                        // Do not dispatch more foreign callbacks after failure.
                        // Block briefly instead of spinning, and retire on EOF
                        // unless the worker itself is receiving (it then sees
                        // EOF and finishes); a hung DSP/stop never returns.
                        if(wait_for_peer_close(channel) && phase.load(std::memory_order_acquire)!=1 &&
                            !done.load(std::memory_order_acquire)) std::_Exit(Cancelled);
                        continue;
                    }
                    // The worker owns active response framing. Publishing a
                    // main-lane failure lets it finish any current response
                    // before sending the terminal error; a direct main write
                    // could interleave an error header/body with PCM/receipts.
                    if(!instance.synchronize_parameter_feedback_on_control()) {
                        control_failure(runtime_host_failure(instance.host_refusals(),true));continue;
                    }
                    instance.dispatch_host_callbacks();
                    if(const auto flags=instance.host_refusals()) {
                        control_failure(runtime_host_failure(flags,false));continue;
                    }
                    if(page_stage.load(std::memory_order_acquire)==PageRequested) {
                        page_failure=0;
                        try {
                            // Acquire the exact prior worker publication even
                            // if it arrived after this main iteration's sync.
                            if(!instance.synchronize_parameter_feedback_on_control()) page_failure=HostControl;
                            else if(hex_instance::editor_request(page_query.kind)) {
                                @autoreleasepool {
                                    const bool was_open=editor.is_open();
                                    if(!editor.close()) page_failure=Teardown;
                                    else if(was_open || page_query.kind==hex_instance::CLOSE_EDITOR)
                                        editor_state=hex_instance::EditorState::Closed;
                                    else if(const auto result=editor.open(instance.create_editor_on_control())) editor_state=*result;
                                    else page_failure=Teardown;
                                }
                            } else if(!instance.parameter_page_on_control(page_query,parameter_page)) page_failure=HostControl;
                        } catch(...) {
                            if(hex_instance::editor_request(page_query.kind)) editor.quarantine();
                            page_failure=Unexpected;
                        }
                        page_stage.store(page_failure?PageFailed:PageComplete,std::memory_order_release);
                        if(page_failure) control_failure(page_failure);
                    }
                    if(worker_failure.load()) continue;
                    if(live_stage.load(std::memory_order_acquire)==LiveRequested) {
                        // Admission observes only that instant's queue. Main
                        // callbacks may arrive before or during SDK capture.
                        // Discard that staged snapshot and retry after actual
                        // processing, never publish pre-edit component state.
                        if(instance.pending_parameter_edits()) {
                            live_stage.store(LiveFeedbackPending,std::memory_order_release);
                        } else {
                            const auto admission_revision=instance.host_callback_revision_on_control();
                            try {
                                const auto captured=instance.synchronize_parameter_feedback_on_control()?
                                    instance.capture_current_state_on_control(live_state,live_overflow):hex_instance::Instance::CaptureResult::Refused;
                                instance.dispatch_host_callbacks();
                                live_failure=captured!=hex_instance::Instance::CaptureResult::Refused && !instance.host_refusals()?0:Capture;
                                if(!live_failure && captured==hex_instance::Instance::CaptureResult::Overflow &&
                                    !instance.pending_parameter_edits() && instance.host_callback_revision_on_control()==admission_revision) {
                                    live_stage.store(LiveOverflow,std::memory_order_release);
                                } else {
                                    live_stage.store(live_failure?LiveFailed:
                                        (instance.pending_parameter_edits() || instance.host_callback_revision_on_control()!=admission_revision?
                                            LiveFeedbackPending:LiveComplete),std::memory_order_release);
                                }
                            } catch(...) { live_failure=Unexpected; }
                            if(live_failure) live_stage.store(LiveFailed,std::memory_order_release);
                        }
                        if(live_failure) {
                            // Refusal/exception cannot admit another DSP call.
                            // The original worker reports the failure at its
                            // current exchange boundary, then exits without
                            // stop/unload beneath the failed capture owner.
                            control_failure(live_failure);
                        }
                    }
                    if(worker_failure.load()) continue;
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
                        if(!wire::send(channel,"HXVIQF03",body)) std::_Exit(Cancelled);
                        std::lock_guard<std::mutex> lock(startup_mutex);
                        epoch_stage=FactsSent;startup_changed.notify_all();
                    } else if(stage==ReactivateRequested) {
                        try {
                            const auto covered=instance.latency_restart();
                            if(!instance.reactivate_on_control()) { error(channel,&*config,Quiescence);std::_Exit(Quiescence); }
                            latency=instance.reported_latency_on_control();
                            tail=instance.reported_tail_on_control();
                            available=instance.presentation_latency_available_on_control();
                            if(!latency || !tail || !available || instance.host_refusals()) {
                                error(channel,&*config,Quiescence);std::_Exit(Quiescence);
                            }
                            instance.cover_latency_restart(covered);
                            auto body=facts_body(*config,*latency,*tail,*available,restoration);
                            wire::number(body,blocks,8);wire::number(body,frames,8);wire::number(body,reactivate_operation,8);
                            if(!wire::send(channel,"HXVIAR02",body)) std::_Exit(Cancelled);
                        } catch(...) {
                            // Foreign activation/facts have unknown side effects.
                            // Never retry teardown/release/unload beneath them.
                            error(channel,&*config,Unexpected);std::_Exit(Unexpected);
                        }
                        std::lock_guard<std::mutex> lock(startup_mutex);
                        epoch_stage=ReactivateSent;startup_changed.notify_all();
                    } else if(stage==FlushRequested) {
                        // The original worker is stopped and waiting at this
                        // barrier. Synchronize controller values before ACK;
                        // neither flush nor mirror advances the PCM prefix.
                        auto pending=flushed_values;
                        if(!instance.publish_parameter_feedback(pending) || !instance.synchronize_parameter_feedback_on_control()) {
                            error(channel,&*config,HostControl);std::_Exit(HostControl);
                        }
                        instance.dispatch_host_callbacks();
                        const auto current_latency=instance.reported_latency_on_control(),current_tail=instance.reported_tail_on_control();
                        if(instance.host_refusals() || !current_latency || !current_tail || *current_latency!=*latency || *current_tail!=*tail) {
                            error(channel,&*config,HostControl);std::_Exit(HostControl);
                        }
                        const auto* snapshot=instance.parameter_snapshot();
                        wire::Bytes body;config->stamp(body);wire::number(body,blocks,8);wire::number(body,frames,8);
                        wire::number(body,flush_operation,8);wire::number(body,flush_applied?1:0,1);wire::number(body,flushed_values.count,2);
                        for(uint32 n=0;n<flushed_values.count;++n) {
                            const auto id=flushed_values.at(n).id;
                            const hex_instance::ParameterSnapshot::Parameter* value=nullptr;
                            if(snapshot) for(uint32 i=0;i<snapshot->count;++i) if(snapshot->parameters[i].info.id==id) value=&snapshot->parameters[i];
                            if(!value) { error(channel,&*config,HostControl);std::_Exit(HostControl); }
                            wire::number(body,id,4);uint64_t bits=0;std::memcpy(&bits,&value->value,8);wire::number(body,bits,8);
                        }
                        if(!wire::send(channel,"HXVIFL01",body)) std::_Exit(Cancelled);
                        std::lock_guard<std::mutex> lock(startup_mutex);epoch_stage=FlushSent;startup_changed.notify_all();
                    } else if(stage==CaptureRequested) {
                        hex_instance::CapturedState current{};
                        const auto admission_revision=instance.host_callback_revision_on_control();
                        // A stopped read cannot silently process a newly
                        // admitted gesture or publish a mixed component/controller
                        // cut. Edits that keep arriving refuse only this capture
                        // (still changing); the stopped epoch stays usable.
                        bool changing=instance.pending_parameter_edits();
                        uint32_t observed=0;
                        auto captured=hex_instance::Instance::CaptureResult::Captured;
                        if(!changing) captured=instance.capture_current_state_on_control(current,observed);
                        if(captured==hex_instance::Instance::CaptureResult::Refused) { error(channel,&*config,Capture);std::_Exit(Capture); }
                        instance.dispatch_host_callbacks();
                        const auto current_latency=instance.reported_latency_on_control();
                        const auto current_tail=instance.reported_tail_on_control();
                        if(instance.host_refusals() || !current_latency || !current_tail || *current_latency!=*latency || *current_tail!=*tail) {
                            error(channel,&*config,Capture);std::_Exit(Capture);
                        }
                        changing=changing || instance.pending_parameter_edits() || instance.host_callback_revision_on_control()!=admission_revision;
                        if(changing) {
                            if(!send_still_changing(channel,*config,blocks,frames,capture_operation)) std::_Exit(Cancelled);
                            std::lock_guard<std::mutex> lock(startup_mutex);
                            epoch_stage=CaptureSent;startup_changed.notify_all();
                            continue;
                        }
                        if(captured==hex_instance::Instance::CaptureResult::Overflow) {
                            if(!send_state_overflow(channel,*config,blocks,frames,capture_operation,observed)) std::_Exit(Cancelled);
                            std::lock_guard<std::mutex> lock(startup_mutex);
                            epoch_stage=CaptureSent;startup_changed.notify_all();
                            continue;
                        }
                        wire::Bytes body;config->stamp(body);
                        wire::number(body,blocks,8);wire::number(body,frames,8);wire::number(body,capture_operation,8);
                        const auto payload=state_payload(*config,current);wire::number(body,payload.size(),4);
                        body.insert(body.end(),payload.begin(),payload.end());
                        if(!wire::send(channel,"HXVICS02",body)) std::_Exit(Cancelled);
                        std::lock_guard<std::mutex> lock(startup_mutex);
                        epoch_stage=CaptureSent;startup_changed.notify_all();
                    } else if(stage==ResumeRequested) {
                        if(instance.latency_restart()) { error(channel,&*config,Quiescence);std::_Exit(Quiescence); }
                        supplied=next_presentation;
                        presentation=apply_presentation(instance,*available,supplied);
                        if(!presentation) { error(channel,&*config,Presentation);std::_Exit(Presentation); }
                        const auto current_latency=instance.reported_latency_on_control();
                        const auto current_tail=instance.reported_tail_on_control();
                        if(!current_latency || !current_tail || *current_latency!=*latency || *current_tail!=*tail || instance.host_refusals()) {
                            error(channel,&*config,Presentation);std::_Exit(Presentation);
                        }
                        if(instance.latency_restart()) { error(channel,&*config,Quiescence);std::_Exit(Quiescence); }
                        // The old input/reply sequence is quiescent. Only this
                        // acknowledged transition changes its wire binding;
                        // no old queued request is accepted under the new one.
                        config->binding[2]=next_graph;config->binding[3]=next_epoch;
                        std::lock_guard<std::mutex> lock(startup_mutex);
                        epoch_stage=ResumeConfigured;startup_changed.notify_all();
                    } else if(stage==Restarted) {
                        auto body=facts_body(*config,*latency,*tail,*available,restoration);
                        wire::number(body,blocks,8);wire::number(body,frames,8);
                        presentation_receipt(body,*presentation,supplied);
                        if(!wire::send(channel,"HXVIRR03",body)) std::_Exit(Cancelled);
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
                    try {
                        if(!editor.pump()) control_failure(Teardown);
                    } catch(...) {
                        editor.quarantine();
                        control_failure(Unexpected);
                    }
                    CFRunLoopRunInMode(kCFRunLoopDefaultMode,0.001,true);
                } catch(...) { control_failure(Unexpected); }
            }
            worker.join();failure=worker_failure.load()&0xFF;
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
        try { if(editor.close()) closing=instance.close_for_retirement(); } catch(...) {}
    }
    @autoreleasepool {
        bool unloaded=false;
        try { if(closing==CloseResult::Closed) unloaded=module.close(); } catch(...) {}
        // A successful stop can leave a foreign host reference alive. Keep
        // its host pool and module mapped and let the OS reclaim both on exit.
        const bool process_only=closing==CloseResult::RetainedHost && normal_stop && !failure;
        const bool closed=closing==CloseResult::Closed || process_only;
        if(!closed || (!unloaded && !process_only)) failure=Teardown;
        bool terminal_sent=false;
        if(config) {
            wire::Bytes terminal;config->stamp(terminal);wire::number(terminal,blocks,8);wire::number(terminal,frames,8);
            wire::number(terminal,failure,4);wire::number(terminal,closed?1:0,1);wire::number(terminal,unloaded?1:0,1);
            terminal_sent=wire::send(channel,"HXVITR01",terminal);
        }
        ::close(channel);
        if(process_only) std::_Exit(terminal_sent?0:ProtocolFailure);
        return failure==0 && normal_stop && closed && unloaded?0:static_cast<int>(failure?failure:ProtocolFailure);
    }
}
