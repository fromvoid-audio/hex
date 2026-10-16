// Original bounded controlled offline wire/state adapters. Pinned SDK MIT notices retained.
#pragma once
#include "pluginterfaces/base/ibstream.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstprocesscontext.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivsthostapplication.h"
#include "pluginterfaces/vst/ivstmessage.h"
#include "pluginterfaces/vst/ivstattributes.h"
#include "pluginterfaces/vst/ivstparameterchanges.h"
#include <atomic>
#include <algorithm>
#include <array>
#include <cmath>
#include <cstring>
#include <cstdint>
#include <stdexcept>
#include <string>
#include <vector>
using namespace Steinberg;
using namespace Steinberg::Vst;
namespace Controlled  {
  inline constexpr uint32 gain_id=0x210001;
  inline constexpr TUID component_id= {
    0x21,0x02,0x10,0x02,3,4,5,6,7,8,9,0x10,0x11,0x12,0x13,0x14
  };
  inline constexpr TUID controller_id= {
    0x21,0x03,0x10,0x02,3,4,5,6,7,8,9,0x10,0x11,0x12,0x13,0x14
  };
  inline bool equal(const char* a,const char* b) {
    return std::memcmp(a,b,16)==0;
  }
  inline bool norm(double v) {
    return std::isfinite(v)&&v>=0&&v<=1;
  }
  inline void check(bool v,const char* reason) {
    if(!v)throw std::runtime_error(reason);
  }
  inline void ok(tresult v,const char* reason) {
    check(v==kResultOk,reason);
  }
  inline void utf16(String128 out,const char* s) {
    std::fill(out,out+128,0);
    for(size_t i=0;s[i]&&i<127;i++)out[i]=static_cast<TChar>(s[i]);
  }
  inline std::string utf8(const String128 text) {
    std::string s;
    for(size_t i=0;i<128;++i) {
      uint32 c=uint16_t(text[i]);
      if(!c)return s;
      if(c>=0xd800&&c<=0xdbff) {
        check(i+1<128,"UTF16 truncation");
        uint32 low=uint16_t(text[++i]);
        check(low>=0xdc00&&low<=0xdfff,"UTF16 surrogate");
        c=0x10000+((c-0xd800)<<10)+(low-0xdc00);
      }else check(c<0xdc00||c>0xdfff,"UTF16 surrogate");
      if(c<0x80)s+=char(c);
      else if(c<0x800) {
        s+=char(0xc0|(c>>6));
        s+=char(0x80|(c&63));
      }else if(c<0x10000) {
        s+=char(0xe0|(c>>12));
        s+=char(0x80|((c>>6)&63));
        s+=char(0x80|(c&63));
      }else {
        s+=char(0xf0|(c>>18));
        s+=char(0x80|((c>>12)&63));
        s+=char(0x80|((c>>6)&63));
        s+=char(0x80|(c&63));
      }
    }throw std::runtime_error("unterminated UTF16");
  }
  struct Writer  {
    std::vector<uint8_t> b;
    void u(uint64_t v,unsigned n) {
      check(b.size()+n<=1024*1024,"writer capacity");
      for(unsigned i=0;i<n;++i)b.push_back(uint8_t(v>>(8*i)));
    }
    void raw(const void* p,size_t n) {
      check(b.size()+n<=1024*1024,"writer capacity");
      auto q=static_cast<const uint8_t*>(p);
      if(n)b.insert(b.end(),q,q+n);
    }
    void blob(const std::vector<uint8_t>& a) {
      u(a.size(),4);
      raw(a.data(),a.size());
    }
    void text(const std::string& s) {
      check(s.size()<=512,"text capacity");
      u(s.size(),4);
      raw(s.data(),s.size());
    }
    void f64(double v) {
      uint64_t bits;
      std::memcpy(&bits,&v,8);
      u(bits,8);
    }
    void f32(float v) {
      uint32 bits;
      std::memcpy(&bits,&v,4);
      u(bits,4);
    }
  };
  struct Reader  {
    const std::vector<uint8_t>& b;
    size_t at=0;
    uint64_t u(unsigned n) {
      check(n<=8&&at<=b.size()&&n<=b.size()-at,"truncated wire");
      uint64_t v=0;
      for(unsigned i=0;i<n;i++)v|=uint64_t(b[at++])<<(8*i);
      return v;
    }
    std::vector<uint8_t> bytes(size_t n) {
      check(at<=b.size()&&n<=b.size()-at,"truncated wire");
      auto a=std::vector<uint8_t>(b.begin()+at,b.begin()+at+n);
      at+=n;
      return a;
    }
    std::vector<uint8_t> blob(size_t cap) {
      auto n=u(4);
      check(n<=cap,"blob capacity");
      return bytes(size_t(n));
    }
    double f64() {
      uint64_t bits=u(8);
      double v;
      std::memcpy(&v,&bits,8);
      return v;
    }
    float f32() {
      uint32 bits=uint32(u(4));
      float v;
      std::memcpy(&v,&bits,4);
      return v;
    }
  };
  // Stack-owned adapters retain their owner until plugin termination, and verify no
  // external references remain. Plugin objects have separate deleting refcounts.
#define STACK_REFS uint32 refs=1; uint32 PLUGIN_API addRef() override{return ++refs;} uint32 PLUGIN_API release() override{return --refs;}
  class Stream final:public IBStream  {
    public:
    STACK_REFS
    std::vector<uint8_t> bytes;
    size_t at=0;
    bool fault=false;
    tresult PLUGIN_API queryInterface(const TUID iid,void** o)override {
      *o=nullptr;
      if(equal(iid,INLINE_UID_OF(FUnknown))||equal(iid,INLINE_UID_OF(IBStream))) {
        *o=this;
        addRef();
        return kResultOk;
      }return kNoInterface;
    }
    tresult PLUGIN_API read(void* p,int32 n,int32* done)override {
      if(done)*done=0;
      if(n<0||(!p&&n)||at>bytes.size()) {
        fault=true;
        return kInvalidArgument;
      }size_t count=std::min(size_t(n),bytes.size()-at);
      if(count)std::memcpy(p,bytes.data()+at,count);
      at+=count;
      if(done)*done=int32(count);
      return kResultOk;
    }
    tresult PLUGIN_API write(void* p,int32 n,int32* done)override {
      if(done)*done=0;
      if(n<0||(!p&&n)||at>65536||size_t(n)>65536-at) {
        fault=true;
        return kInvalidArgument;
      }bytes.resize(std::max(bytes.size(),at+size_t(n)));
      if(n)std::memcpy(bytes.data()+at,p,size_t(n));
      at+=size_t(n);
      if(done)*done=n;
      return kResultOk;
    }
    tresult PLUGIN_API seek(int64 pos,int32 mode,int64* result)override {
      int64 base=mode==kIBSeekSet?0:mode==kIBSeekCur?int64(at):mode==kIBSeekEnd?int64(bytes.size()):-1;
      if(base<0||pos < -base||pos>65536-base) {
        fault=true;
        return kInvalidArgument;
      }at=size_t(base+pos);
      if(result)*result=int64(at);
      return kResultOk;
    }
    tresult PLUGIN_API tell(int64* p)override {
      if(!p)return kInvalidArgument;
      *p=int64(at);
      return kResultOk;
    }
  };
  struct Point  {
    int32 offset;
    double value;
  };
  class Queue final:public IParamValueQueue  {
    public:
    STACK_REFS
    ParamID id=gain_id;
    std::vector<Point> points;
    tresult PLUGIN_API queryInterface(const TUID iid,void** o)override {
      *o=nullptr;
      if(equal(iid,INLINE_UID_OF(FUnknown))||equal(iid,INLINE_UID_OF(IParamValueQueue))) {
        *o=this;
        addRef();
        return kResultOk;
      }return kNoInterface;
    }
    ParamID PLUGIN_API getParameterId()override {
      return id;
    }
    int32 PLUGIN_API getPointCount()override {
      return int32(points.size());
    }
    tresult PLUGIN_API getPoint(int32 i,int32& offset,double& value)override {
      if(i<0||size_t(i)>=points.size())return kInvalidArgument;
      offset=points[size_t(i)].offset;
      value=points[size_t(i)].value;
      return kResultOk;
    }
    tresult PLUGIN_API addPoint(int32,double,int32&)override {
      return kNotImplemented;
    }
  };
  class Changes final:public IParameterChanges  {
    public:
    STACK_REFS
    Queue queue;
    tresult PLUGIN_API queryInterface(const TUID iid,void** o)override {
      *o=nullptr;
      if(equal(iid,INLINE_UID_OF(FUnknown))||equal(iid,INLINE_UID_OF(IParameterChanges))) {
        *o=this;
        addRef();
        return kResultOk;
      }return kNoInterface;
    }
    int32 PLUGIN_API getParameterCount()override {
      return queue.points.empty()?0:1;
    }
    IParamValueQueue* PLUGIN_API getParameterData(int32 i)override {
      return i==0&&!queue.points.empty()?&queue:nullptr;
    }
    IParamValueQueue* PLUGIN_API addParameterData(const ParamID&,int32&)override {
      return nullptr;
    }
  };
}
