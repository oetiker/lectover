# Example site: olten.jetzt

Put `config.toml` at `~/.config/lectover/config.toml` and `lexicon.txt` at
`~/.config/lectover/lexicon/olten.jetzt.txt`, then set `GHOST_CONTENT_KEY` to your Ghost Content API key.

The intro and break sounds are not included. To use your own, save them as
`~/.local/share/lectover/sites/olten.jetzt/intro.wav` and `break.wav` (`sounds = "olten.jetzt"`
in the config selects the directory; a missing `break.wav` falls back to the intro sound).

## Voice

The config names the voice `erzaehler`. A test voice with this id is in
`tests/fixtures/voices/` of the repository. Install it into the voices directory:

```bash
mkdir -p ~/.local/share/lectover/voices
cp tests/fixtures/voices/erzaehler.wav tests/fixtures/voices/erzaehler.txt ~/.local/share/lectover/voices/
cat > ~/.local/share/lectover/voices/voices.json <<'EOF'
{
  "erzaehler": {
    "label": "Erzähler"
  }
}
EOF
```

If `voices.json` already exists, add the `erzaehler` entry to it instead of overwriting the file.

Or create the voice from a description (needs `lectover models fetch --design`):

```bash
lectover voice design erzaehler --label "Erzähler" \
    --instruct "Eine ruhige Männerstimme, deutlich und freundlich."
```
