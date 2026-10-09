import { uiText, uiAttr, tr } from './i18n';
import { shortcutCommand } from './shortcuts';
import { globalShortcut } from './keyboard';
import type { MidiClip } from './midi';
import { invoke, isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open, save } from "@tauri-apps/plugin-dialog";
import { element } from "./dom";
import type { Panels } from './panels';

interface Clip { kind: "audio"; clipId: string; assetId: string; name: string; sourceStart: string; sourceEnd: string }
interface Asset { assetId: string; filename: string; metadata: { sampleRate: number; channels: number; sourceFrames: string } }
export interface ProjectView {
  document: { projectId: string; name: string; primaryClipId: string | null; assets: Asset[]; tracks: { clips: (Clip | MidiClip)[] }[] };
  path: string | null; dirty: boolean; revision: number;
  assets: { assetId: string; status: string; resolvedPath: string | null; message: string | null }[];
  recent: { path: string; name: string; missing: boolean }[];
  notice: string | null; preferenceWarning: string | null;
}
interface RelinkResult { view: ProjectView | null; confirmation: string | null; differences: string[] }
export class ProjectUI {
  busy = false;
  private view: ProjectView | null = null;
  private refreshTask: Promise<void> | null = null;
  private title = "";
  private menu = element<HTMLDetailsElement>("file-menu");
  constructor(private changed: () => void, private runtimeChange: (work: () => Promise<void>) => Promise<void>, private error: (e: unknown) => void, private blocked: () => boolean, private panels: Panels) {
    const bind = (id: string, action: () => Promise<void>) => element(id).addEventListener("click", () => { this.menu.open = false; void this.run(action); });
    bind("project-new", () => this.replace());
    bind("project-open", () => this.chooseProject());
    bind("midi-import", () => this.importMidi());
    bind("midi-export", () => this.exportMidi());
    bind("project-save", async () => { await this.saveCurrent(false); });
    bind("project-save-as", async () => { await this.saveCurrent(true); });
    bind("project-close", async () => { if (await this.confirmChanges()) await getCurrentWindow().destroy(); });
    element('project-assets').addEventListener('click', () => panels.setOpen('media', !panels.isOpen('media')));
    document.addEventListener('panel-visibility', e => { if ((e as CustomEvent).detail.id === 'media' && panels.isOpen('media') && this.view) this.render(this.view); });
    this.menu.addEventListener("toggle", () => { if (this.menu.open) void this.refresh().catch(this.error); });
    globalShortcut(event => {
      const id=shortcutCommand(event);
      const action=id==='project.save'?()=>this.saveCurrent(false):id==='project.saveAs'?()=>this.saveCurrent(true):id==='project.new'?()=>this.replace():id==='project.open'?()=>this.chooseProject():null;
      if(action)return()=>{void this.run(async()=>{await action();});};
    });
    this.controls();
  }
  async initialize(): Promise<void> {
    if (!isTauri()) return;
    await this.refresh();
    await getCurrentWindow().onCloseRequested(event => {
      event.preventDefault();
      void this.run(async () => { if (await this.confirmChanges()) await getCurrentWindow().destroy(); });
    });
  }
  controls(): void {
    for (const id of ["project-new", "project-open", "project-save", "project-save-as", "project-close", "project-assets", "midi-import", "midi-export"]) element<HTMLButtonElement>(id).disabled = !isTauri() || this.busy || this.blocked();
    for (const button of document.querySelectorAll<HTMLButtonElement>("#recent-projects button, #project-asset-list button")) button.disabled = this.busy || button.dataset.unavailable === "true";
  }
  observeRevision(revision?: number): void {
    if (revision !== undefined && (!this.view || revision !== this.view.revision)) void this.refresh().catch(this.error);
  }
  async refresh(): Promise<void> {
    if (!this.refreshTask) this.refreshTask = invoke<ProjectView>("project_snapshot").then(v => this.render(v)).finally(() => { this.refreshTask = null; });
    await this.refreshTask;
  }
  private async run(action: () => Promise<void>): Promise<void> {
    if (!isTauri() || this.busy) return;
    if (this.blocked()) { this.error({ message: "파일 또는 오디오 작업이 끝난 뒤 다시 시도해 주세요." }); return; }
    this.busy = true; this.controls(); this.changed();
    try { await action(); } catch (e) { this.error(e); }
    finally { this.busy = false; this.controls(); this.changed(); }
  }
  private render(view: ProjectView): void {
    if (this.view && view.revision < this.view.revision) return;
    this.view = view;
    document.dispatchEvent(new CustomEvent('project-view',{detail:view}));
    const title = `${view.document.name}${view.dirty ? " *" : ""}`;
    uiText(element("project-name"), title, false); uiAttr(element("project-name"), 'title', view.path ?? "아직 저장하지 않은 프로젝트");
    if (title !== this.title) { this.title = title; void getCurrentWindow().setTitle(`${title} — MiniDAW`).catch(this.error); }
    const unavailable = view.assets.filter(a => a.status !== "available").length;
    uiText(element("project-assets"), `미디어 ${view.document.assets.length}${unavailable ? ` · 연결 필요 ${unavailable}` : ""}`);
    const notice = [view.notice, view.preferenceWarning].filter(Boolean).join(" ");
    uiText(element("project-notice"), notice); element("project-notice").hidden = !notice;
    const recent = element("recent-projects"); recent.replaceChildren();
    for (const entry of view.recent) {
      const row = document.createElement("div"); row.className = "recent-project";
      const button = document.createElement("button"); uiText(button, `${entry.name}${entry.missing ? " (파일 없음)" : ""}`, entry.missing); uiAttr(button, 'title', entry.path, false);
      button.dataset.unavailable = String(entry.missing); button.disabled = entry.missing;
      button.onclick = () => { this.menu.open = false; void this.run(() => this.replace(entry.path)); };
      const remove = document.createElement("button"); uiText(remove, "×"); uiAttr(remove, "aria-label", `${entry.name} 최근 목록에서 제거`);
      remove.onclick = () => { void this.run(async () => { this.render(await invoke<ProjectView>("remove_recent_project", { path: entry.path })); }); };
      row.append(button, remove); recent.append(row);
    }
    if (!view.recent.length) uiText(recent, "최근 프로젝트가 없습니다.");
    const assets = element("project-asset-list"); assets.replaceChildren();
    for (const asset of this.panels.isOpen('media') ? view.document.assets : []) {
      const state = view.assets.find(a => a.assetId === asset.assetId);
      const row = document.createElement("section"); row.className = "project-asset"; row.dataset.assetId = asset.assetId;
      const name = document.createElement("strong"); uiText(name, asset.filename, false);
      const info = document.createElement("p"); info.className = "metric-note";
      uiText(info, `${asset.metadata.sampleRate} Hz · ${asset.metadata.channels} 채널 · ${asset.metadata.sourceFrames} 원본 프레임`);
      const status = document.createElement("p"); status.className = "asset-status"; uiText(status, state?.status === "available" ? "연결됨" : state?.message ?? "파일 연결 필요"); uiAttr(status, 'title', state?.resolvedPath ?? "");
      const relink = document.createElement("button"); uiText(relink, "파일 다시 연결"); relink.className = "relink-asset"; relink.onclick = () => { void this.run(() => this.relink(asset.assetId)); };
      row.append(name, info, status, relink);
      const place = document.createElement('button'); place.className = 'place-asset'; uiText(place, '커서에 배치');
      place.dataset.unavailable = String(state?.status !== 'available');
      uiAttr(place, 'title', '선택한 Audio Track의 커서에 새 Event로 배치');
      place.onclick = () => { this.panels.setOpen('arrangement', true); document.dispatchEvent(new CustomEvent('place-asset', {detail: asset.assetId})); }; row.append(place);
      for (const clip of view.document.tracks.flatMap(t => t.clips).filter((c): c is Clip => c.kind === 'audio' && c.assetId === asset.assetId)) {
        const button = document.createElement("button"); button.className = "preview-clip"; uiText(button, `Clip 선택 · ${clip.name}`); uiAttr(button, 'title', `Source frames [${clip.sourceStart}, ${clip.sourceEnd})`);
        button.onclick = () => { this.panels.setOpen('arrangement', true); requestAnimationFrame(() => document.dispatchEvent(new CustomEvent('select-clips',{detail:[clip.clipId]}))); };
        row.append(button);
      }
      assets.append(row);
    }
    if (!view.document.assets.length) uiText(assets, "오디오 파일을 가져오면 Asset과 Clip이 생성됩니다.");
    this.controls();
  }
  private async importMidi(): Promise<void> {
    const path = await open({title:tr('MIDI 가져오기'),multiple:false,filters:[{name:'Standard MIDI',extensions:['mid','midi']}]});
    if (typeof path !== 'string') return;
    await this.refresh();
    const choice = await this.ask('MIDI 파일의 Tempo/박자도 적용하시겠습니까? 기존 Audio Clip은 초 위치를 유지합니다.', [['tempo','파일 Tempo/박자 적용'],['keep','현재 프로젝트 유지'],['cancel','취소']]);
    if (choice === 'cancel') return;
    this.render(await invoke<ProjectView>('edit_project',{revision:this.view!.revision,request:{command:'midi.import',path,importTempo:choice==='tempo'}}));
    this.panels.setOpen('arrangement',true);
  }
  private async exportMidi(): Promise<void> {
    await this.refresh(); const v=this.view!;
    const path=await save({title:tr('MIDI 내보내기 · SMF Type 1'),defaultPath:`${v.document.name}.mid`,filters:[{name:'Standard MIDI',extensions:['mid']}]});
    if (path) await invoke('export_midi',{path,revision:v.revision});
  }
  private async saveCurrent(saveAs: boolean): Promise<boolean> {
    await this.refresh(); const view = this.view!;
    let path: string | null = null;
    if (saveAs || !view.path) {
      path = await save({ title: tr("MiniDAW 프로젝트 저장"), defaultPath: view.path ?? `${view.document.name}.minidaw`, filters: [{ name: tr("MiniDAW 프로젝트"), extensions: ["minidaw"] }] });
      if (!path) return false;
    }
    this.render(await invoke<ProjectView>("save_project", { path, revision: view.revision })); return true;
  }
  private async confirmChanges(): Promise<boolean> {
    await this.refresh(); if (!this.view?.dirty) return true;
    const choice = await this.ask("변경 사항을 저장하시겠습니까?", [["save", "저장"], ["discard", "저장하지 않음"], ["cancel", "취소"]]);
    if (choice === "save") return this.saveCurrent(false);
    return choice === "discard";
  }
  private async chooseProject(): Promise<void> {
    const path = await open({ title: tr("MiniDAW 프로젝트 열기"), multiple: false, directory: false, filters: [{ name: tr("MiniDAW 프로젝트"), extensions: ["minidaw"] }] });
    if (typeof path === "string") await this.replace(path);
  }
  async openDroppedProject(path: string): Promise<void> { await this.run(() => this.replace(path)); }
  async edit(request:unknown):Promise<void>{await this.run(async()=>{await this.refresh();this.render(await invoke<ProjectView>('edit_project',{revision:this.view!.revision,request}));});}
  async editFromWindow(request:unknown):Promise<void>{
    while(this.busy&&!this.blocked())await new Promise(r=>setTimeout(r,20));
    if(this.blocked())throw new Error(tr('파일 또는 오디오 작업이 끝난 뒤 다시 시도해 주세요.'));
    this.busy=true;this.controls();this.changed();
    try{await this.refresh();this.render(await invoke<ProjectView>('edit_project',{revision:this.view!.revision,request}));}
    finally{this.busy=false;this.controls();this.changed();}
  }
  private async replace(path?: string): Promise<void> {
    if (!(await this.confirmChanges())) return;
    await this.runtimeChange(async () => {
      this.render(await invoke<ProjectView>(path ? "open_project" : "new_project", { path, revision: this.view!.revision, discard: true }));
    });
  }
  private async relink(assetId: string): Promise<void> {
    const path = await open({ title: tr("원본 오디오 파일 다시 연결"), multiple: false, directory: false, filters: [{ name: tr("오디오 파일"), extensions: ["wav", "mp3", "flac"] }] });
    if (typeof path !== "string") return;
    await this.refresh(); const revision = this.view!.revision;
    let result: RelinkResult | undefined;
    const apply = async (confirmation: string | null) => this.runtimeChange(async () => { result = await invoke<RelinkResult>("relink_asset", { assetId, path, revision, confirmation }); });
    await apply(null);
    if (result?.confirmation) {
      const choice = await this.ask(`다른 파일일 수 있습니다. 기존 Clip ID와 편집 범위를 유지한 채 대체하시겠습니까?\n${result.differences.join("\n")}`, [["replace", "이 파일로 대체"], ["cancel", "취소"]]);
      if (choice !== "replace") return;
      await apply(result.confirmation);
      if (result?.confirmation) throw { message: "확인 중 파일이 변경되었습니다. 다시 연결해 주세요." };
    }
    if (result?.view) this.render(result.view);
  }
  private ask(message: string, choices: [string, string][]): Promise<string> {
    const dialog = element<HTMLDialogElement>("project-confirm"); uiText(element("project-confirm-text"), message);
    const actions = element("project-confirm-actions"); actions.replaceChildren();
    return new Promise(resolve => {
      const finish = (choice: string) => { dialog.oncancel = null; dialog.onclose = null; dialog.close(); resolve(choice); };
      for (const [choice, label] of choices) { const button = document.createElement("button"); button.id = `project-confirm-${choice}`; uiText(button, label); button.autofocus = choice === "cancel"; button.onclick = () => finish(choice); actions.append(button); }
      dialog.oncancel = event => { event.preventDefault(); finish("cancel"); }; dialog.onclose = () => finish("cancel"); dialog.showModal();
    });
  }
}
