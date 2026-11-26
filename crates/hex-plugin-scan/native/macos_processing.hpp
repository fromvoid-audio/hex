// Original bounded native processing contract. SDK pointers/storage stay in
// the isolated helper. This does not admit installed devices or graph routes.
#pragma once
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstparameterchanges.h"
#include <algorithm>
#include <array>
#include <cmath>
#include <cstdint>
#include <limits>
#include <memory>
#include <new>
#include <type_traits>

namespace hex_instance {
using namespace Steinberg;
using namespace Steinberg::Vst;

struct ProcessingConfiguration {
    double sample_rate=48000;
    int32 maximum_frames=64;
    int32 input_channels=2, output_channels=2;
    int32 precision=kSample32;
    bool input_enabled=true, output_enabled=true;
    int32 process_mode=kRealtime;
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
        (c.input_channels!=0 || !c.input_enabled) &&
        (c.process_mode==kRealtime || c.process_mode==kOffline);
}
inline SpeakerArrangement arrangement(int32 channels) {
    return channels==1?SpeakerArr::kMono:SpeakerArr::kStereo;
}
inline bool audio_bus_info(IComponent* component,BusDirection direction,int32 index,BusInfo& info) {
    return component->getBusInfo(kAudio,direction,index,info)==kResultOk && info.mediaType==kAudio &&
        info.direction==direction && info.channelCount>0 && info.channelCount<=64 &&
        !(info.flags&BusInfo::kIsControlVoltage);
}
inline bool audio_bus_info(IComponent* component,BusDirection direction,int32 index,BusType type,BusInfo& info) {
    return audio_bus_info(component,direction,index,info) && info.busType==type;
}
inline constexpr int32 max_audio_inputs=16;
using AudioInputLayout=std::array<SpeakerArrangement,max_audio_inputs>;
inline constexpr int32 max_instrument_outputs=16;
using InstrumentOutputLayout=std::array<SpeakerArrangement,max_instrument_outputs>;

// Effects accept bounded inactive trailing auxiliary inputs. A single silent
// instrument input can itself be auxiliary. Neither becomes a graph route.
inline bool audio_buses(IComponent* component,BusDirection direction,int32 count,int32 channels,
                        bool exact,BusType first_type=kMain) {
    const bool trailing_aux=direction==kInput && channels && count>1 && count<=max_audio_inputs;
    if (count!=(channels?1:0) && !trailing_aux) return false;
    if (!count) return true;
    BusInfo first{};
    if (!audio_bus_info(component,direction,0,first_type,first) || (exact && first.channelCount!=channels))
        return false;
    for (int32 i=1;i<count;++i) {
        BusInfo aux{};
        if (!audio_bus_info(component,direction,i,kAux,aux)) return false;
    }
    return true;
}

// Access only the SDK union member selected for this process call.
template<class Sample> inline Sample** channel_buffers(const AudioBusBuffers& bus) {
    if constexpr (std::is_same_v<Sample,Sample64>) return bus.channelBuffers64;
    else return bus.channelBuffers32;
}
inline void set_channel_buffers(AudioBusBuffers& bus,Sample32** channels) { bus.channelBuffers32=channels; }
inline void set_channel_buffers(AudioBusBuffers& bus,Sample64** channels) { bus.channelBuffers64=channels; }

// Bounded auxiliary sidechain input bus (input index 1) for effects. Preallocated
// on control during configure; zero processing-thread allocation.
template<class Sample> class AuxInputBus {
    int32 channels_=0;
    int32 stride_=0;
    std::unique_ptr<Sample[]> storage_;
    std::array<Sample*,2> pointers_{};
    AudioBusBuffers aux_bus_{};
    std::array<AudioBusBuffers,2> buses_{};
public:
    bool prepare(int32 channels,int32 maximum_frames) {
        channels_=0;stride_=0;storage_.reset();pointers_={};aux_bus_={};
        if (!channels) return true;
        if (channels<1 || channels>2 || maximum_frames<1) return false;
        storage_.reset(new (std::nothrow) Sample[static_cast<size_t>(channels)*static_cast<size_t>(maximum_frames)]);
        if (!storage_) return false;
        channels_=channels;stride_=maximum_frames;
        for (int32 c=0;c<channels_;++c) pointers_[c]=storage_.get()+c*stride_;
        aux_bus_.numChannels=channels_;
        set_channel_buffers(aux_bus_,pointers_.data());
        return true;
    }
    bool active() const { return channels_>0; }
    int32 channels() const { return channels_; }
    void attach(ProcessData& sdk,const float* aux_left,const float* aux_right) {
        if (!channels_ || !sdk.numInputs || !sdk.inputs) return;
        buses_[0]=sdk.inputs[0];
        if (aux_left && aux_right) {
            uint64_t silence=0;
            for (int32 c=0;c<channels_;++c) {
                const float* src=c==0?aux_left:aux_right;
                bool all_zero=true;
                for (int32 n=0;n<sdk.numSamples;++n) {
                    const Sample value=static_cast<Sample>(src[n]);
                    pointers_[c][n]=value;
                    if (value!=Sample(0)) all_zero=false;
                }
                if (all_zero) silence|=(uint64_t{1}<<c);
            }
            aux_bus_.silenceFlags=silence;
        } else {
            for (int32 c=0;c<channels_;++c) std::fill_n(pointers_[c],sdk.numSamples,Sample(0));
            aux_bus_.silenceFlags=(uint64_t{1}<<channels_)-1;
        }
        buses_[1]=aux_bus_;
        sdk.numInputs=2;
        sdk.inputs=buses_.data();
    }
    bool unchanged(const ProcessData& sdk) const {
        if (!channels_ || sdk.numInputs<2 || !sdk.inputs) return true;
        if (sdk.inputs[1].numChannels!=channels_ ||
            channel_buffers<Sample>(sdk.inputs[1])!=pointers_.data()) return false;
        for (int32 c=0;c<channels_;++c) {
            if (pointers_[c]!=storage_.get()+c*stride_) return false;
        }
        return true;
    }
};

// Preserve SDK slots for multi-out instrument outputs. Activated auxiliary output
// buses receive preallocated non-null sample buffers and mix cleanly into main
// stereo output; inactive trailing buses keep stable null sample pointers.
// Borrowed disjoint storage on the serialized SDK lane. Bus zero remains in
// ProcessData. Other bus rows retain their SDK index, with stereo frames at
// (bus - 1) * numSamples. An inactive row or unused mono channel is zero.
struct AuxiliaryOutputBlock {
    std::array<Sample32,2>* frames=nullptr;
    size_t capacity=0;

    bool fits(int32 bus_count,int32 num_samples) const {
        const auto required=static_cast<size_t>(bus_count-1)*static_cast<size_t>(num_samples);
        return required<=capacity && (!required || frames);
    }
};

template<class Sample> class InstrumentOutputs {
    int32 count=1;
    int32 stride_=0;
    std::unique_ptr<Sample[]> storage_;
    std::array<AudioBusBuffers,max_instrument_outputs> buses{};
    std::array<std::array<Sample*,64>,max_instrument_outputs-1> pointers{};
    std::array<std::array<Sample*,64>,max_instrument_outputs-1> expected_pointers{};
    std::array<int32,max_instrument_outputs-1> channels{};
    std::array<bool,max_instrument_outputs-1> active_{};
public:
    bool prepare(int32 output_count,const InstrumentOutputLayout& layout,uint32_t active_mask,int32 maximum_frames) {
        count=output_count;stride_=maximum_frames;storage_.reset();
        pointers={};expected_pointers={};channels={};active_={};
        size_t active_channels=0;
        for (int32 i=1;i<count;++i) {
            channels[i-1]=SpeakerArr::getChannelCount(layout[i]);
            if ((active_mask&(uint32_t{1}<<i)) && channels[i-1]>=1 && channels[i-1]<=2) {
                active_[i-1]=true;
                active_channels+=static_cast<size_t>(channels[i-1]);
            }
        }
        if (active_channels) {
            storage_.reset(new (std::nothrow) Sample[active_channels*static_cast<size_t>(maximum_frames)]);
            if (!storage_) return false;
        }
        size_t slot=0;
        for (int32 i=1;i<count;++i) {
            if (active_[i-1]) {
                for (int32 c=0;c<channels[i-1];++c) {
                    auto* buf=storage_.get()+(slot++)*static_cast<size_t>(stride_);
                    pointers[i-1][c]=buf;
                    expected_pointers[i-1][c]=buf;
                }
            }
            buses[i].numChannels=channels[i-1];
            set_channel_buffers(buses[i],pointers[i-1].data());
        }
        return true;
    }
    void attach(ProcessData& data) {
        if (count==1) return;
        buses[0]=data.outputs[0];
        for (int32 i=1;i<count;++i) {
            buses[i].silenceFlags=0;
            if (active_[i-1]) {
                for (int32 c=0;c<channels[i-1];++c)
                    std::fill_n(pointers[i-1][c],data.numSamples,Sample(0));
            }
        }
        data.numOutputs=count;
        data.outputs=buses.data();
    }
    bool unchanged() const {
        for (int32 i=1;i<count;++i) {
            if (buses[i].numChannels!=channels[i-1] ||
                channel_buffers<Sample>(buses[i])!=pointers[i-1].data()) return false;
            for (int32 c=0;c<channels[i-1];++c) {
                if (pointers[i-1][c]!=expected_pointers[i-1][c]) return false;
            }
        }
        return true;
    }
    void copy_unmixed(AuxiliaryOutputBlock target,int32 num_samples) const {
        for (int32 bus=1;bus<count;++bus) {
            auto* row=target.frames+static_cast<size_t>(bus-1)*num_samples;
            for (int32 n=0;n<num_samples;++n) {
                row[n]={};
                if (!active_[bus-1]) continue;
                for (int32 c=0;c<channels[bus-1];++c) {
                    const auto value=pointers[bus-1][c][n];
                    if (std::abs(value)<=std::numeric_limits<Sample32>::max())
                        row[n][c]=static_cast<Sample32>(value);
                }
            }
        }
    }
    // Mix active auxiliary output buses into bus zero so multi-out instruments
    // deliver all routed voices with non-finite sanitization.
    void mix_into_main(AudioBusBuffers& main_bus,int32 num_samples) const {
        if (count==1 || !main_bus.numChannels) return;
        auto** main_ch=channel_buffers<Sample>(main_bus);
        if (!main_ch) return;
        for (int32 i=1;i<count;++i) {
            if (!active_[i-1]) continue;
            const int32 aux_ch=channels[i-1];
            for (int32 n=0;n<num_samples;++n) {
                const Sample l=std::isfinite(pointers[i-1][0][n])?pointers[i-1][0][n]:Sample(0);
                const Sample right=aux_ch==2?pointers[i-1][1][n]:l;
                const Sample r=std::isfinite(right)?right:Sample(0);
                if (main_bus.numChannels==1 && main_ch[0]) {
                    const Sample sum=aux_ch>=2?Sample(0.5)*(l+r):l;
                    if (sum!=Sample(0)) {
                        main_ch[0][n]+=sum;
                        main_bus.silenceFlags&=~uint64_t{1};
                    }
                } else {
                    if (main_ch[0] && l!=Sample(0)) {
                        main_ch[0][n]+=l;
                        main_bus.silenceFlags&=~uint64_t{1};
                    }
                    if (main_bus.numChannels>=2 && main_ch[1] && r!=Sample(0)) {
                        main_ch[1][n]+=r;
                        main_bus.silenceFlags&=~uint64_t{2};
                    }
                }
            }
        }
    }
};
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
    if (data.processMode!=c.process_mode || data.symbolicSampleSize!=kSample32 ||
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
template<class Sample> struct ProcessingShape {
    ProcessData original;
    AudioBusBuffers input{},output{};
    Sample* inputs[2]{};Sample* outputs[2]{};
    explicit ProcessingShape(const ProcessData& data):original(data) {
        if (data.numInputs) {
            input=data.inputs[0];
            std::copy_n(channel_buffers<Sample>(input),input.numChannels,inputs);
        }
        if (data.numOutputs) {
            output=data.outputs[0];
            std::copy_n(channel_buffers<Sample>(output),output.numChannels,outputs);
        }
    }
    bool unchanged(const ProcessData& data) const {
        if (data.numSamples!=original.numSamples || data.processMode!=original.processMode ||
            data.symbolicSampleSize!=original.symbolicSampleSize || data.numInputs!=original.numInputs ||
            data.numOutputs!=original.numOutputs || data.inputs!=original.inputs || data.outputs!=original.outputs ||
            data.inputParameterChanges!=original.inputParameterChanges || data.outputParameterChanges!=original.outputParameterChanges ||
            data.inputEvents!=original.inputEvents || data.outputEvents!=original.outputEvents ||
            data.processContext!=original.processContext)
            return false;
        const auto matches=[](const AudioBusBuffers& current,const AudioBusBuffers& before,Sample* const* samples) {
            if (current.numChannels!=before.numChannels || channel_buffers<Sample>(current)!=channel_buffers<Sample>(before)) return false;
            for (int32 n=0;n<before.numChannels;++n) if (channel_buffers<Sample>(current)[n]!=samples[n]) return false;
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
// The bridge always supplies validated float buffers. Reserve up to four
// distinct channels on control. Exact aliases map to the same SDK buffer,
// including crossed in-place stereo. Disabled channels stay null.
class DoubleProcessing {
    std::unique_ptr<Sample64[]> storage_;
    int32 stride_=0;
    AudioBusBuffers input_{},output_{};
    std::array<Sample64*,2> inputs_{},outputs_{};
public:
    AuxInputBus<Sample64> aux_input;
    InstrumentOutputs<Sample64> instrument_outputs;
    bool reserve(int32 maximum_frames) {
        stride_=maximum_frames;
        storage_.reset(new (std::nothrow) Sample64[4*static_cast<size_t>(maximum_frames)]);
        return storage_!=nullptr;
    }
    void attach(const ProcessingConfiguration& config,ProcessData& sdk) {
        std::array<const Sample32*,4> originals{};
        size_t used=0;
        const auto convert=[&](const Sample32* source,bool input) -> Sample64* {
            if (!source) return nullptr;
            for (size_t i=0;i<used;++i)
                if (originals[i]==source) return storage_.get()+i*stride_;
            auto* samples=storage_.get()+used*stride_;
            originals[used++]=source;
            if (input) std::copy_n(source,sdk.numSamples,samples);
            else std::fill_n(samples,sdk.numSamples,0.0);
            return samples;
        };
        sdk.symbolicSampleSize=kSample64;
        if (sdk.numInputs) {
            input_=sdk.inputs[0];
            for (int32 c=0;c<input_.numChannels;++c)
                inputs_[c]=config.input_enabled?convert(sdk.inputs[0].channelBuffers32[c],true):nullptr;
            input_.channelBuffers64=inputs_.data();
            sdk.inputs=&input_;
        }
        if (sdk.numOutputs) {
            output_=sdk.outputs[0];
            for (int32 c=0;c<output_.numChannels;++c)
                outputs_[c]=config.output_enabled?convert(sdk.outputs[0].channelBuffers32[c],false):nullptr;
            output_.channelBuffers64=outputs_.data();
            sdk.outputs=&output_;
        }
    }
    void copy_outputs(ProcessData& data) const {
        if (!data.numOutputs) return;
        for (int32 c=0;c<data.outputs[0].numChannels;++c) {
            auto* output=data.outputs[0].channelBuffers32[c];
            if (!output) continue;
            for (int32 n=0;n<data.numSamples;++n) {
                const auto value=outputs_[c][n];
                output[n]=std::abs(value)<=std::numeric_limits<Sample32>::max()
                    ? static_cast<Sample32>(value) : 0.0f;
            }
        }
    }
};

inline bool finite_outputs(const ProcessingConfiguration& c,ProcessData& data) {
    if (c.output_enabled && data.numOutputs && data.outputs) {
        data.outputs[0].silenceFlags&=(uint64_t(1)<<c.output_channels)-1;
    }
    if (!bus_buffers(data.outputs,data.numOutputs,c.output_channels,c.output_enabled)) return false;
    if (!c.output_enabled || !data.numOutputs) return true;
    for (int32 channel=0;channel<c.output_channels;++channel) {
        auto* samples=data.outputs[0].channelBuffers32[channel];
        const uint64_t silence_bit=uint64_t(1)<<channel;
        bool has_nonzero=false;
        for (int32 frame=0;frame<data.numSamples;++frame) {
            const auto value=samples[frame];
            if (!std::isfinite(value)) {
                samples[frame]=0.0f;
            } else if (value!=0.0f) {
                has_nonzero=true;
            }
        }
        if (has_nonzero && (data.outputs[0].silenceFlags&silence_bit)) {
            data.outputs[0].silenceFlags&=~silence_bit;
        }
    }
    return true;
}
} // namespace hex_instance
