// ORIGINAL gain-before-FIFO fixture. No installed/foreign module admission.
#include "delay_wire.hpp"
#include <clap/ext/render.h>
#include <clap/ext/audio-ports.h>
#include <clap/ext/note-ports.h>
#include <clap/ext/params.h>
#include <clap/ext/state.h>
#include <clap/ext/latency.h>
#include <clap/ext/tail.h>
#include <clap/ext/thread-check.h>
#include <thread>
#include <cstdio>
#include <csignal>
#include <unistd.h>
#ifndef FIXTURE_MODE
#define FIXTURE_MODE 0
#endif
#ifndef FIXTURE_DELAY
#define FIXTURE_DELAY 16
#endif
using namespace Controlled;
static unsigned entries=0,instances=0;
static const char* features[]=  {
  "audio-effect","stereo",nullptr
};
static const clap_plugin_descriptor_t descriptor=  {
  CLAP_VERSION_INIT,delay_plugin_id,"original finite delay","hex","https://invalid.example",nullptr,nullptr,"finite-delay-v1","controlled original gain",features
};
struct Instance   {
  clap_plugin_t plugin;
  const clap_host_t* host;
  bool initialized=false,active=false,processing=false,offline=false;
  DelayMemory memory;
  double& gain=memory.gain;
  uint64_t& processed=memory.frames;
  const clap_output_events_t* last_output=nullptr;
  bool in_activate=false, drift=false;
  uint32_t reported_tail=FIXTURE_DELAY;
  explicit Instance(const clap_host_t* h):host(h)  {
    plugin=  {
      &descriptor,this,init,destroy,activate,deactivate,start,stop,reset,process,extension,on_main
    };
    ++instances;
  }
  static Instance& self(const clap_plugin_t* p)  {
    return *static_cast<Instance*>(p->plugin_data);
  }
  void thread(bool audio)  {
    auto* t=static_cast<const clap_host_thread_check_t*>(host->get_extension(host,CLAP_EXT_THREAD_CHECK));
    check(t&&(audio?t->is_audio_thread(host):t->is_main_thread(host)),"fixture thread violation");
  }
  static bool init(const clap_plugin_t* p)  {
    auto& x=self(p);
    x.thread(false);
    std::fputs("instance init\n",stderr);
    if(FIXTURE_MODE==1)  {
      auto* l=static_cast<const clap_host_latency_t*>(x.host->get_extension(x.host,CLAP_EXT_LATENCY));
      l->changed(x.host);
      // deliberately illegal inactive callback
    }
    x.initialized=true;
    x.host->request_callback(x.host);
    return true;
  }
  static void destroy(const clap_plugin_t* p)  {
    auto& x=self(p);
    x.thread(false);
    check(!x.active&&!x.processing,"destroy active");
    std::fputs("instance destroy\n",stderr);
    if(FIXTURE_MODE==18&&x.last_output) {
      clap_event_header_t event{};x.last_output->try_push(x.last_output,&event);
    }
    if(FIXTURE_MODE==16) {
      auto* l=static_cast<const clap_host_latency_t*>(x.host->get_extension(x.host,CLAP_EXT_LATENCY));l->changed(x.host);
    }
    x.host->request_callback(x.host);
    --instances;
    delete &x;
  }
  static bool activate(const clap_plugin_t* p,double rate,uint32_t min,uint32_t max)  {
    auto& x=self(p);
    x.thread(false);
    check(x.initialized&&!x.active&&x.offline,"activate order");
    if(!(rate==44100||rate==48000)||min!=1||max!=1024)return false;
    if(x.memory.rate&&x.memory.rate!=uint32_t(rate))return false;
    x.memory.rate=uint32_t(rate);
    x.memory.delay=FIXTURE_DELAY;
    x.in_activate=true;
    auto* l=static_cast<const clap_host_latency_t*>(x.host->get_extension(x.host,CLAP_EXT_LATENCY));
    check(l,"latency host missing");
    l->changed(x.host);
    x.in_activate=false;
    x.active=true;
    std::fputs("instance activate\n",stderr);
    return true;
  }
  static void deactivate(const clap_plugin_t* p)  {
    auto& x=self(p);
    x.thread(false);
    check(x.active&&!x.processing,"deactivate order");
    x.active=false;
    std::fputs("instance deactivate\n",stderr);
  }
  static bool start(const clap_plugin_t* p)  {
    auto& x=self(p);
    x.thread(true);
    check(x.active&&!x.processing,"start order");
    x.processing=true;
    auto* t=static_cast<const clap_host_tail_t*>(x.host->get_extension(x.host,CLAP_EXT_TAIL));
    t->changed(x.host);
    std::fputs("instance start\n",stderr);
    return true;
  }
  static void stop(const clap_plugin_t* p)  {
    auto& x=self(p);
    x.thread(true);
    check(x.processing,"stop order");
    auto* t=static_cast<const clap_host_tail_t*>(x.host->get_extension(x.host,CLAP_EXT_TAIL));
    if(FIXTURE_MODE==15)x.reported_tail=32;
    t->changed(x.host);
    x.processing=false;
    std::fputs("instance stop\n",stderr);
  }
  static void reset(const clap_plugin_t* p)  {
    auto& x=self(p);
    x.thread(true);
    x.memory.queue= {
    };
    x.memory.cursor=0;
  }
  static clap_process_status process(const clap_plugin_t* p,const clap_process_t* b)  {
    auto& x=self(p);
    x.thread(true);
    check(x.processing&&b&&b->audio_inputs_count==1&&b->audio_outputs_count==1&&b->frames_count<=1024&&b->steady_time>=0,"process profile");
    x.last_output=b->out_events;
    if(FIXTURE_MODE==17||FIXTURE_MODE==19) {
      auto emit=[&]{clap_event_header_t event{};b->out_events->try_push(b->out_events,&event);};
      if(FIXTURE_MODE==19){std::thread foreign(emit);foreign.join();}else emit();
    }
    bool zero=true;
    for(unsigned c=0;c<2;c++)for(uint32_t i=0;i<b->frames_count;i++)if(b->audio_inputs[0].data32[c][i]!=0)zero=false;
    if(FIXTURE_MODE==12) {
      auto* l=static_cast<const clap_host_latency_t*>(x.host->get_extension(x.host,CLAP_EXT_LATENCY));l->changed(x.host);
    }
    if(FIXTURE_MODE==14)x.reported_tail=33;
    if(FIXTURE_MODE==3)x.drift=true;
    if(FIXTURE_MODE==4)x.host->request_restart(x.host);
    if(FIXTURE_MODE==5)x.reported_tail=UINT32_MAX;
    if(FIXTURE_MODE==6)x.reported_tail=x.processed?0:32;
    // shrinking must not truncate drain
    auto* t=static_cast<const clap_host_tail_t*>(x.host->get_extension(x.host,CLAP_EXT_TAIL));
    if(FIXTURE_MODE==2)  {
      std::thread foreign([&] {
        t->changed(x.host);
      });
      foreign.join();
    }
    else t->changed(x.host);
    if(FIXTURE_MODE==8&&x.processed&&zero)for(;;)pause();
    if(FIXTURE_MODE==9&&x.processed&&zero)std::raise(SIGSEGV);
    if(FIXTURE_MODE==10)  {
      auto pid=fork();
      if(pid==0) {
        signal(SIGTERM,SIG_IGN);
        for(;;)pause();
      }
      std::fprintf(stderr,"fixture descendant=%ld\n",long(pid));
    }
    uint32_t event=0;
    bool quiet=true;
    for(uint32_t i=0;i<b->frames_count;i++)  {
      while(event<b->in_events->size(b->in_events))  {
        auto* h=b->in_events->get(b->in_events,event);
        check(h&&h->size==sizeof(clap_event_param_value_t)&&h->space_id==CLAP_CORE_EVENT_SPACE_ID&&h->type==CLAP_EVENT_PARAM_VALUE,"event profile");
        if(h->time!=i)break;
        auto* e=reinterpret_cast<const clap_event_param_value_t*>(h);
        check(e->param_id==gain_id&&e->note_id==-1&&e->port_index==-1&&e->channel==-1&&e->key==-1&&plain(e->value),"event value");
        x.gain=e->value;
        ++event;
      }
      for(unsigned c=0;c<2;c++)  {
        float v=float(double(b->audio_inputs[0].data32[c][i])*x.gain);
        if(FIXTURE_DELAY)  {
          auto old=x.memory.queue[c][x.memory.cursor];
          x.memory.queue[c][x.memory.cursor]=v;
          v=old;
        }
        b->audio_outputs[0].data32[c][i]=v;
        if(v!=0)quiet=false;
      }
      if constexpr(FIXTURE_DELAY!=0)x.memory.cursor=(x.memory.cursor+1)%FIXTURE_DELAY;
    }
    check(event==b->in_events->size(b->in_events),"events unconsumed");
    check(x.processed<=uint64_t(INT64_MAX)-b->frames_count,"fixture state frame overflow");
    x.processed+=b->frames_count;
    if(FIXTURE_MODE==7&&x.processed>b->frames_count)return CLAP_PROCESS_ERROR;
    if(FIXTURE_MODE==11&&x.processed>b->frames_count)b->audio_outputs[0].data32[0][0]=NAN;
    (void)quiet;
    return CLAP_PROCESS_CONTINUE;
  }
  static void on_main(const clap_plugin_t* p)  {
    auto& x=self(p);
    x.thread(false);
    if(FIXTURE_MODE==13) {
      auto* t=static_cast<const clap_host_tail_t*>(x.host->get_extension(x.host,CLAP_EXT_TAIL));t->changed(x.host);
    }
    std::fputs("instance main callback\n",stderr);
  }
  static const void* extension(const clap_plugin_t*,const char*);
};
static uint32_t ports_count(const clap_plugin_t* p,bool)  {
  Instance::self(p).thread(false);
  return 1;
}
static bool port_get(const clap_plugin_t* p,uint32_t i,bool input,clap_audio_port_info_t* out)  {
  auto& x=Instance::self(p);
  x.thread(false);
  if(i||!out)return false;
  *out=  {
  };
  out->id=input?1:2;
  std::strcpy(out->name,"main");
  out->flags=CLAP_AUDIO_PORT_IS_MAIN;
  out->channel_count=2;
  out->port_type=CLAP_PORT_STEREO;
  out->in_place_pair=CLAP_INVALID_ID;
  return true;
}
static uint32_t notes_count(const clap_plugin_t*,bool)  {
  return 0;
}
static bool note_get(const clap_plugin_t*,uint32_t,bool,clap_note_port_info_t*)  {
  return false;
}
static bool hard(const clap_plugin_t*)  {
  return false;
}
static bool render_set(const clap_plugin_t* p,clap_plugin_render_mode mode)  {
  auto& x=Instance::self(p);
  x.thread(false);
  if(x.active||mode!=CLAP_RENDER_OFFLINE)return false;
  x.offline=true;
  return true;
}
static uint32_t param_count(const clap_plugin_t*)  {
  return 1;
}
static bool param_info(const clap_plugin_t* p,uint32_t i,clap_param_info_t* out)  {
  Instance::self(p).thread(false);
  if(i||!out)return false;
  *out=  {
  };
  out->id=gain_id;
  out->flags=CLAP_PARAM_IS_AUTOMATABLE;
  std::strcpy(out->name,"gain");
  out->min_value=0;
  out->max_value=2;
  out->default_value=1;
  return true;
}
static bool param_value(const clap_plugin_t* p,clap_id id,double* v)  {
  Instance::self(p).thread(false);
  if(id!=gain_id||!v)return false;
  *v=Instance::self(p).gain;
  return true;
}
static bool value_text(const clap_plugin_t*,clap_id,double,char*,uint32_t)  {
  return false;
}
static bool text_value(const clap_plugin_t*,clap_id,const char*,double*)  {
  return false;
}
static void flush(const clap_plugin_t* p,const clap_input_events_t* in,const clap_output_events_t*)  {
  auto& x=Instance::self(p);
  x.thread(false);
  check(!x.active&&!x.processing,"flush overlap");
  for(uint32_t i=0;i<in->size(in);i++)  {
    auto* h=in->get(in,i);
    check(h&&h->type==CLAP_EVENT_PARAM_VALUE&&h->size==sizeof(clap_event_param_value_t),"flush event");
    auto* e=reinterpret_cast<const clap_event_param_value_t*>(h);
    check(e->param_id==gain_id&&plain(e->value),"flush gain");
    x.gain=e->value;
  }
}
static bool save(const clap_plugin_t* p,const clap_ostream_t* stream)  {
  auto& x=Instance::self(p);
  x.thread(false);
  check(!x.active,"state active");
  Writer w;
  w.b=encode_delay_state(x.memory);
  size_t at=0;
  while(at<w.b.size())  {
    auto n=stream->write(stream,w.b.data()+at,w.b.size()-at);
    if(n<=0||uint64_t(n)>w.b.size()-at)return false;
    at+=size_t(n);
  }return true;
}
static bool load(const clap_plugin_t* p,const clap_istream_t* stream)  {
  auto& x=Instance::self(p);
  x.thread(false);
  check(!x.active,"state active");
  std::vector<uint8_t> bytes(288);
  size_t at=0;
  while(at<bytes.size())  {
    auto n=stream->read(stream,bytes.data()+at,bytes.size()-at);
    if(n<=0||uint64_t(n)>bytes.size()-at)return false;
    at+=size_t(n);
  }uint8_t extra;
  if(stream->read(stream,&extra,1)!=0)return false;
  try  {
    x.memory=decode_delay_state(bytes,uint32_t(bytes[4])|(uint32_t(bytes[5])<<8)|(uint32_t(bytes[6])<<16)|(uint32_t(bytes[7])<<24),FIXTURE_DELAY,
    [&] {
      Reader r {
        bytes
      };r.u(4);r.u(4);r.u(4);r.u(4);return r.u(8);
    }());
  }
  catch(...)  {
    return false;
  }
  return true;
}
static uint32_t latency(const clap_plugin_t* p)  {
  auto& x=Instance::self(p);
  x.thread(false);
  check(x.active||x.in_activate,"latency query phase");
  return FIXTURE_DELAY+(x.drift?1:0);
}
static uint32_t tail(const clap_plugin_t* p)  {
  auto& x=Instance::self(p);
  auto* t=static_cast<const clap_host_thread_check_t*>(x.host->get_extension(x.host,CLAP_EXT_THREAD_CHECK));
  check(t&&(t->is_main_thread(x.host)||t->is_audio_thread(x.host)),"tail query thread");
  return x.reported_tail;
}
static const clap_plugin_audio_ports_t ports=  {
  ports_count,port_get
};
static const clap_plugin_note_ports_t notes=  {
  notes_count,note_get
};
static const clap_plugin_render_t render=  {
  hard,render_set
};
static const clap_plugin_params_t params=  {
  param_count,param_info,param_value,value_text,text_value,flush
};
static const clap_plugin_state_t state=  {
  save,load
};
static const clap_plugin_latency_t delay=  {
  latency
};
static const clap_plugin_tail_t tails=  {
  tail
};
const void* Instance::extension(const clap_plugin_t*,const char* id)  {
  if(!std::strcmp(id,CLAP_EXT_AUDIO_PORTS))return &ports;
  if(!std::strcmp(id,CLAP_EXT_NOTE_PORTS))return &notes;
  if(!std::strcmp(id,CLAP_EXT_RENDER))return &render;
  if(!std::strcmp(id,CLAP_EXT_PARAMS))return &params;
  if(!std::strcmp(id,CLAP_EXT_STATE))return &state;
  if(!std::strcmp(id,CLAP_EXT_LATENCY))return &delay;
  if(!std::strcmp(id,CLAP_EXT_TAIL))return &tails;
  return nullptr;
}
static bool entry_init(const char*)  {
  ++entries;
  std::fputs("entry init\n",stderr);
  return true;
}
static void entry_deinit()  {
  check(instances==0,"live instance at deinit");
  --entries;
  std::fputs("entry deinit\n",stderr);
}
static uint32_t count(const clap_plugin_factory_t*)  {
  return 1;
}
static const clap_plugin_descriptor_t* get(const clap_plugin_factory_t*,uint32_t i)  {
  return i?nullptr:&descriptor;
}
static const clap_plugin_t* create(const clap_plugin_factory_t*,const clap_host_t* host,const char* id)  {
  if(!host||std::strcmp(id,delay_plugin_id))return nullptr;
  return &(new Instance(host))->plugin;
}
static const clap_plugin_factory_t factory=  {
  count,get,create
};
static const void* get_factory(const char* id)  {
  return std::strcmp(id,CLAP_PLUGIN_FACTORY_ID)?nullptr:&factory;
}
extern "C" CLAP_EXPORT const clap_plugin_entry_t clap_entry=  {
  CLAP_VERSION_INIT,entry_init,entry_deinit,get_factory
};
__attribute__((destructor)) static void unloaded()  {
  std::fprintf(stderr,"fixture unload entries=%u instances=%u\n",entries,instances);
}
