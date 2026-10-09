// Deterministic synthetic fixtures only; all generated audio is ignored.
import {mkdirSync, writeFileSync, readFileSync} from 'node:fs';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
const dir='tests/generated/fidelity';mkdirSync(dir,{recursive:true});
const formats=[['pcm16',16,1,2,44100],['pcm24',24,1,2,48000],['pcm32',32,1,2,48000],['float32',32,3,2,44100],['float64',64,3,2,48000],['mono16',16,1,1,48000]];
const manifest=[];
for(const [name,bits,code,channels,rate] of formats){
 const frames=rate*8,n=frames*channels,pcm=Buffer.alloc(n*bits/8),expected=Buffer.alloc(n*8);let seed=1978;
 for(let i=0;i<n;i++){
  seed=(Math.imul(seed,1664525)+1013904223)>>>0;let v;
  if(code===1){let integer=bits===32?(seed|0):((seed>>>(32-bits))-(2**(bits-1)));if(i<8)integer=[0,1,-1,2**(bits-1)-1,-(2**(bits-1)),123,-123,0][i];pcm.writeIntLE(integer,i*bits/8,bits/8);v=integer/2**(bits-1);}
  else {v=i<8?[0,.5,-.5,1.25,-1.5,1+2**-40,-1-2**-40,Math.PI/8][i]:(seed/2**32-.5)*3;if(bits===32){v=Math.fround(v);pcm.writeFloatLE(v,i*4);}else pcm.writeDoubleLE(v,i*8);}
  expected.writeDoubleLE(v,i*8);
 }
 const header=Buffer.alloc(44);header.write('RIFF');header.writeUInt32LE(36+pcm.length,4);header.write('WAVEfmt ',8);header.writeUInt32LE(16,16);header.writeUInt16LE(code,20);header.writeUInt16LE(channels,22);header.writeUInt32LE(rate,24);header.writeUInt32LE(rate*channels*bits/8,28);header.writeUInt16LE(channels*bits/8,32);header.writeUInt16LE(bits,34);header.write('data',36);header.writeUInt32LE(pcm.length,40);
 writeFileSync(`${dir}/${name}.wav`,Buffer.concat([header,pcm]));writeFileSync(`${dir}/${name}.f64`,expected);
 const entry={name,bits,code,channels,rate,frames,pcmMd5:createHash('md5').update(pcm).digest('hex')};
 if(['pcm16','pcm24'].includes(name)){
  execFileSync('ffmpeg',['-v','error','-y','-i',`${dir}/${name}.wav`,'-c:a','flac',`${dir}/${name}.flac`]);
  execFileSync('ffmpeg',['-v','error','-y','-i',`${dir}/${name}.flac`,'-c:a','pcm_f64le','-f','f64le',`${dir}/${name}-flac-reference.f64`]);
  const flac=readFileSync(`${dir}/${name}.flac`);entry.flacStreaminfoMd5=flac.subarray(26,42).toString('hex');
  if(entry.flacStreaminfoMd5!==entry.pcmMd5)throw Error('FLAC STREAMINFO MD5 differs from authored PCM');
 }
 manifest.push(entry);
}
// Reproduce embedded Info frames and an overstated first Info count, like the
// user's file. Never use or copy user audio into these synthetic fixtures.
const original=readFileSync('tests/fixtures/stereo-44100.mp3');
let info=original.indexOf('Info');if(info<0)info=original.indexOf('Xing');if(info<0)throw Error('No Xing/Info fixture');
const concat=Buffer.concat([original,original,original,original]);const frames=original.readUInt32BE(info+8);
concat.writeUInt32BE(frames*4+3,info+8);concat.writeUInt32BE(concat.length,info+12);
writeFileSync(`${dir}/embedded-info.mp3`,concat);
const under=Buffer.from(original);under.writeUInt32BE(Math.floor(frames/2),info+8);writeFileSync(`${dir}/understated-info.mp3`,under);
const over=Buffer.from(original);over.writeUInt32BE(frames+7,info+8);writeFileSync(`${dir}/overstated-info.mp3`,over);
const flac=readFileSync(`${dir}/pcm24.flac`),seek=Buffer.alloc(22);seek[0]=3;seek.writeUIntBE(18,1,3);seek.writeUInt16BE(flac.readUInt16BE(8),20);
writeFileSync(`${dir}/with-seektable.flac`,Buffer.concat([flac.subarray(0,42),seek,flac.subarray(42)]));
const unknown=Buffer.from(flac);unknown.writeBigUInt64BE(unknown.readBigUInt64BE(18)&~((1n<<36n)-1n),18);
writeFileSync(`${dir}/unknown-length.flac`,unknown);
const tag=Buffer.alloc(10+2*1024*1024);tag.write('ID3');tag[3]=4;let tagSize=tag.length-10;for(let i=9;i>=6;i--){tag[i]=tagSize&127;tagSize>>>=7;}
writeFileSync(`${dir}/large-id3.mp3`,Buffer.concat([tag,original]));
writeFileSync(`${dir}/manifest.json`,JSON.stringify(manifest,null,2));console.log(JSON.stringify(manifest,null,2));
