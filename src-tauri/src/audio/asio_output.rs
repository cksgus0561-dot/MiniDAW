//! CPAL ASIO host/device initialization, output-only SDK callback adapter.
//! CPAL 0.16/asio-sys' stock dispatcher takes blocking mutexes in the callback.
//! This adapter never registers with that dispatcher. Buffers/context are stable
//! from ASIOCreateBuffers to ASIOStop + ASIODisposeBuffers; teardown is non-RT.
use super::{output::OutputInfo, renderer::Renderer};
use crate::error::{AppError, AppResult};
use asio_sys::bindings::asio_import as ai;
use cpal::FromSample;
use std::{
    ffi::c_void,
    ptr,
    sync::{
        atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    thread,
};

unsafe extern "C" {
    fn minidaw_asio_create(
        buffers: *mut ai::ASIOBufferInfo,
        channels: i32,
        frames: i32,
        buffer: unsafe extern "C" fn(i32, i32),
        rate: unsafe extern "C" fn(),
        message: unsafe extern "C" fn(i32, i32, *mut c_void, *mut f64) -> i32,
        time: unsafe extern "C" fn(*mut ai::ASIOTime, i32, i32) -> *mut ai::ASIOTime,
    ) -> i32;
    fn minidaw_asio_panel() -> i32;
    fn minidaw_asio_latencies(input: *mut i32, output: *mut i32) -> i32;
}

static CONTEXT: AtomicPtr<Context> = AtomicPtr::new(ptr::null_mut());
static IN_CALLBACK: AtomicUsize = AtomicUsize::new(0);
static NEEDS_RECONNECT: AtomicBool = AtomicBool::new(false);
static OWNED: AtomicBool = AtomicBool::new(false);

// Acquire BEFORE CPAL driver loading: the SDK has process-global driver state.
// Release only after CPAL's device/host have been destroyed, including failures.
pub(crate) struct AsioSlot;
impl AsioSlot {
    pub(crate) fn acquire() -> AppResult<Self> {
        OWNED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| {
                AppError::new(
                    "asio_busy",
                    "ASIO 출력은 한 번에 하나만 사용할 수 있습니다.",
                )
            })
    }
}
impl Drop for AsioSlot {
    fn drop(&mut self) {
        OWNED.store(false, Ordering::Release);
    }
}

struct Context {
    renderer: Renderer,
    interleaved: Box<[f32]>,
    buffers: Box<[ai::ASIOBufferInfo]>,
    formats: Box<[Format]>,
    frames: usize,
    output_ready: bool,
    errors: Arc<AtomicU64>,
    metrics: Arc<super::metrics::AudioMetrics>,
    rate: u32,
}

#[derive(Clone, Copy)]
struct Format {
    kind: i32,
    bytes: usize,
    big_endian: bool,
}
impl Format {
    fn from_asio(kind: i32) -> AppResult<Self> {
        let base = kind & !16;
        let bytes = match base {
            0 => 2,
            1 => 3,
            2 | 3 => 4,
            4 => 8,
            _ => {
                return Err(AppError::new(
                    "asio_format",
                    "ASIO 출력 샘플 형식을 지원하지 않습니다.",
                )
                .detail(kind))
            }
        };
        if !(0..=4).contains(&kind) && !(16..=20).contains(&kind) {
            return Err(
                AppError::new("asio_format", "ASIO 출력 샘플 형식을 지원하지 않습니다.")
                    .detail(kind),
            );
        }
        Ok(Self {
            kind: base,
            bytes,
            big_endian: kind < 16,
        })
    }
    fn encode(self, sample: f32) -> [u8; 8] {
        let mut bytes = [0; 8];
        match self.kind {
            0 => bytes[..2].copy_from_slice(&i16::from_sample_(sample).to_le_bytes()),
            1 => bytes[..3].copy_from_slice(&(i32::from_sample_(sample) >> 8).to_le_bytes()[..3]),
            2 => bytes[..4].copy_from_slice(&i32::from_sample_(sample).to_le_bytes()),
            3 => bytes[..4].copy_from_slice(&sample.to_le_bytes()),
            4 => bytes.copy_from_slice(&f64::from(sample).to_le_bytes()),
            _ => unreachable!(), // validated before driver start
        }
        if self.big_endian {
            bytes[..self.bytes].reverse();
        }
        bytes
    }
}

pub struct AsioOutput {
    context: Box<Context>,
    prepared: bool,
    started: bool,
    // Explicit drop order: callback stopped and buffers disposed BEFORE device/host.
    _device: cpal::Device,
    _host: cpal::Host,
    _slot: AsioSlot,
}

fn check(code: i32, operation: &str) -> AppResult<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(AppError::new("asio", "선택한 ASIO 드라이버를 초기화할 수 없습니다. 오디오 설정에서 WASAPI Shared를 선택할 수 있습니다.").detail(format!("{operation}: ASIO error {code}")))
    }
}

impl AsioOutput {
    pub(crate) fn open(
        device: cpal::Device,
        host: cpal::Host,
        renderer: Renderer,
        errors: Arc<AtomicU64>,
        info: &mut OutputInfo,
        slot: AsioSlot,
    ) -> AppResult<Self> {
        // A guard releases the single-instance slot even if setup fails.
        let metrics = renderer.metrics();
        let mut stream = Self {
            context: Box::new(Context {
                renderer,
                interleaved: Box::new([]),
                buffers: Box::new([]),
                formats: Box::new([]),
                frames: 0,
                output_ready: false,
                errors,
                metrics,
                rate: info.sample_rate,
            }),
            prepared: false,
            started: false,
            _device: device,
            _host: host,
            _slot: slot,
        };
        NEEDS_RECONNECT.store(false, Ordering::Release);
        let (mut min, mut max, mut preferred, mut granularity) = (0, 0, 0, 0);
        // SAFETY: CPAL has initialized the selected driver on this same controller.
        unsafe {
            check(
                ai::ASIOGetBufferSize(&mut min, &mut max, &mut preferred, &mut granularity),
                "buffer size",
            )?;
        }
        if preferred <= 0 || preferred > 1_048_576 || info.channels == 0 || info.channels > 256 {
            return Err(AppError::new(
                "asio_buffer",
                "ASIO 드라이버가 유효하지 않은 버퍼/채널 수를 반환했습니다.",
            ));
        }
        let frames = preferred as usize;
        let channels = info.channels as usize;
        let mut formats = Vec::with_capacity(channels);
        for channel in 0..channels {
            let mut channel_info: ai::ASIOChannelInfo = unsafe { std::mem::zeroed() };
            channel_info.channel = channel as i32;
            channel_info.isInput = 0;
            unsafe {
                check(ai::ASIOGetChannelInfo(&mut channel_info), "channel info")?;
            }
            formats.push(Format::from_asio(channel_info.type_)?);
        }
        stream.context.frames = frames;
        stream.context.formats = formats.into_boxed_slice();
        stream.context.interleaved = vec![0.0; frames * channels].into_boxed_slice();
        stream.context.buffers = (0..channels)
            .map(|channel| ai::ASIOBufferInfo {
                isInput: 0,
                channelNum: channel as i32,
                buffers: [ptr::null_mut(); 2],
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        unsafe {
            check(
                minidaw_asio_create(
                    stream.context.buffers.as_mut_ptr(),
                    channels as i32,
                    preferred,
                    buffer_switch,
                    rate_changed,
                    message,
                    time_switch,
                ),
                "create output buffers",
            )?;
            stream.prepared = true;
            for (buffer, format) in stream
                .context
                .buffers
                .iter()
                .zip(stream.context.formats.iter())
            {
                let pointers = buffer.buffers;
                for pointer in pointers {
                    if pointer.is_null() {
                        return Err(AppError::new(
                            "asio_buffer",
                            "ASIO 출력 버퍼가 비어 있습니다.",
                        ));
                    }
                    ptr::write_bytes(pointer.cast::<u8>(), 0, frames * format.bytes);
                }
            }
            let (mut input, mut output) = (0, 0);
            if minidaw_asio_latencies(&mut input, &mut output) == 0 {
                info.driver_input_latency_frames = Some(input);
                info.driver_output_latency_frames = Some(output);
            }
            info.control_panel = true;
            stream.context.output_ready = ai::ASIOOutputReady() == 0;
            CONTEXT.store(&mut *stream.context, Ordering::Release);
            check(ai::ASIOStart(), "start")?;
            stream.started = true;
        }
        Ok(stream)
    }
}

impl Drop for AsioOutput {
    fn drop(&mut self) {
        // SDK contract: Stop completes pending buffer processing. The counter also
        // drains callbacks that entered before pointer detachment. Late calls see null.
        unsafe {
            if self.started {
                ai::ASIOStop();
            }
        }
        CONTEXT.store(ptr::null_mut(), Ordering::SeqCst);
        while IN_CALLBACK.load(Ordering::SeqCst) != 0 {
            thread::yield_now();
        }
        unsafe {
            if self.prepared {
                ai::ASIODisposeBuffers();
            }
        }
    }
}

pub fn control_panel() -> AppResult<()> {
    // Controller only; no SDK state is freed/reallocated here. Reset/rate messages
    // atomically silence output until the UI explicitly reconnects after the panel.
    let result = unsafe { minidaw_asio_panel() };
    if result == 0 {
        Ok(())
    } else {
        Err(AppError::new(
            "asio_panel",
            "이 ASIO 드라이버의 Control Panel을 열 수 없습니다.",
        )
        .detail(result))
    }
}

unsafe extern "C" fn rate_changed() {
    NEEDS_RECONNECT.store(true, Ordering::Release);
}
unsafe extern "C" fn message(selector: i32, value: i32, _: *mut c_void, _: *mut f64) -> i32 {
    match selector {
        1 => i32::from(matches!(value, 2..=7)), // selectorSupported
        2 => 2,                                 // engine version
        3..=6 => {
            NEEDS_RECONNECT.store(true, Ordering::Release);
            1
        }
        7 => 1, // supportsTimeInfo
        _ => 0,
    }
}
unsafe extern "C" fn time_switch(
    time: *mut ai::ASIOTime,
    index: i32,
    direct: i32,
) -> *mut ai::ASIOTime {
    unsafe {
        buffer_switch(index, direct);
    }
    time
}
unsafe extern "C" fn buffer_switch(index: i32, _: i32) {
    let started = std::time::Instant::now();
    // No waiting, locks, dynamic ownership changes, or allocation on this path.
    let entered = IN_CALLBACK.fetch_add(1, Ordering::SeqCst);
    if entered != 0 {
        NEEDS_RECONNECT.store(true, Ordering::Release);
        IN_CALLBACK.fetch_sub(1, Ordering::SeqCst);
        return;
    }
    let pointer = CONTEXT.load(Ordering::SeqCst);
    if !pointer.is_null() && (0..=1).contains(&index) {
        // Exclusive callback, stable box until counter is drained on control thread.
        let context = unsafe { &mut *pointer };
        if NEEDS_RECONNECT.load(Ordering::Acquire) {
            context.errors.store(1, Ordering::Relaxed);
            context.interleaved.fill(0.0);
        } else {
            context.renderer.render(&mut context.interleaved);
        }
        let channels = context.buffers.len();
        for (channel, (buffer, format)) in context
            .buffers
            .iter()
            .zip(context.formats.iter())
            .enumerate()
        {
            let pointers = buffer.buffers;
            let target = pointers[index as usize].cast::<u8>();
            for frame in 0..context.frames {
                let bytes = format.encode(context.interleaved[frame * channels + channel]);
                unsafe {
                    ptr::copy_nonoverlapping(
                        bytes.as_ptr(),
                        target.add(frame * format.bytes),
                        format.bytes,
                    );
                }
            }
        }
        if context.output_ready {
            unsafe {
                ai::ASIOOutputReady();
            }
        }
        context.metrics.record_device_callback(
            started.elapsed().as_nanos() as u64,
            context.frames,
            context.rate,
        );
    }
    IN_CALLBACK.fetch_sub(1, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn driver_slot_rejects_overlap_and_recovers_after_failure() {
        let slot = AsioSlot::acquire().unwrap();
        assert!(AsioSlot::acquire().is_err());
        drop(slot);
        assert!(AsioSlot::acquire().is_ok());
    }
    #[test]
    fn native_formats_endianness_and_silence() {
        for kind in [0, 1, 2, 3, 4, 16, 17, 18, 19, 20] {
            let format = Format::from_asio(kind).unwrap();
            assert_eq!(format.encode(0.0), [0; 8]);
            let little = Format::from_asio(kind | 16).unwrap();
            let mut bytes = little.encode(0.25);
            if kind < 16 {
                bytes[..format.bytes].reverse();
            }
            assert_eq!(format.encode(0.25), bytes);
        }
        assert!(Format::from_asio(24).is_err());
    }
}
