#!/bin/bash
# Compila libheddle.so (x86_64 Android) de una version de 43fdfdg45454/heddle, como lo hace su propio CI: la biblioteca
# guest de libm (bionic en el commit de guest/BIONIC_COMMIT) y la biblioteca para x86_64-linux-android con el NDK del
# ejecutor. Uso: ci/build-heddle.sh REF SALIDA.so
set -euo pipefail
ref="$1"; salida="$2"
case "$ref" in -*|*[!A-Za-z0-9._/-]*) echo "build-heddle: referencia no valida: $ref" >&2; exit 2 ;; esac
dir="$(mktemp -d)"
git clone -q https://github.com/43fdfdg45454/heddle "$dir/heddle"
git -C "$dir/heddle" checkout -q "$ref"
echo "heddle: $(git -C "$dir/heddle" log --oneline -1)"
NDK="${ANDROID_NDK_LATEST_HOME:-$ANDROID_NDK_HOME}"
git clone -q --filter=blob:none --sparse https://github.com/aosp-mirror/platform_bionic "$dir/bionic"
git -C "$dir/bionic" checkout -q "$(cat "$dir/heddle/guest/BIONIC_COMMIT")"
git -C "$dir/bionic" sparse-checkout set libm libc/include libc/private
mkdir -p "$dir/heddle/guest/out"
( cd "$dir/heddle" && NDK="$NDK" guest/build.sh "$dir/bionic" guest/out/libheddle_guest.so )
rustup target add x86_64-linux-android > /dev/null
export CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin/x86_64-linux-android26-clang"
export CC_x86_64_linux_android="$CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER"
export HEDDLE_VERSION="$ref-$(git -C "$dir/heddle" rev-parse --short HEAD)"
( cd "$dir/heddle" && cargo build -q --release --target x86_64-linux-android --lib )
cp "$dir/heddle/target/x86_64-linux-android/release/libheddle.so" "$salida"
ls -l "$salida"
