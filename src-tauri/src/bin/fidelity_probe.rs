//! Offline audit tools; never used by the audio callback. User audio is read-only.
use serde_json::json;
use std::{fs::File, path::Path, time::Instant};
use symphonia::core::{
    audio::SampleBuffer, codecs::DecoderOptions, formats::FormatOptions, io::MediaSourceStream,
    meta::MetadataOptions, probe::Hint,
};
fn inspect(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let mut hint = Hint::new();
    hint.with_extension(path.extension().unwrap().to_str().unwrap());
    let mut format = symphonia::default::get_probe()
        .format(
            &hint,
            MediaSourceStream::new(Box::new(File::open(path)?), Default::default()),
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )?
        .format;
    let track = format.default_track().unwrap();
    let params = track.codec_params.clone();
    let id = track.id;
    let mut decoder = symphonia::default::get_codecs().make(&params, &DecoderOptions::default())?;
    let mut frames = 0_u64;
    let mut packets = 0;
    let mut first = Vec::new();
    let mut last = Vec::new();
    let mut decode_errors = Vec::new();
    let mut native = String::new();
    let eof = loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(e) => break e.to_string(),
        };
        if packet.track_id() != id {
            continue;
        }
        packets += 1;
        match decoder.decode(&packet) {
            Ok(buf) => {
                native = match &buf {
                    symphonia::core::audio::AudioBufferRef::U8(_) => "U8",
                    symphonia::core::audio::AudioBufferRef::S16(_) => "S16",
                    symphonia::core::audio::AudioBufferRef::S24(_) => "S24",
                    symphonia::core::audio::AudioBufferRef::S32(_) => "S32",
                    symphonia::core::audio::AudioBufferRef::F32(_) => "F32",
                    symphonia::core::audio::AudioBufferRef::F64(_) => "F64",
                    _ => "other",
                }
                .into();
                let n = buf.frames();
                let mut converted = SampleBuffer::<f32>::new(buf.capacity() as u64, *buf.spec());
                converted.copy_interleaved_ref(buf);
                let record = json!({"packet":packets,"ts":packet.ts(),"dur":packet.dur(),"trimStart":packet.trim_start(),"trimEnd":packet.trim_end(),"decodedFrames":n,"convertedSamples":converted.samples().len(),"totalBefore":frames});
                if packets <= 4 {
                    first.push(record.clone());
                }
                last.push(record);
                if last.len() > 4 {
                    last.remove(0);
                }
                frames += n as u64;
            }
            Err(e) => {
                decode_errors
                    .push(json!({"packet":packets,"ts":packet.ts(),"error":e.to_string()}));
                break e.to_string();
            }
        }
    };
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"path":path,"codecParameters":format!("{params:?}"),"nativeFormat":native,"packets":packets,"decodedFrames":frames,"firstPackets":first,"lastPackets":last,"decodeErrors":decode_errors,"end":eof,"elapsedMs":start.elapsed().as_secs_f64()*1000.0})
        )?
    );
    Ok(())
}
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = inspect(Path::new(&args[0])) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
