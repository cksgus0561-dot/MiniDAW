// Generated only on demand, ignored by Git. Bounded RAM, deterministic PCM.
// Defaults: 400 s / 1200 s stereo f32 WAV at 48 kHz (146 / 439 MiB).
// --compressed also creates 12-minute stereo MP3/FLAC at 44.1k and mono WAV48k.
import { mkdirSync, openSync, closeSync, writeSync, existsSync, statSync } from "node:fs";
import { resolve } from "node:path";
import { execFileSync } from "node:child_process";
const folder = resolve("tests/generated"); mkdirSync(folder, { recursive: true });
function wav(seconds, rate, channels) {
  const path = resolve(folder, `long-${seconds}s-${rate}-${channels}ch.wav`);
  const frames = seconds*rate, bytes = frames*channels*4;
  if (existsSync(path) && statSync(path).size === bytes+44) { console.log(`Already generated: ${path}`); return; }
  if (bytes+36 > 0xffffffff) throw new Error("Use RF64 for fixtures larger than the RIFF 4 GiB format limit");
  const file = openSync(path,"w");
  try {
    const h=Buffer.alloc(44); h.write("RIFF");h.writeUInt32LE(bytes+36,4);h.write("WAVEfmt ",8);
    h.writeUInt32LE(16,16);h.writeUInt16LE(3,20);h.writeUInt16LE(channels,22);h.writeUInt32LE(rate,24);
    h.writeUInt32LE(rate*channels*4,28);h.writeUInt16LE(channels*4,32);h.writeUInt16LE(32,34);h.write("data",36);h.writeUInt32LE(bytes,40);
    writeSync(file,h);
    const chunk=Buffer.alloc(rate*channels*4);
    for(let second=0;second<seconds;second++) {
      for(let f=0;f<rate;f++) for(let ch=0;ch<channels;ch++) {
        const t=second+f/rate;
        // Low listening level and slowly changing envelope: seek regions differ.
        const value=.035*Math.sin(2*Math.PI*(ch?443:227)*t)*(0.55+0.45*Math.sin(t*.071)**2);
        chunk.writeFloatLE(value,(f*channels+ch)*4);
      }
      writeSync(file,chunk);
    }
  } finally { closeSync(file); }
  console.log(`Generated ${path}: ${((bytes+44)/1048576).toFixed(1)} MiB`);
}
for (const seconds of [400,1200]) wav(seconds,48000,2);
if(process.argv.includes("--compressed")) {
  wav(720,48000,1);
  for(const extension of ["mp3","flac"]) {
    const output=resolve(folder,`long-720s-44100-2ch.${extension}`);
    if(existsSync(output)) continue;
    execFileSync("ffmpeg",["-hide_banner","-loglevel","error","-y","-i",resolve(folder,"long-1200s-48000-2ch.wav"),"-t","720","-ar","44100",...(extension==="mp3"?["-q:a","2"]:[]),output],{stdio:"inherit"});
    console.log(`Generated ${output}: ${(statSync(output).size/1048576).toFixed(1)} MiB`);
  }
}
