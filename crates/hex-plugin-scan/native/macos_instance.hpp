// Native-helper-only component/controller ownership. Original Hex implementation.
// The admitted module outlives this owner and may unload only after close() succeeds.
// UI methods require the helper's main thread. Processing methods require one
// serialized worker; its joining control owner must join before close(). No ABI
// pointer crosses into Rust. This is not installed-module or audio-bridge admission.
#pragma once
#include <Foundation/Foundation.h>
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "macos_host.hpp"
#include "macos_processing.hpp"
#include "pluginterfaces/vst/ivstmessage.h"
#include <atomic>
#include <cmath>
#include <cstring>
#include <new>
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
    bool configured = false, configuration_started = false, opened = false, used = false;
    ProcessingConfiguration configuration_{};
    std::thread::id processing_thread;
    std::atomic<bool> processing{false}, processing_fault{false}, quarantined{false};

    template<typename T> static bool optional(FUnknown* object, const TUID iid, T*& out) {
        const auto result = object->queryInterface(iid, reinterpret_cast<void**>(&out));
        return (result == kResultOk && out) || (result == kNoInterface && !out);
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
    bool refuse_cleanup() { quarantined.store(true); return false; }
public:
    Instance() = default;
    Instance(const Instance&) = delete;
    Instance& operator=(const Instance&) = delete;
    // No foreign calls during C++ destruction. A failed/omitted close deliberately
    // retains references in this isolated helper until its supervisor reaps it;
    // the caller must also retain its module rather than unload live objects.
    ~Instance() = default;

    bool open(IPluginFactory* admitted_factory, const TUID class_id) {
        if (![NSThread isMainThread] || used || !admitted_factory || quarantined.load())
            return false;
        used = true;
        factory = admitted_factory;
        factory->addRef();
        host = new(std::nothrow) Host();
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
        if (!main_bus(component,kInput,request.input_channels,false) ||
            !main_bus(component,kOutput,request.output_channels,false))
            return ConfigurationResult::UnsupportedTopology;
        auto input=arrangement(request.input_channels),output=arrangement(request.output_channels);
        if (processor->setBusArrangements(request.input_channels?&input:nullptr,
            request.input_channels?1:0,&output,1)!=kResultOk)
            return ConfigurationResult::ArrangementRefused;
        SpeakerArrangement observed{};
        if (!main_bus(component,kInput,request.input_channels,true) ||
            !main_bus(component,kOutput,request.output_channels,true) ||
            processor->getBusArrangement(kOutput,0,observed)!=kResultOk || observed!=output ||
            (request.input_channels && (processor->getBusArrangement(kInput,0,observed)!=kResultOk || observed!=input)))
            return ConfigurationResult::ArrangementMismatch;
        ProcessSetup setup{};
        setup.processMode = kRealtime; setup.symbolicSampleSize = kSample32;
        setup.maxSamplesPerBlock = request.maximum_frames; setup.sampleRate = request.sample_rate;
        if (processor->setupProcessing(setup) != kResultOk) return ConfigurationResult::SetupRefused;
        if (component->activateBus(kAudio,kOutput,0,request.output_enabled)!=kResultOk)
            return ConfigurationResult::ActivationRefused;
        output_active = request.output_enabled;
        if (request.input_channels) {
            if (component->activateBus(kAudio,kInput,0,request.input_enabled)!=kResultOk)
                return ConfigurationResult::ActivationRefused;
            input_active = request.input_enabled;
        }
        if (component->setActive(true) != kResultOk) return ConfigurationResult::ActivationRefused;
        configuration_=request;
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

    bool start_processing() {
        if ([NSThread isMainThread] || !configured || !active || processing.load() || processing_fault.load() || quarantined.load()) return false;
        if (processing_thread != std::thread::id{} && processing_thread != std::this_thread::get_id()) return false;
        processing_thread = std::this_thread::get_id();
        if (processor->setProcessing(true) != kResultOk) { processing_fault.store(true); return false; }
        processing.store(true);
        return true;
    }
    bool process(ProcessData& data) {
        if (!processing.load() || processing_thread != std::this_thread::get_id() ||
            quarantined.load() || processing_fault.load() || !processing_data(configuration_,data)) return false;
        const ProcessingShape shape(data);
        clear_disjoint_outputs(configuration_,data);
        // Foreign DSP is not rollbackable. A failed call invalidates this owner;
        // only stop/close remain, never another process or restart as a retry.
        if (processor->process(data) != kResultOk || !shape.unchanged(data) || !finite_outputs(configuration_,data)) {
            processing_fault.store(true); return false;
        }
        return true;
    }
    // A parameter-only stopped flush is distinct from a positive-frame block.
    // Serialize it on the same helper worker; main/control must join before close.
    FlushResult flush_stopped(ProcessData& data) {
        if ([NSThread isMainThread] || !configured || !active || processing.load() ||
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
        if (processor->process(data)!=kResultOk) { processing_fault.store(true); return FlushResult::Refused; }
        return FlushResult::Applied;
    }
    bool stop_processing() {
        if (!processing.load() || processing_thread != std::this_thread::get_id() || quarantined.load())
            return false;
        if (processor->setProcessing(false) != kResultOk) return refuse_cleanup();
        processing.store(false);
        return true;
    }

    HostDispatch dispatch_host_callbacks() {
        if (![NSThread isMainThread]) { HostDispatch refused{}; refused.disposition=HostDisposition::WrongThread; return refused; }
        if (!opened || !host || quarantined.load()) { HostDispatch refused{}; refused.disposition=HostDisposition::Retiring; return refused; }
        return host->dispatch(controller);
    }
    const ParameterSnapshot* parameter_snapshot() const {
        return host?host->snapshot():nullptr;
    }
    uint32 host_refusals() const { return host?host->refusal_flags():0; }

    bool close() {
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
        if (factory) { factory->release(); factory = nullptr; }
        if (host) {
            // Keep the control lease when a message/context is still retained.
            // Its last release on another thread cannot finally free this pool.
            if (host->reference_count()!=1) return refuse_cleanup();
            host->release(); host=nullptr;
        }
        return true;
    }
};
} // namespace hex_instance
