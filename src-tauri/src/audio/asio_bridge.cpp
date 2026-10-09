// Application glue only; the SDK stays outside the repository and bundle.
#include "asiosys.h"
#include "asio.h"

static void (*rate_changed)() = nullptr;
static void on_rate_changed(ASIOSampleRate) { rate_changed(); }
static ASIOCallbacks callbacks;
extern "C" long minidaw_asio_create(ASIOBufferInfo* buffers, long channels, long frames,
    void (*buffer)(long, ASIOBool), void (*rate)(),
    long (*message)(long, long, void*, double*),
    ASIOTime* (*time)(ASIOTime*, long, ASIOBool)) {
    rate_changed = rate;
    callbacks.bufferSwitch = buffer;
    callbacks.sampleRateDidChange = on_rate_changed;
    callbacks.asioMessage = message;
    callbacks.bufferSwitchTimeInfo = time;
    return ASIOCreateBuffers(buffers, channels, frames, &callbacks);
}
extern "C" long minidaw_asio_panel() { return ASIOControlPanel(); }
extern "C" long minidaw_asio_latencies(long* input, long* output) { return ASIOGetLatencies(input, output); }
