import { element } from "./dom";

const FONT_SCALE_KEY = "minidaw.ui.fontScale";
const DEFAULT_FONT_SCALE = 110;
const scales = [90, 95, 100, 105, 110, 115, 120, 125];

/** App/WebView profile preference, independent of loaded audio/project state. */
export function initializeFontScale(onChange: () => void, onError: (error: unknown) => void): void {
  const select = element<HTMLSelectElement>("font-scale");
  let scale = DEFAULT_FONT_SCALE;
  try {
    const saved = Number(localStorage.getItem(FONT_SCALE_KEY));
    if (scales.includes(saved)) scale = saved;
  } catch { /* Restricted storage still permits the default and in-session changes. */ }
  const apply = (value: number) => {
    document.documentElement.style.setProperty("--ui-font-scale", String(value / 100));
    select.value = String(value);
    onChange();
  };
  apply(scale);
  select.addEventListener("change", () => {
    const value = Number(select.value);
    if (!scales.includes(value)) return;
    apply(value);
    try { localStorage.setItem(FONT_SCALE_KEY, String(value)); }
    catch { onError({ message: "글꼴 크기를 저장하지 못했습니다. 이번 실행에는 적용됩니다." }); }
  });
}
