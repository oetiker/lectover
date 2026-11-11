---
title: LECTOVER
section: 1
header: lectover manual
footer: lectover
date: 2026-11-11
---

# NAME

lectover - render Ghost blog posts as narrated audio for a read-aloud player

# SYNOPSIS

**lectover** [**--config** *FILE*] **render** *POST* [*options*]

**lectover** [**--config** *FILE*] **say** *TEXT*... [*options*]

**lectover** [**--config** *FILE*] **models fetch** [**--design**] [**--models** *DIR*]

**lectover** [**--config** *FILE*] **voice design**|**record**|**clean** *ID* [*options*]

**lectover** [**--config** *FILE*] **dev build** *POST* | **dev serve** [*options*]

**lectover** [**--config** *FILE*] **doctor** [**--site** *DOMAIN*] [**--offline**]

# CONFIGURATION

The configuration file is TOML.
Its path is the **--config** option, else `$LECTOVER_CONFIG`, else `~/.config/lectover/config.toml`.
A setting given as an option overrides the environment, which overrides the file, which overrides the default.
A leading `~` in a path is the home directory; a relative path resolves against the directory of the configuration file.
An unknown key produces a warning.

- `paths.models`: Models directory. Default `~/.cache/lectover/models`.
- `paths.takes`: Cache of synthesised takes. Default `~/.cache/lectover/takes`.
- `paths.site`: Deploy tree. Default `~/.local/share/lectover/site`.
- `paths.voices`: Voice registry directory. Default `~/.local/share/lectover/voices`.
- `paths.sounds`: Parent directory of the per-site sound directories. Default `~/.local/share/lectover/sites`.
- `defaults.device`: Compute device of every command that has **--device**: `cpu`, `metal`, `cuda`, `hip`, `vulkan` or `auto`. Default `auto`, the first GPU, else the CPU.

Each site is a table `[site."DOMAIN"]`, named by the public domain of the blog.
The domain also names the site's folder in the deploy tree.

- `ghost_url`: Base URL of the Ghost site. Required.
- `language`: Language of the posts. Default and only supported value `de-CH`.
- `since`: A date. Validated, not used.
- `voices`: List of `{ id = "...", label = "..." }` tables: the voices rendered for the site and their labels in the player. Default: the first voice in the registry.
- `sounds`: Directory under the sounds root holding `intro.wav` and `break.wav`. Default: the domain.
- `lexicon`: Lexicon file under `~/.config/lectover/lexicon/`, without `.txt`. Default: the domain.
- `ghost_key`: Where the Ghost Content API key is read from: `{ env = "NAME" }` or `{ file = "PATH" }`. Default `{ env = "GHOST_CONTENT_KEY" }`. A key file readable by group or others is refused.

The optional `[upload]` table has `target` and `ssh_key` (both required) and `known_hosts`.
**doctor** checks it; no command uses it.

# DESCRIPTION

**lectover** fetches a public post through the Ghost Content API, splits it into chunks, speaks each chunk with Qwen3-TTS through ggml, transcribes the take with Qwen3 ASR and compares the transcript with the text.
A take that scores below 0.90 is generated again with the next seed, for up to four rounds.
A forced aligner gives the time of every word.

The result is an Ogg/Opus file, an MP3 copy (64 kbps, 24 kHz mono) and a JSON file of word timings per voice, plus a `manifest.json`, under `<deploy tree>/<domain>/<slug>/`.
File names carry a content hash, `<voice>.<hash>.ogg`, `<voice>.<hash>.mp3` and `<voice>.<hash>.json`; the Ogg and the timings share one hash.
The player script `player/lectover.js` plays the audio on the post page and highlights the spoken word.

Every recording opens with a spoken intro naming the title, the authors and the publication date, followed by the site's `intro.wav`.
An `<hr>` in the post plays `break.wav`, or `intro.wav` when there is no `break.wav`.
An HTML comment that starts with `AUDIO ONLY` is spoken where it stands and not shown on the page; blank lines in it separate paragraphs, and a line of three or more `-` plays the break sound.

The text normalisation, the intro and the player labels are German as spoken in Switzerland.
The per-site lexicon holds `written = spoken` lines applied before synthesis.

# OPTIONS

- `--config FILE`: Configuration file. Accepted by every command.
- `-h, --help`: Print a usage summary and exit.
- `-V, --version`: Print the version and exit.

Options shared by several commands:

- `--device D`: Compute device: `cpu`, `metal`, `cuda`, `hip` or `vulkan`. Default: `defaults.device`, else the first GPU, else the CPU.
- `--models DIR`: Models directory.
- `--voices DIR`: Voice registry directory.
- `--site DOMAIN`: The configured site. Default: the only configured site. A post URL names its site by host; a slug needs **--site** when several sites are configured.
- `--seed N`: Seed of the first take. Default 42.

# COMMANDS

## render POST

Renders a public post, given as URL or slug, into the deploy tree.

- `--voice ID`: Voice id from the registry; repeat for more voices. Default: the site's `voices`, else the first voice in the registry.
- `--out DIR`: Deploy tree. The post goes to `DIR/<domain>/<slug>/`.
- `--temperature T`: Sampling temperature of the talker. Default 0.9.
- `--subtalker-temperature T`: Sampling temperature of the residual codebooks. Default 0.6.
- `--max-chars N`: Longest chunk in characters; longer blocks are split at sentences. Default 1000.
- `--max-batch N`: Takes decoded together, 1 to 64. Batched takes depend on what shares the batch and differ from unbatched ones. Default 6.
- `--streaming`: Feed the chunk text to the talker step by step instead of in the prompt.
- `--no-pace`: Do not pace-match the chunks.
- `--no-cache`: Synthesise every take again instead of reusing cached takes.
- `--lexicon FILE`: Pronunciation lexicon. A missing file means none. Default: the site's lexicon.
- `--sounds DIR`: Directory holding `intro.wav` and `break.wav`. Default: the site's sounds directory.

## say TEXT...

Speaks each line after the lexicon, with the check, retries, trimming and levelling of **render**, writes `<voice>-<n>.wav` and plays it with **afplay**, else **aplay**.
Lines the lexicon changed are printed as "spoken as".
Takes are cached.

- `--voice ID`: Voice id; repeat for more voices. Default: the first voice.
- `--out DIR`: Output directory. Default `~/.local/state/lectover/say`.
- `--no-play`: Write the takes without playing them.
- `--lexicon FILE`: Pronunciation lexicon. Default: the lexicon of **--site**.

## models fetch

Downloads the Qwen3-TTS talker and codec, the Qwen3 ASR and forced-aligner models and the RE-USE speech-cleanup weights from Hugging Face at pinned revisions, and converts the RE-USE weights to `reuse-f32.gguf`.
An interrupted download resumes.

- `--design`: Also fetch the VoiceDesign talker for **voice design**.

## voice design ID

Creates a reference voice from a description with the VoiceDesign model, writes `ID.wav` and `ID.txt` to the voice registry and registers it in `voices.json`.

- `--label TEXT`: Label of the voice in the player. Required.
- `--instruct TEXT`: Description of the voice. Required.
- `--temperature T`: Sampling temperature. Default 0.9.
- `--force`: Replace an existing voice; its old files are kept with a timestamp.

## voice record ID

Records a reference voice from the microphone on macOS.
The command measures the room noise and sets the input gain, waits for speech, records with a countdown and plays the take for review.
`r` or `Space` restarts the take; in the review, `Enter` saves, `r` records again, `p` replays and `q` quits.
Each take is cleaned of noise and reverb with RE-USE, resampled to 24 kHz mono, trimmed, levelled and saved as `ID.wav` with its transcript; the raw take is kept as `ID.raw.wav`.

- `--label TEXT`: Label of the voice in the player. Required.
- `--text TEXT`: Text to read. Default: a standard reference text.
- `--seconds N`: Longest take in seconds. Default: twice the estimate from the length of the text.
- `--force`: Replace an existing voice; its old files are kept with a timestamp.

## voice clean ID

Writes a RE-USE-cleaned copy of a registered voice as a new voice and logs the noise floor before and after.
The source is not changed.
Clips longer than 90 seconds are refused.

- `--as ID`: Id of the new voice. Default `<ID>reuse`.
- `--label TEXT`: Label of the new voice. Default: the source label followed by "(RE-USE)".
- `--force`: Replace an existing target voice; its old files are kept with a timestamp.

## dev build POST

Writes `<deploy tree>/<domain>/<slug>/index.html`, a Ghost-like page that loads the player from `/js?domain=<domain>`.

- `--out DIR`: Deploy tree.

## dev serve

Serves the deploy tree on 127.0.0.1 with HTTP Range support and answers `/js` with `player/lectover.js`, read relative to the working directory.

- `--port N`: Port. Default 8000.
- `--out DIR`: Deploy tree.

## doctor

Checks the configuration file, the directories and their free space, the models, the voice registry, `[upload]`, each site (voices, language, lexicon, sounds, Ghost key, and online a request to the Ghost Content API) and the compute device.
Prints one line per check, `ok`, `warn` or `FAIL`, with a hint after a failure.

- `--site DOMAIN`: Check only this site. Default: every configured site.
- `--offline`: Skip the checks that need the network.

# EXIT STATUS

**lectover** exits 0 on success and 1 on an error.
**doctor** exits 1 when any check failed, else 0.
A usage error exits 2.

# ENVIRONMENT

- `LECTOVER_CONFIG`: Configuration file.
- `LECTOVER_MODELS`: Models directory.
- `LECTOVER_TAKES`: Cache of synthesised takes.
- `LECTOVER_SITE`: Deploy tree.
- `LECTOVER_VOICES`: Voice registry directory.
- `XDG_CONFIG_HOME, XDG_DATA_HOME, XDG_STATE_HOME, XDG_CACHE_HOME`: Replace the `~/.config`, `~/.local/share`, `~/.local/state` and `~/.cache` prefixes of the default paths, on Linux and macOS alike.
- `GHOST_CONTENT_KEY`: The Ghost Content API key of a site whose `ghost_key` is the default.
- `GGML_CUDA_ENABLE_UNIFIED_MEMORY`: When set to any value, **lectover** warns at startup. On HIP and CUDA builds it makes the talker produce takes that never end.

# FILES

- `~/.config/lectover/config.toml`: Configuration file.
- `~/.config/lectover/lexicon/<domain>.txt`: Pronunciation lexicon of a site.
- `~/.cache/lectover/models/`: Model files.
- `~/.cache/lectover/takes/`: Cached takes.
- `~/.local/share/lectover/site/`: Deploy tree.
- `~/.local/share/lectover/voices/`: Voice registry: `voices.json` and, per voice, `<id>.wav` and `<id>.txt`.
- `~/.local/share/lectover/sites/<domain>/`: Sounds of a site, `intro.wav` and `break.wav`.
- `~/.local/state/lectover/say/`: Takes written by **say**.

# EXAMPLES

Fetch the models and check the installation:

```sh
lectover models fetch
lectover doctor
```

Render a post in two voices:

```sh
lectover render https://example.org/a-post/ --voice erzaehler --voice warm
```

Try two spellings of a word:

```sh
lectover say "Der Sessjonsbericht vom Mai." "Der Sessi-onsbericht vom Mai."
```

Create a voice from a description:

```sh
lectover voice design warm --label "Warme Stimme" \
    --instruct "Eine warme, ruhige Frauenstimme mittleren Alters."
```

# AUTHORS

Tobias Oetiker <tobi@oetiker.ch>,
Claude (Anthropic).

# REPORTING BUGS

<https://github.com/oetiker/lectover/issues>

# COPYRIGHT

Copyright (C) 2026 Tobias Oetiker.
Released under the GNU Affero General Public License, version 3 or later, see
<https://github.com/oetiker/lectover/blob/main/LICENSE>.

# SEE ALSO

Project home: <https://github.com/oetiker/lectover>

**afplay**(1), **aplay**(1)
