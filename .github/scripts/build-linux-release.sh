#!/usr/bin/env bash
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
# The floor is squeezed from BOTH sides and the second side used to be OpenSSL:
# a base old enough to give glibc 2.28 (rockylinux:8, almalinux:8,
# manylinux_2_28) ships OpenSSL 1.1, so it emitted a binary needing
# `libssl.so.1.1`, which does NOT exist on Ubuntu 22.04, Debian 12 or RHEL 9.
#
#   almalinux:9   glibc 2.34   <- lowest glibc that still has OpenSSL 3
#   ubuntu:22.04  glibc 2.35   <- lowest OpenSSL-3 base with arm64 multiarch
#
# #1128 REVERSED THE OPENSSL HALF OF THAT, DELIBERATELY. This block used to end
# "going below 2.34 requires vendoring OpenSSL, which forfeits distro security
# updates for TLS. That is a product decision, not a build-container choice."
# The product decision has now been made, because the premise under it was
# wrong.
#
# That note (and the dependency comment below) attributed libssl to
# `reqwest -> openssl-sys`. reqwest is declared `default-features = false,
# features = ["rustls-tls"]`; it has never linked OpenSSL here. Every provider
# connection, and lettre's SMTP leg, are rustls. OpenSSL had exactly ONE entry
# point into this workspace: `imap` 2.x, whose `connect` / `connect_starttls`
# take a concrete `native_tls::TlsConnector` and expose no rustls path. So
# "distro security updates for TLS" only ever covered the IMAP inbound leg of
# the email channel -- not the product's TLS.
#
# Against that: the shipped artifact could not start AT ALL on node:22-slim,
# which ships neither libssl.so.3 nor libdbus-1.so.3, and which is an entirely
# ordinary landing place for a Node-distributed CLI. The user got a dynamic
# linker error that does not look like a Wayland problem.
#
# OpenSSL (imap -> native-tls) and libdbus (keyring -> dbus-secret-service) are
# now vendored and statically linked. THE COST IS REAL AND IS ACCEPTED: an
# OpenSSL CVE affecting the IMAP leg now needs a wayland-core release rather
# than the user's `apt upgrade`. `cargo audit` runs in CI and RustSec issues
# advisories against `openssl-src`, so an unpatched vendored OpenSSL fails the
# build -- but that is a slower loop than distro patching, and it is the price
# of an artifact that starts on a stock slim image.
#
# The glibc floor is UNCHANGED at 2.34. Vendoring removes the OpenSSL squeeze,
# not the glibc one; lowering the floor is a separate decision with its own
# evidence, and this lane did not make it.
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
# feature) only. libssl-dev and libdbus-1-dev were dropped in #1128: OpenSSL
# (imap -> native-tls) and libdbus (keyring -> dbus-secret-service) are now
# vendored, so `openssl-sys` never consults pkg-config and `libdbus-sys` builds
# the bundled C source with `cc`. Their ABSENCE is the proof: if either edge is
# ever un-vendored, the build fails HERE rather than shipping an artifact that
# cannot start on a slim image. `perl` is required by the vendored OpenSSL
# build and is already installed below.
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
