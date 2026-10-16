// Original child-owned bounded host interfaces. No native editor or audio backend.
#pragma once
#include "offline_wire.hpp"
#include <map>
#include <thread>
#include <variant>
namespace Controlled  {
  class Attributes final:public IAttributeList  {
    using Value=std::variant<int64,double,std::u16string,std::vector<uint8_t>>;
    std::map<std::string,Value> values;
    uint32 refs=1;
    bool key(const char* id,std::string& s) {
      if(!id||strnlen(id,128)==128)return false;
      s=id;
      return values.count(s)||values.size()<32;
    }
    template<class V> tresult set(const char* id,V value) {
      std::string s;
      if(!key(id,s))return kInvalidArgument;
      values[s]=std::move(value);
      return kResultOk;
    }
    template<class V> V* get(const char* id) {
      if(!id||strnlen(id,128)==128)return nullptr;
      auto i=values.find(id);
      return i==values.end()?nullptr:std::get_if<V>(&i->second);
    }
    public:
    uint32 PLUGIN_API addRef()override {
      return ++refs;
    }uint32 PLUGIN_API release()override {
      uint32 n=--refs;
      if(!n)delete this;
      return n;
    }
    tresult PLUGIN_API queryInterface(const TUID iid,void** o)override {
      *o=nullptr;
      if(equal(iid,INLINE_UID_OF(FUnknown))||equal(iid,INLINE_UID_OF(IAttributeList))) {
        *o=this;
        addRef();
        return kResultOk;
      }return kNoInterface;
    }
    tresult PLUGIN_API setInt(AttrID id,int64 v)override {
      return set(id,v);
    }tresult PLUGIN_API getInt(AttrID id,int64& v)override {
      auto p=get<int64>(id);
      if(!p)return kResultFalse;
      v=*p;
      return kResultOk;
    }
    tresult PLUGIN_API setFloat(AttrID id,double v)override {
      return std::isfinite(v)?set(id,v):kInvalidArgument;
    }tresult PLUGIN_API getFloat(AttrID id,double& v)override {
      auto p=get<double>(id);
      if(!p)return kResultFalse;
      v=*p;
      return kResultOk;
    }
    tresult PLUGIN_API setString(AttrID id,const TChar* v)override {
      if(!v)return kInvalidArgument;
      size_t n=0;
      while(n<256&&v[n])++n;
      if(n==256)return kInvalidArgument;
      return set(id,std::u16string(reinterpret_cast<const char16_t*>(v),n));
    }
    tresult PLUGIN_API getString(AttrID id,TChar* out,uint32 bytes)override {
      auto p=get<std::u16string>(id);
      if(!p||!out||bytes<(p->size()+1)*2)return kResultFalse;
      std::memcpy(out,p->c_str(),(p->size()+1)*2);
      return kResultOk;
    }
    tresult PLUGIN_API setBinary(AttrID id,const void* p,uint32 n)override {
      if(n>1024||(!p&&n))return kInvalidArgument;
      std::vector<uint8_t> v(n);
      if(n)std::memcpy(v.data(),p,n);
      return set(id,std::move(v));
    }
    tresult PLUGIN_API getBinary(AttrID id,const void*& out,uint32& n)override {
      auto p=get<std::vector<uint8_t>>(id);
      if(!p)return kResultFalse;
      out=p->data();
      n=uint32(p->size());
      return kResultOk;
    }
  };
  class Message final:public IMessage  {
    uint32 refs=1;
    std::string id;
    Attributes* attributes=new Attributes;
    uint32& live;
    bool& fault;
    public:
    Message(uint32& l,bool& f):live(l),fault(f) {
      ++live;
    }
    ~Message() {
      attributes->release();
      --live;
    }
    uint32 PLUGIN_API addRef()override {
      return ++refs;
    }uint32 PLUGIN_API release()override {
      uint32 n=--refs;
      if(!n)delete this;
      return n;
    }
    tresult PLUGIN_API queryInterface(const TUID iid,void** o)override {
      *o=nullptr;
      if(equal(iid,INLINE_UID_OF(FUnknown))||equal(iid,INLINE_UID_OF(IMessage))) {
        *o=this;
        addRef();
        return kResultOk;
      }return kNoInterface;
    }
    FIDString PLUGIN_API getMessageID()override {
      return id.c_str();
    }
    void PLUGIN_API setMessageID(FIDString v)override {
      if(!v||strnlen(v,128)==128) {
        fault=true;
        return;
      }id=v;
    }
    IAttributeList* PLUGIN_API getAttributes()override {
      return attributes;
    }
  };
  class Host final:public IHostApplication,public IComponentHandler  {
    std::thread::id control=std::this_thread::get_id();
    bool gesture=false;
    bool admission(ParamID id) {
      if(std::this_thread::get_id()!=control||id!=gain_id||callbacks.size()>=128) {
        fault=true;
        return false;
      }return true;
    }
    public:
    STACK_REFS
    uint32 messages=0;
    bool fault=false;
    std::vector<std::string> callbacks;
    tresult PLUGIN_API queryInterface(const TUID iid,void** o)override {
      *o=nullptr;
      if(equal(iid,INLINE_UID_OF(FUnknown))||equal(iid,INLINE_UID_OF(IHostApplication)))*o=static_cast<IHostApplication*>(this);
      else if(equal(iid,INLINE_UID_OF(IComponentHandler)))*o=static_cast<IComponentHandler*>(this);
      else return kNoInterface;
      addRef();
      return kResultOk;
    }
    tresult PLUGIN_API getName(String128 name)override {
      utf16(name,"hex controlled offline fixture");
      return kResultOk;
    }
    tresult PLUGIN_API createInstance(TUID cid,TUID iid,void** o)override {
      *o=nullptr;
      if(std::this_thread::get_id()!=control||messages>=32) {
        fault=true;
        return kResultFalse;
      }if(equal(cid,INLINE_UID_OF(IMessage))&&equal(iid,INLINE_UID_OF(IMessage))) {
        *o=new Message(messages,fault);
        return kResultOk;
      }return kNoInterface;
    }
    tresult PLUGIN_API beginEdit(ParamID id)override {
      if(!admission(id)||gesture) {
        fault=true;
        return kResultFalse;
      }gesture=true;
      callbacks.push_back("begin-edit");
      return kResultOk;
    }
    tresult PLUGIN_API performEdit(ParamID id,double v)override {
      if(!admission(id)||!gesture||!norm(v)) {
        fault=true;
        return kResultFalse;
      }callbacks.push_back("perform-edit:"+std::to_string(v));
      return kResultOk;
    }
    tresult PLUGIN_API endEdit(ParamID id)override {
      if(!admission(id)||!gesture) {
        fault=true;
        return kResultFalse;
      }gesture=false;
      callbacks.push_back("end-edit");
      return kResultOk;
    }
    tresult PLUGIN_API restartComponent(int32 flags)override {
      if(std::this_thread::get_id()!=control||callbacks.size()>=128) {
        fault=true;
        return kResultFalse;
      }callbacks.push_back("restart:"+std::to_string(flags));
      fault=true;
      return kResultFalse;
    }
    bool balanced()const {
      return !gesture;
    }
  };
}
