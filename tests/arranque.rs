//! Arranques que fallan a medias, sin QEMU real: con el binario de weft, una carpeta de estado temporal (modo de estado propio:
//! todo dentro de ella) y programas falsos en lugar de QEMU y de virtiofsd. Comprueba que un fallo no deja procesos ni el
//! pid de una maquina que no existe, que --dry-run no crea nada y que las rutas con coma se rechazan con un error claro.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

/// Carpeta temporal propia de cada prueba.
fn carpeta(nombre: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("weft-arranque-{}-{}", nombre, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn weft(estado: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_weft"));
    c.args(["--state-dir", &estado.to_string_lossy(), "--name", "m"]).args(args);
    // las carpetas salen solo de --state-dir (modo de estado propio): nada del entorno de quien ejecuta las pruebas
    for v in ["WEFT_ROOT", "WEFT_DATA_DIR", "WEFT_CONFIG_DIR", "WEFT_CACHE_DIR", "WEFT_LOGS_DIR", "WEFT_STATE_DIR", "WEFT_QEMU_DIR", "WEFT_GFX_DIR"] {
        c.env_remove(v);
    }
    c
}

fn texto(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

fn guion(ruta: &Path, cuerpo: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(ruta, format!("#!/bin/sh\n{}", cuerpo)).unwrap();
    std::fs::set_permissions(ruta, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// virtiofsd falso: crea su socket, anota su pid junto al guion y espera; con SIGTERM termina (y su hijo con el). Su nombre
/// de proceso es "virtiofsd", como el de verdad (weft solo senala a los procesos que lo son).
fn virtiofsd_falso(dir: &Path) -> PathBuf {
    let bin = dir.join("virtiofsd");
    guion(
        &bin,
        "sock=\"\"\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = \"--socket-path\" ]; then sock=\"$2\"; fi\n  shift\ndone\necho $$ > \"$(dirname \"$0\")/virtiofsd.pid\"\n: > \"$sock\"\ntrap 'kill $! 2>/dev/null; exit 0' TERM\nsleep 60 &\nwait\n",
    );
    bin
}

/// Campos de /proc/<pid>/stat que siguen al nombre: [estado, ppid, grupo de procesos, ...]. None si el proceso no existe.
fn stat(pid: i32) -> Option<Vec<String>> {
    let t = std::fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    Some(t[t.rfind(')')? + 1..].split_whitespace().map(|s| s.to_string()).collect())
}

/// ¿Termino el proceso? (no existe o es un zombi a la espera de que lo recojan)
fn termino(pid: i32) -> bool {
    let t0 = Instant::now();
    loop {
        let fuera = stat(pid).map_or(true, |s| s[0] == "Z" || s[0] == "X");
        if fuera || t0.elapsed() > Duration::from_secs(5) {
            return fuera;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn leer_pid(f: &Path) -> i32 {
    let t0 = Instant::now();
    loop {
        if let Some(p) = std::fs::read_to_string(f).ok().and_then(|t| t.trim().parse().ok()) {
            return p;
        }
        assert!(t0.elapsed() < Duration::from_secs(10), "no aparecio {}", f.display());
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Sin el binario de QEMU el arranque falla despues de haber lanzado virtiofsd: no queda ni el servicio de archivos, ni
/// helpers.pid, ni un pid de maquina. Y el registro de QEMU del intento anterior se conserva como qemu.log.1.
#[test]
fn sin_qemu_no_queda_nada_lanzado() {
    let d = carpeta("sin-qemu");
    let estado = d.join("estado");
    let compartida = d.join("compartida");
    std::fs::create_dir_all(&compartida).unwrap();
    let kernel = d.join("kernel");
    std::fs::write(&kernel, b"no es un kernel").unwrap();
    let vfs = virtiofsd_falso(&d);
    let share = format!("datos={}", compartida.display());
    let args = ["start", "--kernel", kernel.to_str().unwrap(), "--qemu", "/no/existe/qemu-system-x86_64", "--share", &share, "--virtiofsd", vfs.to_str().unwrap(), "--accel", "tcg"];
    let o = weft(&estado, &args).output().unwrap();
    let t = texto(&o);
    assert_eq!(o.status.code(), Some(2), "{}", t);
    assert!(t.contains("no se pudo ejecutar /no/existe/qemu-system-x86_64"), "{}", t);
    let m = estado.join("m");
    let pid = leer_pid(&d.join("virtiofsd.pid"));
    assert!(termino(pid), "virtiofsd sigue vivo tras el fallo");
    for f in ["helpers.pid", "pid", "pid-start", "virtiofs0.sock"] {
        assert!(!m.join(f).exists(), "quedo {} tras el fallo", f);
    }
    // segundo intento: el registro de QEMU del primero pasa a qemu.log.1
    std::fs::write(m.join("qemu.log"), "registro del primer intento\n").unwrap();
    let _ = std::fs::remove_file(d.join("virtiofsd.pid"));
    let o = weft(&estado, &args).output().unwrap();
    assert_eq!(o.status.code(), Some(2), "{}", texto(&o));
    assert_eq!(std::fs::read_to_string(m.join("qemu.log.1")).unwrap(), "registro del primer intento\n");
    assert!(termino(leer_pid(&d.join("virtiofsd.pid"))));
    let _ = std::fs::remove_dir_all(&d);
}

/// QEMU (falso) pasa a segundo plano y escribe su pid, pero su control no responde: a los 15 s el arranque falla y se
/// deshace todo (el "QEMU" muere y su pid se borra, y virtiofsd tambien). Mientras tanto, virtiofsd esta en su propio
/// grupo de procesos (un Ctrl+C sobre la orden no lo alcanzaria).
#[test]
fn qemu_sin_control_se_deshace() {
    let d = carpeta("sin-control");
    let estado = d.join("estado");
    let compartida = d.join("compartida");
    std::fs::create_dir_all(&compartida).unwrap();
    let kernel = d.join("kernel");
    std::fs::write(&kernel, b"no es un kernel").unwrap();
    let vfs = virtiofsd_falso(&d);
    let qemu = d.join("qemu-falso");
    guion(&qemu, "pidfile=\"\"\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = \"-pidfile\" ]; then pidfile=\"$2\"; fi\n  shift\ndone\nsleep 60 &\necho $! > \"$pidfile\"\necho $! > \"$(dirname \"$0\")/qemu.pid\"\nexit 0\n");
    let share = format!("datos={}", compartida.display());
    let hijo = weft(&estado, &["start", "--kernel", kernel.to_str().unwrap(), "--qemu", qemu.to_str().unwrap(), "--share", &share, "--virtiofsd", vfs.to_str().unwrap(), "--accel", "tcg"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let vpid = leer_pid(&d.join("virtiofsd.pid"));
    let campos = stat(vpid).expect("virtiofsd vivo durante el arranque");
    assert_eq!(campos[2], vpid.to_string(), "virtiofsd deberia ir en su propio grupo de procesos");
    let qpid = leer_pid(&d.join("qemu.pid"));
    let m = estado.join("m");
    // mientras espera el control, la maquina tiene pid (con su tiempo de arranque)
    let t0 = Instant::now();
    while !m.join("pid-start").exists() && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(m.join("pid").exists() && m.join("pid-start").exists());
    let o = hijo.wait_with_output().unwrap();
    let t = texto(&o);
    assert_eq!(o.status.code(), Some(2), "{}", t);
    assert!(t.contains("15 s"), "{}", t);
    assert!(termino(qpid), "el QEMU falso sigue vivo");
    assert!(termino(vpid), "virtiofsd sigue vivo");
    for f in ["helpers.pid", "pid", "pid-start"] {
        assert!(!m.join(f).exists(), "quedo {} tras el fallo", f);
    }
    // y la maquina consta como detenida
    let o = weft(&estado, &["status"]).output().unwrap();
    assert!(texto(&o).contains("detenida"), "{}", texto(&o));
    let _ = std::fs::remove_dir_all(&d);
}

/// --audio wav:F con --dry-run no crea ni vacia F (lo crea QEMU al arrancar) y da su ruta absoluta; una carpeta que no
/// existe es un error.
#[test]
fn audio_wav_en_seco_no_crea_el_archivo() {
    let d = carpeta("wav");
    let kernel = d.join("kernel");
    std::fs::write(&kernel, b"k").unwrap();
    let wav = d.join("sonido.wav");
    let o = weft(&d.join("estado"), &["start", "--kernel", kernel.to_str().unwrap(), "--audio", &format!("wav:{}", wav.display()), "--accel", "tcg", "--dry-run"]).output().unwrap();
    let t = texto(&o);
    assert_eq!(o.status.code(), Some(0), "{}", t);
    assert!(t.contains(&format!("-audiodev wav,id=snd0,path={}", std::fs::canonicalize(&d).unwrap().join("sonido.wav").display())), "{}", t);
    assert!(!wav.exists(), "--dry-run creo el archivo de audio");
    // uno que ya existe no se vacia
    std::fs::write(&wav, b"RIFF").unwrap();
    let o = weft(&d.join("estado"), &["start", "--kernel", kernel.to_str().unwrap(), "--audio", &format!("wav:{}", wav.display()), "--accel", "tcg", "--dry-run"]).output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", texto(&o));
    assert_eq!(std::fs::read(&wav).unwrap(), b"RIFF");
    let o = weft(&d.join("estado"), &["start", "--kernel", kernel.to_str().unwrap(), "--audio", &format!("wav:{}/no/existe.wav", d.display()), "--dry-run"]).output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(texto(&o).contains("no existe"), "{}", texto(&o));
    let _ = std::fs::remove_dir_all(&d);
}

/// Una carpeta de estado con coma no llega a QEMU: error claro al empezar, que dice como evitarlo. Y las opciones con
/// valores imposibles se rechazan al leerlas.
#[test]
fn rutas_con_coma_y_valores_imposibles() {
    let d = carpeta("coma");
    let kernel = d.join("kernel");
    std::fs::write(&kernel, b"k").unwrap();
    let k = kernel.to_str().unwrap();
    let o = weft(&d.join("a,b"), &["start", "--kernel", k, "--dry-run"]).output().unwrap();
    let t = texto(&o);
    assert_eq!(o.status.code(), Some(2), "{}", t);
    assert!(t.contains("contiene una coma") && t.contains("--state-dir/--root sin comas"), "{}", t);
    let estado = d.join("estado");
    for (opcion, valor, aguja) in [("--adb-port", "0", "1 a 65535"), ("--resolution", "10x10", "fuera de rango"), ("--resolution", "720x99999", "fuera de rango"), ("--hvc-log", "1=/tmp/a,b", "comas")] {
        let mut args = vec!["start", "--kernel", k, opcion, valor, "--dry-run"];
        if opcion == "--hvc-log" {
            args.extend(["--console", "hvc", "--hvc-count", "1"]);
        }
        let o = weft(&estado, &args).output().unwrap();
        let t = texto(&o);
        assert_eq!(o.status.code(), Some(2), "{} {}: {}", opcion, valor, t);
        assert!(t.contains(aguja), "{} {}: {}", opcion, valor, t);
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// La carpeta de estado de la maquina tiene que ser propia y no un enlace simbolico en toda orden que la use, no solo en
/// `start` (si no, `stop` mandaria senales al pid que alguien dejo ahi y la ventana escribiria a traves de los enlaces). La
/// que la contiene, elegida con --state-dir, si puede ser un enlace.
#[test]
fn carpeta_de_estado_enlazada() {
    let d = carpeta("enlace");
    let real = d.join("real");
    std::fs::create_dir_all(real.join("otra")).unwrap();
    std::os::unix::fs::symlink(&real, d.join("estado")).unwrap();
    let o = weft(&d.join("estado"), &["status"]).output().unwrap();
    assert_eq!(o.status.code(), Some(1), "{}", texto(&o));
    assert!(texto(&o).contains("detenida"), "{}", texto(&o));
    std::os::unix::fs::symlink(real.join("otra"), real.join("m")).unwrap();
    for orden in [&["status"][..], &["stop"], &["kill"], &["window"], &["launch"]] {
        let o = weft(&d.join("estado"), orden).output().unwrap();
        assert_eq!(o.status.code(), Some(2), "{:?}: {}", orden, texto(&o));
        assert!(texto(&o).contains("es un enlace simbolico"), "{:?}: {}", orden, texto(&o));
    }
    // `doctor` la explica en vez de negarse
    let o = weft(&d.join("estado"), &["doctor"]).output().unwrap();
    assert!(texto(&o).contains("es un enlace simbolico"), "{}", texto(&o));
    let _ = std::fs::remove_dir_all(&d);
}

/// `weft window` con la maquina parada lo dice y sale con error.
#[test]
fn ventana_sin_maquina() {
    let d = carpeta("ventana");
    let o = weft(&d.join("estado"), &["window"]).output().unwrap();
    assert_eq!(o.status.code(), Some(2), "{}", texto(&o));
    assert!(texto(&o).contains("no esta en marcha"), "{}", texto(&o));
    assert!(weft(&d.join("estado"), &["window", "--otra"]).output().unwrap().status.code() == Some(2));
    let _ = std::fs::remove_dir_all(&d);
}
