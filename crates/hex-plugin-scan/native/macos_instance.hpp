// Native-helper-only component/controller ownership. Original Hex implementation.
// The admitted module outlives this owner and may unload only after full closure.
// UI methods require the helper's main thread. Processing methods require one
// serialized worker; its joining control owner must join before close(). No ABI
// pointer crosses into Rust. This is not installed-module or audio-bridge admission.
#pragma once
#include <Foundation/Foundation.h>
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "macos_host.hpp"
#include "macos_parameter_page.hpp"
#include "macos_processing.hpp"
#include "macos_state_stream.hpp"
#include "pluginterfaces/vst/ivstmessage.h"
#include <atomic>
#include <cmath>
#include <cstring>
#include <new>
#include <optional>
#include <thread>

namespace hex_instance {
using namespace Steinberg;
using namespace Steinberg::Vst;

class Instance final {
    IPluginFactory* factory = nullptr;
    IComponent* component = nullptr;
    IAudioProcessor* processor = nullptr;
    IEditController* controller = nullptr;
    IConnectionPoint* component_connection = nullptr;
    IConnectionPoint* controller_connection = nullptr;
    Host* host = nullptr;
    bool component_initialized = false, controller_initialized = false;
    bool handler_set = false, connected_component = false, connected_controller = false;
    bool input_active = false, output_active = false, active = false;
    bool silent_input_ = false;
    int32 event_input_channels_ = 0;
    bool configured = false, configuration_started = false, opened = false, used = false;
    ProcessingConfiguration configuration_{};
    InactiveInstrumentOutputs inactive_outputs_;
    StateStream* component_stream=nullptr;
    StateStream* controller_stream=nullptr;
    StateStream* restore_component_stream=nullptr;
    StateStream* restore_controller_stream=nullptr;
    bool restoration_started=false,restored=false;
    std::thread::id processing_thread;
    std::atomic<bool> processing{false}, processing_fault{false}, quarantined{false};

    // Some modules use kResultFalse to report an optional interface absent.
    // A success with no object, or a failure with an object, is still malformed.
    static bool interface_absent(tresult result,const FUnknown* object) {
        return !object && (result==kNoInterface || result==kResultFalse);
    }
    template<typename T> static bool optional(FUnknown* object, const TUID iid, T*& out) {
        const auto result = object->queryInterface(iid, reinterpret_cast<void**>(&out));
        return (result == kResultOk && out) || interface_absent(result,out);
    }
    static bool identity(FUnknown* left, FUnknown* right, bool& equal) {
        FUnknown* a = nullptr;
        FUnknown* b = nullptr;
        const bool first = left->queryInterface(INLINE_UID_OF(FUnknown), reinterpret_cast<void**>(&a)) == kResultOk && a;
        const bool second = right->queryInterface(INLINE_UID_OF(FUnknown), reinterpret_cast<void**>(&b)) == kResultOk && b;
        equal = a == b;
        if (a) a->release();
        if (b) b->release();
        return first && second;
    }
    bool sync_controller_component_state(StateStream* stream) {
        if (!stream->rewind_read_only_on_control()) return false;
        const auto result=host->apply_saved_state([&] { return controller->setComponentState(stream); });
        // A controller can decline this synchronization method while the
        // processor and controller still own their separate state chunks.
        return (result==kResultOk || result==kNotImplemented) && stream->valid() && stream->exclusive();
    }
    bool refuse_cleanup() { quarantined.store(true); return false; }
public:
    Instance() = default;
    Instance(const Instance&) = delete;
    Instance& operator=(const Instance&) = delete;
    // No foreign calls during C++ destruction. A failed/omitted close deliberately
    // retains references in this isolated helper until its supervisor reaps it;
    // the caller must also retain its module rather than unload live objects.
    ~Instance() = default;

    bool open(IPluginFactory* admitted_factory, const TUID class_id,bool trace_messages=false) {
        if (![NSThread isMainThread] || used || !admitted_factory || quarantined.load())
            return false;
        used = true;
        factory = admitted_factory;
        factory->addRef();
        host = new(std::nothrow) Host(trace_messages);
        if (!host) return false;
        if (factory->createInstance(class_id, INLINE_UID_OF(IComponent),
                                    reinterpret_cast<void**>(&component)) != kResultOk || !component)
            return false;
        if (component->initialize(static_cast<IHostApplication*>(host)) != kResultOk)
            return false;
        component_initialized = true;
        if (component->queryInterface(INLINE_UID_OF(IAudioProcessor),
                                       reinterpret_cast<void**>(&processor)) != kResultOk || !processor)
            return false;
        if (!optional(component, INLINE_UID_OF(IEditController), controller)) return false;
        if (controller) {
            bool same_object = false;
            if (!identity(component, controller, same_object) || !same_object) return false;
        } else {
            TUID controller_id{};
            const auto result = component->getControllerClassId(controller_id);
            if (result == kResultOk) {
                TUID zero{};
                if (same(controller_id, zero) ||
                    factory->createInstance(controller_id, INLINE_UID_OF(IEditController),
                                            reinterpret_cast<void**>(&controller)) != kResultOk || !controller)
                    return false;
                // A malformed factory may hand back the already initialized
                // component. Refuse before a second initialize on that object.
                bool same_object = false;
                if (!identity(component, controller, same_object) || same_object) return false;
                if (controller->initialize(static_cast<IHostApplication*>(host)) != kResultOk)
                    return false;
                controller_initialized = true;
            } else if (result != kResultFalse && result != kNotImplemented && result != kNoInterface) {
                return false;
            }
        }
        if (controller) {
            if (controller->setComponentHandler(host) != kResultOk) return false;
            handler_set = true;
            if (!optional(component, INLINE_UID_OF(IConnectionPoint), component_connection) ||
                !optional(controller, INLINE_UID_OF(IConnectionPoint), controller_connection))
                return false;
            if (component_connection && controller_connection && controller_initialized) {
                if (component_connection->connect(controller_connection) != kResultOk) return false;
                connected_component = true;
                if (controller_connection->connect(component_connection) != kResultOk) return false;
                connected_controller = true;
            }
        }
        opened = true;
        return true;
    }

    // A one-shot startup operation on this exact initialized/connected owner.
    // Restore precedes configure/activation and latency facts. A failed foreign
    // call is not rollbackable and never falls back to new default state.
    bool restore_initial_state_on_control(const CapturedState& state) {
        if (![NSThread isMainThread] || !opened || !component_initialized ||
            configured || configuration_started || active || processing.load() ||
            quarantined.load() || restoration_started || !state.component ||
            state.component_bytes>StateStream::capacity ||
            state.controller_bytes>StateStream::capacity-state.component_bytes ||
            (!state.controller && state.controller_bytes) || (state.controller && !controller) ||
            state.bytes.size()!=state.component_bytes+state.controller_bytes) return false;
        restoration_started=true;
        restore_component_stream=new(std::nothrow) StateStream(state.bytes.data(),state.component_bytes);
        if (!restore_component_stream || !restore_component_stream->valid() ||
            host->apply_saved_state([&] { return component->setState(restore_component_stream); })!=kResultOk ||
            !restore_component_stream->valid() || !restore_component_stream->exclusive()) return false;
        if (controller) {
            if (!sync_controller_component_state(restore_component_stream)) return false;
            if (state.controller) {
                restore_controller_stream=new(std::nothrow) StateStream(
                    state.controller_bytes ? state.bytes.data()+state.component_bytes : nullptr,state.controller_bytes);
                if (!restore_controller_stream || !restore_controller_stream->valid() ||
                    host->apply_saved_state([&] { return controller->setState(restore_controller_stream); })!=kResultOk ||
                    !restore_controller_stream->valid() || !restore_controller_stream->exclusive()) return false;
            }
            if (host->refresh_on_control(controller).disposition!=HostDisposition::Refreshed ||
                host->refusal_flags()) return false;
        }
        restored=true;return true;
    }

    // Prepare only a fresh inactive owner. SDK arrangement refusal may itself
    // mutate foreign state; an existing active owner is never reconfigured or
    // described as rolled back. Its control owner prepares a separate candidate
    // and keeps the live owner until an actual later admission/replacement.
    ConfigurationResult configure(const ProcessingConfiguration& request) {
        if (![NSThread isMainThread] || !opened || !component_initialized || !processor ||
            active || configuration_started || processing.load() || quarantined.load())
            return ConfigurationResult::InvalidState;
        if (!valid_configuration(request)) return ConfigurationResult::InvalidConfiguration;
        // No implicit float64 conversion/cast. Any future conversion is owned
        // by the isolated helper, never by the Rust audio callback.
        if (request.precision!=kSample32) return ConfigurationResult::UnsupportedPrecision;
        configuration_started = true;
        if (processor->canProcessSampleSize(kSample32)!=kResultOk)
            return ConfigurationResult::UnsupportedPrecision;
        const auto input_count=component->getBusCount(kAudio,kInput);
        const auto output_count=component->getBusCount(kAudio,kOutput);
        // An instrument may advertise one main or auxiliary audio input. Keep
        // its native layout and provide silence in that SDK slot; the wire and
        // graph still see a zero-input instrument.
        auto sdk=request;
        const bool silent_input=!request.input_channels && input_count==1;
        BusType first_input_type=kMain;
        if (silent_input) {
            BusInfo info{};
            if (!audio_bus_info(component,kInput,0,info) || info.channelCount>2 ||
                (info.busType!=kMain && info.busType!=kAux))
                return ConfigurationResult::UnsupportedTopology;
            first_input_type=info.busType;
            sdk.input_channels=info.channelCount;
            sdk.input_enabled=true;
        }
        const bool multiple_outputs=!request.input_channels && output_count>1;
        BusInfo first_output{};
        if (!audio_buses(component,kInput,input_count,sdk.input_channels,false,first_input_type) ||
            (multiple_outputs
                ? output_count>max_instrument_outputs || !request.output_enabled ||
                    !audio_bus_info(component,kOutput,0,kMain,first_output)
                : !audio_buses(component,kOutput,output_count,request.output_channels,false)))
            return ConfigurationResult::UnsupportedTopology;
        int32 event_channels=0;
        if (!request.input_channels) {
            const auto event_count=component->getBusCount(kEvent,kInput);
            BusInfo event{};
            if (event_count!=0) {
                if (event_count!=1 || component->getBusInfo(kEvent,kInput,0,event)!=kResultOk ||
                    event.mediaType!=kEvent || event.direction!=kInput || event.busType!=kMain ||
                    event.channelCount<1 || event.channelCount>16) return ConfigurationResult::UnsupportedTopology;
                event_channels=event.channelCount;
            }
        }
        if (silent_input && !event_channels) return ConfigurationResult::UnsupportedTopology;
        auto main_input=arrangement(sdk.input_channels);
        if (silent_input &&
            (processor->getBusArrangement(kInput,0,main_input)!=kResultOk ||
             SpeakerArr::getChannelCount(main_input)!=sdk.input_channels))
            return ConfigurationResult::ArrangementMismatch;
        const auto main_output=arrangement(request.output_channels);
        SpeakerArrangement aux_input{};
        if (input_count==2 &&
            (processor->getBusArrangement(kInput,1,aux_input)!=kResultOk || !aux_input))
            return ConfigurationResult::ArrangementMismatch;
        SpeakerArrangement inputs[]{main_input,aux_input};
        InstrumentOutputLayout outputs{};
        outputs[0]=main_output;
        const auto additional_output_matches=[&](int32 index,SpeakerArrangement& actual) {
            BusInfo info{};
            return audio_bus_info(component,kOutput,index,info) &&
                (info.busType==kMain || info.busType==kAux) &&
                processor->getBusArrangement(kOutput,index,actual)==kResultOk &&
                SpeakerArr::getChannelCount(actual)==info.channelCount;
        };
        for (int32 i=1;i<output_count;++i)
            if (!additional_output_matches(i,outputs[i])) return ConfigurationResult::ArrangementMismatch;
        SpeakerArrangement observed{};
        // Some instruments refuse a redundant setBusArrangements call.
        const bool reuse_layout=(silent_input || multiple_outputs) &&
            processor->getBusArrangement(kOutput,0,observed)==kResultOk && observed==main_output;
        auto requested_outputs=outputs;
        if (!reuse_layout &&
            processor->setBusArrangements(input_count?inputs:nullptr,input_count,requested_outputs.data(),output_count)!=kResultOk)
            return ConfigurationResult::ArrangementRefused;
        if (component->getBusCount(kAudio,kInput)!=input_count ||
            component->getBusCount(kAudio,kOutput)!=output_count ||
            !audio_buses(component,kInput,input_count,sdk.input_channels,true,first_input_type) ||
            !audio_bus_info(component,kOutput,0,kMain,first_output) ||
            first_output.channelCount!=request.output_channels ||
            processor->getBusArrangement(kOutput,0,observed)!=kResultOk || observed!=main_output ||
            (input_count && (processor->getBusArrangement(kInput,0,observed)!=kResultOk || observed!=main_input)) ||
            (input_count==2 && (processor->getBusArrangement(kInput,1,observed)!=kResultOk || observed!=aux_input)))
            return ConfigurationResult::ArrangementMismatch;
        for (int32 i=1;i<output_count;++i)
            if (!additional_output_matches(i,observed) || observed!=outputs[i])
                return ConfigurationResult::ArrangementMismatch;
        ProcessSetup setup{};
        setup.processMode = kRealtime; setup.symbolicSampleSize = kSample32;
        setup.maxSamplesPerBlock = request.maximum_frames; setup.sampleRate = request.sample_rate;
        if (processor->setupProcessing(setup) != kResultOk) return ConfigurationResult::SetupRefused;
        // An inactive trailing bus needs no PCM buffer, even if default active.
        if (input_count==2 && component->activateBus(kAudio,kInput,1,false)!=kResultOk)
            return ConfigurationResult::ActivationRefused;
        for (int32 i=1;i<output_count;++i)
            if (component->activateBus(kAudio,kOutput,i,false)!=kResultOk)
                return ConfigurationResult::ActivationRefused;
        if (component->activateBus(kAudio,kOutput,0,request.output_enabled)!=kResultOk)
            return ConfigurationResult::ActivationRefused;
        output_active = request.output_enabled;
        if (sdk.input_channels) {
            if (component->activateBus(kAudio,kInput,0,sdk.input_enabled)!=kResultOk)
                return ConfigurationResult::ActivationRefused;
            input_active = sdk.input_enabled;
        }
        if (event_channels) {
            if (component->activateBus(kEvent,kInput,0,true)!=kResultOk)
                return ConfigurationResult::ActivationRefused;
            event_input_channels_=event_channels;
        }
        if (component->setActive(true) != kResultOk) return ConfigurationResult::ActivationRefused;
        configuration_=sdk;
        inactive_outputs_.prepare(output_count,outputs);
        silent_input_=silent_input;
        active = true; configured = true;
        return ConfigurationResult::Prepared;
    }
    bool configure_stereo(double rate,int32 frames,bool is_instrument) {
        ProcessingConfiguration request{};
        request.sample_rate=rate; request.maximum_frames=frames;
        request.input_channels=is_instrument?0:2; request.input_enabled=!is_instrument;
        return configure(request)==ConfigurationResult::Prepared;
    }
    const ProcessingConfiguration* configuration() const {
        return configured?&configuration_:nullptr;
    }
    // Immutable while the serialized processing lane owns the instance.
    int32 event_input_channels() const { return event_input_channels_; }

    // One initial stopped capture; later project restore/capture transactions
    // must reuse the same actual instance owner, not create a competing state path.
    enum class CaptureResult { Captured, Overflow, Refused };
    bool capture_initial_state_on_control(CapturedState& result) {
        uint32_t observed=0;
        return capture_state_on_control(result,true,observed)==CaptureResult::Captured;
    }
    // Existing activated instance, including concurrent realtime processing as
    // permitted by IComponent::getState. The main lane owns these streams;
    // the protocol must finish capture before epoch changes or teardown.
    // Reuse only exclusive control streams; never initialize/synchronize the
    // controller again or replace the original startup capture.
    CaptureResult capture_current_state_on_control(CapturedState& result,uint32_t& observed) {
        const auto outcome=capture_state_on_control(result,false,observed);
        return outcome==CaptureResult::Captured && !result.component?CaptureResult::Refused:outcome;
    }
private:
    CaptureResult capture_state_on_control(CapturedState& result,bool initial,uint32_t& observed) {
        observed=0;
        if (![NSThread isMainThread] || !configured || !active || (initial && processing.load()) || quarantined.load() ||
            (initial && (component_stream || controller_stream))) return CaptureResult::Refused;
        CapturedState captured{};
        if(initial) component_stream=new(std::nothrow) StateStream(StateStream::capacity);
        if (!component_stream || (!initial && !component_stream->reset_write_on_control(StateStream::capacity))) return CaptureResult::Refused;
        const auto first=component->getState(component_stream);
        if (!component_stream->valid() || !component_stream->exclusive()) return CaptureResult::Refused;
        if (component_stream->overflow()) {
            if(first!=kResultOk && first!=kResultFalse && first!=kInvalidArgument) return CaptureResult::Refused;
            observed=component_stream->overflow();return CaptureResult::Overflow;
        }
        if (first==kResultOk) {
            captured.component=true;captured.component_bytes=static_cast<uint32>(component_stream->size());
        } else if (first!=kNotImplemented && first!=kResultFalse) return CaptureResult::Refused;
        if (controller) {
            if (initial && !restored && captured.component) {
                if (!sync_controller_component_state(component_stream)) return CaptureResult::Refused;
                if (host->refresh_on_control(controller).disposition!=HostDisposition::Refreshed ||
                    host->refusal_flags()) return CaptureResult::Refused;
            }
            if(initial) controller_stream=new(std::nothrow) StateStream(StateStream::capacity-captured.component_bytes);
            if (!controller_stream || (!initial && !controller_stream->reset_write_on_control(StateStream::capacity-captured.component_bytes))) return CaptureResult::Refused;
            const auto second=controller->getState(controller_stream);
            if (!controller_stream->valid() || !controller_stream->exclusive()) return CaptureResult::Refused;
            if (controller_stream->overflow()) {
                if((!initial && !captured.component) ||
                    (second!=kResultOk && second!=kResultFalse && second!=kInvalidArgument)) return CaptureResult::Refused;
                observed=controller_stream->overflow();return CaptureResult::Overflow;
            }
            if (second==kResultOk) {
                captured.controller=true;captured.controller_bytes=static_cast<uint32>(controller_stream->size());
            } else if (second!=kNotImplemented && second!=kResultFalse) return CaptureResult::Refused;
        }
        captured.bytes.resize(captured.component_bytes+captured.controller_bytes);
        if(captured.component_bytes)
            std::copy_n(component_stream->bytes.begin(),captured.component_bytes,captured.bytes.begin());
        if(captured.controller_bytes)
            std::copy_n(controller_stream->bytes.begin(),captured.controller_bytes,captured.bytes.begin()+captured.component_bytes);
        result=std::move(captured);return CaptureResult::Captured;
    }
public:

    enum class PresentationResult { Absent, Applied, Refused, InvalidState };
    // SDK optional UI-thread & Activated interface. The graph/control owner
    // supplies actual per-direction origin/compensation/device contributions;
    // zero is never invented here as evidence that unknown latency is measured.
    // Query only; never apply placeholder contributions during graph planning.
    std::optional<bool> presentation_latency_available_on_control() const {
        if (![NSThread isMainThread] || !configured || !active || processing.load() || quarantined.load())
            return std::nullopt;
        IAudioPresentationLatency* view=nullptr;
        const auto queried=component->queryInterface(INLINE_UID_OF(IAudioPresentationLatency),reinterpret_cast<void**>(&view));
        if (interface_absent(queried,view)) return false;
        const bool valid=queried==kResultOk && view;
        if (view) view->release();
        if (!valid) return std::nullopt;
        return true;
    }
    PresentationResult set_presentation_latency(uint32 input,uint32 output) {
        if (![NSThread isMainThread] || !configured || !active || processing.load() || quarantined.load())
            return PresentationResult::InvalidState;
        IAudioPresentationLatency* view=nullptr;
        const auto queried=component->queryInterface(INLINE_UID_OF(IAudioPresentationLatency),reinterpret_cast<void**>(&view));
        if (interface_absent(queried,view)) return PresentationResult::Absent;
        if (queried!=kResultOk || !view) { if (view) view->release();return PresentationResult::Refused; }
        const bool ok=(silent_input_ || !configuration_.input_channels ||
            view->setAudioPresentationLatencySamples(kInput,0,input)==kResultOk) &&
            view->setAudioPresentationLatencySamples(kOutput,0,output)==kResultOk;
        view->release();return ok?PresentationResult::Applied:PresentationResult::Refused;
    }
    std::optional<uint32> reported_latency_on_control() const {
        if (![NSThread isMainThread] || !configured || !active || processing.load()) return std::nullopt;
        return processor->getLatencySamples();
    }

    // The SDK permits getTailSamples on the UI lane after setup. Keep it at
    // the same quiescent, main-lane fence as latency; no Hex callback calls SDK.
    std::optional<uint32> reported_tail_on_control() const {
        if (![NSThread isMainThread] || !configured || !active || processing.load()) return std::nullopt;
        return processor->getTailSamples();
    }

    // Explicit latency-facts renewal at the stopped main-lane barrier. Normal
    // epochs do not reset component activity or DSP history. No rollback after
    // a foreign refusal: the existing terminal owner retains this component.
    bool reactivate_on_control() {
        if (![NSThread isMainThread] || !configured || !active || processing.load() ||
            processing_fault.load() || quarantined.load()) return false;
        if (component->setActive(false)!=kResultOk) return refuse_cleanup();
        active=false;
        if (component->setActive(true)!=kResultOk) return refuse_cleanup();
        active=true;
        return true;
    }
    bool start_processing() {
        if ([NSThread isMainThread] || !configured || !active || processing.load() || processing_fault.load() || quarantined.load()) return false;
        if (processing_thread != std::thread::id{} && processing_thread != std::this_thread::get_id()) return false;
        processing_thread = std::this_thread::get_id();
        host->bind_processing_thread();
        // Steinberg's base AudioEffect leaves this notification unimplemented.
        const auto result=processor->setProcessing(true);
        if (result!=kResultOk && result!=kNotImplemented) { processing_fault.store(true); return false; }
        processing.store(true);
        return true;
    }
    bool process(ProcessData& data) {
        if (!processing.load() || processing_thread != std::this_thread::get_id() ||
            quarantined.load() || processing_fault.load() || !processing_data(configuration_,data)) return false;
        auto sdk=data;
        inactive_outputs_.attach(sdk);
        const ProcessingShape shape(sdk);
        clear_disjoint_outputs(configuration_,sdk);
        // Foreign DSP is not rollbackable. A failed call invalidates this owner;
        // only stop/close remain, never another process or restart as a retry.
        if (processor->process(sdk)!=kResultOk || !shape.unchanged(sdk) || !inactive_outputs_.unchanged()) {
            processing_fault.store(true); return false;
        }
        if (data.numOutputs) data.outputs[0].silenceFlags=sdk.outputs[0].silenceFlags;
        if (!finite_outputs(configuration_,data)) {
            processing_fault.store(true); return false;
        }
        return true;
    }
    // A parameter-only stopped flush is distinct from a positive-frame block.
    // Serialize it on the same helper worker; main/control must join before close.
    FlushResult flush_stopped(ProcessData& data) {
        return flush_parameters(data,false);
    }
    // An idle transport can retain an enabled SDK processor. The same SDK
    // lane accepts an empty-buffer flush without stopping or advancing PCM.
    FlushResult flush_idle(ProcessData& data) {
        return flush_parameters(data,true);
    }
private:
    FlushResult flush_parameters(ProcessData& data,bool enabled) {
        if ([NSThread isMainThread] || !configured || !active || processing.load()!=enabled ||
            processing_fault.load() || quarantined.load() ||
            (processing_thread!=std::thread::id{} && processing_thread!=std::this_thread::get_id()))
            return FlushResult::InvalidState;
        if (data.processMode!=kRealtime || data.symbolicSampleSize!=kSample32 || data.numSamples!=0 ||
            data.numInputs!=0 || data.numOutputs!=0 || data.inputs || data.outputs || data.inputEvents || data.outputEvents)
            return FlushResult::InvalidData;
        if (!data.inputParameterChanges) return FlushResult::Empty;
        const auto count=data.inputParameterChanges->getParameterCount();
        if (count<0 || count>64) return FlushResult::InvalidData;
        if (!count) return FlushResult::Empty;
        processing_thread=std::this_thread::get_id();
        const ProcessingShape shape(data);
        if (processor->process(data)!=kResultOk || !shape.unchanged(data)) { processing_fault.store(true); return FlushResult::Refused; }
        return FlushResult::Applied;
    }
public:
    bool stop_processing() {
        if (!processing.load() || processing_thread != std::this_thread::get_id() || quarantined.load())
            return false;
        const auto result=processor->setProcessing(false);
        if (result!=kResultOk && result!=kNotImplemented) return refuse_cleanup();
        processing.store(false);
        return true;
    }

    HostDispatch dispatch_host_callbacks() {
        if (![NSThread isMainThread]) { HostDispatch refused{}; refused.disposition=HostDisposition::WrongThread; return refused; }
        if (!opened || !host || quarantined.load()) { HostDispatch refused{}; refused.disposition=HostDisposition::Retiring; return refused; }
        return host->dispatch(controller);
    }
    IPlugView* create_editor_on_control() {
        return [NSThread isMainThread] && opened && controller && !quarantined.load()
            ? controller->createView(ViewType::kEditor) : nullptr;
    }
    const ParameterSnapshot* parameter_snapshot() const {
        return host?host->snapshot():nullptr;
    }
    const HostMessageDiagnostics* message_diagnostics_on_control() const {
        return host?&host->message_diagnostics():nullptr;
    }
    bool parameter_page_on_control(const ControllerQuery& query,ControllerParameterPage& result) {
        if(![NSThread isMainThread]) return false;
        return opened && host && !quarantined.load() &&
            controller_page(controller,host->snapshot(),query,result);
    }
    ProcessingParameterSnapshot processing_parameters_on_control() const {
        return host?host->processing_snapshot_on_control():ProcessingParameterSnapshot{};
    }
    void refresh_processing_parameters(ProcessingParameterSnapshot& current) {
        if(host) host->refresh_processing_snapshot(current);
    }
    bool publish_parameter_feedback(ParameterFeedback& pending) {
        return host && host->publish_parameter_feedback(pending);
    }
    bool synchronize_parameter_feedback_on_control() {
        return opened && host && !quarantined.load() && host->synchronize_parameter_feedback(controller);
    }
    bool peek_parameter_edit(EditEvent& result) const { return host && host->peek_edit(result); }
    void consume_parameter_edit() { if(host) host->consume_edit(); }
    uint32 ready_parameter_edits() const { return host?host->ready_parameter_edits():0; }
    uint32 pending_parameter_edits() const { return host?host->pending_parameter_edits():0; }
    uint64 host_callback_revision_on_control() const { return host?host->callback_revision_on_control():0; }
    void complete_parameter_edit() { if(host) host->complete_parameter_edit(); }
    uint32 host_refusals() const { return host?host->refusal_flags():0; }
    void observe_latency_restarts() { if(host) host->observe_latency_restarts(); }
    void finish_startup_latency_observation_on_control() {
        if(host) host->finish_startup_latency_observation_on_control();
    }
    uint64 latency_restart() const { return host?host->latency_restart():0; }
    void cover_latency_restart(uint64 covered) { if(host) host->cover_latency_restart(covered); }

    enum class CloseResult { Closed, RetainedHost, Refused };

    bool close() {
        const auto result=close_for_retirement();
        if (result==CloseResult::RetainedHost) return refuse_cleanup();
        return result==CloseResult::Closed;
    }

    // RetainedHost means all component/controller teardown and stream disposal
    // succeeded. The isolated worker may then retire by process exit while
    // keeping the stopped Host and module alive.
    CloseResult close_for_retirement() {
        if (!close_objects()) return CloseResult::Refused;
        if (host) {
            if (host->reference_count()!=1) return CloseResult::RetainedHost;
            host->release(); host=nullptr;
        }
        return CloseResult::Closed;
    }

private:
    bool close_objects() {
        if (![NSThread isMainThread] || processing.load() || quarantined.load()) return false;
        if (host) host->stop_accepting();
        // Stop/join is the caller's prerequisite. Never release/unload past a
        // refused teardown step; its foreign state cannot be assumed rolled back.
        if (active) {
            if (component->setActive(false) != kResultOk) return refuse_cleanup();
            active = false;
        }
        if (input_active) {
            if (component->activateBus(kAudio, kInput, 0, false) != kResultOk) return refuse_cleanup();
            input_active = false;
        }
        if (event_input_channels_) {
            if (component->activateBus(kEvent,kInput,0,false)!=kResultOk) return refuse_cleanup();
            event_input_channels_=0;
        }
        if (output_active) {
            if (component->activateBus(kAudio, kOutput, 0, false) != kResultOk) return refuse_cleanup();
            output_active = false;
        }
        if (connected_controller) {
            if (controller_connection->disconnect(component_connection) != kResultOk) return refuse_cleanup();
            connected_controller = false;
        }
        if (connected_component) {
            if (component_connection->disconnect(controller_connection) != kResultOk) return refuse_cleanup();
            connected_component = false;
        }
        if (handler_set) {
            if (controller->setComponentHandler(nullptr) != kResultOk) return refuse_cleanup();
            handler_set = false;
        }
        if (controller_initialized) {
            if (controller->terminate() != kResultOk) return refuse_cleanup();
            controller_initialized = false;
        }
        if (component_initialized) {
            if (component->terminate() != kResultOk) return refuse_cleanup();
            component_initialized = false;
        }
        opened = false;
        if (controller_connection) { controller_connection->release(); controller_connection = nullptr; }
        if (component_connection) { component_connection->release(); component_connection = nullptr; }
        if (controller) { controller->release(); controller = nullptr; }
        if (processor) { processor->release(); processor = nullptr; }
        if (component) { component->release(); component = nullptr; }
        // Foreign component/controller references have retired first. A retained
        // stream keeps its control lease and module until process retirement.
        if (component_stream) {
            if (!component_stream->dispose_on_control()) return refuse_cleanup();
            component_stream=nullptr;
        }
        if (controller_stream) {
            if (!controller_stream->dispose_on_control()) return refuse_cleanup();
            controller_stream=nullptr;
        }
        if (restore_component_stream) {
            if (!restore_component_stream->dispose_on_control()) return refuse_cleanup();
            restore_component_stream=nullptr;
        }
        if (restore_controller_stream) {
            if (!restore_controller_stream->dispose_on_control()) return refuse_cleanup();
            restore_controller_stream=nullptr;
        }
        if (factory) { factory->release(); factory = nullptr; }
        return true;
    }
};
} // namespace hex_instance
