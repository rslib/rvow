#!/usr/bin/env bash
# Builds the static rvow binary for a Linux musl target in the cross image the
# release uses, with the toolchain pinned in rust-toolchain.toml. RUSTFLAGS,
# when set, reaches the build.
# Usage: build-musl.sh TARGET OUT_DIR   (prints the binary path last)
set -euo pipefail
target=$1
out=$2
case $target in
  x86_64-unknown-linux-musl) image=messense/rust-musl-cross:x86_64-musl ;;
  aarch64-unknown-linux-musl) image=messense/rust-musl-cross:aarch64-musl ;;
  *)
    echo "no cross image for $target" >&2
    exit 2
    ;;
esac
toolchain=$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)
mkdir -p "$out/target" "$out/registry"
flags=()
[ -n "${RUSTFLAGS:-}" ] && flags=(-e "RUSTFLAGS=$RUSTFLAGS")
# The target and registry mounts keep root-owned files out of the checkout.
docker run --rm \
  -v "$PWD":/src -w /src \
  -v "$out/target":/src/target \
  -v "$out/registry":/root/.cargo/registry \
  ${flags[@]+"${flags[@]}"} \
  "$image" \
  sh -c "rustup toolchain install $toolchain --profile minimal --target $target && cargo build --release --locked --target $target -p rvow"
bin="$out/target/$target/release/rvow"
file "$bin" >&2
if ! file "$bin" | grep -Eq 'statically linked|static-pie linked'; then
  echo "$bin is not static" >&2
  exit 1
fi
echo "$bin"
