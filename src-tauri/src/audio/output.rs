use super::{
    metrics::AudioMetrics,
    renderer::{Command, Renderer, COMMAND_CAPACITY},
    transport::TransportCell,
};
use crate::error::{AppError, AppResult};
use crate::preferences::{AudioPreferences, DriverType};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rtrb::{Producer, RingBuffer};
use serde::Serialize;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputInfo {
    pub backend: String,
    pub sample_format: String,
    pub device: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub requested_buffer: Option<u32>,
    pub supported_buffer_min: Option<u32>,
    pub supported_buffer_max: Option<u32>,
    pub driver_type: DriverType,
    pub driver_input_latency_frames: Option<i32>,
    pub driver_output_latency_frames: Option<i32>,
    pub control_panel: bool,
}

pub struct Output {
    // Keep the stream alive on the control thread, never in Tauri UI state.
    _stream: StreamOwner,
    pub commands: Producer<Command>,
    pub info: OutputInfo,
}

enum StreamOwner {
    Cpal {
        _stream: cpal::Stream,
    },
    #[cfg(all(windows, feature = "asio"))]
    Asio {
        _stream: super::asio_output::AsioOutput,
    },
}

fn host(driver_type: DriverType) -> AppResult<cpal::Host> {
    match driver_type {
        DriverType::Wasapi => {
            #[cfg(windows)]
            {
                cpal::host_from_id(cpal::HostId::Wasapi).map_err(|e| {
                    AppError::new("output", "WASAPI Shared를 초기화할 수 없습니다.").detail(e)
                })
            }
            #[cfg(not(windows))]
            {
                Ok(cpal::default_host())
            }
        }
        DriverType::Asio => {
            #[cfg(all(windows, feature = "asio"))]
            {
                cpal::host_from_id(cpal::HostId::Asio)
                    .map_err(|e| AppError::new("asio", "ASIO를 초기화할 수 없습니다.").detail(e))
            }
            #[cfg(not(all(windows, feature = "asio")))]
            {
                Err(AppError::new(
                    "asio_unavailable",
                    "이 빌드는 ASIO를 지원하지 않습니다. WASAPI Shared를 선택해 주세요.",
                ))
            }
        }
    }
}

// Runs only on the one output controller. ASIO name enumeration deliberately
// does not load every driver (CPAL devices() initializes devices while iterating).
pub fn devices(driver_type: DriverType) -> AppResult<Vec<String>> {
    if driver_type == DriverType::Asio {
        #[cfg(all(windows, feature = "asio"))]
        {
            return Ok(asio_sys::Asio::new().driver_names());
        }
    }
    host(driver_type)?
        .output_devices()
        .map_err(|e| AppError::new("devices", "출력 장치 목록을 읽을 수 없습니다.").detail(e))?
        .map(|d| {
            d.name().map_err(|e| {
                AppError::new("devices", "출력 장치 이름을 읽을 수 없습니다.").detail(e)
            })
        })
        .collect()
}

pub fn control_panel(output: Option<&Output>) -> AppResult<()> {
    if output.is_none_or(|o| o.info.driver_type != DriverType::Asio) {
        return Err(AppError::new(
            "asio_panel",
            "ASIO 출력 장치를 먼저 연결해 주세요.",
        ));
    }
    #[cfg(all(windows, feature = "asio"))]
    {
        super::asio_output::control_panel()
    }
    #[cfg(not(all(windows, feature = "asio")))]
    {
        Err(AppError::new(
            "asio_panel",
            "이 빌드는 ASIO를 지원하지 않습니다.",
        ))
    }
}

pub fn open(
    shared: Arc<TransportCell>,
    metrics: Arc<AudioMetrics>,
    errors: Arc<AtomicU64>,
    buffer: Option<u32>,
    preferences: &AudioPreferences,
    spectrum: &Arc<super::spectrum::Spectrum>,
) -> AppResult<Output> {
    let fail = |e| {
        AppError::new("output", if preferences.driver_type == DriverType::Asio { "선택한 ASIO 드라이버를 초기화할 수 없습니다. 오디오 설정에서 WASAPI Shared를 선택할 수 있습니다." } else { "선택한 오디오 출력 장치를 열 수 없습니다." }).detail(e)
    };
    #[cfg(all(windows, feature = "asio"))]
    let asio_slot = if preferences.driver_type == DriverType::Asio {
        Some(super::asio_output::AsioSlot::acquire()?)
    } else {
        None
    };
    let host = host(preferences.driver_type)?;
    let device = if let Some(selected) = preferences.selected_device() {
        host.output_devices()
            .map_err(|e| fail(e.to_string()))?
            .find(|d| d.name().ok().as_deref() == Some(selected))
    } else if preferences.driver_type == DriverType::Asio {
        return Err(AppError::new(
            "asio_device",
            "사용할 ASIO 드라이버를 선택해 주세요.",
        ));
    } else {
        host.default_output_device()
    }
    .ok_or_else(|| {
        AppError::new(
            "output",
            "선택한 출력 장치를 찾을 수 없습니다. 오디오 설정에서 장치를 다시 선택해 주세요.",
        )
    })?;
    let supported = device
        .default_output_config()
        .map_err(|e| fail(e.to_string()))?;
    let (min, max) = match supported.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => (Some(*min), Some(*max)),
        _ => (None, None),
    };
    let buffer = if preferences.driver_type == DriverType::Asio {
        None
    } else {
        buffer
    };
    if let Some(size) = buffer {
        if !matches!(size, 128 | 256 | 512 | 1024)
            || min.is_some_and(|min| size < min)
            || max.is_some_and(|max| size > max)
        {
            return Err(AppError::new(
                "buffer",
                "출력 장치가 요청한 버퍼 크기를 지원하지 않습니다.",
            ));
        }
    }
    let mut config = supported.config();
    config.buffer_size = buffer
        .map(cpal::BufferSize::Fixed)
        .unwrap_or(cpal::BufferSize::Default);
    let info = OutputInfo {
        backend: host.id().name().to_owned(),
        sample_format: format!("{:?}", supported.sample_format()),
        device: device.name().unwrap_or_else(|_| "기본 출력 장치".into()),
        sample_rate: config.sample_rate.0,
        channels: config.channels,
        requested_buffer: buffer,
        supported_buffer_min: min,
        supported_buffer_max: max,
        driver_type: preferences.driver_type,
        driver_input_latency_frames: None,
        driver_output_latency_frames: None,
        control_panel: false,
    };
    let (commands, rx) = RingBuffer::new(COMMAND_CAPACITY);
    let mut renderer = Renderer::new(
        rx,
        shared,
        metrics,
        info.sample_rate,
        info.channels as usize,
    );
    renderer.set_declick(preferences.transport_declick);
    renderer.set_spectrum(spectrum.tap(info.sample_rate)?);
    if preferences.driver_type == DriverType::Asio {
        #[cfg(all(windows, feature = "asio"))]
        {
            let mut info = info;
            let stream = super::asio_output::AsioOutput::open(
                device,
                host,
                renderer,
                errors,
                &mut info,
                asio_slot.expect("ASIO slot acquired before host initialization"),
            )?;
            return Ok(Output {
                _stream: StreamOwner::Asio { _stream: stream },
                commands,
                info,
            });
        }
    }
    let on_error = move |_error| {
        errors.fetch_add(1, Ordering::Relaxed);
    };
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => device.build_output_stream(
            &config,
            move |data: &mut [f32], _| renderer.render_device(data),
            on_error,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_output_stream(
            &config,
            move |data: &mut [i16], _| renderer.render_device(data),
            on_error,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_output_stream(
            &config,
            move |data: &mut [u16], _| renderer.render_device(data),
            on_error,
            None,
        ),
        cpal::SampleFormat::I32 => device.build_output_stream(
            &config,
            move |data: &mut [i32], _| renderer.render_device(data),
            on_error,
            None,
        ),
        cpal::SampleFormat::F64 => device.build_output_stream(
            &config,
            move |data: &mut [f64], _| renderer.render_device(data),
            on_error,
            None,
        ),
        format => {
            return Err(AppError::new(
                "sample_format",
                "출력 장치의 샘플 형식을 지원하지 않습니다.",
            )
            .detail(format))
        }
    }
    .map_err(|e| fail(e.to_string()))?;
    stream.play().map_err(|e| fail(e.to_string()))?;
    Ok(Output {
        _stream: StreamOwner::Cpal { _stream: stream },
        commands,
        info,
    })
}
