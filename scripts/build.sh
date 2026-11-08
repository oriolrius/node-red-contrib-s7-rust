#!/bin/sh
# Builds the Rust Node-API addon (crates/s7napi). Linux x64 only.
#   sh scripts/build.sh dev         release addon for this host  -> bin/linux-x64-<gnu|musl>/s7napi.node
#   sh scripts/build.sh test-addon  debug addon + fake PLCs      -> test/.bin/s7napi-test.node
#   sh scripts/build.sh release     both published binaries:
#                                     gnu  (glibc >= 2.28, cargo-zigbuild) -> bin/linux-x64-gnu/
#                                     musl (Alpine, e.g. nodered/node-red)  -> bin/linux-x64-musl/
# release needs cargo-zigbuild: "pip install --require-hashes --no-deps -r .github/requirements/zigbuild.txt" or uv (uvx is used when it is not on PATH).
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

# no local paths (home, user name, checkout) in the published binaries
remap="--remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo --remap-path-prefix=${RUSTUP_HOME:-$HOME/.rustup}=/rustup --remap-path-prefix=$root=/build"

zigbuild() {
  if command -v cargo-zigbuild >/dev/null 2>&1; then cargo-zigbuild "$@"; else uvx --from cargo-zigbuild cargo-zigbuild "$@"; fi
}

put() { # file dir
  mkdir -p "bin/$2"
  cp "$1" "bin/$2/s7napi.node"
  echo "built bin/$2/s7napi.node"
}

libc() {
  if [ "$(node -p '!!process.report.getReport().header.glibcVersionRuntime')" = true ]; then echo gnu; else echo musl; fi
}

case "${1:-dev}" in
dev)
  cargo build --release -p s7napi
  put target/release/libs7napi.so "linux-x64-$(libc)"
  ;;
test-addon)
  # the same addon plus in-process fake PLCs (feature "fake"); own target dir so it never mixes with release
  cargo build -p s7napi --features fake --target-dir target/fake
  mkdir -p test/.bin
  cp target/fake/debug/libs7napi.so test/.bin/s7napi-test.node
  echo "built test/.bin/s7napi-test.node"
  ;;
release)
  rustup target add x86_64-unknown-linux-musl >/dev/null 2>&1 || true
  RUSTFLAGS="$remap" zigbuild build --release -p s7napi --target x86_64-unknown-linux-gnu.2.28
  put target/x86_64-unknown-linux-gnu/release/libs7napi.so linux-x64-gnu
  # cdylib on musl: dynamic libc (the host's musl), not a static CRT
  RUSTFLAGS="$remap -C target-feature=-crt-static" zigbuild build --release -p s7napi --target x86_64-unknown-linux-musl --target-dir target/musl
  put target/musl/x86_64-unknown-linux-musl/release/libs7napi.so linux-x64-musl
  ;;
*)
  echo "usage: $0 dev|test-addon|release" >&2
  exit 2
  ;;
esac
