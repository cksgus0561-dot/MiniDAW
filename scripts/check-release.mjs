import {readFileSync,writeFileSync,mkdirSync} from 'node:fs';
import {resolve,dirname,join} from 'node:path';
import {homedir} from 'node:os';
import {createHash} from 'node:crypto';
const path=resolve(process.argv[2]??'src-tauri/target/release/minidaw.exe'),b=readFileSync(path);
const pe=b.readUInt32LE(0x3c);if(b.toString('ascii',pe,pe+4)!=='PE\0\0')throw Error('Not PE');
if(b.readUInt16LE(pe+4)!==0x8664)throw Error('Expected Windows x64');
const opt=pe+24,n=b.readUInt16LE(pe+6),sections=opt+b.readUInt16LE(pe+20);
const offset=r=>{for(let i=0;i<n;i++){let p=sections+40*i,va=b.readUInt32LE(p+12),size=Math.max(b.readUInt32LE(p+8),b.readUInt32LE(p+16));if(r>=va&&r<va+size)return b.readUInt32LE(p+20)+r-va;}throw Error('Invalid PE RVA');};
const cstr=p=>b.toString('ascii',p,b.indexOf(0,p));
let p=offset(b.readUInt32LE(opt+112+8));const imports=[];
while(b.readUInt32LE(p+12)){imports.push(cstr(offset(b.readUInt32LE(p+12))));p+=20;}
if(imports.some(x=>/^(msvcp|vcruntime|concrt)\d/i.test(x)))throw Error('VC runtime DLL still required: '+imports.join(', '));
const sourceRoot=resolve(import.meta.dirname,'..');
const toolPolicy=JSON.parse(readFileSync(join(sourceRoot,'licenses/packaging-tools.json')));
for(const f of toolPolicy.files){const file=join(process.env.LOCALAPPDATA,'tauri','NSIS',f.file);if(createHash('sha256').update(readFileSync(file)).digest('hex')!==f.sha256)throw Error('Installer tool changed; review license/source: '+f.file);}
const helperBuild=JSON.parse(readFileSync(join(sourceRoot,'output/package-validation/nsis-helper-build.json')));
const final=JSON.parse(readFileSync(join(sourceRoot,'output/package-validation/nsis-final.json')));
const hash=p=>createHash('sha256').update(readFileSync(p)).digest('hex');
const version=JSON.parse(readFileSync(join(sourceRoot,'package.json'))).version;
const installer=join(sourceRoot,`src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/MiniDAW_${version}_x64-setup.exe`);
if(final.upstreamPrebuiltHelperUsed!==false||final.helperSha256!==helperBuild.dllSha256||hash(join(sourceRoot,'output/nsis-helper/minidaw_nsis_utils.dll'))!==final.helperSha256||hash(installer)!==final.installerSha256)throw Error('Final installer provenance mismatch');
const variants=[homedir(),sourceRoot].flatMap(s=>[s,s.replaceAll('\\','/'),s.replaceAll('\\','\\\\')]);
for(const v of variants)for(const encoding of ['utf8','utf16le'])if(b.includes(Buffer.from(v,encoding)))throw Error('Developer path embedded in executable: '+v);
const report={file:path.split(/[\\/]/).at(-1),sha256:createHash('sha256').update(b).digest('hex'),bytes:b.length,architecture:'x86_64',imports,developerPathsFound:false,dynamicVcRuntime:false};
mkdirSync('output/package-validation',{recursive:true});writeFileSync('output/package-validation/executable.json',JSON.stringify(report,null,2)+'\n');
console.log(JSON.stringify(report,null,2));
