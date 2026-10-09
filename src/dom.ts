import { uiText } from './i18n';
export function element<T extends HTMLElement>(id: string): T {
  const result = document.getElementById(id);
  if (!result) throw new Error(`Missing UI element: ${id}`);
  return result as T;
}

export function time(seconds: number, milliseconds = true): string {
  const total = Math.max(0, Math.round(seconds * 1000));
  const minutes = Math.floor(total / 60000);
  const rest = Math.floor(total / 1000) % 60;
  return `${String(minutes).padStart(2, "0")}:${String(rest).padStart(2, "0")}${milliseconds ? `.${String(total % 1000).padStart(3, "0")}` : ""}`;
}

export function text(id: string, value: string): void {
  const target = element(id);
  if (target.textContent !== value) uiText(target, value, id !== "file-name");
}
