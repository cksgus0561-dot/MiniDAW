// Build the separately licensed installer helper from pinned upstream source.
// No changes to Tauri's shared tool cache or to the MiniDAW audio application.
import {readFileSync,writeFileSync,mkdirSync,cpSync,readdirSync} from 'node:fs';
import {resolve,join,dirname} from 'node:path';
import {createHash} from 'node:crypto';
import {execFileSync} from 'node:child_process';
const root=resolve(import.meta.dirname,'..');
const sha=p=>createHash('sha256').update(readFileSync(p)).digest('hex');
const policy=JSON.parse(readFileSync(join(root,'licenses/tools/nsis-helper/dependencies.json')));
const input=JSON.parse(readFileSync(join(root,'licenses/source-inputs.json'))).files.find(f=>f.name==='nsis-tauri-utils');
const archive=join(root,'output/source-inputs',input.file);
if(sha(archive)!==input.sha256)throw Error('Unreviewed NSIS helper source');
const lock=join(root,'packaging/nsis-helper/Cargo.lock');
if(sha(lock)!==policy.lockHash)throw Error('Unreviewed NSIS helper lockfile');
const work=join(root,'output/nsis-helper'),source=join(work,'source');
mkdirSync(source,{recursive:true});
execFileSync('tar',['-xf',archive,'-C',source,'--strip-components','1'],{stdio:'inherit'});
cpSync(lock,join(source,'Cargo.lock'));
const env={...process.env};delete env.RUSTFLAGS;
const cargoHome=env.CARGO_HOME||join(env.USERPROFILE,'.cargo');
env.CARGO_ENCODED_RUSTFLAGS=['-C','target-feature=+crt-static','--remap-path-prefix='+root+'=/minidaw','--remap-path-prefix='+cargoHome+'=/cargo'].join('\x1f');
const manifest=join(source,'Cargo.toml');
execFileSync('cargo',['build','--locked','--release','--target',policy.target,'--manifest-path',manifest,'-p','nsis-tauri-utils'],{env,stdio:'inherit'});
const metadata=JSON.parse(execFileSync('cargo',['metadata','--locked','--offline','--format-version','1','--manifest-path',manifest],{env,encoding:'utf8',maxBuffer:8e6}));
const actual=metadata.packages.filter(p=>p.source);
if(actual.length!==policy.packages.length)throw Error('Helper dependency set changed');
for(const p of actual){const reviewed=policy.packages.find(q=>q.name===p.name&&q.version===p.version&&q.license===p.license);if(!reviewed)throw Error('Unreviewed helper dependency '+p.name);for(const f of reviewed.notices)if(sha(join(root,f))!==sha(join(dirname(p.manifest_path),f.split('/').at(-1))))throw Error('Helper notice differs from compiled crate '+p.name);}
const dll=join(work,'minidaw_nsis_utils.dll');
cpSync(join(source,'target',policy.target,'release/nsis_tauri_utils.dll'),dll);
const b=readFileSync(dll);if(b.readUInt16LE(b.readUInt32LE(0x3c)+4)!==0x14c)throw Error('Helper must be x86 NSIS plugin');
for(const p of [root,cargoHome])for(const s of [p,p.replaceAll('\\','/')])if(b.includes(Buffer.from(s))||b.includes(Buffer.from(s,'utf16le')))throw Error('Developer path in helper');
// Source files are verified again by Cargo during a normal locked rebuild.
const report={sourceArchiveSha256:input.sha256,lockSha256:sha(lock),target:policy.target,rustc:execFileSync('rustc',['-Vv'],{encoding:'utf8'}).trim(),dllSha256:sha(dll),bytes:b.length,packages:policy.packages.map(({name,version,license,scope})=>({name,version,license,scope}))};
mkdirSync(join(root,'output/package-validation'),{recursive:true});
writeFileSync(join(root,'output/package-validation/nsis-helper-build.json'),JSON.stringify(report,null,2)+'\n');
console.log(JSON.stringify(report,null,2));
