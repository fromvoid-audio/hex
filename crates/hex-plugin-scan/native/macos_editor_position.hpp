// Bounded copied placement for one editor handoff, in AppKit screen points.
// Optional width/height hold the last accepted content size (1..=4096 each; 0,0 when absent).
#pragma once
#include <cmath>
#include <cstdint>

namespace hex_instance {
struct EditorPosition {
    double x=0,y=0;
    uint16_t width=0,height=0;
    bool has_size() const { return width!=0 || height!=0; }
    bool valid_size() const {
        return (!width && !height) || (width>=1 && width<=4096 && height>=1 && height<=4096);
    }
    bool valid() const {
        constexpr double limit=1000000.0;
        return std::abs(x)<=limit && std::abs(y)<=limit && valid_size();
    }
};
}
