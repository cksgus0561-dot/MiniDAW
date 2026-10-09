import {readFileSync,writeFileSync} from 'node:fs';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
const path=process.argv[2];if(!path)throw Error('Pass primary MP3 path');
const data=readFileSync(path),tags=[],info=[],bitrates={},rates={};let offset=0,frames=0,junk=0,rawSamples=0;
const synchsafe=(b,o)=>(b[o]<<21)|(b[o+1]<<14)|(b[o+2]<<7)|b[o+3];
while(offset+4<=data.length){
 if(offset+10<=data.length&&data.toString('ascii',offset,offset+3)==='ID3'&&data[offset+3]>=2&&data[offset+3]<=4&&data.subarray(offset+6,offset+10).every(b=>b<128)){const size=10+synchsafe(data,offset+6);if(offset+size<=data.length){tags.push({offset,size,version:data[offset+3]});offset+=size;continue;}}
 const h=data.readUInt32BE(offset),version=(h>>>19)&3,layer=(h>>>17)&3,br=(h>>>12)&15,sr=(h>>>10)&3;
 if((h>>>21)!==0x7ff||version===1||layer!==1||br===0||br===15||sr===3){offset++;junk++;continue;}
 const rate=[44100,48000,32000][sr]/(version===3?1:version===2?2:4),bitrate=(version===3?[0,32,40,48,56,64,80,96,112,128,160,192,224,256,320]:[0,8,16,24,32,40,48,56,64,80,96,112,128,144,160])[br];
 const bytes=Math.floor((version===3?144000:72000)*bitrate/rate)+((h>>>9)&1);
 if(offset+bytes>data.length)break;
 const mono=((h>>>6)&3)===3,side=version===3?(mono?17:32):(mono?9:17);
 const x=offset+4+(((h>>>16)&1)?0:2)+side;const marker=data.toString('ascii',x,x+4);
 if(marker==='Info'||marker==='Xing'){
  const flags=data.readUInt32BE(x+4);let pos=x+8;const entry={offset,marker,flags};
  if(flags&1){entry.frames=data.readUInt32BE(pos);pos+=4;}if(flags&2){entry.bytes=data.readUInt32BE(pos);pos+=4;}
  if(flags&4){entry.tocBytes=100;pos+=100;}if(flags&8){entry.quality=data.readUInt32BE(pos);pos+=4;}
  entry.encoder=data.toString('ascii',pos,pos+9);const trim=data.readUIntBE(pos+21,3);entry.encoderDelay=trim>>>12;entry.encoderPadding=trim&4095;info.push(entry);
 }
 frames++;rawSamples+=version===3?1152:576;bitrates[bitrate]=(bitrates[bitrate]??0)+1;rates[rate]=(rates[rate]??0)+1;offset+=bytes;
}
const probe=JSON.parse(execFileSync('ffprobe',['-v','error','-show_format','-show_streams','-of','json',path],{encoding:'utf8'}));
const report={filename:path.split(/[\\/]/).at(-1),sha256:createHash('sha256').update(data).digest('hex'),bytes:data.length,id3:tags,mpegFramesIncludingInfo:frames,infoHeaders:info,bitrates,rates,junkBytes:junk,trailingBytes:data.length-offset,rawSamplesIncludingInfo:rawSamples,ffprobe:probe};
writeFileSync('docs/validation/primary-metadata.json',JSON.stringify(report,null,2));console.log(JSON.stringify(report,null,2));
