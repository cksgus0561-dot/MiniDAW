// Fail-closed release gate. `candidate` generates a review, never approves it.
import {readFileSync, writeFileSync, readdirSync, existsSync, mkdirSync} from 'node:fs';
import {resolve, join, relative, dirname} from 'node:path';
import {homedir} from 'node:os';
import {createHash} from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
const root=resolve(dirname(fileURLToPath(import.meta.url)),'..');
const mode=process.argv[2]??'check';
const sha=b=>createHash('sha256').update(b).digest('hex');
const read=p=>readFileSync(p,'utf8').replace(/^\uFEFF/,'');
const json=p=>JSON.parse(read(p));
const stable=x=>JSON.stringify(x,null,2)+'\n';
const licenseDir=join(root,'licenses'), approvedPath=join(licenseDir,'reviewed-dependencies.json');
const locks=['package-lock.json','src-tauri/Cargo.lock'];
const lockHashes=Object.fromEntries(locks.map(p=>[p,sha(readFileSync(join(root,p)))]));
const fail=m=>{throw Error(m);};
const allowed=new Set(['MIT','Apache-2.0','BSD-2-Clause','BSD-3-Clause','ISC','Zlib','Unlicense','CC0-1.0','0BSD','MIT-0','Unicode-3.0','MPL-2.0','Apache-2.0 WITH LLVM-exception','GPL-3.0-only']);
// Cargo's historical slash syntax means OR. AND is never flattened to OR.
export function compatible(expression) {
 const tokens=expression.replace(/\s*\/\s*/g,' OR ').match(/\(|\)|AND|OR|[^\s()]+/g)??[];let i=0;
 function atom(){if(tokens[i]==='('){i++;const v=or();if(tokens[i++]!==')')fail('Malformed SPDX');return v;}let id=tokens[i++];if(tokens[i]==='WITH'){i++;id+=' WITH '+tokens[i++];}return allowed.has(id);}
 function and(){let v=atom();while(tokens[i]==='AND'){i++;const r=atom();v=v&&r;}return v;}
 function or(){let v=and();while(tokens[i]==='OR'){i++;const r=and();v=v||r;}return v;}
 if(!tokens.length)return false;const v=or();return i===tokens.length&&v;
}
function walk(dir,depth=0){if(!existsSync(dir))return [];return readdirSync(dir,{withFileTypes:true}).sort((a,b)=>a.name.localeCompare(b.name,'en')).flatMap(e=>e.isFile()?[join(dir,e.name)]:e.isDirectory()&&depth<5&&!['.git','target','node_modules'].includes(e.name)?walk(join(dir,e.name),depth+1):[]);}
const noticeName=/^(licen[cs]e|copying|copyright|notice|authors|third[-_]?party)/i;
// The upstream branding guide is audited by hash when present in a developer
// checkout, but intentionally omitted from the public source allowlist. It is
// not code, a build input, or a license/notice required by the CLAP interfaces.
const omittedSourceDocuments=new Set(['src-tauri/vendor/clap/artwork/CLAP Logo Guidelines.pdf']);
function notices(dir){return walk(dir).filter(p=>noticeName.test(p.split(/[\\/]/).at(-1))&&!/\.(png|svg|rs|toml|json|ts|js|html?)$/i.test(p));}
if(mode==='check') {
 const a=json(approvedPath);
 for(const p of locks)if(a.lockHashes[p]!==lockHashes[p])fail(p+' changed: run npm run licenses:update and review the candidate');
 for(const [p,h]of Object.entries(a.buildInputHashes??{}))if(sha(readFileSync(join(root,p)))!==h)fail('Build/feature/bundle inputs changed: '+p+'; review license scope');
 if(a.projectLicenseHash!==sha(readFileSync(join(root,'LICENSE'))))fail('GPL text changed');
 for(const p of a.packages)if(!compatible(p.license))fail('Unreviewed/incompatible license: '+p.name+' '+p.license);
 const helper=json(join(root,'licenses/tools/nsis-helper/dependencies.json'));
 if(sha(readFileSync(join(root,'packaging/nsis-helper/Cargo.lock')))!==helper.lockHash)fail('Installer helper lock changed');
 for(const p of helper.packages)if(!compatible(p.license)||!p.notices.length)fail('Unreviewed helper license: '+p.name);
 for(const [p,h]of Object.entries(a.evidenceHashes))if(!existsSync(join(root,p))||sha(readFileSync(join(root,p)))!==h)fail('Missing/changed license evidence: '+p);
 for(const [p,h]of Object.entries(a.vendorHashes)){if(!existsSync(join(root,p))&&omittedSourceDocuments.has(p))continue;if(!existsSync(join(root,p))||sha(readFileSync(join(root,p)))!==h)fail('Vendored code changed: '+p);}
 const currentVendor=walk(join(root,'src-tauri/vendor')).map(p=>relative(root,p).replaceAll('\\','/')).filter(p=>!omittedSourceDocuments.has(p)).sort();
 if(JSON.stringify(currentVendor)!==JSON.stringify(Object.keys(a.vendorHashes).filter(p=>!omittedSourceDocuments.has(p)).sort()))fail('Vendored file added/removed; license review required');
 for(const p of ['LICENSE','SOURCE_CODE.md','THIRD_PARTY_NOTICES.md'])if(!existsSync(join(root,p)))fail('Missing '+p);
 if(json(join(root,'package.json')).license!=='GPL-3.0-only')fail('MiniDAW license changed');
 console.log(`License gate OK: ${a.packages.length} locked Rust/npm components; ${Object.keys(a.vendorHashes).length} vendored files; ${Object.keys(a.evidenceHashes).length} notice files.`);
} else if(mode==='candidate') {
 const registry=process.env.MINIDAW_RUST_SOURCES;
 if(!registry||!existsSync(registry))fail('Set process-local MINIDAW_RUST_SOURCES to cargo vendor --locked --versioned-dirs output');
 const metadata=JSON.parse(execFileSync('cargo',['metadata','--offline','--locked','--format-version','1','--filter-platform','x86_64-pc-windows-msvc','--features','asio','--manifest-path',join(root,'src-tauri/Cargo.toml')],{encoding:'utf8',maxBuffer:32*1024*1024}));
 const pk=new Map(metadata.packages.map(p=>[p.id,p])), nodes=new Map(metadata.resolve.nodes.map(n=>[n.id,n])), seen=new Set(), runtime=new Set(), build=new Set();
 function visit(id,b){let k=id+':'+b;if(seen.has(k))return;seen.add(k);(b?build:runtime).add(id);for(const d of nodes.get(id)?.deps??[])for(const kind of d.dep_kinds){if(kind.kind==='dev')continue;visit(d.pkg,b||kind.kind==='build'||pk.get(d.pkg).targets.some(t=>t.kind.includes('proc-macro')));}}
 visit(metadata.resolve.root,false);
 const target=new Map(metadata.packages.map(p=>[p.name+'@'+p.version,{license:p.license,scope:runtime.has(p.id)?'Windows runtime':build.has(p.id)?'Windows build':'Other target / inactive'}]));
 const packages=[], evidenceHashes={},out=join(licenseDir,'review-candidate-notices');mkdirSync(out,{recursive:true});
 function addEvidence(key,dir,files){const result=[];for(const f of files){const rel=relative(dir,f).replaceAll('\\','/');const dest=join(out,key,rel);mkdirSync(dirname(dest),{recursive:true});const bytes=readFileSync(f);writeFileSync(dest,bytes);const p='licenses/components/'+key+'/'+rel;evidenceHashes[p]=sha(bytes);result.push(p);}return result;}
 const cargo=read(join(root,'src-tauri/Cargo.lock')).split('[[package]]').slice(1);
 for(const block of cargo){const field=k=>block.match(new RegExp('^'+k+' = "([^"]+)"','m'))?.[1];const name=field('name'),version=field('version');if(!field('source'))continue;
  const dir=join(registry,name+'-'+version);if(!existsSync(dir))fail('Missing vendored crate '+dir);
  const manifest=read(join(dir,'Cargo.toml'));const lic=manifest.match(/^license = "([^"]+)"/m)?.[1];if(!lic||!compatible(lic))fail('Requires license review: '+name+' '+lic);
  let files=notices(dir);const evidence=addEvidence('rust/'+name+'-'+version,dir,files);
  // Preserve actual source copyright/permission headers too, especially MPL crates
  // whose published .crate omits its LICENSE. Full covered source is shipped separately.
  const headers=walk(dir).filter(p=>/\.(rs|c|cpp|h|hpp)$/.test(p)).flatMap(p=>{const t=read(p).split('\n').slice(0,55).join('\n');return /copyright|Mozilla Public|Permission is hereby granted|SPDX-License-Identifier/i.test(t)?['--- '+relative(dir,p).replaceAll('\\','/')+' ---\n'+t]:[];}).join('\n\n');
  if(headers){const dest=join(out,'rust',name+'-'+version,'SOURCE-HEADERS.txt');mkdirSync(dirname(dest),{recursive:true});writeFileSync(dest,headers);const p='licenses/components/rust/'+name+'-'+version+'/SOURCE-HEADERS.txt';evidenceHashes[p]=sha(Buffer.from(headers));evidence.push(p);}
  packages.push({ecosystem:'cargo',name,version,license:lic,checksum:field('checksum'),scope:target.get(name+'@'+version)?.scope??'Other target / inactive',source:'https://crates.io/api/v1/crates/'+name+'/'+version+'/download',notices:evidence});
 }
 for(const [path,p]of Object.entries(json(join(root,'package-lock.json')).packages)){if(!path)continue;const name=path.split('node_modules/').at(-1),dir=join(root,path),lic=p.license;if(!compatible(lic??''))fail('Requires license review: '+name+' '+lic);
  const evidence=existsSync(dir)?addEvidence('npm/'+name.replaceAll('/','__')+'-'+p.version,dir,notices(dir)):[];
  packages.push({ecosystem:'npm',name,version:p.version,license:lic,checksum:p.integrity,scope:!p.dev?'Frontend runtime':name==='vite'?'Build + emitted runtime helpers':existsSync(dir)?'Development tool':'Other platform optional tool',source:p.resolved,notices:evidence});
 }
 const vendorHashes=Object.fromEntries(walk(join(root,'src-tauri/vendor')).map(p=>[relative(root,p).replaceAll('\\','/'),sha(readFileSync(p))]));
 const candidate={schema:1,reviewDate:'2026-10-09',projectLicense:'GPL-3.0-only',lockHashes,packages,vendorHashes,evidenceHashes};
 writeFileSync(join(licenseDir,'review-candidate.json'),stable(candidate));
 console.log('Candidate only; inspect licenses/review-candidate.json, original notices, missing evidence and GPL compatibility before replacing reviewed-dependencies.json.');
 console.log('Missing notice files:',packages.filter(p=>!p.notices.length&&p.scope!=='Other platform optional tool').map(p=>p.name+' '+p.version+' ['+p.scope+']'));
} else fail('Expected check or candidate');
