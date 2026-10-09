import { mkdirSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const folder = fileURLToPath(new URL("../tests/fixtures/", import.meta.url));
mkdirSync(folder, { recursive: true });
// Own synthetic audio only. FFmpeg is a fixture authoring tool, not an app dependency.
for (const extension of ["wav", "mp3", "flac"]) {
  const output = `${folder}/stereo-44100.${extension}`;
  execFileSync("ffmpeg", ["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i",
    "aevalsrc=0.08*sin(2*PI*220*t)|0.08*sin(2*PI*440*t):s=44100:d=6",
    "-af", "afade=t=in:d=0.03,afade=t=out:st=5.9:d=0.1", output], { stdio: "inherit" });
}
writeFileSync(`${folder}/corrupt.wav`, "MiniDAW intentional corrupt fixture\n");
execFileSync("ffmpeg", ["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i",
  "aevalsrc=0.05*sin(2*PI*330*t):s=48000:d=0.25", "-c:a", "pcm_f32le", `${folder}/모노-48000.WAV`], { stdio: "inherit" });
console.log("WAV / MP3 / FLAC fixtures generated.");
// Untagged VBR MP3 has only a bitrate estimate for its duration.
execFileSync("ffmpeg", ["-hide_banner", "-loglevel", "error", "-y", "-i", `${folder}/stereo-44100.wav`,
  "-q:a", "4", "-write_xing", "0", `${folder}/untagged-vbr.mp3`], { stdio: "inherit" });
