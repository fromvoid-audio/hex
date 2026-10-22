// Native-helper-only component/controller ownership. Original Hex implementation.
// The admitted module outlives this owner and may unload only after close() succeeds.
// UI methods require the helper's main thread. Processing methods require one
// serialized worker; its joining control owner must join before close(). No ABI
// pointer crosses into Rust. This is not installed-module or audio-bridge admission.
#pragma once
#include <Foundation/Foundation.h>
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivsthostapplication.h"
#include "pluginterfaces/vst/ivstmessage.h"
#include <atomic>
#include <cmath>
#include <cstring>
#include <new>
#include <thread>

namespace hex_instance {
using namespace Steinberg;
using namespace Steinberg::Vst;

inline bool same(const TUID left, const TUID right) {
    return std::memcmp(left, right, sizeof(TUID)) == 0;
}

// A deliberately finite host context. Unsupported message factories and edit
// callbacks report unsupported; they never pretend a canonical edit was applied.
// Their product implementations belong to later host-capability/parameter leaves.
class Host final : public IHostApplication, public IComponentHandler {
    std::atomic<uint32> references{1};
public:
    uint32 PLUGIN_API addRef() override { return ++references; }
    uint32 PLUGIN_API release() override {
        auto remaining = --references;
        if (!remaining) delete this;
        return remaining;
    }
    tresult PLUGIN_API queryInterface(const TUID iid, void** out) override {
        if (!out) return kInvalidArgument;
        *out = nullptr;
        if (same(iid, INLINE_UID_OF(FUnknown)) || same(iid, INLINE_UID_OF(IHostApplication)))
            *out = static_cast<IHostApplication*>(this);
        else if (same(iid, INLINE_UID_OF(IComponentHandler)))
            *out = static_cast<IComponentHandler*>(this);
        else return kNoInterface;
        addRef();
        return kResultOk;
    }
    tresult PLUGIN_API getName(String128 name) override {
        if (![NSThread isMainThread] || !name) return kInvalidArgument;
        std::memset(name, 0, sizeof(String128));
        const char16_t label[] = u"hex";
        std::memcpy(name, label, sizeof(label));
        return kResultOk;
    }
    tresult PLUGIN_API createInstance(TUID, TUID, void** out) override {
        if (!out) return kInvalidArgument;
        *out = nullptr;
        return kNoInterface;
    }
    tresult PLUGIN_API beginEdit(ParamID) override { return kNotImplemented; }
    tresult PLUGIN_API performEdit(ParamID, ParamValue) override { return kNotImplemented; }
    tresult PLUGIN_API endEdit(ParamID) override { return kNotImplemented; }
    tresult PLUGIN_API restartComponent(int32) override { return kNotImplemented; }
};

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
    bool instrument = false, configured = false, configuration_started = false, opened = false, used = false;
    int32 maximum = 0;
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

    // Explicit initial stereo/sample32 capability, not general bus negotiation.
    bool configure_stereo(double rate, int32 frames, bool is_instrument) {
        if (![NSThread isMainThread] || !opened || !component_initialized || !processor ||
            active || configuration_started || quarantined.load() || !std::isfinite(rate) ||
            rate <= 0 || rate > 768000 || frames < 1 || frames > 8192) return false;
        configuration_started = true;
        SpeakerArrangement stereo = SpeakerArr::kStereo;
        ProcessSetup setup{};
        setup.processMode = kRealtime; setup.symbolicSampleSize = kSample32;
        setup.maxSamplesPerBlock = frames; setup.sampleRate = rate;
        if (processor->setBusArrangements(is_instrument ? nullptr : &stereo,
            is_instrument ? 0 : 1, &stereo, 1) != kResultOk ||
            processor->setupProcessing(setup) != kResultOk) return false;
        instrument = is_instrument; maximum = frames;
        if (component->activateBus(kAudio, kOutput, 0, true) != kResultOk) return false;
        output_active = true;
        if (!instrument) {
            if (component->activateBus(kAudio, kInput, 0, true) != kResultOk) return false;
            input_active = true;
        }
        if (component->setActive(true) != kResultOk) return false;
        active = true; configured = true;
        return true;
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
            quarantined.load() || processing_fault.load() || data.numSamples < 1 || data.numSamples > maximum ||
            data.processMode != kRealtime || data.symbolicSampleSize != kSample32 ||
            data.numInputs != (instrument ? 0 : 1) || data.numOutputs != 1) return false;
        // Foreign DSP is not rollbackable. A failed call invalidates this owner;
        // only stop/close remain, never another process or restart as a retry.
        if (processor->process(data) != kResultOk) { processing_fault.store(true); return false; }
        return true;
    }
    bool stop_processing() {
        if (!processing.load() || processing_thread != std::this_thread::get_id() || quarantined.load())
            return false;
        if (processor->setProcessing(false) != kResultOk) return refuse_cleanup();
        processing.store(false);
        return true;
    }

    bool close() {
        if (![NSThread isMainThread] || processing.load() || quarantined.load()) return false;
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
            const auto outstanding = host->release();
            host = nullptr;
            if (outstanding) return refuse_cleanup();
        }
        return true;
    }
};
} // namespace hex_instance
