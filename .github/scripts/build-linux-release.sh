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
# The floor is squeezed from BOTH sides and the second side is easy to miss.
# The binary carries `NEEDED libssl.so.3` (openssl-sys + native-tls are in the
# dependency graph). A base old enough to give glibc 2.28 — rockylinux:8,
# almalinux:8, manylinux_2_28 — ships OpenSSL 1.1, so it would emit a binary
# needing `libssl.so.1.1`, which does NOT exist on Ubuntu 22.04, Debian 12 or
# RHEL 9. That trades a glibc break for an OpenSSL break on precisely the
# distros this is meant to reach, and is a net regression.
#
#   almalinux:9   glibc 2.34 + libssl.so.3   <- lowest glibc that still has OpenSSL 3
#   ubuntu:22.04  glibc 2.35 + libssl.so.3   <- lowest OpenSSL-3 base with arm64 multiarch
#
# Going below 2.34 requires vendoring OpenSSL, which forfeits distro security
# updates for TLS. That is a product decision, not a build-container choice.
set -euo pipefail

IMAGE="${1:?usage: build-linux-release.sh <image> <target-triple>}"
TARGET="${2:?usage: build-linux-release.sh <image> <target-triple>}"
WORKDIR="${GITHUB_WORKSPACE:-$PWD}"
RUST_VERSION="$(sed -n 's/^channel = "\(.*\)"/\1/p' "${WORKDIR}/rust-toolchain.toml")"

echo "building ${TARGET} in ${IMAGE} (rust ${RUST_VERSION})"

docker run --rm \
  -v "${WORKDIR}":"${WORKDIR}" \
  -w "${WORKDIR}" \
  -e TARGET="${TARGET}" \
  -e RUST_VERSION="${RUST_VERSION}" \
  -e CARGO_TERM_COLOR=never \
  -e CARGO_NET_RETRY=10 \
  -e CARGO_HTTP_TIMEOUT=600 \
  "${IMAGE}" bash -euo pipefail -c '
# ---- system dependencies ------------------------------------------------
# Same set the native runner used to install: libdbus-1-dev (keyring ->
# libdbus-sys), libssl-dev (reqwest -> openssl-sys), libseccomp-dev
# (wcore-sandbox), libasound2-dev (cpal -> alsa-sys, voice feature).
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
      libdbus-1-dev:arm64 libseccomp-dev:arm64 libasound2-dev:arm64 libssl-dev:arm64
  else
    apt-get install -y -qq --no-install-recommends \
      libdbus-1-dev libseccomp-dev libasound2-dev libssl-dev
  fi
elif command -v dnf >/dev/null 2>&1; then
  dnf -y -q install dnf-plugins-core >/dev/null 2>&1 || true
  # alsa-lib-devel lives in CRB on the el9 rebuilds (PowerTools on el8).
  dnf config-manager --set-enabled crb >/dev/null 2>&1 \
    || dnf config-manager --set-enabled powertools >/dev/null 2>&1 || true
  # `curl` is deliberately NOT requested: el9 ships curl-minimal, which already
  # provides /usr/bin/curl, and naming `curl` is a hard dnf conflict.
  dnf -y -q install gcc gcc-c++ make pkgconfig cmake perl \
    dbus-devel libseccomp-devel alsa-lib-devel openssl-devel git file binutils
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
