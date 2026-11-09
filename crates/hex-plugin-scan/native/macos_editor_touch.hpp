// One opt-in exact editor takeover. The serialized instance worker owns this
// state. No graph, DSP pointer, worker lease or second edit reader is retained.
#pragma once
#include "macos_host.hpp"
#include <optional>

namespace hex_instance {
struct EditorTouchPlan {
    uint64_t scope=0;
    ParamID parameter=0;
    ParamValue base=0;
    bool reading=false;

    bool valid() const {
        return scope && parameter<=0x7fffffffu && std::isfinite(base) && base>=0 && base<=1;
    }
    bool matches(const EditorTouchPlan& other) const {
        return scope==other.scope && parameter==other.parameter && base==other.base && reading==other.reading;
    }
};
struct EditorRestoration {
    uint64_t sequence=0;
    uint64_t gesture=0;
    ParamID parameter=0;
};

class EditorTouch {
    struct Held {
        EditorTouchPlan plan{};
        uint64_t gesture=0;
        uint64_t end_sequence=0;
    };
    std::optional<EditorTouchPlan> current;
    std::optional<Held> held;
    bool restoring=false;
    uint64_t last_scope=0;
public:
    // End never clears a value in the quantum where its last Perform may
    // enter DSP. Release is eligible at the next real PCM boundary.
    bool open_quantum(const std::optional<EditorTouchPlan>& requested) {
        if(requested && (!current || !requested->matches(*current))) {
            if(requested->scope<=last_scope) return false;
            last_scope=requested->scope;
        }
        current=requested;
        restoring=held && (held->end_sequence || !current || !current->matches(held->plan));
        return true;
    }
    bool would_capture(const EditEvent& edit) const {
        return !restoring && current && edit.kind==Perform && edit.id==current->parameter &&
            (!held || (held->gesture==edit.gesture && !held->end_sequence));
    }
    bool can_stage(const EditEvent& edit) const {
        if(is_group_marker(edit.kind)) return true;
        if(restoring && current && edit.id==current->parameter) return false;
        if(!held || edit.id!=held->plan.parameter) return true;
        // Do not let a queued new gesture collide with the actual release.
        // The original ordered FIFO retains it until the following quantum.
        return !restoring && (!held->end_sequence || edit.gesture==held->gesture);
    }
    void applied(const EditEvent& edit) {
        if(would_capture(edit) && !held) held=Held{*current,edit.gesture,0};
        if(held && !restoring && edit.kind==End && edit.id==held->plan.parameter &&
            edit.gesture==held->gesture) held->end_sequence=edit.sequence;
    }
    bool masks(ParamID id) const {
        return held && !restoring && held->plan.parameter==id;
    }
    std::optional<EditorTouchPlan> restore_target() const {
        return restoring && held?std::optional{held->plan}:std::nullopt;
    }
    bool ending_in_scope() const {
        return restoring && held && current && current->matches(held->plan) && held->end_sequence;
    }
    std::optional<EditorRestoration> processed() {
        if(!restoring || !held) return {};
        const auto old=*held;
        held.reset();restoring=false;
        // A cancellation is not a successful recorded gesture end. It may
        // restore sound, but only the ordered End permits a terminal fact.
        if(!current || !current->matches(old.plan) || !old.end_sequence) return {};
        return EditorRestoration{old.end_sequence,old.gesture,old.plan.parameter};
    }
};
} // namespace hex_instance
