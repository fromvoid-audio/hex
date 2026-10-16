// ORIGINAL MIDI1 CC7 gain fixture; no installed modules, editor, instrument or realtime host.
#include "offline_wire.hpp"
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
using namespace Controlled;
static unsigned entries=0,instances=0;
static const char* features[]=  {
  "audio-effect","stereo",nullptr
};
static const clap_plugin_descriptor_t descriptor=  {
  CLAP_VERSION_INIT,midi_plugin_id,"original offline gain","hex","https://invalid.example",nullptr,nullptr,"fixture-v1","controlled original gain",features
};
struct Instance   {
  clap_plugin_t plugin;
  const clap_host_t* host;
  bool initialized=false,active=false,processing=false,offline=false;
  double gain=1;
  uint64_t processed=0;
  float residual[2]=  {
    0,0
  };
  const clap_output_events_t* last_output=nullptr;
  uint32_t remaining=0;
  uint8_t fault=0;
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
    auto* notes=static_cast<const clap_host_note_ports_t*>(x.host->get_extension(x.host,CLAP_EXT_NOTE_PORTS));
    check(notes&&notes->supported_dialects(x.host)==CLAP_NOTE_DIALECT_MIDI,"host MIDI dialect");
    if(FIXTURE_MODE==6)notes->rescan(x.host,CLAP_NOTE_PORTS_RESCAN_ALL);
    x.initialized=true;
    x.host->request_callback(x.host);
    return true;
  }
  static void destroy(const clap_plugin_t* p)  {
    auto& x=self(p);
    x.thread(false);
    check(!x.active&&!x.processing,"destroy active");
    if(FIXTURE_MODE==15&&x.last_output){clap_event_param_value_t out{};out.header={sizeof(out),0,CLAP_CORE_EVENT_SPACE_ID,CLAP_EVENT_PARAM_VALUE,CLAP_EVENT_DONT_RECORD};out.param_id=gain_id;out.note_id=-1;out.port_index=-1;out.channel=-1;out.key=-1;out.value=x.gain;x.last_output->try_push(x.last_output,&out.header);}
    std::fputs("instance destroy\n",stderr);
    x.host->request_callback(x.host);
    --instances;
    delete &x;
  }
  static bool activate(const clap_plugin_t* p,double rate,uint32_t min,uint32_t max)  {
    auto& x=self(p);
    x.thread(false);
    check(x.initialized&&!x.active&&x.offline,"activate order");
    if(!(rate==44100||rate==48000)||min!=1||max!=1024)return false;
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
    std::fputs("instance start\n",stderr);
    return true;
  }
  static void stop(const clap_plugin_t* p)  {
    auto& x=self(p);
    x.thread(true);
    check(x.processing,"stop order");
    x.processing=false;
    std::fputs("instance stop\n",stderr);
  }
  static void reset(const clap_plugin_t* p)  {
    auto& x=self(p);
    x.thread(true);
    x.remaining=0;
  }
  static clap_process_status process(const clap_plugin_t* p,const clap_process_t* b)  {
    auto& x=self(p);
    x.thread(true);
    check(x.processing&&b&&b->audio_inputs_count==1&&b->audio_outputs_count==1&&b->frames_count<=1024&&b->steady_time>=0,"process profile");
    if(FIXTURE_MODE==4)for(;;)pause();
    if(FIXTURE_MODE==3)std::raise(SIGSEGV);
    if(FIXTURE_MODE==5) {
      auto pid=fork();
      if(pid==0) {
        signal(SIGTERM,SIG_IGN);
        for(;;)pause();
      }std::fprintf(stderr,"fixture descendant=%ld\n",long(pid));
    }
    if(FIXTURE_MODE==12) {
      auto* n=static_cast<const clap_host_note_ports_t*>(x.host->get_extension(x.host,CLAP_EXT_NOTE_PORTS));
      n->supported_dialects(x.host);
    }
    x.last_output=b->out_events;
    uint32_t event=0;
    bool quiet=true;
    auto emit=[&](uint32_t time,double gain) {
      clap_event_param_value_t out {
      };
      out.header= {
        sizeof(out),time,CLAP_CORE_EVENT_SPACE_ID,CLAP_EVENT_PARAM_VALUE,CLAP_EVENT_DONT_RECORD
      };
      out.param_id=gain_id;
      out.cookie=&x;
      out.note_id=-1;
      out.port_index=-1;
      out.channel=-1;
      out.key=-1;
      out.value=gain;
      if(FIXTURE_MODE==8) {
        switch(x.fault%13) {
          case 0:out.header.size=sizeof(clap_event_header_t);
          break;
          case 1:out.header.space_id=1;
          break;
          case 2:out.header.type=CLAP_EVENT_MIDI;
          break;
          case 3:out.header.flags=0;
          break;
          case 4:out.param_id++;
          break;
          case 5:out.note_id=0;
          break;
          case 6:out.port_index=0;
          break;
          case 7:out.value=NAN;
          break;
          case 8:out.header.time=b->frames_count;
          break;
          case 9:out.channel=0;
          break;
          case 10:out.key=0;
          break;
          case 11:out.header.flags|=0x80000000;
          break;
          default:out.value=3;
          break;
        }
      }
      // Mutating after try_push is a negative control for synchronous owned copying.
      b->out_events->try_push(b->out_events,&out.header);
      out.value=-999;
      out.cookie=nullptr;
    };
    for(uint32_t i=0;i<b->frames_count;i++) {
      while(event<b->in_events->size(b->in_events)) {
        auto* h=b->in_events->get(b->in_events,event);
        check(h&&h->space_id==CLAP_CORE_EVENT_SPACE_ID&&h->flags==0,"event header");
        if(h->time!=i)break;
        if(h->type==CLAP_EVENT_PARAM_VALUE) {
          check(h->size==sizeof(clap_event_param_value_t),"parameter size");
          auto* e=reinterpret_cast<const clap_event_param_value_t*>(h);
          check(e->param_id==gain_id&&e->note_id==-1&&e->port_index==-1&&e->channel==-1&&e->key==-1&&plain(e->value),"parameter target/value");
          x.gain=e->value;
        }
        else  {
          check(h->type==CLAP_EVENT_MIDI&&h->size==sizeof(clap_event_midi_t),"MIDI type/size");
          auto* e=reinterpret_cast<const clap_event_midi_t*>(h);
          check(e->port_index==0&&e->data[0]>=0xb0&&e->data[0]<=0xbf&&e->data[1]==7&&e->data[2]<128,"MIDI CC7 profile");
          x.fault=e->data[2];
          x.gain=2.0*e->data[2]/127.0;
          emit(i,x.gain);
        }
        ++event;
      }
      for(unsigned c=0;c<2;c++) {
        float v=float(double(b->audio_inputs[0].data32[c][i])*x.gain);
        b->audio_outputs[0].data32[c][i]=v;
        if(v!=0)quiet=false;
      }
    }
    check(event==b->in_events->size(b->in_events),"events unconsumed");
    check(x.processed<=uint64_t(INT64_MAX)-b->frames_count,"state frame overflow");
    x.processed+=b->frames_count;
    if(FIXTURE_MODE==14){std::thread foreign([&]{emit(0,x.gain);});foreign.join();}
    if(FIXTURE_MODE==9) {
      emit(b->frames_count-1,x.gain);
      emit(0,x.gain);
    }
    if(FIXTURE_MODE==10)for(unsigned i=0;i<129;i++)emit(0,x.gain);
    if(FIXTURE_MODE==11)for(unsigned i=0;i<128;i++)emit(b->frames_count-1,x.gain);
    if(FIXTURE_MODE==2&&x.processed>1)return CLAP_PROCESS_ERROR;
    if(FIXTURE_MODE==1&&quiet)return CLAP_PROCESS_SLEEP;
    return CLAP_PROCESS_CONTINUE;
  }
  static void on_main(const clap_plugin_t* p)  {
    auto& x=self(p);
    x.thread(false);
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
static uint32_t notes_count(const clap_plugin_t* p,bool input)  {
  Instance::self(p).thread(false);
  return input?1:0;
}
static bool note_get(const clap_plugin_t* p,uint32_t i,bool input,clap_note_port_info_t* out)  {
  Instance::self(p).thread(false);
  if(i||!input||!out)return false;
  *out= {
  };
  out->id=midi_port_id;
  out->supported_dialects=FIXTURE_MODE==7?CLAP_NOTE_DIALECT_MIDI_MPE:CLAP_NOTE_DIALECT_MIDI;
  out->preferred_dialect=out->supported_dialects;
  std::strcpy(out->name,"original cc7 input");
  return true;
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
  w.u(0x43433702,4);
  w.f64(x.gain);
  w.u(x.processed,8);
  w.u(x.remaining,4);
  for(auto v:x.residual)w.f32(v);
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
  if(FIXTURE_MODE==13)return false;
  std::vector<uint8_t> bytes(32);
  size_t at=0;
  while(at<bytes.size())  {
    auto n=stream->read(stream,bytes.data()+at,bytes.size()-at);
    if(n<=0||uint64_t(n)>bytes.size()-at)return false;
    at+=size_t(n);
  }uint8_t extra;
  if(stream->read(stream,&extra,1)!=0)return false;
  Reader r  {
    bytes
  };
  if(r.u(4)!=0x43433702)return false;
  auto gain=r.f64();
  auto frames=r.u(8);
  auto remaining=r.u(4);
  float left=r.f32(),right=r.f32();
  if(!plain(gain)||remaining!=0||left!=0||right!=0||frames>INT64_MAX)return false;
  x.gain=gain;
  x.processed=frames;
  x.remaining=uint32_t(remaining);
  x.residual[0]=left;
  x.residual[1]=right;
  return true;
}
static uint32_t latency(const clap_plugin_t*)  {
  return 0;
}
static uint32_t tail(const clap_plugin_t*)  {
  return 0;
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
  if(!host||std::strcmp(id,midi_plugin_id))return nullptr;
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
