#!/usr/bin/env bash
# Prueba del raton como toque en Fedora (QEMU del sistema, sin ventana: WEFT_DISPLAY=none) con el adb propio de weft
# (sin adb de Google). Dos pasadas, cada una con su arranque completo de Android:
#   1. defectos del guion: maquina q35 y --pointer multitouch (el raton es un contacto de la pantalla tactil virtio). `click` toca
#      por QMP (evento mtt); la aplicacion de prueba debe recibir cada toque en su punto (con una tolerancia de 0,02 por el
#      redondeo de la escala 0..32767 de la pantalla tactil).
#   2. WEFT_POINTER=wacom explicito: es lo que el guion usa con las ventanas gtk/sdl, que solo mandan posiciones absolutas. La
#      tableta USB se configura como pantalla tactil con archivos idc por el adb propio; el clic, el arrastre con el boton
#      apretado y un segundo arranque (la configuracion debe seguir sin tocar nada).
# Cada comprobacion sale como "verificacion NOMBRE=ok|FALLO ..."; el codigo de salida es 1 si alguna fallo.
# Entorno: SRC (repositorio, con scripts/), WEFT_DIR (paquete de weft), RES (resultados). Se ejecuta desde una carpeta
# de trabajo vacia (el guion crea ./weft-data).
set -u
: "${SRC:?falta SRC}"; : "${WEFT_DIR:?falta WEFT_DIR}"; RES="${RES:-$PWD/res-raton}"
mkdir -p "$RES"; RES="$(cd "$RES" && pwd)"
export WEFT_DIR WEFT_GPU=virtio-pci WEFT_DISPLAY=none WEFT_AUDIO=none
WF="$WEFT_DIR/weft --root weft-data --name prueba"
FALLOS=0
v() { local n="$1" r="$2"; shift 2; echo "verificacion $n=$r $*"; [ "$r" = ok ] || FALLOS=$((FALLOS + 1)); }
wf() { $WF "$@"; }
# el codigo de salida es el del guion, no el del grep que filtra las barras de progreso (PIPESTATUS, leido justo tras la tuberia)
guion() { bash "$SRC/scripts/dev-fedora.sh" "$@" 2>&1 | grep -v "####"; return "${PIPESTATUS[0]}"; }
lanzar_banco() { wf adb-shell "logcat -c; input keyevent KEYCODE_WAKEUP; wm dismiss-keyguard; am start -n rs.weft.banco/.Main" > /dev/null 2>&1; sleep 12; }
toques() { wf adb-shell "logcat -d -s banco:I | grep -E 'BANCO (toque|biblioteca)' | sed 's/.*BANCO //'" | cut -c1-120; }
# hay_toque X Y TOLERANCIA: alguna linea "toque=baja x=.. y=.." del registro dentro de la tolerancia
hay_toque() {
  toques | LC_ALL=C awk -v X="$1" -v Y="$2" -v T="$3" '/toque=baja/ { for (i = 1; i <= NF; i++) { if ($i ~ /^x=/) x = substr($i, 3); if ($i ~ /^y=/) y = substr($i, 3) }
    dx = x - X; dy = y - Y; if (dx < 0) dx = -dx; if (dy < 0) dy = -dy; if (dx <= T && dy <= T) ok = 1 } END { exit !ok }'
}

echo "=== QEMU del sistema: dispositivos de entrada"
qemu-system-x86_64 -device help 2>&1 | grep -i -E "wacom|multitouch|virtio-tablet|usb-tablet|pcie-root-port"

# ---------------------------------------------------------------- pasada 1: multitouch (defecto)
echo; echo "=== pasada 1: defectos del guion (q35, multitouch)"
WEFT_WAIT=300 guion; echo "guion codigo=$?"
wf doctor > "$RES/doctor-raton.txt" 2>&1; D=$?
cat "$RES/doctor-raton.txt"
[ "$D" = 0 ] && v doctor ok "(codigo 0)" || v doctor FALLO "(codigo $D)"
echo "--- maquina y entrada vistas por Android"
wf adb-shell "getevent -lp" > "$RES/dispositivos-entrada.txt" 2>&1
grep -E "name:|ABS_MT_POSITION_X" "$RES/dispositivos-entrada.txt" | head -n 12
wf adb-shell "dumpsys input" > "$RES/dumpsys-input-multitouch.txt" 2>&1
grep -n -A30 "Virtio MultiTouch" "$RES/dumpsys-input-multitouch.txt" | grep -E "MultiTouch|Classes|Sources|DeviceType|IsExternal|XScale|YScale" | head -n 10 | cut -c1-180
echo "--- aplicacion de prueba"
wf install -r "$SRC/banco-x86_64.apk" 2>&1 | tail -n 1
lanzar_banco
for P in "0.25 0.50" "0.75 0.20" "0.50 0.90"; do wf click $P; echo "click $P -> $?"; sleep 2; done
echo "--- lo que recibio la aplicacion"; toques
wf screenshot "$RES/tras-clic-multitouch.png"
OK=1; for P in "0.25 0.50" "0.75 0.20" "0.50 0.90"; do hay_toque $P 0.02 || OK=0; done
[ "$OK" = 1 ] && v multitouch_toques ok "(tres toques por QMP recibidos en su punto, tolerancia 0,02)" || v multitouch_toques FALLO "(la aplicacion no recibio los tres toques en su punto)"
guion parar

# ---------------------------------------------------------------- pasada 2: wacom explicito (el modo de las ventanas gtk/sdl)
echo; echo "=== pasada 2: WEFT_POINTER=wacom explicito"
WEFT_POINTER=wacom WEFT_WAIT=300 guion; echo "guion codigo=$?"
echo "--- archivos de configuracion"; wf adb-shell "ls -lZ /data/system/devices/idc/; cat /data/system/devices/idc/Vendor_056a_Product_0000.idc"
echo "--- como clasifica Android la tableta"
wf adb-shell "dumpsys input" > "$RES/dumpsys-input.txt" 2>&1
grep -n -A40 "Wacom Penpartner Pen" "$RES/dumpsys-input.txt" | grep -E "Wacom|Classes|Sources|DeviceType|ConfigurationFile|IsExternal|AssociatedDisplay:|XScale|YScale" | head -n 20 | cut -c1-180
echo "--- aplicacion de prueba"; wf install -r "$SRC/banco-x86_64.apk" 2>&1 | tail -n 1
lanzar_banco
for P in "0.25 0.50" "0.75 0.20" "0.50 0.90"; do wf click $P; echo "click $P -> $?"; sleep 2; done
echo "--- arrastre con el boton apretado"
ev() { wf qmp input-send-event "{\"events\":[$1]}" > /dev/null; }
ev '{"type":"abs","data":{"axis":"x","value":8000}},{"type":"abs","data":{"axis":"y","value":8000}}'; ev '{"type":"btn","data":{"button":"left","down":true}}'; sleep 0.3
for V in 10000 14000 18000 22000; do ev "{\"type\":\"abs\",\"data\":{\"axis\":\"y\",\"value\":$V}}"; sleep 0.2; done
ev '{"type":"btn","data":{"button":"left","down":false}}'; sleep 2
echo "--- lo que recibio la aplicacion"; toques
wf screenshot "$RES/tras-clic-wacom.png"
hay_toque 0.25 0.50 0.002 && hay_toque 0.75 0.20 0.002 && hay_toque 0.50 0.90 0.002 && v wacom_toques ok "(clics exactos en su punto)" || v wacom_toques FALLO "(la aplicacion no recibio los clics en su punto)"
echo "--- segundo arranque: la configuracion debe seguir sin tocar nada"
guion parar
WEFT_POINTER=wacom WEFT_WAIT=300 guion > "$RES/segundo-arranque-completo.txt"; echo "guion codigo=$?"
grep -E "raton como toque|bluetooth|arranque_completo|AVISO|ERROR" "$RES/segundo-arranque-completo.txt" | tee "$RES/segundo-arranque.txt"
lanzar_banco
wf click 0.40 0.60; sleep 2; toques
grep -q "raton como toque: ya estaba configurado" "$RES/segundo-arranque.txt" && hay_toque 0.40 0.60 0.002 && v wacom_persistente ok "(configurado sin tocar nada tras reiniciar)" || v wacom_persistente FALLO "(el segundo arranque no conservo la configuracion)"
guion parar

[ "$FALLOS" = 0 ] && echo "raton_completo=ok" || echo "raton_completo=FALLO ($FALLOS)"
exit $((FALLOS > 0))
