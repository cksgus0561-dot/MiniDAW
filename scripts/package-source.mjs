// Explicit allowlist; never archive the checkout/target directory wholesale.
import {readFileSync,writeFileSync,readdirSync,mkdirSync,cpSync,existsSync} from 'node:fs';
import {resolve,join,relative,dirname} from 'node:path';
import {createHash} from 'node:crypto';
import {execFileSync} from 'node:child_process';
const [rustSources,asioSources,sourceInputs,installer] = process.argv.slice(2);
if(!installer)throw Error('Usage: node scripts/package-source.mjs RUST_VENDOR ASIO_SUBSET SOURCE_INPUTS INSTALLER');
const root=resolve(import.meta.dirname,'..'), version=JSON.parse(readFileSync(join(root,'package.json'))).version;
const out=join(root,'output/distribution'),stage=join(root,'output/source-stage-'+Date.now());
const main=join(stage,'MiniDAW'),dep=join(stage,'dependencies');mkdirSync(out,{recursive:true});mkdirSync(main,{recursive:true});
execFileSync(process.execPath,[join(root,'scripts/licenses.mjs'),'check'],{stdio:'inherit'});
const sha=p=>createHash('sha256').update(readFileSync(p)).digest('hex');
const walk=d=>readdirSync(d,{withFileTypes:true}).flatMap(e=>e.isDirectory()?walk(join(d,e.name)):[join(d,e.name)]);
const omit=new Set(['node_modules','target','gen','local','generated','fixtures','review-candidate-notices']);
function copy(from,to){cpSync(from,to,{recursive:true,filter:s=>!relative(from,s).split(/[\\/]/).some(x=>omit.has(x)||x==='review-candidate.json'||/\.(log|pdb|exe|dll|wav|mp3|flac|minidaw|zip|pdf)$/i.test(x))});}
for(const p of ['src','scripts','public','.github','tests','licenses','packaging','src-tauri/src','src-tauri/tests','src-tauri/vendor','src-tauri/capabilities','src-tauri/icons'])copy(join(root,p),join(main,p));
for(const p of ['README.md','BUILDING.md','LICENSE','SOURCE_CODE.md','THIRD_PARTY_NOTICES.md','.gitignore','.editorconfig','index.html','package.json','package-lock.json','tsconfig.json','vite.config.ts','src-tauri/Cargo.toml','src-tauri/Cargo.lock','src-tauri/build.rs','src-tauri/tauri.conf.json','docs/DISTRIBUTION.md']){mkdirSync(dirname(join(main,p)),{recursive:true});cpSync(join(root,p),join(main,p));}
cpSync(join(root,'docs/CLEAN-WINDOWS-TEST.md'),join(main,'docs/CLEAN-WINDOWS-TEST.md'));
const a=JSON.parse(readFileSync(join(root,'licenses/reviewed-dependencies.json')));
const copied=[];
for(const p of a.packages.filter(p=>p.ecosystem==='cargo'&&p.scope.startsWith('Windows'))){const name=p.name+'-'+p.version,from=resolve(rustSources,name),to=join(dep,'rust',name);const sums=JSON.parse(readFileSync(join(from,'.cargo-checksum.json')));if(sums.package!==p.checksum)throw Error('Crate checksum mismatch: '+name);for(const [file,hash]of Object.entries(sums.files))if(sha(join(from,file))!==hash)throw Error('Modified crate: '+name+'/'+file);cpSync(from,to,{recursive:true});copied.push(name);}
cpSync(resolve(asioSources),join(dep,'asio'),{recursive:true});
const sdk=JSON.parse(readFileSync(join(root,'licenses/asio-host-files.json')));for(const f of sdk.files)if(sha(join(dep,'asio',f.path))!==f.sha256)throw Error('ASIO source mismatch');
const sourcePolicy=JSON.parse(readFileSync(join(root,'licenses/source-inputs.json')));
// Include the exact seven source crates used by the locally rebuilt NSIS helper.
const helperMeta=JSON.parse(execFileSync('cargo',['metadata','--offline','--locked','--format-version','1','--manifest-path',join(root,'output/nsis-helper/source/Cargo.toml')],{encoding:'utf8',maxBuffer:8e6}));
if(sha(join(root,'output/nsis-helper/source/Cargo.lock'))!==sha(join(root,'packaging/nsis-helper/Cargo.lock')))throw Error('Helper build used a different lockfile');
for(const p of helperMeta.packages.filter(p=>p.source)){const from=dirname(p.manifest_path);cpSync(from,join(dep,'nsis-helper-crates',p.name+'-'+p.version),{recursive:true});}
for(const f of sourcePolicy.files){const input=resolve(sourceInputs,f.file);if(sha(input)!==f.sha256)throw Error('Frontend source mismatch '+f.file);mkdirSync(join(dep,'npm'),{recursive:true});cpSync(input,join(dep,'npm',f.file));}
const provenance={helper:JSON.parse(readFileSync(join(root,'output/package-validation/nsis-helper-build.json'))),installer:JSON.parse(readFileSync(join(root,'output/package-validation/nsis-final.json'))),application:JSON.parse(readFileSync(join(root,'output/package-validation/executable.json')))};
if(sha(resolve(installer))!==provenance.installer.installerSha256)throw Error('Source packaging requires verified final installer');
writeFileSync(join(dep,'BUILD-PROVENANCE.json'),JSON.stringify(provenance,null,2)+'\n');
writeFileSync(join(dep,'README.md'),`# Component sources\n\n${copied.length} exact Windows runtime/build Rust crate source trees are provided in rust/. Their .cargo-checksum.json files match Cargo.lock. Inactive Apple/Linux crates are not included. Standard Cargo/npm builds use the pinned upstream registries; internet access is required for resolution and development tools. These bundled source trees are available for inspection, modification and use as local Cargo path patches; retain original licenses.\n\nThe three npm source tarballs contain the preferred TypeScript inputs and their upstream build scripts. Their hashes and fixed commits/tags are in MiniDAW/licenses/source-inputs.json. npm ci obtains the matching published build artifacts.\n\nUse asio/ as -SdkPath for MiniDAW/scripts/package-windows.ps1; it contains the exact GPL/BSD Windows host sources. Supply your own LLVM/libclang location. No ASIO proprietary license is selected, and no driver or SDK tooling is included.\n`);
writeFileSync(join(dep,'README.md'),readFileSync(join(dep,'README.md'),'utf8')+'\nThe fourth tarball, npm/nsis-tauri-utils-source.tar.gz, contains the installer helper source. Its seven external crate sources are in nsis-helper-crates/. Use MiniDAW/packaging/nsis-helper/Cargo.lock and scripts/build-nsis-helper.mjs; no source modifications are applied. BUILD-PROVENANCE.json records the compiler and actual helper/installer hashes.\n');
const manifest={version,projectLicense:'GPL-3.0-only',files:{}};
for(const p of walk(stage)){const rel=relative(stage,p).replaceAll('\\','/');manifest.files[rel]=sha(p);if(rel.startsWith('MiniDAW/')&&/\.(rs|cpp|h|ts|js|mjs|json|md|toml|ps1|html|css|yml)$/.test(p)){const t=readFileSync(p,'utf8');if(t.includes(process.env.USERPROFILE)||t.includes(root))throw Error('Local developer path in public source: '+rel);}}
writeFileSync(join(stage,'SOURCE-MANIFEST.json'),JSON.stringify(manifest,null,2)+'\n');
const sourceZip=join(out,`MiniDAW-${version}-corresponding-source.zip`),repoZip=join(out,`MiniDAW-${version}-github-source.zip`);
execFileSync('tar',['-a','-cf',sourceZip,'-C',stage,'MiniDAW','dependencies','SOURCE-MANIFEST.json'],{stdio:'inherit'});
execFileSync('tar',['-a','-cf',repoZip,'-C',stage,'MiniDAW'],{stdio:'inherit'});
const setup=join(out,resolve(installer).split(/[\\/]/).at(-1));cpSync(resolve(installer),setup);
const provenanceFile=join(out,'BUILD-PROVENANCE.json');cpSync(join(dep,'BUILD-PROVENANCE.json'),provenanceFile);
writeFileSync(join(out,'SHA256SUMS.txt'),[setup,sourceZip,repoZip,provenanceFile].map(p=>sha(p)+'  '+p.split(/[\\/]/).at(-1)).join('\n')+'\n');
writeFileSync(join(out,'RELEASE-INSTRUCTIONS.txt'),'Not published. Upload setup + corresponding-source.zip + SHA256SUMS.txt together on the same release page (GPLv3 section 6(d)). The github-source.zip is a clean repository seed, not a substitute for corresponding source. Read docs/DISTRIBUTION.md for verification and unresolved limitations.\n');
console.log(JSON.stringify({out,sourceFiles:Object.keys(manifest.files).length,rustSources:copied.length,stage}));
