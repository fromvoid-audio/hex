// ORIGINAL fixed FIFO identity/state, not a production graph contract.
#pragma once
#include "offline_wire.hpp"
namespace Controlled  {
  constexpr const char* delay_plugin_id="org.hex.controlled.offline-finite-delay";
  constexpr const char* delay_profile="finite-delay-gain-v1";
  inline bool allowed_delay(uint32_t n)  {
    return n==0||n==1||n==16||n==31||n==32;
  }
  struct ClosedOutput {
    std::atomic<bool> rejected{false};
    clap_output_events_t out{this,push};
    static bool push(const clap_output_events_t* p,const clap_event_header_t*) {
      static_cast<ClosedOutput*>(p->ctx)->rejected=true;
      return false;
    }
  };
  struct DelayMemory  {
    std::array<std::array<float,32>,2> queue {
    };
    uint32_t cursor=0,delay=0,rate=0;
    uint64_t frames=0;
    double gain=1;
  };
  static_assert(sizeof(DelayMemory::queue)==256);
  inline DelayMemory decode_delay_state(const std::vector<uint8_t>& bytes,uint32_t rate,uint32_t delay,uint64_t frame)  {
    check(bytes.size()==288,"finite-delay state size");
    Reader r {
      bytes
    };
    DelayMemory x;
    check(r.u(4)==0x444c5903,"finite-delay state version");
    x.rate=uint32_t(r.u(4));
    x.delay=uint32_t(r.u(4));
    x.cursor=uint32_t(r.u(4));
    x.frames=r.u(8);
    x.gain=r.f64();
    check(x.rate==rate&&x.delay==delay&&allowed_delay(delay)&&x.frames==frame&&plain(x.gain)&&x.cursor<(delay?delay:1)&&x.cursor==(delay?frame%delay:0),"finite-delay state ownership");
    for(auto& channel:x.queue)for(size_t i=0;i<32;i++)  {
      channel[i]=r.f32();
      check(std::isfinite(channel[i])&&(i<delay||channel[i]==0),"finite-delay queue state");
    }
    return x;
  }
  inline std::vector<uint8_t> encode_delay_state(const DelayMemory& x)  {
    Writer w;
    w.u(0x444c5903,4);
    w.u(x.rate,4);
    w.u(x.delay,4);
    w.u(x.cursor,4);
    w.u(x.frames,8);
    w.f64(x.gain);
    for(const auto& channel:x.queue)for(auto v:channel)w.f32(v);
    return w.b;
  }
}
