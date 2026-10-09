# weft

<img src="flatpak/plantillas/app.svg" alt="" width="96" align="right">

Emulador mínimo para Linux: lanza una máquina virtual con QEMU acelerada por KVM y la controla por comandos.
Está escrito en Rust sin dependencias. Solo Linux.

Todo se puede manejar sin ventana, así que la mayor parte se prueba de forma automática en el CI (ver «Pruebas» y «CI»). No todo:
`launch` (el CI solo comprueba su salida sin imagen, en la tarea `flatpak`), `restart`, `resolution`, `rotate`, `bridge install`,
`root enable`, `share add`, los mandos y `report` no tienen prueba de extremo a extremo en el CI: su lógica tiene pruebas
unitarias (algunas contra un invitado simulado) y el resto se prueba a mano.

## Requisitos

- `qemu-system-x86_64` instalado.
- Acceso a `/dev/kvm` para tu usuario. Sin KVM funciona igual por emulación de software, mucho más lento.
- No hace falta `adb`: weft trae su propio cliente adb (ver "adb propio"). Para el adb por vsock hace falta
  `/dev/vhost-vsock` accesible. `weft doctor` comprueba todo esto y explica qué falta.

## Uso

```
weft start --kernel vmlinuz --initrd initrd.img --append "console=ttyS0"
weft status
weft wait-serial "texto esperado" --timeout 60
weft sh "uname -a"
weft type "hola\n"
weft key ctrl-alt-f1
weft tap 0.5 0.5
weft screenshot pantalla.png
weft stop
weft restart
```

`weft --help` lista todas las órdenes y opciones. `weft --version` dice la versión y, en los binarios del CI, el commit con
que se compilaron (lo mismo aparece en «Acerca de» y en el informe de `weft report`). Las opciones globales (`--name`, `--root`, `--data-dir`, `--config-dir`, `--cache-dir`, `--logs-dir`, `--state-dir`) van antes de la orden.
Sin `--kernel`/`--initrd`/`--disk`, `weft start` arranca la imagen de la máquina (ver "Imágenes de Android, perfiles y carpetas").

- `stop [--timeout S]`: APAGADO. Primero ordenado por el adb propio (`sys.powerctl=shutdown`: Android cierra sus servicios,
  sincroniza y desmonta los discos y apaga el hardware virtual; la maquina termina sola en unos 2 s y el disco queda desmontado
  limpio, sin recuperacion al arrancar). Si adbd no responde, no acepta la orden o esta falla (p. ej. `setprop` sin permiso:
  entonces el boton ACPI tiene el tiempo completo), o si QEMU no termina, sigue la escalera: boton de apagado ACPI
  (`system_powerdown`; el invitado de prueba lo ignora), `quit` de QMP y SIGKILL. Es el mismo codigo de `restart` y del boton
  **Apagar** de la interfaz. Medido en la maquina de prueba: 1,6 s (con el botón ACPI, 20 s de espera y `quit`, con el sistema de
  archivos marcado como apagado brusco). `WEFT_APAGADO=powerctl|svc-power|reboot-p|reboot-servicio` fuerza otro metodo, solo para pruebas.
- `restart [--timeout S] [--wait S]`: REINICIO COMPLETO de la maquina. Apaga (como `stop`: por adb, luego ACPI y por ultimo forzado), espera a que
  la anterior termine del todo y arranca de nuevo con LOS MISMOS argumentos con que se arranco: `start` los guarda en el
  archivo `arranque` del estado (argumentos, carpeta de trabajo y el entorno que importa: `WEFT_*`, `SDL_*` y las variables
  de escritorio; lo que traiga el entorno de `restart` manda sobre lo guardado), asi que tambien levanta la ventana propia
  si la tenia. Reintenta (hasta 15 s) la carrera `vhost-vsock: unable to set guest cid: Address already in use` de justo
  despues de apagar, conserva la orientacion pedida y, con `--wait S`, espera a `boot_completed` (sin ventana propia aplica ademas
  `apply-settings`, `share setup` y `root ensure`, como el guion; con ventana lo hace su hilo). **No conserva**: las apps
  abiertas (Android arranca de cero; el disco no se toca), las conexiones de mandos hechas a mano (vuelven con
  `--gamepad auto`) ni el proceso de la ventana (es otro proceso: se cierra y se abre una nueva, con el mismo zoom por
  `config`). Sin archivo `arranque` da un error claro: `stop` y `start`.
- **Reiniciar nunca usa `system_reset`**: reinicia el hardware virtual sin que Android cierre sus contextos graficos y el
  renderizador gfxstream del anfitrion conserva el estado viejo, de modo que SurfaceFlinger no vuelve a arrancar (ver `BITACORA.md`). Hay dos vias:
  el *reinicio ordenado de Android* por el adb propio (`weft reboot`; es lo que hace el boton Reiniciar Android) y el
  reinicio completo (`restart`).
- `--display gtk` abre una ventana (funciona en Wayland); por defecto no hay ventana.
- `--display window` abre la ventana propia de weft (SDL3 del sistema, cargada en tiempo de ejecucion):
  QEMU entrega la pantalla por D-Bus (`-display dbus,p2p=yes`, paquete `qemu-ui-dbus`) y la ventana descarta la
  imagen "Display output is not active" que QEMU intercala en cada cambio de superficie con gfxstream, asi que no
  parpadea ni se cae como SDL de QEMU, y la resolucion la fija `--resolution`, no la ventana. Raton (como toques de la
  pantalla tactil virtio con `--pointer multitouch`, o absoluto con wacom/tablet) y teclado por D-Bus; la ventana gira su
  presentacion con `rotate` y sigue los cambios de `resolution`; cerrar la ventana apaga la maquina. `WEFT_WINDOW_STATS=SEGUNDOS` anota los cuadros por
  segundo en `window.log`.
- `weft window [--wait]`: si la ventana propia de una máquina en marcha ya no está (se cerró su proceso o se cayó), abre otra igual que la
  de `start` (misma resolución del arranque); si ya hay una, lo dice ("ya hay una ventana (pid N)"). La ventana tiene tomado el cerrojo
  `window.lock` del estado mientras vive, así que se sabe aunque su pid sea de otra instancia de Flatpak. `launch` hace lo mismo si
  encuentra la máquina en marcha sin ventana. Si la ventana termina porque la máquina se cayó (no por un apagado ordenado), la causa, las
  últimas 30 líneas de `qemu.log` y los últimos eventos quedan en `<registros>/machines/NOMBRE/ultimo-fallo.txt`; y cada `start` conserva
  los registros de la ejecución anterior como `qemu.log.1`, `events.log.1` y `window.log.1`.
- Si un arranque falla a medias (QEMU que no está o no responde por su control en 15 s, un servicio que no se lanza), `start` deshace lo
  que ya lanzó: termina los virtiofsd, mata a QEMU si llegó a escribir su pid y borra ese pid. Los servicios que sobreviven a la orden
  (virtiofsd, sensores, eventos, mandos y la ventana) van en su propio grupo de procesos: un Ctrl+C sobre `launch` no los mata.
- Aceleracion por GPU (gfxstream: Vulkan y OpenGL ES del invitado ejecutados por la GPU del anfitrion). Por defecto (`--gpu auto`) se activa si weft encuentra con que:
  - QEMU del sistema + carpeta `gfx/` junto al ejecutable (o `WEFT_GFX_DIR`) con rutabaga compilado con gfxstream. Es el caso de Fedora: su QEMU trae el dispositivo `virtio-gpu-rutabaga` pero su rutabaga no incluye gfxstream. El paquete publicado trae esa carpeta para Fedora 44.
  - o un QEMU propio completo en `qemu/` (o `WEFT_QEMU_DIR`), para distribuciones cuyo QEMU no trae rutabaga (Ubuntu).
  - Validado en CI (Ubuntu y Fedora 44) con Vulkan por software. Sin validar con Nvidia ni con Wayland real.
- `scripts/dev-fedora.sh`: prueba completa en Fedora/Bazzite con el QEMU del sistema (el guion baja con curl la imagen Cuttlefish y la instala con `image add`, arma el disco y arranca en una ventana; weft en si no descarga nada).
- `make-disk` acepta imagenes dispersas de Android (formato simg) y las expande.
- `--gpu virtio-gl` usa la GPU del equipo para el invitado (virgl). Sin validar con Nvidia.
- `--adb-port 5601` reenvía ese puerto local (1 a 65535) al 5555 del invitado, para `adb connect 127.0.0.1:5601`.
- `--resolution ANCHOxALTO` admite de 64 a 8192 por lado (los mismos límites que `config` y `resolution`); un ancho que no es múltiplo
  de 8 se admite con un aviso (Linux lo redondea). `--audio wav:F` no crea el archivo al leer las opciones (con `--dry-run` no se toca
  nada): lo crea QEMU al arrancar; la carpeta tiene que existir. Ninguna ruta que va dentro de una opción de QEMU puede llevar comas
  (discos, CD, carpetas compartidas, wav, `--hvc-log` y la carpeta de estado, que lleva los sockets): se rechaza con un error que lo dice.
- `--disk imagen[,ro]` y `--cdrom imagen.iso` añaden almacenamiento.
- `sh` necesita que el invitado tenga un intérprete de órdenes en la consola serie.

## adb propio y `doctor`

weft habla el protocolo de adbd directamente por vsock (CID del invitado, puerto 5555) o por TCP (`src/adb.rs`), sin servidor
adb ni el binario de Google: ya no hace falta tener `adb` instalado, ni siquiera para `scripts/dev-fedora.sh`. El adb
de Google puede conectarse en paralelo (adbd admite varias conexiones). Todas las órdenes aceptan `--cid N` antes de sus
argumentos; sin ella se usa el CID con que se arrancó la máquina (archivo `vsock-cid` del estado) o 3. También `--timeout S`:
segundos sin actividad de adbd tras los que la orden falla en vez de quedarse colgada con un invitado colgado (120 por defecto;
`--timeout 0` quita el límite; `logcat` en continuo no lo tiene, y si se corta la conexión lo dice; en `wait-adb` es la espera
total).

**adb por TCP** (`start --adb auto|vsock|tcp|both`, clave `adb.transporte`, variable `WEFT_ADB` del guion): QEMU reenvía un puerto de
127.0.0.1 (`adb.puerto`, por defecto el primero libre desde 15555) al adbd del invitado (`persist.adb.tcp.port=5555`) por la red
de usuario. Sirve donde no hay `/dev/vhost-vsock` o AF_VSOCK está bloqueado (Flatpak) y con varias máquinas a la vez (cada una su
puerto, sin pelear por el CID). `auto` (por defecto): vsock si el equipo lo permite, si no TCP con un aviso. `both` activa las dos
(manda vsock; `--via tcp [--port P]` en cualquier orden del adb fuerza TCP). La vía queda en `adb-transport` y `adb-port` del estado
y la muestra `status`. **Varias máquinas a la vez** (`--name`), sin indicar nada: el CID de vsock es único en todo el equipo y `start` elige
el primero libre desde 3 entre las máquinas en marcha, y el puerto del adb por TCP el primero libre desde 15555; los dos quedan en el
estado (`vsock-cid`, `adb-port`), que es de donde los toman las demás órdenes. Dos `start` a la vez no eligen lo mismo: la elección se hace
con un cerrojo (`assign.lock`, `flock`, en la carpeta de ejecución) tomado hasta que QEMU arrancó con ellos. Si lo elegido lo toma otro
programa mientras QEMU arranca (otra máquina virtual ajena a weft con ese CID, un programa con ese puerto), se reintenta con el siguiente
libre. Lo pedido a mano (`--vsock-cid N`, `--adb-port P`, `adb.puerto`) no se cambia: si otra máquina en marcha ya tiene ese CID, `start`
lo dice y propone uno libre o el adb por TCP; un puerto ocupado se avisa. Cada perfil de imagen declara qué vías admite y por qué tarjeta de red sale TCP (`adb.tcp_red`): en el
teléfono virtual solo la 3.ª tarjeta obtiene dirección (medido), y la imagen de coche no obtiene ninguna: solo vsock.

```
weft adb-shell getprop ro.build.version.release   # shell de Android: stdout y stderr separados y código de salida (shell,v2)
weft push archivo.bin /data/local/tmp/             # sync: SEND (si el destino es un directorio, con el mismo nombre)
weft pull /data/local/tmp/archivo.bin .            # sync: RECV
weft install [-r] [-d] [-g] [-t] [--abi arm64-v8a] app.apk           # exec:cmd package install -S TAMAÑO (como el adb moderno)
weft uninstall [-k] paquete
weft logcat [-d] [-t 20] [ARGS]                    # en continuo hasta Ctrl+C
weft reboot [bootloader|recovery]                  # reinicia Android (la máquina vuelve sola, ~20 s con la caché caliente)
weft root | remount | disable-verity               # como adb (remount y disable-verity pasan antes adbd a root)
weft wait-adb [--timeout S]                        # espera a que adbd responda y a sys.boot_completed=1
```

- `sh` (consola serie) se mantiene; `adb-shell` no abre un shell interactivo (hay que darle la orden).
- Autenticación: las imágenes userdebug como Cuttlefish (`ro.adb.secure=0`) aceptan la conexión sin firma. Si adbd pide AUTH
  (`ro.adb.secure=1`), weft hace lo mismo que el adb de Google, sin crates (`src/rsa.rs`: enteros grandes, primos con criba y 40 rondas de
  Miller-Rabin con azar de `/dev/urandom`): la primera vez que hace falta crea una clave RSA de 2048 bits en la carpeta de configuración
  (`<config>/adbkey`, PEM PKCS#1 con permisos 0600, y `adbkey.pub` en el formato de Android; las dos herramientas pueden usar la misma
  clave), firma el testigo de adbd (PKCS#1 v1.5 con el DigestInfo de SHA-1) y, si Android no conoce la clave, se la manda con el
  comentario fijo `weft` (sin usuario ni nombre del equipo) y espera a que aceptes «Permitir depuración» en la pantalla de Android hasta el
  límite de la orden (`--timeout`). Si no se acepta a tiempo, el error lo dice.
- **Seguridad en equipos compartidos**: sin firma, cualquier usuario del mismo equipo puede conectarse al adbd del invitado
  (el puerto TCP escucha en 127.0.0.1 y vsock no distingue usuarios), y la ventana pasa adbd a root tras cada arranque.
  Eso da un shell root en Android y acceso a las carpetas compartidas del anfitrión. Usa weft en un equipo de un solo
  usuario o con `--no-net` y sin carpetas compartidas si hay otros usuarios.
- Un solo servicio abierto por conexión, sin multiplexar; sin TLS, sin `shell` interactivo con terminal, sin pull de directorios.
- `weft doctor` comprueba el equipo y lo explica en una tabla OK / AVISO / FALLO: `qemu-system-x86_64` (versión 10 o
  mayor), KVM, `/dev/vhost-vsock`, `qemu-ui-dbus` (pantalla D-Bus de QEMU), ventanas gtk/sdl, SDL3 (`dlopen`), `gfx/`
  (rutabaga + gfxstream), `/dev/uinput` y permisos de `/dev/input` (mandos), espacio libre en la carpeta de datos (si aún no
  existe, donde se creará; la fila dice dónde midió) y, con una máquina en marcha, que adbd responde por la vía con que arrancó
  (`vsock:CID` o `tcp:127.0.0.1:PUERTO`). Sale con 0 solo si no hay FALLO. Solo lee: no cambia nada del sistema.

## weft no descarga nada

weft no necesita internet y no lleva integradas funciones de descarga: ni imágenes, ni los componentes del acceso root, ni la
biblioteca del traductor ARM. Todo se instala desde archivos o carpetas ya descargados, con mensajes que dicen qué archivo se espera:

```
weft image add ZIP|CARPETA          # imagen de Android
weft root enable --from CARPETA     # carpeta con los archivos del acceso root (tamaño exacto comprobado)
weft bridge install --file LIB      # biblioteca del traductor ARM
```

Una prueba (`tests/sin_descargas.rs`) lee las fuentes de `src/` y falla si aparece `curl`, `wget`, `http://` o `https://`. Quien baja es
el usuario o el guion de desarrollo `scripts/dev-fedora.sh` (curl con comprobación de tamaño; subórdenes `root` y `traductor`).

## Flatpak

Se distribuye como paquete `.flatpak` (no Flathub): `weft-linux-x86_64.flatpak` en cada release (lo construye el CI) o construido a mano.
Identificador provisional: `io.github._43fdfdg45454.weft` (se cambia en un solo sitio, la variable `APP_ID` de `flatpak/construir.sh`).

- **Requiere Flatpak 1.15.6 o más nuevo** en el equipo: el manifiesto pide `--device=input` (mandos de juegos), que no existe en versiones
  anteriores, y `flatpak install` rechaza el permiso (el paquete entero no se instala). Ubuntu 24.04 y Debian 12 traen Flatpak 1.14 y lo
  rechazan; Fedora 44 trae 1.16 o más nuevo. `flatpak --version` lo dice.
- **Contenido**: QEMU 10.2.2 de upstream sin parches (modulos: pantalla D-Bus, `virtio-gpu-rutabaga`, audio PipeWire y PulseAudio, KVM), rutabaga y
  gfxstream compilados DENTRO del paquete (la aceleración gráfica por hardware no descarga nada en ejecución), virtiofsd y weft.
  Rutabaga se compila con la misma cabecera con la que se compila QEMU: no hace falta el parche de una línea del QEMU de Fedora.
- **Permisos** (`finish-args`): Wayland/X11, `--device=dri` (GPU), `--device=kvm`, `--device=input` (mandos), audio, `WEFT_QEMU_DIR=/app/qemu`.
  `--share=network` (red del equipo: internet para Android y el adb por TCP alcanzable desde cualquier `flatpak run`; weft sigue sin descargar
  nada). Sin acceso a tus carpetas: para agregar una imagen o compartir una carpeta, concédelo con
  `flatpak override --user --filesystem=CARPETA io.github._43fdfdg45454.weft` (o copia el zip a la carpeta de datos de la aplicación).
- **Lo único que Flatpak baja por su cuenta** al instalar el paquete es el runtime `org.freedesktop.Platform//25.08` (si no lo tienes) y, con Nvidia, la
  extensión GL del controlador (`org.freedesktop.Platform.GL.nvidia-<versión>`). La aplicación en sí no descarga nada.
- **Directorios**: los estándar de Flatpak (`~/.var/app/<id>/{data,config,cache}`; `weft paths`); el estado de ejecución va en
  `$XDG_RUNTIME_DIR/app/<id>/weft` (lo único que comparten las instancias de la misma aplicación). `--root`/`--data-dir`... siguen valiendo.
- **adb por TCP** automáticamente (AF_VSOCK está bloqueado por el filtro seccomp de Flatpak): un puerto de 127.0.0.1 reenviado por QEMU. Con
  `--share=network` todas las instancias comparten la red del equipo, así que `flatpak run --command=weft <id> adb-shell ...` (o `stop`, que
  apaga por adb) desde otra instancia alcanza el adb de la máquina que arrancó `launch`.
- **Solo la ventana propia** (`--display window`); sin las ventanas gtk/sdl de QEMU.
- **`weft launch`** (lo que ejecutan el icono del menú y `flatpak run <id>`: la orden del paquete es `weft-launch`, un envoltorio de `weft launch`;
  `weft` a secas imprime la ayuda, así que cualquier otra orden va con `flatpak run --command=weft <id> ORDEN`): usa la imagen ya instalada de la máquina (o la única), arma el disco si falta y abre la
  ventana propia con los directorios estándar. Se queda en primer plano mientras la máquina vive y su ventana sigue abierta (el sandbox termina con él). Sin imagen instalada
  explica cómo agregar una desde un archivo, con las órdenes de `flatpak` ya escritas con el identificador real de la aplicación; como desde el
  icono no hay terminal, lo deja también en `<registros>/machines/NOMBRE/launch.log` (con fecha; ahí van también sus errores) y, si hay sesión
  de escritorio, en una notificación (`Notify` de `org.freedesktop.Notifications` y, si el sandbox no la deja pasar, el portal de
  notificaciones). Con la máquina ya en marcha y sin ventana, la vuelve a abrir. Acepta las opciones de `start` (`--audio none`...),
  que mandan sobre sus valores. Dentro de Flatpak, `launch --no-wait` y `start` también se quedan en primer plano (lo avisan): si volvieran,
  el sandbox terminaría y la máquina con él.
- El acceso root y el traductor ARM se instalan también desde archivos (`root enable --from`, `bridge install --file`; la pantalla de configuración
  tiene un campo para la ruta).
- Los campos de ruta de la pantalla de configuración tienen un botón «Examinar...» que abre el selector de archivos del sistema (portal de
  escritorio, que el sandbox deja pasar y que da acceso al archivo elegido). También se puede soltar el archivo o la carpeta sobre la ventana,
  pegar la ruta (Ctrl+V, también una copiada en el gestor de archivos) o escribirla con `~`.
- Construir: `bash flatpak/construir.sh` (genera `flatpak/generado/` desde las plantillas y llama a flatpak-builder; ver `flatpak/README.md`).

## Valores por defecto del guion (`scripts/dev-fedora.sh`)

- Maquina `--machine q35` (`WEFT_MACHINE=pc` la vuelve a `pc`). Medido contra `pc` con el mismo guion: arranque completo y
  sys.boot_completed a los mismos ~3,8 s de reloj del invitado, gfxstream activo, 60 Hz, adb por vsock, audio y banco
  dentro del ruido normal. q35 es lo que permite conectar mandos en caliente (PCIe).
- Puntero `--pointer multitouch` (`WEFT_POINTER=wacom|tablet` lo cambia): el raton es un contacto de la pantalla tactil
  virtio (`virtio-multitouch-pci`). Con `--display window` la ventana manda los toques por D-Bus (clic, arrastre, pulsacion
  larga, rueda como deslizamiento y pellizco con dos contactos) y no hace falta configurar nada en Android (ni archivos
  `idc` ni `replug-pointer`, que quedan solo para wacom). `tap` y `click` de la consola tambien tocan por esa pantalla
  tactil. Las ventanas `gtk` y `sdl` solo mandan posiciones absolutas, que la pantalla tactil no recibe (QEMU: "Input handler
  not found for event type abs"): con ellas el guion cambia solo a `wacom` y `weft start --pointer multitouch
  --display gtk|sdl` da error.

## Entrada: mandos, rueda, atajos y multitoque

- **Mandos de juegos** (`--gamepad auto|none|/dev/input/eventN`, por defecto `none`; en el guion `WEFT_GAMEPAD`). Cada mando
  del equipo (un evdev con botones `BTN_GAMEPAD`/`BTN_JOYSTICK` y ejes X/Y; teclados, ratones, tabletas y sensores de
  movimiento se excluyen) se entrega a la maquina como dispositivo `virtio-input-host-pci`: Android lo ve como
  `GAMEPAD | JOYSTICK`. Con `auto`, un servicio de fondo (`gamepad-serve`, vigila `/dev/input` con inotify) conecta los
  presentes y los que aparezcan y desconecta los que se vayan. Mientras un mando esta conectado a la maquina el equipo
  no lo ve (QEMU lo reserva con `EVIOCGRAB`); vuelve al apagar la maquina o con `gamepad detach`.
  - `weft gamepad list`: mandos del equipo y cuales estan conectados a la maquina.
  - `weft gamepad attach MANDO` / `gamepad detach MANDO`: conectar/desconectar a mano (ruta, `eventN`, indice de
    `list` o, para detach, el id `padN`). Un mando desconectado a mano no lo vuelve a conectar `auto` mientras siga presente.
  - La conexion en caliente necesita que el kernel del invitado la admita. El de la imagen Cuttlefish trae hotplug PCIe
    (`pciehp`) pero no el de ACPI, asi que hace falta `--machine q35` (con `--gamepad` weft reserva 4 puertos
    PCIe y desactiva el hotplug por ACPI de QEMU para que el invitado reciba el nativo). En `--machine pc` solo se ven
    los mandos presentes al arrancar. Las altas y bajas tardan unos segundos (el invitado las confirma).
  - Necesita permiso de lectura y escritura sobre `/dev/input/eventN` (en una sesion de escritorio lo concede logind).
- **Ventana (`--display window`)**:
  - Rueda del raton = deslizamiento con el lapiz (un lapiz ignora los botones de rueda): por defecto 10 % de la pantalla
    por paso, maximo 60 %, en unos 100-150 ms (`rueda.paso`, `rueda.tope` y `rueda.invertir` de la configuracion);
    vertical, y horizontal con Mayus o con rueda horizontal. No pisa un arrastre real ni un pellizco. Movimientos de rueda
    menores a 0,25 pasos se acumulan.
  - Atajos de teclado, TODOS personalizables en la pantalla de configuracion (seccion Atajos) o con `weft config set
    atajo.X TECLA`. Por defecto: F1 Atras, F2 Inicio, F3 Recientes (Meta+Tab), F5 Volumen -, F6 Volumen +, F7 Rotar (un
    paso: 0, 90, 180, 270, auto; igual que `weft rotate`), F8 Captura, F9 Abrir configuracion,
    Ctrl+= Zoom +, Ctrl+- Zoom -, Ctrl+0 Ajustar el zoom. Las teclas asignadas a un atajo no llegan a la aplicacion
    (ni al pulsar, ni al repetir, ni al soltar); las demas si. Acepta modificadores (Ctrl, Alt, Mayus) con coincidencia
    exacta; sin Ctrl ni Alt solo valen F1-F24 y las teclas de edicion, para no robarle a la aplicacion las que escriben.
    Los atajos propios de la ventana (`atajo.pasar_tecla` Ctrl+Alt+F, `atajo.pantalla_completa` F11, `atajo.barra` F10,
    `atajo.encendido` y `atajo.menu` sin asignar), el interruptor `atajos.desactivados` y los botones del raton con el puntero tactil
    (`raton.derecho` atras, `raton.central` inicio) son claves de `config` como las demas: se guardan y `weft config` las acepta.
  - Pellizco: Ctrl (o Alt: `pellizco.modificador`) + clic fija el ancla en el cursor y crea dos contactos tactiles
    simetricos respecto a ella; mover el cursor acerca/aleja y gira los contactos. Usa `virtio-multitouch-pci`, que la
    ventana agrega si no hay `--touch`; el puntero (`--pointer`) no cambia.
- **La ventana es la barra superior y la pantalla del dispositivo, nada mas** (no hay panel lateral). **Barra superior**: una franja
  fina de 34 px dibujada por weft justo debajo de la barra de titulo del escritorio, encima de la pantalla del dispositivo y a
  todo el ancho. A la izquierda lleva la pestana **Configuraciones** (con la tecla de su atajo, F9 por defecto), que abre el modal de
  configuracion (ver "Configuracion"); la lista de pestanas esta preparada para agregar mas. A la derecha, un indicador de estado:
  "Android en marcha" (o "Arrancando Android..." hasta que Android dice `sys.boot_completed=1` por el adb propio; si adbd
  rechaza al adb propio o no contesta nunca en 3 minutos, la máquina no tiene adb y pasa a "en marcha"; "Reiniciando
  Android..."...), el zoom y la rotacion, que se acorta en ventanas estrechas; un aviso breve (captura
  guardada, mando conectado...) lo sustituye unos segundos, de modo que los atajos tambien dan respuesta con el modal cerrado.
  Hover, pulsado y "pestana abierta" (mientras el modal esta abierto) como el resto del diseno. **Con el teclado**: F10
  (`atajo.barra`) lleva el teclado a la barra, con un anillo de foco en la pestana; Tab, Mayus+Tab, las flechas izquierda y
  derecha, Inicio y Fin la recorren, Intro o Espacio abren la pestana elegida y Esc (o F10 otra vez, o un clic) devuelve el
  teclado a Android. Mientras la barra tiene el teclado no llega ninguna tecla nueva a Android (solo el soltado de lo que ya
  estaba apretado). Si la configuracion se abrio asi (desde la barra con el teclado), al cerrarla (Esc, su boton o una
  confirmacion) el foco vuelve a la pestana de la barra, con su anillo, en lugar de a Android; abierta con el raton o con
  un atajo directo, el teclado vuelve a Android. Un clic o la rueda sobre la barra no
  llegan al invitado; la pantalla del dispositivo empieza 34 px mas abajo (zoom, rotacion y la transformacion de los toques ya lo
  tienen en cuenta: verificado con `getevent` en el invitado en las cuatro esquinas con rotaciones de 0, 90, 180 y 270 grados y zoom
  ajustado o 50 %). La ventana no baja de 320 x 240 dp mas la barra.
- **Controles** (primera seccion de la configuracion, F9): lo que antes ofrecia el panel lateral. Navegacion de Android (Atras,
  Inicio, Recientes), Vol-, Vol+, rotacion (0, 90, 180, 270, Auto y Rotar un paso; como `weft rotate`), Zoom (porcentaje real,
  modo, Acercar y Alejar por los pasos 25, 33, 50, 67, 75, 100, 125, 150, 200 y 300 %, 1:1 y Ajustar) y Captura
  (`captura-AAAAMMDD-HHMMSS.png`, UTC, en la carpeta de trabajo de la ventana). Cada boton hace exactamente lo de su atajo y lleva la tecla
  asignada al lado (si la cambias en Atajos, cambia aqui). **El dialogo se queda abierto** al pulsarlos: una linea de estado de
  altura fija (los botones no se mueven bajo el raton, util con Vol- y Vol+ repetidos) confirma lo ultimo enviado ("Atras enviado");
  Esc o la X lo cierran. Con la configuracion cerrada, las teclas de atajo siguen mandando lo mismo.
- **Maquina** (seccion de la configuracion): estado, tiempo encendida, CPUs, RAM y tipo (por QMP, cada 5 s con la seccion a la vista) y las
  acciones: **Reiniciar Android ahora** (reinicio ORDENADO por el adb propio, como `weft reboot`; nunca `system_reset`),
  **Reinicio completo de la maquina** (`weft restart`; siempre disponible, siempre con confirmacion y avisando de que la ventana se
  cierra y se abre otra; se destaca si adbd no responde en 4 s o Android no vuelve a arrancar en 150 s) y **Apagar la maquina**
  (lanza `weft stop`: por adb en ~2 s, con la escalera de arriba si no responde). Reiniciar Android y Apagar piden una
  caja de confirmacion (se puede desactivar: `confirmar=no`).
- **Entrada > Mandos del equipo**: la lista de mandos del equipo (la misma enumeracion que `gamepad list`, cada 2 s mientras la seccion
  esta a la vista) con el boton Conectar o Desconectar de cada uno (la misma logica que `gamepad attach` y `gamepad detach`; la
  conexion tarda unos segundos y mientras tanto el boton dice `Espere...`).
- Limitaciones: QEMU admite un solo cliente de la pantalla D-Bus y el nuevo desaloja al anterior (la orden interna
  `dbus-input` y el gancho de pruebas `WEFT_WINDOW_INJECT=1` + `window-inject` (con `pclick NOMBRE` para la pestana
  de la barra y los controles de la pantalla de configuracion; `key SC down|up [ctrl] [alt] [mayus]`) existen para probar sin tocar los dispositivos del equipo; `dbus-input` deja sin entrada a la ventana abierta, que termina avisando). Una senal de
  terminacion (SIGTERM) a la ventana equivale a cerrarla: apaga la maquina. Nada se ha probado aun con hardware
  Android/ARM real.

## Root persistente (KernelSU-Next) y carpetas compartidas

Dos funciones avanzadas que se manejan con órdenes de Rust y desde la pantalla de configuración de la ventana (secciones
*Acceso root* y *Carpetas*), sin guiones de shell. Nada de esto está validado en hardware Android/ARM real.

**Acceso root** (la interfaz lo llama así; el proveedor concreto es un detalle técnico). `weft root status|enable|disable|install-manager|ensure` (`root` a secas sigue siendo `adb root`). Usa
KernelSU-Next v3.4.0 en modo LKM: no toca imágenes ni el kernel.
- `enable [--from CARPETA] [--now] [--manager] [--reboot]` toma el `ksud` (5,3 MiB) y, con `--manager`, el gestor (11,3 MiB) de la
  CARPETA de archivos YA DESCARGADOS (`--from`, la clave `root.carpeta` de `config`, o la subcarpeta `root/` de la caché; se consiguen en
  `github.com/KernelSU-Next/KernelSU-Next/releases`, v3.4.0; se comprueba el tamaño exacto; weft no descarga nada; el guion
  `dev-fedora.sh root` los baja con curl), copia `ksud` a `/data/adb/ksud-boot` e instala el servicio de init `/vendor/etc/init/ksunext.rc`, que
  ejecuta `ksud-boot late-load` al terminar el arranque (`sys.boot_completed=1`). Vive en el disco de la máquina: sobrevive
  a los reinicios de Android y de la máquina; se pierde si se regenera el disco. `--now` carga el módulo ya; si hay otro
  módulo `kernelsu` cargado, avisa de que hace falta reiniciar Android; `--reboot` reinicia y verifica. Con `--manager` instala la
  app de KernelSU-Next (y desinstala la oficial `me.weishu.kernelsu` si está). `disable` quita el servicio y la copia (el
  módulo cargado sigue hasta reiniciar). `status` muestra instalado / cargado / versión / gestor.
- Config: `root.cargar_al_inicio` (por defecto `no`; `enable` y `disable` lo cambian) y `root.instalar_automaticamente`
  (por defecto `no`). Con `cargar_al_inicio=si`, tras cada arranque (la ventana lo hace sola; `root ensure` y el guion
  también) se comprueba que el root esté instalado y, si falta y `instalar_automaticamente=si`, se instala; si no, solo avisa.
- Aviso: con root cargado, las apps que detectan root pueden negarse a arrancar. No se midió el banco de heddle con el módulo.

**Carpetas compartidas.** Carpetas del equipo que Android ve en `/sdcard/NOMBRE` y que una app normal (Material Files, sin
root) lista, lee, crea y borra; los cambios se ven a la vez en los dos lados.
- `weft share list|add NOMBRE CARPETA [--ro]|remove NOMBRE|status|setup` y `start --folder NOMBRE=CARPETA[:ro]`
  (repetible; manda sobre la lista). La lista (hasta 4) vive en `config` (`share.0`..`share.3`, formato
  `NOMBRE:rw|ro:/ruta`). Cambiarla se aplica al próximo arranque de la máquina: los dispositivos virtiofs no se añaden en
  caliente. El nombre admite letras, números, `.`, `_` y `-` (no el de una carpeta propia de Android: DCIM, Download...).
  Necesita `virtiofsd` en el equipo (`doctor` lo comprueba).
- `share setup` (lo hacen solos `share add`, el guion y la ventana) instala en el invitado `/system/etc/init/arshare.rc`, deja
  listos los puntos de montaje y monta las carpetas del arranque que falten; la primera vez no hace falta reiniciar Android.
  `share status` muestra el estado dentro del invitado y el uid de MediaProvider.
- El guion `dev-fedora.sh` acepta `WEFT_SHARE=CARPETA|none` (una carpeta; por defecto `./Shared` si existe).
- Cómo funciona (medido; ver `BITACORA.md`): cada carpeta es un virtiofs (`arshareN`) servido por virtiofsd sin privilegios;
  Android la monta en `/data/media/0/NOMBRE` y MediaProvider (el demonio FUSE de `/sdcard`) la sirve a las apps. Como
  MediaProvider abre los archivos con su propio uid, virtiofsd presenta el dueño del equipo como ese uid
  (`--translate-uid`). Ese uid NO es estable entre instalaciones (10101 en la imagen de prueba): `share status` lo lee por
  adb y lo guarda en `share.uid`; si difiere del usado al arrancar, las escrituras fallan hasta el próximo arranque de la
  máquina. Los archivos creados desde Android son del usuario del equipo, con modo 0600/0700 (los crea MediaProvider).
- Limitaciones: máximo 4 carpetas; los archivos de la carpeta del equipo con otro dueño que el de la carpeta raíz salen con un
  uid desconocido y no se pueden escribir desde Android; quitar una carpeta deja un directorio vacío en `/sdcard`; las apps
  necesitan permiso de almacenamiento ("acceso a todos los archivos" para Material Files) como con cualquier carpeta.

## Imágenes de Android, perfiles y carpetas

**Perfiles de imagen** (`src/perfil.rs`, `perfiles/*.profile`): un perfil es DATO (clave=valor, sin código) que dice cómo se arranca una
familia de imágenes: archivos requeridos, particiones del disco GPT, arranque directo del kernel (boot, init_boot, vendor_boot),
parámetros de Android (bootconfig), consolas, sensores, tarjetas de red, opciones de pantalla, motores gráficos que admite, vías de adb,
capacidades y cómo detectarlo. Hay dos integrados en el ejecutable (`phone-x86_64`, `car-x86_64`, que hereda del primero con
`perfil.hereda`) y se agregan perfiles propios copiando un `*.profile` a `<config>/profiles/` (sin recompilar; un error se rechaza con
el número de línea; el mismo id reemplaza al integrado con aviso). Formato y claves: `docs/imagenes-android.md`. La arquitectura es una
propiedad del perfil (`perfil.arquitectura=x86_64|arm64`): x86_64 con KVM es lo soportado; otra se rechaza con un mensaje claro.

```
weft image add ZIP|CARPETA [--id ID] [--profile P]   # instala una imagen YA DESCARGADA (detecta el perfil sin arrancar nada; si no es compatible dice por qué)
weft image list | info [ID] | check ZIP|CARPETA | profiles | current   # info y profiles muestran la descripción del perfil
weft image use ID                           # la maquina usara esa imagen desde el proximo arranque
weft image remove ID [--yes]                # se niega si una maquina la usa; con discos de maquinas pide --yes
weft start [--image ID|CARPETA] ...         # arranca con la receta del perfil: las opciones mandan sobre el perfil
weft paths                                  # donde vive cada cosa
```

- `image add` deja la máquina usando la imagen nueva si su `image.id` era `auto` (o apuntaba a una imagen que ya no está) y está apagada,
  y lo dice ("la máquina usará esta imagen: ID"); así una segunda imagen no deja a `launch` con "hay varias imágenes instaladas". Si la
  máquina usa otra elegida a mano no se toca; si está en marcha (el caso de agregarla desde la ventana) `image.id` queda en la imagen con
  que arrancó, para que siga igual. En ambos casos dice cómo cambiarla (`weft image use ID`).
- **Un disco por imagen y máquina** (`<datos>/machines/NOMBRE/disks/ID.img`; con `disk.cow=si`, un overlay sobre una base compartida, ver
  «Discos de copia en escritura»): cambiar de imagen solo cambia `image.id`; el disco de la otra
  imagen no se toca (sus apps y datos vuelven al regresar) y el de la nueva se arma al primer arranque. Razones: no obliga a borrar datos,
  los datos de Android dependen de la compilación y cuestan solo espacio (`image remove ... --yes` los recupera).
- **Directorios estándar** (XDG; dentro de Flatpak caen en `~/.var/app/<id>/`): datos `$XDG_DATA_HOME/weft` (`images/ID/`,
  `machines/NOMBRE/disks/`), configuración `$XDG_CONFIG_HOME/weft` (`machines/NOMBRE/config`, `profiles/`), caché
  `$XDG_CACHE_HOME/weft` (`downloads/`, `boot/ID/` kernel y discos RAM extraídos, `root/`, `bridge/`), registros
  `$XDG_STATE_HOME/weft` (`machines/NOMBRE/android-log.txt`) y ejecución `$XDG_RUNTIME_DIR/weft/NOMBRE/` (sockets, pid y
  registros de QEMU y de la ventana). Ruta propia: `--root DIR` (todo en una carpeta: `images/`, `machines/`, `profiles/`, `cache/`,
  `state/`), `--data-dir`, `--config-dir`, `--cache-dir`, `--logs-dir`, `--state-dir` (o las variables `WEFT_ROOT`,
  `WEFT_DATA_DIR`...; las claves `dir.datos` y `dir.cache` de `config` cambian datos y caché de esa máquina). Solo `--state-dir` a una
  carpeta propia (distinta del estado de ejecución estándar) es el modo de estado propio (configuración y caché dentro de `DIR/NOMBRE`: lo usan
  las pruebas); las órdenes internas que lanzan `start` y la ventana reciben `--state-dir` con el estado estándar y las carpetas del
  padre en el entorno, así que resuelven lo mismo que él. Los nombres que crea el programa van en inglés. La carpeta de ejecución
  de cada máquina (`NOMBRE/`) y la estándar (sin `XDG_RUNTIME_DIR`, `weft-UID` en el directorio temporal) tienen que ser del
  usuario y no enlaces simbólicos: toda orden lo comprueba antes de usarlas y `start` las crea o las deja en 0700. Una carpeta
  elegida con `--state-dir` o `--root` es del usuario: puede ser un enlace y no se le cambian los permisos.
- **Gráficos**: `maquina.gpu` y `--gpu` guardan intenciones: `auto` (lo mejor para la imagen y el equipo; sin hardware cae a software con
  aviso), `hardware` (aceleración por hardware; error claro si no está) y `software`. Los nombres concretos de motor (`gfxstream`,
  `virtio-pci`) se aceptan como valores avanzados para fijarlo a mano; `none` equivale a `software`.
- Pantalla de configuración, *Imagen de Android*: lista de imágenes (perfil, compilación, tamaño, si la máquina ya tiene disco para
  ella), *Usar* con confirmación, *Agregar imagen* desde la ruta de un zip o de una carpeta (la aplicación no descarga nada), disco y regenerarlo.
- Imagen de coche (`car-x86_64`, compilación 16373615): se instala y arranca el sistema y el adb por vsock responde, pero
  `sys.boot_completed` no llega: el HAL de vehículo se conecta por gRPC a un servicio del anfitrión (192.168.98.1:9300) que weft
  todavía no implementa (pendientes: ver los issues del repositorio). Su perfil expresa lo que sí se midió: una partición de intercambio antes de los datos
  y solo adb por vsock.

## Imagen, disco, traductor ARM, ajustes de Android e informe (ordenes de Rust)

Son ordenes de `weft` (y secciones de la pantalla de configuracion):

- `weft image list|path --dir CARPETA` (una carpeta suelta). weft no descarga nada. La imagen la
  baja el usuario (o `scripts/dev-fedora.sh`, con curl) y se instala con `image add ZIP|CARPETA`: un zip se descomprime con `unzip`, `bsdtar` o
  `python3 -m zipfile` (en ese orden) y se comprueban los archivos del perfil.
- `weft disk create|status|reset [--data TAMANO|img] [--image ID|CARPETA] [--out DISCO]`: el disco GPT de la maquina para su imagen
  (el orden de particiones lo da el perfil; el movil es el de siempre: misc, metadata, frp, boot, init_boot, vendor_boot y vbmeta* en `_a`
  y `_b`, super, userdata; reutiliza `make-disk`). `start` lo arma solo si falta.
  `--data`: `24G`, `4096M` (particion vacia, minimo 2G; Android la formatea en el primer arranque) o `img` (el `userdata.img` de
  la imagen); `--data` > `disk.data` > 24G. `create` no toca un disco que ya existe. `reset --yes` borra y regenera: sin `--yes` se
  niega y explica que se pierden las apps y los datos, y se niega siempre si la maquina de ese estado esta en marcha.
  `weft factory-reset [--yes]` (volver a fabrica) es lo mismo que `disk reset` para la imagen de la maquina.
- **Discos de copia en escritura** (opcional: `weft config set disk.cow si`; por defecto no). El disco GPT se arma una sola vez por
  imagen y tamaño de datos como **base de solo lectura** compartida (`<datos>/bases/ID/DATOS.img`, permisos 0444; QEMU la abre en solo
  lectura) y el disco de cada máquina (`machines/NOMBRE/disks/ID.img`) es un overlay qcow2 sobre ella que guarda solo lo que Android
  cambia: crear el disco o volver a fábrica (`disk reset`/`factory-reset`) es un instante y varias máquinas con la misma imagen no
  repiten los GB de la imagen. weft escribe el overlay él mismo (sin `qemu-img`): qcow2 v3, clúster de 128 KiB con L2 extendido
  (subclústeres de 4 KiB: una escritura pequeña en un clúster que sigue en la base no copia el clúster entero) y metadatos reservados al
  crearlo (`preallocation=metadata`: la primera escritura no asigna tablas; el archivo es disperso y ocupa unos MiB hasta que Android
  escribe), la base guardada con ruta relativa y su formato (raw) declarado; QEMU lo abre con una caché L2 que cubre el disco entero
  (`l2-cache-size`). Los discos que ya existen (completos) siguen funcionando igual; un `disk reset` con `disk.cow=si` los pasa a
  overlay. `disk status` dice sobre qué base está el disco y cuánto ocupa la máquina. La base nunca se modifica ni se borra mientras un
  disco la use: `disk create|reset` se niegan a escribir en la carpeta de bases, y `image remove` (que borra las bases de esa imagen con
  ella) se niega si un disco que no borra (otro `--out`, el `disk.path` de una máquina) usa alguna. `disk create --out` crea siempre un
  disco completo.
  **Por qué no es el defecto** (medido con `qemu-img bench`, mediana de 7, base raw de 16 GiB con 1,5 GiB de datos, en un equipo virtual
  con disco ext4; `+` = más lento que el disco completo): con la caché que usa weft (writeback, aio threads, el defecto de QEMU) el mejor
  overlay (128 KiB + L2 extendido + metadatos) es más lento al leer: lectura secuencial 1M +22 %, 4K dispersa +17 %, escritura
  secuencial nueva +13 %, 4K dispersa nueva +8 %, reescritura −6 %, lectura de lo escrito +13 % (sin metadatos reservados: 4K nueva
  +159 %; con clúster de 64 KiB: +7 % a +38 % según la carga). Con `cache=none` y `aio=io_uring` el overlay ganaba (−10 % a −53 %, 0 % en
  escritura secuencial), pero ahí el disco completo también pierde la caché de páginas. Mientras el overlay sea más de un 5 % más lento
  en alguna carga con la configuración de weft, el defecto es el disco completo.
- `weft bridge status|install|remove|restore|check`: el traductor ARM (heddle) como native bridge. `install
  [--file LIB|CARPETA] [--no-reboot] [--boot-timeout S]` es **vigilado**: respalda lo anterior en el invitado
  (`/data/adb/heddle-backup/`), hace `disable-verity` la primera vez, `remount`, copia la biblioteca a `/system/lib64`
  (con `restorecon`), escribe las cuatro propiedades en `/vendor/build.prop`, reinicia Android y espera `sys.boot_completed`
  (90 s) vigilando las muertes de `system_server` (mas de una = fallo; 30 s de gracia tras arrancar). Si falla, **restaura sola**
  la biblioteca y las propiedades anteriores y reinicia. Salida: 0 instalado o sin cambios, 1 error previo (nada cambio), 2 fallo
  restaurado, 3 fallo y restauracion fallida. Con la misma md5 y configurado no hace nada. La biblioteca se valida antes (ELF
  x86-64 con `NativeBridgeItf`). La biblioteca ya descargada se indica con `--file` (o se deja como `libheddle.so` en la subcarpeta `bridge/` de la caché); weft no
  descarga nada (se consigue en las releases de `github.com/43fdfdg45454/heddle`; `dev-fedora.sh traductor` la baja con curl y llama a
  esta orden). Si junto a la biblioteca hay un archivo `cpu-features` (extensiones que el traductor soporta; contrato en «Perfiles de
  dispositivo»), lo copia al estado de la máquina. `remove` devuelve las propiedades originales (guardadas la primera vez) y quita la biblioteca;
  `restore` devuelve lo de antes de la ultima instalacion; `check` vigila el arranque actual si una instalacion con
  `--no-reboot` quedo sin vigilar (restaura si falla). `status` muestra instalado, md5, propiedades, arranque y la evidencia de
  carga (`libheddle.so` en el zygote64 o registro con la etiqueta `heddle`). No validado en hardware Android/ARM real.
- `weft apply-settings`: Bluetooth apagado (salvo `android.bluetooth=si`) y, con el puntero wacom, los archivos `idc` que lo
  hacen pantalla tactil. Idempotente; lo llaman el guion y la ventana tras cada arranque de Android.
- `weft report [--out ARCHIVO]`: `informe-AAAAMMDD-HHMMSS.tar.gz` sin datos personales (la carpeta personal, el usuario y el
  equipo se reemplazan por `~`, `usuario` y `equipo`; sin capturas ni `/sdcard`) con `doctor`, `config`, version, propiedades del
  invitado, el final de los registros y, con la maquina en marcha, `root`, `share` y `bridge`. Sin `--out` va a
  `<registros>/machines/NOMBRE/` (no a la carpeta actual); al terminar dice la ruta completa.
- Claves de `config` nuevas: `root.carpeta`, `disk.data` (24G), `disk.cow` (no), `android.bluetooth` (no), `image.id`, `adb.transporte`, `adb.puerto`,
  `dir.datos` y `dir.cache`.
- Pantalla de configuracion: secciones *Imagen* (estado de la imagen y del disco, tamano de datos editable,
  *Agregar imagen* desde archivo, *Regenerar disco* con dos confirmaciones en rojo y bloqueado con la maquina en marcha), *Traductor ARM*
  (estado, campo con el archivo ya descargado, *Instalar/Actualizar* con confirmacion que muestra el riesgo, *Quitar*; un retroceso automatico
  sale en rojo con el motivo) y *Diagnostico* (*Ejecutar doctor* con lineas OK/AVISO/FALLO y *Crear informe*).
- El guion `dev-fedora.sh` es un envoltorio: `image add` (tras bajar el zip con curl), `disk create`, `start --image`, `apply-settings` (que ahora reintenta unos
  segundos si adbd aun no esta listo), `share setup` y `root ensure`. Variables `WEFT_*`; `WEFT_CID` (CID de vsock;
  sin ella, el primero libre), `WEFT_IMAGE`, `WEFT_BUILD` (baja esa compilacion con curl), `WEFT_IMAGEN_URL` y `WEFT_ADB` (auto, vsock, tcp, both). Usa `--root .`: todo queda en `./weft-data` con la
  estructura de weft (`images/`, `machines/`, `state/`, `cache/`); subordenes como `imagen ORDEN...`.

## Perfiles de dispositivo

Un **perfil de dispositivo** (`src/dispositivo.rs`, `*.device`) dice lo que la máquina **anuncia** a Android y a las apps ARM y los
recursos de la máquina virtual. No confundir con los perfiles de imagen de arriba (cómo se arranca una familia de imágenes).

- **Se puede crear cualquiera.** weft no comprueba el perfil contra lo que implementa el traductor ARM ni lo corrige: una extensión
  anunciada y no implementada hace que la app que la use reciba SIGILL, como en un procesador real sin ella. weft acepta cualquier
  traductor y no lleva una lista propia de lo que implementa: solo avisa (informativo, en la pantalla y en `weft device show`) si el
  traductor instalado **publica** qué soporta (ver abajo, «Qué extensiones soporta el traductor»). Sin eso no hay aviso.
- **Qué extensiones soporta el traductor** (contrato opcional para cualquier traductor ARM): un archivo de texto `cpu-features` junto a
  la biblioteca del traductor (en el mismo directorio que el `.so` que se pasa a `weft bridge install --file`), con nombres de
  extensiones al estilo de la línea `Features` de `/proc/cpuinfo` de Linux (`fp asimd aes ...`), separados por espacios o saltos de
  línea; `#` empieza un comentario hasta el final de la línea. `bridge install` lo copia al estado de la máquina
  (`traductor-cpu-features`) cuando el traductor queda instalado, y borra la copia anterior si el traductor nuevo no lo trae (también
  `bridge remove` y `bridge restore`, porque ya no se sabe qué traductor queda). Con ese archivo, la sección *Perfiles* y
  `weft device show` nombran las extensiones que el perfil anuncia y que no están en la lista; se anuncian igual.
- **Formato** (clave=valor, comentarios con `#` en líneas enteras; una clave o un valor desconocido rechaza el archivo con su línea):
  `dispositivo.formato=1`, `dispositivo.id`, `dispositivo.nombre`, `dispositivo.descripcion`, `dispositivo.hereda=ID` (toma todas las
  claves de otro perfil y las de este las sustituyen); identidad `producto.fabricante|marca|modelo|dispositivo|nombre`
  (`ro.product.manufacturer|brand|model|device|name` y sus copias `ro.product.<partición>.*`; vacío = lo de la imagen); máquina
  `maquina.nucleos` (`-smp`) y `maquina.ram` (MiB, `-m`; `auto` = lo que digan la configuración o el arranque); `pagina` (KiB anunciados
  a las apps ARM, 4 o 16: `debug.heddle.page_size`); y la CPU con las claves del contrato del traductor (`docs/perfil-cpu.md` de
  heddle): `cpu.nombre`, `cpu.hardware` (línea `Hardware` de `/proc/cpuinfo`), `cpu.base` (modelo de partida: `cortex-a53`, `-a55`,
  `-a76`, `-a77`, `-a78`, `cortex-x1` o `max`; también sin `cortex-`), `cpu.midr` (32 bits), `cpu.revidr` (opcional), `cpu.ctr`,
  `cpu.dczid`, `cpu.features` (nombres de `/proc/cpuinfo` de Linux 6.6 separados por espacios o comas: lista absoluta, o relativa a la
  base si todos llevan `+x`/`-x`) y `cpu.id_aa64pfr0|pfr1|zfr0|dfr0|isar0|isar1|isar2|mmfr0|mmfr1|mmfr2` (64 bits; sustituyen el
  registro entero). Los números van en hexadecimal (`0x`) o decimal y admiten `_`. Solo se escribe en el invitado lo que el perfil
  define: el resto lo pone la base (Cortex-A78 si no hay).
- **Integrados**: `generico-a78` (Cortex-A78 r1p1, `0x411fd411`, 8 núcleos, 6 GiB), `generico-a55` (Cortex-A55 r2p0, 4 núcleos, 4 GiB)
  y `generico-x1` (hereda del A78; Cortex-X1, 8 GiB), con la identidad vacía y páginas de 4 KiB. No se editan: se duplican. Los del
  usuario viven en `<config>/devices/ID.device` (`weft device path`); con el mismo id manda el del usuario.
- **Qué perfil usa la máquina**: la clave `dispositivo.perfil` de `config` (`ninguno` por defecto: la identidad de la imagen y el
  procesador predeterminado del traductor).

```
weft device list | show [ID]                 # el marcado con * es el de la maquina
weft device use ID|ninguno                   # dispositivo.perfil
weft device new ID [--from ID] [--name N]    # copia otro (por defecto el de la maquina, o generico-a78) como perfil propio
weft device set ID CLAVE VALOR               # cambia una clave de uno propio (lo valida antes de guardar)
weft device delete ID
weft device apply [--no-reboot] [--boot-timeout S] | remove   # escribe o quita el perfil en Android (vigilado)
weft device resources                        # nucleos y memoria con que arrancaria la maquina (cpus=, ram=)
```

**Núcleos y memoria (precedencia)**: `--cpus`/`--mem` de `start`/`launch` > `maquina.cpus`/`maquina.ram` de `config` si no son `auto` >
`maquina.nucleos`/`maquina.ram` del perfil > el defecto del arranque (2 núcleos y 2048 MiB con `start`, 4 y 4096 con `launch`).
Cambian en el próximo arranque de la máquina.

**Cómo llega a Android** (lo hace `weft device apply`; también `bridge install` tras instalar el traductor, `apply-settings` y la
ventana tras cada arranque si Android arrancó con otro perfil que el elegido):

1. `/system/etc/heddle/cpu.conf` con las claves del contrato (el traductor lo lee una vez por proceso, al crear el primer hilo ARM).
2. En los build.prop de la imagen (`/system/build.prop`, `/system_ext/etc/build.prop`, `/vendor/build.prop`, `/odm/etc/build.prop`,
   `/vendor_dlkm`, `/odm_dlkm` y `/system_dlkm` `/etc/build.prop` y `/product/etc/build.prop`, los que existan), las líneas
   `ro.product.<campo>` y `ro.product.<partición>.<campo>` de los campos que el perfil define se **sustituyen en su sitio** por sus
   valores (`ro.product.cpu.*` y lo demás no se toca). Las que la imagen no tiene se agregan al final del build.prop de su partición
   (las generales al de product o, sin él, al de system; las de una partición sin archivo, como `bootimage`, no se agregan). Ahí van
   también `debug.heddle.page_size` y la marca `ro.weft.dispositivo=ID:FIRMA`, que permite saber con un solo `getprop`, sin root, si
   Android ya arrancó con el perfil elegido (no hay coste en caliente). Las líneas originales de cada clave cambiada o agregada se
   guardan en el propio invitado, junto a cada archivo (`<archivo>.weft-orig`): cambiar de perfil, quitarlo y la restauración
   automática las reponen exactamente (también el último salto de línea), y las líneas que no son del perfil (por ejemplo las del
   traductor en `/vendor/build.prop`) se respetan.
3. Los archivos se escriben con root y `remount` (`disable-verity` y un reinicio la primera vez), copiando sobre el destino con `cat`
   para conservar dueño, modo y etiqueta de SELinux. Luego se reinicia Android y se espera `sys.boot_completed` (120 s). **Si no
   arranca, se restauran solos los archivos anteriores**; salida como `bridge install`: 0 bien, 1 error previo (nada cambió), 2 fallo
   restaurado, 3 fallo y restauración fallida. Un perfil que ya falló no se reintenta solo tras cada arranque (archivo
   `dispositivo-fallo` del estado; `weft device apply` lo reintenta). Si una escritura falla a medias (una partición que no se deja
   escribir), lo ya escrito se deshace antes de informar. `weft device remove` (o `use ninguno` + `apply`) lo quita todo: repone las
   líneas originales, borra los `.weft-orig` y `cpu.conf`.

**Pantalla de configuración, *Perfiles*** (grupo *Máquina*): la lista con el de la máquina marcado, *Usar en esta máquina*,
*Duplicar*, *Borrar* (solo los propios, con confirmación) y, con la máquina en marcha, *Aplicar ahora en Android* (confirmación; es
`device apply`). En uno propio se editan todos los campos (texto y números; se guarda al confirmar cada uno, con el error en línea si no
vale), el tamaño de página y las extensiones: el modo (*las del procesador base*, *lista propia* o *cambios sobre la base*) y un botón
por extensión (en la lista propia se enciende o apaga; en cambios pasa por `+`, `-` y nada). Los registros de identificación van en un
desplegable. Los de fábrica se ven en solo lectura.

Sin validar todavía en Android real: que todas las particiones con build.prop se dejen escribir con `remount` en cada imagen y el
comportamiento de cada app con un procesador anunciado distinto.

## Configuración

Todo lo que se puede ajustar vive en un archivo `config` del directorio de estado (`clave=valor`, comentarios con `#`,
editable a mano), que escribe la pantalla de configuración de la ventana propia y que lee `start`, `resolution` y el guion.

- **Pantalla de configuración** (`--display window`): la pestaña **Configuraciones** de la barra superior o F9 (configurable) la abren como
  una capa modal sobre toda la ventana (fondo atenuado al 60 %) con un diálogo centrado de hasta 720 px de ancho y el 90 %
  del alto de la ventana. Se cierra con Esc, con la X o con un clic fuera; mientras está abierta no llega nada al invitado
  (ni teclas ni toques). Navegación a la izquierda con encabezados de grupo (en ventanas estrechas, pestañas arriba, sin los
  encabezados) y contenido a la derecha, con rueda y barra de desplazamiento. Se abre en **Controles**. Los grupos: **Controles**; **General**; **Pantalla y entrada**
  (Atajos, Entrada, Pantalla); **Máquina**; **Almacenamiento** (Imagen de Android, Carpetas) y **Avanzado** (Acceso root,
  Traductor ARM, Diagnóstico, Acerca de). **Textos genéricos**: la interfaz ("Acceso root", "Imagen de Android", "Traductor ARM") NO
  nombra productos ni proveedores en NINGÚN sitio: ni en las etiquetas, ni en los mensajes dinámicos (progreso, resultados, errores,
  estado, salida del doctor), ni en los *Detalles técnicos*, que dicen lo útil sin el proveedor (que es un componente de terceros,
  el tamaño de cada descarga, el sitio de origen como "GitHub" o "el servidor de compilaciones de Android", que la integridad se
  comprueba por tamaño y que los detalles completos se ven con `weft root status` o en este README). Los nombres técnicos
  (producto, versión, archivo y origen) están en este README, en la salida de las órdenes de consola (`root status`, `bridge status`,
  `doctor`...), en los informes y en el registro, por un camino aparte: los módulos de operación producen su texto con dos caras
  (`src/textos.rs`: la ventana y las órdenes que lanza, con `WEFT_SALIDA=interfaz`, dan la genérica; la consola, la técnica) y lo
  que llegue por un camino imprevisto pasa por una red de seguridad. Una prueba recorre la interfaz en todos sus estados y falla si
  aparece kernelsu, ksud, ksu, ksunext, rifsxd, weishu, magisk, zygisk, cuttlefish, heddle, libheddle, ci.android.com, gfxstream,
  rutabaga, late-load o lkm.
  **Catálogo de textos**: ningún texto para la persona está en el código. Los de la ventana (barra, configuración, avisos,
  errores y la pantalla de fin), los de las operaciones (las dos ramas de `elige`: la genérica de la interfaz y la técnica
  de la consola) y los de la línea de órdenes (ayuda, salida, avisos y errores de cada orden, errores de validación de la
  configuración y de las carpetas compartidas, ayuda de cada clave de configuración) están en un catálogo por idioma, por
  clave (`src/textos/es.rs`, una sección por módulo; hoy solo español, que es también el de reserva). Otro idioma es otro
  archivo con las mismas claves y una línea en `CATALOGOS` (`src/textos.rs`); se elige por `LC_ALL`, `LC_MESSAGES` o `LANG`.
  Las pruebas exigen que toda clave que usa el código exista con los mismos huecos (`{}` o `{N}`) y que no sobre ninguna;
  que ningún texto del catálogo nombre productos ni proveedores salvo los de las secciones técnicas `*_tec` (solo consola);
  y que no quede ningún texto visible fuera del catálogo: en `src/` (sin pruebas ni comentarios), toda cadena con dos
  palabras seguidas es un texto visible, salvo los mensajes de fallos internos y las comparaciones (`assert!`, `expect`,
  `contains`...), las órdenes para el invitado o el equipo, los registros de depuración (`ventana: ...`) y lo marcado con
  `// texto-interno: motivo` (contenido de archivos y protocolos, textos del sistema en inglés que se traducen). Las
  palabras sueltas visibles se migraron a mano; las que quedan en el código son identificadores, valores o salida para
  programas (`clave=valor`).
  - *Controles*: ver arriba (acciones rápidas del dispositivo, con la tecla de cada atajo; el diálogo se queda abierto). **Cada cambio se guarda al instante** (se ve un "Guardado" breve): no hay botón Guardar.
  - *General*: zoom inicial (Ajustar, 50, 75, 100 %), tema (`ventana.tema`), tamaño del texto (`ventana.texto`) y
    confirmación al reiniciar o apagar.
  - **Tamaño del texto**: 100 (por defecto), 125, 150 o 200 %. Amplía por igual el texto, los controles y los márgenes de
    la barra superior, de la configuración y del aviso de fin (la maquetación en dp no cambia: un dp de la interfaz mide
    1,25-2 dp de la ventana), con la fuente rasterizada al tamaño físico ampliado (nunca se escala un mapa de bits) y sobre la
    densidad de píxeles del escritorio y `WEFT_ESCALA_UI`. La pantalla de Android no cambia de escala: al cambiarlo, la ventana
    crece o mengua lo que crece o mengua la barra, y su tamaño mínimo crece con el factor (sin pasar del 90 % de la pantalla).
  - **Tema**: *Automático* (`auto`, por defecto), *Claro* u *Oscuro*. En automático la ventana pregunta al portal de
    escritorio (`org.freedesktop.portal.Settings`, `org.freedesktop.appearance` / `color-scheme`) y escucha
    `SettingChanged`, así que cambia en caliente al cambiar el escritorio; solo "prefiere claro" (2) da el tema claro: sin
    preferencia (0), sin bus de sesión o sin portal queda el oscuro de siempre. Los colores viven en `formas::tema`
    (`OSCURO`, idéntico al de antes, y `CLARO`); una prueba exige contraste AA de WCAG (4,5:1 el texto, 3:1 iconos y anillo
    de foco) en los dos temas, y otra recorre la configuración, la barra y la pantalla de fin en ambos.
  - *Atajos*: una fila por acción con la tecla en un "chip". Al pulsarlo entra en modo captura ("Pulsa una tecla", Esc
    cancela, Retroceso la borra); acepta Ctrl/Alt/Mayús. Si la tecla ya la usa otra acción muestra "ya la usa «X»" y no
    guarda hasta resolverlo: se elige otra tecla o se pulsa "Quitarla de «X» y usarla aquí". "Restaurar valores por defecto".
  - *Entrada*: paso (5-25 %) y tope de la rueda, invertir, tecla del pellizco (Ctrl o Alt), conexión automática de mandos
    (se aplica en caliente al servicio `gamepad-serve` si la máquina se arrancó con `--gamepad`, que reserva los puertos
    PCIe; si no, lo dice en línea: "al próximo arranque") y la lista **Mandos del equipo** con Conectar y Desconectar.
  - *Pantalla*: orientación (Auto, 0, 90, 180, 270, la misma lógica que `rotate`), seguir la rotación de Android (controla el
    hilo de `rotacion.rs`), resolución y densidad (campos numéricos; el ancho no múltiplo de 8 sale como aviso amarillo, no
    como error) y "Aplicar resolución ahora", que ejecuta `weft resolution` con el aviso de que reinicia SurfaceFlinger
    y cierra las apps (10-20 s). Por defecto la resolución solo se guarda y se usa en el próximo arranque.
  - *Máquina*: *Estado* (estado, tiempo encendida, CPUs, RAM, tipo) y *Acciones*: **Reiniciar Android ahora** (reinicio ordenado por
    adb, con confirmación), **Reinicio completo de la máquina** (`weft restart`; con confirmación y el aviso de que la ventana
    se cierra y se abre otra) y **Apagar la máquina** (`weft stop`: por adb, ~2 s; con confirmación). Después, los ajustes
    (CPUs, RAM, tipo pc/q35, aceleración gráfica automática/por hardware/ninguna): "Se aplican al próximo arranque". "Abrir carpeta de
    estado" solo escribe la ruta en la salida de la ventana (`window.log`); no abre ningún programa.
  - *Carpetas* (compartidas): lista (nombre, `/sdcard/NOMBRE`, modo y carpeta del equipo; aviso si ya no existe), *Quitar* con
    confirmación, y "Agregar una carpeta" con campos de texto (nombre validado al salir del campo, carpeta del equipo con aviso
    si no existe), "Solo lectura" y *Agregar*; nota fija "Se aplica al próximo arranque de la máquina".
  - *Acceso root*: estado (instalado, cargado, versión, gestor de permisos), interruptor "Activar el acceso root en cada arranque"
    (pide confirmación: el texto principal es genérico y dice cuánto se descarga y que es software de un tercero; el botón
    *Detalles técnicos* despliega lo que el usuario debe poder ver antes de aceptar: que es un componente de terceros, el tamaño de
    cada descarga, el sitio de origen y la integridad por tamaño, sin nombrar al proveedor; más el aviso de las apps que detectan root), "Instalarlo solo si falta", *Instalar gestor* y
    *Reiniciar Android ahora* (reinicio ordenado, con confirmación). El código depende de la abstracción `ProveedorRoot`
    (`src/root.rs`: nombre técnico, descargas, estado, habilitar, deshabilitar, gestor, asegurar), con una única implementación
    hoy (KernelSU-Next en modo LKM). Las operaciones largas corren en un hilo con el progreso en pantalla; el resultado o el
    error sale en línea.
  - *Acerca de*: versión, resumen de `doctor` (OK / AVISO / FALLO por comprobación), rutas del estado y fuente en uso.
  - Teclado: Tab y Mayús+Tab recorren los controles (anillo de foco, visible en los dos temas: contraste 3:1 con todo lo que
    rodea), Intro y Espacio activan, flechas arriba/abajo en la navegación (también izquierda/derecha en las pestañas de una
    ventana estrecha) e izquierda/derecha en listas de opciones y pasos. Esc es "atrás": cancela la captura de un atajo, la
    edición de un campo o una pregunta de confirmación pendiente (el foco vuelve al botón que la abrió) y, sin nada pendiente,
    cierra la pantalla. Una prueba recorre todos los estados también con el foco en cada control. Estados: reposo, hover, pulsado, foco, deshabilitado,
    aviso (amarillo), error (rojo) y "Guardado".
- **Orden `weft config`**: `list` (todas las claves y valores), `get CLAVE`, `set CLAVE VALOR` (valida; las teclas no
  pueden repetirse), `reset [CLAVE]` y `path`. Una ventana abierta relee el archivo al guardarse (editado a mano o por
  `config set`) y aplica en caliente zoom, tema, tamaño del texto, atajos, rueda, pellizco, mandos y giro de Android.
- **Precedencia**: opción de la línea de órdenes (o variable `WEFT_*` del guion) > `config` > valor por defecto.
  `start` toma de `config` `maquina.ram`, `maquina.cpus`, `maquina.tipo`, `maquina.gpu`, `gamepad`, `pantalla.resolucion` y
  `pantalla.densidad` (y las carpetas de `share.*`) cuando no se pasa `--mem`, `--cpus`, `--machine`, `--gpu`, `--gamepad`, `--resolution` ni `--density`
  (`auto` / `perfil` = "lo que decida el arranque"; con `maquina.ram`/`maquina.cpus` en `auto`, los del perfil de dispositivo:
  ver «Perfiles de dispositivo»). `resolution` deja la resolución en `config`. El guion
  (`dev-fedora.sh`) hace lo mismo con `WEFT_MEM`, `WEFT_CPUS` (que leen `weft device resources`), `WEFT_MACHINE`, `WEFT_GPU`, `WEFT_GAMEPAD` y `WEFT_RES`.
- Un valor no válido (a mano en el archivo) se ignora con un aviso y se usa el defecto; si dos acciones comparten tecla gana la
  escrita a mano y la otra queda sin asignar. Las claves con el valor por defecto salen comentadas (`#clave=valor`) con su
  explicación: se activan quitando el `#`. `WEFT_CONFIG=RUTA` cambia el archivo (el directorio de estado por defecto vive
  en `XDG_RUNTIME_DIR`, que se borra al cerrar la sesión).

## Tipografía de la ventana

El texto de la ventana (barra superior y pantalla de configuración) usa la fuente sans-serif de interfaz del escritorio, con tildes
y eñes, no una fuente propia: `libfontconfig.so.1` localiza `sans-serif` (y su variante semibold para los títulos; en KDE
sobre Fedora es Noto Sans) y `libfreetype.so.6` la rasteriza con suavizado y ajuste vertical ligero. Las dos bibliotecas se
cargan en tiempo de ejecución (`dlopen`, como SDL3): weft sigue sin dependencias de compilación.

- Tamaños (puntos de la ventana): 12 etiquetas y notas, 13 cuerpo, 13 semibold controles, 16 semibold títulos, 26 cifras
  grandes. Con un escritorio HiDPI se rasteriza al tamaño físico (puntos por la densidad de píxeles de SDL): nunca se
  escala un mapa de bits. `WEFT_ESCALA_UI=2` (solo pruebas) amplía toda la interfaz para comprobarlo en un escritorio a 1x.
- El texto se mide con la fuente real: alinea, centra y trunca con "..." cuando no cabe.
- Respaldo: si faltan FreeType o fontconfig, o ninguna fuente con las letras del español, queda la fuente de mapa de bits 5x7
  de siempre (sin tildes) y la ventana lo anota en `window.log` ("sin fuente del sistema..."). `window.log` también dice qué
  fuente se usó.

## Rotacion y resolucion por comando

- `weft rotate [0|90|180|270|auto]` (sin argumento, un paso 0 > 90 > 180 > 270 > auto > 0; F7 de la ventana y el
  panel hacen lo mismo; en el guion `dev-fedora.sh rotar [grados]`): fija el archivo `orientation` del estado (`auto` por
  defecto). **Regla**: con una orientacion FIJA la ventana queda girada esos grados aunque Android no gire (giro automatico
  apagado, app que no gira), sin retroceso; con `auto` manda lo que Android decida por su cuenta. Lo fijo lo sigue el
  acelerometro del servicio de sensores (`--sensors`), de modo que Android gira su pantalla (Surface.ROTATION_*: 90 = aparato girado a la izquierda,
  apaisado; la rotacion automatica debe estar activa y la aplicacion permitirlo; la pantalla de inicio de Android no gira).
  Como en un movil real, Android compone el contenido ya girado dentro del mismo panel, que queda de lado. Con
  `--display window` la ventana gira su presentacion: con 90/270 grados mide alto x ancho, dibuja la imagen del panel
  girada a la izquierda (SDL_RenderTextureRotated) y lleva el raton, el toque, la rueda y el pellizco al panel con la
  transformacion inversa; Android recibe siempre coordenadas del panel, como un panel tactil real, y las gira el mismo.
  Con gtk/sdl solo gira el contenido dentro del marco vertical.
- Con la orientacion en `auto` la ventana propia sigue la rotacion que Android decida por su cuenta (una app que fuerza
  apaisado, el ajuste de rotacion del usuario): un hilo de la ventana lee la rotacion de Android por el adb propio cada 2 s
  (`dumpsys display | grep -m1 mCurrentOrientation`, 0..3; unos 6 ms por consulta de punta a punta, unos 3 ms de ellos la
  conexion vsock; `dumpsys window displays` cuesta unos 22 ms) y la anota en `android-rotation` cuando cambia. En `auto` el
  acelerometro queda "aparato derecho" (0): es como dejar el movil sobre la mesa y que Android decida. Con
  `pantalla.giro_android=no` en `config` el hilo no consulta nada y `auto` deja la ventana vertical. Sin adbd no hace nada.
- `weft resolution ANCHOxALTO[@DPI] [--wait S] [--no-restart] [--cid N]` (en el guion `dev-fedora.sh resolucion ANCHOxALTO[@DPI]`):
  cambia el tamano del panel con la maquina encendida. Con la ventana propia pide el modo al dispositivo por D-Bus
  (`Console.SetUIInfo`); con la ventana GTK arrancada con `--resolution`, por el VNC de `rfb.rs` (SetDesktopSize). virtio-gpu
  avisa al invitado y el controlador DRM de Linux ofrece el modo nuevo al instante, pero el hwcomposer de Cuttlefish no lo
  adopta (SurfaceFlinger sigue con el modo viejo y los cuadros fallan) hasta que se reinicia SurfaceFlinger
  (`setprop ctl.restart surfaceflinger`, un reinicio blando del entorno grafico que cierra las apps; la maquina no se
  reinicia, ~8 s medidos con la cache caliente). La orden lo hace por el adb propio (pasa adbd a root, reinicia
  SurfaceFlinger, espera a que vuelva con otro pid, espera el modo nuevo y aplica la densidad con `wm density`);
  `--no-restart` solo deja el modo pedido y avisa. La ventana sigue el cambio de tamano. El ancho se redondea a multiplo de 8
  (DRM de Linux: 1348 pasa a 1344). La orden deja la resolucion en `config` (`pantalla.resolucion`, y `pantalla.densidad` si se
  dio): `start` sin `--resolution` (y el guion sin `WEFT_RES`) la usan en el proximo arranque; con maquina detenida la orden solo la guarda. La densidad (`@DPI`) va por
  `--density` (`androidboot.lcd_density`, el perfil trae 280) en el arranque y por `wm density` en caliente.

## Cómo funciona

`weft start` construye la línea de comandos de QEMU y lo deja en segundo plano. En un directorio privado de estado
quedan el socket de control (QMP), el socket y el registro de la consola serie, y el identificador del proceso.
Las demás órdenes hablan con esos sockets.

## Pruebas

- `cargo test`: 245 pruebas unitarias repartidas por casi todos los módulos de `src/` (pantalla de configuración, instalador vigilado del
  traductor con un invitado simulado, línea de QEMU, `config`, cliente de control contra un servidor simulado, perfiles de imagen, gestos,
  rutas, imágenes, carpetas compartidas, reinicio y apagado, mandos, informe, JSON...). No necesitan QEMU ni ventana.
- `tests/arranque.rs`: arranques que fallan a medias con el binario de weft y programas falsos en lugar de QEMU y virtiofsd (sin QEMU real):
  no quedan procesos ni el pid de la máquina, virtiofsd va en su propio grupo de procesos, `--dry-run` no crea el wav, la carpeta de
  estado con coma y los valores imposibles se rechazan, y `qemu.log` del intento anterior queda en `qemu.log.1`.
- `tests/e2e.rs` (con `WEFT_E2E=1`): máquina real con un invitado mínimo. Comprueba arranque, estado, órdenes en
  el invitado, teclado, captura, puntero y los dos apagados.
- `tests/cuttlefish.rs` (con `WEFT_CF_FIRST=1`): Cuttlefish desde cero (disco armado por `make-disk`, arranque directo del
  kernel, q35 + pantalla tactil virtio), entrada, red, carpeta compartida, audio, el banco de pruebas x86_64 y arm64 con
  heddle. Los cuatro casos de bajo nivel del banco (`fallo_recuperable`, `codigo_propio`, `mascara_senales`, `jni_registrado`)
  corren en un proceso aparte de la app (servicio en `:bajo`): si ese proceso muere, el caso queda como `CAIDO senal=N` y el resto
  del banco sigue midiendo. El caso `vulkan` (en el mismo proceso aparte) carga libvulkan con `dlopen`/`vkGetInstanceProcAddr`, dibuja un
  triángulo fuera de pantalla con sombreadores SPIR-V incrustados y lee los píxeles; sin dispositivo Vulkan da `OMITIDO motivo=sin_vulkan`.
  En arm64 corre dos veces: sin `debug.heddle.vulkan` (el defecto de heddle: solo se informa como `aviso:`) y con `debug.heddle.vulkan=1`
  (debe dar OK, como en x86_64; OMITIDO es un aviso). El banco trae además una `NativeActivity` (proceso `:nativa`) cuyo manifiesto
  declara `android.app.lib_name=banconativa` y `android.app.func_name=banco_nativa_crear`; la biblioteca no exporta
  `ANativeActivity_onCreate` (`ci/build-apk.sh` lo comprueba). Debe llamarse ese punto de entrada (con el `JNIEnv` de la actividad),
  llegar `onStart`, `onResume`, la ventana (que pinta de verde), el foco y la cola de entrada, y un toque en el centro debe volver por
  esa cola (`ALooper`): `banco[TAG]: nativa=OK` y `nativa_entrada=OK`. Se exige en x86_64 y en arm64 con heddle, también con su
  última release (en arm64 valida que heddle reconoce el nombre leyendo los metaData de la actividad); que la captura no muestre el
  verde es un aviso. Todo lo que habla con Android usa el adb propio (`wait-adb`, `adb-shell`, `push`, `install`, `logcat`, `root`,
  `remount`, `disable-verity`, `reboot`): no interviene el adb de Google.
- `tests/android.rs` (con `WEFT_ANDROID=1`, manual: el CI no la ejecuta): arranca una imagen de Android x86 tipo SDK y la
  comprueba con el adb propio (hace falta adbd por vsock y sin autenticación).

## CI

Un solo pipeline (`.github/workflows/ci.yml`) con tareas en paralelo; las que arrancan Android usan KVM y ninguna instala ni usa el adb de Google:

- `base`: pruebas unitarias y de integración, y la máquina mínima de `tests/e2e.rs` con KVM y sin aceleración.
- `qemu-ubuntu`: compila (`ci/build-qemu.sh`) el QEMU con gfxstream que necesita `cuttlefish`, porque Ubuntu no trae `virtio-gpu-rutabaga`;
  el resultado queda en la cache de Actions (clave = el guion, la versión de QEMU y los commits de gfxstream y rutabaga), así que los
  pipelines siguientes lo reutilizan mientras la cache no se expulse (GitHub borra las entradas sin uso en 7 días y limita el total por
  repositorio); cuando falta, la tarea lo compila de nuevo.
  No depende de ninguna release de otro repositorio.
- `cuttlefish`: Android completo en Ubuntu con ese QEMU (gfxstream, q35, multitouch), el traductor ARM heddle (la última release de
  `github.com/43fdfdg45454/heddle`; lanzada a mano con `heddle_ref`, esa rama o etiqueta compilada) y el banco de pruebas. Con la última
  release, un caso de bajo nivel que pasa en x86_64 y no en arm64 no hace fallar la tarea: sale como aviso (`::warning`) y en el resumen
  de la tarea; con `heddle_ref` se exige. El Vulkan de Android es el del anfitrión por gfxstream, con lavapipe (Vulkan por software de
  Mesa): no hay `--gpu_mode` porque no se usa el lanzador de Google. Un `adb` falso al frente del PATH anota cualquier uso y hace fallar la tarea.
- `bibliotecas-fedora`: compila rutabaga + gfxstream en Fedora 44 y los prueba con su QEMU; instala `qemu-ui-dbus`, `SDL3`,
  FreeType, fontconfig y Noto Sans; guarda la salida de `weft doctor` (sin KVM en esta tarea, el fallo de la fila KVM es lo esperado).
- `fedora-ventanas`: el guion de usuario (`scripts/dev-fedora.sh`) en Fedora 44 sin ventana, con GTK y con SDL, con y sin
  gfxstream; `weft doctor` con KVM debe terminar con 0.
- `fedora-ventana-propia`: el guion con `--display window` sin pantalla física (`ci/prueba-ventana.sh`). SDL3 trae el controlador
  `offscreen` y un dibujador por software: `SDL_VIDEO_DRIVER=offscreen SDL_RENDER_DRIVER=software`. Comprueba que `window.log`
  (`WEFT_WINDOW_STATS`) anota cuadros reales con la resolución pedida (no solo el aviso «Display output is not active»),
  que la fuente del sistema se encontró, la captura de lo que dibuja la ventana (barra y pantalla, sin panel lateral) y de la pantalla de
  configuración (`WEFT_WINDOW_INJECT=1`, PNG como artefacto), que con `ventana.tema=claro` y `ventana.texto=200` cambiados en marcha la
  barra se aclara y dobla su altura (captura `ventana-claro-200.png`), que un clic inyectado llega a Android como toque multitouch
  y F1 como `KEY_BACK` (`getevent` por el adb propio), `weft doctor` con código 0 y el archivo `config` generado.
- `fedora-raton`: el ratón como toque en Fedora (`ci/prueba-raton.sh`), una pasada con los defectos del guion (q35 + multitouch)
  y otra con wacom explícito (el modo de las ventanas gtk/sdl), con el adb propio.
- `fedora-cow-rsa`: en paralelo, el guion con `disk.cow=si` (`ci/prueba-cow-rsa.sh`): el disco de la máquina es un overlay qcow2 sobre la
  base de la imagen y Android arranca de él; después, con `ro.adb.secure=1` (en `/product/etc/build.prop`, con `remount`), el adb propio
  con una clave desconocida no entra (adbd exige autenticación) y con la suya, puesta antes en `/data/misc/adb/adb_keys`, entra firmando el
  testigo.
- `flatpak`: en un contenedor Fedora 44 (Ubuntu 24.04 trae Flatpak 1.14, que rechaza `--device=input`), valida el manifiesto, el `.desktop` y la
  metainfo, construye el paquete `.flatpak` con flatpak-builder (QEMU 10.2.2 sin parches, rutabaga, gfxstream, virtiofsd y weft; cache de la
  construcción con `actions/cache`), lo instala y comprueba el contenido instalado (bibliotecas de aceleración, virtiofsd y los módulos del
  QEMU); ejecuta `doctor`, `paths` y `launch` sin ninguna imagen (debe terminar con 1 explicando cómo agregar una) y guarda sus salidas. Lo que
  se mira dentro del sandbox (`virtio-gpu-rutabaga` y la pantalla D-Bus del QEMU, `doctor` viéndolo, el `launch`) solo cuenta si el contenedor
  deja lanzar Flatpak.
- `resultados`: corre siempre (salvo pipeline cancelado) y junta los resultados de todas en el release `ci` (`resultados.tar.gz`: transcripciones,
  capturas, `window.log`, salida de `doctor`, `config`); no publica ninguna versión.
- `semver`: calcula la versión con [GitVersion](https://gitversion.net) (`GitVersion.yml`). En `main` es la última etiqueta `vX.Y.Z` más un
  parche; un commit con `+semver: minor` o `+semver: major` en el mensaje sube la versión menor o la mayor (`+semver: none` no la sube). En
  otras ramas sale `X.Y.Z-rama.N`. `base` y `flatpak` la incrustan en el programa (`weft --version`, "Acerca de", el informe) y en el
  metainfo del paquete; en una compilación local vale la de `Cargo.toml`.
- `version`: publica `vX.Y.Z` (con `weft-linux-x86_64.tar.gz` y `weft-linux-x86_64.flatpak`) solo si todas pasaron y es un push.

## Estado

Ver la sección de resultados en la release `ci`. Nada se ha validado con GPU Nvidia ni en hardware ARM.

## Licencia

weft es software libre bajo la licencia MIT (archivo `LICENSE`). Los paquetes publicados llevan dentro software de terceros, cada uno con su
propia licencia:

- QEMU 10.2.2: GPL-2.0-only. Código fuente usado: https://download.qemu.org/qemu-10.2.2.tar.xz.
- libslirp: BSD-3-Clause.
- libcap-ng: LGPL-2.1-or-later.
- gfxstream: Apache-2.0.
- rutabaga_gfx: BSD-3-Clause.
- virtiofsd: Apache-2.0 y BSD-3-Clause.

Los commits exactos de cada módulo del `.flatpak` están en `flatpak/plantillas/app.yml.in`; el paquete instala el texto de cada licencia en
`/app/share/licenses/<módulo>/` y conserva `/app/qemu/share/doc`. El `weft-linux-x86_64.tar.gz` lleva en `LICENSES/` la licencia de weft, las de
las dos bibliotecas de `gfx/` (compiladas en el CI desde un commit fijo de cada repositorio, el mismo que compila el `.flatpak`, anotado en
`LICENSES/VERSIONES.txt`) y un
`THIRD-PARTY.md` que lo explica. La biblioteca rutabaga del tar.gz lleva un parche de una línea en `ffi/src/lib.rs` (el `sed` de
`.github/workflows/ci.yml`): `let renderer_features_ptr = builder.renderer_features;` pasa a
`let renderer_features_ptr: *const c_char = std::ptr::null();`, para que no lea el campo final `renderer_features` de la estructura de inicio,
que el QEMU de Fedora (compilado contra la interfaz 0.1.3) no tiene; la rutabaga del `.flatpak` va sin parche. La fuente de mapa de bits 5x7
de respaldo (`src/fuente.rs`) usa los glifos de la fuente ASCII clásica de pantallas LCD (glcdfont de Adafruit-GFX, licencia BSD), y
`perfiles/cuttlefish.bootconfig` reproduce los parámetros que genera el lanzador de dispositivos virtuales de AOSP (Apache-2.0).

Android es una marca de Google LLC. weft no está afiliado a Google ni al proyecto Android.
