//! Offline verification only; shares the production plan/reader, writes no media.
use minidaw_lib::{
    audio::{
        source::AudioAsset,
        streaming::FrameReader,
        timeline::{PlaybackPlan, TimelineReader},
    },
    project::{
        edit::{self, Clipboard, EditRequest},
        migrations,
        normalize::{self, NormalizeStrategy, Peak},
        schema::*,
        session::resolve_all,
    },
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, fs, io::Read, path::Path, sync::Arc, time::Instant};
fn media(p: &Project, streaming: bool) -> HashMap<String, Arc<AudioAsset>> {
    resolve_all(p, None)
        .into_iter()
        .map(|s| {
            let path = s.resolved_path.unwrap();
            (
                s.asset_id,
                AudioAsset::open_with_budget(
                    Path::new(&path),
                    if streaming { 0 } else { 64 * 1024 * 1024 },
                )
                .unwrap(),
            )
        })
        .collect()
}
fn compare(a: Arc<PlaybackPlan>, b: Arc<PlaybackPlan>) -> Value {
    assert_eq!(a.frames, b.frames);
    let frames = a.frames;
    let rate = a.rate;
    let mut a = TimelineReader::new(a);
    let mut b = TimelineReader::new(b);
    a.seek(0, Arc::new(|| false)).unwrap();
    b.seek(0, Arc::new(|| false)).unwrap();
    let (mut mismatch, mut max, mut sum) = (0_u64, 0_f64, 0_f64);
    let (mut ha, mut hb) = (Sha256::new(), Sha256::new());
    for _ in 0..frames {
        let x = a.read_frame().unwrap();
        let y = b.read_frame().unwrap();
        for c in 0..2 {
            assert!(x[c].is_finite() && y[c].is_finite());
            ha.update(x[c].to_le_bytes());
            hb.update(y[c].to_le_bytes());
            let d = (x[c] as f64 - y[c] as f64).abs();
            mismatch += u64::from(d != 0.0);
            max = max.max(d);
            sum += d * d;
        }
    }
    assert_eq!(mismatch, 0);
    json!({"outputRate":rate,"frames":frames,"mismatchSamples":mismatch,"maxError":max,"rmsError":(sum/(frames*2) as f64).sqrt(),"beforeSha256":format!("{:x}",ha.finalize()),"afterSha256":format!("{:x}",hb.finalize())})
}
fn file_hash(path: &Path) -> String {
    let mut f = fs::File::open(path).unwrap();
    let mut h = Sha256::new();
    let mut block = vec![0; 1024 * 1024];
    loop {
        let n = f.read(&mut block).unwrap();
        if n == 0 {
            break;
        }
        h.update(&block[..n]);
    }
    format!("{:x}", h.finalize())
}
fn request(p: &Project, command: &str) -> EditRequest {
    serde_json::from_value(
        json!({"command":command,"clipIds":p.clips().map(|c|&c.clip_id).collect::<Vec<_>>()}),
    )
    .unwrap()
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let ui: Value = serde_json::from_slice(&fs::read(&args[1]).unwrap()).unwrap();
    let before: Project = serde_json::from_value(ui["document"].clone()).unwrap();
    let restored =
        migrations::decode(&fs::read(ui["savedProject"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(before, restored);
    let mut comparisons = vec![];
    for rate in [44100, 48000] {
        let a =
            PlaybackPlan::compile(Arc::new(before.clone()), media(&before, false), rate).unwrap();
        let b = PlaybackPlan::compile(Arc::new(restored.clone()), media(&restored, true), rate)
            .unwrap();
        comparisons.push(compare(a, b));
    }
    let large = Path::new("tests/generated/long-1200s-48000-2ch.wav");
    let hash_before = file_hash(large);
    let source = AudioAsset::open(large).unwrap();
    assert!(source.info.duration >= 1200.0);
    let mut p = Project::new();
    let mut asset = before.assets[0].clone();
    asset.asset_id = id();
    asset.filename = "long-1200s-48000-2ch.wav".into();
    asset.metadata.sample_rate = 48000;
    asset.metadata.source_frames = Frames(source.info.frames as u64);
    p.import(asset);
    let mut r = request(&p, "audio.splitAtCursor");
    r.cursor = Some(Position::Seconds {
        numerator: Signed(601),
        denominator: 1,
    });
    p = edit::apply(&p, &r, &mut Clipboard::default()).unwrap();
    let mut r = request(&p, "audio.trim");
    r.clip_ids = vec![p.clips().next().unwrap().clip_id.clone()];
    r.source_start = Some(Frames(100));
    p = edit::apply(&p, &r, &mut Clipboard::default()).unwrap();
    let plan = PlaybackPlan::compile(
        Arc::new(p.clone()),
        HashMap::from([(p.assets[0].asset_id.clone(), source.clone())]),
        48000,
    )
    .unwrap();
    assert!(plan.streaming());
    assert_eq!(plan.resident_bytes(), 0);
    let mut reader = TimelineReader::new(plan.clone());
    let mut inspected = 0;
    for at in [0, 100, 48000 * 600 + 47990, 48000 * 601, 48000 * 1199] {
        reader.seek(at, Arc::new(|| false)).unwrap();
        for _ in 0..500 {
            assert!(reader.read_frame().unwrap().iter().all(|s| s.is_finite()));
            inspected += 1;
        }
    }
    let mut peak = Peak::default();
    let t = Instant::now();
    normalize::analyze(large, 48000 * 600, 48000 * 601, &mut peak).unwrap();
    let normalize_ms = t.elapsed().as_secs_f64() * 1000.;
    assert!(peak.gain(-1.).unwrap().is_finite());
    assert_eq!(hash_before, file_hash(large));
    let mut stress = p.clone();
    let template = stress.tracks[0].clips[0].audio().clone();
    stress.primary_clip_id = None;
    stress.tracks[0].clips = (0..1200)
        .map(|i| {
            let mut c = template.clone();
            c.clip_id = id();
            c.position = Position::Seconds {
                numerator: Signed(i * 50),
                denominator: 48000,
            };
            c.source_start = Frames(0);
            c.source_end = Frames(40);
            Clip::Audio(c)
        })
        .collect();
    let stress_plan = PlaybackPlan::compile(
        Arc::new(stress),
        HashMap::from([(p.assets[0].asset_id.clone(), source.clone())]),
        48000,
    )
    .unwrap();
    let t = Instant::now();
    for i in 0..1_000_000_usize {
        let at = std::hint::black_box(i * 997 % stress_plan.frames);
        std::hint::black_box(stress_plan.spans.partition_point(|s| s.end <= at));
    }
    let lookup_ms = t.elapsed().as_secs_f64() * 1000.;
    let mut stress_reader = TimelineReader::new(stress_plan.clone());
    let t = Instant::now();
    stress_reader.seek(0, Arc::new(|| false)).unwrap();
    for _ in 0..stress_plan.frames {
        assert!(stress_reader
            .read_frame()
            .unwrap()
            .iter()
            .all(|s| s.is_finite()));
    }
    let render_ms = t.elapsed().as_secs_f64() * 1000.;
    let result = json!({"uiReleaseSha256":ui["executableSha256"],"savedProject":ui["savedProject"],"realExitRelaunchProjectEquality":true,"roundtripMemoryStreaming":comparisons,"timelineLookup":{"clips":1200,"spans":stress_plan.spans.len(),"buildMs":stress_plan.build_ms,"lookupCount":1_000_000,"lookupTotalMs":lookup_ms,"lookupMeanNs":lookup_ms,"sequentialFrames":stress_plan.frames,"sequentialWorkerMs":render_ms},"large":{"bytes":fs::metadata(large).unwrap().len(),"duration":source.info.duration,"sourcePcmResidentBytes":plan.resident_bytes(),"streaming":plan.streaming(),"seekInspectedFrames":inspected,"normalizeOneSecondMs":normalize_ms,"peak":peak.peak,"sourceSha256Before":hash_before,"sourceSha256After":file_hash(large)},"passed":true});
    fs::write(&args[2], serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    println!("{result}");
}
