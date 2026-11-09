// The serialized SDK worker owns exact, bounded editor takeover. A plan
// names one original worker scope; no graph, DSP pointer or lease is retained.
#pragma once
#include "macos_host.hpp"
#include <optional>

namespace hex_instance {
struct EditorTouchTarget {
    ParamID parameter=0;
    ParamValue base=0;
    bool reading=false;

    bool valid() const {
        return parameter<=0x7fffffffu && std::isfinite(base) && base>=0 && base<=1;
    }
    bool matches(const EditorTouchTarget& other) const {
        return parameter==other.parameter && base==other.base && reading==other.reading;
    }
};
struct EditorTouchPlanSet {
    uint64_t scope=0;
    size_t count=0;
    std::array<EditorTouchTarget,HOST_EDIT_COUNT> targets{};

    bool valid() const {
        if(!scope || !count || count>targets.size()) return false;
        for(size_t n=0;n<count;++n)
            if(!targets[n].valid() || (n && targets[n-1].parameter>=targets[n].parameter)) return false;
        return true;
    }
    const EditorTouchTarget* find(ParamID id) const {
        for(size_t n=0;n<count;++n) if(targets[n].parameter==id) return &targets[n];
        return nullptr;
    }
    bool matches(const EditorTouchPlanSet& other) const {
        if(scope!=other.scope || count!=other.count) return false;
        for(size_t n=0;n<count;++n) if(!targets[n].matches(other.targets[n])) return false;
        return true;
    }
};
struct EditorRestoration {
    uint64_t sequence=0;
    uint64_t gesture=0;
    ParamID parameter=0;
};

class EditorTouch {
    struct Held {
        EditorTouchTarget target{};
        uint64_t scope=0,gesture=0,end_sequence=0;
        bool restoring=false,cancelled=false;
    };
    std::optional<EditorTouchPlanSet> current;
    std::array<Held,HOST_EDIT_COUNT> held{};
    uint64_t last_scope=0;
    bool scope_aborted=false;
    bool in_current_scope(const Held& row) const {
        if(!current || row.scope!=current->scope) return false;
        const auto* target=current->find(row.target.parameter);
        return target && target->matches(row.target);
    }
    const Held* find(ParamID id) const {
        for(const auto& row:held) if(row.gesture && row.target.parameter==id) return &row;
        return nullptr;
    }
    void abort_group() {
        scope_aborted=true;
        for(auto& row:held) if(row.scope==current->scope) row.cancelled=true;
    }
public:
    // End never clears a value in the quantum where its last Perform may
    // enter DSP. Release is eligible at the next real PCM boundary.
    bool open_quantum(const std::optional<EditorTouchPlanSet>& requested) {
        if(requested && (!current || !requested->matches(*current))) {
            if(requested->scope<=last_scope) return false;
            last_scope=requested->scope;scope_aborted=false;
        }
        current=requested;
        for(auto& row:held) if(row.gesture)
            row.restoring=row.end_sequence || row.cancelled || !in_current_scope(row);
        return true;
    }
    bool would_capture(const EditEvent& edit) const {
        if(scope_aborted || !current || edit.kind!=Perform || !current->find(edit.id)) return false;
        if(current->count>1 && (!edit.cohort || !edit.grouped_gesture)) return false;
        const auto* row=find(edit.id);
        return !row || (!row->restoring && row->gesture==edit.gesture && !row->end_sequence);
    }
    bool can_stage(const EditEvent& edit) const {
        if(is_group_marker(edit.kind)) return true;
        // Preserve single-target behavior when an old scope is restoring.
        if(current && current->count==1 && current->find(edit.id) && has_restorations()) return false;
        if(const auto* row=find(edit.id)) {
            // A queued edit cannot collide with the actual restoration.
            return !row->restoring && (!row->end_sequence || edit.gesture==row->gesture);
        }
        if(would_capture(edit)) {
            for(const auto& row:held) if(!row.gesture) return true;
            return false;
        }
        return true;
    }
    void applied(const EditEvent& edit) {
        if(edit.kind==GroupAborted && current && current->count>1) {
            // The take cannot succeed after an abort. Release retained values
            // on the next real quantum without fabricating any End.
            abort_group();
        } else if(current && current->count>1 && edit.kind==Perform && !edit.cohort && edit.grouped_gesture) {
            // An ungrouped Perform within a grouped member's active gesture
            // changes audio outside the grouped trace. Cancel its entire take.
            const auto* row=find(edit.id);
            if(row && row->gesture==edit.gesture && !row->restoring && in_current_scope(*row)) abort_group();
        }
        if(would_capture(edit) && !find(edit.id)) {
            for(auto& row:held) if(!row.gesture) {
                row={*current->find(edit.id),current->scope,edit.gesture,0,false,false};
                break;
            }
        }
        if(edit.kind==End) for(auto& row:held)
            if(!row.restoring && row.target.parameter==edit.id && row.gesture==edit.gesture)
                row.end_sequence=edit.sequence;
    }
    bool masks(ParamID id) const {
        const auto* row=find(id);
        return row && !row->restoring;
    }
    bool has_restorations() const {
        for(const auto& row:held) if(row.restoring) return true;
        return false;
    }
    template<typename Visit>
    void each_restoration(Visit visit) const {
        for(const auto& row:held) if(row.restoring)
            visit(row.target,!row.cancelled && row.end_sequence && in_current_scope(row));
    }
    auto processed() {
        std::array<EditorRestoration,HOST_EDIT_COUNT> reply{};
        size_t count=0;
        for(auto& row:held) if(row.restoring) {
            if(!row.cancelled && row.end_sequence && in_current_scope(row))
                reply[count++]={row.end_sequence,row.gesture,row.target.parameter};
            row={};
        }
        std::sort(reply.begin(),reply.begin()+count,
            [](const auto& left,const auto& right) {return left.sequence<right.sequence;});
        return reply;
    }
};
} // namespace hex_instance
