//! weft: emulador minimo. Lanza una maquina virtual QEMU (acelerada con KVM) y la controla por comandos,
//! sin interfaz grafica obligatoria, para poder probar todo de forma automatica.

mod adb;
mod adbcmd;
mod apagado;
mod ajustes;
mod aplicar;
mod barra;
mod bootimg;
mod catalogo;
mod compartir;
mod cow;
mod config;
mod dbus;
mod dispositivo;
mod doctor;
mod formas;
mod fuente;
mod gamepad;
mod gestos;
mod gpt;
mod imagen;
mod informe;
mod json;
mod lanzar;
mod md5;
mod textos;
mod vista;
mod pantalla;
mod perfil;
mod puente;
mod qmp;
mod reinicio;
mod rotacion;
mod rsa;
mod rfb;
mod root;
mod rutas;
mod sensors;
mod vm;
mod window;

use json::V;
use qmp::Qmp;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use vm::{Accel, Config, State};
use crate::textos::{clave, tx, txf};

/// Numero de version de esta compilacion: el que calcula GitVersion en el CI (`WEFT_VERSION`, incrustado al compilar) o,
/// en una compilacion local, el de `Cargo.toml`.
pub const VERSION_PAQUETE: &str = match option_env!("WEFT_VERSION") {
    Some(v) if !v.is_empty() => v,
    _ => env!("CARGO_PKG_VERSION"),
};

/// Version del programa con lo que identifica la compilacion: el numero y, si se compilo en el CI, el commit
/// (GITHUB_SHA queda incrustado al compilar). Asi `weft --version`, "Acerca de" y el informe dicen exactamente que
/// binario es.
pub fn version() -> String {
    version_con(VERSION_PAQUETE, option_env!("GITHUB_SHA"))
}

/// `version` sin depender de la compilacion (pura, para las pruebas).
fn version_con(paquete: &str, commit: Option<&str>) -> String {
    match commit.map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) => format!("{} (commit {})", paquete, &c[..c.len().min(12)]),
        None => paquete.to_string(),
    }
}

const HELP: &str = clave!("cli_tec.ayuda");

struct Args {
    v: Vec<String>,
    i: usize,
}

impl Args {
    fn next(&mut self) -> Option<String> {
        let x = self.v.get(self.i).cloned();
        self.i += 1;
        x
    }
    fn val(&mut self, flag: &str) -> Result<String, String> {
        self.next().ok_or(txf!("cli.necesita_un_valor", flag))
    }
    fn num<T: std::str::FromStr>(&mut self, flag: &str) -> Result<T, String> {
        self.val(flag)?.parse().map_err(|_| txf!("cli.numero_no_valido", flag))
    }
}

fn main() {
    // las ordenes que lanza la ventana producen texto generico (ver src/textos.rs)
    textos::del_entorno();
    let code = match run() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("weft: {}", e);
            2
        }
    };
    std::process::exit(code);
}

fn run() -> Result<i32, String> {
    let mut a = Args { v: std::env::args().skip(1).collect(), i: 0 };
    let mut name = "default".to_string();
    let cmd = loop {
        match a.next().as_deref() {
            Some("--name") => name = a.val("--name")?,
            // --root, --data-dir, --config-dir, --cache-dir, --logs-dir y --state-dir: ver src/rutas.rs
            Some(o) if rutas::OPCIONES.iter().any(|(x, _)| *x == o) => {
                let v = a.val(o)?;
                rutas::fijar_opcion(o, &v)?;
            }
            Some("-h") | Some("--help") | Some("help") | None => {
                print!("{}", textos::texto(HELP));
                return Ok(0);
            }
            Some("-V") | Some("--version") | Some("version") => {
                println!("weft {}", version());
                return Ok(0);
            }
            Some(c) => break c.to_string(),
        }
    };
    if cmd == "unpack-boot" {
        return unpack_boot(&mut a);
    }
    if cmd == "make-disk" {
        let (mut out, mut parts) = (None, Vec::new());
        while let Some(f) = a.next() {
            match f.as_str() {
                "--out" => out = Some(a.val(&f)?),
                "--part" => parts.push(gpt::parse_part(&a.val(&f)?)?),
                x => return Err(txf!("cli.opcion_desconocida_2", x)),
            }
        }
        let out = out.ok_or("falta --out")?;
        let (total, written) = gpt::assemble(&out, &parts)?;
        println!("{}", txf!("cli.disco_de_mib_con_particiones_mib_de", total >> 20, parts.len(), written >> 20));
        return Ok(0);
    }
    let st = State::new(None, &name)?;
    // antes de leer o escribir nada en la carpeta de estado (pid, helpers.pid, window.pid...: `stop` manda senales a esos
    // pids y la ventana escribe ahi) tiene que ser nuestra y no un enlace, en toda orden y no solo en `start` (ver rutas.rs).
    // `doctor` y `paths` siguen, para poder explicarlo
    if !matches!(cmd.as_str(), "doctor" | "paths") {
        rutas::comprobar_estado(&st.dir)?;
    }
    // la via del adb propio de esta maquina (vsock o TCP), segun como se arranco
    adb::fijar_via(adbcmd::via_del_estado(&st));
    match cmd.as_str() {
        "start" => start_cli(&st, &mut a),
        "launch" => launch(&st, &mut a),
        "window" => window_cmd(&st, &mut a),
        "status" => status(&st),
        "stop" => {
            let t = timeout_opt(&mut a, 20)?;
            stop(&st, t)
        }
        "restart" => restart(&st, &mut a),
        "kill" => {
            if let Some(pid) = st.pid() {
                anotar_cierre_forzado(&st, tx!("cli.pedido_por_kill"));
                vm::signal(pid, 9);
            } else if st.running() {
                // maquina de otra instancia de Flatpak (otro espacio de pids): por su socket de control
                if let Ok(mut q) = Qmp::connect(&st.qmp()) {
                    let _ = q.exec("quit", None);
                }
            }
            wait_exit(&st, 10)
        }
        "qmp" => {
            let c = a.val("qmp")?;
            let args = match a.next() {
                Some(j) => Some(json::parse(&j)?),
                None => None,
            };
            println!("{}", Qmp::connect(&st.qmp())?.exec(&c, args)?.dump());
            Ok(0)
        }
        "screenshot" => screenshot(&st, &a.val("screenshot")?),
        "key" => {
            let mut q = Qmp::connect(&st.qmp())?;
            let mut any = false;
            while let Some(combo) = a.next() {
                let keys: Vec<&str> = combo.split('-').collect();
                send_keys(&mut q, &keys)?;
                any = true;
            }
            if any {
                Ok(0)
            } else {
                Err(tx!("cli.key_necesita_al_menos_una_tecla").into())
            }
        }
        "type" => {
            let text = a.val("type")?;
            let mut q = Qmp::connect(&st.qmp())?;
            for ch in text.chars() {
                let (k, shift) = vm::qcode(ch).ok_or(txf!("cli.caracter_sin_tecla_equivalente", format!("{:?}", ch)))?;
                if shift {
                    send_keys(&mut q, &["shift", k])?;
                } else {
                    send_keys(&mut q, &[k])?;
                }
            }
            Ok(0)
        }
        "tap" => {
            let (x, y): (f64, f64) = (a.num("tap")?, a.num("tap")?);
            if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
                return Err(tx!("cli.las_coordenadas_van_de_0_a_1").into());
            }
            tap(&st, x, y)
        }
        "rotate" => rotate(&st, a.next()),
        "resolution" => {
            let spec = a.val("resolution")?;
            let (mut wait, mut restart, mut cid) = (0u64, true, None);
            while let Some(f) = a.next() {
                match f.as_str() {
                    "--wait" => wait = a.num("--wait")?,
                    "--no-restart" => restart = false,
                    "--cid" => cid = Some(a.num("--cid")?),
                    x => return Err(txf!("cli.opcion_desconocida_2", x)),
                }
            }
            resolution(&st, &spec, wait, restart, cid.unwrap_or_else(|| adbcmd::cid_del_estado(&st)))
        }
        "replug-pointer" => {
            // desenchufa y vuelve a enchufar la tableta USB: Android relee su configuracion al detectarla de nuevo
            aplicar::replug_pointer(&st)?;
            println!("{}", tx!("cli.puntero_reconectado"));
            Ok(0)
        }
        "click" => {
            let (x, y): (f64, f64) = (a.num("click")?, a.num("click")?);
            if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
                return Err(tx!("cli.las_coordenadas_van_de_0_a_1").into());
            }
            click(&st, x, y)
        }
        "serial" => {
            // con la consola en hvc0, lo que salio antes por el puerto serie (cargador de arranque) va primero
            print!("{}", String::from_utf8_lossy(&std::fs::read(st.f("uart.log")).unwrap_or_default()));
            print!("{}", read_log(&st));
            Ok(0)
        }
        "serial-send" => {
            let text = a.val("serial-send")?;
            serial_send(&st, &format!("{}\n", text))?;
            Ok(0)
        }
        "wait-serial" => {
            let pat = a.val("wait-serial")?;
            let t = timeout_opt(&mut a, 60)?;
            wait_serial(&st, &pat, 0, t).map(|_| 0)
        }
        "sh" => {
            let c = a.val("sh")?;
            let t = timeout_opt(&mut a, 30)?;
            guest_sh(&st, &c, t)
        }
        "sensors-serve" => {
            // uso interno: lo lanza `start --sensors`
            let (ctl, data): (u32, u32) = (a.num("sensors-serve")?, a.num("sensors-serve")?);
            let conn = |n: u32| std::os::unix::net::UnixStream::connect(st.f(&format!("hvc{}.sock", n))).map_err(|e| txf!("cli.consola_hvc", n, e));
            let (c, d) = (conn(ctl)?, conn(data)?);
            let cw = c.try_clone().map_err(|e| e.to_string())?;
            // el pid con su tiempo de arranque: si QEMU muere y otro proceso hereda el numero, el servicio termina igual
            let pid = st.pid().and_then(|p| vm::inicio_de(p).map(|i| (p, i)));
            let orient = st.f("orientation");
            // fija: ese giro; auto (por defecto): aparato derecho, la rotacion la decide Android
            let rot = move || std::fs::read_to_string(&orient).ok().and_then(|t| pantalla::Orientacion::parse(&t)).unwrap_or(pantalla::Orientacion::Auto).para_sensores();
            sensors::serve(c, cw, d, move || pid.is_some_and(|(p, i)| vm::alive_con_inicio(p, i)), rot).map_err(|e| txf!("cli.servicio_de_sensores", e))?;
            Ok(0)
        }
        "config" => config_cmd(&st, &mut a),
        "image" => image_cmd(&st, &mut a),
        "paths" => {
            // donde vive cada cosa de esta maquina (lo usan los guiones y los empaquetados, p. ej. Flatpak)
            let (r, cfg, cat) = contexto(&st);
            let m = rutas::maquina_de(&st.dir);
            // sin HOME ni variables XDG las carpetas son relativas al directorio actual: que se vea tambien aqui
            if let Some(av) = rutas::carpetas_relativas() {
                println!("{}", txf!("cli.aviso", av));
            }
            println!("modo={}", if r.estado_propio { "estado-propio (solo --state-dir)" } else { "estandar" });
            println!("data={}\nconfig={}\ncache={}\nlogs={}\nruntime={}", r.datos.display(), r.config.display(), r.cache.display(), r.registros.display(), st.dir.display());
            println!("machine-config={}\nprofiles={}\nimages={}\nandroid-log={}", rutas::Rutas::config_maquina(&r, &m).display(), r.perfiles_usuario().display(), r.imagenes().display(), r.registro_android(&m).display());
            if let Ok(id) = catalogo::id_de_maquina(&cfg, &catalogo::listar(&r, &cat)) {
                println!("image={}\nimage-dir={}\ndisk={}", id, r.imagen(&id).display(), r.disco(&m, &id).display());
            }
            Ok(0)
        }
        "disk" => disk_cmd(&st, &mut a),
        "factory-reset" => factory_reset_cmd(&st, &mut a),
        "bridge" => bridge_cmd(&st, &mut a),
        "device" => device_cmd(&st, &mut a),
        "apply-settings" => {
            let (mut cid, mut espera) = (None, 180u64);
            while let Some(f) = a.next() {
                match f.as_str() {
                    "--cid" => cid = Some(a.num("--cid")?),
                    "--timeout" => espera = a.num("--timeout")?,
                    x => return Err(txf!("cli.apply_settings_opcion_desconocida", x)),
                }
            }
            let cfg = config::Config::cargar(&st.dir);
            let cid = cid.unwrap_or_else(|| adbcmd::cid_del_estado(&st));
            for l in aplicar::aplicar(&st, cid, &cfg, espera)? {
                println!("{}", l);
            }
            // lo ultimo: puede reiniciar Android
            if let Some(l) = dispositivo::sincronizar_maquina(&st.dir, cid, &cfg, &progreso) {
                println!("{}", txf!("compartir.perfil_de_dispositivo", l.replace('\n', "; ")));
            }
            Ok(0)
        }
        "report" => {
            let (mut cid, mut out) = (None, None);
            while let Some(f) = a.next() {
                match f.as_str() {
                    "--cid" => cid = Some(a.num("--cid")?),
                    "--out" => out = Some(std::path::PathBuf::from(a.val("--out")?)),
                    x => return Err(txf!("cli.report_opcion_desconocida", x)),
                }
            }
            let ruta = informe::crear(&st, cid.unwrap_or_else(|| adbcmd::cid_del_estado(&st)), out.as_deref(), &informe::Marcadores::del_entorno(), &progreso)?;
            println!("{}", txf!("cli.informe_guardado_en_sin_datos_personales", ruta.display()));
            Ok(0)
        }
        "root" => root_cmd(&st, &mut a),
        "share" => share_cmd(&st, &mut a),
        "gamepad" => gamepad_cmd(&st, &mut a),
        "gamepad-serve" => {
            // uso interno: lo lanza `start --gamepad`. Vive mientras viva la maquina.
            let modo = gamepad::Modo::parse(&a.val("gamepad-serve")?)?.ok_or(tx!("cli.gamepad_serve_modo_none"))?;
            gamepad::servir(&st, modo)?;
            Ok(0)
        }
        "window-inject" => window::inject_cli(&st.dir, &std::iter::from_fn(|| a.next()).collect::<Vec<_>>()),
        "dbus-input" => window::input_cli(&st.qmp(), &mut std::iter::from_fn(|| a.next())),
        "window-serve" => {
            // uso interno: lo lanza `start --display window`. Cerrar la ventana apaga la maquina, como en GTK.
            let (w, h): (u32, u32) = (a.num("window-serve")?, a.num("window-serve")?);
            // mientras viva este proceso, la ventana tiene tomado window.lock: asi `weft window` y `launch` saben si hay
            // ventana aunque no vean su pid (otra instancia de Flatpak) o el proceso haya quedado zombi. Si lo tiene otra
            // ventana viva de esta maquina no se abre una segunda: QEMU admite un solo cliente de pantalla y desalojaria a
            // la primera (y sin el cerrojo, `weft window` abriria aun otra)
            let _cerrojo = match tomar_cerrojo_ventana(&st, Duration::from_secs(1)) {
                Ok(Some(f)) => Some(f),
                Ok(None) => return Err(tx!("cli.ya_hay_otra_ventana_abierta_de_esta").into()),
                Err(e) => {
                    eprintln!("{}", txf!("cli.aviso_la_ventana_no_pudo_tomar_su", e));
                    None
                }
            };
            // la ventana sigue la rotacion que Android decida por su cuenta (lee la pantalla de Android por el adb propio)
            let (dir, cid) = (st.dir.clone(), adbcmd::cid_del_estado(&st));
            let dir2 = dir.clone();
            std::thread::spawn(move || rotacion::seguir(dir, cid));
            // carpetas compartidas y root tras cada arranque de Android (red de seguridad del servicio de init)
            std::thread::spawn(move || compartir::vigilar(dir2, cid));
            // si la maquina se cae, la causa y los registros de esta ejecucion quedan en ultimo-fallo.txt en cuanto la
            // ventana lo sabe (antes de esperar a que se cierre su aviso: un arranque nuevo rota los registros, y dentro de
            // Flatpak el sandbox puede terminar antes)
            let anotar = |f: &window::Fallo| anotar_ultimo_fallo(&st, f);
            match window::serve(&st.qmp(), &format!("weft: {}", name), (w, h), &anotar)? {
                window::Exit::Closed => {
                    stop(&st, 20)?;
                }
                window::Exit::Ended(why) => eprintln!("ventana: {}", why),
            }
            Ok(0)
        }
        "events-serve" => {
            // uso interno: lo lanza `start`. Anota en events.log lo que QEMU avisa (apagado y su causa, reinicio,
            // panico del invitado...) para poder saber despues por que termino la maquina.
            use std::io::Write;
            let mut q = Qmp::connect(&st.f("events.sock"))?;
            let mut log = std::fs::OpenOptions::new().create(true).append(true).open(st.f("events.log")).map_err(|e| e.to_string())?;
            let now = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let mut shutdown = false;
            loop {
                match q.next_event() {
                    Ok(m) => {
                        if let Some(ev) = m.get("event").and_then(|e| e.as_str()) {
                            shutdown |= ev == "SHUTDOWN";
                            let data = m.get("data").map(|d| d.dump()).unwrap_or_default();
                            let _ = writeln!(log, "{} {} {}", now(), ev, data);
                        }
                    }
                    Err(_) => {
                        let how = if shutdown { tx!("cli.qemu_termino_tras_el_aviso_de_apagado") } else { tx!("cli.qemu_termino_de_golpe_sin_aviso_de") };
                        let _ = writeln!(log, "{} FIN {}", now(), how);
                        return Ok(0);
                    }
                }
            }
        }
        "wait-exit" => {
            let t = timeout_opt(&mut a, 60)?;
            wait_exit(&st, t)
        }
        "doctor" => {
            let filas = doctor::ejecutar(&st);
            print!("{}", doctor::tabla(&filas));
            let (a, f) = (filas.iter().filter(|x| x.nivel == doctor::Nivel::Aviso).count(), filas.iter().filter(|x| x.nivel == doctor::Nivel::Fallo).count());
            println!("{}", txf!("cli.comprobaciones_fallo_s_aviso_s", filas.len(), f, a));
            Ok(doctor::codigo_salida(&filas))
        }
        c if adbcmd::ORDENES.contains(&c) => {
            let rest: Vec<String> = std::iter::from_fn(|| a.next()).collect();
            adbcmd::run(&st, c, rest)
        }
        other => Err(txf!("cli.orden_desconocida_help", other)),
    }
}

/// Prepara el arranque directo del kernel de una imagen de Android: lo que haria el cargador de arranque.
fn unpack_boot(a: &mut Args) -> Result<i32, String> {
    let (mut boot, mut init_boot, mut vendor_boot, mut out) = (None, None, None, None);
    while let Some(f) = a.next() {
        match f.as_str() {
            "--boot" => boot = Some(a.val(&f)?),
            "--init-boot" => init_boot = Some(a.val(&f)?),
            "--vendor-boot" => vendor_boot = Some(a.val(&f)?),
            "--out" => out = Some(a.val(&f)?),
            x => return Err(txf!("cli.opcion_desconocida_2", x)),
        }
    }
    let (boot, out) = (boot.ok_or("falta --boot")?, out.ok_or("falta --out")?);
    let rd = |p: &str| std::fs::read(p).map_err(|e| format!("{}: {}", p, e));
    let wr = |n: &str, d: &[u8]| std::fs::write(format!("{}/{}", out, n), d).map_err(|e| format!("{}/{}: {}", out, n, e));
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {}", out, e))?;
    let bdata = rd(&boot)?;
    let idata = init_boot.as_deref().map(rd).transpose()?;
    let vdata = vendor_boot.as_deref().map(rd).transpose()?;
    let d = bootimg::desempaquetar(&bdata, idata.as_deref(), vdata.as_deref()).map_err(|e| format!("{}: {}", boot, e))?;
    wr("kernel", &d.kernel)?;
    wr("initrd.img", &d.initrd)?;
    wr("cmdline.txt", d.cmdline.as_bytes())?;
    wr("bootconfig.txt", format!("{}\n", d.bootconfig).as_bytes())?;
    println!("kernel={} initrd={} cmdline={:?} bootconfig={} lineas", d.kernel.len(), d.initrd.len(), d.cmdline, d.bootconfig.lines().count());
    Ok(0)
}

fn timeout_opt(a: &mut Args, default: u64) -> Result<u64, String> {
    match a.next().as_deref() {
        Some("--timeout") => a.num("--timeout"),
        Some(x) => Err(txf!("cli.opcion_desconocida_2", x)),
        None => Ok(default),
    }
}

fn start(st: &State, a: &mut Args) -> Result<i32, String> {
    // la carpeta de estado va dentro de las opciones de QEMU: con una coma no hay arranque posible (ni linea de ordenes)
    st.comprobar_ruta()?;
    // la linea de arranque, para `restart` (el reinicio completo arranca con los mismos argumentos)
    let args_dados: Vec<String> = a.v[a.i.min(a.v.len())..].to_vec();
    let mut c = Config::default();
    let mut dry = false;
    let mut show_bc = false;
    let mut bootconfig: Vec<String> = Vec::new();
    let mut bootconfig_cli: Vec<String> = Vec::new();
    let mut virtiofsd: Option<String> = None;
    let mut folders: Vec<compartir::Carpeta> = Vec::new();
    let mut qemu_set = false;
    let (mut image_opt, mut profile_opt, mut adb_opt): (Option<String>, Option<String>, Option<String>) = (None, None, None);
    // opciones dadas en la linea de ordenes: mandan sobre `config` y sobre el perfil de la imagen
    let mut dado: std::collections::HashSet<String> = std::collections::HashSet::new();
    while let Some(f) = a.next() {
        if matches!(f.as_str(), "--mem" | "--cpus" | "--machine" | "--gpu" | "--gamepad" | "--resolution" | "--density" | "--accel" | "--nics" | "--console" | "--hvc-count" | "--hvc-log" | "--sensors" | "--touch" | "--disk-slot" | "--gpu-opts") {
            dado.insert(f.clone());
        }
        match f.as_str() {
            "--kernel" => c.kernel = Some(vm::absolute(&a.val(&f)?)?),
            "--initrd" => c.initrd = Some(vm::absolute(&a.val(&f)?)?),
            "--append" => c.append = a.val(&f)?,
            "--disk" => {
                let v = a.val(&f)?;
                let (path, ro) = match v.strip_suffix(",ro") {
                    Some(p) => (p, true),
                    None => (v.as_str(), false),
                };
                c.disks.push((vm::absolute(path)?, ro));
            }
            "--bootconfig" => bootconfig_cli.push(a.val(&f)?),
            "--bootconfig-file" => {
                // admite tambien una particion de bootconfig (texto seguido de relleno y cola binaria)
                let raw = std::fs::read(a.val(&f)?).map_err(|e| format!("--bootconfig-file: {}", e))?;
                bootconfig.extend(vm::bootconfig_lines(&raw));
            }
            "--cdrom" => c.cdrom = Some(vm::absolute(&a.val(&f)?)?),
            "--gpu-hostmem" => c.gpu_hostmem_mb = a.num(&f)?,
            "--gpu-opts" => c.gpu_opts = a.val(&f)?,
            "--machine" => c.machine = a.val(&f)?,
            "--disk-slot" => c.disk_slot = a.num(&f)?,
            "--resolution" => c.resolution = Some(resolucion_de_arranque(&a.val(&f)?)?),
            "--vsock-cid" => c.vsock_cid = Some(a.num(&f)?),
            "--hvc-count" => c.hvc_count = a.num(&f)?,
            "--hvc-log" => {
                let v = a.val(&f)?;
                let (n, path) = v.split_once('=').ok_or(tx!("cli.hvc_log_se_espera_n_archivo"))?;
                c.hvc_logs.push((n.parse().map_err(|_| tx!("cli.hvc_log_numero_no_valido"))?, path.to_string()));
            }
            "--sensors" => {
                let v = a.val(&f)?;
                let (x, y) = v.split_once(',').ok_or(tx!("cli.sensors_se_espera_control_datos"))?;
                c.sensors = Some((x.parse().map_err(|_| tx!("cli.sensors_numero_no_valido"))?, y.parse().map_err(|_| tx!("cli.sensors_numero_no_valido"))?));
            }
            "--share" => {
                let v = a.val(&f)?;
                let (tag, dir) = v.split_once('=').ok_or(tx!("cli.share_se_espera_etiqueta_carpeta"))?;
                c.shares.push((tag.to_string(), vm::absolute(dir)?));
            }
            "--virtiofsd" => virtiofsd = Some(a.val(&f)?),
            "--folder" => {
                let v = a.val(&f)?;
                let (nombre, resto) = v.split_once('=').ok_or(tx!("cli.folder_se_espera_nombre_carpeta_ro"))?;
                let (ruta, ro) = match resto.strip_suffix(":ro") {
                    Some(r) => (r, true),
                    None => (resto, false),
                };
                compartir::validar_nombre(nombre).map_err(|e| format!("--folder: {}", e))?;
                folders.push(compartir::Carpeta { nombre: nombre.to_string(), ro, ruta: vm::absolute(ruta)? });
            }
            "--touch" => c.touch = true,
            "--density" => {
                let d: u32 = a.num(&f)?;
                if !(72..=960).contains(&d) {
                    return Err("--density: de 72 a 960".into());
                }
                bootconfig_cli.push(vm::density_bootconfig(d));
            }
            "--pointer" => c.pointer = a.val(&f)?,
            "--gamepad" => {
                c.gamepad = a.val(&f)?;
                gamepad::Modo::parse(&c.gamepad)?;
            }
            "--audio" => {
                let v = a.val(&f)?;
                c.audio = match v.strip_prefix("wav:") {
                    // el archivo lo crea QEMU al arrancar (con --dry-run no se toca): aqui solo se comprueba la carpeta
                    Some(path) => format!("wav:{}", vm::ruta_wav(path)?),
                    None => v,
                };
            }
            "--console" => {
                c.console_hvc = match a.val(&f)?.as_str() {
                    "serial" => false,
                    "hvc" => true,
                    x => return Err(txf!("cli.consola_desconocida_serial_hvc", x)),
                }
            }
            "--mem" => c.mem_mb = a.num(&f)?,
            "--cpus" => c.cpus = a.num(&f)?,
            "--display" => c.display = a.val(&f)?,
            "--display-gl" => c.display_gl = a.val(&f)?,
            "--gpu" => c.gpu = a.val(&f)?,
            "--accel" => {
                c.accel = match a.val(&f)?.as_str() {
                    "auto" => Accel::Auto,
                    "kvm" => Accel::Kvm,
                    "tcg" => Accel::Tcg,
                    x => return Err(txf!("cli.aceleracion_desconocida", x)),
                }
            }
            "--adb-port" => c.adb_port = Some(puerto_adb(&a.val(&f)?)?),
            "--no-net" => c.net = false,
            "--nics" => c.nics = a.num(&f)?,
            "--qemu" => {
                c.qemu = a.val(&f)?;
                qemu_set = true;
            }
            "--dry-run" => dry = true,
            "--show-bootconfig" => show_bc = true,
            "--image" => image_opt = Some(a.val(&f)?),
            "--profile" => profile_opt = Some(a.val(&f)?),
            "--adb" => {
                let v = a.val(&f)?;
                if !config::TRANSPORTES_ADB.contains(&v.as_str()) {
                    return Err(txf!("cli.adb_se_espera", config::TRANSPORTES_ADB.join(", ")));
                }
                adb_opt = Some(v);
            }
            "--" => {
                while let Some(x) = a.next() {
                    c.extra.push(x);
                }
            }
            x => return Err(txf!("cli.opcion_desconocida_2", x)),
        }
    }
    // lo que no se dio en la linea de ordenes sale de `config` (opcion > config > defecto)
    let cfg = config::Config::cargar(&st.dir);
    for av in &cfg.avisos {
        eprintln!("{}", txf!("cli.aviso", av));
    }
    // nucleos y memoria: opcion > config > perfil de dispositivo > defecto (ver dispositivo.rs). Un perfil que ya no existe
    // no impide arrancar: se avisa y se arranca sin el
    let ((cpus, ram), aviso_disp) = dispositivo::recursos_de_maquina(&cfg, &rutas::actual().con_claves(&cfg.get("dir.datos"), &cfg.get("dir.cache")));
    if let Some(av) = aviso_disp {
        eprintln!("{}", txf!("cli.aviso", av));
    }
    if !dado.contains("--mem") {
        if let Some(v) = ram {
            c.mem_mb = v;
        }
    }
    if !dado.contains("--cpus") {
        if let Some(v) = cpus {
            c.cpus = v;
        }
    }
    if !dado.contains("--machine") && cfg.es_explicita("maquina.tipo") {
        c.machine = cfg.get("maquina.tipo");
    }
    if !dado.contains("--gpu") && cfg.es_explicita("maquina.gpu") {
        c.gpu = cfg.get("maquina.gpu");
    }
    if !dado.contains("--gamepad") && cfg.es_explicita("gamepad") {
        c.gamepad = cfg.get("gamepad");
    }
    if c.resolution.is_none() {
        c.resolution = cfg.resolucion();
    }
    if let Some((w, h)) = c.resolution.filter(|(w, _)| w % 8 != 0) {
        eprintln!("{}", txf!("cli.aviso_el_controlador_drm_de_linux", w / 8 * 8, h));
    }
    // (la densidad viaja en el bootconfig de Android: solo con arranque directo de un initrd)
    if !dado.contains("--density") && c.initrd.is_some() {
        if let Some(d) = cfg.densidad() {
            if !bootconfig_cli.iter().any(|l| l.starts_with("androidboot.lcd_density=")) {
                bootconfig_cli.push(vm::density_bootconfig(d));
            }
        }
    }
    // --- imagen de Android: la receta de su perfil (kernel, discos RAM, parametros de arranque, disco, consolas, sensores...) fija
    // lo que la linea de ordenes no dio. Con --kernel/--initrd/--disk/--cdrom/-- es el lanzador de QEMU de siempre.
    let rutas = rutas::actual().con_claves(&cfg.get("dir.datos"), &cfg.get("dir.cache"));
    let maquina = rutas::maquina_de(&st.dir);
    let cat = catalogo::cargar_catalogo(&rutas);
    for av in &cat.avisos {
        eprintln!("{}", txf!("cli.aviso", av));
    }
    let crudo = c.kernel.is_some() || c.initrd.is_some() || !c.disks.is_empty() || c.cdrom.is_some() || !c.extra.is_empty();
    let elegida = match (&image_opt, crudo) {
        (Some(x), _) => Some(elegir_imagen(&rutas, &cat, x, profile_opt.as_deref())?),
        (None, false) => {
            let inst = catalogo::listar(&rutas, &cat);
            let id = catalogo::id_de_maquina(&cfg, &inst)?;
            Some(elegir_imagen(&rutas, &cat, &id, profile_opt.as_deref())?)
        }
        (None, true) => None,
    };
    let mut disco_automatico: Option<(std::path::PathBuf, perfil::Perfil, std::path::PathBuf, String)> = None;
    if let Some(e) = &elegida {
        if e.perfil.arquitectura != perfil::Arq::X86_64 {
            return Err(txf!("cli.el_perfil_es_de_arquitectura_arrancarla", format!("{:?}", e.perfil.id), e.perfil.arquitectura.nombre()));
        }
        let faltan: Vec<String> = e.perfil.archivos_requeridos().into_iter().filter(|f| !e.carpeta.join(f).is_file()).collect();
        if !faltan.is_empty() {
            return Err(txf!("cli.a_la_imagen_le_faltan", format!("{:?}", e.id), faltan.join(", ")));
        }
        if !e.perfil.tipos_maquina.contains(&c.machine.split(',').next().unwrap_or("").to_string()) {
            return Err(txf!("cli.el_perfil_admite_las_maquinas_no", format!("{:?}", e.perfil.id), e.perfil.tipos_maquina.join(", "), c.machine));
        }
        if c.mem_mb < e.perfil.ram_minima {
            eprintln!("{}", txf!("cli.aviso_el_perfil_pide_al_menos_mib_de", format!("{:?}", e.perfil.id), e.perfil.ram_minima, c.mem_mb));
        }
        if !e.perfil.aviso.is_empty() {
            eprintln!("{}", txf!("cli.aviso", e.perfil.aviso));
        }
        let arr = perfil::preparar_arranque(&e.carpeta, &e.perfil, &rutas.cache_arranque(&e.id))?;
        let rec = e.perfil.receta(&arr, Some(&rutas.perfiles_usuario()))?;
        if c.kernel.is_none() {
            c.kernel = Some(vm::absolute(&arr.kernel.to_string_lossy())?);
        }
        if c.initrd.is_none() {
            c.initrd = Some(vm::absolute(&arr.initrd.to_string_lossy())?);
        }
        c.append = format!("{} {}", rec.append, c.append).trim().to_string();
        let mut previo = rec.bootconfig.clone();
        previo.append(&mut bootconfig);
        bootconfig = previo;
        if !dado.contains("--disk-slot") {
            c.disk_slot = rec.disk_slot;
        }
        if !dado.contains("--console") {
            c.console_hvc = rec.console_hvc;
        }
        if !dado.contains("--hvc-count") {
            c.hvc_count = rec.hvc_count;
        }
        if !dado.contains("--hvc-log") {
            if let Some(n) = rec.hvc_log {
                c.hvc_logs.push((n, rutas.registro_android(&maquina).to_string_lossy().into_owned()));
            }
        }
        if !dado.contains("--sensors") {
            c.sensors = rec.sensors;
        }
        if !dado.contains("--touch") {
            c.touch = rec.touch;
        }
        if !dado.contains("--nics") {
            c.nics = rec.nics;
        }
        if !dado.contains("--gpu-opts") {
            c.gpu_opts = rec.gpu_opts.clone();
        }
        if !dado.contains("--accel") {
            c.accel = rec.accel.clone();
        }
        if c.disks.is_empty() {
            let d = rutas.disco(&maquina, &e.id);
            if !d.exists() {
                disco_automatico = Some((d.clone(), e.perfil.clone(), e.carpeta.clone(), e.id.clone()));
                c.disks.push((d.to_string_lossy().into_owned(), false));
            } else {
                c.disks.push((vm::absolute(&d.to_string_lossy())?, false));
            }
        }
    }
    // --- adb: vsock y/o TCP (el reenvio de un puerto local a la tarjeta de red de la imagen)
    let mut transporte_adb = String::new();
    // el puerto del adb por TCP lo eligio weft (el primero libre): si QEMU no lo consigue abrir, se prueba con otro
    let mut puerto_automatico = false;
    // el CID de vsock lo elige weft (no se dio --vsock-cid): el primero libre entre las maquinas en marcha, bajo el cerrojo
    // de asignacion (ver mas abajo); uno pedido con --vsock-cid no se cambia
    let mut cid_automatico = false;
    if elegida.is_some() || adb_opt.is_some() || cfg.es_explicita("adb.transporte") {
        let modo = adb_opt.clone().unwrap_or_else(|| cfg.get("adb.transporte"));
        let vsock_ok = vsock_utilizable();
        let sin_vsock = if cfg!(target_os = "linux") && !vsock_ok { vsock_no_motivo() } else { String::new() };
        // lo que la imagen admite (su perfil): p. ej. una familia cuyas tarjetas de red no obtienen direccion no admite TCP
        let tcp_ok = elegida.as_ref().map_or(true, |e| e.perfil.adb_transportes.iter().any(|t| t == "tcp"));
        let vsock_admitido = elegida.as_ref().map_or(true, |e| e.perfil.adb_transportes.iter().any(|t| t == "vsock"));
        let sin_tcp = || txf!("cli.la_imagen_no_admite_adb_por_tcp_su", format!("{:?}", elegida.as_ref().map_or("", |e| e.id.as_str())), elegida.as_ref().map_or(String::new(), |e| e.perfil.adb_transportes.join(" ")));
        let (usa_vsock, usa_tcp) = match modo.as_str() {
            "vsock" if !vsock_ok => return Err(txf!("cli.adb_vsock_usa_adb_tcp_o_auto", sin_vsock)),
            "vsock" if !vsock_admitido => return Err(tx!("cli.la_imagen_no_admite_adb_por_vsock_segun").into()),
            "vsock" => (true, false),
            "tcp" if !tcp_ok => return Err(txf!("cli.adb_tcp_usa_adb_vsock_o_auto", sin_tcp())),
            "tcp" => (false, true),
            "both" if vsock_ok && tcp_ok => (true, true),
            "both" if vsock_ok => {
                eprintln!("{}", txf!("cli.aviso_el_adb_va_solo_por_vsock", sin_tcp()));
                (true, false)
            }
            "both" if tcp_ok => {
                eprintln!("{}", txf!("cli.aviso_el_adb_va_solo_por_tcp", sin_vsock));
                (false, true)
            }
            "both" => return Err(format!("{}; y {}", sin_vsock, sin_tcp())),
            _ if vsock_ok && vsock_admitido => (true, false),
            _ if tcp_ok => {
                eprintln!("{}", txf!("cli.aviso_el_adb_va_por_tcp", sin_vsock));
                (false, true)
            }
            _ => return Err(txf!("cli.no_hay_por_donde_hablar_con_android_y", sin_vsock, sin_tcp())),
        };
        if usa_vsock {
            if c.vsock_cid.is_none() {
                // provisional (lo que muestra --dry-run): el definitivo se elige con el cerrojo tomado
                c.vsock_cid = Some(3);
                cid_automatico = true;
            }
        } else {
            c.vsock_cid = None;
        }
        if usa_tcp {
            let pedido = c.adb_port.map(u32::from).or_else(|| cfg.entero("adb.puerto"));
            let puerto = match pedido {
                Some(p) => p as u16,
                None => {
                    puerto_automatico = true;
                    puerto_libre(15555).ok_or(tx!("cli.no_hay_ningun_puerto_libre_desde_el"))?
                }
            };
            c.adb_port = Some(puerto);
            c.adb_net = elegida.as_ref().map_or(0, |e| e.perfil.adb_tcp_red);
        } else {
            c.adb_port = None;
        }
        transporte_adb = match (usa_vsock, usa_tcp) {
            (true, true) => "both",
            (false, true) => "tcp",
            _ => "vsock",
        }
        .to_string();
    }
    // carpetas compartidas con las apps de Android: las de `config` y las de --folder (estas mandan si coincide el nombre)
    let mut lista = compartir::lista(&cfg);
    for f in folders {
        lista.retain(|x| !x.nombre.eq_ignore_ascii_case(&f.nombre));
        lista.push(f);
    }
    lista.retain(|x| match compartir::comprobar_ruta(&x.ruta) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("{}", txf!("cli.aviso_carpeta_compartida_omitida", format!("{:?}", x.nombre), e));
            false
        }
    });
    lista.truncate(compartir::MAX);
    let uid_android = compartir::uid_configurado(&cfg);
    let servicios_share = compartir::servicios(&lista, uid_android);
    if !lista.is_empty() {
        if c.initrd.is_none() {
            return Err(tx!("cli.las_carpetas_compartidas_necesitan").into());
        }
        for s in &servicios_share {
            c.shares.push((s.etiqueta.clone(), s.ruta.clone()));
        }
        bootconfig_cli.extend(compartir::bootconfig(&lista));
    }
    let kvm = vm::kvm_usable();
    // QEMU propio (con gfxstream) junto al ejecutable o en WEFT_QEMU_DIR: se usa si no se indico otro
    // (si el sistema no deja consultar la ruta del ejecutable, se usa la ruta con la que fue invocado)
    let exe_dir = std::env::current_exe()
        .ok()
        .or_else(|| std::env::args().next().filter(|a| a.contains('/')).map(std::path::PathBuf::from))
        .and_then(|p| p.canonicalize().ok())
        .and_then(|p| p.parent().map(|d| d.to_string_lossy().to_string()));
    let own = if qemu_set { None } else { vm::bundled_qemu(std::env::var("WEFT_QEMU_DIR").ok().as_deref(), exe_dir.as_deref()) };
    let mut qemu_lib: Option<String> = None;
    if let Some(d) = &own {
        c.qemu = format!("{}/bin/qemu-system-x86_64", d);
        qemu_lib = Some(format!("{}/lib", d));
    }
    // o bien el QEMU del sistema mas una carpeta con las bibliotecas de gfxstream
    let gfx = if own.is_some() { None } else { vm::gfx_libs(std::env::var("WEFT_GFX_DIR").ok().as_deref(), exe_dir.as_deref()) };
    if let Some(g) = &gfx {
        qemu_lib = Some(g.clone());
    }
    // motor grafico: la intencion (auto, hardware, software) o un nombre concreto; ver vm::resolver_gpu
    let hay_hardware = (own.is_some() || gfx.is_some()) && qemu_ofrece(&c.qemu, qemu_lib.as_deref(), "virtio-gpu-rutabaga");
    {
        let motores = elegida.as_ref().map(|e| e.perfil.motores.clone());
        let (g, aviso) = vm::resolver_gpu(&c.gpu, hay_hardware, motores.as_deref())?;
        c.gpu = g;
        if let Some(av) = aviso {
            eprintln!("{}", txf!("cli.aviso", av));
        }
    }
    // Android necesita saber que motor grafico usar segun la GPU. Prioridad (gana el ultimo): archivos de
    // configuracion, motor segun --gpu, y por ultimo lo indicado a mano con --bootconfig
    if c.initrd.is_some() && !(bootconfig.is_empty() && bootconfig_cli.is_empty()) {
        bootconfig.extend(vm::gpu_bootconfig(&c.gpu));
    }
    bootconfig.append(&mut bootconfig_cli);
    // la configuracion de arranque viaja pegada al initrd: se usa una copia en el directorio de estado
    let orig_initrd = c.initrd.clone();
    let bootconfig = vm::bootconfig_merge(&bootconfig);
    if !bootconfig.is_empty() {
        if c.initrd.is_none() {
            return Err(tx!("cli.bootconfig_requiere_initrd").into());
        }
        c.initrd = Some(st.f("initrd-bootconfig.img"));
        if !c.append.split(' ').any(|w| w == "bootconfig") {
            c.append = format!("{} bootconfig", c.append).trim().to_string();
        }
    }
    c.paused = c.display == "gtk" && c.resolution.is_some();
    let armar_args = |c: &Config| -> Result<Vec<String>, String> {
        let mut args = vm::qemu_args(c, st, kvm)?;
        if let Some(d) = &own {
            // BIOS y ROM del QEMU propio
            args.extend(["-L".to_string(), format!("{}/share/qemu", d)]);
        }
        Ok(args)
    };
    let mut args = armar_args(&c)?;
    if dry {
        println!("{} {}", c.qemu, args.join(" "));
        if show_bc {
            eprintln!("{}", txf!("cli.bootconfig_lineas", bootconfig.len()));
            for l in &bootconfig {
                eprintln!("{}", l);
            }
        }
        return Ok(0);
    }
    if st.running() {
        return Err(tx!("cli.la_maquina_ya_esta_en_marcha_stop").into());
    }
    // el CID de vsock es unico en todo el equipo: si otra maquina en marcha ya tiene uno pedido con --vsock-cid, QEMU no
    // arrancaria (y en `restart` se confundiria con la espera a que el kernel suelte el CID de la propia)
    let bases: Vec<std::path::PathBuf> = st.dir.parent().map(|p| p.to_path_buf()).into_iter().chain([rutas::actual().ejecucion]).collect();
    if let (Some(cid), false) = (c.vsock_cid, cid_automatico) {
        vm::comprobar_cid(cid, &vm::cids_en_marcha(&bases, &st.dir))?;
    }
    // disco de la imagen elegida: se arma la primera vez (un disco por imagen; ver catalogo.rs)
    if let Some((ruta, p, carpeta, disco_id)) = &disco_automatico {
        if let Some(d) = ruta.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {}", d.display(), e))?;
        }
        let datos = imagen::parsear_datos(&cfg.get("disk.data"))?;
        let base = base_de_copia(&rutas, &cfg, disco_id, &datos);
        let (t, _) = imagen::crear_disco(p, carpeta, &datos, ruta, base.as_deref(), &progreso)?;
        eprintln!("{}", t);
    }
    for (_, ruta) in &c.hvc_logs {
        if let Some(d) = std::path::Path::new(ruta).parent().filter(|d| !d.as_os_str().is_empty()) {
            let _ = std::fs::create_dir_all(d);
        }
    }
    // el estado de ejecucion (la carpeta de esta maquina y, si es la estandar, la de todas) tiene que ser nuestro y privado
    // antes de escribir nada en el: en el directorio temporal compartido otro usuario pudo adelantarse (ver rutas.rs). Una
    // carpeta de estado elegida con --state-dir o --root es del usuario: no se le cambian los permisos
    rutas::asegurar_estado(&st.dir)?;
    // varias maquinas a la vez: el CID de vsock y el puerto del adb que elige weft se eligen con el cerrojo de asignacion
    // tomado, y se tiene hasta que QEMU arranco con ellos (lo suelta el final de esta funcion): dos `start` a la vez no
    // eligen lo mismo. Uno pedido (--vsock-cid, --adb-port, adb.puerto) no se cambia
    let _cerrojo = if cid_automatico || puerto_automatico { vm::cerrojo_asignacion(&bases, Duration::from_secs(90))? } else { Vec::new() };
    if cid_automatico {
        let en_marcha = vm::cids_en_marcha(&bases, &st.dir);
        let cid = vm::cid_libre(3, &en_marcha);
        if cid != 3 {
            eprintln!("{}", txf!("cli.el_cid_de_vsock_3_lo_usa_otra_maquina_en", cid));
        }
        c.vsock_cid = Some(cid);
    }
    if puerto_automatico {
        let otros = vm::puertos_en_marcha(&bases, &st.dir);
        let puerto = (15555u16..15755).find(|p| !otros.iter().any(|(_, o)| *o == u32::from(*p)) && std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok());
        c.adb_port = Some(puerto.ok_or(tx!("cli.no_hay_ningun_puerto_libre_desde_el"))?);
    }
    if cid_automatico || puerto_automatico {
        args = armar_args(&c)?;
    }
    kill_helpers(st);
    // la ventana de la ejecucion anterior, si sigue abierta (con su aviso de fin), antes de borrar su window.pid
    cerrar_ventana_anterior(st);
    // los registros de QEMU y de sus eventos de la ejecucion anterior se conservan en .1 (sirven para saber por que se cayo)
    for f in ["qemu.log", "events.log"] {
        if let Err(e) = vm::rotar_registro(&st.dir.join(f)) {
            eprintln!("{}", txf!("cli.aviso_no_se_pudo_conservar_el_registro_2", f, e));
        }
    }
    for f in ["qmp.sock", "vnc.sock", "events.sock", "serial.sock", "serial.log", "uart.log", "pid", "pid-start", "sensors.log", "touch", "pointer", "orientation", "orientation.tmp", "android-rotation", "android-rotation.tmp", "requested-resolution", "requested-resolution.tmp", "gamepad-ignore", "gamepad.log", "vsock-cid", "android-log.path", "disk.path", "window.pid", "adb-transport", "adb-port", "image-id", "pid-ns"] {
        let _ = std::fs::remove_file(st.f(f));
    }
    if !bootconfig.is_empty() {
        let rd = std::fs::read(orig_initrd.as_deref().unwrap()).map_err(|e| format!("initrd: {}", e))?;
        std::fs::write(st.f("initrd-bootconfig.img"), vm::append_bootconfig(&rd, &bootconfig)).map_err(|e| txf!("cli.initrd_con_bootconfig", e))?;
    }
    // el CID del invitado: lo usan las ordenes del adb propio (adb-shell, push, install...) y la ventana
    if let Some(cid) = c.vsock_cid {
        std::fs::write(st.f("vsock-cid"), format!("{}\n", cid)).map_err(|e| e.to_string())?;
    }
    // via del adb propio de esta maquina (la leen las demas ordenes: adbcmd::via_del_estado)
    if transporte_adb.is_empty() {
        transporte_adb = match (c.vsock_cid.is_some(), c.adb_port.is_some()) {
            (true, true) => "both",
            (false, true) => "tcp",
            _ => "vsock",
        }
        .to_string();
    }
    std::fs::write(st.f("adb-transport"), format!("{}\n", transporte_adb)).map_err(|e| e.to_string())?;
    if let Some(p) = c.adb_port {
        std::fs::write(st.f("adb-port"), format!("{}\n", p)).map_err(|e| e.to_string())?;
    }
    if let Some(e) = &elegida {
        let _ = std::fs::write(st.f("image-id"), format!("{}\n", e.id));
    }
    if c.touch || c.pointer == "multitouch" {
        std::fs::write(st.f("touch"), b"").map_err(|e| e.to_string())?;
    }
    // el disco con que arranca esta maquina, para la pantalla de configuracion (Imagen y almacenamiento)
    if let Some((disco, _)) = c.disks.first() {
        let _ = std::fs::write(st.f("disk.path"), format!("{}\n", disco));
    }
    // donde esta el registro de Android (consola hvc), para `report`
    if let Some((_, ruta)) = c.hvc_logs.first() {
        let abs = if std::path::Path::new(ruta).is_absolute() { std::path::PathBuf::from(ruta) } else { std::env::current_dir().map(|d| d.join(ruta)).unwrap_or_else(|_| std::path::PathBuf::from(ruta)) };
        let _ = std::fs::write(st.f("android-log.path"), format!("{}\n", abs.display()));
    }
    // el puntero, para la ventana (multitouch = el raton es un contacto tactil) y para `click`
    std::fs::write(st.f("pointer"), c.pointer.as_bytes()).map_err(|e| e.to_string())?;
    // el espacio de pids de la maquina es el de este proceso: se anota antes de lanzar QEMU, para que otra instancia de
    // Flatpak que mire el estado mientras este arranque espera al control no tome el pid de QEMU por uno suyo y lo borre
    // (vm::State::pid)
    st.anotar_espacio_de_pids();
    // desde aqui se lanzan procesos (servicios de archivos, QEMU, servicios internos): si un paso falla, se deshace lo que
    // ya se lanzo antes de devolver el error, para no dejar un QEMU o un virtiofsd huerfanos (ver deshacer_arranque)
    let mut lanzado = lanzar_procesos(st, &c, &args, virtiofsd.as_deref(), &servicios_share, qemu_lib.as_deref());
    // lo que eligio weft lo tomo otro antes que QEMU (un programa ajeno a weft con ese puerto, otra maquina virtual que no es
    // de weft con ese CID): se reintenta con el siguiente libre, unas pocas veces. Uno pedido con --adb-port, adb.puerto o
    // --vsock-cid no se cambia
    for _ in 0..8 {
        let Err(e) = &lanzado else { break };
        if let (true, Some(p)) = (reinicio::es_puerto_ocupado(e), c.adb_port) {
            deshacer_arranque(st);
            let Some(n) = puerto_libre(p.saturating_add(1)).filter(|_| puerto_automatico) else {
                lanzado = Err(txf!("cli.el_puerto_de_127_0_0_1_para_el_adb_por", e, p));
                break;
            };
            eprintln!("{}", txf!("cli.aviso_el_puerto_para_el_adb_por_tcp_se", p, n));
            c.adb_port = Some(n);
            std::fs::write(st.f("adb-port"), format!("{}\n", n)).map_err(|e| e.to_string())?;
        } else if let (true, true, Some(cid)) = (reinicio::es_carrera_de_cid(e), cid_automatico, c.vsock_cid) {
            deshacer_arranque(st);
            let n = vm::cid_libre(cid + 1, &vm::cids_en_marcha(&bases, &st.dir));
            eprintln!("{}", txf!("cli.aviso_el_cid_de_vsock_lo_tiene_otra", cid, n));
            c.vsock_cid = Some(n);
            std::fs::write(st.f("vsock-cid"), format!("{}\n", n)).map_err(|e| e.to_string())?;
        } else {
            break;
        }
        args = armar_args(&c)?;
        st.anotar_espacio_de_pids();
        lanzado = lanzar_procesos(st, &c, &args, virtiofsd.as_deref(), &servicios_share, qemu_lib.as_deref());
    }
    if let Err(e) = lanzado {
        deshacer_arranque(st);
        if reinicio::es_carrera_de_cid(&e) {
            let cid = c.vsock_cid.unwrap_or(3);
            return Err(txf!("cli.el_cid_de_vsock_lo_tiene_otra_maquina", e, cid));
        }
        return Err(e);
    }
    // lo que `restart` necesita para repetir este arranque (no se guarda con --dry-run: arriba ya se salio)
    let cwd = std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    if let Err(e) = reinicio::Arranque::del_entorno(&cwd, &args_dados, std::env::vars()).guardar(&st.dir) {
        eprintln!("{}", txf!("cli.aviso", e));
    }
    let accel = if args.iter().any(|x| x.contains("accel=kvm")) { "kvm" } else { tx!("cli.tcg_sin_aceleracion") };
    println!("{}", txf!("cli.en_marcha_pid_aceleracion", st.pid().unwrap_or(0), accel));
    Ok(0)
}

/// Lanza los procesos de un arranque: un servicio de archivos (virtiofsd) por carpeta compartida, QEMU (que pasa a segundo
/// plano y escribe su pid), y los servicios internos (sensores, eventos, mandos y la ventana propia). Los procesos que
/// sobreviven a esta orden van en su propio grupo de procesos: un Ctrl+C sobre `launch` (o sobre `start`) no los mata.
/// Si un paso falla, quien llama deshace lo ya lanzado (`deshacer_arranque`).
fn lanzar_procesos(st: &State, c: &Config, args: &[String], virtiofsd: Option<&str>, servicios_share: &[compartir::Servicio], qemu_lib: Option<&str>) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    // un servicio de archivos por carpeta compartida; QEMU se conecta a su socket al arrancar
    for (i, (tag, dir)) in c.shares.iter().enumerate() {
        let servicio = servicios_share.iter().find(|s| &s.etiqueta == tag);
        let bin = match virtiofsd {
            Some(b) => b.to_string(),
            None => compartir::buscar_virtiofsd().ok_or(tx!("cli.no_se_encontro_virtiofsd_instalalo_o"))?,
        };
        let sock = st.f(&format!("virtiofs{}.sock", i));
        let _ = std::fs::remove_file(&sock);
        let log = std::fs::File::create(st.f(&format!("virtiofs{}.log", i))).map_err(|e| e.to_string())?;
        let mut vcmd = Command::new(&bin);
        if let Some(sv) = servicio {
            // las carpetas visibles para las apps: el dueno del equipo se presenta como el uid de MediaProvider (ver
            // compartir.rs) y, si es de solo lectura, el propio virtiofsd lo impone
            vcmd.args(compartir::args_traduccion(sv));
            if sv.ro {
                vcmd.arg("--readonly");
            }
        }
        let child = vcmd
            // Android etiqueta los archivos con atributos extendidos (SELinux): se guardan como atributos de usuario
            // del anfitrion, para no necesitar privilegios
            .args(["--socket-path", &sock, "--shared-dir", dir, "--sandbox", "none", "--cache", "auto", "--xattr", "--xattrmap", ":map::user.virtiofs.:"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .process_group(0)
            .spawn()
            .map_err(|e| format!("{}: {}", bin, e))?;
        let mut pids = std::fs::read_to_string(st.f("helpers.pid")).unwrap_or_default();
        pids.push_str(&format!("{}\n", child.id()));
        std::fs::write(st.f("helpers.pid"), pids).map_err(|e| e.to_string())?;
        let t0 = Instant::now();
        while !std::path::Path::new(&sock).exists() {
            if t0.elapsed() > Duration::from_secs(10) {
                let err = std::fs::read_to_string(st.f(&format!("virtiofs{}.log", i))).unwrap_or_default();
                return Err(txf!("cli.virtiofsd_no_arranco", err.trim()));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let log = std::fs::File::create(st.f("qemu.log")).map_err(|e| e.to_string())?;
    let mut qcmd = Command::new(&c.qemu);
    if let Some(l) = qemu_lib {
        let prev = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        qcmd.env("LD_LIBRARY_PATH", if prev.is_empty() { l.to_string() } else { format!("{}:{}", l, prev) });
    }
    let out = qcmd
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .status()
        .map_err(|e| txf!("catalogo.no_se_pudo_ejecutar", c.qemu, e))?;
    if !out.success() {
        let err = std::fs::read_to_string(st.f("qemu.log")).unwrap_or_default();
        return Err(txf!("cli.qemu_no_arranco", out, err.trim()));
    }
    // QEMU ya escribio su pid: junto a el va el tiempo de arranque de ese proceso, para no confundir luego a la maquina
    // con otro proceso que herede el numero (vm::State::pid)
    if let Err(e) = st.anotar_inicio() {
        eprintln!("{}", txf!("cli.aviso", e));
    }
    // el control debe responder antes de dar el arranque por bueno
    let t0 = Instant::now();
    loop {
        match Qmp::connect(&st.qmp()) {
            Ok(_) => break,
            Err(e) if t0.elapsed() > Duration::from_secs(15) => return Err(txf!("cli.qemu_no_responde_por_su_socket_de", e)),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    // ventana GTK: la maquina se arranco en pausa. La ventana le indica al dispositivo grafico su tamano inicial
    // (640x480) con un segundo de retardo; pasado ese momento se le pide por VNC la resolucion deseada y recien
    // entonces se deja arrancar al invitado, que asi la encuentra desde el principio.
    if c.paused {
        if let Some((w, h)) = c.resolution {
            std::thread::sleep(Duration::from_millis(1600));
            if let Err(e) = rfb::set_desktop_size(&st.f("vnc.sock"), w as u16, h as u16) {
                eprintln!("{}", txf!("cli.aviso_no_se_pudo_fijar_la_resolucion_de", e));
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        Qmp::connect(&st.qmp())?.exec("cont", None)?;
    }
    if let Some((ctl, data)) = c.sensors {
        // proceso aparte que vive mientras viva la maquina
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let log = std::fs::File::create(st.f("sensors.log")).map_err(|e| e.to_string())?;
        orden_interna(exe, st)
            .args(["sensors-serve", &ctl.to_string(), &data.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .process_group(0)
            .spawn()
            .map_err(|e| txf!("cli.servicio_de_sensores", e))?;
    }
    {
        let exe = std::env::current_exe().ok().or_else(|| std::env::args().next().map(std::path::PathBuf::from)).ok_or(tx!("cli.no_se_pudo_localizar_el_ejecutable"))?;
        let _ = orden_interna(exe, st).args(["events-serve"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0).spawn();
    }
    if let Some(modo) = gamepad::Modo::parse(&c.gamepad)? {
        // proceso aparte que conecta los mandos y vigila los que aparezcan; termina con la maquina
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let log = std::fs::File::create(st.f("gamepad.log")).map_err(|e| e.to_string())?;
        let arg = match &modo {
            gamepad::Modo::Auto => "auto".to_string(),
            gamepad::Modo::Ruta(p) => p.clone(),
        };
        orden_interna(exe, st)
            .args(["gamepad-serve", &arg])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .process_group(0)
            .spawn()
            .map_err(|e| txf!("cli.mandos", e))?;
    }
    if c.display == "window" {
        abrir_ventana(st, c.resolution.unwrap_or(TAMANO_VENTANA))?;
    }
    Ok(())
}

/// Deshace un arranque que fallo a medias (`lanzar_procesos`): termina los servicios de archivos anotados en helpers.pid y,
/// si QEMU llego a escribir su pid, lo mata (SIGKILL: una maquina que no termino de arrancar no tiene nada que guardar) y
/// borra su pid y su espacio de pids. Los servicios internos (sensores, eventos, mandos, ventana) terminan solos al terminar
/// QEMU.
fn deshacer_arranque(st: &State) {
    kill_helpers(st);
    if let Some(pid) = st.pid() {
        vm::signal(pid, 9);
        // un instante para que suelte lo que tenga (el CID de vsock, los sockets); `pid` lo olvida en cuanto termina
        let t0 = Instant::now();
        while st.pid().is_some() && t0.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    for f in ["pid", "pid-start", "pid-ns"] {
        let _ = std::fs::remove_file(st.f(f));
    }
}

/// Tamano de la ventana propia sin --resolution ni pantalla.resolucion en `config`.
const TAMANO_VENTANA: (u32, u32) = (720, 1280);

/// Abre la ventana propia de la maquina: un proceso `window-serve` aparte (en su propio grupo de procesos), que termina
/// cuando QEMU cierra la pantalla. Lo hace `start --display window` y lo repite `weft window` (o `launch`) si la ventana ya
/// no esta. El registro de la ventana anterior se conserva como window.log.1; el pid queda en window.pid (`restart` espera
/// a que la ventana anterior termine antes de abrir otra).
fn abrir_ventana(st: &State, (w, h): (u32, u32)) -> Result<std::process::Child, String> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if let Err(e) = vm::rotar_registro(&st.dir.join("window.log")) {
        eprintln!("{}", txf!("cli.aviso_no_se_pudo_conservar_el_registro", e));
    }
    let log = crear_nuevo(&st.dir.join("window.log")).map_err(|e| format!("window.log: {}", e))?;
    let hijo = orden_interna(exe, st)
        .args(["window-serve", &w.to_string(), &h.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .process_group(0)
        .spawn()
        .map_err(|e| format!("ventana: {}", e))?;
    if let Ok(mut f) = crear_nuevo(&st.dir.join("window.pid")) {
        let _ = writeln!(f, "{}", hijo.id());
    }
    Ok(hijo)
}

/// Crea `ruta` de nuevo (borra lo que hubiera) sin seguir un enlace simbolico que alguien dejara en su lugar: con
/// O_CREAT|O_EXCL, si en ese instante aparece otra cosa con ese nombre, falla en vez de escribir a traves de ella.
fn crear_nuevo(ruta: &std::path::Path) -> std::io::Result<std::fs::File> {
    match std::fs::remove_file(ruta) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    std::fs::OpenOptions::new().write(true).create_new(true).open(ruta)
}

/// Orden interna de weft sobre la misma maquina (`weft --state-dir B --name N ...`) con las carpetas que resolvio este
/// proceso en el entorno del hijo (WEFT_DATA_DIR, WEFT_CONFIG_DIR...; ver `rutas::Rutas::variables`): asi el hijo, y las
/// ordenes que la ventana lanza a su vez con solo `--state-dir`, resuelven la misma configuracion y cache que el padre.
fn orden_interna(exe: std::path::PathBuf, st: &State) -> Command {
    let mut c = Command::new(exe);
    c.args(["--state-dir", &st.dir.parent().unwrap().to_string_lossy(), "--name", &st.dir.file_name().unwrap().to_string_lossy()]);
    for (var, valor) in rutas::actual().variables() {
        c.env(var, valor);
    }
    c
}

/// Imagen elegida para arrancar: carpeta y perfil.
struct Elegida {
    id: String,
    carpeta: std::path::PathBuf,
    perfil: perfil::Perfil,
}

/// `x` es el id de una imagen instalada o la ruta de una carpeta de imagen (que se usa tal cual, sin instalarla; su perfil se
/// detecta o se fuerza con --profile).
fn elegir_imagen(r: &rutas::Rutas, cat: &perfil::Catalogo, x: &str, perfil_forzado: Option<&str>) -> Result<Elegida, String> {
    if let Some(i) = catalogo::buscar_instalada(r, cat, x) {
        let p = match perfil_forzado {
            Some(f) => cat.buscar(f).ok_or_else(|| txf!("catalogo.no_existe_el_perfil_image_profiles", format!("{:?}", f)))?,
            None => catalogo::perfil_de(cat, &i)?,
        };
        return Ok(Elegida { id: i.id, carpeta: i.carpeta, perfil: p.clone() });
    }
    let dir = std::path::Path::new(x);
    if dir.is_dir() {
        let h = catalogo::hechos_de_carpeta(dir, None);
        let pid = match catalogo::detectar(cat, &h, perfil_forzado) {
            catalogo::Deteccion::Elegido(p) => p,
            catalogo::Deteccion::Empate(ps) => return Err(txf!("cli.la_carpeta_encaja_con_varios_perfiles", ps.join(", "))),
            catalogo::Deteccion::Ninguno(m) => return Err(m),
        };
        let canon = std::fs::canonicalize(dir).map_err(|e| format!("{}: {}", x, e))?;
        // id estable para la cache: el nombre de la carpeta y un resumen de su ruta
        let hash = canon.to_string_lossy().bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
        let nombre = canon.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let id = format!("dir-{}-{:08x}", catalogo::id_por_defecto("", None, &nombre).trim_start_matches('-'), hash as u32);
        return Ok(Elegida { id, carpeta: canon, perfil: cat.buscar(&pid).unwrap().clone() });
    }
    let inst = catalogo::listar(r, cat);
    Err(txf!("cli.no_es_una_imagen_instalada_ni_una_2", format!("{:?}", x), if inst.is_empty() { tx!("cli.no_hay_imagenes_instaladas_image_add").to_string() } else { txf!("cli.instaladas", inst.iter().map(|i| i.id.as_str()).collect::<Vec<_>>().join(", ")) }))
}

/// ¿Puede este proceso abrir vhost-vsock y crear sockets vsock? (Flatpak bloquea AF_VSOCK con seccomp.)
fn vsock_utilizable() -> bool {
    vsock_no_motivo().is_empty()
}

fn vsock_no_motivo() -> String {
    if rutas::en_flatpak() {
        return tx!("cli.dentro_de_flatpak_no_se_permite_af_vsock").into();
    }
    let ruta = std::ffi::CString::new("/dev/vhost-vsock").unwrap();
    extern "C" {
        fn access(path: *const std::os::raw::c_char, mode: i32) -> i32;
    }
    if unsafe { access(ruta.as_ptr(), 6) } != 0 {
        return tx!("cli.no_hay_acceso_a_dev_vhost_vsock").into();
    }
    String::new()
}

/// Primer puerto de 127.0.0.1 libre desde `desde`.
fn puerto_libre(desde: u16) -> Option<u16> {
    (desde..desde.saturating_add(200)).find(|p| std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok())
}

/// ¿Ofrece este QEMU el dispositivo `dev`? (`qemu -device help`, con las bibliotecas propias si las hay)
fn qemu_ofrece(qemu: &str, lib: Option<&str>, dev: &str) -> bool {
    let mut cmd = Command::new(qemu);
    cmd.args(["-device", "help"]).stdin(Stdio::null());
    if let Some(l) = lib {
        let prev = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        cmd.env("LD_LIBRARY_PATH", if prev.is_empty() { l.to_string() } else { format!("{}:{}", l, prev) });
    }
    match cmd.output() {
        Ok(o) => format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)).lines().any(|l| l.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')).any(|w| w == dev)),
        Err(_) => false,
    }
}

/// `config list | get CLAVE | set CLAVE VALOR | reset [CLAVE] | path`: la configuracion del estado (archivo `config`).
fn config_cmd(st: &State, a: &mut Args) -> Result<i32, String> {
    use config::{clave, Config};
    let mut c = Config::cargar(&st.dir);
    for av in &c.avisos {
        eprintln!("{}", txf!("cli.aviso", av));
    }
    let sub = a.val("config")?;
    let guardar = |c: &Config| c.guardar(&st.dir).map_err(|e| txf!("cli.config_no_se_pudo_guardar", Config::ruta(&st.dir).display(), e));
    match sub.as_str() {
        "list" => {
            print!("{}", c.listado());
            Ok(0)
        }
        "get" => {
            let k = a.val("config get")?;
            clave(&k).ok_or_else(|| txf!("config.clave_desconocida_config_list", k))?;
            println!("{}", c.get(&k));
            Ok(0)
        }
        "set" => {
            let k = a.val("config set")?;
            let v = a.val("config set")?;
            let norm = c.set(&k, &v)?;
            guardar(&c)?;
            println!("{}={}", k, norm);
            if clave(&k).is_some_and(|d| d.arranque) {
                println!("{}", tx!("cli.se_aplica_al_proximo_arranque_de_la"));
            }
            Ok(0)
        }
        "reset" => {
            match a.next() {
                Some(k) => {
                    clave(&k).ok_or_else(|| txf!("config.clave_desconocida_config_list", k))?;
                    c.reset(Some(&k));
                    println!("{}={}", k, c.get(&k));
                }
                None => {
                    c.reset(None);
                    println!("{}", tx!("cli.configuracion_restablecida_a_los_valores"));
                }
            }
            guardar(&c)?;
            Ok(0)
        }
        "path" => {
            println!("{}", Config::ruta(&st.dir).display());
            Ok(0)
        }
        x => Err(txf!("cli.config_subcomando_desconocido_list_get", x)),
    }
}

/// Aviso de que, dentro de Flatpak, la orden se queda en primer plano: el sandbox (y con el QEMU) termina con este proceso.
const AVISO_FLATPAK_PRIMER_PLANO: &str = clave!("cli.aviso_dentro_de_flatpak_la_maquina_vive");

/// `start` desde la linea de ordenes. Fuera de Flatpak vuelve en cuanto la maquina arranca; dentro, volver terminaria el
/// sandbox y la maquina con el, asi que se queda hasta que la maquina se apague y su ventana se cierre (como `launch`).
fn start_cli(st: &State, a: &mut Args) -> Result<i32, String> {
    let c = start(st, a)?;
    if c == 0 && rutas::en_flatpak() && st.running() {
        eprintln!("{}", textos::texto(AVISO_FLATPAK_PRIMER_PLANO));
        esperar_maquina_y_ventana(st, Duration::from_millis(500));
    }
    Ok(c)
}

/// `launch [--no-wait] [OPCIONES DE start]`: ver lanzar.rs. Lo que dice al fallar queda tambien en el registro launch.log
/// de la maquina: abierta desde el icono no hay terminal donde verlo.
fn launch(st: &State, a: &mut Args) -> Result<i32, String> {
    let r = launch_en_primer_plano(st, a);
    if let Err(e) = &r {
        let (rutas, _, _) = contexto(st);
        let _ = lanzar::registrar(&lanzar::ruta_registro(&rutas, &rutas::maquina_de(&st.dir)), &format!("weft: {}", e));
    }
    r
}

fn launch_en_primer_plano(st: &State, a: &mut Args) -> Result<i32, String> {
    let mut dado: Vec<String> = std::iter::from_fn(|| a.next()).collect();
    let mut esperar = !dado.iter().any(|x| x == "--no-wait");
    dado.retain(|x| x != "--no-wait");
    if !esperar && rutas::en_flatpak() {
        // volver terminaria el sandbox y la maquina con el (ver `start_cli`)
        eprintln!("{}", textos::texto(AVISO_FLATPAK_PRIMER_PLANO));
        esperar = true;
    }
    if st.running() {
        // en marcha pero sin ventana (se cerro el proceso o se cayo): se vuelve a abrir, como `weft window`
        if decidir_ventana(true, con_pantalla_propia(st), ventana_viva(st)) == Ventana::Abrir {
            let hijo = abrir_ventana(st, tamano_de_la_ventana(st))?;
            println!("{}", txf!("cli.la_maquina_ya_estaba_en_marcha_sin", hijo.id()));
            if esperar {
                esperar_maquina_y_ventana(st, Duration::from_millis(500));
            }
            return Ok(0);
        }
        println!("{}", tx!("cli.la_maquina_ya_esta_en_marcha_status"));
        return Ok(0);
    }
    let (r, cfg, cat) = contexto(st);
    let id = if dado.iter().any(|x| x == "--image") {
        None
    } else {
        match catalogo::id_de_maquina(&cfg, &catalogo::listar(&r, &cat)) {
            Ok(id) => Some(id),
            Err(e) => {
                if catalogo::listar(&r, &cat).is_empty() {
                    let t = lanzar::mensaje_sin_imagen(lanzar::id_flatpak());
                    eprint!("{}", t);
                    // abierta desde el icono (sin terminal) no se veria nada: queda en launch.log y, si hay sesion de
                    // escritorio, en una notificacion (si no se puede mandar, no pasa nada: es un extra)
                    let registro = lanzar::ruta_registro(&r, &rutas::maquina_de(&st.dir));
                    if let Err(e) = lanzar::registrar(&registro, &t) {
                        eprintln!("{}", txf!("cli.aviso_no_se_pudo_escribir_el_registro_de", e));
                    }
                    // (la ruta es del usuario: no pasa por textos::limpiar)
                    let cuerpo = txf!("cli.mas_detalles_en", textos::limpiar(&lanzar::cuerpo_notificacion(&t, 3)), registro.display());
                    let _ = dbus::notificar(tx!("cli.falta_una_imagen_de_android"), &cuerpo);
                    return Ok(1);
                }
                return Err(e);
            }
        }
    };
    let rt = std::env::var("XDG_RUNTIME_DIR").unwrap_or_default();
    let audio = lanzar::audio_por_defecto(&|n| !rt.is_empty() && std::path::Path::new(&rt).join(n).exists());
    let (recursos, _) = dispositivo::recursos_de_maquina(&cfg, &rutas::actual().con_claves(&cfg.get("dir.datos"), &cfg.get("dir.cache")));
    let args = lanzar::argumentos_de_arranque(&dado, id.as_deref().unwrap_or(""), &cfg, recursos, audio);
    // sin id (porque dio --image) el argumento --image ya viene en `dado`; con id lo anade lanzar
    let args: Vec<String> = if id.is_none() { args.into_iter().filter(|x| !x.is_empty()).collect() } else { args };
    let c = start(st, &mut Args { v: args, i: 0 })?;
    if c != 0 || !esperar {
        return Ok(c);
    }
    esperar_maquina_y_ventana(st, Duration::from_millis(500));
    Ok(0)
}

/// `launch` se queda en primer plano mientras viva la maquina Y mientras viva su ventana: dentro de Flatpak el sandbox
/// termina con este proceso, y con el la ventana, que tras una caida sigue abierta con su aviso (y anotando
/// ultimo-fallo.txt) hasta que se pulsa Cerrar. La ventana se sabe viva por su cerrojo (`ventana_viva`), que suelta al
/// terminar de verdad. `cada`: cada cuanto se mira.
fn esperar_maquina_y_ventana(st: &State, cada: Duration) {
    while st.running() || ventana_viva(st).is_some() {
        std::thread::sleep(cada);
    }
}

/// Que hacer ante `weft window` (y `launch` con la maquina en marcha).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ventana {
    /// la maquina no esta en marcha: no hay pantalla a la que conectar
    MaquinaParada,
    /// ya hay una ventana viva (con su pid si este proceso lo ve)
    YaHay(Option<i32>),
    /// la maquina se arranco sin la pantalla de la ventana propia (--display window): no se puede abrir
    SinPantallaPropia,
    /// en marcha, con pantalla propia y sin ventana: se abre
    Abrir,
}

/// Decide `Ventana`. `con_pantalla_propia`: la maquina se arranco con --display window (o no se sabe); `viva`: la ventana
/// que hay, si hay una (ver `ventana_viva`). Pura.
fn decidir_ventana(en_marcha: bool, con_pantalla_propia: bool, viva: Option<Option<i32>>) -> Ventana {
    if !en_marcha {
        Ventana::MaquinaParada
    } else if let Some(pid) = viva {
        Ventana::YaHay(pid)
    } else if !con_pantalla_propia {
        Ventana::SinPantallaPropia
    } else {
        Ventana::Abrir
    }
}

/// ¿Se arranco la maquina con la ventana propia? Lo dice la linea de arranque guardada (`start-args`); sin ella (maquina
/// de una version anterior) no se sabe y se intenta.
fn con_pantalla_propia(st: &State) -> bool {
    reinicio::Arranque::leer(&st.dir).map_or(true, |a| a.con_ventana())
}

/// Tamano con que `start` abrio la ventana: el --resolution de la linea de arranque (`args`, los de `start`; el ultimo
/// manda, como en `start`, y lo que va tras `--` es de QEMU), si no la resolucion de `config` y si no la de siempre. Pura.
fn tamano_ventana(args: &[String], de_config: Option<(u32, u32)>) -> (u32, u32) {
    let propios = args.split(|x| x == "--").next().unwrap_or(&[]);
    propios.windows(2).rev().find(|w| w[0] == "--resolution").and_then(|w| resolucion_de_arranque(&w[1]).ok()).or(de_config).unwrap_or(TAMANO_VENTANA)
}

/// `tamano_ventana` de la maquina del estado.
fn tamano_de_la_ventana(st: &State) -> (u32, u32) {
    let args = reinicio::Arranque::leer(&st.dir).map(|a| a.args).unwrap_or_default();
    tamano_ventana(&args, config::Config::cargar(&st.dir).resolucion())
}

extern "C" {
    fn flock(fd: i32, operacion: i32) -> i32;
}
const LOCK_EX: i32 = 2;
const LOCK_NB: i32 = 4;

/// Cerrojo de la ventana (window.lock): `window-serve` lo toma y lo tiene mientras vive. Un cerrojo de archivo (flock) lo
/// suelta el sistema al terminar el proceso, aunque muera de golpe o quede zombi, y lo ve cualquier proceso que abra el
/// archivo, aunque este en otro espacio de pids (otra instancia de Flatpak). Ok(Some) tomado; Ok(None) si lo sigue teniendo
/// otro tras `espera` (se reintenta: `cerrojo_tomado` lo toma un instante para mirarlo, y en ese instante no se podria);
/// Err si no se pudo abrir el archivo.
fn tomar_cerrojo_ventana(st: &State, espera: Duration) -> Result<Option<std::fs::File>, String> {
    use std::os::unix::io::AsRawFd;
    let ruta = st.f("window.lock");
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&ruta).map_err(|e| format!("{}: {}", ruta, e))?;
    let t0 = Instant::now();
    loop {
        if unsafe { flock(f.as_raw_fd(), LOCK_EX | LOCK_NB) } == 0 {
            return Ok(Some(f));
        }
        if t0.elapsed() >= espera {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Una ventana de una ejecucion anterior que siga viva (p. ej. con el aviso de fin abierto tras `weft kill`) se cierra antes
/// de arrancar de nuevo: tiene tomado window.lock, y la ventana nueva no podria tomarlo. SIGKILL, como en `restart` (SIGTERM
/// equivaldria a cerrarla). Solo si su pid se ve y sigue siendo un weft distinto de este proceso.
fn cerrar_ventana_anterior(st: &State) {
    let Some(Some(pid)) = ventana_viva(st) else { return };
    if pid == std::process::id() as i32 || !vm::alive_de(pid, "weft") {
        return;
    }
    eprintln!("{}", txf!("cli.se_cierra_la_ventana_de_la_ejecucion", pid));
    vm::signal(pid, 9);
    let t0 = Instant::now();
    while ventana_viva(st).is_some() && t0.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// ¿Tiene alguien tomado el cerrojo de `ruta`? None si el archivo no existe o no se puede abrir.
fn cerrojo_tomado(ruta: &std::path::Path) -> Option<bool> {
    use std::os::unix::io::AsRawFd;
    let f = std::fs::OpenOptions::new().write(true).open(ruta).ok()?;
    // si se puede tomar, nadie lo tenia; se suelta al cerrar `f`
    Some(unsafe { flock(f.as_raw_fd(), LOCK_EX | LOCK_NB) } != 0)
}

/// La ventana propia viva de la maquina, si hay una: Some(pid, si este proceso lo ve). Manda el cerrojo de window.lock (ver
/// `tomar_cerrojo_ventana`); sin el (ventana de una version anterior), el pid de window.pid si sigue siendo un weft.
fn ventana_viva(st: &State) -> Option<Option<i32>> {
    let pid = std::fs::read_to_string(st.f("window.pid")).ok().and_then(|t| t.trim().parse::<i32>().ok()).filter(|p| vm::alive_de(*p, "weft"));
    match cerrojo_tomado(&st.dir.join("window.lock")) {
        Some(true) => Some(pid),
        Some(false) => None,
        None => pid.map(Some),
    }
}

/// `window [--wait]`: vuelve a abrir la ventana propia de una maquina en marcha si ya no tiene ninguna.
fn window_cmd(st: &State, a: &mut Args) -> Result<i32, String> {
    let mut esperar = false;
    while let Some(f) = a.next() {
        match f.as_str() {
            "--wait" => esperar = true,
            x => return Err(txf!("cli.window_opcion_desconocida_wait", x)),
        }
    }
    match decidir_ventana(st.running(), con_pantalla_propia(st), ventana_viva(st)) {
        Ventana::MaquinaParada => Err(tx!("cli.la_maquina_no_esta_en_marcha_launch_o").into()),
        Ventana::YaHay(Some(pid)) => {
            println!("{}", txf!("cli.ya_hay_una_ventana_pid", pid));
            Ok(0)
        }
        Ventana::YaHay(None) => {
            println!("{}", tx!("cli.ya_hay_una_ventana"));
            Ok(0)
        }
        Ventana::SinPantallaPropia => Err(tx!("cli.la_maquina_se_arranco_sin_la_ventana").into()),
        Ventana::Abrir => {
            let mut hijo = abrir_ventana(st, tamano_de_la_ventana(st))?;
            println!("{}", txf!("cli.ventana_abierta_pid", hijo.id()));
            // dentro de Flatpak el sandbox termina con este proceso (y la ventana con el): hay que quedarse
            if esperar || rutas::en_flatpak() {
                let _ = hijo.wait();
            }
            Ok(0)
        }
    }
}

/// Contenido de ultimo-fallo.txt: cuando, la causa que dio la ventana, las ultimas lineas de qemu.log y los ultimos eventos
/// de QEMU. Pura.
fn texto_ultimo_fallo(fecha: &str, maquina: &str, causa: &str, qemu_log: &str, eventos: &str) -> String {
    let ultimas = |t: &str, n: usize| -> String {
        let l: Vec<&str> = t.lines().collect();
        let mut s = l[l.len().saturating_sub(n)..].join("\n");
        if s.is_empty() {
            s.push_str("(vacío)");
        }
        s
    };
    txf!("cli.la_ventana_de_la_maquina_termino_ultimas", fecha, maquina, causa, ultimas(qemu_log, 30), ultimas(eventos, 6))
}

/// La maquina se cayo sin que nadie lo pidiera (la ventana lo decide con el mismo criterio con que muestra su aviso: ver
/// `window::Fallo`): deja la causa y lo ultimo que dijo QEMU en esta ejecucion en <registros>/machines/NOMBRE/ultimo-fallo.txt,
/// que sobrevive al siguiente arranque.
fn anotar_ultimo_fallo(st: &State, f: &window::Fallo) {
    let maquina = rutas::maquina_de(&st.dir);
    let ruta = rutas::actual().registros_maquina(&maquina).join(rutas::nombres::ULTIMO_FALLO);
    let texto = texto_ultimo_fallo(&lanzar::fecha_utc(lanzar::ahora()), &maquina, &f.causa, &f.qemu_log, &f.eventos);
    let r = ruta.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&ruta, texto));
    match r {
        Ok(()) => eprintln!("ventana: la causa y las ultimas lineas de QEMU quedan en {}", ruta.display()),
        Err(e) => eprintln!("ventana: no se pudo escribir {}: {}", ruta.display(), e),
    }
}

/// `--resolution ANCHOxALTO`: cada lado entre los limites de la pantalla (los mismos que valen para `config` y para la orden
/// `resolution`: pantalla::LADO_MIN..=LADO_MAX). El ancho que no es multiplo de 8 se admite (Linux lo redondea; `start`
/// avisa). Pura.
fn resolucion_de_arranque(v: &str) -> Result<(u32, u32), String> {
    let (w, h) = v.split_once('x').ok_or(tx!("cli.resolution_se_espera_anchoxalto"))?;
    let lado = |t: &str, n: &str| -> Result<u32, String> {
        let x: u32 = t.parse().map_err(|_| txf!("cli.resolution_no_valido", n))?;
        if !(pantalla::LADO_MIN..=pantalla::LADO_MAX).contains(&x) {
            return Err(txf!("cli.resolution_fuera_de_rango", n, pantalla::LADO_MIN, pantalla::LADO_MAX, x));
        }
        Ok(x)
    };
    Ok((lado(w, "ancho")?, lado(h, "alto")?))
}

/// `--adb-port P`: un puerto TCP de 1 a 65535 (0 no es un puerto: el reenvio de QEMU no lo admitiria). Pura.
fn puerto_adb(v: &str) -> Result<u16, String> {
    match v.parse::<u16>() {
        Ok(p) if p > 0 => Ok(p),
        _ => Err(txf!("cli.adb_port_se_espera_un_puerto_de_1_a", format!("{:?}", v))),
    }
}

fn progreso(t: &str) {
    eprintln!("... {}", t);
}

/// Opciones de `image` y `disk`: --dir/--image CARPETA, --out/--disk DISCO, --data, --yes, --id, --profile y
/// los argumentos sueltos (posicionales).
#[derive(Default)]
struct OpcionesAlmacen {
    carpeta: Option<String>,
    disco: Option<String>,
    datos: Option<String>,
    id: Option<String>,
    perfil: Option<String>,
    si: bool,
    posicionales: Vec<String>,
}

fn opciones_almacen(a: &mut Args, cmd: &str) -> Result<OpcionesAlmacen, String> {
    let mut o = OpcionesAlmacen::default();
    while let Some(f) = a.next() {
        match f.as_str() {
            "--dir" | "--image" => o.carpeta = Some(a.val(&f)?),
            "--out" | "--disk" => o.disco = Some(a.val(&f)?),
            "--data" => o.datos = Some(a.val(&f)?),
            "--id" => o.id = Some(a.val(&f)?),
            "--profile" => o.perfil = Some(a.val(&f)?),
            "--yes" => o.si = true,
            x if x.starts_with("--") => return Err(txf!("cli.opcion_desconocida", cmd, x)),
            _ => o.posicionales.push(f),
        }
    }
    Ok(o)
}

/// Rutas de la maquina del estado con las claves de su config (`dir.datos`, `dir.cache`) y su catalogo de perfiles.
fn contexto(st: &State) -> (rutas::Rutas, config::Config, perfil::Catalogo) {
    let cfg = config::Config::cargar(&st.dir);
    let r = rutas::actual().con_claves(&cfg.get("dir.datos"), &cfg.get("dir.cache"));
    let cat = catalogo::cargar_catalogo(&r);
    (r, cfg, cat)
}

/// `image list|path|add|info|remove|check|profiles|use|current` (sin descargas).
fn image_cmd(st: &State, a: &mut Args) -> Result<i32, String> {
    let sub = a.val("image")?;
    let o = opciones_almacen(a, "image")?;
    let (r, mut cfg, cat) = contexto(st);
    for (f, e) in &cat.rechazados {
        eprintln!("{}", txf!("cli.aviso_perfil_rechazado", f, e));
    }
    let maquina = rutas::maquina_de(&st.dir);
    let instaladas = catalogo::listar(&r, &cat);
    let actual = catalogo::id_de_maquina(&cfg, &instaladas).ok();
    match sub.as_str() {
        "list" => {
            if let Some(dir) = &o.carpeta {
                // modo de estado propio: una carpeta de imagen suelta
                let carpeta = std::path::PathBuf::from(dir);
                print!("{}", imagen::listado(&imagen::estado_imagen(&carpeta)));
            } else {
                print!("{}", catalogo::listado(&r, &cat, actual.as_deref()));
            }
            Ok(0)
        }
        "fetch" => Err(tx!("cli.image_fetch_ya_no_existe_no_descarga").into()),
        "add" => {
            if o.posicionales.len() != 1 {
                return Err(tx!("cli.uso_image_add_zip_carpeta_id_id_profile").into());
            }
            let origen = o.posicionales[0].clone();
            let (id, t) = catalogo::instalar(&r, &cat, &catalogo::OpcionesAdd { origen, id: o.id.clone(), perfil: o.perfil.clone() }, &progreso)?;
            println!("{}", t);
            println!("id: {}", id);
            // la maquina en `auto` pasa a usar la imagen nueva: con dos imagenes instaladas y `auto`, el siguiente
            // arranque fallaria ("hay varias imagenes instaladas")
            // (si esta en marcha, la imagen con que arranco: `image-id` del estado)
            let instalada = |x: &str| instaladas.iter().any(|i| i.id == x);
            let ahora = std::fs::read_to_string(st.f("image-id")).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
            let en_marcha = if st.running() { Some(ahora.as_deref()) } else { None };
            let fijar = |cfg: &mut config::Config, v: &str| -> Result<(), String> {
                cfg.set("image.id", v)?;
                cfg.guardar(&st.dir).map_err(|e| format!("config: {}", e))
            };
            match catalogo::tras_instalar(&cfg.get("image.id"), &id, &instalada, en_marcha) {
                catalogo::TrasInstalar::Fijar => {
                    fijar(&mut cfg, &id)?;
                    println!("{}", txf!("cli.la_maquina_usara_esta_imagen_image_id", maquina, id));
                }
                catalogo::TrasInstalar::YaLaUsa => println!("{}", txf!("cli.la_maquina_ya_usa_esta_imagen", maquina)),
                catalogo::TrasInstalar::UsaOtra(otra) => println!("{}", txf!("cli.la_maquina_sigue_usando_la_imagen_para", maquina, otra, id)),
                catalogo::TrasInstalar::MantenerLaDeAhora(otra) => {
                    fijar(&mut cfg, &otra)?;
                    println!("{}", txf!("cli.la_maquina_esta_en_marcha_con_la_imagen", maquina, otra, otra, id));
                }
                catalogo::TrasInstalar::EnMarcha => println!("{}", txf!("cli.la_maquina_esta_en_marcha_su_imagen_no", maquina, id)),
            }
            Ok(0)
        }
        "path" => {
            if let Some(dir) = &o.carpeta {
                let carpeta = std::path::PathBuf::from(dir);
                let p = std::fs::canonicalize(&carpeta).unwrap_or_else(|_| std::env::current_dir().map(|d| d.join(&carpeta)).unwrap_or(carpeta));
                println!("{}", p.display());
            } else {
                let id = o.posicionales.first().cloned().or(actual).ok_or(tx!("cli.no_hay_imagen_elegida_ni_instalada_image"))?;
                println!("{}", r.imagen(&id).display());
            }
            Ok(0)
        }
        "current" => {
            let id = catalogo::id_de_maquina(&cfg, &instaladas)?;
            println!("{}", id);
            Ok(0)
        }
        "info" => {
            let x = o.posicionales.first().cloned().or(actual).ok_or(tx!("cli.image_info_indica_un_id_image_list"))?;
            match catalogo::buscar_instalada(&r, &cat, &x) {
                Some(i) => print!("{}", catalogo::describir(&r, &cat, &i)),
                None => {
                    // una carpeta cualquiera: lo que se sabe de ella
                    let h = catalogo::hechos_de_carpeta(std::path::Path::new(&x), None);
                    if h.archivos.is_empty() {
                        return Err(txf!("cli.no_es_una_imagen_instalada_ni_una", format!("{:?}", x)));
                    }
                    print!("{}", catalogo::informe_check(&cat, &h, o.perfil.as_deref()).1);
                }
            }
            Ok(0)
        }
        "check" => {
            let x = o.posicionales.first().cloned().ok_or(tx!("cli.uso_image_check_zip_carpeta_profile_id"))?;
            let h = match catalogo::fuente_de(&x)? {
                catalogo::Fuente::Carpeta(d) => catalogo::hechos_de_carpeta(&d, None),
                catalogo::Fuente::Zip(z) => {
                    let tmp = r.descargas().join(format!(".check-{}", std::process::id()));
                    let h = catalogo::hechos_de_zip(&z, &tmp);
                    let _ = std::fs::remove_dir_all(&tmp);
                    h?
                }
            };
            let (ok, t) = catalogo::informe_check(&cat, &h, o.perfil.as_deref());
            print!("{}", t);
            Ok(if ok { 0 } else { 1 })
        }
        "remove" => {
            let id = o.posicionales.first().cloned().ok_or(tx!("cli.uso_image_remove_id_yes"))?;
            println!("{}", catalogo::quitar(&r, &cat, &id, o.si)?);
            Ok(0)
        }
        "profiles" => {
            print!("{}", catalogo::listado_perfiles(&cat, &r));
            Ok(0)
        }
        "use" => {
            let id = o.posicionales.first().cloned().ok_or(tx!("cli.uso_image_use_id"))?;
            let i = catalogo::buscar_instalada(&r, &cat, &id).ok_or_else(|| txf!("catalogo.no_hay_una_imagen_instalada_con_el_id", format!("{:?}", id)))?;
            if !i.completa {
                return Err(txf!("cli.la_imagen_esta_incompleta_faltan", format!("{:?}", id), i.faltan.join(", ")));
            }
            let antes = cfg.get("image.id");
            cfg.set("image.id", &id)?;
            cfg.guardar(&st.dir).map_err(|e| format!("config: {}", e))?;
            let disco = r.disco(&maquina, &id);
            println!("{}", txf!("cli.la_maquina_usara_la_imagen_desde_el", maquina, id, antes));
            println!("{}", if disco.exists() { txf!("cli.conserva_su_disco_de_esa_imagen_apps_y", disco.display()) } else { tx!("cli.no_tiene_disco_para_esa_imagen_se_crea").to_string() });
            if st.running() {
                println!("{}", tx!("cli.la_maquina_esta_en_marcha_apagala_y"));
            }
            Ok(0)
        }
        x => Err(txf!("cli.image_subcomando_desconocido_list_add", x)),
    }
}

/// Base de copia en escritura para el disco de una maquina con la imagen `id` y el tamano de datos `datos`
/// (`<datos>/bases/ID/DATOS.img`, ver cow.rs), o None si `disk.cow` esta en no (disco completo, como antes).
fn base_de_copia(r: &rutas::Rutas, cfg: &config::Config, id: &str, datos: &imagen::Datos) -> Option<std::path::PathBuf> {
    cfg.bool("disk.cow").then(|| r.base_disco(id, &datos.texto()))
}

/// `factory-reset [--yes]`: volver a fabrica el disco de la maquina para su imagen (lo mismo que `disk reset`).
fn factory_reset_cmd(st: &State, a: &mut Args) -> Result<i32, String> {
    let mut v = vec!["reset".to_string()];
    while let Some(f) = a.next() {
        match f.as_str() {
            "--yes" => v.push(f),
            "--image" | "--data" => {
                let x = a.val(&f)?;
                v.extend([f, x]);
            }
            x => return Err(txf!("cli.factory_reset_opcion_desconocida_uso", x)),
        }
    }
    disk_cmd(st, &mut Args { v, i: 0 })
}

/// `disk create | status | reset`.
fn disk_cmd(st: &State, a: &mut Args) -> Result<i32, String> {
    let sub = a.val("disk")?;
    let o = opciones_almacen(a, "disk")?;
    let (r, cfg, cat) = contexto(st);
    let maquina = rutas::maquina_de(&st.dir);
    // imagen: --image (id o carpeta), o la de la maquina
    let x = match &o.carpeta {
        Some(x) => x.clone(),
        None => catalogo::id_de_maquina(&cfg, &catalogo::listar(&r, &cat))?,
    };
    let e = elegir_imagen(&r, &cat, &x, o.perfil.as_deref())?;
    let disco = match &o.disco {
        Some(d) => std::path::PathBuf::from(d),
        None => r.disco(&maquina, &e.id),
    };
    // las bases de copia en escritura son de solo lectura y las comparten las maquinas: ni create ni reset las tocan
    if let (Ok(d), Ok(b)) = (std::fs::canonicalize(disco.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."))), std::fs::canonicalize(r.datos.join(rutas::nombres::BASES))) {
        if d.starts_with(&b) {
            return Err(txf!("cli.esta_en_la_carpeta_de_las_bases_de_copia", disco.display(), b.display()));
        }
    }
    // opcion > config > defecto
    let datos = imagen::parsear_datos(&o.datos.clone().unwrap_or_else(|| cfg.get("disk.data")))?;
    // copia en escritura (disk.cow) para el disco de la maquina; un disco elegido con --out es completo, como antes
    let base = if o.disco.is_none() { base_de_copia(&r, &cfg, &e.id, &datos) } else { None };
    let crear_dir = |d: &std::path::Path| -> Result<(), String> {
        match d.parent().filter(|p| !p.as_os_str().is_empty()) {
            Some(p) => std::fs::create_dir_all(p).map_err(|e| format!("{}: {}", p.display(), e)),
            None => Ok(()),
        }
    };
    match sub.as_str() {
        "create" => {
            crear_dir(&disco)?;
            let (t, _) = imagen::crear_disco(&e.perfil, &e.carpeta, &datos, &disco, base.as_deref(), &progreso)?;
            println!("{}", t);
            Ok(0)
        }
        "status" => {
            println!("{}", txf!("cli.imagen", e.id, e.carpeta.display()));
            print!("{}", imagen::resumen_disco(&imagen::estado_disco(&disco)));
            println!("{}", txf!("cli.maquina_en_marcha_con_este_estado", if st.running() { tx!("cli.si_no_se_puede_regenerar_el_disco") } else { "no" }));
            println!("{}", txf!("cli.tamano_de_datos_para_un_disco_nuevo_disk", datos.texto()));
            Ok(0)
        }
        "reset" => {
            if !o.si {
                return Err(txf!("cli.disk_reset_borra_el_disco_y_lo_regenera", disco.display(), textos::texto(imagen::AVISO_REGENERAR)));
            }
            crear_dir(&disco)?;
            let t = imagen::regenerar_disco(&e.perfil, &e.carpeta, &datos, &disco, base.as_deref(), st.running(), &progreso)?;
            println!("{}", t);
            Ok(0)
        }
        x => Err(txf!("cli.disk_subcomando_desconocido_create", x)),
    }
}

/// `bridge status | install | remove | restore | check`.
fn bridge_cmd(st: &State, a: &mut Args) -> Result<i32, String> {
    let sub = a.val("bridge")?;
    let (mut cid, mut archivo, mut sin_reinicio, mut limite) = (None, None::<String>, false, puente::LIMITE_ARRANQUE_S);
    while let Some(f) = a.next() {
        match f.as_str() {
            "--cid" => cid = Some(a.num("--cid")?),
            "--file" => archivo = Some(a.val(&f)?),
            "--no-reboot" => sin_reinicio = true,
            "--boot-timeout" => limite = a.num(&f)?,
            x => return Err(txf!("cli.bridge_opcion_desconocida", x)),
        }
    }
    let cid = cid.unwrap_or_else(|| adbcmd::cid_del_estado(st));
    let cache = puente::dir_cache(&st.dir);
    let en_marcha = || if st.running() { Ok(()) } else { Err(tx!("cli.la_maquina_esta_apagada_el_traductor_arm").to_string()) };
    let mut inv = puente::InvitadoAdb::nuevo(cid, cache.clone());
    let terminar = |r: Result<puente::Resultado, puente::ErrorPuente>| -> Result<i32, String> {
        let t = puente::describir(&r);
        let c = puente::codigo(&r);
        if r.is_err() {
            eprintln!("{}", t);
        } else {
            println!("{}", t);
        }
        Ok(c)
    };
    // en install, remove, restore y check, lo que falla antes de tocar nada sale con 1 ("error previo" del instalador vigilado);
    // el 2 queda para "fallo vigilado y restaurado" y el 3 para "la restauracion tambien fallo"
    let previo = |r: Result<i32, String>| -> Result<i32, String> {
        r.or_else(|e| {
            eprintln!("weft: {}", e);
            Ok(1)
        })
    };
    match sub.as_str() {
        "status" => {
            en_marcha()?;
            print!("{}", puente::estado(cid)?.resumen());
            Ok(0)
        }
        "install" => previo((|| {
            // la biblioteca ya descargada se valida antes de tocar nada (y antes de pedir la maquina)
            let p = puente::biblioteca_local(archivo.as_deref(), &cache)?;
            let (n, md5) = puente::validar_archivo(&p).map_err(|e| format!("{}: {}", p.display(), e))?;
            if !textos::interfaz() {
                eprintln!("{}", txf!("cli.se_usara_el_archivo_md5", p.display(), puente::mib(n), md5));
            }
            en_marcha()?;
            eprintln!("{}", textos::texto(puente::AVISO_RIESGO));
            let r = puente::instalar(&mut inv, &p, &md5, !sin_reinicio, limite, &progreso);
            let instalado = matches!(r, Ok(puente::Resultado::Instalado { .. } | puente::Resultado::SinCambios(_) | puente::Resultado::SinVigilar(_)));
            let c = terminar(r)?;
            // lo que el traductor nuevo dice soportar (cpu-features junto a la biblioteca), o nada si no lo trae
            if instalado {
                if let Err(e) = dispositivo::copiar_soportadas(&p, &st.dir) {
                    eprintln!("{}", txf!("cli.aviso", e));
                }
            }
            // con el traductor en su sitio, el perfil de dispositivo de la maquina (cpu.conf que lee el traductor, identidad y
            // pagina); sin perfil elegido no se toca nada (quitar uno es `device remove`)
            let cfg = config::Config::cargar(&st.dir);
            if !instalado || cfg.get("dispositivo.perfil") == dispositivo::NINGUNO {
                return Ok(c);
            }
            let rr = rutas::actual().con_claves(&cfg.get("dir.datos"), &cfg.get("dir.cache"));
            let cat = dispositivo::Catalogo::cargar(Some(&dispositivo::carpeta_usuario(&rr)));
            let deseado = cat.elegido(&cfg)?;
            let mut di = dispositivo::InvitadoAdb::nuevo(cid, rr.cache_de(&st.dir, "device", "dispositivo"));
            let rd = dispositivo::aplicar(&mut di, deseado, !sin_reinicio, dispositivo::LIMITE_ARRANQUE_S, &progreso);
            let t = txf!("compartir.perfil_de_dispositivo", dispositivo::describir(&rd));
            if rd.is_err() {
                eprintln!("{}", t);
            } else {
                println!("{}", t);
            }
            Ok(if c != 0 { c } else { dispositivo::codigo(&rd) })
        })()),
        // sin traductor, o con el de antes (que no se sabe que publicaba): sin lista de extensiones soportadas
        "remove" | "restore" => previo(en_marcha().and_then(|_| {
            let r = if sub == "remove" { puente::quitar(&mut inv, !sin_reinicio, &progreso) } else { puente::restaurar(&mut inv, &progreso) };
            if r.is_ok() {
                let _ = dispositivo::olvidar_soportadas(&st.dir);
            }
            terminar(r)
        })),
        "check" => previo(en_marcha().and_then(|_| terminar(puente::comprobar(&mut inv, limite, &progreso)))),
        x => Err(txf!("cli.bridge_subcomando_desconocido_status", x)),
    }
}

/// `weft device list|show|use|new|set|delete|apply|remove|resources|path`: perfiles de dispositivo (ver dispositivo.rs).
fn device_cmd(st: &State, a: &mut Args) -> Result<i32, String> {
    use dispositivo::{Catalogo, NINGUNO};
    let sub = a.val("device")?;
    let mut cfg = config::Config::cargar(&st.dir);
    let r = rutas::actual().con_claves(&cfg.get("dir.datos"), &cfg.get("dir.cache"));
    let dir = dispositivo::carpeta_usuario(&r);
    let cat = Catalogo::cargar(Some(&dir));
    let avisar = |cat: &Catalogo| {
        for (f, e) in &cat.rechazados {
            eprintln!("{}", txf!("cli.aviso", format!("{}: {}", f, e)));
        }
        for av in &cat.avisos {
            eprintln!("{}", txf!("cli.aviso", av));
        }
    };
    let actual = cfg.get("dispositivo.perfil");
    let guardar = |c: &config::Config| c.guardar(&st.dir).map_err(|e| txf!("cli.config_no_se_pudo_guardar", config::Config::ruta(&st.dir).display(), e));
    match sub.as_str() {
        "list" => {
            avisar(&cat);
            let marca = |id: &str| if id == actual { "*" } else { " " };
            println!("{} {:<24} {}", marca(NINGUNO), NINGUNO, tx!("cli_tec.dispositivo_ninguno"));
            for d in &cat.dispositivos {
                let origen = if d.origen == perfil::Origen::Integrado { tx!("dispositivo.integrado") } else { tx!("dispositivo.del_usuario") };
                println!("{} {:<24} {} ({}, {})", marca(&d.id), d.id, d.nombre, d.cpu_nombre, origen);
            }
            Ok(0)
        }
        "show" => {
            let id = a.next().unwrap_or_else(|| actual.clone());
            if id == NINGUNO {
                println!("{}", tx!("cli_tec.dispositivo_ninguno"));
                return Ok(0);
            }
            let d = cat.buscar(&id).ok_or_else(|| txf!("dispositivo.no_existe", format!("{:?}", id)))?;
            println!("{} - {}", d.id, d.nombre);
            if !d.descripcion.is_empty() {
                println!("{}", d.descripcion);
            }
            for (k, v) in d.resumen() {
                println!("  {}: {}", k, v);
            }
            if !d.desconocidas().is_empty() {
                println!("{}", txf!("cli_tec.dispositivo_desconocidas", d.desconocidas().join(" ")));
            }
            // solo si el traductor instalado publica lo que soporta (su cpu-features, copiado por `bridge install`)
            if let Some(sop) = dispositivo::soportadas_de_maquina(&st.dir) {
                let sin = d.sin_soporte(&sop);
                if !sin.is_empty() {
                    println!("{}", txf!("cli_tec.dispositivo_sin_soporte", sin.join(" ")));
                }
            }
            if let Some(f) = &d.archivo {
                println!("{}", txf!("cli_tec.dispositivo_archivo", f.display()));
            }
            println!("{}", txf!("cli_tec.dispositivo_cpu_conf", dispositivo::CPU_CONF));
            print!("{}", d.cpu_conf());
            println!("{}", tx!("cli_tec.dispositivo_propiedades"));
            for (k, v) in d.propiedades_con_marca() {
                println!("{}={}", k, v);
            }
            Ok(0)
        }
        "use" => {
            let id = a.val("device use")?;
            if id != NINGUNO {
                cat.buscar(&id).ok_or_else(|| txf!("dispositivo.no_existe", format!("{:?}", id)))?;
            }
            let v = cfg.set("dispositivo.perfil", &id)?;
            guardar(&cfg)?;
            println!("dispositivo.perfil={}", v);
            println!("{}", tx!("cli_tec.dispositivo_use_nota"));
            Ok(0)
        }
        "new" => {
            let id = a.val("device new")?;
            let (mut desde, mut nombre) = (None::<String>, None::<String>);
            while let Some(f) = a.next() {
                match f.as_str() {
                    "--from" => desde = Some(a.val(&f)?),
                    "--name" => nombre = Some(a.val(&f)?),
                    x => return Err(txf!("cli.opcion_desconocida_2", x)),
                }
            }
            let desde = desde.unwrap_or_else(|| if actual != NINGUNO && cat.buscar(&actual).is_some() { actual.clone() } else { dispositivo::ID_BASE.to_string() });
            let d = dispositivo::duplicar(&dir, &cat, &desde, &id, nombre.as_deref())?;
            println!("{}", txf!("cli_tec.dispositivo_creado", d.id, desde, d.archivo.as_ref().map(|p| p.display().to_string()).unwrap_or_default()));
            Ok(0)
        }
        "set" => {
            let id = a.val("device set")?;
            let k = a.val("device set")?;
            let v = a.val("device set")?;
            let mut d = cat.buscar(&id).cloned().ok_or_else(|| txf!("dispositivo.no_existe", format!("{:?}", id)))?;
            if !dispositivo::CLAVES.contains(&k.as_str()) || matches!(k.as_str(), "dispositivo.id" | "dispositivo.hereda" | "dispositivo.formato") {
                return Err(txf!("dispositivo.clave_desconocida", k));
            }
            d.fijar(&k, &v)?;
            dispositivo::guardar(&dir, &mut d)?;
            println!("{}={}", k, d.valor(&k));
            Ok(0)
        }
        "delete" => {
            let id = a.val("device delete")?;
            let d = cat.buscar(&id).ok_or_else(|| txf!("dispositivo.no_existe", format!("{:?}", id)))?;
            dispositivo::borrar(d)?;
            println!("{}", txf!("cli_tec.dispositivo_borrado", id));
            if actual == id {
                eprintln!("{}", txf!("cli.aviso", txf!("dispositivo.no_existe", format!("{:?}", id))));
            }
            Ok(0)
        }
        "apply" | "remove" => {
            let (mut cid, mut sin_reinicio, mut limite) = (None, false, dispositivo::LIMITE_ARRANQUE_S);
            while let Some(f) = a.next() {
                match f.as_str() {
                    "--cid" => cid = Some(a.num("--cid")?),
                    "--no-reboot" => sin_reinicio = true,
                    "--boot-timeout" => limite = a.num(&f)?,
                    x => return Err(txf!("cli.opcion_desconocida_2", x)),
                }
            }
            avisar(&cat);
            let deseado = if sub == "remove" { None } else { cat.elegido(&cfg)? };
            if !st.running() {
                eprintln!("weft: {}", tx!("cli.dispositivo_maquina_apagada"));
                return Ok(1);
            }
            let cid = cid.unwrap_or_else(|| adbcmd::cid_del_estado(st));
            let mut inv = dispositivo::InvitadoAdb::nuevo(cid, rutas::actual().cache_de(&st.dir, "device", "dispositivo"));
            let res = dispositivo::aplicar(&mut inv, deseado, !sin_reinicio, limite, &progreso);
            // un `apply` explicito vuelve a permitir la sincronizacion tras el arranque
            let _ = std::fs::remove_file(st.dir.join(dispositivo::ARCHIVO_FALLO));
            let t = dispositivo::describir(&res);
            if res.is_err() {
                eprintln!("{}", t);
            } else {
                println!("{}", t);
            }
            Ok(dispositivo::codigo(&res))
        }
        "resources" => {
            let ((cpus, ram), aviso) = dispositivo::recursos_de_maquina(&cfg, &r);
            if let Some(av) = aviso {
                eprintln!("{}", txf!("cli.aviso", av));
            }
            let v = |n: Option<u32>| n.map_or("auto".to_string(), |n| n.to_string());
            println!("cpus={}\nram={}", v(cpus), v(ram));
            Ok(0)
        }
        "path" => {
            println!("{}", dir.display());
            Ok(0)
        }
        x => Err(txf!("cli.device_subcomando_desconocido", x)),
    }
}

fn root_cmd(st: &State, a: &mut Args) -> Result<i32, String> {
    // `root` a secas (o solo con --cid) sigue siendo `adb root`: adbd como root
    let primero = a.v.get(a.i).cloned();
    if primero.as_deref().map_or(true, |p| p.starts_with("--cid")) {
        let rest: Vec<String> = std::iter::from_fn(|| a.next()).collect();
        return adbcmd::run(st, "root", rest);
    }
    let sub = a.val("root")?;
    let mut cid: Option<u32> = None;
    let (mut ahora, mut gestor, mut reiniciar) = (false, false, false);
    let mut desde: Option<String> = None;
    while let Some(f) = a.next() {
        match f.as_str() {
            "--cid" => cid = Some(a.num("--cid")?),
            "--from" => desde = Some(a.val("--from")?),
            "--now" => ahora = true,
            "--manager" => gestor = true,
            "--reboot" => reiniciar = true,
            x => return Err(txf!("cli.root_opcion_desconocida", x)),
        }
    }
    let cid = cid.unwrap_or_else(|| adbcmd::cid_del_estado(st));
    let mut cfg = config::Config::cargar(&st.dir);
    // carpeta con los archivos ya descargados: --from, root.carpeta de `config` o la subcarpeta root/ de la cache
    let cache = match &desde {
        Some(d) => std::path::PathBuf::from(d),
        None => root::carpeta(&st.dir, &cfg),
    };
    let guardar = |c: &config::Config| c.guardar(&st.dir).map_err(|e| format!("config: {}", e));
    match sub.as_str() {
        "status" => {
            println!("{}", root::proveedor().estado(cid)?.resumen());
            Ok(0)
        }
        "enable" => {
            // el detalle del proveedor es de consola: la interfaz ya lo mostro (generico) al pedir la confirmacion
            if !textos::interfaz() {
                eprintln!("{}", txf!("cli.proveedor_de_root", root::proveedor().nombre_tecnico()));
                for d in root::proveedor().descargas(gestor) {
                    eprintln!("{}", txf!("cli.se_usara_ya_descargado_se_consigue_en_de", d.nombre, root::mib(d.bytes), d.origen, cache.display()));
                }
            }
            eprintln!("{}", textos::texto(root::AVISO_DETECCION));
            let r = root::proveedor().habilitar(cid, &st.dir, &cache, &root::Opciones { ahora, gestor, reiniciar }, &progreso)?;
            cfg.set("root.cargar_al_inicio", "si")?;
            guardar(&cfg)?;
            println!("{}", r);
            Ok(0)
        }
        "disable" => {
            let r = root::proveedor().deshabilitar(cid, &progreso)?;
            cfg.set("root.cargar_al_inicio", "no")?;
            guardar(&cfg)?;
            println!("{}", r);
            Ok(0)
        }
        "install-manager" => {
            println!("{}", root::proveedor().instalar_gestor(cid, &cache, &progreso)?);
            Ok(0)
        }
        "ensure" => {
            println!("{}", root::proveedor().asegurar(cid, &st.dir, &cfg, &progreso)?);
            Ok(0)
        }
        x => Err(txf!("cli.root_subcomando_desconocido_status", x)),
    }
}

fn share_cmd(st: &State, a: &mut Args) -> Result<i32, String> {
    let sub = a.val("share")?;
    let rest: Vec<String> = std::iter::from_fn(|| a.next()).collect();
    let mut cid: Option<u32> = None;
    let mut pos: Vec<String> = Vec::new();
    let mut ro = false;
    let mut it = rest.into_iter();
    while let Some(x) = it.next() {
        match x.as_str() {
            "--cid" => cid = Some(it.next().and_then(|v| v.parse().ok()).ok_or(tx!("cli.cid_se_espera_un_numero"))?),
            "--ro" => ro = true,
            _ => pos.push(x),
        }
    }
    let cid = cid.unwrap_or_else(|| adbcmd::cid_del_estado(st));
    let mut cfg = config::Config::cargar(&st.dir);
    let guardar = |c: &config::Config| c.guardar(&st.dir).map_err(|e| format!("config: {}", e));
    // si la maquina esta en marcha, deja el servicio de montaje en el invitado (idempotente)
    let preparar = || -> String {
        if !st.running() {
            return tx!("cli.la_maquina_esta_apagada_el_servicio_de").into();
        }
        match compartir::instalar_rc(cid, &st.dir.join("tmp"), false) {
            Ok(true) => tx!("cli.servicio_de_montaje_instalado_en_el").into(),
            Ok(false) => tx!("cli.el_servicio_de_montaje_ya_estaba").into(),
            Err(e) => txf!("cli.aviso_no_se_pudo_instalar_el_servicio_de", e),
        }
    };
    match sub.as_str() {
        "list" => {
            print!("{}", compartir::listado(&compartir::lista(&cfg)));
            Ok(0)
        }
        "add" => {
            let [nombre, carpeta] = &pos[..] else { return Err(tx!("cli.uso_share_add_nombre_carpeta_ro").into()) };
            let ruta = vm::absolute(carpeta)?;
            compartir::agregar(&mut cfg, compartir::Carpeta { nombre: nombre.clone(), ro, ruta }, true)?;
            guardar(&cfg)?;
            println!("{}", txf!("cli.agregada_se_aplica_al_proximo_arranque", format!("{:?}", nombre)));
            println!("{}", preparar());
            Ok(0)
        }
        "remove" => {
            let [nombre] = &pos[..] else { return Err(tx!("cli.uso_share_remove_nombre").into()) };
            let c = compartir::quitar(&mut cfg, nombre)?;
            guardar(&cfg)?;
            println!("{}", txf!("cli.quitada_se_aplica_al_proximo_arranque_de", format!("{:?}", c.nombre), c.ruta));
            Ok(0)
        }
        "setup" => {
            println!("{}", if compartir::instalar_rc(cid, &st.dir.join("tmp"), false)? { tx!("cli.servicio_de_montaje_instalado") } else { tx!("cli.el_servicio_de_montaje_ya_estaba") });
            // puntos de montaje listos y, si init no las monto (primera vez), montarlas ya
            print!("{}", compartir::preparar_y_montar(cid)?);
            Ok(0)
        }
        "status" => {
            let l = compartir::lista(&cfg);
            print!("{}", compartir::listado(&l));
            let usado = compartir::uid_configurado(&cfg);
            println!("{}", txf!("cli.uid_de_mediaprovider_usado_al_arrancar", usado));
            if !st.running() {
                return Ok(0);
            }
            println!("{}", txf!("cli.servicio_de_montaje_en_el_invitado", if compartir::rc_instalado(cid)? { tx!("root.instalado") } else { tx!("cli.no_instalado_share_setup") }));
            let real = compartir::aprender_uid(cid)?;
            println!("{}", txf!("cli.uid_de_mediaprovider_en_android", real));
            if real != usado {
                cfg.set("share.uid", &real.to_string())?;
                guardar(&cfg)?;
                println!("{}", tx!("cli.aviso_difiere_del_usado_guardado_en"));
            }
            print!("{}", compartir::estado_invitado(cid, &l)?);
            Ok(0)
        }
        x => Err(txf!("cli.share_subcomando_desconocido_list_add", x)),
    }
}

fn gamepad_cmd(st: &State, a: &mut Args) -> Result<i32, String> {
    let (pads, ilegibles) = gamepad::enumerar();
    let sub = a.val("gamepad")?;
    match sub.as_str() {
        "list" => {
            let con = if st.running() { Qmp::connect(&st.qmp()).and_then(|mut q| gamepad::conectados(&mut q)).unwrap_or_default() } else { Vec::new() };
            print!("{}", gamepad::listado(&pads, &con, ilegibles));
            if !st.running() {
                println!("{}", tx!("cli.la_maquina_no_esta_en_marcha_2"));
            }
            Ok(0)
        }
        "attach" => {
            let path = gamepad::resolver(&a.val("gamepad attach")?, &pads)?;
            println!("{}", gamepad::conectar_a_mano(st, &path)?);
            Ok(0)
        }
        "detach" => {
            println!("{}", gamepad::desconectar_a_mano(st, &a.val("gamepad detach")?, &pads)?);
            Ok(0)
        }
        x => Err(txf!("cli.gamepad_subcomando_desconocido_list", x)),
    }
}

/// `rotate`: fija la orientacion pedida (archivo `orientation`): 0, 90, 180, 270 o `auto`. Sin argumento da un paso
/// (0 -> 90 -> 180 -> 270 -> auto -> 0). Una orientacion fija la sigue el acelerometro del servicio de sensores (Android
/// gira si puede) y la ventana propia queda girada aunque Android no gire; `auto` deja que Android decida (la ventana lo
/// sigue) con el acelerometro derecho.
fn rotate(st: &State, arg: Option<String>) -> Result<i32, String> {
    if !st.running() {
        return Err(tx!("cli.la_maquina_no_esta_en_marcha").into());
    }
    let o = match arg.as_deref() {
        None => pantalla::leer_orientacion(&st.dir).siguiente(),
        Some("auto") => pantalla::Orientacion::Auto,
        Some(g) => g.parse::<u32>().ok().and_then(pantalla::rot_de_grados).map(pantalla::Orientacion::Fija).ok_or(tx!("cli.rotate_0_90_180_270_o_auto_o_sin"))?,
    };
    pantalla::escribir_orientacion(&st.dir, o).map_err(|e| txf!("cli.orientacion", e))?;
    println!("{}", txf!("cli.orientacion", o.descripcion()));
    Ok(0)
}

/// Tamano del panel segun QEMU (captura de pantalla de la consola), sin dejar archivos.
fn panel_actual(st: &State) -> Result<(u32, u32), String> {
    // QEMU escribe el archivo: ruta absoluta
    let f = std::fs::canonicalize(&st.dir).map_err(|e| txf!("cli.estado", e))?.join("medida.png").to_string_lossy().into_owned();
    let _ = std::fs::remove_file(&f);
    Qmp::connect(&st.qmp())?.exec("screendump", Some(V::obj(&[("filename", V::s(&f)), ("format", V::s("png"))])))?;
    let data = std::fs::read(&f).map_err(|e| txf!("cli.medida", e));
    let _ = std::fs::remove_file(&f);
    vm::png_info(&data?)
}

/// `resolution ANCHOxALTO[@DPI] [--wait S] [--no-restart] [--cid N]`: cambia el tamano del panel con la maquina en
/// marcha y lo deja guardado para el proximo arranque (pantalla.resolucion y pantalla.densidad de `config`). Con la ventana propia se pide
/// al dispositivo por D-Bus (Console.SetUIInfo); con la ventana GTK arrancada con --resolution, por el VNC que se abrio
/// para fijarla. Ambos llegan a virtio-gpu por el mismo camino de QEMU (dpy_set_ui_info). El dispositivo avisa al
/// invitado y el controlador DRM de Linux ofrece el modo nuevo, pero el hwcomposer de Cuttlefish no lo adopta en
/// caliente (SurfaceFlinger sigue con el modo viejo y los cuadros fallan hasta que se reinicia): Android lo usa tras
/// `setprop ctl.restart surfaceflinger` (reinicio blando del entorno grafico; no reinicia la maquina), que esta orden
/// hace por el adb propio. Despues aplica la densidad (`wm density`). Con `--no-restart` solo deja el modo pedido.
fn resolution(st: &State, spec: &str, wait: u64, restart: bool, cid: u32) -> Result<i32, String> {
    let r = pantalla::parse_resolucion(spec)?;
    std::fs::create_dir_all(&st.dir).map_err(|e| txf!("cli.estado", e))?;
    {
        // queda en `config` (pantalla.resolucion y, si se dio, pantalla.densidad) para el proximo arranque
        let mut c = config::Config::cargar(&st.dir);
        c.set("pantalla.resolucion", &format!("{}x{}", r.w, r.h))?;
        if let Some(d) = r.dpi {
            c.set("pantalla.densidad", &d.to_string())?;
        }
        c.guardar(&st.dir).map_err(|e| format!("config: {}", e))?;
    }
    if r.w % 8 != 0 {
        println!("{}", txf!("cli.aviso_el_controlador_drm_de_linux", r.w / 8 * 8, r.h));
    }
    if !st.running() {
        if let Some(d) = r.dpi {
            println!("{}", txf!("cli.densidad_se_aplica_en_el_proximo", d));
        }
        println!("{}", txf!("cli.la_maquina_no_esta_en_marcha_x_se_usara", r.w, r.h));
        return Ok(0);
    }
    let densidad = |r: &pantalla::Resolucion| -> Result<(), String> {
        if let Some(d) = r.dpi {
            adb::hacer_root(cid)?;
            let (c, o, e) = adb::shell_una_vez(cid, &format!("wm density {}", d), 15)?;
            if c != 0 {
                return Err(format!("wm density {}: {}{}", d, o.trim(), e.trim()));
            }
            println!("{}", txf!("cli.densidad", d));
        }
        Ok(())
    };
    let antes = panel_actual(st)?;
    // el controlador DRM de Linux redondea el ancho a un multiplo de 8
    let objetivo = (r.w / 8 * 8, r.h);
    if antes == objetivo {
        println!("{}", txf!("cli.el_panel_ya_mide_x", antes.0, antes.1));
        if restart {
            densidad(&r).unwrap_or_else(|e| println!("{}", txf!("cli.aviso_no_se_aplico_la_densidad", e)));
        }
        return Ok(0);
    }
    let (via, ok) = if std::path::Path::new(&st.f("vnc.sock")).exists() {
        (tx!("cli.vnc_setdesktopsize"), rfb::set_desktop_size(&st.f("vnc.sock"), r.w as u16, r.h as u16).is_ok())
    } else {
        pantalla::pedir_resolucion(&st.dir, &r).map_err(|e| txf!("cli.resolucion", e))?;
        // la ventana la toma en 100 ms; si sigue ahi tras 3 s no hay ventana propia
        let t0 = Instant::now();
        while std::path::Path::new(&st.f("requested-resolution")).exists() && t0.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(100));
        }
        let tomada = !std::path::Path::new(&st.f("requested-resolution")).exists();
        let _ = std::fs::remove_file(st.f("requested-resolution"));
        (tx!("cli.d_bus_setuiinfo_ventana_de"), tomada)
    };
    if !ok {
        println!("{}", tx!("cli.no_hay_forma_de_cambiarla_en_caliente"));
        return Ok(0);
    }
    println!("{}", txf!("cli.pedido_al_dispositivo_por_x_el_panel", via, r.w, r.h, antes.0, antes.1));
    let mut wait = wait;
    if restart {
        println!("{}", tx!("cli.reiniciando_surfaceflinger_reinicio"));
        match adb::reiniciar_surfaceflinger(cid) {
            Ok(s) => println!("{}", txf!("cli.surfaceflinger_de_vuelta_tras_s", format!("{:.0}", s))),
            Err(e) => {
                println!("{}", txf!("cli.aviso_no_se_pudo_reiniciar", e));
                return Ok(1);
            }
        }
        wait = wait.max(30);
    } else {
        println!("{}", tx!("cli.android_lo_adopta_al_reiniciar"));
    }
    let t0 = Instant::now();
    let mut adoptado = wait == 0;
    while t0.elapsed() < Duration::from_secs(wait) {
        if let Ok(ahora) = panel_actual(st) {
            if ahora == objetivo || ahora == (r.w, r.h) {
                println!("{}", txf!("cli.panel_x_tras_s", ahora.0, ahora.1, format!("{:.1}", t0.elapsed().as_secs_f64())));
                adoptado = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    if restart {
        densidad(&r).unwrap_or_else(|e| println!("{}", txf!("cli.aviso_no_se_aplico_la_densidad", e)));
    }
    if !adoptado {
        println!("{}", txf!("cli.el_panel_sigue_en_otro_tamano_tras_s", wait));
        return Ok(1);
    }
    Ok(0)
}

/// `restart [--timeout S] [--wait S]`: reinicio COMPLETO de la maquina (apagado y arranque nuevo con los mismos argumentos).
/// Es lo que hace el boton "Reinicio completo de la maquina" de la interfaz cuando adbd no responde. Por que existe en vez
/// de `system_reset`: ver src/reinicio.rs.
fn restart(st: &State, a: &mut Args) -> Result<i32, String> {
    let (mut timeout, mut wait) = (30u64, 0u64);
    while let Some(f) = a.next() {
        match f.as_str() {
            "--timeout" => timeout = a.num("--timeout")?,
            "--wait" => wait = a.num("--wait")?,
            x => return Err(txf!("cli.restart_opcion_desconocida", x)),
        }
    }
    // el estado en ruta absoluta antes de cambiar de carpeta (puede haberse dado relativo)
    let st = State { dir: std::path::PathBuf::from(vm::absolute(&st.dir.to_string_lossy())?) };
    let arr = reinicio::Arranque::leer(&st.dir)?;
    for (k, v) in arr.faltantes(|k| std::env::var_os(k).is_some()) {
        std::env::set_var(k, v);
    }
    std::env::set_current_dir(&arr.cwd).map_err(|e| txf!("cli.restart_no_se_pudo_volver_a_la_carpeta", arr.cwd, e))?;
    // la orientacion pedida es estado de la maquina en marcha (start la borra): se conserva
    let orient = std::fs::read_to_string(st.f("orientation")).ok();
    let ventana: Option<i32> = std::fs::read_to_string(st.f("window.pid")).ok().and_then(|t| t.trim().parse().ok());
    eprintln!("{}", tx!("cli.apagando_la_maquina"));
    if st.running() {
        stop(&st, timeout)?;
    } else {
        kill_helpers(&st);
    }
    // la ventana anterior termina sola cuando QEMU cierra la pantalla; si no, SIGKILL (SIGTERM equivaldria a cerrarla).
    // Solo mientras ese pid siga siendo un weft: el numero pudo pasar a otro proceso
    if let Some(p) = ventana.filter(|p| vm::alive_de(*p, "weft")) {
        let t0 = Instant::now();
        while vm::alive_de(p, "weft") && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(100));
        }
        if vm::alive_de(p, "weft") {
            vm::signal(p, 9);
            std::thread::sleep(Duration::from_millis(300));
        }
    }
    eprintln!("{}", tx!("cli.arrancando_con_los_mismos_argumentos"));
    // justo tras apagar, el kernel tarda un instante en soltar el CID de vsock: QEMU dice "unable to set guest cid:
    // Address already in use". Se reintenta con una espera corta y acotada (15 s en total).
    let t0 = Instant::now();
    let mut intentos = 0;
    let codigo = loop {
        intentos += 1;
        let mut args = Args { v: arr.args.clone(), i: 0 };
        match start(&st, &mut args) {
            Ok(c) => break c,
            Err(e) if reinicio::es_carrera_de_cid(&e) && t0.elapsed() < Duration::from_secs(15) => {
                eprintln!("{}", txf!("cli.el_cid_de_vsock_aun_no_esta_libre", intentos));
                std::thread::sleep(Duration::from_secs(1));
            }
            Err(e) => return Err(e),
        }
    };
    if let Some(o) = orient {
        let _ = std::fs::write(st.f("orientation"), o);
    }
    if wait > 0 {
        let cid = adbcmd::cid_del_estado(&st);
        eprintln!("{}", txf!("cli.esperando_a_que_android_termine_de", wait));
        let t = adb::esperar(cid, wait, true)?;
        println!("{}", txf!("cli.android_arranco_en_s", format!("{:.0}", t)));
        if !arr.con_ventana() {
            // sin la ventana propia nadie hace lo que hace su hilo tras cada arranque: se hace aqui (como el guion)
            let cfg = config::Config::cargar(&st.dir);
            std::thread::sleep(Duration::from_secs(6));
            match compartir::preparar_y_montar(cid) {
                Ok(t) if !t.trim().is_empty() => println!("{}", txf!("compartir.carpetas_compartidas", t.trim().replace('\n', "; "))),
                Ok(_) => {}
                Err(e) => eprintln!("{}", txf!("compartir.carpetas_compartidas", e)),
            }
            match aplicar::aplicar(&st, cid, &cfg, 60) {
                Ok(l) => println!("{}", txf!("compartir.ajustes_de_android", l.join("; "))),
                Err(e) => eprintln!("{}", txf!("compartir.ajustes_de_android", e)),
            }
            if cfg.bool("root.cargar_al_inicio") {
                match root::proveedor().asegurar(cid, &st.dir, &cfg, &progreso) {
                    Ok(t) => println!("root: {}", t),
                    Err(e) => eprintln!("root: {}", e),
                }
            }
            if let Some(l) = dispositivo::sincronizar_maquina(&st.dir, cid, &cfg, &progreso) {
                println!("{}", txf!("compartir.perfil_de_dispositivo", l.replace('\n', "; ")));
            }
        }
    }
    Ok(codigo)
}

fn status(st: &State) -> Result<i32, String> {
    if !st.running() {
        println!("{}", tx!("cli.detenida"));
        // por que termino la ultima vez
        if let Ok(ev) = std::fs::read_to_string(st.f("events.log")) {
            let l: Vec<&str> = ev.lines().collect();
            for e in l.iter().rev().take(6).rev() {
                println!("{}", txf!("cli.evento", e));
            }
        }
        return Ok(1);
    }
    let mut q = Qmp::connect(&st.qmp())?;
    let s = q.exec("query-status", None)?;
    let kvm = q.exec("query-kvm", None).ok().and_then(|k| k.get("enabled").and_then(|e| e.as_bool())).unwrap_or(false);
    println!("estado={} pid={} kvm={}", s.get("status").and_then(|x| x.as_str()).unwrap_or("?"), st.pid().unwrap_or(0), if kvm { "si" } else { "no" });
    // imagen y via del adb con que arranco esta maquina
    if let Ok(i) = std::fs::read_to_string(st.f("image-id")) {
        println!("imagen={}", i.trim());
    }
    if let Ok(t) = std::fs::read_to_string(st.f("adb-transport")) {
        let puerto = adbcmd::puerto_tcp_del_estado(st).map_or(String::new(), |p| format!(" tcp=127.0.0.1:{}", p));
        let cid = std::fs::read_to_string(st.f("vsock-cid")).map_or(String::new(), |c| format!(" vsock-cid={}", c.trim()));
        println!("adb={}{}{}", t.trim(), puerto, cid);
    }
    Ok(0)
}

fn wait_exit(st: &State, secs: u64) -> Result<i32, String> {
    let t0 = Instant::now();
    while st.running() {
        if t0.elapsed() > Duration::from_secs(secs) {
            return Err(txf!("cli.la_maquina_sigue_en_marcha_tras_s", secs));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    println!("{}", tx!("cli.detenida"));
    Ok(0)
}

/// Termina los procesos auxiliares (servicios de archivos compartidos) que hayan quedado de esta maquina.
fn kill_helpers(st: &State) {
    if st.otro_espacio_de_pids() && st.running() {
        // la maquina es de otra instancia de Flatpak: sus pids no son los de este espacio; no se toca nada
        return;
    }
    // (con otro espacio de pids y la maquina ya parada, los pids anotados no valen: solo se limpian los archivos; y un
    // numero que ya no sea de un virtiofsd paso a otro proceso, que no se toca)
    let validos = !st.otro_espacio_de_pids();
    for pid in std::fs::read_to_string(st.f("helpers.pid")).unwrap_or_default().lines().filter_map(|l| l.trim().parse::<i32>().ok()).filter(|p| validos && vm::alive_de(*p, "virtiofsd")) {
        vm::signal(pid, 15);
    }
    let _ = std::fs::remove_file(st.f("helpers.pid"));
    for i in 0..8 {
        let _ = std::fs::remove_file(st.f(&format!("virtiofs{}.sock", i)));
        let _ = std::fs::remove_file(st.f(&format!("virtiofs{}.sock.pid", i)));
    }
}

fn stop(st: &State, secs: u64) -> Result<i32, String> {
    let r = stop_vm(st, secs);
    kill_helpers(st);
    r
}

fn stop_vm(st: &State, secs: u64) -> Result<i32, String> {
    use apagado::Paso;
    // pid solo si este proceso lo ve (otra instancia de Flatpak tiene otro espacio de pids: se para por adb y QMP, nunca por
    // senal) y sigue siendo el QEMU de la maquina (vm::State::pid)
    let pid = st.pid();
    if pid.is_none() && !st.running() {
        println!("{}", tx!("cli.detenida"));
        return Ok(0);
    }
    // escalera: 1) apagado ordenado de Android por adb; 2) boton de apagado ACPI; 3) cierre de QEMU; 4) senal.
    // Se corta en cuanto QEMU termina. El tiempo de cada paso sale de `apagado::plan`.
    let t0 = Instant::now();
    let cid = adbcmd::cid_del_estado(st);
    let mut adb_pidio = false;
    let mut pasos = apagado::plan(secs, false).into_iter();
    // paso 1 (el plan se recalcula tras saber si adbd contesto)
    let (_, espera_adb) = pasos.next().unwrap();
    let metodo = apagado::metodo_en_uso();
    match apagado::pedir(cid, metodo) {
        Ok(()) => {
            adb_pidio = true;
            eprintln!("{}", txf!("cli.apagado_ordenado_pedido_por_adb", metodo.nombre()));
        }
        Err(e) => eprintln!("{}", txf!("cli.no_se_pudo_pedir_el_apagado_por_adb_se", e)),
    }
    if adb_pidio && wait_exit_en_silencio(st, espera_adb) {
        eprintln!("{}", txf!("cli.la_maquina_termino_a_los_s_del_apagado", format!("{:.1}", t0.elapsed().as_secs_f64())));
        println!("{}", tx!("cli.detenida"));
        return Ok(0);
    }
    for (paso, espera) in apagado::plan(secs, adb_pidio).into_iter().skip(1) {
        match paso {
            Paso::Adb => unreachable!(),
            Paso::Acpi => {
                if let Ok(mut q) = Qmp::connect(&st.qmp()) {
                    let _ = q.exec("system_powerdown", None);
                }
            }
            Paso::Quit => {
                eprintln!("{}", tx!("cli.el_invitado_no_se_apago_solo_se_cierra"));
                if let Ok(mut q) = Qmp::connect(&st.qmp()) {
                    let _ = q.exec("quit", None);
                }
            }
            Paso::Kill => {
                // se vuelve a comprobar el pid: tras la espera el numero pudo pasar a otro proceso
                if let Some(pid) = st.pid() {
                    anotar_cierre_forzado(st, tx!("cli.pedido_por_stop_ultimo_paso_del_apagado"));
                    let _ = vm::signal(pid, 9);
                }
            }
        }
        if wait_exit_en_silencio(st, espera) {
            eprintln!("{}", txf!("cli.la_maquina_termino_a_los_s", format!("{:.1}", t0.elapsed().as_secs_f64()), format!("{:?}", paso)));
            println!("{}", tx!("cli.detenida"));
            return Ok(0);
        }
    }
    Err(tx!("cli.la_maquina_sigue_en_marcha_tras_el").into())
}

/// Anota en events.log que se va a matar a QEMU a proposito (`por`: quien lo pide): QEMU no avisa de nada al recibir
/// SIGKILL, y sin esta linea la ventana lo tomaria por una caida (aviso y ultimo-fallo.txt; ver vista::cierre_ordenado).
fn anotar_cierre_forzado(st: &State, por: &str) {
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(st.f("events.log")) {
        let _ = writeln!(f, "{}", vista::linea_cierre_forzado(ts, por));
    }
}

/// Espera a que la maquina termine sin imprimir nada. true si termino.
fn wait_exit_en_silencio(st: &State, secs: u64) -> bool {
    let t0 = Instant::now();
    while st.running() {
        if t0.elapsed() > Duration::from_secs(secs) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    true
}

fn send_keys(q: &mut Qmp, keys: &[&str]) -> Result<(), String> {
    const HOLD_MS: u64 = 40;
    let ks: Vec<V> = keys.iter().map(|k| V::obj(&[("type", V::s("qcode")), ("data", V::s(k))])).collect();
    q.exec("send-key", Some(V::obj(&[("keys", V::Arr(ks)), ("hold-time", V::n(HOLD_MS as i64))])))?;
    // la orden vuelve antes de soltar la tecla: esperar para no solapar pulsaciones
    std::thread::sleep(Duration::from_millis(HOLD_MS + 30));
    Ok(())
}

fn tap(st: &State, x: f64, y: f64) -> Result<i32, String> {
    if std::path::Path::new(&st.f("touch")).exists() {
        // (el control de QEMU atiende una sola conexion a la vez: se abre solo donde se usa)
        let mut q = Qmp::connect(&st.qmp())?;
        let (down, up) = vm::touch_events(x, y);
        q.exec("input-send-event", Some(V::obj(&[("events", down)])))?;
        std::thread::sleep(Duration::from_millis(80));
        q.exec("input-send-event", Some(V::obj(&[("events", up)])))?;
        return Ok(0);
    }
    click(st, x, y)
}

/// Clic de raton (puntero absoluto + boton izquierdo): lo mismo que genera el raton sobre la ventana.
fn click(st: &State, x: f64, y: f64) -> Result<i32, String> {
    // con --pointer multitouch no hay puntero absoluto: el clic es un toque de la pantalla tactil
    if std::fs::read_to_string(st.f("pointer")).is_ok_and(|t| t.trim() == "multitouch") {
        return tap(st, x, y);
    }
    let mut q = Qmp::connect(&st.qmp())?;
    let abs = |axis: &str, v: f64| V::obj(&[("type", V::s("abs")), ("data", V::obj(&[("axis", V::s(axis)), ("value", V::n((v * 32767.0) as i64))]))]);
    let btn = |down: bool| V::obj(&[("type", V::s("btn")), ("data", V::obj(&[("button", V::s("left")), ("down", V::Bool(down))]))]);
    let send = |q: &mut Qmp, ev: Vec<V>| q.exec("input-send-event", Some(V::obj(&[("events", V::Arr(ev))])));
    send(&mut q, vec![abs("x", x), abs("y", y)])?;
    send(&mut q, vec![btn(true)])?;
    std::thread::sleep(Duration::from_millis(60));
    send(&mut q, vec![btn(false)])?;
    Ok(0)
}

fn screenshot(st: &State, out: &str) -> Result<i32, String> {
    println!("{}", vm::captura(&st.qmp(), out)?.1);
    Ok(0)
}

fn read_log(st: &State) -> String {
    String::from_utf8_lossy(&read_log_bytes(st)).into_owned()
}

/// La consola serie tal cual esta en el archivo (puede acabar a mitad de un caracter que QEMU aun no termino de escribir).
fn read_log_bytes(st: &State) -> Vec<u8> {
    std::fs::read(st.serial_log()).unwrap_or_default()
}

/// Texto de la consola a partir del byte `from` del archivo. Las posiciones se guardan en bytes del archivo, no de un
/// texto ya convertido: un caracter de varios bytes cortado al final se convierte en U+FFFD (3 bytes), y al completarse
/// cambia de tamano, asi que una posicion en el texto viejo puede caer dentro de un caracter del nuevo. Pura.
fn log_desde(log: &[u8], from: usize) -> std::borrow::Cow<'_, str> {
    String::from_utf8_lossy(&log[from.min(log.len())..])
}

/// Posicion (en bytes del archivo) donde termina la primera aparicion de `pat` a partir del byte `from`. Se busca en los
/// bytes: `pat` es UTF-8 valido, asi que coincide en los bytes justo donde coincidiria en el texto. Pura.
fn buscar_en_log(log: &[u8], pat: &str, from: usize) -> Option<usize> {
    let desde = from.min(log.len());
    let p = pat.as_bytes();
    if p.is_empty() {
        return Some(desde);
    }
    log[desde..].windows(p.len()).position(|w| w == p).map(|i| desde + i + p.len())
}

fn serial_send(st: &State, text: &str) -> Result<(), String> {
    let mut s = std::os::unix::net::UnixStream::connect(st.serial_sock()).map_err(|e| txf!("cli.consola_serie", e))?;
    s.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    s.flush().ok();
    // dar tiempo a QEMU a leer antes de cerrar la conexion
    std::thread::sleep(Duration::from_millis(150));
    let _ = s.set_read_timeout(Some(Duration::from_millis(10)));
    let _ = s.read(&mut [0u8; 64]);
    Ok(())
}

/// Espera a que `pat` aparezca en la consola serie a partir del byte `from` del archivo. Devuelve la posicion (en bytes
/// del archivo) donde termina.
fn wait_serial(st: &State, pat: &str, from: usize, secs: u64) -> Result<usize, String> {
    let t0 = Instant::now();
    loop {
        let bytes = read_log_bytes(st);
        if let Some(fin) = buscar_en_log(&bytes, pat, from) {
            return Ok(fin);
        }
        if t0.elapsed() > Duration::from_secs(secs) {
            let log = String::from_utf8_lossy(&bytes);
            let tail: String = log.lines().rev().take(15).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
            return Err(txf!("cli.no_aparecio_en_s_ultimas_lineas_de_la", format!("{:?}", pat), secs, tail));
        }
        if !st.running() {
            return Err(txf!("cli.la_maquina_termino_sin_mostrar", format!("{:?}", pat)));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Ejecuta una orden en el interprete del invitado conectado a la consola serie. Devuelve su codigo de salida.
fn guest_sh(st: &State, cmd: &str, secs: u64) -> Result<i32, String> {
    let token = format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0) & 0xffff_ffff);
    // en bytes del archivo (ver `log_desde`)
    let from = read_log_bytes(st).len();
    // el eco de la orden contiene "$?" literal; solo la salida real contiene "RC=<numero>"
    serial_send(st, &format!("{}; echo \"WEFT-{}-RC=$?\"\n", cmd, token))?;
    let mark = format!("WEFT-{}-RC=", token);
    let t0 = Instant::now();
    loop {
        if let Some((body, rc)) = resultado_de_orden(&log_desde(&read_log_bytes(st), from), &mark) {
            print!("{}", body);
            return Ok(rc);
        }
        if t0.elapsed() > Duration::from_secs(secs) {
            return Err(txf!("cli.la_orden_no_termino_en_s", secs));
        }
        if !st.running() {
            return Err(tx!("cli.la_maquina_termino_mientras_se_ejecutaba").into());
        }
        std::thread::sleep(Duration::from_millis(80));
    }
}

/// En lo que salio por la consola desde que se mando una orden, busca la marca `mark` seguida de digitos y fin de linea:
/// devuelve la salida de la orden (lo que hay entre el eco de la orden y la marca, sin \r) y su codigo (como mucho 125).
/// None si todavia no termino. Pura.
fn resultado_de_orden(tail: &str, mark: &str) -> Option<(String, i32)> {
    let mut search = 0;
    while let Some(i) = tail[search..].find(mark) {
        let after = &tail[search + i + mark.len()..];
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() && after[digits.len()..].starts_with(['\r', '\n']) {
            let body = &tail[..search + i];
            let body = body.split_once('\n').map_or("", |x| x.1);
            return Some((body.replace('\r', ""), digits.parse::<i32>().unwrap_or(1).min(125)));
        }
        search += i + mark.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_version_dice_el_commit_de_la_compilacion() {
        assert_eq!(version_con("1.0.0", None), "1.0.0");
        assert_eq!(version_con("1.0.0", Some("  ")), "1.0.0");
        assert_eq!(version_con("1.0.0", Some("2aec18b")), "1.0.0 (commit 2aec18b)");
        assert_eq!(version_con("1.0.0", Some("98fafd8c0ffee1234567890abcdef0123456789a")), "1.0.0 (commit 98fafd8c0ffe)");
        assert!(version().starts_with(VERSION_PAQUETE));
    }

    /// `sh` guarda la posicion de partida en bytes del archivo de la consola. Contada en el texto ya convertido, un emoji a
    /// medio escribir (2 de sus 4 bytes: un U+FFFD de 3 bytes en el texto) dejaba la posicion dentro del emoji cuando se
    /// completaba, y cortar ahi hacia panico.
    #[test]
    fn la_consola_con_un_caracter_partido_no_hace_panico() {
        let todo = "consola$ \u{1F600}ls; echo \"WEFT-ab-RC=$?\"\r\nuno\r\ndos \u{f1}\r\nWEFT-ab-RC=3\r\n".as_bytes();
        let antes = &todo[..b"consola$ ".len() + 2];
        let viejo = String::from_utf8_lossy(antes).len();
        assert!(!std::str::from_utf8(todo).unwrap().is_char_boundary(viejo), "la cuenta vieja caia dentro del emoji");
        let tail = log_desde(todo, antes.len());
        assert!(tail.starts_with("\u{fffd}\u{fffd}ls; echo"), "{:?}", tail);
        assert_eq!(resultado_de_orden(&tail, "WEFT-ab-RC="), Some(("uno\ndos \u{f1}\n".to_string(), 3)));
        // desde cualquier byte (tambien a mitad de un caracter o mas alla del final): nunca hace panico
        for from in 0..=todo.len() + 3 {
            let _ = resultado_de_orden(&log_desde(todo, from), "WEFT-ab-RC=");
            let _ = buscar_en_log(todo, "\u{f1}", from);
        }
        // la marca sin su fin de linea todavia no cuenta; el codigo se limita a 125
        assert_eq!(resultado_de_orden("eco\r\nWEFT-ab-RC=3", "WEFT-ab-RC="), None);
        assert_eq!(resultado_de_orden("eco\r\nhola\r\nWEFT-ab-RC=200\n", "WEFT-ab-RC="), Some(("hola\n".to_string(), 125)));
    }

    #[test]
    fn buscar_en_la_consola_por_bytes() {
        let log = "a\u{1F600}b\u{f1}c b\u{f1}c".as_bytes();
        // posiciones en bytes del archivo, donde termina el patron
        assert_eq!(buscar_en_log(log, "b\u{f1}c", 0), Some(9));
        assert_eq!(buscar_en_log(log, "b\u{f1}c", 6), Some(14));
        assert_eq!(buscar_en_log(log, "b\u{f1}c", 7), Some(14));
        assert_eq!(buscar_en_log(log, "\u{1F600}", 0), Some(5));
        // empezando a mitad del emoji no se encuentra el emoji, pero si lo que sigue
        assert_eq!(buscar_en_log(log, "\u{1F600}", 2), None);
        assert_eq!(buscar_en_log(log, "b", 2), Some(6));
        assert_eq!(buscar_en_log(log, "zz", 0), None);
        assert_eq!(buscar_en_log(log, "", 3), Some(3));
        assert_eq!(buscar_en_log(log, "c", 100), None);
        assert_eq!(buscar_en_log(b"", "", 5), Some(0));
    }

    #[test]
    fn las_ordenes_internas_llevan_el_estado_y_las_carpetas_del_padre() {
        let st = State { dir: std::path::PathBuf::from("/run/user/7/weft/prueba") };
        let c = orden_interna(std::path::PathBuf::from("/bin/weft"), &st);
        let args: Vec<String> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args, vec!["--state-dir", "/run/user/7/weft", "--name", "prueba"]);
        // en el entorno del hijo van exactamente las variables que `rutas` calcula para este proceso (en el modo de estado propio,
        // ninguna; en el estandar y con --root, las cuatro carpetas), con su ruta tal cual
        // (`get_envs` las da ordenadas por nombre)
        let envs: Vec<(String, String)> = c.get_envs().map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned()).unwrap_or_default())).collect();
        let mut esperadas: Vec<(String, String)> = rutas::actual().variables().into_iter().map(|(k, p)| (k.to_string(), p.to_string_lossy().into_owned())).collect();
        esperadas.sort();
        assert_eq!(envs, esperadas);
        for (k, _) in &envs {
            assert!(rutas::OPCIONES.iter().any(|(_, v)| v == k), "variable que no es de rutas: {}", k);
        }
    }

    /// Los pids anotados en helpers.pid solo reciben la senal si siguen siendo un virtiofsd: aqui esta el propio proceso de
    /// pruebas (que no lo es), y si la senal se mandara lo mataria.
    #[test]
    fn kill_helpers_no_senala_a_un_proceso_que_ya_no_es_un_virtiofsd() {
        let d = std::env::temp_dir().join(format!("weft-helpers-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let st = State { dir: d.clone() };
        std::fs::write(st.f("helpers.pid"), format!("{}\n{}\nnada\n", std::process::id(), i32::MAX)).unwrap();
        std::fs::write(st.f("virtiofs0.sock"), b"").unwrap();
        kill_helpers(&st);
        // seguimos vivos, y la limpieza de archivos se hizo igual
        assert!(!d.join("helpers.pid").exists() && !d.join("virtiofs0.sock").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Un arranque que falla a medias no deja nada vivo ni el pid de una maquina que no existe: aqui sin servicios de
    /// archivos y con un pid que no es de este arranque (el del propio proceso de pruebas, que no es un QEMU: no se toca).
    #[test]
    fn deshacer_un_arranque_fallido() {
        let d = std::env::temp_dir().join(format!("weft-deshacer-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let st = State { dir: d.clone() };
        std::fs::write(st.pidfile(), format!("{}\n", std::process::id())).unwrap();
        std::fs::write(st.f("pid-start"), "1\n").unwrap();
        std::fs::write(st.f("helpers.pid"), "nada\n").unwrap();
        st.anotar_espacio_de_pids();
        deshacer_arranque(&st);
        assert!(!d.join("pid").exists() && !d.join("pid-start").exists() && !d.join("helpers.pid").exists() && !d.join("pid-ns").exists());
        // sin nada que deshacer tampoco falla
        deshacer_arranque(&st);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn opciones_de_resolucion_y_puerto() {
        assert_eq!(resolucion_de_arranque("720x1348"), Ok((720, 1348)));
        assert_eq!(resolucion_de_arranque("64x64"), Ok((64, 64)));
        let max = pantalla::LADO_MAX;
        assert_eq!(resolucion_de_arranque(&format!("{}x{}", max, max)), Ok((max, max)));
        // el ancho no multiplo de 8 se admite (Linux lo redondea; start avisa)
        assert_eq!(resolucion_de_arranque("1348x720"), Ok((1348, 720)));
        for mala in ["0x0", "63x720", "720x63", &format!("{}x720", max + 1), &format!("720x{}", max + 1), "720", "ax720", "720xb", "-1x720", "720x", "x720"] {
            assert!(resolucion_de_arranque(mala).is_err(), "{}", mala);
        }
        assert!(resolucion_de_arranque("10x720").unwrap_err().contains("fuera de rango"));
        assert!(resolucion_de_arranque("720").unwrap_err().contains("ANCHOxALTO"));
        assert_eq!(puerto_adb("5601"), Ok(5601));
        assert_eq!(puerto_adb("65535"), Ok(65535));
        for malo in ["0", "65536", "-1", "x", ""] {
            assert!(puerto_adb(malo).unwrap_err().contains("1 a 65535"), "{}", malo);
        }
    }

    /// `weft window` (y `launch` con la maquina en marcha): solo se abre una ventana si la maquina esta en marcha, tiene la
    /// pantalla de la ventana propia y no hay ninguna viva.
    #[test]
    fn decision_de_abrir_la_ventana() {
        assert_eq!(decidir_ventana(true, true, None), Ventana::Abrir);
        assert_eq!(decidir_ventana(true, true, Some(Some(42))), Ventana::YaHay(Some(42)));
        assert_eq!(decidir_ventana(true, true, Some(None)), Ventana::YaHay(None));
        assert_eq!(decidir_ventana(true, false, None), Ventana::SinPantallaPropia);
        assert_eq!(decidir_ventana(false, true, None), Ventana::MaquinaParada);
        assert_eq!(decidir_ventana(false, true, Some(Some(42))), Ventana::MaquinaParada);
        // el tamano: el --resolution del arranque (el ultimo, sin contar lo que va a QEMU tras --), si no el de config
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(tamano_ventana(&v(&["--display", "window", "--resolution", "800x1400"]), Some((1, 1))), (800, 1400));
        assert_eq!(tamano_ventana(&v(&["--resolution", "800x1400", "--resolution", "720x1348"]), None), (720, 1348));
        assert_eq!(tamano_ventana(&v(&["--display", "window"]), Some((1080, 1920))), (1080, 1920));
        assert_eq!(tamano_ventana(&v(&["--display", "window", "--", "--resolution", "800x1400"]), None), TAMANO_VENTANA);
        assert_eq!(tamano_ventana(&v(&["--resolution"]), None), TAMANO_VENTANA);
    }

    /// El cerrojo de window.lock dice si hay ventana aunque no se vea su pid; se suelta al terminar el proceso (aqui, al
    /// cerrar el archivo).
    #[test]
    fn cerrojo_de_la_ventana() {
        let d = std::env::temp_dir().join(format!("weft-cerrojo-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let st = State { dir: d.clone() };
        // sin cerrojo ni pid: no hay ventana
        assert_eq!(cerrojo_tomado(&d.join("window.lock")), None);
        assert_eq!(ventana_viva(&st), None);
        let c = tomar_cerrojo_ventana(&st, Duration::ZERO).unwrap().expect("cerrojo");
        assert_eq!(cerrojo_tomado(&d.join("window.lock")), Some(true));
        // un segundo no lo consigue mientras el primero viva (ni esperando un poco)
        assert!(tomar_cerrojo_ventana(&st, Duration::from_millis(100)).unwrap().is_none());
        // tomado: hay ventana; su pid solo si es un weft vivo (este proceso de pruebas lo es)
        assert_eq!(ventana_viva(&st), Some(None));
        std::fs::write(st.f("window.pid"), format!("{}\n", std::process::id())).unwrap();
        assert_eq!(ventana_viva(&st), Some(Some(std::process::id() as i32)));
        drop(c);
        // suelto: no hay ventana aunque window.pid apunte a un weft vivo (un zombi, o un numero reutilizado)
        assert_eq!(cerrojo_tomado(&d.join("window.lock")), Some(false));
        assert_eq!(ventana_viva(&st), None);
        // sin window.lock (ventana de una version anterior): por su pid
        std::fs::remove_file(d.join("window.lock")).unwrap();
        assert_eq!(ventana_viva(&st), Some(Some(std::process::id() as i32)));
        std::fs::write(st.f("window.pid"), format!("{}\n", i32::MAX)).unwrap();
        assert_eq!(ventana_viva(&st), None);
        // sin poder abrir el archivo (la carpeta no existe): error, no "lo tiene otro"
        let otra = State { dir: d.join("no-existe") };
        assert!(tomar_cerrojo_ventana(&otra, Duration::ZERO).unwrap_err().contains("window.lock"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// La ventana que arranca justo cuando otra orden mira el cerrojo (`cerrojo_tomado` lo toma un instante) lo consigue
    /// igual: reintenta. Sin reintentar se quedaba toda su vida sin cerrojo y la siguiente `weft window` abria otra.
    #[test]
    fn el_cerrojo_se_toma_aunque_otro_lo_este_mirando() {
        use std::os::unix::io::AsRawFd;
        let d = std::env::temp_dir().join(format!("weft-cerrojo-carrera-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let st = State { dir: d.clone() };
        let ruta = d.join("window.lock");
        std::fs::write(&ruta, b"").unwrap();
        // alguien lo tiene tomado 200 ms
        let (tx, rx) = std::sync::mpsc::channel();
        let h = std::thread::spawn(move || {
            let f = std::fs::OpenOptions::new().write(true).open(&ruta).unwrap();
            assert_eq!(unsafe { flock(f.as_raw_fd(), LOCK_EX | LOCK_NB) }, 0);
            tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(200));
        });
        rx.recv().unwrap();
        assert!(tomar_cerrojo_ventana(&st, Duration::ZERO).unwrap().is_none(), "sin reintentar no lo consigue");
        let c = tomar_cerrojo_ventana(&st, Duration::from_secs(3)).unwrap();
        assert!(c.is_some(), "reintentando lo consigue en cuanto se suelta");
        h.join().unwrap();
        assert_eq!(cerrojo_tomado(&d.join("window.lock")), Some(true));
        drop(c);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// `launch` sigue en primer plano mientras la ventana viva, aunque la maquina ya no (la ventana muestra el aviso de la
    /// caida y anota ultimo-fallo.txt; dentro de Flatpak moriria con `launch`). Sin maquina ni ventana vuelve enseguida.
    #[test]
    fn launch_espera_tambien_a_la_ventana() {
        let d = std::env::temp_dir().join(format!("weft-launch-espera-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let st = State { dir: d.clone() };
        let t0 = Instant::now();
        esperar_maquina_y_ventana(&st, Duration::from_millis(20));
        assert!(t0.elapsed() < Duration::from_secs(1));
        // la maquina esta parada (no hay pid) pero la ventana tiene su cerrojo 300 ms mas
        let c = tomar_cerrojo_ventana(&st, Duration::ZERO).unwrap().expect("cerrojo");
        let h = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(c);
        });
        let t0 = Instant::now();
        esperar_maquina_y_ventana(&st, Duration::from_millis(20));
        assert!(t0.elapsed() >= Duration::from_millis(300), "{:?}", t0.elapsed());
        h.join().unwrap();
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Al arrancar, una ventana de la ejecucion anterior que siga viva (aqui, un proceso falso llamado weft-... que tiene
    /// tomado window.lock, como window-serve con el aviso de fin abierto) se cierra y suelta el cerrojo; este mismo proceso
    /// nunca se senala. Usa la orden flock(1) de util-linux; sin ella la prueba no puede montar el proceso falso.
    #[test]
    fn arrancar_cierra_la_ventana_de_la_ejecucion_anterior() {
        use std::os::unix::fs::PermissionsExt;
        if Command::new("flock").arg("--version").output().is_err() {
            eprintln!("sin flock(1): no se prueba");
            return;
        }
        let d = std::env::temp_dir().join(format!("weft-ventana-vieja-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let st = State { dir: d.clone() };
        // sin ventana: nada
        cerrar_ventana_anterior(&st);
        // la "ventana" es este proceso (p. ej. `restart` lanzado desde ella): no se senala a si mismo
        let c = tomar_cerrojo_ventana(&st, Duration::ZERO).unwrap().expect("cerrojo");
        std::fs::write(st.f("window.pid"), format!("{}\n", std::process::id())).unwrap();
        cerrar_ventana_anterior(&st);
        drop(c);
        // un window-serve viejo: toma el cerrojo (el `sleep` no hereda el descriptor) y se queda esperando
        let bin = d.join("weft-ventana");
        std::fs::write(&bin, "#!/bin/sh\nexec 9>>\"$1\"\nflock -x 9\n: > \"$1.listo\"\nsleep 20 9>&-\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut hijo = Command::new(&bin).arg(d.join("window.lock")).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let t0 = Instant::now();
        while !d.join("window.lock.listo").exists() && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::fs::write(st.f("window.pid"), format!("{}\n", hijo.id())).unwrap();
        assert_eq!(ventana_viva(&st), Some(Some(hijo.id() as i32)));
        // (se recoge en otro hilo, como lo haria el proceso padre de la ventana)
        let h = std::thread::spawn(move || hijo.wait());
        cerrar_ventana_anterior(&st);
        assert_eq!(ventana_viva(&st), None, "el cerrojo sigue tomado");
        assert!(!h.join().unwrap().unwrap().success());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// ultimo-fallo.txt: con la causa, las 30 ultimas lineas de qemu.log y los ultimos eventos (cuando se escribe lo decide la
    /// ventana: window::Fallo). Y la marca que deja un cierre forzado pedido en events.log.
    #[test]
    fn registro_del_ultimo_fallo() {
        let d = std::env::temp_dir().join(format!("weft-kill-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let st = State { dir: d.clone() };
        std::fs::write(st.f("events.log"), "1 RESET {}\n").unwrap();
        anotar_cierre_forzado(&st, "pedido por weft kill");
        let ev = std::fs::read_to_string(st.f("events.log")).unwrap();
        let ultima = ev.lines().last().unwrap();
        assert!(ev.starts_with("1 RESET {}\n") && ultima.ends_with(" KILL pedido por weft kill"), "{}", ev);
        assert!(ultima.split_whitespace().next().unwrap().parse::<u64>().unwrap() > 1_700_000_000);
        assert!(vista::cierre_ordenado(&ev));
        let _ = std::fs::remove_dir_all(&d);
        let qemu: String = (1..=40).map(|i| format!("linea {}\n", i)).collect();
        let t = texto_ultimo_fallo("2026-10-08 01:02:03 UTC", "default", "QEMU cerro la pantalla", &qemu, "1 FIN QEMU termino de golpe\n");
        assert!(t.starts_with("2026-10-08 01:02:03 UTC\n") && t.contains("default") && t.contains("QEMU cerro la pantalla"), "{}", t);
        assert!(t.contains("linea 11\n") && t.contains("linea 40\n") && !t.contains("linea 10\n"), "{}", t);
        assert!(t.contains("FIN QEMU termino de golpe"));
        let vacio = texto_ultimo_fallo("f", "m", "c", "", "");
        assert_eq!(vacio.matches("(vacío)").count(), 2);
    }
}
