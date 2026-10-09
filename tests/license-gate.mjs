// Verify fail-closed behavior against a disposable source copy, never the checkout.
import {cpSync,mkdtempSync,readFileSync,writeFileSync,mkdirSync,existsSync,unlinkSync} from 'node:fs';
import {join,resolve,dirname} from 'node:path';
import {execFileSync,spawnSync} from 'node:child_process';
import assert from 'node:assert/strict';
const root=resolve(import.meta.dirname,'..'),a=JSON.parse(readFileSync(join(root,'licenses/reviewed-dependencies.json')));
mkdirSync(join(root,'output'),{recursive:true});const temp=mkdtempSync(join(root,'output/license-gate-test-'));
const files=new Set(['scripts/licenses.mjs','licenses/reviewed-dependencies.json','LICENSE','SOURCE_CODE.md','THIRD_PARTY_NOTICES.md',...Object.keys(a.lockHashes),...Object.keys(a.buildInputHashes),...Object.keys(a.vendorHashes),...Object.keys(a.evidenceHashes)]);
const brandingGuide='src-tauri/vendor/clap/artwork/CLAP Logo Guidelines.pdf';
for(const f of files){if(f===brandingGuide&&!existsSync(join(root,f)))continue;mkdirSync(dirname(join(temp,f)),{recursive:true});cpSync(join(root,f),join(temp,f));}
const run=()=>spawnSync(process.execPath,[join(temp,'scripts/licenses.mjs'),'check'],{encoding:'utf8'});
assert.equal(run().status,0);
if(existsSync(join(temp,brandingGuide)))unlinkSync(join(temp,brandingGuide));
assert.equal(run().status,0,'Public source archive intentionally excludes branding PDF');
function rejection(file,change,pattern){const p=join(temp,file),old=readFileSync(p);writeFileSync(p,change(old));const r=run();writeFileSync(p,old);assert.notEqual(r.status,0);assert.match(r.stderr,pattern);}
rejection('package-lock.json',b=>Buffer.concat([b,Buffer.from(' ')]),/changed/);
rejection('src-tauri/Cargo.toml',b=>Buffer.concat([b,Buffer.from('\n# new feature\n')]),/inputs changed/);
rejection('packaging/nsis-helper/Cargo.lock',b=>Buffer.concat([b,Buffer.from('\n')]),/inputs changed/);
rejection('scripts/rebundle-nsis.mjs',b=>Buffer.concat([b,Buffer.from('\n')]),/inputs changed/);
rejection('LICENSE',()=>Buffer.from('not the GPL'),/GPL text changed/);
rejection(Object.keys(a.evidenceHashes)[0],()=>Buffer.from('missing original notice'),/license evidence/);
rejection(Object.keys(a.vendorHashes)[0],b=>Buffer.concat([b,Buffer.from('\n')]),/Vendored code changed/);
rejection('licenses/reviewed-dependencies.json',b=>{const m=JSON.parse(b);m.packages[0].license='MIT AND LicenseRef-Unknown';return JSON.stringify(m);},/incompatible license/);
const report={passed:true,checks:['baseline','public source without branding PDF','lock update','feature change','helper lock change','helper integration change','GPL text modification','missing/changed notice','vendor source modification','unknown AND license']};
mkdirSync(join(root,'output/package-validation'),{recursive:true});writeFileSync(join(root,'output/package-validation/license-gate.json'),JSON.stringify(report,null,2)+'\n');console.log(report);
