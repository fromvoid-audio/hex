// ORIGINAL controlled CLAP offline child; no third-party admission or realtime path.
#include "offline_host.hpp"
#include <clap/ext/audio-ports.h>
#include <clap/ext/note-ports.h>
#include <clap/ext/render.h>
#include <clap/ext/params.h>
#include <clap/ext/state.h>
#include <clap/ext/latency.h>
#include <clap/ext/tail.h>
#include <dlfcn.h>
#include <unistd.h>
#include <sys/resource.h>
#include <cstdio>
#include <limits>
using namespace Controlled;
struct Block  {
  std::vector<float> left,right;
  Events events;
  MidiEvents midi_events;
};
struct Owner   {
  Host host;
  // Both host and state adapter contexts outlive plugin destruction, including unwind.
  Stream stream;
  OutputEvents outputs;
  void* module=nullptr;
  const clap_plugin_entry_t* entry=nullptr;
  const clap_plugin_t* plugin=nullptr;
  bool entry_ready=false,active=false;
  std::vector<std::string> lifecycle;
  void close()  {
    if(active)  {
      plugin->deactivate(plugin);
      active=false;
      lifecycle.push_back("deactivate main");
    }if(plugin)  {
      plugin->destroy(plugin);
      plugin=nullptr;
      lifecycle.push_back("destroy main");
    }if(entry_ready)  {
      entry->deinit();
      entry_ready=false;
      lifecycle.push_back("entry deinit main");
    }if(module)  {
      check(dlclose(module)==0,"dlclose failed");
      module=nullptr;
      lifecycle.push_back("unload main");
    }
  }
  ~Owner()  {
    try  {
      close();
    }catch(...)  {
      std::fputs("cleanup failed\n",stderr);
      std::_Exit(3);
    }
  }
};
static void io(void* p,size_t n,bool output)  {
  auto b=static_cast<uint8_t*>(p);
  while(n)  {
    auto got=output?write(0,b,n):read(0,b,n);
    if(got<0&&errno==EINTR)continue;
    check(got>0,"IPC transfer failed");
    b+=got;
    n-=size_t(got);
  }
}
static clap_event_param_value_t value(uint32_t offset,double gain)  {
  clap_event_param_value_t e  {
  };
  e.header=  {
    sizeof(e),offset,CLAP_CORE_EVENT_SPACE_ID,CLAP_EVENT_PARAM_VALUE,0
  };
  e.param_id=gain_id;
  e.note_id=-1;
  e.port_index=-1;
  e.channel=-1;
  e.key=-1;
  e.value=gain;
  return e;
}
#include "delay_attempt.hpp"
int main(int argc,char** argv)  {
  rlimit core  {
    0,0
  };
  setrlimit(RLIMIT_CORE,&core);
  try  {
    check(argc==3,"arguments");
    uint64_t expected=std::stoull(argv[2]);
    uint8_t header[12];
    io(header,12,false);
    if(!std::memcmp(header,"HCOFFQ03",8))return delay_attempt(header,argv,expected);
    const bool midi=!std::memcmp(header,"HCOFFQ02",8);
    check(midi||!std::memcmp(header,"HCOFFQ01",8),"request magic");
    uint32_t size=0;
    for(unsigned i=0;i<4;i++)size|=uint32_t(header[8+i])<<(8*i);
    check(size<=1024*1024,"request capacity");
    std::vector<uint8_t> bytes(size);
    io(bytes.data(),bytes.size(),false);
    uint8_t extra;
    check(read(0,&extra,1)==0,"request trailing bytes");
    Reader r  {
      bytes
    };
    check(r.u(2)==(midi?2:1),"request version");
    const char* expected_id=midi?midi_plugin_id:plugin_id;
    auto request=r.u(8);
    check(request==expected&&request!=0,"request identity");
    auto id=r.blob(128);
    check(std::string(id.begin(),id.end())==expected_id,"plugin identity");
    auto rate=r.u(4),frame=r.u(8);
    const auto start_frame=frame;
    double gain=r.f64();
    std::string module_digest;
    if(midi) {
      auto profile=r.blob(64);
      check(std::string(profile.begin(),profile.end())==midi_profile&&r.u(4)==2,"MIDI state profile/version");
      auto digest=r.blob(64);
      module_digest=std::string(digest.begin(),digest.end());
      check(module_digest.size()==64&&std::all_of(module_digest.begin(),module_digest.end(),[](char c) {
        return (c>='0'&&c<='9')||(c>='a'&&c<='f');
      }),"MIDI state module identity");
    }
    auto state_bytes=r.blob(65536);
    auto blocks=r.u(4);
    check((rate==44100||rate==48000)&&plain(gain)&&blocks>=1&&blocks<=32&&frame<=INT64_MAX,"request profile");
    check(!state_bytes.empty()||frame==0,"fresh frame");
    check(blocks<=std::numeric_limits<size_t>::max()/128/sizeof(InputSlot),"input ABI demand overflow");
    const size_t input_reservation=size_t(blocks)*128*sizeof(InputSlot);
    std::vector<Block> input;
    input.reserve(size_t(blocks));
    size_t total=0,event_count=0;
    for(size_t i=0;i<blocks;i++)  {
      auto n=r.u(4),events=r.u(4);
      check(n>=1&&n<=1024&&events<=128,"block capacity");
      total+=n;
      event_count+=events;
      check(total<=8192&&event_count<=1024&&frame<=uint64_t(INT64_MAX)-total,"aggregate capacity");
      Block b;
      uint32_t prior=0;
      for(size_t j=0;j<events;j++)  {
        auto type=midi?r.u(1):0;
        auto offset=r.u(4);
        check(offset<n&&(!j||offset>=prior),"event order/offset");
        if(type==0) {
          double v=r.f64();
          check(plain(v),"gain value");
          if(midi)b.midi_events.values[b.midi_events.count++]=value(uint32_t(offset),v);
          else b.events.values.push_back(value(uint32_t(offset),v));
        }
        else if(type==1&&midi) {
          clap_event_midi_t e {
          };
          e.header= {
            sizeof(e),uint32_t(offset),CLAP_CORE_EVENT_SPACE_ID,CLAP_EVENT_MIDI,0
          };
          e.port_index=uint16_t(r.u(2));
          for(auto& v:e.data)v=uint8_t(r.u(1));
          check(e.port_index==0&&e.data[0]>=0xb0&&e.data[0]<=0xbf&&e.data[1]==7&&e.data[2]<128,"unsupported MIDI CC7 event");
          b.midi_events.values[b.midi_events.count++]=e;
        }
        else check(false,"unsupported MIDI wire type");
        prior=uint32_t(offset);
      }for(auto* channel:  {
        &b.left,&b.right
      })for(size_t j=0;j<n;j++)  {
        float v=r.f32();
        check(std::isfinite(v),"input nonfinite");
        channel->push_back(v);
      }input.push_back(std::move(b));
    }
    check(r.at==bytes.size(),"request trailing fields");
    Owner owner;
    owner.host.midi=midi;
    owner.module=dlopen(argv[1],RTLD_NOW|RTLD_LOCAL);
    check(owner.module,"module load");
    owner.entry=static_cast<const clap_plugin_entry_t*>(dlsym(owner.module,"clap_entry"));
    check(owner.entry&&owner.entry->clap_version.major==1&&owner.entry->init&&owner.entry->deinit&&owner.entry->get_factory,"entry profile");
    check(owner.entry->init(argv[1]),"entry init failed");
    owner.entry_ready=true;
    owner.lifecycle.push_back("entry init main");
    auto* f=static_cast<const clap_plugin_factory_t*>(owner.entry->get_factory(CLAP_PLUGIN_FACTORY_ID));
    check(f&&f->get_plugin_count&&f->get_plugin_descriptor&&f->create_plugin&&f->get_plugin_count(f)==1,"factory profile");
    auto* d=f->get_plugin_descriptor(f,0);
    check(d&&d->id&&!std::strcmp(d->id,expected_id)&&d->clap_version.major==1,"descriptor identity");
    owner.plugin=f->create_plugin(f,&owner.host.host,expected_id);
    auto* p=owner.plugin;
    check(p&&p->desc&&p->desc->id&&!std::strcmp(p->desc->id,expected_id)&&p->init&&p->destroy&&p->activate&&p->deactivate&&p->start_processing&&p->stop_processing&&p->process&&p->get_extension&&p->on_main_thread,"instance profile");
    owner.lifecycle.push_back("create main");
    check(p->init(p),"instance init failed");
    owner.lifecycle.push_back("init main");
    owner.host.service(p);
    auto* ports=static_cast<const clap_plugin_audio_ports_t*>(p->get_extension(p,CLAP_EXT_AUDIO_PORTS));
    auto* notes=static_cast<const clap_plugin_note_ports_t*>(p->get_extension(p,CLAP_EXT_NOTE_PORTS));
    auto* render=static_cast<const clap_plugin_render_t*>(p->get_extension(p,CLAP_EXT_RENDER));
    auto* params=static_cast<const clap_plugin_params_t*>(p->get_extension(p,CLAP_EXT_PARAMS));
    auto* state=static_cast<const clap_plugin_state_t*>(p->get_extension(p,CLAP_EXT_STATE));
    auto* latency=static_cast<const clap_plugin_latency_t*>(p->get_extension(p,CLAP_EXT_LATENCY));
    auto* tail=static_cast<const clap_plugin_tail_t*>(p->get_extension(p,CLAP_EXT_TAIL));
    check(ports&&ports->count&&ports->get&&render&&render->set&&render->has_hard_realtime_requirement&&params&&params->count&&params->get_info&&params->get_value&&params->flush&&state&&state->load&&state->save&&latency&&latency->get&&tail&&tail->get,"required extensions");
    clap_note_port_info_t note_port {
    };
    if(midi) {
      check(notes&&notes->count&&notes->get&&notes->count(p,true)==1&&notes->count(p,false)==0&&notes->get(p,0,true,&note_port),"MIDI port count/get");
      check(note_port.id==midi_port_id&&note_port.supported_dialects==CLAP_NOTE_DIALECT_MIDI&&note_port.preferred_dialect==CLAP_NOTE_DIALECT_MIDI&&std::memchr(note_port.name,0,sizeof note_port.name)&&note_port.name[0],"MIDI port dialect/identity");
    }
    else check(!notes||(notes->count&&notes->count(p,true)==0&&notes->count(p,false)==0),"MIDI buses refused");
    for(bool in:  {
      true,false
    })  {
      check(ports->count(p,in)==1,"bus count refused");
      clap_audio_port_info_t info  {
      };
      check(ports->get(p,0,in,&info)&&info.channel_count==2&&info.flags==CLAP_AUDIO_PORT_IS_MAIN&&info.port_type&&!std::strcmp(info.port_type,CLAP_PORT_STEREO)&&info.in_place_pair==CLAP_INVALID_ID,"stereo float profile");
    }
    check(!render->has_hard_realtime_requirement(p)&&render->set(p,CLAP_RENDER_OFFLINE),"offline render refused");
    check(params->count(p)==1,"parameter count");
    clap_param_info_t param  {
    };
    check(params->get_info(p,0,&param)&&param.id==gain_id&&param.flags==CLAP_PARAM_IS_AUTOMATABLE&&param.min_value==0&&param.max_value==2&&param.default_value==1&&std::memchr(param.name,0,sizeof param.name),"plain gain profile");
    owner.lifecycle.push_back("extensions offline stereo gain main");
    auto& stream=owner.stream;
    if(!state_bytes.empty())  {
      stream.bytes=state_bytes;
      check(state->load(p,&stream.in)&&!stream.fault&&stream.at==stream.bytes.size(),"state load failed");
      owner.lifecycle.push_back("state load partial main");
    }else  {
      Events initial;
      initial.values.push_back(value(0,gain));
      params->flush(p,&initial.in,&initial.out);
      check(!initial.rejected,"initial flush output refused");
      owner.lifecycle.push_back("initial gain flush inactive main");
    }double admitted_gain=0;
    check(params->get_value(p,gain_id,&admitted_gain)&&plain(admitted_gain),"admitted gain invalid");
    check(p->activate(p,double(rate),1,1024),"activate failed");
    owner.active=true;
    owner.lifecycle.push_back("activate main");
    auto delay=latency->get(p),tail_frames=tail->get(p);
    check(delay==0&&(midi?tail_frames==0:(tail_frames==0||tail_frames==2)),"latency/tail profile refused");
    std::vector<float> left,right;
    std::vector<uint32_t> statuses;
    std::string failure;
    std::vector<std::string> audio_lifecycle;
    std::thread worker([&]  {
      audio_thread=true;bool processing=false,sleeping=false;uint32_t block_index=0;try  {
        for(auto& b:input)  {
          bool zero=std::all_of(b.left.begin(),b.left.end(),[](float v)  {
            return v==0;
          })&&std::all_of(b.right.begin(),b.right.end(),[](float v)  {
            return v==0;
          });auto n=b.left.size();bool wake=owner.host.wake.exchange(0)!=0;
          if(sleeping&&zero&&(midi?b.midi_events.count==0:b.events.values.empty())&&!wake)  {
            left.insert(left.end(),n,0);right.insert(right.end(),n,0);statuses.push_back(5);audio_lifecycle.push_back("sleep skip zero audio");frame+=n;++block_index;continue;
          }
          if(!processing)  {
            check(p->start_processing(p),"start failed");processing=true;audio_lifecycle.push_back("start audio");
          }sleeping=false;
          std::vector<float> l(n,std::numeric_limits<float>::quiet_NaN()),rr(n,std::numeric_limits<float>::quiet_NaN());float* in[2]=  {
            b.left.data(),b.right.data()
          };float* out[2]=  {
            l.data(),rr.data()
          };clap_audio_buffer_t ib  {
            in,nullptr,2,0,0
          },ob  {
            out,nullptr,2,0,0
          };clap_process_t process  {
            int64_t(frame),uint32_t(n),nullptr,&ib,&ob,1,1,midi?&b.midi_events.in:&b.events.in,midi?&owner.outputs.out:&b.events.out
          };
          // Vector moves do not update embedded event adapter contexts.
          b.events.in.ctx=&b.events;b.events.out.ctx=&b.events;b.midi_events.in.ctx=&b.midi_events;
          if(midi)owner.outputs.begin(block_index,uint32_t(n),frame);
          auto status=p->process(p,&process);if(midi)owner.outputs.finish();check(status>=CLAP_PROCESS_CONTINUE&&status<=CLAP_PROCESS_SLEEP,"process ERROR/status discard");check(!(midi?owner.outputs.rejected.load():b.events.rejected),"output event rejection discards whole request");owner.host.bounded();check(std::all_of(l.begin(),l.end(),[](float v)  {
            return std::isfinite(v);
          })&&std::all_of(rr.begin(),rr.end(),[](float v)  {
            return std::isfinite(v);
          }),"processed PCM nonfinite");
          if(status==CLAP_PROCESS_SLEEP)  {
            check(tail_frames==0&&std::all_of(l.begin(),l.end(),[](float v)  {
              return v==0;
            })&&std::all_of(rr.begin(),rr.end(),[](float v)  {
              return v==0;
            }),"sleep nonquiet/tail refused");p->stop_processing(p);processing=false;sleeping=true;audio_lifecycle.push_back("stop sleep audio");
          }
          left.insert(left.end(),l.begin(),l.end());right.insert(right.end(),rr.begin(),rr.end());statuses.push_back(uint32_t(status));audio_lifecycle.push_back("process audio");frame+=n;++block_index;
        }
      }catch(const std::exception& e)  {
        if(midi)owner.outputs.finish();
        failure=e.what();
      }if(processing)  {
        p->stop_processing(p);audio_lifecycle.push_back("stop audio");
      }audio_thread=false;
    });
    worker.join();
    check(failure.empty(),failure.c_str());
    owner.lifecycle.insert(owner.lifecycle.end(),audio_lifecycle.begin(),audio_lifecycle.end());
    owner.host.service(p);
    p->deactivate(p);
    owner.active=false;
    owner.lifecycle.push_back("deactivate main");
    double final_gain=0;
    if(midi)check(params->get_value(p,gain_id,&final_gain)&&plain(final_gain),"MIDI final gain");
    stream.bytes.clear();
    stream.at=0;
    stream.fault=false;
    check(state->save(p,&stream.out)&&!stream.fault,"state save failed");
    owner.lifecycle.push_back("state save partial main");
    owner.close();
    owner.host.bounded();
    check(!owner.outputs.rejected.load(),"late output rejection discards whole request");
    check(owner.lifecycle.size()<=128,"lifecycle capacity");
    auto callbacks=owner.host.records();
    check(callbacks.size()<=128,"callback record capacity");
    Writer w;
    w.u(midi?2:1,2);
    w.u(request,8);
    w.text("195b42a004144fab0b3cf95e9c067187d15365b7");
    w.text("linux-x86_64");
    w.text(expected_id);
    w.u(rate,4);
    w.u(start_frame,8);
    w.u(frame,8);
    w.u(delay,4);
    w.u(tail_frames,4);
    w.u(2,4);
    w.u(2,4);
    w.u(midi?1:0,4);
    w.u(0,4);
    w.u(1,4);
    w.u(param.id,4);
    w.u(param.flags,4);
    w.f64(param.min_value);
    w.f64(param.max_value);
    w.f64(param.default_value);
    w.text(param.name);
    w.u(total,4);
    for(auto v:left)w.f32(v);
    for(auto v:right)w.f32(v);
    w.blob(stream.bytes);
    for(auto* list:  {
      &owner.lifecycle,&callbacks
    })  {
      w.u(list->size(),4);
      for(auto& s:*list)w.text(s);
    }w.u(statuses.size(),4);
    for(auto s:statuses)w.u(s,4);
    if(midi) {
      w.text(midi_profile);
      w.u(2,4);
      w.text(module_digest);
      w.u(note_port.id,4);
      w.u(note_port.supported_dialects,4);
      w.u(note_port.preferred_dialect,4);
      w.text(note_port.name);
      w.u(sizeof(clap_event_midi_t),4);
      w.u(sizeof(clap_event_param_value_t),4);
      w.u(sizeof(InputSlot),4);
      w.u(input_reservation,8);
      w.u(sizeof(OwnedOutput)*owner.outputs.values.size(),8);
      w.f64(final_gain);
      w.u(owner.outputs.count,4);
      for(size_t i=0;i<owner.outputs.count;i++) {
        const auto& e=owner.outputs.values[i];
        w.u(e.block,4);
        w.u(e.offset,4);
        w.u(e.flags,4);
        w.u(e.parameter,4);
        w.u(e.frame,8);
        w.f64(e.value);
      }
    }
    Writer packet;
    packet.raw(midi?"HCOFFR02":"HCOFFR01",8);
    packet.u(w.b.size(),4);
    packet.raw(w.b.data(),w.b.size());
    io(packet.b.data(),packet.b.size(),true);
    return 0;
  }catch(const std::exception& e)  {
    std::fprintf(stderr,"controlled offline refusal: %s\n",e.what());
    return 2;
  }
}
