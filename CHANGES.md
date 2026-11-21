# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
except that the first subsection is called `New` rather than `Added`. The release
workflow matches on `### New`; renaming it silently drops the section from the
release notes.

This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### New
- `lectover render FILE.md` and `lectover dev build FILE.md` read a local Markdown or HTML file instead of a Ghost post: no Content API key, no network. `--title`, `--author`, `--date` and `--slug` set what Ghost would otherwise supply.
- Player: the volume, speed and voice buttons show their value and open a popover: a vertical slider for volume and speed, a list for the voice (no native select anymore). A popover closes shortly after the mouse leaves its button. Speed is continuous from 0.8× to 2× in steps of 0.05. Times shown are divided by the playback speed. On a phone, the speed button replaces the step button.

### Changed
- On macOS a plain `cargo build` includes the Metal backend; `--features metal` is no longer needed.
- Gender forms like `Schüler:innen`, `Autor*in`, `Lehrer_innen` and `Bürger/innen` are spoken with a short glottal stop instead of a pause. A lexicon entry still takes precedence.

### Fixed
- Player: clicking the play/pause button while audio was playing was sometimes ignored.

## 0.1.1 - 2026-11-16
### Changed
- On x86-64, `lectover` checks at start for AVX2, FMA and F16C, which ggml's CPU code is built for, and exits with an error naming the missing ones instead of stopping with "Illegal instruction". The build pins these instruction sets, so a build with `SOURCE_DATE_EPOCH` set no longer drops ggml to plain SSE.

### Fixed
- The AlmaLinux install steps in the README install `dnf-plugins-core`, which `crb enable` needs and minimal AlmaLinux systems lack.
- `brew install lectover` on Linux or an Intel Mac says the formula needs macOS on Apple silicon, instead of failing with "formula requires at least a URL".

## 0.1.0 - 2026-11-14
### New
- Release archives with a prebuilt `lectover`: macOS on Apple silicon with Metal, and Linux x86_64 and arm64 with Vulkan (glibc 2.39 or newer, `libvulkan.so.1`).
- The manual page `lectover(1)`, built from `docs/manual.md` and shipped in every archive.
- Linux packages with the Vulkan backend for Debian 13, Ubuntu 24.04 and 26.04, Fedora 45, AlmaLinux 9 and 10 and Alpine 3.23 and 3.24, amd64 and arm64, in the apt, dnf and apk repositories on gitea.oetiker.ch. They install the player script as `/usr/share/lectover/lectover.js`.
- A Homebrew formula for macOS on Apple silicon, with a bottle; this repository is the tap. The release archives carry the player script `lectover.js`.

### Fixed
- `lectover doctor` writes "1 voice" and "1 entry" instead of "1 voices" and "1 entries".
