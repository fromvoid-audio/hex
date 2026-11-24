#import <AppKit/AppKit.h>
#include "macos_host.hpp"
#include "macos_parameters.hpp"
#include "macos_processing.hpp"
#include <cassert>
#include <chrono>
#include <cstdio>
#include <thread>
using namespace Steinberg;
using namespace Steinberg::Vst;
using namespace hex_instance;
class Controller : public IEditController {
public:
    ParamValue value=0.25;
    int32 step_count=0;
    int32 flags=ParameterInfo::kCanAutomate;
    uint32 setter_calls=0;
    uint32 descriptor_reads=0;
    Host* late_edit_host=nullptr;
    Host* recursive_value_host=nullptr;
    tresult PLUGIN_API queryInterface(const TUID,void**) override { return kNoInterface; }
    uint32 PLUGIN_API addRef() override { return 1; }
    uint32 PLUGIN_API release() override { return 1; }
    tresult PLUGIN_API initialize(FUnknown*) override { return kResultOk; }
    tresult PLUGIN_API terminate() override { return kResultOk; }
    tresult PLUGIN_API setComponentState(IBStream*) override { return kNotImplemented; }
    tresult PLUGIN_API setState(IBStream*) override { return kNotImplemented; }
    tresult PLUGIN_API getState(IBStream*) override { return kNotImplemented; }
    int32 PLUGIN_API getParameterCount() override { return 1; }
    tresult PLUGIN_API getParameterInfo(int32 n,ParameterInfo& info) override {
        ++descriptor_reads;
        if(n) return kInvalidArgument; info={}; info.id=7; info.flags=flags; info.stepCount=step_count;
        info.defaultNormalizedValue=0.25; return kResultOk;
    }
    tresult PLUGIN_API getParamStringByValue(ParamID,ParamValue,String128) override { return kNotImplemented; }
    tresult PLUGIN_API getParamValueByString(ParamID,TChar*,ParamValue&) override { return kNotImplemented; }
    ParamValue PLUGIN_API normalizedParamToPlain(ParamID,ParamValue v) override { return v; }
    ParamValue PLUGIN_API plainParamToNormalized(ParamID,ParamValue v) override { return v; }
    ParamValue PLUGIN_API getParamNormalized(ParamID) override {
        if(recursive_value_host)
            assert(recursive_value_host->restartComponent(kParamValuesChanged)==kResultFalse);
        return value;
    }
    tresult PLUGIN_API setParamNormalized(ParamID,ParamValue v) override {
        if(late_edit_host) assert(late_edit_host->beginEdit(7)==kResultFalse);
        ++setter_calls;
        value=v; return kResultOk;
    }
    tresult PLUGIN_API setComponentHandler(IComponentHandler*) override { return kResultOk; }
    IPlugView* PLUGIN_API createView(FIDString) override { return nullptr; }
};
class LargeController final : public Controller {
public:
    bool reverse=false, duplicate_unit=false, changed_id=false;
    int32 parameter_count=HOST_PARAMETER_COUNT;
    std::array<ParamValue,HOST_PARAMETER_COUNT> values=[] {
        std::array<ParamValue,HOST_PARAMETER_COUNT> init{};
        init.fill(0.25);
        return init;
    }();
    int32 PLUGIN_API getParameterCount() override { return parameter_count; }
    tresult PLUGIN_API getParameterInfo(int32 ordinal,ParameterInfo& info) override {
        if(ordinal<0 || ordinal>=getParameterCount()) return kInvalidArgument;
        const int32 id=reverse?getParameterCount()-1-ordinal:ordinal;
        info={};
        info.id=changed_id && id==17?getParameterCount():id;
        info.unitId=duplicate_unit && id==17?16:id;
        info.flags=ParameterInfo::kCanAutomate|ParameterInfo::kIsBypass;
        return kResultOk;
    }
    ParamValue PLUGIN_API getParamNormalized(ParamID id) override {
        return id<static_cast<ParamID>(parameter_count)?values[id]:0.0;
    }
    tresult PLUGIN_API setParamNormalized(ParamID id,ParamValue v) override {
        if(id>=static_cast<ParamID>(parameter_count)) return kInvalidArgument;
        ++setter_calls;
        values[id]=v;
        return kResultOk;
    }
};

static void large_metadata_refresh() {
    // Benchmark the complete native control transaction, including SDK reads,
    // descriptor hashing and publication. One warm-up precedes five timed pairs.
    using Clock=std::chrono::steady_clock;
    std::array<double,5> initial_ms{},reversed_ms{};
    for(size_t trial=0;trial<=initial_ms.size();++trial) {
        LargeController controller;
        Host host;
        const auto start=Clock::now();
        assert(host.refresh_on_control(&controller).disposition==HostDisposition::Refreshed);
        const auto loaded=Clock::now();
        assert(host.beginEdit(7)==kResultOk && host.performEdit(7,0.75)==kResultOk);
        assert(host.endEdit(7)==kResultOk);
        const auto before=host.snapshot()->parameters[7];
        assert(before.editor_sequence && before.feedback_floor);
        controller.reverse=true;
        const auto reverse_start=Clock::now();
        assert(host.refresh_on_control(&controller).disposition==HostDisposition::Refreshed);
        const auto refreshed=Clock::now();
        const auto& retained=host.snapshot()->parameters[HOST_PARAMETER_COUNT-1-7];
        assert(retained.info.id==7 && retained.editor_sequence==before.editor_sequence);
        assert(retained.feedback_floor==before.feedback_floor);
        const auto revision=host.snapshot()->revision;
        controller.duplicate_unit=true;
        assert(host.refresh_on_control(&controller).disposition==HostDisposition::InvalidMetadata);
        controller.duplicate_unit=false;
        controller.changed_id=true;
        assert(host.refresh_on_control(&controller).disposition==HostDisposition::InvalidMetadata);
        assert(host.snapshot()->revision==revision && retained.info.id==7);
        if(trial) {
            initial_ms[trial-1]=std::chrono::duration<double,std::milli>(loaded-start).count();
            reversed_ms[trial-1]=std::chrono::duration<double,std::milli>(refreshed-reverse_start).count();
        }
    }
    std::sort(initial_ms.begin(),initial_ms.end());
    std::sort(reversed_ms.begin(),reversed_ms.end());
    std::printf("8192 unique bypass units, median of five: initial %.3f ms, reversed %.3f ms\n",
        initial_ms[2],reversed_ms[2]);
}

static void benchmark_feedback_lookup() {
    using Clock=std::chrono::steady_clock;
    for(int32 count : {128, static_cast<int32>(HOST_PARAMETER_COUNT)}) {
        LargeController controller;
        controller.parameter_count=count;
        Host host;
        assert(host.refresh_on_control(&controller).disposition==HostDisposition::Refreshed);
        ParameterFeedback batch;
        for(int32 i=count-128;i<count;++i)
            assert(batch.put(static_cast<ParamID>(i),0.25));
        for(int w=0;w<10;++w) {
            auto pending=batch;
            assert(host.publish_parameter_feedback(pending));
            assert(host.synchronize_parameter_feedback(&controller));
        }
        std::array<double,101> samples_us{};
        for(size_t s=0;s<samples_us.size();++s) {
            auto pending=batch;
            assert(host.publish_parameter_feedback(pending));
            const auto start=Clock::now();
            assert(host.synchronize_parameter_feedback(&controller));
            const auto finish=Clock::now();
            samples_us[s]=std::chrono::duration<double,std::micro>(finish-start).count();
        }
        assert(controller.setter_calls==0);
        std::sort(samples_us.begin(),samples_us.end());
        std::printf("feedback lookup (%d descriptors, 128 final unchanged IDs, 101 samples): median %.2f us, p95 %.2f us\n",
            count,samples_us[50],samples_us[95]);
    }
}

int main(int argc,char** argv) { @autoreleasepool {
    if(argc==1 || !std::strcmp(argv[1],"large-metadata")) large_metadata_refresh();
    if(argc==1 || !std::strcmp(argv[1],"feedback-lookup-bench")) benchmark_feedback_lookup();
    if(argc==1 || !std::strcmp(argv[1],"reordered-feedback-lookup")) {
        LargeController reordered;
        reordered.reverse=true;
        Host host;
        assert(host.refresh_on_control(&reordered).disposition==HostDisposition::Refreshed);
        // SDK inventory order is preserved while lookup uses the sorted by_id index.
        assert(host.snapshot()->parameters[0].info.id==HOST_PARAMETER_COUNT-1);
        assert(host.snapshot()->parameters[HOST_PARAMETER_COUNT-1].info.id==0);

        // Unchanged feedback values avoid redundant setter calls.
        ParameterFeedback unchanged;
        for(ParamID id : {ParamID{0}, ParamID{7}, ParamID{4095}, ParamID{HOST_PARAMETER_COUNT-1}})
            assert(unchanged.put(id,0.25));
        assert(host.publish_parameter_feedback(unchanged));
        assert(host.synchronize_parameter_feedback(&reordered));
        assert(reordered.setter_calls==0);

        // Changed feedback values reach the exact controller IDs and cached inventory slots.
        ParameterFeedback changed;
        assert(changed.put(0,0.125));
        assert(changed.put(7,0.5));
        assert(changed.put(4095,0.625));
        assert(changed.put(HOST_PARAMETER_COUNT-1,0.875));
        assert(host.publish_parameter_feedback(changed));
        assert(host.synchronize_parameter_feedback(&reordered));
        assert(reordered.setter_calls==4);
        assert(reordered.values[0]==0.125 && host.snapshot()->parameters[HOST_PARAMETER_COUNT-1].value==0.125);
        assert(reordered.values[7]==0.5 && host.snapshot()->parameters[HOST_PARAMETER_COUNT-1-7].value==0.5);
        assert(reordered.values[4095]==0.625 && host.snapshot()->parameters[HOST_PARAMETER_COUNT-1-4095].value==0.625);
        assert(reordered.values[HOST_PARAMETER_COUNT-1]==0.875 && host.snapshot()->parameters[0].value==0.875);

        // Stale-gesture filtering still holds on reordered descriptors.
        reordered.values[7]=0.75;
        assert(host.beginEdit(7)==kResultOk && host.performEdit(7,0.75)==kResultOk && host.endEdit(7)==kResultOk);
        ParameterFeedback stale;
        assert(stale.put(7,0.25,0));
        assert(host.publish_parameter_feedback(stale));
        assert(host.synchronize_parameter_feedback(&reordered));
        assert(reordered.values[7]==0.75 && reordered.setter_calls==4);
        const auto after_end=host.editor_frontier_on_worker();
        ParameterFeedback fresh;
        assert(fresh.put(7,0.375,after_end));
        assert(host.publish_parameter_feedback(fresh));
        assert(host.synchronize_parameter_feedback(&reordered));
        assert(reordered.values[7]==0.375 && reordered.setter_calls==5);

        // Undeclared IDs still fail validation with FeedbackParameterMissing.
        ParameterFeedback missing;
        assert(missing.put(HOST_PARAMETER_COUNT+5,0.5));
        assert(host.publish_parameter_feedback(missing));
        assert(!host.synchronize_parameter_feedback(&reordered));
        assert(host.refusal_flags() & FeedbackParameterMissing);
    }
    // Each scenario can also be run independently against an unfixed host.
    if(argc==1 || !std::strcmp(argv[1],"redundant-setter")) {
        Controller unchanged;
        Host controls;
        assert(controls.refresh_on_control(&unchanged).disposition==HostDisposition::Refreshed);
        ParameterFeedback repeat;
        assert(repeat.put(7,0.25) && controls.publish_parameter_feedback(repeat));
        assert(controls.synchronize_parameter_feedback(&unchanged));
        assert(unchanged.setter_calls==0);
        assert(repeat.put(7,0.5) && controls.publish_parameter_feedback(repeat));
        assert(controls.synchronize_parameter_feedback(&unchanged));
        assert(unchanged.value==0.5 && unchanged.setter_calls==1);
    }
    if(argc==1 || !std::strcmp(argv[1],"held-native-value")) {
        Controller delayed;
        Host controls;
        assert(controls.refresh_on_control(&delayed).disposition==HostDisposition::Refreshed);
        assert(controls.beginEdit(7)==kResultOk && controls.performEdit(7,0.5)==kResultOk);
        const auto older=controls.editor_frontier_on_worker();
        assert(controls.performEdit(7,0.75)==kResultOk);
        const auto latest=controls.editor_frontier_on_worker();
        ParameterFeedback pending;
        assert(pending.put(7,0.5,older,true) && controls.publish_parameter_feedback(pending));
        assert(controls.synchronize_parameter_feedback(&delayed));
        assert(delayed.value==0.25 && delayed.setter_calls==0);
        assert(pending.put(7,0.75,latest,true) && controls.publish_parameter_feedback(pending));
        assert(controls.synchronize_parameter_feedback(&delayed));
        assert(delayed.value==0.75 && delayed.setter_calls==1);
        assert(controls.endEdit(7)==kResultOk && controls.synchronize_parameter_feedback(&delayed));
        assert(delayed.value==0.75 && delayed.setter_calls==1);
    }
    if(argc==1 || !std::strcmp(argv[1],"values-only-refresh")) {
        Controller controller;
        Host host;
        assert(host.refresh_on_control(&controller).disposition==HostDisposition::Refreshed);
        const auto initial_revision=host.snapshot()->revision;
        const auto descriptor_signature=host.snapshot()->descriptor_signature;
        controller.value=0.75;
        assert(host.restartComponent(kParamValuesChanged)==kResultOk);
        assert(host.dispatch(&controller).disposition==HostDisposition::Refreshed);
        assert(controller.descriptor_reads==1);
        assert(host.snapshot()->revision==initial_revision+1);
        assert(host.snapshot()->parameters[0].value==0.75);
        assert(host.snapshot()->descriptor_signature==descriptor_signature);

        assert(host.restartComponent(kParamValuesChanged|kParamTitlesChanged)==kResultOk);
        assert(host.dispatch(&controller).disposition==HostDisposition::Refreshed);
        assert(controller.descriptor_reads==2);
        const auto valid_revision=host.snapshot()->revision;
        controller.value=std::numeric_limits<ParamValue>::quiet_NaN();
        assert(host.restartComponent(kParamValuesChanged)==kResultOk);
        assert(host.dispatch(&controller).disposition==HostDisposition::InvalidMetadata);
        assert(host.snapshot()->revision==valid_revision && host.snapshot()->parameters[0].value==0.75);

        controller.value=0.125;
        controller.recursive_value_host=&host;
        assert(host.restartComponent(kParamValuesChanged)==kResultOk);
        assert(host.dispatch(&controller).disposition==HostDisposition::Recursive);
        assert(host.snapshot()->revision==valid_revision && host.snapshot()->parameters[0].value==0.75);
    }
    if(argc==1 || !std::strcmp(argv[1],"preset-restart-fences-stale-feedback")) {
        // 1. kParamValuesChanged with older pending 0.25 feedback must keep newly selected 0.75
        //    in both controller and refreshed inventory in the worker's main-lane dispatch order.
        for(int32 restart_flags : {static_cast<int32>(kParamValuesChanged),
                                   static_cast<int32>(kParamValuesChanged|kParamTitlesChanged)}) {
            Controller controller;
            Host host;
            assert(host.refresh_on_control(&controller).disposition==HostDisposition::Refreshed);

            ParameterFeedback stale;
            const auto before_restart=host.editor_frontier_on_worker();
            assert(stale.put(7,0.25,before_restart) && host.publish_parameter_feedback(stale));

            controller.value=0.75;
            assert(host.restartComponent(restart_flags)==kResultOk);
            const auto after_restart=host.editor_frontier_on_worker();
            assert(after_restart>before_restart);
            assert(host.value_restart_frontier_on_worker()==after_restart);

            // Worker main-lane order: synchronize_parameter_feedback runs BEFORE dispatch.
            assert(host.synchronize_parameter_feedback(&controller));
            assert(controller.setter_calls==0);
            assert(controller.value==0.75);
            assert(host.dispatch(&controller).disposition==HostDisposition::Refreshed);
            assert(controller.value==0.75);
            assert(host.snapshot()->parameters[0].value==0.75);

            // Feedback originating AFTER the value invalidation still reaches parameter 7.
            ParameterFeedback post;
            assert(post.put(7,0.5,after_restart) && host.publish_parameter_feedback(post));
            assert(host.synchronize_parameter_feedback(&controller));
            assert(controller.value==0.5 && controller.setter_calls==1);
            assert(host.snapshot()->parameters[0].value==0.5);

            // Pre-restart native gesture feedback must not undo a subsequent preset selection,
            // while post-restart native gestures preserve normal ordering.
            assert(host.beginEdit(7)==kResultOk && host.performEdit(7,0.625)==kResultOk);
            const auto native_seq=host.editor_frontier_on_worker();
            assert(host.endEdit(7)==kResultOk);
            controller.value=0.875;
            assert(host.restartComponent(restart_flags)==kResultOk);
            ParameterFeedback pre_preset_native;
            assert(pre_preset_native.put(7,0.625,native_seq,true) &&
                   host.publish_parameter_feedback(pre_preset_native));
            assert(host.synchronize_parameter_feedback(&controller));
            assert(controller.value==0.875 && controller.setter_calls==1);
            for(int i=0;i<3;++i)
                assert(host.dispatch(&controller).disposition==HostDisposition::EditQueued);
            assert(host.dispatch(&controller).disposition==HostDisposition::Refreshed);
            assert(host.snapshot()->parameters[0].value==0.875);

            // Fresh native gesture after preset load still works normally.
            assert(host.beginEdit(7)==kResultOk && host.performEdit(7,0.375)==kResultOk);
            const auto post_native=host.editor_frontier_on_worker();
            controller.value=0.875;
            ParameterFeedback post_native_fb;
            assert(post_native_fb.put(7,0.375,post_native,true) &&
                   host.publish_parameter_feedback(post_native_fb));
            assert(host.synchronize_parameter_feedback(&controller));
            assert(controller.value==0.375 && controller.setter_calls==2);
            assert(host.endEdit(7)==kResultOk);
        }
    }
    if(argc==1 || !std::strcmp(argv[1],"midi-cc-assignment-nonterminal")) {
        Controller controller;
        Host host;
        assert(host.refresh_on_control(&controller).disposition==HostDisposition::Refreshed);

        // Mapping-only notification reports kNotImplemented honestly without raising a
        // sticky HostRefusal that would terminate the active worker.
        assert(host.restartComponent(kMidiCCAssignmentChanged)==kNotImplemented);
        assert(host.refusal_flags()==0);
        assert(host.dispatch(&controller).disposition==HostDisposition::Idle);

        // Combined mapping + parameter-value invalidation admits the value refresh,
        // updates cached inventory, and leaves refusal_flags at zero.
        controller.value=0.75;
        assert(host.restartComponent(kMidiCCAssignmentChanged|kParamValuesChanged)==kResultOk);
        assert(host.refusal_flags()==0);
        assert(host.dispatch(&controller).disposition==HostDisposition::Refreshed);
        assert(host.snapshot()->parameters[0].value==0.75);
        assert(host.refusal_flags()==0);

        // Structural restart flags combined with mapping schedule a controlled
        // resync and record diagnostics without latching a fatal HostRefusal.
        controller.value=0.625;
        assert(host.restartComponent(kMidiCCAssignmentChanged|kIoChanged)==kResultOk);
        assert(host.refusal_flags()==0);
        assert((host.tolerated_flags() & GraphControlRequired)!=0);
        assert(host.dispatch(&controller).disposition==HostDisposition::Refreshed);
        assert(host.snapshot()->parameters[0].value==0.625);
        assert(host.refusal_flags()==0);
    }
    if(argc==1 || !std::strcmp(argv[1],"commercial-sdk-tolerance")) {
        // 1. restartComponent flags commonly emitted by commercial VST3s and
        //    startup kLatencyChanged never latch a fatal HostRefusal.
        {
            Controller controller;
            Host host;
            assert(host.refresh_on_control(&controller).disposition==HostDisposition::Refreshed);
            // Startup kLatencyChanged before observe_latency_restarts() is tolerated.
            assert(host.restartComponent(kLatencyChanged)==kResultOk);
            assert(host.refusal_flags()==0);
            // Benign unsupported notifications return kNotImplemented with zero refusals.
            for(int32 benign : {static_cast<int32>(kPrefetchableSupportChanged),
                                static_cast<int32>(kRoutingInfoChanged),
                                static_cast<int32>(kMidiCCAssignmentChanged),
                                static_cast<int32>(kNoteExpressionChanged)}) {
                assert(host.restartComponent(benign)==kNotImplemented);
                assert(host.refusal_flags()==0);
            }
            // Structural commercial restart flags schedule controlled resync cleanly.
            for(int32 structural : {static_cast<int32>(kReloadComponent),
                                    static_cast<int32>(kIoChanged),
                                    static_cast<int32>(kParamIDMappingChanged)}) {
                controller.value=0.5;
                assert(host.restartComponent(structural)==kResultOk);
                assert(host.refusal_flags()==0);
                assert(host.dispatch(&controller).disposition==HostDisposition::Refreshed);
                assert(host.snapshot()->parameters[0].value==0.5);
            }
            // >8 restartComponent burst coalesces cleanly without latching QueueFull.
            for(int i=0;i<20;++i) {
                assert(host.restartComponent(kParamValuesChanged)==kResultOk);
            }
            assert(host.refusal_flags()==0);
            while(host.dispatch(&controller).disposition!=HostDisposition::Idle) {}
            assert(host.refusal_flags()==0);
        }
        // 2. beginEdit/performEdit/endEdit tolerate hidden/internal/read-only params,
        //    out-of-range finite clamping, NaN rejection without sticky refusal,
        //    duplicate Begin / unmatched End, and >8 (16) simultaneous active edits.
        {
            LargeController large;
            large.parameter_count=64;
            Host host;
            assert(host.refresh_on_control(&large).disposition==HostDisposition::Refreshed);
            // Hidden/unknown ID tolerated without latching HostRefusal.
            assert(host.beginEdit(99999)==kResultOk);
            assert(host.performEdit(99999,0.5)==kResultOk);
            assert(host.endEdit(99999)==kResultOk);
            assert(host.refusal_flags()==0);
            // Out-of-range finite values clamp cleanly to [0, 1]; NaN returns
            // kInvalidArgument without latching a fatal HostRefusal.
            assert(host.beginEdit(1)==kResultOk);
            assert(host.beginEdit(1)==kResultOk); // duplicate beginEdit idempotent
            assert(host.performEdit(1,-0.25)==kResultOk);
            assert(host.performEdit(1,1.75)==kResultOk);
            assert(host.performEdit(1,std::numeric_limits<ParamValue>::quiet_NaN())==kInvalidArgument);
            assert(host.endEdit(1)==kResultOk);
            assert(host.endEdit(1)==kResultOk); // unmatched endEdit idempotent
            assert(host.refusal_flags()==0);
            // Drain clamped edits and verify values are strictly in [0, 1].
            while(host.dispatch(&large).disposition==HostDisposition::EditQueued) {
                EditEvent ev{};
                while(host.peek_edit(ev)) {
                    if(ev.kind==Perform) {
                        assert(ev.normalized>=0.0 && ev.normalized<=1.0);
                    }
                    host.consume_edit();
                    host.complete_parameter_edit();
                }
            }
            // 16 simultaneous active edits (>8) all succeed with zero refusals.
            for(ParamID id=10;id<26;++id) {
                assert(host.beginEdit(id)==kResultOk);
                assert(host.performEdit(id,0.5)==kResultOk);
            }
            for(ParamID id=10;id<26;++id) {
                assert(host.endEdit(id)==kResultOk);
            }
            assert(host.refusal_flags()==0);
        }
        // 3. createInstance(IMessage) from processing thread and bursts beyond 16 & 64.
        {
            Host host(true);
            TUID msg_id{}; std::memcpy(msg_id,INLINE_UID_OF(IMessage),sizeof(TUID));
            bool proc_ok=false;
            std::thread worker([&] {
                host.bind_processing_thread();
                IMessage* m=nullptr;
                proc_ok=(host.createInstance(msg_id,msg_id,reinterpret_cast<void**>(&m))==kResultOk && m!=nullptr);
                if(m) m->release();
            });
            worker.join();
            assert(proc_ok);
            assert(host.refusal_flags()==0);
            assert(host.message_diagnostics().processing_thread_messages.load()==1);
            // Burst beyond 16 (all 64 slots) succeeds; 65th returns kOutOfMemory
            // cleanly without latching a fatal HostRefusal.
            std::array<IMessage*,HOST_MESSAGE_COUNT> held{};
            for(auto& slot : held) {
                assert(host.createInstance(msg_id,msg_id,reinterpret_cast<void**>(&slot))==kResultOk && slot);
            }
            IMessage* overflow=nullptr;
            assert(host.createInstance(msg_id,msg_id,reinterpret_cast<void**>(&overflow))==kOutOfMemory);
            assert(overflow==nullptr);
            assert(host.refusal_flags()==0);
            for(auto* slot : held) slot->release();
        }
        // 4. setParamNormalized returning kResultFalse / kNotImplemented & kIsHidden without kIsReadOnly.
        {
            class TolerantController final : public Controller {
            public:
                tresult setter_return=kResultFalse;
                int32 PLUGIN_API getParameterCount() override { return 2; }
                tresult PLUGIN_API getParameterInfo(int32 n,ParameterInfo& info) override {
                    if(n<0 || n>=2) return kInvalidArgument;
                    info={};
                    if(n==0) {
                        info.id=7; info.flags=ParameterInfo::kCanAutomate;
                        info.defaultNormalizedValue=0.25;
                    } else {
                        // kIsHidden WITHOUT kIsReadOnly (commercial SDK pattern).
                        info.id=8; info.flags=ParameterInfo::kIsHidden;
                        info.defaultNormalizedValue=1.2; // finite out-of-range clamps
                    }
                    return kResultOk;
                }
                tresult PLUGIN_API setParamNormalized(ParamID,ParamValue) override {
                    ++setter_calls;
                    return setter_return;
                }
            };
            TolerantController ctrl;
            Host host;
            assert(host.refresh_on_control(&ctrl).disposition==HostDisposition::Refreshed);
            assert(host.refusal_flags()==0);
            assert(host.snapshot()->count==2);
            assert(host.snapshot()->parameters[1].info.defaultNormalizedValue==1.0);
            // Hidden parameter edits are tolerated as benign no-ops.
            assert(host.beginEdit(8)==kResultOk && host.performEdit(8,0.5)==kResultOk && host.endEdit(8)==kResultOk);
            assert(host.refusal_flags()==0);
            for(tresult ret : {kResultFalse, kNotImplemented}) {
                ctrl.setter_return=ret;
                ParameterFeedback fb;
                assert(fb.put(7,0.75));
                assert(host.publish_parameter_feedback(fb));
                assert(host.synchronize_parameter_feedback(&ctrl));
                assert(host.refusal_flags()==0);
            }
        }
        // 5. finite_outputs tolerates non-zero audio with silenceFlags set and sanitizes NaN/Inf.
        {
            ProcessingConfiguration cfg{};
            cfg.input_channels=2; cfg.output_channels=2; cfg.maximum_frames=4;
            std::array<float,4> out_l{0.25f, std::numeric_limits<float>::quiet_NaN(), -0.5f, std::numeric_limits<float>::infinity()};
            std::array<float,4> out_r{0.0f, 0.0f, 0.0f, 0.0f};
            float* ptrs[2]={out_l.data(), out_r.data()};
            AudioBusBuffers out_bus{};
            out_bus.numChannels=2; out_bus.channelBuffers32=ptrs;
            out_bus.silenceFlags=0x3; // claims both channels silent despite non-zero L
            ProcessData data{};
            data.numSamples=4; data.numOutputs=1; data.outputs=&out_bus;
            assert(finite_outputs(cfg,data));
            assert((out_bus.silenceFlags & 0x1)==0); // L silence bit cleared
            assert((out_bus.silenceFlags & 0x2)!=0); // R genuinely silent bit kept
            assert(out_l[0]==0.25f && out_l[1]==0.0f && out_l[2]==-0.5f && out_l[3]==0.0f);
        }
    }
    if(argc==1 || !std::strcmp(argv[1],"stale-step-count-history")) {
        Controller controller;
        Host host;
        assert(host.refresh_on_control(&controller).disposition==HostDisposition::Refreshed);
        ProcessingParameterSnapshot snapshot{};
        host.refresh_processing_snapshot(snapshot);
        auto* info=snapshot.find(7);
        assert(info && info->accepts_authored(0));
        assert(!info->is_stale_authored_steps(0));
        assert(!info->is_stale_authored_steps(3));
        assert(!info->is_stale_authored_steps(99));

        // Preset changes stepCount 0 -> 3 while keeping kCanAutomate.
        controller.step_count=3;
        assert(host.restartComponent(kParamTitlesChanged)==kResultOk);
        assert(host.dispatch(&controller).disposition==HostDisposition::Refreshed);
        assert(host.refusal_flags()==0);
        host.refresh_processing_snapshot(snapshot);
        info=snapshot.find(7);
        assert(info && info->accepts_authored(3));
        assert(!info->accepts_authored(0));
        assert(info->is_stale_authored_steps(0));
        assert(!info->is_stale_authored_steps(99));

        // Clearing kCanAutomate is never a stale step-count transition.
        controller.flags=0;
        assert(host.restartComponent(kParamTitlesChanged)==kResultOk);
        assert(host.dispatch(&controller).disposition==HostDisposition::Refreshed);
        host.refresh_processing_snapshot(snapshot);
        info=snapshot.find(7);
        assert(info && !info->accepts_authored(3));
        assert(!info->is_stale_authored_steps(0));
        assert(!info->is_stale_authored_steps(3));
    }
    if(argc==1 || !std::strcmp(argv[1],"combined-feedback-capacity")) {
        LargeController large;
        Host host;
        assert(host.refresh_on_control(&large).disposition==HostDisposition::Refreshed);

        // Existing single-direction 128-point limit still rejects a 129th output ID
        // with sticky refusal.
        ParameterChanges overflow_output;
        assert(overflow_output.reset(64,true) && overflow_output.begin_sdk());
        for(ParamID id=0;id<PARAMETER_POINT_COUNT;++id) {
            int32 idx=-1;
            auto* queue=overflow_output.addParameterData(id,idx);
            assert(queue && queue->addPoint(32,static_cast<ParamValue>(id+1)/256.0,idx)==kResultOk);
        }
        {
            int32 idx=-1;
            assert(overflow_output.addParameterData(201,idx)==nullptr);
        }
        assert(!overflow_output.end_sdk());
        assert(!overflow_output.reset(64,true));

        // 1 valid input (ID 200) + 128 distinct valid outputs (IDs 0..127) must fit in
        // bounded ParameterFeedback and synchronize all 129 latest values to the controller,
        // while malformed output points remain individually rejected.
        ParameterChanges input, output;
        assert(input.reset(64,false) && output.reset(64,true));
        assert(input.input(200,0,0.75));
        assert(input.begin_sdk() && output.begin_sdk());
        for(ParamID id=0;id<PARAMETER_POINT_COUNT;++id) {
            int32 idx=-1;
            auto* queue=output.addParameterData(id,idx);
            assert(queue);
            if(id==0) {
                assert(queue->addPoint(0,std::numeric_limits<ParamValue>::quiet_NaN(),idx)==kResultFalse);
                assert(queue->addPoint(0,-0.25,idx)==kResultFalse);
                assert(queue->addPoint(0,1.25,idx)==kResultFalse);
            }
            assert(queue->addPoint(32,static_cast<ParamValue>(id+1)/256.0,idx)==kResultOk);
        }
        assert(input.end_sdk() && output.end_sdk());
        assert(input.size()==1 && output.size()==PARAMETER_POINT_COUNT);

        ParameterFeedback feedback;
        ParameterPoint pt{};
        assert(input.point(0,pt) && feedback.put(pt.id,pt.normalized));
        for(size_t n=0;n<output.size();++n) {
            assert(output.point(n,pt));
            assert(feedback.put(pt.id,pt.normalized));
        }
        // Invalid IDs / out-of-range values are rejected by ParameterFeedback::put.
        assert(!feedback.put(0x80000000u,0.5));
        assert(!feedback.put(202,std::numeric_limits<ParamValue>::quiet_NaN()));
        assert(!feedback.put(202,-0.1));
        assert(!feedback.put(202,1.1));
        assert(feedback.count==PARAMETER_POINT_COUNT+1);
        assert(host.publish_parameter_feedback(feedback) && feedback.count==0);
        assert(host.synchronize_parameter_feedback(&large));
        assert(large.values[200]==0.75);
        for(ParamID id=0;id<PARAMETER_POINT_COUNT;++id) {
            assert(large.values[id]==static_cast<ParamValue>(id+1)/256.0);
        }

        // Overlapping ID appearing in both input and output coalesces to the latest value.
        assert(feedback.put(200,0.125) && feedback.put(200,0.875));
        assert(feedback.count==1 && feedback.at(0).id==200 && feedback.at(0).normalized==0.875);
        assert(host.publish_parameter_feedback(feedback));
        assert(host.synchronize_parameter_feedback(&large));
        assert(large.values[200]==0.875);
    }
    if(argc>1) return 0;
    Controller controller;
    Host host;
    assert(host.refresh_on_control(&controller).disposition==HostDisposition::Refreshed);
    ParameterFeedback pending;
    assert(pending.put(7,0.25,0) && host.publish_parameter_feedback(pending));
    controller.value=0.75;
    assert(host.beginEdit(7)==kResultOk && host.performEdit(7,0.75)==kResultOk);
    assert(host.synchronize_parameter_feedback(&controller));
    assert(controller.value==0.75); // Feedback predating the first native edit is stale.
    assert(pending.put(7,0.25,0) && host.publish_parameter_feedback(pending));
    assert(host.synchronize_parameter_feedback(&controller));
    assert(controller.value==0.75); // An older DSP base remains stale after admission.

    const auto during=host.editor_frontier_on_worker();
    assert(during>0);
    assert(pending.put(7,0.5,during) && host.publish_parameter_feedback(pending));
    assert(host.synchronize_parameter_feedback(&controller));
    assert(controller.value==0.75); // Host feedback cannot move an active editor gesture.
    assert(pending.put(7,0.4,during) && host.publish_parameter_feedback(pending));
    assert(host.endEdit(7)==kResultOk);
    assert(host.synchronize_parameter_feedback(&controller));
    assert(controller.value==0.75); // Feedback sampled before End stays stale after it.
    controller.value=0.25; // Some editors defer their controller update to the host.
    assert(pending.put(7,0.75,during,true) && host.publish_parameter_feedback(pending));
    assert(host.synchronize_parameter_feedback(&controller));
    assert(controller.value==0.75); // The final processed native edit remains valid.
    const auto after=host.editor_frontier_on_worker();
    assert(after>during);
    assert(pending.put(7,0.5,after) && host.publish_parameter_feedback(pending));
    assert(host.synchronize_parameter_feedback(&controller));
    assert(controller.value==0.5); // Deliberate host input after End remains effective.
    controller.value=0.875;
    assert(host.beginEdit(7)==kResultOk && host.performEdit(7,0.875)==kResultOk);
    assert(pending.put(7,0.4,after) && host.publish_parameter_feedback(pending));
    assert(host.synchronize_parameter_feedback(&controller));
    assert(controller.value==0.875);
    assert(host.endEdit(7)==kResultOk);

    // A live tail may keep processing while its editor moves to a successor.
    // Do not detach mid-gesture; fence reentrant callbacks only during detach.
    Host detaching;
    assert(detaching.refresh_on_control(&controller).disposition==HostDisposition::Refreshed);
    assert(detaching.editor_admission_sequence_on_worker()==0);
    assert(detaching.beginEdit(7)==kResultOk);
    assert(detaching.editor_admission_sequence_on_worker()==1);
    assert(detaching.editor_frontier_on_worker()==0);
    assert(detaching.performEdit(7,0.5)==kResultOk);
    assert(detaching.begin_editor_detach_on_control()==EditorDetachAdmission::GestureActive);
    assert(detaching.endEdit(7)==kResultOk);
    const auto retained=detaching.pending_parameter_edits();
    assert(detaching.begin_editor_detach_on_control()==EditorDetachAdmission::Granted);
    assert(detaching.begin_editor_detach_on_control()==EditorDetachAdmission::Refused);
    assert(!detaching.seal_editor_edits_on_control());
    assert(detaching.beginEdit(7)==kResultFalse);
    assert(detaching.performEdit(7,0.75)==kResultFalse);
    assert(detaching.endEdit(7)==kResultFalse);
    assert(detaching.refusal_flags()==0 && detaching.pending_parameter_edits()==retained);
    detaching.end_editor_detach_on_control();
    assert(detaching.beginEdit(7)==kResultOk);
    assert(detaching.performEdit(7,0.75)==kResultOk);
    assert(detaching.endEdit(7)==kResultOk);
    assert(detaching.pending_parameter_edits()==retained+3);

    Host held(false,true);
    assert(held.refresh_on_control(&controller).disposition==HostDisposition::Refreshed);
    assert(held.beginEdit(7)==kResultOk);
    assert(!held.hold_editor_admission_on_control(1));
    assert(held.performEdit(7,0.5)==kResultOk && held.endEdit(7)==kResultOk);
    const auto accepted=held.editor_admission_sequence_on_worker();
    assert(held.hold_editor_admission_on_control(1));
    assert(held.hold_editor_admission_on_control(1));
    assert(!held.hold_editor_admission_on_control(2));
    assert(!held.release_editor_admission_on_control(2));
    assert(held.beginEdit(7)==kResultFalse && held.performEdit(7,0.75)==kResultFalse);
    assert(held.startGroupEdit()==kResultFalse);
    assert(held.editor_admission_sequence_on_worker()==accepted);
    assert(held.pending_parameter_edits()==3 && held.refusal_flags()==0);
    assert(held.release_editor_admission_on_control(1));
    assert(!held.hold_editor_admission_on_control(1));
    assert(held.startGroupEdit()==kResultOk);
    assert(!held.hold_editor_admission_on_control(2));
    assert(held.finishGroupEdit()==kResultOk);
    assert(held.performEdit(7,0.75)==kResultOk);
    assert(held.hold_editor_admission_on_control(2));
    assert(held.seal_editor_edits_on_control());
    assert(!held.release_editor_admission_on_control(2));
    assert(held.performEdit(7,0.25)==kResultFalse);

    // A closed input boundary must keep already accepted work in both main
    // and worker queues and refuse later SDK calls without faulting the host.
    Host retiring;
    assert(retiring.refresh_on_control(&controller).disposition==HostDisposition::Refreshed);
    assert(retiring.beginEdit(7)==kResultOk);
    assert(retiring.performEdit(7,0.5)==kResultOk);
    assert(!retiring.seal_editor_edits_on_control()); // An open gesture still owns its End.
    assert(retiring.endEdit(7)==kResultOk);
    for(int n=0;n<3;++n) {
        assert(retiring.beginEdit(7)==kResultOk);
        assert(retiring.performEdit(7,0.5)==kResultOk);
        assert(retiring.endEdit(7)==kResultOk);
    }
    for(int n=0;n<8;++n)
        assert(retiring.dispatch(&controller).disposition==HostDisposition::EditQueued);
    const auto revision=retiring.callback_revision_on_control();
    assert(retiring.pending_parameter_edits()==12 && retiring.ready_parameter_edits()==8);
    assert(retiring.seal_editor_edits_on_control());
    assert(retiring.begin_editor_detach_on_control()==EditorDetachAdmission::Refused);
    assert(retiring.seal_editor_edits_on_control()); // Idempotent for the same owner.
    assert(retiring.apply_saved_state([&] {
        assert(retiring.beginEdit(7)==kResultOk);
        assert(retiring.performEdit(7,0.25)==kResultOk);
        assert(retiring.endEdit(7)==kResultOk);
        return kResultOk;
    })==kResultOk);
    assert(retiring.beginEdit(7)==kResultFalse);
    assert(retiring.performEdit(7,0.75)==kResultFalse);
    assert(retiring.endEdit(7)==kResultFalse);
    assert(retiring.refusal_flags()==0);
    assert(retiring.callback_revision_on_control()==revision);
    assert(retiring.pending_parameter_edits()==12 && retiring.ready_parameter_edits()==8);
    // Sealing editor input must not discard later latency restart intent.
    retiring.observe_latency_restarts();
    assert(retiring.restartComponent(kLatencyChanged)==kResultOk);
    assert(retiring.latency_restart()>revision);
    uint64 last=0;
    for(int n=0;n<12;++n) {
        EditEvent event{};
        assert(retiring.peek_edit(event));
        assert(event.sequence>last && event.id==7);
        assert(event.kind==(n%3==0?Begin:(n%3==1?Perform:End)));
        last=event.sequence;
        retiring.consume_edit();
        retiring.complete_parameter_edit();
        if(n>=4 && n<8)
            assert(retiring.dispatch(&controller).disposition==HostDisposition::EditQueued);
    }
    assert(retiring.pending_parameter_edits()==0 && retiring.ready_parameter_edits()==0);
    // Refusing a late editor callback during feedback must not poison the
    // accepted feedback operation as a recursive restart.
    controller.late_edit_host=&retiring;
    assert(pending.put(7,0.125,retiring.editor_frontier_on_worker(),true));
    assert(retiring.publish_parameter_feedback(pending));
    assert(retiring.synchronize_parameter_feedback(&controller));
    assert(controller.value==0.125 && retiring.refusal_flags()==0);
    controller.late_edit_host=nullptr;

    Host grouped(false,true);
    assert(grouped.refresh_on_control(&controller).disposition==HostDisposition::Refreshed);
    assert(grouped.startGroupEdit()==kResultOk);
    assert(grouped.editor_admission_sequence_on_worker()==1);
    assert(grouped.beginEdit(7)==kResultOk && grouped.performEdit(7,0.5)==kResultOk);
    assert(grouped.begin_editor_detach_on_control()==EditorDetachAdmission::GestureActive);
    assert(!grouped.seal_editor_edits_on_control()); // An unfinished cohort is not discarded.
    assert(grouped.endEdit(7)==kResultOk && grouped.finishGroupEdit()==kResultOk);
    assert(grouped.editor_admission_sequence_on_worker()==5);
    assert(grouped.editor_frontier_on_worker()==4);
    assert(grouped.begin_editor_detach_on_control()==EditorDetachAdmission::Granted);
    assert(grouped.startGroupEdit()==kResultFalse);
    assert(grouped.finishGroupEdit()==kResultFalse);
    grouped.end_editor_detach_on_control();
    assert(grouped.refusal_flags()==0 && grouped.pending_parameter_edits()==5);
    assert(grouped.seal_editor_edits_on_control());
    assert(grouped.apply_saved_state([&] {
        assert(grouped.startGroupEdit()==kResultOk);
        assert(grouped.finishGroupEdit()==kResultOk);
        return kResultOk;
    })==kResultOk);
    assert(grouped.startGroupEdit()==kResultFalse && grouped.finishGroupEdit()==kResultFalse);
    assert(grouped.editor_admission_sequence_on_worker()==5);
    assert(grouped.refusal_flags()==0 && grouped.pending_parameter_edits()==5);
    assert(grouped.dispatch(&controller).disposition==HostDisposition::EditQueued);
    const uint32 expected[]={GroupBegin,Begin,Perform,End,GroupEnd};
    for(auto kind:expected) {
        EditEvent event{}; assert(grouped.peek_edit(event) && event.kind==kind);
        grouped.consume_edit(); grouped.complete_parameter_edit();
    }
    assert(grouped.pending_parameter_edits()==0);
    return 0;
}}
