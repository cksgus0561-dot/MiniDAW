// Tauri initially generates its upstream installer. Recompile that exact generated
// script with only the helper name/path changed; the final artifact never uses the
// upstream prebuilt helper. Keeping generated inputs avoids a forked UI template.
import {readFileSync,writeFileSync,cpSync,readdirSync} from 'node:fs';
import {resolve,join} from 'node:path';
import {createHash} from 'node:crypto';
import {execFileSync} from 'node:child_process';
const root=resolve(import.meta.dirname,'..'),release=join(root,'src-tauri/target/x86_64-pc-windows-msvc/release');
const dir=join(release,'nsis/x64'),helper=join(root,'output/nsis-helper'),dll=join(helper,'minidaw_nsis_utils.dll');
const sha=p=>createHash('sha256').update(readFileSync(p)).digest('hex');
const build=JSON.parse(readFileSync(join(root,'output/package-validation/nsis-helper-build.json')));
if(sha(dll)!==build.dllSha256)throw Error('Helper changed after build');
const tools=JSON.parse(readFileSync(join(root,'licenses/packaging-tools.json')));
const toolDir=join(process.env.LOCALAPPDATA,'tauri/NSIS');
for(const f of tools.files)if(sha(join(toolDir,f.file))!==f.sha256)throw Error('Unreviewed NSIS binary: '+f.file);
const script=readFileSync(join(dir,'installer.nsi'),'utf8');
if(!script.includes('nsis_tauri_utils::')||!script.includes('!define ADDITIONALPLUGINSPATH'))throw Error('Tauri template changed; review helper integration');
if(!script.includes('SetCompressor /SOLID "zlib"'))throw Error('Expected reviewed zlib stub');
// Escape NSIS interpolation characters in a developer-selected checkout path.
const escape=s=>s.replaceAll('$','$$').replaceAll('"','$\\"');
for(const name of readdirSync(dir).filter(n=>/\.(nsi|nsh)$/.test(n))){let text=readFileSync(join(dir,name),'utf8');text=text.replaceAll('nsis_tauri_utils::','minidaw_nsis_utils::');if(name==='installer.nsi')text=text.replace(/^!define ADDITIONALPLUGINSPATH .*$/m,'!define ADDITIONALPLUGINSPATH "'+escape(helper)+'"');writeFileSync(join(dir,name),text);}
const env={...process.env};delete env.NSISDIR;delete env.NSISCONFDIR;
const log=execFileSync(join(toolDir,'makensis.exe'),['-INPUTCHARSET','UTF8','-OUTPUTCHARSET','UTF8','-V4',join(dir,'installer.nsi')],{cwd:dir,env,encoding:'utf8',maxBuffer:16e6});
writeFileSync(join(root,'output/package-validation/nsis-compile.log'),log);
if(!log.includes('minidaw_nsis_utils::SemverCompare')||!log.includes('minidaw_nsis_utils::RunAsUser'))throw Error('Expected locally built helper not in compiled script');
const version=JSON.parse(readFileSync(join(root,'package.json'))).version;
const installer=join(release,`bundle/nsis/MiniDAW_${version}_x64-setup.exe`);
cpSync(join(dir,'nsis-output.exe'),installer);
const report={installerSha256:sha(installer),helperSha256:sha(dll),helper:'minidaw_nsis_utils.dll',upstreamPrebuiltHelperUsed:false,compression:'zlib'};
writeFileSync(join(root,'output/package-validation/nsis-final.json'),JSON.stringify(report,null,2)+'\n');
console.log(report);
