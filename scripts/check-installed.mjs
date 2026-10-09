import {readFileSync,readdirSync,writeFileSync,mkdirSync} from 'node:fs';
import {join,resolve,relative} from 'node:path';
import {homedir} from 'node:os';
import {createHash} from 'node:crypto';
import assert from 'node:assert/strict';
const installed=resolve(process.argv[2]),root=resolve(import.meta.dirname,'..');
const a=JSON.parse(readFileSync(join(root,'licenses/reviewed-dependencies.json'))),sha=b=>createHash('sha256').update(b).digest('hex');
let count=0;
for(const[p,h]of Object.entries(a.evidenceHashes)){if(!/^licenses\/(components|supplemental|texts|sdk|tools)\//.test(p))continue;assert.equal(sha(readFileSync(join(installed,p))),h,p);count++;}
for(const p of ['LICENSE','SOURCE_CODE.md','THIRD_PARTY_NOTICES.md','licenses/reviewed-dependencies.json'])assert.equal(sha(readFileSync(join(installed,p))),sha(readFileSync(join(root,p))),p);
assert.deepEqual(readdirSync(installed).filter(x=>x.endsWith('.exe')).sort(),['minidaw.exe','uninstall.exe']);
const walk=d=>readdirSync(d,{withFileTypes:true}).flatMap(e=>e.isDirectory()?walk(join(d,e.name)):[join(d,e.name)]);
const needles=[root,homedir()].flatMap(s=>[s,s.replaceAll('\\','/'),s.replaceAll('\\','\\\\')]).flatMap(s=>[Buffer.from(s,'utf8'),Buffer.from(s,'utf16le')]);
const files=walk(installed);
for(const f of files){const rel=relative(installed,f).replaceAll('\\','/');assert.ok(['minidaw.exe','uninstall.exe','LICENSE','SOURCE_CODE.md','THIRD_PARTY_NOTICES.md'].includes(rel)||rel.startsWith('licenses/'),'Unexpected payload '+rel);assert.ok(!/\.(dll|pdb|wav|mp3|flac|minidaw|zip|pdf)$/i.test(rel),'Excluded payload '+rel);const bytes=readFileSync(f);for(const n of needles)assert.ok(!bytes.includes(n),'Developer path in '+rel);}
mkdirSync(join(root,'output/package-validation'),{recursive:true});
writeFileSync(join(root,'output/package-validation/installed-notices.json'),JSON.stringify({passed:true,verifiedNoticeHashes:count,files:files.length,developerPathsFound:false,executables:['minidaw.exe','uninstall.exe']},null,2)+'\n');
console.log(`Installed payload: ${files.length} files, ${count} license hashes, no developer paths or excluded payload.`);
