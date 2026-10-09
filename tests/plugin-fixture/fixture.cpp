// Test-only, deterministic impulse/delay plugins. Never linked into MiniDAW.
#define NOMINMAX
#define INIT_CLASS_IID
#include <windows.h>
#include <atomic>
#include <array>
#include <algorithm>
#include <cstring>
#include <cmath>
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/base/ibstream.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivstevents.h"
#include "pluginterfaces/vst/ivstparameterchanges.h"
#include <clap/clap.h>
using namespace Steinberg;using namespace Steinberg::Vst;
static std::atomic<int> forced[2]{{-1},{-1}};
extern "C" __declspec(dllexport) void pdc_test_latency(int instrument,int samples){forced[instrument?1:0]=samples;}
struct Engine{
 bool instrument;std::atomic<uint32_t> latency;std::atomic<double> gain{.25};std::array<std::array<float,2>,8193> line{};size_t at=0,valid=0;bool changed=false;float note=0;
 explicit Engine(bool i):instrument(i),latency(i?127:13){}
 void reset(){at=valid=0;note=0;}
 void parameter(uint32_t id,double v){if(id==0){auto n=(uint32_t)std::clamp(std::round(v),0.,8192.);if(latency.exchange(n)!=n)changed=true;}else if(id==1)gain=std::clamp(v,-1.,1.);}
 void sample(float*l,float*r){int f=forced[instrument?1:0];if(f>=0)parameter(0,f);uint32_t n=latency;std::array<float,2>x=instrument?std::array<float,2>{note,note}:std::array<float,2>{*l,*r};note=0;auto y=n==0?x:valid>=n?line[(at+line.size()-n)%line.size()]:std::array<float,2>{0,0};line[at]=x;at=(at+1)%line.size();valid=std::min(valid+1,line.size());*l=y[0]*(float)gain;*r=y[1]*(float)gain;}
};
static const char* fx_features[]={CLAP_PLUGIN_FEATURE_AUDIO_EFFECT,nullptr};static const char* ins_features[]={CLAP_PLUGIN_FEATURE_INSTRUMENT,nullptr};
static const clap_plugin_descriptor_t descs[]={{CLAP_VERSION,"minidaw.pdc.effect","PDC Test Effect","MiniDAW","","","","1","Test fixture",fx_features},{CLAP_VERSION,"minidaw.pdc.instrument","PDC Test Instrument","MiniDAW","","","","1","Test fixture",ins_features}};
struct Clap {clap_plugin_t api{};const clap_host_t*host;Engine e;Clap(const clap_host_t*h,bool i):host(h),e(i){} };
Clap* cp(const clap_plugin_t*p){return (Clap*)p->plugin_data;}
static clap_plugin_audio_ports_t ports{[](const clap_plugin_t*p,bool in)->uint32_t{return in&&cp(p)->e.instrument?0:1;},[](const clap_plugin_t*,uint32_t n,bool,clap_audio_port_info_t*i)->bool{if(n)return false;*i={};i->id=0;i->flags=CLAP_AUDIO_PORT_IS_MAIN;i->channel_count=2;i->port_type=CLAP_PORT_STEREO;i->in_place_pair=CLAP_INVALID_ID;return true;}};
static clap_plugin_note_ports_t notes{[](const clap_plugin_t*p,bool in)->uint32_t{return cp(p)->e.instrument&&in?1:0;},[](const clap_plugin_t*,uint32_t n,bool,clap_note_port_info_t*i)->bool{if(n)return false;*i={};i->id=0;i->supported_dialects=i->preferred_dialect=CLAP_NOTE_DIALECT_MIDI;return true;}};
static clap_plugin_params_t params{[](const clap_plugin_t*)->uint32_t{return 2;},[](const clap_plugin_t*,uint32_t n,clap_param_info_t*i)->bool{if(n>1)return false;*i={};i->id=n;i->flags=CLAP_PARAM_IS_AUTOMATABLE|(n==0?CLAP_PARAM_IS_STEPPED:0);strcpy_s(i->name,n?"Gain":"Latency samples");i->min_value=n?-1:0;i->max_value=n?1:8192;i->default_value=n?.25:13;return true;},[](const clap_plugin_t*p,clap_id id,double*v)->bool{*v=id?cp(p)->e.gain.load():cp(p)->e.latency.load();return id<2;},[](const clap_plugin_t*,clap_id,double,char*,uint32_t)->bool{return false;},[](const clap_plugin_t*,clap_id,const char*,double*)->bool{return false;},[](const clap_plugin_t*,const clap_input_events_t*,const clap_output_events_t*){}};
static clap_plugin_latency_t lat{[](const clap_plugin_t*p)->uint32_t{return cp(p)->e.latency;}};
static clap_plugin_tail_t tail{[](const clap_plugin_t*)->uint32_t{return 0;}};
static clap_plugin_state_t state{[](const clap_plugin_t*p,const clap_ostream_t*s)->bool{double v[2]{(double)cp(p)->e.latency.load(),cp(p)->e.gain.load()};return s->write(s,v,sizeof v)==sizeof v;},[](const clap_plugin_t*p,const clap_istream_t*s)->bool{double v[2]{};if(s->read(s,v,sizeof v)!=sizeof v)return false;cp(p)->e.parameter(0,v[0]);cp(p)->e.parameter(1,v[1]);return true;}};
const void* CLAP_ABI ext(const clap_plugin_t*,const char*i){if(!strcmp(i,CLAP_EXT_AUDIO_PORTS))return &ports;if(!strcmp(i,CLAP_EXT_NOTE_PORTS))return &notes;if(!strcmp(i,CLAP_EXT_PARAMS))return &params;if(!strcmp(i,CLAP_EXT_LATENCY))return &lat;if(!strcmp(i,CLAP_EXT_TAIL))return &tail;if(!strcmp(i,CLAP_EXT_STATE))return &state;return nullptr;}
static clap_plugin_factory_t factory{[](const clap_plugin_factory_t*)->uint32_t{return 2;},[](const clap_plugin_factory_t*,uint32_t n)->const clap_plugin_descriptor_t*{return n<2?descs+n:nullptr;},[](const clap_plugin_factory_t*,const clap_host_t*h,const char*id)->const clap_plugin_t*{int i=!strcmp(id,descs[1].id);auto*p=new Clap(h,i);p->api={descs+i,p,[](const clap_plugin_t*)->bool{return true;},[](const clap_plugin_t*p){delete cp(p);},[](const clap_plugin_t*p,double,uint32_t,uint32_t)->bool{cp(p)->e.reset();return true;},[](const clap_plugin_t*){},[](const clap_plugin_t*)->bool{return true;},[](const clap_plugin_t*){},[](const clap_plugin_t*p){cp(p)->e.reset();},[](const clap_plugin_t*p,const clap_process_t*d)->clap_process_status{auto*c=cp(p);uint32_t ei=0;for(uint32_t f=0;f<d->frames_count;f++){while(ei<d->in_events->size(d->in_events)){auto*v=d->in_events->get(d->in_events,ei);if(v->time>f)break;ei++;if(v->type==CLAP_EVENT_PARAM_VALUE){auto*q=(const clap_event_param_value_t*)v;c->e.parameter(q->param_id,q->value);}if(v->type==CLAP_EVENT_MIDI){auto*q=(const clap_event_midi_t*)v;if((q->data[0]&0xf0)==0x90)c->e.note=q->data[2]/127.f;}}
float l=d->audio_inputs_count?d->audio_inputs[0].data32[0][f]:0,r=d->audio_inputs_count?d->audio_inputs[0].data32[1][f]:0;c->e.sample(&l,&r);d->audio_outputs[0].data32[0][f]=l;d->audio_outputs[0].data32[1][f]=r;}if(c->e.changed){c->e.changed=false;auto*l=(const clap_host_latency_t*)c->host->get_extension(c->host,CLAP_EXT_LATENCY);if(l)l->changed(c->host);}return CLAP_PROCESS_CONTINUE;},ext,[](const clap_plugin_t*){}};return &p->api;}};
extern "C" __declspec(dllexport) const clap_plugin_entry_t clap_entry={CLAP_VERSION,[](const char*)->bool{return true;},[](){},[](const char*id)->const void*{return !strcmp(id,CLAP_PLUGIN_FACTORY_ID)?&factory:nullptr;}};
static FUID ids[]={FUID(0x50444300,0x12345678,0xAABBCCDD,1),FUID(0x50444300,0x12345678,0xAABBCCDD,2)};
struct TestVst:IComponent,IAudioProcessor,IEditController {
 std::atomic<uint32>refs{1};Engine e;IComponentHandler*handler=nullptr;explicit TestVst(bool i):e(i){}
 tresult PLUGIN_API queryInterface(const TUID id,void**o)override{*o=nullptr;
 if(FUnknownPrivate::iidEqual(id,IComponent::iid)||FUnknownPrivate::iidEqual(id,FUnknown::iid))*o=static_cast<IComponent*>(this);else if(FUnknownPrivate::iidEqual(id,IAudioProcessor::iid))*o=static_cast<IAudioProcessor*>(this);else if(FUnknownPrivate::iidEqual(id,IEditController::iid))*o=static_cast<IEditController*>(this);if(*o){addRef();return kResultOk;}return kNoInterface;}
 uint32 PLUGIN_API addRef()override{return ++refs;}uint32 PLUGIN_API release()override{auto n=--refs;if(!n)delete this;return n;}
 tresult PLUGIN_API initialize(FUnknown*)override{return kResultOk;}tresult PLUGIN_API terminate()override{return kResultOk;}
 tresult PLUGIN_API getControllerClassId(TUID)override{return kResultFalse;}tresult PLUGIN_API setIoMode(IoMode)override{return kResultOk;}
 int32 PLUGIN_API getBusCount(MediaType t,BusDirection d)override{return t==kEvent?(e.instrument&&d==kInput?1:0):d==kInput&&e.instrument?0:1;}
 tresult PLUGIN_API getBusInfo(MediaType t,BusDirection d,int32 n,BusInfo&i)override{if(n)return kResultFalse;i={};i.mediaType=t;i.direction=d;i.channelCount=t==kEvent?16:2;i.busType=kMain;i.flags=BusInfo::kDefaultActive;return kResultOk;}
 tresult PLUGIN_API getRoutingInfo(RoutingInfo&,RoutingInfo&)override{return kNotImplemented;}tresult PLUGIN_API activateBus(MediaType,BusDirection,int32,TBool)override{return kResultOk;}
 tresult PLUGIN_API setActive(TBool on)override{if(on)e.reset();return kResultOk;}
 tresult PLUGIN_API setState(IBStream*s)override{double v[2]{};if(s->read(v,sizeof v)!=kResultOk)return kResultFalse;e.parameter(0,v[0]);e.parameter(1,v[1]);return kResultOk;}
 tresult PLUGIN_API getState(IBStream*s)override{double v[2]{(double)e.latency.load(),e.gain.load()};return s->write(v,sizeof v);}
 tresult PLUGIN_API setBusArrangements(SpeakerArrangement*,int32,SpeakerArrangement*,int32)override{return kResultOk;}tresult PLUGIN_API getBusArrangement(BusDirection,int32,SpeakerArrangement&a)override{a=SpeakerArr::kStereo;return kResultOk;}
 tresult PLUGIN_API canProcessSampleSize(int32 n)override{return n==kSample32?kResultOk:kResultFalse;}uint32 PLUGIN_API getLatencySamples()override{return e.latency;}
 tresult PLUGIN_API setupProcessing(ProcessSetup&)override{return kResultOk;}tresult PLUGIN_API setProcessing(TBool)override{return kResultOk;}uint32 PLUGIN_API getTailSamples()override{return 0;}
 tresult PLUGIN_API process(ProcessData&d)override{for(int f=0;f<d.numSamples;f++){if(d.inputParameterChanges)for(int i=0;i<d.inputParameterChanges->getParameterCount();i++){auto*q=d.inputParameterChanges->getParameterData(i);int32 at;double v;if(q->getPoint(q->getPointCount()-1,at,v)==kResultOk)e.parameter(q->getParameterId(),q->getParameterId()==0?v*8192:v*2-1);}if(d.inputEvents)for(int i=0;i<d.inputEvents->getEventCount();i++){Event v{};d.inputEvents->getEvent(i,v);if(v.sampleOffset==f&&v.type==Event::kNoteOnEvent)e.note=v.noteOn.velocity;}
 float l=d.numInputs?d.inputs[0].channelBuffers32[0][f]:0,r=d.numInputs?d.inputs[0].channelBuffers32[1][f]:0;e.sample(&l,&r);d.outputs[0].channelBuffers32[0][f]=l;d.outputs[0].channelBuffers32[1][f]=r;}if(e.changed){e.changed=false;if(handler)handler->restartComponent(kLatencyChanged);}return kResultOk;}
 tresult PLUGIN_API setComponentState(IBStream*s)override{return setState(s);}int32 PLUGIN_API getParameterCount()override{return 2;}
 tresult PLUGIN_API getParameterInfo(int32 i,ParameterInfo&p)override{if(i>1)return kResultFalse;p={};p.id=i;memcpy(p.title,i?u"Gain":u"Latency samples",i?10:32);p.flags=ParameterInfo::kCanAutomate;p.stepCount=i?0:8192;return kResultOk;}
 tresult PLUGIN_API getParamStringByValue(ParamID,ParamValue,String128)override{return kNotImplemented;}tresult PLUGIN_API getParamValueByString(ParamID,TChar*,ParamValue&)override{return kNotImplemented;}
 ParamValue PLUGIN_API normalizedParamToPlain(ParamID id,ParamValue v)override{return id?v*2-1:v*8192;}ParamValue PLUGIN_API plainParamToNormalized(ParamID id,ParamValue v)override{return id?(v+1)/2:v/8192;}
 ParamValue PLUGIN_API getParamNormalized(ParamID id)override{return id?(e.gain.load()+1)/2:e.latency.load()/8192.;}tresult PLUGIN_API setParamNormalized(ParamID,ParamValue)override{return kResultOk;}
 tresult PLUGIN_API setComponentHandler(IComponentHandler*h)override{handler=h;return kResultOk;}IPlugView* PLUGIN_API createView(FIDString)override{return nullptr;}
};
struct Factory:IPluginFactory2{
 tresult PLUGIN_API queryInterface(const TUID id,void**o)override{*o=nullptr;if(FUnknownPrivate::iidEqual(id,IPluginFactory2::iid)||FUnknownPrivate::iidEqual(id,IPluginFactory::iid)||FUnknownPrivate::iidEqual(id,FUnknown::iid)){*o=this;return kResultOk;}return kNoInterface;}
 uint32 PLUGIN_API addRef()override{return 1;}uint32 PLUGIN_API release()override{return 1;}
 tresult PLUGIN_API getFactoryInfo(PFactoryInfo*i)override{*i={};strcpy_s(i->vendor,"MiniDAW");return kResultOk;}int32 PLUGIN_API countClasses()override{return 2;}
 tresult PLUGIN_API getClassInfo(int32 n,PClassInfo*i)override{if(n>1)return kResultFalse;*i={};memcpy(i->cid,ids[n].toTUID(),16);i->cardinality=PClassInfo::kManyInstances;strcpy_s(i->category,kVstAudioEffectClass);strcpy_s(i->name,n?"PDC Test Instrument":"PDC Test Effect");return kResultOk;}
 tresult PLUGIN_API getClassInfo2(int32 n,PClassInfo2*i)override{if(n>1)return kResultFalse;*i={};memcpy(i->cid,ids[n].toTUID(),16);strcpy_s(i->category,kVstAudioEffectClass);strcpy_s(i->subCategories,n?"Instrument|Synth":"Fx");strcpy_s(i->name,n?"PDC Test Instrument":"PDC Test Effect");return kResultOk;}
 tresult PLUGIN_API createInstance(FIDString cid,FIDString iid,void**o)override{for(int i=0;i<2;i++)if(FUnknownPrivate::iidEqual(cid,ids[i].toTUID())){auto*p=new TestVst(i);auto r=p->queryInterface(iid,o);p->release();return r;}*o=nullptr;return kResultFalse;}
};
extern "C" __declspec(dllexport) IPluginFactory* GetPluginFactory(){static Factory f;return &f;}
