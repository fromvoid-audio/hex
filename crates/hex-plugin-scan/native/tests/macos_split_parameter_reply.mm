// Bounded multi-slice parameter output reply tests exercised directly through ParameterChanges & Packet.
#include "macos_instance_wire.hpp"
#include <cassert>
#include <cstring>

int main() {
    using namespace hex_instance;
    using namespace hex_instance_wire;

    const auto decode_point = [](const Packet& packet, size_t index, int32& offset, uint32& id, double& value) {
        const auto* p = packet.bytes.data() + header_size + packet.frames * 8 + index * 16;
        offset = static_cast<int32>(number(p, 2));
        id = static_cast<uint32>(number(p + 4, 4));
        const uint64_t bits = number(p + 8, 8);
        std::memcpy(&value, &bits, 8);
    };

    // 1. 64 + 64 boundary across two 64-frame slices preserves all 128 points verbatim
    //    with accurate slice-adjusted sample offsets.
    {
        Packet packet;
        packet.frames = 128;
        ParameterChanges slice0, slice1;
        assert(slice0.reset(64, true) && slice0.begin_sdk());
        for (ParamID id = 0; id < 64; ++id) {
            int32 idx = -1;
            auto* q = slice0.addParameterData(id, idx);
            assert(q && q->addPoint(static_cast<int32>(id), 0.25, idx) == kResultOk);
        }
        assert(slice0.end_sdk());
        assert(slice1.reset(64, true) && slice1.begin_sdk());
        for (ParamID id = 0; id < 64; ++id) {
            int32 idx = -1;
            auto* q = slice1.addParameterData(id, idx);
            assert(q && q->addPoint(static_cast<int32>(id), 0.75, idx) == kResultOk);
        }
        assert(slice1.end_sdk());

        assert(packet.append_parameter_reply(0, slice0));
        assert(packet.append_parameter_reply(64, slice1));
        assert(packet.reply_parameter_count == 128);
        for (size_t i = 0; i < 128; ++i) {
            int32 off = -1;
            uint32 id = 0;
            double val = 0;
            decode_point(packet, i, off, id, val);
            assert(off == static_cast<int32>(i));
            assert(id == static_cast<uint32>(i % 64));
            assert(val == (i < 64 ? 0.25 : 0.75));
        }
    }

    // 2. 65 + 65 same-ID boundary across two 64-frame slices coalesces to the 65 latest
    //    points with accurate second-slice sample offsets and values.
    {
        Packet packet;
        packet.frames = 128;
        ParameterChanges slice0, slice1;
        assert(slice0.reset(64, true) && slice0.begin_sdk());
        for (ParamID id = 0; id < 65; ++id) {
            int32 idx = -1;
            auto* q = slice0.addParameterData(1001 + id, idx);
            assert(q && q->addPoint(10, 0.25, idx) == kResultOk);
        }
        assert(slice0.end_sdk());
        assert(slice1.reset(64, true) && slice1.begin_sdk());
        for (ParamID id = 0; id < 65; ++id) {
            int32 idx = -1;
            auto* q = slice1.addParameterData(1001 + id, idx);
            assert(q && q->addPoint(12, 0.75, idx) == kResultOk);
        }
        assert(slice1.end_sdk());

        assert(packet.append_parameter_reply(0, slice0));
        assert(packet.append_parameter_reply(64, slice1));
        assert(packet.reply_parameter_count == 65);
        for (size_t i = 0; i < 65; ++i) {
            int32 off = -1;
            uint32 id = 0;
            double val = 0;
            decode_point(packet, i, off, id, val);
            assert(off == 76); // 64 + 12
            assert(id == static_cast<uint32>(1001 + i));
            assert(val == 0.75);
        }
    }

    // 3. 128 + 128 completely distinct IDs across two slices stays strictly bounded at
    //    PARAMETER_POINT_COUNT (128) and preserves the latest 128 observations in order.
    {
        Packet packet;
        packet.frames = 128;
        ParameterChanges slice0, slice1;
        assert(slice0.reset(64, true) && slice0.begin_sdk());
        for (ParamID id = 0; id < 128; ++id) {
            int32 idx = -1;
            auto* q = slice0.addParameterData(id, idx);
            assert(q && q->addPoint(5, 0.125, idx) == kResultOk);
        }
        assert(slice0.end_sdk());
        assert(slice1.reset(64, true) && slice1.begin_sdk());
        for (ParamID id = 0; id < 128; ++id) {
            int32 idx = -1;
            auto* q = slice1.addParameterData(1000 + id, idx);
            assert(q && q->addPoint(7, 0.875, idx) == kResultOk);
        }
        assert(slice1.end_sdk());

        assert(packet.append_parameter_reply(0, slice0));
        assert(packet.append_parameter_reply(64, slice1));
        assert(packet.reply_parameter_count == PARAMETER_POINT_COUNT);
        for (size_t i = 0; i < PARAMETER_POINT_COUNT; ++i) {
            int32 off = -1;
            uint32 id = 0;
            double val = 0;
            decode_point(packet, i, off, id, val);
            assert(off == 71); // 64 + 7
            assert(id == static_cast<uint32>(1000 + i));
            assert(val == 0.875);
        }
    }

    // 4. Out-of-range slice offset still fails explicitly.
    {
        Packet packet;
        packet.frames = 64;
        ParameterChanges bad;
        assert(bad.reset(64, true) && bad.begin_sdk());
        int32 idx = -1;
        auto* q = bad.addParameterData(7, idx);
        assert(q && q->addPoint(10, 0.5, idx) == kResultOk);
        assert(bad.end_sdk());
        assert(!packet.append_parameter_reply(60, bad));
    }
    return 0;
}
