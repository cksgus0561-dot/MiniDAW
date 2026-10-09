import {build} from 'esbuild';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
// Small DOM contract double; the release suite separately checks the real DOM.
class TextNode {nodeType=3;childNodes=[];constructor(value=''){this.textContent=value;}}
class ElementNode {
 nodeType=1;childNodes=[];attributes=new Map();isContentEditable=false;
 get firstChild(){return this.childNodes[0]??null;}get textContent(){return this.childNodes.map(n=>n.textContent).join('');}
 set textContent(s){this.childNodes=s?[new TextNode(s)]:[];}
 append(...children){this.childNodes.push(...children);}replaceChildren(...children){this.childNodes=children;}
 setAttribute(k,v){this.attributes.set(k,v);}getAttribute(k){return this.attributes.get(k)??null;}closest(){return null;}
}
globalThis.Node={TEXT_NODE:3};globalThis.HTMLElement=ElementNode;globalThis.Element=ElementNode;
const storage=new Map();globalThis.localStorage={getItem:k=>storage.get(k)??null,setItem:(k,v)=>storage.set(k,v)};
const listeners=new Map();const body=new ElementNode();globalThis.document={body,documentElement:{lang:'ko'},getElementById:()=>null,querySelector:()=>null,querySelectorAll:()=>[],createTextNode:s=>new TextNode(s),addEventListener:(k,f)=>{listeners.set(k,[...listeners.get(k)??[],f]);},dispatchEvent:e=>{for(const f of listeners.get(e.type)??[])f(e);}};
const bundled=await build({stdin:{contents:"export * from './src/i18n'; export * from './src/shortcuts'; export * from './src/keyboard';",resolveDir:process.cwd(),loader:'ts'},bundle:true,format:'esm',platform:'node',write:false});
const m=await import('data:text/javascript;base64,'+Buffer.from(bundled.outputFiles[0].text).toString('base64'));
const key=(code,extra={})=>({code,key:code,ctrlKey:false,altKey:false,shiftKey:false,metaKey:false,isComposing:false,defaultPrevented:false,target:body,...extra});
assert.equal(m.shortcutCommand(key('Space',{key:' '})),'transport.toggle');
assert.equal(m.shortcutCommand(key('Numpad0',{key:'0'})),'transport.stop');
assert.equal(m.shortcutCommand(key('Digit0',{key:'0'})),undefined);
assert.equal(m.shortcutCommand(key('NumpadDecimal',{key:'.'})),'transport.start');
assert.equal(m.shortcutCommand(key('NumpadComma',{key:','})),'transport.start');
assert.equal(m.shortcutCommand(key('NumpadDivide',{key:'/'})),'transport.cycle');
assert.equal(m.shortcutCommand(key('KeyJ',{key:'ㅓ'})),'grid.snap');
assert.equal(m.shortcutCommand(key('KeyJ',{isComposing:true})),undefined);
assert.equal(m.shortcutCommand(key('Backspace')),'edit.delete');
assert.equal(m.shortcutCommand(key('Home')),undefined);
assert.throws(()=>m.assignShortcut('transport.toggle','J'));
m.assignShortcut('transport.toggle','J',true);assert.deepEqual(m.bindingsFor('grid.snap'),[]);
assert.equal(m.shortcutCommand(key('KeyJ')),'transport.toggle');assert.equal(m.shortcutCommand(key('Space',{key:' '})),undefined);
m.clearShortcut('edit.delete');assert.equal(m.shortcutCommand(key('Backspace')),undefined);assert.equal(m.shortcutCommand(key('Delete')),undefined);
m.resetShortcuts();assert.equal(m.shortcutCommand(key('Space',{key:' '})),'transport.toggle');
const input=new ElementNode();input.type='number';input.closest=s=>s==='input'?input:null;
assert.equal(m.shortcutCommand(key('Space',{key:' ',target:input})),undefined);
input.type='range';assert.equal(m.shortcutCommand(key('Space',{key:' ',target:input})),'transport.toggle');
const label=new ElementNode(),control=new ElementNode();m.uiText(label,'재생');label.append(control);body.append(label);
const list=new ElementNode();m.uiText(list,'오디오 없음');body.append(list);const row=new ElementNode();m.uiText(row,'재생',false);list.replaceChildren(row);
m.setLanguage('en');assert.equal(label.textContent,'Play');assert.equal(label.childNodes[1],control);assert.equal(list.firstChild,row);assert.equal(row.textContent,'재생');
assert.equal(m.tr('Clip 선택 · 재생.wav'),'Select Clip · 재생.wav');
assert.equal(m.tr('MIDI Clip을 선택하세요 · 수신 0 / 처리 0 / 누락 0 · — Ch 1 0 · sample 0 · 악기 없음'),'Select a MIDI Clip · received 0 / processed 0 / dropped 0 · — Ch 1 0 · sample 0 · No instrument');
assert.equal(m.tr('Gain은 -144~144 dB 범위입니다.'),'Gain must be between -144 and 144 dB.');
assert.equal(m.tr('샘플레이트: 44100 → 48000 Hz'),'Sample rate: 44100 → 48000 Hz');
m.setLanguage('ko');assert.equal(label.textContent,'재생');assert.equal(list.firstChild,row);assert.equal(label.childNodes[1],control);
const catalog=JSON.parse(await readFile('src/locales/messages.json','utf8'));
for(const[a,b]of Object.entries(catalog))assert.deepEqual([...a.matchAll(/\{\d+\}/g)].map(x=>x[0]).sort(),[...b.matchAll(/\{\d+\}/g)].map(x=>x[0]).sort(),a);
console.log(JSON.stringify({passed:true,checks:['Cubase defaults/aliases and physical keys','IME/text focus protection','conflict reassignment/clear/reset','language switch preserves nested controls and replaced list rows','opaque user data and backend error localization','catalog placeholder parity'],catalogEntries:Object.keys(catalog).length}));
