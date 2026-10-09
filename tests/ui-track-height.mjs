// Focused Track height regression against the compiled release WebView and real commands.
import { chromium } from 'playwright-core';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { readFile, writeFile, mkdir, unlink } from 'node:fs/promises';
import { createHash, randomUUID } from 'node:crypto';
import { resolve, join } from 'node:path';
import assert from 'node:assert/strict';

const exe = resolve('src-tauri/target/release/minidaw.exe');
const folder = resolve('tests/local/ui-track-height', randomUUID());
await mkdir(folder, { recursive: true });
const recent = join(process.env.APPDATA, 'local.minidaw.desktop/recent-projects.json');
let savedRecent; try { savedRecent = await readFile(recent); } catch {}
const report = { executableSha256: createHash('sha256').update(await readFile(exe)).digest('hex'), checks: [] };
let child, browser, page, prefs;
const errors = [], sleep = ms => new Promise(r => setTimeout(r, ms));
async function until(fn, label, timeout = 20000) {
  const started = performance.now();
  while (performance.now() - started < timeout) { if (await fn()) return; await sleep(40); }
  throw Error(label);
}
const invoke = (name, args = {}) => page.evaluate(({ name, args }) => window.__TAURI_INTERNALS__.invoke(name, args), { name, args });
const project = () => invoke('project_snapshot'), snapshot = () => invoke('engine_snapshot');
const clips = p => p.document.tracks.flatMap(t => t.clips);
const header = id => page.locator(`.track-header[data-track-id="${id}"]`);
const height = id => header(id).evaluate(el => el.offsetHeight);
const fontSize = () => page.evaluate(() => parseFloat(getComputedStyle(document.documentElement).fontSize));
async function idle() { await until(async () => !(await page.locator('#project-save').isDisabled()), 'Project idle'); await sleep(120); }
async function toggle(id, open) {
  const b = page.locator('#show-' + id);
  if ((await b.getAttribute('aria-pressed') === 'true') !== open) await b.click();
  await sleep(100);
}
async function tool(name) { await page.locator(`[data-command="tool.${name}"]`).click(); await sleep(50); }
async function key(k) { await page.keyboard.press(k); await idle(); }
async function align() {
  const geometry = await page.evaluate(() => {
    const layer = document.getElementById('clip-layer').getBoundingClientRect();
    return [...document.querySelectorAll('.track-header')].map(el => {
      const b = el.getBoundingClientRect();
      const handle = document.querySelector(`#clip-layer > [data-resize-track="${el.dataset.trackId}"]`).getBoundingClientRect();
      const events = [...document.querySelectorAll(`.audio-clip[data-track-id="${el.dataset.trackId}"]`)].map(n => {
        const r = n.getBoundingClientRect(), c = n.querySelector('canvas');
        return { top: r.top, bottom: r.bottom, h: r.height, canvas: c.height, inner: n.clientHeight, dpr: devicePixelRatio };
      });
      return { id: el.dataset.trackId, top: b.top, bottom: b.bottom, height: b.height, boundary: handle.bottom, layerTop: layer.top, events };
    });
  });
  for (const [i, row] of geometry.entries()) {
    assert.ok(Math.abs(row.bottom - row.boundary) < 1.1, 'Header/Event bottom alignment');
    if (i) assert.ok(Math.abs(geometry[i - 1].bottom - row.top) < 1.1, 'Contiguous Tracks');
    for (const e of row.events) {
      assert.ok(e.top >= row.top + 3 && e.bottom <= row.bottom - 3, 'Event contained in Track');
      assert.ok(Math.abs(e.canvas - Math.round(e.inner * e.dpr)) <= 1, 'Waveform resolution follows Event height');
    }
  }
  return geometry;
}
async function resize(id, delta, area = 'header', options = {}) {
  const target = area === 'header' ? header(id).locator('.track-resize') : page.locator(`#clip-layer > [data-resize-track="${id}"]`);
  await target.scrollIntoViewIfNeeded(); await sleep(100);
  const b = await target.boundingBox(), h = await height(id), p = await project(), s = await snapshot();
  const x = b.x + Math.min(b.width / 2, 70), y = b.y + b.height / 2;
  await page.mouse.move(x, y); await page.mouse.down();
  assert.equal(await page.locator('#track-scroll').evaluate(el => el.hasPointerCapture(1)), true, 'Stable scroll container captures pointer');
  await page.mouse.move(options.outside ? 5 : x, y + delta, { steps: options.fast ? 1 : 25 });
  await sleep(100);
  if (options.cancel) {
    await page.locator('#track-scroll').dispatchEvent('pointercancel', { pointerId: 1 });
  }
  await page.mouse.up(); await sleep(120);
  assert.equal(await page.locator('#track-scroll').evaluate(el => el.hasPointerCapture(1)), false);
  assert.equal(await page.locator('.track-height-resizing').count(), 0);
  assert.deepEqual((await project()).document, p.document, 'Height is view state only');
  assert.equal((await project()).revision, p.revision, 'No editing command');
  assert.deepEqual((await project()).history, p.history, 'No Undo entry');
  const after = await snapshot();
  assert.equal(after.transport.appliedCommand, s.transport.appliedCommand, 'No transport command');
  assert.deepEqual(after.waveform, s.waveform, 'No peak cache rebuild');
  await align();
  if (options.cancel) assert.equal(await height(id), h, 'Cancel restores previous height');
  return { before: h, after: await height(id) };
}
async function revealClip(id) {
  const p = await project(), track = p.document.tracks.find(t => t.clips.some(c => c.clipId === id));
  await header(track.trackId).evaluate(el => document.getElementById('track-scroll').scrollTop = el.offsetTop);
  await sleep(120);
  const node = page.locator(`.audio-clip[data-clip-id="${id}"]`);
  await node.scrollIntoViewIfNeeded(); return node;
}
async function editDrag(id, handle, dx) {
  const node = await revealClip(id), b = await (handle ? node.locator(`[data-handle="${handle}"]`) : node).boundingBox();
  const x = b.x + (handle ? b.width / 2 : Math.min(45, b.width * .3)), y = b.y + b.height * .6;
  const p = await project();
  await page.mouse.move(x, y); await page.mouse.down(); await page.mouse.move(x + dx, y, { steps: 20 });
  assert.equal((await project()).revision, p.revision);
  await page.mouse.up(); await until(async () => (await project()).revision === p.revision + 1, 'One Clip edit'); await idle();
}
async function openDocument(doc, name) {
  const path = join(folder, name + '.minidaw'); await writeFile(path, JSON.stringify(doc));
  await invoke('open_project', { path, revision: (await project()).revision, discard: true }); await idle();
}
async function nativeWindowSize(width, height) {
  // Resize only the test process's native window; no extra Tauri production permissions.
  const command = `Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public class TrackHeightWindow { [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int width, int height, uint flags); }'; $h=(Get-Process -Id ${child.pid}).MainWindowHandle; if($h -eq 0 -or -not [TrackHeightWindow]::SetWindowPos($h,[IntPtr]::Zero,0,0,${width},${height},22)){exit 1}`;
  const process = spawn('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', command], { stdio: 'pipe', windowsHide: true });
  const [code] = await once(process, 'exit'); assert.equal(code, 0, 'Native window resize');
  await sleep(200);
  return page.evaluate(() => ({ width: innerWidth, height: innerHeight }));
}
try {
  child = spawn(exe, [], { env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: '--remote-debugging-port=19237' }, stdio: 'ignore', windowsHide: true });
  await until(async () => { try { browser = await chromium.connectOverCDP('http://127.0.0.1:19237'); return true; } catch { return false; } }, 'Release CDP');
  await until(async () => { page = browser.contexts()[0].pages().find(p => p.url().includes('tauri.localhost')); return !!page; }, 'Release page');
  page.on('pageerror', e => errors.push(e.message));
  await nativeWindowSize(1440, 960); await idle();
  prefs = await page.evaluate(() => ({ panels: localStorage.getItem('minidaw.ui.panels.v1'), font: localStorage.getItem('minidaw.ui.fontScale') }));
  await toggle('arrangement', true); await toggle('media', false); await toggle('performance', false);
  await page.locator('#font-scale').selectOption('110');
  await invoke('load_audio', { path: resolve('tests/fixtures/stereo-44100.wav') });
  await until(async () => await page.locator('.audio-clip').count() === 1, 'Imported clip'); await idle();
  await until(async () => (await snapshot()).waveform?.complete, 'Ready peaks'); await sleep(350);
  let p = await project(); const track = p.document.tracks[0].trackId, id = clips(p)[0].clipId;
  const initial = await height(track), root = await fontSize();
  const grown = await resize(track, 100); assert.equal(grown.after, initial + 100);
  const reduced = await resize(track, -50, 'events'); assert.equal(reduced.after, grown.after - 50);
  await resize(track, 40, 'header', { cancel: true });
  await resize(track, 60, 'events', { outside: true, fast: true });
  await resize(track, -1500); assert.ok(Math.abs(await height(track) - root * 4.5) <= .5);
  await resize(track, 1600, 'events'); assert.ok(Math.abs(await height(track) - root * 24) <= .5);
  await resize(track, 180 - await height(track));
  report.checks.push('Single Track: header/Event boundaries, growth/shrink, min/max clamp, rapid/outside drag with pointer capture, cancel restore; no project/history/transport/peak-build changes.');

  // All Clip operations below use the real command/data layer, after changing the row height.
  await tool('objectSelection');
  let node = await revealClip(id), box = await node.boundingBox();
  await page.mouse.click(box.x + 45, box.y + box.height * .6);
  assert.equal(await node.getAttribute('aria-selected'), 'true');
  const beforeMove = structuredClone((await project()).document);
  await editDrag(id, null, 25); const moved = structuredClone((await project()).document);
  assert.notDeepEqual(moved, beforeMove); await key('Control+z'); assert.deepEqual((await project()).document, beforeMove);
  await key('Control+Shift+z'); assert.deepEqual((await project()).document, moved);
  assert.equal(await height(track), 180, 'Height survives edits and Undo/Redo');
  await editDrag(id, 'trim-left', 12); await editDrag(id, 'trim-right', -12);
  await tool('split'); await resize(track, 20, 'events');
  const count = clips(await project()).length; node = await revealClip(id); box = await node.boundingBox();
  await page.mouse.click(box.x + box.width / 2, box.y + box.height * .6); await idle();
  assert.equal(clips(await project()).length, count + 1); await key('Control+z'); assert.equal(clips(await project()).length, count);
  await tool('rangeSelection'); await resize(track, -20, 'events');
  assert.equal(await page.locator('#edit-range').isVisible(), false, 'Resize does not start Range gesture');
  await tool('objectSelection');
  report.checks.push('Selection, Move, both Trim edges, Split and Undo/Redo pass after resizing. Split/Range tools do not intercept Track boundary drag.');

  // Real document fixture: several Tracks; existing audio restriction is left unchanged.
  p = await project(); const multi = structuredClone(p.document), template = multi.tracks[0];
  multi.projectId = randomUUID(); multi.name = 'Track height validation';
  multi.tracks = Array.from({ length: 7 }, (_, i) => ({ ...structuredClone(template), trackId: randomUUID(), name: `Audio ${i + 1}`, clips: i === 2 ? template.clips : [] }));
  await openDocument(multi, 'tracks');
  const ids = multi.tracks.map(t => t.trackId), originals = await Promise.all(ids.map(height));
  await resize(ids[0], 85); await resize(ids[2], 60, 'events'); await resize(ids[4], -25);
  assert.equal(await height(ids[0]), originals[0] + 85); assert.equal(await height(ids[2]), originals[2] + 60);
  assert.equal(await height(ids[4]), originals[4] - 25); assert.equal(await height(ids[1]), originals[1]);
  await page.locator('#track-scroll').evaluate(el => el.scrollTop = 0); await sleep(120);
  const scrollBox = await page.locator('#track-scroll').boundingBox();
  await page.mouse.move(scrollBox.x + scrollBox.width * .6, scrollBox.y + 100); await page.mouse.wheel(0, 400);
  await until(async () => await page.locator('#track-scroll').evaluate(el => el.scrollTop > 0), 'Vertical wheel'); await align();
  await page.locator('#track-scroll').evaluate(el => el.scrollTop = el.scrollHeight); await sleep(100);
  await resize(ids[6], -20, 'events'); assert.equal(await height(ids[6]), originals[6] - 20, 'Bottom scroll shrink does not drift');
  await editDrag(id, null, 15); await key('Control+z');
  const savedHeights = await Promise.all(ids.map(height));
  // Run native resizing before Playwright installs a fixed emulated viewport.
  const small = await nativeWindowSize(1000, 760); await align();
  const large = await nativeWindowSize(1280, 860); await align();
  report.nativeWindowResize = { small, large };
  assert.ok(large.width > small.width + 100 && large.height > small.height + 40, 'Actual native resize reaches WebView layout');
  assert.deepEqual(await Promise.all(ids.map(height)), savedHeights);
  report.checks.push('Actual Windows window resize updates the release WebView; independent Track heights and Track/Event alignment are retained.');
  report.layouts = [];
  for (const size of [{ width: 1440, height: 960 }, { width: 960, height: 720 }, { width: 760, height: 520 }]) {
    await page.setViewportSize(size);
    for (const font of ['90', '110', '125']) {
      await page.locator('#font-scale').selectOption(font); await sleep(180); await align();
      const scale = await fontSize();
      assert.ok(Math.abs(await height(ids[0]) - savedHeights[0] / root * scale) < 1.1, 'Track preference scales with font');
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth + 1), false);
      report.layouts.push({ size, font, heights: await Promise.all(ids.map(height)) });
    }
  }
  await page.setViewportSize({ width: 1440, height: 960 }); await page.locator('#font-scale').selectOption('110'); await sleep(150);
  await toggle('performance', true); await toggle('performance', false); await align();
  report.checks.push('Seven independent Track heights, synchronized vertical scrolling, resize at scroll bottom, third-Track editing; 3 window viewport sizes × 3 Font Scales (90/110/125%), panel show/hide preserve alignment.');

  // Display multiple populated Tracks and overlapping Events without changing playback semantics.
  const visible = structuredClone((await project()).document);
  visible.tracks[0].clips = template.clips.map(c => ({ ...structuredClone(c), clipId: randomUUID() }));
  visible.tracks[0].clips.push(...template.clips.map(c => ({ ...structuredClone(c), clipId: randomUUID() })));
  await openDocument(visible, 'populated-tracks'); await sleep(300);
  const beforeOverlap = await height(ids[0]); await resize(ids[0], 80, 'events'); assert.equal(await height(ids[0]), beforeOverlap + 80);
  await page.locator('#track-scroll').evaluate(el => el.scrollTop = 0); await sleep(100);
  const geometry = await align(); const row = geometry.find(r => r.id === ids[0]);
  assert.equal(row.events.length, 2);
  assert.ok(row.events[0].bottom <= row.events[1].top || row.events[1].bottom <= row.events[0].top, 'Overlap display lanes still separate');
  await page.screenshot({ path: 'docs/validation/ui-track-height.png' });
  report.checks.push('Multiple populated Tracks and overlap lanes: Track resize scales all Event/waveform areas without covering adjacent Events.');
  assert.deepEqual(errors, []); report.passed = true;
} catch (e) {
  report.error = String(e.stack ?? e); process.exitCode = 1;
  if (page) { await page.screenshot({ path: 'docs/validation/ui-track-height-failure.png' }).catch(() => {}); report.project = await project().catch(() => null); }
} finally {
  if (page && prefs) await page.evaluate(p => {
    for (const [k, v] of [['minidaw.ui.panels.v1', p.panels], ['minidaw.ui.fontScale', p.font]]) {
      if (v === null) localStorage.removeItem(k); else localStorage.setItem(k, v);
    }
  }, prefs).catch(() => {});
  if (browser) await browser.close();
  if (child?.exitCode === null) { const exit = once(child, 'exit'); child.kill(); await exit; }
  if (savedRecent) await writeFile(recent, savedRecent); else await unlink(recent).catch(() => {});
  await writeFile('docs/validation/ui-track-height.json', JSON.stringify(report, null, 2));
  console.log(JSON.stringify({ passed: report.passed, error: report.error, checks: report.checks }));
}
