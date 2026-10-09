import messages from './locales/messages.json';
import terms from './locales/terms.json';

export type Language = 'ko' | 'en';
const key='minidaw.ui.language.v1';
let current:Language='ko';
try { if(localStorage.getItem(key)==='en')current='en'; } catch { /* Session defaults. */ }
const english:Record<string,string>=messages, korean:Record<string,string>=terms;
const reverse=new Map(Object.entries(english).map(([ko,en])=>[en,ko]));
const pattern=(s:string)=>new RegExp('^'+s.split(/(\{\d+\})/).map(p=>/^\{\d+\}$/.test(p)?'([\\s\\S]*?)':p.replace(/[.*+?^${}()|[\]\\]/g,'\\$&')).join('')+'$');
const needle=(s:string)=>s.split(/\{\d+\}/).sort((a,b)=>b.length-a.length)[0];
const patterns=Object.entries(english).filter(([s])=>/\{\d+\}/.test(s)).sort((a,b)=>b[0].replace(/\{\d+\}/g,'').length-a[0].replace(/\{\d+\}/g,'').length).map(([ko,en])=>({ko,en,re:pattern(ko),back:pattern(en),koNeedle:needle(ko),enNeedle:needle(en)}));
const cache=new Map<string,string>();
// These slots contain app-authored sub-messages, not user names/paths. All other
// template slots stay opaque, including track names supplied by the project.
const nested:Record<string,number[]>={
  '{0} · 수신 {1} / 처리 {2} / 누락 {3} · {4} Ch {5} {6} · sample {7} · {8}':[0,8],
  '{0} · Enter/포커스 이동 적용 · Esc 취소 · Snap 무시{1}{2}':[0,1,2],
  '완료 · {0} s · {1}초 처리 ({2}×)\n{3}{4}':[4],
};
export function language():Language{return current;}
export function tr(value:string):string {
  if(!value)return value;
  const cached=cache.get(value);if(cached!==undefined)return cached;
  const trimmed=value.trim(),prefix=value.slice(0,value.indexOf(trimmed)),suffix=value.slice(value.indexOf(trimmed)+trimmed.length);
  let result=current==='en'?(english[value]??english[trimmed]):(korean[value]??korean[trimmed]??reverse.get(value)??reverse.get(trimmed));
  if(result!==undefined&&!(value in english)&&!(value in korean)&&!reverse.has(value))result=prefix+result+suffix;
  if(result===undefined&&(current==='en'?/[가-힣]/.test(value):/[A-Za-z]/.test(value))) {
    for(const p of patterns){if(!value.includes(current==='en'?p.koNeedle:p.enNeedle))continue;const m=(current==='en'?p.re:p.back).exec(value);if(!m)continue;
      // Placeholders are opaque project/user data, never translated.
      const source=current==='en'?p.ko:p.en, dest=current==='en'?p.en:p.ko;
      const slots=[...source.matchAll(/\{(\d+)\}/g)].map(x=>Number(x[1]));const values=new Map(slots.map((slot,i)=>[slot,nested[p.ko]?.includes(slot)?tr(m[i+1]):m[i+1]]));
      result=dest.replace(/\{(\d+)\}/g,(_,i)=>values.get(Number(i))??'');break;
    }
  }
  result??=value;
  // High-rate meter/readout strings must not grow a permanent cache.
  if(cache.size>=1024)cache.clear();cache.set(value,result);return result;
}
interface Binding {source:string;translate:boolean}
const texts=new WeakMap<Node,Binding>();
const attrs=new WeakMap<Element,Map<string,Binding>>();
export function uiText(target:Node,value:unknown,translate=true):void {
  const source=String(value??''),shown=translate?tr(source):source;
  // Bind the actual Text node, never its container. A label may later append an
  // input, or an empty list may gain rows; switching language must preserve them.
  if(target.nodeType===Node.TEXT_NODE){texts.set(target,{source,translate});if(target.textContent!==shown)target.textContent=shown;return;}
  if(target.textContent!==shown||target.childNodes.length!==1||target.firstChild?.nodeType!==Node.TEXT_NODE)target.textContent=shown;
  if(target.firstChild)texts.set(target.firstChild,{source,translate});
}
export function uiAttr(target:Element,name:string,value:string,translate=true):void {
  let map=attrs.get(target);if(!map){map=new Map();attrs.set(target,map);}map.set(name,{source:value,translate});
  const shown=translate?tr(value):value;if(target.getAttribute(name)!==shown)target.setAttribute(name,shown);
}
export function uiAppend(target:Element,...children:(Node|string)[]):void {
  target.append(...children.map(c=>{if(typeof c!=='string')return c;const node=document.createTextNode('');uiText(node,c);return node;}));
}
export function uiOption(label:string,value:string):HTMLOptionElement {
  const option=new Option('',value);uiText(option,label);return option;
}
export function initializeLocalization():void {
  document.documentElement.lang=current;
  const walker=document.createTreeWalker(document.body,NodeFilter.SHOW_TEXT|NodeFilter.SHOW_ELEMENT);
  while(walker.nextNode()) {
    const node=walker.currentNode;
    if(node instanceof Element){for(const name of ['title','aria-label','placeholder']){const value=node.getAttribute(name);if(value)uiAttr(node,name,value);}}
    else if(node.textContent?.trim()&&!node.parentElement?.closest('script,style,textarea,[data-i18n-skip]'))uiText(node,node.textContent);
  }
}
export function setLanguage(value:Language):void {
  current=value;cache.clear();document.documentElement.lang=current;
  // Existing bindings are revisited only on a language change. No MutationObserver,
  // full-DOM polling, Canvas loop or audio IPC is needed for localization.
  const visit=(node:Node)=>{const binding=texts.get(node);if(binding){uiText(node,binding.source,binding.translate);return;}for(const child of [...node.childNodes])visit(child);};
  visit(document.body);
  for(const el of document.querySelectorAll('*')){for(const[name,binding]of attrs.get(el)??[])uiAttr(el,name,binding.source,binding.translate);}
  document.dispatchEvent(new Event('language-changed'));
  localStorage.setItem(key,value);
}
