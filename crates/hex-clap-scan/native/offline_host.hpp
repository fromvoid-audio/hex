// ORIGINAL offline-only host callbacks. Stable stack ownership until destroy returns.
#pragma once
#include "offline_wire.hpp"
#include <clap/ext/thread-check.h>
#include <thread>
#include <clap/ext/note-ports.h>
#include <clap/ext/latency.h>
#include <clap/ext/tail.h>
namespace Controlled   {
  struct Host   {
    bool midi=false, finite_delay=false;
    enum Phase { inactive, activating, active, starting, processing, stopping };
    std::atomic<Phase> phase{inactive};
    std::atomic<unsigned> latency_changes{0},tail_changes{0};
    static void latency_changed(const clap_host_t* p) {
      auto& x=self(p);
      ++x.total;
      if(!is_main(p)||x.phase.load()!=activating)x.invalidated=true;
      ++x.latency_changes;
    }
    static void tail_changed(const clap_host_t* p) {
      auto& x=self(p); ++x.total;
      auto phase=x.phase.load();
      if(!is_audio(p)||(phase!=starting&&phase!=processing&&phase!=stopping))x.invalidated=true;
      ++x.tail_changes;
    }
    std::atomic<bool> invalidated {
      false
    };
    std::thread::id main=std::this_thread::get_id();
    std::atomic<unsigned> total {
      0
    };
    std::atomic<unsigned> restart  {
      0
    },wake  {
      0
    },callback  {
      0
    };
    clap_host_t host  {
      CLAP_VERSION_INIT,this,"hex controlled offline","hex","https://invalid.example","fixture-v1",extension,request_restart,request_process,request_callback
    };
    static Host& self(const clap_host_t* p)  {
      return *static_cast<Host*>(p->host_data);
    }
    static bool is_main(const clap_host_t* p)  {
      return std::this_thread::get_id()==self(p).main&&!audio_thread;
    }
    static bool is_audio(const clap_host_t*)  {
      return audio_thread;
    }
    static const void* extension(const clap_host_t* p,const char* id)  {
      static const clap_host_thread_check_t check=  {
        is_main,is_audio
      };
      if(!std::strcmp(id,CLAP_EXT_THREAD_CHECK))return &check;
      static const clap_host_note_ports_t notes= {
        supported_dialects,rescan
      };
      static const clap_host_latency_t latency={latency_changed};
      static const clap_host_tail_t tail={tail_changed};
      if(self(p).finite_delay&&!std::strcmp(id,CLAP_EXT_LATENCY))return &latency;
      if(self(p).finite_delay&&!std::strcmp(id,CLAP_EXT_TAIL))return &tail;
      return self(p).midi&&!std::strcmp(id,CLAP_EXT_NOTE_PORTS)?&notes:nullptr;
    }
    static uint32_t supported_dialects(const clap_host_t* p) {
      if(!is_main(p)) {
        self(p).invalidated=true;
        return 0;
      }return CLAP_NOTE_DIALECT_MIDI;
    }
    static void rescan(const clap_host_t* p,uint32_t) {
      self(p).invalidated=true;
    }
    static void request_restart(const clap_host_t* p)  {
      ++self(p).total;
      ++self(p).restart;
    }
    static void request_process(const clap_host_t* p)  {
      ++self(p).total;
      ++self(p).wake;
    }
    static void request_callback(const clap_host_t* p)  {
      ++self(p).total;
      ++self(p).callback;
    }
    void bounded()  {
      check(!invalidated.load(),"fixed profile callback phase/thread refusal");
      check(total.load()<=128,"callback capacity");
      check(!restart.load(),"restart invalidates controlled attempt");
    }
    void service(const clap_plugin_t* p)  {
      bounded();
      auto n=callback.exchange(0);
      for(unsigned i=0;i<n;i++)p->on_main_thread(p);
      serviced+=n;
    }
    unsigned serviced=0;
    std::vector<std::string> records()  {
      std::vector<std::string> r;
      for(unsigned i=0;i<serviced;i++)r.push_back("request_callback serviced main");
      for(unsigned i=0;i<callback.load();i++)r.push_back("request_callback during destroy retained");
      for(unsigned i=0;i<wake.load();i++)r.push_back("request_process");
      return r;
    }
  };
}
