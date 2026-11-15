// Main-lane controller observations. Fixed copied pages contain no SDK object
// pointers and grant no edit, graph or DSP-application acknowledgement.
#pragma once
#include "macos_host.hpp"
#include "macos_editor_position.hpp"

namespace hex_instance {
inline constexpr size_t CONTROLLER_PAGE_COUNT=4;
inline constexpr uint8 TOGGLE_EDITOR=6;
inline constexpr uint8 CLOSE_EDITOR=7;
inline constexpr uint8 SELECTED_PARAMETERS=8;
inline constexpr uint8 DETACH_EDITOR=11;
inline constexpr uint8 REOPEN_EDITOR=12;
inline constexpr uint8 SEAL_EDITOR_EDITS=13;
inline constexpr uint8 DETACH_EDITOR_WHEN_IDLE=14;
inline bool editor_request(uint8 kind) {
    return kind==TOGGLE_EDITOR || kind==CLOSE_EDITOR || kind==DETACH_EDITOR || kind==REOPEN_EDITOR || kind==SEAL_EDITOR_EDITS || kind==DETACH_EDITOR_WHEN_IDLE;
}
struct ControllerQuery {
    uint16 first=0;uint64 revision=0;
    uint8 kind=0;uint32 id=0;uint64 bits=0;String128 text{};
    bool operator==(const ControllerQuery& other) const {
        return first==other.first && revision==other.revision && kind==other.kind &&
            id==other.id && bits==other.bits && !std::memcmp(text,other.text,sizeof(text));
    }
};
// The reopen kind uses the reserved revision/bits words as two doubles.
inline EditorPosition editor_reopen_position(const ControllerQuery& query) {
    EditorPosition position{};
    std::memcpy(&position.x,&query.revision,8);
    std::memcpy(&position.y,&query.bits,8);
    return position;
}
struct ControllerConversion {
    uint32 id=0;uint8 status=3; // converted/refused/unsupported/missing
    double normalized=0,plain=std::numeric_limits<double>::quiet_NaN();
    bool formatted=false;String128 display{};
};
struct ControllerParameter {
    ParameterInfo info{};
    uint64 editor_sequence=0;
    ParamValue normalized=0;
    double plain=0,default_plain=0;
    String128 display{},default_display{};
    bool formatted=false,default_formatted=false;
};
struct ControllerParameterPage {
    uint64 revision=0;
    uint16 first=0,total=0,count=0;
    bool changed=false;
    bool conversion=false;ControllerConversion converted{};
    std::array<unsigned char,32> descriptor_signature{};
    std::array<ControllerParameter,CONTROLLER_PAGE_COUNT> parameters{};
};
static_assert(sizeof(ControllerParameterPage)<8192,
              "one bounded controller page, separate from the existing Host pool");

// Validate SDK UTF-16 and clear bytes after its terminator before IPC. A plugin
// may leave padding unspecified; it is neither text nor data to export.
inline bool controller_text(String128 value) {
    for(size_t n=0;n<128;++n) {
        const auto c=static_cast<uint16>(value[n]);
        if(!c) {
            for(size_t tail=n;tail<128;++tail) value[tail]=0;
            return true;
        }
        if(c>=0xD800 && c<=0xDBFF) {
            if(++n>=128) return false;
            const auto next=static_cast<uint16>(value[n]);
            if(next<0xDC00 || next>0xDFFF) return false;
        } else if(c>=0xDC00 && c<=0xDFFF) return false;
    }
    return false;
}
inline bool valid_controller_query(const ControllerQuery& query) {
    if(query.first>HOST_PARAMETER_COUNT ||
       (query.first && !query.revision && query.kind!=SELECTED_PARAMETERS) || query.kind>DETACH_EDITOR_WHEN_IDLE) return false;
    String128 text{};std::memcpy(text,query.text,sizeof(text));
    if(!controller_text(text) || std::memcmp(text,query.text,sizeof(text))) return false;
    if(query.kind==REOPEN_EDITOR) {
        return !query.first && !text[0] &&
            (query.id==1?editor_reopen_position(query).valid():!query.id && !query.revision && !query.bits);
    }
    if(editor_request(query.kind)) return !query.first && !query.revision && !query.id && !query.bits && !text[0];
    if(query.kind==SELECTED_PARAMETERS) return !query.revision && !query.id && !query.bits && !text[0];
    if(query.kind>5) return false;
    if(!query.kind) return !query.id && !query.bits && !text[0];
    if(query.first || query.id>static_cast<uint32>(std::numeric_limits<int32>::max())) return false;
    double value=0;std::memcpy(&value,&query.bits,8);
    if(query.kind==2) return !query.bits;
    if(text[0]) return false;
    if(query.kind>=4) return !query.bits;
    return std::isfinite(value) && (query.kind!=1 || (value>=0 && value<=1));
}
inline bool controller_conversion(IEditController* controller,const ParameterSnapshot* snapshot,
                                   const ControllerQuery& query,ControllerConversion& result) {
    result={};result.id=query.id;
    bool exists=false;
    if(snapshot) for(uint32 n=0;n<snapshot->count;++n) exists|=snapshot->parameters[n].info.id==query.id;
    if(!controller || !exists) return true;
    double input=0;std::memcpy(&input,&query.bits,8);
    double normalized=input;
    if(query.kind==2) {
        String128 text{};std::memcpy(text,query.text,sizeof(text));
        const auto parsed=controller->getParamValueByString(query.id,text,normalized);
        if(parsed!=kResultOk) {result.status=parsed==kNotImplemented?2:1;return true;}
        // Success with an invalid normalized value violates the SDK contract.
        if(!std::isfinite(normalized) || normalized<0 || normalized>1) return false;
    } else if(query.kind>=3) {
        if(query.kind==4) input=-std::numeric_limits<double>::infinity();
        if(query.kind==5) input=std::numeric_limits<double>::infinity();
        normalized=controller->plainParamToNormalized(query.id,input);
        if(!std::isfinite(normalized) || normalized<0 || normalized>1) {result.status=1;return true;}
    }
    result.status=0;result.normalized=normalized;
    result.plain=controller->normalizedParamToPlain(query.id,normalized);
    result.formatted=controller->getParamStringByValue(query.id,normalized,result.display)==kResultOk;
    if(result.formatted) return controller_text(result.display);
    std::memset(result.display,0,sizeof(result.display));return true;
}
inline bool controller_page(IEditController* controller,const ParameterSnapshot* snapshot,
                            const ControllerQuery& query,ControllerParameterPage& result) {
    const auto first=query.first;const auto revision=query.revision;
    if(![NSThread isMainThread] || !valid_controller_query(query) || editor_request(query.kind)) return false;
    result={};result.first=first;
    if((controller && !snapshot) || (snapshot && snapshot->count>HOST_PARAMETER_COUNT)) return false;
    if(snapshot) {
        result.revision=snapshot->revision;result.total=static_cast<uint16>(snapshot->count);
        result.descriptor_signature=snapshot->descriptor_signature;
    }
    if((revision && revision!=result.revision) || first>result.total) {result.changed=true;return true;}
    // Selected eligibility reads remain fresh while PCM feedback advances the
    // ordinary enumeration revision. The original reader validates its target
    // row before granting eligibility; this page itself is only copied data.
    if(query.kind && query.kind!=SELECTED_PARAMETERS) {
        result.conversion=true;
        return controller_conversion(controller,snapshot,query,result.converted);
    }
    if(!controller) return result.total==0;
    for(size_t index=first;index<result.total && result.count<result.parameters.size();++index) {
        auto& parameter=result.parameters[result.count];
        parameter.info=snapshot->parameters[index].info;
        // Same main-lane observation as the value and its formatted text.
        parameter.editor_sequence=snapshot->parameters[index].editor_sequence;
        if(!controller_text(parameter.info.title) || !controller_text(parameter.info.shortTitle) ||
            !controller_text(parameter.info.units)) return false;
        const auto id=parameter.info.id;
        parameter.normalized=controller->getParamNormalized(id);
        if(!std::isfinite(parameter.normalized) || parameter.normalized<0 || parameter.normalized>1) return false;
        // Plain infinities are meaningful (for example minus-infinity dB).
        // The wire represents them explicitly, never as numeric UI bounds.
        parameter.plain=controller->normalizedParamToPlain(id,parameter.normalized);
        parameter.default_plain=controller->normalizedParamToPlain(id,parameter.info.defaultNormalizedValue);
        const auto format=[&](ParamValue value,String128& text,bool& supported) {
            supported=controller->getParamStringByValue(id,value,text)==kResultOk;
            if(supported) return controller_text(text);
            std::memset(text,0,sizeof(String128));return true;
        };
        if(!format(parameter.normalized,parameter.display,parameter.formatted) ||
            !format(parameter.info.defaultNormalizedValue,parameter.default_display,parameter.default_formatted)) return false;
        ++result.count;
    }
    return true;
}
} // namespace hex_instance
