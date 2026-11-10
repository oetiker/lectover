import { test } from "node:test";
import assert from "node:assert/strict";
import { load, makePage, fixtureHtml, fakeTimings, fakeFetch, FakeAudio } from "./helpers.mjs";

const BASE = "https://lectover.oetiker.ch/olten.jetzt/frohburg-spiel-satz-patt/";
const SLUG = "frohburg-spiel-satz-patt";
const KEY = "lectover.pos." + SLUG;
const DAY = 24 * 3600 * 1000;

async function setup({ voices = [["erzaehler", 0.3], ["zweite", 0.2]], stored = {}, audio = new FakeAudio() } = {}) {
  const v = load();
  const page = makePage(fixtureHtml());
  const blocks = v.extractBlocks(page.content);
  const hashes = await Promise.all(blocks.map((b) => v.hashText(b.text)));
  const routes = {};
  const entries = voices.map(([id, step]) => {
    routes[`${BASE}${id}.aaaa1111.json`] = fakeTimings(blocks, hashes, { voice: id, step });
    return { id, label: id, audio: `${id}.aaaa1111.ogg`, mp3: `${id}.aaaa1111.mp3`, timings: `${id}.aaaa1111.json`, duration: 1 };
  });
  routes[BASE + "manifest.json"] = { version: 1, default: voices[0][0], voices: entries };
  page.win.fetch = fakeFetch(routes);
  page.win.console = { warn() {} };
  for (const [k, val] of Object.entries(stored)) page.win.localStorage.setItem(k, val);
  const c = await v.init(page.win, page.script, { createAudio: () => audio });
  return { v, ...page, audio, c, blocks, hashes };
}

const raw = (s) => s.win.localStorage.getItem(KEY);
const saved = (s) => JSON.parse(raw(s));
function clock(s) {
  const st = { t: 0 };
  Object.defineProperty(s.win, "performance", { value: { now: () => st.t }, configurable: true });
  return st;
}
// flat token index of (block, word)
const tok = (s, b, w) => s.blocks.slice(0, b).reduce((n, x) => n + x.tokens.length, 0) + w;

test("posKey", () => {
  assert.equal(load().posKey("a-b"), "lectover.pos.a-b");
});

test("round trip", async () => {
  const v = load();
  const s = await setup();
  v.savePos(s.win, SLUG, 1, 2, s.hashes[1], 1000);
  assert.deepEqual(JSON.parse(raw(s)), { block: 1, word: 2, hash: s.hashes[1], t: 1000 });
  assert.deepEqual(v.loadPos(s.win, SLUG, s.blocks, s.hashes, 2000), { token: tok(s, 1, 2) });
  v.clearPos(s.win, SLUG);
  assert.equal(raw(s), null);
});

test("expired after 30 days + 1 ms is dropped, exactly 30 days is kept", async () => {
  const v = load();
  const s = await setup();
  v.savePos(s.win, SLUG, 0, 1, s.hashes[0], 0);
  assert.ok(v.loadPos(s.win, SLUG, s.blocks, s.hashes, 30 * DAY));
  assert.equal(v.loadPos(s.win, SLUG, s.blocks, s.hashes, 30 * DAY + 1), null);
  assert.equal(raw(s), null);
});

test("hash mismatch, out-of-range block/word and garbage are dropped", async () => {
  const v = load();
  const s = await setup();
  const bad = [
    { block: 0, word: 0, hash: "deadbeef", t: 1 },
    { block: s.blocks.length, word: 0, hash: s.hashes[0], t: 1 },
    { block: -1, word: 0, hash: s.hashes[0], t: 1 },
    { block: 0, word: s.blocks[0].tokens.length, hash: s.hashes[0], t: 1 },
    { block: 0, word: -1, hash: s.hashes[0], t: 1 },
    { block: 0, word: 1.5, hash: s.hashes[0], t: 1 },
    { block: 0, word: 0, hash: s.hashes[0] },
    null,
    "str",
  ];
  for (const b of bad) {
    s.win.localStorage.setItem(KEY, JSON.stringify(b));
    assert.equal(v.loadPos(s.win, SLUG, s.blocks, s.hashes, 2), null, JSON.stringify(b));
    assert.equal(raw(s), null);
  }
  s.win.localStorage.setItem(KEY, "{not json");
  assert.equal(v.loadPos(s.win, SLUG, s.blocks, s.hashes, 2), null);
  assert.equal(raw(s), null);
});

test("throwing storage: no exception", () => {
  const v = load();
  const boom = () => { throw new Error("denied"); };
  const win = { localStorage: { getItem: boom, setItem: boom, removeItem: boom } };
  assert.equal(v.loadPos(win, SLUG, [{ tokens: [1] }], ["aaaaaaaa"], 1), null);
  v.savePos(win, SLUG, 0, 0, "aaaaaaaa", 1);
  v.clearPos(win, SLUG);
});

test("resumeToken reads the saved position once at init", async () => {
  const v = load();
  const probe = await setup();
  const t = { block: 1, word: 1, hash: probe.hashes[1], t: Date.now() };
  const s = await setup({ stored: { [KEY]: JSON.stringify(t) } });
  assert.equal(s.c.resumeToken(), tok(s, 1, 1));
  s.win.localStorage.removeItem(KEY);
  assert.equal(s.c.resumeToken(), tok(s, 1, 1));
  assert.equal((await setup()).c.resumeToken(), null);
  assert.ok(v);
});

test("saves on pause (toggle), close and seek", async () => {
  const s = await setup();
  s.c.start(tok(s, 1, 2));
  assert.deepEqual({ b: saved(s).block, w: saved(s).word, h: saved(s).hash }, { b: 1, w: 2, h: s.hashes[1] });
  s.c.seek(tok(s, 2, 0));
  assert.deepEqual({ b: saved(s).block, w: saved(s).word }, { b: 2, w: 0 });
  s.audio.currentTime = s.audio.currentTime + 0.4;
  s.c.update();
  s.c.toggle(); // pause
  const afterPause = saved(s);
  assert.equal(s.c.state().playing, false);
  s.win.localStorage.removeItem(KEY);
  s.c.toggle();
  s.c.seek(tok(s, 0, 3));
  s.win.localStorage.removeItem(KEY);
  s.c.close();
  assert.equal(saved(s).block, 0);
  assert.equal(saved(s).word, 3);
  assert.ok(afterPause.block === 2);
});

test("saves on the audio pause event (headset / media keys)", async () => {
  const s = await setup();
  s.c.start(tok(s, 1, 0));
  s.win.localStorage.removeItem(KEY);
  s.audio.pause();
  s.audio.dispatchEvent(new Event("pause"));
  assert.equal(saved(s).block, 1);
});

test("saves on pagehide and when the page becomes hidden", async () => {
  const s = await setup();
  s.c.start(tok(s, 1, 1));
  s.win.localStorage.removeItem(KEY);
  s.win.dispatchEvent(new s.win.Event("pagehide"));
  assert.equal(saved(s).word, 1);
  s.win.localStorage.removeItem(KEY);
  Object.defineProperty(s.doc, "visibilityState", { value: "visible", configurable: true });
  s.doc.dispatchEvent(new s.win.Event("visibilitychange"));
  assert.equal(raw(s), null);
  Object.defineProperty(s.doc, "visibilityState", { value: "hidden", configurable: true });
  s.doc.dispatchEvent(new s.win.Event("visibilitychange"));
  assert.equal(saved(s).word, 1);
});

test("nothing is saved without a current word", async () => {
  const s = await setup();
  s.win.dispatchEvent(new s.win.Event("pagehide"));
  assert.equal(raw(s), null);
  s.c.start(0);
  s.c.close();
  s.win.localStorage.removeItem(KEY);
  s.win.dispatchEvent(new s.win.Event("pagehide"));
  assert.equal(raw(s), null);
});

test("while playing it saves at most every 5 seconds", async () => {
  const s = await setup();
  const clk = clock(s);
  s.c.start(1); // seek saves word 1
  s.win.localStorage.removeItem(KEY);
  clk.t = 1000;
  s.audio.currentTime = 0.5;
  s.c.update();
  clk.t = 4900;
  s.audio.currentTime = 1.0;
  s.c.update();
  assert.equal(raw(s), null, "throttled");
  clk.t = 5100;
  s.audio.currentTime = 1.6;
  s.c.update();
  const first = saved(s);
  assert.ok(first.word >= 5);
  clk.t = 6000;
  s.audio.currentTime = 2.5;
  s.c.update();
  assert.deepEqual(saved(s), first, "still throttled");
  clk.t = 10200;
  s.c.update();
  assert.ok(saved(s).word > first.word || saved(s).block > first.block);
});

test("ended clears the position", async () => {
  const s = await setup();
  s.c.start(tok(s, 1, 0));
  assert.ok(raw(s));
  s.audio.dispatchEvent(new Event("pause"));
  s.audio.dispatchEvent(new Event("ended"));
  assert.equal(raw(s), null);
  s.win.dispatchEvent(new s.win.Event("pagehide"));
  assert.equal(raw(s), null);
});

test("a voice switch keeps the saved word", async () => {
  const s = await setup();
  s.c.start(tok(s, 1, 2));
  const before = raw(s);
  s.c.setVoice("zweite");
  s.audio.dispatchEvent(new Event("loadedmetadata"));
  s.win.dispatchEvent(new s.win.Event("pagehide"));
  const after = saved(s);
  assert.equal(after.block, 1);
  assert.equal(after.word, 2);
  assert.equal(JSON.parse(before).word, 2);
});

test("the Ogg to MP3 fallback keeps the saved word", async () => {
  const s = await setup();
  s.c.start(tok(s, 1, 2));
  s.audio.emitError(4);
  s.win.dispatchEvent(new s.win.Event("pagehide"));
  assert.equal(saved(s).block, 1);
  assert.equal(saved(s).word, 2);
});
