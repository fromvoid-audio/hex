// ORIGINAL offline-only host callbacks. Stable stack ownership until destroy returns.
#pragma once
#include "offline_wire.hpp"
#include <clap/ext/thread-check.h>
#include <thread>
#include <clap/ext/note-ports.h>
namespace Controlled   {
  struct Host   {
    bool midi=false;
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
      check(!invalidated.load(),"fixed MIDI profile rescan/thread refusal");
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
