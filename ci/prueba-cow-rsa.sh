#!/usr/bin/env bash
# Disco de copia en escritura (disk.cow=si) y autenticacion RSA del adb propio contra un adbd real (ro.adb.secure=1), en
# Fedora (QEMU del sistema, sin ventana). Un arranque de Android con el guion de usuario (scripts/dev-fedora.sh) y dos
# reinicios:
#   cow       el disco de la maquina es un overlay qcow2 sobre la base de la imagen (`disk status`: "copia en escritura") y
#             Android arranca de el
#   rsa_sin   con ro.adb.secure=1 y otra clave (desconocida para adbd), el adb propio NO entra: adbd pide AUTH, la firma no
#             vale y weft manda la clave publica para que se acepte en la pantalla (nadie la acepta)
#   rsa       con la clave de weft en /data/misc/adb/adb_keys, entra firmando el testigo (AUTH SIGNATURE)
# La clave de weft (adbkey, PEM PKCS#1 como la de Google) se crea aqui con openssl y su publica en el formato de Android
# (android_pubkey) con python3, antes de activar ro.adb.secure (que se pone en /product/etc/build.prop con remount).
# Cada comprobacion sale como "verificacion NOMBRE=ok|FALLO ..."; el codigo de salida es 1 si alguna fallo.
# Entorno: SRC (repositorio, con scripts/), WEFT_DIR (paquete de weft), RES (resultados), WEFT_IMAGE (opcional: imagen ya
# descargada, id o carpeta; sin ella el guion baja la de prueba), WEFT_WAIT (espera de cada arranque, 300 s). Se ejecuta
# desde una carpeta de trabajo vacia (el guion crea ./weft-data).
set -u
: "${SRC:?falta SRC}"; : "${WEFT_DIR:?falta WEFT_DIR}"; RES="${RES:-$PWD/res-cow-rsa}"
mkdir -p "$RES"; RES="$(cd "$RES" && pwd)"
WAIT="${WEFT_WAIT:-300}"
export WEFT_DIR WEFT_GPU="${WEFT_GPU:-virtio-pci}" WEFT_DISPLAY=none WEFT_AUDIO=none WEFT_ADB="${WEFT_ADB:-tcp}"
WF="$WEFT_DIR/weft --root weft-data --name prueba"
FALLOS=0
v() { local n="$1" r="$2"; shift 2; echo "verificacion $n=$r $*"; [ "$r" = ok ] || FALLOS=$((FALLOS + 1)); }
wf() { $WF "$@"; }
guion() { bash "$SRC/scripts/dev-fedora.sh" "$@" 2>&1 | grep -v "####"; return "${PIPESTATUS[0]}"; }
reiniciar() { wf reboot > /dev/null 2>&1; sleep 10; wf wait-adb --timeout "$WAIT"; }

# ---------------------------------------------------------------- copia en escritura
echo "=== disk.cow=si y arranque con el guion"
mkdir -p weft-data
wf config set disk.cow si
WEFT_WAIT="$WAIT" guion > "$RES/arranque.txt"; G=$?
grep -E "^imagen:|arranque_completo|AVISO|ERROR" "$RES/arranque.txt"
wf disk status ${WEFT_IMAGE:+--image "$WEFT_IMAGE"} > "$RES/disk-status.txt" 2>&1; cat "$RES/disk-status.txt"
if [ "$G" = 0 ] && wf wait-adb --timeout 60 > /dev/null 2>&1 && grep -q "copia en escritura sobre la base" "$RES/disk-status.txt"; then
  v cow ok "($(grep -o 'ocupa .*' "$RES/disk-status.txt"); Android arranco del overlay)"
else
  v cow FALLO "(guion=$G; ver arranque.txt y disk-status.txt)"
fi

# ---------------------------------------------------------------- clave de weft y ro.adb.secure=1
CFG="$(wf paths 2> /dev/null | sed -n 's/^config=//p')"; [ -n "$CFG" ] || CFG=weft-data/config
mkdir -p "$CFG"
openssl genrsa -traditional -out "$CFG/adbkey" 2048 2> /dev/null || openssl genrsa -out "$CFG/adbkey" 2048 2> /dev/null
chmod 600 "$CFG/adbkey"
# android_pubkey: palabras (64), n0inv, n y R^2 mod n en little endian, e; en base64 y un comentario
openssl rsa -in "$CFG/adbkey" -noout -modulus 2> /dev/null | cut -d= -f2 | python3 -c '
import base64, struct, sys
n = int(sys.stdin.read().strip(), 16); w = 64
le = lambda x: x.to_bytes(w * 4, "little")
blob = struct.pack("<II", w, (-pow(n, -1, 1 << 32)) % (1 << 32)) + le(n) + le(pow(2, 4096, n)) + struct.pack("<I", 65537)
print(base64.b64encode(blob).decode() + " weft@ci")' > "$CFG/adbkey.pub"
echo "clave: $(cut -c1-24 "$CFG/adbkey.pub")... ($(wc -c < "$CFG/adbkey.pub") bytes)"
wf root; wf wait-adb --timeout 60 > /dev/null
wf push "$CFG/adbkey.pub" /data/local/tmp/adbkey.pub
wf adb-shell 'cat /data/local/tmp/adbkey.pub >> /data/misc/adb/adb_keys && chown system:shell /data/misc/adb/adb_keys && chmod 640 /data/misc/adb/adb_keys && rm /data/local/tmp/adbkey.pub && ls -l /data/misc/adb/'
echo "--- ro.adb.secure antes: $(wf adb-shell getprop ro.adb.secure)"
wf disable-verity 2>&1 | tail -n 2; reiniciar > /dev/null || echo "AVISO: no volvio tras disable-verity"
wf root > /dev/null; wf wait-adb --timeout 60 > /dev/null
wf remount 2>&1 | tail -n 2
wf adb-shell 'f=/product/etc/build.prop; grep -q "^ro.adb.secure=" $f && sed -i "s/^ro.adb.secure=.*/ro.adb.secure=1/" $f || echo ro.adb.secure=1 >> $f; grep ro.adb.secure $f'
wf reboot > /dev/null 2>&1; sleep 10

# ---------------------------------------------------------------- sin la clave conocida: adbd debe rechazarla
mv "$CFG/adbkey" "$CFG/adbkey.buena"; mv "$CFG/adbkey.pub" "$CFG/adbkey.pub.buena"
# adbd en marcha y la clave rechazada: el adb propio manda la publica y espera a que alguien la acepte (nadie lo hace)
T=0; S=0; while [ "$T" -lt "$WAIT" ]; do
  wf adb-shell --timeout 15 id > "$RES/rsa-sin.txt" 2>&1; S=$?
  { [ "$S" = 0 ] || grep -q "no acepto la clave" "$RES/rsa-sin.txt"; } && break
  sleep 5; T=$((T + 20))
done
head -n 3 "$RES/rsa-sin.txt"
[ "$S" != 0 ] && grep -q "no acepto la clave" "$RES/rsa-sin.txt" && v rsa_sin ok "(adbd exige autenticacion: una clave desconocida no entra)" || v rsa_sin FALLO "(codigo $S: adbd no exigio la clave)"
rm -f "$CFG/adbkey" "$CFG/adbkey.pub"; mv "$CFG/adbkey.buena" "$CFG/adbkey"; mv "$CFG/adbkey.pub.buena" "$CFG/adbkey.pub"

# ---------------------------------------------------------------- con la clave de weft
wf wait-adb --timeout "$WAIT" > "$RES/rsa.txt" 2>&1; S=$?
SEC="$(wf adb-shell getprop ro.adb.secure 2>> "$RES/rsa.txt" | tr -d '\r')"
head -n 3 "$RES/rsa.txt"
[ "$S" = 0 ] && [ "$SEC" = 1 ] && ! grep -q "Permitir depuracion" "$RES/rsa.txt" && v rsa ok "(ro.adb.secure=1: entro firmando el testigo con la clave de weft)" || v rsa FALLO "(wait-adb=$S ro.adb.secure=$SEC)"

guion parar > /dev/null
[ "$FALLOS" = 0 ] && echo "cow_rsa_completo=ok" || echo "cow_rsa_completo=FALLO ($FALLOS)"
exit $((FALLOS > 0))
