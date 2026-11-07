// Original flexible-bus SDK oracle. No third-party DSP or installed module.
#define HEX_LIFECYCLE_FIXTURE 1
#include "macos_fixture.cpp"
#include "pluginterfaces/vst/ivstparameterchanges.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivstevents.h"
#include "pluginterfaces/vst/ivstprocesscontext.h"
#include <cmath>
#include <algorithm>
#include <array>
#include <stdexcept>
#include <string>
#include <vector>
#include <csignal>
#include <unistd.h>
#include <limits>
#include <pthread.h>
#include <cstdio>
#include <cstdlib>
#include "pluginterfaces/base/ibstream.h"
#include "pluginterfaces/base/iplugincompatibility.h"
#ifndef HEX_FIXTURE_RESIDUAL_ECHO
#define HEX_FIXTURE_RESIDUAL_ECHO 0
#endif
#ifndef HEX_FIXTURE_INFINITE_TAIL
#define HEX_FIXTURE_INFINITE_TAIL 0
#endif
#ifndef HEX_INSTANCE_VARIANT
#define HEX_INSTANCE_VARIANT 0
#endif

namespace {
struct Observation {
    std::atomic<int> violations{0}, created{0}, initialized{0}, terminated{0};
    std::atomic<int> arrangements{0}, setups{0}, activations{0}, starts{0}, stops{0}, blocks{0}, flushes{0};
} observed;
int next_mode=0,next_fault=HEX_INSTANCE_VARIANT;
bool control() { const bool valid=pthread_main_np()!=0; if (!valid) ++observed.violations; return valid; }
const TUID processing_id={0x48,0x45,0x58,0x50,1,2,3,4,5,6,7,8,9,10,11,12};

struct RestoreObservation {
    int component_set=0,controller_component=0,controller_set=0,info_reads=0,value_reads=0;
    int component_gain=0,controller_gain=0,scroll=7,latency=0,phase=0;
    int component_get=0,controller_get=0,controller_created=0,controller_terminated=0,controller_destroyed=0;
} restoration;
static int bits(float value) { int result=0;std::memcpy(&result,&value,4);return result; }
static void publish_trace(const char* path,const int* values,size_t count) {
    if (!path || std::strlen(path)>4096) return;
    const auto pending=std::string(path)+".next";
    auto* output=std::fopen(pending.c_str(),"wb");if (!output) return;
    const bool written=std::fwrite(values,sizeof(int),count,output)==count;
    if (std::fclose(output)==0 && written) std::rename(pending.c_str(),path);
}
static bool module_event(char event) {
    const auto* path=std::getenv("HEX_ORIGINAL_INSTANCE_MODULE_EVENTS");
    if(!path) return true;
    if(!std::strlen(path) || std::strlen(path)>4096) return false;
    auto* output=std::fopen(path,"ab");if(!output) return false;
    const bool written=std::fputc(event,output)!=EOF;
    return std::fclose(output)==0 && written;
}
static bool controller_release_observed=false;
static bool defer_module_event(const char& event) {
    CFRunLoopObserverContext context{};
    context.info=const_cast<char*>(&event);
    context.release=[](const void* info) {
        const auto event=*static_cast<const char*>(info);
        if(event=='C') controller_release_observed=true;
        module_event(event);
    };
    auto observer=CFRunLoopObserverCreate(kCFAllocatorDefault,kCFRunLoopBeforeWaiting,false,0,
        [](CFRunLoopObserverRef,CFRunLoopActivity,void*) {},&context);
    if(!observer) return false;
    CFAutorelease(observer);
    return true;
}
static void trace(int stage,int factory_refs=-1) {
    // Opt-in original fixture control observations, atomically published so a
    // guardian exit cannot truncate the last completed phase. Normal DSP does
    // no file I/O; only explicit fault barriers trace on processing entry.
    const int values[]={stage,observed.violations,observed.created,observed.initialized,observed.terminated,
        observed.arrangements,observed.setups,observed.activations,observed.starts,observed.stops,
        observed.blocks,observed.flushes,static_cast<int>(objects.load()),factory_refs};
    publish_trace(std::getenv("HEX_ORIGINAL_INSTANCE_TRACE"),values,14);
    const int restored[]={restoration.component_set,restoration.controller_component,restoration.controller_set,
        restoration.info_reads,restoration.value_reads,restoration.component_gain,restoration.controller_gain,
        restoration.scroll,restoration.latency,restoration.phase,restoration.component_get,restoration.controller_get,
        restoration.controller_created,restoration.controller_terminated,restoration.controller_destroyed};
    publish_trace(std::getenv("HEX_ORIGINAL_RESTORE_TRACE"),restored,15);
}
static void fault_hang(int stage) {
    trace(stage);
    const auto* release=std::getenv("HEX_ORIGINAL_INSTANCE_RELEASE");
    if (!release || std::strlen(release)>4096) { for (;;) ::pause(); }
    while (::access(release,F_OK)!=0) ::usleep(1000);
}
static void fault_crash(int stage) { trace(stage);std::raise(SIGSEGV);std::_Exit(70); }
struct MainThreadTail { bool enabled=false;~MainThreadTail(){if(enabled)fault_hang(13);} };
static void arm_main_thread_tail(){static thread_local MainThreadTail tail;tail.enabled=true;}
const TUID restore_controller_id={0x48,0x45,0x58,0x43,1,2,3,4,5,6,7,8,9,10,11,12};
// Original one-parameter SDK fixtures preserve the same opaque state shape.
// Distinct variants exercise actual metadata and processing, not host guesses.
static ParamID parameter_id(int variant) { return variant==80 || variant==89 || variant==92?0u:7u; }
constexpr ParamID drag_axis_id=11;
constexpr int mixed_input_fixture=143, held_input_fixture=144;
static bool mixed_parameter_input(int variant) { return variant==mixed_input_fixture || variant==held_input_fixture; }
static bool saturated_editor_drag(int variant) { return variant==87 || variant==131; }
static int32 parameter_inventory_size() {
    const auto* shape=std::getenv("HEX_ORIGINAL_PARAMETER_INVENTORY");
    if(!shape) return 1;
    if(!std::strcmp(shape,"2593") || !std::strcmp(shape,"duplicate")) return 2593;
    if(!std::strcmp(shape,"2625")) return 2625;
    if(!std::strcmp(shape,"9")) return 9;
    return 1;
}
static double parameter_value(int variant,double value) {
    if(variant==81) return std::round(value*3.0)/3.0;
    if(variant==82) return value>=0.5?1.0:0.0;
    return value;
}
// The latency fixture invokes one additional restart synchronously during
// activation. Its controller owns this borrowed handler on the main lane.
IComponentHandler* latency_handler=nullptr;
class RestoreController final : public IEditController {
    std::atomic<uint32> refs{1};
    const int fault=next_fault;
    const int32 inventory_size=(saturated_editor_drag(fault) || mixed_parameter_input(fault))?2:parameter_inventory_size();
    FUnknown* context=nullptr;
    IComponentHandler* handler=nullptr;
    float component_gain=0;uint32 scroll=7;
    ParamValue drag_axis=0.25;
    CFRunLoopTimerRef edit_timer=nullptr;bool edits_sent=false;
    static void edit_tick(CFRunLoopTimerRef,void* context) {
        auto* self=static_cast<RestoreController*>(context);
        const auto* release=std::getenv("HEX_ORIGINAL_INSTANCE_RELEASE");
        if(!self || self->edits_sent || !release || std::strlen(release)>4096 || ::access(release,F_OK)!=0) return;
        self->edits_sent=true;CFRunLoopTimerInvalidate(self->edit_timer);
        if(self->fault==134) {
            static constexpr char released='R';
            if(!defer_module_event(released)) ++observed.violations;
            return;
        }
        if(self->fault==105) {
            self->handler->restartComponent(kLatencyChanged);
            return;
        }
        if(self->fault==141 || self->fault==142) {
            self->handler->restartComponent(kParamTitlesChanged);
            return;
        }
        const auto id=parameter_id(self->fault);
        if(self->fault==86) {
            self->handler->beginEdit(id);
            self->handler->performEdit(id,0.5);
            self->handler->endEdit(id);
            // A stopped controller emits standalone values in the same
            // main-loop turn, including more than the host FIFO can retain.
            self->component_gain=3.5;
            for(int n=1;n<=24;++n) self->handler->performEdit(id,0.5+n/64.0);
        }
        else if(self->fault==133)
            self->handler->performEdit(id,std::numeric_limits<ParamValue>::quiet_NaN());
        else if(self->fault==135) {
            self->handler->beginEdit(id);
            for(int n=0;n<12;++n) self->handler->performEdit(id,0.25);
            self->component_gain=3.5;
            self->handler->performEdit(id,0.875);
            self->handler->endEdit(id);
        }
        else if(saturated_editor_drag(self->fault)) {
            // Emit the next gesture before main can dispatch the previous one.
            const int gestures=self->fault==131?5:1;
            for(int drag=0;drag<gestures;++drag) {
                const double gain=0.875-(gestures-1-drag)*0.09375;
                const double axis=0.625-(gestures-1-drag)*0.0625;
                self->handler->beginEdit(id);self->handler->beginEdit(drag_axis_id);
                for(int n=0;n<96;++n) {
                    self->handler->performEdit(id,n==95?gain:0.25);
                    self->handler->performEdit(drag_axis_id,n==95?axis:0.125);
                }
                self->handler->endEdit(id);self->handler->endEdit(drag_axis_id);
            }
            // The editor has already painted the final position. Treat any
            // setter replay during the drain as a regression.
            self->component_gain=3.5;
            self->drag_axis=0.625;
        }
        else {
            self->handler->beginEdit(id);
            if(self->fault!=132) self->component_gain=3.0;
            const int repeats=self->fault==116?12:1;
            for(int n=0;n<repeats;++n) self->handler->performEdit(id,0.75);
            self->handler->endEdit(id);
        }
        trace(25);
    }
public:
    RestoreController() { ++objects;++restoration.controller_created; }
    ~RestoreController() {
        if(edit_timer) {CFRunLoopTimerInvalidate(edit_timer);CFRelease(edit_timer);}
        if (!control() || context || handler) ++observed.violations;
        --objects;++restoration.controller_destroyed;
    }
    uint32 PLUGIN_API addRef() override { return ++refs; }
    uint32 PLUGIN_API release() override { const auto value=--refs;if(!value)delete this;return value; }
    tresult PLUGIN_API queryInterface(const TUID iid,void** out) override {
        if (!iid || !out) return kInvalidArgument;*out=nullptr;
        if (!same(iid,INLINE_UID_OF(FUnknown)) && !same(iid,INLINE_UID_OF(IEditController))) return kNoInterface;
        *out=static_cast<IEditController*>(this);addRef();return kResultOk;
    }
    tresult PLUGIN_API initialize(FUnknown* host) override {
        if (!control() || context || !host) return kResultFalse;
        context=host;context->addRef();return kResultOk;
    }
    tresult PLUGIN_API terminate() override {
        if (!control() || !context || handler) return kResultFalse;
        if(fault==134) {
            static constexpr char released='C';
            if(!defer_module_event(released)) return kResultFalse;
        }
        context->release();context=nullptr;++restoration.controller_terminated;return kResultOk;
    }
    tresult PLUGIN_API setComponentHandler(IComponentHandler* value) override {
        if (!control() || !context || (value && handler)) return kResultFalse;
        if(value)value->addRef();if(handler)handler->release();handler=value;
        if(fault==104 || fault==105 || fault==118 || fault==119) latency_handler=handler;
        if(value && ((fault>=85 && fault<=89) || fault==105 || fault==116 || (fault>=131 && fault<=135) || fault==141 || fault==142)) {
            CFRunLoopTimerContext timer_context{};timer_context.info=this;
            edit_timer=CFRunLoopTimerCreate(nullptr,CFAbsoluteTimeGetCurrent(),0.001,0,0,edit_tick,&timer_context);
            if(!edit_timer) return kResultFalse;
            CFRunLoopAddTimer(CFRunLoopGetMain(),edit_timer,kCFRunLoopDefaultMode);
        }
        if(!value && edit_timer) CFRunLoopTimerInvalidate(edit_timer);
        return kResultOk;
    }
    tresult PLUGIN_API setComponentState(IBStream* stream) override {
        if (!control() || !context || !handler || !stream) return kResultFalse;
        ++restoration.controller_component;trace(17);
        if(fault==117) { restoration.phase=2;return kNotImplemented; }
        if(fault==42)return kResultFalse;
        if(fault==43)throw std::runtime_error("original controller synchronization exception");
        if(fault==44)fault_crash(17);
        if(fault==45)fault_hang(17);
        std::array<unsigned char,32> data{};int32 read=0;
        if(stream->read(data.data(),data.size(),&read)!=kResultOk || read!=32 || std::memcmp(data.data(),"HXPS",4)) return kResultFalse;
        if (restoration.component_set && restoration.phase!=1) {++observed.violations;return kResultFalse;}
        std::memcpy(&component_gain,data.data()+8,4);
        if(!std::isfinite(component_gain) || component_gain<0 || component_gain>4)return kResultFalse;
        if((fault==110 || fault==111) && handler->restartComponent(kLatencyChanged)!=kResultOk) return kResultFalse;
        if(fault==123) handler->restartComponent(kParamValuesChanged|kParamIDMappingChanged);
        if(fault==53)stream->write(data.data(),1,nullptr); // ignored illegal write must still invalidate stream
        restoration.controller_gain=bits(component_gain);restoration.phase=2;trace(17);return kResultOk;
    }
    tresult PLUGIN_API setState(IBStream* stream) override {
        if (!control() || !context || !handler || !stream || restoration.phase!=2) return kResultFalse;
        ++restoration.controller_set;trace(18);
        if(fault==46)return kResultFalse;
        if(fault==47)throw std::runtime_error("original controller state exception");
        if(fault==48)fault_crash(18);
        if(fault==49)fault_hang(18);
        std::array<unsigned char,8> data{};int32 read=0;
        if(stream->read(data.data(),data.size(),&read)!=kResultOk || read!=8 || std::memcmp(data.data(),"HXCT",4))return kResultFalse;
        std::memcpy(&scroll,data.data()+4,4);
        if(fault==54)stream->write(data.data(),1,nullptr);
        if(fault==123) handler->restartComponent(kParamValuesChanged|kParamIDMappingChanged);
        restoration.scroll=static_cast<int>(scroll);restoration.phase=3;trace(18);return kResultOk;
    }
    tresult PLUGIN_API getState(IBStream* stream) override {
        if (!control() || !context || !handler || !stream) return kResultFalse;
        ++restoration.controller_get;
        if(fault==123 && restoration.controller_get==2) handler->restartComponent(kParamIDMappingChanged);
        if(fault==96 && restoration.controller_get==2) {
            std::vector<unsigned char> huge(512*1024);return stream->write(huge.data(),static_cast<int32>(huge.size()),nullptr);
        }
        // Original sensitivity fixture: an edit admitted during a live SDK
        // state read must not let that pre-edit component snapshot escape.
        if(fault==90 && restoration.controller_get>1 && !edits_sent) {
            edits_sent=true;
            handler->beginEdit(parameter_id(fault));component_gain=3.0;
            handler->performEdit(parameter_id(fault),0.75);
            handler->endEdit(parameter_id(fault));trace(25);
        }
        std::array<unsigned char,8> data{};std::memcpy(data.data(),"HXCT",4);std::memcpy(data.data()+4,&scroll,4);
        int32 written=0;const auto result=stream->write(data.data(),data.size(),&written);
        trace(20);return result==kResultOk && written==8?kResultOk:kResultFalse;
    }
    int32 PLUGIN_API getParameterCount() override { return control() && context?inventory_size:-1; }
    tresult PLUGIN_API getParameterInfo(int32 index,ParameterInfo& info) override {
        if (!control() || !context || index<0 || index>=inventory_size) return kInvalidArgument;
        ++restoration.info_reads;info={};info.id=parameter_id(fault);info.defaultNormalizedValue=fault==81?1.0/3.0:(fault==82?0.0:0.25);
        // Keep the low gain ID last to exercise SDK page order separately
        // from the host's sorted processing lookup.
        if(inventory_size>1 && index<inventory_size-1) {
            if(saturated_editor_drag(fault) || mixed_parameter_input(fault)) {
                info.id=drag_axis_id;
                if(fault==held_input_fixture) info.flags=ParameterInfo::kCanAutomate;
                return kResultOk;
            }
            const auto* shape=std::getenv("HEX_ORIGINAL_PARAMETER_INVENTORY");
            if(index!=inventory_size-2 || !shape || std::strcmp(shape,"duplicate"))
                info.id=static_cast<ParamID>(1001+index);
            info.flags=ParameterInfo::kIsReadOnly;
            const char16_t name[]=u"observed";
            std::memcpy(info.title,name,sizeof(name));std::memcpy(info.shortTitle,name,sizeof(name));
            return kResultOk;
        }
        if(fault==80 || fault==81 || fault==82 || fault==91 || fault==92 || mixed_parameter_input(fault)) info.flags=ParameterInfo::kCanAutomate;
        if(fault==91 || fault==92) {const char16_t unit[]=u"ratio";std::memcpy(info.units,unit,sizeof(unit));}
        if(fault==81) info.stepCount=3;
        if(fault==141 || (fault==142 && !edits_sent)) info.flags=ParameterInfo::kCanAutomate;
        if(fault==141 && edits_sent) info.stepCount=3;
        if(fault==82) {info.stepCount=1;info.flags|=ParameterInfo::kIsBypass;}
        if(fault==83 || fault==84 || fault==88) info.flags=ParameterInfo::kIsReadOnly;
        if(fault==84) info.flags|=ParameterInfo::kIsHidden;
        const char16_t name[]=u"gain";std::memcpy(info.title,name,sizeof(name));std::memcpy(info.shortTitle,name,sizeof(name));return kResultOk;
    }
    ParamValue PLUGIN_API getParamNormalized(ParamID id) override {
        if(control() && context && (saturated_editor_drag(fault) || mixed_parameter_input(fault)) && id==drag_axis_id) return drag_axis;
        if(control() && context && id>1000 && id<static_cast<ParamID>(1000+inventory_size)) return 0.25;
        if (!control() || !context || id!=parameter_id(fault)) {++observed.violations;return 0;}
        ++restoration.value_reads;trace(19);
        if(fault==57)fault_hang(19);
        return fault==56?std::numeric_limits<double>::quiet_NaN():component_gain/4.0;
    }
    tresult PLUGIN_API getParamStringByValue(ParamID id,ParamValue value,String128 text) override {
        if(fault!=91 && fault!=92 && fault!=93) return kNotImplemented;
        if(!control() || !context || id!=parameter_id(fault) || !std::isfinite(value) || value<0 || value>1) return kInvalidArgument;
        // Only the explicit controller-page query invokes formatting. Holding
        // this foreign main-lane getter must not hold the PCM worker or permit
        // stop/unload beneath its live SDK frame.
        if(fault==93) fault_hang(26);
        char formatted[32]{};
        if(value==0.25) std::strcpy(formatted,"unity");
        else if(value==0.5) std::strcpy(formatted,"double");
        else std::snprintf(formatted,sizeof(formatted),"%.3f x",value*4.0);
        std::memset(text,0,sizeof(String128));
        for(size_t n=0;n<std::strlen(formatted);++n) text[n]=static_cast<TChar>(formatted[n]);
        return kResultOk;
    }
    tresult PLUGIN_API getParamValueByString(ParamID id,TChar* text,ParamValue& value) override {
        if(fault!=91 && fault!=92 && fault!=94) return kNotImplemented;
        if(!control() || !context || id!=parameter_id(fault) || !text) return kInvalidArgument;
        if(fault==94) {value=std::numeric_limits<double>::quiet_NaN();return kResultOk;}
        char input[32]{};size_t n=0;
        for(;n<31 && text[n];++n) {if(text[n]>127) return kResultFalse;input[n]=static_cast<char>(text[n]);}
        if(text[n]) return kResultFalse;
        if(!std::strcmp(input,"unity")) value=0.25;
        else if(!std::strcmp(input,"double")) value=0.5;
        else {
            char* end=nullptr;const auto plain=std::strtod(input,&end);
            if(end==input || !end || std::strcmp(end," x") || !std::isfinite(plain) || plain<0 || plain>4) return kResultFalse;
            value=plain/4.0;
        }
        return kResultOk;
    }
    ParamValue PLUGIN_API normalizedParamToPlain(ParamID id,ParamValue value) override {
        if(fault!=91 && fault!=92) return value;
        if(!control() || !context || id!=parameter_id(fault)) {++observed.violations;return std::numeric_limits<double>::quiet_NaN();}
        return value*4.0;
    }
    ParamValue PLUGIN_API plainParamToNormalized(ParamID id,ParamValue value) override {
        if(fault!=91 && fault!=92) return value;
        if(!control() || !context || id!=parameter_id(fault)) {++observed.violations;return std::numeric_limits<double>::quiet_NaN();}
        return value/4.0;
    }
    tresult PLUGIN_API setParamNormalized(ParamID id,ParamValue value) override {
        if(!control() || !context || !handler || !std::isfinite(value) || value<0 || value>1)
            return kInvalidArgument;
        if((fault==86 || saturated_editor_drag(fault) || fault==132) && edits_sent) return kResultFalse;
        // The editor has already painted 0.875. The fixture also accepts a
        // deliberately later authored value, but rejects replay of 0.25.
        if(fault==135 && edits_sent && value!=0.875 && value!=0.5) return kResultFalse;
        if((saturated_editor_drag(fault) || mixed_parameter_input(fault)) && id==drag_axis_id) {drag_axis=value;return kResultOk;}
        if(id!=parameter_id(fault)) return kInvalidArgument;
        if(fault==76 && observed.flushes.load()) return kResultFalse;
        component_gain=static_cast<float>(parameter_value(fault,value)*4.0);
        if(fault==104 && observed.blocks) {
            const auto extra=value==0.25?kIoChanged:kParamValuesChanged;
            handler->restartComponent(kLatencyChanged|extra);
        }
        restoration.controller_gain=bits(component_gain);trace(21);return kResultOk;
    }
    IPlugView* PLUGIN_API createView(FIDString) override { return nullptr; }
};
class ProcessingProcessor final : public Processor,public IAudioPresentationLatency {
    const int fault=next_fault;
    const bool multiple_outputs=fault>=128 && fault<=130;
    const bool silent_input=fault==120 || fault==136;
    const bool midi_instrument=fault==115 || silent_input || fault==127 || multiple_outputs;
    const int mode=midi_instrument?1:next_mode;
    bool initialized=false, arranged=false, setup=false, active=false;
    std::atomic<bool> processing{false};
    bool input=false,output=false,thread_bound=false;
    bool auxiliary_disabled=false;
    unsigned inactive_outputs=0;
    bool event_active=false;
    float note_signal=0;
    struct Voice { int32 id=-1; float gain=0; };
    std::array<Voice,2> named_voices{};
    bool activation_restart_sent=false;
    bool activated_once=false;
    int32 inputs=2,outputs=2,maximum=0,precision=kSample32;
    std::atomic<int32> block{0};
    double sample_rate=0;
    int32 process_mode=kRealtime;
    std::atomic<float> gain{1};
    double manual_gain=1.0;
    uint32 input_latency=0,output_latency=0,restored_latency=0;
    std::array<std::array<double,2>,32> delay{};uint32 delay_cursor=0;
#if HEX_FIXTURE_RESIDUAL_ECHO
    std::array<std::array<double,2>,3> residual{};uint32 residual_cursor=0;
#endif
    IBStream* retained_stream=nullptr;
    pthread_t worker{};
    bool same_worker() const { return !thread_bound || pthread_equal(worker,pthread_self()); }
public:
    ProcessingProcessor() {
        if(fault==81) gain.store(4.0f/3.0f);
        if(fault==82) gain.store(0);
        ++observed.created;
    }
    ~ProcessingProcessor() {
        if (initialized || active || processing) ++observed.violations;
        if (retained_stream && fault!=16 && fault!=52) retained_stream->release();
    }
    uint32 PLUGIN_API addRef() override { return Processor::addRef(); }
    uint32 PLUGIN_API release() override { return Processor::release(); }
    tresult PLUGIN_API queryInterface(const TUID iid,void** out) override {
        if (!out || !iid) return kInvalidArgument;
        if ((fault==112 || fault==113) && same(iid,INLINE_UID_OF(IEditController))) {
            *out=nullptr;return fault==112?kResultFalse:kResultOk;
        }
        if (fault==114 && same(iid,INLINE_UID_OF(IAudioProcessor))) {
            *out=nullptr;return kResultFalse;
        }
        if (same(iid,INLINE_UID_OF(IAudioPresentationLatency))) {
            if (fault==112) { *out=nullptr;return kResultFalse; }
            *out=static_cast<IAudioPresentationLatency*>(this);addRef();return kResultOk;
        }
        return Processor::queryInterface(iid,out);
    }
    uint32 presentation_calls=0;
    uint32 PLUGIN_API getLatencySamples() override {
        if(!control()) return std::numeric_limits<uint32>::max();
        if(fault==66 && observed.stops) fault_hang(21);
        trace(presentation_calls?15:14); // actual facts phase versus applied presentation
        if(fault==68 && observed.stops && presentation_calls>2) return restored_latency+1;
        return fault==36 && presentation_calls?7:(fault>=37?restored_latency:0);
    }
    uint32 PLUGIN_API getTailSamples() override {
        if(!control()) return kInfiniteTail;
        const auto* value=std::getenv(presentation_calls?"HEX_ORIGINAL_TAIL_AFTER_PRESENTATION":"HEX_ORIGINAL_TAIL_FRAMES");
        if(observed.stops && std::getenv("HEX_ORIGINAL_TAIL_WHEN_QUIESCENT"))
            value=std::getenv("HEX_ORIGINAL_TAIL_WHEN_QUIESCENT");
        if(presentation_calls>2 && std::getenv("HEX_ORIGINAL_TAIL_AFTER_RESUME"))
            value=std::getenv("HEX_ORIGINAL_TAIL_AFTER_RESUME");
        if(!value) value=std::getenv("HEX_ORIGINAL_TAIL_FRAMES");
        if(!value) return HEX_FIXTURE_INFINITE_TAIL?kInfiniteTail:(HEX_FIXTURE_RESIDUAL_ECHO?3:kNoTail);
        char* end=nullptr;const auto frames=std::strtoull(value,&end,10);
        if(!*value || !end || *end || frames>std::numeric_limits<uint32>::max()) return kInfiniteTail;
        return static_cast<uint32>(frames);
    }
    tresult PLUGIN_API setAudioPresentationLatencySamples(BusDirection direction,int32 index,uint32 value) override {
        if (!control() || !active || processing || index || (direction!=kInput && direction!=kOutput) ||
            (direction==kInput && (!inputs || fault==120)) || fault==17) return kResultFalse;
        if(fault==67 && observed.stops) fault_hang(22);
        ++presentation_calls;
        (direction==kInput?input_latency:output_latency)=value;return kResultOk;
    }
    tresult PLUGIN_API getControllerClassId(TUID id) override {
        if(!control() || !initialized)return kResultFalse;
        if(fault<37 || fault==55)return kResultFalse;
        std::memcpy(id,restore_controller_id,16);return kResultOk;
    }
    tresult PLUGIN_API setState(IBStream* stream) override {
        if(!control() || !initialized || active || arranged || setup || processing || !stream)return kResultFalse;
        ++restoration.component_set;trace(16);
        if(fault==38)return kResultFalse;
        if(fault==39)throw std::runtime_error("original component state exception");
        if(fault==40)fault_crash(16);
        if(fault==41)fault_hang(16);
        std::array<unsigned char,32> data{};int32 read=0;
        if(stream->read(data.data(),data.size(),&read)!=kResultOk || read!=32 || std::memcmp(data.data(),"HXPS",4))return kResultFalse;
        if(fault==121) {
            int64 tail=0;unsigned char marker=0;
            if(stream->seek(-1,IBStream::kIBSeekEnd,&tail)!=kResultOk || tail!=150544 ||
               stream->read(&marker,1,&read)!=kResultOk || read!=1 || marker!=0x37) return kResultFalse;
        }
        float restored_gain=0;std::memcpy(&restored_gain,data.data()+8,4);std::memcpy(&restored_latency,data.data()+4,4);
        if(!std::isfinite(restored_gain) || restored_gain<0 || restored_gain>4 || restored_latency>32 || (fault<37 && restored_latency))return kResultFalse;
        gain.store(restored_gain);
        if(fault==118 && (!latency_handler ||
            latency_handler->beginEdit(51)!=kResultOk ||
            latency_handler->performEdit(51,0.5)!=kResultOk ||
            latency_handler->endEdit(51)!=kResultOk)) return kResultFalse;
        if(fault==50)stream->write(data.data(),1,nullptr);
        if(fault==51 || fault==52) {stream->addRef();retained_stream=stream;}
        restoration.component_gain=bits(restored_gain);restoration.latency=static_cast<int>(restored_latency);restoration.phase=1;
        trace(16);return kResultOk;
    }
    tresult PLUGIN_API getState(IBStream* stream) override {
        if (!control() || !active || !stream) return kResultFalse;
        ++restoration.component_get;
        if(fault==119 && restoration.component_set &&
            (!latency_handler || latency_handler->restartComponent(0)!=kResultOk)) return kResultFalse;
        // Original current-state capture fault controls leave startup valid.
        if(restoration.component_get>1) {
            if(fault==95 && restoration.component_get==2) {
                std::vector<unsigned char> huge(512*1024);return stream->write(huge.data(),static_cast<int32>(huge.size()),nullptr);
            }
            if(fault==97 && restoration.component_get==2) {
                std::vector<unsigned char> huge(512*1024);
                stream->write(huge.data(),static_cast<int32>(huge.size()),nullptr);
                stream->seek(-1,IBStream::kIBSeekSet,nullptr);return kResultOk;
            }
            if(fault==98 && restoration.component_get==2) {
                std::vector<unsigned char> huge(512*1024);
                stream->write(huge.data(),static_cast<int32>(huge.size()),nullptr);
                return kInternalError;
            }
            if(fault==69) return kResultFalse;
            if(fault==70) throw std::runtime_error("original SDK current capture exception");
            if(fault==71) fault_hang(22);
        }
        if (fault==35) arm_main_thread_tail();
        if (fault==34) fault_hang(12);
        if (fault==26) { trace(7);throw std::runtime_error("original SDK state exception"); }
        if (fault==15) { std::vector<unsigned char> huge(512*1024);return stream->write(huge.data(),static_cast<int32>(huge.size()),nullptr); }
        if (fault==23) {
            stream->seek(std::numeric_limits<int64>::min(),IBStream::kIBSeekCur,nullptr);
            stream->seek(std::numeric_limits<int64>::max(),IBStream::kIBSeekSet,nullptr);
            return kResultOk; // rejected stream operations must not masquerade as captured state
        }
        if (fault==24) stream->release(); // original invalid-reference sensitivity control
        if (fault==16) { stream->addRef();retained_stream=stream; }
        std::array<unsigned char,32> bytes{};std::memcpy(bytes.data(),"HXPS",4);
        const uint32 count=fault>=37?restored_latency:static_cast<uint32>(block);std::memcpy(bytes.data()+4,&count,4);
        const auto captured_gain=gain.load();
        std::memcpy(bytes.data()+8,&captured_gain,4);std::memcpy(bytes.data()+12,&sample_rate,8);
        bytes[20]=static_cast<unsigned char>(inputs);bytes[21]=static_cast<unsigned char>(outputs);
        std::memcpy(bytes.data()+24,&input_latency,4);std::memcpy(bytes.data()+28,&output_latency,4);
        ISizeableStream* sized=nullptr;
        if (stream->queryInterface(INLINE_UID_OF(ISizeableStream),reinterpret_cast<void**>(&sized))!=kResultOk || !sized)
            return kResultFalse;
        const bool sized_ok=sized->setStreamSize(32)==kResultOk;sized->release();
        int32 written=0;
        const bool ok=sized_ok && stream->seek(4,IBStream::kIBSeekSet,nullptr)==kResultOk &&
            stream->write(bytes.data()+4,28,&written)==kResultOk && written==28 &&
            stream->seek(0,IBStream::kIBSeekSet,nullptr)==kResultOk && stream->write(bytes.data(),4,&written)==kResultOk && written==4;
        if(ok && fault==121) {
            unsigned char marker=0x37;
            if(stream->seek(150544,IBStream::kIBSeekSet,nullptr)!=kResultOk ||
               stream->write(&marker,1,&written)!=kResultOk || written!=1) return kResultFalse;
        }
        trace(4);return ok?kResultOk:kResultFalse;
    }
    tresult PLUGIN_API initialize(FUnknown* host) override {
        if (!control() || initialized || !host) return kResultFalse;
        const auto result=Processor::initialize(host);
        if (result==kResultOk) {
            initialized=true; ++observed.initialized;
            if (fault==137) host->addRef(); // simulate a process-lifetime host reference
        }
        return result;
    }
    tresult PLUGIN_API terminate() override {
        if (!control() || !initialized || active || processing) return kResultFalse;
        const auto result=Processor::terminate();
        if (result==kResultOk) { initialized=false; ++observed.terminated;trace(5); }
        return result;
    }
    int32 PLUGIN_API getBusCount(MediaType media,BusDirection direction) override {
        if (!control() || !initialized) return 0;
        if (media==kEvent) return midi_instrument && direction==kInput?1:0;
        if (media!=kAudio) return 0;
        if (fault==5) return 2;
        if (fault==106 && direction==kInput) return 2;
        if (multiple_outputs && direction==kOutput) return 3;
        return direction==kInput && mode==1 && !silent_input?0:1;
    }
    tresult PLUGIN_API getBusInfo(MediaType media,BusDirection direction,int32 index,BusInfo& info) override {
        const bool auxiliary=fault==106 && media==kAudio && direction==kInput && index==1;
        const bool extra_output=multiple_outputs && media==kAudio && direction==kOutput && index>0 && index<3;
        if (!control() || (index && !auxiliary && !extra_output) || !getBusCount(media,direction))
            return kInvalidArgument;
        info={};info.mediaType=media;info.direction=direction;info.busType=kMain;
        info.channelCount=media==kEvent?4:(direction==kInput?inputs:outputs);
        if (auxiliary) { info.busType=kAux;info.channelCount=2; }
        if (fault==136 && media==kAudio && direction==kInput) info.busType=kAux;
        if (extra_output) {
            info.busType=index==1?kAux:kMain;
            info.channelCount=index==1 && fault!=130?1:2;
        }
        if (fault==12 && arranged) ++info.channelCount;
        info.flags=BusInfo::kDefaultActive;
        const char16_t name[]=u"main";std::memcpy(info.name,name,sizeof(name));
        return kResultOk;
    }
    tresult PLUGIN_API getBusArrangement(BusDirection direction,int32 index,SpeakerArrangement& value) override {
        if (control() && fault==106 && direction==kInput && index==1) {
            value=SpeakerArr::kStereo;return kResultOk;
        }
        if (control() && multiple_outputs && direction==kOutput && index>0 && index<3) {
            value=index==1?SpeakerArr::kMono:SpeakerArr::kStereo;return kResultOk;
        }
        if (!control() || index || !getBusCount(kAudio,direction)) return kInvalidArgument;
        value=(direction==kInput?inputs:outputs)==1?SpeakerArr::kMono:SpeakerArr::kStereo;
        return kResultOk;
    }
    tresult PLUGIN_API canProcessSampleSize(int32 size) override {
        if (!control() || !initialized || fault==6) return kResultFalse;
        return size==kSample32 || (mode==2 && size==kSample64)?kResultOk:kResultFalse;
    }
    tresult PLUGIN_API setBusArrangements(SpeakerArrangement* in,int32 ni,SpeakerArrangement* out,int32 no) override {
        if(silent_input) return kResultFalse; // this default is already configured
        if (!control() || !initialized || active || processing || ni!=(fault==106?2:(mode==1?0:1)) || no!=1 ||
            !out || (ni && !in)) return kInvalidArgument;
        if (fault==106 && in[1]!=SpeakerArr::kStereo) return kResultFalse;
        const auto count=[](SpeakerArrangement value)->int32 {
            return value==SpeakerArr::kMono?1:(value==SpeakerArr::kStereo?2:0);
        };
        const int32 requested_input=ni?count(in[0]):0,requested_output=count(out[0]);
        if (!requested_output || (ni && !requested_input)) return kResultFalse;
        ++observed.arrangements;
        inputs=requested_input;outputs=requested_output;arranged=true;
        if (fault==1 || fault==2) { inputs=1;outputs=1; }
        return fault==1?kResultFalse:kResultOk; // real refusal can mutate arrangement
    }
    tresult PLUGIN_API setupProcessing(ProcessSetup& request) override {
        if (!control() || (!arranged && !silent_input && !multiple_outputs) || active || processing ||
            (request.processMode!=kRealtime && request.processMode!=kOffline) ||
            (request.symbolicSampleSize!=kSample32 && !(mode==2 && request.symbolicSampleSize==kSample64)) ||
            request.maxSamplesPerBlock<1 || request.maxSamplesPerBlock>256 ||
            (request.sampleRate!=44100 && request.sampleRate!=48000 && request.sampleRate!=96000)) return kResultFalse;
        ++observed.setups;
        sample_rate=request.sampleRate;maximum=request.maxSamplesPerBlock;precision=request.symbolicSampleSize;
        process_mode=request.processMode;
        if (fault==3) { sample_rate=12345;return kResultFalse; }
        setup=true;return kResultOk;
    }
    tresult PLUGIN_API activateBus(MediaType media,BusDirection direction,int32 index,TBool enabled) override {
        if (control() && setup && !active && !processing && fault==106 && media==kAudio && direction==kInput && index==1) {
            if (enabled) return kResultFalse;
            auxiliary_disabled=true;return kResultOk;
        }
        if (control() && setup && !active && !processing && multiple_outputs &&
            media==kAudio && direction==kOutput && index>0 && index<3) {
            if (enabled) return kResultFalse;
            inactive_outputs|=1u<<index;
            return kResultOk;
        }
        if (!control() || !setup || active || processing || index || !getBusCount(media,direction)) return kResultFalse;
        if (media==kEvent) { event_active=enabled;return kResultOk; }
        ++observed.activations;
        if (enabled && fault==7) return kResultFalse;
        (direction==kInput?input:output)=enabled;return kResultOk;
    }
    tresult PLUGIN_API setActive(TBool enabled) override {
        if (!enabled && fault==25) trace(7); // sensitivity: teardown after unknown startup
        if (!control() || !setup || processing) return kResultFalse;
        if (enabled && fault==106 && !auxiliary_disabled) return kResultFalse;
        if (enabled && multiple_outputs && inactive_outputs!=6) return kResultFalse;
        if (enabled && midi_instrument && !event_active) return kResultFalse;
        if (enabled && fault==111) {
            if (activated_once) restored_latency=7;
            activated_once=true;
        }
        if(observed.stops) {
            if(fault>=100 && fault<=103 && !module_event(enabled?'A':'D')) return kResultFalse;
            if((fault==100 && !enabled) || (fault==101 && enabled)) { trace(24);return kResultFalse; }
            if((fault==102 && !enabled) || (fault==103 && enabled)) {
                trace(25);throw std::runtime_error("original SDK reactivation exception");
            }
            if((fault==99 || fault==104 || fault==105) && enabled && !active) {
                restored_latency=7;delay={};delay_cursor=0;
                if((fault==104 || fault==105) && !activation_restart_sent && latency_handler) {
                    activation_restart_sent=true;
                    latency_handler->restartComponent(kLatencyChanged);
                }
            }
        }
        active=enabled;return kResultOk;
    }
    tresult PLUGIN_API setProcessing(TBool enabled) override {
        if (!active || (enabled?processing.load():(!processing || !same_worker()))) return kResultFalse;
        if (pthread_main_np()) { ++observed.violations;return kResultFalse; }
        if (enabled && fault==18) return kResultFalse;
        if(!enabled) {
            if(fault==58) return kResultFalse;
            if(fault==59) throw std::runtime_error("original SDK quiescence exception");
            if(fault==60) fault_crash(20);
            if(fault==61) fault_hang(20);
        } else if(observed.starts) {
            if(fault==62) return kResultFalse;
            if(fault==63) throw std::runtime_error("original SDK resume exception");
            if(fault==64) fault_crash(23);
            if(fault==65) fault_hang(23);
        }
        if (enabled) { worker=pthread_self();thread_bound=true;++observed.starts; }
        else ++observed.stops;
        processing=enabled;
        if (enabled && fault==33) fault_hang(11);
        if (enabled && fault==25) throw std::runtime_error("original SDK startup exception");
        return fault==122?kNotImplemented:kResultOk;
    }
    tresult PLUGIN_API process(ProcessData& data) override {
        if (pthread_main_np() || !same_worker() || !active || !std::isfinite(sample_rate) || data.processMode!=process_mode ||
            data.symbolicSampleSize!=precision) { ++observed.violations;return kResultFalse; }
        if (!data.numSamples && !data.numInputs && !data.numOutputs && !data.inputs && !data.outputs) {
            if(saturated_editor_drag(fault) && data.inputParameterChanges) {
                const auto count=data.inputParameterChanges->getParameterCount();
                if(count<1 || count>2) return kInvalidArgument;
                for(int32 n=0;n<count;++n) {
                    auto* queue=data.inputParameterChanges->getParameterData(n);
                    int32 offset=-1;ParamValue value=-1;
                    if(!queue || queue->getPointCount()!=1 || queue->getPoint(0,offset,value)!=kResultOk ||
                        offset || !std::isfinite(value) || value<0 || value>1) return kInvalidArgument;
                    if(queue->getParameterId()==parameter_id(fault)) gain=static_cast<float>(value*4.0);
                    else if(queue->getParameterId()!=drag_axis_id) return kInvalidArgument;
                }
                ++observed.flushes;
                return kResultOk;
            }
            if ((processing && fault!=37 && fault!=86 && fault!=116 && fault!=132) ||
                !data.inputParameterChanges || data.inputParameterChanges->getParameterCount()!=1)
                return kInvalidArgument;
            auto* queue=data.inputParameterChanges->getParameterData(0);
            int32 offset=-1;ParamValue value=-1;
            if (!queue || queue->getParameterId()!=(fault>=37?parameter_id(fault):0x1234u) || queue->getPointCount()!=1 ||
                queue->getPoint(0,offset,value)!=kResultOk || offset || !std::isfinite(value) || value<0 || value>1)
                return kInvalidArgument;
            ++observed.flushes;
            if (fault==11 || fault==72) return kResultFalse;
            if(fault==73) throw std::runtime_error("original stopped parameter flush exception");
            if(fault==74) fault_hang(24);
            if(fault==75) data.numSamples=1;
            if(fault==77) data.inputParameterChanges->addRef();
            value=parameter_value(fault,value);
            gain=static_cast<float>(value*(fault>=37?4.0:1.0));
            if(fault>=37 && data.outputParameterChanges) {
                int32 index=-1;const ParamID id=parameter_id(fault);
                auto* feedback=data.outputParameterChanges->addParameterData(id,index);
                if(!feedback || feedback->addPoint(0,value,index)!=kResultOk) return kResultFalse;
            }
            return kResultOk;
        }
        if (!processing || data.numSamples<1 || data.numSamples>maximum ||
            (midi_instrument?(!event_active || !data.inputEvents):data.inputEvents!=nullptr) ||
            data.numInputs!=((mode==1 && !silent_input) || (!input && data.numInputs==0)?0:1) ||
            data.numOutputs!=(!output && data.numOutputs==0?0:(multiple_outputs?3:1))) return kInvalidArgument;
        if (data.numInputs && (!data.inputs || data.inputs[0].numChannels!=inputs ||
            (precision==kSample32?!data.inputs[0].channelBuffers32:!data.inputs[0].channelBuffers64)))
            return kInvalidArgument;
        if (silent_input) {
            if (!input || data.numInputs!=1 || data.inputs[0].silenceFlags!=3) return kInvalidArgument;
            for(int32 channel=0;channel<inputs;++channel) for(int32 frame=0;frame<data.numSamples;++frame)
                if(data.inputs[0].channelBuffers32[channel][frame]!=0) return kInvalidArgument;
        }
        if (data.numOutputs && (!data.outputs || data.outputs[0].numChannels!=outputs ||
            (precision==kSample32?!data.outputs[0].channelBuffers32:!data.outputs[0].channelBuffers64)))
            return kInvalidArgument;
        if (multiple_outputs) {
            const auto& mono=data.outputs[1];
            const auto& stereo=data.outputs[2];
            if (mono.numChannels!=1 || stereo.numChannels!=2 ||
                !mono.channelBuffers32 || !stereo.channelBuffers32 ||
                mono.channelBuffers32[0] || stereo.channelBuffers32[0] || stereo.channelBuffers32[1])
                return kInvalidArgument;
            if (fault==129) stereo.channelBuffers32[1]=data.outputs[0].channelBuffers32[0];
        }
        if(fault==126) {
            const auto* context=data.processContext;
            if(block==1) {
                constexpr auto expected=ProcessContext::kPlaying|ProcessContext::kProjectTimeMusicValid|
                    ProcessContext::kTempoValid;
                if(!context || context->state!=expected || context->sampleRate!=sample_rate ||
                    context->projectTimeSamples!=48000 || context->projectTimeMusic!=2.5 ||
                    context->tempo!=123.125) return kInvalidArgument;
            } else if(block==2) {
                if(!context || context->state || context->sampleRate!=sample_rate ||
                    context->projectTimeSamples!=0) return kInvalidArgument;
            } else if(block==3) {
                constexpr auto expected=ProcessContext::kPlaying|ProcessContext::kTimeSigValid|
                    ProcessContext::kCycleActive|ProcessContext::kCycleValid;
                if(!context || context->state!=expected || context->sampleRate!=sample_rate ||
                    context->projectTimeSamples!=30000 || context->timeSigNumerator!=7 ||
                    context->timeSigDenominator!=8 || context->cycleStartMusic!=1.25 ||
                    context->cycleEndMusic!=5.5) return kInvalidArgument;
            } else if(block==4) {
                if(!context || context->state!=ProcessContext::kTimeSigValid ||
                    context->timeSigNumerator!=3 || context->timeSigDenominator!=4)
                    return kInvalidArgument;
            } else if(block==5) {
                if(!context || context->state || context->cycleStartMusic || context->cycleEndMusic)
                    return kInvalidArgument;
            } else if(context) return kInvalidArgument;
        }
        // Independent SDK automation oracle. The previous block's final value
        // is the implicit point at -1; jumps require adjacent old/new points.
        struct Curve {
            std::array<std::pair<int32,ParamValue>,64> points{};
            int32 count=0,next=0,left_offset=-1;
            double left_value=0.0;
            bool read(IParamValueQueue& queue,int32 frames) {
                count=queue.getPointCount();
                if(count<1 || count>static_cast<int32>(points.size())) return false;
                for(int32 n=0;n<count;++n) {
                    auto& point=points[static_cast<size_t>(n)];
                    if(queue.getPoint(n,point.first,point.second)!=kResultOk || point.first<0 ||
                        point.first>=frames || !std::isfinite(point.second) || point.second<0 || point.second>1 ||
                        (n && points[static_cast<size_t>(n-1)].first>=point.first)) return false;
                }
                return true;
            }
            double at(int32 frame) {
                while(next<count && points[static_cast<size_t>(next)].first<frame) {
                    const auto& point=points[static_cast<size_t>(next++)];
                    left_offset=point.first;left_value=point.second;
                }
                if(next==count) return left_value;
                const auto& right=points[static_cast<size_t>(next)];
                return left_value+(right.second-left_value)*static_cast<double>(frame-left_offset)/
                    static_cast<double>(right.first-left_offset);
            }
        } primary,secondary;
        primary.left_value=static_cast<double>(gain.load())/4.0;
        secondary.left_value=manual_gain/4.0;
        if(data.inputParameterChanges) {
            const auto count=data.inputParameterChanges->getParameterCount();
            if(count<1 || count>(mixed_parameter_input(fault)?2:1)) return kInvalidArgument;
            for(int32 index=0;index<count;++index) {
                auto* queue=data.inputParameterChanges->getParameterData(index);
                if(!queue) return kInvalidArgument;
                if(mixed_parameter_input(fault) && queue->getParameterId()==drag_axis_id) {
                    if(!secondary.read(*queue,data.numSamples) ||
                        (fault==mixed_input_fixture &&
                            (secondary.count!=1 || secondary.points[0].first!=0))) return kInvalidArgument;
                } else if(queue->getParameterId()!=parameter_id(fault) ||
                    !primary.read(*queue,data.numSamples)) return kInvalidArgument;
            }
        }
        ++observed.blocks; // count attempted foreign DSP, including a refusal
        if (fault==140) fault_hang(27); // controlled in-flight host-input barrier
        if (fault==28) fault_hang(9);
        if (fault==31) fault_crash(9);
        if (fault==10) return kResultFalse;
        const float scale=(fault==127 || fault==held_input_fixture)?1.0f:0.5f+static_cast<float>(block)*0.125f;
        const auto point_count=primary.count;
        double current_gain=gain.load();
        bool silent=true;
        const auto event_count=data.inputEvents?data.inputEvents->getEventCount():0;
        int32 next_event=0;
        Event event{};
        if (event_count && data.inputEvents->getEvent(0,event)!=kResultOk) return kResultFalse;
        for (int32 frame=0;frame<data.numSamples;++frame) {
            while (next_event<event_count && event.sampleOffset==frame) {
                if (event.busIndex!=0) return kResultFalse;
                const bool on=event.type==Event::kNoteOnEvent;
                if ((!on && event.type!=Event::kNoteOffEvent) ||
                    (on?event.noteOn.channel:event.noteOff.channel)!=(fault==127?0:2) ||
                    (on?event.noteOn.pitch:event.noteOff.pitch)!=60) return kResultFalse;
                const auto id=on?event.noteOn.noteId:event.noteOff.noteId;
                if(id==-1) note_signal=on?event.noteOn.velocity:0;
                else {
                    const auto existing=std::find_if(named_voices.begin(),named_voices.end(),
                        [id](const Voice& voice) {return voice.id==id;});
                    if(on) {
                        if(existing!=named_voices.end()) return kResultFalse;
                        const auto free=std::find_if(named_voices.begin(),named_voices.end(),
                            [](const Voice& voice) {return voice.id==-1;});
                        if(free==named_voices.end()) return kResultFalse;
                        *free={id,event.noteOn.velocity};
                    } else {
                        if(existing==named_voices.end()) return kResultFalse;
                        *existing={};
                    }
                }
                if (++next_event<event_count && data.inputEvents->getEvent(next_event,event)!=kResultOk)
                    return kResultFalse;
            }
            if(point_count) current_gain=parameter_value(fault,primary.at(frame))*4.0;
            if(secondary.count) manual_gain=secondary.at(frame)*4.0;
            // Independent internal change at offset0; later host automation
            // still reaches its endpoint at the end of this actual SDK block.
            if(fault==79 && point_count && frame==0) current_gain=1.0;
            if(fault==83 || fault==84) current_gain=2.0;
            // Read all inputs for this sample before writing any aliased output.
            const double note_output=note_signal+named_voices[0].gain+named_voices[1].gain;
            const double left=mode==1?(midi_instrument?note_output:0.25):(input?(precision==kSample32?data.inputs[0].channelBuffers32[0][frame]:data.inputs[0].channelBuffers64[0][frame]):0.0);
            const double right=mode==1?left:(input && inputs==2?(precision==kSample32?data.inputs[0].channelBuffers32[1][frame]:data.inputs[0].channelBuffers64[1][frame]):left);
            if (output) for (int32 channel=0;channel<outputs;++channel) {
                double value=(channel?right:left)*(fault==82?(current_gain>=2.0?1.0:scale):scale*current_gain)*manual_gain;
#if HEX_FIXTURE_RESIDUAL_ECHO
                // Original three-frame FIR echo. Store the immediate sample,
                // not the summed result: zero input has a genuinely finite tail.
                const auto immediate=value;
                value+=residual[residual_cursor][channel]*0.25;
                residual[residual_cursor][channel]=immediate;
#endif
                if(fault>=37 && restored_latency) {
                    std::swap(value,delay[delay_cursor][channel]);
                }
                if (precision==kSample32) {
                    auto* destination=data.outputs[0].channelBuffers32[channel];if (!destination) return kInvalidArgument;
                    destination[frame]=fault==4?std::numeric_limits<float>::quiet_NaN():static_cast<float>(value);
                } else {
                    auto* destination=data.outputs[0].channelBuffers64[channel];if (!destination) return kInvalidArgument;
                    destination[frame]=fault==4?std::numeric_limits<double>::quiet_NaN():value;
                }
                silent=silent && value==0;
            }
            if(fault>=37 && restored_latency)delay_cursor=(delay_cursor+1)%restored_latency;
#if HEX_FIXTURE_RESIDUAL_ECHO
            residual_cursor=(residual_cursor+1)%residual.size();
#endif
        }
        if((fault==124 || fault==125) && data.outputParameterChanges) {
            int32 index=-1;
            auto* queue=data.outputParameterChanges->addParameterData(parameter_id(fault),index);
            if(!queue || queue->addPoint(0,-0.444444417953,index)!=kResultFalse ||
                queue->addPoint(1,0.25,index)!=kResultOk ||
                queue->addPoint(1,std::numeric_limits<double>::quiet_NaN(),index)!=kResultFalse)
                return kResultFalse;
            if(fault==125) queue->addRef();
        }
        if(point_count || fault==83 || fault==84) {
            gain.store(static_cast<float>(current_gain));
            if(data.outputParameterChanges && fault!=78) {
                int32 index=-1;const ParamID id=parameter_id(fault);
                auto* queue=data.outputParameterChanges->addParameterData(id,index);
                if(!queue || queue->addPoint(fault==79 || fault==83 || fault==84?0:data.numSamples-1,fault==79?0.25:current_gain/4.0,index)!=kResultOk) return kResultFalse;
            }
        }
        if (output) data.outputs[0].silenceFlags=silent || fault==9?((uint64(1)<<outputs)-1):0;
        if (fault==8 && output) data.outputs[0].numChannels=3;
        if (fault==13) data.numSamples=8192;
        ++block;return kResultOk;
    }
};

class ProcessingFactory final : public IPluginFactory {
    std::atomic<uint32> refs{0};
public:
    uint32 count() const { return refs.load(); }
    tresult PLUGIN_API queryInterface(const TUID iid,void** out) override {
        if (!out) return kInvalidArgument;*out=nullptr;
        if (!same(iid,INLINE_UID_OF(FUnknown)) && !same(iid,INLINE_UID_OF(IPluginFactory))) return kNoInterface;
        *out=static_cast<IPluginFactory*>(this);addRef();return kResultOk;
    }
    uint32 PLUGIN_API addRef() override { return ++refs; }
    uint32 PLUGIN_API release() override { return --refs; }
    tresult PLUGIN_API getFactoryInfo(PFactoryInfo* out) override {
        if (!out) return kInvalidArgument;
        *out=PFactoryInfo("hex original fixtures","https://invalid.example","fixture@invalid.example",0);return kResultOk;
    }
    int32 PLUGIN_API countClasses() override {
        if (next_fault==138 || next_fault==139) return 3;
        return next_fault>=37 && next_fault!=55?2:1;
    }
    tresult PLUGIN_API getClassInfo(int32 index,PClassInfo* out) override {
        if (!out || index<0 || index>=countClasses()) return kInvalidArgument;
        if (index==2) {
            const auto category=next_fault==138?kPluginCompatibilityClass:kVstAudioEffectClass;
            *out=PClassInfo(processing_id,PClassInfo::kManyInstances,category,"hex original shared id");
            return kResultOk;
        }
        if(index==1) {*out=PClassInfo(restore_controller_id,PClassInfo::kManyInstances,"Component Controller Class","hex original restore controller");return kResultOk;}
        *out=PClassInfo(processing_id,PClassInfo::kManyInstances,"Audio Module Class","hex original processing fixture");return kResultOk;
    }
    tresult PLUGIN_API createInstance(FIDString cid,FIDString iid,void** out) override {
        if (!out) return kInvalidArgument;*out=nullptr;
        if (!cid || !iid) return kInvalidArgument;
        if(next_fault>=37 && next_fault!=55 && same(cid,restore_controller_id)) {
            auto* object=new(std::nothrow) RestoreController();if(!object)return kOutOfMemory;
            const auto result=object->queryInterface(iid,out);object->release();return result;
        }
        if(!same(cid,processing_id))return kInvalidArgument;
        auto* object=new(std::nothrow) ProcessingProcessor();if (!object) return kOutOfMemory;
        const auto result=object->queryInterface(iid,out);object->release();return result;
    }
} processing_factory;
} // namespace

extern "C" __attribute__((visibility("default"))) bool HexProcessingConfigure(int mode,int fault) {
    if (!control() || mode<0 || mode>2 || fault<0 || fault>13) return false;
    next_mode=mode;next_fault=fault;return true;
}
extern "C" __attribute__((visibility("default"))) int HexProcessingObservation(int* out,int capacity) {
    if (!control() || !out || capacity<13) return -1;
    const int values[]={observed.violations,observed.created,observed.initialized,observed.terminated,
        observed.arrangements,observed.setups,observed.activations,observed.starts,observed.stops,
        observed.blocks,observed.flushes,static_cast<int>(objects.load()),static_cast<int>(processing_factory.count())};
    std::copy(std::begin(values),std::end(values),out);return 13;
}
extern "C" __attribute__((visibility("default"))) bool bundleEntry(CFBundleRef) {
    // Supported fault control belongs only to this original SDK fixture.
    // The production helper never reads this environment variable.
    const auto* selected=std::getenv("HEX_ORIGINAL_INSTANCE_VARIANT");
    if (selected) {
        const auto length=std::strlen(selected);if (!length || length>3) return false;
        int value=0;for (size_t n=0;n<length;++n) { if (selected[n]<'0' || selected[n]>'9') return false;value=value*10+selected[n]-'0'; }
        if (value>held_input_fixture) return false;next_fault=value;
    }
    if(!module_event('E')) return false;
    trace(1,0);
    if (next_fault==27) fault_hang(8);
    if (next_fault==30) fault_crash(8);
    if (next_fault==21) for (int n=0;n<4096;++n) { std::fputs("owned SDK stdout noise\n",stdout);std::fputs("owned SDK stderr noise\n",stderr); }
    if (next_fault==22 && std::fgetc(stdin)!=EOF) return false;
    return next_fault!=19;
}
extern "C" __attribute__((visibility("default"))) bool bundleExit() {
    if(!module_event('X')) return false;
    trace(6,static_cast<int>(processing_factory.count()));
    if (next_fault==29) fault_hang(10);
    if (next_fault==32) fault_crash(10);
    if (next_fault==134) {
        if(!controller_release_observed) return false;
        static constexpr char released='T';
        if(!defer_module_event(released)) return false;
    }
    return next_fault!=20 && objects.load()==0 && processing_factory.count()==0;
}
extern "C" __attribute__((visibility("default"))) IPluginFactory* GetPluginFactory() {
    processing_factory.addRef();return &processing_factory;
}
