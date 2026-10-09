// Build-only staging: select the Windows host API files with verified GPL/BSD
// grants. Never compile or redistribute the unrelated legacy COM driver samples.
import {readFileSync, writeFileSync, mkdirSync, copyFileSync, existsSync, readdirSync} from 'node:fs';
import {resolve, dirname, join} from 'node:path';
import {createHash} from 'node:crypto';
const [source, destination] = process.argv.slice(2);
if (!source || !destination) throw Error('Usage: node scripts/prepare-asio-sdk.mjs SDK_ROOT EMPTY_OUTPUT_DIR');
const from=resolve(source), to=resolve(destination);
if (from===to || to.startsWith(from+'\\')) throw Error('Output must be separate from the SDK');
const policy=JSON.parse(readFileSync(new URL('../licenses/asio-host-files.json', import.meta.url)));
mkdirSync(to,{recursive:true});
const existing = readdirSync(to);
if (existing.length) throw Error('Use an empty output directory, to exclude unreviewed SDK files');
for (const f of policy.files) {
  const bytes=readFileSync(join(from,f.path));
  if(createHash('sha256').update(bytes).digest('hex')!==f.sha256) throw Error('ASIO source changed; license review required: '+f.path);
  const p=join(to,f.path);mkdirSync(dirname(p),{recursive:true});copyFileSync(join(from,f.path),p);
}
copyFileSync(new URL('../LICENSE',import.meta.url),join(to,'COPYING.GPLv3'));
writeFileSync(join(to,'MINIDAW-SOURCE-NOTICE.txt'),
 'Unmodified Windows host subset of Steinberg ASIO SDK 2.3.4 (2025-10-15).\n'+
 'MiniDAW elects GPL version 3 for files referencing LICENSE.txt.\n'+
 'Host files retain their embedded BSD-3-Clause notices.\n'+
 'No Steinberg proprietary license is elected. No driver/sample/COM code is included.\n'+
 'Upstream: '+policy.source+'\n');
console.log('Prepared reviewed ASIO host subset ('+policy.files.length+' files)');
