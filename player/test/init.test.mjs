import { test } from "node:test";
import assert from "node:assert/strict";
import { load, makePage, fixtureHtml, fakeTimings, fakeFetch, FakeAudio } from "./helpers.mjs";

const BASE = "https://lectover.oetiker.ch/olten.jetzt/frohburg-spiel-satz-patt/";

async function setup({ html = fixtureHtml(), extra = {}, voices = ["erzaehler"], noMp3 = [], pageOpts, scriptAttrs = {}, mutate } = {}) {
  const v = load();
  const page = makePage(html, pageOpts);
  const blocks = v.extractBlocks(page.content);
  const hashes = await Promise.all(blocks.map((b) => v.hashText(b.text)));
  const routes = {};
  const entries = voices.map((id) => {
    routes[`${BASE}${id}.aaaa1111.json`] = fakeTimings(blocks, hashes, { voice: id });
    return { id, label: id, audio: `${id}.aaaa1111.ogg`, ...(noMp3.includes(id) ? {} : { mp3: `${id}.aaaa1111.mp3` }), timings: `${id}.aaaa1111.json`, duration: 1 };
  });
  routes[BASE + "manifest.json"] = { version: 1, slug: "frohburg-spiel-satz-patt", default: voices[0], voices: entries };
  Object.assign(routes, extra);
  if (mutate) mutate(page, routes);
  for (const [k, val] of Object.entries(scriptAttrs)) page.script.setAttribute(k, val);
  const fetch = fakeFetch(routes);
  page.win.fetch = fetch;
  const warns = [];
  page.win.console = { warn: (...a) => warns.push(a.join(" ")) };
  return { v, ...page, fetch, warns, blocks, hashes, routes };
}

const run = (s, audio = new FakeAudio()) => s.v.init(s.win, s.script, { createAudio: () => audio });

test("no post-template body: no fetch, null", async () => {
  const s = await setup({ pageOpts: { bodyClass: "home-template" } });
  const before = s.doc.documentElement.outerHTML;
  assert.equal(await run(s), null);
  assert.equal(s.fetch.calls.length, 0);
  assert.equal(s.doc.documentElement.outerHTML, before);
});

test("neither supported: no fetch, null", async () => {
  const s = await setup();
  const before = s.doc.documentElement.outerHTML;
  assert.equal(await run(s, new FakeAudio({ ogg: false, mp3: false })), null);
  assert.equal(s.fetch.calls.length, 0);
  assert.equal(s.doc.documentElement.outerHTML, before);
});

test("ogg supported: plays .ogg", async () => {
  const s = await setup();
  const c = await run(s);
  assert.equal(c.state().format, "ogg");
  c.start(0);
  assert.ok(c.audio.src.endsWith(".ogg"));
});

test("ogg unsupported, mp3 supported: plays .mp3", async () => {
  const s = await setup();
  const c = await run(s, new FakeAudio({ ogg: false }));
  assert.equal(c.state().format, "mp3");
  c.start(0);
  assert.ok(c.audio.src.endsWith(".mp3"));
});

test("data-format=mp3 forces mp3 on an ogg browser", async () => {
  const s = await setup({ scriptAttrs: { "data-format": "mp3" } });
  const c = await run(s, new FakeAudio());
  assert.equal(c.state().format, "mp3");
  c.start(0);
  assert.ok(c.audio.src.endsWith(".mp3"));
});

test("mp3 path skips entries without mp3; voice list has only mp3 voices", async () => {
  const s = await setup({ voices: ["a", "b"], noMp3: ["a"] });
  const c = await run(s, new FakeAudio({ ogg: false }));
  assert.deepEqual(c.voices(), ["b"]);
  assert.ok(s.warns.some((w) => w.includes('voice "a" has no mp3; re-render the audio')));
  assert.equal(c.state().voice, "b");
  c.start(0);
  assert.ok(c.audio.src.endsWith("b.aaaa1111.mp3"));
});

test("mp3 path: stored voice without mp3 falls back to a voice with mp3", async () => {
  const s = await setup({ voices: ["a", "b"], noMp3: ["a"] });
  s.win.localStorage.setItem("lectover.voice", "a");
  const c = await run(s, new FakeAudio({ ogg: false }));
  assert.equal(c.state().voice, "b");
  c.start(0);
  assert.ok(c.audio.src.endsWith("b.aaaa1111.mp3"));
});

test("mp3 path, old manifest without mp3 keys: null, warning, no .mp3 fetched", async () => {
  const s = await setup({ noMp3: ["erzaehler"] });
  const before = s.doc.documentElement.outerHTML;
  assert.equal(await run(s, new FakeAudio({ ogg: false })), null);
  assert.ok(s.warns.some((w) => w.includes('voice "erzaehler" has no mp3')));
  assert.ok(!s.fetch.calls.some((c) => c.url.endsWith(".mp3")));
  assert.equal(s.doc.documentElement.outerHTML, before);
});

test("manifest 404: null, page unchanged", async () => {
  const s = await setup({ mutate: (p, r) => delete r[BASE + "manifest.json"] });
  const before = s.doc.documentElement.outerHTML;
  assert.equal(await run(s), null);
  assert.equal(s.doc.documentElement.outerHTML, before);
});

test("manifest bad JSON: null, page unchanged", async () => {
  const s = await setup();
  s.win.fetch = async () => ({ ok: true, status: 200, json: async () => { throw new SyntaxError("bad"); } });
  const before = s.doc.documentElement.outerHTML;
  assert.equal(await run(s), null);
  assert.equal(s.doc.documentElement.outerHTML, before);
});

test("crypto failure is swallowed: null, page unchanged", async () => {
  const s = await setup();
  const before = s.doc.documentElement.outerHTML;
  const orig = Object.getOwnPropertyDescriptor(globalThis, "crypto");
  Object.defineProperty(globalThis, "crypto", { value: { subtle: { digest: () => Promise.reject(new Error("insecure")) } }, configurable: true });
  try {
    assert.equal(await run(s), null);
  } finally {
    Object.defineProperty(globalThis, "crypto", orig);
  }
  assert.equal(s.doc.documentElement.outerHTML, before);
});

test("manifest fetched with cache no-cache from <script dir>/<domain>/", async () => {
  const s = await setup();
  await run(s);
  const call = s.fetch.calls[0];
  assert.equal(call.url, BASE + "manifest.json");
  assert.deepEqual(call.options, { cache: "no-cache" });
});

test("timings 404 for the only voice: null", async () => {
  const s = await setup({ mutate: (p, r) => delete r[BASE + "erzaehler.aaaa1111.json"] });
  const before = s.doc.documentElement.outerHTML;
  assert.equal(await run(s), null);
  assert.equal(s.doc.documentElement.outerHTML, before);
});

test("edited block hides player", async () => {
  const s = await setup();
  const el = s.blocks[2].el;
  const walker = s.doc.createTreeWalker(el, 4);
  const node = walker.nextNode();
  node.nodeValue = node.nodeValue.replace(/\S+/, "Geändert");
  const before = s.doc.documentElement.outerHTML;
  assert.equal(await run(s), null);
  assert.equal(s.doc.documentElement.outerHTML, before);
  assert.equal(s.warns.length, 1);
  assert.match(s.warns[0], /block 2/);
});

test("extra block hides player", async () => {
  const s = await setup();
  s.content.appendChild(s.doc.createElement("p")).textContent = "Neu dazu";
  const before = s.doc.documentElement.outerHTML;
  assert.equal(await run(s), null);
  assert.equal(s.doc.documentElement.outerHTML, before);
});

test("stale second voice is not offered", async () => {
  const s = await setup({
    voices: ["erzaehler", "zweite"],
    mutate: (p, r) => {
      const t = r[BASE + "zweite.aaaa1111.json"];
      t.blocks[1].hash = "deadbeef";
    },
  });
  const c = await run(s);
  assert.deepEqual(c.voices(), ["erzaehler"]);
  assert.equal(s.warns.length, 1);
  assert.match(s.warns[0], /voice "zweite"/);
});

test("matching audio wraps tokens without changing text", async () => {
  const s = await setup();
  const before = s.content.textContent;
  const total = s.blocks.reduce((n, b) => n + b.tokens.length, 0);
  assert.equal(total, 47);
  const c = await run(s);
  assert.ok(c);
  assert.equal(s.content.textContent, before);
  const ids = new Set([...s.content.querySelectorAll("span.lo-w")].map((x) => x.dataset.lo));
  assert.equal(ids.size, total);
  const two = [...s.content.querySelectorAll('span[data-lo="2"]')];
  assert.equal(two.map((x) => x.textContent).join(""), "Klubspielerinnen");
  assert.equal(two.length, 2);
  for (const a of s.content.querySelectorAll("a")) assert.ok(a.textContent.trim().length > 0);
});

test("wrapped token count mismatch: null, page restored, warning", async () => {
  const s = await setup();
  const before = s.doc.documentElement.outerHTML;
  const orig = s.win.fetch;
  s.win.fetch = async (url, o) => {
    const r = await orig(url, o);
    if (String(url).endsWith(".json") && !String(url).endsWith("manifest.json")) {
      // DOM drifts after extraction but before wrapping: one more word than verified
      const p = s.content.querySelector("p");
      p.appendChild(s.doc.createTextNode(" extra"));
    }
    return r;
  };
  assert.equal(await run(s), null);
  assert.equal(s.content.querySelectorAll("span.lo-w").length, 0);
  assert.equal(s.doc.querySelector(".lo-bar, .lo-root"), null);
  assert.ok(s.warns.some((w) => /^lectover: /.test(w)));
  assert.ok(!s.doc.documentElement.outerHTML.includes("lo-w"));
  assert.notEqual(before, "");
});

test("wrapTokens returns next index and leaves br and whitespace alone", async () => {
  const v = load();
  const { doc } = makePage("<p>a<br>b <em>c</em>d  e</p>");
  const [b] = v.extractBlocks(doc.querySelector(".gh-content"));
  assert.equal(b.tokens.length, 4);
  const next = v.wrapTokens(b, 10);
  assert.equal(next, 14);
  assert.equal(b.el.innerHTML, '<span class="lo-w" data-lo="10">a</span><br><span class="lo-w" data-lo="11">b</span> <em><span class="lo-w" data-lo="12">c</span></em><span class="lo-w" data-lo="12">d</span>  <span class="lo-w" data-lo="13">e</span>');
});

test("data-base and data-content attributes are honoured", async () => {
  const other = "https://x.example/a/frohburg-spiel-satz-patt/";
  const s = await setup({
    html: '<div class="alt">' + fixtureHtml() + "</div><p>ausserhalb</p>",
    scriptAttrs: { "data-base": "https://x.example/a", "data-content": ".alt" },
  });
  const v = s.v;
  const blocks = v.extractBlocks(s.doc.querySelector(".alt"));
  const hashes = await Promise.all(blocks.map((b) => v.hashText(b.text)));
  const routes = {
    [other + "manifest.json"]: { voices: [{ id: "erzaehler", timings: "t.json" }] },
    [other + "t.json"]: fakeTimings(blocks, hashes),
  };
  s.win.fetch = fakeFetch(routes);
  const c = await run(s);
  assert.deepEqual(c.voices(), ["erzaehler"]);
  assert.equal(s.doc.querySelectorAll(".alt span.lo-w").length > 0, true);
  assert.equal(s.doc.querySelectorAll("p + p span.lo-w, .gh-content > p:last-child span.lo-w").length, 0);
});

test("without ?domain= the page's own hostname picks the folder", async () => {
  const s = await setup({ pageOpts: { src: "https://lectover.oetiker.ch/js" } });
  assert.ok(await run(s));
  assert.equal(s.fetch.calls[0].url, BASE + "manifest.json");
});

test("www. on either side still matches", async () => {
  const s = await setup({ pageOpts: { url: "https://www.olten.jetzt/frohburg-spiel-satz-patt/" } });
  assert.ok(await run(s));
  assert.equal(s.fetch.calls[0].url, BASE + "manifest.json");
});

test("domain= for another site: null, nothing fetched, warning", async () => {
  const s = await setup({ pageOpts: { url: "https://evil.example/frohburg-spiel-satz-patt/" } });
  const before = s.doc.documentElement.outerHTML;
  assert.equal(await run(s), null);
  assert.equal(s.fetch.calls.length, 0);
  assert.equal(s.doc.documentElement.outerHTML, before);
  assert.match(s.warns[0], /^lectover: domain "olten.jetzt" does not match this page \(evil.example\)/);
});

test("localhost may play any domain (dev page)", async () => {
  const s = await setup({ pageOpts: { url: "http://localhost:8000/olten.jetzt/frohburg-spiel-satz-patt/", src: "http://localhost:8000/js?domain=olten.jetzt" } });
  await run(s);
  assert.equal(s.warns.length, 0);
  assert.equal(s.fetch.calls[0].url, "http://localhost:8000/olten.jetzt/frohburg-spiel-satz-patt/manifest.json");
});

const UA = {
  iphone: "Mozilla/5.0 (iPhone; CPU iPhone OS 26_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Mobile/15E148 Safari/604.1",
  iphoneChrome: "Mozilla/5.0 (iPhone; CPU iPhone OS 26_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/140.0 Mobile/15E148 Safari/604.1",
  macSafari: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.6 Safari/605.1.15",
  macChrome: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36",
  macFirefox: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:159.0) Gecko/20100101 Firefox/159.0",
};

test("prefersMp3: WebKit (Safari, any iPhone/iPad browser) yes; Chrome, Firefox no", () => {
  const v = load();
  assert.equal(v.prefersMp3({ userAgent: UA.iphone }), true);
  assert.equal(v.prefersMp3({ userAgent: UA.iphoneChrome }), true);
  assert.equal(v.prefersMp3({ userAgent: UA.macSafari, maxTouchPoints: 0 }), true);
  assert.equal(v.prefersMp3({ userAgent: UA.macSafari, maxTouchPoints: 5 }), true);
  assert.equal(v.prefersMp3({ userAgent: UA.macChrome, maxTouchPoints: 0 }), false);
  assert.equal(v.prefersMp3({ userAgent: UA.macFirefox, maxTouchPoints: 0 }), false);
});

function withUA(ua) {
  return (page) => Object.defineProperty(page.win.navigator, "userAgent", { value: ua, configurable: true });
}

test("Safari that can play Ogg still plays the MP3", async () => {
  const s = await setup({ mutate: withUA(UA.macSafari) });
  const c = await run(s, new FakeAudio());
  assert.equal(c.state().format, "mp3");
  c.start(0);
  assert.ok(c.audio.src.endsWith(".mp3"));
});

test("Safari with an old render without MP3 falls back to the Ogg", async () => {
  const s = await setup({ voices: ["a"], noMp3: ["a"], mutate: withUA(UA.iphone) });
  const c = await run(s, new FakeAudio());
  assert.equal(c.state().format, "ogg");
  c.start(0);
  assert.ok(c.audio.src.endsWith(".ogg"));
});

test("data-format=ogg forces the Ogg on Safari", async () => {
  const s = await setup({ scriptAttrs: { "data-format": "ogg" }, mutate: withUA(UA.macSafari) });
  const c = await run(s, new FakeAudio());
  assert.equal(c.state().format, "ogg");
});

test("Firefox keeps the Ogg", async () => {
  const s = await setup({ mutate: withUA(UA.macFirefox) });
  const c = await run(s, new FakeAudio());
  assert.equal(c.state().format, "ogg");
});
