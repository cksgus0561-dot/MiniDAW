// Worker-only, exception-contained phase-vocoder pitch/time processing.
#include "signalsmith-stretch.h"
using Stretch = signalsmith::stretch::SignalsmithStretch<double>;
struct Input {
    const float *p;
    struct Channel { const float *p; float operator[](int i) const { return p[2*i]; } };
    Channel operator[](int c) const { return {p+c}; }
};
struct Output {
    float *p;
    struct Channel { float *p; float &operator[](int i) const { return p[2*i]; } };
    Channel operator[](int c) const { return {p+c}; }
};
extern "C" {
void *minidaw_stretch_new(unsigned rate, bool precise) noexcept {
    try { auto *s=new Stretch(0); try {s->configure(2,int(rate*0.16),int(rate*0.02));s->setPreciseTimeCompensation(precise);} catch (...) {delete s;throw;} return s; } catch (...) {return nullptr;}
}
void minidaw_stretch_delete(void *p) noexcept {delete static_cast<Stretch *>(p);}
int minidaw_stretch_prime_length(void *p, double rate) noexcept {return static_cast<Stretch *>(p)->outputSeekLength(rate);}
bool minidaw_stretch_seek(void *p,const float *in,int n,double rate) noexcept {
    try {static_cast<Stretch *>(p)->outputSeek(Input{in},n);return true;}catch(...){return false;}
}
bool minidaw_stretch_process(void *p,const float *in,int ni,float *out,int no) noexcept {
    try {static_cast<Stretch *>(p)->process(Input{in},ni,Output{out},no);return true;}catch(...){return false;}
}
bool minidaw_stretch_flush(void *p,float *out,int n,float rate) noexcept {
    try {static_cast<Stretch *>(p)->flush(Output{out},n,rate);return true;}catch(...){return false;}
}
}
