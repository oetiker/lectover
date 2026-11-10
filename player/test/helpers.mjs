import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { JSDOM } from "jsdom";

const require = createRequire(import.meta.url);
const FIXTURES = fileURLToPath(new URL("../../tests/fixtures/contract/", import.meta.url));

export function load() {
  return require("../lectover.js");
}

export function makePage(contentHtml, { bodyClass = "post-template", byline = true, url = "https://olten.jetzt/frohburg-spiel-satz-patt/", src = "https://lectover.oetiker.ch/js?domain=olten.jetzt" } = {}) {
  const html = `<!doctype html><html><head><title>t</title></head><body class="${bodyClass}">
<article><header>${byline ? '<div class="byline-reading-time">5 Min.</div>' : ""}</header>
<section class="gh-content">${contentHtml}</section></article>
<script src="${src}"></script></body></html>`;
  const dom = new JSDOM(html, { url });
  const win = dom.window;
  const doc = win.document;
  return { dom, win, doc, content: doc.querySelector(".gh-content"), script: doc.querySelector("script") };
}

export function fixtureHtml() {
  return readFileSync(FIXTURES + "post.html", "utf8");
}

export function fixtureExpected() {
  return JSON.parse(readFileSync(FIXTURES + "expected.json", "utf8"));
}

// Timings file for the given blocks (from extractBlocks) and their hashes.
// lead: seconds of audio before the first page word (the spoken intro and its sting).
export function fakeTimings(blocks, hashes, { voice = "erzaehler", step = 0.3, lead = 0 } = {}) {
  let t = lead;
  const round = (x) => Math.round(x * 1000) / 1000;
  const out = blocks.map((b, i) => ({
    tag: b.tag,
    hash: hashes[i],
    words: b.tokens.map(() => {
      const w = [round(t), round(t + step)];
      t += step;
      return w;
    }),
  }));
  return { version: 1, voice, duration: round(t), blocks: out };
}

// routes: { url: object | string | {status, body} }. Unknown URL -> 404. Records calls.
export function fakeFetch(routes) {
  const calls = [];
  const fn = async (url, options) => {
    calls.push({ url: String(url), options });
    const hit = routes[String(url)];
    if (hit === undefined) return { ok: false, status: 404, json: async () => ({}), text: async () => "" };
    const status = hit && typeof hit === "object" && "status" in hit ? hit.status : 200;
    const body = hit && typeof hit === "object" && "status" in hit ? hit.body : hit;
    return { ok: status >= 200 && status < 300, status, json: async () => body, text: async () => (typeof body === "string" ? body : JSON.stringify(body)) };
  };
  fn.calls = calls;
  return fn;
}

export class FakeAudio extends EventTarget {
  constructor({ ogg = true, mp3 = true } = {}) {
    super();
    this._ogg = ogg;
    this._mp3 = mp3;
    this._src = "";
    this.defaultPlaybackRate = 1;
    this.currentTime = 0;
    this.duration = NaN;
    this.playbackRate = 1;
    this.paused = true;
    this.preservesPitch = false;
    this.playCalls = 0;
    this.error = null;
    this.rejectPlay = false; // true or an error name: play() rejects like a blocked autoplay (default NotAllowedError)
  }
  // Like a failing media element: sets audio.error and fires "error".
  emitError(code) {
    this.error = { code };
    this.dispatchEvent(new Event("error"));
  }
  emitPlaying() {
    this.dispatchEvent(new Event("playing"));
  }
  // Like a real element: assigning src runs the load algorithm, which resets playbackRate.
  get src() {
    return this._src;
  }
  set src(v) {
    this._src = v;
    this.playbackRate = this.defaultPlaybackRate;
  }
  play() {
    this.playCalls++;
    if (this.rejectPlay) {
      this.paused = true;
      const err = new Error("play rejected");
      err.name = typeof this.rejectPlay === "string" ? this.rejectPlay : "NotAllowedError";
      return Promise.reject(err);
    }
    this.paused = false;
    return Promise.resolve();
  }
  pause() {
    this.paused = true;
  }
  canPlayType(type) {
    if (/ogg/.test(type)) return this._ogg ? "probably" : "";
    if (/mpeg/.test(type)) return this._mp3 ? "probably" : "";
    return "";
  }
}
