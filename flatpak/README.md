# weft como Flatpak (paquete `.flatpak`)

Se distribuye como paquete `.flatpak` (no Flathub): lo construye el CI (`flatpak` en `.github/workflows/ci.yml`) y lo publica en el release
`v1.0.N` como `weft-linux-x86_64.flatpak`. Identificador provisional: `io.github._43fdfdg45454.weft`.

## Archivos

- `plantillas/app.yml.in`, `app.desktop.in`, `app.metainfo.xml.in`, `app.svg`: plantillas con `@APP_ID@`.
- `construir.sh`: **la variable `APP_ID` de su primera línea es el único sitio donde vive el identificador.** Genera `generado/<APP_ID>.{yml,desktop,metainfo.xml,svg}`
  (carpeta ignorada por git) y construye. `SOLO_GENERAR=1` solo genera.

## Contenido del paquete (`/app`)

| Parte | Qué es |
| --- | --- |
| `/app/bin/weft` | weft (Rust, sin crates); se invoca con `flatpak run --command=weft <id> ORDEN` |
| `/app/bin/weft-launch` | envoltorio `exec /app/bin/weft launch "$@"`: la orden del paquete (`command`) y el `Exec` del `.desktop` |
| `/app/qemu` | QEMU 10.2.2 de upstream **sin parches**, solo x86_64, con módulos (pantalla D-Bus, `virtio-gpu-rutabaga`, audio PipeWire/PulseAudio), KVM, red de usuario (libslirp) |
| `/app/qemu/lib` | `librutabaga_gfx_ffi.so.0` y `libgfxstream_backend` (aceleración gráfica por hardware, compiladas aquí) |
| `/app/libexec/virtiofsd` | carpetas compartidas (v1.14.0) |

Rutabaga y QEMU se compilan contra la misma cabecera, así que sobra el parche de una línea de `.github/workflows/ci.yml` (el `sed` de `renderer_features_ptr`; era solo para el QEMU de Fedora, que usa la
cabecera 0.1.3). Comprobado: con el QEMU y la rutabaga de este paquete la GPU arranca sin parche (`Gfxstream initialized successfully!`). No cuenta como
«QEMU a medida»: es el tarball oficial de QEMU, sin parches, con otra lista de opciones de `configure`.

## Permisos (`finish-args`)

`--socket=wayland`, `--socket=fallback-x11`, `--share=ipc`, `--device=dri`, `--device=kvm`, `--device=input`, `--socket=pulseaudio`,
`--filesystem=xdg-run/pipewire-0`, `--env=WEFT_QEMU_DIR=/app/qemu` y `--share=network`: internet para Android (red de usuario de QEMU) y una
sola 127.0.0.1 para todas las instancias de `flatpak run`, de modo que el adb por TCP de una máquina lo alcanza cualquier instancia. weft sigue
sin descargar nada.

**Requiere Flatpak 1.15.6 o más nuevo** en el equipo (y para construir): `--device=input` (mandos de juegos) no existe en versiones anteriores y
`flatpak install` (igual que `flatpak-builder`) rechaza el permiso, con lo que el paquete entero no se instala. Ubuntu 24.04 y Debian 12 traen
Flatpak 1.14 y lo rechazan; Fedora 44 trae 1.16 o más nuevo (por eso la tarea `flatpak` del CI construye y prueba en un contenedor Fedora 44).

## Construir

```
flatpak install --user flathub org.flatpak.Builder      # o flatpak-builder del sistema
bash flatpak/construir.sh [SALIDA.flatpak]               # instala runtime 25.08, SDK y la extensión de Rust, construye y empaqueta
flatpak install --user --bundle weft.flatpak
flatpak run io.github._43fdfdg45454.weft                        # = weft launch (la orden del paquete es el envoltorio weft-launch)
flatpak run --command=weft io.github._43fdfdg45454.weft doctor  # cualquier otra orden de weft
```

Los fuentes (git por commit, QEMU por sha256) y las dependencias de cargo y meson se bajan **durante la construcción**
(`--share=network` solo en `build-args` de los módulos de compilación; válido para un paquete propio, no para Flathub).
Al instalar el paquete, lo único que Flatpak baja por su cuenta es el runtime `org.freedesktop.Platform//25.08` (si falta) y, con Nvidia, la extensión
`org.freedesktop.Platform.GL.nvidia-<versión>`.

## Uso

1. Baja una imagen de Android x86_64 (zip o carpeta). Dentro de Flatpak no hay acceso a tus carpetas: da acceso a la carpeta
   (`flatpak override --user --filesystem=CARPETA <id>`) y agrégala:
   `flatpak run --command=weft <id> image add CARPETA/imagen.zip`.
2. Abre la aplicación desde el menú o con `flatpak run <id>`: las dos ejecutan `weft-launch`, el envoltorio de `weft launch` (la orden del
   paquete, `command`, y el `Exec` del `.desktop`), que arma el disco y abre la ventana. Sin imagen explica cómo agregar una. `weft` a secas
   solo imprime la ayuda: cualquier otra orden va con `flatpak run --command=weft <id> ORDEN`.
3. Acceso root y traductor ARM: `flatpak run --command=weft <id> root enable --from CARPETA` y `... bridge install --file LIB` (o los campos
   de ruta de la pantalla de configuración).

## Limitaciones conocidas

- `AF_VSOCK` está bloqueado por el seccomp de Flatpak (comprobado con `--device=all` y todos los `--allow`): el adb va por TCP (127.0.0.1). Con
  `--share=network` todas las instancias comparten la red del equipo y cualquiera alcanza ese puerto (`adb-shell`, `push`, `stop` por adb...).
- Cada instancia tiene su espacio de pids: weft anota el suyo en `pid-ns` y nunca manda señales a pids de otra instancia.
- Solo ventana propia (sin GTK/SDL de QEMU). La tipografía sale de fontconfig del runtime (DejaVu Sans, no Noto Sans).
- Mandos: `--device=input` abre los mismos `/dev/input/event*` que el usuario; la conexión en caliente de mandos nuevos dentro del sandbox no se probó.
- Audio PipeWire/PulseAudio dentro del sandbox: no probado con sonido real (las pruebas usaron `--audio none`).
- No validado en hardware Android/ARM real.
