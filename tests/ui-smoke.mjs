// End-to-end test of the compiled Tauri executable and its real Rust/CPAL IPC.
// No engine/IPC mocks and no browser installation. The process-local WebView2
// debugging flag is used only by this test; the shipped app has no debug port.
import { chromium } from "playwright-core";
import { spawn } from "node:child_process";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { resolve } from "node:path";
import assert from "node:assert/strict";
import { verifyUiRefresh } from "./ui-refresh.mjs";
import { verifyControls } from "./ui-controls.mjs";
import { once } from "node:events";

const executable = resolve(process.argv[2] ?? "src-tauri/target/debug/minidaw.exe");
const directory = resolve("docs/validation");
await mkdir(directory, { recursive: true });
const port = 19223;
const launch = () => spawn(executable, [], {
  env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` },
  stdio: "ignore", windowsHide: true,
});
let child = launch();
let browser, page, originalFont, originalAudio;
let capturedPreference = false;
const report = { executable, executableSha256: createHash("sha256").update(await readFile(executable)).digest("hex"),
  started: new Date().toISOString(), checks: [], output: [], limitations: [
  "Drag-drop tests inject the Tauri native event payload; they do not synthesize an OS file-manager gesture.",
  "Checks prove PCM submission to the real output callback, not acoustic/loopback recording.",
] };
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
async function connectApp() {
  for (let attempt = 0; attempt < 100; attempt++) {
    try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`); break; }
    catch { if (child.exitCode !== null) throw new Error(`MiniDAW exited: ${child.exitCode}`); await pause(100); }
  }
  assert.ok(browser, "WebView2 debugging connection did not open");
  const context = browser.contexts()[0];
  for (let attempt = 0; attempt < 100; attempt++) {
    page = context.pages().find(page => page.url().includes("tauri.localhost"));
    if (page) break;
    await pause(100);
  }
  assert.ok(page, "Tauri webview not found");
}
async function closeApp() {
  if (browser) { await browser.close(); browser = null; }
  if (child.exitCode === null) {
    const exited = once(child, "exit");
    child.kill();
    await exited;
  }
  page = null;
}
try {
  await connectApp();
  const errors = [];
  page.on("pageerror", error => errors.push(error.message));
  const invoke = (command, args = {}) => page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
  const snapshot = () => invoke("engine_snapshot");
  originalAudio = (await snapshot()).preferences;
  // Buffer-request cases below exercise WASAPI; ASIO uses the driver's buffer.
  await invoke("apply_audio_settings", { settings: { ...originalAudio, driverType: "wasapi", outputDevice: null } });
  const drop = path => invoke("plugin:event|emit_to", {
    target: { kind: "AnyLabel", label: "main" }, event: "tauri://drag-drop", payload: { paths: [resolve(path)], position: { x: 200, y: 400 } },
  });
  async function until(predicate, message) {
    for (let attempt = 0; attempt < 100; attempt++) { if (await predicate()) return; await pause(30); }
    throw new Error(message);
  }
  await until(async () => (await page.locator("#status-text").textContent()).includes("엔진 연결됨"), "Engine not ready");
  originalFont = await page.evaluate(() => localStorage.getItem("minidaw.ui.fontScale"));
  capturedPreference = true;
  if (originalFont === null) assert.equal(await page.locator("#font-scale").inputValue(), "110", "Default font should be 110%");
  await page.locator("#font-scale").selectOption("110");
  const initial = await snapshot();
  assert.equal(initial.outputError, null);
  report.checks.push("Real Tauri IPC and output device connected");

  for (const extension of ["wav", "mp3", "flac"]) {
    const p=await invoke("project_snapshot"); await invoke("new_project",{revision:p.revision,discard:true});
    await drop(`tests/fixtures/stereo-44100.${extension}`);
    await until(async () => (await page.locator("#file-name").textContent()) === `stereo-44100.${extension}`, `Cannot load ${extension}`);
    await until(async () => !(await page.locator("#play-pause").isDisabled()), "Play not enabled");
    const decoded = await snapshot();
    assert.equal(decoded.file.sampleRate, 44100);
    assert.equal(decoded.file.channels, 2);
    assert.ok(Math.abs(decoded.duration - 6) < 0.05);
    await page.locator("#play-pause").click();
    await until(async () => (await snapshot()).transport.state === "playing", "Play not applied");
    await pause(150);
    await page.locator("#play-pause").click();
    await until(async () => (await snapshot()).transport.state === "paused", "Pause not applied");
    const paused = await snapshot();
    await pause(150);
    assert.equal((await snapshot()).position, paused.position);
    const bounds = await page.locator("#waveform").boundingBox();
    await page.mouse.click(bounds.x + bounds.width * 0.3, bounds.y + 12);
    await until(async () => Math.abs((await snapshot()).position - 3) < 0.05, "Paused waveform seek failed");
    assert.equal((await snapshot()).metrics.seekMs, null);
    await page.locator("#play-pause").click();
    await until(async () => (await snapshot()).transport.state === "playing", "Resume failed");
    await page.mouse.click(bounds.x + bounds.width * 0.15, bounds.y + 12);
    await until(async () => {
      const s = await snapshot(); return s.position > 1.4 && s.position < 2.0 && s.metrics.seekMs !== null;
    }, "Playing waveform seek failed");

    const before = await snapshot();
    const draws = await page.locator("#metric-draws").textContent();
    // Deliberately stall the real webview JS thread. Rust must keep rendering.
    await page.evaluate(() => { const end = performance.now() + 500; while (performance.now() < end) {} });
    const after = await snapshot();
    assert.ok(after.position - before.position > 0.4, "Audio was stalled by JS");
    assert.ok(after.metrics.callbacks > before.metrics.callbacks);
    assert.ok(after.metrics.nonSilentFrames > before.metrics.nonSilentFrames);
    assert.equal(after.streamErrors, 0);
    await pause(100);
    assert.equal(await page.locator("#metric-draws").textContent(), draws, "Playhead caused waveform redraw");
    await page.locator("#stop").click();
    await until(async () => (await snapshot()).position === 0, "Stop did not reset position");
    report.output.push(await snapshot());
    report.checks.push(`${extension}: real decode, native-event routing, play/pause/resume, paused/playing canvas seek, stop, 500 ms UI stall isolation, static waveform`);
  }

  await page.locator("#zoom-in").click();
  await until(async () => (await page.locator("#zoom-value").textContent()) === "2.0×", "Zoom did not update on next animation frame");
  await page.locator("#scroll").evaluate(element => { element.value = "2.5"; element.dispatchEvent(new Event("input", { bubbles: true })); });
  await until(async () => (await page.locator("#view-range").textContent()).includes("00:02.5"), "Horizontal scroll failed");
  await page.locator("#zoom-fit").click();
  await until(async () => (await page.locator("#zoom-value").textContent()) === "1.0×", "Fit failed");
  await drop("tests/fixtures/corrupt.wav");
  await until(async () => await page.locator("#error").isVisible(), "Korean decoding error not shown");
  assert.match(await page.locator("#error-text").textContent(), /디코딩/);
  assert.equal((await snapshot()).file.name, "stereo-44100.flac");
  await page.locator("#dismiss-error").click();
  report.checks.push("Zoom, horizontal scroll, fit, Korean decode error, previous file preserved");

  await page.locator("#stop").focus();
  await page.keyboard.press("Space");
  await until(async () => (await snapshot()).transport.state === "playing", "Space did not start playback from button focus");
  await until(async () => (await page.locator("#transport-state").textContent()) === "재생 중", "Playing snapshot did not reach keyboard UI");
  await page.keyboard.press("Space");
  await until(async () => (await snapshot()).transport.state === "paused", "Space did not pause");
  await page.keyboard.press("Home");
  await until(async () => (await snapshot()).position === 0, "Home did not stop");
  const beforeInvalidBuffer = (await snapshot()).transport.clipId;
  await assert.rejects(invoke("reconnect_output", { buffer: 3 }));
  assert.equal((await snapshot()).transport.clipId, beforeInvalidBuffer);
  assert.equal((await snapshot()).outputError, null);
  report.checks.push("Keyboard transport and invalid buffer preserves live output");

  await verifyUiRefresh({ page, snapshot, until, report, directory });

  for (const buffer of ["128", "256", "0"]) {
    await page.locator("#open-audio-settings").click();
    await until(async () => !(await page.locator("#output-device").isDisabled()), "Device list did not load");
    await page.locator("#buffer-size").selectOption(buffer);
    await page.locator("#reconnect").click();
    await until(async () => {
      const s = await snapshot(); return s.output?.requestedBuffer === (Number(buffer) || null) && s.file && s.metrics.callbacks > 3;
    }, "Output reconfiguration failed");
    await page.locator("#close-audio-settings").click();
    await page.locator("#play-pause").click();
    await until(async () => (await snapshot()).metrics.nonSilentFrames > 100, "Reconfigured output did not render PCM");
    await page.locator("#stop").click();
    report.output.push(await snapshot());
  }
  report.checks.push("128 / 256 / default buffer requests, measured callback sizes, output reconnect");
  await verifyControls({ page, snapshot, until, report, directory });
  await page.locator(".performance").evaluate(element => { element.scrollTop = 0; });
  await page.locator(".editor").evaluate(element => { element.scrollTop = 0; });
  await page.screenshot({ path: resolve(directory, "desktop.png") });
  await page.setViewportSize({ width: 800, height: 600 });
  await pause(200);
  await page.locator(".performance").evaluate(element => { element.scrollTop = 0; });
  assert.ok(await page.locator("#play-pause").isVisible());
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  await page.screenshot({ path: resolve(directory, "compact.png") });
  assert.deepEqual(errors, []);
  report.checks.push("Desktop/compact layout, no webview JavaScript errors");
  await closeApp();
  child = launch();
  await connectApp();
  page.on("pageerror", error => errors.push(error.message));
  await until(async () => (await page.locator("#status-text").textContent()).includes("엔진 연결됨"), "Restarted engine not ready");
  assert.equal(await page.locator("#font-scale").inputValue(), "125");
  assert.equal(await page.evaluate(() => getComputedStyle(document.documentElement).fontSize), "18.75px");
  assert.equal((await snapshot()).file, null, "UI preference should not need an audio project");
  report.checks.push("Actual release process restarted: saved 125% font restored with no audio/project loaded");
  assert.deepEqual(errors, []);
  report.passed = true;
} catch (error) {
  report.passed = false;
  report.error = error.stack ?? String(error);
  process.exitCode = 1;
} finally {
  await writeFile(resolve(directory, "ui-smoke.json"), JSON.stringify(report, null, 2));
  if (page && originalAudio) await page.evaluate(settings => window.__TAURI_INTERNALS__.invoke("apply_audio_settings", { settings }), originalAudio).catch(() => {});
  if (page && capturedPreference) await page.evaluate(value => {
    if (value === null) localStorage.removeItem("minidaw.ui.fontScale");
    else localStorage.setItem("minidaw.ui.fontScale", value);
  }, originalFont).catch(() => {});
  await closeApp();
}
console.log(JSON.stringify({ passed: report.passed, checks: report.checks, error: report.error }, null, 2));

