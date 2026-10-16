// ORIGINAL protocol3 attempt. Publish only after successful stop/unload/reap.
#pragma once
#include "delay_wire.hpp"
static int delay_attempt(const uint8_t* header,char** argv,uint64_t expected)  {
  uint32_t size=0;
  for(unsigned i=0;i<4;i++)size|=uint32_t(header[8+i])<<(8*i);
  check(size<=1024*1024,"request capacity");
  std::vector<uint8_t> bytes(size);
  io(bytes.data(),size,false);
  uint8_t extra;
  check(read(0,&extra,1)==0,"trailing bytes");
  Reader r {
    bytes
  };
  check(r.u(2)==3,"delay version");
  auto request=r.u(8);
  check(request==expected&&request,"request identity");
  auto text=[&](size_t cap) {
    auto b=r.blob(cap);
    return std::string(b.begin(),b.end());
  };
  check(text(128)==delay_plugin_id,"plugin identity");
  auto rate=uint32_t(r.u(4));
  auto frame=r.u(8),start_frame=frame;
  double gain=r.f64();
  check(text(64)==delay_profile&&r.u(4)==3,"profile/version");
  auto module=text(64);
  check(module.size()==64&&std::all_of(module.begin(),module.end(),[](char c) {
    return (c>='0'&&c<='9')||(c>='a'&&c<='f');
  }),"module digest");
  auto delay=uint32_t(r.u(4)),max_tail=uint32_t(r.u(4)),drained=uint32_t(r.u(4));
  auto state_bytes=r.blob(65536);
  auto blocks=r.u(4);
  check((rate==44100||rate==48000)&&plain(gain)&&allowed_delay(delay)&&max_tail>=delay&&max_tail<=32&&drained<=32&&drained<=frame&&blocks>=1&&blocks<=32&&frame<=INT64_MAX,"header/capacity");
  if(state_bytes.empty())check(frame==0&&drained==0&&max_tail==delay,"fresh delay state");
  else  {
    check(state_bytes.size()==360,"continuation envelope size");
    Reader envelope {
      state_bytes
    };
    auto plugin_bytes=envelope.bytes(288);
    check(envelope.u(4)==max_tail&&envelope.u(4)==drained,"continuation ledger mismatch");
    auto digest=envelope.bytes(64);
    check(std::string(digest.begin(),digest.end())==module,"continuation module mismatch");
    auto restored=decode_delay_state(plugin_bytes,rate,delay,frame);
    if(drained>=max_tail)check(std::all_of(restored.queue.begin(),restored.queue.end(),[](const auto& c) {
      return std::all_of(c.begin(),c.end(),[](float v) {
        return v==0;
      });
    }),"drained state queue nonempty");
    state_bytes=std::move(plugin_bytes);
  }
  check(blocks<=std::numeric_limits<size_t>::max()/128/sizeof(clap_event_param_value_t),"input ABI demand overflow");
  const size_t input_reservation=size_t(blocks)*128*sizeof(clap_event_param_value_t);
  std::vector<Block> input;
  input.reserve(blocks);
  std::vector<bool> drain;
  drain.reserve(blocks);
  size_t total=0,events_total=0;
  bool suffix=false;
  uint32_t planned_drained=drained;
  for(size_t i=0;i<blocks;i++)  {
    auto is_drain=r.u(1),n=r.u(4),events=r.u(4);
    check(is_drain<=1&&n>=1&&n<=1024&&events<=128,"block capacity");
    total+=n;
    events_total+=events;
    check(total<=8192&&events_total<=1024&&frame<=uint64_t(INT64_MAX)-total,"aggregate capacity");
    check(!suffix||is_drain,"drain suffix order");
    suffix=suffix||is_drain;
    if(is_drain) {
      check(events==0&&n<=32-planned_drained,"drain ceiling");
      planned_drained+=uint32_t(n);
    }else planned_drained=0;
    Block b;
    b.events.values.reserve(128);check(b.events.values.capacity()==128,"input ABI reservation");
    uint32_t prior=0;
    for(size_t j=0;j<events;j++)  {
      auto offset=r.u(4);
      auto v=r.f64();
      check(offset<n&&(!j||offset>=prior)&&plain(v),"event order/value");
      b.events.values.push_back(value(uint32_t(offset),v));
      prior=uint32_t(offset);
    }
    for(auto* c: {
      &b.left,&b.right
    })for(size_t j=0;j<n;j++) {
      auto v=r.f32();
      check(std::isfinite(v)&&(!is_drain||v==0),"drain/input PCM");
      c->push_back(v);
    }
    input.push_back(std::move(b));
    drain.push_back(is_drain);
  }
  check(r.at==bytes.size(),"trailing fields");
  ClosedOutput output; // declared first: lifetime extends through Owner unwind/destruction
  Owner owner;
  owner.host.finite_delay=true;
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
  check(d&&d->id&&!std::strcmp(d->id,delay_plugin_id),"descriptor identity");
  owner.plugin=f->create_plugin(f,&owner.host.host,delay_plugin_id);
  auto* p=owner.plugin;
  check(p&&p->desc&&p->desc->id&&!std::strcmp(p->desc->id,delay_plugin_id)&&p->init&&p->destroy&&p->activate&&p->deactivate&&p->start_processing&&p->stop_processing&&p->process&&p->get_extension&&p->on_main_thread,"instance profile");
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
  check(!notes||(notes->count&&notes->count(p,true)==0&&notes->count(p,false)==0),"MIDI buses refused");
  for(bool in: {
    true,false
  }) {
    clap_audio_port_info_t info {
    };
    check(ports->count(p,in)==1&&ports->get(p,0,in,&info)&&info.channel_count==2&&info.flags==CLAP_AUDIO_PORT_IS_MAIN&&info.port_type&&!std::strcmp(info.port_type,CLAP_PORT_STEREO)&&info.in_place_pair==CLAP_INVALID_ID,"stereo profile");
  }
  check(!render->has_hard_realtime_requirement(p)&&render->set(p,CLAP_RENDER_OFFLINE),"offline render refused");
  clap_param_info_t param {
  };
  check(params->count(p)==1&&params->get_info(p,0,&param)&&param.id==gain_id&&param.flags==CLAP_PARAM_IS_AUTOMATABLE&&param.min_value==0&&param.max_value==2&&param.default_value==1&&std::memchr(param.name,0,sizeof param.name),"gain profile");
  owner.lifecycle.push_back("extensions offline stereo gain main");
  auto& stream=owner.stream;
  if(state_bytes.empty()) {
    Events initial;
    initial.values.push_back(value(0,gain));
    params->flush(p,&initial.in,&output.out);
    check(!output.rejected.load(),"initial flush output refused");
    owner.lifecycle.push_back("initial gain flush inactive main");
  }
  else {
    stream.bytes=state_bytes;
    check(state->load(p,&stream.in)&&!stream.fault&&stream.at==stream.bytes.size(),"state load failed");
    owner.lifecycle.push_back("state load partial main");
  }
  owner.host.phase=Host::activating;
  check(p->activate(p,rate,1,1024),"activate failed");
  owner.active=true;
  owner.host.phase=Host::active;
  owner.lifecycle.push_back("activate main");
  check(latency->get(p)==delay,"declared latency mismatch");
  owner.host.bounded();
  auto query_tail=[&]() {
    auto n=tail->get(p);
    check(n<=32,"unknown/infinite/oversized tail refused");
    max_tail=std::max(max_tail,n);
    owner.host.tail_changes.exchange(0);
    owner.host.bounded();
  };
  query_tail();
  std::vector<float> left,right;
  left.reserve(total);
  right.reserve(total);
  std::vector<uint32_t> statuses;
  std::vector<std::string> audio_lifecycle;
  std::string failure;
  std::thread worker([&] {
    audio_thread=true;bool processing=false;try  {
      owner.host.phase=Host::starting;check(p->start_processing(p),"start failed");processing=true;owner.host.phase=Host::active;query_tail();audio_lifecycle.push_back("start audio");
      for(size_t i=0;i<input.size();i++)  {
        auto& b=input[i];auto n=b.left.size();std::vector<float> l(n,NAN),rr(n,NAN);
        float* in[2]= {
          b.left.data(),b.right.data()
        };float* out[2]= {
          l.data(),rr.data()
        };clap_audio_buffer_t ib {
          in,nullptr,2,0,0
        },ob {
          out,nullptr,2,0,0
        };
        b.events.in.ctx=&b.events;b.events.out.ctx=&b.events;
        clap_process_t process {
          int64_t(frame),uint32_t(n),nullptr,&ib,&ob,1,1,&b.events.in,&output.out
        };
        owner.host.phase=Host::processing;auto status=p->process(p,&process);owner.host.phase=Host::active;query_tail();
        check(status>=CLAP_PROCESS_CONTINUE&&status<=CLAP_PROCESS_SLEEP&&!output.rejected.load(),"process ERROR/output discard");
        check(std::all_of(l.begin(),l.end(),[](float v) {
          return std::isfinite(v);
        })&&std::all_of(rr.begin(),rr.end(),[](float v) {
          return std::isfinite(v);
        }),"processed PCM nonfinite");
        if(drain[i])drained+=uint32_t(n);else drained=0;
        if(status==CLAP_PROCESS_SLEEP)check(drained>=max_tail&&std::all_of(l.begin(),l.end(),[](float v) {
          return v==0;
        })&&std::all_of(rr.begin(),rr.end(),[](float v) {
          return v==0;
        }),"sleep with pending delay refused");
        left.insert(left.end(),l.begin(),l.end());right.insert(right.end(),rr.begin(),rr.end());statuses.push_back(status);frame+=n;audio_lifecycle.push_back("process audio");
      }
    }catch(const std::exception& e) {
      failure=e.what();
    }
    if(processing) {
      try {
        owner.host.phase=Host::stopping;p->stop_processing(p);owner.host.phase=Host::active;query_tail();audio_lifecycle.push_back("stop audio");
      }catch(const std::exception& e) {
        failure=e.what();
      }
    }
    audio_thread=false;
  });
  worker.join();
  check(failure.empty(),failure.c_str());
  owner.lifecycle.insert(owner.lifecycle.end(),audio_lifecycle.begin(),audio_lifecycle.end());
  owner.host.service(p);
  check(latency->get(p)==delay,"active latency drift refused");
  query_tail();
  p->deactivate(p);
  owner.active=false;
  owner.host.phase=Host::inactive;
  owner.lifecycle.push_back("deactivate main");
  stream.bytes.clear();
  stream.at=0;
  stream.fault=false;
  check(state->save(p,&stream.out)&&!stream.fault,"state save failed");
  auto saved=decode_delay_state(stream.bytes,rate,delay,frame);
  if(drained>=max_tail)check(std::all_of(saved.queue.begin(),saved.queue.end(),[](const auto& c) {
    return std::all_of(c.begin(),c.end(),[](float v) {
      return v==0;
    });
  }),"drained queue nonempty");
  Writer envelope;
  envelope.raw(stream.bytes.data(),stream.bytes.size());
  envelope.u(max_tail,4);
  envelope.u(drained,4);
  envelope.raw(module.data(),module.size());
  stream.bytes=std::move(envelope.b);
  owner.lifecycle.push_back("state save partial main");
  owner.close();
  check(!output.rejected.load(),"late output rejection discards whole request");
  owner.host.bounded();
  check(owner.lifecycle.size()<=128,"lifecycle capacity");
  auto callbacks=owner.host.records();
  check(callbacks.size()<=128,"callback capacity");
  Writer w;
  w.u(3,2);
  w.u(request,8);
  w.text("195b42a004144fab0b3cf95e9c067187d15365b7");
  w.text("linux-x86_64");
  w.text(delay_plugin_id);
  w.u(rate,4);
  w.u(start_frame,8);
  w.u(frame,8);
  w.u(delay,4);
  w.u(max_tail,4);
  w.u(2,4);
  w.u(2,4);
  w.u(0,4);
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
  for(auto* list: {
    &owner.lifecycle,&callbacks
  }) {
    w.u(list->size(),4);
    for(auto& s:*list)w.text(s);
  }w.u(statuses.size(),4);
  for(auto status:statuses)w.u(status,4);
  w.text(delay_profile);
  w.u(3,4);
  w.text(module);
  w.u(drained,4);
  w.u(max_tail>drained?max_tail-drained:0,4);
  w.u(sizeof(DelayMemory::queue),4);
  w.u(sizeof(DelayMemory)-sizeof(DelayMemory::queue),4);
  w.u(input_reservation,8);
  Writer packet;
  packet.raw("HCOFFR03",8);
  packet.u(w.b.size(),4);
  packet.raw(w.b.data(),w.b.size());
  io(packet.b.data(),packet.b.size(),true);
  return 0;
}
