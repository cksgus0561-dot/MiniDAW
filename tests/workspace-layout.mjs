import {build} from 'esbuild';
import assert from 'node:assert/strict';
const bundle=await build({entryPoints:['src/workspace-layout.ts'],bundle:true,format:'esm',platform:'node',write:false});
const {readWorkspace,workspaceGeometry}=await import('data:text/javascript;base64,'+Buffer.from(bundle.outputFiles[0].text).toString('base64'));
const defaults=readWorkspace(null,null);assert.equal(defaults.arrangement,true);assert.equal(defaults.zones.left.open,true);assert.equal(defaults.zones.right.open,false);assert.equal(defaults.zones.lower.open,false);
const migrated=readWorkspace(null,{media:{open:false},arrangement:{open:true},spectrum:{open:true},performance:{open:true},mixer:{open:true}});
assert.equal(migrated.zones.left.open,false);assert.equal(migrated.zones.right.active,'spectrum');assert.equal(migrated.zones.lower.active,'mixer');
const invalid=readWorkspace({arrangement:5,zones:{left:null,right:{open:true,active:'mixer',size:NaN},lower:{active:'bad',size:-200}}},null);
assert.equal(invalid.arrangement,true);assert.equal(invalid.zones.right.active,'spectrum');assert.equal(invalid.zones.right.size,340);assert.equal(invalid.zones.lower.size,120);
let count=0;
for(const [w,h]of [[760,310],[1000,490],[1280,560],[1440,760],[1920,850]])for(const scale of [.82,1,1.14])for(let mask=0;mask<16;mask++){
 const s=structuredClone(defaults);s.arrangement=!!(mask&8);s.zones.left.open=!!(mask&1);s.zones.right.open=!!(mask&2);s.zones.lower.open=!!(mask&4);
 s.zones.left.size=640;s.zones.right.size=890;s.zones.lower.size=980;const before=JSON.stringify(s),g=workspaceGeometry(s,w,h,scale);
 assert.equal(JSON.stringify(s),before,'window clamps never mutate preferences');assert.ok(g.left+g.right+g.leftGap+g.rightGap<w);assert.ok(g.lower+g.lowerGap<=h);assert.ok(g.left>=0&&g.right>=0&&g.lower>=0);
 if(!s.zones.left.open)assert.equal(g.left+g.leftGap,0);if(!s.zones.right.open)assert.equal(g.right+g.rightGap,0);if(!s.zones.lower.open)assert.equal(g.lower+g.lowerGap,0);
 if(!s.arrangement&&s.zones.lower.open)assert.equal(g.lower,h);count++;
}
assert.deepEqual(readWorkspace(JSON.parse(JSON.stringify(migrated)),null),migrated);
console.log(`Workspace defaults, migration, invalid preferences, persistence and ${count} geometry cases passed.`);
