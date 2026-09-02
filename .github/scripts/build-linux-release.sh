#!/usr/bin/env bash
# ADMISSION: caller-decides -- this is a build step inside a platform matrix,
# not a gate. Its callers select it by `runner.os`, which is the point.
#
# Build the release binary for a Linux target inside a PINNED container.
#
# Run by .github/workflows/release.yml, and runnable by hand on any Docker host
# so the shipped floor can be re-measured outside CI with the same commands:
#
#   .github/scripts/build-linux-release.sh almalinux:9 x86_64-unknown-linux-gnu
#   .github/scripts/build-linux-release.sh ubuntu:22.04 aarch64-unknown-linux-gnu
#
# WHY A CONTAINER AT ALL
# ----------------------
# A binary's glibc floor is a property of the BUILD SYSROOT, not of the source.
# Nothing in the repo pinned it, so it silently tracked whatever `ubuntu-latest`
# resolved to. When that became 24.04 (glibc 2.39) the published Linux binaries
# stopped being able to execute on Ubuntu 22.04 (2.35), Debian 12 (2.36) and
# RHEL 9 / Rocky 9 / Amazon Linux 2023 (2.34) — i.e. on most deployed server
# Linux. Pinning the container pins the floor.
#
# WHY NOT AN EVEN OLDER BASE
# --------------------------
# The floor used to be squeezed from BOTH sides, and the second side was
# OpenSSL: a base old enough to give glibc 2.28 (rockylinux:8, almalinux:8,
# manylinux_2_28) ships OpenSSL 1.1, so it emitted a binary needing
# `libssl.so.1.1`, which does NOT exist on Ubuntu 22.04, Debian 12 or RHEL 9.
#
#   almalinux:9   glibc 2.34
#   ubuntu:22.04  glibc 2.35   <- lowest base with arm64 multiarch
#
# THAT SQUEEZE IS GONE: NOTHING HERE LINKS OPENSSL AT ALL. The old note
# attributed libssl to `reqwest -> openssl-sys`, which was wrong -- reqwest is
# declared `default-features = false, features = ["rustls-tls"]` and has never
# linked OpenSSL here. OpenSSL had exactly ONE entry point into this workspace:
# `imap` 2.x's default `tls` feature, i.e. native-tls, used by the IMAP inbound
# leg of the email channel.
#
# #1128 closed that by VENDORING OpenSSL into the artifact. This branch closes
# it by REMOVING the dependency: `wcore-channel-email` takes `imap` with
# `default-features = false` and drives both TLS legs on the rustls stack the
# rest of the workspace (reqwest, lettre SMTP) already used.
# `cargo tree -i openssl-sys --target all -e all` now prints nothing.
#
# Removing it beats vendoring on three counts: no `make`/perl requirement in
# every image that builds this workspace (the CI image has neither, and the
# vendored build reddened `CI (linux-containerized)`), no OpenSSL CVE ownership
# -- an advisory would have needed a wayland-core release instead of the user's
# `apt upgrade` -- and one TLS implementation in the product instead of two.
#
# libdbus (keyring -> dbus-secret-service) IS still vendored, on its own merits:
# `libdbus-sys/vendored` is a `cc` build of the bundled C source, so it needs
# only the C compiler every image already has, and it is what keeps
# `libdbus-1.so.3` out of DT_NEEDED without dropping the Linux keyring backend.
#
# The glibc floor is UNCHANGED at 2.34. Lowering it is a separate decision with
# its own evidence, and this lane did not make it.
set -euo pipefail

IMAGE="${1:?usage: build-linux-release.sh <image> <target-triple>}"
TARGET="${2:?usage: build-linux-release.sh <image> <target-triple>}"
WORKDIR="${GITHUB_WORKSPACE:-$PWD}"
RUST_VERSION="$(sed -n 's/^channel = "\(.*\)"/\1/p' "${WORKDIR}/rust-toolchain.toml")"

# crates/wcore-cli/build.rs REFUSES a release build with no attributable source
# identity, and resolves it from `git rev-parse HEAD` when
# WAYLAND_BUILD_SOURCE_SHA is unset. That git call CANNOT work from inside this
# container: the container runs as root over a workspace owned by the runner
# user, and git rejects that with "detected dubious ownership". Measured
# 2026-07-30 — both targets failed at build.rs:11 before this was passed in.
# So the SHA is resolved on the HOST, where git works, and injected.
SOURCE_SHA="${WAYLAND_BUILD_SOURCE_SHA:-${GITHUB_SHA:-$(git -C "${WORKDIR}" rev-parse HEAD)}}"
if ! printf '%s' "${SOURCE_SHA}" | grep -Eq '^[0-9a-f]{40}$'; then
  echo "::error::WAYLAND_BUILD_SOURCE_SHA must be 40 lowercase hex chars, got '${SOURCE_SHA}'" >&2
  exit 1
fi

echo "building ${TARGET} in ${IMAGE} (rust ${RUST_VERSION}, source ${SOURCE_SHA})"

docker run --rm \
  -v "${WORKDIR}":"${WORKDIR}" \
  -w "${WORKDIR}" \
  -e TARGET="${TARGET}" \
  -e RUST_VERSION="${RUST_VERSION}" \
  -e WAYLAND_BUILD_SOURCE_SHA="${SOURCE_SHA}" \
  -e CARGO_TERM_COLOR=never \
  -e CARGO_NET_RETRY=10 \
  -e CARGO_HTTP_TIMEOUT=600 \
  "${IMAGE}" bash -euo pipefail -c '
# ---- system dependencies ------------------------------------------------
# libseccomp-dev (wcore-sandbox) and libasound2-dev (cpal -> alsa-sys, voice
# feature) only. libssl-dev and libdbus-1-dev were dropped in #1128 and stay
# dropped: nothing in the workspace links OpenSSL any more (the `imap`
# native-tls edge is gone, see the header), and `libdbus-sys` builds the bundled
# C source with `cc`. Their ABSENCE is the proof: if either edge comes back the
# build fails HERE rather than shipping an artifact that cannot start on a slim
# image.
if command -v apt-get >/dev/null 2>&1; then
  export DEBIAN_FRONTEND=noninteractive
  if [ "$TARGET" = "aarch64-unknown-linux-gnu" ]; then
    . /etc/os-release
    # arm64 packages are served by ports.ubuntu.com, never archive.ubuntu.com,
    # so the default sources must be pinned to amd64 and a ports source added.
    if [ -f /etc/apt/sources.list.d/ubuntu.sources ]; then
      # Ubuntu 24.04+ uses deb822; editing sources.list there is a silent no-op
      # and apt then 404s asking archive.ubuntu.com for binary-arm64.
      sed -i "/^Types:/a Architectures: amd64" /etc/apt/sources.list.d/ubuntu.sources
      cat > /etc/apt/sources.list.d/arm64.sources <<EOF
Types: deb
URIs: http://ports.ubuntu.com/ubuntu-ports
Suites: ${UBUNTU_CODENAME} ${UBUNTU_CODENAME}-updates
Components: main universe
Architectures: arm64
Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg
EOF
    else
      sed -i "s|^deb |deb [arch=amd64] |g" /etc/apt/sources.list
      cat >> /etc/apt/sources.list <<EOF
deb [arch=arm64] http://ports.ubuntu.com/ubuntu-ports ${UBUNTU_CODENAME} main universe
deb [arch=arm64] http://ports.ubuntu.com/ubuntu-ports ${UBUNTU_CODENAME}-updates main universe
EOF
    fi
    dpkg --add-architecture arm64
  fi
  apt-get update -qq
  apt-get install -y -qq --no-install-recommends \
    build-essential curl ca-certificates pkg-config cmake perl git file binutils
  if [ "$TARGET" = "aarch64-unknown-linux-gnu" ]; then
    apt-get install -y -qq --no-install-recommends \
      gcc-aarch64-linux-gnu g++-aarch64-linux-gnu \
      libseccomp-dev:arm64 libasound2-dev:arm64
  else
    apt-get install -y -qq --no-install-recommends \
      libseccomp-dev libasound2-dev
  fi
elif command -v dnf >/dev/null 2>&1; then
  dnf -y -q install dnf-plugins-core >/dev/null 2>&1 || true
  # alsa-lib-devel lives in CRB on the el9 rebuilds (PowerTools on el8).
  dnf config-manager --set-enabled crb >/dev/null 2>&1 \
    || dnf config-manager --set-enabled powertools >/dev/null 2>&1 || true
  # `curl` is deliberately NOT requested: el9 ships curl-minimal, which already
  # provides /usr/bin/curl, and naming `curl` is a hard dnf conflict.
  dnf -y -q install gcc gcc-c++ make pkgconfig cmake perl \
    libseccomp-devel alsa-lib-devel git file binutils
else
  echo "unsupported base image: no apt-get and no dnf" >&2
  exit 1
fi

echo "container glibc: $(ldd --version | head -1)"

# ---- rust, pinned to rust-toolchain.toml --------------------------------
export CARGO_HOME=${CARGO_HOME:-/usr/local/cargo}
export RUSTUP_HOME=${RUSTUP_HOME:-/usr/local/rustup}
export PATH=$CARGO_HOME/bin:$PATH
curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path \
  --default-toolchain "$RUST_VERSION" --profile minimal
rustup target add "$TARGET"
cargo --version
rustc --version

# ---- cross-compilation wiring for aarch64 -------------------------------
if [ "$TARGET" = "aarch64-unknown-linux-gnu" ]; then
  export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
  export CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
  export CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++
  export AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar
  # pkg-config refuses cross-arch queries unless told otherwise.
  export PKG_CONFIG_ALLOW_CROSS=1
  export PKG_CONFIG_LIBDIR=/usr/lib/aarch64-linux-gnu/pkgconfig
  export PKG_CONFIG_SYSROOT_DIR=/
fi

cargo build --release --target "$TARGET" -p wcore-cli
'

echo "built: ${WORKDIR}/target/${TARGET}/release/wayland-core"
