import { test } from "node:test";
import assert from "node:assert/strict";
import { load, makePage, fixtureHtml, fakeTimings, fakeFetch, FakeAudio } from "./helpers.mjs";

const BASE = "https://lectover.oetiker.ch/olten.jetzt/frohburg-spiel-satz-patt/";

async function setup({ voices = [["erzaehler", 0.3]], byline = true, html = fixtureHtml(), scriptAttrs = {}, prepare = null, stored = null } = {}) {
  const v = load();
  const page = makePage(html, { byline });
  for (const [k, val] of Object.entries(scriptAttrs)) page.script.setAttribute(k, val);
  if (prepare) prepare(page);
  const blocks = v.extractBlocks(page.content);
  const hashes = await Promise.all(blocks.map((b) => v.hashText(b.text)));
  const routes = {};
  const entries = voices.map(([id, step, label]) => {
    routes[`${BASE}${id}.aaaa1111.json`] = fakeTimings(blocks, hashes, { voice: id, step });
    return { id, label: label || id, audio: `${id}.aaaa1111.ogg`, timings: `${id}.aaaa1111.json`, duration: 1 };
  });
  routes[BASE + "manifest.json"] = { version: 1, default: voices[0][0], voices: entries };
  if (stored) page.win.localStorage.setItem("lectover.pos.frohburg-spiel-satz-patt", JSON.stringify({ ...stored, hash: hashes[stored.block], t: Date.now() }));
  page.win.fetch = fakeFetch(routes);
  page.win.console = { warn() {} };
  const audio = new FakeAudio();
  const c = await v.init(page.win, page.script, { createAudio: () => audio });
  const bar = page.doc.querySelector(".lo-bar");
  return { v, ...page, audio, c, blocks, bar };
}

const click = (s, e) => e.dispatchEvent(new s.win.MouseEvent("click", { bubbles: true }));

test("style injected once and nothing visible before start", async () => {
  const s = await setup();
  assert.equal(s.doc.querySelectorAll("#lo-style").length, 1);
  assert.equal(s.bar.hidden, true);
  assert.ok(!s.doc.documentElement.classList.contains("lo-active"));
  s.c.start(0);
  assert.equal(s.bar.hidden, false);
  s.c.close();
  assert.equal(s.bar.hidden, true);
});

test("byline Anhören link after reading time starts at the top of the audio", async () => {
  const s = await setup();
  const next = s.doc.querySelector(".byline-reading-time").nextElementSibling;
  assert.ok(next.classList.contains("lo-byline"));
  const btn = next.querySelector("button");
  assert.equal(btn.textContent.trim(), "Anhören");
  assert.equal(btn.querySelector("svg").getAttribute("stroke"), "currentColor");
  click(s, btn);
  assert.equal(s.c.state().open, true);
  assert.equal(s.c.state().cur, -1); // the intro plays before word 0
  assert.equal(s.audio.currentTime, 0);
});

test("byline fallback before first block when no byline element", async () => {
  const s = await setup({ byline: false });
  const wrap = s.doc.querySelector(".lo-listen-wrap");
  assert.ok(wrap);
  assert.equal(wrap.nextElementSibling, s.blocks[0].el);
  assert.equal(wrap.querySelector("button").textContent.trim(), "Anhören");
});

test("hover speaker per block starts at that block", async () => {
  const s = await setup();
  const before = s.content.textContent;
  const spk = [...s.doc.querySelectorAll(".lo-spk")];
  assert.equal(spk.length, 13);
  assert.equal(spk[2].getAttribute("aria-label"), "Ab hier vorlesen");
  click(s, spk[2]);
  const first = s.blocks.slice(0, 2).reduce((n, b) => n + b.tokens.length, 0);
  assert.equal(s.c.state().cur, first);
  assert.equal(s.content.textContent, before);
});

test("bar has region role and German labels", async () => {
  const s = await setup();
  assert.equal(s.bar.getAttribute("role"), "region");
  assert.equal(s.bar.getAttribute("aria-label"), "Vorlesen");
  const labels = [...s.bar.querySelectorAll("[aria-label]")].map((e) => e.getAttribute("aria-label"));
  for (const l of ["Wiedergabe", "Vorheriger Absatz", "Nächster Absatz", "Tempo", "Stimme", "Vorlesen schliessen", "Fortschritt"]) {
    assert.ok(labels.includes(l), l);
  }
  assert.equal(s.bar.querySelector(".lo-track").getAttribute("role"), "slider");
  assert.deepEqual([...s.bar.querySelectorAll("select.lo-speed option")].map((o) => o.textContent), ["0,8×", "0,9×", "1×", "1,1×", "1,25×", "1,5×", "2×"]);
  s.c.start(0);
  assert.equal(s.bar.querySelector(".lo-play").getAttribute("aria-label"), "Pause");
  assert.match(s.bar.querySelector(".lo-sub").textContent, /^Absatz 1 von 13$/);
});

test("voice select hidden for one voice, listed for two", async () => {
  const one = await setup();
  assert.equal(one.bar.querySelector("select.lo-voice").hidden, true);
  const two = await setup({ voices: [["a", 0.3, "Anna"], ["b", 0.2, "Marco"]] });
  const sel = two.bar.querySelector("select.lo-voice");
  assert.equal(sel.hidden, false);
  assert.deepEqual([...sel.options].map((o) => o.textContent), ["Anna", "Marco"]);
  sel.value = "b";
  sel.dispatchEvent(new two.win.Event("change", { bubbles: true }));
  assert.equal(two.c.state().voice, "b");
});

test("track click seeks proportionally", async () => {
  const s = await setup();
  s.c.start(0);
  s.audio.duration = 152.3;
  const track = s.bar.querySelector(".lo-track");
  track.getBoundingClientRect = () => ({ left: 0, width: 200, top: 0, height: 6 });
  track.dispatchEvent(new s.win.MouseEvent("click", { bubbles: true, clientX: 100 }));
  assert.ok(Math.abs(s.audio.currentTime - 76.15) < 1e-6);
});

test("dragging the knob seeks along", async () => {
  const s = await setup();
  s.c.start(0);
  s.audio.duration = 200;
  const track = s.bar.querySelector(".lo-track");
  track.getBoundingClientRect = () => ({ left: 0, width: 200, top: 0, height: 6 });
  const ev = (type, x) => track.dispatchEvent(new s.win.MouseEvent(type, { bubbles: true, clientX: x, button: 0 }));
  ev("pointerdown", 20);
  assert.ok(Math.abs(s.audio.currentTime - 20) < 1e-6);
  ev("pointermove", 120);
  assert.ok(Math.abs(s.audio.currentTime - 120) < 1e-6);
  ev("pointerup", 150);
  assert.ok(Math.abs(s.audio.currentTime - 150) < 1e-6);
  // moving without a pressed pointer does nothing
  ev("pointermove", 10);
  assert.ok(Math.abs(s.audio.currentTime - 150) < 1e-6);
});

test("time display formats m:ss", async () => {
  const s = await setup();
  s.c.start(0);
  s.audio.duration = 152.3;
  s.audio.currentTime = 61;
  s.audio.dispatchEvent(new Event("timeupdate"));
  assert.equal(s.bar.querySelector(".lo-time").textContent, "1:01 / 2:32");
  assert.equal(s.bar.querySelector(".lo-el").textContent, "1:01");
  assert.equal(s.bar.querySelector(".lo-tot").textContent, "2:32");
});

test("no title in the bar and no 10-word buttons", async () => {
  const s = await setup();
  assert.equal(s.bar.querySelector("strong"), null);
  assert.equal(s.bar.querySelector(".lo-title"), null);
  const labels = [...s.bar.querySelectorAll("[aria-label]")].map((e) => e.getAttribute("aria-label"));
  assert.ok(!labels.some((l) => /^10 Wörter/.test(l)));
});

test("track arrow keys still skip 10 words", async () => {
  const s = await setup();
  s.c.start(0);
  const track = s.bar.querySelector(".lo-track");
  track.dispatchEvent(new s.win.KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
  assert.equal(s.c.state().cur, 10);
  track.dispatchEvent(new s.win.KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true }));
  assert.equal(s.c.state().cur, 0);
});

test("step button shows speed and steps through the list, wrapping to 0,8", async () => {
  const s = await setup();
  const step = s.bar.querySelector(".lo-step");
  assert.equal(step.textContent, "1×");
  assert.equal(step.getAttribute("aria-label"), "Tempo 1×");
  const seen = [];
  for (let i = 0; i < 7; i++) { click(s, step); seen.push(step.textContent); }
  assert.deepEqual(seen, ["1,1×", "1,25×", "1,5×", "2×", "0,8×", "0,9×", "1×"]);
  click(s, step); click(s, step);
  assert.equal(step.getAttribute("aria-label"), "Tempo 1,25×");
});

test("stored speed outside the list: step button 1×, next tap 1,1×", async () => {
  const v = load();
  const page = makePage(fixtureHtml(), { byline: true });
  const blocks = v.extractBlocks(page.content);
  const hashes = await Promise.all(blocks.map((b) => v.hashText(b.text)));
  page.win.localStorage.setItem("lectover.speed", "7");
  page.win.fetch = fakeFetch({
    [BASE + "manifest.json"]: { version: 1, default: "e", voices: [{ id: "e", label: "e", audio: "e.aaaa1111.ogg", timings: "e.aaaa1111.json", duration: 1 }] },
    [`${BASE}e.aaaa1111.json`]: fakeTimings(blocks, hashes, { voice: "e", step: 0.3 })
  });
  page.win.console = { warn() {} };
  await v.init(page.win, page.script, { createAudio: () => new FakeAudio() });
  const step = page.doc.querySelector(".lo-step");
  assert.equal(step.textContent, "1×");
  step.dispatchEvent(new page.win.MouseEvent("click", { bubbles: true }));
  assert.equal(step.textContent, "1,1×");
});

test("sub line is Absatz N von M", async () => {
  const s = await setup();
  s.c.start(0);
  assert.equal(s.bar.querySelector(".lo-sub").textContent, "Absatz 1 von 13");
});

test("bar has lo-ends row with elapsed and total below the track", async () => {
  const s = await setup();
  const ends = s.bar.querySelector(".lo-ends");
  assert.ok(ends);
  assert.equal(ends.previousElementSibling, s.bar.querySelector(".lo-track"));
  assert.ok(ends.querySelector(".lo-el") && ends.querySelector(".lo-tot"));
  assert.ok(s.bar.querySelector(".lo-row .lo-time"));
});

test("CSS: 760 px query swaps desktop and phone elements, 16 px fields", async () => {
  const s = await setup();
  const css = s.doc.getElementById("lo-style").textContent;
  const m = css.match(/@media \(max-width:760px\)\{([^@]*)\}/);
  assert.ok(m, "760 media query");
  assert.match(m[1], /\.lo-speed,\.lo-sub,\.lo-time,\.lo-vol\{display:none\}/);
  assert.match(m[1], /\.lo-step,\.lo-ends\{display:/);
  assert.match(m[1], /\.lo-bar select\{font-size:16px/);
  // A long voice label must not push the row past 360 px: the select shrinks, the rest stays.
  assert.match(m[1], /\.lo-bar select\{[^}]*flex:0 1 auto;min-width:0;/);
});

test("voice select rebuilds when the controller's voice list shrinks", async () => {
  const s = await setup({ voices: [["a", 0.3, "Anna"], ["b", 0.2, "Marco"]] });
  const sel = s.bar.querySelector("select.lo-voice");
  assert.equal(sel.hidden, false);
  const orig = s.c.voices;
  s.c.voices = () => ["b"];
  s.c.setSpeed(1.5); // fires onChange
  assert.equal(sel.hidden, true);
  assert.deepEqual([...sel.options].map((o) => o.textContent), ["Marco"]);
  s.c.voices = orig;
  s.c.setSpeed(1);
  assert.equal(sel.hidden, false);
  assert.deepEqual([...sel.options].map((o) => o.textContent), ["Anna", "Marco"]);
});

test("needsScroll band", () => {
  const { needsScroll } = load();
  assert.equal(needsScroll(300, 320, 1000, 80), false);
  assert.equal(needsScroll(900, 920, 1000, 80), true);
  assert.equal(needsScroll(50, 70, 1000, 80), true);
});

test("manual wheel suspends autoscroll for 4 s", async () => {
  const s = await setup();
  let t = 1000;
  Object.defineProperty(s.win, "performance", { value: { now: () => t }, configurable: true });
  const calls = [];
  s.win.scrollBy = (o) => calls.push(o);
  s.win.innerHeight = 1000;
  s.win.HTMLElement.prototype.getBoundingClientRect = function () {
    return { top: 900, bottom: 920, left: 0, right: 10, width: 10, height: 20 };
  };
  s.c.start(1);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].behavior, "smooth");
  assert.equal(calls[0].top, 900 - 400);
  t += 1000;
  s.win.dispatchEvent(new s.win.Event("wheel"));
  t += 3900;
  s.c.skipWords(1);
  assert.equal(calls.length, 1);
  t += 200;
  s.c.skipWords(1);
  assert.equal(calls.length, 2);
});

function fakeSession(failOn = null) {
  const ms = { metadata: null, playbackState: "none", handlers: {} };
  ms.setActionHandler = (a, fn) => {
    if (a === failOn) throw new TypeError("unsupported action");
    ms.handlers[a] = fn;
  };
  return ms;
}
function mediaPrepare(ms) {
  return ({ win, doc }) => {
    win.MediaMetadata = function (o) { Object.assign(this, o); };
    if (ms) Object.defineProperty(win.navigator, "mediaSession", { value: ms, configurable: true });
    const h1 = doc.createElement("h1");
    h1.textContent = " Frohburg spielt ";
    doc.querySelector("header").appendChild(h1);
  };
}

test("media session: metadata, handlers and playbackState", async () => {
  const ms = fakeSession();
  const s = await setup({ voices: [["a", 0.3, "Anna"], ["b", 0.3, "Beat"]], prepare: mediaPrepare(ms) });
  s.c.start(0);
  assert.equal(ms.metadata.title, "Frohburg spielt");
  assert.equal(ms.metadata.artist, "Anna");
  for (const a of ["play", "pause", "previoustrack", "nexttrack", "seekto"]) assert.equal(typeof ms.handlers[a], "function", a);
  s.audio.paused = false;
  s.c.update();
  s.audio.dispatchEvent(new Event("play"));
  assert.equal(ms.playbackState, "playing");
  ms.handlers.pause();
  s.audio.dispatchEvent(new Event("pause"));
  assert.equal(ms.playbackState, "paused");
  assert.equal(s.c.state().playing, false);
  ms.handlers.play();
  assert.equal(s.c.state().playing, true);
  s.c.setVoice("b");
  assert.equal(ms.metadata.artist, "Beat");
});

test("media session: track and seek handlers use the controller", async () => {
  const ms = fakeSession();
  const s = await setup({ prepare: mediaPrepare(ms) });
  s.c.start(0);
  ms.handlers.nexttrack();
  const afterNext = s.c.state().cur;
  assert.ok(afterNext > 0);
  ms.handlers.previoustrack();
  assert.ok(s.c.state().cur < afterNext);
  ms.handlers.seekto({ seekTime: s.c.duration() / 2 });
  assert.ok(Math.abs(s.audio.currentTime - s.c.duration() / 2) < 1e-6);
});

test("media session: closed bar ignores track and seek actions", async () => {
  const ms = fakeSession();
  const s = await resumed2(ms);
  const before = s.c.resumePoint();
  assert.equal(s.c.state().open, false);
  ms.handlers.nexttrack();
  ms.handlers.previoustrack();
  ms.handlers.seekto({ seekTime: 10 });
  assert.equal(s.c.resumePoint(), before);
  assert.equal(s.c.state().open, false);
  assert.equal(s.audio.currentTime, 0);
  assert.equal(s.audio.playCalls, 0);
});

test("media session: play while closed starts at the saved position", async () => {
  const ms = fakeSession();
  const s = await resumed2(ms);
  ms.handlers.play();
  assert.equal(s.c.state().open, true);
  assert.equal(s.c.state().cur, 7);
  assert.equal(s.audio.playCalls, 1);
});

test("media session: absent API is harmless", async () => {
  const s = await setup({ prepare: mediaPrepare(null) });
  s.c.start(0);
  assert.equal(s.c.state().open, true);
});

test("media session: one unsupported action does not block the others", async () => {
  const ms = fakeSession("seekto");
  const s = await setup({ prepare: mediaPrepare(ms) });
  s.c.start(0);
  assert.equal(typeof ms.handlers.play, "function");
  assert.equal(typeof ms.handlers.nexttrack, "function");
  assert.equal(ms.handlers.seekto, undefined);
});

// Resume buttons. Token 7 at 36 s per word starts at 252 s = 4:12.
const bylineButtons = (s) => [...s.doc.querySelector(".lo-byline").querySelectorAll("button")];
const shown = (b) => !b.hidden;
async function resumed() {
  const probe = await setup();
  // find (block, word) giving flat token 7
  let n = 0, block = 0;
  while (n + probe.blocks[block].tokens.length <= 7) n += probe.blocks[block++].tokens.length;
  return setup({ voices: [["erzaehler", 36]], stored: { block, word: 7 - n } });
}

async function resumed2(ms) {
  const probe = await setup();
  let n = 0, block = 0;
  while (n + probe.blocks[block].tokens.length <= 7) n += probe.blocks[block++].tokens.length;
  return setup({ voices: [["erzaehler", 36]], stored: { block, word: 7 - n }, prepare: mediaPrepare(ms) });
}

test("saved position: Weiterhören ab 4:12 + Von vorne, nothing plays or highlights yet", async () => {
  const s = await resumed();
  const [a, b] = bylineButtons(s);
  assert.equal(a.textContent.trim(), "Weiterhören ab 4:12");
  assert.equal(b.textContent.trim(), "Von vorne");
  assert.ok(shown(b));
  assert.equal(s.c.timeOf(7), 252);
  assert.equal(s.audio.playCalls, 0);
  assert.equal(s.bar.hidden, true);
  assert.equal(s.doc.querySelectorAll(".lo-cur").length, 0);
});

test("click Weiterhören opens at the saved token and plays", async () => {
  const s = await resumed();
  click(s, bylineButtons(s)[0]);
  assert.equal(s.c.state().open, true);
  assert.equal(s.c.state().cur, 7);
  assert.equal(s.audio.currentTime, 252);
  assert.equal(s.audio.playCalls, 1);
});

test("click Von vorne starts at the top of the audio and plays", async () => {
  const s = await resumed();
  click(s, bylineButtons(s)[1]);
  assert.equal(s.c.state().cur, -1);
  assert.equal(s.audio.currentTime, 0);
  assert.equal(s.audio.playCalls, 1);
});

test("Von vorne forgets the saved position", async () => {
  const s = await resumed();
  click(s, bylineButtons(s)[1]);
  assert.equal(s.c.resumePoint(), null);
  assert.equal([...Object.keys(s.win.localStorage)].filter((k) => k.startsWith("lectover.pos.")).length, 0);
});

test("no saved position: only Anhören, Von vorne hidden", async () => {
  const s = await setup();
  const [a, b] = bylineButtons(s);
  assert.equal(a.textContent.trim(), "Anhören");
  assert.ok(!shown(b));
});

test("saved token 0 counts as no position", async () => {
  const s = await setup({ stored: { block: 0, word: 0 } });
  const [a, b] = bylineButtons(s);
  assert.equal(a.textContent.trim(), "Anhören");
  assert.ok(!shown(b));
});

test("live: pause at token N relabels, close keeps it, ended resets", async () => {
  const s = await setup({ voices: [["erzaehler", 36]] });
  s.c.start(0);
  s.c.seek(3);
  s.audio.paused = false;
  s.c.toggle();
  const [a, b] = bylineButtons(s);
  assert.equal(a.textContent.trim(), "Weiterhören ab 1:48");
  assert.ok(shown(b));
  s.c.close();
  assert.equal(a.textContent.trim(), "Weiterhören ab 1:48");
  click(s, a);
  assert.equal(s.c.state().cur, 3);
  s.audio.dispatchEvent(new Event("ended"));
  assert.equal(a.textContent.trim(), "Anhören");
  assert.ok(!shown(b));
});

test("hover speaker still starts at its paragraph with a saved position", async () => {
  const s = await resumed();
  const spk = s.doc.querySelectorAll(".lo-spk");
  click(s, spk[1]);
  assert.equal(s.c.state().cur, s.blocks[0].tokens.length);
});

test("css hides the Von vorne button when [hidden]", async () => {
  const s = await setup();
  assert.ok(s.doc.getElementById("lo-style").textContent.includes(".lo-again[hidden]{display:none!important}"));
});

test("render writes the byline label only when it changed", async () => {
  const s = await resumed();
  const [a] = bylineButtons(s);
  let writes = 0;
  const mo = new s.win.MutationObserver((recs) => { writes += recs.length; });
  mo.observe(a, { childList: true, subtree: true });
  s.audio.dispatchEvent(new Event("timeupdate"));
  s.audio.dispatchEvent(new Event("timeupdate"));
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(writes, 0);
});

test("desktop speed dropdown sets and follows the speed", async () => {
  const s = await setup();
  s.c.start(0);
  const sel = s.bar.querySelector("select.lo-speed");
  assert.equal(sel.getAttribute("aria-label"), "Tempo");
  assert.equal(sel.value, String(s.c.state().speed));
  sel.value = "1.25";
  sel.dispatchEvent(new s.win.Event("change"));
  assert.equal(s.c.state().speed, 1.25);
  s.bar.querySelector(".lo-step").click();
  assert.equal(sel.value, "1.5");
});

test("CSS: desktop bar is a centred floating pill", async () => {
  const s = await setup();
  const css = s.doc.getElementById("lo-style").textContent;
  const m = css.match(/@media \(min-width:761px\)\{([^@]*)\}/);
  assert.ok(m, "desktop media query");
  assert.match(m[1], /\.lo-bar\{[^}]*left:50%;[^}]*transform:translateX\(-50%\);[^}]*border-radius:999px/);
  assert.match(m[1], /max-width:calc\(100vw - 192px\)/);
});

test("byline separates Weiterhören and Von vorne with ' / ', hidden without a position", async () => {
  const s = await resumed();
  const [a, b] = bylineButtons(s);
  const sep = a.nextElementSibling;
  assert.ok(sep.classList.contains("lo-sep"));
  assert.equal(sep.textContent, " / ");
  assert.equal(sep.nextElementSibling, b);
  assert.equal(sep.hidden, false);
  const none = await setup();
  assert.equal(none.doc.querySelector(".lo-sep").hidden, true);
  const css = none.doc.getElementById("lo-style").textContent;
  assert.ok(css.includes(".lo-sep[hidden]{display:none!important}"));
});

test("CSS: phone bar keeps controls clear of rounded corners and the home indicator", async () => {
  const s = await setup();
  const css = s.doc.getElementById("lo-style").textContent;
  const m = css.match(/@media \(max-width:760px\)\{([^@]*)\}/);
  assert.match(m[1], /\.lo-row\{[^}]*max\(20px,env\(safe-area-inset-left\)\)/);
  assert.match(m[1], /\.lo-row\{[^}]*env\(safe-area-inset-bottom\)/);
  assert.match(m[1], /\.lo-ends\{padding:[^}]*max\(20px,env\(safe-area-inset-left\)\)/);
});

test("desktop volume slider sets, stores and restores the volume", async () => {
  const s = await setup();
  s.c.start(0);
  const vol = s.bar.querySelector("input.lo-vol");
  assert.equal(vol.type, "range");
  assert.equal(vol.getAttribute("aria-label"), "Lautstärke");
  assert.equal(s.audio.volume, 1);
  vol.value = "0.4";
  vol.dispatchEvent(new s.win.Event("input"));
  assert.equal(s.audio.volume, 0.4);
  assert.equal(s.c.state().volume, 0.4);
  assert.equal(s.win.localStorage.getItem("lectover.volume"), "0.4");
  const again = await setup({ prepare: (p) => p.win.localStorage.setItem("lectover.volume", "0.3") });
  assert.equal(again.audio.volume, 0.3);
  assert.equal(again.bar.querySelector("input.lo-vol").value, "0.3");
});

test("stored volume outside 0..1 or garbage falls back to 1", async () => {
  const s = await setup({ prepare: (p) => p.win.localStorage.setItem("lectover.volume", "7") });
  assert.equal(s.audio.volume, 1);
});
