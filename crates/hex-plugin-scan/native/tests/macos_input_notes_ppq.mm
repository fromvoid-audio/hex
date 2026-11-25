// Direct SDK-boundary verification of Event::ppqPosition in InputNotes
// using the real wire timing decoder (Packet::parse_timing).
#include "macos_instance_wire.hpp"
#include <cassert>
#include <cmath>
#include <cstring>

namespace {
void write_le(uint8_t* dst, uint64_t value, size_t width) {
    for (size_t i = 0; i < width; ++i) dst[i] = static_cast<uint8_t>((value >> (i * 8)) & 0xffu);
}

Steinberg::Vst::ProcessContext decode_context(double sample_rate, uint64_t frame, uint64_t beat_q32,
                                              uint32_t tempo_micro_bpm, uint8_t flags) {
    uint8_t raw[40]{};
    write_le(raw + 0, frame, 8);
    write_le(raw + 8, beat_q32, 8);
    write_le(raw + 16, tempo_micro_bpm, 4);
    raw[20] = flags;
    hex_instance_wire::Packet packet;
    Steinberg::Vst::ProcessContext ctx{};
    assert(packet.parse_timing(raw, sample_rate, true, false, ctx));
    return ctx;
}

bool near(double a, double b, double tol = 1e-12) {
    return std::fabs(a - b) <= tol;
}
}  // namespace

int main() {
    using namespace hex_instance;
    using namespace Steinberg::Vst;

    // 1. Moving a MIDI clip from beat 8 to beat 16 at 48 kHz / 120 BPM updates
    //    sample-accurate ppqPosition (8/8.0005 -> 16/16.0005) without altering
    //    pitch, noteId, duration, or sample offsets.
    {
        std::array<InputNote, INPUT_NOTE_COUNT> notes{};
        notes[0] = {0, 2, 60, 100, InputNoteKind::On, 41};
        notes[1] = {12, 2, 60, 0, InputNoteKind::Off, 41};

        const auto beat8 = decode_context(48000.0, 192000, 8ULL << 32, 120000000, 3);
        const auto beat16 = decode_context(48000.0, 384000, 16ULL << 32, 120000000, 3);

        InputNotes list;
        Event on8{}, off8{}, on16{}, off16{};

        assert(list.begin(notes, 2, 0, 0, &beat8));
        assert(list.getEventCount() == 2);
        assert(list.getEvent(0, on8) == Steinberg::kResultOk);
        assert(list.getEvent(1, off8) == Steinberg::kResultOk);
        assert(list.end());

        assert(list.begin(notes, 2, 0, 0, &beat16));
        assert(list.getEventCount() == 2);
        assert(list.getEvent(0, on16) == Steinberg::kResultOk);
        assert(list.getEvent(1, off16) == Steinberg::kResultOk);
        assert(list.end());

        assert(on8.sampleOffset == 0 && off8.sampleOffset == 12);
        assert(on16.sampleOffset == 0 && off16.sampleOffset == 12);
        assert(on8.noteOn.pitch == 60 && on16.noteOn.pitch == 60);
        assert(on8.noteOn.noteId == 41 && on16.noteOn.noteId == 41);
        assert(off8.noteOff.noteId == 41 && off16.noteOff.noteId == 41);
        assert(near(on8.ppqPosition, 8.0));
        assert(near(off8.ppqPosition, 8.0005));
        assert(near(on16.ppqPosition, 16.0));
        assert(near(off16.ppqPosition, 16.0005));
        assert(near(off8.ppqPosition - on8.ppqPosition, off16.ppqPosition - on16.ppqPosition));
    }

    // 2. Nonzero sample offsets and tempo changes (150 BPM @ 48 kHz and 137.5 BPM @ 44.1 kHz).
    {
        std::array<InputNote, INPUT_NOTE_COUNT> notes{};
        const int32 offsets[4] = {0, 5, 37, 63};
        for (size_t i = 0; i < 4; ++i) {
            notes[i] = {offsets[i], 0, static_cast<int16>(60 + i), 110, InputNoteKind::On, static_cast<int32>(10 + i)};
        }
        const auto ctx150 = decode_context(48000.0, 240000, 10ULL << 32, 150000000, 3);
        InputNotes list;
        assert(list.begin(notes, 4, 0, 0, &ctx150));
        for (size_t i = 0; i < 4; ++i) {
            Event ev{};
            assert(list.getEvent(static_cast<int32>(i), ev) == Steinberg::kResultOk);
            const double expected = 10.0 + (static_cast<double>(offsets[i]) * 150.0) / (60.0 * 48000.0);
            assert(ev.sampleOffset == offsets[i]);
            assert(near(ev.ppqPosition, expected));
        }
        assert(list.end());

        const auto ctx137 = decode_context(44100.0, 88200, (5ULL << 32) | (1ULL << 31), 137500000, 3);
        assert(list.begin(notes, 4, 0, 0, &ctx137));
        for (size_t i = 0; i < 4; ++i) {
            Event ev{};
            assert(list.getEvent(static_cast<int32>(i), ev) == Steinberg::kResultOk);
            const double expected = 5.5 + (static_cast<double>(offsets[i]) * 137.5) / (60.0 * 44100.0);
            assert(near(ev.ppqPosition, expected));
        }
        assert(list.end());
    }

    // 3. Across a loop split: slice 0 [0..4) starts near loop end; slice 1 [4..16)
    //    wraps to beat 4.0 and adjusts sampleOffset and ppqPosition relative to slice start.
    {
        std::array<InputNote, INPUT_NOTE_COUNT> notes{};
        notes[0] = {2, 2, 60, 127, InputNoteKind::On, 51};   // slice 0, local offset 2
        notes[1] = {4, 2, 62, 127, InputNoteKind::On, 52};   // slice 1, local offset 0
        notes[2] = {10, 2, 62, 0, InputNoteKind::Off, 52};   // slice 1, local offset 6

        const uint64_t pre_beat_raw = (7ULL << 32) + ((3ULL << 32) / 4ULL);  // 7.75
        const auto pre = decode_context(48000.0, 186000, pre_beat_raw, 120000000, 3);
        const auto post = decode_context(48000.0, 96000, 4ULL << 32, 120000000, 3);

        InputNotes list;
        Event e0{}, e1{}, e2{};
        assert(list.begin(notes, 1, 0, 0, &pre));
        assert(list.getEvent(0, e0) == Steinberg::kResultOk);
        assert(list.end());
        assert(e0.sampleOffset == 2);
        assert(near(e0.ppqPosition, 7.75 + (2.0 * 120.0) / (60.0 * 48000.0)));

        assert(list.begin(notes, 2, 1, 4, &post));
        assert(list.getEvent(0, e1) == Steinberg::kResultOk);
        assert(list.getEvent(1, e2) == Steinberg::kResultOk);
        assert(list.end());
        assert(e1.sampleOffset == 0 && near(e1.ppqPosition, 4.0));
        assert(e2.sampleOffset == 6 && near(e2.ppqPosition, 4.0 + (6.0 * 120.0) / (60.0 * 48000.0)));
    }

    // 4. Unavailable musical clock (null context or kProjectTimeMusicValid clear) leaves
    //    ppqPosition at 0.0; paused with kProjectTimeMusicValid holds projectTimeMusic.
    {
        std::array<InputNote, INPUT_NOTE_COUNT> notes{};
        notes[0] = {5, 0, 60, 100, InputNoteKind::On, 70};

        InputNotes list;
        Event ev{};
        assert(list.begin(notes, 1, 0, 0, nullptr));
        assert(list.getEvent(0, ev) == Steinberg::kResultOk);
        assert(list.end());
        assert(ev.ppqPosition == 0.0);

        const auto no_music = decode_context(48000.0, 1000, 0, 120000000, 1);  // playing, no beat flag
        assert(list.begin(notes, 1, 0, 0, &no_music));
        assert(list.getEvent(0, ev) == Steinberg::kResultOk);
        assert(list.end());
        assert(ev.ppqPosition == 0.0);

        const auto paused = decode_context(48000.0, 192000, 8ULL << 32, 120000000, 2);  // paused, beat valid
        assert(list.begin(notes, 1, 0, 0, &paused));
        assert(list.getEvent(0, ev) == Steinberg::kResultOk);
        assert(list.end());
        assert(near(ev.ppqPosition, 8.0));
    }

    return 0;
}
