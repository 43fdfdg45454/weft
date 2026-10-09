//! Mandos de juegos del anfitrion hacia el invitado. Cada mando (un /dev/input/eventN con botones de mando y
//! ejes) se entrega a la maquina como dispositivo `virtio-input-host-pci` de QEMU, que lo lee por evdev, lo
//! reserva en exclusiva (EVIOCGRAB) y se lo muestra al kernel del invitado como un dispositivo de entrada mas.
//! La conexion y desconexion en caliente se hacen por QMP (device_add / device_del) y se vigila /dev/input con
//! inotify, sin sondeo.
//!
//! Sin bibliotecas externas: los ioctl de evdev y inotify se declaran a mano (numeros calculados como _IOC).

use crate::json::V;
use crate::qmp::Qmp;
use crate::vm::{self, State};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::time::{Duration, Instant};
use crate::textos::{clave, tx, txf};

extern "C" {
    fn ioctl(fd: i32, req: u64, ...) -> i32;
    fn inotify_init1(flags: i32) -> i32;
    fn inotify_add_watch(fd: i32, path: *const std::os::raw::c_char, mask: u32) -> i32;
    fn poll(fds: *mut PollFd, n: u64, timeout_ms: i32) -> i32;
}

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

const O_NONBLOCK: i32 = 0o4000;
const IN_ATTRIB: u32 = 0x4;
const IN_CREATE: u32 = 0x100;
const IN_DELETE: u32 = 0x200;
const IN_CLOEXEC: i32 = 0o2000000;

/// Numero de ioctl de Linux (_IOC): direccion en los 2 bits altos, tamano, tipo y numero.
const fn ioc(dir: u64, ty: u8, nr: u64, size: u64) -> u64 {
    (dir << 30) | (size << 16) | ((ty as u64) << 8) | nr
}
const IOC_READ: u64 = 2;
const fn eviocgname(len: u64) -> u64 {
    ioc(IOC_READ, b'E', 0x06, len)
}
const fn eviocgbit(ev: u64, len: u64) -> u64 {
    ioc(IOC_READ, b'E', 0x20 + ev, len)
}
const EVIOCGID: u64 = ioc(IOC_READ, b'E', 0x02, 8);

const EV_KEY: usize = 1;
const EV_ABS: usize = 3;
const BTN_JOYSTICK: usize = 0x120;
const BTN_GAMEPAD: usize = 0x130; // BTN_SOUTH
const ABS_X: usize = 0;
const ABS_Y: usize = 1;

/// Cuantos puertos PCIe de reserva se crean en q35 (el bus raiz de q35 no admite conexion en caliente).
pub const PUERTOS_RESERVADOS: u32 = 4;
/// Identificador del puerto de reserva n.
pub fn puerto_id(n: u32) -> String {
    format!("padrp{}", n)
}

/// Capacidades de un dispositivo evdev tal como las informan EVIOCGNAME y EVIOCGBIT.
#[derive(Clone, Debug, Default)]
pub struct Caps {
    pub name: String,
    /// mascara de tipos de evento (EVIOCGBIT(0))
    pub ev: Vec<u8>,
    /// mascara de teclas y botones (EVIOCGBIT(EV_KEY))
    pub key: Vec<u8>,
    /// mascara de ejes absolutos (EVIOCGBIT(EV_ABS))
    pub abs: Vec<u8>,
}

fn bit(v: &[u8], n: usize) -> bool {
    v.get(n / 8).is_some_and(|b| b >> (n % 8) & 1 != 0)
}

/// Es un mando: botones de mando (BTN_GAMEPAD o BTN_JOYSTICK) y los ejes X e Y. Los teclados, ratones, tabletas
/// y paneles tactiles no tienen esos botones; los sensores de movimiento de un mando tienen ejes pero no botones.
pub fn es_mando(c: &Caps) -> bool {
    bit(&c.ev, EV_KEY) && bit(&c.ev, EV_ABS) && (bit(&c.key, BTN_GAMEPAD) || bit(&c.key, BTN_JOYSTICK)) && bit(&c.abs, ABS_X) && bit(&c.abs, ABS_Y)
}

#[derive(Clone, Debug)]
pub struct Pad {
    pub path: String,
    pub name: String,
    pub vendor: u16,
    pub product: u16,
}

/// Numero N de "/dev/input/eventN" (o de "eventN").
pub fn numero_evento(path: &str) -> Option<u32> {
    path.rsplit('/').next()?.strip_prefix("event")?.parse().ok()
}

/// Identificador del dispositivo de QEMU para un evdev.
pub fn id_de(path: &str) -> Option<String> {
    numero_evento(path).map(|n| format!("pad{}", n))
}

fn leer_caps(f: &std::fs::File) -> Option<(Caps, u16, u16)> {
    let fd = f.as_raw_fd();
    let mut name = [0u8; 256];
    let n = unsafe { ioctl(fd, eviocgname(256), name.as_mut_ptr()) };
    if n < 0 {
        return None;
    }
    let name = String::from_utf8_lossy(&name[..(n as usize).min(256)]).trim_end_matches('\0').to_string();
    let mask = |ev: u64, bytes: usize| -> Vec<u8> {
        let mut b = vec![0u8; bytes];
        if unsafe { ioctl(fd, eviocgbit(ev, bytes as u64), b.as_mut_ptr()) } < 0 {
            b.iter_mut().for_each(|x| *x = 0);
        }
        b
    };
    let caps = Caps { name, ev: mask(0, 4), key: mask(EV_KEY as u64, 96), abs: mask(EV_ABS as u64, 8) };
    let mut id = [0u16; 4]; // bustype, vendor, product, version
    unsafe { ioctl(fd, EVIOCGID, id.as_mut_ptr()) };
    Some((caps, id[1], id[2]))
}

/// Mandos presentes en el anfitrion (por orden de numero de evento) y cuantos dispositivos no se pudieron leer.
pub fn enumerar() -> (Vec<Pad>, usize) {
    let mut nums: Vec<u32> = std::fs::read_dir("/dev/input").map(|d| d.flatten().filter_map(|e| numero_evento(&e.file_name().to_string_lossy())).collect()).unwrap_or_default();
    nums.sort();
    let (mut pads, mut ilegibles) = (Vec::new(), 0);
    for n in nums {
        let path = format!("/dev/input/event{}", n);
        let f = match std::fs::OpenOptions::new().read(true).custom_flags(O_NONBLOCK).open(&path) {
            Ok(f) => f,
            Err(_) => {
                ilegibles += 1;
                continue;
            }
        };
        if let Some((c, vendor, product)) = leer_caps(&f) {
            if es_mando(&c) {
                pads.push(Pad { path, name: c.name, vendor, product });
            }
        }
    }
    (pads, ilegibles)
}

/// Resuelve el argumento de attach/detach: ruta, "eventN" o indice dentro de `pads`.
pub fn resolver(arg: &str, pads: &[Pad]) -> Result<String, String> {
    if arg.starts_with("/dev/input/event") && numero_evento(arg).is_some() {
        return Ok(arg.to_string());
    }
    if let Some(n) = numero_evento(arg).filter(|_| arg.starts_with("event")) {
        return Ok(format!("/dev/input/event{}", n));
    }
    let i: usize = arg.parse().map_err(|_| txf!("gamepad.mando_no_valido_ruta_dev_input_eventn", arg))?;
    pads.get(i).map(|p| p.path.clone()).ok_or(txf!("gamepad.no_hay_un_mando_con_el_indice_hay", i, pads.len()))
}

// ---------------------------------------------------------------------------------------------------------------
// QMP

/// Abre el control de la maquina, esperando mientras otro cliente lo usa (QEMU atiende uno a la vez).
fn qmp(st: &State) -> Result<Qmp, String> {
    let t0 = Instant::now();
    loop {
        match Qmp::connect(&st.qmp()) {
            Ok(q) => return Ok(q),
            Err(e) if t0.elapsed() > Duration::from_secs(20) => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

fn perifericos(q: &mut Qmp) -> Result<Vec<String>, String> {
    let r = q.exec("qom-list", Some(V::obj(&[("path", V::s("/machine/peripheral"))])))?;
    Ok(r.as_arr().map(|a| a.iter().filter_map(|e| e.get("name").and_then(|n| n.as_str()).map(|s| s.to_string())).collect()).unwrap_or_default())
}

/// ¿La maquina tiene puertos PCIe de reserva (`padrp0..`) para conectar mandos en caliente? Solo los crea
/// `start --gamepad` en q35.
pub fn tiene_puertos(q: &mut Qmp) -> Result<bool, String> {
    Ok(perifericos(q)?.iter().any(|p| p.starts_with("padrp")))
}

/// Procesos `gamepad-serve` de esta maquina (mismos `--state-dir` y `--name`), mirando /proc.
pub fn servicios_en_marcha(st: &State) -> Vec<i32> {
    let (base, nombre) = (st.dir.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(), st.dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let mut v = Vec::new();
    let Ok(rd) = std::fs::read_dir("/proc") else { return v };
    for e in rd.flatten() {
        let Some(pid) = e.file_name().to_string_lossy().parse::<i32>().ok() else { continue };
        let Ok(raw) = std::fs::read(e.path().join("cmdline")) else { continue };
        let args: Vec<String> = raw.split(|b| *b == 0).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
        if coincide_servicio(&args, &base, &nombre) {
            v.push(pid);
        }
    }
    v
}

/// ¿Es la linea de ordenes de `gamepad-serve` para esa maquina?
pub fn coincide_servicio(args: &[String], base: &str, nombre: &str) -> bool {
    let tras = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1));
    args.iter().any(|a| a == "gamepad-serve") && tras("--name").is_some_and(|n| n == nombre) && tras("--state-dir").is_some_and(|b| b == base)
}

/// Mandos conectados a la maquina: (id, evdev).
pub fn conectados(q: &mut Qmp) -> Result<Vec<(String, String)>, String> {
    let mut v = Vec::new();
    for id in perifericos(q)? {
        if id.starts_with("pad") && !id.starts_with("padrp") {
            let ev = q
                .exec("qom-get", Some(V::obj(&[("path", V::s(&format!("/machine/peripheral/{}", id))), ("property", V::s("evdev"))])))
                .ok()
                .and_then(|r| r.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            v.push((id, ev));
        }
    }
    Ok(v)
}

/// Conecta el evdev a la maquina. En q35 usa un puerto de reserva libre; en pc basta el bus PCI.
pub fn conectar(q: &mut Qmp, path: &str) -> Result<String, String> {
    let id = id_de(path).ok_or(txf!("gamepad.ruta_de_mando_no_valida", path))?;
    let per = perifericos(q)?;
    if per.contains(&id) {
        return Err(txf!("gamepad.ya_esta_conectado", path, id));
    }
    let args = |bus: Option<String>| {
        let mut a = vec![("driver", V::s("virtio-input-host-pci")), ("id", V::s(&id)), ("evdev", V::s(path))];
        if let Some(b) = bus {
            a.push(("bus", V::s(&b)));
        }
        V::obj(&a)
    };
    let puertos: Vec<String> = per.iter().filter(|p| p.starts_with("padrp")).cloned().collect();
    if puertos.is_empty() {
        q.exec("device_add", Some(args(None)))?;
        return Ok(id);
    }
    let mut ultimo = String::new();
    for p in puertos {
        match q.exec("device_add", Some(args(Some(p)))) {
            Ok(_) => return Ok(id),
            Err(e) if e.contains("in use") || e.contains("occupied") || e.contains("not available") || e.contains("full") || e.contains("busy") => ultimo = e,
            Err(e) => return Err(e),
        }
    }
    Err(txf!("gamepad.no_queda_ningun_puerto_libre_para_mandos", ultimo))
}

pub fn desconectar(q: &mut Qmp, id: &str) -> Result<(), String> {
    q.exec("device_del", Some(V::obj(&[("id", V::s(id))]))).map(|_| ())
}

// ---------------------------------------------------------------------------------------------------------------
// servicio

/// Que mandos atiende el servicio: todos los presentes y los que aparezcan, o uno concreto.
#[derive(Clone, Debug, PartialEq)]
pub enum Modo {
    Auto,
    Ruta(String),
}

impl Modo {
    pub fn parse(s: &str) -> Result<Option<Modo>, String> {
        match s {
            "none" => Ok(None),
            "auto" => Ok(Some(Modo::Auto)),
            p if p.starts_with("/dev/input/event") && numero_evento(p).is_some() => Ok(Some(Modo::Ruta(p.to_string()))),
            x => Err(txf!("gamepad.gamepad_no_valido_auto_none_o_dev_input", x)),
        }
    }
}

/// Que hay que hacer para que lo conectado coincida con lo presente: (a conectar, a desconectar).
/// `ignorados`: mandos que el usuario desconecto a mano (no se vuelven a conectar mientras sigan presentes).
pub fn diferencia(modo: &Modo, presentes: &[String], conectados: &[(String, String)], ignorados: &[String]) -> (Vec<String>, Vec<String>) {
    let deseados: Vec<&String> = presentes.iter().filter(|p| match modo {
        Modo::Auto => true,
        Modo::Ruta(r) => *p == r,
    }).collect();
    let alta = deseados.iter().filter(|p| !ignorados.contains(p) && !conectados.iter().any(|(_, e)| e == **p)).map(|p| p.to_string()).collect();
    // un mando que desaparecio del anfitrion: se desconecta (solo los que se conocen por su evdev)
    let baja = conectados.iter().filter(|(_, e)| !e.is_empty() && !presentes.contains(e)).map(|(id, _)| id.clone()).collect();
    (alta, baja)
}

fn ignorados_leer(st: &State) -> Vec<String> {
    std::fs::read_to_string(st.f("gamepad-ignore")).unwrap_or_default().lines().map(|l| l.to_string()).filter(|l| !l.is_empty()).collect()
}

pub fn ignorados_guardar(st: &State, v: &[String]) {
    let _ = std::fs::write(st.f("gamepad-ignore"), v.join("\n"));
}

/// Una pasada: compara el anfitrion con la maquina y concilia.
fn sincronizar(st: &State, modo: &Modo, bajas: &mut std::collections::HashMap<String, Instant>) {
    let (pads, _) = enumerar();
    let presentes: Vec<String> = pads.iter().map(|p| p.path.clone()).collect();
    // los que el usuario desconecto y ya no estan presentes dejan de estar ignorados
    let mut ign = ignorados_leer(st);
    let n = ign.len();
    ign.retain(|p| presentes.contains(p));
    if ign.len() != n {
        ignorados_guardar(st, &ign);
    }
    let mut q = match qmp(st) {
        Ok(q) => q,
        Err(e) => return eprintln!("gamepad: {}", e),
    };
    let con = match conectados(&mut q) {
        Ok(c) => c,
        Err(e) => return eprintln!("gamepad: {}", e),
    };
    let (alta, baja) = diferencia(modo, &presentes, &con, &ign);
    // la baja de un dispositivo PCI termina cuando el invitado lo suelta; mientras tanto no se repite la orden
    bajas.retain(|id, t| t.elapsed() < Duration::from_secs(20) && con.iter().any(|(c, _)| c == id));
    let nuevas: Vec<String> = baja.into_iter().filter(|id| !bajas.contains_key(id)).collect();
    for id in nuevas {
        bajas.insert(id.clone(), Instant::now());
        match desconectar(&mut q, &id) {
            Ok(()) => eprintln!("gamepad: {} desconectado (el mando ya no esta)", id),
            Err(e) => eprintln!("gamepad: {}", e),
        }
    }
    for path in alta {
        // si se acaba de desconectar el mismo mando, el invitado tarda en soltar el dispositivo: reintentos acotados
        let mut intento = 0;
        loop {
            match conectar(&mut q, &path) {
                Ok(id) => {
                    let nombre = pads.iter().find(|p| p.path == path).map_or("", |p| p.name.as_str());
                    eprintln!("gamepad: {} conectado como {} ({})", path, id, nombre);
                    break;
                }
                Err(e) if intento < 10 && (e.contains("duplicate") || e.contains("already") || e.contains(crate::textos::parte_fija(clave!("gamepad.ya_esta_conectado")))) => {
                    intento += 1;
                    std::thread::sleep(Duration::from_millis(500));
                }
                Err(e) => {
                    eprintln!("gamepad: no se pudo conectar {}: {}", path, e);
                    break;
                }
            }
        }
    }
}

/// Servicio de fondo: concilia al empezar y despues de cada cambio en /dev/input. Termina con la maquina.
pub fn servir(st: &State, modo: Modo) -> Result<(), String> {
    let fd = unsafe { inotify_init1(IN_CLOEXEC) };
    if fd < 0 {
        return Err(tx!("gamepad.inotify_no_disponible").into());
    }
    let mut ino = unsafe { std::fs::File::from_raw_fd(fd) };
    // IN_ATTRIB: el nodo aparece solo para root y el sistema le concede el acceso al usuario despues
    if unsafe { inotify_add_watch(fd, c"/dev/input".as_ptr(), IN_CREATE | IN_DELETE | IN_ATTRIB) } < 0 {
        return Err(tx!("gamepad.no_se_pudo_vigilar_dev_input").into());
    }
    // la maquina se vigila por su pid y su tiempo de arranque: si QEMU termina y otro proceso hereda el numero, el
    // servicio no se queda vivo para siempre (como sensors-serve)
    let pid = st.pid().and_then(|p| vm::inicio_de(p).map(|i| (p, i)));
    let mut bajas = std::collections::HashMap::new();
    sincronizar(st, &modo, &mut bajas);
    use std::io::Read;
    let mut buf = [0u8; 4096];
    while pid.is_some_and(|(p, i)| vm::alive_con_inicio(p, i)) {
        let mut p = PollFd { fd, events: 1, revents: 0 };
        if unsafe { poll(&mut p, 1, 5000) } <= 0 {
            continue;
        }
        // varios eventos llegan juntos (nodo creado, permisos...): se espera a que se calme y se concilia una vez
        std::thread::sleep(Duration::from_millis(400));
        loop {
            let mut q = PollFd { fd, events: 1, revents: 0 };
            if unsafe { poll(&mut q, 1, 0) } <= 0 {
                break;
            }
            if ino.read(&mut buf).unwrap_or(0) == 0 {
                break;
            }
        }
        sincronizar(st, &modo, &mut bajas);
    }
    Ok(())
}

/// `gamepad attach`: conecta el mando a la maquina a mano (y deja de ignorarlo en el servicio `auto`). Devuelve el
/// mensaje que imprime la orden. Lo usa tambien la seccion Entrada de la ventana, que recibe la frase para la interfaz
/// (`textos::elige`: sin rutas de dispositivo ni ordenes de terminal).
pub fn conectar_a_mano(st: &State, path: &str) -> Result<String, String> {
    let mut q = Qmp::connect(&st.qmp())?;
    let id = conectar(&mut q, path).map_err(|e| {
        if e.contains("hotplug") || e.contains("hot-plug") {
            txf!("gamepad.con_la_maquina_q35_hay_que_arrancar_con", e)
        } else {
            e
        }
    })?;
    let mut ign = ignorados_leer(st);
    ign.retain(|p| p != path);
    ignorados_guardar(st, &ign);
    Ok(mensaje_conectado(path, &id))
}

/// `gamepad detach`: desconecta el mando (id `padN`, ruta, `eventN` o indice de `pads`) y evita que `auto` lo vuelva a
/// conectar mientras siga presente.
pub fn desconectar_a_mano(st: &State, arg: &str, pads: &[Pad]) -> Result<String, String> {
    let mut q = Qmp::connect(&st.qmp())?;
    let con = conectados(&mut q)?;
    let (id, path) = if let Some(c) = con.iter().find(|(id, _)| id == arg) {
        c.clone()
    } else {
        let path = resolver(arg, pads)?;
        let id = id_de(&path).ok_or(tx!("gamepad.mando_no_valido"))?;
        (id, path)
    };
    desconectar(&mut q, &id)?;
    // un servicio con --gamepad auto no debe volver a conectarlo mientras siga presente
    let mut ign = ignorados_leer(st);
    if !path.is_empty() && !ign.contains(&path) {
        ign.push(path);
    }
    ignorados_guardar(st, &ign);
    Ok(mensaje_desconectado(&id))
}

/// Lo que dice `attach` al conectar `path` como `id`: en la consola, el dispositivo, el id y la orden para soltarlo; en la
/// interfaz, una frase sin rutas ni ordenes (alli hay un boton Desconectar).
fn mensaje_conectado(path: &str, id: &str) -> String {
    crate::textos::elige(
        tx!("gamepad.mando_conectado_a_la_maquina_el_equipo"),
        &txf!("gamepad.conectado_a_la_maquina_como_el_equipo_ya", path, id, id),
    )
}

/// Lo que dice `detach` al soltar el mando `id` (consola o interfaz, como `mensaje_conectado`).
fn mensaje_desconectado(id: &str) -> String {
    crate::textos::elige(tx!("gamepad.mando_desconectado_de_la_maquina_el"), &txf!("gamepad.desconectado_de_la_maquina_el_equipo", id))
}

/// Texto de `gamepad list`.
pub fn listado(pads: &[Pad], conectados: &[(String, String)], ilegibles: usize) -> String {
    let mut s = String::new();
    if pads.is_empty() {
        s.push_str(tx!("gamepad.no_hay_mandos_conectados_al_equipo"));
    }
    for (i, p) in pads.iter().enumerate() {
        let estado = match conectados.iter().find(|(_, e)| *e == p.path) {
            Some((id, _)) => txf!("gamepad.conectado_a_la_maquina_como_el_equipo_no", id),
            None => "libre".to_string(),
        };
        s.push_str(&format!("[{}] {}  {}  ({:04x}:{:04x})  {}\n", i, p.path, p.name, p.vendor, p.product, estado));
    }
    if ilegibles > 0 {
        s.push_str(&txf!("gamepad.dispositivos_de_entrada_sin_permiso_de", ilegibles));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mascara(bits: &[usize], bytes: usize) -> Vec<u8> {
        let mut v = vec![0u8; bytes];
        for b in bits {
            v[b / 8] |= 1 << (b % 8);
        }
        v
    }

    /// La consola conserva el detalle (dispositivo, id y orden); la interfaz recibe una frase con tildes, sin rutas de
    /// dispositivo ni ordenes de terminal.
    #[test]
    fn mensajes_de_conexion_para_consola_e_interfaz() {
        let (c1, c2) = crate::textos::con_modo(false, || (mensaje_conectado("/dev/input/event7", "pad0"), mensaje_desconectado("pad0")));
        assert_eq!(c1, "/dev/input/event7 conectado a la maquina como pad0; el equipo ya no lo ve hasta `gamepad detach pad0`");
        assert_eq!(c2, "pad0 desconectado de la maquina; el equipo recupera el mando");
        let (i1, i2) = crate::textos::con_modo(true, || (mensaje_conectado("/dev/input/event7", "pad0"), mensaje_desconectado("pad0")));
        for t in [&i1, &i2] {
            assert!(t.contains("máquina") && !t.contains("/dev/") && !t.contains("pad0") && !t.contains('`'), "{}", t);
        }
    }

    #[test]
    fn numeros_de_ioctl() {
        // valores de <linux/input.h> en x86-64
        assert_eq!(eviocgname(256), 0x8100_4506);
        assert_eq!(eviocgbit(1, 96), 0x8060_4521);
        assert_eq!(eviocgbit(0, 4), 0x8004_4520);
        assert_eq!(EVIOCGID, 0x8008_4502);
    }

    #[test]
    fn clasificacion() {
        let mando = Caps { name: "Pad".into(), ev: mascara(&[0, 1, 3], 4), key: mascara(&[0x130, 0x131, 0x133], 96), abs: mascara(&[0, 1, 2, 3, 4, 5, 16, 17], 8) };
        assert!(es_mando(&mando));
        // palanca clasica: BTN_JOYSTICK (BTN_TRIGGER)
        let palanca = Caps { key: mascara(&[0x120], 96), ..mando.clone() };
        assert!(es_mando(&palanca));
        // teclado: teclas, sin ejes
        let teclado = Caps { name: "kbd".into(), ev: mascara(&[0, 1, 4, 17], 4), key: mascara(&[1, 30, 57], 96), abs: vec![0; 8] };
        assert!(!es_mando(&teclado));
        // raton con ejes relativos y botones de raton
        let raton = Caps { name: "raton".into(), ev: mascara(&[0, 1, 2], 4), key: mascara(&[0x110, 0x111], 96), abs: vec![0; 8] };
        assert!(!es_mando(&raton));
        // tableta: ejes absolutos, boton de lapiz, sin botones de mando
        let tableta = Caps { name: "tab".into(), ev: mascara(&[0, 1, 3], 4), key: mascara(&[0x140, 0x14a], 96), abs: mascara(&[0, 1, 24], 8) };
        assert!(!es_mando(&tableta));
        // sensores de movimiento de un mando: ejes sin botones
        let mov = Caps { name: "Motion".into(), ev: mascara(&[0, 3], 4), key: vec![0; 96], abs: mascara(&[0, 1, 2, 3, 4, 5], 8) };
        assert!(!es_mando(&mov));
        // mando sin ejes X/Y (solo gatillos): no
        let sinxy = Caps { abs: mascara(&[2, 5], 8), ..mando.clone() };
        assert!(!es_mando(&sinxy));
        // mascaras cortas o vacias no revientan
        assert!(!es_mando(&Caps::default()));
    }

    #[test]
    fn rutas_e_ids() {
        assert_eq!(numero_evento("/dev/input/event12"), Some(12));
        assert_eq!(numero_evento("event3"), Some(3));
        assert_eq!(numero_evento("/dev/input/mouse0"), None);
        assert_eq!(id_de("/dev/input/event7").as_deref(), Some("pad7"));
        let pads = vec![Pad { path: "/dev/input/event5".into(), name: "A".into(), vendor: 1, product: 2 }];
        assert_eq!(resolver("0", &pads).unwrap(), "/dev/input/event5");
        assert_eq!(resolver("event9", &pads).unwrap(), "/dev/input/event9");
        assert_eq!(resolver("/dev/input/event9", &pads).unwrap(), "/dev/input/event9");
        assert!(resolver("3", &pads).is_err());
        assert!(resolver("/etc/passwd", &pads).is_err());
        assert_eq!(puerto_id(1), "padrp1");
    }

    #[test]
    fn linea_del_servicio() {
        let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let ok = l(&["/x/weft", "--state-dir", "./estado", "--name", "prueba", "gamepad-serve", "auto"]);
        assert!(coincide_servicio(&ok, "./estado", "prueba"));
        assert!(!coincide_servicio(&ok, "./estado", "otra"));
        assert!(!coincide_servicio(&ok, "/otro", "prueba"));
        assert!(!coincide_servicio(&l(&["/x/weft", "--state-dir", "./estado", "--name", "prueba", "window-serve", "720", "1348"]), "./estado", "prueba"));
        assert!(!coincide_servicio(&[], "./estado", "prueba"));
    }

    #[test]
    fn modos() {
        assert_eq!(Modo::parse("none").unwrap(), None);
        assert_eq!(Modo::parse("auto").unwrap(), Some(Modo::Auto));
        assert_eq!(Modo::parse("/dev/input/event4").unwrap(), Some(Modo::Ruta("/dev/input/event4".into())));
        assert!(Modo::parse("/dev/sda").is_err());
        assert!(Modo::parse("todos").is_err());
    }

    #[test]
    fn conciliacion() {
        let s = |x: &str| x.to_string();
        let presentes = vec![s("/dev/input/event5"), s("/dev/input/event6")];
        let con = vec![(s("pad5"), s("/dev/input/event5")), (s("pad9"), s("/dev/input/event9"))];
        let (alta, baja) = diferencia(&Modo::Auto, &presentes, &con, &[]);
        assert_eq!((alta, baja), (vec![s("/dev/input/event6")], vec![s("pad9")]));
        // un mando concreto: solo ese
        let (alta, _) = diferencia(&Modo::Ruta(s("/dev/input/event6")), &presentes, &[], &[]);
        assert_eq!(alta, vec![s("/dev/input/event6")]);
        let (alta, _) = diferencia(&Modo::Ruta(s("/dev/input/event8")), &presentes, &[], &[]);
        assert!(alta.is_empty());
        // desconectado a mano: no se vuelve a conectar
        let (alta, _) = diferencia(&Modo::Auto, &presentes, &[], &[s("/dev/input/event6")]);
        assert_eq!(alta, vec![s("/dev/input/event5")]);
        // evdev desconocido (propiedad no leida): no se desconecta
        let (_, baja) = diferencia(&Modo::Auto, &presentes, &[(s("pad1"), s(""))], &[]);
        assert!(baja.is_empty());
    }

    #[test]
    fn texto_del_listado() {
        let pads = vec![Pad { path: "/dev/input/event5".into(), name: "8BitDo".into(), vendor: 0x2dc8, product: 0x310b }, Pad { path: "/dev/input/event6".into(), name: "Otro".into(), vendor: 1, product: 2 }];
        let t = listado(&pads, &[("pad5".into(), "/dev/input/event5".into())], 2);
        assert!(t.contains("[0] /dev/input/event5  8BitDo  (2dc8:310b)  conectado a la maquina como pad5"));
        assert!(t.contains("[1] /dev/input/event6  Otro  (0001:0002)  libre"));
        assert!(t.contains("2 dispositivos"));
    }
}
