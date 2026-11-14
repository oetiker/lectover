#!/bin/sh
# Builds the throwaway home the README demo runs in: the example site's
# config and lexicon, the fixture voice `erzaehler`, and a link to the
# models. A first render fills the take cache, so the recorded render
# reuses the takes and finishes in seconds.
# Usage: demo/setup.sh HOME_DIR BINARY   (GHOST_CONTENT_KEY must be set)
set -eu
home=$1
bin=$2
root=$(cd "$(dirname "$0")/.." && pwd)
models=${LECTOVER_MODELS:-${XDG_CACHE_HOME:-$HOME/.cache}/lectover/models}

: "${GHOST_CONTENT_KEY:?set GHOST_CONTENT_KEY to the Content API key of olten.jetzt}"
[ -d "$models" ] || { echo "no models in $models; run lectover models fetch" >&2; exit 1; }

mkdir -p "$home/.config/lectover/lexicon" "$home/.local/share/lectover/voices" "$home/.cache/lectover"
cp "$root/examples/olten.jetzt/config.toml" "$home/.config/lectover/config.toml"
cp "$root/examples/olten.jetzt/lexicon.txt" "$home/.config/lectover/lexicon/olten.jetzt.txt"
cp "$root/tests/fixtures/voices/erzaehler.wav" "$root/tests/fixtures/voices/erzaehler.txt" \
  "$home/.local/share/lectover/voices/"
printf '{\n  "erzaehler": {\n    "label": "Erzähler"\n  }\n}\n' \
  >"$home/.local/share/lectover/voices/voices.json"
ln -sfn "$models" "$home/.cache/lectover/models"

echo "warming the take cache"
env -u XDG_CONFIG_HOME -u XDG_DATA_HOME -u XDG_STATE_HOME -u XDG_CACHE_HOME \
  -u LECTOVER_CONFIG -u LECTOVER_MODELS -u LECTOVER_TAKES -u LECTOVER_VOICES \
  HOME="$home" LECTOVER_SITE="$home/site" \
  "$bin" render stadtfuhrung-mit-dem-oltner-stadtgartner >/dev/null 2>"$home/warmup.log"
