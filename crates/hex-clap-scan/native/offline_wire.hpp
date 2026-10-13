// ORIGINAL bounded wire and partial CLAP stream adapters, controlled fixtures only.
#pragma once
#include <clap/clap.h>
#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstring>
#include <stdexcept>
#include <string>
#include <vector>
#include <array>
#include <variant>
namespace Controlled   {
  constexpr const char* plugin_id="org.hex.controlled.offline-gain";
  constexpr uint32_t gain_id=0x200001;
  inline void check(bool v,const char* why)   {
    if(!v)throw std::runtime_error(why);
  }
  inline bool plain(double v)  {
    return std::isfinite(v)&&v>=0&&v<=2;
  }
  struct Writer   {
    std::vector<uint8_t> b;
    void u(uint64_t v,unsigned n)  {
      check(b.size()+n<=1024*1024,"wire capacity");
      for(unsigned i=0;i<n;i++)b.push_back(uint8_t(v>>(8*i)));
    }
    void raw(const void* p,size_t n)  {
      check(b.size()+n<=1024*1024,"wire capacity");
      auto q=static_cast<const uint8_t*>(p);
      if(n)b.insert(b.end(),q,q+n);
    }
    void blob(const std::vector<uint8_t>& v)  {
      u(v.size(),4);
      raw(v.data(),v.size());
    }
    void text(const std::string& v)  {
      check(v.size()<=512,"text capacity");
      u(v.size(),4);
      raw(v.data(),v.size());
    }
    void f64(double v)  {
      uint64_t b;
      std::memcpy(&b,&v,8);
      u(b,8);
    }
    void f32(float v)  {
      uint32_t b;
      std::memcpy(&b,&v,4);
      u(b,4);
    }
  };
  struct Reader   {
    const std::vector<uint8_t>& b;
    size_t at=0;
    uint64_t u(unsigned n)  {
      check(n<=8&&at<=b.size()&&n<=b.size()-at,"wire truncation");
      uint64_t v=0;
      for(unsigned i=0;i<n;i++)v|=uint64_t(b[at++])<<(8*i);
      return v;
    }
    std::vector<uint8_t> bytes(size_t n)  {
      check(at<=b.size()&&n<=b.size()-at,"wire truncation");
      auto v=std::vector<uint8_t>(b.begin()+at,b.begin()+at+n);
      at+=n;
      return v;
    }
    std::vector<uint8_t> blob(size_t cap)  {
      auto n=u(4);
      check(n<=cap,"blob capacity");
      return bytes(n);
    }
    double f64()  {
      auto b=u(8);
      double v;
      std::memcpy(&v,&b,8);
      return v;
    }
    float f32()  {
      uint32_t b=uint32_t(u(4));
      float v;
      std::memcpy(&v,&b,4);
      return v;
    }
  };
  struct Stream   {
    std::vector<uint8_t> bytes;
    size_t at=0;
    bool fault=false;
    clap_istream_t in  {
      this,read
    };
    clap_ostream_t out  {
      this,write
    };
    static int64_t read(const clap_istream_t* s,void* p,uint64_t n)  {
      auto& x=*static_cast<Stream*>(s->ctx);
      if((!p&&n)||x.at>x.bytes.size())  {
        x.fault=true;
        return -1;
      }auto count=std::min(  {
        size_t(n),size_t(7),x.bytes.size()-x.at
      });
      if(count)std::memcpy(p,x.bytes.data()+x.at,count);
      x.at+=count;
      return int64_t(count);
    }
    static int64_t write(const clap_ostream_t* s,const void* p,uint64_t n)  {
      auto& x=*static_cast<Stream*>(s->ctx);
      if((!p&&n)||n>65536||x.bytes.size()>65536)  {
        x.fault=true;
        return -1;
      }auto count=std::min(n,uint64_t(7));
      if(count>65536-x.bytes.size())  {
        x.fault=true;
        return -1;
      }auto b=static_cast<const uint8_t*>(p);
      if(count)x.bytes.insert(x.bytes.end(),b,b+count);
      return int64_t(count);
    }
  };
  struct Events   {
    std::vector<clap_event_param_value_t> values;
    clap_input_events_t in  {
      this,size,get
    };
    static uint32_t size(const clap_input_events_t* p)  {
      return uint32_t(static_cast<Events*>(p->ctx)->values.size());
    }
    static const clap_event_header_t* get(const clap_input_events_t* p,uint32_t i)  {
      auto& v=static_cast<Events*>(p->ctx)->values;
      return i<v.size()?&v[i].header:nullptr;
    }
    bool rejected=false;
    clap_output_events_t out  {
      this,push
    };
    static bool push(const clap_output_events_t* p,const clap_event_header_t*)  {
      static_cast<Events*>(p->ctx)->rejected=true;
      return false;
    }
  };
  inline thread_local bool audio_thread=false;
  constexpr const char* midi_plugin_id="org.hex.controlled.offline-midi-gain";
  constexpr const char* midi_profile="midi1-cc7-gain-v1";
  constexpr uint32_t midi_port_id=0x200101;
  struct OwnedOutput  {
    uint32_t block,offset,flags,parameter;
    uint64_t frame;
    double value;
  };
  using InputSlot=std::variant<clap_event_param_value_t,clap_event_midi_t>;
  static_assert(sizeof(clap_event_midi_t)==24&&sizeof(clap_event_param_value_t)==56&&sizeof(InputSlot)==64&&sizeof(OwnedOutput)==32,"review pinned Linux x86_64 ABI before supporting other layout");
  struct OutputEvents  {
    std::array<OwnedOutput,1024> values {
    };
    size_t count=0;
    uint32_t block=0,frames=0,block_count=0,prior=0;
    uint64_t frame=0;
    std::atomic<bool> rejected{false};
    bool active=false;
    clap_output_events_t out {
      this,push
    };
    void begin(uint32_t b,uint32_t n,uint64_t f) {
      block=b;
      frames=n;
      frame=f;
      block_count=0;
      prior=0;
      active=true;
    }
    void finish() {
      active=false;
    }
    static bool active_audio(const OutputEvents& x) {
      return audio_thread&&x.active;
    }
    static bool push(const clap_output_events_t* list,const clap_event_header_t* h) {
      auto& x=*static_cast<OutputEvents*>(list->ctx);
      if(x.rejected.load())return false;
      if(!active_audio(x)) {
        x.rejected=true;
        return false;
      }
      if(!h||h->size!=sizeof(clap_event_param_value_t)||h->space_id!=CLAP_CORE_EVENT_SPACE_ID||h->type!=CLAP_EVENT_PARAM_VALUE||h->flags!=CLAP_EVENT_DONT_RECORD||h->time>=x.frames||x.count>=x.values.size()||x.block_count>=128||(x.block_count&&h->time<x.prior)) {
        x.rejected=true;
        return false;
      }
      auto* p=reinterpret_cast<const clap_event_param_value_t*>(h);
      if(p->param_id!=gain_id||p->note_id!=-1||p->port_index!=-1||p->channel!=-1||p->key!=-1||!plain(p->value)) {
        x.rejected=true;
        return false;
      }
      // Copy only owned scalar fields synchronously. cookie is deliberately never retained.
      x.values[x.count++]= {
        x.block,h->time,h->flags,p->param_id,x.frame+h->time,p->value
      };
      ++x.block_count;
      x.prior=h->time;
      return true;
    }
  };
  struct MidiEvents  {
    std::array<InputSlot,128> values {
    };
    uint32_t count=0;
    clap_input_events_t in {
      this,size,get
    };
    static uint32_t size(const clap_input_events_t* p) {
      return static_cast<MidiEvents*>(p->ctx)->count;
    }
    static const clap_event_header_t* get(const clap_input_events_t* p,uint32_t index) {
      auto& x=*static_cast<MidiEvents*>(p->ctx);
      if(index>=x.count)return nullptr;
      return std::visit([](const auto& e) {
        return &e.header;
      },x.values[index]);
    }
  };
}
