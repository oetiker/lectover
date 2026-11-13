# Lectover: read-aloud for Ghost posts

[![CI](https://github.com/oetiker/lectover/actions/workflows/ci.yml/badge.svg)](https://github.com/oetiker/lectover/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/oetiker/lectover)](https://github.com/oetiker/lectover/releases/latest)

Lectover renders a public Ghost blog post as spoken Ogg/Opus audio (plus an MP3 copy for
browsers without Ogg/Opus support, such as older Safari and iOS) with
word-level timings (Qwen3-TTS plus Qwen3 forced alignment), and ships a small
vanilla-JS player (`player/lectover.js`) that plays the audio on the post page,
highlights the spoken word, and lets readers click a word to jump there.

German as spoken in Switzerland (`de-CH`) is built in: the text normalisation
(numbers, dates, units), the intro text and the player labels are German. The
pronunciation lexicon is a per-site file (see the example site).

The renderer is one Rust binary, `lectover`, running the models through ggml
(Metal on macOS; HIP and Vulkan tested on an AMD Strix Halo APU; CUDA is
untested). No Python, ffmpeg or other runtime is needed to render.

The manual is [docs/manual.md](docs/manual.md), installed as `man lectover`.

## Install

Linux packages with the Vulkan backend, for amd64 and arm64. Each needs a
Vulkan driver (Mesa or the GPU vendor's). The player script is installed as
`/usr/share/lectover/lectover.js`.

Debian 13, Ubuntu 24.04 and 26.04:

```sh
sudo install -d -m 0755 /etc/apt/keyrings
sudo curl -fsSL -o /etc/apt/keyrings/gitea-oposs.asc https://gitea.oetiker.ch/api/packages/oposs/debian/repository.key
echo "deb [signed-by=/etc/apt/keyrings/gitea-oposs.asc] https://gitea.oetiker.ch/api/packages/oposs/debian $(. /etc/os-release && echo $VERSION_CODENAME) main" | sudo tee /etc/apt/sources.list.d/oposs.list
sudo apt update && sudo apt install lectover
```

Fedora 45, AlmaLinux 9 and 10 (`<dist>` is `fc45`, `el9` or `el10`):

```sh
sudo curl -fsSL -o /etc/yum.repos.d/oposs.repo https://gitea.oetiker.ch/api/packages/oposs/rpm/<dist>.repo
sudo dnf install lectover
```

AlmaLinux takes some libraries from EPEL and CRB; turn them on first:

```sh
sudo dnf install -y epel-release
sudo crb enable
```

Alpine 3.23 and 3.24 (`<branch>` is `v3.23` or `v3.24`):

```sh
cd /etc/apk/keys && sudo curl -fJO https://gitea.oetiker.ch/api/packages/oposs/alpine/key
echo "https://gitea.oetiker.ch/api/packages/oposs/alpine/<branch>/main" | sudo tee -a /etc/apk/repositories
sudo apk update && sudo apk add lectover
```

Archives of the [latest release](https://github.com/oetiker/lectover/releases/latest):

- `lectover-<version>-aarch64-apple-darwin.tar.gz`: macOS on Apple silicon, Metal.
- `lectover-<version>-x86_64-unknown-linux-gnu.tar.gz`, `lectover-<version>-aarch64-unknown-linux-gnu.tar.gz`: Linux with Vulkan; needs glibc 2.39 or newer, `libvulkan.so.1` and a Vulkan driver.

```bash
tar -xzf lectover-<version>-<target>.tar.gz
sudo install -m 0755 lectover/lectover /usr/local/bin/
sudo install -m 0644 lectover/man/lectover.1 /usr/local/share/man/man1/
```

For HIP or CUDA, or another platform, build from source.

## Build

Needs a Rust toolchain, CMake and a C/C++ compiler (Xcode command line tools
on macOS; on Linux the Vulkan, ROCm/HIP or CUDA development files for the
backend you choose; Vulkan also needs `glslc` and SPIRV-Headers, and
the [Containerfile](Containerfile) lists the packages per distribution). The native libraries (ggml from `oetiker/llama.cpp`,
`qwentts.cpp`, `qwen3-asr.cpp`, `reuse.cpp`) are git submodules:

```bash
git clone --recursive https://github.com/oetiker/lectover.git
cd lectover
cargo build --release --features metal      # or cuda, hip, vulkan; none = CPU
cargo test -p lectover                      # fast tests, no models, no network
make man                                    # man/lectover.1, needs pandoc
```

On HIP (and CUDA) builds, do not set `GGML_CUDA_ENABLE_UNIFIED_MEMORY`: ggml
then allocates managed memory, the TTS code predictor computes garbage and the
takes never end ("transcribe: empty audio"). Lectover warns at startup when it
is set (any value counts, even `0`).

The binary is `target/release/lectover`. It runs from any directory and keeps
its data at XDG paths (see Configuration). Only `lectover dev serve` still reads
`player/lectover.js` relative to the working directory, so start it from the
repository root.

## Configuration

Settings come from, in order of precedence: a command-line flag, an environment
variable, the config file, the default. XDG paths apply on Linux and macOS
alike (never `~/Library`).

| Path | Default | Environment | Config key |
| --- | --- | --- | --- |
| config file | `~/.config/lectover/config.toml` | `LECTOVER_CONFIG` | none (`--config FILE`) |
| models | `~/.cache/lectover/models` | `LECTOVER_MODELS` | `paths.models` |
| takes | `~/.cache/lectover/takes` | `LECTOVER_TAKES` | `paths.takes` |
| deploy tree | `~/.local/share/lectover/site` | `LECTOVER_SITE` | `paths.site` |
| voices | `~/.local/share/lectover/voices` | `LECTOVER_VOICES` | `paths.voices` |
| site sounds | `~/.local/share/lectover/sites/<domain>/` | none | `paths.sounds` (parent dir) |
| site lexicon | `~/.config/lectover/lexicon/<domain>.txt` | none | `lexicon` per site |
| `say` takes | `~/.local/state/lectover/say` | none | none |

`$XDG_CONFIG_HOME`, `$XDG_DATA_HOME`, `$XDG_STATE_HOME` and `$XDG_CACHE_HOME`
replace the `~/.config`, `~/.local/share`, `~/.local/state` and `~/.cache`
prefixes. `--config` is accepted by every command.

A site must be configured before its posts can be rendered.
[examples/olten.jetzt/](examples/olten.jetzt/) holds a minimal `config.toml`,
a `lexicon.txt` and a README for a worked example, including how to install
the `erzaehler` voice it names (see also Voices and recording). The full set of keys:

```toml
[paths]                            # all optional
models = "/big/disk/lectover/models"
site   = "/big/disk/lectover/site"

[defaults]
device = "auto"                    # cpu | metal | cuda | hip | vulkan | auto; --device overrides

[site."olten.jetzt"]
ghost_url = "https://olten.jetzt"
language  = "de-CH"                # default; the only supported language
since     = 2026-01-01             # validated; not used yet
voices    = [ { id = "erzaehler", label = "Erzähler" }, { id = "warm", label = "Warme Stimme" } ]
sounds    = "olten.jetzt"          # directory under the sounds root; default: the domain
lexicon   = "olten.jetzt"          # file under lexicon/ without .txt; default: the domain
ghost_key = { env = "GHOST_CONTENT_KEY" }    # default; or { file = "~/.config/lectover/secrets/olten.key" }

[upload]                           # optional; checked by `doctor`, no command uses it yet
target      = "user@host:/srv/lectover/"   # required
ssh_key     = "~/.ssh/lectover"            # required
known_hosts = "~/.ssh/known_hosts"         # optional
```

- `[upload]` is validated and checked by `doctor` (see below). No command
  uploads yet; upload is the `rsync` shown under Deployment.
- The Ghost Content API key is the `data-key` attribute in the site's page
  source (a public, read-only key). `ghost_key` names where it is read from:
  `{ env = "NAME" }` or `{ file = "PATH" }`. A key is never written into the
  config file. A `file` that is readable by group or others is refused.
- `voices` lists the voices rendered for the site and their labels in the
  player. Without it, the first voice in the registry is used.
- Unknown keys produce a warning, not an error. A leading `~` in a path is the
  home directory; a relative path resolves against the config file's directory.
- `[defaults] device` selects the compute device of every command that has
  `--device`. Precedence: `--device`, then `device`, then `auto` (the first GPU,
  else the CPU). Any other value is an error.
- The site's deploy folder is named by its config key, the site's public domain.
- With one configured site, `--site` can be left out; with several, a post URL
  selects its site by host and a bare slug needs `--site <domain>`.

## Doctor

```bash
lectover doctor                    # every configured site
lectover doctor --site olten.jetzt
lectover doctor --offline          # skip the checks that need the network
```

`doctor` prints one line per check (`ok`, `warn`, `FAIL`), with a hint after a
failure, and exits non-zero if any check failed. It checks:

- the config file: present, parsable, unknown keys;
- the models, takes, deploy, voices and state directories (exist or can be
  created) and free disk space for the models and the deploy tree;
- the render models, and the VoiceDesign model (warning only);
- the voice registry;
- `[upload]`: that `ssh_key` exists and is not accessible by group or others;
- per site: voices in the registry, language, lexicon, sounds, and that the
  Ghost key can be read; online, a request to the Ghost Content API with that key;
- the compute device (`[defaults] device`; an invalid value fails) and the
  `GGML_CUDA_ENABLE_UNIFIED_MEMORY` warning.

## Models and licences

`models fetch` downloads, from Hugging Face at pinned revisions:

- the Qwen3-TTS talker and codec (`Serveurperso/Qwen3-TTS-GGUF`);
- the Qwen3 ASR and forced-aligner models (`megaice/Qwen3-ASR-GGUF-Models`);
- the RE-USE speech-cleanup weights (`nvidia/NVIDIA-NemotronLabs-RE-USE`);
- with `--design`, the VoiceDesign talker from the TTS repository.

Each model keeps the licence of its upstream publisher; read the model card of
every repository above before you use the output commercially or redistribute
a model. Lectover does not ship or redistribute any weights.

```bash
lectover models fetch              # talker, codec, ASR, aligner and RE-USE from Hugging Face
lectover models fetch --design     # also the VoiceDesign talker for `voice design`
```

Files go to the models directory (`--models DIR` to change; every command that
loads models takes the same flag). Downloads resume after an
interruption.

The model tests load these files:

```bash
LECTOVER_MODELS=~/.cache/lectover/models cargo test --release -p lectover --features metal -- --ignored
```

`--features metal` is the macOS backend; use `cuda`, `hip` or `vulkan` on Linux.

### RE-USE weights

The RE-USE weights are published by NVIDIA under the OpenMDW-1.1 licence
(commercial use allowed; no restrictions on the generated audio). Use the
`NVIDIA-NemotronLabs-RE-USE` weights only: the sibling repository `nvidia/RE-USE`
is non-commercial. If you distribute the weights or a GGUF derived from them,
you must keep a copy of the OpenMDW-1.1 agreement and the NVIDIA copyright
notices with them: the `LICENSE` and `NOTICE` files of
[reuse.cpp](https://github.com/oetiker/reuse.cpp) contain exactly that text, so
ship them (for example in your third-party notices) alongside.

The RE-USE weights (`model.safetensors`, 38.6 MB, stored as
`reuse-model.safetensors`) are converted once to `reuse-f32.gguf`. They are
used by `voice record` and `voice clean` to remove noise and room reverb from
reference clips (C++ library: `reuse.cpp`, submodule
`crates/lectover-sys/vendor/reuse.cpp`).

## Rendering a post

Configure the site first (see Configuration). With the default `ghost_key`,
export the key, then render:

```bash
read -rs GHOST_CONTENT_KEY; export GHOST_CONTENT_KEY
lectover render https://olten.jetzt/frohburg-spiel-satz-patt/
lectover render frohburg-spiel-satz-patt --voice erzaehler --voice warm   # `warm`: see Voices and recording
```

`--voice` takes one id and can be repeated; without it the site's `voices`
are used, else the first voice in the registry. Other options: `--site DOMAIN`
(for a slug), `--out DIR` (deploy tree, default
`~/.local/share/lectover/site`), `--seed N` (42), `--temperature T` (0.9),
`--no-pace`, `--device cpu|metal|cuda|hip|vulkan`, `--lexicon FILE` (default:
the site's lexicon), `--sounds DIR` (default: the site's sounds).

Each chunk is generated, transcribed and compared with its text; a take
scoring below 0.90 is regenerated with the next seed, up to four rounds. On an
M-series Mac a post renders in about 0.6 times its audio length.

Output goes to `<deploy tree>/<domain>/<slug>/`, where `<domain>` is the
site's config key: `manifest.json` plus `<voice>.<hash>.ogg`,
`<voice>.<hash>.mp3` and `<voice>.<hash>.json` per voice. The MP3 (64 kbps,
24 kHz mono) is always written beside the Ogg and has its own content hash;
the `.json` timings are shared with the Ogg (same hash). Only public posts are
rendered. Re-render after editing a post; the player verifies the text against
the timings and stays silent (with a console warning) if they no longer match.

### What the audio adds to the page

- **Intro.** Every recording opens with *Sie hören: «Titel». Verfasst von A und
  B. Publiziert am 16. Oktober 2026.* (authors and publication date from Ghost;
  parts Ghost does not report are left out), followed by the site's intro sound.
- **Breaks.** An `<hr>` in the post plays the site's break sound.
- **Audio-only text.** An HTML comment starting with `AUDIO ONLY` (for example in an
  HTML card) is spoken where it stands and never shown on the page. Blank lines
  separate paragraphs; a run of three or more `-` plays the break sound:

  ```html
  <!-- AUDIO ONLY
  Etwas, das nur im Audio gesprochen wird.
  ---
  Noch mehr Text nach dem Break.
  -->
  ```

Nothing in the intro or the audio-only text is highlighted; the page blocks and
their hashes stay exactly as before.

The sounds come from the site's sounds directory,
`~/.local/share/lectover/sites/<domain>/` by default (`--sounds DIR`):
`intro.wav` plays after the intro, `break.wav` at breaks
(without it, breaks reuse `intro.wav`). Any sample rate and channel count; they
are mixed to mono, resampled and levelled to the speech level. Without
`intro.wav` the intro is spoken without a sound and breaks are a pause.

### Trying pronunciations

`lectover say "Der Sessjonsbericht vom Mai." "Der Sessi-onsbericht vom Mai."
--voice erzaehler --voice warm` (`warm` is created under Voices and recording) speaks each line (after the lexicon, with the
same check, retries, trim and levelling as a render), writes
`~/.local/state/lectover/say/<voice>-<n>.wav` (`--out DIR`) and plays it
(`afplay`, else `aplay`; `--no-play` skips). The lexicon is the one of `--site`
(default: the only configured site). Takes are cached, so repeating a command
plays at once. Lines the lexicon changed are printed as "spoken as".

## Voices and recording

Voices live in the voices directory (`~/.local/share/lectover/voices`;
`--voices DIR` or `LECTOVER_VOICES` to change): `voices.json` (id -> label)
and, per voice, a reference clip `<id>.wav` with its transcript `<id>.txt`.

Test voices for development are in `tests/fixtures/voices/`.

Create a new voice from a description with the VoiceDesign model (needs
`models fetch --design`). The player shows the label to readers, so write it
in German:

```bash
lectover voice design warm --label "Warme Stimme" \
    --instruct "Eine warme, ruhige Frauenstimme mittleren Alters, deutlich und freundlich."
```

This writes `warm.wav` and `warm.txt` to the voices directory and registers the voice.
`--force` replaces an existing voice (its old files are kept with a
timestamp); `--seed` picks another variant of the same description.

Or record a reference clip directly from your microphone on macOS:

```bash
lectover voice record me --label "Eigene Stimme"
```

The command runs in four steps:
1. **Gain calibration**: measures ambient room noise, shows a short test
   phrase to read, and sets the input volume to −6 dBFS.
2. **Armed recording**: shows the reference text and waits for speech
   (with ~200 ms pre-roll).
3. **Live countdown**: records up to the estimated duration (or `--seconds N`).
   `r` or `Space` restarts at any point.
4. **Review**: plays the audio (`afplay`); `[Enter]` saves, `[r]` records
   again, `[p]` replays, `[q]` quits.

Every take is cleaned (noise and reverb) with RE-USE before the preview, so you
hear what will be saved. The audio is resampled to 24 kHz mono,
trimmed, levelled and saved as `<id>.wav` alongside its transcript in
the voices directory; the sample-aligned raw take is kept as `<id>.raw.wav` (not
registered in `voices.json`). `--text "..."` sets the spoken
reference text. `--force` replaces an existing voice and keeps its old files,
including `.raw.wav`, with a timestamp. `--device` and `--models` work as for
`render`.

To clean an existing voice, write the result as a new one (the source stays
untouched):

```bash
lectover voice clean erzaehler                  # -> erzaehlerreuse, label "<label> (RE-USE)"
lectover voice clean erzaehler --as erzaehler-clean --label "Erzähler sauber" --force
```

The noise floor before and after is logged. Clips longer than 90 s are refused.

## Dev page

A Ghost-like page for trying the player locally, without touching the blog:

```bash
lectover dev build https://olten.jetzt/frohburg-spiel-satz-patt/
lectover dev serve                  # --port 8000; --out is the deploy tree
```

Then open `http://localhost:8000/olten.jetzt/frohburg-spiel-satz-patt/` in a
normal browser. `build` writes `<deploy tree>/<domain>/<slug>/index.html` next to
the post's audio; the page loads the player from `/js?domain=<domain>` like
the real embed, and the server answers `/js` with `player/lectover.js`. The
server supports HTTP Range requests, which Chrome needs for seeking. Render
the post first so the audio exists.

## Player tests

```bash
# needs Node.js and pnpm
cd player && pnpm install && pnpm test   # node --test + jsdom
```

## Deployment

Everything is served as static files from
a host of your choice (here `lectover.oetiker.ch`, the host the player
script comes from): the player plus one folder per blog.

```
/srv/lectover/
  lectover.js                    # served as /js
  olten.jetzt/<slug>/manifest.json, <voice>.<hash>.ogg, <voice>.<hash>.mp3, <voice>.<hash>.json
```

Upload a blog's rendered posts and the player (the dev pages are excluded):

```bash
rsync -av --exclude index.html ~/.local/share/lectover/site/olten.jetzt player/lectover.js <user>@lectover.oetiker.ch:/srv/lectover/
```

Apache (needs `mod_rewrite` and `mod_headers`; Range requests work by default):

```apache
<VirtualHost *:443>
    ServerName lectover.oetiker.ch
    DocumentRoot /srv/lectover
    # ... TLS settings ...

    <Directory /srv/lectover>
        Options -Indexes
        Require all granted
    </Directory>

    RewriteEngine On
    RewriteRule ^/js$ /lectover.js [L]

    AddType audio/ogg .ogg
    AddType audio/mpeg .mp3
    AddType text/javascript .js
    # Audio is public; "always" also covers 404s, so a missing post fails quietly.
    Header always set Access-Control-Allow-Origin "*"

    # One script URL for every blog: keep it short-lived so updates arrive quickly.
    <Files "lectover.js">
        Header set Cache-Control "public, max-age=300"
    </Files>
    # The manifest changes on every re-render; audio and timings are content-hashed.
    <Files "manifest.json">
        Header set Cache-Control "no-cache"
    </Files>
    <FilesMatch "\.[0-9a-f]{8}\.(ogg|mp3|json)$">
        Header set Cache-Control "public, max-age=31536000, immutable"
    </FilesMatch>
</VirtualHost>
```

Ghost admin: Settings -> Code injection -> Site Footer:

```html
<script src="https://lectover.oetiker.ch/js?domain=olten.jetzt" defer></script>
```

The player loads audio from `https://lectover.oetiker.ch/<domain>/<slug>/`.
Without `?domain=` it uses the page's own host. If `domain` does not match the
page's host (ignoring `www.`), the player stays hidden and logs a warning, so a
site cannot show another site's audio; `localhost` and `127.0.0.1` are exempt
for the dev page.

It must be a plain classic script, **never `type="module"`**: the player finds
its own URL through `document.currentScript`, which is null for modules.
Optional attributes on the tag: `data-base` (full audio base URL for this site,
overrides the domain folder), `data-content` (content selector, default
`.gh-content`), `data-byline` (selector for the byline element the "Anhören"
link is placed next to, default `.byline-reading-time`).

The player host (`lectover.oetiker.ch` in this example) runs code on every blog that embeds
it: whoever controls that host controls the script, so keep it locked down.

### Formats, compact bar, resume

- **MP3 fallback.** The player prefers Ogg/Opus and falls back to the MP3
  automatically where the browser cannot play Ogg (Safari before 18.4). If the
  Ogg is chosen but fails with a decode or "unsupported" error before playback
  has started, the player switches to the MP3 at the same position. Network
  errors and errors after playback has begun do not trigger the fallback.
- **Format choice.** Safari and every browser on iPhone/iPad get the MP3 (WebKit seeks Ogg/Opus
  imprecisely); others get the Ogg. `data-format="mp3"` or `"ogg"` on the script tag forces one.
- **Compact bar.** The player is a slim bar at the bottom of the page (a
  two-row layout on phones): previous/next paragraph, play/pause, time, speed
  and voice, close.
- **Resume.** The listening position is kept per post in `localStorage`
  (`lectover.pos.<slug>`) for 30 days. On the next visit the link reads
  "Weiterhören ab M:SS", with "Von vorne" beside it. A position is dropped when
  the paragraph's text has changed since it was saved. Where `localStorage` is
  unavailable, resume is off.

A browser that can play neither format gets an unchanged page.

## Licence

Copyright (C) 2026 Tobias Oetiker. Lectover is free software under the GNU
Affero General Public License, version 3 or later (AGPL-3.0-or-later); see
[LICENSE](LICENSE).
