// Original bounded SDK state stream exercised directly through IBStream / ISizeableStream.
#include "macos_state_stream.hpp"
#include <cassert>
#include <cstdint>
#include <limits>

int main() {
    using namespace hex_instance;
    using namespace Steinberg;

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
    return 0;
}
