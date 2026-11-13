#import <AppKit/AppKit.h>
#include "macos_host.hpp"
#include <cassert>
using namespace Steinberg;
using namespace Steinberg::Vst;
using namespace hex_instance;
class Controller final : public IEditController {
public:
    ParamValue value=0.25;
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
    tresult PLUGIN_API setParamNormalized(ParamID,ParamValue v) override { value=v; return kResultOk; }
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
    return 0;
}}
