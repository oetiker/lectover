# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
except that the first subsection is called `New` rather than `Added`. The release
workflow matches on `### New`; renaming it silently drops the section from the
release notes.

This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### New
- Release archives with a prebuilt `lectover`: macOS on Apple silicon with Metal, and Linux x86_64 and arm64 with Vulkan (glibc 2.39 or newer, `libvulkan.so.1`).
- The manual page `lectover(1)`, built from `docs/manual.md` and shipped in every archive.
- Linux packages with the Vulkan backend for Debian 13, Ubuntu 24.04 and 26.04, Fedora 45, AlmaLinux 9 and 10 and Alpine 3.23 and 3.24, amd64 and arm64, in the apt, dnf and apk repositories on gitea.oetiker.ch. They install the player script as `/usr/share/lectover/lectover.js`.
- A Homebrew formula for macOS on Apple silicon, with a bottle; this repository is the tap. The release archives carry the player script `lectover.js`.
