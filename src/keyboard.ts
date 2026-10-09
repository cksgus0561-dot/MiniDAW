// A focused slider/button/select is not a text editor. Native typing and IME,
// including input-local Undo, keep priority while editing actual text/numbers.
export function editingText(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable || target.closest('textarea')) return true;
  const input = target.closest('input');
  return !!input && !['range','checkbox','radio','button','submit','reset','color','file','hidden'].includes(input.type);
}
export function keyboardBlocked(e: KeyboardEvent): boolean {
  return e.defaultPrevented || e.isComposing || e.keyCode === 229 || editingText(e.target)
    || !!(e.target instanceof Element && e.target.closest('dialog'));
}
// Capture only registered global commands, before native button activation or
// a local control consumes them. Holding a key never repeats transport actions.
export function globalShortcut(resolve: (e: KeyboardEvent) => (() => void) | undefined): void {
  const held = new Set<string>();
  document.addEventListener('keydown', e => {
    if (keyboardBlocked(e)) return;
    const action = resolve(e); if (!action) return;
    e.preventDefault(); e.stopImmediatePropagation(); held.add(e.code);
    if (!e.repeat) action();
  }, true);
  document.addEventListener('keyup', e => {
    if (held.delete(e.code)) { e.preventDefault(); e.stopImmediatePropagation(); }
  }, true);
  window.addEventListener('blur', () => held.clear());
}
