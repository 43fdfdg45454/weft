# Perfil integrado: dispositivo virtual AOSP de automocion x86_64. Hereda del movil: solo cambia lo que difiere.
perfil.id=car-x86_64
perfil.nombre=Coche virtual x86_64
perfil.descripcion=Imagen de dispositivo virtual AOSP para automocion (misma estructura que el teléfono; servicios de vehículo).
perfil.hereda=phone-x86_64
perfil.aviso=Esta familia necesita un servicio de vehiculo del anfitrion (el HAL de vehiculo de la imagen se conecta por gRPC a 192.168.98.1:9300) que weft todavia no implementa: el sistema arranca y el adb funciona, pero sys.boot_completed no llega (medido en 16373615).

detectar.requiere=archivo:boot.img archivo:vendor_boot.img archivo:super.img
detectar.requiere=kernel:x86_64
detectar.requiere=vendor_bootconfig:androidboot.hardware=cutf_cvm
detectar.puntua=android_info:config=auto
detectar.puntua=archivo:init_boot.img

# --- disco: como el movil mas una particion de intercambio (hibernacion) ANTES de los datos. Medido: el arranque falla (reinicio en
# bucle hacia recovery) con el disco del movil, porque la politica de seguridad de esta imagen etiqueta /dev/block/vda19 como
# dispositivo de intercambio (swap_block_device) y pide androidboot.hibernation_resume_device=259:3 (vda19): con el disco del
# movil la particion de datos queda en vda19 y make_f2fs/vold no pueden abrirla. Con la de intercambio en vda19 los datos pasan a vda20.
disco.modo=gpt
disco.particion=misc:1M
disco.particion=metadata:64M
disco.particion=frp:1M
disco.particion_ab=boot init_boot vendor_boot vbmeta vbmeta_system vbmeta_system_dlkm vbmeta_vendor_dlkm
disco.particion=super:super.img
disco.particion=swap:1G
disco.datos=userdata:userdata.img

# --- adb: solo vsock. Medido: con la red de usuario de QEMU ninguna tarjeta obtiene direccion en esta imagen (solo `lo` tiene IP; la
# red del vehiculo la configura su propio servicio), asi que no hay a donde reenviar un puerto TCP.
adb.transportes=vsock

