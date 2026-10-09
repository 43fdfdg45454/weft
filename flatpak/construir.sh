#!/usr/bin/env bash
# Construye el paquete .flatpak de weft. Uso:  bash flatpak/construir.sh [SALIDA.flatpak]
#
# IDENTIFICADOR DE LA APLICACION (provisional): se cambia SOLO aqui (o con la variable de entorno APP_ID). Las plantillas de
# flatpak/plantillas/ llevan @APP_ID@ y este guion genera flatpak/generado/<APP_ID>.{yml,desktop,metainfo.xml,svg}.
APP_ID="${APP_ID:-io.github._43fdfdg45454.weft}"
#
# FLATPAK_BUILDER_EXTRA="--disable-rofiles-fuse" agrega opciones a flatpak-builder (el CI lo usa dentro de un contenedor sin FUSE).
# Con SOLO_GENERAR=1 deja los archivos generados y no construye (lo usa el CI para validarlos).
# Herramientas: flatpak y org.flatpak.Builder (flatpak install flathub org.flatpak.Builder), mas el runtime, el SDK y la extension de
# Rust de la version del manifiesto (flatpak-builder los instala con --install-deps-from=flathub).
set -euo pipefail
AQUI="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SALIDA="${1:-$AQUI/../weft.flatpak}"
GEN="$AQUI/generado"
rm -rf "$GEN"; mkdir -p "$GEN"
# VERSION: la que calcula GitVersion en el CI (WEFT_VERSION); en local, la de Cargo.toml. Va al binario y al metainfo,
# con la fecha del commit.
VERSION="${WEFT_VERSION:-$(sed -n 's/^version = "\(.*\)"$/\1/p' "$AQUI/../Cargo.toml" | head -n1)}"
FECHA="$(git -C "$AQUI/.." log -1 --format=%cs 2>/dev/null || date -u +%F)"
case "$VERSION" in *[!0-9A-Za-z.+-]*|"") echo "version no valida: '$VERSION'" >&2; exit 1 ;; esac
SUST=(-e "s/@APP_ID@/$APP_ID/g" -e "s/@VERSION@/$VERSION/g" -e "s/@FECHA@/$FECHA/g")
sed "${SUST[@]}" "$AQUI/plantillas/app.yml.in" > "$GEN/$APP_ID.yml"
sed "${SUST[@]}" "$AQUI/plantillas/app.desktop.in" > "$GEN/$APP_ID.desktop"
sed "${SUST[@]}" "$AQUI/plantillas/app.metainfo.xml.in" > "$GEN/$APP_ID.metainfo.xml"
cp "$AQUI/plantillas/app.svg" "$GEN/$APP_ID.svg"
echo "generado en $GEN para $APP_ID $VERSION ($FECHA)"
[ "${SOLO_GENERAR:-0}" = 1 ] && exit 0
ESTADO="${FLATPAK_BUILDER_STATE:-$AQUI/../_flatpak-state}"
CONSTRUCCION="${FLATPAK_BUILDER_DIR:-$AQUI/../_flatpak-build}"
REPO="${FLATPAK_BUILDER_REPO:-$AQUI/../_flatpak-repo}"
# --repo deja el resultado en un repositorio ostree y de ahi sale el paquete .flatpak
if command -v flatpak-builder > /dev/null; then FB=(flatpak-builder); else FB=(flatpak run --command=flatpak-builder org.flatpak.Builder); fi
"${FB[@]}" --user --force-clean ${FLATPAK_BUILDER_EXTRA:-} --install-deps-from=flathub --state-dir="$ESTADO" --repo="$REPO" "$CONSTRUCCION" "$GEN/$APP_ID.yml"
# --runtime-repo: al instalar el paquete, Flatpak baja el runtime org.freedesktop.Platform de Flathub si falta (lo unico que baja)
flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo "$REPO" "$SALIDA" "$APP_ID"
ls -l "$SALIDA"
sha256sum "$SALIDA"
