// A borrowed read-only SDK event list on the isolated processing lane.
#pragma once
#include "pluginterfaces/vst/ivstevents.h"
#include "pluginterfaces/vst/ivstmidicontrollers.h"
#include "pluginterfaces/vst/ivstnoteexpression.h"
#include "pluginterfaces/vst/ivstprocesscontext.h"
#include <algorithm>
#include <array>
#include <atomic>
#include <cmath>
#include <cstring>
#include <limits>
#include <optional>
#include <thread>

namespace hex_instance {
using namespace Steinberg;
using namespace Steinberg::Vst;

inline constexpr size_t LEGACY_INPUT_NOTE_COUNT=64;
// Matches the copied identified-note limit in hex-plugin-host. Keep room for
// the corresponding release set on stopped worker renewal.
inline constexpr size_t INPUT_NOTE_COUNT=160;
enum class InputNoteKind : uint8 { Off, On, PolyPressure, Tuning, Brightness };
struct InputNote {
    int32 offset=0;
    int16 channel=0,pitch=0;
    uint8 value=0;
    InputNoteKind kind=InputNoteKind::Off;
    int32 note_id=-1;
    bool is_live=false;
    double normalized=0;
};

// Owned by the serialized processing lane. The host's authored and live
// voices fit within this fixed table. Legacy events match by channel/pitch.
class HeldInputNotes {
    std::array<InputNote,INPUT_NOTE_COUNT> held{};
    size_t count=0;
public:
    bool stage(std::array<InputNote,INPUT_NOTE_COUNT>& notes,size_t& length,
               uint16_t tuning,uint16_t brightness) {
        if(length>notes.size()) return false;
        size_t retained=0;
        for(size_t index=0;index<length;++index) {
            const auto note=notes[index];
            const auto end=held.begin()+count;
            if(note.kind==InputNoteKind::Tuning || note.kind==InputNoteKind::Brightness) {
                const auto voice=std::find_if(held.begin(),end,[&](const InputNote& active) {
                    return active.note_id==note.note_id && active.channel==note.channel;
                });
                if(voice==end) return false;
                const auto mask=note.kind==InputNoteKind::Tuning?tuning:brightness;
                if(mask & (uint16_t{1}<<note.channel)) notes[retained++]=note;
                continue;
            }
            notes[retained++]=note;
            if(note.kind==InputNoteKind::PolyPressure) continue;
            const auto match=std::find_if(held.begin(),end,[&](const InputNote& active) {
                return active.note_id==note.note_id && active.channel==note.channel && active.pitch==note.pitch;
            });
            if(note.kind==InputNoteKind::On) {
                if(note.note_id>=0 && match!=end) continue;
                if(count==held.size()) return false;
                held[count++]=note;
            } else if(match!=end) {
                std::move(match+1,end,match);
                --count;
            }
        }
        length=retained;
        return true;
    }
    size_t release_into(std::array<InputNote,INPUT_NOTE_COUNT>& notes) {
        for(size_t index=0;index<count;++index) {
            auto note=held[index];
            note.offset=0;note.value=0;note.kind=InputNoteKind::Off;
            notes[index]=note;
        }
        const auto released=count;
        count=0;
        return released;
    }
};

template<size_t Capacity>
class ProcessEvents : public IEventList {
protected:
    const std::thread::id worker=std::this_thread::get_id();
    std::array<Event,Capacity> events{};
    size_t count=0;
    bool active=false;
    std::atomic<uint32> references{1};
    std::atomic<bool> refused{false};

    bool accessible() {
        if(std::this_thread::get_id()==worker && active) return true;
        refused.store(true,std::memory_order_relaxed);
        return false;
    }
public:
    bool end() {
        active=false;
        return references.load()==1 && !refused.load();
    }
    tresult PLUGIN_API queryInterface(const TUID iid,void** out) override {
        if(!out) return kInvalidArgument;
        *out=nullptr;
        if(!accessible() || !iid) return kInvalidArgument;
        if(std::memcmp(iid,INLINE_UID_OF(IEventList),16) &&
           std::memcmp(iid,INLINE_UID_OF(FUnknown),16)) return kNoInterface;
        addRef();
        *out=static_cast<IEventList*>(this);
        return kResultOk;
    }
    uint32 PLUGIN_API addRef() override {
        if(!accessible()) return references.load();
        const auto count=references.load();
        if(count==std::numeric_limits<uint32>::max()) {
            refused.store(true);
            return count;
        }
        return ++references;
    }
    uint32 PLUGIN_API release() override {
        if(!accessible()) return references.load();
        if(references.load()==1) {
            refused.store(true);
            return 1;
        }
        return --references;
    }
    int32 PLUGIN_API getEventCount() override {
        return accessible()?static_cast<int32>(count):0;
    }
    tresult PLUGIN_API getEvent(int32 index,Event& event) override {
        if(!accessible() || index<0 || static_cast<size_t>(index)>=count) return kInvalidArgument;
        event=events[static_cast<size_t>(index)];
        return kResultOk;
    }
    tresult PLUGIN_API addEvent(Event&) override { return kNotImplemented; }
};

class InputNotes final : public ProcessEvents<INPUT_NOTE_COUNT> {
public:
    // Populate Event::ppqPosition from the active slice's ProcessContext:
    // - When kProjectTimeMusicValid is set and playing with a valid positive
    //   tempo and sample rate, advance by sampleOffset from projectTimeMusic.
    // - When kProjectTimeMusicValid is set while stopped/paused or without a
    //   valid tempo/sample rate, report projectTimeMusic at the slice origin.
    // - When no project musical clock is available (null context or
    //   kProjectTimeMusicValid clear), leave ppqPosition at 0.0.
    bool begin(const std::array<InputNote,INPUT_NOTE_COUNT>& notes,size_t length,
               size_t first=0,int32 start_frame=0,const ProcessContext* context=nullptr) {
        if(active || refused.load() || references.load()!=1 || first>notes.size() ||
            length>notes.size()-first) return false;
        const bool has_music=context &&
            (context->state&ProcessContext::kProjectTimeMusicValid) &&
            std::isfinite(context->projectTimeMusic);
        const bool advance_music=has_music &&
            (context->state&ProcessContext::kPlaying) &&
            (context->state&ProcessContext::kTempoValid) &&
            std::isfinite(context->tempo) && context->tempo>0.0 &&
            std::isfinite(context->sampleRate) && context->sampleRate>0.0;
        const double sample_denom=advance_music?60.0*context->sampleRate:0.0;
        count=length;
        for(size_t index=0;index<count;++index) {
            const auto& note=notes[first+index];
            auto& event=events[index];
            event={};event.sampleOffset=note.offset-start_frame;
            if(note.is_live) event.flags|=Event::kIsLive;
            if(has_music) {
                event.ppqPosition=advance_music
                    ?context->projectTimeMusic+
                        (static_cast<double>(event.sampleOffset)*context->tempo)/sample_denom
                    :context->projectTimeMusic;
            }
            // A byte-only MIDI message retains the SDK's absent note ID.
            const auto value=note.value/127.0f;
            if(note.kind==InputNoteKind::On) {
                event.type=Event::kNoteOnEvent;
                event.noteOn={note.channel,note.pitch,0,value,0,note.note_id};
            } else if(note.kind==InputNoteKind::Tuning || note.kind==InputNoteKind::Brightness) {
                event.type=Event::kNoteExpressionValueEvent;
                event.noteExpressionValue.typeId=note.kind==InputNoteKind::Tuning?kTuningTypeID:kBrightnessTypeID;
                event.noteExpressionValue.noteId=note.note_id;
                event.noteExpressionValue.value=note.normalized;
            } else if(note.kind==InputNoteKind::PolyPressure) {
                event.type=Event::kPolyPressureEvent;
                event.polyPressure={note.channel,note.pitch,value,note.note_id};
            } else {
                event.type=Event::kNoteOffEvent;
                event.noteOff={note.channel,note.pitch,value,note.note_id,0};
            }
        }
        active=true;
        return true;
    }
};

struct ChannelVoiceMessage {
    uint8 length;
    std::array<uint8,3> bytes;
};

// Legacy output has no note identity. Only pitch bend and poly pressure use
// the second value. System messages have no channel-voice wire representation.
inline std::optional<ChannelVoiceMessage> channel_voice_output(const LegacyMIDICCOutEvent& cc) {
    if(cc.channel<0 || cc.channel>15 || cc.value<0) return std::nullopt;
    const auto value=static_cast<uint8>(cc.value);
    const auto status=[&](uint8 kind) { return static_cast<uint8>(kind|cc.channel); };
    if(cc.controlNumber<128)
        return ChannelVoiceMessage{3,{status(0xb0),cc.controlNumber,value}};
    switch(cc.controlNumber) {
    case kAfterTouch: return ChannelVoiceMessage{2,{status(0xd0),value,0}};
    case kCtrlProgramChange: return ChannelVoiceMessage{2,{status(0xc0),value,0}};
    case kPitchBend:
    case kCtrlPolyPressure:
        if(cc.value2>=0) return ChannelVoiceMessage{3,{status(cc.controlNumber==kPitchBend?0xe0:0xa0),
                                                       value,static_cast<uint8>(cc.value2)}};
        break;
    }
    return std::nullopt;
}

// An SDK slice borrows its bus-zero output list on the processing lane.
// Keep validation failures separate from lifetime violations: a refused
// event invalidates the whole reply, including any earlier slice's releases.
inline constexpr size_t OUTPUT_NOTE_COUNT=64;
class OutputNotes final : public ProcessEvents<OUTPUT_NOTE_COUNT> {
    int32 frames=0,channels=0;
    size_t capacity=0;
    bool invalid=false;
public:
    bool begin(int32 slice_frames,int32 output_channels,size_t remaining) {
        if(active || refused.load() || references.load()!=1 || remaining>events.size()) return false;
        active=true;count=0;invalid=false;
        frames=slice_frames;channels=output_channels;capacity=remaining;
        return true;
    }
    bool valid() const { return !invalid; }
    size_t size() const { return count; }
    const Event& at(size_t index) const { return events[index]; }

    tresult PLUGIN_API addEvent(Event& event) override {
        if(!accessible()) return kInvalidArgument;
        if(invalid || count>=capacity || event.busIndex!=0 ||
            event.sampleOffset<0 || event.sampleOffset>=frames ||
            (count && events[count-1].sampleOffset>event.sampleOffset)) {
            invalid=true;return kInvalidArgument;
        }
        if(event.type==Event::kLegacyMIDICCOutEvent) {
            if(event.midiCCOut.channel>=channels || !channel_voice_output(event.midiCCOut)) {
                invalid=true;return kInvalidArgument;
            }
            events[count++]=event;
            return kResultOk;
        }
        int16 channel=-1,pitch=-1;
        int32 id=-1;
        float value=0;
        switch(event.type) {
        case Event::kNoteOnEvent:
            channel=event.noteOn.channel;pitch=event.noteOn.pitch;id=event.noteOn.noteId;value=event.noteOn.velocity;
            break;
        case Event::kNoteOffEvent:
            channel=event.noteOff.channel;pitch=event.noteOff.pitch;id=event.noteOff.noteId;value=event.noteOff.velocity;
            break;
        case Event::kPolyPressureEvent:
            channel=event.polyPressure.channel;pitch=event.polyPressure.pitch;id=event.polyPressure.noteId;value=event.polyPressure.pressure;
            break;
        default:
            invalid=true;return kNotImplemented;
        }
        if(channel<0 || channel>=channels || pitch<0 || pitch>127 ||
            !std::isfinite(value) || value<0 || value>1 ||
            (id< -1 && (id< -10000 || id> -1000))) {
            invalid=true;return kInvalidArgument;
        }
        events[count++]=event;
        return kResultOk;
    }
};
} // namespace hex_instance
