export const floatingPanels = ['spectrum', 'performance', 'piano', 'mixer'] as const;
export type FloatingPanel = typeof floatingPanels[number];
export const panelWindow = new URLSearchParams(location.search).get('panel');
export const isPanel = (id: string): id is FloatingPanel => (floatingPanels as readonly string[]).includes(id);
export const satellite = panelWindow && isPanel(panelWindow) ? panelWindow : null;
export const panelTitles: Record<FloatingPanel, string> = { spectrum:'Spectrum', performance:'Performance', piano:'Piano Roll', mixer:'Mixer' };
export interface PanelStateAdapter { capture(): unknown; restore(value:any): void; suspend?(): Promise<void> }
export interface FloatingLayout { detached(id:string):boolean; visible(id:string):boolean; show(id:string,open:boolean):void; detach(id:string):void }
