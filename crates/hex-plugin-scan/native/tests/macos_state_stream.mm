// Original bounded SDK state stream exercised directly through IBStream / ISizeableStream.
#include "macos_state_stream.hpp"
#include <array>
#include <cassert>
#include <cstdint>
#include <limits>

int main() {
    using namespace hex_instance;
    using namespace Steinberg;

    // An empty helper stream must not carry the maximum vendor payload.
    {
        auto* stream = new StateStream(StateStream::capacity);
        assert(stream->payload().capacity() == 0);
        assert(stream->seek(200, IBStream::kIBSeekSet, nullptr) == kResultOk);
        assert(stream->payload().capacity() == 0);
        unsigned char marker = 7;
        assert(stream->write(&marker, 1, nullptr) == kResultOk);
        const auto allocated = stream->payload().capacity();
        assert(allocated >= 201 && allocated < 512);
        assert(stream->reset_write_on_control(StateStream::capacity));
        assert(stream->setStreamSize(201) == kResultOk);
        assert(stream->payload().capacity() == allocated);
        assert(std::all_of(stream->payload().begin(), stream->payload().end(),
                           [](unsigned char b) { return b == 0; }));
        assert(stream->dispose_on_control());
    }

    // 1. Over-budget setStreamSize on a stream with an 8-byte remaining allowance
    //    records bounded overflow (including reserved prefix), keeps valid()==true,
    //    and allows reset_write_on_control + subsequent smaller capture.
    {
        auto* stream = new StateStream(8);
        const auto prefix = StateStream::capacity - 8;
        assert(stream->valid() && stream->exclusive() && stream->overflow() == 0);
        assert(stream->setStreamSize(9) == kInvalidArgument);
        assert(stream->valid());
        assert(stream->overflow() == static_cast<uint32_t>(prefix + 9));
        assert(stream->size() == 0);

        // Huge positive size overflow preserves a bounded uint32_t report and stays valid.
        assert(stream->setStreamSize(std::numeric_limits<int64>::max()) == kInvalidArgument);
        assert(stream->valid());
        assert(stream->overflow() == std::numeric_limits<uint32_t>::max());

        // Reset on control succeeds and clears overflow for a smaller Save.
        assert(stream->reset_write_on_control(16));
        assert(stream->valid() && stream->overflow() == 0 && stream->size() == 0);
        const unsigned char payload[4] = {'H', 'X', 'O', 'K'};
        int32 written = 0;
        assert(stream->setStreamSize(4) == kResultOk);
        assert(stream->write(const_cast<unsigned char*>(payload), 4, &written) == kResultOk);
        assert(written == 4 && stream->size() == 4 && stream->valid() && stream->overflow() == 0);
        assert(stream->dispose_on_control());
    }

    // 2. Negative setStreamSize still permanently invalidates the stream without overflow.
    {
        auto* stream = new StateStream(16);
        assert(stream->setStreamSize(-1) == kInvalidArgument);
        assert(!stream->valid());
        assert(stream->overflow() == 0);
        assert(!stream->reset_write_on_control(16));
        assert(stream->dispose_on_control());
    }

    // 3. Writes and setStreamSize on a read-only stream still refuse and invalidate.
    {
        const unsigned char source[4] = {'R', 'O', 'S', 'T'};
        auto* ro_write = new StateStream(source, 4);
        int32 written = 99;
        assert(ro_write->write(const_cast<unsigned char*>(source), 1, &written) == kInvalidArgument);
        assert(written == 0 && !ro_write->valid() && ro_write->overflow() == 0);
        assert(ro_write->dispose_on_control());

        auto* ro_size = new StateStream(source, 4);
        assert(ro_size->setStreamSize(2) == kInvalidArgument);
        assert(!ro_size->valid() && ro_size->overflow() == 0);
        assert(ro_size->dispose_on_control());
    }

    auto verify_capture_and_readback = [](StateStream* stream,
                                          const unsigned char* expected,
                                          size_t expected_len) {
        assert(stream->valid() && stream->exclusive() && stream->overflow() == 0);
        assert(stream->size() == expected_len);
        int64 reported = -1;
        assert(stream->getStreamSize(reported) == kResultOk);
        assert(reported == static_cast<int64>(expected_len));
        assert(std::memcmp(stream->payload().data(), expected, expected_len) == 0);

        assert(stream->rewind_read_only_on_control());
        std::array<unsigned char, 32> readback{};
        int32 read_bytes = 0;
        assert(stream->read(readback.data(), static_cast<int32>(expected_len), &read_bytes) ==
               kResultOk);
        assert(read_bytes == static_cast<int32>(expected_len));
        assert(std::memcmp(readback.data(), expected, expected_len) == 0);
        int32 trailing = 99;
        assert(stream->read(readback.data(), 1, &trailing) == kResultFalse);
        assert(trailing == 0);
    };

    // 4. Truncate then seek forward and write sparsely: discarded suffix bytes must not reappear.
    {
        auto* stream = new StateStream(32);
        IBStream* ib = nullptr;
        ISizeableStream* sizeable = nullptr;
        assert(stream->queryInterface(INLINE_UID_OF(IBStream), reinterpret_cast<void**>(&ib)) ==
               kResultOk);
        assert(stream->queryInterface(
                   INLINE_UID_OF(ISizeableStream), reinterpret_cast<void**>(&sizeable)) ==
               kResultOk);

        const unsigned char initial[6] = {'A', 'B', 'C', 'D', 'E', 'F'};
        int32 written = 0;
        assert(ib->write(const_cast<unsigned char*>(initial), 6, &written) == kResultOk &&
               written == 6);
        assert(sizeable->setStreamSize(2) == kResultOk);
        int64 pos = -1;
        assert(ib->seek(5, IBStream::kIBSeekSet, &pos) == kResultOk && pos == 5);
        const unsigned char marker = 'X';
        assert(ib->write(const_cast<unsigned char*>(&marker), 1, &written) == kResultOk &&
               written == 1);

        sizeable->release();
        ib->release();
        const unsigned char expected[6] = {'A', 'B', 0, 0, 0, 'X'};
        verify_capture_and_readback(stream, expected, sizeof(expected));
        assert(stream->dispose_on_control());
    }

    // 5. Sparse extension from empty stream zero-fills leading gap.
    {
        auto* stream = new StateStream(32);
        int64 pos = -1;
        assert(stream->seek(4, IBStream::kIBSeekSet, &pos) == kResultOk && pos == 4);
        const unsigned char tail[2] = {'Y', 'Z'};
        int32 written = 0;
        assert(stream->write(const_cast<unsigned char*>(tail), 2, &written) == kResultOk &&
               written == 2);
        const unsigned char expected[6] = {0, 0, 0, 0, 'Y', 'Z'};
        verify_capture_and_readback(stream, expected, sizeof(expected));
        assert(stream->dispose_on_control());
    }

    // 6. Explicit regrowth via setStreamSize after truncation zero-fills regrown bytes.
    {
        auto* stream = new StateStream(32);
        const unsigned char initial[6] = {'A', 'B', 'C', 'D', 'E', 'F'};
        int32 written = 0;
        assert(stream->write(const_cast<unsigned char*>(initial), 6, &written) == kResultOk &&
               written == 6);
        assert(stream->setStreamSize(2) == kResultOk);
        assert(stream->setStreamSize(5) == kResultOk);
        const unsigned char expected[5] = {'A', 'B', 0, 0, 0};
        verify_capture_and_readback(stream, expected, sizeof(expected));
        assert(stream->dispose_on_control());
    }

    // 7. Ordinary in-range overwrite preserves extent and surrounding retained bytes.
    {
        auto* stream = new StateStream(32);
        const unsigned char initial[6] = {'A', 'B', 'C', 'D', 'E', 'F'};
        int32 written = 0;
        assert(stream->write(const_cast<unsigned char*>(initial), 6, &written) == kResultOk &&
               written == 6);
        int64 pos = -1;
        assert(stream->seek(2, IBStream::kIBSeekSet, &pos) == kResultOk && pos == 2);
        const unsigned char patch[2] = {'x', 'y'};
        assert(stream->write(const_cast<unsigned char*>(patch), 2, &written) == kResultOk &&
               written == 2);
        const unsigned char expected[6] = {'A', 'B', 'x', 'y', 'E', 'F'};
        verify_capture_and_readback(stream, expected, sizeof(expected));
        assert(stream->dispose_on_control());
    }

    // 8. Reusing a read-only stream on control replaces bytes cleanly and refuses invalid inputs.
    {
        const unsigned char first[4] = {'S', 'T', '0', '1'};
        const unsigned char second[6] = {'R', 'E', 'S', 'T', '0', '2'};
        auto* stream = new StateStream(first, sizeof(first));
        assert(stream->valid() && stream->exclusive() && stream->size() == sizeof(first));
        assert(stream->reset_read_only_on_control(second, sizeof(second)));
        assert(stream->valid() && stream->exclusive() && stream->size() == sizeof(second));
        std::array<unsigned char, 8> readback{};
        int32 read_bytes = 0;
        assert(stream->read(readback.data(), static_cast<int32>(sizeof(second)), &read_bytes) ==
               kResultOk);
        assert(read_bytes == static_cast<int32>(sizeof(second)));
        assert(std::memcmp(readback.data(), second, sizeof(second)) == 0);
        // Empty reset is allowed even with nullptr source.
        assert(stream->reset_read_only_on_control(nullptr, 0));
        assert(stream->size() == 0 && stream->valid() && stream->exclusive());
        // Null non-empty and over-capacity requests are refused without corrupting validity.
        assert(!stream->reset_read_only_on_control(nullptr, 4));
        assert(!stream->reset_read_only_on_control(first, StateStream::capacity + 1));
        assert(stream->valid());
        // Non-exclusive stream refuses reset.
        stream->addRef();
        assert(!stream->reset_read_only_on_control(first, sizeof(first)));
        stream->release();
        assert(stream->dispose_on_control());
    }
    return 0;
}
