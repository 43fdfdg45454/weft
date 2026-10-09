//! `weft report [--out ARCHIVO]`: reune en un tar.gz lo necesario para diagnosticar un fallo SIN datos personales: la
//! ruta de usuario/home, el nombre de usuario y el nombre del equipo se reemplazan por marcadores (`~`, `usuario`, `equipo`)
//! y se omiten las capturas de pantalla y el contenido de /sdcard. Contenido: `doctor`, `config list`, version del binario
//! y del sistema, propiedades relevantes del invitado, el final de los registros (registro-android, qemu, ventana,
//! sensores, eventos) y, si la maquina esta en marcha, `root status`, `share status` y `bridge status`.
//!
//! Se arma una carpeta temporal dentro del estado y se empaqueta con `tar -czf` del sistema (sin crates). Este modulo no
//! toca SDL; lo usan la orden `report` y el boton "Crear informe" de la pantalla de configuracion.

use crate::vm::State;
use crate::{adb, compartir, config, doctor, puente, root};
use std::path::{Path, PathBuf};
use std::process::Command;
use crate::textos::{tx, txf};

/// Lineas del final de cada registro.
pub const LINEAS_REGISTRO: usize = 200;
const ANCHO_LINEA: usize = 300;

/// Lo que se reemplaza para no revelar datos personales.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Marcadores {
    pub home: Option<String>,
    pub usuario: Option<String>,
    pub equipo: Option<String>,
}

impl Marcadores {
    pub fn del_entorno() -> Marcadores {
        let home = std::env::var("HOME").ok().filter(|h| h.len() > 1);
        let usuario = std::env::var("USER").ok().or_else(|| std::env::var("LOGNAME").ok()).filter(|u| !u.is_empty());
        let equipo = std::fs::read_to_string("/proc/sys/kernel/hostname").ok().map(|h| h.trim().to_string()).or_else(|| std::env::var("HOSTNAME").ok()).filter(|h| !h.is_empty());
        Marcadores { home, usuario, equipo }
    }
}

fn es_palabra(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// Reemplaza `buscado` como palabra completa (sin letras, numeros, `_` ni `-` pegados).
fn reemplazar_palabra(t: &str, buscado: &str, por: &str) -> String {
    if buscado.is_empty() {
        return t.to_string();
    }
    let mut out = String::with_capacity(t.len());
    let mut resto = t;
    while let Some(i) = resto.find(buscado) {
        let antes = resto[..i].chars().last();
        let despues = resto[i + buscado.len()..].chars().next();
        out.push_str(&resto[..i]);
        if antes.is_some_and(es_palabra) || despues.is_some_and(es_palabra) {
            out.push_str(buscado);
        } else {
            out.push_str(por);
        }
        resto = &resto[i + buscado.len()..];
    }
    out.push_str(resto);
    out
}

/// Aplica los marcadores a un texto (y corta las lineas demasiado largas).
pub fn sanear(t: &str, m: &Marcadores) -> String {
    let mut s = t.to_string();
    if let Some(h) = &m.home {
        s = s.replace(h.as_str(), "~");
    }
    // la carpeta personal tambien aparece como /home/NOMBRE o /var/home/NOMBRE aunque HOME sea otra
    if let Some(u) = &m.usuario {
        for pre in ["/var/home/", "/home/"] {
            s = s.replace(&format!("{}{}", pre, u), "~");
        }
        s = reemplazar_palabra(&s, u, "usuario");
    }
    if let Some(e) = &m.equipo {
        s = reemplazar_palabra(&s, e, "equipo");
    }
    let mut r = String::with_capacity(s.len());
    for l in s.lines() {
        if l.chars().count() > ANCHO_LINEA {
            r.extend(l.chars().take(ANCHO_LINEA));
            r.push_str(" [...]");
        } else {
            r.push_str(l);
        }
        r.push('\n');
    }
    r
}

/// Las ultimas `n` lineas de un texto.
pub fn ultimas(t: &str, n: usize) -> String {
    let l: Vec<&str> = t.lines().collect();
    l[l.len().saturating_sub(n)..].join("\n") + "\n"
}

fn leer_cola(p: &Path, n: usize) -> Option<String> {
    let b = std::fs::read(p).ok()?;
    // los registros pueden pesar decenas de MB: solo se mira el final
    let desde = b.len().saturating_sub(512 * 1024);
    Some(ultimas(&String::from_utf8_lossy(&b[desde..]), n))
}

/// Propiedades del invitado que sirven para diagnosticar (ninguna lleva nombres ni numeros de serie).
pub const PROPIEDADES: [&str; 14] = [
    "ro.build.fingerprint",
    "ro.build.version.release",
    "ro.build.version.sdk",
    "ro.product.cpu.abilist",
    "ro.product.cpu.abilist64",
    "ro.dalvik.vm.native.bridge",
    "ro.dalvik.vm.isa.arm64",
    "sys.boot_completed",
    "ro.hardware",
    "ro.hardware.vulkan",
    "ro.hardware.egl",
    "ro.opengles.version",
    "ro.surface_flinger.max_frame_buffer_acquired_buffers",
    "persist.sys.timezone",
];

fn orden_invitado() -> String {
    let mut s = String::from("uname -a; id -u; ");
    for p in PROPIEDADES {
        s.push_str(&format!("echo {p}=$(getprop {p}); ", p = p));
    }
    s
}

/// Nombre por defecto del archivo: `informe-AAAAMMDD-HHMMSS.tar.gz` (UTC).
pub fn nombre_defecto(epoch: u64) -> String {
    let f: String = crate::lanzar::fecha_utc(epoch).chars().filter(char::is_ascii_digit).collect();
    format!("informe-{}-{}.tar.gz", &f[..8], &f[8..])
}

/// Donde se guarda el informe: lo pedido con `--out` (si es relativo, respecto de `actual`, el directorio de trabajo) o,
/// sin `--out`, `informe-AAAAMMDD-HHMMSS.tar.gz` en `carpeta` (la de registros de la maquina). Nunca cae en el directorio
/// de trabajo por defecto: la ventana lanza `report` desde donde se abrio (desde el icono puede ser `/`). Siempre
/// absoluta. Pura.
pub fn destino(out: Option<&Path>, carpeta: &Path, actual: &Path, epoch: u64) -> PathBuf {
    // `join` con una ruta absoluta la deja tal cual
    match out {
        Some(o) => actual.join(o),
        None => actual.join(carpeta).join(nombre_defecto(epoch)),
    }
}

fn ahora() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Secciones del informe: (nombre del archivo, contenido ya saneado). El contenido es para quien arregla el fallo: lleva los
/// nombres tecnicos aunque lo pida la interfaz (el progreso que se ve en pantalla sigue siendo generico).
pub fn reunir(st: &State, cid: u32, m: &Marcadores, prog: &dyn Fn(&str)) -> Vec<(String, String)> {
    crate::textos::con_modo(false, || reunir_tecnico(st, cid, m, prog))
}

fn reunir_tecnico(st: &State, cid: u32, m: &Marcadores, prog: &dyn Fn(&str)) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = Vec::new();
    let mut poner = |n: &str, t: String| v.push((n.to_string(), sanear(&t, m)));
    prog(tx!("informe.version_y_sistema"));
    let uname = Command::new("uname").arg("-sr").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    poner("version.txt", txf!("informe.equipo", crate::version(), uname));
    prog(tx!("informe.doctor"));
    let filas = doctor::ejecutar(st);
    poner("doctor.txt", txf!("informe.codigo_de_salida", doctor::tabla(&filas), doctor::codigo_salida(&filas)));
    prog(tx!("informe.configuracion"));
    let cfg = config::Config::cargar(&st.dir);
    let mut c = cfg.listado();
    for a in &cfg.avisos {
        c.push_str(&txf!("informe.aviso", a));
    }
    poner("config.txt", c);
    let mut estado = txf!("informe.maquina_en_marcha", if st.running() { "si" } else { "no" });
    if let Ok(ev) = std::fs::read_to_string(st.f("events.log")) {
        estado.push_str(&txf!("informe.eventos", ultimas(&ev, 12)));
    }
    poner("estado.txt", estado);
    prog(tx!("informe.registros"));
    let ruta_android = std::fs::read_to_string(st.f("android-log.path"))
        .ok()
        .map(|t| PathBuf::from(t.trim()))
        .filter(|p| p.exists())
        .unwrap_or_else(|| crate::rutas::actual().registro_android(&crate::rutas::maquina_de(&st.dir)));
    let registros: Vec<(&str, PathBuf)> = vec![
        ("android-log.txt", ruta_android),
        ("qemu.log", st.dir.join("qemu.log")),
        ("window.log", st.dir.join("window.log")),
        ("sensors.log", st.dir.join("sensors.log")),
        ("events.log", st.dir.join("events.log")),
    ];
    for (n, p) in registros {
        match leer_cola(&p, LINEAS_REGISTRO) {
            Some(t) => poner(n, txf!("informe.ultimas_lineas", LINEAS_REGISTRO, t)),
            None => poner(n, tx!("informe.no_hay_este_registro").into()),
        }
    }
    if st.running() {
        prog(tx!("informe.invitado_adb"));
        let guest = |cmd: &str| adb::conectar(cid, 8).and_then(|mut a| a.shell_texto(cmd)).map(|(_, o, e)| format!("{}{}", o, e));
        poner("invitado.txt", guest(&orden_invitado()).unwrap_or_else(|e| txf!("informe.no_disponible", e)));
        prog(tx!("informe.root_carpetas_y_traductor"));
        poner("root-status.txt", root::proveedor().estado(cid).map(|e| e.resumen() + "\n").unwrap_or_else(|e| txf!("informe.no_disponible", e)));
        let l = compartir::lista(&cfg);
        let mut s = compartir::listado(&l);
        match compartir::rc_instalado(cid) {
            Ok(i) => s.push_str(&txf!("informe.servicio_de_montaje_en_el_invitado", if i { tx!("root.instalado") } else { tx!("informe.no_instalado") })),
            Err(e) => s.push_str(&txf!("informe.servicio_de_montaje_no_disponible", e)),
        }
        match compartir::estado_invitado(cid, &l) {
            Ok(t) => s.push_str(&t),
            Err(e) => s.push_str(&txf!("informe.estado_en_el_invitado_no_disponible", e)),
        }
        poner("share-status.txt", s);
        poner("bridge-status.txt", puente::estado(cid).map(|e| e.resumen()).unwrap_or_else(|e| txf!("informe.no_disponible", e)));
    } else {
        poner("invitado.txt", tx!("informe.la_maquina_no_esta_en_marcha_sin_datos").into());
    }
    v.push(("LEEME.txt".into(), tx!("informe.informe_de_se_reemplazaron_la_carpeta").into()));
    v
}

/// Arma y empaqueta el informe (en `out` o, sin el, en la carpeta de registros de la maquina: ver `destino`). Devuelve
/// la ruta absoluta del archivo.
pub fn crear(st: &State, cid: u32, out: Option<&Path>, m: &Marcadores, prog: &dyn Fn(&str)) -> Result<PathBuf, String> {
    let secciones = reunir(st, cid, m, prog);
    let carpeta = crate::rutas::actual().registros_maquina(&crate::rutas::maquina_de(&st.dir));
    let actual = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let ruta = destino(out, &carpeta, &actual, ahora());
    if out.is_none() {
        // la carpeta de registros de la maquina puede no existir todavia (maquina que nunca arranco)
        std::fs::create_dir_all(&carpeta).map_err(|e| format!("{}: {}", carpeta.display(), e))?;
    }
    empaquetar(&st.dir, &secciones, &ruta)
}

/// Escribe las secciones en una carpeta temporal del estado y las empaqueta con `tar -czf` en `destino`. Devuelve la ruta
/// absoluta del archivo.
pub fn empaquetar(estado: &Path, secciones: &[(String, String)], destino: &Path) -> Result<PathBuf, String> {
    let base = estado.join("report-tmp");
    let _ = std::fs::remove_dir_all(&base);
    let carpeta = base.join("weft-informe");
    std::fs::create_dir_all(&carpeta).map_err(|e| format!("{}: {}", carpeta.display(), e))?;
    for (n, t) in secciones {
        std::fs::write(carpeta.join(n), t).map_err(|e| format!("{}: {}", n, e))?;
    }
    let st = Command::new("tar").arg("-czf").arg(destino).arg("-C").arg(&base).arg("weft-informe").output().map_err(|e| txf!("informe.no_se_pudo_ejecutar_tar", e))?;
    let _ = std::fs::remove_dir_all(&base);
    if !st.status.success() {
        return Err(txf!("informe.tar_fallo", String::from_utf8_lossy(&st.stderr).trim()));
    }
    // `destino` ya es absoluta cuando la calcula `destino()`; canonicalize solo quita enlaces y `..`
    Ok(std::fs::canonicalize(destino).unwrap_or_else(|_| destino.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m() -> Marcadores {
        Marcadores { home: Some("/var/home/maria".into()), usuario: Some("maria".into()), equipo: Some("portatil-de-maria".into()) }
    }

    #[test]
    fn marcadores_sin_datos_personales() {
        let t = "ruta /var/home/maria/android/imagen\nmontado en /run/media/maria/DISCO/x\nusuario maria en portatil-de-maria\nmariana no es el usuario\nHOME=/home/maria/.config\n";
        let s = sanear(t, &m());
        assert!(!s.contains("maria") || s.contains("mariana"), "{}", s);
        assert!(s.contains("~/android/imagen") && s.contains("/run/media/usuario/DISCO/x") && s.contains("usuario usuario en equipo"));
        assert!(s.contains("mariana no es el usuario"), "una palabra que solo contiene el nombre no se toca");
        assert!(s.contains("HOME=~/.config"));
        assert!(!s.contains("portatil-de-maria"));
        // sin datos, no cambia nada
        assert_eq!(sanear("a b\n", &Marcadores::default()), "a b\n");
        // lineas muy largas se cortan
        let largo = "x".repeat(1000);
        assert!(sanear(&largo, &Marcadores::default()).len() < 320);
    }

    #[test]
    fn ultimas_lineas() {
        assert_eq!(ultimas("a\nb\nc\nd\n", 2), "c\nd\n");
        assert_eq!(ultimas("a\n", 5), "a\n");
    }

    #[test]
    fn nombre_del_archivo() {
        assert_eq!(nombre_defecto(1_790_000_000), "informe-20260921-141320.tar.gz");
        assert_eq!(nombre_defecto(0), "informe-19700101-000000.tar.gz");
    }

    #[test]
    fn destino_por_defecto_en_la_carpeta_de_registros_y_absoluto() {
        let carpeta = Path::new("/home/maria/.local/state/weft/machines/default");
        let actual = Path::new("/");
        // sin --out: nunca en el directorio de trabajo (la ventana abierta desde el icono puede estar en /)
        assert_eq!(destino(None, carpeta, actual, 1_790_000_000), PathBuf::from("/home/maria/.local/state/weft/machines/default/informe-20260921-141320.tar.gz"));
        // carpeta relativa (sin HOME ni XDG las carpetas son relativas al directorio actual): se hace absoluta
        let r = destino(None, Path::new("weft-logs/machines/default"), Path::new("/trabajo"), 0);
        assert_eq!(r, PathBuf::from("/trabajo/weft-logs/machines/default/informe-19700101-000000.tar.gz"));
        // --out sigue valiendo: absoluto tal cual, relativo respecto del directorio de trabajo
        assert_eq!(destino(Some(Path::new("/tmp/x.tar.gz")), carpeta, actual, 0), PathBuf::from("/tmp/x.tar.gz"));
        assert_eq!(destino(Some(Path::new("sub/x.tar.gz")), carpeta, Path::new("/trabajo"), 0), PathBuf::from("/trabajo/sub/x.tar.gz"));
        for out in [None, Some(Path::new("x.tar.gz"))] {
            assert!(destino(out, Path::new("rel"), Path::new("/trabajo"), 5).is_absolute());
        }
    }

    #[test]
    fn propiedades_sin_identificadores() {
        for p in PROPIEDADES {
            assert!(!p.contains("serial") && !p.contains("name") && !p.contains("mac") && !p.contains("imei"), "{}", p);
        }
        let o = orden_invitado();
        assert!(o.contains("uname -a") && o.contains("ro.dalvik.vm.native.bridge"));
    }

    #[test]
    fn el_informe_se_empaqueta_y_no_lleva_nombres() {
        let d = std::env::temp_dir().join(format!("ar-informe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("qemu.log"), "error en /var/home/maria/weft-data en portatil-de-maria\n".repeat(3)).unwrap();
        let st = State { dir: d.clone() };
        let sec = reunir(&st, 3, &m(), &|_| {});
        let nombres: Vec<&str> = sec.iter().map(|s| s.0.as_str()).collect();
        for n in ["version.txt", "doctor.txt", "config.txt", "estado.txt", "qemu.log", "window.log", "sensors.log", "events.log", "android-log.txt", "invitado.txt", "LEEME.txt"] {
            assert!(nombres.contains(&n), "falta {}: {:?}", n, nombres);
        }
        assert!(!nombres.iter().any(|n| n.ends_with(".png") || n.ends_with(".ppm")));
        assert!(sec.iter().all(|(_, t)| !t.contains("maria")), "quedaron datos personales");
        let q = &sec.iter().find(|s| s.0 == "qemu.log").unwrap().1;
        assert!(q.contains("~/weft-data en equipo"), "{}", q);
        let out = d.join("salida.tar.gz");
        let ruta = empaquetar(&d, &sec, &out).unwrap();
        assert!(ruta.exists());
        let l = Command::new("tar").arg("-tzf").arg(&ruta).output().unwrap();
        let l = String::from_utf8_lossy(&l.stdout).to_string();
        assert!(l.contains("weft-informe/doctor.txt") && l.contains("weft-informe/LEEME.txt"), "{}", l);
        assert!(!d.join("report-tmp").exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
