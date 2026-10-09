// Focused regression: horizontal editing must not repack the vertical Event order.
import { chromium } from 'playwright-core';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { readFile, writeFile, mkdir, unlink } from 'node:fs/promises';
import { createHash, randomUUID } from 'node:crypto';
import { resolve, join } from 'node:path';
import assert from 'node:assert/strict';

const baseline = process.argv.includes('--before');
const exe = resolve('src-tauri/target/release/minidaw.exe');
const folder = resolve('tests/local/ui-clip-order', randomUUID());
await mkdir(folder, { recursive: true });
const recent = join(process.env.APPDATA, 'local.minidaw.desktop/recent-projects.json');
let savedRecent; try { savedRecent = await readFile(recent); } catch {}
const report = { executableSha256: createHash('sha256').update(await readFile(exe)).digest('hex'), moves: [], checks: [] };
let child, browser, page, prefs;
const errors = [], sleep = ms => new Promise(r => setTimeout(r, ms));
async function until(fn, label) {
  const start = performance.now();
  while (performance.now() - start < 20000) { if (await fn()) return; await sleep(40); }
  throw Error(label);
}
const invoke = (name, args = {}) => page.evaluate(({ name, args }) => window.__TAURI_INTERNALS__.invoke(name, args), { name, args });
const project = () => invoke('project_snapshot');
const clips = p => p.document.tracks[0].clips;
const seconds = c => Number(c.position.numerator) / c.position.denominator;
const position = n => ({ unit: 'seconds', numerator: String(Math.round(n * 1e9)), denominator: 1e9 });
async function idle() { await until(async () => !(await page.locator('#project-save').isDisabled()), 'Project idle'); await sleep(120); }
async function fit() { await page.locator('#zoom-fit').click(); await sleep(100); }
async function key(k) { await page.keyboard.press(k); await idle(); await fit(); }
async function rows() {
  return page.locator('.audio-clip').evaluateAll(nodes => nodes.map(n => ({ id: n.dataset.clipId, top: n.offsetTop, height: n.offsetHeight })).sort((a, b) => a.top - b.top));
}
async function editDrag(id, deltaSeconds, handle) {
  await fit(); const p = await project(), node = page.locator(`.audio-clip[data-clip-id="${id}"]`);
  const target = handle ? node.locator(`[data-handle="${handle}"]`) : node;
  const b = await target.boundingBox();
  const width = await page.locator('#waveform').evaluate(el => el.clientWidth);
  const secondsPerPixel = Number(await page.locator('#scroll').getAttribute('step'));
  assert.ok(width > 100 && secondsPerPixel > 0);
  const x = b.x + (handle ? b.width / 2 : Math.min(40, b.width * .3)), y = b.y + b.height * .65;
  await page.mouse.move(x, y); await page.mouse.down();
  await page.mouse.move(x + deltaSeconds / secondsPerPixel, y, { steps: 30 });
  assert.equal((await project()).revision, p.revision, 'Preview does not commit');
  await page.mouse.up();
  await until(async () => (await project()).revision === p.revision + 1, 'Exactly one edit commit'); await idle(); await fit();
  assert.equal((await project()).history.undo, p.history.undo + 1);
  assert.equal(await node.getAttribute('aria-selected'), 'true', 'Move/Trim retain selection');
}
try {
  child = spawn(exe, [], { env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: '--remote-debugging-port=19238' }, stdio: 'ignore', windowsHide: true });
  await until(async () => { try { browser = await chromium.connectOverCDP('http://127.0.0.1:19238'); return true; } catch { return false; } }, 'Release CDP');
  await until(async () => { page = browser.contexts()[0].pages().find(p => p.url().includes('tauri.localhost')); return !!page; }, 'Release page');
  page.on('pageerror', e => errors.push(e.message)); await page.setViewportSize({ width: 1440, height: 960 }); await idle();
  prefs = await page.evaluate(() => ({ panels: localStorage.getItem('minidaw.ui.panels.v1'), font: localStorage.getItem('minidaw.ui.fontScale') }));
  for (const [id, open] of [['arrangement', true], ['media', false], ['performance', false]]) {
    const b = page.locator('#show-' + id); if ((await b.getAttribute('aria-pressed') === 'true') !== open) await b.click();
  }
  await page.locator('#font-scale').selectOption('110');
  await invoke('load_audio', { path: resolve('tests/fixtures/stereo-44100.wav') }); await idle();
  let p = await project(); const doc = structuredClone(p.document), a = doc.tracks[0].clips[0], b = structuredClone(a);
  a.name = 'Clip A'; a.position = position(2);
  b.clipId = randomUUID(); b.name = 'Clip B'; b.position = position(3);
  doc.tracks[0].clips = [a, b]; doc.projectId = randomUUID(); doc.name = 'Stable Event order';
  const path = join(folder, 'order.minidaw'); await writeFile(path, JSON.stringify(doc));
  await invoke('open_project', { path, revision: p.revision, discard: true }); await idle(); await fit();
  await until(async () => await page.locator('.audio-clip').count() === 2, 'Two Events');
  const initial = await rows(); report.initial = initial;
  assert.deepEqual(initial.map(r => r.id), [a.clipId, b.clipId], 'Initially A above B');
  const destinations = baseline ? [.5] : [.5, 5, 8.5, 0, 2, 6.75, 3];
  for (const at of destinations) {
    const before = structuredClone((await project()).document);
    const previous = clips(await project()).find(c => c.clipId === b.clipId);
    await editDrag(b.clipId, at - seconds(previous));
    const moved = structuredClone((await project()).document), current = await rows();
    report.moves.push({ at, rows: current });
    assert.ok(Math.abs(seconds(moved.tracks[0].clips[1]) - at) < .02, 'Horizontal destination');
    const onlyPosition = structuredClone(moved); onlyPosition.tracks[0].clips[1].position = previous.position;
    assert.deepEqual(onlyPosition, before, 'Move changes only position in the document');
    if (baseline) {
      assert.deepEqual(current.map(r => r.id), [b.clipId, a.clipId], 'Reproduced old vertical swap');
      report.reproduced = true; break;
    }
    assert.deepEqual(current, initial, 'Moving B cannot repack rows or change their height');
    await key('Control+z'); assert.deepEqual((await project()).document, before); assert.deepEqual(await rows(), initial);
    await key('Control+Shift+z'); assert.deepEqual((await project()).document, moved); assert.deepEqual(await rows(), initial);
  }
  if (!baseline) {
    report.checks.push('A/B initial vertical positions recorded; B moved to 7 positions before/after A, equal starts and overlap/non-overlap; every Move/Undo/Redo retains exact row tops/heights.');
    await editDrag(a.clipId, 2.5); assert.deepEqual(await rows(), initial);
    await key('Control+z'); assert.deepEqual(await rows(), initial);
    // Left trim crosses the other Clip start; only source/time bounds may change.
    await editDrag(b.clipId, -1.2); assert.deepEqual(await rows(), initial);
    await editDrag(b.clipId, .4, 'trim-left'); assert.deepEqual(await rows(), initial);
    await editDrag(b.clipId, -.4, 'trim-right'); assert.deepEqual(await rows(), initial);
    report.checks.push('Move of A and left/right Trim of B retain selection and vertical order, including a Trim crossing A start.');
    const beforeSplit = structuredClone((await project()).document);
    await page.locator('[data-command="tool.split"]').click();
    const box = await page.locator(`.audio-clip[data-clip-id="${b.clipId}"]`).boundingBox();
    await page.mouse.click(box.x + box.width / 2, box.y + box.height * .6); await idle();
    p = await project(); assert.equal(clips(p).length, 3);
    assert.equal(clips(p)[1].sourceEnd, clips(p)[2].sourceStart, 'Split preserves source adjacency');
    assert.deepEqual((await rows()).map(r => r.id), clips(p).map(c => c.clipId));
    const split = structuredClone(p.document), splitRows = await rows();
    await key('Control+z'); assert.deepEqual((await project()).document, beforeSplit); assert.deepEqual(await rows(), initial);
    await key('Control+Shift+z'); assert.deepEqual((await project()).document, split); assert.deepEqual(await rows(), splitRows);
    await page.locator('[data-command="tool.objectSelection"]').click();
    await editDrag(clips(p)[2].clipId, -1); assert.deepEqual(await rows(), splitRows);
    report.checks.push('Split creates source-adjacent fragments; Undo/Redo restores document and row order; moving a split fragment does not repack rows.');
    await page.screenshot({ path: 'docs/validation/ui-clip-order.png' });
    assert.deepEqual(errors, []); report.passed = true;
  }
} catch (e) { report.error = String(e.stack ?? e); process.exitCode = 1; }
finally {
  if (page && prefs) await page.evaluate(p => {
    for (const [k, v] of [['minidaw.ui.panels.v1', p.panels], ['minidaw.ui.fontScale', p.font]]) {
      if (v === null) localStorage.removeItem(k); else localStorage.setItem(k, v);
    }
  }, prefs).catch(() => {});
  if (browser) await browser.close();
  if (child?.exitCode === null) { const exit = once(child, 'exit'); child.kill(); await exit; }
  if (savedRecent) await writeFile(recent, savedRecent); else await unlink(recent).catch(() => {});
  await writeFile(`docs/validation/ui-clip-order${baseline ? '-before' : ''}.json`, JSON.stringify(report, null, 2));
  console.log(JSON.stringify(report));
}
