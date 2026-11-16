#import <AppKit/AppKit.h>
#include "macos_host.hpp"
#include <cassert>
using namespace Steinberg;
using namespace Steinberg::Vst;
using namespace hex_instance;
class Controller final : public IEditController {
public:
    ParamValue value=0.25;
    Host* late_edit_host=nullptr;
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
        if(n) return kInvalidArgument; info={}; info.id=7; info.flags=ParameterInfo::kCanAutomate;
        info.defaultNormalizedValue=0.25; return kResultOk;
    }
    tresult PLUGIN_API getParamStringByValue(ParamID,ParamValue,String128) override { return kNotImplemented; }
    tresult PLUGIN_API getParamValueByString(ParamID,TChar*,ParamValue&) override { return kNotImplemented; }
    ParamValue PLUGIN_API normalizedParamToPlain(ParamID,ParamValue v) override { return v; }
    ParamValue PLUGIN_API plainParamToNormalized(ParamID,ParamValue v) override { return v; }
    ParamValue PLUGIN_API getParamNormalized(ParamID) override { return value; }
    tresult PLUGIN_API setParamNormalized(ParamID,ParamValue v) override {
        if(late_edit_host) assert(late_edit_host->beginEdit(7)==kResultFalse);
        value=v; return kResultOk;
    }
    tresult PLUGIN_API setComponentHandler(IComponentHandler*) override { return kResultOk; }
    IPlugView* PLUGIN_API createView(FIDString) override { return nullptr; }
};
int main() { @autoreleasepool {
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
    assert(detaching.beginEdit(7)==kResultOk);
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
    assert(grouped.beginEdit(7)==kResultOk && grouped.performEdit(7,0.5)==kResultOk);
    assert(grouped.begin_editor_detach_on_control()==EditorDetachAdmission::GestureActive);
    assert(!grouped.seal_editor_edits_on_control()); // An unfinished cohort is not discarded.
    assert(grouped.endEdit(7)==kResultOk && grouped.finishGroupEdit()==kResultOk);
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
