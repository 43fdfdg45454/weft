#!/usr/bin/env bash
# Construye el banco de pruebas (sin Gradle) con la biblioteca nativa para UNA arquitectura.
# Uso: ci/build-apk.sh <arm64|x86_64> <salida.apk>    Necesita ANDROID_HOME (build-tools, platforms) y un NDK.
set -euo pipefail
ARCH="$1"; OUT="$(realpath -m "$2")"; HERE="$(cd "$(dirname "$0")/.." && pwd)"; SRC="$HERE/banco"
case "$ARCH" in
  arm64) ABI=arm64-v8a; TRIPLE=aarch64-linux-android24; XFLAGS="-mno-outline-atomics" ;;   # LL/SC en linea (LDAXR/STLXR) en el banco
  x86_64) ABI=x86_64; TRIPLE=x86_64-linux-android24; XFLAGS="" ;;
  *) echo "arquitectura desconocida: $ARCH" >&2; exit 2 ;;
esac
BT="$(ls -d "$ANDROID_HOME"/build-tools/* | sort -V | tail -n1)"
JAR="$(ls -d "$ANDROID_HOME"/platforms/android-* | sort -V | tail -n1)/android.jar"
NDK="${ANDROID_NDK_LATEST_HOME:-${ANDROID_NDK_HOME:-$(ls -d "$ANDROID_HOME"/ndk/* | sort -V | tail -n1)}}"
CC="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin/$TRIPLE-clang"
W="$(mktemp -d)"; trap 'rm -rf "$W"' EXIT
mkdir -p "$W/classes" "$W/apk/lib/$ABI"
javac -source 8 -target 8 -nowarn -Xlint:-options -cp "$JAR" -d "$W/classes" $(find "$SRC/src" -name '*.java')
"$BT/d8" --lib "$JAR" --min-api 24 --output "$W/apk" $(find "$W/classes" -name '*.class')
"$CC" -shared -O2 -Wall $XFLAGS -o "$W/apk/lib/$ABI/libbanco.so" "$SRC/jni/banco.c" "$SRC/jni/bench.c" -lEGL -lGLESv2 -landroid -llog -lm
# NativeActivity con biblioteca y punto de entrada propios (android.app.lib_name/func_name del manifiesto): no debe
# exportar ANativeActivity_onCreate, para que arrancar demuestre que se uso el nombre declarado
"$CC" -shared -O2 -Wall -fvisibility=hidden $XFLAGS -o "$W/apk/lib/$ABI/libbanconativa.so" "$SRC/jni/nativa.c" -landroid -llog
SYMS="$("$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-readelf" --dyn-syms "$W/apk/lib/$ABI/libbanconativa.so")"
grep -qw banco_nativa_crear <<<"$SYMS" || { echo "libbanconativa.so no exporta banco_nativa_crear" >&2; exit 1; }
if grep -qw ANativeActivity_onCreate <<<"$SYMS"; then echo "libbanconativa.so no debe exportar ANativeActivity_onCreate" >&2; exit 1; fi
"$BT/aapt2" link -o "$W/base.apk" --manifest "$SRC/AndroidManifest.xml" -I "$JAR"
( cd "$W/apk" && zip -q -r "$W/base.apk" classes.dex lib )
"$BT/zipalign" -f 4 "$W/base.apk" "$W/aligned.apk"
keytool -genkeypair -keystore "$W/k.jks" -storepass android -keypass android -alias k -keyalg RSA -keysize 2048 -validity 1000 -dname "CN=banco" 2>/dev/null
"$BT/apksigner" sign --ks "$W/k.jks" --ks-pass pass:android --key-pass pass:android --out "$OUT" "$W/aligned.apk"
unzip -l "$OUT" | grep -E "lib/|classes"; echo "apk ($ARCH): $OUT"
