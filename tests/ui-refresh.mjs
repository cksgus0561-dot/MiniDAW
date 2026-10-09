import assert from "node:assert/strict";
import { resolve } from "node:path";
import { writeFile } from "node:fs/promises";

// Runs inside the real release WebView2, with the real Rust engine. The wrappers
// below only count forwarded calls/draws; they never supply mock return values.
export async function verifyUiRefresh({ page, snapshot, until, report, directory }) {
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.locator("#zoom-fit").click();
  await pause(150);
  await page.evaluate(() => {
    const probe = window.uiProbe = { seeks: [], commands: 0, active: 0, maxActive: 0, paints: 0, views: 0, pointer: null, moves: 0, delayMs: 0, presses: [] };
    const original = window.fetch;
    window.fetch = function(url, options) {
      if (String(url).endsWith("/asset_waveform")) probe.views++;
      const args = String(url).endsWith("/transport_command") ? JSON.parse(options.body) : null;
      if (args) probe.commands++;
      if (args?.action !== "seek") return original.call(this, url, options);
      probe.seeks.push({ seconds: args.seconds, at: performance.now() });
      probe.maxActive = Math.max(probe.maxActive, ++probe.active);
      const delay = probe.delayMs;
      return original.call(this, url, options).then(async response => {
        if (delay) await new Promise(resolve => setTimeout(resolve, delay));
        return response;
      }).finally(() => { probe.active--; });
    };
    const stroke = CanvasRenderingContext2D.prototype.stroke;
    CanvasRenderingContext2D.prototype.stroke = function(...args) { probe.paints++; return stroke.apply(this, args); };
    const waveform = document.getElementById("waveform");
    waveform.addEventListener("pointerdown", event => {
      probe.pointer = event.pointerId;
      probe.presses.push({ x: event.clientX, y: event.clientY, target: event.target.id, trusted: event.isTrusted });
    });
    waveform.addEventListener("pointermove", event => { if (event.buttons & 1) probe.moves++; });
  });
  const probe = () => page.evaluate(() => window.uiProbe);
  const center = () => page.locator("#playhead").evaluate(el => {
    const r = el.getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + 4 };
  });
  const settled = async expected => {
    await until(async () => Math.abs((await snapshot()).position - expected) < .003, `Final Rust position != ${expected}`);
    await pause(60); // also let the authoritative snapshot reach the UI
  };
  async function clickAt(seconds, start = 0, span = 10) {
    const before = await probe();
    const engineBefore = await snapshot();
    const r = await page.locator("#waveform").boundingBox();
    const x = r.x + (seconds - start) / span * r.width;
    await page.mouse.move(x, r.y + 12);
    await page.mouse.down();
    assert.ok(Math.abs((await center()).x - x) < 1, "Click down did not preview immediately");
    await pause(60);
    assert.equal((await probe()).commands, before.commands, "Click sought before release");
    assert.equal((await snapshot()).transport.appliedCommand, engineBefore.transport.appliedCommand);
    await page.mouse.up();
    await settled(seconds);
    assert.equal((await probe()).seeks.length - before.seeks.length, 1, "Click seek must still send exactly one command");
  }
  async function begin(ratio = .15, verticalRatio = .4) {
    const before = await probe();
    const r = await page.locator("#waveform").boundingBox();
    // Choose a point in the waveform, independently of the current playhead.
    const x = r.x + r.width * Math.min(.55,ratio);
    await page.mouse.move(x, r.y + 12);
    await page.mouse.down();
    assert.ok(await page.locator("#waveform").evaluate(el => el.classList.contains("dragging")), "Waveform did not start drag");
    assert.ok(await page.locator("#waveform").evaluate(el => el.hasPointerCapture(window.uiProbe.pointer)), "Waveform pointer capture missing");
    assert.ok(Math.abs((await center()).x - x) < 1, "Down did not immediately preview at the arbitrary press position");
    assert.equal((await probe()).commands, before.commands, "Waveform down sent a transport command");
    const press = (await probe()).presses.at(-1);
    assert.equal(press.target, "waveform");
    assert.equal(press.trusted, true, "Expected actual WebView mouse input, not a dispatched DOM event");
    assert.ok(Math.abs(press.x - x) < .01 && Math.abs(press.y - (r.y + 12)) < .01);
    return r;
  }
  async function previewAt(x, y) {
    await page.mouse.move(x, y);
    // The transform must already be updated without waiting for any Rust reply.
    const head = await center();
    assert.ok(Math.abs(head.x - x) < 1, `Drag preview lag: ${head.x} vs ${x}`);
  }
  async function dragTo(seconds, { from = .15, verticalRatio = .4, start = 0, span = 10, outside = 0 } = {}) {
    const before = await probe();
    const engineBefore = await snapshot();
    const r = await begin(from, verticalRatio);
    assert.equal((await probe()).commands, before.commands, "Pointer down sent a transport command");
    const x = r.x + (seconds - start) / span * r.width + outside;
    await page.mouse.move(x, r.y - 12, { steps: 12 }); // outside vertically: capture must survive
    assert.ok(await page.locator("#waveform").evaluate(el => el.classList.contains("dragging")));
    await pause(100); // Any old timer-based scrubbing would have fired by now.
    const during = await probe();
    assert.equal(during.commands, before.commands, "Pointer move sent a transport command");
    assert.equal(during.seeks.length, before.seeks.length);
    assert.equal((await snapshot()).position, engineBefore.position, "Preview changed stopped/paused Rust position");
    await page.mouse.up();
    await settled(seconds);
    const after = await probe();
    assert.equal(after.seeks.length - before.seeks.length, 1, "Pointer up must seek exactly once");
    assert.equal((await snapshot()).transport.appliedCommand, engineBefore.transport.appliedCommand + 1);
    assert.ok(!(await page.locator("#waveform").evaluate(el => el.hasPointerCapture(window.uiProbe.pointer))));
    assert.equal(after.paints, before.paints, "Drag redrew the waveform");
    assert.equal(after.views, before.views, "Drag fetched waveform cache");
    assert.ok(Math.abs(after.seeks.at(-1).seconds - seconds) < .000001, `Final seek must be exact: ${after.seeks.at(-1).seconds} vs ${seconds}; scroll=${await page.locator("#scroll").inputValue()}`);
    const actualCenter = await center();
    assert.ok(Math.abs(actualCenter.x - (r.x + (seconds - start) / span * r.width)) < 1, "Settled playhead disagrees with Rust");
  }

  await clickAt(1);
  await dragTo(2.2, { from: .08, verticalRatio: .02 }); // left -> right, timeline
  await dragTo(4.1, { from: .5, verticalRatio: .9 }); // right -> left, ruler right side
  await dragTo(0, { outside: -20 });
  await dragTo(6, { from: .5, outside: 20 });
  report.checks.push("Trusted mouse presses across the dedicated timeline ruler, left-to-right and right-to-left drags; 0 down/move commands, 1 up seek; outside capture and zero/end clamping");

  await clickAt(3.5);
  await page.locator("#zoom-in").click();
  await page.locator("#scroll").evaluate(el => { el.value = "2.5"; el.dispatchEvent(new Event("input", { bubbles: true })); });
  await until(async () => (await page.locator("#view-range").textContent()).includes("00:02.5"), "Scrolled zoom viewport not ready");
  await pause(100);
  const scrolledStart = Number(await page.locator("#scroll").inputValue());
  await dragTo(4.65, { from: .12, start: scrolledStart, span: 5 });
  await page.locator("#zoom-fit").click();
  await pause(100);
  report.checks.push("Drag maps correctly through 2x zoom and nonzero horizontal scroll");

  // Natural mouse drags while audio is already playing.
  await clickAt(1);
  await page.locator("#play-pause").click();
  await until(async () => (await snapshot()).transport.state === "playing", "Playback not ready for drag");
  const audioBefore = await snapshot();
  const before = await probe();
  const r = await begin(.5, .85);
  const began = Date.now();
  for (let i = 0; i < 5; i++) {
    await page.mouse.move(r.x + r.width * (i % 2 ? .25 : .5), r.y + 12, { steps: 19 });
    await previewAt(r.x + r.width * .4, r.y + 90);
  }
  const endX = r.x + r.width * .4;
  await pause(100);
  const during = await probe();
  const audioDuring = await snapshot();
  assert.equal(during.commands, before.commands, "Playing preview sent a transport command");
  assert.equal(during.seeks.length, before.seeks.length, "Playing preview sent a seek");
  assert.equal(audioDuring.transport.appliedCommand, audioBefore.transport.appliedCommand, "Rust received a command during drag");
  assert.equal(audioDuring.transport.state, "playing");
  assert.ok(audioDuring.position > audioBefore.position + .1, "Existing audio did not continue during preview");
  const renderedSeconds = (audioDuring.metrics.renderedFrames - audioBefore.metrics.renderedFrames) / audioDuring.output.sampleRate;
  assert.ok(Math.abs(audioDuring.position - audioBefore.position - renderedSeconds) < .03, "Existing playback jumped during preview");
  assert.ok(Math.abs((await center()).x - endX) < 1, "Engine snapshot overwrote drag preview");
  assert.equal(during.moves - before.moves, 100, "Expected 100 pressed mouse moves");
  await page.screenshot({ path: resolve(directory, "waveform-drag-preview.png") });
  await page.mouse.up();
  await until(async () => {
    const s = await snapshot(); return s.position >= 4 && s.position < 4.8 && s.transport.state === "playing";
  }, "Playing drag did not commit");
  const after = await probe();
  const audioAfter = await snapshot();
  assert.equal(after.paints, before.paints);
  assert.equal(after.views, before.views);
  assert.equal(after.maxActive, 1, "More than one seek in flight");
  assert.equal(after.seeks.length - before.seeks.length, 1, "Playing pointer up must seek exactly once");
  assert.equal(audioAfter.transport.appliedCommand, audioBefore.transport.appliedCommand + 1);
  assert.ok(Math.abs(after.seeks.at(-1).seconds - 4) < .000001);
  assert.ok(audioAfter.metrics.callbacks > audioBefore.metrics.callbacks + 10);
  assert.ok(audioAfter.metrics.nonSilentFrames > audioBefore.metrics.nonSilentFrames);
  assert.equal(audioAfter.streamErrors, audioBefore.streamErrors);
  assert.equal(audioAfter.metrics.overruns, audioBefore.metrics.overruns);
  await page.locator("#play-pause").click();
  await until(async () => (await snapshot()).transport.state === "paused", "Pause after drag failed");
  report.dragMetrics = { moves: after.moves - before.moves, seeksDuringDrag: during.seeks.length - before.seeks.length,
    transportCommandsDuringDrag: during.commands - before.commands, seeksAfterUp: after.seeks.length - before.seeks.length,
    elapsedMs: Date.now() - began, maxInFlight: after.maxActive, canvasStrokes: after.paints - before.paints,
    waveformRequests: after.views - before.views, positionBefore: audioBefore.position, positionBeforeUp: audioDuring.position,
    audioBefore: audioBefore.metrics, audioDuring: audioDuring.metrics, audioAfter: audioAfter.metrics };
  report.checks.push("Real release ruler mouse drag with 100 pressed moves: 0 transport/seek IPC until up, exactly 1 final seek; original audio continues, no waveform draw/cache request or callback error increase");

  await clickAt(2);
  const burstBefore = await probe();
  await begin();
  const burst = await page.evaluate(({ x, y, width }) => {
    const el = document.getElementById("waveform");
    for (let i = 0; i < 240; i++) el.dispatchEvent(new PointerEvent("pointermove", {
      pointerId: window.uiProbe.pointer, clientX: x + width * (i % 2 ? .2 : .7), clientY: y + 100, buttons: 1,
    }));
    const rect = document.getElementById("playhead").getBoundingClientRect();
    return { x: rect.x + rect.width / 2, requests: window.uiProbe.seeks.length };
  }, r);
  assert.ok(Math.abs(burst.x - (r.x + r.width * .2)) < 1);
  assert.equal(burst.requests, burstBefore.seeks.length, "Pointer burst sent a seek");
  // A distinct up coordinate must win, even without a preceding move event.
  await page.locator("#waveform").evaluate((el, { x, y }) => el.dispatchEvent(new PointerEvent("pointerup", {
    pointerId: window.uiProbe.pointer, clientX: x, clientY: y, button: 0,
  })), { x: endX, y: r.y + 120 });
  await page.mouse.up();
  await settled(4);
  assert.equal((await probe()).seeks.length - burstBefore.seeks.length, 1, "Duplicate up committed twice");
  assert.ok(Math.abs((await probe()).seeks.at(-1).seconds - 4) < .000001);
  report.checks.push("240-event burst: 0 intermediate seeks, distinct final pointer-up coordinate commits once, duplicate up ignored, paused state preserved");
  assert.equal((await snapshot()).transport.state, "paused");

  // Delay delivery of genuine Rust responses, without replacing their contents.
  // The final preview must survive delayed acknowledgment of its one commit.
  await page.evaluate(() => { window.uiProbe.delayMs = 250; });
  const delayedBefore = await probe();
  await begin();
  await previewAt(r.x + r.width * .6, r.y + 12);
  await page.mouse.up();
  await pause(80);
  assert.ok(Math.abs((await center()).x - (r.x + r.width * .6)) < 1, "Stale snapshot snapped back the final preview");
  await settled(6);
  await until(async () => (await probe()).active === 0, "Delayed final seek did not complete");
  await page.evaluate(() => { window.uiProbe.delayMs = 0; });
  assert.equal((await probe()).maxActive, 1);
  assert.equal((await probe()).seeks.length - delayedBefore.seeks.length, 1);
  report.checks.push("Delayed genuine IPC reply: only 1 final seek; final preview stays until Rust acknowledgment");

  for (const reason of ["pointercancel", "captureloss", "blur"]) {
    const cancelledBefore = await probe();
    const engineBefore = await snapshot();
    await begin();
    await previewAt(r.x + r.width * .3, r.y + 120);
    await page.locator("#waveform").evaluate((el, reason) => {
      if (reason === "pointercancel") el.dispatchEvent(new PointerEvent("pointercancel", { pointerId: window.uiProbe.pointer }));
      else if (reason === "captureloss") el.releasePointerCapture(window.uiProbe.pointer);
      else window.dispatchEvent(new Event("blur"));
    }, reason);
    await page.mouse.up();
    await pause(100);
    assert.equal((await probe()).commands, cancelledBefore.commands, `${reason} sent a transport command`);
    assert.equal((await snapshot()).position, engineBefore.position, `${reason} changed engine position`);
    assert.ok(Math.abs((await center()).x - (r.x + r.width * engineBefore.position / 10)) < 1, `${reason} did not restore engine display`);
    assert.ok(!(await page.locator("#waveform").evaluate(el => el.classList.contains("dragging") || el.hasPointerCapture(window.uiProbe.pointer))));
  }
  await page.locator("#stop").click();
  await settled(0);
  report.checks.push("Pointer cancel / capture loss / blur discard preview with 0 transport commands, release capture and restore engine display; stop still resets position");

  const theme = await page.evaluate(() => {
    const css = getComputedStyle(document.documentElement);
    const font = selector => parseFloat(getComputedStyle(document.querySelector(selector)).fontSize);
    return { background: css.backgroundColor, body: font("body"), timeline: font("canvas"),
      button: font("button"), metric: font("dt"), note: font(".metric-note"), file: font(".file-meta"),
      left: css.getPropertyValue("--wave-left").trim(), right: css.getPropertyValue("--wave-right").trim(),
      playhead: css.getPropertyValue("--playhead").trim(), cursor: getComputedStyle(document.getElementById("waveform")).cursor,
      playheadPointerEvents: getComputedStyle(document.getElementById("playhead")).pointerEvents };
  });
  assert.equal(theme.background, "rgb(25, 28, 31)");
  for (const key of ["body", "timeline", "button", "metric", "note", "file"]) assert.ok(theme[key] >= 13.99, `${key} font too small`);
  assert.equal(theme.cursor, "crosshair");
  assert.equal(theme.playheadPointerEvents, "none");
  report.theme = theme;

  // DPR emulation inside the existing real WebView; no global Windows settings.
  const cdp = await page.context().newCDPSession(page);
  report.layouts = [];
  for (const [width, height, scale] of [[1280, 800, 1], [800, 600, 1], [800, 600, 1.5], [640, 480, 2]]) {
    await page.setViewportSize({ width, height });
    await cdp.send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: scale, mobile: false });
    await pause(120);
    const layout = await page.evaluate(() => {
      const canvas = document.getElementById("waveform");
      const overflow = [".editor", ".performance", ".transport-bar", ".workspace"].filter(selector => {
        const el = document.querySelector(selector); return el.scrollWidth > el.clientWidth + 1;
      });
      return { width: innerWidth, height: innerHeight, dpr: devicePixelRatio, horizontalOverflow: document.documentElement.scrollWidth > innerWidth,
        overflow, backingWidth: canvas.width, cssWidth: canvas.clientWidth };
    });
    assert.equal(layout.horizontalOverflow, false);
    assert.deepEqual(layout.overflow, []);
    assert.ok(Math.abs(layout.backingWidth - layout.cssWidth * scale) <= 1, `Canvas does not honor DPR: ${JSON.stringify(layout)}`);
    report.layouts.push(layout);
    // Playwright screenshot resets a manual DPR override. Capture through the
    // same CDP session so both layout and screenshot use the measured density.
    const capture = await cdp.send("Page.captureScreenshot", { format: "png", fromSurface: true, captureBeyondViewport: false });
    const png = Buffer.from(capture.data, "base64");
    assert.equal(png.readUInt32BE(16), width * scale, "Screenshot width does not match emulated DPR");
    await writeFile(resolve(directory, `ui-refresh-${width}-${scale}x.png`), png);
  }
  await cdp.send("Emulation.clearDeviceMetricsOverride");
  await page.setViewportSize({ width: 1280, height: 800 });
  await pause(100);
  report.checks.push("Charcoal tokens, >=14px typography, readable desktop/compact layout, DPR 1/1.5/2 canvas backing and no horizontal overflow");
}

