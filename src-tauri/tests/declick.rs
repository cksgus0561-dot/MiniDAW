use minidaw_lib::audio::{
    declick::Declick,
    decoder::{AudioData, FileInfo},
    metrics::AudioMetrics,
    renderer::{Action, Command, Renderer, COMMAND_CAPACITY},
    source::AudioSource,
    transport::TransportCell,
};
use rtrb::RingBuffer;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Instant,
};
thread_local! { static IN_RENDER: Cell<bool> = const { Cell::new(false) }; }
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static FREES: AtomicUsize = AtomicUsize::new(0);
struct CountingAllocator;
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if IN_RENDER.try_with(Cell::get).unwrap_or(false) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        if IN_RENDER.try_with(Cell::get).unwrap_or(false) {
            FREES.fetch_add(1, Ordering::Relaxed);
        }
        unsafe {
            System.dealloc(p, layout);
        }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
fn realtime_transitions_allocate_and_free_nothing() {
    let owner = AudioSource::memory(AudioData {
        info: FileInfo {
            name: "dc".into(),
            sample_rate: 48000,
            channels: 2,
            frames: 48000,
            duration: 1.0,
            sanitized_samples: 0,
        },
        samples: vec![0.8; 96000],
    });
    let (mut tx, rx) = RingBuffer::new(COMMAND_CAPACITY);
    let shared = Arc::new(TransportCell::default());
    let mut renderer = Renderer::new(
        rx,
        shared.clone(),
        Arc::new(AudioMetrics::default()),
        48000,
        2,
    );
    renderer.set_declick(true);
    let mut block = [0.0_f32; 128];
    tx.push(Command {
        id: 0,
        issued: Instant::now(),
        action: Action::Load {
            clip_id: 1,
            audio: owner.clone(),
        },
    })
    .ok()
    .unwrap();
    for n in 1..2000 {
        let action = match n % 7 {
            0 => Action::Play,
            1 => Action::Pause,
            2 => Action::Seek(0.31),
            3 => Action::Stop,
            4 => Action::Toggle,
            5 => Action::Declick(false),
            _ => Action::Declick(true),
        };
        tx.push(Command {
            id: n,
            issued: Instant::now(),
            action,
        })
        .ok()
        .unwrap();
        IN_RENDER.set(true);
        renderer.render(&mut block);
        IN_RENDER.set(false);
        assert!(block.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
        assert_eq!(shared.read().applied_command, n);
    }
    assert_eq!(ALLOCATIONS.load(Ordering::Relaxed), 0);
    assert_eq!(FREES.load(Ordering::Relaxed), 0);
}

#[test]
fn transition_holds_boundary_then_becomes_bit_exact_and_off_is_unconditional() {
    for rate in [44100, 48000, 96000] {
        let frames = (rate as f64 * 0.002).round() as usize;
        let mut d = Declick::new(rate, 2.0);
        d.set_enabled(true);
        for _ in 0..frames {
            d.process(Some([0.75, -0.75]));
        }
        d.transition();
        assert_eq!(d.process(Some([-0.5, 0.5])), [0.75, -0.75]);
        for _ in 1..frames {
            d.process(Some([-0.5, 0.5]));
        }
        assert_eq!(d.process(Some([-0.5, 0.5])), [-0.5, 0.5]);
        d.transition();
        assert_eq!(d.process(None), [-0.5, 0.5]);
        for _ in 1..frames {
            d.process(None);
        }
        assert_eq!(d.process(None), [0.0; 2]);
        d.set_enabled(false);
        d.transition();
        assert_eq!(
            d.process(Some([0.987654321, -0.123456789])),
            [0.987654321, -0.123456789]
        );
        assert_eq!(d.process(None), [0.0; 2]);
    }
}
