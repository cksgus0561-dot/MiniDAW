import { element } from './dom';
import { language, setLanguage, tr, uiText } from './i18n';
import { shortcutCommands, bindingsFor, defaultBindings, shortcutLabel, commandName, conflicts, eventBinding, assignShortcut, clearShortcut, resetShortcuts, type ShortcutId } from './shortcuts';

export function initializeAppSettings():void {
  const dialog=element<HTMLDialogElement>('app-settings');
  const locale=element<HTMLSelectElement>('ui-language');
  const search=element<HTMLInputElement>('shortcut-search');
  const list=element('shortcut-list');
  const capture=element<HTMLInputElement>('shortcut-capture');
  const message=element('shortcut-message');
  const assign=element<HTMLButtonElement>('shortcut-assign');
  const replace=element<HTMLButtonElement>('shortcut-reassign');
  let selected:ShortcutId='transport.toggle',pending:string|null=null;
  function feedback():void {
    uiText(element('shortcut-selected'),commandName(selected),false);
    uiText(element('shortcut-current'),shortcutLabel(selected)||'지정 안 함');
    const used=pending?conflicts(pending,selected):[];
    uiText(element('shortcut-conflicts'),used.length?tr('충돌하는 기능')+': '+used.map(commandName).join(', '):pending?tr('충돌 없음'):'');
    assign.disabled=!pending||used.length>0;replace.hidden=!used.length;replace.disabled=!pending;
  }
  function render():void {
    locale.value=language();const query=search.value.trim().toLocaleLowerCase();list.replaceChildren();
    for(const[id,source]of shortcutCommands){
      if(query&&![id,source,commandName(id),...bindingsFor(id),...defaultBindings(id)].join(' ').toLocaleLowerCase().includes(query))continue;
      const row=document.createElement('button');row.type='button';row.className='shortcut-row';row.dataset.shortcut=id;row.setAttribute('aria-pressed',String(id===selected));
      for(const value of [commandName(id),shortcutLabel(id)||tr('지정 안 함'),defaultBindings(id).join(' / ')||'—']){const cell=document.createElement('span');uiText(cell,value,false);row.append(cell);}
      row.onclick=()=>{selected=id;pending=null;capture.value='';uiText(message,'');render();};list.append(row);
    }
    if(!list.childElementCount){const empty=document.createElement('p');uiText(empty,'검색 결과가 없습니다.');list.append(empty);}
    feedback();
  }
  function change(action:()=>void):void {try{action();pending=null;capture.value='';uiText(message,'설정이 저장되었습니다.');}catch(e){uiText(message,tr('설정을 저장하지 못했습니다. 이번 실행에는 적용됩니다.')+' '+String(e));}render();}
  element('open-app-settings').onclick=()=>{render();dialog.showModal();};
  element('app-settings-close').onclick=()=>dialog.close();
  locale.onchange=()=>{try{setLanguage(locale.value==='en'?'en':'ko');uiText(message,'설정이 저장되었습니다.');}catch{uiText(message,'설정을 저장하지 못했습니다. 이번 실행에는 적용됩니다.');}render();};
  search.oninput=render;
  capture.onkeydown=e=>{
    if(e.key==='Tab')return;
    e.preventDefault();e.stopPropagation();if(e.repeat)return;
    if(e.key==='Escape'){pending=null;capture.value='';feedback();return;}
    const binding=eventBinding(e);if(!binding){uiText(message,'문자/기능 키와 Ctrl, Alt, Shift 조합을 누르세요. Tab과 Esc는 편집에 사용합니다.');return;}
    pending=binding;capture.value=binding;uiText(message,'');feedback();
  };
  assign.onclick=()=>{if(pending)change(()=>assignShortcut(selected,pending!));};
  replace.onclick=()=>{if(pending)change(()=>assignShortcut(selected,pending!,true));};
  element('shortcut-clear').onclick=()=>change(()=>clearShortcut(selected));
  element('shortcut-reset').onclick=()=>change(resetShortcuts);
  document.addEventListener('language-changed',render);
}
