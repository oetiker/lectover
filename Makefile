MAN_NAME = lectover
include build/man.mk

# build, check and install are the contract of ri-release-distro-packages,
# which runs them in the Containerfile's image of each distribution release.
# The GPU backend is a cargo feature: Metal on macOS, Vulkan elsewhere.
ifeq ($(shell uname -s),Darwin)
FEATURES ?= metal
else
FEATURES ?= vulkan
endif
CARGO ?= cargo
PREFIX ?= /usr
# ggml tunes its CPU code to the build machine by default (-march=native).
# A package runs on other CPUs than the runner's, and on Debian 12 arm64
# GCC 12 refuses the native flags outright ("inlining failed in call to
# 'always_inline' 'vfmaq_f16': target specific option mismatch").
export GGML_NATIVE ?= OFF
# Apart from target/, so a container build and a host build of the same
# checkout do not overwrite each other's artifacts.
export CARGO_TARGET_DIR ?= target/make
BINARY = $(CARGO_TARGET_DIR)/release/lectover

all: build

build:
	$(CARGO) build --release --locked --features $(FEATURES)

check: build
	$(CARGO) test --release --locked -p lectover --features $(FEATURES)
	$(BINARY) --version

install: build man
	install -d $(DESTDIR)$(PREFIX)/bin $(DESTDIR)$(PREFIX)/share/man/man1 \
	  $(DESTDIR)$(PREFIX)/share/doc/lectover $(DESTDIR)$(PREFIX)/share/lectover
	install -m 0755 $(BINARY) $(DESTDIR)$(PREFIX)/bin/lectover
	install -m 0644 man/lectover.1 $(DESTDIR)$(PREFIX)/share/man/man1/lectover.1
	install -m 0644 README.md CHANGES.md $(DESTDIR)$(PREFIX)/share/doc/lectover/
	install -m 0644 player/lectover.js $(DESTDIR)$(PREFIX)/share/lectover/lectover.js

# The README demo (docs/demo/lectover.webp), recorded with ansidrama
# (https://github.com/oetiker/ansidrama) in a throwaway home. It renders a
# post of olten.jetzt, so GHOST_CONTENT_KEY must be set, and it needs the
# models (`lectover models fetch`). Run by hand; CI does not render it.
DEMO_HOME ?= /tmp/lectover-demo

demo: build
	demo/setup.sh $(DEMO_HOME) $(abspath $(BINARY))
	cd demo && LECTOVER_DEMO_HOME=$(DEMO_HOME) \
	  LECTOVER_DEMO_BIN=$(abspath $(dir $(BINARY))) ansidrama record lectover.toml

.PHONY: all build check install demo
