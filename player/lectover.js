/* lectover.js - read-aloud player for Ghost posts (no dependencies, no build step). */
(function () {
  "use strict";

  // Text contract shared with ghost_text.py. Never use \s: engines disagree on it.
  var SPACE_RE = /[ \t\n\r\f\v  -   　]+/g;
  var BLOCK_TAGS = { p: 1, li: 1, blockquote: 1, h1: 1, h2: 1, h3: 1, h4: 1, h5: 1, h6: 1 };
  var SKIPPED_TAGS = { figure: 1, figcaption: 1, pre: 1, table: 1, script: 1, style: 1, template: 1 };

  function collapse(s) {
    return s.replace(SPACE_RE, " ").replace(/^ | $/g, "");
  }

  function hasClass(el, name) {
    return el.classList.contains(name);
  }

  function isBlock(el) {
    return BLOCK_TAGS[el.localName] === 1 || hasClass(el, "kg-callout-text");
  }

  function isSkipped(el) {
    return SKIPPED_TAGS[el.localName] === 1 || (hasClass(el, "kg-card") && !hasClass(el, "kg-callout-card"));
  }

  // Text of el without text inside nested blocks / skipped elements; <br> counts as a space.
  function ownText(el) {
    var out = "";
    for (var n = el.firstChild; n; n = n.nextSibling) {
      if (n.nodeType === 3) {
        out += n.nodeValue;
      } else if (n.nodeType === 1) {
        if (n.localName === "br") out += " ";
        else if (!isBlock(n) && !isSkipped(n)) out += ownText(n);
      }
    }
    return out;
  }

  function extractBlocks(root) {
    var blocks = [];
    (function walk(el) {
      for (var c = el.firstElementChild; c; c = c.nextElementSibling) {
        if (isSkipped(c)) continue;
        if (isBlock(c)) {
          var text = collapse(ownText(c));
          if (text) blocks.push({ el: c, tag: c.localName, text: text, tokens: text.split(" ") });
        }
        walk(c);
      }
    })(root);
    return blocks;
  }

  // First 8 hex of SHA-1 over the UTF-8 bytes of the NFC-normalised text.
  function hashText(text) {
    var bytes = new TextEncoder().encode(text.normalize("NFC"));
    return crypto.subtle.digest("SHA-1", bytes).then(function (buf) {
      var hex = "";
      new Uint8Array(buf).forEach(function (b) { hex += (b < 16 ? "0" : "") + b.toString(16); });
      return hex.slice(0, 8);
    });
  }

  var NONSPACE_RE = new RegExp("[^" + SPACE_RE.source.slice(1, -2) + "]+", "g");

  function slugFromPath(pathname) {
    var parts = String(pathname).split("/").filter(function (p) { return p !== ""; });
    if (!parts.length) return null;
    try {
      return decodeURIComponent(parts[parts.length - 1]);
    } catch (e) {
      return null;
    }
  }

  // blocks: [{tag, tokens}], hashes: string[] (page), timings: parsed timings file.
  function verifyTimings(blocks, hashes, timings) {
    var tb = timings && Array.isArray(timings.blocks) ? timings.blocks : null;
    if (!tb) return { ok: false, index: 0, reason: "no blocks in timings" };
    if (tb.length !== blocks.length) {
      return { ok: false, index: Math.min(tb.length, blocks.length), reason: "block count " + tb.length + " != " + blocks.length };
    }
    for (var i = 0; i < blocks.length; i++) {
      if (!tb[i] || tb[i].hash !== hashes[i]) return { ok: false, index: i, reason: "text changed" };
      if (!Array.isArray(tb[i].words) || tb[i].words.length !== blocks[i].tokens.length) {
        return { ok: false, index: i, reason: "word count mismatch" };
      }
    }
    return { ok: true };
  }

  // Wraps every token piece of block.el in <span class="lo-w" data-lo="N"> (N counts from firstIndex).
  // Stores block.spans (one array of spans per token) and returns the next free index.
  function wrapTokens(block, firstIndex) {
    var doc = block.el.ownerDocument;
    var next = firstIndex;
    var open = false; // previous piece ended exactly at the end of its text node
    var spans = [];
    (function walk(el) {
      for (var n = el.firstChild; n; ) {
        var following = n.nextSibling;
        if (n.nodeType === 3) {
          var text = n.nodeValue;
          var re = new RegExp(NONSPACE_RE.source, "g");
          var m, cur = n, consumed = 0;
          var pieces = [];
          while ((m = re.exec(text))) pieces.push([m.index, m.index + m[0].length]);
          if (!pieces.length) {
            if (text.length) open = false;
          }
          for (var k = 0; k < pieces.length; k++) {
            var s = pieces[k][0], e = pieces[k][1];
            if (!(k === 0 && s === 0 && open)) { next++; spans.push([]); }
            if (s - consumed > 0) cur = cur.splitText(s - consumed);
            var rest = e - s < cur.nodeValue.length ? cur.splitText(e - s) : null;
            var span = doc.createElement("span");
            span.className = "lo-w";
            span.setAttribute("data-lo", String(next - 1));
            cur.parentNode.insertBefore(span, cur);
            span.appendChild(cur);
            spans[spans.length - 1].push(span);
            cur = rest;
            consumed = e;
            open = e === text.length;
          }
        } else if (n.nodeType === 1) {
          if (n.localName === "br") open = false;
          else if (!isBlock(n) && !isSkipped(n)) walk(n);
        }
        n = following;
      }
    })(block.el);
    block.spans = spans;
    return next;
  }

  function scriptDir(script) {
    var src = script && script.src;
    return src ? new URL(".", src).href : "";
  }

  var DEV_HOSTS = ["localhost", "127.0.0.1"];

  // Audio folder for this site: <script dir>/<domain>/, domain from ?domain= or the page host.
  // Another site's domain hides the player (except on a local dev host).
  function siteBase(win, script) {
    var dir = scriptDir(script);
    if (!dir) return "";
    var bare = function (h) { return String(h).toLowerCase().replace(/^www\./, ""); };
    var host = bare(win.location.hostname);
    var domain = bare(new URL(script.src).searchParams.get("domain") || host);
    if (domain !== host && DEV_HOSTS.indexOf(host) < 0) {
      win.console.warn('lectover: domain "' + domain + '" does not match this page (' + host + "); player disabled");
      return "";
    }
    return dir + encodeURIComponent(domain) + "/";
  }


  var SPEEDS = [0.8, 0.9, 1, 1.1, 1.25, 1.5, 2];
  var INTERACTIVE = "button, input, select, textarea, a, [contenteditable]";

  // Last index i with starts[i] <= t; -1 when t is before the first token.
  function findToken(starts, t) {
    var lo = 0, hi = starts.length - 1, ans = -1;
    while (lo <= hi) {
      var mid = (lo + hi) >> 1;
      if (starts[mid] <= t) { ans = mid; lo = mid + 1; } else hi = mid - 1;
    }
    return ans;
  }

  function storeGet(win, key) {
    try { return win.localStorage.getItem(key); } catch (e) { return null; }
  }

  function storeSet(win, key, value) {
    try { win.localStorage.setItem(key, String(value)); } catch (e) { /* storage unavailable */ }
  }

  function storeRemove(win, key) {
    try { win.localStorage.removeItem(key); } catch (e) { /* storage unavailable */ }
  }

  // ---- Resume position: one word per article, kept on this device only.
  var POS_MAX_AGE = 30 * 24 * 3600 * 1000;

  function posKey(slug) { return "lectover.pos." + slug; }

  function savePos(win, slug, block, word, hash, now) {
    storeSet(win, posKey(slug), JSON.stringify({ block: block, word: word, hash: hash, t: now }));
  }

  function clearPos(win, slug) { storeRemove(win, posKey(slug)); }

  // Returns {token} (flat token index) for a still-valid saved position; drops anything else.
  function loadPos(win, slug, blocks, hashes, now) {
    var key = posKey(slug), raw = storeGet(win, key);
    if (raw === null) return null;
    var p = null;
    try { p = JSON.parse(raw); } catch (e) { /* garbage */ }
    var ok = p && typeof p === "object" && isInt(p.block) && isInt(p.word) && typeof p.t === "number" &&
      p.block >= 0 && p.block < blocks.length && p.word >= 0 && p.word < blocks[p.block].tokens.length &&
      p.hash === hashes[p.block] && now - p.t <= POS_MAX_AGE;
    if (!ok) { storeRemove(win, key); return null; }
    var token = p.word;
    for (var i = 0; i < p.block; i++) token += blocks[i].tokens.length;
    return { token: token };
  }

  function isInt(n) { return typeof n === "number" && isFinite(n) && Math.floor(n) === n; }

  function createController(win, ctx) {
    var doc = win.document;
    var audio = ctx.audio, blocks = ctx.blocks, valid = ctx.valid, dir = ctx.base, format = ctx.format;
    var allEntries = ctx.entries || valid;
    var root = doc.documentElement;

    // Flat token tables: first token per block, owning block per token, spans per token.
    var blockFirst = [], tokenBlock = [], tokenSpans = [];
    blocks.forEach(function (b, bi) {
      blockFirst.push(tokenSpans.length);
      b.spans.forEach(function (sp) { tokenSpans.push(sp); tokenBlock.push(bi); });
    });
    var total = tokenSpans.length;

    // Per voice: start/end time of every token, flattened over blocks. Built for every verified
    // entry so a switch to MP3 can use voices that were hidden while on Ogg.
    var tables = {};
    allEntries.forEach(function (v) {
      var starts = [], ends = [];
      v.timings.blocks.forEach(function (b) {
        b.words.forEach(function (w) { starts.push(w[0]); ends.push(w[1]); });
      });
      tables[v.entry.id] = { entry: v.entry, starts: starts, ends: ends, duration: v.timings.duration };
    });
    var ids = valid.map(function (v) { return v.entry.id; });

    var voice = storeGet(win, "lectover.voice");
    if (ids.indexOf(voice) < 0) {
      var def0 = ctx.manifest && ctx.manifest["default"];
      voice = ids.indexOf(def0) >= 0 ? def0 : ids[0];
    }
    var speed = parseFloat(storeGet(win, "lectover.speed"));
    if (SPEEDS.indexOf(speed) < 0) speed = 1;
    audio.preload = "metadata";
    audio.defaultPlaybackRate = speed;
    audio.playbackRate = speed;
    audio.preservesPitch = true;
    // Volume (desktop slider; iOS ignores audio.volume). Stored like the speed.
    var volume = parseFloat(storeGet(win, "lectover.volume"));
    if (!(volume >= 0 && volume <= 1)) volume = 1;
    audio.volume = volume;

    var open = false, cur = -1, srcVoice = null, listeners = [];
    var played = false; // a "playing" event was seen this visit (blocks the Ogg -> MP3 fallback)
    var wantPlay = false; // the listener wants audio: set by play(), cleared by pause, close, ended
    var fallbackTried = false;
    var looping = false;
    var switching = null; // {token, resume} while a voice switch waits for loadedmetadata

    var slug = ctx.slug, hashes = ctx.hashes || [];
    var lastSave = -Infinity;
    var resume = slug ? loadPos(win, slug, blocks, hashes, Date.now()) : null;
    var resumeAt = resume ? resume.token : null; // live "continue here" token; null after the end

    // Remember the current word (needs one >= 0). Without a slug nothing is stored.
    function savePosition() {
      if (cur >= 0 && cur < total) resumeAt = cur;
      if (!slug || cur < 0 || cur >= total) return;
      var b = tokenBlock[cur];
      savePos(win, slug, b, cur - blockFirst[b], hashes[b], Date.now());
      lastSave = win.performance.now();
    }

    function notify() {
      listeners.forEach(function (fn) { fn(); });
    }

    function mark(index, prev) {
      var lo = prev < 0 || index < 0 ? 0 : Math.min(prev, index);
      var hi = Math.max(prev, index);
      for (var i = lo; i <= hi && i < total; i++) {
        var cls = i < index ? "lo-read" : i === index ? "lo-cur" : "";
        tokenSpans[i].forEach(function (sp) {
          sp.classList.toggle("lo-read", cls === "lo-read");
          sp.classList.toggle("lo-cur", cls === "lo-cur");
        });
      }
    }

    function setCur(index) {
      if (index === cur) return false;
      var prev = cur;
      cur = index;
      mark(index, prev);
      return true;
    }

    function audioUrl(entry) { return dir + (format === "mp3" ? entry.mp3 : entry.audio); }

    function ensureSrc() {
      if (srcVoice !== voice) {
        audio.src = audioUrl(tables[voice].entry);
        srcVoice = voice;
        // The media load algorithm resets playbackRate to defaultPlaybackRate.
        audio.playbackRate = speed;
      }
    }

    function playing() { return open && !audio.paused; }

    function update() {
      if (!open || switching) return;
      if (setCur(findToken(tables[voice].starts, audio.currentTime))) notify();
      if (playing() && win.performance.now() - lastSave >= 5000) savePosition();
    }

    function loop() {
      if (!win.requestAnimationFrame || looping) return;
      looping = true;
      var tick = function () {
        if (!playing()) { looping = false; return; }
        update();
        win.requestAnimationFrame(tick);
      };
      win.requestAnimationFrame(tick);
    }

    var playGen = 0; // identifies the latest play() call; rejections of superseded calls are ignored
    function play() {
      wantPlay = true;
      var p = audio.play();
      var g = ++playGen;
      // Only a blocked autoplay means "listener not playing". Other rejections (WebKit's
      // NotSupportedError before the error event, AbortError from a src swap) are handled elsewhere.
      if (p && p.catch) p.catch(function (e) {
        if (g === playGen && (!e || e.name === "NotAllowedError")) wantPlay = false;
        notify();
      });
      loop();
    }

    function clamp(i) { return Math.max(0, Math.min(total - 1, i)); }

    function seek(i) {
      ensureSrc();
      if (switching) switching.token = clamp(i);
      audio.currentTime = tables[voice].starts[clamp(i)];
      setCur(clamp(i));
      savePosition();
      notify();
    }

    // To the very start of the audio: the spoken intro and its sting come before the first
    // page word, so token 0's start would skip them.
    function rewind() {
      ensureSrc();
      if (switching) switching.token = 0;
      audio.currentTime = 0;
      setCur(-1);
      resumeAt = 0; // from the top: like a saved word 0, no "continue here"
      if (slug) clearPos(win, slug);
      notify();
    }

    // i: the token to start at; 0 or none starts at the top of the audio.
    function start(i) {
      open = true;
      root.classList.add("lo-active");
      ensureSrc();
      if (i > 0) seek(i); else rewind();
      play();
      notify();
    }

    function toggle() {
      if (!open) return start(0);
      if (!audio.paused) {
        wantPlay = false;
        audio.pause();
        savePosition();
        notify();
      } else {
        if (cur < 0 && audio.currentTime >= tables[voice].duration - 0.05) rewind();
        play();
        notify();
      }
    }

    function close() {
      wantPlay = false;
      audio.pause();
      savePosition();
      switching = null;
      open = false;
      setCur(-1);
      root.classList.remove("lo-active");
      notify();
    }

    function curOrZero() { return cur < 0 ? 0 : cur; }

    function blockStart(i) { return blockFirst[tokenBlock[i]]; }

    doc.addEventListener("keydown", function (e) {
      if (!open || (e.key !== " " && e.key !== "Spacebar")) return;
      if (e.repeat || e.shiftKey || e.ctrlKey || e.metaKey || e.altKey) return;
      var t = e.target;
      if (t && t.closest && t.closest(INTERACTIVE)) return;
      e.preventDefault();
      toggle();
    });

    ctx.content.addEventListener("click", function (e) {
      if (!open || !e.target || !e.target.closest) return;
      if (e.target.closest("a")) return;
      var sel = win.getSelection ? win.getSelection() : null;
      if (sel && !sel.isCollapsed) return;
      var w = e.target.closest(".lo-w");
      if (!w) return;
      seek(parseInt(w.getAttribute("data-lo"), 10));
      play();
    });

    audio.addEventListener("timeupdate", update);
    audio.addEventListener("playing", function () { played = true; });

    // Some WebKit versions claim Ogg/Opus support and then fail to decode it: recover with MP3.
    function fallbackToMp3() {
      fallbackTried = true;
      var withMp3 = allEntries.filter(function (v) { return v.entry.mp3; });
      if (!withMp3.length) {
        wantPlay = false;
        audio.pause();
        notify();
        return;
      }
      var token = curOrZero();
      format = "mp3";
      ids = withMp3.map(function (v) { return v.entry.id; });
      if (ids.indexOf(voice) < 0) {
        var def = ctx.manifest && ctx.manifest["default"];
        voice = ids.indexOf(def) >= 0 ? def : ids[0];
      }
      srcVoice = null;
      if (open) {
        seek(token); // loads the MP3 and keeps the word
        if (wantPlay) play(); else audio.pause();
      }
      notify();
    }

    audio.addEventListener("error", function () {
      switching = null;
      var code = audio.error && audio.error.code;
      if (format !== "ogg" || played || fallbackTried || (code !== 3 && code !== 4)) return;
      fallbackToMp3();
    });
    audio.addEventListener("loadedmetadata", function () {
      if (!switching) return;
      var sw = switching;
      switching = null;
      audio.currentTime = tables[voice].starts[sw.token];
      update();
      if (sw.resume) play();
      notify();
    });
    // Pauses that do not come through toggle(): headset buttons, media keys, system interruptions.
    audio.addEventListener("pause", function () { if (open && !switching) savePosition(); });
    win.addEventListener("pagehide", savePosition);
    doc.addEventListener("visibilitychange", function () {
      if (doc.visibilityState === "hidden") savePosition();
    });
    audio.addEventListener("ended", function () {
      wantPlay = false;
      audio.pause();
      setCur(-1);
      resumeAt = null;
      if (slug) clearPos(win, slug);
      notify();
    });

    return {
      audio: audio,
      base: ctx.base,
      manifest: ctx.manifest,
      blocks: blocks,
      content: ctx.content,
      script: ctx.script,
      valid: valid,
      get format() { return format; },
      entries: ctx.entries,
      voices: function () { return ids.slice(); },
      resumeToken: function () { return resume ? resume.token : null; },
      // Token to continue from (live, this visit included); token 0 counts as no position.
      resumePoint: function () { return resumeAt > 0 ? resumeAt : null; },
      timeOf: function (token) { return tables[voice].starts[clamp(token)]; },
      start: start,
      toggle: toggle,
      seek: seek,
      seekFraction: function (f) {
        var d = isFinite(audio.duration) && audio.duration > 0 ? audio.duration : tables[voice].duration;
        ensureSrc();
        audio.currentTime = Math.max(0, Math.min(1, f)) * d;
        update();
        notify();
      },
      skipWords: function (n) { seek(curOrZero() + n); },
      prevBlock: function () {
        var i = curOrZero(), bs = blockStart(i);
        seek(i > bs + 3 ? bs : bs > 0 ? blockStart(bs - 1) : 0);
      },
      nextBlock: function () {
        var b = tokenBlock[curOrZero()] + 1;
        if (b < blockFirst.length) seek(blockFirst[b]);
      },
      setSpeed: function (rate) {
        speed = rate;
        audio.defaultPlaybackRate = rate;
        audio.playbackRate = rate;
        audio.preservesPitch = true;
        storeSet(win, "lectover.speed", rate);
        notify();
      },
      setVolume: function (v) {
        v = Math.max(0, Math.min(1, v));
        volume = v;
        audio.volume = v;
        storeSet(win, "lectover.volume", v);
        notify();
      },
      setVoice: function (id) {
        if (!tables[id] || id === voice) return;
        var loaded = srcVoice !== null;
        if (loaded && !switching) switching = { token: curOrZero(), resume: playing() };
        voice = id;
        storeSet(win, "lectover.voice", id);
        if (loaded) ensureSrc();
        notify();
      },
      close: close,
      update: update,
      duration: function () { return tables[voice].duration; },
      onChange: function (fn) { listeners.push(fn); },
      state: function () {
        return { open: open, playing: playing(), cur: cur, voice: voice, speed: speed, volume: volume, format: format };
      }
    };
  }

  // ---------------------------------------------------------------- UI

  var SVG_SPK = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M11 5L6 9H2v6h4l5 4V5z"/><path d="M15.5 8.5a5 5 0 0 1 0 7"/><path d="M19 5a10 10 0 0 1 0 14"/></svg>';
  var SVG_PLAY = '<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M8 5v14l11-7z"/></svg>';
  var SVG_PAUSE = '<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M6 5h4v14H6zM14 5h4v14h-4z"/></svg>';
  var SVG_PREV = '<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M6 5h2v14H6zM20 5v14L9 12z"/></svg>';
  var SVG_NEXT = '<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M16 5h2v14h-2zM4 5v14l11-7z"/></svg>';
  var SVG_CLOSE = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18"/></svg>';

  var CSS = [
    ".lo-w{border-radius:3px}",
    ".lo-active .lo-w{cursor:pointer;transition:background-color .12s,color .12s,opacity .25s,box-shadow .12s}",
    ".lo-active .lo-w:hover{box-shadow:inset 0 -2px 0 rgba(59,130,246,.35)}",
    ".lo-active .lo-w.lo-read{opacity:.45}",
    ".lo-active .lo-w.lo-cur{background:#ffe68a;box-shadow:0 0 0 2px #ffe68a;border-radius:3px;opacity:1}",
    ".lo-active body{padding-bottom:84px}",
    ".lo-blk{position:relative}",
    ".lo-spk{position:absolute;left:-52px;top:-1px;width:36px;height:36px;margin:0;padding:0;border-radius:50%;border:1px solid #e3e3e3;background:#fff;color:#d6402a;display:flex;align-items:center;justify-content:center;cursor:pointer;opacity:0;transition:opacity .15s,transform .15s;box-shadow:0 1px 4px rgba(0,0,0,.08);font:inherit;line-height:1}",
    ".lo-blk:hover>.lo-spk,.lo-spk:focus-visible{opacity:1}",
    ".lo-spk:hover{transform:scale(1.08);background:#fff5f3}",
    ".lo-spk svg{width:18px;height:18px}",
    "@media (max-width:820px){.lo-spk{display:none}}",
    ".lo-listen{border:0;background:none;padding:2px 6px;margin:-2px -6px;border-radius:4px;font:inherit;color:#d6402a;font-weight:600;cursor:pointer;text-transform:none;letter-spacing:inherit}",
    ".lo-listen svg{width:1.1em;height:1.1em;vertical-align:-.2em;margin-right:.3em}",
    ".lo-listen:hover{background:#fff5f3;text-decoration:underline}",
    ".lo-bar{position:fixed;left:0;right:0;bottom:0;z-index:1000;display:flex;flex-direction:column;background:#fff;color:#15171a;border-top:1px solid #e5e7eb;box-shadow:0 -4px 18px rgba(0,0,0,.06);font:14px/1.3 -apple-system,BlinkMacSystemFont,\"Segoe UI\",Roboto,sans-serif;box-sizing:border-box}",
    ".lo-bar *{box-sizing:border-box}",
    ".lo-bar[hidden],.lo-bar [hidden]{display:none!important}",
    ".lo-again[hidden]{display:none!important}",
    ".lo-sep[hidden]{display:none!important}",
    ".lo-sep{color:#9ca3af;padding:0 .35em}",
    ".lo-track{height:6px;background:#eef0f3;cursor:pointer;position:relative;touch-action:none}",
    ".lo-fill{position:absolute;left:0;top:0;bottom:0;background:#d6402a;width:0}",
    ".lo-knob{position:absolute;top:50%;left:0;width:12px;height:12px;margin:-6px 0 0 -6px;border-radius:50%;background:#d6402a}",
    ".lo-row{display:flex;align-items:center;gap:14px;padding:10px 20px;max-width:1100px;margin:0 auto;width:100%}",
    ".lo-sub{flex:1;min-width:0;overflow:hidden;white-space:nowrap;text-overflow:ellipsis;color:#6b7280;font-size:13px}",
    ".lo-sp{display:none}",
    ".lo-ends,.lo-step{display:none}",
    ".lo-ends{justify-content:space-between;padding:5px 10px 0;font-size:11px;line-height:1;color:#6b7280;font-variant-numeric:tabular-nums}",
    ".lo-step{flex:none;align-items:center;justify-content:center;height:36px;min-width:44px;padding:0 6px;margin:0;border:1px solid #d6d9de;border-radius:6px;background:#fff;color:#15171a;font:inherit;font-size:14px;font-weight:600;line-height:1;cursor:pointer;font-variant-numeric:tabular-nums}",
    ".lo-ib{border:0;background:transparent;cursor:pointer;color:#333;display:inline-flex;align-items:center;justify-content:center;padding:0;margin:0;font:inherit;width:36px;height:36px;border-radius:50%;flex:none}",
    ".lo-ib:hover{background:#f2f3f5}",
    ".lo-ib svg{width:20px;height:20px}",
    ".lo-bar .lo-play{width:44px;height:44px;background:#15171a;color:#fff}",
    ".lo-bar .lo-play:hover{background:#333}",
    ".lo-time{font-variant-numeric:tabular-nums;color:#6b7280;font-size:13px;min-width:92px;text-align:center}",
    ".lo-bar select{font:inherit;font-size:13px;padding:5px 8px;border:1px solid #d6d9de;border-radius:6px;background:#fff;color:#15171a;width:auto;height:auto}",
    ".lo-bar select.lo-speed{font-variant-numeric:tabular-nums}",
    ".lo-vol{width:84px;margin:0;accent-color:#6b7280;cursor:pointer}",
    // Desktop: a centred floating pill, clear of floating site buttons (e.g. Ghost's subscribe button).
    "@media (min-width:761px){",
    ".lo-bar{left:50%;right:auto;bottom:16px;transform:translateX(-50%);width:max-content;max-width:calc(100vw - 192px);border:1px solid #e5e7eb;border-radius:999px;box-shadow:0 6px 24px rgba(0,0,0,.14)}",
    ".lo-track{margin:8px 28px 0;border-radius:3px}",
    ".lo-fill{border-radius:3px}",
    ".lo-row{gap:10px;padding:4px 14px 6px 22px;max-width:none}",
    ".lo-sub{flex:none;max-width:9em}",
    ".lo-sp{display:none}",
    ".lo-time{min-width:0}",
    ".lo-active body{padding-bottom:96px}",
    "}",
    "@media (max-width:760px){",
    ".lo-speed,.lo-sub,.lo-time,.lo-vol{display:none}",
    ".lo-step,.lo-ends{display:flex}",
    ".lo-sp{display:block;flex:1}",
    ".lo-row{gap:4px;padding:2px max(20px,env(safe-area-inset-right)) calc(10px + env(safe-area-inset-bottom)) max(20px,env(safe-area-inset-left))}",
    ".lo-ends{padding:5px max(20px,env(safe-area-inset-right)) 0 max(20px,env(safe-area-inset-left))}",
    ".lo-bar select{font-size:16px;height:36px;padding:0 22px 0 8px;flex:0 1 auto;min-width:0;max-width:40vw;text-overflow:ellipsis;-webkit-appearance:none;appearance:none;background:#fff url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='10' height='6'%3E%3Cpath d='M0 0l5 6 5-6z' fill='%23666'/%3E%3C/svg%3E\") no-repeat right 7px center}",
    ".lo-active body{padding-bottom:calc(120px + env(safe-area-inset-bottom))}",
    "}"
  ].join("\n");

  // True when the word rect is outside the comfortable band [0.2*h, 0.7*(h - barH)].
  function needsScroll(rectTop, rectBottom, viewportH, barH) {
    return rectTop < 0.2 * viewportH || rectBottom > 0.7 * (viewportH - barH);
  }

  function fmtTime(s) {
    s = isFinite(s) && s > 0 ? Math.floor(s) : 0;
    return Math.floor(s / 60) + ":" + (s % 60 < 10 ? "0" : "") + (s % 60);
  }

  function el(doc, tag, cls, html) {
    var e = doc.createElement(tag);
    if (cls) e.className = cls;
    if (html) e.innerHTML = html;
    return e;
  }

  function iconButton(doc, cls, label, svg) {
    var b = el(doc, "button", "lo-ib" + (cls ? " " + cls : ""), svg);
    b.type = "button";
    b.setAttribute("aria-label", label);
    b.title = label;
    return b;
  }

  // Lock-screen / headset controls. Play and pause go through the controller so its listener-intent
  // flag stays correct; every step is guarded because browsers support different actions.
  function setupMediaSession(win, c) {
    var ms = win.navigator && win.navigator.mediaSession;
    if (!ms) return;
    var doc = win.document;
    var h1 = doc.querySelector("h1");
    var title = ((h1 && h1.textContent) || doc.title || "").trim();
    var lastArtist = null;

    function voiceLabel(id) {
      var list = c.entries || c.valid || [];
      for (var i = 0; i < list.length; i++) {
        var e = list[i].entry || list[i];
        if (e.id === id) return e.label || e.id;
      }
      return id;
    }
    function sync() {
      var st = c.state();
      try {
        if (st.open && win.MediaMetadata) {
          var artist = voiceLabel(st.voice);
          if (artist !== lastArtist) {
            lastArtist = artist;
            ms.metadata = new win.MediaMetadata({ title: title, artist: artist });
          }
        }
        ms.playbackState = st.open && st.playing ? "playing" : st.open ? "paused" : "none";
      } catch (e) { /* ignore */ }
    }
    function handle(action, fn) {
      try { ms.setActionHandler(action, fn); } catch (e) { /* action unsupported here */ }
    }
    handle("play", function () {
      if (!c.state().open) c.start(c.resumePoint() || 0);
      else if (!c.state().playing) c.toggle();
    });
    handle("pause", function () { if (c.state().playing) c.toggle(); });
    handle("previoustrack", function () { if (c.state().open) c.prevBlock(); });
    handle("nexttrack", function () { if (c.state().open) c.nextBlock(); });
    handle("seekto", function (d) {
      var dur = c.duration();
      if (!c.state().open) return;
      if (d && isFinite(d.seekTime) && dur > 0) c.seekFraction(d.seekTime / dur);
    });
    c.onChange(sync);
    c.audio.addEventListener("play", sync);
    c.audio.addEventListener("pause", sync);
    sync();
  }

  function buildUI(win, c, script) {
    var doc = win.document;
    var audio = c.audio, blocks = c.blocks;

    if (!doc.getElementById("lo-style")) {
      var style = doc.createElement("style");
      style.id = "lo-style";
      style.textContent = CSS;
      (doc.head || doc.documentElement).appendChild(style);
    }

    // First token of every block (same flat numbering as data-lo).
    var firsts = [], n = 0;
    blocks.forEach(function (b) { firsts.push(n); n += b.spans.length; });

    // Hover speaker per block.
    blocks.forEach(function (b, bi) {
      b.el.classList.add("lo-blk");
      var btn = el(doc, "button", "lo-spk", SVG_SPK);
      btn.type = "button";
      btn.setAttribute("aria-label", "Ab hier vorlesen");
      btn.title = "Ab hier vorlesen";
      btn.addEventListener("click", function (e) {
        e.stopPropagation();
        c.start(firsts[bi]);
      });
      b.el.insertBefore(btn, b.el.firstChild);
    });

    // Byline link ("Anhören").
    var bylineSel = (script && script.getAttribute && script.getAttribute("data-byline")) || ".byline-reading-time";
    var by = null;
    try { by = doc.querySelector(bylineSel); } catch (e) { by = null; }
    var listen = el(doc, "button", "lo-listen", SVG_SPK + "Anhören");
    listen.type = "button";
    listen.title = "Artikel vorlesen";
    listen.addEventListener("click", function () { c.start(c.resumePoint() || 0); });
    var again = el(doc, "button", "lo-listen lo-again", "Von vorne");
    again.type = "button";
    again.title = "Artikel von vorne vorlesen";
    again.hidden = true;
    again.addEventListener("click", function () { c.start(0); });
    // " / " between "Weiterhören ab …" and "Von vorne"; shown and hidden together with "Von vorne".
    var sep = el(doc, "span", "lo-sep", " / ");
    sep.setAttribute("aria-hidden", "true");
    sep.hidden = true;
    if (by && by.parentNode) {
      var wrap = el(doc, "span", "lo-byline", " <span class=\"bull\">•</span> ");
      wrap.appendChild(listen);
      wrap.appendChild(sep);
      wrap.appendChild(again);
      by.parentNode.insertBefore(wrap, by.nextSibling);
    } else {
      var p = el(doc, "p", "lo-listen-wrap");
      p.appendChild(listen);
      p.appendChild(sep);
      p.appendChild(again);
      var first = blocks[0].el;
      first.parentNode.insertBefore(p, first);
    }

    // Control bar.
    var bar = el(doc, "div", "lo-bar");
    bar.setAttribute("role", "region");
    bar.setAttribute("aria-label", "Vorlesen");
    bar.hidden = true;

    var track = el(doc, "div", "lo-track", '<div class="lo-fill"></div><div class="lo-knob"></div>');
    track.setAttribute("role", "slider");
    track.setAttribute("aria-label", "Fortschritt");
    track.setAttribute("aria-valuemin", "0");
    track.setAttribute("aria-valuemax", "100");
    track.setAttribute("aria-valuenow", "0");
    track.tabIndex = 0;
    var fill = track.firstChild, knob = track.lastChild;

    // Phone only: elapsed / total just below the track.
    var ends = el(doc, "div", "lo-ends", '<span class="lo-el"></span><span class="lo-tot"></span>');
    var elEl = ends.firstChild, elTot = ends.lastChild;

    var row = el(doc, "div", "lo-row");
    var sub = el(doc, "span", "lo-sub");
    var sp = el(doc, "span", "lo-sp");

    var bPrev = iconButton(doc, "", "Vorheriger Absatz", SVG_PREV);
    var bPlay = iconButton(doc, "lo-play", "Wiedergabe", SVG_PLAY);
    var bNext = iconButton(doc, "", "Nächster Absatz", SVG_NEXT);
    var time = el(doc, "span", "lo-time");
    function fmtSpeed(s) { return String(s).replace(".", ",") + "×"; }
    // Desktop: speed as a dropdown (keeps the floating bar narrow).
    var speedSel = el(doc, "select", "lo-speed");
    speedSel.setAttribute("aria-label", "Tempo");
    speedSel.title = "Tempo";
    SPEEDS.forEach(function (s) {
      var o = el(doc, "option");
      o.value = String(s);
      o.textContent = fmtSpeed(s);
      speedSel.appendChild(o);
    });
    speedSel.addEventListener("change", function () { c.setSpeed(parseFloat(speedSel.value)); });
    // Desktop: volume slider (hidden on phones, where the hardware buttons set the volume).
    var vol = el(doc, "input", "lo-vol");
    vol.type = "range";
    vol.min = "0";
    vol.max = "1";
    vol.step = "0.05";
    vol.setAttribute("aria-label", "Lautstärke");
    vol.title = "Lautstärke";
    vol.addEventListener("input", function () { c.setVolume(parseFloat(vol.value)); });
    // Phone: one button that steps to the next speed, wrapping after the last.
    var step = el(doc, "button", "lo-step");
    step.type = "button";
    step.addEventListener("click", function () {
      var i = SPEEDS.indexOf(c.state().speed);
      c.setSpeed(SPEEDS[(i + 1) % SPEEDS.length]);
    });
    var select = el(doc, "select", "lo-voice");
    select.setAttribute("aria-label", "Stimme");
    select.title = "Stimme";
    // Options follow c.voices(), which shrinks after a switch to MP3 (c.valid may be stale).
    var optionsKey = null;
    function rebuildVoices() {
      var ids = c.voices();
      var labels = {};
      (c.entries || c.valid || []).forEach(function (v) { labels[v.entry.id] = v.entry.label || v.entry.id; });
      var key = ids.map(function (id) { return id + "\n" + (labels[id] || id); }).join("\n");
      if (key !== optionsKey) {
        optionsKey = key;
        select.innerHTML = "";
        ids.forEach(function (id) {
          var o = el(doc, "option");
          o.value = id;
          o.textContent = labels[id] || id;
          select.appendChild(o);
        });
      }
      select.hidden = ids.length < 2;
    }
    select.addEventListener("change", function () { c.setVoice(select.value); });
    var bClose = iconButton(doc, "", "Vorlesen schliessen", SVG_CLOSE);

    bPrev.addEventListener("click", function () { c.prevBlock(); });
    bPlay.addEventListener("click", function () { c.toggle(); });
    bNext.addEventListener("click", function () { c.nextBlock(); });
    bClose.addEventListener("click", function () { c.close(); });

    [sub, bPrev, bPlay, bNext, sp, time, vol, speedSel, step, select, bClose].forEach(function (x) { row.appendChild(x); });
    bar.appendChild(track);
    bar.appendChild(ends);
    bar.appendChild(row);
    doc.body.appendChild(bar);

    function seekTo(e) {
      var r = track.getBoundingClientRect();
      if (!r.width) return;
      c.seekFraction((e.clientX - r.left) / r.width);
    }
    // Drag the knob: seek on press and along every move until release (pointer capture keeps
    // the moves coming when the pointer leaves the thin track). A click without pointer events
    // (old browsers, keyboard activation) still seeks.
    var dragging = false, dragged = false;
    track.addEventListener("pointerdown", function (e) {
      if (e.button !== 0) return;
      dragging = dragged = true;
      if (track.setPointerCapture && e.pointerId !== undefined) {
        try { track.setPointerCapture(e.pointerId); } catch (err) { /* not capturable */ }
      }
      e.preventDefault();
      seekTo(e);
    });
    track.addEventListener("pointermove", function (e) { if (dragging) seekTo(e); });
    function endDrag(e) {
      if (!dragging) return;
      dragging = false;
      if (e.type === "pointerup") seekTo(e);
    }
    track.addEventListener("pointerup", endDrag);
    track.addEventListener("pointercancel", endDrag);
    track.addEventListener("click", function (e) {
      if (dragged) { dragged = false; return; } // the press already seeked
      seekTo(e);
    });
    track.addEventListener("keydown", function (e) {
      if (e.key === "ArrowLeft") { e.preventDefault(); c.skipWords(-10); }
      else if (e.key === "ArrowRight") { e.preventDefault(); c.skipWords(10); }
    });

    var suspendedUntil = 0, lastScroll = -Infinity;
    var now = function () { return win.performance.now(); };

    function suspend() { suspendedUntil = now() + 4000; }
    win.addEventListener("wheel", suspend, { passive: true });
    win.addEventListener("touchmove", suspend, { passive: true });
    var SCROLL_KEYS = { PageUp: 1, PageDown: 1, Home: 1, End: 1, ArrowUp: 1, ArrowDown: 1, ArrowLeft: 1, ArrowRight: 1 };
    win.addEventListener("keydown", function (e) {
      if (!SCROLL_KEYS[e.key]) return;
      var t = e.target;
      if (t && t.closest && t.closest("input, select, textarea, [contenteditable]")) return;
      suspend();
    });

    function autoscroll() {
      var t = now();
      if (t < suspendedUntil || t - lastScroll < 600 || typeof win.scrollBy !== "function") return;
      var word = doc.querySelector(".lo-cur");
      if (!word) return;
      var r = word.getBoundingClientRect();
      if (!r.width && !r.height) return; // not laid out
      var h = win.innerHeight;
      var barH = bar.getBoundingClientRect().height || bar.offsetHeight || 0;
      if (!needsScroll(r.top, r.bottom, h, barH)) return;
      lastScroll = t;
      win.scrollBy({ top: r.top - 0.4 * h, behavior: "smooth" });
    }

    var lastListenLabel = null;
    function render() {
      var st = c.state();
      var rp = c.resumePoint();
      var listenLabel = rp ? "Weiterhören ab " + fmtTime(c.timeOf(rp)) : "Anhören";
      if (listenLabel !== lastListenLabel) {
        lastListenLabel = listenLabel;
        listen.innerHTML = SVG_SPK + listenLabel;
      }
      again.hidden = !rp;
      sep.hidden = !rp;
      bar.hidden = !st.open;
      var label = st.playing ? "Pause" : "Wiedergabe";
      bPlay.innerHTML = st.playing ? SVG_PAUSE : SVG_PLAY;
      bPlay.setAttribute("aria-label", label);
      bPlay.title = label;
      var d = isFinite(audio.duration) && audio.duration > 0 ? audio.duration : c.duration();
      var cur = audio.currentTime || 0;
      var frac = d > 0 ? Math.max(0, Math.min(1, cur / d)) : 0;
      time.textContent = fmtTime(cur) + " / " + fmtTime(d);
      elEl.textContent = fmtTime(cur);
      elTot.textContent = fmtTime(d);
      fill.style.width = frac * 100 + "%";
      knob.style.left = frac * 100 + "%";
      track.setAttribute("aria-valuenow", String(Math.round(frac * 100)));
      var bi = 0;
      if (st.cur >= 0) while (bi + 1 < firsts.length && firsts[bi + 1] <= st.cur) bi++;
      sub.textContent = "Absatz " + (bi + 1) + " von " + blocks.length;
      if (speedSel.value !== String(st.speed)) speedSel.value = String(st.speed);
      if (parseFloat(vol.value) !== st.volume) vol.value = String(st.volume);
      step.textContent = fmtSpeed(st.speed);
      step.setAttribute("aria-label", "Tempo " + fmtSpeed(st.speed));
      rebuildVoices();
      if (select.value !== st.voice) select.value = st.voice;
      if (st.open && st.playing) autoscroll();
    }

    c.onChange(render);
    audio.addEventListener("timeupdate", render);
    audio.addEventListener("loadedmetadata", render);
    audio.addEventListener("durationchange", render);
    render();
  }

  // WebKit (Safari, and every browser on iPhone/iPad) seeks Ogg/Opus imprecisely: after setting
  // currentTime the sound can come from up to ~20 s earlier (measured).
  // Constant-bitrate MP3 seeks exactly there, so WebKit prefers the MP3.
  function prefersMp3(nav) {
    var ua = String((nav && nav.userAgent) || "");
    if (/iPhone|iPad|iPod/.test(ua)) return true;
    if (/Macintosh/.test(ua) && nav.maxTouchPoints > 1) return true; // iPadOS desktop mode
    return /Safari\//.test(ua) && !/Chrome|Chromium|CriOS|Edg|OPR|Firefox|FxiOS/.test(ua);
  }

  function init(win, script, opts) {
    opts = opts || {};
    return Promise.resolve().then(function () { return run(win, script, opts); }).catch(function () { return null; });
  }

  function run(win, script, opts) {
    var doc = win.document;
    if (!doc.body || !doc.body.classList.contains("post-template")) return null;
    var audio = opts.createAudio ? opts.createAudio() : new win.Audio();
    var attr = function (name) { return script && script.getAttribute ? script.getAttribute(name) : null; };
    var canOgg = !!audio.canPlayType("audio/ogg; codecs=opus");
    var canMp3 = !!audio.canPlayType("audio/mpeg");
    var forced = attr("data-format");
    var format = forced === "mp3" || forced === "ogg" ? forced
      : canMp3 && prefersMp3(win.navigator) ? "mp3"
      : canOgg ? "ogg" : canMp3 ? "mp3" : null;
    if (!format) return null;
    var slug = slugFromPath(win.location.pathname);
    if (!slug) return null;
    var base = attr("data-base") || siteBase(win, script);
    if (!base) return null;
    if (base.charAt(base.length - 1) !== "/") base += "/";
    var content = doc.querySelector(attr("data-content") || ".gh-content");
    if (!content) return null;
    var dir = base + encodeURIComponent(slug) + "/";

    var getJson = function (url, noCache) {
      return win.fetch(url, noCache ? { cache: "no-cache" } : undefined).then(function (r) {
        if (!r.ok) throw new Error("HTTP " + r.status);
        return r.json();
      });
    };

    return getJson(dir + "manifest.json", true).then(function (manifest) {
      var entries = manifest && Array.isArray(manifest.voices) ? manifest.voices : [];
      if (!entries.length) return null;
      var blocks = extractBlocks(content);
      if (!blocks.length) return null;
      var hashes;
      return Promise.all(blocks.map(function (b) { return hashText(b.text); })).then(function (hs) {
        hashes = hs;
        return Promise.all(entries.map(function (v) {
          return getJson(dir + v.timings).then(function (t) {
            var res = verifyTimings(blocks, hashes, t);
            if (!res.ok) {
              win.console.warn('lectover: voice "' + v.id + '" does not match block ' + res.index + " (" + res.reason + "); re-render the audio");
              return null;
            }
            return { entry: v, timings: t };
          }).catch(function () { return null; });
        }));
      }).then(function (found) {
        var all = found.filter(Boolean);
        var valid = all;
        if (format === "mp3") {
          valid = all.filter(function (v) {
            if (v.entry.mp3) return true;
            win.console.warn('lectover: voice "' + v.entry.id + '" has no mp3; re-render the audio');
            return false;
          });
        }
        // Old renders without MP3: a WebKit browser that can play Ogg still gets the Ogg.
        if (!valid.length && format === "mp3" && !forced && canOgg && all.length) {
          format = "ogg";
          valid = all;
        }
        if (!valid.length) return null;
        var idx = 0;
        var saved = blocks.map(function (b) { return b.el.innerHTML; });
        blocks.forEach(function (b) { idx = wrapTokens(b, idx); });
        var expected = blocks.reduce(function (n, b) { return n + b.tokens.length; }, 0);
        if (idx !== expected) {
          blocks.forEach(function (b, i) { b.el.innerHTML = saved[i]; });
          win.console.warn("lectover: wrapped " + idx + " tokens but verified " + expected + "; page left unchanged");
          return null;
        }
        var controller = createController(win, { audio: audio, base: dir, manifest: manifest, blocks: blocks, content: content, script: script, valid: valid, format: format, entries: all, slug: slug, hashes: hashes });
        buildUI(win, controller, script);
        setupMediaSession(win, controller);
        return controller;
      });
    });
  }

  if (typeof module === "object" && module.exports) {
    module.exports = { prefersMp3: prefersMp3, needsScroll: needsScroll, SPACE_RE: SPACE_RE, collapse: collapse, extractBlocks: extractBlocks, hashText: hashText, slugFromPath: slugFromPath, posKey: posKey, loadPos: loadPos, savePos: savePos, clearPos: clearPos, verifyTimings: verifyTimings, wrapTokens: wrapTokens, findToken: findToken, init: init };
  } else {
    init(window, document.currentScript);
  }
})();
