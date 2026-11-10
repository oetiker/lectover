import { test } from "node:test";
import assert from "node:assert/strict";
import { load, makePage, fixtureHtml, fakeTimings, fakeFetch, FakeAudio } from "./helpers.mjs";

const BASE = "https://lectover.oetiker.ch/olten.jetzt/frohburg-spiel-satz-patt/";
const EPS = 1e-6;
const near = (a, b) => assert.ok(Math.abs(a - b) < EPS, `${a} !~ ${b}`);

async function setup({ voices = [["erzaehler", 0.3]], def, stored = {}, audio = new FakeAudio(), noMp3 = [], lead = 0 } = {}) {
  const v = load();
  const page = makePage(fixtureHtml());
  const blocks = v.extractBlocks(page.content);
  const hashes = await Promise.all(blocks.map((b) => v.hashText(b.text)));
  const routes = {};
  const entries = voices.map(([id, step]) => {
    routes[`${BASE}${id}.aaaa1111.json`] = fakeTimings(blocks, hashes, { voice: id, step, lead });
    return { id, label: id, audio: `${id}.aaaa1111.ogg`, ...(noMp3.includes(id) ? {} : { mp3: `${id}.aaaa1111.mp3` }), timings: `${id}.aaaa1111.json`, duration: 1 };
  });
  routes[BASE + "manifest.json"] = { version: 1, default: def || voices[0][0], voices: entries };
  page.win.fetch = fakeFetch(routes);
  page.win.console = { warn() {} };
  for (const [k, val] of Object.entries(stored)) page.win.localStorage.setItem(k, val);
  const c = await v.init(page.win, page.script, { createAudio: () => audio });
  return { v, ...page, audio, c, blocks, routes, hashes };
}

const spansOf = (s, i) => [...s.doc.querySelectorAll(`[data-lo="${i}"]`)];
const withClass = (s, cls) => new Set([...s.doc.querySelectorAll("." + cls)].map((e) => e.getAttribute("data-lo")));

test("findToken", () => {
  const { findToken } = load();
  assert.equal(findToken([0, 0.3, 0.3, 0.9], 0.31), 2);
  assert.equal(findToken([0.5, 1], 0.1), -1);
  assert.equal(findToken([0, 1], 99), 1);
});

test("start sets src lazily, seeks and plays", async () => {
  const s = await setup();
  assert.ok(!s.audio.src);
  s.c.start(5);
  assert.ok(s.audio.src.endsWith(".ogg"));
  near(s.audio.currentTime, 1.5);
  assert.equal(s.audio.playCalls, 1);
  assert.equal(s.audio.preload, "metadata");
  assert.equal(s.c.state().open, true);
  assert.ok(s.doc.documentElement.classList.contains("lo-active"));
});

test("start from the top plays the intro before the first word", async () => {
  const s = await setup({ lead: 17 });
  s.c.start(0);
  assert.equal(s.audio.currentTime, 0);
  assert.equal(withClass(s, "lo-cur").size, 0);
  s.audio.currentTime = 17.1;
  s.c.update();
  assert.deepEqual([...withClass(s, "lo-cur")], ["0"]);
});

test("a resume point still starts at its word", async () => {
  const s = await setup({ lead: 17 });
  s.c.start(5);
  near(s.audio.currentTime, 17 + 1.5);
});

test("play after the end restarts with the intro", async () => {
  const s = await setup({ lead: 17 });
  s.c.start(0);
  s.audio.currentTime = s.audio.duration = 9999;
  s.audio.dispatchEvent(new Event("ended"));
  s.c.toggle();
  assert.equal(s.audio.currentTime, 0);
});

test("update marks current and read tokens", async () => {
  const s = await setup();
  s.c.start(0);
  s.audio.currentTime = 1.0;
  s.c.update();
  assert.deepEqual([...withClass(s, "lo-cur")], ["3"]);
  assert.deepEqual([...withClass(s, "lo-read")].sort(), ["0", "1", "2"]);
  assert.ok(spansOf(s, 3).every((e) => e.classList.contains("lo-cur")));
  s.audio.currentTime = 0.1;
  s.c.update();
  assert.deepEqual([...withClass(s, "lo-cur")], ["0"]);
  assert.equal(withClass(s, "lo-read").size, 0);
});

test("word click seeks only while open", async () => {
  const s = await setup();
  const target = spansOf(s, 12)[0];
  target.dispatchEvent(new s.win.MouseEvent("click", { bubbles: true }));
  assert.equal(s.audio.currentTime, 0);
  assert.equal(s.audio.playCalls, 0);
  s.c.start(0);
  target.dispatchEvent(new s.win.MouseEvent("click", { bubbles: true }));
  near(s.audio.currentTime, 3.6);
  assert.equal(s.audio.playCalls, 2);
});

test("word click inside a link does not seek", async () => {
  const s = await setup();
  s.c.start(0);
  const span = [...s.doc.querySelectorAll(".lo-w")].find((e) => e.closest("a"));
  assert.ok(span, "fixture has a linked token");
  const before = s.audio.currentTime;
  const ev = new s.win.MouseEvent("click", { bubbles: true, cancelable: true });
  span.dispatchEvent(ev);
  assert.equal(s.audio.currentTime, before);
  assert.equal(ev.defaultPrevented, false);
});

test("word click ignored with a non-collapsed selection", async () => {
  const s = await setup();
  s.c.start(0);
  const span = spansOf(s, 12)[0];
  s.win.getSelection().selectAllChildren(s.content);
  span.dispatchEvent(new s.win.MouseEvent("click", { bubbles: true }));
  assert.equal(s.audio.currentTime, 0);
});

const key = (s, target, k = " ", init = {}) => {
  const ev = new s.win.KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init });
  target.dispatchEvent(ev);
  return ev;
};

test("space toggles while open", async () => {
  const s = await setup();
  const closed = key(s, s.doc.body);
  assert.equal(closed.defaultPrevented, false);
  assert.equal(s.audio.playCalls, 0);
  s.c.start(0);
  assert.equal(s.audio.paused, false);
  const ev = key(s, s.doc.body);
  assert.equal(ev.defaultPrevented, true);
  assert.equal(s.audio.paused, true);
  key(s, s.doc.body);
  assert.equal(s.audio.paused, false);
  assert.equal(s.audio.playCalls, 2);
});

test("space with modifiers or auto-repeat is ignored", async () => {
  const s = await setup();
  s.c.start(0);
  assert.equal(s.audio.paused, false);
  for (const m of ["shiftKey", "ctrlKey", "metaKey", "altKey", "repeat"]) {
    const ev = key(s, s.doc.body, " ", { [m]: true });
    assert.equal(ev.defaultPrevented, false, m);
    assert.equal(s.audio.paused, false, m);
  }
});

test("space on focused button does not toggle twice", async () => {
  const s = await setup();
  s.c.start(0);
  const btn = s.doc.createElement("button");
  const inner = s.doc.createElement("span");
  btn.appendChild(inner);
  s.doc.body.appendChild(btn);
  let clicks = 0;
  btn.addEventListener("click", () => clicks++);
  const ev = key(s, inner);
  assert.equal(ev.defaultPrevented, false);
  btn.dispatchEvent(new s.win.MouseEvent("click", { bubbles: true }));
  assert.equal(clicks, 1);
  assert.equal(s.audio.paused, false);
  assert.equal(s.audio.playCalls, 1);
});

test("speed persists", async () => {
  const s = await setup();
  s.c.setSpeed(1.1);
  assert.equal(s.c.state().speed, 1.1);
  s.c.setSpeed(1.5);
  assert.equal(s.audio.playbackRate, 1.5);
  assert.equal(s.audio.preservesPitch, true);
  assert.equal(s.win.localStorage.getItem("lectover.speed"), "1.5");
  assert.equal(s.c.state().speed, 1.5);
  const s2 = await setup({ stored: { "lectover.speed": "1.5" } });
  assert.equal(s2.c.state().speed, 1.5);
  assert.equal(s2.audio.playbackRate, 1.5);
  const s4 = await setup({ stored: { "lectover.speed": "1.1" } });
  assert.equal(s4.c.state().speed, 1.1);
  const s3 = await setup({ stored: { "lectover.speed": "7" } });
  assert.equal(s3.c.state().speed, 1);
});

test("voice switch keeps the token", async () => {
  const s = await setup({ voices: [["a", 0.3], ["b", 0.5]] });
  assert.equal(s.c.state().voice, "a");
  s.c.start(7);
  near(s.audio.currentTime, 2.1);
  s.c.setVoice("b");
  assert.ok(s.audio.src.includes("b.aaaa1111.ogg"));
  assert.equal(s.win.localStorage.getItem("lectover.voice"), "b");
  s.audio.dispatchEvent(new Event("loadedmetadata"));
  near(s.audio.currentTime, 3.5);
  assert.equal(s.audio.paused, false);
  assert.equal(s.c.state().voice, "b");
});

test("voice switch while paused stays paused", async () => {
  const s = await setup({ voices: [["a", 0.3], ["b", 0.5]] });
  s.c.start(4);
  s.c.toggle();
  s.c.setVoice("b");
  s.audio.dispatchEvent(new Event("loadedmetadata"));
  near(s.audio.currentTime, 2.0);
  assert.equal(s.audio.paused, true);
});

test("initial voice: stored, then manifest default, then first valid", async () => {
  const two = [["a", 0.3], ["b", 0.5]];
  assert.equal((await setup({ voices: two, stored: { "lectover.voice": "b" } })).c.state().voice, "b");
  assert.equal((await setup({ voices: two, def: "b" })).c.state().voice, "b");
  assert.equal((await setup({ voices: two, stored: { "lectover.voice": "zz" }, def: "b" })).c.state().voice, "b");
  assert.equal((await setup({ voices: two, def: "nope" })).c.state().voice, "a");
});

test("skipWords, prevBlock, nextBlock", async () => {
  const s = await setup();
  const first = [];
  let n = 0;
  for (const b of s.c.blocks) { first.push(n); n += b.tokens.length; }
  const total = n;
  assert.ok(first.length >= 3);
  s.c.start(first[1] + 1);
  s.c.skipWords(2);
  assert.equal(s.c.state().cur, first[1] + 3);
  s.c.skipWords(-100000);
  assert.equal(s.c.state().cur, 0);
  s.c.skipWords(100000);
  assert.equal(s.c.state().cur, total - 1);
  s.c.seek(first[1] + 5);
  s.c.prevBlock();
  assert.equal(s.c.state().cur, first[1]);
  s.c.prevBlock();
  assert.equal(s.c.state().cur, first[0]);
  s.c.seek(first[1] + 2);
  s.c.prevBlock();
  assert.equal(s.c.state().cur, first[0]);
  s.c.seek(first[0]);
  s.c.nextBlock();
  assert.equal(s.c.state().cur, first[1]);
  s.c.seek(total - 1);
  s.c.nextBlock();
  assert.equal(s.c.state().cur, total - 1);
});

test("seekFraction", async () => {
  const s = await setup();
  s.c.start(0);
  s.audio.duration = 100;
  s.c.seekFraction(0.5);
  near(s.audio.currentTime, 50);
});

test("onChange fires on play, pause, seek, rate, voice", async () => {
  const s = await setup({ voices: [["a", 0.3], ["b", 0.5]] });
  let n = 0;
  s.c.onChange(() => n++);
  s.c.start(0);
  assert.ok(n > 0);
  for (const act of [() => s.c.toggle(), () => s.c.seek(3), () => s.c.setSpeed(2), () => s.c.setVoice("b")]) {
    const before = n;
    act();
    assert.ok(n > before);
  }
});

test("close clears highlight and dimming", async () => {
  const s = await setup();
  s.c.start(4);
  s.c.update();
  assert.ok(withClass(s, "lo-cur").size > 0);
  s.c.close();
  assert.equal(s.doc.querySelectorAll(".lo-cur, .lo-read").length, 0);
  assert.ok(!s.doc.documentElement.classList.contains("lo-active"));
  assert.equal(s.audio.paused, true);
  assert.equal(s.c.state().open, false);
});

test("ended stops and clears", async () => {
  const s = await setup();
  s.c.start(4);
  s.c.update();
  s.audio.dispatchEvent(new Event("ended"));
  assert.equal(s.c.state().playing, false);
  assert.equal(s.doc.querySelectorAll(".lo-cur, .lo-read").length, 0);
});

test("playbackRate survives src assignment", async () => {
  const s = await setup({ voices: [["a", 0.3], ["b", 0.5]], stored: { "lectover.speed": "1.5" } });
  s.c.start(0);
  assert.equal(s.audio.playbackRate, 1.5);
  s.c.setSpeed(2);
  s.c.setVoice("b");
  assert.equal(s.audio.playbackRate, 2);
  assert.equal(s.audio.defaultPlaybackRate, 2);
});

test("at most one rAF chain runs", async () => {
  const s = await setup();
  const pending = [];
  s.win.requestAnimationFrame = (fn) => pending.push(fn);
  s.c.start(0);
  s.c.start(3);
  s.c.toggle();
  s.c.toggle();
  assert.equal(pending.length, 1);
  pending.shift()();
  assert.equal(pending.length, 1);
});

test("pending voice switch: no flicker, second switch keeps the word", async () => {
  const s = await setup({ voices: [["a", 0.3], ["b", 0.5], ["c", 0.4]] });
  s.c.start(7);
  s.c.setVoice("b");
  s.audio.currentTime = 0;
  s.audio.dispatchEvent(new Event("timeupdate"));
  assert.equal(s.c.state().cur, 7);
  assert.deepEqual([...withClass(s, "lo-cur")], ["7"]);
  s.c.setVoice("c");
  s.audio.dispatchEvent(new Event("loadedmetadata"));
  near(s.audio.currentTime, 2.8);
  assert.equal(s.c.state().cur, 7);
  assert.equal(s.c.state().voice, "c");
});

test("audio error during a pending switch un-gates update", async () => {
  const s = await setup({ voices: [["a", 0.3], ["b", 0.5]] });
  s.c.start(0);
  s.c.setVoice("b");
  s.audio.dispatchEvent(new Event("error"));
  s.audio.currentTime = 1.0;
  s.c.update();
  assert.equal(s.c.state().cur, 2);
});

test("seek during a pending switch wins over the captured token", async () => {
  const s = await setup({ voices: [["a", 0.3], ["b", 0.5]] });
  s.c.start(7);
  s.c.setVoice("b");
  s.c.seek(2);
  s.audio.dispatchEvent(new Event("loadedmetadata"));
  near(s.audio.currentTime, 1.0);
  assert.equal(s.c.state().cur, 2);
});

test("close during a pending switch cancels the resume", async () => {
  const s = await setup({ voices: [["a", 0.3], ["b", 0.5]] });
  s.c.start(7);
  s.c.setVoice("b");
  s.c.close();
  const plays = s.audio.playCalls;
  s.audio.dispatchEvent(new Event("loadedmetadata"));
  assert.equal(s.audio.playCalls, plays);
  assert.equal(s.audio.paused, true);
});

// ---- Ogg -> MP3 error fallback

test("decode error before first play switches to mp3, keeps the word, retries play once", async () => {
  const s = await setup({ voices: [["a", 0.3], ["b", 0.5]] });
  let changes = 0;
  s.c.onChange(() => changes++);
  s.c.start(7);
  assert.equal(s.audio.playCalls, 1);
  changes = 0;
  s.audio.emitError(3);
  assert.equal(s.c.state().format, "mp3");
  assert.ok(s.audio.src.endsWith(".mp3"));
  assert.equal(s.c.state().cur, 7);
  near(s.audio.currentTime, 2.1);
  assert.equal(s.audio.playCalls, 2);
  assert.equal(s.c.state().open, true);
  assert.deepEqual(s.c.voices(), ["a", "b"]);
  assert.ok(changes > 0);
});

test("src-not-supported error switches too", async () => {
  const s = await setup();
  s.c.start(0);
  s.audio.emitError(4);
  assert.equal(s.c.state().format, "mp3");
  assert.ok(s.audio.src.endsWith(".mp3"));
});

test("network error does not switch", async () => {
  const s = await setup();
  s.c.start(0);
  s.audio.emitError(2);
  assert.equal(s.c.state().format, "ogg");
  assert.ok(s.audio.src.endsWith(".ogg"));
  assert.equal(s.audio.playCalls, 1);
});

test("error after playback started does not switch", async () => {
  const s = await setup();
  s.c.start(0);
  s.audio.emitPlaying();
  s.audio.emitError(3);
  assert.equal(s.c.state().format, "ogg");
  assert.equal(s.audio.playCalls, 1);
});

test("timeupdate during the start seek does not block the fallback", async () => {
  const s = await setup();
  s.c.start(7);
  s.audio.dispatchEvent(new Event("timeupdate"));
  assert.ok(s.audio.currentTime > 0 && !s.audio.paused);
  s.audio.emitError(3);
  assert.equal(s.c.state().format, "mp3");
});

test("close then decode error: switches format, stays closed, no play", async () => {
  const s = await setup();
  s.c.start(3);
  s.c.close();
  s.audio.emitError(3);
  assert.equal(s.c.state().format, "mp3");
  assert.equal(s.c.state().open, false);
  assert.equal(s.audio.playCalls, 1);
  assert.equal(s.audio.paused, true);
  assert.equal(s.c.state().cur, -1);
  s.c.start(3);
  assert.ok(s.audio.src.endsWith(".mp3"));
});

test("pause then decode error: switches, keeps the word, stays paused", async () => {
  const s = await setup();
  s.c.start(3);
  s.c.toggle();
  s.audio.emitError(3);
  assert.equal(s.c.state().format, "mp3");
  assert.ok(s.audio.src.endsWith(".mp3"));
  assert.equal(s.c.state().cur, 3);
  assert.equal(s.audio.playCalls, 1);
  assert.equal(s.c.state().playing, false);
});

test("no mp3 anywhere: later errors do not repeat pause and notify", async () => {
  const s = await setup({ noMp3: ["erzaehler"] });
  s.c.start(0);
  s.audio.emitError(3);
  let n = 0;
  s.c.onChange(() => n++);
  s.audio.emitError(3);
  assert.equal(n, 0);
});

test("current voice without mp3 -> manifest default, else first voice with mp3", async () => {
  const s = await setup({ voices: [["a", 0.3], ["b", 0.5], ["c", 0.4]], def: "b", stored: { "lectover.voice": "c" }, noMp3: ["c"] });
  assert.equal(s.c.state().voice, "c");
  s.c.start(0);
  s.audio.emitError(4);
  assert.equal(s.c.state().voice, "b");
  assert.deepEqual(s.c.voices(), ["a", "b"]);
  assert.ok(s.audio.src.endsWith("b.aaaa1111.mp3"));

  const t = await setup({ voices: [["a", 0.3], ["b", 0.5], ["c", 0.4]], def: "c", stored: { "lectover.voice": "c" }, noMp3: ["c", "a"] });
  t.c.start(0);
  t.audio.emitError(4);
  assert.equal(t.c.state().voice, "b");
  assert.deepEqual(t.c.voices(), ["b"]);
});

test("no voice has mp3 -> bar stays open, paused", async () => {
  const s = await setup({ voices: [["a", 0.3]], noMp3: ["a"] });
  s.c.start(3);
  s.audio.emitError(3);
  assert.equal(s.c.state().open, true);
  assert.equal(s.c.state().playing, false);
  assert.equal(s.audio.paused, true);
  assert.ok(s.doc.documentElement.classList.contains("lo-active"));
  assert.equal(s.audio.playCalls, 1);
});

test("refused retry play() -> paused state, next toggle plays", async () => {
  const s = await setup();
  s.c.start(4);
  s.audio.rejectPlay = true;
  s.audio.emitError(3);
  await Promise.resolve();
  await Promise.resolve();
  assert.equal(s.c.state().format, "mp3");
  assert.equal(s.c.state().open, true);
  assert.equal(s.c.state().playing, false);
  assert.equal(s.audio.playCalls, 2);
  s.audio.rejectPlay = false;
  s.c.toggle();
  assert.equal(s.audio.playCalls, 3);
  assert.equal(s.c.state().playing, true);
});

test("WebKit order: play() rejects NotSupportedError before the error event -> still falls back and retries", async () => {
  const s = await setup();
  s.audio.rejectPlay = "NotSupportedError";
  s.c.start(4);
  s.audio.rejectPlay = false;
  await Promise.resolve();
  await Promise.resolve();
  s.audio.emitError(4);
  assert.equal(s.c.state().format, "mp3");
  assert.equal(s.audio.playCalls, 2);
  assert.equal(s.c.state().playing, true);
});

test("AbortError rejection keeps the listener intent: a following fallback still retries play", async () => {
  const s = await setup();
  s.audio.rejectPlay = "AbortError";
  s.c.start(0);
  s.audio.rejectPlay = false;
  await Promise.resolve();
  await Promise.resolve();
  s.audio.emitError(3);
  assert.equal(s.audio.playCalls, 2);
  assert.equal(s.c.state().playing, true);
});

test("already on mp3: error does nothing more", async () => {
  const s = await setup();
  s.c.start(0);
  s.audio.emitError(3);
  assert.equal(s.audio.playCalls, 2);
  s.audio.emitError(3);
  s.audio.emitError(4);
  assert.equal(s.audio.playCalls, 2);
  assert.equal(s.c.state().format, "mp3");
});
