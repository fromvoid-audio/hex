// A borrowed read-only SDK event list on the isolated processing lane.
#pragma once
#include "pluginterfaces/vst/ivstevents.h"
#include "pluginterfaces/vst/ivstprocesscontext.h"
#include <algorithm>
#include <array>
#include <atomic>
#include <cmath>
#include <cstring>
#include <limits>
#include <thread>

namespace hex_instance {
using namespace Steinberg;
using namespace Steinberg::Vst;

inline constexpr size_t LEGACY_INPUT_NOTE_COUNT=64;
// Matches the copied identified-note limit in hex-plugin-host. Keep room for
// the corresponding release set on stopped worker renewal.
inline constexpr size_t INPUT_NOTE_COUNT=160;
enum class InputNoteKind : uint8 { Off, On, PolyPressure };
struct InputNote {
    int32 offset=0;
    int16 channel=0,pitch=0;
    uint8 value=0;
    InputNoteKind kind=InputNoteKind::Off;
    int32 note_id=-1;
    bool is_live=false;
};

// Owned by the serialized processing lane. The host's authored and live
// voices fit within this fixed table. Legacy events match by channel/pitch.
class HeldInputNotes {
    std::array<InputNote,INPUT_NOTE_COUNT> held{};
    size_t count=0;
public:
    bool stage(const std::array<InputNote,INPUT_NOTE_COUNT>& notes,size_t length) {
        if(length>notes.size()) return false;
        for(size_t index=0;index<length;++index) {
            const auto note=notes[index];
            if(note.kind==InputNoteKind::PolyPressure) continue;
            const auto end=held.begin()+count;
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

class InputNotes final : public IEventList {
    const std::thread::id worker=std::this_thread::get_id();
    std::array<Event,INPUT_NOTE_COUNT> events{};
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
} // namespace hex_instance
