# The Linux build environment of lectover with the Vulkan backend.
#
# Two users:
# - release-build-local.yml builds it with the default base, Ubuntu 24.04,
#   for the glibc release archives (oldest glibc they run on: 2.39).
# - ri-release-distro-packages builds the stage build-deps with BASE set to
#   the image of each distribution release and runs `make`, `make check` and
#   `make install DESTDIR=` in it, with the checkout mounted at /src.
#
# The image is built without a build context, so the package names of every
# family live here. ggml-vulkan's CMakeLists needs glslc and the
# SPIRV-Headers CMake config besides the Vulkan headers; bindgen needs
# libclang. Not built: Ubuntu 22.04, which ships no glslc package, and
# Debian 12, whose Vulkan headers (1.3.239) lack VK_EXT_layer_settings.
#
# Alpine builds with its own cargo: rustup's musl toolchain links the C
# runtime statically, and a static binary cannot load the system's Vulkan
# driver through libvulkan.so.1.
ARG BASE=docker.io/library/ubuntu:24.04
FROM ${BASE} AS build-deps

RUN . /etc/os-release; \
    case " $ID ${ID_LIKE:-} " in \
      *" debian "*) \
        apt-get update \
        && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
             ca-certificates curl git make build-essential cmake clang libclang-dev \
             libvulkan-dev glslc spirv-headers \
        && rm -rf /var/lib/apt/lists/* ;; \
      *" fedora "*) \
        dnf install -y ca-certificates git make gcc gcc-c++ cmake clang clang-devel \
             vulkan-headers vulkan-loader-devel glslc spirv-headers-devel \
        && dnf clean all ;; \
      *" alpine "*) \
        apk add --no-cache ca-certificates curl git make build-base cmake clang-dev \
             linux-headers vulkan-headers vulkan-loader-dev shaderc spirv-headers cargo ;; \
      *) echo "no build dependencies for $ID" >&2; exit 1 ;; \
    esac

# audiopus_sys builds its bundled opus with CMake and looks for it in
# <out>/lib; GNUInstallDirs on Fedora and AlmaLinux installs to lib64, and
# the link fails ("could not find native static library `opus`"). CMake reads
# the toolchain file from the environment for every project, so all of them
# install to lib (lectover-sys sets the same for its own build). A plain
# variable, not a cache entry: a PATH cache entry is made absolute against
# the build directory, and ggml then installs into build/lib.
RUN printf 'set(CMAKE_INSTALL_LIBDIR lib)\n' > /usr/local/share/lectover-libdir.cmake
ENV CMAKE_TOOLCHAIN_FILE=/usr/local/share/lectover-libdir.cmake

ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
RUN if ! command -v cargo >/dev/null; then \
      curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable \
      && chmod -R a+w "$RUSTUP_HOME" "$CARGO_HOME"; \
    fi

WORKDIR /src
