#!/usr/bin/env bash
# Prueba de la ventana propia de weft (--display window) sin pantalla fisica. Pensada para el CI (Fedora 44 en
# docker con KVM) y ejecutable en local por partes. SDL3 trae un controlador de video "offscreen" (sin servidor grafico) y un
# dibujador por software: la ventana se crea, dibuja y recibe eventos como siempre, solo que nadie la ve.
#
# Arranca Android con el guion de usuario (scripts/dev-fedora.sh, WEFT_DISPLAY=window) y comprueba:
#   frames      window.log (WEFT_WINDOW_STATS) anota cuadros reales (Scanout/Update) con la resolucion pedida, es decir,
#               no solo el aviso "Display output is not active" que QEMU manda en cada cambio de superficie
#   fuente      la ventana encontro una fuente del sistema (no cayo a la 5x7 sin tildes)
#   captura     la ventana dibuja la barra superior y la pantalla (sin panel lateral) (gancho WEFT_WINDOW_INJECT=1): PPM -> PNG, no lisa
#   tema_claro_200  con ventana.tema=claro y ventana.texto=200 la barra se aclara y dobla su altura (relectura de config en marcha)
#   ajustes     la pestana Configuraciones abre la pantalla de configuracion en Controles (pclick config) y se cierra; Maquina trae Reiniciar y Apagar
#   toque       un clic inyectado en la pantalla llega a Android como toque multitouch (getevent por el adb propio)
#   atajo       F1 llega a Android como KEY_BACK
#   doctor      `weft doctor` termina con 0 (hay KVM)
# Escribe en $RES: arranque.txt, window.log, ventana-barra.png, ventana-config.png, ventana-maquina.png, ventana-claro-200.png, getevent-*.txt, doctor-ventana.txt,
# config-list.txt, config (si existe), verificaciones.txt. Cada verificacion sale como "verificacion NOMBRE=ok|FALLO ..." y el
# codigo de salida es 1 si alguna fallo.
#
# Entorno: WEFT_DIR (paquete de weft descomprimido) y SRC (repositorio con scripts/) para arrancar; RES (por defecto
# ./res-ventana); ESPERADA (resolucion de la pantalla, por defecto 720x1348); WEFT_WAIT (espera del arranque, 300 s).
# Para pruebas locales sin Android: SIN_ANDROID=1 salta el arranque, los cuadros, el toque y el atajo; WEFT_CMD es la orden
# completa de weft (con --root o --state-dir y --name) y ESTADO la carpeta de estado de esa maquina.
set -u
RES="${RES:-$PWD/res-ventana}"; ESPERADA="${ESPERADA:-720x1348}"; SIN_ANDROID="${SIN_ANDROID:-0}"
mkdir -p "$RES"; RES="$(cd "$RES" && pwd)"
AQUI="$(cd "$(dirname "$0")" && pwd)"
FALLOS=0
: > "$RES/verificaciones.txt"
v() { # v NOMBRE ok|FALLO detalle...
  local n="$1" r="$2"; shift 2
  echo "verificacion $n=$r $*" | tee -a "$RES/verificaciones.txt"
  [ "$r" = ok ] || FALLOS=$((FALLOS + 1))
}

# Sin pantalla fisica: controlador de video sin servidor y dibujador por software; gancho de pruebas y estadisticas de cuadros
export SDL_VIDEO_DRIVER=offscreen SDL_RENDER_DRIVER=software WEFT_WINDOW_INJECT=1 WEFT_WINDOW_STATS=5

if [ "$SIN_ANDROID" != 1 ]; then
  : "${WEFT_DIR:?falta WEFT_DIR}"; : "${SRC:?falta SRC}"
  echo "=== guion del usuario con la ventana propia (SDL3 offscreen), gfxstream"
  WEFT_DISPLAY=window WEFT_AUDIO=none WEFT_WAIT="${WEFT_WAIT:-300}" bash "$SRC/scripts/dev-fedora.sh" 2>&1 | tee "$RES/arranque.txt"
  ARRANQUE=${PIPESTATUS[0]}
  grep -E "^gpu=|arranque_completo|^captura [0-9]|motor grafico" "$RES/arranque.txt"
  cp weft-data/screenshot.png "$RES/captura-qmp.png" 2> /dev/null
  cd weft-data || exit 1
  WEFT_CMD="${WEFT_CMD:-$WEFT_DIR/weft --root . --name prueba}"
  ESTADO="${ESTADO:-state/prueba}"
  [ "$ARRANQUE" = 0 ] && grep -q "arranque_completo=si" "$RES/arranque.txt" && v arranque ok || v arranque FALLO "(codigo $ARRANQUE)"
fi
: "${WEFT_CMD:?falta WEFT_CMD}"; : "${ESTADO:?falta ESTADO}"
wf() { $WEFT_CMD "$@"; }
LOG="$ESTADO/window.log"

# --- cuadros reales: "cuadros en 5.0 s: Scanout=N avisos_descartados=P Update=U mostrados=M (x/s) pantalla=WxH formato=0x.."
if [ "$SIN_ANDROID" != 1 ]; then
  sleep 6   # una ventana de estadisticas mas con el sistema ya dibujado
  cp "$LOG" "$RES/window.log" 2> /dev/null
  grep -E "cuadros en" "$LOG" | tail -n 3
  OKF=$(LC_ALL=C awk -v e="pantalla=$ESPERADA" '/cuadros en/ { s=0; u=0; for (i=1;i<=NF;i++) { if ($i ~ /^Scanout=/) s=substr($i,9); if ($i ~ /^Update=/) u=substr($i,8) } if (s+u > 0 && index($0, e)) n++ } END { print n+0 }' "$LOG" 2> /dev/null)
  PLAC=$(grep -c "avisos_descartados=[1-9]" "$LOG" 2> /dev/null)
  [ "${OKF:-0}" -gt 0 ] && v frames ok "(${OKF} ventanas de estadisticas con cuadros reales de $ESPERADA; con avisos 'Display output is not active' descartados en ${PLAC:-0})" || v frames FALLO "(ningun cuadro real de $ESPERADA en $LOG)"
else
  cp "$LOG" "$RES/window.log" 2> /dev/null
fi

# --- fuente del sistema
fc-match "sans-serif:lang=es" 2>&1 | tee "$RES/fc-match.txt"
grep -a "ventana: fuente" "$LOG" | head -n 1
if grep -aq "ventana: fuente" "$LOG" && ! grep -aq "sin fuente del sistema" "$LOG"; then v fuente ok "($(grep -a -m1 'ventana: fuente' "$LOG" | cut -c1-120))"; else v fuente FALLO "(la ventana cayo a la fuente 5x7 o no anoto la fuente)"; fi

# --- captura de lo que dibuja la ventana (panel desplegado) y de la pantalla de configuracion
captura() { # captura NOMBRE -> RES/NOMBRE.png ; devuelve 0 si llego y no es lisa
  local ppm="$RES/$1.ppm" t=0
  rm -f "$ppm" "$ppm.botones"
  echo "$ppm" > "$ESTADO/window-capture.request"
  while [ ! -s "$ppm.botones" ] && [ "$t" -lt 30 ]; do sleep 0.5; t=$((t + 1)); done
  [ -s "$ppm" ] || return 1
  python3 "$AQUI/ppm2png.py" "$ppm" "$RES/$1.png" > "$RES/$1.txt" && cat "$RES/$1.txt"
}
# la ventana es la barra superior y la pantalla del dispositivo (sin panel lateral): solo hay un boton, la pestana Configuraciones
if captura ventana-barra && grep -q "^config " "$RES/ventana-barra.ppm.botones" && ! grep -q "^plegar " "$RES/ventana-barra.ppm.botones"; then
  v captura ok "($(cat "$RES/ventana-barra.txt"); barra con $(wc -l < "$RES/ventana-barra.ppm.botones") boton)"
else
  v captura FALLO "(sin captura de la ventana, lisa, o sin la pestana Configuraciones)"
fi
# la pestana abre el dialogo en Controles (acciones rapidas con su tecla); Maquina trae reiniciar y apagar; Entrada, los mandos
wf window-inject "pclick config" > /dev/null 2>&1; sleep 1.5
if captura ventana-config && grep -q "^a-cerrar " "$RES/ventana-config.ppm.botones" && grep -q "^a-ctl-atras " "$RES/ventana-config.ppm.botones"; then
  v ajustes ok "(la pestana Configuraciones abre el dialogo en Controles)"
else
  v ajustes FALLO "(pclick config no abrio la pantalla de configuracion en Controles)"
fi
wf window-inject "pclick a-nav-maquina" > /dev/null 2>&1; sleep 1.5
# la ventana de CI es pequena: el dialogo es desplazable y solo los botones visibles figuran en .botones; se baja con la rueda
# (coordenadas de ventana, sobre el contenido del dialogo) (2 pasos de 144 dp) para que Reiniciar y Apagar esten a la vista
wf window-inject "wwheel 0 -6 175 500" > /dev/null 2>&1; sleep 1.5
if captura ventana-maquina && grep -q "^a-btn-reiniciar-android " "$RES/ventana-maquina.ppm.botones" && grep -q "^a-btn-apagar " "$RES/ventana-maquina.ppm.botones"; then
  v maquina ok "(Maquina ofrece reiniciar y Apagar, tras desplazar el dialogo)"
else
  v maquina FALLO "(Maquina sin los botones de reiniciar y apagar)"
fi
wf window-inject "pclick a-cerrar" > /dev/null 2>&1; sleep 1.5
# tema claro y texto al 200 % (la ventana relee config al cambiar): la barra se aclara y su boton dobla la altura; se
# compara con la captura de la barra de antes (tema auto: oscuro sin portal de escritorio, y 100 %)
alto() { awk '$1 == "config" { print $5 }' "$1" 2> /dev/null; }
wf config set ventana.tema claro > /dev/null; wf config set ventana.texto 200 > /dev/null; sleep 2
H1="$(alto "$RES/ventana-barra.ppm.botones")"
if captura ventana-claro-200 && H2="$(alto "$RES/ventana-claro-200.ppm.botones")" && [ -n "$H1" ] && [ -n "$H2" ]; then
  B1="$(python3 "$AQUI/ppm2png.py" --brillo "$RES/ventana-barra.ppm" "$H1")"; B2="$(python3 "$AQUI/ppm2png.py" --brillo "$RES/ventana-claro-200.ppm" "$H2")"
  if [ "$((H2 * 10))" -ge "$((H1 * 17))" ] && [ "$B2" -ge 150 ] && [ "$B2" -gt "$((B1 + 40))" ]; then
    v tema_claro_200 ok "(barra: alto $H1 -> $H2 px, brillo $B1 -> $B2)"
  else
    v tema_claro_200 FALLO "(barra: alto $H1 -> $H2 px, brillo $B1 -> $B2; se esperaba >= 1,7 veces y claro)"
  fi
else
  v tema_claro_200 FALLO "(sin captura con el tema claro al 200 %)"
fi
wf config set ventana.tema auto > /dev/null; wf config set ventana.texto 100 > /dev/null; sleep 2
rm -f "$RES"/*.ppm   # los PPM pesan ~1 MB cada uno; quedan los PNG y .botones

# --- entrada: lo que ve el kernel de Android al inyectar eventos en la ventana
if [ "$SIN_ANDROID" != 1 ]; then
  wf root > /dev/null 2>&1
  observar() { # observar ARCHIVO LINEA... : captura getevent 9 s en el invitado mientras la ventana recibe las lineas
    local f="$1"; shift
    wf adb-shell "timeout 9 getevent -l" > "$RES/$f" 2>&1 &
    local p=$!
    sleep 3
    wf window-inject "$@"
    wait "$p"
  }
  # clic en el centro de la pantalla (coordenadas de la vista, es decir, de la imagen del panel)
  C="$(echo "$ESPERADA" | awk -Fx '{ printf "%d %d", $1/2, $2/2 }')"
  observar getevent-clic.txt "move $C" "btn 1 down $C" "wait 150" "btn 1 up $C"
  if grep -q "ABS_MT_POSITION_X" "$RES/getevent-clic.txt" && grep -Eq "ABS_MT_TRACKING_ID +[0-9a-f]{8}" "$RES/getevent-clic.txt" && grep -Eq "ABS_MT_TRACKING_ID +ffffffff" "$RES/getevent-clic.txt"; then
    v toque ok "(contacto multitouch con posicion y suelta recibidos en Android)"
  else
    v toque FALLO "(getevent no vio un toque multitouch completo; ver getevent-clic.txt)"
  fi
  sleep 2
  # F1 (codigo HID 58 de SDL) = Atras
  observar getevent-f1.txt "key 58 down" "wait 100" "key 58 up"
  if grep -Eq "KEY_BACK +DOWN" "$RES/getevent-f1.txt" && grep -Eq "KEY_BACK +UP" "$RES/getevent-f1.txt"; then
    v atajo ok "(F1 llego a Android como KEY_BACK)"
  else
    v atajo FALLO "(getevent no vio KEY_BACK al pulsar F1; ver getevent-f1.txt)"
  fi
fi

# --- doctor (con la maquina en marcha incluye la fila de adbd) y configuracion generada
wf doctor > "$RES/doctor-ventana.txt" 2>&1; D=$?
cat "$RES/doctor-ventana.txt"
[ "$D" = 0 ] && v doctor ok "(codigo 0)" || v doctor FALLO "(codigo $D)"
# `config set` con el valor por defecto genera el archivo `config` completo (claves comentadas con su explicacion)
wf config set confirmar si > /dev/null 2>&1
wf config list > "$RES/config-list.txt" 2>&1
wf config path > "$RES/config-ruta.txt" 2>&1
CFG="$(tr -d '\r\n' < "$RES/config-ruta.txt" 2> /dev/null)"
if [ -s "$CFG" ] && cp "$CFG" "$RES/config" && [ "$(wf config get confirmar 2> /dev/null)" = si ] && grep -q "atajo.atras" "$RES/config-list.txt"; then
  v config ok "($(wc -l < "$RES/config") lineas; claves: $(wc -l < "$RES/config-list.txt"))"
else
  v config FALLO "(no se genero el archivo config en $CFG)"
fi
cp "$ESTADO/qemu.log" "$RES/qemu.log" 2> /dev/null; tail -c 3000 "$ESTADO/qemu.log" > "$RES/qemu-final.txt" 2> /dev/null

if [ "$SIN_ANDROID" != 1 ]; then
  wf stop --timeout 20 > /dev/null 2>&1   # (el guion de desarrollo no sirve aqui: estamos dentro de weft-data y buscaria otra weft-data)
fi
[ "$FALLOS" = 0 ] && echo "ventana_completa=ok" || echo "ventana_completa=FALLO ($FALLOS)"
exit $((FALLOS > 0))
