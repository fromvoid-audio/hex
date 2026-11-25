// Direct SDK-boundary verification of Event::kIsLive in InputNotes and HeldInputNotes.
#include "macos_instance_wire.hpp"
#include <cassert>
#include <unistd.h>

namespace {
void write_le(uint8_t* dst, uint64_t value, size_t width) {
    for (size_t i = 0; i < width; ++i) dst[i] = static_cast<uint8_t>((value >> (i * 8)) & 0xffu);
}
}  // namespace

int main() {
    using namespace hex_instance;
    using namespace Steinberg::Vst;

    // 1. Live press and matching release carry Event::kIsLive; sequenced clip notes
    //    (including overlapping identified notes sharing a pitch) leave kIsLive clear.
    {
        std::array<InputNote, INPUT_NOTE_COUNT> notes{};
        notes[0] = {0, 0, 60, 100, InputNoteKind::On, 11, true};    // live hardware/keyboard press
        notes[1] = {1, 0, 60, 90, InputNoteKind::On, 21, false};    // sequenced clip note 1
        notes[2] = {2, 0, 60, 80, InputNoteKind::On, 22, false};    // overlapping sequenced note 2
        notes[3] = {4, 0, 60, 0, InputNoteKind::Off, 11, true};     // matching live release
        notes[4] = {5, 0, 60, 0, InputNoteKind::Off, 21, false};    // matching sequenced release 1
        notes[5] = {6, 0, 60, 0, InputNoteKind::Off, 22, false};    // matching sequenced release 2

        InputNotes list;
        assert(list.begin(notes, 6, 0, 0, nullptr));
        assert(list.getEventCount() == 6);
        for (size_t i = 0; i < 6; ++i) {
            Event ev{};
            assert(list.getEvent(static_cast<int32>(i), ev) == Steinberg::kResultOk);
            assert(ev.sampleOffset == notes[i].offset);
            const uint16 expected_flags = notes[i].is_live ? Event::kIsLive : 0;
            assert(ev.flags == expected_flags);
            if (notes[i].kind == InputNoteKind::On) {
                assert(ev.type == Event::kNoteOnEvent);
                assert(ev.noteOn.noteId == notes[i].note_id);
                assert(ev.noteOn.pitch == notes[i].pitch);
            } else {
                assert(ev.type == Event::kNoteOffEvent);
                assert(ev.noteOff.noteId == notes[i].note_id);
                assert(ev.noteOff.pitch == notes[i].pitch);
            }
        }
        assert(list.end());
    }

    // 2. HeldInputNotes::release_into preserves is_live per held note.
    {
        std::array<InputNote, INPUT_NOTE_COUNT> staged{};
        staged[0] = {0, 0, 64, 100, InputNoteKind::On, 31, true};
        staged[1] = {1, 0, 67, 100, InputNoteKind::On, 32, false};
        HeldInputNotes held;
        assert(held.stage(staged, 2));
        std::array<InputNote, INPUT_NOTE_COUNT> released{};
        assert(held.release_into(released) == 2);
        assert(released[0].kind == InputNoteKind::Off && released[0].note_id == 31 && released[0].is_live);
        assert(released[1].kind == InputNoteKind::Off && released[1].note_id == 32 && !released[1].is_live);
    }

    // 3. Wire decoder decodes byte 6 (0 -> sequenced, 1 -> live) and rejects > 1.
    {
        hex_instance_wire::Bytes cfg_bytes;
        hex_instance_wire::number(cfg_bytes, 1, 2);
        for (uint64_t id : {1ULL, 2ULL, 3ULL, 4ULL}) hex_instance_wire::number(cfg_bytes, id, 8);
        cfg_bytes.insert(cfg_bytes.end(), 16, 1);  // cid
        cfg_bytes.insert(cfg_bytes.end(), 32, 0);  // executable_hash
        cfg_bytes.insert(cfg_bytes.end(), 32, 0);  // info_hash
        const double rate = 48000.0;
        uint64_t rate_bits = 0;
        std::memcpy(&rate_bits, &rate, 8);
        hex_instance_wire::number(cfg_bytes, rate_bits, 8);
        hex_instance_wire::number(cfg_bytes, 8, 2);  // maximum
        cfg_bytes.push_back(0);                      // inputs
        cfg_bytes.push_back(2);                      // outputs
        cfg_bytes.push_back(0);                      // precision
        hex_instance_wire::number(cfg_bytes, 0, 3);  // reserved
        const auto push_text = [&](const char* s) {
            const size_t n = std::strlen(s);
            hex_instance_wire::number(cfg_bytes, n, 4);
            cfg_bytes.insert(cfg_bytes.end(), s, s + n);
        };
        push_text("/tmp");
        push_text("fixture");
        hex_instance_wire::Reader reader{cfg_bytes};
        const hex_instance_wire::Config cfg(reader);
        const auto run_packet = [&](uint8_t flag_byte) -> int {
            int fds[2]{-1, -1};
            assert(::socketpair(AF_UNIX, SOCK_STREAM, 0, fds) == 0);
            std::array<uint8_t, 64 + 4 * 8 + 12> raw{};
            std::memcpy(raw.data(), "HXVB", 4);
            write_le(raw.data() + 4, 3, 2);   // version 3 (identified notes)
            write_le(raw.data() + 6, 4, 2);   // 4 frames
            write_le(raw.data() + 8, 1, 2);   // 1 note
            for (size_t n = 0; n < 4; ++n) write_le(raw.data() + 16 + n * 8, cfg.binding[n], 8);
            uint8_t* ev = raw.data() + 64 + 4 * 8;
            write_le(ev + 0, 2, 2);     // offset 2
            ev[2] = 3;                  // len
            ev[3] = 0x90;               // note-on ch 0
            ev[4] = 60;                 // pitch
            ev[5] = 100;                // velocity
            ev[6] = flag_byte;          // is_live flag
            ev[7] = 0;
            write_le(ev + 8, 99, 4);    // note_id
            assert(hex_instance_wire::write_exact(fds[1], raw.data(), raw.size()));
            ::close(fds[1]);
            hex_instance_wire::Packet pkt;
            const int rc = pkt.read(fds[0], cfg, 0, 0);
            ::close(fds[0]);
            if (rc == 1) {
                assert(pkt.note_count == 1);
                assert(pkt.notes[0].is_live == (flag_byte != 0));
                assert(pkt.notes[0].note_id == 99);
            }
            return rc;
        };
        assert(run_packet(0) == 1);
        assert(run_packet(1) == 1);
        assert(run_packet(2) == -1);
    }

    return 0;
}
