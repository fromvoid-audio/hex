// Bounded copied placement for one editor handoff, in AppKit screen points.
#pragma once
#include <cmath>

namespace hex_instance {
struct EditorPosition {
    double x,y;
    bool valid() const {
        constexpr double limit=1000000.0;
        return std::abs(x)<=limit && std::abs(y)<=limit;
    }
};
}
