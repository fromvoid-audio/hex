#import <AppKit/AppKit.h>
#include "macos_host.hpp"
#include "macos_parameters.hpp"
#include <cassert>
#include <chrono>
#include <cstdio>
using namespace Steinberg;
using namespace Steinberg::Vst;
using namespace hex_instance;
class Controller : public IEditController {
public:
    ParamValue value=0.25;
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
        if(n) return kInvalidArgument; info={}; info.id=7; info.flags=ParameterInfo::kCanAutomate;
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
    std::array<ParamValue,HOST_PARAMETER_COUNT> values=[] {
        std::array<ParamValue,HOST_PARAMETER_COUNT> init{};
        init.fill(0.25);
        return init;
    }();
    int32 PLUGIN_API getParameterCount() override { return HOST_PARAMETER_COUNT; }
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
        return id<HOST_PARAMETER_COUNT?values[id]:0.0;
    }
    tresult PLUGIN_API setParamNormalized(ParamID id,ParamValue v) override {
        if(id>=HOST_PARAMETER_COUNT) return kInvalidArgument;
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

int main(int argc,char** argv) { @autoreleasepool {
    if(argc==1 || !std::strcmp(argv[1],"large-metadata")) large_metadata_refresh();
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
