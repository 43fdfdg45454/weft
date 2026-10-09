#!/usr/bin/env bash
# Construye el invitado de prueba: un initramfs minimo con busybox. Uso: ci/make-guest.sh <busybox estatico> <salida>
set -euo pipefail
BB="$1"; OUT="$(realpath -m "$2")"
W="$(mktemp -d)"; trap 'rm -rf "$W"' EXIT
mkdir -p "$W"/{bin,proc,sys,dev,tmp}
cp "$BB" "$W/bin/busybox"
cat > "$W/init" <<'INIT'
#!/bin/busybox sh
/bin/busybox --install -s /bin
mount -t proc proc /proc
mount -t sysfs sys /sys
mount -t devtmpfs dev /dev 2>/dev/null
echo "WEFT-BOOT-OK kernel=$(uname -r)"
# lo que se escribe con el teclado emulado (terminal virtual 1) se repite en la consola serie
( while read l; do echo "WEFT-KEY:$l" > /dev/ttyS0; done < /dev/tty1 ) &
# interprete de ordenes en la consola serie
exec setsid cttyhack sh
INIT
chmod +x "$W/init"
mkdir -p "$(dirname "$OUT")"
( cd "$W" && find . | cpio -o -H newc --quiet | gzip -9 > "$OUT" )
echo "invitado: $(du -k "$OUT" | cut -f1) KiB"
