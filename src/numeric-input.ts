// Commit a whole numeric edit through the caller's existing project/history path.
// Native step controls keep working; typing a partial sign never edits the DSP.
export function numericInput(input: HTMLInputElement, commit: (value: number) => void): void {
  let previous = input.value;
  let cancelling = false;
  input.addEventListener('focus', () => { previous = input.value; });
  input.addEventListener('change', () => {
    if (cancelling) return;
    const value = input.valueAsNumber;
    if (!Number.isFinite(value)) { input.value = previous; return; }
    const min = input.min === '' ? -Infinity : Number(input.min);
    const max = input.max === '' ? Infinity : Number(input.max);
    const step = Number(input.step);
    const base = Number.isFinite(min) ? min : 0;
    let next = Math.max(min, Math.min(max, value));
    if (step > 0) next = base + Math.round((next - base) / step) * step;
    next = Math.max(min, Math.min(max, Number(next.toFixed(12))));
    input.value = String(next);
    previous = input.value;
    commit(next);
  });
  input.addEventListener('keydown', event => {
    if (event.key !== 'Enter' && event.key !== 'Escape') return;
    event.preventDefault(); event.stopPropagation();
    if (event.key === 'Escape') { cancelling = true; input.value = previous; }
    // Blur dispatches the native change once, including keyboard/spinner edits.
    input.blur();
    cancelling = false;
  });
}
