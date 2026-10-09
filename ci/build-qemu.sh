#!/usr/bin/env bash
# Compila QEMU con GPU gfxstream (rutabaga). Uso: ci/build-qemu.sh <etiqueta de QEMU> <directorio de salida>
# Pensado para Ubuntu 24.04. Deja en <salida>: bin/qemu-system-x86_64, lib/ (gfxstream, rutabaga), share/qemu.
# Todo el trabajo (clones, compilaciones y registros *.log) queda en el directorio de trabajo WORK (por defecto ./_qemu_build, que
# ignoran git y el manifiesto Flatpak), nunca en el directorio actual.
set -uo pipefail
REF="$1"; OUT="$2"; W="${WORK:-$PWD/_qemu_build}"; J="$(nproc)"
# commits de gfxstream y rutabaga_gfx: DEBEN coincidir con los del manifiesto flatpak/plantillas/app.yml.in (y con el env de
# .github/workflows/ci.yml, que los pasa por el entorno y los lleva en la clave de la cache de la tarea qemu-ubuntu)
GFXSTREAM_REF="${GFXSTREAM_REF:-07ee40efb0e7037a9a9b7fe59071e7c4997e7cbe}"
RUTABAGA_REF="${RUTABAGA_REF:-ced6af5b1135173c269a037e06213e8a263de271}"
step() { echo; echo "=== $*"; }
fail() { echo "FALLO: $*"; exit 1; }
mkdir -p "$W" "$OUT" || fail "crear $W y $OUT"
W="$(cd "$W" && pwd)"; OUT="$(cd "$OUT" && pwd)"; P="$W/prefix"; mkdir -p "$P"
cd "$W" || fail "entrar en $W"
# clonar URL DIR COMMIT: baja solo ese commit (git clone --depth 1 no sirve para un commit concreto)
clonar() { git init -q "$2" && git -C "$2" fetch -q --depth 1 "$1" "$3" && git -C "$2" checkout -q FETCH_HEAD; }
export PKG_CONFIG_PATH="$P/lib/pkgconfig:$P/lib/x86_64-linux-gnu/pkgconfig:${PKG_CONFIG_PATH:-}"
export LD_LIBRARY_PATH="$P/lib:$P/lib/x86_64-linux-gnu:${LD_LIBRARY_PATH:-}"
export CMAKE_PREFIX_PATH="$P"

step "dependencias"
sudo apt-get update -qq
sudo apt-get install -y -qq build-essential git ninja-build meson cmake pkg-config python3-venv python3-pip flex bison \
  libglib2.0-dev libpixman-1-dev libslirp-dev libgtk-3-dev libsdl2-dev libepoxy-dev libdrm-dev libgbm-dev libegl-dev libgl-dev \
  libvulkan-dev libcap-ng-dev libattr1-dev libaio-dev libpulse-dev libpipewire-0.3-dev libusb-1.0-0-dev libfdt-dev libseccomp-dev \
  libx11-dev libxext-dev libwayland-dev wayland-protocols libxkbcommon-dev libvte-2.91-dev libglm-dev libstb-dev curl patchelf || fail "apt"
rustc --version; cargo --version; meson --version; cmake --version | head -n1

step "gfxstream (renderizador del anfitrion) $GFXSTREAM_REF"
[ -d gfxstream ] || clonar https://github.com/google/gfxstream gfxstream "$GFXSTREAM_REF" || fail "clonar gfxstream en $GFXSTREAM_REF"
[ "$(git -C gfxstream rev-parse HEAD)" = "$GFXSTREAM_REF" ] || fail "gfxstream no esta en el commit $GFXSTREAM_REF (borra $W/gfxstream)"
meson setup gfxstream/host-build gfxstream --prefix="$P" --libdir=lib -Dbuildtype=release > gfx-config.log 2>&1 || { tail -n 40 gfx-config.log; fail "configurar gfxstream"; }
meson install -C gfxstream/host-build > gfx-build.log 2>&1 || { tail -n 60 gfx-build.log; fail "compilar gfxstream"; }
git -C gfxstream log -1 --format='gfxstream %h %cs'

step "rutabaga (interfaz entre QEMU y gfxstream) $RUTABAGA_REF"
[ -d rutabaga_gfx ] || clonar https://github.com/magma-gpu/rutabaga_gfx rutabaga_gfx "$RUTABAGA_REF" || fail "clonar rutabaga_gfx en $RUTABAGA_REF"
[ "$(git -C rutabaga_gfx rev-parse HEAD)" = "$RUTABAGA_REF" ] || fail "rutabaga_gfx no esta en el commit $RUTABAGA_REF (borra $W/rutabaga_gfx)"
git -C rutabaga_gfx log -1 --format='rutabaga_gfx %h %cs'
# primero con meson (como lo documenta el proyecto); si la version de meson no alcanza, con cargo y una instalacion a mano
if ( cd rutabaga_gfx && meson setup build . --prefix="$P" --libdir=lib -Dbuildtype=release -Dfeatures=gfxstream -Dffi=true && meson install -C build ) > rutabaga-build.log 2>&1; then
  echo "rutabaga: compilado con meson"
else
  tail -n 25 rutabaga-build.log
  echo "rutabaga: meson no pudo; se compila con cargo"
  ( cd rutabaga_gfx/ffi && cargo build --release --features gfxstream ) > rutabaga-cargo.log 2>&1 || { tail -n 60 rutabaga-cargo.log; fail "compilar rutabaga con cargo"; }
  SO="$(find rutabaga_gfx -name 'librutabaga_gfx_ffi.so' -path '*release*' | head -n1)"
  [ -n "$SO" ] || fail "no se genero librutabaga_gfx_ffi.so"
  mkdir -p "$P/lib/pkgconfig" "$P/include/rutabaga_gfx"
  cp "$SO" "$P/lib/librutabaga_gfx_ffi.so"
  cp rutabaga_gfx/ffi/src/include/rutabaga_gfx_ffi.h "$P/include/rutabaga_gfx/"
  V="$(grep -m1 '^version' rutabaga_gfx/ffi/Cargo.toml | cut -d'"' -f2)"
  printf 'prefix=%s\nlibdir=${prefix}/lib\nincludedir=${prefix}/include\n\nName: rutabaga_gfx_ffi\nDescription: C FFI bindings to Rutabaga VGI\nVersion: %s\nLibs: -L${libdir} -lrutabaga_gfx_ffi\nCflags: -I${includedir}\n' "$P" "${V:-0.1.85}" > "$P/lib/pkgconfig/rutabaga_gfx_ffi.pc"
fi
ls "$P/lib" | grep -i -E "rutabaga|gfxstream|aemu"; pkg-config --modversion rutabaga_gfx_ffi gfxstream_backend 2>&1

step "QEMU $REF"
[ -d qemu ] || git clone -q --depth 1 -b "$REF" https://gitlab.com/qemu-project/qemu.git || fail "clonar qemu"
mkdir -p qemu/build && cd qemu/build
../configure --prefix="$OUT" --target-list=x86_64-softmmu --enable-kvm --enable-rutabaga-gfx --enable-gtk --enable-sdl --enable-opengl --enable-slirp \
  --enable-vhost-user --enable-virtfs --disable-docs --disable-werror > "$W/qemu-config.log" 2>&1 || { tail -n 60 "$W/qemu-config.log"; fail "configurar qemu"; }
grep -i -E "rutabaga|gtk support|sdl support|opengl|slirp" "$W/qemu-config.log" | head -n 12
make -j "$J" > "$W/qemu-build.log" 2>&1 || { tail -n 60 "$W/qemu-build.log"; fail "compilar qemu"; }
make install > /dev/null 2>&1 || fail "instalar qemu"

step "paquete"
mkdir -p "$OUT/lib"
cp -a "$P"/lib/*.so* "$OUT/lib/" 2>/dev/null
# QEMU pide la biblioteca por su nombre con version
[ -e "$OUT/lib/librutabaga_gfx_ffi.so.0" ] || ln -s librutabaga_gfx_ffi.so "$OUT/lib/librutabaga_gfx_ffi.so.0"
patchelf --set-rpath '$ORIGIN/../lib' "$OUT/bin/qemu-system-x86_64" || fail "rpath"
ls -l "$OUT/bin" "$OUT/lib" | head -n 40
echo "=== listo"
