# Imagenes de Android en weft: diseno y viabilidad

Estado: fases 0, 1 y 2 IMPLEMENTADAS (2026-10-07; ver la seccion 0 y BITACORA.md). Las secciones 1 a 7 son el diseno original: donde difieren, manda la seccion 0. Nada validado en hardware Android/ARM real.

Nota: las secciones 1 a 7 son diseno historico, escrito antes de implementar, y no se han reescrito: sus referencias a lineas
(`scripts/dev-fedora.sh:NNN`, `src/*.rs:NNN`) y los nombres de entonces (`perfiles/*.perfil`, `$XDG_DATA_HOME/weft/imagenes/`, `./imagen`)
pueden no coincidir con el codigo actual (hoy los perfiles integrados son `perfiles/*.profile` y las imagenes viven en
`$XDG_DATA_HOME/weft/images/<id>/`; ver README). Los pendientes que esas secciones citan ("PENDIENTES") son los issues del repositorio.

Convencion de marcas:
- **[V: archivo:linea]** verificado leyendo el codigo del repo (lineas al 2026-10-07; `src/main.rs` se esta editando en paralelo,
  por eso alli se cita la funcion).
- **[VE: ruta]** verificado ESTATICAMENTE sobre archivos de imagen (cabeceras, discos RAM, tablas de particiones), sin arrancar nada.
  Los analisis estan en `tmp/imagenes-analisis/` y `tmp/imagenes-sdk/` de la carpeta de trabajo (fuera del repo).
- **[H]** hipotesis o recuerdo no verificado, con la forma de comprobarla.

Peticion: "permitir instalar cualquier imagen de Android", con una interfaz generica (sin nombrar la familia de imagen ni el
proveedor de root).

Conclusion en una linea: "cualquier imagen" no es alcanzable tal cual; lo alcanzable es **cualquier imagen de dispositivo virtual
Android x86_64 cuyo hardware esperado pueda dar QEMU estandar mas weft**. Eso se modela con **perfiles de imagen** declarativos;
el primer paso de valor es sacar la receta de arranque del guion a un perfil integrado y probar una segunda imagen de la misma familia.

---------------------------------------------------------------------------------------------------------------------------------

## 0. Lo implementado (fases 0, 1 y 2)

Codigo: `src/perfil.rs` (formato, herencia, catalogo, receta de arranque con cache), `src/catalogo.rs` (detector, instalacion, lista,
quitar, imagen de cada maquina), `src/rutas.rs` (directorios), `src/adb.rs` (via TCP), `perfiles/*.profile`.

Los perfiles de **dispositivo** (`src/dispositivo.rs`, `perfiles/dispositivos/*.device`: CPU anunciada, extensiones, identidad
`ro.product.*`, tamano de pagina, nucleos y memoria) son otra cosa y valen para cualquier imagen: ver README, «Perfiles de dispositivo».

### Formato del perfil (version 1)

Una clave por linea (`clave=valor`); `#` solo al principio de linea. Repetibles: `detectar.requiere`, `detectar.puntua` (cada linea
admite varias condiciones separadas por espacios), `disco.particion`, `disco.particion_ab`, `disco.datos` (un solo orden: el del archivo)
y `bc.CLAVE=VALOR`. `perfil.hereda=ID`: el hijo toma todo del padre; una clave de un valor se sobrescribe, un grupo repetible que el hijo
define reemplaza al del padre y `bc.*` se sobrescribe clave a clave. Clave o valor desconocido = perfil rechazado con el numero de linea.

| Clave | Valor | Para que |
|-------|-------|----------|
| `perfil.id`, `perfil.nombre`, `perfil.descripcion`, `perfil.familia`, `perfil.formato=1`, `perfil.hereda`, `perfil.aviso` | texto | identidad; `aviso` lo muestran `start` e `image info` |
| `perfil.arquitectura` | `x86_64` / `arm64` | otra arquitectura se admite en el modelo y `start` la rechaza con un mensaje claro (necesitaria qemu-system-aarch64 sin KVM) |
| `detectar.requiere`, `detectar.puntua` | `archivo:NOMBRE`, `kernel:x86_64\|arm64`, `android_info:K=V`, `vendor_bootconfig:K=V` | deteccion estatica: todas las de `requiere`; `puntua` desempata (empate = elegir con `--profile`) |
| `disco.modo=gpt`, `disco.ranura_pci` | | disco GPT armado por weft; ranura PCI (Android lo localiza por ella) |
| `disco.particion=NOMBRE:TAMANO\|ARCHIVO`, `disco.particion_ab=a b c`, `disco.datos=NOMBRE:ARCHIVO` | | particiones en orden (`_a`/`_b` para la lista A/B); la de datos admite `disk.data` |
| `arranque.modo=directo-android`, `arranque.boot/init_boot/vendor_boot`, `arranque.cmdline_extra`, `arranque.bootconfig_archivo`, `bc.K=V` | | kernel de boot, discos RAM concatenados (vendor + generico), linea del kernel, parametros de Android (bootconfig: lineas del fabricante, luego el archivo, luego `bc.*`) |
| `aceleracion`, `consola`, `consola.hvc`, `consola.registro`, `sensores`, `entrada.tactil`, `red.tarjetas`, `gpu.opciones`, `gpu.motores` | | dispositivos de la maquina; `gpu.motores`: `hardware`, `software` |
| `maquina.ram_minima`, `maquina.tipos` | | avisos y limites |
| `adb.transportes`, `adb.puerto`, `adb.tcp_red` | `vsock tcp`, 5555, indice de tarjeta | por donde se puede hablar con adbd y por que tarjeta sale el reenvio TCP |
| `capacidad.root/carpetas/traductor/mandos/resolucion` | palabras cerradas | informativas (se ven en `image info`) |

Precedencia al arrancar: opcion de la linea de ordenes > `config` > perfil > valor por defecto.

### Mediciones de las fases 1 y 2 (arranques reales, ver BITACORA.md)

- Telefono: `start --image` produce la linea de QEMU y el bootconfig (47 lineas) de la receta del perfil.
- adb por TCP: en el telefono SOLO la 3.a tarjeta (eth2, 10.0.4.15) obtiene direccion por DHCP: por eso `adb.tcp_red=2`. Probado
  `--adb tcp` sin vsock: boot_completed, apply-settings, push y reinicio completo por TCP.
- Coche (`aosp_cf_x86_64_auto`, 16373615): detectado por `android-info.txt` (`config=auto`); con el disco del telefono el arranque
  entra en bucle `reboot,recovery` porque su politica etiqueta `/dev/block/vda19` como `swap_block_device` (y pide
  `androidboot.hibernation_resume_device=259:3`): hace falta una particion de intercambio ANTES de los datos (datos en vda20), que el
  perfil expresa con `disco.particion=swap:1G`. Con eso arranca el sistema, `ro.product.name=aosp_cf_x86_64_auto` por vsock, pero
  `sys.boot_completed` no llega: su HAL de vehiculo se conecta por gRPC a 192.168.98.1:9300 (servicio del anfitrion de la familia) y
  sin el, CarService espera. Ninguna tarjeta de red recibe direccion (solo `lo`): sin TCP.

---------------------------------------------------------------------------------------------------------------------------------

## 1. Inventario: lo que weft asume hoy sobre la imagen

Alcance: **CF** = especifico de la familia de dispositivo virtual de Google usada hoy (vsoc_x86_64, `androidboot.hardware=cutf_cvm`);
**AOSP** = comun a imagenes AOSP de dispositivos virtuales recientes (GKI, boot v4, particiones dinamicas); **ANY** = cualquier Android.

Hallazgo previo importante: la receta de arranque completa NO esta en Rust sino en el guion: `scripts/dev-fedora.sh:181` (unpack-boot
con los tres .img), `:186-191` (linea de `start`: `--append ... console=hvc0 earlycon=uart8250,io,0x3f8 pnpacpi=off`, dos
`--bootconfig-file`, `--disk-slot 4`, `--console hvc --hvc-count 30 --hvc-log 2=... --sensors 18,19 --touch --nics 3`), `:196` (`edid=off`),
`:199` (vsock), `:213` (fin de arranque detectado por texto del registro de la consola 2). `start` es un lanzador generico de QEMU; la
"imagen" vive repartida entre el guion, `perfiles/cuttlefish.bootconfig` e `imagen.rs`. `restart` reutiliza los argumentos guardados por
`start` (`src/reinicio.rs`, archivo `arranque` del estado).

| # | Supuesto | Donde | Alcance |
|---|----------|-------|---------|
| 1 | La imagen es el zip `aosp_cf_x86_64_only_phone-img-N.zip` de ci.android.com, target fijo | [V: src/imagen.rs:17-21, 43-56] | CF |
| 2 | Archivos requeridos: boot, init_boot, vendor_boot, super, userdata, vbmeta + vbmeta_system, vbmeta_system_dlkm, vbmeta_vendor_dlkm | [V: src/imagen.rs:23-25, 180-189] | CF (AOSP >= 13 con init_boot y dlkm) |
| 3 | Disco GPT propio: misc 1M, metadata 64M, frp 1M, siete particiones en _a y _b, super, userdata (vacia de N GiB o userdata.img) | [V: src/imagen.rs:397-413; src/gpt.rs] | Orden CF; nombres by-name AOSP |
| 4 | Android encuentra el disco por ruta PCI: `androidboot.boot_devices=pci0000:00/0000:00:04.0` y `--disk-slot 4` | [V: perfiles/cuttlefish.bootconfig:6; scripts/dev-fedora.sh:189; src/vm.rs:245-258] | Mecanismo AOSP; valor CF |
| 5 | `androidboot.fstab_suffix=cf.f2fs.hctr2` elige `fstab.cf.f2fs.hctr2` del disco RAM del fabricante | [V: perfiles/cuttlefish.bootconfig:7] [VE: vendor_ramdisk trae fstab.cf.{ext4,f2fs}.{cts,hctr2}] | CF |
| 6 | La particion de datos vacia se formatea sola (`formattable` en la linea de /data del fstab) | [VE: fstab.cf.f2fs.hctr2, linea userdata] | CF (en el fstab del SDK /data NO es formattable [VE]) |
| 7 | Cabeceras de arranque v3/v4 solamente (rechaza v0-v2) | [V: src/bootimg.rs:35-37, 60-62] | AOSP >= 11 |
| 8 | Disco RAM = ramdisk de vendor_boot + ramdisk generico de init_boot (o de boot) concatenados, como un cargador | [V: src/main.rs, fn unpack_boot] | AOSP (boot v4) |
| 9 | Kernel x86 bzImage dentro de boot.img; se arranca con `-kernel` sin firmware | [V: src/vm.rs:409-419] [VE: boot.img v4, kernel 22897664 bytes, cabecera HdrS] | ANY x86_64 con arranque directo |
| 10 | cmdline: la de vendor_boot + `console=hvc0 earlycon=uart8250,io,0x3f8 pnpacpi=off` + `bootconfig` | [VE: cmdline de vendor_boot `printk.devkmsg=on audit=1 panic=-1 8250.nr_uarts=1 binder.impl=rust cma=0 firmware_class.path=/vendor/etc/ loop.max_part=7 init=/init bootconfig`] [V: scripts/dev-fedora.sh:187] | CF |
| 11 | Parametros `androidboot.*` por bootconfig pegado al initrd (Android 12+, kernel >= 5.10) | [V: src/vm.rs:466-497; src/main.rs, fn start: `--bootconfig requiere --initrd`] | AOSP >= 12 |
| 12 | 40 parametros fijos del perfil (hwcomposer ranchu, minigbm, apex de keymint/gatekeeper no seguros, puertos vsock de luces/modem/audiocontrol, wifi mac80211_hwsim_virtio, slot _a, verifiedbootstate=orange, densidad 280, ddr_size fija) | [V: perfiles/cuttlefish.bootconfig:6-55] | CF |
| 13 | Motor grafico por bootconfig segun --gpu: `egl=angle`, `vulkan=ranchu`, `gltransport=virtio-gpu-asg`, `cpuvulkan.version=0` | [V: src/vm.rs:174-181] | AOSP de dispositivo virtual (CF y SDK traen gfxstream invitado) [H para otras] |
| 14 | GPU `virtio-gpu-rutabaga,gfxstream-vulkan=on` + `edid=off` para 60 Hz | [V: src/vm.rs:303-306; scripts/dev-fedora.sh:196] | AOSP con gfxstream; edid=off ANY virtio-gpu |
| 15 | Consola principal hvc0; 30 consolas hvc; hvc2 = registro (logd); hvc18/19 = sensores | [V: src/vm.rs:264-289; scripts/dev-fedora.sh:190] [VE: ueventd.cutf_cvm.rc: hvc2 seriallogging, hvc3 keymaster, hvc4 gatekeeper, hvc5 bluetooth, hvc6/7 gnss, hvc8 confirmationui, hvc9 uwb, hvc10 oemlock, hvc11 keymint, hvc12 NFC, hvc14/15 MCU, hvc16 Ti50, hvc17 jcardsim, hvc18/19 sensores] | CF |
| 16 | Servicio de sensores por consolas con el protocolo de la familia (tramas u32 orden/tamano) | [V: src/sensors.rs:1-5] | CF |
| 17 | adb por vsock CID 3 puerto 5555 sin autenticacion; AUTH/RSA no implementado | [V: src/adb.rs:1-16, 197, 629-654] | CF userdebug; cualquier imagen con `ro.adb.secure=1` queda fuera |
| 18 | adbd tambien escucha TCP 5555 | [VE: prop.default del vendor_ramdisk: `persist.adb.tcp.port=5555`] [V: src/vm.rs:395 ya sabe hacer `hostfwd`] | CF; TCP es el camino mas generico [H] |
| 19 | Tres tarjetas de red: la 1.a la toma el modem simulado, la 2.a es Ethernet | [V: src/vm.rs:397-401] | CF |
| 20 | Dispositivos solo modernos (`disable-legacy=on`) | [V: src/vm.rs:245-246] | ANY kernel reciente |
| 21 | Mandos en caliente: q35 + pcie-root-port + `acpi-pci-hotplug-with-bridge-support=off` porque el kernel trae pciehp y no acpiphp | [V: src/vm.rs:361-369] | Configuracion del kernel GKI [H para otros kernels] |
| 22 | Audio virtio-sound; entrada virtio-multitouch/teclado; virtio-rng | [V: src/vm.rs:354-387] [VE: modules.load del vendor_ramdisk: virtio_input, virtio-gpu, virtio_net, virtio_blk, virtio_console, vmw_vsock_virtio_transport...] | AOSP de dispositivo virtual; ANY con esos controladores |
| 23 | Fin del arranque: texto "Boot is finished/Starting phase 1000" en hvc2, o `sys.boot_completed` por adb | [V: scripts/dev-fedora.sh:213; src/adb.rs:662-669] | hvc2: CF; boot_completed: ANY con adb |
| 24 | Carpetas compartidas: `/system/etc/init/arshare.rc` instalado por adb root + remount, nombres por bootconfig, uid de MediaProvider 10101 por defecto | [V: src/compartir.rs:1-34; src/main.rs, fn start: "necesitan arranque directo (--initrd)"; src/config.rs:109] | AOSP userdebug con bootconfig |
| 25 | `--share` crudo: lo monta el fstab de la familia en /mnt/vendor/shared | [VE: fstab.cf.f2fs.hctr2 ultima linea `shared /mnt/vendor/shared virtiofs`] | CF |
| 26 | Root: modulo cargable del proveedor actual, `/vendor/etc/init/<proveedor>.rc`, exige KMI GKI coincidente (android16-6.12) y adb root | [V: src/root.rs:1-30] [V: BITACORA 2026-10-07 KERNELSU (modo LKM)] | GKI >= 5.10 userdebug |
| 27 | Root temprano (manual): parche de init_boot | [V: BITACORA 2026-10-07 init_boot parcheado] [VE: el init_boot instalado tiene `init` (661624) + `init.real`; el original solo `init`] | AOSP con init_boot |
| 28 | Traductor ARM: `/system/lib64/libheddle.so` + 4 propiedades en `/vendor/build.prop`, con remount/disable-verity | [V: src/puente.rs:33-38] [VE: el vendor ya anuncia `ro.vendor.product.cpu.abilist=x86_64,arm64-v8a`] | AOSP userdebug x86_64 |
| 29 | Densidad por `androidboot.lcd_density`; cambio de resolucion = modo DRM + `setprop ctl.restart surfaceflinger` + `wm density` | [V: src/vm.rs:184-186; src/adb.rs:737-745] | AOSP (lcd_density) [H fuera de CF]; resto ANY con root |
| 30 | Textos de interfaz y ayuda que nombran la familia | [V: src/config.rs:97, 110; src/main.rs HELP (image, --sensors, adb)] | Cosmetico (pendiente: ver los issues del repositorio) |

Lectura: los supuestos 1-6, 10, 12, 15, 16, 19, 23 (hvc2) y 25 son de la familia; 7-9, 11, 13, 14, 24, 26-29 son de "AOSP virtual
moderno"; el resto es generico. Casi todo lo especifico ya es **dato** (lista de archivos, lista de particiones, lineas de bootconfig,
numeros de consola): se puede mover a un archivo sin tocar la logica.

---------------------------------------------------------------------------------------------------------------------------------

## 2. Familias de imagenes

### (a) Otras compilaciones y variantes de la misma familia de dispositivo virtual

- Verificado por peticion de 1 byte con rango (sin bajar nada): para la compilacion 16373615 existen tambien
  `aosp_cf_x86_64_auto` (1199863633 bytes) y `aosp_cf_arm64_only_phone` (1101175103); `..._x86_64_phone`, `_tv`, `_pc`, `_foldable`,
  `_wear`, `_slim` dan 404 con ese numero (pueden existir con otro numero de compilacion o rama [H: comprobar en la portada de
  ci.android.com por rama]).
- [VE: tmp/imagenes-analisis/lista-*.txt] el zip de `auto` tiene **exactamente los mismos 12 archivos** que el de telefono (boot, init_boot,
  vendor_boot, super, vbmeta*, userdata, android-info, fastboot-info). Su `android-info.txt` dice `config=auto`, `vhost_user_vsock=true`,
  `output_audio_streams_count=6`, `lights_server_enabled=false` (el de telefono: `config=phone`).
- Que cambia: solo datos del perfil. `auto` necesita ademas los servicios de vehiculo (VHAL: `androidboot.vhal_proxy_server_port`,
  audiocontrol, varios flujos de audio) que hoy apuntan a puertos vsock sin nadie al otro lado [V: perfiles/cuttlefish.bootconfig:44-47].
  [H] el arranque de `auto` puede quedarse esperando al VHAL igual que el telefono se quedaba esperando a los sensores; comprobar con un
  arranque de prueba y el registro de hvc2.
- `arm64` (y cualquier imagen ARM): el kernel es ARM64 (`boot.img` con imagen `ARM\x64` en 0x38 [H: comprobar con la cabecera]).
  Exige `qemu-system-aarch64` con TCG (sin KVM en x86): emulacion completa, ordenes de magnitud mas lenta. heddle traduce
  **aplicaciones** ARM en un Android x86_64; no puede ejecutar un kernel ARM. **Fuera de alcance.**
- Veredicto: **soportable** (x86_64). Riesgo bajo; es la mejor segunda imagen.

### (b) Imagenes del emulador del SDK (goldfish/ranchu)

Verificado estaticamente sobre `system-images;android-36;default;x86_64` (`x86_64-36_r02.zip`, 844217077 bytes), leyendo solo el
directorio central del zip y archivos pequenos (en total ~2 MB) [VE: tmp/imagenes-sdk/]:
- Contenido: `kernel-ranchu` (bzImage, 20 MB), `ramdisk.img` (lz4 legacy, 2,1 MB), `system.img` (1,9 GB), `vendor.img` (104 MB),
  `encryptionkey.img`, `VerifiedBootParams.textproto`, `kernel_cmdline.txt` (`8250.nr_uarts=1 clocksource=pit`), `advancedFeatures.ini`,
  `source.properties` (`SystemImage.Abi=x86_64`, `Pkg.Dependencies=emulator#35.4.9`), `build.prop`, `data/...`.
- `system.img` es un **disco GPT** con dos particiones: `vbmeta` (1 MiB) y `super` (1808 MiB). `vendor.img` es GPT con `vendor`;
  `encryptionkey.img` es GPT con `metadata`. No hay boot.img: kernel y ramdisk vienen sueltos (no hace falta unpack-boot).
- `ramdisk.img` trae `first_stage_ramdisk/fstab.ranchu`: system/vendor/product/system_ext/system_dlkm logicas; `/data` en
  `/dev/block/vdc` ext4 **sin formattable**; `metadata` por `/dev/block/pci/pci0000:00/0000:00:06.0/by-name/metadata`. Es decir, exige un
  orden y ranuras PCI concretos de discos (el que ya usaba `tests/android.rs`: vda sistema, vdb cache, vdc datos, vdd metadatos, vde vendor).
- `modules.load` del primer estadio: solo virtio (virtio_blk, virtio_console, virtio_pci, vmw_vsock_virtio_transport...).
- `advancedFeatures.ini` declara, entre otras: `VirtioVsockPipe`, `VirtconsoleLogcat`, `VirtioInput`, `VirtioSndCard`, `VirtioWifi`,
  `ModemSimulator`, `HostComposition`, `AndroidbootProps2`, `DeviceStateOnBoot`.
- Esta familia tiene su propia prueba (`tests/android.rs`), que no corre en CI [V: tests/android.rs:1-8].
- Lo que falta [H, el punto critico]: el emulador oficial es un QEMU modificado que ofrece servicios "qemud"/pipe (propiedades de
  arranque, sensores, bateria, huella, modem, registro...) por goldfish_pipe o, con `VirtioVsockPipe`, **por vsock**. Si el vendor de la
  imagen espera esos servicios para arrancar (como el de la familia actual esperaba a los sensores), weft tendria que implementar el
  lado anfitrion de ese protocolo en Rust (lo que se hizo con los sensores, pero mas grande). Comprobar estaticamente: bajar `vendor.img`
  (37 MB comprimidos, pedir permiso) y leer `init.ranchu.rc`, `ueventd.ranchu.rc` y los servicios HAL de `/vendor/etc/init`.
- QEMU estandar no tiene goldfish_pipe, goldfish_battery ni goldfish_address_space [H: comprobar con `qemu-system-x86_64 -device help`
  del anfitrion]. El usuario no quiere un QEMU a medida: todo lo que no sea virtio queda fuera o se reimplementa por vsock/virtio-serial.
- Veredicto: **con trabajo (L/XL), condicionado** a que la investigacion del vendor diga que los servicios imprescindibles son pocos.

### (c) GSI (system generico) sobre un vendor

- Una GSI trae solo `system.img` (y a veces vbmeta). Necesita un vendor compatible (Treble): el de la familia actual sirve [H: es el flujo
  documentado de Google para probar GSI en dispositivos virtuales].
- Dos caminos:
  1. **Reconstruir super** con `system_a` = GSI y el resto del vendor original. Requiere escribir metadatos de particiones logicas
     (lp_metadata: geometria, cabecera, particiones, extents, grupos) en Rust; leerlos ya se demostro en 150 lineas de Python
     [VE: tmp/imagenes-analisis/inspeccionar.py lee el super local: grupos `google_system_dynamic_partitions_a/b` y
     `google_vendor_dynamic_partitions_a/b`, system_a 910,8 MiB, vendor_a 274,6 MiB...]. AVB: con `verifiedbootstate=orange` y vbmeta
     desactivable (`disable-verity` ya existe) [H: puede hacer falta un vbmeta con la bandera de verificacion desactivada].
  2. **DSU** (Dynamic System Updates) dentro del Android ya arrancado: instala la GSI en `/data/gsi` y arranca de ella sin tocar el
     disco. Choca con la regla "no borrar nada bajo /data/gsi" (ahi vive el overlay de remount) y con el espacio de /data. Descartado como
     camino principal, anotado.
- Veredicto: **con trabajo (L)**. El trabajo es el escritor de super; arranque, consolas y sensores son los del perfil de la familia.

### (d) Android-x86 / BlissOS (y Waydroid)

- Formato [H]: ISO hibrida con GRUB/isolinux, `kernel`, `initrd.img`, `system.sfs` o `system.efs` (squashfs/erofs con system.img
  dentro). init propio de Android-x86 que busca `SRC=` en la linea de ordenes; graficos Mesa (virgl o software), sin gfxstream;
  adb por red (TCP), sin vsock; propiedades por cmdline (`androidboot.hardware=android_x86_64`), sin bootconfig en versiones viejas.
- Arranque en QEMU: o firmware (SeaBIOS/OVMF + ISO, `-cdrom` ya existe [V: src/vm.rs:403-408]) o arranque directo extrayendo kernel e
  initrd de la ISO (leer ISO9660 en Rust es pequeno). Graficos: virtio-vga-gl/virgl (QEMU de Fedora lo trae [H]), que choca con la ventana
  propia (`-display dbus,gl=off`: virgl necesita contexto GL en QEMU) [V: src/vm.rs:336-337 rechaza virtio-gl con window].
- Lo que se pierde: gfxstream (la ventaja de rendimiento principal), sensores, root por modulo GKI (kernels no GKI), carpetas por
  bootconfig (sin bootconfig se pasarian por cmdline: `androidboot.*` en cmdline sigue llegando a ro.boot.* [H]), traductor (traen
  libhoudini/libndk propio; el puente iria igual por native bridge [H]).
- Waydroid: es un contenedor LXC sobre el kernel del anfitrion (binder del anfitrion), no una maquina virtual. Sus system/vendor
  (LineageOS) esperan ese entorno. **Fuera de alcance.**
- Veredicto Android-x86/BlissOS: **con trabajo (L)**, valor dudoso frente a (a)/(c). Waydroid: fuera.

### (e) Imagenes ARM64 para `qemu-system-aarch64 -M virt`

- Sin KVM en un anfitrion x86 todo el sistema se emula (TCG): Android tarda minutos en arrancar y la interfaz no es usable;
  gfxstream sobre TCG aarch64 tampoco esta en el QEMU del sistema con rutabaga+gfxstream probado [H].
- Veredicto: **fuera de alcance**. El caso "apps ARM" ya lo cubre heddle dentro de un Android x86_64.

### Resumen

| Familia | Veredicto | Motivo principal |
|---------|-----------|------------------|
| (a) misma familia x86_64, otras compilaciones/variantes | soportable (S-M) | Mismo formato verificado; solo cambian datos |
| (a') misma familia arm64 | fuera | Kernel ARM: emulacion completa |
| (b) SDK goldfish/ranchu x86_64 | con trabajo (L/XL), condicionado | Servicios del QEMU modificado del SDK; discos ya compatibles |
| (c) GSI sobre el vendor de (a) | con trabajo (L) | Escritor de super (lp_metadata) en Rust |
| (d) Android-x86/BlissOS | con trabajo (L) | Arranque ISO/firmware, virgl, sin gfxstream |
| (d') Waydroid | fuera | No es una maquina virtual |
| (e) ARM64 en virt | fuera | Sin KVM |
| Imagenes de telefonos reales | fuera | ARM y HAL de hardware real |

---------------------------------------------------------------------------------------------------------------------------------

## 3. Modelo propuesto: perfil de imagen

### Principios
- El perfil es **dato declarativo** (clave=valor como `config`, sin crates, con comentarios). La logica (QEMU, GPT, bootconfig, adb,
  sensores) sigue en Rust y se elige por **nombres cerrados** (enums), no por codigo en el archivo.
- Dos niveles: **perfil** (como se arranca una familia) e **imagen instalada** (una carpeta concreta con su perfil, origen y huella).
- Perfiles integrados en el ejecutable (`include_str!` de `perfiles/*.perfil`; hoy el `.bootconfig` lo pasa el guion y solo la prueba lo incrusta [V: src/vm.rs:716]) y perfiles
  de usuario en `$XDG_CONFIG_HOME/weft/perfiles/*.perfil` (persistente: el estado vive en XDG_RUNTIME_DIR, que se borra
  [V: src/config.rs:12-13]). Mismo nombre: manda el del usuario, con aviso. Agregar un perfil = copiar un archivo; sin recompilar.
- Los nombres de perfil son descriptivos y genericos ("movil-virtual-x86_64", "coche-virtual-x86_64"); la familia concreta solo
  aparece en el comentario y en `perfil.familia` (dato tecnico visible en `image info`, no en la interfaz).

### Archivo de perfil (ejemplo completo del que reproduce el arranque actual)

```
# Perfil integrado: dispositivo virtual movil AOSP x86_64 (GKI, boot v4). Reproduce la receta de scripts/dev-fedora.sh.
perfil.id=movil-virtual-x86_64
perfil.nombre=Telefono virtual x86_64
perfil.familia=cuttlefish            # dato tecnico; no se muestra en la interfaz
perfil.formato=1                     # version del formato de perfil

# --- deteccion (todas las condiciones "requiere" deben cumplirse; "puntua" suma para desempatar)
detectar.requiere=archivo:boot.img archivo:vendor_boot.img archivo:super.img
detectar.requiere=kernel:x86_64
detectar.requiere=vendor_bootconfig:androidboot.hardware=cutf_cvm
detectar.puntua=android_info:config=phone
detectar.puntua=archivo:init_boot.img

# (weft NO descarga imagenes: los perfiles ya no llevan origen; la imagen se instala desde un archivo con `image add`)

# --- disco
disco.modo=gpt                       # gpt (un disco armado) | archivos (cada .img es un disco tal cual)
disco.ranura_pci=4
disco.particion=misc:1M
disco.particion=metadata:64M
disco.particion=frp:1M
disco.particion_ab=boot init_boot vendor_boot vbmeta vbmeta_system vbmeta_system_dlkm vbmeta_vendor_dlkm
disco.particion=super:super.img
disco.datos=userdata                 # particion que toma disk.data (vacia N GiB o img)
disco.datos_formatea_solo=si         # el fstab la marca formattable (si no, hay que copiar userdata.img o crear el sistema de archivos)

# --- arranque
arranque.modo=directo                # directo (kernel+initrd) | firmware (BIOS/UEFI con disco o ISO)
arranque.kernel=boot.img#kernel
arranque.ramdisk=vendor_boot.img#ramdisk init_boot.img#ramdisk
arranque.cmdline={vendor_cmdline} console=hvc0 earlycon=uart8250,io,0x3f8 pnpacpi=off
arranque.bootconfig_archivo=vendor_boot.img#bootconfig
arranque.parametros=bootconfig       # bootconfig | cmdline (androidboot.* en la linea del kernel, imagenes viejas)
bc.androidboot.boot_devices=pci0000:00/0000:00:{disco.ranura_pci:02x}.0
bc.androidboot.fstab_suffix=cf.f2fs.hctr2
bc.androidboot.lcd_density={densidad|280}
bc.androidboot.serialno=WEFT{maquina:04}
# ... el resto de las 40 lineas actuales de perfiles/cuttlefish.bootconfig, igual
maquina.ram_minima=2048
maquina.tipos=q35 pc

# --- dispositivos
consola.principal=hvc
consola.hvc=30
consola.registro=2                   # hvc2: registro de Android (registro-android.txt)
sensores=consolas-movil:18,19        # protocolo de sensores por dos consolas (src/sensors.rs)
gpu.motores=gfxstream software
gpu.bc.gfxstream=androidboot.hardware.egl=angle androidboot.hardware.vulkan=ranchu androidboot.hardware.gltransport=virtio-gpu-asg androidboot.cpuvulkan.version=0
gpu.bc.software=androidboot.hardware.egl=angle androidboot.hardware.vulkan=pastel androidboot.cpuvulkan.version=4202496
gpu.opciones=edid=off
red.tarjetas=3
audio=virtio
entrada=virtio-multitouch

# --- adb y fin del arranque
adb.transporte=vsock tcp             # en orden de preferencia
adb.puerto=5555
arranque.listo=adb:sys.boot_completed registro:Boot is finished|Starting phase 1000

# --- capacidades (cada una con su requisito; si falta, la funcion se oculta o se explica)
capacidad.root=modulo-gki            # modulo-gki | parche-ramdisk | ninguna
capacidad.carpetas=init-rc           # init-rc (arshare.rc por adb root) | fstab (etiqueta virtiofs del vendor) | ninguna
capacidad.carpetas_fstab=shared:/mnt/vendor/shared
capacidad.traductor=native-bridge    # native-bridge | ninguna
capacidad.mandos=virtio-input-host
capacidad.resolucion=drm-surfaceflinger
```

Reglas del formato:
- Claves repetibles (`disco.particion`, `detectar.requiere`, `bc.*`) conservan el orden. `#` comenta. Valor no valido = perfil
  rechazado con la linea exacta (no se adivina).
- Plantillas con `{variable}` cerradas y documentadas: `{build}`, `{vendor_cmdline}`, `{densidad|defecto}`, `{maquina}`,
  `{disco.ranura_pci:02x}`, `{ram_mb}`. Sin expresiones.
- `ARCHIVO#parte` = parte extraida por `bootimg.rs` (kernel, ramdisk, bootconfig, cmdline de boot o vendor_boot).

### Estructuras en Rust (borrador)

```rust
pub struct Perfil {
    pub id: String, pub nombre: String, pub familia: String, pub origen_archivo: OrigenPerfil, // Integrado | Usuario(PathBuf)
    pub detectar: Vec<Condicion>, pub puntuar: Vec<Condicion>,
    pub descarga: Option<Descarga>,               // plantilla de URL, build por defecto, modo de tamano
    pub disco: Disco, pub arranque: Arranque, pub disp: Dispositivos, pub adb: AdbPerfil, pub cap: Capacidades,
}
pub enum Condicion { Archivo(String), Kernel(Arq), AndroidInfo(String, String), VendorBootconfig(String, String),
                     SourceProp(String, String), Iso, CabeceraBoot(u32) }
pub enum Arq { X86_64, Arm64 }
pub enum Disco { Gpt { ranura: u32, partes: Vec<ParteDisco>, datos: String, formatea_solo: bool },
                 Archivos { discos: Vec<DiscoArchivo> } }          // DiscoArchivo { archivo, ranura, solo_lectura, copia_trabajo }
pub enum ParteDisco { Vacia(String, u64), Archivo(String, String), Ab(Vec<String>) }
pub enum Arranque { Directo { kernel: Fuente, ramdisks: Vec<Fuente>, cmdline: Plantilla, bootconfig: Vec<Fuente>,
                              parametros: ViaParametros, lineas: Vec<(String, Plantilla)> },
                    Firmware { iso: Option<String>, uefi: bool } }
pub enum Fuente { Archivo(String), Parte(String, ParteBoot) }   // ParteBoot: Kernel | Ramdisk | Bootconfig | Cmdline
pub enum ViaParametros { Bootconfig, Cmdline }
pub struct Dispositivos { consola: Consola, hvc: u32, registro: Option<u32>, sensores: Sensores, gpu: Vec<Motor>,
                          gpu_bc: Vec<(Motor, Vec<String>)>, gpu_opciones: String, nics: u32, audio: bool, entrada: Entrada }
pub enum Sensores { Ninguno, ConsolasMovil(u32, u32) /* , futuro: PipeVsock */ }
pub struct AdbPerfil { transportes: Vec<Transporte>, puerto: u32, listo: Vec<Listo> }   // Transporte: Vsock | Tcp
pub struct Capacidades { root: Root, carpetas: Carpetas, traductor: bool, mandos: bool, resolucion: bool }
pub struct ImagenInstalada { id: String, carpeta: PathBuf, perfil: String, origen: String, build: Option<u32>,
                             huella: String /* ro.build.fingerprint */, version: String, kmi: Option<String>, tamano: u64 }
```

`ImagenInstalada` se guarda como `imagen.info` (clave=valor) dentro de la carpeta de cada imagen:
`$XDG_DATA_HOME/weft/imagenes/<id>/` (o la carpeta que el usuario indique; hoy `./imagen`, que sigue valiendo).

### Detector (estatico, sin arrancar)

Todo lo necesario se demostro sin arrancar nada en `tmp/imagenes-analisis/inspeccionar.py`:
1. Archivos presentes y `android-info.txt` / `source.properties` / `fastboot-info.txt` [VE: la imagen local trae `config=phone`,
   `gfxstream=supported`; fastboot-info empieza por `# cuttlefish`].
2. Cabeceras de boot/vendor_boot (`bootimg.rs` ya las lee) y bootconfig de vendor_boot [VE: `androidboot.hardware=cutf_cvm`].
3. Arquitectura del kernel: `HdrS` en 0x202 (bzImage) o `ARM\x64` en 0x38.
4. Disco RAM (lz4 legacy: descompresor de ~60 lineas, ya escrito en Python; gzip: hace falta inflate, que se puede evitar porque las
   imagenes vistas usan lz4 [VE: init_boot, vendor_boot y ramdisk del SDK son lz4 legacy]) -> cpio -> `fstab.*`, `ueventd.*.rc`,
   `prop.default`/`build.prop` (huella, version, tipo userdebug/user, `ro.adb.secure`), `modules.load`.
5. Discos: GPT (`gpt.rs`), simg y lp_metadata de super (lectura).
6. Huella de root ya puesta: init_boot con `init` + `init.real` [VE].
Salida: perfil elegido y, si ninguno cumple, la lista de condiciones que fallaron en lenguaje claro ("el kernel es ARM64: este
emulador solo ejecuta Android x86_64", "adbd exige firma (imagen user): no se podra controlar", "falta vendor_boot.img").

---------------------------------------------------------------------------------------------------------------------------------

## 4. Interfaz y ordenes (genericas)

Ordenes nuevas (la palabra "imagen" no nombra la familia):
```
weft image add ORIGEN [--id ID] [--profile PERFIL] [--yes]
      ORIGEN: archivo .zip | carpeta, ya descargados (weft no descarga nada)
      detecta el perfil, comprueba espacio, descomprime en $XDG_DATA_HOME/weft/imagenes/ID, escribe imagen.info
weft image list                 # id, nombre del perfil, version de Android, tamano, maquinas que la usan
weft image info ID|CARPETA      # perfil, huella, version, tipo, kernel/KMI, capacidades disponibles y por que no las demas
weft image remove ID [--yes]    # se niega si una maquina la usa
weft image profiles             # perfiles integrados y de usuario, con su origen
weft image check CARPETA|ZIP    # solo detecta y explica (no copia nada)
weft start --image ID           # arma el disco si falta, extrae kernel/initrd (cache en la carpeta de la imagen) y arranca
                                      # con la receta del perfil; las opciones de siempre mandan sobre el perfil
```
- weft no descarga nada; `image path|list --dir` toman una carpeta suelta, y `disk create|status|reset` toman la imagen de la
  maquina. Seleccion por maquina: clave `imagen` en `config` de la maquina (precedencia de siempre: `--image` > config > defecto
  = la unica imagen instalada o la integrada por defecto).
- `start` sin `--image` y con `--kernel/--initrd/--disk` sigue siendo el lanzador crudo (no se rompe nada ni la CI).

Seccion de la interfaz "Imagen de Android" (dentro de Avanzado; pendiente: ver los issues del repositorio):
- Lista de imagenes instaladas (nombre del perfil, version de Android, tamano) con "En uso por esta maquina".
- Botones: Agregar (selector de archivo o carpeta; URL en un campo), Detalles, Quitar (deshabilitado si esta en uso).
- "Usar en esta maquina" (se aplica al proximo arranque; cambiar de imagen implica disco nuevo: aviso en rojo con dos confirmaciones).
- Capacidades de la imagen elegida como lista de comprobacion: Aceleracion grafica, Acceso root, Traductor ARM, Carpetas compartidas,
  Sensores, Mandos; las que no aplican, grises con el motivo en una linea.

Preguntas al usuario (solo cuando no se pueden deducir):
- Si dos perfiles empatan: elegir cual (con su descripcion).
- Tamano de /data para el disco nuevo (con el valor de `disk.data` ya puesto).
- Confirmar descarga con tamano declarado; confirmar el reemplazo del disco al cambiar de imagen.
No se pregunta: particiones, ranuras, consolas, parametros de arranque, motor grafico, transporte de adb (todo sale del perfil).

---------------------------------------------------------------------------------------------------------------------------------

## 5. Impacto en lo existente

- **Root** (proveedor): hoy modulo cargable que exige KMI GKI coincidente [V: src/root.rs; BITACORA KMI android16-6.12] y userdebug
  (adb root para escribir `/vendor/etc/init/...`). Con perfiles: `capacidad.root=modulo-gki` se ofrece solo si el KMI de la imagen
  coincide con el que publica el proveedor (KMI leido de `uname -r` por adb tras el primer arranque, o estatico descomprimiendo el
  bzImage lz4). Imagen sin init_boot (boot v3 o v4 sin init_boot): el parche temprano iria sobre el ramdisk de `boot.img`; con
  ramdisk dentro del kernel o kernel no GKI: solo `ninguna`. Encaja con la abstraccion de "proveedor de root" (pedido 3): el perfil
  dice que METODO admite la imagen; el proveedor dice que KMI soporta.
- **Traductor ARM**: rutas `/system/lib64` y `/vendor/build.prop` [V: src/puente.rs:33-35] valen para AOSP x86_64 userdebug con remount.
  GSI: igual (el vendor es el mismo). SDK: [H] mismas rutas pero las propiedades de ABI estan en `vendor/build.prop` del vendor.img;
  verificar. Android-x86: trae su native bridge (houdini/ndk) y otra ubicacion de propiedades [H]. Se declara por perfil; el instalador
  vigilado no cambia (su seguridad no depende de la imagen).
- **Carpetas compartidas**: el metodo actual (init rc en /system por adb root + bootconfig + MediaProvider) es AOSP generico salvo que
  exige bootconfig y userdebug; en imagenes sin bootconfig los nombres irian por cmdline (`androidboot.arshareN` en la linea del kernel
  [H: init los copia a ro.boot.* igual]). El uid de MediaProvider ya se aprende por adb (`share status`) [V: src/compartir.rs:18-20].
  El modo crudo `--share` depende del fstab de la familia (`capacidad.carpetas_fstab`).
- **Sensores**: protocolo por consolas de la familia [V: src/sensors.rs]. Otras familias: `sensores=ninguno` (si el vendor no los
  exige para arrancar) o un protocolo nuevo (SDK: [H] servicio "sensors" por pipe sobre vsock). Es la principal fuente de cuelgues de
  arranque vista hasta ahora [V: BITACORA 2026-10-07 ARRANQUE COLGADO POR SENSORES], asi que cada perfil nuevo debe declarar que servicios
  del anfitrion espera.
- **Pantalla**: `edid=off` es de QEMU y vale para cualquier virtio-gpu. Densidad: `androidboot.lcd_density` (AOSP); en el SDK el
  emulador usa `qemu.sf.lcd_density`/propiedades por pipe [H]; en Android-x86, cmdline. Cambio de resolucion en caliente depende del
  hwcomposer (drm en la familia actual) -> `capacidad.resolucion`.
- **adb**: hoy solo vsock. TCP con `hostfwd` ya existe en vm.rs [V: src/vm.rs:395]; falta el transporte TCP en `adb.rs` (el protocolo es
  el mismo; `Adb::handshake` ya es generico sobre el flujo [V: src/adb.rs:184]). TCP ademas resuelve "varias maquinas a la vez" sin pelear
  por el CID 3 (pendiente: ver los issues del repositorio) y funciona sin /dev/vhost-vsock. AUTH RSA sigue fuera: imagenes `user` (ro.adb.secure=1) no se podran
  controlar -> el detector lo dice antes de instalar.
- **Entrada**: virtio-multitouch funciona con cualquier kernel con virtio_input [VE: modulo presente en ambas familias]; el SDK lo
  anuncia (`VirtioInput=on`) [VE]. Android-x86 tambien deberia [H]. Los idc de wacom son genericos de Android.
- **CI**: la tarea actual sigue con el perfil integrado; prueba nueva: `start --image X --dry-run` debe dar la MISMA linea de QEMU y el
  mismo bootconfig que el guion actual (comparacion textual), sin arrancar Android.

---------------------------------------------------------------------------------------------------------------------------------

## 6. Plan por fases

| Fase | Contenido | Esfuerzo | Criterio de aceptacion medible | Sin arrancar imagenes de terceros |
|------|-----------|----------|--------------------------------|-----------------------------------|
| 0 | Textos genericos (ya encargado) y **receta del guion a Rust**: `start --image` con la receta actual codificada como perfil integrado; el guion queda en `image add` + `start --image` | M | `--dry-run` de `start --image` igual byte a byte (salvo rutas) a la linea que arma hoy `dev-fedora.sh`; bootconfig resultante identico; CI cuttlefish verde sin cambios | Si (dry-run y pruebas unitarias) |
| 1 | Formato `.perfil` + parser + perfiles integrados/usuario + detector estatico (lz4 legacy, cpio, android-info, cabeceras, lp de super) + `image add/list/info/remove/check/profiles` | M | Pruebas unitarias del parser (errores con numero de linea); `image check` sobre la imagen local elige el perfil, da huella `...:17/CP2A.260605.016/16373615:userdebug`, detecta el init_boot ya parcheado; sobre los metadatos del SDK en tmp responde "no compatible todavia" con el motivo; sobre un zip arm64 responde "ARM64: fuera de alcance" | Si |
| 2 | **Segunda imagen de la misma familia**: otra compilacion del telefono x86_64 y luego la variante coche (`config=auto`) | S + M | Por imagen: boot_completed, gfxstream activo ("Virtio-GPU GFXStream"), adb, toque, apply-settings; en el telefono tambien el banco (modo bench) y el traductor con el instalador vigilado. Corridas acotadas (<=5 por tanda, avisadas) | No: requiere bajar ~1,2 GB por imagen y arrancarla (pedir permiso) |
| 3 | adb por TCP (`hostfwd`) como segundo transporte + fin de arranque por adb sin depender de hvc2 + capacidades que se ocultan con motivo | M | `adb-shell`, `install`, `wait-adb` funcionan con `--adb tcp` sin vhost-vsock; dos maquinas a la vez con CID/puerto distintos | Parcial (pruebas unitarias; la prueba real necesita arrancar la imagen actual) |
| 4 | GSI sobre el vendor del perfil movil: escritor de lp_metadata (super) en Rust + vbmeta sin verificacion si hace falta | L | Super reconstruido con el system ORIGINAL da un disco que arranca igual que el actual (prueba de no regresion); luego una GSI x86_64 llega a boot_completed | El escritor y su prueba de ida y vuelta (leer lo escrito) si; el arranque no |
| 5 | Investigacion SDK: bajar `vendor.img` (37 MB, con permiso) y enumerar servicios del anfitrion que exige; decidir si se implementan por vsock | S (investigacion) + L/XL (implementacion) | Informe con la lista de servicios, cuales bloquean el arranque y el protocolo de cada uno | La investigacion si (estatica) |
| 6 | Android-x86/BlissOS: arranque por ISO/firmware o directo desde la ISO, virgl, adb TCP | L | boot_completed y adb por TCP en una imagen de esta familia | No |
| - | ARM64, Waydroid, telefonos reales | - | Fuera de alcance (mensaje claro del detector) | - |

Recomendacion: empezar por **0 -> 1 -> 2**. Motivos: (1) la receta hoy vive en un guion de shell: sin la fase 0 cada imagen nueva seria
otro guion; (2) la fase 1 se puede hacer y probar entera sin arrancar nada; (3) la variante coche es la segunda imagen mas barata y ya
se verifico que existe para la misma compilacion con el mismo formato de 12 archivos: valida que el perfil separa bien "familia" de
"variante" (cambian bootconfig y servicios, no el formato); (4) GSI (fase 4) es la puerta a "cualquier system" con un vendor que ya
funciona, y es mas valiosa que el SDK, cuyo coste depende de servicios aun no inventariados.

---------------------------------------------------------------------------------------------------------------------------------

## 7. Riesgos y preguntas abiertas

Riesgos
1. Cuelgues de arranque por servicios del anfitrion que la imagen espera y nadie atiende (ya visto con sensores). Cada perfil nuevo debe
   inventariar esos servicios antes de arrancar; el instalador de imagen no puede garantizarlo de forma estatica.
2. "Cualquier imagen" crea expectativas: imagenes `user` (adb con firma), ARM, Waydroid y de telefonos no van a funcionar. El detector
   debe decirlo antes de copiar gigas.
3. Root por modulo depende del KMI exacto: otra compilacion con otro kernel puede quedarse sin root hasta que el proveedor publique ese KMI.
4. El escritor de super toca la pieza mas delicada del disco: un error deja la imagen sin arrancar; se mitiga con prueba de ida y vuelta y
   reconstruyendo primero con el system original.
5. Espacio: cada imagen ocupa 2-4 GB descomprimida mas el disco de la maquina.
6. Perfiles de usuario mal escritos: se validan al cargar y se rechazan con la linea exacta; nunca se adivina un valor.

Preguntas para el usuario
1. Que imagenes te importan de verdad (por orden): otras compilaciones del telefono actual, la variante coche/TV/tablet, GSI de otras
   ROM (cuales), el emulador del SDK (con Google Play o sin), BlissOS/Android-x86, otra?
2. Aceptas que queden fuera ARM64, Waydroid, imagenes `user` con adb firmado e imagenes de telefonos reales?
3. Perfiles: archivo por perfil en `~/.config/weft/perfiles/` (propuesto) o todo integrado en el ejecutable?
4. Donde guardar las imagenes: `$XDG_DATA_HOME/weft/imagenes/` (propuesto) o junto al disco como hoy (`./imagen`)?
5. Una imagen por maquina con disco propio (propuesto), o permitir cambiar la imagen de una maquina regenerando el disco?
6. Para GSI: aceptas DSU como alternativa pese a que escribe en /data/gsi, o solo reconstruir super (propuesto)?
7. Implementar el lado anfitrion de servicios del SDK por vsock (si la investigacion de la fase 5 lo justifica) o dejar el SDK fuera?
8. adb por TCP como transporte por defecto cuando no haya vhost-vsock (y para varias maquinas), o vsock siempre que este?
9. Permiso para bajar imagenes de prueba de la fase 2 (~1,2 GB cada una) y, en la fase 5, `vendor.img` del SDK (37 MB).
