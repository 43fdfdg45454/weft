//! Ordenes de weft que usan el adb propio (src/adb.rs): adb-shell, push, pull, install, uninstall, logcat,
//! reboot, root, remount, disable-verity y wait-adb. Todas aceptan `--cid N` antes de sus argumentos; sin ella se usa el
//! CID con que se arranco la maquina (archivo `vsock-cid` del estado) o 3. Y `--timeout S`: segundos sin actividad de
//! adbd tras los que la orden se da por colgada (INACTIVIDAD_S por defecto; 0: sin limite); en `wait-adb`, la espera total.

use crate::adb;
use crate::vm::State;
use std::io::Write;
use crate::textos::{clave, tx, txf};

pub const ORDENES: &[&str] = &["adb-shell", "push", "pull", "install", "uninstall", "logcat", "reboot", "root", "remount", "disable-verity", "wait-adb"];

/// CID del invitado de la maquina del estado (por defecto 3).
pub fn cid_del_estado(st: &State) -> u32 {
    std::fs::read_to_string(st.f("vsock-cid")).ok().and_then(|t| t.trim().parse().ok()).unwrap_or(3)
}

/// Via del adb de la maquina del estado: lo que dejo `start` en `adb-transport` (`vsock`, `tcp` o `both`; con `both` manda vsock,
/// que es mas rapido) y el puerto local de `adb-port`. Sin esos archivos (maquina arrancada de otra forma): vsock.
pub fn via_del_estado(st: &State) -> adb::Via {
    let t = std::fs::read_to_string(st.f("adb-transport")).unwrap_or_default();
    if t.trim() == "tcp" {
        if let Some(p) = std::fs::read_to_string(st.f("adb-port")).ok().and_then(|t| t.trim().parse().ok()) {
            return adb::Via::Tcp(p);
        }
    }
    adb::Via::Vsock
}

/// Puerto TCP local del adb de la maquina (si se arranco con TCP o con ambas vias).
pub fn puerto_tcp_del_estado(st: &State) -> Option<u16> {
    std::fs::read_to_string(st.f("adb-port")).ok().and_then(|t| t.trim().parse().ok())
}

/// Segundos sin actividad de adbd (ninguna lectura ni escritura del socket avanza) tras los que una orden de consola del
/// adb propio se da por colgada: un invitado colgado no deja colgado a `push` o `install` para siempre. `--timeout S` lo
/// cambia y `--timeout 0` lo quita. `logcat` en continuo no lo usa (puede pasar mucho tiempo sin lineas nuevas).
pub const INACTIVIDAD_S: u64 = 120;

/// Opciones de logcat que vuelcan y terminan (o no leen el registro): con ellas logcat no es continuo.
const LOGCAT_TERMINA: &[&str] = &["-d", "-c", "-g", "-t", "--dump", "--clear", "--buffer-size"];

/// Limite de inactividad (segundos de cada lectura y escritura del socket con adbd; 0: sin limite) de la orden `cmd` con
/// sus argumentos `args` y el `--timeout` dado. Sin `--timeout`: INACTIVIDAD_S, salvo `logcat` en continuo (sin limite,
/// que es lo esperado: se corta con Ctrl+C). Pura.
fn limite_inactividad(cmd: &str, args: &[String], timeout: Option<u64>) -> u32 {
    let s = match timeout {
        Some(t) => t,
        None if cmd == "logcat" && !args.iter().any(|a| LOGCAT_TERMINA.contains(&a.as_str())) => 0,
        None => INACTIVIDAD_S,
    };
    s.min(u32::MAX as u64) as u32
}

/// Ordenes cuya conexion con adbd lleva el limite de inactividad (`root`, `wait-adb` y `reboot` a secas tienen sus propias
/// esperas).
const CON_LIMITE: &[&str] = &["adb-shell", "logcat", "push", "pull", "install", "uninstall", "remount", "disable-verity"];

/// Error de una orden con el adb propio, mas claro: si fue por el limite de inactividad, cual era y como cambiarlo; si se
/// corto `logcat` en continuo, que se perdio la conexion. Pura.
fn explicar_error(cmd: &str, e: String, limite: u32) -> String {
    if !CON_LIMITE.contains(&cmd) {
        return e;
    }
    if limite > 0 && e.contains(crate::textos::parte_fija(clave!("adb.adbd_no_respondio_a_tiempo"))) {
        return txf!("adbcmd.sin_actividad_en_s_timeout_s_cambia_el", e, limite);
    }
    if cmd == "logcat" && limite == 0 && (e.contains(crate::textos::parte_fija(clave!("adb.adbd_cerro_la_conexion"))) || e.starts_with("lectura") || e.starts_with("escritura")) {
        return txf!("adbcmd.logcat_se_corto_la_conexion_con_adbd_la", e);
    }
    e
}

/// Opciones iniciales de las ordenes del adb propio: (cid, timeout, via forzada, puerto).
type OpcionesIniciales = (Option<u32>, Option<u64>, Option<String>, Option<u16>);

/// Saca las opciones iniciales `--cid N`, `--timeout S`, `--via vsock|tcp` y `--port P` de los argumentos; el resto queda en
/// `args`. Devuelve (cid, timeout, via forzada, puerto).
fn opciones(args: &mut Vec<String>) -> Result<OpcionesIniciales, String> {
    let (mut cid, mut timeout, mut via, mut puerto) = (None, None, None, None);
    loop {
        match args.first().map(|s| s.as_str()) {
            Some("--cid") => {
                cid = Some(args.get(1).and_then(|v| v.parse().ok()).filter(|c| *c >= 3).ok_or(tx!("adbcmd.cid_se_espera_un_numero_de_3_en_adelante"))?);
                args.drain(..2);
            }
            Some("--via") => {
                via = Some(args.get(1).filter(|v| *v == "vsock" || *v == "tcp").cloned().ok_or(tx!("adbcmd.via_se_espera_vsock_o_tcp"))?);
                args.drain(..2);
            }
            Some("--port") => {
                puerto = Some(args.get(1).and_then(|v| v.parse().ok()).filter(|p| *p >= 1).ok_or(tx!("adbcmd.port_se_espera_un_puerto"))?);
                args.drain(..2);
            }
            Some("--timeout") => {
                timeout = Some(args.get(1).and_then(|v| v.parse().ok()).ok_or(tx!("adbcmd.timeout_se_espera_un_numero_de_segundos"))?);
                args.drain(..2);
            }
            Some("--") => {
                args.remove(0);
                return Ok((cid, timeout, via, puerto));
            }
            _ => return Ok((cid, timeout, via, puerto)),
        }
    }
}

/// Caracteres que el shell de Android interpretaria en una opcion de `install` (la orden viaja como texto a `sh -c`):
/// separadores de ordenes, sustituciones, comillas, redirecciones y espacios o saltos de linea.
const NO_EN_OPCIONES: &[char] = &['\'', '"', '`', ';', '&', '|', '$', '(', ')', '<', '>', ' ', '\t', '\n', '\r', '\\'];

/// Opciones de `install` (se pasan tal cual a `cmd package install`): banderas que empiezan por `-` y, para `--abi`,
/// `--user` e `--install-location`, su valor en la palabra siguiente (p. ej. `--abi arm64-v8a`).
fn opciones_install(args: &[String]) -> Result<(), String> {
    let mal = |a: &str| Err(txf!("adbcmd.install_opcion_no_valida", format!("{:?}", a)));
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if !a.starts_with('-') || a.contains(NO_EN_OPCIONES) {
            return mal(a);
        }
        if matches!(a.as_str(), "--abi" | "--user" | "--install-location") {
            match it.next() {
                Some(v) if !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') => {}
                Some(v) => return mal(v),
                None => return Err(txf!("adbcmd.install_necesita_un_valor", a)),
            }
        }
    }
    Ok(())
}

pub fn run(st: &State, cmd: &str, mut args: Vec<String>) -> Result<i32, String> {
    let (cid, timeout, via, puerto) = opciones(&mut args)?;
    let cid = cid.unwrap_or_else(|| cid_del_estado(st));
    let limite = limite_inactividad(cmd, &args, timeout);
    // --via/--port fuerzan la via de esta orden (p. ej. comprobar el adb por TCP de una maquina que tambien tiene vsock)
    match via.as_deref() {
        Some("tcp") => adb::fijar_via(adb::Via::Tcp(puerto.or_else(|| puerto_tcp_del_estado(st)).ok_or(tx!("adbcmd.via_tcp_la_maquina_no_tiene_un_puerto"))?)),
        Some(_) => adb::fijar_via(adb::Via::Vsock),
        None if puerto.is_some() => adb::fijar_via(adb::Via::Tcp(puerto.unwrap())),
        None => {}
    }
    orden(cmd, args, cid, timeout, limite).map_err(|e| explicar_error(cmd, e, limite))
}

/// La orden `cmd` ya con sus opciones iniciales resueltas; `limite`: segundos sin actividad de adbd (ver limite_inactividad).
fn orden(cmd: &str, mut args: Vec<String>, cid: u32, timeout: Option<u64>, limite: u32) -> Result<i32, String> {
    match cmd {
        "adb-shell" => {
            if args.is_empty() {
                return Err(tx!("adbcmd.adb_shell_necesita_una_orden_no_hay").into());
            }
            let (so, se) = (std::io::stdout(), std::io::stderr());
            adb::conectar(cid, limite)?.shell(&args.join(" "), &mut so.lock(), &mut se.lock())
        }
        "logcat" => {
            let c = if args.is_empty() { "logcat".to_string() } else { format!("logcat {}", args.join(" ")) };
            let (so, se) = (std::io::stdout(), std::io::stderr());
            adb::conectar(cid, limite)?.shell(&c, &mut so.lock(), &mut se.lock())
        }
        "push" => {
            let [src, dst] = &args[..] else { return Err(tx!("adbcmd.uso_push_origen_destino").into()) };
            let t0 = std::time::Instant::now();
            let (dest, n) = adb::conectar(cid, limite)?.push(std::path::Path::new(src), dst)?;
            let s = t0.elapsed().as_secs_f64().max(0.001);
            println!("{}", txf!("adbcmd.bytes_en_s_mb_s", dest, n, format!("{:.2}", s), format!("{:.1}", n as f64 / 1e6 / s)));
            Ok(0)
        }
        "pull" => {
            let (src, dst) = match &args[..] {
                [s] => (s.as_str(), "."),
                [s, d] => (s.as_str(), d.as_str()),
                _ => return Err(tx!("adbcmd.uso_pull_origen_destino").into()),
            };
            let t0 = std::time::Instant::now();
            let (dest, n) = adb::conectar(cid, limite)?.pull(src, std::path::Path::new(dst))?;
            let s = t0.elapsed().as_secs_f64().max(0.001);
            println!("{}", txf!("adbcmd.bytes_en_s_mb_s", dest.display(), n, format!("{:.2}", s), format!("{:.1}", n as f64 / 1e6 / s)));
            Ok(0)
        }
        "install" => {
            // las opciones (-r, -d, -g, -t...) se pasan a `cmd package install`; el ultimo argumento es el APK
            let apk = args.pop().filter(|a| !a.starts_with('-')).ok_or("uso: install [-r] [-d] [-g] [-t] APK")?;
            opciones_install(&args)?;
            let r = adb::conectar(cid, limite)?.install(std::path::Path::new(&apk), &args)?;
            println!("{}", r);
            Ok(if r.contains("Success") { 0 } else { 1 })
        }
        "uninstall" => {
            let (keep, pkg) = match &args[..] {
                [k, p] if k == "-k" => ("-k ", p),
                [p] => ("", p),
                _ => return Err("uso: uninstall [-k] PAQUETE".into()),
            };
            if pkg.is_empty() || !pkg.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_') {
                return Err(txf!("adbcmd.uninstall_nombre_de_paquete_no_valido", format!("{:?}", pkg)));
            }
            let (code, o, e) = adb::conectar(cid, limite)?.shell_texto(&format!("cmd package uninstall {}{}", keep, pkg))?;
            print!("{}", o);
            eprint!("{}", e);
            Ok(if o.contains("Success") { 0 } else { code.max(1) })
        }
        "reboot" => {
            let svc = match args.first() {
                Some(a) if !a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == ',') => return Err(txf!("adbcmd.reboot_argumento_no_valido", format!("{:?}", a))),
                Some(a) => format!("reboot:{}", a),
                None => "reboot:".to_string(),
            };
            if args.is_empty() {
                // reinicio ordenado de Android (el mismo codigo que el boton Reiniciar de la interfaz)
                crate::reinicio::ordenado(cid)?;
            } else {
                let mut c = adb::conectar(cid, limite)?;
                // adbd cierra la conexion al reiniciar: un error de lectura aqui es lo normal
                let _ = c.servicio_texto(&svc);
            }
            println!("{}", tx!("adbcmd.reiniciando"));
            Ok(0)
        }
        "root" => {
            println!("{}", adb::hacer_root(cid)?);
            Ok(0)
        }
        "remount" | "disable-verity" => {
            // como `adb remount`: antes hace falta adbd como root (adb root)
            adb::hacer_root(cid)?;
            let mut c = adb::conectar(cid, limite)?;
            if c.has("remount_shell") {
                let (so, se) = (std::io::stdout(), std::io::stderr());
                let extra = if args.is_empty() { String::new() } else { format!(" {}", args.join(" ")) };
                return c.shell(&format!("{}{}", cmd, extra), &mut so.lock(), &mut se.lock());
            }
            let t = c.servicio_texto(&format!("{}:", cmd))?;
            print!("{}", t);
            let _ = std::io::stdout().flush();
            Ok(if t.to_lowercase().contains("failed") || t.to_lowercase().contains("error") { 1 } else { 0 })
        }
        "wait-adb" => {
            let t = timeout.unwrap_or(180);
            let s = adb::esperar(cid, t, true)?;
            println!("{}", txf!("adbcmd.adbd_responde_y_sys_boot_completed_1", format!("{:.1}", s)));
            Ok(0)
        }
        _ => Err(txf!("adbcmd.orden_desconocida", cmd)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opciones_de_install() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(opciones_install(&v(&["-r", "--abi", "arm64-v8a", "-g"])).is_ok());
        assert!(opciones_install(&v(&[])).is_ok());
        assert!(opciones_install(&v(&["--abi"])).is_err());
        assert!(opciones_install(&v(&["--abi", "a;b"])).is_err());
        assert!(opciones_install(&v(&["-r", "suelta"])).is_err());
        assert!(opciones_install(&v(&["-r;rm"])).is_err());
        // nada que el shell de Android interprete: sustituciones, comillas, redirecciones, espacios ni saltos de linea
        for mala in ["-r$(reboot)", "-r`reboot`", "-r\"x\"", "-r'x'", "-r>/data/x", "-r</x", "-r\nreboot", "-r\treboot", "-r x", "-r(x)", "-r&&x", "-r|x", "-r\\x"] {
            assert!(opciones_install(&v(&[mala])).is_err(), "{:?}", mala);
        }
        // las normales siguen valiendo
        assert!(opciones_install(&v(&["-r", "-d", "-g", "-t", "--user", "0", "--install-location", "1", "--abi", "x86_64"])).is_ok());
    }

    #[test]
    fn opciones_iniciales() {
        let mut a: Vec<String> = ["--cid", "5", "ls", "--cid"].iter().map(|s| s.to_string()).collect();
        assert_eq!(opciones(&mut a), Ok((Some(5), None, None, None)));
        assert_eq!(a, vec!["ls", "--cid"]);
        let mut b: Vec<String> = ["--timeout", "9", "--", "--cid", "x"].iter().map(|s| s.to_string()).collect();
        assert_eq!(opciones(&mut b), Ok((None, Some(9), None, None)));
        assert_eq!(b, vec!["--cid", "x"]);
        let mut c: Vec<String> = ["--cid", "1"].iter().map(|s| s.to_string()).collect();
        assert!(opciones(&mut c).is_err());
        let mut d: Vec<String> = vec![];
        assert_eq!(opciones(&mut d), Ok((None, None, None, None)));
        let mut e: Vec<String> = ["--via", "tcp", "--port", "15555", "ls"].iter().map(|s| s.to_string()).collect();
        assert_eq!(opciones(&mut e), Ok((None, None, Some("tcp".into()), Some(15555))));
        assert_eq!(e, vec!["ls"]);
        assert!(opciones(&mut vec!["--via".to_string(), "x".to_string()]).is_err());
    }

    /// `--timeout S` es global de las ordenes del adb: un numero de segundos (0 vale: sin limite); lo demas es un error.
    #[test]
    fn opcion_timeout() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<String>>();
        let mut a = v(&["--timeout", "0", "--cid", "4", "/sdcard/x", "."]);
        assert_eq!(opciones(&mut a), Ok((Some(4), Some(0), None, None)));
        assert_eq!(a, v(&["/sdcard/x", "."]));
        let mut b = v(&["--timeout", "300", "ls"]);
        assert_eq!(opciones(&mut b), Ok((None, Some(300), None, None)));
        for malo in [&["--timeout"][..], &["--timeout", "x"], &["--timeout", "-1"], &["--timeout", "1.5"], &["--timeout", ""]] {
            let e = opciones(&mut v(malo)).unwrap_err();
            assert!(e.contains("--timeout"), "{:?}: {}", malo, e);
        }
    }

    /// Sin `--timeout` las ordenes de consola tienen INACTIVIDAD_S; logcat en continuo no tiene limite (si vuelca y
    /// termina, si); `--timeout` manda siempre, y 0 lo quita solo si se pide.
    #[test]
    fn limite_de_inactividad() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<String>>();
        for c in ["adb-shell", "push", "pull", "install", "uninstall", "remount"] {
            assert_eq!(limite_inactividad(c, &v(&["x"]), None), INACTIVIDAD_S as u32, "{}", c);
            assert_eq!(limite_inactividad(c, &v(&["x"]), Some(0)), 0, "{}", c);
            assert_eq!(limite_inactividad(c, &v(&["x"]), Some(7)), 7, "{}", c);
        }
        assert_eq!(limite_inactividad("logcat", &[], None), 0);
        assert_eq!(limite_inactividad("logcat", &v(&["-v", "time", "*:E"]), None), 0);
        assert_eq!(limite_inactividad("logcat", &v(&["-d", "-t", "3000"]), None), INACTIVIDAD_S as u32);
        assert_eq!(limite_inactividad("logcat", &v(&["-c"]), None), INACTIVIDAD_S as u32);
        assert_eq!(limite_inactividad("logcat", &[], Some(30)), 30);
        // un numero enorme no da la vuelta
        assert_eq!(limite_inactividad("push", &[], Some(u64::MAX)), u32::MAX);
    }

    #[test]
    fn errores_explicados() {
        let tiempo = "lectura: adbd no respondio a tiempo".to_string();
        let e = explicar_error("push", tiempo.clone(), 120);
        assert!(e.starts_with(&tiempo) && e.contains("120 s") && e.contains("--timeout 0"), "{}", e);
        // las ordenes con su propia espera no llevan la pista
        assert_eq!(explicar_error("wait-adb", tiempo.clone(), 6), tiempo);
        assert_eq!(explicar_error("root", tiempo.clone(), 120), tiempo);
        // logcat en continuo: si muere el socket, se dice
        let e = explicar_error("logcat", "lectura: adbd cerro la conexion".into(), 0);
        assert!(e.starts_with("logcat: se corto la conexion con adbd") && e.contains("adbd cerro la conexion"), "{}", e);
        // otros errores pasan tal cual
        assert_eq!(explicar_error("logcat", "uso: x".into(), 0), "uso: x");
        assert_eq!(explicar_error("push", "uso: push ORIGEN DESTINO".into(), 120), "uso: push ORIGEN DESTINO");
    }

    #[test]
    fn la_via_sale_del_estado_de_la_maquina() {
        let d = std::env::temp_dir().join(format!("ar-via-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let st = State { dir: d.clone() };
        assert_eq!(via_del_estado(&st), adb::Via::Vsock);
        std::fs::write(d.join("adb-transport"), "tcp\n").unwrap();
        assert_eq!(via_del_estado(&st), adb::Via::Vsock, "sin puerto no hay TCP");
        std::fs::write(d.join("adb-port"), "15555\n").unwrap();
        assert_eq!(via_del_estado(&st), adb::Via::Tcp(15555));
        assert_eq!(puerto_tcp_del_estado(&st), Some(15555));
        // con ambas vias manda vsock (mas rapido); el puerto sigue disponible para --via tcp
        std::fs::write(d.join("adb-transport"), "both\n").unwrap();
        assert_eq!(via_del_estado(&st), adb::Via::Vsock);
        assert_eq!(puerto_tcp_del_estado(&st), Some(15555));
        let _ = std::fs::remove_dir_all(&d);
    }
}
