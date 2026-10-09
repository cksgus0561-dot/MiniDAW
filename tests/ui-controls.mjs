import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function verifyControls({ page, snapshot, until, report, directory }) {
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const toggle = page.locator("#play-pause");
  const fit = async () => { await page.locator("#zoom-fit").click(); await pause(120); };
  const state = async expected => {
    await until(async () => (await snapshot()).transport.state === expected, `Transport != ${expected}`);
    await until(async () => (await toggle.getAttribute("aria-label")) === (expected === "playing" ? "일시정지" : "재생"), "Toggle label disagrees with Rust");
    assert.equal(await toggle.getAttribute("aria-pressed"), String(expected === "playing"));
    assert.match(await toggle.textContent(), expected === "playing" ? /⏸ 일시정지/ : /▶ 재생/);
  };
  await page.evaluate(() => {
    window.controlProbe = { commands: [], views: 0, viewActive: 0, maxViewActive: 0, clicks: [] };
    const p = window.controlProbe;
    const fetch = window.fetch;
    window.fetch = function(url, options) {
      if (String(url).endsWith("/transport_command")) p.commands.push({ ...JSON.parse(options.body), at: performance.now() });
      if (!String(url).endsWith("/asset_waveform")) return fetch.call(this, url, options);
      p.views++;
      p.maxViewActive = Math.max(p.maxViewActive, ++p.viewActive);
      return fetch.call(this, url, options).finally(() => p.viewActive--);
    };
    document.getElementById("play-pause").addEventListener("click", () => p.clicks.push(performance.now()), true);
  });
  const probe = () => page.evaluate(() => window.controlProbe);
  const viewport = () => page.evaluate(() => {
    const parts = document.getElementById("view-range").textContent.split(" — ");
    const seconds = value => { const [m, s] = value.split(":").map(Number); return m * 60 + s; };
    return { start: seconds(parts[0]), span: 10 - Number(document.getElementById("scroll").max) };
  });
  await page.locator("#stop").click();
  await state("stopped");
  await toggle.click(); await state("playing");
  await pause(100);
  await toggle.click(); await state("paused");
  const paused = await snapshot();
  await pause(90); assert.equal((await snapshot()).position, paused.position);
  await toggle.click(); await state("playing");
  const resumed = await snapshot();
  assert.ok(resumed.position >= paused.position && resumed.position < paused.position + .25);
  await page.locator("#stop").click(); await state("stopped");
  assert.equal((await snapshot()).position, 0);
  await toggle.click(); await state("playing");
  await toggle.click(); await state("paused");
  await page.locator("#stop").click(); await state("stopped");
  assert.equal((await snapshot()).position, 0);

  // Trusted fast mouse clicks; do not wait for a displayed state between them.
  const rapidBefore = await snapshot();
  for (let i = 0; i < 11; i++) await toggle.click({ force: true });
  await until(async () => (await snapshot()).transport.appliedCommand === rapidBefore.transport.appliedCommand + 11, "Rapid toggles dropped commands");
  await state("playing");
  await toggle.click(); await state("paused");
  // A same-JS-task burst exercises stale UI snapshots and FIFO order as well.
  const burstBefore = await snapshot();
  await toggle.evaluate(el => { for (let i = 0; i < 10; i++) el.click(); });
  await until(async () => (await snapshot()).transport.appliedCommand === burstBefore.transport.appliedCommand + 10, "Burst toggles dropped commands");
  await state("paused");
  const timing = await probe();
  const toggleCommands = timing.commands.filter(c => c.action === "toggle");
  report.toggle = { rapidClicks: 11, burstClicks: 10, commandCount: toggleCommands.length,
    firstClickToIpcMs: toggleCommands[0].at - timing.clicks[0], playMs: resumed.metrics.playMs };
  assert.ok(report.toggle.firstClickToIpcMs < 100, "Unexpected UI delay before toggle IPC");
  report.checks.push("Toggle: stopped/play/pause/resume, stop from playing/paused, 11 rapid real clicks + 10 stale-snapshot burst clicks; Rust/UI state and command counts agree");

  await page.locator("#stop").click(); await state("stopped");
  await fit();
  await toggle.click(); await state("playing");
  const audioBefore = await snapshot();
  const before = await probe();
  let bounds = await page.locator("#waveform").boundingBox();
  const ratio = .23;
  await page.mouse.move(bounds.x + bounds.width * ratio, bounds.y + 55);
  const initial = await viewport();
  await page.keyboard.down("Control");
  await page.mouse.wheel(0, -140);
  await until(async () => (await viewport()).span < initial.span, "Ctrl wheel did not zoom in");
  const zoomed = await viewport();
  assert.ok(Math.abs(initial.start + ratio * initial.span - zoomed.start - ratio * zoomed.span) < .002, "Cursor anchor shifted on zoom in");
  await page.mouse.wheel(0, 100);
  await until(async () => (await viewport()).span > zoomed.span, "Ctrl wheel did not zoom out");
  const out = await viewport();
  assert.ok(Math.abs(zoomed.start + ratio * zoomed.span - out.start - ratio * out.span) < .002, "Cursor anchor shifted on zoom out");
  for (let i = 0; i < 10; i++) await page.mouse.wheel(0, -35);
  await page.keyboard.up("Control");
  await pause(150);
  const scrolledBefore = await viewport();
  await page.keyboard.down("Shift"); await page.mouse.wheel(0, 80); await page.keyboard.up("Shift");
  await until(async () => (await viewport()).start > scrolledBefore.start, "Shift wheel did not scroll horizontally");
  const shifted = await viewport();
  assert.equal(shifted.span, scrolledBefore.span);
  const ordinaryBefore = await page.locator(".editor").evaluate(el => el.scrollTop);
  await page.mouse.wheel(0, 140);
  await pause(120);
  assert.deepEqual(await viewport(), shifted, "Ordinary wheel changed waveform view");
  assert.ok(await page.locator(".editor").evaluate(el => el.scrollTop) > ordinaryBefore, "Ordinary wheel was swallowed");
  await page.locator(".editor").evaluate(el => { el.scrollTop = 0; });
  await pause(100);
  const burstViewsBefore = (await probe()).views;
  await page.locator("#waveform").evaluate(el => {
    const r = el.getBoundingClientRect();
    for (let i = 0; i < 100; i++) el.dispatchEvent(new WheelEvent("wheel", { bubbles: true, cancelable: true,
      ctrlKey: true, deltaY: i % 2 ? 1 : -2, clientX: r.x + r.width * .23, clientY: r.y + 50 }));
  });
  await pause(150);
  const wheelAfter = await probe();
  const audioAfter = await snapshot();
  assert.equal(wheelAfter.commands.length, before.commands.length, "Wheel sent a transport command");
  assert.equal(audioAfter.transport.appliedCommand, audioBefore.transport.appliedCommand);
  assert.equal(audioAfter.cacheBuildMs, audioBefore.cacheBuildMs);
  assert.equal(audioAfter.transport.clipId, audioBefore.transport.clipId);
  assert.ok(wheelAfter.views - burstViewsBefore <= 2, "Wheel burst did not coalesce cache queries");
  assert.equal(wheelAfter.maxViewActive, 1);
  assert.ok(audioAfter.metrics.callbacks > audioBefore.metrics.callbacks);
  assert.ok(audioAfter.metrics.nonSilentFrames > audioBefore.metrics.nonSilentFrames);
  assert.equal(audioAfter.metrics.overruns, audioBefore.metrics.overruns);
  assert.equal(audioAfter.streamErrors, audioBefore.streamErrors);
  report.wheel = { cursorAnchorErrorSeconds: Math.abs(initial.start + ratio * initial.span - zoomed.start - ratio * zoomed.span),
    burstInputs: 100, burstViewRequests: wheelAfter.views - burstViewsBefore, maxViewActive: wheelAfter.maxViewActive,
    transportCommands: wheelAfter.commands.length - before.commands.length, cacheBuildMs: audioAfter.cacheBuildMs,
    before: audioBefore.metrics, after: audioAfter.metrics };
  await toggle.click(); await state("paused");

  // Click and drag through the wheel-created zoom + offset, not a reset viewport.
  const view = await viewport(); bounds = await page.locator("#waveform").boundingBox();
  await page.mouse.click(bounds.x + bounds.width * .65, bounds.y + 12);
  await until(async () => Math.abs((await snapshot()).position - (view.start + view.span * .65)) < .003, "Click seek after wheel zoom failed");
  const dragBefore = await probe();
  await page.mouse.move(bounds.x + bounds.width * .3, bounds.y + 12); await page.mouse.down();
  await page.mouse.move(bounds.x + bounds.width * .8, bounds.y + 12, { steps: 100 });
  assert.equal((await probe()).commands.length, dragBefore.commands.length);
  await page.mouse.up();
  await until(async () => Math.abs((await snapshot()).position - (view.start + view.span * .8)) < .003, "Drag seek after wheel zoom failed");
  assert.equal((await probe()).commands.length - dragBefore.commands.length, 1);
  await fit();
  await page.locator("#zoom-in").click();
  await until(async () => (await page.locator("#zoom-value").textContent()) === "2.0×", "Zoom button regressed");
  await page.locator("#zoom-out").click();
  await until(async () => (await page.locator("#zoom-value").textContent()) === "1.0×", "Zoom out button regressed");
  await page.locator("#stop").click(); await state("stopped");
  report.checks.push("Ctrl-wheel in/out anchored at 23% cursor, fast real wheels + 100-event coalescing, Shift pan, ordinary wheel default scroll, zoomed click/drag, +/- buttons; no transport commands or cache rebuild");

  report.fontLayouts = [];
  for (const scale of [90, 110, 125]) {
    await page.locator("#font-scale").selectOption(String(scale));
    for (const [width, height] of [[1280, 800], [800, 600]]) {
      await page.setViewportSize({ width, height });
      await pause(120);
      const layout = await page.evaluate(() => {
        const font = sel => parseFloat(getComputedStyle(document.querySelector(sel)).fontSize);
        const clipped = [".topbar", ".header-controls", ".transport-bar", ".workspace", ".editor", ".performance", ".zoom-controls", "#play-pause", "#font-scale"]
          .filter(sel => { const el = document.querySelector(sel); return el.scrollWidth > el.clientWidth + 1; });
        return { root: font("html"), timeline: font("canvas"), button: font("#play-pause"), panel: font("dd"), clipped,
          horizontalOverflow: document.documentElement.scrollWidth > innerWidth, saved: localStorage.getItem("minidaw.ui.fontScale") };
      });
      assert.ok(Math.abs(layout.root - 15 * scale / 100) < .01);
      assert.ok(Math.abs(layout.timeline - 14 * scale / 100) < .01);
      assert.deepEqual(layout.clipped, []);
      assert.equal(layout.horizontalOverflow, false);
      assert.equal(layout.saved, String(scale));
      report.fontLayouts.push({ scale, width, height, ...layout });
      await page.screenshot({ path: resolve(directory, `font-${scale}-${width}.png`) });
    }
  }
  await page.setViewportSize({ width: 1280, height: 800 });
  // Leave 125% saved for the separate actual-process-restart check.
  report.checks.push("Font 90/110(default)/125% scales root/Canvas/controls consistently, saves UI preference, no horizontal clipping at desktop/compact sizes");
}
