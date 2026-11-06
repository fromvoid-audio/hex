// Deterministically gated original native tests. The gate is fixture-only, not
// a product processing/callback wait or a physical deadline qualification.
#pragma once
#include "macos_instance.hpp"
#include <condition_variable>
#include <mutex>

struct HostEvidence {
    int scenario=-1, callback=0, message=0, unknown=0, graph=0, known=0;
    int disposition=0, enqueued=0, dispatched=0, received=0, violations=-1;
    int processing_callback=0, processing_message=0, worker_dispatch=0;
    int edit_count=0,edit_order=0,edit_backpressure=0;
    uint64_t before_revision=0, after_revision=0, first_sequence=0, last_sequence=0;
    double before_value=0, after_value=0;
    uint32_t refusals=0;
    bool pcm=false, closed=false;
} host_evidence;

static bool host_case(CFBundleRef bundle,Steinberg::IPluginFactory* factory) {
    using namespace Steinberg;
    using namespace Steinberg::Vst;
    auto configure=reinterpret_cast<bool(*)()>(CFBundleGetFunctionPointerForName(bundle,CFSTR("HexHostConfigure")));
    auto action=reinterpret_cast<int(*)(int,int)>(CFBundleGetFunctionPointerForName(bundle,CFSTR("HexHostAction")));
    auto messages=reinterpret_cast<int(*)(int)>(CFBundleGetFunctionPointerForName(bundle,CFSTR("HexHostMessages")));
    auto edit=reinterpret_cast<int(*)(int,uint32,double)>(CFBundleGetFunctionPointerForName(bundle,CFSTR("HexHostEdit")));
    auto observe=reinterpret_cast<int(*)(int*,int)>(CFBundleGetFunctionPointerForName(bundle,CFSTR("HexLifecycleObservation")));
    if (!configure || !action || !messages || !edit || !observe || !configure()) return false;
    auto& e=host_evidence;
    hex_instance::Instance instance;
    PClassInfo info{};
    if (factory->getClassInfo(0,&info)!=kResultOk || !instance.open(factory,info.cid) || !instance.configure_stereo(48000,64,false)) {
        lifecycle_evidence.closed=instance.close(); return false;
    }
    const bool baseline=action(0,hex_instance::Host::supported_restarts)==kResultOk &&
        instance.dispatch_host_callbacks().disposition==hex_instance::HostDisposition::Refreshed;
    const auto* initial=instance.parameter_snapshot();
    if (!baseline || !initial || initial->count!=2) { lifecycle_evidence.closed=instance.close(); return false; }
    e.before_revision=initial->revision; e.before_value=initial->parameters[0].value;
    if (e.scenario==10) action(3,0);
    std::mutex mutex; std::condition_variable changed; bool reached=false, resume=false,edits_drained=false,second_ready=false;
    bool started=false, stopped=false, processed=true;
    std::thread worker([&] {
        std::array<float,64> left{},right{},out_left{},out_right{};
        left.fill(0.75f); right.fill(-0.5f);
        float* inputs[]={left.data(),right.data()}; float* outputs[]={out_left.data(),out_right.data()};
        AudioBusBuffers input{},output{}; input.numChannels=output.numChannels=2;
        input.channelBuffers32=inputs; output.channelBuffers32=outputs;
        ProcessData data{}; data.processMode=kRealtime; data.symbolicSampleSize=kSample32;
        data.numSamples=64; data.numInputs=1; data.numOutputs=1; data.inputs=&input; data.outputs=&output;
        started=instance.start_processing(); e.pcm=started;
        for (int block=0;block<2;++block) {
            processed=started && processed && instance.process(data);
            if (processed) for (size_t n=0;n<64;++n) {
                const float scale=block==0?1.0f:0.5f;
                e.pcm=e.pcm && out_left[n]==0.375f*scale && out_right[n]==-0.25f*scale;
            }
            if (!block) {
                if (e.scenario==10) e.worker_dispatch=static_cast<int>(instance.dispatch_host_callbacks().disposition);
                std::unique_lock<std::mutex> lock(mutex);
                reached=true; changed.notify_all(); changed.wait(lock,[&] { return resume; });
                if(e.scenario==13) {
                    lock.unlock();e.edit_order=1;
                    for(int wave=0;wave<2;++wave) {
                        for(int n=0;n<8;++n) {
                            hex_instance::EditEvent event{};
                            if(!instance.peek_parameter_edit(event)) {e.edit_order=0;continue;}
                            instance.consume_parameter_edit();++e.edit_count;
                            const uint64 expected=2+static_cast<uint64>(wave*8+n);
                            const uint32 kind=n==0?hex_instance::Begin:(n==7?hex_instance::End:hex_instance::Perform);
                            if(event.sequence!=expected || event.gesture!=2+static_cast<uint64>(wave*8) ||
                                event.id!=static_cast<uint32>(wave?11:7) || event.kind!=kind ||
                                (kind==hex_instance::Perform && event.normalized!=0.75)) e.edit_order=0;
                        }
                        if(!wave) {
                            std::unique_lock<std::mutex> again(mutex);edits_drained=true;changed.notify_all();
                            changed.wait(again,[&]{return second_ready;});
                        }
                    }
                }
            }
        }
        if (started) stopped=instance.stop_processing();
    });
    { std::unique_lock<std::mutex> lock(mutex); changed.wait(lock,[&] { return reached; }); }
    if (e.scenario==0) {
        e.message=messages(0); action(1,0); action(5,0);
    } else if (e.scenario==1) {
        e.unknown=action(0,1<<24); e.graph=action(0,kIoChanged|kLatencyChanged);
        e.known=action(0,kMidiCCAssignmentChanged);
    } else if (e.scenario==2) {
        action(1,0);
        for (int n=0;n<8;++n) if (action(0,hex_instance::Host::supported_restarts)==kResultOk) ++e.enqueued;
        e.callback=action(0,hex_instance::Host::supported_restarts);
    } else if (e.scenario>=3 && e.scenario<=9) {
        action(1,0); action(2,e.scenario-2);
        e.callback=action(0,hex_instance::Host::supported_restarts);
    } else if (e.scenario==10) {
        e.processing_callback=action(4,0); e.processing_message=action(7,0);
    } else if (e.scenario==11) e.message=messages(1);
    else if(e.scenario==14) {
        // Independent of both the main thread and the original PCM reader.
        std::thread background([&] {e.message=messages(2);});
        background.join();
    }
    else if(e.scenario==13) {
        const auto wave=[&](uint32 id) {
            bool ok=edit(1,id,0)==kResultOk;
            for(int n=0;n<6;++n) ok=edit(2,id,0.75)==kResultOk && ok;
            return edit(3,id,0)==kResultOk && ok;
        };
        e.callback=wave(7)?kResultOk:kResultFalse;
        for(int n=0;n<8;++n) if(instance.dispatch_host_callbacks().disposition==hex_instance::HostDisposition::EditQueued) ++e.enqueued;
        e.callback=wave(11) && e.callback==kResultOk?kResultOk:kResultFalse;
        const auto pending=instance.dispatch_host_callbacks();
        e.edit_backpressure=pending.disposition==hex_instance::HostDisposition::Pending && pending.sequence==10;
        {
            std::unique_lock<std::mutex> lock(mutex);resume=true;changed.notify_all();
            changed.wait(lock,[&]{return edits_drained;});
        }
        for(int n=0;n<8;++n) if(instance.dispatch_host_callbacks().disposition==hex_instance::HostDisposition::EditQueued) ++e.enqueued;
        {std::lock_guard<std::mutex> lock(mutex);second_ready=true;changed.notify_all();}
    }
    for (int n=0;n<8;++n) {
        const auto result=instance.dispatch_host_callbacks();
        if (result.disposition==hex_instance::HostDisposition::Idle) break;
        e.disposition=static_cast<int>(result.disposition);
        if (!e.first_sequence) e.first_sequence=result.sequence;
        e.last_sequence=result.sequence; ++e.dispatched;
    }
    const auto* current=instance.parameter_snapshot();
    if (current) { e.after_revision=current->revision; e.after_value=current->parameters[0].value; }
    e.refusals=instance.host_refusals(); e.received=action(6,0);
    { std::lock_guard<std::mutex> lock(mutex); resume=true; changed.notify_all(); }
    worker.join();
    e.pcm=e.pcm && processed && stopped;
    e.closed=instance.close(); lifecycle_evidence.closed=e.closed;
    int observation[135]{};
    const int size=observe(observation,135);
    if (size>=7) e.violations=observation[0];
    return e.pcm && e.violations==0 && (e.scenario!=13 || (e.edit_count==16 && e.edit_order && e.edit_backpressure && e.enqueued==16 && e.callback==kResultOk));
}
static void print_host() {
    const auto& e=host_evidence;
    std::printf("{\"version\":1,\"scope\":\"original-macos-host-control\",\"scenario\":%d,"
        "\"callback\":%d,\"message\":%d,\"unknown\":%d,\"graph\":%d,\"known\":%d,"
        "\"disposition\":%d,\"enqueued\":%d,\"dispatched\":%d,\"received\":%d,\"violations\":%d,"
        "\"processing_callback\":%d,\"processing_message\":%d,\"worker_dispatch\":%d,"
        "\"edit_count\":%d,\"edit_order\":%d,\"edit_backpressure\":%d,"
        "\"before_revision\":%llu,\"after_revision\":%llu,\"first_sequence\":%llu,\"last_sequence\":%llu,"
        "\"before_value\":%.17g,\"after_value\":%.17g,\"refusals\":%u,\"pcm\":%s,\"closed\":%s,\"unloaded\":%s}\n",
        e.scenario,e.callback,e.message,e.unknown,e.graph,e.known,e.disposition,e.enqueued,e.dispatched,e.received,e.violations,
        e.processing_callback,e.processing_message,e.worker_dispatch,e.edit_count,e.edit_order,e.edit_backpressure,
        static_cast<unsigned long long>(e.before_revision),static_cast<unsigned long long>(e.after_revision),
        static_cast<unsigned long long>(e.first_sequence),static_cast<unsigned long long>(e.last_sequence),
        e.before_value,e.after_value,e.refusals,e.pcm?"true":"false",e.closed?"true":"false",lifecycle_evidence.unloaded?"true":"false");
}
