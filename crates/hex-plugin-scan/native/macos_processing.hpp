// Original bounded native processing contract. SDK pointers/storage stay in
// the isolated helper. This does not admit installed devices or graph routes.
#pragma once
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstparameterchanges.h"
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <limits>

namespace hex_instance {
using namespace Steinberg;
using namespace Steinberg::Vst;

struct ProcessingConfiguration {
    double sample_rate=48000;
    int32 maximum_frames=64;
    int32 input_channels=2, output_channels=2;
    int32 precision=kSample32;
    bool input_enabled=true, output_enabled=true;
};
enum class ConfigurationResult {
    Prepared, InvalidState, InvalidConfiguration, UnsupportedPrecision,
    UnsupportedTopology, ArrangementRefused, ArrangementMismatch, SetupRefused,
    ActivationRefused
};
enum class FlushResult { Applied, Empty, InvalidState, InvalidData, Refused };

inline bool valid_configuration(const ProcessingConfiguration& c) {
    return std::isfinite(c.sample_rate) && c.sample_rate>0 && c.sample_rate<=768000 &&
        c.maximum_frames>=1 && c.maximum_frames<=8192 &&
        c.input_channels>=0 && c.input_channels<=2 && c.output_channels>=1 && c.output_channels<=2 &&
        (c.input_channels!=0 || !c.input_enabled);
}
inline SpeakerArrangement arrangement(int32 channels) {
    return channels==1?SpeakerArr::kMono:SpeakerArr::kStereo;
}
inline bool audio_bus_info(IComponent* component,BusDirection direction,int32 index,BusType type,BusInfo& info) {
    return component->getBusInfo(kAudio,direction,index,info)==kResultOk && info.mediaType==kAudio &&
        info.direction==direction && info.busType==type && info.channelCount>0 && info.channelCount<=64 &&
        !(info.flags&BusInfo::kIsControlVoltage);
}
// Only one trailing auxiliary input is supported, and it is never routed.
inline bool audio_buses(IComponent* component,BusDirection direction,int32 count,int32 channels,bool exact) {
    if (count!=(channels?1:0) && !(direction==kInput && channels && count==2)) return false;
    if (!count) return true;
    BusInfo main{},aux{};
    return audio_bus_info(component,direction,0,kMain,main) && (!exact || main.channelCount==channels) &&
        (count==1 || audio_bus_info(component,direction,1,kAux,aux));
}
inline bool bus_buffers(AudioBusBuffers* buses,int32 count,int32 channels,bool enabled) {
    if (!channels) return count==0;
    // The only supported bus is index zero. Only an inactive trailing bus may
    // be omitted; no slot is ever removed from the middle of an SDK bus array.
    if (!enabled && count==0) return true;
    if (count!=1 || !buses || buses[0].numChannels!=channels || !buses[0].channelBuffers32 ||
        (buses[0].silenceFlags&~((uint64_t(1)<<channels)-1))) return false;
    if (enabled) for (int32 channel=0;channel<channels;++channel)
        if (!buses[0].channelBuffers32[channel]) return false;
    // Silence is a hint; it never makes an active null sample buffer legal.
    return true;
}
inline bool partial_overlap(const float* a,const float* b,int32 frames) {
    if (!a || !b || a==b) return false;
    const auto bytes=static_cast<uintptr_t>(frames)*sizeof(float);
    const auto left=reinterpret_cast<uintptr_t>(a),right=reinterpret_cast<uintptr_t>(b);
    if (left>std::numeric_limits<uintptr_t>::max()-bytes || right>std::numeric_limits<uintptr_t>::max()-bytes)
        return true;
    return left<right+bytes && right<left+bytes;
}
inline bool processing_data(const ProcessingConfiguration& c,const ProcessData& data) {
    if (data.processMode!=kRealtime || data.symbolicSampleSize!=kSample32 ||
        data.numSamples<1 || data.numSamples>c.maximum_frames ||
        !bus_buffers(data.inputs,data.numInputs,c.input_channels,c.input_enabled) ||
        !bus_buffers(data.outputs,data.numOutputs,c.output_channels,c.output_enabled)) return false;
    float* pointers[4]{}; int32 count=0;
    if (c.input_enabled && data.numInputs) for (int32 channel=0;channel<c.input_channels;++channel) {
        const auto* input=data.inputs[0].channelBuffers32[channel];
        for (int32 frame=0;frame<data.numSamples;++frame) {
            if (!std::isfinite(input[frame]) ||
                ((data.inputs[0].silenceFlags&(uint64_t(1)<<channel)) && input[frame]!=0)) return false;
        }
        pointers[count++]=data.inputs[0].channelBuffers32[channel];
    }
    if (c.output_enabled && data.numOutputs) for (int32 channel=0;channel<c.output_channels;++channel)
        pointers[count++]=data.outputs[0].channelBuffers32[channel];
    // Exact shared/in-place buffers are supported. Partial overlapping ranges
    // are outside this bounded host profile and refuse before foreign DSP.
    for (int32 left=0;left<count;++left) for (int32 right=left+1;right<count;++right)
        if (partial_overlap(pointers[left],pointers[right],data.numSamples)) return false;
    return true;
}
// Foreign code may set output silence flags, but cannot replace the host-owned
// block shape. Compare captured descriptors before following any returned pointer
// or trusting a changed frame count during output validation.
struct ProcessingShape {
    ProcessData original;
    AudioBusBuffers input{},output{};
    float* inputs[2]{};float* outputs[2]{};
    explicit ProcessingShape(const ProcessData& data):original(data) {
        if (data.numInputs) {
            input=data.inputs[0];
            std::copy_n(input.channelBuffers32,input.numChannels,inputs);
        }
        if (data.numOutputs) {
            output=data.outputs[0];
            std::copy_n(output.channelBuffers32,output.numChannels,outputs);
        }
    }
    bool unchanged(const ProcessData& data) const {
        if (data.numSamples!=original.numSamples || data.processMode!=original.processMode ||
            data.symbolicSampleSize!=original.symbolicSampleSize || data.numInputs!=original.numInputs ||
            data.numOutputs!=original.numOutputs || data.inputs!=original.inputs || data.outputs!=original.outputs ||
            data.inputParameterChanges!=original.inputParameterChanges || data.outputParameterChanges!=original.outputParameterChanges ||
            data.inputEvents!=original.inputEvents || data.outputEvents!=original.outputEvents)
            return false;
        const auto matches=[](const AudioBusBuffers& current,const AudioBusBuffers& before,float* const* samples) {
            if (current.numChannels!=before.numChannels || current.channelBuffers32!=before.channelBuffers32) return false;
            for (int32 n=0;n<before.numChannels;++n) if (current.channelBuffers32[n]!=samples[n]) return false;
            return true;
        };
        return (!data.numInputs || matches(data.inputs[0],input,inputs)) &&
            (!data.numOutputs || matches(data.outputs[0],output,outputs));
    }
};
inline void clear_disjoint_outputs(const ProcessingConfiguration& c,ProcessData& data) {
    if (!c.output_enabled || !data.numOutputs) return;
    for (int32 channel=0;channel<c.output_channels;++channel) {
        auto* output=data.outputs[0].channelBuffers32[channel];
        bool aliases_input=false;
        if (c.input_enabled && data.numInputs) for (int32 input=0;input<c.input_channels;++input)
            aliases_input=aliases_input || output==data.inputs[0].channelBuffers32[input];
        if (!aliases_input) std::fill_n(output,data.numSamples,0.0f);
    }
    data.outputs[0].silenceFlags=0;
}
inline bool finite_outputs(const ProcessingConfiguration& c,const ProcessData& data) {
    if (!bus_buffers(data.outputs,data.numOutputs,c.output_channels,c.output_enabled)) return false;
    if (!c.output_enabled || !data.numOutputs) return true;
    for (int32 channel=0;channel<c.output_channels;++channel)
        for (int32 frame=0;frame<data.numSamples;++frame) {
            const auto value=data.outputs[0].channelBuffers32[channel][frame];
            if (!std::isfinite(value) || ((data.outputs[0].silenceFlags&(uint64_t(1)<<channel)) && value!=0))
                return false;
        }
    return true;
}
} // namespace hex_instance
