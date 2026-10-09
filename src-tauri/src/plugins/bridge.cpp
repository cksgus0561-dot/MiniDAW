// Windows x64 VST3 / CLAP host. All lifecycle/UI operations use our message thread.
// process() uses preallocated storage only; plugin DSP is never called by the UI.
#define NOMINMAX
#define INIT_CLASS_IID
#include <windows.h>
#include <objbase.h>
#include <algorithm>
#include <array>
#include <atomic>
#include <cmath>
#include <charconv>
#include <cstring>
#include <functional>
#include <future>
#include <map>
#include <memory>
#include <mutex>
#include <queue>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/base/ibstream.h"
#include "pluginterfaces/gui/iplugview.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivsthostapplication.h"
#include "pluginterfaces/vst/ivstevents.h"
#include "pluginterfaces/vst/ivstparameterchanges.h"
#include "pluginterfaces/vst/ivstmidicontrollers.h"
#include "pluginterfaces/vst/ivstprocesscontext.h"
#include <clap/clap.h>
using namespace Steinberg;
using namespace Steinberg::Vst;
namespace md {
constexpr size_t LIMIT = 32 * 1024 * 1024;
constexpr int MAX_EVENTS = 1024;
thread_local bool audio_thread = false;
thread_local void* fault_stack[24]{};thread_local USHORT fault_depth=0;
thread_local DWORD fault_code=0;thread_local void* fault_address=nullptr;
int record_fault(EXCEPTION_POINTERS*p){fault_code=p->ExceptionRecord->ExceptionCode;fault_address=p->ExceptionRecord->ExceptionAddress;fault_depth=audio_thread?0:CaptureStackBackTrace(0,24,fault_stack,nullptr);return EXCEPTION_EXECUTE_HANDLER;}
struct AudioRole { bool old=audio_thread; AudioRole(){audio_thread=true;} ~AudioRole(){audio_thread=old;} };
std::string utf8(const wchar_t* s) {
    if(!s)return {};
    int n=WideCharToMultiByte(CP_UTF8,0,s,-1,nullptr,0,nullptr,nullptr);
    std::string r(n,0);WideCharToMultiByte(CP_UTF8,0,s,-1,r.data(),n,nullptr,nullptr);r.pop_back();return r;
}
std::wstring wide(const char* s) {
    int n=MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,s,-1,nullptr,0);
    if(!n)throw std::runtime_error("Invalid UTF-8 path");
    std::wstring r(n,0);MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,s,-1,r.data(),n);r.pop_back();return r;
}
std::string quote(const std::string& s) {
    std::string o="\"";for(unsigned char c:s){if(c=='"'||c=='\\'){o+='\\';o+=c;}else if(c<32){char b[7];sprintf_s(b,"\\u%04x",c);o+=b;}else o+=c;}return o+'"';
}
std::string fault_trace(){std::string s;for(unsigned i=0;i<fault_depth;i++){HMODULE m=nullptr;char name[MAX_PATH]{};GetModuleHandleExA(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS|GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,(const char*)fault_stack[i],&m);GetModuleFileNameA(m,name,MAX_PATH);s+="\n"+std::string(name)+"+"+std::to_string((uintptr_t)fault_stack[i]-(uintptr_t)m);}return s;}
std::string number(double v) { char b[64]; auto r=std::to_chars(b,b+64,v,std::chars_format::general,17); return std::string(b,r.ptr); }
std::string hex(const void* p,size_t n) {
    const auto*b=(const unsigned char*)p;const char*h="0123456789abcdef";std::string s; s.reserve(n*2);
    for(size_t i=0;i<n;i++){s+=h[b[i]>>4];s+=h[b[i]&15];}return s;
}
std::vector<unsigned char> unhex(const char*s) {
    size_t n=strlen(s);if(n%2||n/2>LIMIT)throw std::runtime_error("Invalid plugin state");
    std::vector<unsigned char> o(n/2);auto nib=[](char c){if(c>='0'&&c<='9')return c-'0';if(c>='a'&&c<='f')return c-'a'+10;if(c>='A'&&c<='F')return c-'A'+10;throw std::runtime_error("Invalid hex");};
    for(size_t i=0;i<n;i+=2)o[i/2]=(unsigned char)((nib(s[i])<<4)|nib(s[i+1]));return o;
}
void require(bool b,const char*s){if(!b)throw std::runtime_error(s);}
template<class T> T* query(FUnknown*p){T*r=nullptr;if(p)p->queryInterface(T::iid.toTUID(),(void**)&r);return r;}
// Host-owned callback objects have a lifetime longer than their plugin.
#define HOST_UNKNOWN(T) \
 tresult PLUGIN_API queryInterface(const TUID iid_arg,void**o) override {if(FUnknownPrivate::iidEqual(iid_arg,T::iid)||FUnknownPrivate::iidEqual(iid_arg,FUnknown::iid)){*o=static_cast<T*>(this);addRef();return kResultOk;}*o=nullptr;return kNoInterface;} \
 uint32 PLUGIN_API addRef() override{return 1;} uint32 PLUGIN_API release() override{return 1;}
struct Stream:IBStream {
    std::vector<unsigned char> data;int64 at=0;HOST_UNKNOWN(IBStream)
    tresult PLUGIN_API read(void*p,int32 n,int32*done) override {if(n<0)return kInvalidArgument;int32 k=(int32)std::min<size_t>(n,data.size()-std::min<size_t>((size_t)at,data.size()));if(k)memcpy(p,data.data()+at,k);at+=k;if(done)*done=k;return kResultOk;}
    tresult PLUGIN_API write(void*p,int32 n,int32*done) override {if(n<0||at+n>LIMIT)return kInvalidArgument;data.resize(std::max<size_t>(data.size(),(size_t)at+n));if(n)memcpy(data.data()+at,p,n);at+=n;if(done)*done=n;return kResultOk;}
    tresult PLUGIN_API seek(int64 p,int32 mode,int64*result) override {int64 next=p+(mode==kIBSeekCur?at:mode==kIBSeekEnd?(int64)data.size():0);if(next<0||next>LIMIT)return kInvalidArgument;at=next;if(result)*result=at;return kResultOk;}
    tresult PLUGIN_API tell(int64*p) override {*p=at;return kResultOk;}
};
struct Attributes:IAttributeList {
    std::map<std::string,int64> ints;std::map<std::string,double> floats;
    std::map<std::string,std::u16string> strings;std::map<std::string,std::vector<unsigned char>> bins;
    HOST_UNKNOWN(IAttributeList)
    tresult PLUGIN_API setInt(AttrID k,int64 v)override{if(audio_thread)return kNotImplemented;ints[k]=v;return kResultOk;}
    tresult PLUGIN_API getInt(AttrID k,int64&v)override{if(audio_thread)return kNotImplemented;auto i=ints.find(k);if(i==ints.end())return kResultFalse;v=i->second;return kResultOk;}
    tresult PLUGIN_API setFloat(AttrID k,double v)override{if(audio_thread)return kNotImplemented;floats[k]=v;return kResultOk;}
    tresult PLUGIN_API getFloat(AttrID k,double&v)override{if(audio_thread)return kNotImplemented;auto i=floats.find(k);if(i==floats.end())return kResultFalse;v=i->second;return kResultOk;}
    tresult PLUGIN_API setString(AttrID k,const TChar*v)override{if(audio_thread)return kNotImplemented;strings[k]=(const char16_t*)v;return kResultOk;}
    tresult PLUGIN_API getString(AttrID k,TChar*v,uint32 n)override{if(audio_thread)return kNotImplemented;auto i=strings.find(k);if(i==strings.end()||n<2)return kResultFalse;auto count=std::min<size_t>(n/2-1,i->second.size());memcpy(v,i->second.data(),count*2);v[count]=0;return kResultOk;}
    tresult PLUGIN_API setBinary(AttrID k,const void*v,uint32 n)override{if(audio_thread)return kNotImplemented;if(n>LIMIT)return kInvalidArgument;bins[k]=std::vector<unsigned char>((const unsigned char*)v,(const unsigned char*)v+n);return kResultOk;}
    tresult PLUGIN_API getBinary(AttrID k,const void*&v,uint32&n)override{if(audio_thread)return kNotImplemented;auto i=bins.find(k);if(i==bins.end())return kResultFalse;v=i->second.data();n=(uint32)i->second.size();return kResultOk;}
};
struct Message;
// Plugins may release a main-thread host message from process(). Reclaim on main.
std::array<std::atomic<Message*>,1024> retired_messages{};
struct Message:IMessage {
    std::atomic<uint32> refs{1};std::string id;Attributes attrs;
    tresult PLUGIN_API queryInterface(const TUID i,void**o)override{if(FUnknownPrivate::iidEqual(i,IMessage::iid)||FUnknownPrivate::iidEqual(i,FUnknown::iid)){*o=this;addRef();return kResultOk;}*o=nullptr;return kNoInterface;}
    uint32 PLUGIN_API addRef()override{return ++refs;} uint32 PLUGIN_API release()override{auto n=--refs;if(!n){if(audio_thread){for(auto&slot:retired_messages){Message*empty=nullptr;if(slot.compare_exchange_strong(empty,this))return 0;}/* Full: quarantine rather than free on RT. */}else delete this;}return n;}
    FIDString PLUGIN_API getMessageID()override{return id.c_str();}
    void PLUGIN_API setMessageID(FIDString p)override{if(!audio_thread)id=p?p:"";}
    IAttributeList* PLUGIN_API getAttributes()override{return &attrs;}
};
struct Events:IEventList {
    std::array<Event,MAX_EVENTS> values{};int32 count=0;HOST_UNKNOWN(IEventList)
    int32 PLUGIN_API getEventCount()override{return count;}
    tresult PLUGIN_API getEvent(int32 i,Event&e)override{if(i<0||i>=count)return kInvalidArgument;e=values[i];return kResultOk;}
    tresult PLUGIN_API addEvent(Event&e)override{if(count==MAX_EVENTS)return kResultFalse;values[count++]=e;return kResultOk;}
};
struct ParamQueue:IParamValueQueue {
    ParamID id=0;double value=0;bool used=false;HOST_UNKNOWN(IParamValueQueue)
    ParamID PLUGIN_API getParameterId()override{return id;}
    int32 PLUGIN_API getPointCount()override{return used?1:0;}
    tresult PLUGIN_API getPoint(int32 i,int32&offset,ParamValue&v)override{if(i||!used)return kInvalidArgument;offset=0;v=value;return kResultOk;}
    tresult PLUGIN_API addPoint(int32,ParamValue v,int32&i)override{value=v;used=true;i=0;return kResultOk;}
};
struct Changes:IParameterChanges {
    std::array<ParamQueue,MAX_EVENTS> q{};int32 count=0;HOST_UNKNOWN(IParameterChanges)
    int32 PLUGIN_API getParameterCount()override{return count;}
    IParamValueQueue* PLUGIN_API getParameterData(int32 i)override{return i>=0&&i<count?&q[i]:nullptr;}
    IParamValueQueue* PLUGIN_API addParameterData(const ParamID&id,int32&i)override{for(i=0;i<count;i++)if(q[i].id==id)return &q[i];if(count==MAX_EVENTS)return nullptr;i=count++;q[i].id=id;q[i].used=false;return &q[i];}
    void set(ParamID id,double v){int32 i=0;if(auto*queue=addParameterData(id,i))queue->addPoint(0,v,i);}
};
struct Library {
    HMODULE module=nullptr;const clap_plugin_entry_t*entry=nullptr;bool vst=false;
    Library(const char*path,bool format):vst(format){module=LoadLibraryExW(wide(path).c_str(),nullptr,LOAD_WITH_ALTERED_SEARCH_PATH);require(module!=nullptr,("LoadLibrary failed: "+std::to_string(GetLastError())).c_str());if(vst){auto init=(bool(*)())GetProcAddress(module,"InitDll");require(!init||init(),"InitDll failed");}else{entry=(const clap_plugin_entry_t*)GetProcAddress(module,"clap_entry");require(entry&&clap_version_is_compatible(entry->clap_version),"CLAP entry/version unsupported");require(entry->init(path),"CLAP entry init failed");}}
    ~Library(){if(entry)entry->deinit();if(vst){auto exit=(bool(*)())GetProcAddress(module,"ExitDll");if(exit)exit();}if(module)FreeLibrary(module);}
};
std::shared_ptr<Library> library(const char*path,bool vst){// Keep loaded modules pinned for the host lifetime: third-party message windows
    // and deferred OS callbacks may outlive a destroyed instance. Instances still
    // deactivate/destroy normally when removed; scans run in disposable processes.
    static auto* modules=new std::map<std::string,std::shared_ptr<Library>>;auto&lib=(*modules)[path];if(!lib)lib=std::make_shared<Library>(path,vst);return lib;}
struct Instance;
LRESULT CALLBACK window_proc(HWND,UINT,WPARAM,LPARAM);
struct Main {
    DWORD id=0;HANDLE wake=CreateEventW(nullptr,FALSE,FALSE,nullptr);
    std::mutex mutex;std::queue<std::function<void()>> jobs;
    std::vector<Instance*> instances;
    Main();
    void tick();
    template<class F> auto call(F f)->decltype(f()){
        if(GetCurrentThreadId()==id)return f();
        auto task=std::make_shared<std::packaged_task<decltype(f())()>>(std::move(f));auto result=task->get_future();
        {std::lock_guard<std::mutex>lock(mutex);jobs.push([task]{(*task)();});}SetEvent(wake);return result.get();
    }
};
Main& main(){static Main* m=new Main;return *m;}
struct Parameter {
    uint32_t id=0;std::string name;double min=0,max=1,value=0;bool stepped=false,readonly=false;double step=0;
};
struct Host:IHostApplication,IComponentHandler,IPlugFrame {
    Instance*owner;explicit Host(Instance*p):owner(p){}
    tresult PLUGIN_API queryInterface(const TUID,void**)override;
    uint32 PLUGIN_API addRef()override{return 1;}uint32 PLUGIN_API release()override{return 1;}
    tresult PLUGIN_API getName(String128 name)override{memcpy(name,u"MiniDAW",16);return kResultOk;}
    tresult PLUGIN_API createInstance(TUID cid,TUID requested,void**o)override{*o=nullptr;if(audio_thread)return kNotImplemented;if(FUnknownPrivate::iidEqual(cid,IMessage::iid)&&FUnknownPrivate::iidEqual(requested,IMessage::iid)){*o=new Message;return kResultOk;}return kNoInterface;}
    tresult PLUGIN_API beginEdit(ParamID)override{return kResultOk;}
    tresult PLUGIN_API performEdit(ParamID,double)override;
    tresult PLUGIN_API endEdit(ParamID)override{return kResultOk;}
    tresult PLUGIN_API restartComponent(int32)override;
    tresult PLUGIN_API resizeView(IPlugView*,ViewRect*)override;
};
struct Instance {
    std::shared_ptr<Library> library_owner;HMODULE module=nullptr;bool vst=false,initialized=false,active=false,single_controller=false,processing=false;
    const char*stage="library";bool instrument=false,offline=false;std::atomic<bool>editor{false};HWND window=nullptr,owner=nullptr;double rate=48000;int inputs=0,outputs=2;
    std::array<int,16> input_channels{},output_channels{};int input_buses=0,output_buses=0;
    std::atomic<uint64_t>latency_revision{0},latency_seen{0};std::atomic<uint32_t>latency{0};uint32_t tail=0;std::atomic<bool> fault{false},callback{false},restart{false},dirty{false},metadata_changed{false},quiescent{false};
    std::atomic<int> restart_reason{0};std::atomic<uint64_t> calls{0},dropped{0},resize_request{0};std::atomic<int> window_request{0};std::string name,error,path;
    Host host{this};IPluginFactory*factory=nullptr;IComponent*component=nullptr;IAudioProcessor*processor=nullptr;
    IEditController*controller=nullptr;IConnectionPoint*cp=nullptr,*cc=nullptr;IPlugView*view=nullptr;
    IMidiMapping*mapping=nullptr;std::array<std::array<ParamID,130>,16> midi_map{};
    std::array<Event,256> held{};std::array<bool,256> held_used{};
    Events events,out_events;Changes changes,out_changes;ProcessContext context{};
    const clap_plugin_entry_t*entry=nullptr;const clap_plugin_t*plugin=nullptr;clap_host_t clap_host{};
    const clap_plugin_gui_t*gui=nullptr;const clap_plugin_params_t*params=nullptr;const clap_plugin_state_t*state=nullptr;
    const clap_plugin_timer_support_t*timer=nullptr;bool native_notes=false;
    struct Timer{ULONGLONG next;uint32_t period;};std::map<uint32_t,Timer> timers;uint32_t timer_serial=0;
    struct alignas(8) ClapEvent { unsigned char bytes[sizeof(clap_event_transport_t)]; };
    std::array<ClapEvent,MAX_EVENTS> clap_events{};uint32_t clap_count=0;
    std::vector<Parameter> parameters;
    // UI -> audio parameter mailbox. Allocation occurs only during initialization.
    std::unique_ptr<std::atomic<double>[]> pending;
    std::unique_ptr<std::atomic<bool>[]> changed;
    std::atomic<bool> pending_any{false};
    ~Instance();
    void open(const char*,const char*,const char*,double,const char*);
    void show();void hide();void resize(int,int);
    std::string info(bool details=true);std::string save();
    void midi(uint8_t,uint8_t,uint8_t,uint32_t);
    void parameter(uint32_t,double);
    void flush_pending(){if(pending_any.exchange(false))for(size_t i=0;i<parameters.size();i++)if(changed[i].exchange(false))parameter(parameters[i].id,pending[i].load());}
    void reset();
    void process(float*,int64_t,double,int,int,bool);
    void poll();void restart_plugin();
    template<class T>void clap_event(const T&e){if(clap_count<MAX_EVENTS)memcpy(clap_events[clap_count++].bytes,&e,sizeof e);else dropped++;}
    static uint32_t CLAP_ABI event_count(const clap_input_events_t*l){return ((Instance*)l->ctx)->clap_count;}
    static const clap_event_header_t* CLAP_ABI event_at(const clap_input_events_t*l,uint32_t i){auto*p=(Instance*)l->ctx;return i<p->clap_count?(clap_event_header_t*)p->clap_events[i].bytes:nullptr;}
    static bool CLAP_ABI event_out(const clap_output_events_t*l,const clap_event_header_t*e){
        auto*p=(Instance*)l->ctx;if(p->editor&&e->space_id==CLAP_CORE_EVENT_SPACE_ID&&e->type==CLAP_EVENT_PARAM_VALUE&&!(e->flags&CLAP_EVENT_DONT_RECORD))p->dirty.store(true);return true;
    }
    void changed_param(uint32_t id,double v){if(!pending){dirty.store(true);return;}for(size_t i=0;i<parameters.size();i++)if(parameters[i].id==id){if(pending[i].load()==v)return;pending[i].store(v);changed[i].store(true);pending_any.store(true);dirty.store(true);break;}}
};
Main::Main(){
    std::promise<void> ready;auto started=ready.get_future();
    std::thread([this,&ready]{
        id=GetCurrentThreadId();CoInitializeEx(nullptr,COINIT_APARTMENTTHREADED);
        WNDCLASSW wc{};wc.lpfnWndProc=window_proc;wc.hInstance=GetModuleHandleW(nullptr);wc.lpszClassName=L"MiniDAWPluginEditor";wc.hCursor=LoadCursor(nullptr,IDC_ARROW);RegisterClassW(&wc);
        ready.set_value();
        for(;;){MsgWaitForMultipleObjects(1,&wake,FALSE,10,QS_ALLINPUT);
            std::queue<std::function<void()>> q;{std::lock_guard<std::mutex>lock(mutex);q.swap(jobs);}while(!q.empty()){q.front()();q.pop();}
            MSG msg;while(PeekMessageW(&msg,nullptr,0,0,PM_REMOVE)){TranslateMessage(&msg);DispatchMessageW(&msg);}
            tick();
        }
    }).detach();started.get();
}
int guarded(void(*fn)(void*),void*arg);
void Main::tick(){for(auto&slot:retired_messages)delete slot.exchange(nullptr);for(auto*p:instances){if(!guarded([](void*v){auto*p=(Instance*)v;try{p->poll();}catch(...){p->fault=true;}},p))p->fault=true;}}
LRESULT CALLBACK window_proc(HWND h,UINT m,WPARAM w,LPARAM l){
    auto*p=(Instance*)GetWindowLongPtrW(h,GWLP_USERDATA);
    if(m==WM_NCCREATE){p=(Instance*)((CREATESTRUCTW*)l)->lpCreateParams;SetWindowLongPtrW(h,GWLP_USERDATA,(LONG_PTR)p);}
    if(p&&m==WM_CLOSE){p->hide();return 0;}
    if(p&&m==WM_SIZE&&p->editor){RECT r;GetClientRect(h,&r);if(p->view){ViewRect v{0,0,r.right,r.bottom};p->view->onSize(&v);}else if(p->gui)p->gui->set_size(p->plugin,r.right,r.bottom);}
    return DefWindowProcW(h,m,w,l);
}
tresult Host::queryInterface(const TUID i,void**o){
    *o=nullptr;
    if(FUnknownPrivate::iidEqual(i,IHostApplication::iid)||FUnknownPrivate::iidEqual(i,FUnknown::iid))*o=static_cast<IHostApplication*>(this);
    else if(FUnknownPrivate::iidEqual(i,IComponentHandler::iid))*o=static_cast<IComponentHandler*>(this);
    else if(FUnknownPrivate::iidEqual(i,IPlugFrame::iid))*o=static_cast<IPlugFrame*>(this);
    return *o?kResultOk:kNoInterface;
}
tresult Host::performEdit(ParamID id,double v){if(owner)owner->changed_param(id,v);return kResultOk;}
tresult Host::restartComponent(int32 flags){if(!owner)return kResultFalse;if(flags&kLatencyChanged)owner->latency_revision.fetch_add(1);if(flags&(kReloadComponent|kIoChanged))owner->restart.store(true);if(owner->editor&&(flags&(kParamValuesChanged|kReloadComponent)))owner->dirty.store(true);return kResultOk;}
tresult Host::resizeView(IPlugView*,ViewRect*r){if(owner)owner->resize(r->right-r->left,r->bottom-r->top);return kResultOk;}
Instance* from(const clap_host_t*h){return (Instance*)h->host_data;}
static const clap_host_thread_check_t thread_check{
 [](const clap_host_t*)->bool{return !audio_thread&&GetCurrentThreadId()==main().id;},
 [](const clap_host_t*)->bool{return audio_thread;}
};
static const clap_host_gui_t host_gui{
 [](const clap_host_t*){},[](const clap_host_t*h,uint32_t w,uint32_t y)->bool{if(w>8192||y>8192)return false;from(h)->resize_request.store(((uint64_t)w<<32)|y);return true;},
 [](const clap_host_t*h)->bool{from(h)->window_request.store(1);return true;},
 [](const clap_host_t*h)->bool{from(h)->window_request.store(2);return true;},
 [](const clap_host_t*h,bool){from(h)->editor=false;if(from(h)->window)ShowWindow(from(h)->window,SW_HIDE);}
};
static const clap_host_state_t host_state{[](const clap_host_t*h){if(from(h)->editor)from(h)->dirty.store(true);}};
static const clap_host_params_t host_params{
 [](const clap_host_t*h,uint32_t flags){if(from(h)->editor&&(flags&(CLAP_PARAM_RESCAN_VALUES|CLAP_PARAM_RESCAN_ALL)))from(h)->dirty.store(true);if(flags&CLAP_PARAM_RESCAN_ALL)from(h)->metadata_changed.store(true);},
 [](const clap_host_t*,clap_id,uint32_t){},
 [](const clap_host_t*h){from(h)->callback.store(true);}
};
static const clap_host_latency_t host_latency{[](const clap_host_t*h){from(h)->latency_revision.fetch_add(1);}};
static const clap_host_timer_support_t host_timer{
 [](const clap_host_t*h,uint32_t period,clap_id*id)->bool{auto*p=from(h);if(p->timers.size()>=64||period==0)return false;*id=++p->timer_serial;p->timers[*id]={GetTickCount64()+period,period};return true;},
 [](const clap_host_t*h,clap_id id)->bool{return from(h)->timers.erase(id)>0;}
};
const void* CLAP_ABI extension(const clap_host_t*,const char*id){
    if(!strcmp(id,CLAP_EXT_THREAD_CHECK))return &thread_check;
    if(!strcmp(id,CLAP_EXT_GUI))return &host_gui;
    if(!strcmp(id,CLAP_EXT_STATE))return &host_state;
    if(!strcmp(id,CLAP_EXT_PARAMS))return &host_params;
    if(!strcmp(id,CLAP_EXT_LATENCY))return &host_latency;
    if(!strcmp(id,CLAP_EXT_TIMER_SUPPORT))return &host_timer;
    return nullptr;
}
std::string scan(const char*path,const char*format){
    auto owner=library(path,!strcmp(format,"vst3"));HMODULE module=owner->module;
    require(module!=nullptr,("LoadLibrary failed: "+std::to_string(GetLastError())).c_str());
    std::string list="[";bool first=true;
    auto add=[&](std::string id,std::string name,std::string vendor,bool inst){if(!first)list+=",";first=false;list+="{\"id\":"+quote(id)+",\"name\":"+quote(name)+",\"vendor\":"+quote(vendor)+",\"instrument\":"+(inst?"true":"false")+"}";};
    if(!strcmp(format,"vst3")){
        
        auto get=(IPluginFactory*(*)())GetProcAddress(module,"GetPluginFactory");require(get!=nullptr,"GetPluginFactory missing");auto*f=get();require(f!=nullptr,"Plugin factory missing");
        PFactoryInfo fi{};f->getFactoryInfo(&fi);auto*f2=query<IPluginFactory2>(f);
        for(int32 i=0;i<f->countClasses()&&i<1024;i++){PClassInfo c{};if(f->getClassInfo(i,&c)!=kResultOk||strcmp(c.category,kVstAudioEffectClass))continue;PClassInfo2 c2{};if(f2)f2->getClassInfo2(i,&c2);FUID uid(c.cid);char id[33]{};uid.toString(id);add(id,c.name,c2.vendor[0]?c2.vendor:fi.vendor,strstr(c2.subCategories,"Instrument")!=nullptr);}
        if(f2)f2->release();f->release();
    }else{
        auto*e=(const clap_plugin_entry_t*)GetProcAddress(module,"clap_entry");require(e&&clap_version_is_compatible(e->clap_version),"CLAP entry/version unsupported");
        auto*f=(const clap_plugin_factory_t*)e->get_factory(CLAP_PLUGIN_FACTORY_ID);require(f!=nullptr,"CLAP factory missing");
        for(uint32_t i=0;i<f->get_plugin_count(f)&&i<1024;i++){auto*d=f->get_plugin_descriptor(f,i);bool inst=false;if(d->features)for(auto s=d->features;*s;s++)if(!strcmp(*s,CLAP_PLUGIN_FEATURE_INSTRUMENT))inst=true;add(d->id,d->name,d->vendor?d->vendor:"",inst);}
    }
    return list+"]";
}
void Instance::open(const char*p,const char*fmt,const char*id,double sr,const char*stored){
    path=p;vst=!strcmp(fmt,"vst3");rate=sr;
    library_owner=library(p,vst);module=library_owner->module;
    auto bytes=unhex(stored);
    if(vst){
        
        stage="VST3 factory";auto get=(IPluginFactory*(*)())GetProcAddress(module,"GetPluginFactory");require(get!=nullptr,"GetPluginFactory missing");factory=get();require(factory!=nullptr,"Plugin factory missing");
        if(auto*f=query<IPluginFactory3>(factory)){static auto*factory_host=new Host(nullptr);f->setHostContext(static_cast<IHostApplication*>(factory_host));f->release();}
        stage="VST3 create component";FUID uid;require(uid.fromString(id),"Invalid VST3 class ID");require(factory->createInstance(uid.toTUID(),IComponent::iid.toTUID(),(void**)&component)==kResultOk&&component,"Cannot create VST3 component");
        stage="VST3 component init";component->setIoMode(kAdvanced);require(component->initialize(static_cast<IHostApplication*>(&host))==kResultOk,"VST3 initialize failed");initialized=true;
        processor=query<IAudioProcessor>(component);require(processor!=nullptr,"VST3 audio processor missing");require(processor->canProcessSampleSize(kSample32)==kResultTrue,"32-bit processing unsupported");
        controller=query<IEditController>(component);single_controller=controller!=nullptr;
        if(!controller){TUID cid{};if(component->getControllerClassId(cid)==kResultOk&&factory->createInstance(cid,IEditController::iid.toTUID(),(void**)&controller)==kResultOk&&controller)require(controller->initialize(static_cast<IHostApplication*>(&host))==kResultOk,"VST3 controller initialize failed");}
        if(controller){controller->setComponentHandler(&host);cp=query<IConnectionPoint>(component);cc=query<IConnectionPoint>(controller);if(cp&&cc){cp->connect(cc);cc->connect(cp);}}
        stage="VST3 state load";if(!bytes.empty()){
            require(bytes.size()>=4,"Invalid VST3 state");uint32_t n;memcpy(&n,bytes.data(),4);require(n<=bytes.size()-4,"Invalid VST3 component state");
            Stream stream;stream.data.assign(bytes.begin()+4,bytes.begin()+4+n);require(component->setState(&stream)==kResultOk,"VST3 state restore failed");
            if(controller){stream.at=0;controller->setComponentState(&stream);stream.data.assign(bytes.begin()+4+n,bytes.end());stream.at=0;if(!stream.data.empty())require(controller->setState(&stream)==kResultOk,"VST3 controller state restore failed");}
        }else if(controller){Stream s;if(component->getState(&s)==kResultOk){s.at=0;controller->setComponentState(&s);}}
        stage="VST3 buses";int32 ni=component->getBusCount(kAudio,kInput),no=component->getBusCount(kAudio,kOutput);require(no>0&&ni<=16&&no<=16,"VST3 audio buses unsupported");input_buses=ni;output_buses=no;
        BusInfo bi{};if(ni){component->getBusInfo(kAudio,kInput,0,bi);inputs=bi.channelCount;}component->getBusInfo(kAudio,kOutput,0,bi);outputs=bi.channelCount;
        require((inputs==0||inputs==1||inputs==2)&&(outputs==1||outputs==2),"Only mono/stereo main buses supported");
        std::vector<SpeakerArrangement> in(ni,0),out(no,0);if(ni)in[0]=inputs==1?SpeakerArr::kMono:SpeakerArr::kStereo;out[0]=outputs==1?SpeakerArr::kMono:SpeakerArr::kStereo;
        require(processor->setBusArrangements(in.data(),ni,out.data(),no)==kResultOk,"VST3 main bus arrangement rejected");
        for(int32 i=0;i<ni;i++)component->activateBus(kAudio,kInput,i,i==0);for(int32 i=0;i<no;i++)component->activateBus(kAudio,kOutput,i,i==0);
        for(int32 i=0;i<component->getBusCount(kEvent,kInput);i++)component->activateBus(kEvent,kInput,i,i==0);
        if(controller){int32 count=controller->getParameterCount();require(count>=0&&count<=16384,"Parameter count exceeds host limit");for(int32 i=0;i<count;i++){ParameterInfo q{};if(controller->getParameterInfo(i,q)!=kResultOk)continue;parameters.push_back({q.id,utf8((wchar_t*)q.title),0,1,controller->getParamNormalized(q.id),q.stepCount>0,(q.flags&ParameterInfo::kIsReadOnly)!=0,q.stepCount>0?1.0/q.stepCount:0});}
            mapping=query<IMidiMapping>(controller);}
        for(int ch=0;ch<16;ch++)for(int control=0;control<130;control++){ParamID mapped=kNoParamId;if(mapping)mapping->getMidiControllerAssignment(0,(int16)ch,(CtrlNumber)control,mapped);midi_map[ch][control]=mapped;}
        stage="VST3 activation";ProcessSetup setup{offline?kOffline:kRealtime,kSample32,1,rate};require(processor->setupProcessing(setup)==kResultOk,"VST3 setupProcessing failed");
        require(component->setActive(true)==kResultOk,"VST3 activation failed");active=true;processor->setProcessing(true);processing=true;
        latency=processor->getLatencySamples();tail=processor->getTailSamples();
    }else{
        entry=library_owner->entry;initialized=true;
        clap_host={CLAP_VERSION,this,"MiniDAW","MiniDAW","https://localhost","0.1.0",extension,[](const clap_host_t*h){from(h)->restart_reason=1;from(h)->restart.store(true);},[](const clap_host_t*){},[](const clap_host_t*h){from(h)->callback.store(true);}};
        auto*f=(const clap_plugin_factory_t*)entry->get_factory(CLAP_PLUGIN_FACTORY_ID);require(f!=nullptr,"CLAP factory missing");stage="CLAP init";plugin=f->create_plugin(f,&clap_host,id);stage="CLAP plugin init";require(plugin&&plugin->init(plugin),"CLAP plugin init failed");
        auto*ports=(const clap_plugin_audio_ports_t*)plugin->get_extension(plugin,CLAP_EXT_AUDIO_PORTS);require(ports!=nullptr,"CLAP audio ports missing");auto ni=ports->count(plugin,true),no=ports->count(plugin,false);require(ni<=16&&no>0&&no<=16,"CLAP bus count unsupported");input_buses=ni;output_buses=no;for(uint32_t i=0;i<ni;i++){clap_audio_port_info_t q{};require(ports->get(plugin,i,true,&q)&&q.channel_count>0&&q.channel_count<=2,"CLAP bus channels unsupported");input_channels[i]=q.channel_count;}for(uint32_t i=0;i<no;i++){clap_audio_port_info_t q{};require(ports->get(plugin,i,false,&q)&&q.channel_count>0&&q.channel_count<=2,"CLAP bus channels unsupported");output_channels[i]=q.channel_count;}clap_audio_port_info_t info{};if(ni){require(ports->get(plugin,0,true,&info),"CLAP input port");inputs=info.channel_count;}require(ports->get(plugin,0,false,&info),"CLAP output port");outputs=info.channel_count;require(inputs<=2&&outputs>0&&outputs<=2,"Only mono/stereo CLAP supported");
        auto*notes=(const clap_plugin_note_ports_t*)plugin->get_extension(plugin,CLAP_EXT_NOTE_PORTS);if(notes&&notes->count(plugin,true)){clap_note_port_info_t q{};if(notes->get(plugin,0,true,&q)){native_notes=(q.supported_dialects&CLAP_NOTE_DIALECT_MIDI)==0;require(!native_notes,"Phase 1 requires a CLAP MIDI 1 input port (CC/Sustain/Pitch Bend)");}}
        stage="CLAP state load";state=(const clap_plugin_state_t*)plugin->get_extension(plugin,CLAP_EXT_STATE);
        if(!bytes.empty()){require(state!=nullptr,"CLAP state restore unavailable");struct Read{const std::vector<unsigned char>*v;size_t at=0;}r{&bytes};clap_istream_t in{&r,[](const clap_istream_t*s,void*b,uint64_t n)->int64_t{auto&r=*(Read*)s->ctx;n=std::min<uint64_t>(n,r.v->size()-r.at);memcpy(b,r.v->data()+r.at,(size_t)n);r.at+=(size_t)n;return n;}};require(state->load(plugin,&in),"CLAP state restore failed");}
        stage="CLAP parameters";params=(const clap_plugin_params_t*)plugin->get_extension(plugin,CLAP_EXT_PARAMS);if(params){auto count=params->count(plugin);require(count<=16384,"Parameter count exceeds host limit");for(uint32_t i=0;i<count;i++){clap_param_info_t q{};if(params->get_info(plugin,i,&q)){double v=q.default_value;params->get_value(plugin,q.id,&v);parameters.push_back({q.id,q.name,q.min_value,q.max_value,v,(q.flags&CLAP_PARAM_IS_STEPPED)!=0,(q.flags&CLAP_PARAM_IS_READONLY)!=0,(q.flags&CLAP_PARAM_IS_STEPPED)?1.0:0});}}}
        stage="CLAP gui extensions";gui=(const clap_plugin_gui_t*)plugin->get_extension(plugin,CLAP_EXT_GUI);
        timer=(const clap_plugin_timer_support_t*)plugin->get_extension(plugin,CLAP_EXT_TIMER_SUPPORT);
        stage="CLAP render mode";if(auto*r=(const clap_plugin_render_t*)plugin->get_extension(plugin,CLAP_EXT_RENDER))require(r->set(plugin,offline?CLAP_RENDER_OFFLINE:CLAP_RENDER_REALTIME),"CLAP render mode rejected");
        stage="CLAP activate";require(plugin->activate(plugin,rate,1,1),"CLAP activate failed");active=true;
        if(auto*l=(const clap_plugin_latency_t*)plugin->get_extension(plugin,CLAP_EXT_LATENCY))latency=l->get(plugin);
        if(auto*t=(const clap_plugin_tail_t*)plugin->get_extension(plugin,CLAP_EXT_TAIL))tail=t->get(plugin);
    }
    stage="parameter mailbox";pending=std::make_unique<std::atomic<double>[]>(parameters.size());changed=std::make_unique<std::atomic<bool>[]>(parameters.size());
    for(size_t i=0;i<parameters.size();i++){pending[i].store(parameters[i].value);changed[i].store(false);}
    // Requests during state restore/activation are satisfied by this initial setup.
    restart.store(false);dirty.store(false);
    main().instances.push_back(this);
}
void Instance::resize(int w,int h){if(!window)return;RECT r{0,0,std::clamp(w,100,8192),std::clamp(h,80,8192)};AdjustWindowRectEx(&r,GetWindowLongW(window,GWL_STYLE),FALSE,0);SetWindowPos(window,nullptr,0,0,r.right-r.left,r.bottom-r.top,SWP_NOMOVE|SWP_NOZORDER);}
void Instance::show(){
    if(editor){ShowWindow(window,SW_SHOW);SetForegroundWindow(window);return;}
    if(!window)window=CreateWindowExW(0,L"MiniDAWPluginEditor",wide(("MiniDAW · "+name).c_str()).c_str(),WS_OVERLAPPEDWINDOW,100,100,800,600,owner,nullptr,GetModuleHandle(nullptr),this);
    require(window!=nullptr,"Cannot create editor window");
    if(vst){require(controller!=nullptr,"Plugin has no editor");view=controller->createView(ViewType::kEditor);require(view!=nullptr,"Plugin has no native editor");require(view->isPlatformTypeSupported(kPlatformTypeHWND)==kResultTrue,"HWND editor unsupported");view->setFrame(&host);ViewRect r{};require(view->getSize(&r)==kResultOk,"Editor size unavailable");resize(r.right-r.left,r.bottom-r.top);require(view->attached(window,kPlatformTypeHWND)==kResultOk,"Editor attach failed");editor=true;}
    else{require(gui&&gui->is_api_supported(plugin,CLAP_WINDOW_API_WIN32,false),"CLAP embedded Win32 editor unavailable");require(gui->create(plugin,CLAP_WINDOW_API_WIN32,false),"CLAP editor creation failed");clap_window_t w{};w.api=CLAP_WINDOW_API_WIN32;w.win32=window;uint32_t x=800,y=600;gui->get_size(plugin,&x,&y);resize(x,y);require(gui->set_parent(plugin,&w),"CLAP editor parent failed");require(gui->show(plugin),"CLAP editor show failed");editor=true;}
    ShowWindow(window,SW_SHOW);SetForegroundWindow(window);
}
void Instance::hide(){if(view){view->removed();view->setFrame(nullptr);view->release();view=nullptr;}if(!vst&&gui&&editor){gui->hide(plugin);gui->destroy(plugin);}editor=false;if(window)ShowWindow(window,SW_HIDE);dirty.store(true);}
void Instance::restart_plugin(){
    if(!plugin||!quiescent.load())return;
    if(active){plugin->deactivate(plugin);active=false;}
    auto*ports=(const clap_plugin_audio_ports_t*)plugin->get_extension(plugin,CLAP_EXT_AUDIO_PORTS);
    require(ports&&ports->count(plugin,true)==(uint32_t)input_buses&&ports->count(plugin,false)==(uint32_t)output_buses,"Plugin bus structure changed; reload project");
    require(plugin->activate(plugin,rate,1,1),"CLAP reactivation failed");active=true;
    if(auto*l=(const clap_plugin_latency_t*)plugin->get_extension(plugin,CLAP_EXT_LATENCY))latency=l->get(plugin);
    if(auto*t=(const clap_plugin_tail_t*)plugin->get_extension(plugin,CLAP_EXT_TAIL))tail=t->get(plugin);
    restart.store(false);quiescent.store(false);
}
void Instance::poll(){
    if(fault.load())return;
    if(plugin&&restart.load()&&quiescent.load())restart_plugin();
    int req=window_request.exchange(0);if(req==1)show();else if(req==2)hide();auto size=resize_request.exchange(0);if(size)resize((int)(size>>32),(int)(size&0xffffffff));
    if(callback.exchange(false)&&plugin)plugin->on_main_thread(plugin);
    if(auto rev=latency_revision.load();rev!=latency_seen.load()){if(processor)latency=processor->getLatencySamples();else if(plugin)if(auto*l=(const clap_plugin_latency_t*)plugin->get_extension(plugin,CLAP_EXT_LATENCY))latency=l->get(plugin);latency_seen.store(rev);}
    if(metadata_changed.exchange(false)&&params){bool same=params->count(plugin)==parameters.size();for(uint32_t i=0;same&&i<parameters.size();i++){clap_param_info_t q{};same=params->get_info(plugin,i,&q)&&q.id==parameters[i].id&&q.min_value==parameters[i].min&&q.max_value==parameters[i].max;if(same)parameters[i].name=q.name;}if(!same){restart_reason=3;restart.store(true);}}
    if(timer){std::array<uint32_t,64> due{};size_t count=0;auto now=GetTickCount64();for(auto&[id,t]:timers)if(now>=t.next){t.next=now+t.period;due[count++]=id;}for(size_t i=0;i<count;i++)if(timers.count(due[i]))timer->on_timer(plugin,due[i]);}
    if(controller&&pending)for(size_t i=0;i<parameters.size();i++){double v=pending[i].load();if(v!=parameters[i].value){controller->setParamNormalized(parameters[i].id,v);parameters[i].value=v;}}
}
std::string Instance::save(){
    std::vector<unsigned char> b;
    if(vst){Stream s,c;require(component->getState(&s)==kResultOk,"VST3 getState failed");if(controller)controller->getState(&c);uint32_t n=(uint32_t)s.data.size();b.resize(4);memcpy(b.data(),&n,4);b.insert(b.end(),s.data.begin(),s.data.end());b.insert(b.end(),c.data.begin(),c.data.end());}
    else {require(state!=nullptr,"CLAP state extension unavailable");clap_ostream_t out{&b,[](const clap_ostream_t*s,const void*p,uint64_t n)->int64_t{auto&v=*(std::vector<unsigned char>*)s->ctx;if(n>LIMIT-v.size())return -1;v.insert(v.end(),(const unsigned char*)p,(const unsigned char*)p+n);return n;}};require(state->save(plugin,&out),"CLAP state save failed");}
    require(b.size()<=LIMIT,"Plugin state exceeds 32 MiB");dirty.store(false);return hex(b.data(),b.size());
}
std::string Instance::info(bool details){
    std::string s="{\"latency\":"+std::to_string(latency.load())+",\"tail\":"+std::to_string(tail)+",\"editorOpen\":"+(editor?"true":"false")+",\"dirty\":"+(dirty.load()?"true":"false")+",\"faulted\":"+(fault.load()?"true":"false")+",\"restartRequired\":"+(restart.load()?"true":"false")+",\"processCalls\":"+std::to_string(calls.load())+",\"droppedEvents\":"+std::to_string(dropped.load())+",\"parameters\":[";
    if(!details)return s+"],\"restartReason\":"+std::to_string(restart_reason.load())+"}";
    for(size_t i=0;i<parameters.size();i++){auto&p=parameters[i];if(params){clap_param_info_t q{};if(params->get_info(plugin,(uint32_t)i,&q)&&q.id==p.id)p.name=q.name;}else if(controller){ParameterInfo q{};if(controller->getParameterInfo((int32)i,q)==kResultOk&&q.id==p.id)p.name=utf8((wchar_t*)q.title);}
        double v=pending[i].load();if(params)params->get_value(plugin,p.id,&v);else if(controller)v=controller->getParamNormalized(p.id);if(i)s+=",";s+="{\"id\":"+std::to_string(p.id)+",\"name\":"+quote(p.name)+",\"min\":"+number(p.min)+",\"max\":"+number(p.max)+",\"value\":"+number(v)+",\"step\":"+number(p.step)+",\"stepped\":"+(p.stepped?"true":"false")+",\"readonly\":"+(p.readonly?"true":"false")+"}";}
    return s+"]}";
}
void Instance::parameter(uint32_t id,double value){
    if(vst)changes.set(id,value);
    else{clap_event_param_value_t e{};e.header={sizeof(e),0,CLAP_CORE_EVENT_SPACE_ID,CLAP_EVENT_PARAM_VALUE,0};e.param_id=id;e.note_id=-1;e.port_index=e.channel=e.key=-1;e.value=value;clap_event(e);}
}
void Instance::midi(uint8_t status,uint8_t a,uint8_t b,uint32_t voice){
    auto ch=status&15;auto kind=status&0xf0;
    if(vst){
        if(kind==0x90||kind==0x80){Event e{};e.busIndex=0;e.sampleOffset=0;e.type=kind==0x90&&b?Event::kNoteOnEvent:Event::kNoteOffEvent;if(e.type==Event::kNoteOnEvent)e.noteOn={(int16)ch,(int16)a,0,b/127.f,0,(int32)voice};else e.noteOff={(int16)ch,(int16)a,b/127.f,(int32)voice,0};if(events.addEvent(e)!=kResultOk)dropped++;
            if(e.type==Event::kNoteOnEvent){for(size_t i=0;i<held.size();i++)if(!held_used[i]){held[i]=e;held_used[i]=true;break;}}
            else for(size_t i=0;i<held.size();i++)if(held_used[i]&&held[i].noteOn.noteId==(int32)voice&&held[i].noteOn.channel==ch){held_used[i]=false;break;}}
        else {int control=kind==0xb0?a:kind==0xe0?128:kind==0xd0?129:-1;if(control>=0){auto id=midi_map[ch][control];if(id!=kNoParamId)parameter(id,kind==0xe0?(a+128*b)/16383.0:kind==0xd0?a/127.0:b/127.0);}}
    }else if(native_notes&&(kind==0x90||kind==0x80)){
        clap_event_note_t e{};e.header={sizeof(e),0,CLAP_CORE_EVENT_SPACE_ID,(uint16_t)(kind==0x90&&b?CLAP_EVENT_NOTE_ON:CLAP_EVENT_NOTE_OFF),0};e.note_id=(int32_t)voice;e.port_index=0;e.channel=ch;e.key=a;e.velocity=b/127.;clap_event(e);
    }else{clap_event_midi_t e{};e.header={sizeof(e),0,CLAP_CORE_EVENT_SPACE_ID,CLAP_EVENT_MIDI,0};e.port_index=0;e.data[0]=status;e.data[1]=a;e.data[2]=b;clap_event(e);}
}
void Instance::reset(){
    events.count=changes.count=clap_count=0;
    if(plugin&&processing&&!restart.load())plugin->reset(plugin);
    else {for(size_t i=0;i<held.size();i++)if(held_used[i]){auto n=held[i].noteOn;Event e{};e.busIndex=0;e.type=Event::kNoteOffEvent;e.noteOff={n.channel,n.pitch,0,n.noteId,0};events.addEvent(e);held_used[i]=false;}
        for(int c=0;c<16;c++){midi((uint8_t)(0xb0|c),64,0,0);midi((uint8_t)(0xb0|c),123,0,0);}}
}
void Instance::process(float*pair,int64_t frame,double bpm,int numerator,int denominator,bool playing){
    AudioRole role;
    if(fault.load()){pair[0]=pair[1]=0;return;}
    if(restart.load()){if(plugin){if(processing){plugin->stop_processing(plugin);processing=false;}quiescent.store(true);}if(instrument)pair[0]=pair[1]=0;return;}
    flush_pending();
    float input[2]{inputs==1?(pair[0]+pair[1])*.5f:pair[0],pair[1]},output[2]{};
    float*in[2]{&input[0],&input[1]},*out[2]{&output[0],&output[1]};
    if(vst){
        std::array<AudioBusBuffers,16> ib{},ob{};ib[0].numChannels=inputs;ib[0].channelBuffers32=in;ob[0].numChannels=outputs;ob[0].channelBuffers32=out;
        context.state=ProcessContext::kTempoValid|ProcessContext::kTimeSigValid|ProcessContext::kProjectTimeMusicValid|(playing?ProcessContext::kPlaying:0);
        context.sampleRate=rate;context.projectTimeSamples=frame;context.projectTimeMusic=frame/rate*bpm/60;context.tempo=bpm;context.timeSigNumerator=numerator;context.timeSigDenominator=denominator;
        ProcessData d{};d.processMode=offline?kOffline:kRealtime;d.symbolicSampleSize=kSample32;d.numSamples=1;d.numInputs=input_buses;d.numOutputs=output_buses;d.inputs=ib.data();d.outputs=ob.data();d.inputEvents=&events;d.outputEvents=&out_events;d.inputParameterChanges=&changes;d.outputParameterChanges=&out_changes;d.processContext=&context;
        if(processor->process(d)!=kResultOk)fault.store(true);
        events.count=changes.count=out_events.count=out_changes.count=0;
    }else{
        if(!processing){if(!plugin->start_processing(plugin)){fault.store(true);pair[0]=pair[1]=0;return;}processing=true;}
        std::array<clap_audio_buffer_t,16> ib{},ob{};std::array<std::array<float,2>,16> scratch_in{},scratch_out{};std::array<std::array<float*,2>,16> input_ptr{},output_ptr{};for(int i=0;i<input_buses;i++){input_ptr[i]={&scratch_in[i][0],&scratch_in[i][1]};ib[i]={i?input_ptr[i].data():in,nullptr,(uint32_t)input_channels[i],0,0};}for(int i=0;i<output_buses;i++){output_ptr[i]={&scratch_out[i][0],&scratch_out[i][1]};ob[i]={i?output_ptr[i].data():out,nullptr,(uint32_t)output_channels[i],0,0};}
        clap_input_events_t ein{this,event_count,event_at};clap_output_events_t eout{this,event_out};
        clap_event_transport_t t{};t.header={sizeof(t),0,CLAP_CORE_EVENT_SPACE_ID,CLAP_EVENT_TRANSPORT,0};t.flags=CLAP_TRANSPORT_HAS_TEMPO|CLAP_TRANSPORT_HAS_BEATS_TIMELINE|CLAP_TRANSPORT_HAS_SECONDS_TIMELINE|CLAP_TRANSPORT_HAS_TIME_SIGNATURE|(playing?CLAP_TRANSPORT_IS_PLAYING:0);t.song_pos_beats=(int64_t)(frame/rate*bpm/60*CLAP_BEATTIME_FACTOR);t.song_pos_seconds=(int64_t)(frame/rate*CLAP_SECTIME_FACTOR);t.tempo=bpm;t.tsig_num=(uint16_t)numerator;t.tsig_denom=(uint16_t)denominator;
        clap_process_t d{(int64_t)calls.load(),1,&t,ib.data(),ob.data(),(uint32_t)input_buses,(uint32_t)output_buses,&ein,&eout};if(plugin->process(plugin,&d)==CLAP_PROCESS_ERROR)fault.store(true);clap_count=0;
    }
    calls.fetch_add(1,std::memory_order_relaxed);pair[0]=std::isfinite(output[0])?output[0]:0;pair[1]=std::isfinite(output[outputs-1])?output[outputs-1]:0;
}
Instance::~Instance(){
    auto&v=main().instances;v.erase(std::remove(v.begin(),v.end(),this),v.end());hide();if(window)DestroyWindow(window);
    if(plugin){if(processing){AudioRole role;plugin->stop_processing(plugin);}if(active)plugin->deactivate(plugin);plugin->destroy(plugin);}
    if(processor&&processing)processor->setProcessing(false);if(component&&active)component->setActive(false);
    if(cp&&cc){cp->disconnect(cc);cc->disconnect(cp);}if(cp)cp->release();if(cc)cc->release();if(mapping)mapping->release();
    if(controller){controller->setComponentHandler(nullptr);if(!single_controller)controller->terminate();controller->release();}if(processor)processor->release();if(component){if(initialized)component->terminate();component->release();}if(factory)factory->release();
    library_owner.reset();
}
// SEH boundary contains hardware faults, not just C++ exceptions. Faulted modules
// remain quarantined until process exit; executing their destructors is unsafe.
int guarded(void(*fn)(void*),void*arg){__try{fn(arg);return 1;}__except(record_fault(GetExceptionInformation())){return 0;}}
template<class F>bool safe(F&& f){auto call=[](void*p){(*(F*)p)();};return guarded(call,&f)!=0;}
char* copy(const std::string&s){auto*p=(char*)malloc(s.size()+1);memcpy(p,s.c_str(),s.size()+1);return p;}
}
extern "C" {
void md_plugin_string_free(char*p){free(p);}
char* md_plugin_scan(const char*path,const char*format){
    return md::main().call([=]{std::string out;bool ok=md::safe([&]{try{out=md::scan(path,format);}catch(const std::exception&e){out="{\"error\":"+md::quote(e.what())+"}";}});return md::copy(ok?out:"{\"error\":\"Plugin scan crashed (SEH)\"}");});
}
void* md_plugin_create(const char*path,const char*format,const char*id,double rate,const char*state,const char*name,bool instrument,bool offline,char**error){
    return md::main().call([=]()->void*{auto*p=new md::Instance;p->name=name;p->instrument=instrument;p->offline=offline;bool success=false;bool ok=md::safe([&]{try{p->open(path,format,id,rate,state);success=true;}catch(const std::exception&e){*error=md::copy(e.what());}});if(!ok){p->fault=true;*error=md::copy(std::string("Plugin initialization crashed (SEH): ")+p->stage+" code="+std::to_string(md::fault_code)+" address="+std::to_string((uintptr_t)md::fault_address)+md::fault_trace());return nullptr;}if(!success){delete p;return nullptr;}return p;});
}
void md_plugin_destroy(void*ptr){auto*p=(md::Instance*)ptr;md::main().call([=]{if(!p->fault.load())md::safe([&]{delete p;});});}
// Latency publication is an atomic read on the processing thread.
bool md_plugin_latency_pending(void*ptr){auto*p=(md::Instance*)ptr;return p->latency_revision.load()!=p->latency_seen.load();}
void md_plugin_poll_latency(void*ptr){auto*p=(md::Instance*)ptr;md::main().call([p]{md::safe([&]{p->poll();});});}
uint32_t md_plugin_latency(void*ptr){return ((md::Instance*)ptr)->latency.load(std::memory_order_acquire);}
void md_plugin_prime(void*ptr){auto*p=(md::Instance*)ptr;md::main().call([=]{md::safe([&]{for(int i=0;i<64;i++){float x[2]{};p->process(x,0,120,4,4,false);}p->poll();if(p->processor)p->latency=p->processor->getLatencySamples();else if(p->plugin)if(auto*l=(const clap_plugin_latency_t*)p->plugin->get_extension(p->plugin,CLAP_EXT_LATENCY))p->latency=l->get(p->plugin);{md::AudioRole role;p->reset();}});});}
unsigned md_plugin_fault(void*ptr){auto*p=(md::Instance*)ptr;return p->fault.load()?1:p->restart.load()?2:0;}
bool md_plugin_dirty(void*ptr){return ((md::Instance*)ptr)->dirty.load();}
char* md_plugin_status(void*ptr){auto*p=(md::Instance*)ptr;return md::main().call([=]{return md::copy(p->info(false));});}
char* md_plugin_info(void*ptr){auto*p=(md::Instance*)ptr;return md::main().call([=]{std::string s;bool ok=md::safe([&]{s=p->info();});return md::copy(ok?s:"{\"error\":\"Plugin info failed\"}");});}
char* md_plugin_state(void*ptr){auto*p=(md::Instance*)ptr;return md::main().call([=]{std::string s;bool ok=md::safe([&]{try{s="{\"state\":"+md::quote(p->save())+"}";}catch(const std::exception&e){s="{\"error\":"+md::quote(e.what())+"}";}});return md::copy(ok?s:"{\"error\":\"Plugin state failed\"}");});}
char* md_plugin_editor(void*ptr,bool show,uintptr_t owner){auto*p=(md::Instance*)ptr;return md::main().call([=]{std::string s="{}";bool ok=md::safe([&]{try{p->owner=(HWND)owner;if(p->window)SetWindowLongPtrW(p->window,GWLP_HWNDPARENT,(LONG_PTR)p->owner);if(show)p->show();else p->hide();}catch(const std::exception&e){s="{\"error\":"+md::quote(e.what())+"}";}});return md::copy(ok?s:"{\"error\":\"Plugin editor failed\"}");});}
bool md_plugin_process(void*ptr,float*pair,int64_t frame,double bpm,int numerator,int denominator,bool playing){
    auto*p=(md::Instance*)ptr;bool ok=md::safe([&]{try{
        if(p->offline&&p->plugin&&p->restart.load()) { {md::AudioRole role;if(p->processing){p->plugin->stop_processing(p->plugin);p->processing=false;}p->quiescent.store(true);}md::main().call([p]{p->restart_plugin();}); }
        p->process(pair,frame,bpm,numerator,denominator,playing);}catch(...){p->fault=true;}});if(!ok||p->fault.load()){p->fault=true;pair[0]=pair[1]=0;}return !p->fault.load()&&(!p->restart.load()||p->plugin);
}
void md_plugin_midi(void*ptr,uint8_t s,uint8_t a,uint8_t b,uint32_t voice){auto*p=(md::Instance*)ptr;p->flush_pending();p->midi(s,a,b,voice);}
void md_plugin_initial_parameter(void*ptr,uint32_t id,double v){auto*p=(md::Instance*)ptr;p->changed_param(id,v);p->dirty.store(false);}
void md_plugin_parameter(void*ptr,uint32_t id,double v){auto*p=(md::Instance*)ptr;p->flush_pending();p->parameter(id,v);}
void md_plugin_reset(void*ptr){auto*p=(md::Instance*)ptr;md::AudioRole role;if(!md::safe([&]{p->reset();}))p->fault=true;}
}
