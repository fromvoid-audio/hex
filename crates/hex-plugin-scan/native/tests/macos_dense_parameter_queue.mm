// Original fixed-capacity queue exercised directly through the SDK interfaces.
#include "macos_parameters.hpp"
#include <cassert>
#include <limits>

int main() {
    using namespace hex_instance;
    ParameterChanges input;
    constexpr int32 frames=static_cast<int32>(DENSE_PARAMETER_SAMPLE_COUNT);
    constexpr ParamID dense=512;
    std::array<ParamValue,DENSE_PARAMETER_SAMPLE_COUNT> normalized{};
    for(size_t frame=0;frame<normalized.size();++frame)
        normalized[frame]=frame%2?.125:.75;

    assert(input.reset(frames,false));
    for(ParamID id=0;id<PARAMETER_POINT_COUNT;++id) assert(input.input(id,17,.25));
    assert(!input.input_dense(0,normalized.data(),normalized.size()));
    assert(!input.input_dense(dense,normalized.data(),normalized.size()-1));
    const auto last=normalized.back();
    normalized.back()=std::numeric_limits<ParamValue>::quiet_NaN();
    assert(!input.input_dense(dense,normalized.data(),normalized.size()));
    normalized.back()=last;
    assert(input.size()==PARAMETER_POINT_COUNT);
    assert(input.input_dense(dense,normalized.data(),normalized.size()));
    assert(!input.input_dense(dense+1,normalized.data(),normalized.size()));
    assert(input.begin_sdk());
    assert(input.getParameterCount()==static_cast<int32>(PARAMETER_POINT_COUNT+1));
    auto* sparse=input.getParameterData(0);
    auto* values=input.getParameterData(static_cast<int32>(PARAMETER_POINT_COUNT));
    assert(sparse && sparse->getParameterId()==0 && sparse->getPointCount()==1);
    assert(values && values->getParameterId()==dense && values->getPointCount()==frames);
    int32 offset=0;
    ParamValue value=0;
    assert(sparse->getPoint(0,offset,value)==kResultOk && offset==17 && value==.25);
    for(int32 frame=0;frame<frames;++frame) {
        assert(values->getPoint(frame,offset,value)==kResultOk);
        assert(offset==frame && value==normalized[static_cast<size_t>(frame)]);
    }
    assert(values->getPoint(frames,offset,value)==kInvalidArgument);
    assert(input.end_sdk());
    assert(input.erase_input(dense));
    assert(input.size()==PARAMETER_POINT_COUNT);
    assert(input.reset(0,false));
    assert(!input.input_dense(dense,normalized.data(),1));
    assert(input.input(dense,0,.125));

    ParameterChanges output;
    assert(output.reset(frames,true));
    assert(!output.input_dense(dense,normalized.data(),normalized.size()));
    assert(output.begin_sdk());
    for(ParamID id=0;id<PARAMETER_POINT_COUNT;++id) {
        int32 index=0;
        auto* queue=output.addParameterData(id,index);
        assert(queue && queue->addPoint(0,.5,index)==kResultOk);
    }
    int32 index=0;
    assert(!output.addParameterData(dense,index));
    assert(!output.end_sdk());
    assert(output.size()==PARAMETER_POINT_COUNT);
}
