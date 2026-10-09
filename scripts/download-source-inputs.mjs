import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {resolve,join} from 'node:path';
import {createHash} from 'node:crypto';
const policy=JSON.parse(await readFile(new URL('../licenses/source-inputs.json',import.meta.url)));
const destination=resolve(process.argv[2]??'output/source-inputs');await mkdir(destination,{recursive:true});
for(const f of policy.files){const cached=await readFile(join(destination,f.file)).catch(()=>null);if(cached&&createHash('sha256').update(cached).digest('hex')===f.sha256){console.log('Verified cached '+f.file);continue;}const response=await fetch(f.url);if(!response.ok)throw Error(`${f.file}: HTTP ${response.status}`);const data=Buffer.from(await response.arrayBuffer());if(createHash('sha256').update(data).digest('hex')!==f.sha256)throw Error('Source archive changed: '+f.file);await writeFile(join(destination,f.file),data);console.log('Verified '+f.file);}
