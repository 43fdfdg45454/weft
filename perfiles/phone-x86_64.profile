# Perfil integrado: dispositivo virtual movil AOSP x86_64 (arranque directo del kernel, cabeceras de arranque v3/v4).
# Es la receta que antes vivia en scripts/dev-fedora.sh. Formato: clave=valor, una por linea; solo lineas enteras
# de comentario (#). Las claves repetibles conservan el orden. Un valor desconocido rechaza el perfil con su numero de linea.
perfil.id=phone-x86_64
perfil.nombre=Teléfono virtual x86_64
perfil.descripcion=Imagen de dispositivo virtual AOSP de tipo teléfono (kernel x86_64, particiones A/B, GKI).
perfil.familia=cuttlefish
perfil.formato=1
perfil.arquitectura=x86_64

# --- deteccion: todas las condiciones "requiere" deben cumplirse; "puntua" suma para desempatar entre perfiles
detectar.requiere=archivo:boot.img archivo:vendor_boot.img archivo:super.img
detectar.requiere=kernel:x86_64
detectar.requiere=vendor_bootconfig:androidboot.hardware=cutf_cvm
detectar.puntua=android_info:config=phone
detectar.puntua=archivo:init_boot.img


# --- disco: GPT propio en el orden de siempre (misc, metadata, frp, A/B, super, datos)
disco.modo=gpt
disco.ranura_pci=4
disco.particion=misc:1M
disco.particion=metadata:64M
disco.particion=frp:1M
disco.particion_ab=boot init_boot vendor_boot vbmeta vbmeta_system vbmeta_system_dlkm vbmeta_vendor_dlkm
disco.particion=super:super.img
disco.datos=userdata:userdata.img

# --- arranque directo: kernel de boot, discos RAM de vendor_boot e init_boot concatenados, bootconfig de vendor_boot + este perfil
arranque.modo=directo-android
arranque.boot=boot.img
arranque.init_boot=init_boot.img
arranque.vendor_boot=vendor_boot.img
arranque.cmdline_extra=console=hvc0 earlycon=uart8250,io,0x3f8 pnpacpi=off
arranque.bootconfig_archivo=cuttlefish.bootconfig
aceleracion=kvm

# --- dispositivos
consola=hvc
consola.hvc=30
consola.registro=2
sensores=18,19
entrada.tactil=si
red.tarjetas=3
gpu.opciones=edid=off
# motores graficos que admite la imagen: hardware (aceleracion por hardware del equipo) y software (sin aceleracion)
gpu.motores=hardware software
maquina.ram_minima=2048
maquina.tipos=q35 pc

# --- adb: vsock primero; por TCP se reenvia un puerto local al 5555 de la tarjeta de red indicada. Medido en el invitado: solo la
# 3.a tarjeta (indice 2, eth2, red 10.0.4.0/24) obtiene direccion por DHCP; la 1.a la usa el modem simulado
adb.transportes=vsock tcp
adb.puerto=5555
adb.tcp_red=2

# --- capacidades (informativas: `image info` y la pantalla de configuracion las muestran)
capacidad.root=modulo-gki
capacidad.carpetas=init-rc
capacidad.traductor=native-bridge
capacidad.mandos=virtio-input-host
capacidad.resolucion=drm-surfaceflinger
