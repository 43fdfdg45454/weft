//! Configuracion de la maquina virtual y construccion de la linea de comandos de QEMU.

use std::path::{Path, PathBuf};
use crate::textos::{tx, txf};

#[derive(Clone, Debug, PartialEq)]
pub enum Accel {
    Auto,
    Kvm,
    Tcg,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub qemu: String,
    pub kernel: Option<String>,
    pub initrd: Option<String>,
    pub append: String,
    /// (ruta, solo lectura)
    pub disks: Vec<(String, bool)>,
    pub cdrom: Option<String>,
    /// propiedades adicionales del dispositivo de GPU (texto tal cual para QEMU)
    pub gpu_opts: String,
    /// memoria del anfitrion que la GPU gfxstream puede compartir con el invitado (MiB)
    pub gpu_hostmem_mb: u32,
    /// q35 | pc, opcionalmente con propiedades ("pc,nvdimm=on")
    pub machine: String,
    /// ranura PCI del primer disco
    pub disk_slot: u32,
    /// resolucion de la pantalla virtual (GPU virtio)
    pub resolution: Option<(u32, u32)>,
    /// identificador vsock del invitado (para adb por vsock)
    pub vsock_cid: Option<u32>,
    /// la consola principal es la consola virtio hvc0 (en lugar del puerto serie)
    pub console_hvc: bool,
    /// puertos del bus de consolas virtio (hvc)
    pub hvc_ports: u32,
    /// consolas hvc1..=N que crea weft (0 = ninguna); sin destino salvo las indicadas abajo
    pub hvc_count: u32,
    /// (numero de consola, archivo) : lo que el invitado escribe en esa consola se guarda en el archivo
    pub hvc_logs: Vec<(u32, String)>,
    /// consolas de control y de datos del servicio de sensores (Cuttlefish)
    pub sensors: Option<(u32, u32)>,
    /// carpetas del anfitrion compartidas con el invitado por virtiofs: (etiqueta, carpeta)
    pub shares: Vec<(String, String)>,
    /// pantalla tactil (virtio-multitouch) y teclado virtio, como los espera Android
    pub touch: bool,
    /// puntero del raton de la ventana: tablet (USB, coordenadas absolutas; Android no lo usa), wacom (tableta
    /// grafica USB: Android la trata como pantalla tactil, asi el raton toca), multitouch (la pantalla tactil virtio:
    /// la ventana propia envia los toques por D-Bus; con gtk/sdl no llegan, porque esas ventanas solo mandan
    /// posiciones absolutas, que la pantalla tactil no recibe) o none
    pub pointer: String,
    /// mandos de juegos del anfitrion: none | auto | /dev/input/eventN (ver gamepad.rs). Si no es none, en q35 se
    /// reservan puertos PCIe para poder conectarlos en caliente
    pub gamepad: String,
    /// dibujo de la ventana con OpenGL: auto | on | off
    pub display_gl: String,
    /// arrancar en pausa para fijar la resolucion antes de que el invitado la lea (ventana GTK con --resolution)
    pub paused: bool,
    /// audio: none | wav:ARCHIVO (lo que suena en el invitado se graba en el archivo)
    pub audio: String,
    pub mem_mb: u32,
    pub cpus: u32,
    /// none | gtk | sdl
    pub display: String,
    /// std | virtio | virtio-gl | none
    pub gpu: String,
    pub accel: Accel,
    pub net: bool,
    /// numero de tarjetas de red (modo usuario)
    pub nics: u32,
    /// puerto local reenviado al 5555 del invitado (adb)
    pub adb_port: Option<u16>,
    /// tarjeta de red (0..) cuyo reenvio lleva al adbd del invitado (la 1.a, n0, la usa el modem simulado de la familia actual)
    pub adb_net: u32,
    pub extra: Vec<String>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            qemu: "qemu-system-x86_64".into(),
            kernel: None,
            initrd: None,
            append: String::new(),
            disks: Vec::new(),
            cdrom: None,
            gpu_hostmem_mb: 256,
            gpu_opts: String::new(),
            machine: "q35".into(),
            disk_slot: 3,
            resolution: None,
            vsock_cid: None,
            console_hvc: false,
            hvc_ports: 31,
            hvc_count: 0,
            hvc_logs: Vec::new(),
            sensors: None,
            shares: Vec::new(),
            touch: false,
            pointer: "tablet".into(),
            gamepad: "none".into(),
            display_gl: "auto".into(),
            paused: false,
            audio: "none".into(),
            mem_mb: 2048,
            cpus: 2,
            display: "none".into(),
            gpu: "auto".into(),
            accel: Accel::Auto,
            net: true,
            nics: 1,
            adb_port: None,
            adb_net: 0,
            extra: Vec::new(),
        }
    }
}

/// Archivos de estado de una maquina (todo dentro de un directorio privado).
pub struct State {
    pub dir: PathBuf,
}

impl State {
    pub fn new(base: Option<&str>, name: &str) -> Result<State, String> {
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(tx!("vm.el_nombre_solo_admite_letras_numeros_y").into());
        }
        // sin --state-dir: WEFT_STATE_DIR, <raiz>/state o $XDG_RUNTIME_DIR/weft (ver rutas.rs)
        let base = match base {
            Some(b) => PathBuf::from(b),
            None => crate::rutas::actual().ejecucion,
        };
        Ok(State { dir: base.join(name) })
    }
    pub fn f(&self, n: &str) -> String {
        self.dir.join(n).to_string_lossy().into_owned()
    }
    pub fn qmp(&self) -> String {
        self.f("qmp.sock")
    }
    pub fn serial_sock(&self) -> String {
        self.f("serial.sock")
    }
    pub fn serial_log(&self) -> String {
        self.f("serial.log")
    }
    pub fn pidfile(&self) -> String {
        self.f("pid")
    }
    /// Pid de QEMU, si este proceso lo puede ver y el proceso sigue siendo ese QEMU. Con Flatpak cada `flatpak run` tiene su
    /// propio espacio de pids (comprobado), asi que el pid guardado por otra instancia apuntaria a un proceso ajeno: `start`
    /// anota el espacio de pids en `pid-ns`. Y un numero de pid se reutiliza: si QEMU murio sin borrar su archivo (SIGKILL,
    /// apagon) otro proceso cualquiera puede llevar ese numero; `start` guarda en `pid-start` el tiempo de arranque del
    /// proceso (ver `alive_con_inicio`) y aqui solo cuenta como vivo el proceso que lo tenga. Sin `pid-start` (maquina
    /// arrancada por una version anterior) basta con que el proceso sea un QEMU. Un pid que ya no es el de la maquina se
    /// olvida (se borran `pid` y `pid-start`): la maquina esta detenida. Solo si `pid-ns` dice que es de este espacio de
    /// pids: sin el no se sabe (una maquina de una version anterior, o una que otra instancia de Flatpak esta arrancando y
    /// cuyo QEMU este proceso no ve), y borrar el pid dejaria a esa maquina sin el; `start` lo limpia de todos modos.
    pub fn pid(&self) -> Option<i32> {
        if self.otro_espacio_de_pids() {
            return None;
        }
        let pid: i32 = std::fs::read_to_string(self.pidfile()).ok()?.trim().parse().ok()?;
        let vivo = match self.inicio_guardado() {
            Some(inicio) => alive_con_inicio(pid, inicio),
            None => alive_de(pid, "qemu"),
        };
        if !vivo {
            if std::path::Path::new(&self.f("pid-ns")).exists() {
                self.olvidar_pid();
            }
            return None;
        }
        Some(pid)
    }
    /// Tiempo de arranque del QEMU de esta maquina guardado por `start` (archivo `pid-start`).
    fn inicio_guardado(&self) -> Option<u64> {
        std::fs::read_to_string(self.f("pid-start")).ok()?.trim().parse().ok()
    }
    /// Guarda junto al pid que QEMU acaba de escribir el tiempo de arranque de ese proceso (`pid-start`): con el, `pid` no
    /// confunde a la maquina con otro proceso que herede el numero. Se llama en `start` en cuanto QEMU escribio su pid.
    pub fn anotar_inicio(&self) -> Result<(), String> {
        let texto = std::fs::read_to_string(self.pidfile()).map_err(|e| txf!("vm.no_se_pudo_leer_el_pid_de_qemu", self.pidfile(), e))?;
        let pid: i32 = texto.trim().parse().map_err(|_| txf!("vm.el_archivo_de_pid_de_qemu_no_contiene_un", format!("{:?}", texto.trim())))?;
        let inicio = inicio_de(pid).ok_or_else(|| txf!("vm.no_se_pudo_leer_proc_stat_del_qemu", pid))?;
        std::fs::write(self.f("pid-start"), format!("{}\n", inicio)).map_err(|e| txf!("vm.no_se_pudo_guardar_el_tiempo_de_arranque", e))
    }
    /// Borra el pid guardado y su tiempo de arranque: el proceso ya no es la maquina.
    fn olvidar_pid(&self) {
        let _ = std::fs::remove_file(self.pidfile());
        let _ = std::fs::remove_file(self.f("pid-start"));
    }
    /// ¿La maquina la arranco un proceso de otro espacio de pids (otra instancia de Flatpak)?
    pub fn otro_espacio_de_pids(&self) -> bool {
        match std::fs::read_to_string(self.f("pid-ns")) {
            Ok(t) => std::fs::read_link("/proc/self/ns/pid").is_ok_and(|l| l.to_string_lossy() != t.trim()),
            Err(_) => false,
        }
    }
    /// Anota el espacio de pids de este proceso junto al pid de QEMU.
    pub fn anotar_espacio_de_pids(&self) {
        if let Ok(l) = std::fs::read_link("/proc/self/ns/pid") {
            let _ = std::fs::write(self.f("pid-ns"), format!("{}\n", l.to_string_lossy()));
        }
    }
    pub fn running(&self) -> bool {
        match self.pid() {
            Some(p) => alive(p),
            // otra instancia de Flatpak la arranco: sin ver el pid, se sabe por el socket de control
            None => self.otro_espacio_de_pids() && std::os::unix::net::UnixStream::connect(self.qmp()).is_ok(),
        }
    }
    /// La carpeta de estado va dentro de las opciones de QEMU (sockets de control y de eventos, consola serie, hvcN, VNC,
    /// virtiofs, initrd con bootconfig): con una coma QEMU partiria la opcion en dos. Se comprueba antes de arrancar.
    pub fn comprobar_ruta(&self) -> Result<(), String> {
        let d = self.dir.to_string_lossy();
        sin_comas(&d, tx!("vm.de_la_carpeta_de_estado")).map_err(|_| txf!("vm.la_carpeta_de_estado_contiene_una_coma", d))
    }
}

/// Maquinas en marcha con CID de vsock: (nombre, CID). Ver `anotados_en_marcha`.
pub fn cids_en_marcha(bases: &[PathBuf], propia: &Path) -> Vec<(String, u32)> {
    anotados_en_marcha(bases, propia, "vsock-cid")
}

/// Maquinas en marcha con adb por TCP: (nombre, puerto de 127.0.0.1). Ver `anotados_en_marcha`.
pub fn puertos_en_marcha(bases: &[PathBuf], propia: &Path) -> Vec<(String, u32)> {
    anotados_en_marcha(bases, propia, "adb-port")
}

/// Primer CID de vsock desde `desde` (3 como minimo: 0-2 estan reservados) que no tiene ninguna maquina en marcha. Pura.
pub fn cid_libre(desde: u32, en_marcha: &[(String, u32)]) -> u32 {
    (desde.max(3)..).find(|c| !en_marcha.iter().any(|(_, o)| o == c)).unwrap_or(u32::MAX)
}

/// Cerrojo de la eleccion de CID de vsock y puerto del adb: lo toma `start` desde que elige hasta que QEMU arranco (y
/// tiene su CID y su puerto), asi dos `start` a la vez no eligen lo mismo. Un `assign.lock` (flock) por carpeta de
/// ejecucion de `bases` que exista, en un orden fijo (sin bloqueos cruzados); lo suelta el sistema al cerrar los archivos o
/// al terminar el proceso. Espera hasta `espera`; despues, Err.
pub fn cerrojo_asignacion(bases: &[PathBuf], espera: std::time::Duration) -> Result<Vec<std::fs::File>, String> {
    use std::os::unix::io::AsRawFd;
    let mut dirs: Vec<PathBuf> = bases.iter().filter_map(|b| std::fs::canonicalize(b).ok()).filter(|b| b.is_dir()).collect();
    dirs.sort();
    dirs.dedup();
    let mut tomados = Vec::new();
    for d in dirs {
        let ruta = d.join("assign.lock");
        let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&ruta).map_err(|e| format!("{}: {}", ruta.display(), e))?;
        let t0 = std::time::Instant::now();
        while unsafe { flock(f.as_raw_fd(), 2 | 4) } != 0 {
            if t0.elapsed() >= espera {
                return Err(txf!("vm.otro_start_lleva_s_eligiendo_cid_de", espera.as_secs(), ruta.display()));
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        tomados.push(f);
    }
    Ok(tomados)
}

extern "C" {
    fn flock(fd: i32, operacion: i32) -> i32;
}

/// Maquinas en marcha con el numero anotado en el archivo `archivo` de su estado: (nombre, numero). Recorre los estados (un
/// directorio por maquina) de las carpetas de ejecucion `bases` sin contar el de `propia` ni repetir una carpeta, y solo
/// cuenta las maquinas cuyo QEMU sigue vivo (`State::running`).
pub fn anotados_en_marcha(bases: &[PathBuf], propia: &Path, archivo: &str) -> Vec<(String, u32)> {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| crate::rutas::normalizar(p));
    let propia = canon(propia);
    let mut vistas: Vec<PathBuf> = Vec::new();
    let mut v = Vec::new();
    for base in bases {
        let b = canon(base);
        if vistas.contains(&b) {
            continue;
        }
        vistas.push(b.clone());
        let Ok(rd) = std::fs::read_dir(&b) else { continue };
        for e in rd.flatten() {
            let dir = e.path();
            if canon(&dir) == propia || !dir.is_dir() {
                continue;
            }
            let Some(cid) = std::fs::read_to_string(dir.join(archivo)).ok().and_then(|t| t.trim().parse().ok()) else { continue };
            let nombre = e.file_name().to_string_lossy().into_owned();
            if (State { dir }).running() {
                v.push((nombre, cid));
            }
        }
    }
    v
}

/// ¿Puede esta maquina arrancar con el CID de vsock `cid`? El CID es unico en todo el equipo: si otra maquina en marcha
/// (`en_marcha`, de `cids_en_marcha`) ya lo tiene, QEMU no arrancaria; Err con un mensaje que dice cual y propone un CID
/// libre o el adb por TCP. Pura.
pub fn comprobar_cid(cid: u32, en_marcha: &[(String, u32)]) -> Result<(), String> {
    let Some((nombre, _)) = en_marcha.iter().find(|(_, c)| *c == cid) else { return Ok(()) };
    let libre = (3..).find(|c| *c != cid && !en_marcha.iter().any(|(_, o)| o == c)).unwrap_or(cid + 1);
    Err(txf!("vm.el_cid_de_vsock_ya_lo_usa_la_maquina_que", cid, format!("{:?}", nombre), libre))
}

/// QEMU separa las propiedades de una opcion con comas (`file=RUTA,if=none,...`): una ruta con coma se partiria. Toda ruta
/// que va dentro de una opcion de QEMU pasa por aqui. `que`: de que ruta se trata ("del disco", "del CD"...).
pub fn sin_comas(ruta: &str, que: &str) -> Result<(), String> {
    if ruta.contains(',') {
        return Err(txf!("vm.la_ruta_no_puede_contener_comas_qemu_las", que, ruta));
    }
    Ok(())
}

/// Ruta absoluta del archivo de `--audio wav:F` SIN crearlo (lo crea QEMU al arrancar; con --dry-run no se toca nada): la
/// carpeta tiene que existir y poder escribirse, y F no puede ser una carpeta. La ruta es absoluta porque QEMU cambia de
/// carpeta al pasar a segundo plano.
pub fn ruta_wav(ruta: &str) -> Result<String, String> {
    let p = Path::new(ruta);
    let nombre = match p.file_name() {
        Some(n) if !ruta.ends_with('/') => n,
        _ => return Err(txf!("vm.audio_wav_falta_el_nombre_del_archivo", format!("{:?}", ruta))),
    };
    let dir = p.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let abs = std::fs::canonicalize(dir).map_err(|e| txf!("vm.audio_wav_la_carpeta_no_existe", dir.display(), e))?;
    if !abs.is_dir() {
        return Err(txf!("vm.audio_wav_no_es_una_carpeta", dir.display()));
    }
    let c = std::ffi::CString::new(abs.to_string_lossy().as_bytes()).map_err(|_| tx!("vm.audio_wav_ruta_no_valida").to_string())?;
    // W_OK | X_OK: crear un archivo dentro
    if unsafe { access(c.as_ptr(), 2 | 1) } != 0 {
        return Err(txf!("vm.audio_wav_no_se_puede_escribir_en_la", abs.display()));
    }
    let f = abs.join(nombre);
    if f.is_dir() {
        return Err(txf!("vm.audio_wav_es_una_carpeta", f.display()));
    }
    Ok(f.to_string_lossy().into_owned())
}

/// Conserva el registro de la ejecucion anterior antes de que un arranque nuevo lo borre o lo vacie: `ruta` pasa a
/// `ruta.1` (pisando el `.1` anterior). Sin `ruta` no hace nada (y el `.1` que hubiera se queda).
pub fn rotar_registro(ruta: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(ruta).is_err() {
        return Ok(());
    }
    let mut viejo = ruta.as_os_str().to_owned();
    viejo.push(".1");
    std::fs::rename(ruta, viejo)
}

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
    fn access(path: *const std::os::raw::c_char, mode: i32) -> i32;
}

pub fn alive(pid: i32) -> bool {
    pid > 1 && unsafe { kill(pid, 0) } == 0
}

/// Tiempo de arranque de un proceso segun el texto de su `/proc/<pid>/stat`: el campo 22 (`starttime`, en tics del reloj
/// desde que arranco el equipo). El campo 2 es el nombre del ejecutable entre parentesis, que puede llevar espacios y
/// parentesis, asi que los campos se cuentan desde el ultimo `)`. Pura.
pub fn inicio_de_stat(texto: &str) -> Option<u64> {
    let resto = &texto[texto.rfind(')')? + 1..];
    // tras el `)` sigue el campo 3 (estado): starttime es el 20.o de los que quedan
    resto.split_whitespace().nth(19)?.parse().ok()
}

/// Tiempo de arranque del proceso `pid` (ver `inicio_de_stat`); None si no existe.
pub fn inicio_de(pid: i32) -> Option<u64> {
    inicio_de_stat(&std::fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?)
}

/// Nombre del ejecutable del proceso `pid` (`/proc/<pid>/comm`, 15 caracteres como mucho); None si no existe.
pub fn comm_de(pid: i32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{}/comm", pid)).ok().map(|c| c.trim().to_string())
}

/// ¿Sigue vivo el proceso `pid` que arranco en `inicio` (su tiempo de arranque, de `inicio_de`)? Otro proceso que herede
/// el numero tiene otro tiempo de arranque, asi que no se confunde con el.
pub fn alive_con_inicio(pid: i32, inicio: u64) -> bool {
    pid > 1 && inicio_de(pid) == Some(inicio)
}

/// ¿Es `pid` un proceso vivo de `programa` (su `/proc/<pid>/comm` lo contiene)? Para los pids guardados sin su tiempo de
/// arranque (ventana, servicios auxiliares, maquinas arrancadas por una version anterior): antes de mandarles una senal.
pub fn alive_de(pid: i32, programa: &str) -> bool {
    pid > 1 && comm_de(pid).is_some_and(|c| c.contains(programa))
}

pub fn signal(pid: i32, sig: i32) -> bool {
    pid > 1 && unsafe { kill(pid, sig) } == 0
}

/// Parametros de arranque de Android segun el modo grafico. Con gfxstream: ANGLE (OpenGL ES) sobre el Vulkan que
/// ejecuta el anfitrion. En el resto de los modos: ANGLE sobre un Vulkan por software dentro de Android.
pub fn gpu_bootconfig(gpu: &str) -> Vec<String> {
    let v: &[&str] = if gpu == "gfxstream" {
        &["androidboot.hardware.egl=angle", "androidboot.hardware.vulkan=ranchu", "androidboot.hardware.gltransport=virtio-gpu-asg", "androidboot.cpuvulkan.version=0"]
    } else {
        &["androidboot.hardware.egl=angle", "androidboot.hardware.vulkan=pastel", "androidboot.cpuvulkan.version=4202496"]
    };
    v.iter().map(|s| s.to_string()).collect()
}

/// Motor grafico que corresponde a una INTENCION: `auto` (el mejor disponible), `hardware` (aceleracion grafica por hardware;
/// hoy gfxstream), `software` (sin aceleracion: virtio-gpu sin aceleracion) o el nombre concreto de un motor (`gfxstream`,
/// `virtio-pci`, `std`...: se respeta tal cual, avanzado). `hay_hardware`: el equipo tiene el motor por hardware (QEMU con el
/// dispositivo y bibliotecas). `motores`: lo que declara el perfil de la imagen (None = sin perfil, no restringe).
/// Devuelve (motor, aviso). Err si se pidio `hardware` y no esta disponible.
pub fn resolver_gpu(intencion: &str, hay_hardware: bool, motores: Option<&[String]>) -> Result<(String, Option<String>), String> {
    let imagen_admite = motores.map_or(true, |m| m.iter().any(|x| x == "hardware"));
    match intencion {
        "hardware" if !hay_hardware => Err(tx!("vm.aceleracion_grafica_por_hardware_no").into()),
        "hardware" if !imagen_admite => Err(tx!("vm.el_perfil_de_esta_imagen_no_admite").into()),
        "hardware" => Ok(("gfxstream".into(), None)),
        "software" => Ok(("virtio-pci".into(), None)),
        "auto" if hay_hardware && imagen_admite => Ok(("gfxstream".into(), None)),
        "auto" if motores.is_some() => {
            let motivo = if !hay_hardware { tx!("vm.el_equipo_no_ofrece_aceleracion_grafica") } else { tx!("vm.el_perfil_de_la_imagen_no_la_admite") };
            Ok(("virtio-pci".into(), Some(txf!("vm.sin_aceleracion_por_hardware_se_dibuja", motivo))))
        }
        // sin perfil (arranque sin imagen): auto cae al dispositivo VGA estandar como siempre
        "auto" => Ok(("std".into(), None)),
        otro => Ok((otro.to_string(), None)),
    }
}

/// Parametros de arranque de Android para una densidad de pantalla (puntos por pulgada).
pub fn density_bootconfig(dpi: u32) -> String {
    format!("androidboot.lcd_density={}", dpi)
}

/// Directorio del QEMU propio (con gfxstream) si existe: `WEFT_QEMU_DIR` o `qemu/` junto al ejecutable.
/// Debe contener bin/qemu-system-x86_64; se usan tambien lib/ y share/qemu.
pub fn bundled_qemu(env_dir: Option<&str>, exe_dir: Option<&str>) -> Option<String> {
    let cands = [env_dir.map(|d| d.to_string()), exe_dir.map(|d| format!("{}/qemu", d))];
    cands.into_iter().flatten().find(|d| std::path::Path::new(&format!("{}/bin/qemu-system-x86_64", d)).is_file())
}

/// Carpeta con las bibliotecas de gfxstream para el QEMU del sistema (rutabaga compilado con gfxstream), si existe:
/// `WEFT_GFX_DIR` o `gfx/` junto al ejecutable. Sirve cuando el QEMU de la distribucion trae el dispositivo
/// virtio-gpu-rutabaga pero su biblioteca rutabaga no incluye gfxstream (caso de Fedora).
pub fn gfx_libs(env_dir: Option<&str>, exe_dir: Option<&str>) -> Option<String> {
    let cands = [env_dir.map(|d| d.to_string()), exe_dir.map(|d| format!("{}/gfx", d))];
    cands.into_iter().flatten().find(|d| std::path::Path::new(&format!("{}/librutabaga_gfx_ffi.so.0", d)).is_file())
}

/// KVM utilizable: el dispositivo existe y este usuario puede abrirlo para lectura y escritura.
pub fn kvm_usable() -> bool {
    unsafe { access(c"/dev/kvm".as_ptr(), 6) == 0 }
}

/// Formato del disco: por la firma del archivo (qcow2 empieza por "QFI\xfb") y, si no se puede leer, por la extension.
fn disk_format(path: &str) -> &'static str {
    use std::io::Read;
    let mut magic = [0u8; 4];
    let sniffed = std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut magic)).is_ok();
    if (sniffed && magic == *b"QFI\xfb") || (!sniffed && path.ends_with(".qcow2")) {
        "qcow2"
    } else {
        "raw"
    }
}

/// Opciones de unidad para un disco qcow2 (los overlays de copia en escritura, cow.rs): cache L2 que cubre el disco entero
/// (con la de QEMU por defecto, 1 MiB, las lecturas dispersas de un disco grande releen tablas L2 del archivo). Vacio para
/// un disco que no es qcow2.
fn opciones_qcow2(path: &str) -> String {
    crate::cow::cabecera(Path::new(path)).map_or(String::new(), |c| format!(",l2-cache-size={}", crate::cow::OPCIONES.cache_l2(c.bytes)))
}

/// Linea de comandos de QEMU. Funcion pura: `kvm` indica si se puede usar la aceleracion.
pub fn qemu_args(c: &Config, st: &State, kvm: bool) -> Result<Vec<String>, String> {
    let use_kvm = match c.accel {
        Accel::Kvm if !kvm => return Err(tx!("vm.se_pidio_kvm_pero_dev_kvm_no_esta").into()),
        Accel::Kvm => true,
        Accel::Tcg => false,
        Accel::Auto => kvm,
    };
    // la carpeta de estado va en varias opciones (sockets, consolas, registros): ver State::comprobar_ruta
    st.comprobar_ruta()?;
    let mut a: Vec<String> = Vec::new();
    let mut p = |xs: &[&str]| a.extend(xs.iter().map(|x| x.to_string()));
    p(&["-nodefaults", "-no-user-config"]);
    let mtype = c.machine.split(',').next().unwrap_or("");
    if mtype != "q35" && mtype != "pc" {
        return Err(txf!("vm.maquina_desconocida_q35_pc", c.machine));
    }
    p(&["-machine", &format!("{},accel={}", c.machine, if use_kvm { "kvm" } else { "tcg" })]);
    p(&["-cpu", if use_kvm { "host" } else { "max" }]);
    p(&["-smp", &c.cpus.max(1).to_string(), "-m", &format!("{}M", c.mem_mb.max(64))]);
    if !c.shares.is_empty() {
        // virtiofs: el servicio de archivos (virtiofsd) accede a la memoria del invitado, que debe ser compartida
        p(&["-object", &format!("memory-backend-memfd,id=mem0,size={}M,share=on", c.mem_mb.max(64)), "-numa", "node,memdev=mem0"]);
    }
    if c.disk_slot < 2 || c.disk_slot as usize + c.disks.len() > 31 {
        return Err(tx!("vm.disk_slot_fuera_de_rango_2_31").into());
    }
    // Todos los dispositivos virtio son solo modernos (disable-legacy): algunos cargadores de arranque (u-boot de
    // Cuttlefish) no traen el controlador heredado y no verian los discos.
    // Discos primero y con direccion PCI fija (ranuras 3, 4, 5... por defecto): Android localiza sus particiones por la ruta
    // PCI del disco (androidboot.boot_devices), asi que no puede depender del orden de los demas dispositivos.
    for (i, (path, ro)) in c.disks.iter().enumerate() {
        sin_comas(path, tx!("vm.del_disco"))?;
        if i >= 16 {
            return Err(tx!("vm.demasiados_discos_maximo_16").into());
        }
        p(&["-drive", &format!("file={},if=none,id=disk{},format={}{}{}", path, i, disk_format(path), opciones_qcow2(path), if *ro { ",readonly=on" } else { "" })]);
        p(&["-device", &format!("virtio-blk-pci,disable-legacy=on,drive=disk{},addr={:#x}", i, c.disk_slot as usize + i)]);
    }
    // control y consola serie: sockets Unix dentro del directorio de estado
    p(&["-qmp", &format!("unix:{},server=on,wait=off", st.qmp())]);
    // segundo canal de control, solo para registrar eventos (apagado, reinicio, panico del invitado...)
    p(&["-qmp", &format!("unix:{},server=on,wait=off", st.f("events.sock"))]);
    p(&["-chardev", &format!("socket,id=ser0,path={},server=on,wait=off,logfile={},logappend=off", st.serial_sock(), st.serial_log())]);
    if c.console_hvc {
        // consola principal en hvc0; mas consolas: -device virtconsole,bus=vser.0,chardev=... (hvc1, hvc2...)
        // el puerto serie queda para el cargador de arranque y los primeros mensajes del kernel
        p(&["-chardev", &format!("file,id=uart0,path={}", st.f("uart.log")), "-serial", "chardev:uart0"]);
        p(&["-device", &format!("virtio-serial-pci,disable-legacy=on,id=vser,max_ports={}", c.hvc_ports.clamp(1, 31)), "-device", "virtconsole,bus=vser.0,chardev=ser0"]);
        for n in 1..=c.hvc_count.min(30) {
            let dev = if let Some((_, path)) = c.hvc_logs.iter().find(|(i, _)| *i == n) {
                sin_comas(path, &txf!("vm.del_registro_de_la_consola_hvc_hvc_log", n))?;
                format!("file,id=hvc{},path={}", n, path)
            } else if c.sensors.is_some_and(|(a, b)| n == a || n == b) {
                format!("socket,id=hvc{},path={},server=on,wait=off", n, st.f(&format!("hvc{}.sock", n)))
            } else {
                format!("null,id=hvc{}", n)
            };
            p(&["-chardev", &dev, "-device", &format!("virtconsole,bus=vser.0,chardev=hvc{}", n)]);
        }
    } else {
        if c.hvc_count != 0 || c.sensors.is_some() {
            return Err(tx!("vm.hvc_count_hvc_log_y_sensors_requieren").into());
        }
        p(&["-serial", "chardev:ser0"]);
    }
    if let Some((a, b)) = c.sensors {
        if a == b || a == 0 || b == 0 || a > c.hvc_count || b > c.hvc_count {
            return Err(tx!("vm.sensors_las_dos_consolas_deben_ser").into());
        }
    }
    p(&["-pidfile", &st.pidfile(), "-daemonize"]);
    // graficos
    let gl = c.gpu == "virtio-gl" || c.gpu == "virtio-gl-pci";
    let res = c.resolution.map(|(w, h)| format!(",xres={},yres={}", w, h)).unwrap_or_default();
    match c.gpu.as_str() {
        // "auto" lo resuelve quien arma la configuracion (gfxstream si hay un QEMU que lo soporte); aqui equivale a std
        "std" | "auto" => p(&["-device", "VGA"]),
        "virtio" => p(&["-device", &format!("virtio-vga{}", res)]),
        "virtio-pci" => p(&["-device", &format!("virtio-gpu-pci{}", res)]),
        "virtio-gl" => p(&["-device", &format!("virtio-vga-gl{}", res)]),
        "virtio-gl-pci" => p(&["-device", &format!("virtio-gpu-gl-pci{}", res)]),
        // gfxstream: Vulkan (y OpenGL por encima) del invitado ejecutados por la GPU del anfitrion. Requiere un QEMU
        // compilado con rutabaga + gfxstream.
        "gfxstream" => {
            let extra = if c.gpu_opts.is_empty() { String::new() } else { format!(",{}", c.gpu_opts) };
            p(&["-device", &format!("virtio-gpu-rutabaga,gfxstream-vulkan=on,hostmem={}M{}{}", c.gpu_hostmem_mb.max(64), extra, res)])
        }
        "none" => {}
        other => return Err(txf!("vm_tec.gpu_desconocida_auto_std_virtio_virtio", other)),
    }
    match c.display.as_str() {
        "none" if gl => p(&["-display", "egl-headless"]),
        "none" => p(&["-display", "none"]),
        // gtk: la ventana escala la imagen; sin esto, el tamano inicial de la ventana (640x480) se le impone al
        // invitado como resolucion de pantalla
        // gtk: la ventana toma el tamano de la pantalla del invitado (sin escalado libre). Si se pidio una
        // resolucion, se anade un VNC local por el que weft se la indica al dispositivo (ver rfb.rs) y la
        // maquina arranca en pausa hasta entonces.
        "gtk" => {
            p(&["-display", &format!("gtk,gl={},zoom-to-fit=off", if gl { "on" } else { "off" })]);
            if c.paused {
                p(&["-vnc", &format!("unix:{}", st.f("vnc.sock")), "-S"]);
            }
        }
        // sdl: con OpenGL la ventana aborta si el invitado entrega la imagen en RGBA (QEMU solo admite BGRA y
        // similares en ese modo), cosa que Android hace al cambiar de modo de composicion: solo si se pide.
        "sdl" => {
            let on = match c.display_gl.as_str() {
                "on" => true,
                "off" => false,
                _ => gl,
            };
            p(&["-display", &format!("sdl,gl={}", if on { "on" } else { "off" })])
        }
        // window: ventana propia de weft (window.rs). QEMU entrega la pantalla por D-Bus a un unico cliente que
        // weft conecta por QMP; sin OpenGL (los cuadros llegan como pixeles)
        "window" if gl => return Err(tx!("vm_tec.display_window_no_admite_gpu_virtio_gl").into()),
        "window" => p(&["-display", "dbus,p2p=yes,gl=off"]),
        other => return Err(txf!("vm.pantalla_desconocida_none_gtk_sdl_window", other)),
    }
    // entrada: puntero absoluto (toques) por USB; el teclado PS/2 lo trae la maquina
    p(&["-device", "qemu-xhci,id=usb0"]);
    match c.pointer.as_str() {
        "tablet" => p(&["-device", "usb-tablet,bus=usb0.0"]),
        "wacom" => p(&["-device", "usb-wacom-tablet,bus=usb0.0,id=ptr0"]),
        // la pantalla tactil virtio se agrega mas abajo; el raton solo la alcanza por la ventana propia (D-Bus) o por
        // las ordenes tap/click (control de QEMU): las ventanas gtk y sdl solo envian posiciones absolutas y botones
        // (QEMU: "Input handler not found for event type abs" sin otro dispositivo absoluto)
        "multitouch" if c.display == "gtk" || c.display == "sdl" => {
            return Err(tx!("vm.pointer_multitouch_no_recibe_el_raton_de").into())
        }
        "multitouch" | "none" => {}
        other => return Err(txf!("vm.puntero_desconocido_multitouch_wacom", other)),
    }
    if c.touch {
        p(&["-device", "virtio-multitouch-pci", "-device", "virtio-keyboard-pci"]);
    } else if c.display == "window" || c.pointer == "multitouch" {
        // la ventana propia hace pellizco (Ctrl+clic) con la pantalla tactil virtio; con --pointer multitouch es
        // ademas el puntero
        p(&["-device", "virtio-multitouch-pci"]);
    }
    // el bus raiz de q35 no admite conexion en caliente: puertos PCIe libres para los mandos (en pc basta el bus PCI)
    if c.gamepad != "none" && mtype == "q35" {
        // por defecto QEMU anuncia el hotplug por ACPI (acpiphp), que el kernel de Cuttlefish no trae; sin esto el
        // firmware tampoco cede el control nativo (pciehp, que si trae) y el invitado nunca ve altas ni bajas
        p(&["-global", "ICH9-LPC.acpi-pci-hotplug-with-bridge-support=off"]);
        for i in 0..crate::gamepad::PUERTOS_RESERVADOS {
            p(&["-device", &format!("pcie-root-port,id={},chassis={},slot={}", crate::gamepad::puerto_id(i), i + 1, i + 1)]);
        }
    }
    for (i, (tag, dir)) in c.shares.iter().enumerate() {
        if tag.is_empty() || !tag.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-') {
            return Err(txf!("vm.carpeta_compartida_no_valida", tag, dir));
        }
        sin_comas(dir, &txf!("vm.de_la_carpeta_compartida", tag))?;
        p(&["-chardev", &format!("socket,id=vfs{},path={}", i, st.f(&format!("virtiofs{}.sock", i))), "-device", &format!("vhost-user-fs-pci,chardev=vfs{},tag={}", i, tag)]);
    }
    match c.audio.split_once(':') {
        None if c.audio == "none" => {}
        Some(("wav", path)) if !path.is_empty() => {
            sin_comas(path, tx!("vm.del_audio_audio_wav"))?;
            p(&["-audiodev", &format!("wav,id=snd0,path={}", path), "-device", "virtio-sound-pci,audiodev=snd0"]);
        }
        // sonido por el servidor de audio del anfitrion
        None if c.audio == "pipewire" || c.audio == "pa" => {
            p(&["-audiodev", &format!("{},id=snd0", c.audio), "-device", "virtio-sound-pci,audiodev=snd0"]);
        }
        _ => return Err(txf!("vm.audio_desconocido_none_pipewire_pa_wav", c.audio)),
    }
    p(&["-device", "virtio-rng-pci,disable-legacy=on"]);
    if let Some(cid) = c.vsock_cid {
        if cid < 3 {
            return Err(tx!("vm.el_identificador_vsock_del_invitado_debe").into());
        }
        p(&["-device", &format!("vhost-vsock-pci,disable-legacy=on,guest-cid={}", cid)]);
    }
    if c.net {
        if c.adb_port.is_some() && c.adb_net >= c.nics.clamp(1, 4) {
            return Err(txf!("vm.el_reenvio_de_adb_va_a_la_tarjeta_de_red", c.adb_net, c.nics.clamp(1, 4)));
        }
        let fwd = |i: u32| c.adb_port.filter(|_| c.adb_net == i).map(|port| format!(",hostfwd=tcp:127.0.0.1:{}-:5555", port)).unwrap_or_default();
        p(&["-netdev", &format!("user,id=n0{}", fwd(0)), "-device", "virtio-net-pci,disable-legacy=on,netdev=n0"]);
        // tarjetas adicionales, cada una en su propia red: Cuttlefish trata la primera como red movil (la gestiona
        // su modem simulado) y la segunda como Ethernet, que es la que obtiene direccion por DHCP
        for i in 1..c.nics.clamp(1, 4) {
            p(&["-netdev", &format!("user,id=n{},net=10.0.{}.0/24{}", i, 2 + i, fwd(i)), "-device", &format!("virtio-net-pci,disable-legacy=on,netdev=n{}", i)]);
        }
    }
    if let Some(iso) = &c.cdrom {
        sin_comas(iso, tx!("vm.del_cd"))?;
        p(&["-drive", &format!("file={},media=cdrom,readonly=on", iso)]);
    }
    if let Some(k) = &c.kernel {
        p(&["-kernel", k]);
        if let Some(i) = &c.initrd {
            p(&["-initrd", i]);
        }
        if !c.append.is_empty() {
            p(&["-append", &c.append]);
        }
    } else if c.initrd.is_some() || !c.append.is_empty() {
        return Err(tx!("vm.initrd_y_append_requieren_kernel").into());
    }
    if c.kernel.is_none() && c.disks.is_empty() && c.cdrom.is_none() && c.extra.is_empty() {
        return Err(tx!("vm.no_hay_nada_que_arrancar_indica_kernel").into());
    }
    a.extend(c.extra.iter().cloned());
    Ok(a)
}

/// QEMU cambia de directorio al pasar a segundo plano: las rutas deben ser absolutas.
pub fn absolute(path: &str) -> Result<String, String> {
    let p = Path::new(path);
    let abs = std::fs::canonicalize(p).map_err(|e| format!("{}: {}", path, e))?;
    Ok(abs.to_string_lossy().into_owned())
}

/// Nombre de tecla QEMU (qcode) de un caracter, y si necesita Mayusculas. Distribucion de teclado US.
pub fn qcode(ch: char) -> Option<(&'static str, bool)> {
    const LOW: &[(char, &str)] = &[
        (' ', "spc"), ('\n', "ret"), ('\t', "tab"), ('-', "minus"), ('=', "equal"), ('[', "bracket_left"), (']', "bracket_right"),
        ('\\', "backslash"), (';', "semicolon"), ('\'', "apostrophe"), ('`', "grave_accent"), (',', "comma"), ('.', "dot"), ('/', "slash"),
    ];
    const UP: &[(char, &str)] = &[
        ('!', "1"), ('@', "2"), ('#', "3"), ('$', "4"), ('%', "5"), ('^', "6"), ('&', "7"), ('*', "8"), ('(', "9"), (')', "0"),
        ('_', "minus"), ('+', "equal"), ('{', "bracket_left"), ('}', "bracket_right"), ('|', "backslash"), (':', "semicolon"),
        ('"', "apostrophe"), ('~', "grave_accent"), ('<', "comma"), ('>', "dot"), ('?', "slash"),
    ];
    const ALNUM: &[&str] = &[
        "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s", "t", "u", "v", "w", "x", "y", "z", "0", "1", "2", "3", "4",
        "5", "6", "7", "8", "9",
    ];
    if ch.is_ascii_lowercase() {
        return Some((ALNUM[(ch as u8 - b'a') as usize], false));
    }
    if ch.is_ascii_uppercase() {
        return Some((ALNUM[(ch as u8 - b'A') as usize], true));
    }
    if ch.is_ascii_digit() {
        return Some((ALNUM[26 + (ch as u8 - b'0') as usize], false));
    }
    if let Some((_, q)) = LOW.iter().find(|(c, _)| *c == ch) {
        return Some((q, false));
    }
    UP.iter().find(|(c, _)| *c == ch).map(|(_, q)| (*q, true))
}

/// Anade una configuracion de arranque (bootconfig del kernel) al final de un initrd. Android 12+ recibe por aqui sus
/// parametros `androidboot.*`. Formato: datos + relleno a 4 bytes + tamano (u32) + suma (u32) + "#BOOTCONFIG\n".
pub fn append_bootconfig(initrd: &[u8], entries: &[String]) -> Vec<u8> {
    let mut data: Vec<u8> = Vec::new();
    for e in entries {
        data.extend_from_slice(e.as_bytes());
        data.push(b'\n');
    }
    data.push(0);
    let mut out = initrd.to_vec();
    while (out.len() + data.len()) % 4 != 0 {
        data.push(0);
    }
    let sum = data.iter().fold(0u32, |a, b| a.wrapping_add(*b as u32));
    out.extend_from_slice(&data);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&sum.to_le_bytes());
    out.extend_from_slice(b"#BOOTCONFIG\n");
    out
}

/// Une parametros de arranque: si una clave se repite, vale la ultima definicion (el kernel rechaza una
/// configuracion con claves repetidas). Se conserva el orden de la primera aparicion.
pub fn bootconfig_merge(entries: &[String]) -> Vec<String> {
    let key = |l: &str| l.split('=').next().unwrap_or("").trim().to_string();
    let mut out: Vec<String> = Vec::new();
    for e in entries {
        match out.iter().position(|o| key(o) == key(e)) {
            Some(i) => out[i] = e.clone(),
            None => out.push(e.clone()),
        }
    }
    out
}

/// Lineas `clave=valor` de un archivo de bootconfig. Se detiene en el primer byte nulo (relleno o cola binaria de
/// una particion de bootconfig) y descarta comentarios y lineas que no sean texto.
pub fn bootconfig_lines(raw: &[u8]) -> Vec<String> {
    let text = &raw[..raw.iter().position(|b| *b == 0).unwrap_or(raw.len())];
    String::from_utf8_lossy(text)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && !l.starts_with('#') && l.contains('=') && l.chars().all(|c| !c.is_control() && c != '\u{fffd}'))
        .collect()
}

/// Eventos de control para un toque en pantalla tactil (virtio-multitouch): contacto en (x, y) relativos 0..1.
/// Devuelve (pulsar, soltar) como listas de eventos `input-send-event`.
pub fn touch_events(x: f64, y: f64) -> (crate::json::V, crate::json::V) {
    use crate::json::V;
    let mtt = |kind: &str, axis: &str, value: i64| {
        V::obj(&[("type", V::s("mtt")), ("data", V::obj(&[("type", V::s(kind)), ("slot", V::n(0)), ("tracking-id", V::n(1)), ("axis", V::s(axis)), ("value", V::n(value))]))])
    };
    let scale = |v: f64| (v.clamp(0.0, 1.0) * 32767.0) as i64;
    // al soltar, el identificador de contacto es -1: QEMU lo entrega tal cual al invitado
    let up = V::obj(&[("type", V::s("mtt")), ("data", V::obj(&[("type", V::s("end")), ("slot", V::n(0)), ("tracking-id", V::n(-1)), ("axis", V::s("x")), ("value", V::n(0))]))]);
    // Android solo considera que hay contacto (y no un dedo "flotando") si ademas llega el boton de toque
    let btn = |down: bool| V::obj(&[("type", V::s("btn")), ("data", V::obj(&[("button", V::s("touch")), ("down", V::Bool(down))]))]);
    (V::Arr(vec![mtt("begin", "x", 0), mtt("data", "x", scale(x)), mtt("data", "y", scale(y)), btn(true)]), V::Arr(vec![btn(false), up]))
}

/// Captura de pantalla por QMP (`screendump`): QEMU escribe el archivo `out` (la carpeta debe existir; PNG si termina en
/// .png, si no PPM). Devuelve la ruta absoluta y la descripcion que imprime la orden `screenshot`.
pub fn captura(qmp_path: &str, out: &str) -> Result<(String, String), String> {
    use crate::json::V;
    // QEMU escribe el archivo: ruta absoluta (el directorio debe existir)
    let p = Path::new(out);
    let dir = p.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let abs = std::fs::canonicalize(dir).map_err(|e| format!("{}: {}", out, e))?.join(p.file_name().ok_or(tx!("vm.nombre_de_archivo_no_valido"))?);
    let abs_s = abs.to_string_lossy().into_owned();
    let _ = std::fs::remove_file(&abs);
    let png = abs_s.ends_with(".png");
    let mut args = vec![("filename", V::s(&abs_s))];
    if png {
        args.push(("format", V::s("png")));
    }
    crate::qmp::Qmp::connect(qmp_path)?.exec("screendump", Some(V::obj(&args)))?;
    let data = std::fs::read(&abs).map_err(|e| txf!("vm.la_captura_no_se_escribio", e))?;
    let desc = if png {
        let (w, h) = png_info(&data)?;
        txf!("vm.captura_x_png", w, h)
    } else {
        let (w, h, colors) = ppm_info(&data)?;
        txf!("vm.captura_x_colores", w, h, colors, if colors >= 256 { "+" } else { "" })
    };
    Ok((abs_s, desc))
}

/// Tamano de una imagen PNG (cabecera IHDR).
pub fn png_info(data: &[u8]) -> Result<(u32, u32), String> {
    if data.len() < 24 || data[0..8] != *b"\x89PNG\r\n\x1a\n" || data[12..16] != *b"IHDR" {
        return Err(tx!("vm.la_captura_no_es_un_png_valido").into());
    }
    let be = |o: usize| u32::from_be_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
    Ok((be(16), be(20)))
}

/// Comprueba una captura PPM (P6) y devuelve (ancho, alto, colores distintos hasta 256).
pub fn ppm_info(data: &[u8]) -> Result<(u32, u32, usize), String> {
    let mut i = 0usize;
    let mut tok = |data: &[u8]| -> Option<String> {
        while i < data.len() && (data[i] as char).is_ascii_whitespace() {
            i += 1;
        }
        let s = i;
        while i < data.len() && !(data[i] as char).is_ascii_whitespace() {
            i += 1;
        }
        if s == i {
            None
        } else {
            Some(String::from_utf8_lossy(&data[s..i]).into_owned())
        }
    };
    let magic = tok(data).ok_or(tx!("vm.captura_vacia"))?;
    if magic != "P6" {
        return Err(txf!("vm.formato_de_captura_inesperado", magic));
    }
    let w: u32 = tok(data).and_then(|t| t.parse().ok()).ok_or(tx!("vm.ancho_no_valido"))?;
    let h: u32 = tok(data).and_then(|t| t.parse().ok()).ok_or(tx!("vm.alto_no_valido"))?;
    let _max = tok(data).ok_or(tx!("vm.cabecera_incompleta"))?;
    let px = &data[(i + 1).min(data.len())..];
    if (px.len() as u64) < w as u64 * h as u64 * 3 {
        return Err(tx!("vm.captura_truncada").into());
    }
    let mut colors = std::collections::BTreeSet::new();
    for c in px.chunks_exact(3) {
        if colors.len() >= 256 {
            break;
        }
        colors.insert([c[0], c[1], c[2]]);
    }
    Ok((w, h, colors.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st() -> State {
        State::new(Some("/run/x"), "vm1").unwrap()
    }

    /// El CID de vsock es unico en el equipo: otra maquina en marcha con el mismo da un error claro que propone uno libre
    /// o el adb por TCP; una detenida o con otro CID no estorba.
    #[test]
    fn conflicto_de_cid() {
        assert_eq!(comprobar_cid(3, &[]), Ok(()));
        assert_eq!(comprobar_cid(3, &[("otra".into(), 4)]), Ok(()));
        let e = comprobar_cid(3, &[("otra".into(), 3)]).unwrap_err();
        assert!(e.contains("CID de vsock 3") && e.contains("\"otra\"") && e.contains("--vsock-cid 4") && e.contains("--adb tcp"), "{}", e);
        // propone el primero libre desde 3
        let e = comprobar_cid(4, &[("a".into(), 3), ("b".into(), 4), ("c".into(), 6)]).unwrap_err();
        assert!(e.contains("\"b\"") && e.contains("--vsock-cid 5"), "{}", e);
    }

    /// Un overlay de copia en escritura va como qcow2 con una cache L2 que cubre todo el disco.
    #[test]
    fn disco_de_copia_en_escritura() {
        let d = std::env::temp_dir().join(format!("weft-vm-cow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let base = d.join("base.img");
        std::fs::File::create(&base).unwrap().set_len(30 << 30).unwrap();
        let ov = d.join("m.img");
        crate::cow::crear(&base, &ov).unwrap();
        let c = Config { disks: vec![(ov.to_string_lossy().into_owned(), false)], ..Config::default() };
        let a = qemu_args(&c, &st(), true).unwrap().join(" ");
        assert!(a.contains(&format!("-drive file={},if=none,id=disk0,format=qcow2,l2-cache-size=4194304 ", ov.display())), "{}", a);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Los estados de las demas maquinas: cuenta la que esta en marcha (aqui, con el pid y el tiempo de arranque de este
    /// mismo proceso) y no la detenida, la propia, la que no tiene CID ni la carpeta repetida.
    #[test]
    fn cids_de_las_maquinas_en_marcha() {
        let base = std::env::temp_dir().join(format!("weft-cids-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let yo = std::process::id() as i32;
        let maquina = |nombre: &str, cid: Option<u32>, viva: bool| {
            let d = base.join(nombre);
            std::fs::create_dir_all(&d).unwrap();
            if let Some(c) = cid {
                std::fs::write(d.join("vsock-cid"), format!("{}\n", c)).unwrap();
            }
            if viva {
                std::fs::write(d.join("pid"), format!("{}\n", yo)).unwrap();
                std::fs::write(d.join("pid-start"), format!("{}\n", inicio_de(yo).unwrap())).unwrap();
            } else {
                std::fs::write(d.join("pid"), format!("{}\n", i32::MAX)).unwrap();
            }
            d
        };
        maquina("viva", Some(3), true);
        maquina("parada", Some(5), false);
        maquina("sin-cid", None, true);
        let propia = maquina("propia", Some(7), true);
        std::fs::write(base.join("suelto"), "3\n").unwrap();
        let v = cids_en_marcha(&[base.clone(), base.join(".")], &propia);
        assert_eq!(v, vec![("viva".to_string(), 3)]);
        assert!(comprobar_cid(3, &v).is_err() && comprobar_cid(5, &v).is_ok() && comprobar_cid(7, &v).is_ok());
        // una carpeta que no existe no es un error
        assert!(cids_en_marcha(&[base.join("nada")], &propia).is_empty());
        // CID automatico: el primero libre (3 lo tiene "viva")
        assert_eq!(cid_libre(3, &v), 4);
        assert_eq!(cid_libre(0, &[]), 3);
        assert_eq!(cid_libre(3, &[("a".into(), 3), ("b".into(), 4), ("c".into(), 6)]), 5);
        // el puerto del adb por TCP se lee igual (adb-port)
        std::fs::write(base.join("viva").join("adb-port"), "15556\n").unwrap();
        std::fs::write(base.join("parada").join("adb-port"), "15557\n").unwrap();
        assert_eq!(puertos_en_marcha(std::slice::from_ref(&base), &propia), vec![("viva".to_string(), 15556)]);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// El cerrojo de asignacion: mientras uno lo tiene, otro no lo consigue (espera y da un error claro); al soltarlo, si.
    /// Una carpeta que no existe no cuenta, y la misma carpeta repetida se toma una vez.
    #[test]
    fn cerrojo_de_asignacion() {
        let base = std::env::temp_dir().join(format!("weft-asigna-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let bases = [base.clone(), base.join("."), base.join("no-existe")];
        let a = cerrojo_asignacion(&bases, std::time::Duration::from_secs(1)).unwrap();
        assert_eq!(a.len(), 1);
        // flock es por descripcion de archivo abierta: otra apertura en el mismo proceso tambien espera
        let t0 = std::time::Instant::now();
        let e = cerrojo_asignacion(&bases, std::time::Duration::from_millis(200)).unwrap_err();
        assert!(t0.elapsed() >= std::time::Duration::from_millis(200) && e.contains("assign.lock"), "{}", e);
        drop(a);
        assert!(cerrojo_asignacion(&bases, std::time::Duration::from_millis(200)).is_ok());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn mandos_y_multitoque() {
        let base = Config { kernel: Some("/k".into()), ..Config::default() };
        // por defecto nada cambia
        let a = qemu_args(&base, &st(), true).unwrap().join(" ");
        assert!(!a.contains("pcie-root-port") && !a.contains("multitouch"));
        // q35 con mandos: puertos PCIe de reserva para la conexion en caliente
        let c = Config { gamepad: "auto".into(), ..base.clone() };
        let a = qemu_args(&c, &st(), true).unwrap().join(" ");
        assert_eq!(a.matches("pcie-root-port").count(), crate::gamepad::PUERTOS_RESERVADOS as usize);
        assert!(a.contains("-global ICH9-LPC.acpi-pci-hotplug-with-bridge-support=off"));
        assert!(a.contains("-device pcie-root-port,id=padrp0,chassis=1,slot=1") && a.contains("id=padrp3,chassis=4,slot=4"));
        // pc: el bus PCI admite conexion en caliente, sin puertos
        let c = Config { gamepad: "/dev/input/event3".into(), machine: "pc".into(), ..base.clone() };
        assert!(!qemu_args(&c, &st(), true).unwrap().join(" ").contains("pcie-root-port"));
        // ventana propia: pantalla tactil virtio para el pellizco, sin teclado virtio ni cambiar el puntero
        let c = Config { display: "window".into(), pointer: "wacom".into(), ..base.clone() };
        let a = qemu_args(&c, &st(), true).unwrap().join(" ");
        assert!(a.contains("-device virtio-multitouch-pci") && !a.contains("virtio-keyboard-pci") && a.contains("usb-wacom-tablet"));
        // con --touch no se duplica
        let c = Config { display: "window".into(), touch: true, ..base };
        assert_eq!(qemu_args(&c, &st(), true).unwrap().join(" ").matches("virtio-multitouch-pci").count(), 1);
    }

    /// --pointer multitouch: la pantalla tactil virtio es el unico puntero (sin tableta USB); con las ventanas gtk/sdl
    /// no sirve y se rechaza.
    #[test]
    fn puntero_multitoque() {
        let base = Config { kernel: Some("/k".into()), pointer: "multitouch".into(), display: "window".into(), ..Config::default() };
        let a = qemu_args(&base, &st(), true).unwrap().join(" ");
        assert!(a.contains("-device virtio-multitouch-pci") && !a.contains("usb-wacom-tablet") && !a.contains("usb-tablet"));
        assert_eq!(a.matches("virtio-multitouch-pci").count(), 1);
        // sin ventana (control por QMP) tambien lleva la pantalla tactil; con --touch no se duplica
        let c = Config { display: "none".into(), ..base.clone() };
        assert!(qemu_args(&c, &st(), true).unwrap().join(" ").contains("virtio-multitouch-pci"));
        let c = Config { touch: true, ..base.clone() };
        let a = qemu_args(&c, &st(), true).unwrap().join(" ");
        assert_eq!(a.matches("virtio-multitouch-pci").count(), 1);
        assert!(a.contains("virtio-keyboard-pci"));
        for d in ["gtk", "sdl"] {
            let c = Config { display: d.into(), ..base.clone() };
            let e = qemu_args(&c, &st(), true).unwrap_err();
            assert!(e.contains("wacom|tablet"), "{}", e);
            // con wacom y tablet esas ventanas siguen funcionando
            for p in ["wacom", "tablet"] {
                assert!(qemu_args(&Config { pointer: p.into(), ..c.clone() }, &st(), true).is_ok());
            }
        }
        assert_eq!(density_bootconfig(320), "androidboot.lcd_density=320");
    }

    #[test]
    fn linea_de_comandos_basica() {
        let c = Config { kernel: Some("/k".into()), initrd: Some("/i".into()), append: "console=ttyS0".into(), ..Config::default() };
        let a = qemu_args(&c, &st(), true).unwrap().join(" ");
        assert!(a.contains("-machine q35,accel=kvm ") && a.contains("-cpu host"));
        assert!(a.contains("-qmp unix:/run/x/vm1/qmp.sock,server=on,wait=off"));
        assert!(a.contains("logfile=/run/x/vm1/serial.log"));
        assert!(a.contains("-display none") && a.contains("-device VGA"));
        assert!(a.contains("-kernel /k -initrd /i -append console=ttyS0"));
        assert!(a.contains("-daemonize") && a.contains("-pidfile /run/x/vm1/pid"));
        // sin KVM y en modo automatico: emulacion por software
        let a = qemu_args(&c, &st(), false).unwrap().join(" ");
        assert!(a.contains("accel=tcg") && a.contains("-cpu max"));
    }

    #[test]
    fn opciones_y_errores() {
        let mut c = Config { disks: vec![("/d/system.img".into(), true), ("/d/data.qcow2".into(), false)], adb_port: Some(5601), ..Config::default() };
        c.gpu = "virtio-gl".into();
        c.display = "gtk".into();
        let a = qemu_args(&c, &st(), true).unwrap().join(" ");
        assert!(a.contains("-drive file=/d/system.img,if=none,id=disk0,format=raw,readonly=on -device virtio-blk-pci,disable-legacy=on,drive=disk0,addr=0x3"));
        assert!(a.contains("-drive file=/d/data.qcow2,if=none,id=disk1,format=qcow2 -device virtio-blk-pci,disable-legacy=on,drive=disk1,addr=0x4"));
        // los discos van antes que cualquier otro dispositivo
        assert!(a.find("virtio-blk-pci").unwrap() < a.find("-qmp").unwrap());
        c.machine = "pc".into();
        assert!(qemu_args(&c, &st(), false).unwrap().join(" ").contains("-machine pc,accel=tcg"));
        c.machine = "pc,nvdimm=on".into();
        c.disk_slot = 4;
        c.resolution = Some((720, 1348));
        c.vsock_cid = Some(3);
        c.console_hvc = true;
        c.gpu = "virtio-pci".into();
        let a2 = qemu_args(&c, &st(), true).unwrap().join(" ");
        assert!(a2.contains("-machine pc,nvdimm=on,accel=kvm"));
        assert!(a2.contains("drive=disk0,addr=0x4") && a2.contains("drive=disk1,addr=0x5"));
        assert!(a2.contains("-device virtio-gpu-pci,xres=720,yres=1348"));
        assert!(a2.contains("-device vhost-vsock-pci,disable-legacy=on,guest-cid=3"));
        assert!(a2.contains("-chardev file,id=uart0,path=/run/x/vm1/uart.log -serial chardev:uart0 -device virtio-serial-pci,disable-legacy=on,id=vser,max_ports=31 -device virtconsole,bus=vser.0,chardev=ser0"));
        c.hvc_count = 3;
        c.hvc_logs = vec![(2, "/l/logcat.txt".into())];
        c.sensors = Some((1, 3));
        let a3 = qemu_args(&c, &st(), true).unwrap().join(" ");
        assert!(a3.contains("-chardev socket,id=hvc1,path=/run/x/vm1/hvc1.sock,server=on,wait=off -device virtconsole,bus=vser.0,chardev=hvc1"));
        assert!(a3.contains("-chardev file,id=hvc2,path=/l/logcat.txt -device virtconsole,bus=vser.0,chardev=hvc2"));
        assert!(a3.contains("-chardev socket,id=hvc3,path=/run/x/vm1/hvc3.sock"));
        c.sensors = Some((1, 9));
        assert!(qemu_args(&c, &st(), true).is_err());
        c.sensors = None;
        c.touch = true;
        c.audio = "wav:/o/s.wav".into();
        let a4 = qemu_args(&c, &st(), true).unwrap().join(" ");
        assert!(a4.contains("-device virtio-multitouch-pci -device virtio-keyboard-pci"));
        assert!(a4.contains("-audiodev wav,id=snd0,path=/o/s.wav -device virtio-sound-pci,audiodev=snd0"));
        c.nics = 2;
        assert!(qemu_args(&c, &st(), true).unwrap().join(" ").contains("-netdev user,id=n1,net=10.0.3.0/24 -device virtio-net-pci,disable-legacy=on,netdev=n1"));
        c.nics = 1;
        // el perfil de Cuttlefish no fija el motor grafico: lo decide el modo de GPU
        let perfil = bootconfig_lines(include_bytes!("../perfiles/cuttlefish.bootconfig"));
        assert!(!perfil.iter().any(|l| l.contains("hardware.vulkan") || l.contains("hardware.egl")));
        let mut todo = gpu_bootconfig("gfxstream");
        todo.extend(perfil.iter().cloned());
        let m = bootconfig_merge(&todo);
        assert!(m.contains(&"androidboot.hardware.vulkan=ranchu".to_string()) && m.contains(&"androidboot.hardware.gltransport=virtio-gpu-asg".to_string()));
        assert!(gpu_bootconfig("virtio-pci").contains(&"androidboot.hardware.vulkan=pastel".to_string()));
        c.pointer = "wacom".into();
        let w = qemu_args(&c, &st(), true).unwrap().join(" ");
        assert!(w.contains("-device usb-wacom-tablet,bus=usb0.0,id=ptr0") && !w.contains("usb-tablet"));
        c.pointer = "raro".into();
        assert!(qemu_args(&c, &st(), true).is_err());
        c.pointer = "tablet".into();
        c.gpu = "gfxstream".into();
        c.display = "sdl".into();
        assert!(qemu_args(&c, &st(), true).unwrap().join(" ").contains("-display sdl,gl=off"));
        c.display_gl = "on".into();
        assert!(qemu_args(&c, &st(), true).unwrap().join(" ").contains("-display sdl,gl=on"));
        c.display = "gtk".into();
        c.display_gl = "auto".into();
        c.paused = true;
        let g = qemu_args(&c, &st(), true).unwrap().join(" ");
        assert!(g.contains("-display gtk,gl=off,zoom-to-fit=off -vnc unix:") && g.contains("vnc.sock -S"));
        c.paused = false;
        assert!(!qemu_args(&c, &st(), true).unwrap().join(" ").contains("-vnc"));
        c.display = "sdl".into();
        c.display_gl = "off".into();
        assert!(qemu_args(&c, &st(), true).unwrap().join(" ").contains("-display sdl,gl=off"));
        c.display_gl = "auto".into();
        c.gpu = "virtio-pci".into();
        assert!(qemu_args(&c, &st(), true).unwrap().join(" ").contains("-display sdl,gl=off"));
        c.display = "none".into();
        // "auto" sin resolver equivale a la tarjeta estandar
        c.gpu = "auto".into();
        assert!(qemu_args(&c, &st(), true).unwrap().join(" ").contains("-device VGA"));
        assert_eq!(bundled_qemu(None, Some("/no/existe")), None);
        let d = std::env::temp_dir().join(format!("weft-qemu-{}", std::process::id()));
        std::fs::create_dir_all(d.join("qemu/bin")).unwrap();
        std::fs::write(d.join("qemu/bin/qemu-system-x86_64"), b"").unwrap();
        let ds = d.to_string_lossy().to_string();
        assert_eq!(bundled_qemu(None, Some(&ds)), Some(format!("{}/qemu", ds)));
        assert_eq!(bundled_qemu(Some(&format!("{}/qemu", ds)), None), Some(format!("{}/qemu", ds)));
        assert_eq!(gfx_libs(None, Some(&ds)), None);
        std::fs::create_dir_all(d.join("gfx")).unwrap();
        std::fs::write(d.join("gfx/librutabaga_gfx_ffi.so.0"), b"").unwrap();
        assert_eq!(gfx_libs(None, Some(&ds)), Some(format!("{}/gfx", ds)));
        let _ = std::fs::remove_dir_all(&d);
        c.gpu = "std".into();
        c.audio = "pipewire".into();
        assert!(qemu_args(&c, &st(), true).unwrap().join(" ").contains("-audiodev pipewire,id=snd0 -device virtio-sound-pci,audiodev=snd0"));
        c.audio = "none".into();
        c.gpu = "gfxstream".into();
        assert!(qemu_args(&c, &st(), true).unwrap().join(" ").contains("-device virtio-gpu-rutabaga,gfxstream-vulkan=on,hostmem=256M,xres=720,yres=1348"));
        c.gpu_opts = "x-gfxstream-composer=on".into();
        assert!(qemu_args(&c, &st(), true).unwrap().join(" ").contains("hostmem=256M,x-gfxstream-composer=on,xres=720"));
        c.gpu_opts.clear();
        c.gpu = "virtio-pci".into();
        c.shares = vec![("shared".into(), "/h/datos".into())];
        let a5 = qemu_args(&c, &st(), true).unwrap().join(" ");
        assert!(a5.contains("-object memory-backend-memfd,id=mem0,size=2048M,share=on -numa node,memdev=mem0"));
        assert!(a5.contains("-chardev socket,id=vfs0,path=/run/x/vm1/virtiofs0.sock -device vhost-user-fs-pci,chardev=vfs0,tag=shared"));
        c.shares = vec![("mala etiqueta".into(), "/h".into())];
        assert!(qemu_args(&c, &st(), true).is_err());
        c.shares.clear();
        c.audio = "mp3".into();
        assert!(qemu_args(&c, &st(), true).is_err());
        c.audio = "none".into();
        c.touch = false;
        c.sensors = None;
        c.hvc_count = 0;
        c.hvc_logs.clear();
        c.vsock_cid = Some(2);
        assert!(qemu_args(&c, &st(), true).is_err());
        c.vsock_cid = None;
        c.console_hvc = false;
        c.gpu = "virtio-gl".into();
        c.resolution = None;
        c.disk_slot = 3;
        c.machine = "q35".into();
        assert!(a.contains("hostfwd=tcp:127.0.0.1:5601-:5555"));
        c.cdrom = Some("/d/a.iso".into());
        assert!(qemu_args(&c, &st(), true).unwrap().join(" ").contains("-drive file=/d/a.iso,media=cdrom,readonly=on"));
        assert!(a.contains("-device virtio-vga-gl") && a.contains("-display gtk,gl=on"));
        c.display = "none".into();
        assert!(qemu_args(&c, &st(), true).unwrap().join(" ").contains("-display egl-headless"));
        c.accel = Accel::Kvm;
        assert!(qemu_args(&c, &st(), false).is_err());
        assert!(qemu_args(&Config::default(), &st(), true).is_err());
        assert!(qemu_args(&Config { append: "x".into(), disks: vec![("/d".into(), false)], ..Config::default() }, &st(), true).is_err());
        assert!(State::new(None, "../x").is_err());
    }

    #[test]
    fn eventos_de_toque() {
        let (down, up) = touch_events(0.5, 1.0);
        let d = down.dump();
        assert!(d.contains(r#""type":"begin""#) && d.contains(r#""axis":"x","slot":0,"tracking-id":1,"type":"data","value":16383"#));
        assert!(d.contains(r#""axis":"y","slot":0,"tracking-id":1,"type":"data","value":32767"#));
        assert!(d.contains(r#"{"button":"touch","down":true}"#));
        assert!(up.dump().contains(r#""tracking-id":-1,"type":"end""#) && up.dump().contains(r#"{"button":"touch","down":false}"#));
    }

    #[test]
    fn claves_repetidas_en_bootconfig() {
        let v: Vec<String> = ["a=1", "b=2", "a = 3", "c=4"].iter().map(|s| s.to_string()).collect();
        assert_eq!(bootconfig_merge(&v), vec!["a = 3", "b=2", "c=4"]);
    }

    #[test]
    fn lineas_de_bootconfig() {
        let mut raw = b"# comentario\na.b=1\n\n  c.d = \"x y\"\nbasura\n".to_vec();
        raw.extend_from_slice(&[0, 0, 0, 0x10, 0, 0, 0, 0xff, 0xfe]);
        raw.extend_from_slice(b"#BOOTCONFIG\nz=9\n");
        assert_eq!(bootconfig_lines(&raw), vec!["a.b=1", "c.d = \"x y\""]);
    }

    #[test]
    fn la_intencion_de_gpu_se_resuelve() {
        let m = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let ambos = m(&["hardware", "software"]);
        let solo_sw = m(&["software"]);
        assert_eq!(resolver_gpu("hardware", true, Some(&ambos)).unwrap(), ("gfxstream".into(), None));
        assert_eq!(resolver_gpu("software", true, Some(&ambos)).unwrap(), ("virtio-pci".into(), None));
        assert_eq!(resolver_gpu("auto", true, Some(&ambos)).unwrap().0, "gfxstream");
        // auto sin hardware o con una imagen que no lo admite: software con aviso
        let (g, a) = resolver_gpu("auto", false, Some(&ambos)).unwrap();
        assert_eq!(g, "virtio-pci");
        assert!(a.unwrap().contains("equipo"));
        let (g, a) = resolver_gpu("auto", true, Some(&solo_sw)).unwrap();
        assert_eq!(g, "virtio-pci");
        assert!(a.unwrap().contains("perfil"));
        // hardware pedido y no disponible: error claro (no cae solo)
        assert!(resolver_gpu("hardware", false, Some(&ambos)).unwrap_err().contains("no disponible"));
        assert!(resolver_gpu("hardware", true, Some(&solo_sw)).unwrap_err().contains("perfil"));
        // los nombres concretos se respetan; sin perfil auto sigue siendo VGA estandar sin hardware
        for v in ["gfxstream", "virtio-pci", "std", "none", "virtio-gl"] {
            assert_eq!(resolver_gpu(v, false, None).unwrap(), (v.to_string(), None));
        }
        assert_eq!(resolver_gpu("auto", false, None).unwrap().0, "std");
        assert_eq!(resolver_gpu("auto", true, None).unwrap().0, "gfxstream");
    }

    #[test]
    fn formato_de_disco_por_firma() {
        let p = std::env::temp_dir().join(format!("weft-fmt-{}.img", std::process::id()));
        std::fs::write(&p, b"QFI\xfb\0\0\0\x03").unwrap();
        assert_eq!(disk_format(p.to_str().unwrap()), "qcow2");
        std::fs::write(&p, b"\0\0\0\0\0\0\0\0").unwrap();
        assert_eq!(disk_format(p.to_str().unwrap()), "raw");
        let _ = std::fs::remove_file(&p);
        assert_eq!(disk_format("/no/existe.qcow2"), "qcow2");
        assert_eq!(disk_format("/no/existe.img"), "raw");
    }

    #[test]
    fn bootconfig_al_final_del_initrd() {
        let out = append_bootconfig(b"RAMDISK", &["androidboot.hardware=ranchu".to_string(), "a.b=\"x y\"".to_string()]);
        assert!(out.starts_with(b"RAMDISK") && out.ends_with(b"#BOOTCONFIG\n"));
        let n = out.len();
        let size = u32::from_le_bytes(out[n - 20..n - 16].try_into().unwrap()) as usize;
        let sum = u32::from_le_bytes(out[n - 16..n - 12].try_into().unwrap());
        let data = &out[n - 20 - size..n - 20];
        assert_eq!(n - 20 - size, 7);
        assert!(data.starts_with(b"androidboot.hardware=ranchu\na.b=\"x y\"\n\0"));
        assert_eq!(sum, data.iter().map(|b| *b as u32).sum::<u32>());
        assert_eq!((7 + size) % 4, 0);
    }

    #[test]
    fn teclas_y_captura() {
        assert_eq!(qcode('a'), Some(("a", false)));
        assert_eq!(qcode('Z'), Some(("z", true)));
        assert_eq!(qcode('7'), Some(("7", false)));
        assert_eq!(qcode('_'), Some(("minus", true)));
        assert_eq!(qcode('\n'), Some(("ret", false)));
        assert_eq!(qcode('ñ'), None);
        let mut ppm = b"P6\n2 2\n255\n".to_vec();
        ppm.extend_from_slice(&[0, 0, 0, 255, 255, 255, 0, 0, 0, 9, 9, 9]);
        assert_eq!(ppm_info(&ppm), Ok((2, 2, 3)));
        assert!(ppm_info(&ppm[..ppm.len() - 2]).is_err());
        assert!(ppm_info(b"P5\n1 1\n255\n\0").is_err());
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&[0, 0, 4, 0, 0, 0, 3, 0]);
        assert_eq!(png_info(&png), Ok((1024, 768)));
        assert!(png_info(b"nada").is_err());
    }

    /// El tiempo de arranque del proceso (campo 22 de /proc/<pid>/stat) distingue a la maquina de otro proceso que herede
    /// su numero de pid.
    #[test]
    fn tiempo_de_arranque_del_proceso() {
        // texto real de /proc/<pid>/stat; el nombre del ejecutable va entre parentesis y puede llevar espacios y parentesis
        let stat = "8626 (cat) R 8625 8626 8625 0 -1 4194304 86 0 0 0 0 0 0 0 20 0 1 0 309445 2924544 364 18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 17 2 0 0 0 0 0";
        assert_eq!(inicio_de_stat(stat), Some(309445));
        let raro = "77 (a b (c) d) S 1 77 77 0 -1 4194560 10 0 0 0 0 0 0 0 20 0 1 0 12345 0 0 0";
        assert_eq!(inicio_de_stat(raro), Some(12345));
        assert_eq!(inicio_de_stat("77 (x) S 1 2"), None);
        assert_eq!(inicio_de_stat("sin parentesis"), None);
        assert_eq!(inicio_de_stat(""), None);
        // comparado con el guardado: solo el mismo valor es el mismo proceso
        let guardado = 309445u64;
        assert!(inicio_de_stat(stat) == Some(guardado) && inicio_de_stat(stat) != Some(guardado + 1));
        // este mismo proceso: vivo con su tiempo de arranque real, no con otro
        let yo = std::process::id() as i32;
        let inicio = inicio_de(yo).expect("/proc/<pid>/stat de este proceso");
        assert!(alive(yo) && alive_con_inicio(yo, inicio));
        assert!(!alive_con_inicio(yo, inicio.wrapping_add(1)));
        assert!(!alive_con_inicio(yo, 0) || inicio == 0);
        // un pid que no existe: ni tiempo de arranque ni nombre
        assert_eq!(inicio_de(i32::MAX), None);
        assert!(!alive_con_inicio(i32::MAX, inicio) && !alive_de(i32::MAX, "weft"));
        assert!(!alive_con_inicio(0, inicio) && !alive_de(1, ""));
        // el nombre del ejecutable: este proceso es el binario de pruebas de weft, no un QEMU
        assert!(comm_de(yo).is_some_and(|c| c.starts_with("weft")));
        assert!(alive_de(yo, "weft") && !alive_de(yo, "qemu"));
    }

    /// `pid` solo da el pid guardado si el proceso sigue siendo el que arranco; si no, la maquina esta detenida y el pid se
    /// olvida.
    #[test]
    fn pid_guardado_de_otro_proceso_no_cuenta() {
        let d = std::env::temp_dir().join(format!("weft-pid-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let st = State { dir: d.clone() };
        let yo = std::process::id() as i32;
        // sin archivo de pid no hay nada que anotar
        assert!(st.anotar_inicio().unwrap_err().contains("pid"), "{:?}", st.anotar_inicio());
        assert_eq!(st.pid(), None);
        // la maquina es de este espacio de pids (lo que `start` anota antes de lanzar QEMU)
        st.anotar_espacio_de_pids();
        assert!(d.join("pid-ns").exists() && !st.otro_espacio_de_pids());
        // con el pid y su tiempo de arranque: es el mismo proceso, la maquina esta en marcha
        std::fs::write(st.pidfile(), format!("{}\n", yo)).unwrap();
        st.anotar_inicio().unwrap();
        assert_eq!(std::fs::read_to_string(st.f("pid-start")).unwrap().trim(), inicio_de(yo).unwrap().to_string());
        assert_eq!(st.pid(), Some(yo));
        assert!(st.running());
        assert!(d.join("pid").exists() && d.join("pid-start").exists());
        // otro tiempo de arranque: el numero lo heredo otro proceso; detenida, y se olvidan pid y pid-start
        std::fs::write(st.f("pid-start"), "1\n").unwrap();
        assert_eq!(st.pid(), None);
        assert!(!st.running());
        assert!(!d.join("pid").exists() && !d.join("pid-start").exists());
        // sin pid-start (maquina arrancada por una version anterior) vale si el proceso es un QEMU: este no lo es
        std::fs::write(st.pidfile(), format!("{}\n", yo)).unwrap();
        assert_eq!(st.pid(), None);
        assert!(!d.join("pid").exists());
        // un pid que no existe
        std::fs::write(st.pidfile(), format!("{}\n", i32::MAX)).unwrap();
        std::fs::write(st.f("pid-start"), "5\n").unwrap();
        assert_eq!(st.pid(), None);
        assert!(!d.join("pid").exists() && !d.join("pid-start").exists());
        // un archivo de pid sin numero: no hay maquina, y anotar lo dice
        std::fs::write(st.pidfile(), "nada\n").unwrap();
        assert_eq!(st.pid(), None);
        assert!(st.anotar_inicio().unwrap_err().contains("no contiene un numero"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Sin `pid-ns` no se sabe de que espacio de pids es el pid guardado (otra instancia de Flatpak que esta arrancando la
    /// maquina, o una version anterior): no cuenta como vivo si este proceso no lo ve, pero tampoco se borra. Y con el
    /// `pid-ns` de otro espacio no se mira el pid.
    #[test]
    fn sin_espacio_de_pids_el_pid_no_se_olvida() {
        let d = std::env::temp_dir().join(format!("weft-pid-ns-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let st = State { dir: d.clone() };
        let yo = std::process::id() as i32;
        // un QEMU que este proceso no ve (aqui, un numero que no existe) con su tiempo de arranque
        std::fs::write(st.pidfile(), format!("{}\n", i32::MAX)).unwrap();
        std::fs::write(st.f("pid-start"), "5\n").unwrap();
        assert_eq!(st.pid(), None);
        assert!(!st.running());
        assert!(d.join("pid").exists() && d.join("pid-start").exists(), "se borro el pid de una maquina de otro espacio de pids");
        // otro tiempo de arranque para un proceso visible: tampoco se borra sin pid-ns
        std::fs::write(st.pidfile(), format!("{}\n", yo)).unwrap();
        assert_eq!(st.pid(), None);
        assert!(d.join("pid").exists());
        // con el pid-ns de otro espacio de pids: el pid no se mira, ni se borra
        std::fs::write(st.f("pid-ns"), "pid:[1]\n").unwrap();
        std::fs::write(st.f("pid-start"), format!("{}\n", inicio_de(yo).unwrap())).unwrap();
        assert!(st.otro_espacio_de_pids());
        assert_eq!(st.pid(), None);
        assert!(d.join("pid").exists() && d.join("pid-start").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Ninguna ruta que va dentro de una opcion de QEMU puede llevar comas: ni las del estado (sockets, consolas, registros),
    /// ni --hvc-log, ni los discos, el CD, las carpetas compartidas o el wav. El error dice cual es y que hacer.
    #[test]
    fn rutas_con_comas() {
        assert!(sin_comas("/a/b", "del disco").is_ok());
        let e = sin_comas("/a,b", "del disco").unwrap_err();
        assert!(e.contains("del disco") && e.contains("/a,b") && e.contains("comas"), "{}", e);
        let base = Config { kernel: Some("/k".into()), ..Config::default() };
        // carpeta de estado con coma: error claro, que dice como evitarlo
        let st_coma = State::new(Some("/run/x,y"), "vm1").unwrap();
        let e = st_coma.comprobar_ruta().unwrap_err();
        assert!(e.contains("/run/x,y/vm1") && e.contains("--state-dir/--root sin comas"), "{}", e);
        assert_eq!(qemu_args(&base, &st_coma, true).unwrap_err(), e);
        assert!(st().comprobar_ruta().is_ok());
        // cada ruta propia
        let hvc = Config { console_hvc: true, hvc_count: 2, hvc_logs: vec![(2, "/l/a,b.txt".into())], ..base.clone() };
        assert!(qemu_args(&hvc, &st(), true).unwrap_err().contains("hvc2"));
        let hvc_ok = Config { hvc_logs: vec![(2, "/l/ab.txt".into())], ..hvc.clone() };
        assert!(qemu_args(&hvc_ok, &st(), true).unwrap().join(" ").contains("-chardev file,id=hvc2,path=/l/ab.txt"));
        assert!(qemu_args(&Config { disks: vec![("/d/a,b.img".into(), false)], ..base.clone() }, &st(), true).unwrap_err().contains("del disco"));
        assert!(qemu_args(&Config { cdrom: Some("/d/a,b.iso".into()), ..base.clone() }, &st(), true).unwrap_err().contains("del CD"));
        assert!(qemu_args(&Config { shares: vec![("s".into(), "/h/a,b".into())], ..base.clone() }, &st(), true).unwrap_err().contains("compartida s"));
        // un wav con coma ya no se confunde con un audio desconocido
        let e = qemu_args(&Config { audio: "wav:/o/a,b.wav".into(), ..base.clone() }, &st(), true).unwrap_err();
        assert!(e.contains("wav") && e.contains("comas") && !e.contains("desconocido"), "{}", e);
        assert!(qemu_args(&Config { audio: "wav:".into(), ..base }, &st(), true).unwrap_err().contains("desconocido"));
    }

    /// `--audio wav:F` no crea ni vacia F (lo hace QEMU al arrancar): solo comprueba la carpeta y da la ruta absoluta.
    #[test]
    fn ruta_del_wav_sin_crearlo() {
        let d = std::env::temp_dir().join(format!("weft-wav-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("sonido.wav");
        let abs = ruta_wav(&f.to_string_lossy()).unwrap();
        assert_eq!(abs, std::fs::canonicalize(&d).unwrap().join("sonido.wav").to_string_lossy());
        assert!(!f.exists(), "no se crea el archivo");
        // un archivo que ya existe no se toca
        std::fs::write(&f, b"datos").unwrap();
        assert_eq!(ruta_wav(&f.to_string_lossy()).unwrap(), abs);
        assert_eq!(std::fs::read(&f).unwrap(), b"datos");
        // carpeta que no existe, una carpeta en vez de un archivo, sin nombre
        assert!(ruta_wav(&d.join("no/existe.wav").to_string_lossy()).unwrap_err().contains("no existe"));
        assert!(ruta_wav(&d.to_string_lossy()).unwrap_err().contains("es una carpeta"));
        assert!(ruta_wav(&format!("{}/", d.display())).unwrap_err().contains("falta el nombre"));
        // relativa: respecto a la carpeta actual, y absoluta al salir
        assert!(ruta_wav("relativo.wav").unwrap().starts_with('/'));
        assert!(!Path::new("relativo.wav").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// El registro de la ejecucion anterior se conserva en `.1` (pisando el `.1` de antes).
    #[test]
    fn rotacion_de_registros() {
        let d = std::env::temp_dir().join(format!("weft-rotar-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let log = d.join("qemu.log");
        let viejo = d.join("qemu.log.1");
        // sin registro no pasa nada
        rotar_registro(&log).unwrap();
        assert!(!viejo.exists());
        std::fs::write(&log, "primera").unwrap();
        rotar_registro(&log).unwrap();
        assert!(!log.exists());
        assert_eq!(std::fs::read_to_string(&viejo).unwrap(), "primera");
        // la siguiente rotacion pisa el .1
        std::fs::write(&log, "segunda").unwrap();
        rotar_registro(&log).unwrap();
        assert_eq!(std::fs::read_to_string(&viejo).unwrap(), "segunda");
        // sin registro nuevo el .1 se queda como estaba
        rotar_registro(&log).unwrap();
        assert_eq!(std::fs::read_to_string(&viejo).unwrap(), "segunda");
        let _ = std::fs::remove_dir_all(&d);
    }
}
