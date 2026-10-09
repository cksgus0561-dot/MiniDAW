import { uiAttr, uiText } from './i18n';
import { globalShortcut } from './keyboard';
import { refreshShortcutLabels } from './shortcuts';
import {choosePlugin,openPlugin,type PluginSelection} from './plugins';
import {AudioTempoSync, sync, syncTicks} from './audio-tempo-sync';
import { AudioPitch } from './audio-pitch';
import { AudioStretch, recipe as stretchRecipe, ratio as stretchRatio, originalTime, renderedTime, STRETCH } from './audio-stretch';
import { ClipStart } from './clip-start';
import { AutomationLanes } from './automation';
import type { MidiClip } from './midi';
import { TrackControls, type TrackMix } from './track-controls';
import { invoke } from '@tauri-apps/api/core';
import { element } from './dom';
import { commands, keyCommand, type CommandId } from './commands';
import type { Waveform } from './waveform';
import type { WaveformView } from './types';
type Position = {
    unit: 'seconds';
    numerator: string;
    denominator: number;
} | {
    unit: 'ticks';
    ticks: string;
};
interface Envelope {
    sourceStart: string;
    sourceEnd: string;
    fadeIn: {
        sourceFrames: string;
        curve: string;
    };
    fadeOut: {
        sourceFrames: string;
        curve: string;
    };
}
interface AudioClip extends Envelope {
    kind: 'audio';
    clipId: string;
    assetId: string;
    name: string;
    position: Position;
    gain: number;
    mute: boolean;
    extensions?: Record<string, unknown>;
}
type Clip = AudioClip | MidiClip;
interface Asset {
    assetId: string;
    metadata: {
        sampleRate: number;
        sourceFrames: string;
    };
}
interface View {
    document: {
        projectId: string;
        assets: Asset[];
        tracks: {
            trackId: string;
            name: string;
            kind: 'audio' | 'midi';
            instrument?: 'none' | 'basicSynth' | 'external';
            extensions?:Record<string,unknown>;
            mix?: TrackMix;
            clips: Clip[];
        }[];
        musicalTime: {
            ticksPerQuarter: number;
            tempoMap: {
                tick: string;
                bpm: number;
            }[];
        };
    };
    revision: number;
    history: {
        undo: number;
        redo: number;
    };
    clipboardCount: number;
    assets: {
        assetId: string;
        status: string;
    }[];
}
interface Item {
    clip: Clip;
    trackId: string;
    start: number;
    end: number;
    rate: number;
    lane: number;
    top: number;
    height: number;
    color: string;
}
interface TrackRow { id: string; name: string; top: number; height: number; minRem: number; lanes: number; items: Item[]; color: string }
interface TrackResize { id: string; pointer: number; y: number; scroll: number; height: number; previous?: number }
const MIN_LANE_REM = 4.5, MAX_LANE_REM = 24;
// MIDI name, instrument and Add Clip rows need room above the 6px resize target.
const MIN_MIDI_TRACK_REM = 6;
interface Gesture {
    pointer: number;
    kind: string;
    originX: number;
    originY: number;
    toggleOnClick: boolean;
    additive: boolean;
    constraint?: 'horizontal' | 'vertical';
    targetTrackId?: string;
    invalidTrack?: boolean;
    item: Item | null;
    items: Item[];
    start: number;
    end: number;
    delta: number;
    targetTick?: number;
    stretchFrames?: number;
    moved: boolean;
    track: number;
}
export class Editing {
    private view: View | null = null;
    private tool: 'objectSelection' | 'rangeSelection' | 'split' | 'glue' = 'objectSelection';
    private selected = new Set<string>();
    private expandedParts = new Set<string>();
    private partNodes = new Map<string, HTMLElement>();
    private partId(item: Item): string | undefined {
        const p = item.clip.kind === 'audio' ? item.clip.extensions?.['minidaw.audioPart.v1'] as {partId:string} | undefined : undefined;
        return p ? `${item.trackId}:${p.partId}` : undefined;
    }
    private groupIds(item: Item): string[] {
        const part = this.partId(item);
        return part && !this.expandedParts.has(part) ? this.items.filter(i=>this.partId(i)===part).map(i=>i.clip.clipId) : [item.clip.clipId];
    }
    private renderParts(): void {
        const old=this.partNodes;this.partNodes=new Map();
        const groups = new Map<string, Item[]>();
        for(const i of this.items) { const p=this.partId(i); if(p&&!this.expandedParts.has(p)){const g=groups.get(p)??[];g.push(i);groups.set(p,g);} }
        const v=this.wave.viewport;
        for(const [id,items] of groups) {
            const first=items[0],start=first.start,end=Math.max(...items.map(i=>i.end));
            if(end<=v.start||start>=v.start+v.span||first.top+first.height<this.trackScroll.scrollTop||first.top>this.trackScroll.scrollTop+this.trackScroll.clientHeight)continue;
            const n=old.get(id)??document.createElement('div');old.delete(id);n.className='audio-part';n.dataset.clipId=first.clip.clipId;n.dataset.audioPart=id;n.style.setProperty('--track-color',first.color);
            this.place(n,{...first,start,end});uiAttr(n, 'title', 'Audio Part · 두 번 클릭: 내부 Event 펼치기 / 접기');n.setAttribute('role','option');
            const title=n.querySelector<HTMLElement>('.clip-name')??document.createElement('span');title.className='clip-name';title.style.left=`${Math.max(0,-parseFloat(n.style.left))+10}px`;uiText(title, `▣ ${first.clip.name} · ${items.length} Events`);n.append(title);if(!n.isConnected)this.layer.append(n);this.partNodes.set(id,n);
        }
        for(const n of old.values())n.remove();
    }
    private range: {
        start: number;
        end: number;
        trackIds: string[];
    } | null = null;
    private items: Item[] = [];
    private prefix: number[] = [];
    private tracks: TrackRow[] = [];
    // View state only: total requested height per Track, in font-scaled rem units.
    private trackHeights = new Map<string, number>();
    private trackResize: TrackResize | null = null;
    private resizeFrame = 0;
    private rootSize = 0;
    private trackScroll = element('track-scroll');
    private scrollScheduled = false;
    private gesture: Gesture | null = null;
    private layer = element('clip-layer');
    private context = element('clip-context');
    private rangeBox = element('edit-range');
    private cache = new Map<string, WaveformView>();
    private inflight = new Map<string, Promise<(WaveformView & {
        complete: boolean;
    }) | null>>();
    private nodes = new Map<string, {
        node: HTMLElement;
        item: Item;
        peaks: WaveformView | null;
    }>();
    private generation = 0;
    private pending = false;
    private busy = false;
    private selectedTrackId: string | null = null;
    private trackControls: TrackControls;
    private automation: AutomationLanes;
    private clipStart: ClipStart;
    private stretch: AudioStretch;
    private pitch: AudioPitch;
    private tempoSync: AudioTempoSync;
    automationDisplay(seconds:number,playing:boolean):void {this.automation.display(seconds,playing);}
    audioImportTrack(): string | null { return this.view?.document.tracks.find(t=>t.trackId===this.selectedTrackId&&t.kind==='audio')?.trackId ?? null; }
    private queries: Promise<void> = Promise.resolve();
    private wanted = new Set<string>();
    private assetKeys = new Map<string, string>();
    private lastViewport = '';
    private cacheEpoch = 0;
    public cursor = 0;
    public drawCount = 0;
    private displayTime: { label: string; seconds: number } | null = null;
    private showPosition(label: string, seconds: number): void {
        this.displayTime = { label, seconds };
        const output = element('edit-position');
        uiText(output, `${label} ${this.wave.editPosition(seconds)}`);
        uiAttr(output, 'title', `Bar.Beat.Sixteenth.Tick · PPQ ${this.wave.musical.time.ticksPerQuarter}\n${seconds.toFixed(9)} s · Trim/Split은 원본 sample 경계`);
    }
    spectrumSelection(): { revision: number; clipIds: string[]; trackIds: string[]; start: number | null; end: number | null } {
        const range = this.tool === 'rangeSelection' ? this.range : null;
        return { revision: this.view?.revision ?? 0, clipIds: this.items.filter(i => i.clip.kind === 'audio' && this.selected.has(i.clip.clipId)).map(i => i.clip.clipId), trackIds: range?.trackIds ?? [], start: range?.start ?? null, end: range?.end ?? null };
    }
    constructor(private wave: Waveform, private edit: (request: unknown) => Promise<void>, private error: (e: unknown) => void) {
        this.tempoSync=new AudioTempoSync({clips:()=>{const chosen=this.items.filter(i=>this.selected.has(i.clip.clipId));return chosen.every(i=>i.clip.kind==='audio')?chosen.map(i=>i.clip):[];},bpm:()=>this.wave.musical.time.tempoMap[0].bpm,busy:()=>this.busy||!!this.gesture,apply:(ids,fields)=>this.commitGesture('audio.tempoSync',{clipIds:ids,...fields})});
        this.pitch=new AudioPitch({clip:()=>{const chosen=this.items.filter(i=>this.selected.has(i.clip.clipId));return chosen.length===1&&chosen[0].clip.kind==='audio'?chosen[0].clip:undefined;},busy:()=>this.busy||!!this.gesture,apply:(id,pitchShift)=>this.commitGesture('audio.pitch',{clipIds:[id],pitchShift})});
        this.stretch=new AudioStretch({item:()=>{const chosen=this.items.filter(i=>this.selected.has(i.clip.clipId));const i=chosen[0];return chosen.length===1&&i.clip.kind==='audio'?{clip:i.clip,rate:i.rate}:undefined;},busy:()=>this.busy||!!this.gesture,apply:(id,frames)=>this.commitGesture('audio.stretch',{clipIds:[id],stretchFrames:frames})});
        this.clipStart = new ClipStart({items:()=>[...this.selected].flatMap(id=>{const i=this.items.find(i=>i.clip.clipId===id);return i?[{id,kind:i.clip.kind,start:i.start,position:i.clip.kind==='audio'?i.clip.position:{unit:'ticks' as const,ticks:i.clip.startTick}}]:[];}),clock:()=>this.wave.musical,busy:()=>this.busy||!!this.gesture||!!this.trackResize,move:(command,r)=>this.commitGesture(command,r)});
        this.automation = new AutomationLanes(wave, async r => { await this.edit(r); }, () => { this.layoutTracks(); this.render(); });
        this.trackControls = new TrackControls(async request => { if(this.busy)return;const {command,...args}=request;await this.commitGesture(String(command),args); },
            id=>this.canTransfer(id), id=>{void this.commitGesture('clip.moveTrack',{targetTrackId:id});});
        wave.editing = true;
        wave.viewportChanged = () => this.render();
        this.layer.tabIndex = 0;
        const menu = (id: string, filter: (id: string) => boolean) => { const root = element(id); for (const [id, label, key] of commands.filter(c => filter(c[0]))) {
            const b = document.createElement('button');
            b.dataset.command = id;
            uiText(b, `${label}${key ? '   ' + key : ''}`);
            b.onclick = () => { this.context.hidden = true; b.closest('details')?.removeAttribute('open'); void this.command(id); };
            root.append(b);
        } };
        menu('edit-menu-items', id => id.startsWith('edit.'));
        menu('audio-menu-items', id => id.startsWith('audio.'));
        menu('clip-context', id => !id.startsWith('tool.'));
        for (const [id, label, key] of commands.filter(c => c[0].startsWith('tool.'))) {
            const b = document.createElement('button');
            b.dataset.command = id;
            uiText(b, `${key} ${label}`);
            b.onclick = () => void this.command(id);
            element('edit-tools').append(b);
        }
        globalShortcut(e => {
            const target=e.target as HTMLElement;
            if(target.closest('#panel-piano'))return;
            const command=keyCommand(e);
            if(target.closest('.automation-header,.automation-canvas,#automation-point-editor')&&command&&['edit.delete','edit.cut','edit.copy','edit.paste','edit.duplicate','audio.crossfade'].includes(command))return;
            if(command)return()=>{void this.command(command);};
        });
        refreshShortcutLabels();
        document.addEventListener('project-view', e => this.accept((e as CustomEvent<View>).detail));
        document.addEventListener('ruler-format-changed', () => {
            if (this.displayTime) this.showPosition(this.displayTime.label, this.displayTime.seconds);
        });
        document.addEventListener('select-clips', e => {
            this.selected = new Set((e as CustomEvent<string[]>).detail.flatMap(id=>{const i=this.items.find(i=>i.clip.clipId===id);return i?this.groupIds(i):[id];}));
            const item = this.items.find(i => this.selected.has(i.clip.clipId));
            if (item) { this.trackScroll.scrollTop = Math.max(0, item.top - 8); this.wave.reveal(item.start, item.end); }
            this.render(); this.selection();
        });
        document.addEventListener('workspace-changing', () => this.cancel());
        this.trackScroll.addEventListener('pointerdown', e => this.startTrackResize(e), true);
        this.trackScroll.addEventListener('pointermove', e => this.moveTrackResize(e));
        this.trackScroll.addEventListener('pointerup', e => {
            if (this.trackResize?.pointer !== e.pointerId) return;
            this.moveTrackResize(e); this.finishTrackResize(false);
        });
        for (const type of ['pointercancel', 'lostpointercapture'])
            this.trackScroll.addEventListener(type, () => this.finishTrackResize(true));
        this.trackScroll.addEventListener('scroll', () => {
            if (this.scrollScheduled) return;
            this.scrollScheduled = true;
            requestAnimationFrame(() => { this.scrollScheduled = false; this.render(); });
        });
        new ResizeObserver(() => {
            const width = this.trackScroll.offsetWidth - this.trackScroll.clientWidth;
            element('drop-area').style.setProperty('--track-scrollbar', `${width}px`);
            this.render();
        }).observe(this.trackScroll);
        document.addEventListener('pointerdown', e => { if (!(e.target as HTMLElement).closest('#clip-context'))
            this.context.hidden = true; });
        this.layer.addEventListener('contextmenu', e => { e.preventDefault(); const id = (e.target as HTMLElement).closest<HTMLElement>('[data-clip-id]')?.dataset.clipId; if (id && !this.selected.has(id)) {
            const item=this.items.find(i=>i.clip.clipId===id);this.selected = new Set(item?this.groupIds(item):[id]);
            this.selection();
        } this.context.hidden = false; this.context.style.left = `${Math.max(0, Math.min(innerWidth - 260, e.clientX))}px`; this.context.style.top = `${Math.max(0, Math.min(innerHeight - 520, e.clientY))}px`; });
        this.layer.addEventListener('pointerdown', e => this.down(e));
        this.layer.addEventListener('pointermove', e => this.move(e));
        this.layer.addEventListener('pointerup', e => this.up(e));
        for (const type of ['pointercancel', 'lostpointercapture'])
            this.layer.addEventListener(type, () => this.cancel());
        window.addEventListener('blur', () => this.cancel());
        element<HTMLInputElement>('clip-gain').addEventListener('change', () => void this.command('audio.gain', { gainDb: Number(element<HTMLInputElement>('clip-gain').value) }));
        element('apply-fade').addEventListener('click', () => { const item = this.items.find(i => this.selected.has(i.clip.clipId)); if (!item)
            return; void this.command('audio.fade', { fadeIn: String(Math.round(Number(element<HTMLInputElement>('fade-in-ms').value) * item.rate / 1000)), fadeOut: String(Math.round(Number(element<HTMLInputElement>('fade-out-ms').value) * item.rate / 1000)), curve: element<HTMLSelectElement>('fade-curve').value }); });
        element('normalize-apply').addEventListener('click', () => { element<HTMLDialogElement>('normalize-dialog').close(); void this.command('audio.normalize', { normalizeTargetDb: Number(element<HTMLInputElement>('normalize-target').value) }); });
        element('normalize-cancel').addEventListener('click', () => element<HTMLDialogElement>('normalize-dialog').close());
        element('midi-track-add').onclick = () => { void this.addTrack('midi'); };
        element('audio-track-add').onclick = () => { void this.addTrack('audio'); };
        element('piano-open').onclick = () => this.selectMidi(true);
        for(const [id,replace] of [['bounce-replace',true],['bounce-keep',false]] as const) element(id).onclick=()=>{element<HTMLDialogElement>('bounce-dialog').close();void this.command('audio.consolidate',{bounceReplace:replace});};
        element('bounce-cancel').onclick=()=>element<HTMLDialogElement>('bounce-dialog').close();
        document.addEventListener('place-asset',e=>{void this.placeAsset((e as CustomEvent<string>).detail);});
        this.layer.addEventListener('dblclick', e => {
            // Pointer capture retargets click/dblclick to the layer. Hit-test the
            // released pointer to recover the actual Event/Part beneath it.
            const target=(e.target as HTMLElement).closest<HTMLElement>('[data-clip-id]')??document.elementFromPoint(e.clientX,e.clientY)?.closest<HTMLElement>('[data-clip-id]');
            const item = this.items.find(i => i.clip.clipId === target?.dataset.clipId);
            const part=item?this.partId(item):undefined;
            if(part){if(this.expandedParts.has(part))this.expandedParts.delete(part);else this.expandedParts.add(part);this.layoutTracks();this.render();return;}
            if (item?.clip.kind === 'midi') { if(!this.selected.has(item.clip.clipId))this.selected = new Set([item.clip.clipId]); this.selection(); this.selectMidi(true); }
        });
        document.addEventListener('piano-part-activated',e=>{
            const id=(e as CustomEvent<{clipId:string}>).detail.clipId;
            const item=this.items.find(i=>i.clip.clipId===id&&i.clip.kind==='midi');
            if(item&&this.selected.has(id)){this.selectedTrackId=item.trackId;this.selection();}
        });
        this.selection();
    }
    exportSelection(): {start:number|null;end:number|null;trackIds:string[]} {
        const range=this.tool==='rangeSelection'?this.range:null;
        const ids=[...new Set(this.items.filter(i=>this.selected.has(i.clip.clipId)).map(i=>i.trackId))];
        return {start:range?.start??null,end:range?.end??null,trackIds:range?.trackIds??(ids.length?ids:this.selectedTrackId?[this.selectedTrackId]:[])};
    }
    private selectMidi(open = false): void {
        const clipIds = this.items.filter(i => this.selected.has(i.clip.clipId) && i.clip.kind === 'midi').map(i=>i.clip.clipId);
        document.dispatchEvent(new CustomEvent('midi-clip-selected', { detail: { clipId:clipIds[0]??null, clipIds, open } }));
    }
    private canTransfer(id: string): boolean {
        const target=this.view?.document.tracks.find(t=>t.trackId===id),items=this.items.filter(i=>this.selected.has(i.clip.clipId));
        return !!target&&items.length>0&&items.every(i=>i.clip.kind===target.kind)&&items.some(i=>i.trackId!==id);
    }
    private async placeAsset(assetId:string): Promise<void> {
        if(this.busy)return;
        const before=new Set(this.items.map(i=>i.clip.clipId));
        await this.commitGesture('audio.placeAsset',{assetId,trackIds:this.audioImportTrack()?[this.audioImportTrack()!]:[],cursor:{unit:'ticks',ticks:String(this.wave.editTick(this.cursor))}});
        const created=this.items.find(i=>!before.has(i.clip.clipId));
        if(created){this.selected=new Set([created.clip.clipId]);this.wave.reveal(created.start,created.end);this.selection();}
    }
    private async addTrack(kind: 'audio' | 'midi'): Promise<void> {
        if(this.busy)return;const before=new Set(this.view?.document.tracks.map(t=>t.trackId));
        await this.commitGesture('track.add',{trackKind:kind});this.selectedTrackId=this.view?.document.tracks.find(t=>!before.has(t.trackId))?.trackId??null;this.selected.clear();this.selection();
    }
    private async addMidiClip(trackId: string): Promise<void> {
        if (this.busy) return;
        const before = new Set(this.items.map(i => i.clip.clipId)), tick = this.wave.editTick(this.cursor);
        await this.commitGesture('midi.clip.add', { trackIds: [trackId], targetTick: String(tick), lengthTick: String(this.wave.musical.signature(tick).barTicks * 4) });
        const created = this.items.find(i => !before.has(i.clip.clipId) && i.clip.kind === 'midi');
        if (created) { this.selected = new Set([created.clip.clipId]); this.wave.reveal(created.start, created.end); this.selection(); this.selectMidi(true); }
    }
    private seconds(p: Position): number { return this.wave.musical.position(p); }
    private accept(v: View): void {
        if (this.view?.revision === v.revision && this.view.document.projectId === v.document.projectId) {
            this.view = v;
            this.selection();
            return;
        }
        const changed = this.view?.document.projectId !== v.document.projectId;
        this.cancel();
        if (JSON.stringify(this.view?.document.assets) !== JSON.stringify(v.document.assets)) {
            this.cache.clear();
            this.cacheEpoch++;
        }
        this.view = v;
        this.assetKeys = new Map(v.document.assets.map(a => [a.assetId, JSON.stringify(a)]));
        if (changed) {
            this.expandedParts.clear();
            this.selectedTrackId = null;
            this.trackHeights.clear();
            this.selected.clear();
            this.range = null;
            this.cache.clear();
            this.cursor = 0;
        }
        this.items = v.document.tracks.flatMap(t => t.clips.map(clip => {
            const rate = clip.kind === 'audio' ? v.document.assets.find(a => a.assetId === clip.assetId)!.metadata.sampleRate : 1;
            const start = clip.kind === 'audio' ? this.seconds(clip.position) : this.wave.musical.seconds(Number(clip.startTick));
            const synced=clip.kind==='audio'?syncTicks(clip):null;
            const end = clip.kind === 'audio' ? synced!==null?this.wave.musical.seconds(Number((clip.position as {ticks:string}).ticks)+synced):start + (Number(clip.sourceEnd) - Number(clip.sourceStart)) / rate : this.wave.musical.seconds(Number(clip.startTick) + Number(clip.lengthTick));
            return { clip, trackId: t.trackId, rate, start, end, lane: 0, top: 0, height: 0, color: '' };
        })).sort((a, b) => a.start - b.start || a.clip.clipId.localeCompare(b.clip.clipId));
        this.layoutTracks();
        let end = 0;
        this.prefix = this.items.map(i => end = Math.max(end, i.end));
        this.selected = new Set([...this.selected].filter(id => this.items.some(i => i.clip.clipId === id)));
        if (!v.document.tracks.some(t=>t.trackId===this.selectedTrackId)) this.selectedTrackId=null;
        const cycle = this.wave.cycle;
        this.wave.setTimeline(v.document.projectId, end > 0 ? Math.max(end, cycle ? this.wave.musical.seconds(Number(cycle.endTick)) : 0) : 0);
        this.render();
        this.selection();
    }
    private layoutTracks(): void {
        this.rootSize = parseFloat(getComputedStyle(document.documentElement).fontSize);
        const colors = ['#80aaa0', '#8c9fb6', '#ad9e82', '#a796b1'];
        const byId = new Map(this.items.map(item => [item.clip.clipId, item]));
        let top = 0;
        this.tracks = (this.view?.document.tracks ?? []).map((track, index) => {
            // Document order is stable across Move/Trim and restored by Undo/Redo.
            // Keep the time-sorted this.items only for viewport lookup; never repack rows by time.
            const items = track.clips.map(clip => byId.get(clip.clipId)!);
            const rows = new Map<string, number>();
            items.forEach(item => {
                const part=this.partId(item), key=part&&!this.expandedParts.has(part)?part:item.clip.clipId;
                if(!rows.has(key))rows.set(key,rows.size);
                item.lane = rows.get(key)!; item.color = colors[index % colors.length];
            });
            const lanes = Math.max(1, rows.size);
            const minRem = Math.max(lanes * MIN_LANE_REM, track.kind === 'midi' ? MIN_MIDI_TRACK_REM : 0);
            const height = Math.round(Math.max(minRem, Math.min(lanes * MAX_LANE_REM, this.trackHeights.get(track.trackId) ?? lanes * 6.4)) * this.rootSize);
            for (const item of items) { item.top = top + item.lane * height / lanes; item.height = height / lanes; }
            const row = { id: track.trackId, name: track.name, top, height, minRem, lanes, items, color: colors[index % colors.length] };
            top += row.height + this.automation.height(track.trackId);
            return row;
        });
        uiText(element('track-count'), `${this.tracks.length} Tracks · ${this.items.length} Events`);
        this.layer.style.height = `${top + this.automation.height('master')}px`;
        this.automation.clear();
        const list = element('track-list'); list.replaceChildren();
        const anySolo=this.view?.document.tracks.some(t=>t.mix?.solo)??false;
        this.trackControls.update(this.view?.document.tracks??[]);
        this.layer.querySelectorAll('.track-band, .track-resize').forEach(n => n.remove());
        for (const row of this.tracks) {
            const header = document.createElement('div'); header.className = 'track-header'; header.dataset.trackId = row.id;
            header.style.height = `${row.height}px`; header.style.setProperty('--track-color', row.color);
            const button = document.createElement('button'); button.className = 'track-label';
            const title = document.createElement('strong'); uiText(title, row.name, false); uiAttr(title, 'title', row.name, false);
            const description = document.createElement('span'); uiText(description, `${row.items.length} Events${row.lanes > 1 ? ` · ${row.lanes} 표시 행` : ''}`);
            button.append(title, description); uiAttr(button, 'title', `${row.name}의 Clip 선택`);
            button.onclick = () => { this.selectedTrackId=row.id;this.selected = new Set(row.items.map(i => i.clip.clipId)); this.range = null; this.paintRange(); this.selection(); };
            header.append(button);
            const track = this.view?.document.tracks.find(t => t.trackId === row.id);
            if(track){header.append(this.trackControls.header(track,anySolo));header.classList.toggle('track-inaudible',!!track.mix?.mute||(anySolo&&!track.mix?.solo));}
            if (track?.kind === 'midi') {
                header.classList.add('midi-track-header');
                const routing = document.createElement('select'); routing.className = 'midi-instrument';
                uiAttr(routing, 'aria-label', `${row.name} 악기 출력`);
                uiAttr(routing, 'title', 'MIDI Track Instrument → Mixer → Master');
                const external=track.extensions?.['minidaw.plugin.v1'] as PluginSelection|undefined;
                for (const [value, text] of [['none', '악기 없음'], ['basicSynth', 'MiniDAW Synth'],...(track.instrument==='external'?[['external',external?.descriptor.name??'External Instrument']]:[]),['choose','VST3 / CLAP 선택…']]) {
                    const option = document.createElement('option'); option.value = value; uiText(option, text); routing.append(option);
                }
                routing.value = track.instrument ?? 'none'; routing.disabled = this.busy;
                routing.onchange = () => {if(routing.value==='choose'){routing.value=track.instrument??'none';choosePlugin(row.id,'instrument');}else void this.edit({command:'midi.track.instrument', trackIds:[row.id], instrument:routing.value}); };
                const routeRow=document.createElement('div');routeRow.className='instrument-route-row';routeRow.append(routing);if(track.instrument==='external'){const editor=document.createElement('button');uiText(editor, 'e');uiAttr(editor, 'title', 'Plugin Editor 열기');editor.onclick=()=>openPlugin(row.id);routeRow.append(editor);}header.append(routeRow);
                const add = document.createElement('button'); add.className = 'midi-clip-add'; uiText(add, '＋ MIDI Clip'); uiAttr(add, 'title', '커서 위치에 4마디 MIDI Clip 생성');
                add.onclick = () => { void this.addMidiClip(row.id); }; header.append(add);
            }
            header.append(this.resizeHandle(row)); list.append(header);
            const band = document.createElement('div'); band.className = 'track-band'; band.style.top = `${row.top}px`; band.style.height = `${row.height}px`;
            band.style.setProperty('--lane-height', `${row.height / row.lanes}px`); this.layer.prepend(band);
            const boundary = this.resizeHandle(row); boundary.style.top = `${row.top + row.height - 6}px`; this.layer.append(boundary);
            this.automation.mount(row.id,row.top+row.height,list,this.layer);
        }
        this.automation.mount('master',top,list,this.layer);
        if (!this.tracks.length) { const hint = document.createElement('p'); hint.className = 'metric-note'; uiText(hint, 'Track 없음'); list.append(hint); }
        this.lastViewport = '';
    }
    private resizeHandle(row: TrackRow): HTMLElement {
        const handle = document.createElement('div');
        handle.className = 'track-resize'; handle.dataset.resizeTrack = row.id;
        handle.setAttribute('role', 'separator'); handle.setAttribute('aria-orientation', 'horizontal');
        uiAttr(handle, 'aria-label', `${row.name} 높이 조절`); uiAttr(handle, 'title', '위아래로 드래그하여 Track 높이 조절');
        return handle;
    }
    private startTrackResize(e: PointerEvent): void {
        const id = (e.target as HTMLElement).closest<HTMLElement>('[data-resize-track]')?.dataset.resizeTrack;
        if (!id || e.button !== 0) return;
        // Capture before the Clip layer sees this pointer: no selection, trim or split command.
        e.preventDefault(); e.stopPropagation();
        if (this.busy || this.gesture || this.trackResize || this.wave.dragging) return;
        const row = this.tracks.find(t => t.id === id)!;
        this.trackResize = { id, pointer: e.pointerId, y: e.clientY, scroll: this.trackScroll.scrollTop, height: row.height, previous: this.trackHeights.get(id) };
        this.trackScroll.classList.add('track-height-resizing');
        // Keep the scroll range while shrinking, so native scroll clamping cannot feed back into the drag.
        this.trackScroll.firstElementChild!.setAttribute('style', `min-height:${this.trackScroll.scrollHeight}px`);
        this.trackScroll.setPointerCapture(e.pointerId);
    }
    private moveTrackResize(e: PointerEvent): void {
        const drag = this.trackResize;
        if (!drag || drag.pointer !== e.pointerId) return;
        const row = this.tracks.find(t => t.id === drag.id)!;
        const pixels = drag.height + e.clientY - drag.y + this.trackScroll.scrollTop - drag.scroll;
        this.trackHeights.set(drag.id, Math.max(row.minRem, Math.min(row.lanes * MAX_LANE_REM, pixels / this.rootSize)));
        if (!this.resizeFrame) this.resizeFrame = requestAnimationFrame(() => {
            this.resizeFrame = 0;
            if (!this.trackResize) return;
            const content = this.trackScroll.firstElementChild as HTMLElement;
            content.style.minHeight = `${Math.max(content.offsetHeight, this.trackScroll.scrollHeight)}px`;
            this.layoutTracks(); this.render();
        });
    }
    private finishTrackResize(cancel: boolean): void {
        const drag = this.trackResize;
        if (!drag) return;
        this.trackResize = null;
        cancelAnimationFrame(this.resizeFrame); this.resizeFrame = 0;
        if (cancel) {
            if (drag.previous === undefined) this.trackHeights.delete(drag.id);
            else this.trackHeights.set(drag.id, drag.previous);
        }
        if (this.trackScroll.hasPointerCapture(drag.pointer)) this.trackScroll.releasePointerCapture(drag.pointer);
        this.layoutTracks();
        (this.trackScroll.firstElementChild as HTMLElement).style.removeProperty('min-height');
        this.trackScroll.classList.remove('track-height-resizing'); this.render();
    }
    private trackAt(clientY: number): number {
        const y = clientY - this.layer.getBoundingClientRect().top;
        const index = this.tracks.findIndex(t => y < t.top + t.height);
        return index < 0 ? Math.max(0, this.tracks.length - 1) : index;
    }
    private selection(): void {
        document.dispatchEvent(new CustomEvent('track-selected', { detail: { trackId: this.selectedTrackId } }));
        this.clipStart.render();
        this.stretch.render();
        this.pitch.render();
        this.tempoSync.render();
        this.trackControls.refresh();
        for(const header of document.querySelectorAll<HTMLElement>('.track-header'))header.classList.toggle('track-selected',header.dataset.trackId===this.selectedTrackId);
        for (const control of document.querySelectorAll<HTMLSelectElement>('.midi-instrument')) control.disabled = this.busy;
        for (const [id, entry] of this.nodes) {
            entry.node.classList.toggle('selected', this.selected.has(id));
            entry.node.setAttribute('aria-selected', String(this.selected.has(id)));
        }
        for(const [part,node] of this.partNodes) {const selected=this.items.some(i=>this.partId(i)===part&&this.selected.has(i.clip.clipId));node.classList.toggle('selected',selected);node.setAttribute('aria-selected',String(selected));}
        for (const b of document.querySelectorAll<HTMLButtonElement>('[data-command]')) {
            const id = b.dataset.command!;
            b.classList.toggle('active', id === `tool.${this.tool}`);
            b.disabled = this.busy || (!id.startsWith('tool.') && (id === 'edit.undo' ? !this.view?.history.undo : id === 'edit.redo' ? !this.view?.history.redo : id === 'edit.paste' ? !this.view?.clipboardCount : !this.selected.size && !this.range));
        }
        const chosen = this.items.find(i => this.selected.has(i.clip.clipId))?.clip;
        const c = chosen?.kind === 'audio' ? chosen : undefined;
        const midiSelected = chosen?.kind === 'midi';
        element<HTMLButtonElement>('piano-open').disabled = !this.items.some(i=>this.selected.has(i.clip.clipId)&&i.clip.kind==='midi');
        if (midiSelected) for (const b of document.querySelectorAll<HTMLButtonElement>('[data-command]')) {
            const id = b.dataset.command!;
            if (id.startsWith('audio.') || ['edit.cut','edit.copy','edit.paste','edit.duplicate'].includes(id)) b.disabled = true;
        }
        this.selectMidi();
        const gain = element<HTMLInputElement>('clip-gain');
        gain.disabled = !c || this.busy;
        if (document.activeElement !== gain)
            gain.value = c ? (20 * Math.log10(Math.max(1e-12, c.gain))).toFixed(2) : '0';
        const rate = this.items.find(i => i.clip === c)?.rate ?? 48000;
        for (const [id, n] of [['fade-in-ms', c?.fadeIn.sourceFrames], ['fade-out-ms', c?.fadeOut.sourceFrames]] as const) {
            const field = element<HTMLInputElement>(id);
            field.disabled = !c || this.busy;
            if (document.activeElement !== field)
                field.value = (Number(n ?? 0) / rate * 1000).toFixed(2);
        }
        element<HTMLButtonElement>('apply-fade').disabled = !c || this.busy;
        uiText(element('edit-selection'), this.range ? `범위 ${this.range.start.toFixed(3)}–${this.range.end.toFixed(3)}초` : `Clip ${this.selected.size}개 선택`);
        if (!this.gesture) this.showPosition(c ? 'Start' : '위치', chosen ? (chosen.kind === 'audio' ? this.seconds(chosen.position) : this.wave.musical.seconds(Number(chosen.startTick))) : this.cursor);
        this.layer.dataset.tool = this.tool;
    }
    async command(id: CommandId, extra: Record<string, unknown> = {}): Promise<void> {
        if (this.busy || this.trackResize)
            return;
        if (id.startsWith('tool.')) {
            this.cancel();
            this.tool = id.slice(5) as typeof this.tool;
            this.range = null;
            this.rangeBox.hidden = true;
            this.selection();
            return;
        }
        const selectedMidi = this.items.filter(i => this.selected.has(i.clip.clipId) && i.clip.kind === 'midi');
        if (selectedMidi.length && !['edit.undo', 'edit.redo'].includes(id)) {
            if (id === 'edit.delete') await this.commitGesture('midi.clip.delete', { clipIds: selectedMidi.map(i => i.clip.clipId) });
            return;
        }
        if (id === 'audio.gain' && extra.gainDb === undefined) {
            element('clip-gain').focus();
            return;
        }
        if (id === 'audio.fade' && extra.fadeIn === undefined && extra.fadeOut === undefined) {
            element('fade-in-ms').focus();
            return;
        }
        if(id==='audio.consolidate'&&extra.bounceReplace===undefined){element<HTMLDialogElement>('bounce-dialog').showModal();return;}
        if (id === 'audio.normalize' && extra.normalizeTargetDb === undefined) {
            element<HTMLDialogElement>('normalize-dialog').showModal();
            return;
        }
        const range = this.tool === 'rangeSelection' ? this.range : null;
        const snappedPosition = (s: number): Position => ({ unit: 'ticks', ticks: String(this.wave.editTick(s)) });
        const request = { command: id, clipIds: [...this.selected], trackIds: range?.trackIds ?? [], cursor: snappedPosition(range?.start ?? this.cursor), rangeEnd: range ? snappedPosition(range.end) : null, ...extra };
        this.busy = true;
        this.selection();
        const old = new Set(this.items.map(i => i.clip.clipId));
        if (id === 'audio.consolidate') uiText(element('edit-selection'), 'Bounce 렌더링 중… 재생 제어는 계속 사용할 수 있습니다.');
        if (id === 'audio.normalize')
            uiText(element('edit-selection'), 'Peak 분석 중… 재생 제어는 계속 사용할 수 있습니다.');
        try {
            await this.edit(request);
            const created = this.items.filter(i => !old.has(i.clip.clipId));
            if (created.length)
                this.selected = new Set(created.map(i => i.clip.clipId));
        }
        catch (e) {
            this.error(e);
        }
        finally {
            this.busy = false;
            this.selection();
        }
    }
    private at(x: number): number { const r = this.layer.getBoundingClientRect(); const v = this.wave.viewport; return Math.max(0, v.start + (x - r.left) / Math.max(1, r.width) * v.span); }
    private down(e: PointerEvent): void {
        if (e.button !== 0 || this.busy || !this.view || this.gesture)
            return;
        this.layer.focus({ preventScroll: true });
        const node = (e.target as HTMLElement).closest<HTMLElement>('[data-clip-id]');
        const item = this.items.find(i => i.clip.clipId === node?.dataset.clipId) ?? null;
        if(item)this.selectedTrackId=item.trackId;
        if(this.tool==='glue') {
            if(item?.clip.kind==='audio') {
                const picked=this.items.filter(i=>this.selected.has(i.clip.clipId));
                const objects=new Set(picked.map(i=>{const p=this.partId(i);return p&&!this.expandedParts.has(p)?p:i.clip.clipId;}));
                if(e.altKey||!this.selected.has(item.clip.clipId)||objects.size<2) {
                    const later=this.items.filter(i=>i.trackId===item.trackId&&i.start>=item.start&&!this.groupIds(item).includes(i.clip.clipId));
                    const next=e.altKey?later:later.slice(0,1);
                    if(!next.length)return;
                    this.selected=new Set([...this.groupIds(item),...next.flatMap(i=>this.groupIds(i))]);
                }
                void this.command('audio.glue');
            }
            return;
        }
        if (this.tool === 'split') {
            if (item?.clip.kind === 'audio') {
                this.selected = new Set([item.clip.clipId]);
                this.cursor = this.at(e.clientX);
                void this.command('audio.splitAtCursor');
            }
            return;
        }
        if (this.tool === 'objectSelection') {
            if (!item) {
                this.selected.clear();
                this.range = null;
                this.selection();
                return;
            }
            if (e.ctrlKey) {
                // Defer Ctrl-click toggling until pointerup so Ctrl-drag can use
                // Cubase's horizontal/vertical direction constraint.
            } else if (!this.selected.has(item.clip.clipId)) {
                this.selected = new Set(this.groupIds(item));
            }
        }
        const toggleOnClick=!!item&&e.ctrlKey&&this.selected.has(item.clip.clipId);
        if(item&&(e.ctrlKey||this.tool==='objectSelection'))for(const id of this.groupIds(item))this.selected.add(id);
        e.preventDefault();
        this.layer.setPointerCapture(e.pointerId);
        let kind = this.tool === 'rangeSelection' ? 'range' : (e.target as HTMLElement).dataset.handle ?? 'move';
        if(item?.clip.kind==='audio'&&!sync(item.clip)?.enabled&&kind.startsWith('trim')&&element<HTMLSelectElement>('audio-sizing').value==='stretch')kind=kind.replace('trim','stretch');
        this.gesture = { pointer: e.pointerId, kind, originX: e.clientX, originY:e.clientY, toggleOnClick, additive:e.ctrlKey, item, items: this.items.filter(i => this.selected.has(i.clip.clipId) && i.clip.kind === item?.clip.kind), start: this.at(e.clientX), end: this.at(e.clientX), delta: 0, moved: false, track: this.trackAt(e.clientY) };
        if (kind === 'range') {
            this.selected.clear();
            this.gesture.start = this.wave.musical.seconds(this.wave.editTick(this.gesture.start));
            this.range = { start: this.gesture.start, end: this.gesture.start, trackIds: this.tracks[this.gesture.track] ? [this.tracks[this.gesture.track].id] : [] };
            this.rangeBox.hidden = false;
            this.paintRange();
        }
        this.selection();
    }
    private move(e: PointerEvent): void {
        const g = this.gesture;
        if (!g) {
            const tick = this.wave.editTick(this.at(e.clientX));
            this.showPosition(this.tool === 'split' ? 'Split' : '위치', this.wave.musical.seconds(tick));
            return;
        }
        if (g.pointer !== e.pointerId)
            return;
        const dx=e.clientX-g.originX,dy=e.clientY-g.originY;
        g.moved ||= Math.abs(dx)>2||(['move','range'].includes(g.kind)&&Math.abs(dy)>2);
        g.delta = this.at(e.clientX) - g.start;
        g.end = this.at(e.clientX);
        if (g.kind === 'range') {
            g.end = this.wave.musical.seconds(this.wave.editTick(g.end));
            const track = this.trackAt(e.clientY);
            this.range = { start: Math.min(g.start, g.end), end: Math.max(g.start, g.end), trackIds: this.tracks.slice(Math.min(track, g.track), Math.max(track, g.track) + 1).map(t => t.id) };
            this.paintRange();
            this.showPosition('Range', g.end);
            return;
        }
        const item = g.item!;
        let delta = Math.round(g.delta * item.rate) / item.rate;
        if (['move', 'trim-left', 'trim-right','stretch-left','stretch-right'].includes(g.kind)) {
            const anchor = g.kind.endsWith('-right') ? item.end : item.start;
            g.targetTick = this.wave.editTick(anchor + g.delta);
            delta = this.wave.musical.seconds(g.targetTick) - anchor;
        }
        if (g.kind === 'move') {
            if(e.ctrlKey&&g.moved&&!g.constraint)g.constraint=Math.abs(dy)>Math.abs(dx)?'vertical':'horizontal';
            const target=this.tracks[g.constraint==='horizontal'?g.track:this.trackAt(e.clientY)];
            const kind=this.view!.document.tracks.find(t=>t.trackId===target?.id)?.kind;
            g.invalidTrack=!!target&&kind!==item.clip.kind;
            g.targetTrackId=!g.invalidTrack&&target?.id!==item.trackId?target?.id:undefined;
            const horizontal=g.constraint!=='vertical'&&Math.abs(dx)>2;
            const minimum = item.start - Math.min(...g.items.map(i => i.start));
            g.targetTick = horizontal?Math.max(g.targetTick!, Math.ceil(this.wave.musical.ticks(minimum) - 1e-7)):undefined;
            delta = horizontal?this.wave.musical.seconds(g.targetTick!) - item.start:0;
            for(const h of document.querySelectorAll<HTMLElement>('.track-header'))h.classList.toggle('track-drop-target',h.dataset.trackId===g.targetTrackId);
            for (const i of g.items) {
                const node = this.nodes.get(i.clip.clipId)?.node;
                if (node) {
                    const shift=horizontal&&item.clip.kind==='midi'&&i.clip.kind==='midi'?this.wave.musical.seconds(Number(i.clip.startTick)+g.targetTick!-Number(item.clip.startTick))-i.start:delta;
                    node.style.transform=`translate(${shift/this.wave.viewport.span*this.wave.viewport.width}px,${g.targetTrackId?target.top-item.top:0}px)`;
                }
            }
            for(const [part,n] of this.partNodes)if(g.items.some(i=>this.partId(i)===part))n.style.transform=`translate(${delta/this.wave.viewport.span*this.wave.viewport.width}px,${g.targetTrackId?target.top-item.top:0}px)`;
            g.delta = delta;
            this.showPosition(g.invalidTrack?'다른 종류의 Track':g.targetTrackId?`Move → ${target.name}`:'Move', item.start + delta);
            return;
        }
        if (item.clip.kind === 'midi') {
            const c = { ...item.clip }, start = Number(c.startTick), end = start + Number(c.lengthTick);
            if (g.kind === 'trim-left') {
                g.targetTick = Math.max(0, Math.min(end - 1, g.targetTick!));
                c.contentOffsetTick = String(Number(c.contentOffsetTick ?? 0) + g.targetTick - start);
                c.startTick = String(g.targetTick); c.lengthTick = String(end - g.targetTick);
            } else if (g.kind === 'trim-right') {
                g.targetTick = Math.min(Number.MAX_SAFE_INTEGER, Math.max(start + 1, g.targetTick!));
                c.lengthTick = String(g.targetTick - start);
            } else return;
            const entry = this.nodes.get(c.clipId);
            if (entry) {
                this.place(entry.node, { ...item, clip: c, start: this.wave.musical.seconds(Number(c.startTick)), end: this.wave.musical.seconds(Number(c.startTick) + Number(c.lengthTick)) });
                this.paint(entry.node, c, 1, null);
            }
            this.showPosition(g.kind === 'trim-left' ? 'Part Start' : 'Part End', this.wave.musical.seconds(g.targetTick));
            return;
        }
        if(g.kind.startsWith('stretch')){
            const left=g.kind==='stretch-left',length=Number(item.clip.sourceEnd)-Number(item.clip.sourceStart),ratio=stretchRatio(item.clip);
            const min=Math.max(1,Math.ceil(length/ratio*.5)),max=Math.floor(length/ratio*2);
            g.stretchFrames=Math.max(min,Math.min(max,Math.round(length+(left?-delta:delta)*item.rate),left?Math.floor(item.end*item.rate):max));
            const c=structuredClone(item.clip),r=stretchRecipe(c)??{sourceStart:c.sourceStart,sourceEnd:c.sourceEnd,outputFrames:String(length)};
            const offset=stretchRecipe(c)?0:Number(c.sourceStart),factor=g.stretchFrames/length;
            c.sourceStart=String(Math.round((Number(c.sourceStart)-offset)*factor));c.sourceEnd=String(Number(c.sourceStart)+g.stretchFrames);
            c.extensions={...c.extensions,[STRETCH]:{...r,outputFrames:String(Math.max(Number(c.sourceEnd),Math.round(Number(r.outputFrames)*factor)))}};
            delete c.extensions['minidaw.envelopeWindow.v1'];
            c.fadeIn.sourceFrames=String(Math.round(Number(c.fadeIn.sourceFrames)*factor));c.fadeOut.sourceFrames=String(Math.round(Number(c.fadeOut.sourceFrames)*factor));
            const start=left?item.end-g.stretchFrames/item.rate:item.start,end=start+g.stretchFrames/item.rate,entry=this.nodes.get(c.clipId);
            if(entry){this.place(entry.node,{...item,clip:c,start,end});this.paint(entry.node,c,item.rate,entry.peaks);}
            this.showPosition(`Stretch ${(stretchRatio(c)*100).toFixed(2)}%`,left?start:end);return;
        }
        const c = structuredClone(item.clip);
        const maximum = Number(stretchRecipe(c)?.outputFrames ?? this.view!.document.assets.find(a => a.assetId === c.assetId)!.metadata.sourceFrames);
        if (c.extensions)
            delete c.extensions['minidaw.envelopeWindow.v1'];
        let start = item.start;
        if (g.kind === 'trim-left') {
            const value = Math.max(0, Number(c.sourceStart) - Math.floor(item.start * item.rate + 1e-7), Math.min(Number(c.sourceEnd) - 1, Number(c.sourceStart) + Math.round(delta * item.rate)));
            start += (value - Number(c.sourceStart)) / item.rate;
            c.sourceStart = String(value);
        }
        if (g.kind === 'trim-right')
            c.sourceEnd = String(Math.max(Number(c.sourceStart) + 1, Math.min(maximum, Number(c.sourceEnd) + Math.round(delta * item.rate))));
        if (g.kind === 'fade-in')
            c.fadeIn.sourceFrames = String(Math.max(0, Math.min(Number(c.sourceEnd) - Number(c.sourceStart), Math.round((g.end - item.start) * item.rate))));
        if (g.kind === 'fade-out')
            c.fadeOut.sourceFrames = String(Math.max(0, Math.min(Number(c.sourceEnd) - Number(c.sourceStart), Math.round((item.end - g.end) * item.rate))));
        const entry = this.nodes.get(c.clipId);
        if (entry) {
            this.place(entry.node, { ...item, clip: c, start, end: start + (Number(c.sourceEnd) - Number(c.sourceStart)) / item.rate });
            this.paint(entry.node, c, item.rate, entry.peaks);
        }
        if (g.kind.startsWith('trim')) this.showPosition(g.kind === 'trim-left' ? 'Trim Start' : 'Trim End', g.kind === 'trim-left' ? start : start + (Number(c.sourceEnd) - Number(c.sourceStart)) / item.rate);
    }
    private up(e: PointerEvent): void {
        const g = this.gesture;
        if (!g || g.pointer !== e.pointerId)
            return;
        this.move(e);
        this.gesture = null;
        if (this.layer.hasPointerCapture(e.pointerId))
            this.layer.releasePointerCapture(e.pointerId);
        if (g.kind === 'range') {
            this.selection();
            return;
        }
        if (!g.moved) {
            if (g.item) {
                if(g.toggleOnClick)for(const id of this.groupIds(g.item))this.selected.delete(id);
                else if(!g.additive)this.selected = new Set(this.groupIds(g.item));
            }
            this.selection();
            this.render();
            return;
        }
        if(g.kind==='move'&&(g.invalidTrack||(!g.targetTrackId&&g.targetTick===undefined))){this.render();return;}
        const item = g.item!;
        const entry = this.nodes.get(item.clip.clipId);
        const preview = entry?.node.dataset.preview ? JSON.parse(entry.node.dataset.preview) as Clip : item.clip;
        if (g.kind === 'move')
            void this.commitGesture(g.targetTrackId?'clip.moveTrack':item.clip.kind === 'midi' ? 'midi.clip.move' : 'audio.move', { clipIds: g.items.map(i => i.clip.clipId), ...(g.targetTick===undefined?{}:{targetTick:String(g.targetTick)}), ...(g.targetTrackId?{targetTrackId:g.targetTrackId}:{}), anchorClipId: item.clip.clipId });
        else if(g.kind.startsWith('stretch')){
            this.selected=new Set([item.clip.clipId]);
            void this.commitGesture('audio.stretch',{stretchFrames:String(g.stretchFrames),trimSide:g.kind==='stretch-left'?'left':'right'});
        }
        else if (g.kind.startsWith('trim')) {
            this.selected = new Set([item.clip.clipId]);
            void this.commitGesture(item.clip.kind === 'midi' ? 'midi.clip.resize' : 'audio.trim', { targetTick: String(g.targetTick), trimSide: g.kind === 'trim-left' ? 'left' : 'right' });
        }
        else {
            this.selected = new Set([item.clip.clipId]);
            if (preview.kind !== 'audio') return;
            void this.command('audio.fade', { fadeIn: preview.fadeIn.sourceFrames, fadeOut: preview.fadeOut.sourceFrames });
        }
    }
    private async commitGesture(command: string, extra: Record<string, unknown>): Promise<void> { this.busy = true; this.selection(); try {
        await this.edit({ command, clipIds: [...this.selected], ...extra });
    }
    catch (e) {
        this.error(e);
    }
    finally {
        this.busy = false;
        this.render();
        this.selection();
    } }
    private cancel(): void { this.finishTrackResize(true); const g = this.gesture; if (!g)
        return; this.gesture = null; if (this.layer.hasPointerCapture(g.pointer))
        this.layer.releasePointerCapture(g.pointer); this.render(); }
    private place(node: HTMLElement, i: Item): void { const v = this.wave.viewport; node.style.left = `${(i.start - v.start) / v.span * v.width}px`; node.style.width = `${Math.max(3, (i.end - i.start) / v.span * v.width)}px`; node.style.top = `${i.top + 4}px`; node.style.height = `${i.height - 8}px`; node.style.transform = ''; node.dataset.preview = JSON.stringify(i.clip); }
    private paintRange(): void { const v = this.wave.viewport; this.rangeBox.hidden = !this.range; if (this.range) {
        this.rangeBox.style.left = `${(this.range.start - v.start) / v.span * v.width}px`;
        this.rangeBox.style.width = `${(this.range.end - this.range.start) / v.span * v.width}px`;
        const rows = this.tracks.filter(t => this.range!.trackIds.includes(t.id));
        this.rangeBox.style.top = `${rows[0]?.top ?? 0}px`;
        this.rangeBox.style.height = `${rows.reduce((sum, t) => sum + t.height, 0)}px`;
    } }
    private paint(node: HTMLElement, c: Clip, rate: number, peaks: WaveformView | null): void {
        const canvas = node.querySelector('canvas')!;
        const fullWidth = Math.max(1, node.clientWidth), offset = Math.max(0, -parseFloat(node.style.left));
        node.classList.toggle('tiny',fullWidth<24);
        canvas.hidden=fullWidth<=3;
        if(canvas.hidden)return; // Subpixel clips need only the existing CSS outline.
        const width = Math.max(1, Math.min(this.wave.viewport.width, fullWidth - offset)), height = Math.max(1, node.clientHeight);
        const dpr = devicePixelRatio || 1;
        canvas.width = Math.round(width * dpr);
        canvas.height = Math.round(height * dpr);
        canvas.style.width = `${width}px`;
        canvas.style.left = `${offset}px`;
        node.querySelector<HTMLElement>('.clip-name')!.style.left = `${offset + 10}px`;
        const ctx = canvas.getContext('2d')!;
        ctx.scale(dpr, dpr);
        this.drawCount++;
        if (c.kind === 'midi') {
            ctx.fillStyle = '#bac2e8';
            const origin = Number(c.startTick), begin = this.wave.musical.seconds(origin), duration = this.wave.musical.seconds(origin + Number(c.lengthTick)) - begin;
            for (const n of c.notes) {
                const x = (this.wave.musical.seconds(origin - Number(c.contentOffsetTick ?? 0) + Number(n.startTick)) - begin) / duration * fullWidth - offset;
                const end = (this.wave.musical.seconds(origin - Number(c.contentOffsetTick ?? 0) + Number(n.startTick) + Number(n.lengthTick)) - begin) / duration * fullWidth - offset;
                if (end >= 0 && x <= width) ctx.fillRect(x, 30 + (127 - n.pitch) / 128 * Math.max(1, height - 36), Math.max(2, end - x), 2);
            }
            return;
        }
        const seconds = (Number(c.sourceEnd) - Number(c.sourceStart)) / rate;
        ctx.strokeStyle = c.mute ? '#687273' : node.style.getPropertyValue('--track-color');
        ctx.beginPath();
        if (peaks)
            for (const [channel, data] of peaks.channels.entries()) {
                const center = 30 + (height - 30) * (channel + .5) / peaks.channels.length;
                const scale = (height - 35) / peaks.channels.length * .43;
                data.forEach(([min, max], i) => { const t = peaks.start + i / data.length * (peaks.end - peaks.start); const x = (renderedTime(c,rate,t) - Number(c.sourceStart) / rate) / seconds * fullWidth - offset; if (x < 0 || x > width)
                    return; ctx.moveTo(x, center - Math.max(-1, Math.min(1, max * c.gain)) * scale); ctx.lineTo(x, center - Math.max(-1, Math.min(1, min * c.gain)) * scale); });
            }
        ctx.stroke();
        const w = (c.extensions?.['minidaw.envelopeWindow.v1'] as Envelope | undefined) ?? c;
        ctx.strokeStyle = '#e6c77d';
        ctx.beginPath();
        const envelope = (x: number) => { const frame = Number(c.sourceStart) + (x + offset) / fullWidth * (Number(c.sourceEnd) - Number(c.sourceStart)); const ramp = (n: number, curve: string) => { const v = Math.max(0, Math.min(1, n)); return curve === 'linear' ? v : .5 - .5 * Math.cos(Math.PI * v); }; return (Number(w.fadeIn.sourceFrames) ? ramp((frame - Number(w.sourceStart)) / Math.max(1, Number(w.fadeIn.sourceFrames) - 1), w.fadeIn.curve) : 1) * (Number(w.fadeOut.sourceFrames) ? ramp((Number(w.sourceEnd) - 1 - frame) / Math.max(1, Number(w.fadeOut.sourceFrames) - 1), w.fadeOut.curve) : 1); };
        for (let x = 0; x <= width; x += 4) {
            const y = 28 + (1 - envelope(x)) * (height - 35);
            if (x === 0)
                ctx.moveTo(x, y);
            else
                ctx.lineTo(x, y);
        }
        ctx.stroke();
        const fi = node.querySelector<HTMLElement>('[data-handle="fade-in"]')!, fo = node.querySelector<HTMLElement>('[data-handle="fade-out"]')!;
        fi.style.left = `${Math.max(8, Math.min(fullWidth - 16, Number(c.fadeIn.sourceFrames) / rate / seconds * fullWidth))}px`;
        fo.style.right = `${Math.max(8, Math.min(fullWidth - 16, Number(c.fadeOut.sourceFrames) / rate / seconds * fullWidth))}px`;
    }
    render(): void {
        if (this.gesture || !this.layer.clientWidth || !this.trackScroll.clientHeight)
            return;
        for(const h of document.querySelectorAll('.track-drop-target'))h.classList.remove('track-drop-target');
        if (this.rootSize !== parseFloat(getComputedStyle(document.documentElement).fontSize)) this.layoutTracks();
        this.automation.draw();
        this.generation++;
        const generation = this.generation;
        this.wanted.clear();
        const v = this.wave.viewport;
        const old = this.nodes;
        this.nodes = new Map();
        const viewport = JSON.stringify(v) + ':' + this.cacheEpoch + ':' + devicePixelRatio;
        const sameViewport = viewport === this.lastViewport;
        this.lastViewport = viewport;
        this.paintRange();
        let lo = 0, hi = this.prefix.length;
        while (lo < hi) {
            const mid = (lo + hi) >> 1;
            if (this.prefix[mid] <= v.start)
                lo = mid + 1;
            else
                hi = mid;
        }
        for (let n = lo; n < this.items.length && this.items[n].start < v.start + v.span; n++) {
            const i = this.items[n];
            if (i.end <= v.start || i.top + i.height < this.trackScroll.scrollTop || i.top > this.trackScroll.scrollTop + this.trackScroll.clientHeight)
                continue;
            const previous = old.get(i.clip.clipId);
            old.delete(i.clip.clipId);
            const node = previous?.node ?? document.createElement('div');
            const part=this.partId(i);node.classList.toggle('part-member',!!part&&!this.expandedParts.has(part));
            node.classList.add('audio-clip'); node.classList.toggle('midi-clip', i.clip.kind === 'midi');
            node.dataset.clipId = i.clip.clipId;
            node.dataset.trackId = i.trackId;
            node.dataset.lane = String(i.lane);
            node.style.setProperty('--track-color', i.color);
            node.setAttribute('role', 'option');
            uiAttr(node, 'title', `${i.clip.name} · ${this.tracks.find(t => t.id === i.trackId)?.name} · ${i.start.toFixed(3)}–${i.end.toFixed(3)} s`);
            node.classList.toggle('muted', i.clip.kind === 'audio' && i.clip.mute);
            const mix=this.view?.document.tracks.find(t=>t.trackId===i.trackId)?.mix;
            node.classList.toggle('track-silent',!!mix?.mute||(!!this.view?.document.tracks.some(t=>t.mix?.solo)&&!mix?.solo));
            const clip = i.clip;
            node.classList.toggle('missing', clip.kind === 'audio' && !this.view?.assets.some(a => a.assetId === clip.assetId && a.status === 'available'));
            if (!previous) {
                const title = document.createElement('span');
                title.className = 'clip-name';
                node.append(title, document.createElement('canvas'));
                for (const handle of i.clip.kind === 'audio' ? ['trim-left', 'trim-right', 'fade-in', 'fade-out'] : ['trim-left', 'trim-right']) {
                    const h = document.createElement('span');
                    h.className = `clip-handle ${handle}`;
                    h.dataset.handle = handle;
                    uiAttr(h, 'title', handle);
                    node.append(h);
                }
                this.layer.append(node);
            }
            uiText(node.querySelector('.clip-name')!, `${i.clip.kind === 'audio' && i.clip.mute ? 'M · ' : i.clip.kind === 'midi' ? '♫ ' : ''}${i.clip.kind==='audio'&&sync(i.clip)?.enabled?'♪ ':''}${i.clip.name}`);
            if (previous && sameViewport && !node.style.transform && node.style.top === `${i.top + 4}px` && node.style.height === `${i.height - 8}px` && node.dataset.preview === JSON.stringify(i.clip) && (previous.peaks || i.clip.kind === 'midi' || node.offsetWidth <= 3)) {
                this.nodes.set(i.clip.clipId, { ...previous, item: i });
                continue;
            }
            this.place(node, i);
            const entry = { node, item: i, peaks: null as WaveformView | null };
            this.nodes.set(i.clip.clipId, entry);
            if (clip.kind === 'midi') { this.paint(node, clip, 1, null); continue; }
            const start = originalTime(clip,i.rate,Number(clip.sourceStart) / i.rate + Math.max(0, v.start - i.start)), end = originalTime(clip,i.rate,Number(clip.sourceEnd) / i.rate - Math.max(0, i.end - v.start - v.span));
            const width = Math.max(1, Math.min(2048, Math.round((Math.min(i.end, v.start + v.span) - Math.max(i.start, v.start)) / v.span * v.width)));
            const key = `${this.assetKeys.get(clip.assetId)}:${start.toFixed(9)}:${end.toFixed(9)}:${width}`;
            this.wanted.add(key);
            entry.peaks = this.cache.get(key) ?? null;
            this.paint(node, i.clip, i.rate, entry.peaks);
            if (width > 3 && !entry.peaks && !node.classList.contains('missing')) {
                let query = this.inflight.get(key);
                if (!query) {
                    query = this.queries.then(() => { if (this.gesture || this.wave.dragging) {
                        this.refreshPeaks();
                        return null;
                    } return this.wanted.has(key) ? invoke<WaveformView & {
                        complete: boolean;
                    }>('asset_waveform', { assetId: clip.assetId, start, end, width }) : null; });
                    this.queries = query.then(() => { }, () => { });
                    this.inflight.set(key, query);
                }
                void query.then(peaks => { if (!peaks)
                    return; if (!peaks.complete)
                    this.refreshPeaks(); this.cache.set(key, peaks); if (this.cache.size > 256)
                    this.cache.delete(this.cache.keys().next().value!); if (generation === this.generation && !this.gesture && !this.wave.dragging) {
                    entry.peaks = peaks;
                    this.paint(node, i.clip, i.rate, peaks);
                } }).catch(e => { if (generation === this.generation)
                    this.error(e); }).finally(() => this.inflight.delete(key));
            }
        }
        for (const entry of old.values())
            entry.node.remove();
        this.renderParts();
        this.selection();
    }
    refreshPeaks(): void { if (this.pending)
        return; this.pending = true; setTimeout(() => { this.pending = false; if (this.gesture || this.wave.dragging)
        this.refreshPeaks();
    else {
        this.cache.clear();
        this.cacheEpoch++;
        this.render();
    } }, 200); }
}
