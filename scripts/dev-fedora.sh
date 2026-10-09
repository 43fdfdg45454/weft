#!/usr/bin/env bash
# Prueba weft en Fedora / Bazzite con el QEMU del sistema (sin instalar ni compilar nada) y la imagen
# Cuttlefish de Google (Android x86_64). Todo queda en la carpeta ./weft-data; para borrar la prueba
# basta con borrar esa carpeta.
#
# Uso:  bash dev-fedora.sh            arranca Android en una ventana
#       bash dev-fedora.sh parar      apaga la maquina
#       bash dev-fedora.sh captura    guarda una captura de pantalla en weft-data/screenshot.png
#       bash dev-fedora.sh diagnostico  si la maquina se cerro sola: muestra por que (sin datos personales)
#       bash dev-fedora.sh rotar [0|90|180|270]   gira la pantalla (sin argumento, un paso); con WEFT_DISPLAY=window la
#                                      ventana gira su presentacion y el raton/toque sigue siendo el de la ventana
#       bash dev-fedora.sh resolucion ANCHOxALTO[@DPI]   cambia la resolucion con la maquina en marcha: pide el modo al
#                                      dispositivo (ventana propia o GTK), reinicia SurfaceFlinger por el adb propio de weft (reinicio blando
#                                      del entorno grafico de Android, ~20 s, se cierran las apps; la maquina no se reinicia)
#                                      y aplica la densidad con `wm density`. Queda guardada para el proximo arranque
#       bash dev-fedora.sh adb ORDEN...   ejecuta una orden en el shell de Android por el adb propio de weft (vsock;
#                                      no hace falta adb de Google). Mas ordenes: weft push/pull/install/logcat/...
#       bash dev-fedora.sh doctor   comprueba el equipo (QEMU, KVM, vsock, qemu-ui-dbus, SDL3, gfxstream, mandos, disco)
#       bash dev-fedora.sh imagen ORDEN...   ordenes `weft image` (list, add, info, use, remove, check, profiles)
#       bash dev-fedora.sh root     baja (curl, con comprobacion de tamano y sha256) los archivos del acceso root a ./weft-data/cache/root
#                                      y los instala con `weft root enable --from CARPETA --now` (WEFT_ROOT_MANAGER=1: tambien el gestor)
#       bash dev-fedora.sh traductor   baja la release v0.1.0 del traductor ARM (tamano y sha256 comprobados) y la instala,
#                                      vigilada, con `weft bridge install --file LIB` (WEFT_PUENTE_URL=URL usa otra release)
#
# weft NO descarga nada: este guion (herramienta de desarrollo, fuera del programa) es quien baja con curl la imagen, los archivos
# de root y el traductor, los deja en ./weft-data y llama a las ordenes con el archivo local.
#
# Este guion es un envoltorio fino: la logica de producto vive en weft (ordenes `image add`, `disk create`, `start --image`,
# `apply-settings`, `share setup`, `root ensure`; ver `weft --help`). Aqui solo se juntan los requisitos del equipo, las
# variables WEFT_* y el orden de las ordenes.
#
# Ajustes opcionales (variables de entorno):
#   WEFT_GPU=auto|hardware|software      auto: aceleracion grafica por hardware si el equipo la ofrece y la imagen la admite (si no,
#                                      software con aviso); hardware: la exige; software: sin aceleracion. Tambien valen gfxstream y
#                                      virtio-pci (nombres concretos del motor, avanzado). Sin esta variable manda maquina.gpu de `config`
#   WEFT_DISPLAY=auto|gtk|sdl|window|none  auto: ventana GTK si QEMU la trae, si no SDL; window: ventana propia de
#                                      weft (SDL3 + qemu-ui-dbus; sin parpadeo); none: sin ventana
#   WEFT_SDL_GL=auto|on|off              solo con WEFT_DISPLAY=sdl: dibujar la ventana con OpenGL (on aborta si Android
#                                      entrega la imagen en RGBA; no recomendado con gfxstream)
#   WEFT_AUDIO=pipewire|pa|none
#   WEFT_MEM=8192  WEFT_CPUS=4   (memoria en MiB, por defecto 8192)
#   PRECEDENCIA: variable WEFT_* > archivo `config` del estado (`weft config`, la pantalla de configuracion de la
#   ventana propia) > valor por defecto del guion. Aplica a WEFT_MEM, WEFT_CPUS, WEFT_MACHINE, WEFT_GPU, WEFT_GAMEPAD y WEFT_RES.
#   WEFT_RES=ANCHOxALTO[@DPI]            resolucion de la pantalla; sin esta variable se usa pantalla.resolucion/densidad de
#                                      `config` (las deja tambien la orden `resolucion`) y, si no hay, 720x1348. La densidad
#                                      (@DPI) va como androidboot.lcd_density (el perfil trae 280). El ancho se redondea a
#                                      multiplo de 8 (DRM de Linux)
#   WEFT_DATA=24G|img|TAMANO             tamano de la particion de datos (/data); por defecto disk.data de `config` (24G): se crea
#                                      VACIA y Android la formatea (f2fs) en el primer arranque. img: usar el userdata.img de la imagen
#   WEFT_IMAGE=ID                        imagen de Android instalada que arranca la maquina (`bash dev-fedora.sh imagen list`); sin esta
#                                      variable: la clave image.id de `config` y, con `auto`, la unica instalada. Cada imagen tiene su
#                                      propio disco: cambiar de imagen no borra los datos de la otra
#   WEFT_BUILD=N                         si no hay ninguna imagen instalada (o con esta variable) BAJA CON CURL la compilacion N de la imagen
#                                      de prueba (por defecto 16373615) y la instala con `weft image add ZIP`. WEFT_IMAGEN_URL=URL
#                                      baja otro zip. Para otras imagenes: `bash dev-fedora.sh imagen add ZIP|CARPETA`
#   WEFT_ADB=auto|vsock|tcp|both         por donde habla el adb propio: auto (vsock si hay /dev/vhost-vsock y no se esta en Flatpak, si no
#                                      TCP por un puerto local), vsock, tcp o both (los dos)
#   WEFT_CID=N                           CID de vsock del invitado (adb propio); sin ella weft elige uno libre (3 si nadie lo usa)
#   WEFT_DIR=CARPETA                     usar un paquete de weft ya descomprimido en vez de descargarlo
#   WEFT_POINTER=multitouch|wacom|tablet  dispositivo que recibe el raton. multitouch (por defecto): la pantalla tactil virtio;
#                                      la ventana propia (WEFT_DISPLAY=window) manda toques por D-Bus (clic, arrastre, rueda como
#                                      deslizamiento, pellizco con Ctrl+clic) y no hace falta configurar nada en Android. Las
#                                      ventanas gtk/sdl solo mandan posiciones absolutas, que la pantalla tactil no recibe: con
#                                      ellas el guion usa wacom automaticamente (tableta USB que Android trata como pantalla
#                                      tactil; la configura por el adb propio la primera vez; necesita /dev/vhost-vsock)
#   WEFT_KERNEL=ARCHIVO                  kernel propio (bzImage x86_64) en vez del que trae la imagen
#   WEFT_GPU_OPTS=TEXTO                  propiedades extra del dispositivo de pantalla (--gpu-opts), separadas por comas.
#                                      Por defecto edid=off: sin EDID el invitado usa un modo de 60 Hz (con EDID, QEMU anuncia
#                                      75 Hz y los juegos que piden 60 fps quedan en 37,5). WEFT_GPU_OPTS= (vacio) lo quita.
#   WEFT_GAMEPAD=none|auto|/dev/input/eventN  mandos de juegos del equipo: auto conecta los presentes y los que aparezcan
#                                      (por defecto none). Mientras un mando esta conectado a Android el equipo no lo ve;
#                                      vuelve al apagar la maquina o con `weft gamepad detach`.
#   WEFT_MACHINE=q35|pc                  tipo de maquina de QEMU (por defecto q35). La conexion en caliente de mandos necesita q35
#                                      (el kernel de la imagen no trae hotplug PCI por ACPI, solo PCIe); en pc los mandos
#                                      de WEFT_GAMEPAD solo se ven si ya estaban presentes al arrancar.
#   WEFT_SHARE=CARPETA|none              carpeta del equipo visible para las apps de Android en /sdcard/NOMBRE (virtiofs, necesita
#                                      virtiofsd; NOMBRE = el de la carpeta). Por defecto la carpeta Shared de este directorio, si
#                                      existe. none la desactiva. Para varias o permanentes: `weft share add` y la pantalla
#                                      de configuracion (Carpetas compartidas)
#   WEFT_WAIT=SEGUNDOS                   esperar a que Android termine de arrancar, guardar captura e informar
set -u
W=weft-data
GPU="${WEFT_GPU:-auto}"; DISP="${WEFT_DISPLAY:-auto}"; AUDIO="${WEFT_AUDIO:-pipewire}"
MEM="${WEFT_MEM:-8192}"; DATA="${WEFT_DATA:-}"; CPUS="${WEFT_CPUS:-4}"; POINTER="${WEFT_POINTER:-multitouch}"; GAMEPAD="${WEFT_GAMEPAD:-none}"; MACHINE="${WEFT_MACHINE:-q35}"
CID="${WEFT_CID:-}"
die() { echo "ERROR: $*" >&2; exit 1; }
paso() { echo; echo "== $*"; }

# ---- descargas (solo este guion descarga: weft no) -------------------------------------------------------------------------
# bajar URL DESTINO [TAMANO] [SHA256]: curl a DESTINO.part, comprueba el tamano (el pedido o, si no se da, el que anuncia el servidor con
# un GET de rango, porque ci.android.com contesta 404 a HEAD) y, si se da, el sha256, y lo deja en DESTINO. Si DESTINO ya existe con
# ese tamano (y ese sha256) no baja nada.
sha_de() { sha256sum "$1" | cut -d" " -f1; }
tam_remoto() { curl -sL -D - -o /dev/null -r 0-0 --max-filesize 4096 --max-time 25 "$1" 2> /dev/null | tr -d '\r' | awk 'tolower($1)=="content-range:" { n=$3; sub(/.*\//, "", n); t=n } END { if (t != "") print t }'; }
bajar() {
  local url="$1" dest="$2" esperado="${3:-}" sha="${4:-}" total
  total="${esperado:-$(tam_remoto "$url")}"
  if [ -n "$total" ] && [ -f "$dest" ] && [ "$(stat -c %s "$dest")" = "$total" ] && { [ -z "$sha" ] || [ "$(sha_de "$dest")" = "$sha" ]; }; then echo "ya esta bajado: $dest"; return 0; fi
  mkdir -p "$(dirname "$dest")" || return 1
  echo "bajando $(basename "$dest")${total:+ ($((total / 1048576)) MiB)}"
  curl -fL --retry 2 --progress-bar -o "$dest.part" "$url" || { rm -f "$dest.part"; echo "ERROR: fallo la descarga de $url" >&2; return 1; }
  if [ -n "$total" ] && [ "$(stat -c %s "$dest.part")" != "$total" ]; then
    echo "ERROR: tamano bajado $(stat -c %s "$dest.part") distinto del esperado $total" >&2; rm -f "$dest.part"; return 1
  fi
  if [ -n "$sha" ] && [ "$(sha_de "$dest.part")" != "$sha" ]; then
    echo "ERROR: el sha256 de $(basename "$dest") no es el esperado ($sha): el archivo publicado cambio o la descarga se corrompio" >&2; rm -f "$dest.part"; return 1
  fi
  mv "$dest.part" "$dest"
}
# archivos del acceso root (KernelSU-Next v3.4.0): nombre, tamano exacto y sha256 (calculados con  curl -fsSL URL | sha256sum  sobre
# la release; si se cambia la version hay que recalcularlos)
KSU_BASE="https://github.com/KernelSU-Next/KernelSU-Next/releases/download/v3.4.0"
bajar_root() {
  local d="$1"
  bajar "$KSU_BASE/ksud-x86_64-linux-android" "$d/ksud-x86_64-linux-android" 5583488 63797693f344c758bf4187b5e48255c991084f8821ab8310b4bec185a304f7b2 || return 1
  if [ "${2:-}" = gestor ]; then bajar "$KSU_BASE/KernelSU_Next_v3.4.0_33294-release.apk" "$d/KernelSU_Next_v3.4.0_33294-release.apk" 11857955 50339a93c0f812b8a72c1a387a1b441891e3df0f20b2d9daf80fd798d04b3de8 || return 1; fi
}
# biblioteca del traductor ARM: release FIJA v0.1.0 de heddle (la misma que usa CI), con tamano y sha256 (curl -fsSL URL | sha256sum);
# WEFT_PUENTE_URL=URL baja otra, sin comprobar tamano ni sha256. Baja el tar.gz y deja libheddle.so en la carpeta dada.
HEDDLE_URL="https://github.com/43fdfdg45454/heddle/releases/download/v0.1.0/heddle-android-x86_64.tar.gz"
bajar_traductor() {
  local d="$1"
  mkdir -p "$d" && rm -f "$d/heddle-android-x86_64.tar.gz"
  if [ -n "${WEFT_PUENTE_URL:-}" ]; then
    bajar "$WEFT_PUENTE_URL" "$d/heddle-android-x86_64.tar.gz" || return 1
  else
    bajar "$HEDDLE_URL" "$d/heddle-android-x86_64.tar.gz" 3688099 662a1e9bc3260c14440390e132dc2a13c92cd8ed0c022e8d46413e8ebce21755 || return 1
  fi
  tar -xzf "$d/heddle-android-x86_64.tar.gz" -C "$d" || { echo "ERROR: no se pudo descomprimir la release" >&2; return 1; }
  [ -f "$d/libheddle.so" ] || { echo "ERROR: la release no trae libheddle.so" >&2; return 1; }
}

mkdir -p "$W" || die "no se pudo crear $W"
[ -n "${WEFT_DIR:-}" ] && WEFT_DIR="$(cd "$WEFT_DIR" && pwd)"
KERNEL_ARGS=()
if [ -n "${WEFT_KERNEL:-}" ]; then
  [ -f "$WEFT_KERNEL" ] || die "no existe el kernel indicado en WEFT_KERNEL"
  KERNEL_ARGS=(--kernel "$(cd "$(dirname "$WEFT_KERNEL")" && pwd)/$(basename "$WEFT_KERNEL")")
fi
ORIG="$PWD"
cd "$W" || die "no se pudo entrar en $W"
WF="./weft/weft"
[ -n "${WEFT_DIR:-}" ] && WF="$WEFT_DIR/weft"
# --root .: todo (images/, machines/, state/, cache/, profiles/) queda dentro de esta carpeta, con la estructura estandar de weft
# (ver `weft paths`). Sin --root, weft usaria los directorios XDG del usuario.
wf() { "$WF" --root . --name prueba "$@"; }
STATE=state/prueba                         # sockets y registros de QEMU y de la ventana de la maquina "prueba"
LOGA=machines/prueba/android-log.txt       # lo que Android escribe en la consola de registro

case "${1:-arrancar}" in
  parar)   wf stop --timeout 20; exit $? ;;
  captura) wf screenshot screenshot.png && echo "captura guardada en $W/screenshot.png"; exit $? ;;
  imagen)  shift; wf image "$@"; exit $? ;;
  root)
    # BAJA los archivos con curl y llama a la orden con la carpeta local (weft no descarga)
    bajar_root cache/root ${WEFT_ROOT_MANAGER:+gestor} || die "descarga de los archivos de root"
    wf root enable --from cache/root --now ${WEFT_ROOT_MANAGER:+--manager}; exit $? ;;
  traductor)
    bajar_traductor cache/bridge || die "descarga del traductor ARM"
    wf bridge install --file cache/bridge/libheddle.so; exit $? ;;
  rotar)   wf rotate ${2:+"$2"}; exit $? ;;
  resolucion)
    [ -n "${2:-}" ] || die "uso: bash $0 resolucion ANCHOxALTO[@DPI]"
    # la orden hace todo por el adb propio de weft: pide el modo, reinicia SurfaceFlinger (reinicio blando del entorno
    # grafico, ~20 s, se cierran las apps; la maquina no se reinicia) y aplica la densidad con `wm density`
    wf resolution "$2"; exit $? ;;
  adb)     shift; wf adb-shell "$@"; exit $? ;;
  doctor)  wf doctor; exit $? ;;
  diagnostico)
    U="${USER:-$(id -un 2> /dev/null)}"; H="$(hostname 2> /dev/null)"; U="${U:-usuario-desconocido}"; H="${H:-equipo-desconocido}"
    L() { sed -E "s#${HOME:-/nonexistent}#~#g; s#\\b$U\\b#usuario#g; s#$H#equipo#g" | cut -c1-220; }
    echo "===== 1. estado y causa del cierre"; wf status 2>&1 | L
    echo "===== 2. errores de QEMU/gfxstream"; grep -a -i -E "error|fail|abort|fatal|assert|lost|invalid|cannot|unable|VK_ERROR|SDL|wayland|GL_|EGL_" $STATE/qemu.log 2> /dev/null | grep -v "EglOnEgl" | sed -E "s/^qemu-system-x86_64: (-device [^:]*: )?//" | sort | uniq -c | sort -rn | head -n 20 | L
    echo "===== 3. ultimas lineas de QEMU"; tail -n 6 $STATE/qemu.log 2> /dev/null | sed -E "s/^qemu-system-x86_64: (-device [^:]*: )?//" | L
    echo "===== 4. consola del kernel de Android (final)"; wf serial 2> /dev/null | tail -n 12 | L
    echo "===== 5. Android: abortos y ultimas lineas"; grep -a -i -E "Abort message|Fatal signal|FATAL EXCEPTION|Kernel panic|reboot|shutdown" $LOGA 2> /dev/null | tail -n 8 | L; tail -n 4 $LOGA 2> /dev/null | L
    echo "===== 6. ultimo volcado de QEMU en este arranque del equipo: senal y pila del hilo que fallo"
    journalctl -b -o cat --no-pager -t systemd-coredump 2> /dev/null \
      | awk '/dumped core/ { buf = "" } { buf = buf $0 "\n" } END { printf "%s", buf }' \
      | grep -v -E "^ *Module |^ *ELF object" \
      | awk '/Stack trace of thread/ { t++ } t <= 1' | head -n 70 | L
    coredumpctl list qemu-system-x86_64 --no-pager 2> /dev/null | tail -n 3 | awk '{ print $1, $2, $3, "senal=" $(NF-3), $(NF-2) }' | L
    echo "===== 6b. sistema: falta de memoria, GPU y compositor (ultimos 30 min)"
    journalctl -b -o cat --no-pager --since "-30min" 2> /dev/null | grep -i -E "oomd.*kill|killed process|out of memory|NVRM|Xid|protocol error|wl_display" | tail -n 10 | L
    journalctl --user -b -o cat --no-pager --since "-30min" 2> /dev/null | grep -i -E "protocol error|wl_display|kwin.*(error|crash|kill)|not responding" | tail -n 6 | L
    echo "===== 7. entorno"; echo "sesion=${XDG_SESSION_TYPE:-?} escritorio=${XDG_CURRENT_DESKTOP:-?} sdl_gl=${WEFT_SDL_GL:-auto}"; qemu-system-x86_64 --version | head -n1
    nvidia-smi --query-gpu=driver_version,memory.used,memory.total --format=csv,noheader 2> /dev/null | head -n 1
    echo "memoria de GPU por proceso:"; nvidia-smi --query-compute-apps=name,used_memory --format=csv,noheader 2> /dev/null | sed -E "s#.*/##" | sort -t, -k2 -rn | head -n 6
    free -g | awk 'NR==2 { print "memoria del equipo (GiB): total=" $2 " usada=" $3 " disponible=" $7 }'
    exit 0 ;;
  arrancar) ;;
  *) die "orden desconocida: $1 (arrancar, parar, captura, diagnostico, rotar, resolucion, adb, doctor, imagen, root, traductor)" ;;
esac

paso "Requisitos"
command -v qemu-system-x86_64 > /dev/null || die "falta qemu-system-x86_64"
command -v curl > /dev/null || die "falta curl"
[ -r /dev/kvm ] && [ -w /dev/kvm ] || die "no hay acceso a /dev/kvm (virtualizacion desactivada o sin permiso)"
qemu-system-x86_64 --version | head -n1
RUT=no; qemu-system-x86_64 -device help 2>&1 | grep -q virtio-gpu-rutabaga && RUT=si
echo "dispositivo virtio-gpu-rutabaga en QEMU: $RUT"
if [ "$DISP" = auto ]; then
  # GTK repinta en diferido y acepta cualquier orden de color. SDL, con gfxstream, parpadea mostrando el mensaje
  # "Display output is not active" (modo 2D) o aborta cuando Android entrega la imagen en RGBA (modo OpenGL).
  if qemu-system-x86_64 -display help 2>&1 | grep -qx gtk; then DISP=gtk
  elif qemu-system-x86_64 -display help 2>&1 | grep -qx sdl; then DISP=sdl; echo "AVISO: este QEMU no trae ventana GTK; se usa SDL, que parpadea con gfxstream"
  else die "este QEMU no trae ventana GTK ni SDL (instala qemu-ui-gtk) o usa WEFT_DISPLAY=none"; fi
fi
# la pantalla tactil virtio no recibe el raton de las ventanas gtk/sdl (solo mandan posiciones absolutas): con ellas, wacom
if [ "$POINTER" = multitouch ] && { [ "$DISP" = gtk ] || [ "$DISP" = sdl ]; }; then
  POINTER=wacom; echo "AVISO: la ventana $DISP no manda toques a la pantalla tactil virtio; el raton usa wacom (WEFT_DISPLAY=window permite multitouch)"
fi
if [ -z "${WEFT_DIR:-}" ]; then
  paso "weft (ultima version publicada)"
  # se descarga siempre (4 MB): asi el guion y el programa son de la misma version
  # RIESGO: releases/latest no esta fijado ni se comprueba (ni tamano ni sha256, cambian en cada version): se instala lo que haya
  # publicado en ese momento, incluida una version rota o, si alguien tomara el control del repositorio, una maliciosa. Se deja asi
  # a proposito para que guion y programa vayan a la par; con WEFT_DIR se usa un weft ya instalado y no se descarga nada
  wf stop --timeout 10 > /dev/null 2>&1; wf kill > /dev/null 2>&1
  curl -fL --progress-bar -o weft.tar.gz https://github.com/43fdfdg45454/weft/releases/latest/download/weft-linux-x86_64.tar.gz || die "descarga de weft"
  rm -rf weft && tar -xzf weft.tar.gz && rm -f weft.tar.gz || die "descomprimir weft"
fi
[ -x "$WF" ] || die "no esta el ejecutable de weft"
PKG="$(dirname "$WF")"
# valores del archivo `config` (weft config): WEFT_* manda; sin WEFT_*, `config`; sin `config`, el valor por defecto del guion
cfgv() { wf config get "$1" 2> /dev/null; }
# nucleos y memoria: `weft device resources` ya aplica `config` > perfil de dispositivo (auto = ninguno de los dos); un weft sin
# esa orden no imprime nada y se lee solo `config`
RECURSOS="$(wf device resources 2> /dev/null)"
recurso() { v="$(printf '%s\n' "$RECURSOS" | sed -n "s/^$1=//p")"; [ -n "$RECURSOS" ] || v="$(cfgv "$2")"; case "$v" in ""|auto) ;; *) printf '%s' "$v";; esac; }
[ -n "${WEFT_MEM:-}" ] || { v="$(recurso ram maquina.ram)"; [ -n "$v" ] && MEM="$v"; }
[ -n "${WEFT_CPUS:-}" ] || { v="$(recurso cpus maquina.cpus)"; [ -n "$v" ] && CPUS="$v"; }
[ -n "${WEFT_MACHINE:-}" ] || { v="$(cfgv maquina.tipo)"; [ -n "$v" ] && MACHINE="$v"; }
[ -n "${WEFT_GPU:-}" ] || { v="$(cfgv maquina.gpu)"; [ -n "$v" ] && GPU="$v"; }
[ -n "${WEFT_GAMEPAD:-}" ] || { v="$(cfgv gamepad)"; [ -n "$v" ] && GAMEPAD="$v"; }
# resolucion: WEFT_RES, o pantalla.resolucion (+ pantalla.densidad) de `config`, o 720x1348 (con @DPI opcional)
RES_ARG="${WEFT_RES:-}"
if [ -z "$RES_ARG" ]; then
  v="$(cfgv pantalla.resolucion)"; case "$v" in ""|auto) ;; *) RES_ARG="$v"; d="$(cfgv pantalla.densidad)"; case "$d" in ""|perfil) ;; *) RES_ARG="$v@$d";; esac;; esac
fi
[ -n "$DATA" ] || DATA="$(cfgv disk.data)"; DATA="${DATA:-24G}"
RES_ARG="${RES_ARG:-720x1348}"
RES="${RES_ARG%%@*}"; DPI=""; case "$RES_ARG" in *@*) DPI="${RES_ARG##*@}";; esac
echo "bibliotecas gfxstream: $([ -f "$PKG/gfx/librutabaga_gfx_ffi.so.0" ] && echo si || echo no)"

NEED=12; case "$DATA" in *[Gg]) NEED=$((12 + ${DATA%[Gg]}));; esac
LIBRE=$(df -Pk . | awk 'NR==2 {print int($4/1048576)}'); echo "espacio libre: ${LIBRE} GiB (hacen falta unos $NEED como maximo)"
[ "$LIBRE" -ge "$NEED" ] || die "espacio insuficiente"

paso "Imagen de Android (1,2 GB la primera vez)"
# La imagen la elige WEFT_IMAGE, o la clave image.id de `config`, o la unica instalada. Si no hay ninguna (o se pide WEFT_BUILD) este guion BAJA
# CON CURL el zip de la imagen de prueba (weft no descarga nada) y lo instala con `image add ZIP`: weft lo descomprime,
# comprueba que sea compatible y lo deja en images/ID. Si ya esta instalada no vuelve a copiarla.
bajar_imagen() {
  local b="${1:-16373615}" url
  url="${WEFT_IMAGEN_URL:-https://ci.android.com/builds/submitted/$b/aosp_cf_x86_64_only_phone-userdebug/latest/raw/aosp_cf_x86_64_only_phone-img-$b.zip}"
  bajar "$url" "cache/downloads/aosp_cf_x86_64_only_phone-img-$b.zip" || return 1
  wf image add "cache/downloads/aosp_cf_x86_64_only_phone-img-$b.zip" --profile phone-x86_64 || return 1
  rm -f "cache/downloads/aosp_cf_x86_64_only_phone-img-$b.zip"
}
if [ -n "${WEFT_IMAGE:-}" ]; then
  IMG="$WEFT_IMAGE"
elif [ -n "${WEFT_BUILD:-}" ]; then
  bajar_imagen "$WEFT_BUILD" || die "imagen de Android"
  IMG="phone-x86_64-$WEFT_BUILD"
elif IMG="$(wf image current 2> /dev/null)"; then :
else
  bajar_imagen || die "imagen de Android"
  IMG="$(wf image current)" || die "no se pudo saber la imagen instalada"
fi
echo "imagen: $IMG"
wf image info "$IMG" | grep -E "^(perfil|compilacion|tamano|completa):" | sed 's/^/  /'

paso "Disco y arranque"
# el disco es de esta maquina y de esta imagen (machines/prueba/disks/IMAGEN.img); weft lo arma si falta
wf disk status --image "$IMG" | grep -q "no existe" && { wf disk create --image "$IMG" ${WEFT_DATA:+--data "$WEFT_DATA"} || die "disk create"; }

wf stop --timeout 5 > /dev/null 2>&1; wf kill > /dev/null 2>&1
A=(start --image "$IMG" --machine "$MACHINE" ${KERNEL_ARGS[@]+"${KERNEL_ARGS[@]}"} --mem "$MEM" --cpus "$CPUS" --gpu "$GPU" --resolution "$RES"
   --display "$DISP" --display-gl "${WEFT_SDL_GL:-auto}" --pointer "$POINTER" --gamepad "$GAMEPAD" --audio "$AUDIO"
   --adb "${WEFT_ADB:-auto}" ${CID:+--vsock-cid "$CID"})
[ -n "$DPI" ] && A+=(--density "$DPI")
SHARE="${WEFT_SHARE-}"; [ -z "$SHARE" ] && [ -d "$ORIG/Shared" ] && SHARE="$ORIG/Shared"; [ "$SHARE" = none ] && SHARE=""
case "$SHARE" in ""|/*) ;; *) SHARE="$ORIG/$SHARE" ;; esac
if [ -n "$SHARE" ]; then [ -d "$SHARE" ] || die "WEFT_SHARE: no existe la carpeta $SHARE"; A+=(--folder "$(basename "$SHARE")=$SHARE"); fi
# el perfil de la imagen trae gpu.opciones=edid=off (60 Hz); WEFT_GPU_OPTS manda sobre el perfil (WEFT_GPU_OPTS= vacio lo quita)
[ -n "${WEFT_GPU_OPTS+x}" ] && A+=(--gpu-opts "$WEFT_GPU_OPTS")
echo "kernel=$([ -n "${WEFT_KERNEL:-}" ] && echo propio || echo "de la imagen")"
echo "carpeta compartida=$([ -n "$SHARE" ] && basename "$SHARE" || echo "ninguna (mas en config: share.*)")"
echo "gpu=$GPU pantalla=$DISP puntero=$POINTER maquina=$MACHINE resolucion=$RES${DPI:+@$DPI} audio=$AUDIO adb=${WEFT_ADB:-auto}"
wf "${A[@]}" || { echo "--- registro de QEMU:"; tail -n 15 $STATE/qemu.log 2>/dev/null | sed -E 's#/[^ ]*/##g'; die "no arranco la maquina"; }

echo
echo "Android esta arrancando (la primera vez tarda cerca de un minuto)."
echo "  apagar:   bash $0 parar"
echo "  captura:  bash $0 captura"
echo "  si se cierra sola:  bash $0 diagnostico"
# el adb propio va por vsock (el CID queda en el estado: vsock-cid) o por un puerto TCP local, segun WEFT_ADB y lo que permita el equipo
ADB=si; echo "  adb:      bash $0 adb ORDEN   (adb propio de weft por $(tr -d '\n' < "$STATE/adb-transport" 2> /dev/null); no hace falta tener adb instalado)"

arrancado() { grep -a -q -E "Boot is finished|Starting phase 1000" "$LOGA" 2> /dev/null; }

# Ajustes dentro de Android que weft aplica por el adb propio (`weft apply-settings`, idempotente; tambien los
# repite la ventana tras cada arranque de Android): Bluetooth apagado (android.bluetooth) y, con el puntero wacom, que la
# tableta USB que recibe el raton cuente como pantalla tactil.
if [ "$ADB" = si ]; then
  paso "Ajustes en Android (raton como toque, Bluetooth apagado)"
  T=0; while [ "$T" -lt 240 ] && ! arrancado; do wf status > /dev/null 2>&1 || break; sleep 3; T=$((T+3)); done
  if arrancado; then
    wf wait-adb --timeout 60 > /dev/null 2>&1 || echo "AVISO: adbd no respondio a tiempo"
    wf apply-settings || echo "AVISO: no se pudieron aplicar los ajustes de Android"
    # carpetas compartidas: el servicio de init que las monta debe estar en el invitado (una vez; queda en el disco)
    if [ -n "$SHARE" ] || ! wf share list 2> /dev/null | grep -q "no hay carpetas"; then
      wf share setup 2>&1 | sed 's/^/compartir: /'
    fi
    # root persistente, si esta pedido en la configuracion (root.cargar_al_inicio): si ademas debe instalarse solo y faltan los archivos,
    # este guion los baja antes a la carpeta que usa `root ensure` (cache/root)
    if [ "$(cfgv root.cargar_al_inicio)" = si ] && [ "$(cfgv root.instalar_automaticamente)" = si ]; then
      wf root status 2> /dev/null | grep -q "instalado para cada arranque: si" || bajar_root cache/root || echo "AVISO: no se pudieron bajar los archivos de root"
    fi
    wf root ensure 2>&1 | sed 's/^/root: /'
  else
    echo "AVISO: Android no termino de arrancar a tiempo; ajustes sin aplicar"
  fi
fi

if [ -n "${WEFT_WAIT:-}" ]; then
  paso "Esperando el arranque (hasta $WEFT_WAIT s)"
  T=0; OK=no
  while [ "$T" -lt "$WEFT_WAIT" ]; do
    wf status > /dev/null 2>&1 || { echo "la maquina termino sola"; break; }
    if arrancado; then OK=si; break; fi
    sleep 5; T=$((T+5))
  done
  echo "arranque_completo=$OK segundos=$T"
  sleep 10; wf screenshot screenshot.png && echo "captura guardada en $W/screenshot.png"
  echo "motor grafico: $(grep -a -o -m1 -E 'Virtio-GPU GFXStream[^)]*\)|SwiftShader Device[^)]*\)' "$LOGA" | head -n1)"
  grep -a -m3 -i -E "Abort message|Fatal signal" "$LOGA" | cut -c1-200
  [ "$OK" = si ] || { tail -n 5 $STATE/qemu.log 2>/dev/null | sed -E 's#/[^ ]*/##g'; exit 2; }
fi
