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
#include <chrono>
#include <time.h>
#include <cmath>
#include "pluginterfaces/vst/ivstnoteexpression.h"
#include <cstring>
#include <limits>
#include <new>
#include <optional>
#include <thread>

namespace hex_instance {
using namespace Steinberg;
using namespace Steinberg::Vst;

struct NoteExpressionCapabilities {
    uint16_t tuning=0, brightness=0, unreadable=0;
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
    bool silent_input_ = false;
    int32 event_input_channels_ = 0, event_output_channels_ = 0;
    int32 aux_input_channels_ = 0;
    int32 output_bus_count_ = 1;
    // SDK descriptors copied during final layout verification, before processing.
    std::array<BusInfo,max_instrument_outputs> output_bus_info_{};
    uint32_t active_aux_outputs_mask_ = 0;
    bool configured = false, configuration_started = false, opened = false, used = false;
    ProcessingConfiguration configuration_{};
    AuxInputBus<Sample32> aux_input_;
    InstrumentOutputs<Sample32> instrument_outputs_;
    std::unique_ptr<DoubleProcessing> double_processing_;
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

    bool open(IPluginFactory* admitted_factory,const TUID class_id,bool trace_messages=false,bool editor_groups=false,bool instrument=false) {
        if (![NSThread isMainThread] || used || !admitted_factory || quarantined.load())
            return false;
        used = true;
        factory = admitted_factory;
        factory->addRef();
        host = new(std::nothrow) Host(trace_messages,editor_groups,instrument);
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
        // The wire stays float. Only a double-only processor needs conversion.
        if (request.precision!=kSample32) return ConfigurationResult::UnsupportedPrecision;
        configuration_started = true;
        if (processor->canProcessSampleSize(kSample32)!=kResultOk) {
            if (processor->canProcessSampleSize(kSample64)!=kResultOk)
                return ConfigurationResult::UnsupportedPrecision;
            double_processing_.reset(new (std::nothrow) DoubleProcessing);
            if (!double_processing_ || !double_processing_->reserve(request.maximum_frames))
                return ConfigurationResult::SetupRefused;
        }
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
        const auto event_count=component->getBusCount(kEvent,kInput);
        // Instruments require a supported layout. Effects may keep processing
        // audio when bus zero is absent or cannot supply main MIDI events.
        if (!request.input_channels && (event_count<0 || event_count>1))
            return ConfigurationResult::UnsupportedTopology;
        if (event_count>0) {
            BusInfo event{};
            if (component->getBusInfo(kEvent,kInput,0,event)==kResultOk &&
                event.mediaType==kEvent && event.direction==kInput && event.busType==kMain &&
                event.channelCount>=1 && event.channelCount<=16) {
                event_channels=event.channelCount;
            } else if (!request.input_channels) {
                return ConfigurationResult::UnsupportedTopology;
            }
        }
        int32 output_event_channels=0;
        const auto output_event_buses=component->getBusCount(kEvent,kOutput);
        if(output_event_buses<0 || output_event_buses>1) return ConfigurationResult::UnsupportedTopology;
        if(output_event_buses==1) {
            BusInfo info{};
            if(component->getBusInfo(kEvent,kOutput,0,info)!=kResultOk || info.mediaType!=kEvent ||
                info.direction!=kOutput || info.busType!=kMain || info.channelCount<1 || info.channelCount>16)
                return ConfigurationResult::UnsupportedTopology;
            output_event_channels=info.channelCount;
        }
        if (silent_input && !event_channels) return ConfigurationResult::UnsupportedTopology;
        auto main_input=arrangement(sdk.input_channels);
        if (silent_input &&
            (processor->getBusArrangement(kInput,0,main_input)!=kResultOk ||
             SpeakerArr::getChannelCount(main_input)!=sdk.input_channels))
            return ConfigurationResult::ArrangementMismatch;
        const auto main_output=arrangement(request.output_channels);
        AudioInputLayout inputs{};
        if (input_count) inputs[0]=main_input;
        const auto auxiliary_input_matches=[&](int32 index,SpeakerArrangement& actual) {
            BusInfo info{};
            return audio_bus_info(component,kInput,index,kAux,info) &&
                processor->getBusArrangement(kInput,index,actual)==kResultOk &&
                actual && SpeakerArr::getChannelCount(actual)==info.channelCount;
        };
        for (int32 i=1;i<input_count;++i)
            if (!auxiliary_input_matches(i,inputs[i])) return ConfigurationResult::ArrangementMismatch;
        InstrumentOutputLayout outputs{};
        outputs[0]=main_output;
        const auto additional_output_matches=[&](int32 index,SpeakerArrangement& actual) {
            BusInfo info{};
            if (!(audio_bus_info(component,kOutput,index,info) &&
                (info.busType==kMain || info.busType==kAux) &&
                processor->getBusArrangement(kOutput,index,actual)==kResultOk &&
                SpeakerArr::getChannelCount(actual)==info.channelCount)) return false;
            output_bus_info_[index]=info;
            return true;
        };
        for (int32 i=1;i<output_count;++i)
            if (!additional_output_matches(i,outputs[i])) return ConfigurationResult::ArrangementMismatch;
        SpeakerArrangement observed{};
        // Some instruments refuse a redundant setBusArrangements call.
        const bool reuse_layout=(silent_input || multiple_outputs) &&
            processor->getBusArrangement(kOutput,0,observed)==kResultOk && observed==main_output;
        auto requested_inputs=inputs;
        auto requested_outputs=outputs;
        if (!reuse_layout &&
            processor->setBusArrangements(input_count?requested_inputs.data():nullptr,input_count,
                                          requested_outputs.data(),output_count)!=kResultOk)
            return ConfigurationResult::ArrangementRefused;
        if (component->getBusCount(kAudio,kInput)!=input_count ||
            component->getBusCount(kAudio,kOutput)!=output_count ||
            !audio_buses(component,kInput,input_count,sdk.input_channels,true,first_input_type) ||
            !audio_bus_info(component,kOutput,0,kMain,first_output) ||
            first_output.channelCount!=request.output_channels ||
            processor->getBusArrangement(kOutput,0,observed)!=kResultOk || observed!=main_output ||
            (input_count && (processor->getBusArrangement(kInput,0,observed)!=kResultOk || observed!=main_input)))
            return ConfigurationResult::ArrangementMismatch;
        for (int32 i=1;i<input_count;++i)
            if (!auxiliary_input_matches(i,observed) || observed!=inputs[i])
                return ConfigurationResult::ArrangementMismatch;
        for (int32 i=1;i<output_count;++i)
            if (!additional_output_matches(i,observed) || observed!=outputs[i])
                return ConfigurationResult::ArrangementMismatch;
        ProcessSetup setup{};
        setup.processMode = request.process_mode; setup.symbolicSampleSize = double_processing_?kSample64:kSample32;
        setup.maxSamplesPerBlock = request.maximum_frames; setup.sampleRate = request.sample_rate;
        if (processor->setupProcessing(setup) != kResultOk) return ConfigurationResult::SetupRefused;
        // Activate the first stereo/mono kAux sidechain input bus on effects
        // when exposed; keep any additional trailing input buses inactive.
        int32 aux_in_channels=0;
        for (int32 i=1;i<input_count;++i) {
            BusInfo aux_info{};
            if (i==1 && request.input_channels>0 &&
                audio_bus_info(component,kInput,1,kAux,aux_info) &&
                aux_info.channelCount>=1 && aux_info.channelCount<=2 &&
                component->activateBus(kAudio,kInput,1,true)==kResultOk) {
                aux_in_channels=aux_info.channelCount;
            } else if (component->activateBus(kAudio,kInput,i,false)!=kResultOk) {
                return ConfigurationResult::ActivationRefused;
            }
        }
        // Activate multi-out stereo/mono instrument buses when exposed so
        // voices routed to auxiliary buses render into real buffers; decline
        // or unsupported channel widths fall back to inactive null slots.
        uint32_t active_out_mask=0;
        for (int32 i=1;i<output_count;++i) {
            const int32 bus_channels=SpeakerArr::getChannelCount(outputs[i]);
            if (multiple_outputs && bus_channels>=1 && bus_channels<=2 &&
                component->activateBus(kAudio,kOutput,i,true)==kResultOk) {
                active_out_mask|=(uint32_t{1}<<i);
            } else if (component->activateBus(kAudio,kOutput,i,false)!=kResultOk) {
                return ConfigurationResult::ActivationRefused;
            }
        }
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
        if(output_event_channels) {
            if(component->activateBus(kEvent,kOutput,0,true)!=kResultOk) return ConfigurationResult::ActivationRefused;
            event_output_channels_=output_event_channels;
        }
        if (component->setActive(true) != kResultOk) return ConfigurationResult::ActivationRefused;
        configuration_=sdk;
        aux_input_channels_=aux_in_channels;
        output_bus_count_=output_count;
        output_bus_info_[0]=first_output;
        active_aux_outputs_mask_=active_out_mask;
        if (double_processing_) {
            if (!double_processing_->aux_input.prepare(aux_in_channels,request.maximum_frames) ||
                !double_processing_->instrument_outputs.prepare(output_count,outputs,active_out_mask,request.maximum_frames))
                return ConfigurationResult::SetupRefused;
        } else {
            if (!aux_input_.prepare(aux_in_channels,request.maximum_frames) ||
                !instrument_outputs_.prepare(output_count,outputs,active_out_mask,request.maximum_frames))
                return ConfigurationResult::SetupRefused;
        }
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
    int32 event_output_channels() const { return event_output_channels_; }
    int32 output_bus_count() const { return output_bus_count_; }
    bool negotiated_output_bus(int32 index,BusInfo& info,bool& active_bus) const {
        if (!configured || index<0 || index>=output_bus_count_ || index>=max_instrument_outputs) return false;
        info=output_bus_info_[index];
        active_bus=index==0?output_active:(active_aux_outputs_mask_&(uint32_t{1}<<index))!=0;
        return true;
    }
    int32 event_input_channels() const { return event_input_channels_; }
    int32 aux_input_channels() const { return aux_input_channels_; }
    uint32_t active_aux_outputs_mask() const { return active_aux_outputs_mask_; }

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
            if (initial) {
                const bool sync_component=!restored && captured.component;
                if (sync_component && !sync_controller_component_state(component_stream))
                    return CaptureResult::Refused;
                // Restored controller values were read before bus negotiation. Publish
                // effect assignments only after the main event input is active.
                const bool effect_midi=configuration_.input_channels && !silent_input_ && event_input_channels_;
                if (effect_midi) host->set_effect_midi_channels_on_control(static_cast<int16>(event_input_channels_));
                if ((sync_component || effect_midi) &&
                    (host->refresh_on_control(controller).disposition!=HostDisposition::Refreshed ||
                     host->refusal_flags())) return CaptureResult::Refused;
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
            std::copy_n(component_stream->payload().begin(),captured.component_bytes,captured.bytes.begin());
        if(captured.controller_bytes)
            std::copy_n(controller_stream->payload().begin(),captured.controller_bytes,captured.bytes.begin()+captured.component_bytes);
        result=std::move(captured);return CaptureResult::Captured;
    }
public:

    enum class PresentationResult { Absent, Applied, Refused, InvalidState };
    // SDK optional UI-thread & Activated interface. The graph/control owner
    // supplies actual per-direction origin/compensation/device contributions;
    // zero is never invented here as evidence that unknown latency is measured.
    // Query only; never apply placeholder contributions during graph planning.
    // Called only on the control lane before a stopped facts response. A broken
    // optional inventory cannot revoke ordinary note playback.
    std::optional<NoteExpressionCapabilities> note_expression_capabilities_on_control() const {
        if (![NSThread isMainThread] || !configured || !active || processing.load() || quarantined.load())
            return std::nullopt;
        NoteExpressionCapabilities result;
        if (!controller || !event_input_channels_) return result;
        const auto channels=event_input_channels_;
        const auto active=static_cast<uint16_t>((uint32_t{1}<<channels)-1);
        INoteExpressionController* expressions=nullptr;
        const auto queried=controller->queryInterface(INLINE_UID_OF(INoteExpressionController),
                                                     reinterpret_cast<void**>(&expressions));
        if (interface_absent(queried,expressions)) return result;
        if (queried!=kResultOk || !expressions) {
            if (expressions) expressions->release();
            result.unreadable=active;
            return result;
        }
        constexpr int32 max_types_per_channel=256;
        for (int16 channel=0;channel<channels;++channel) {
            const auto bit=static_cast<uint16_t>(uint32_t{1}<<channel);
            const auto count=expressions->getNoteExpressionCount(0,channel);
            bool valid=count>=0 && count<=max_types_per_channel;
            bool tuning=false,brightness=false;
            for (int32 index=0;valid && index<count;++index) {
                NoteExpressionTypeInfo info{};
                valid=expressions->getNoteExpressionInfo(0,channel,index,info)==kResultOk &&
                    info.typeId!=kInvalidTypeID;
                if (!valid || (info.typeId!=kTuningTypeID && info.typeId!=kBrightnessTypeID)) continue;
                bool& found=info.typeId==kTuningTypeID?tuning:brightness;
                const auto& range=info.valueDesc;
                valid=!found && 0.0<=range.minimum && range.minimum<=range.defaultValue &&
                    range.defaultValue<=range.maximum && range.maximum<=1.0 && range.stepCount>=0;
                found=true;
            }
            if (!valid) result.unreadable|=bit;
            else {
                if (tuning) result.tuning|=bit;
                if (brightness) result.brightness|=bit;
            }
        }
        expressions->release();
        return result;
    }
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
        bool ok=(silent_input_ || !configuration_.input_channels ||
            view->setAudioPresentationLatencySamples(kInput,0,input)==kResultOk) &&
            view->setAudioPresentationLatencySamples(kOutput,0,output)==kResultOk;
        // All admitted instrument auxiliaries currently mix into the same
        // monitored stereo output and share its downstream presentation delay.
        for (int32 bus=1;ok && bus<output_bus_count_;++bus) {
            if (active_aux_outputs_mask_ & (uint32_t{1}<<bus))
                ok=view->setAudioPresentationLatencySamples(kOutput,bus,output)==kResultOk;
        }
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
    bool process(ProcessData& data,uint64_t* sdk_process_ns=nullptr,
                 const float* aux_left=nullptr,const float* aux_right=nullptr,
                 uint64_t* sdk_thread_cpu_ns=nullptr) {
        if (!processing.load() || processing_thread != std::this_thread::get_id() ||
            quarantined.load() || processing_fault.load() || !processing_data(configuration_,data)) return false;
        auto sdk=data;
        clear_disjoint_outputs(configuration_,data);
        if (double_processing_) {
            double_processing_->attach(configuration_,sdk);
            if (!process_checked(sdk,double_processing_->aux_input,double_processing_->instrument_outputs,
                                 aux_left,aux_right,sdk_process_ns,sdk_thread_cpu_ns)) return false;
            if (configuration_.output_enabled) {
                double_processing_->instrument_outputs.mix_into_main(sdk.outputs[0],sdk.numSamples);
                double_processing_->copy_outputs(data);
            }
        } else {
            if (!process_checked(sdk,aux_input_,instrument_outputs_,aux_left,aux_right,sdk_process_ns,sdk_thread_cpu_ns)) return false;
            if (configuration_.output_enabled)
                instrument_outputs_.mix_into_main(sdk.outputs[0],sdk.numSamples);
        }
        if (data.numOutputs) data.outputs[0].silenceFlags=sdk.outputs[0].silenceFlags;
        if (!finite_outputs(configuration_,data)) { processing_fault.store(true); return false; }
        return true;
    }
private:
    template<class Sample> bool process_checked(ProcessData& sdk,AuxInputBus<Sample>& aux_input,
                                                InstrumentOutputs<Sample>& outputs,
                                                const float* aux_left,const float* aux_right,
                                                uint64_t* sdk_process_ns,uint64_t* sdk_thread_cpu_ns) {
        aux_input.attach(sdk,aux_left,aux_right);
        outputs.attach(sdk);
        const ProcessingShape<Sample> shape(sdk);
        // Foreign DSP is not rollbackable. A failed call invalidates this owner;
        // only stop/close remain, never another process or restart as a retry.
        const auto cpu_start=sdk_thread_cpu_ns?clock_gettime_nsec_np(CLOCK_THREAD_CPUTIME_ID):0;
        const auto start=sdk_process_ns?std::chrono::steady_clock::now():std::chrono::steady_clock::time_point{};
        const auto result=processor->process(sdk);
        if(sdk_process_ns) {
            const auto elapsed=std::chrono::duration_cast<std::chrono::nanoseconds>(std::chrono::steady_clock::now()-start).count();
            const auto ns=elapsed>0?static_cast<uint64_t>(elapsed):0;
            *sdk_process_ns=ns>std::numeric_limits<uint64_t>::max()-*sdk_process_ns?
                std::numeric_limits<uint64_t>::max():*sdk_process_ns+ns;
        }
        if(sdk_thread_cpu_ns) {
            const auto cpu_end=clock_gettime_nsec_np(CLOCK_THREAD_CPUTIME_ID);
            // Any unavailable slice makes the complete quantum unavailable.
            if(!cpu_start || cpu_end<cpu_start || cpu_end-cpu_start>=UINT64_MAX-*sdk_thread_cpu_ns)
                *sdk_thread_cpu_ns=UINT64_MAX;
            else
                *sdk_thread_cpu_ns+=cpu_end-cpu_start;
        }
        if (result!=kResultOk || !shape.unchanged(sdk) || !aux_input.unchanged(sdk) || !outputs.unchanged()) {
            processing_fault.store(true); return false;
        }
        return true;
    }
public:
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
        if (data.processMode!=configuration_.process_mode || data.symbolicSampleSize!=kSample32 || data.numSamples!=0 ||
            data.numInputs!=0 || data.numOutputs!=0 || data.inputs || data.outputs || data.inputEvents || data.outputEvents)
            return FlushResult::InvalidData;
        if (!data.inputParameterChanges) return FlushResult::Empty;
        const auto count=data.inputParameterChanges->getParameterCount();
        if (count<0 || count>64) return FlushResult::InvalidData;
        if (!count) return FlushResult::Empty;
        processing_thread=std::this_thread::get_id();
        auto sdk=data;
        if (double_processing_) sdk.symbolicSampleSize=kSample64;
        // Flush has no audio buses. The shape checks the negotiated precision
        // and all parameter/event pointers without accessing sample storage.
        const ProcessingShape<Sample32> shape(sdk);
        if (processor->process(sdk)!=kResultOk || !shape.unchanged(sdk)) { processing_fault.store(true); return FlushResult::Refused; }
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
    bool processing_parameters_on_control(ProcessingParameterSnapshot& target) const {
        return host && host->processing_snapshot_on_control(target);
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
    bool peek_parameter_edit(EditEvent& result,size_t offset=0) const { return host && host->peek_edit(result,offset); }
    void consume_parameter_edit() { if(host) host->consume_edit(); }
    uint32 ready_parameter_edits() const { return host?host->ready_parameter_edits():0; }
    uint32 pending_parameter_edits() const { return host?host->pending_parameter_edits():0; }
    bool seal_editor_edits_on_control() {
        return opened && host && !quarantined.load() && host->seal_editor_edits_on_control();
    }
    bool hold_editor_admission_on_control(uint64 ordinal) {
        return opened && host && !quarantined.load() && host->hold_editor_admission_on_control(ordinal);
    }
    bool release_editor_admission_on_control(uint64 ordinal) {
        return opened && host && !quarantined.load() && host->release_editor_admission_on_control(ordinal);
    }
    EditorDetachAdmission begin_editor_detach_on_control() {
        if(!opened || !host || quarantined.load()) return EditorDetachAdmission::Refused;
        return host->begin_editor_detach_on_control();
    }
    void end_editor_detach_on_control() {
        if(host) host->end_editor_detach_on_control();
    }
    bool editor_edits_sealed_on_control() const {
        return host && host->editor_edits_sealed_on_control();
    }
    uint64 host_callback_revision_on_control() const { return host?host->callback_revision_on_control():0; }
    uint64 editor_admission_sequence_on_worker() const {
        return host?host->editor_admission_sequence_on_worker():0;
    }
    uint64 editor_frontier_on_worker() const { return host?host->editor_frontier_on_worker():0; }
    uint64 value_restart_frontier_on_worker() const { return host?host->value_restart_frontier_on_worker():0; }
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
    CloseResult close_for_retirement(bool clean_stop=false) {
        if (!close_objects(clean_stop)) return CloseResult::Refused;
        if (host) {
            if (host->reference_count()!=1) return CloseResult::RetainedHost;
            host->release(); host=nullptr;
        }
        return CloseResult::Closed;
    }

private:
    bool close_objects(bool clean_stop) {
        if (![NSThread isMainThread] || processing.load() || quarantined.load()) return false;
        if (host) host->stop_accepting();
        // Stop/join is the caller's prerequisite. Never release/unload past a
        // refused teardown step; its foreign state cannot be assumed rolled back.
        if (active) {
            if (component->setActive(false) != kResultOk) return refuse_cleanup();
            active = false;
        }
        // After a clean STOP, the joined worker and inactive component need no
        // further bus reconfiguration before checked termination. Partial or
        // failed setup retains the existing conservative cleanup path.
        if (clean_stop) {
            input_active = false;
            aux_input_channels_ = 0;
            active_aux_outputs_mask_ = 0;
            event_input_channels_ = 0;
            event_output_channels_ = 0;
            output_active = false;
        } else {
            if (aux_input_channels_) {
                if (component->activateBus(kAudio, kInput, 1, false) != kResultOk) return refuse_cleanup();
                aux_input_channels_ = 0;
            }
            for (int32 i = 1; i < output_bus_count_; ++i) {
                if (active_aux_outputs_mask_ & (uint32_t{1} << i)) {
                    if (component->activateBus(kAudio, kOutput, i, false) != kResultOk) return refuse_cleanup();
                }
            }
            active_aux_outputs_mask_ = 0;
            if (input_active) {
                if (component->activateBus(kAudio, kInput, 0, false) != kResultOk) return refuse_cleanup();
                input_active = false;
            }
            if(event_output_channels_) {
                if(component->activateBus(kEvent,kOutput,0,false)!=kResultOk) return refuse_cleanup();
                event_output_channels_=0;
            }
            if (event_input_channels_) {
                if (component->activateBus(kEvent,kInput,0,false)!=kResultOk) return refuse_cleanup();
                event_input_channels_=0;
            }
            if (output_active) {
                if (component->activateBus(kAudio, kOutput, 0, false) != kResultOk) return refuse_cleanup();
                output_active = false;
            }
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
