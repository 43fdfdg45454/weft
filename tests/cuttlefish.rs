//! Arranque de Cuttlefish (el Android x86_64 de Google para maquinas virtuales) con weft sobre QEMU estandar.
//! El kernel se arranca directamente (weft unpack-boot), sin el cargador de arranque de Google.
//!
//!   WEFT_CF_IMAGES   directorio de la imagen descargada (boot.img, super.img, userdata.img, vbmeta*.img...)
//!   WEFT_OUT         directorio para resultados
//! Dos pruebas, cada una con su interruptor:
//!   WEFT_CF_FIRST=1  primer arranque desde cero: el disco lo ensambla weft (make-disk) y los parametros
//!                          de arranque salen de perfiles/cuttlefish.bootconfig. No interviene ninguna herramienta de Google.
//!   WEFT_CF=1        discos ya ensamblados por el lanzador de Google (WEFT_CF_INSTANCE)
//! Todo lo que habla con Android usa el adb propio de weft (adb-shell, push, install, logcat, root, remount,
//! disable-verity, reboot, wait-adb): no interviene el adb de Google. Maquina q35 (defecto de `start`) con el raton como
//! pantalla tactil virtio (--pointer multitouch): el QEMU propio 10.0.2 ya traia virtio-multitouch-pci (la prueba usaba
//! --touch) y pcie-root-port viene con cualquier q35.

use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_weft");
/// Directorio de estado de la maquina en prueba (lo fija `boot`); las ordenes del adb propio toman de ahi el CID.
static ESTADO: OnceLock<String> = OnceLock::new();

fn run(bin: &str, args: &[&str]) -> (i32, String) {
    match Command::new(bin).args(args).output() {
        Ok(o) => (o.status.code().unwrap_or(-1), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))),
        Err(e) => (-1, e.to_string()),
    }
}

fn env(k: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| panic!("falta {}", k))
}

/// Orden de weft sobre la maquina en prueba (sin imprimir nada).
fn ar(args: &[&str]) -> (i32, String) {
    let mut a = vec!["--state-dir", ESTADO.get().map_or(".", String::as_str), "--name", "cf"];
    a.extend_from_slice(args);
    run(BIN, &a)
}

/// Orden de shell en Android por el adb propio.
fn sh(c: &str) -> String {
    ar(&["adb-shell", c]).1.trim().to_string()
}

/// Salida binaria (stdout) de una orden de shell en Android, p. ej. `screencap -p`.
fn sh_bytes(c: &str) -> Vec<u8> {
    let a = ["--state-dir", ESTADO.get().map_or(".", String::as_str), "--name", "cf", "adb-shell", c];
    Command::new(BIN).args(a).output().map(|o| o.stdout).unwrap_or_default()
}

/// Si el QEMU que usara weft (el propio de WEFT_QEMU_DIR o el del PATH) ofrece el dispositivo `dev`.
fn qemu_tiene(dev: &str) -> bool {
    let (bin, lib) = match std::env::var("WEFT_QEMU_DIR") {
        Ok(d) => (format!("{}/bin/qemu-system-x86_64", d), format!("{}/lib", d)),
        Err(_) => ("qemu-system-x86_64".to_string(), String::new()),
    };
    match Command::new(bin).args(["-device", "help"]).env("LD_LIBRARY_PATH", lib).output() {
        Ok(o) => format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)).lines().any(|l| l.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')).any(|w| w == dev)),
        Err(_) => false,
    }
}

struct Machine {
    dir: String,
    out: String,
    /// argumentos con los que se arranco, para poder relanzarla con otros parametros
    start: Vec<String>,
}

impl Machine {
    fn emu(&self, args: &[&str]) -> (i32, String) {
        let mut a = vec!["--state-dir", self.dir.as_str(), "--name", "cf"];
        a.extend_from_slice(args);
        let r = run(BIN, &a);
        println!("$ weft {} -> {}\n{}", args.join(" "), r.0, r.1.trim_end().chars().take(2500).collect::<String>());
        r
    }
    fn quiet(&self, args: &[&str]) -> (i32, String) {
        let mut a = vec!["--state-dir", self.dir.as_str(), "--name", "cf"];
        a.extend_from_slice(args);
        run(BIN, &a)
    }
}

/// Extrae kernel e initrd de la imagen, arranca la maquina con los discos dados y espera el arranque completo.
/// Devuelve (maquina, arranco, texto de resultados).
fn boot(out: &str, images: &str, disks: &[String], bootconfigs: &[String], limit: u64) -> (Machine, bool, String) {
    std::fs::create_dir_all(out).unwrap();
    let mut m = Machine { dir: format!("{}/estado", out), out: out.to_string(), start: Vec::new() };
    let _ = ESTADO.set(m.dir.clone());
    let boot_dir = format!("{}/arranque", out);
    let img = |n: &str| format!("{}/{}", images, n);
    let (c, o) = run(BIN, &["unpack-boot", "--boot", &img("boot.img"), "--init-boot", &img("init_boot.img"), "--vendor-boot", &img("vendor_boot.img"), "--out", &boot_dir]);
    println!("unpack-boot: {}", o.trim());
    assert_eq!(c, 0, "weft unpack-boot");
    let cmdline = format!("{} console=hvc0 earlycon=uart8250,io,0x3f8 pnpacpi=off", std::fs::read_to_string(format!("{}/cmdline.txt", boot_dir)).unwrap().trim());
    let (kernel, initrd, bc_img) = (format!("{}/kernel", boot_dir), format!("{}/initrd.img", boot_dir), format!("{}/bootconfig.txt", boot_dir));
    let logcat = format!("2={}/logcat.txt", out);
    let audio = format!("wav:{}/audio.wav", out);
    // carpeta del anfitrion compartida: con la etiqueta "shared" Cuttlefish la monta sola en /mnt/vendor/shared
    let shared = format!("{}/compartida", out);
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(format!("{}/desde-anfitrion.txt", shared), "hola desde el anfitrion\n").unwrap();
    let share = format!("shared={}", shared);
    // defectos de `start`: maquina q35; el raton es la pantalla tactil virtio (--pointer multitouch). Se anota en la
    // transcripcion si este QEMU ofrece los dos dispositivos (si faltara uno, `start` fallaria con su mensaje)
    println!("QEMU: virtio-multitouch-pci={} pcie-root-port={}", qemu_tiene("virtio-multitouch-pci"), qemu_tiene("pcie-root-port"));
    let mut start: Vec<String> = ["start", "--pointer", "multitouch", "--kernel", &kernel, "--initrd", &initrd, "--append", &cmdline, "--bootconfig-file", &bc_img].iter().map(|s| s.to_string()).collect();
    for b in bootconfigs {
        start.extend(["--bootconfig-file".to_string(), b.clone()]);
    }
    // los discos van a partir de la ranura PCI 4: es donde los busca la configuracion de arranque
    for d in disks {
        start.extend(["--disk".to_string(), d.clone()]);
    }
    start.extend(
        [
            "--disk-slot", "4", "--mem", "4096", "--cpus", "4", "--accel", "kvm", "--resolution", "720x1348", "--vsock-cid", "3",
            // consolas virtio de Cuttlefish: hvc0 consola, hvc2 registro de Android, hvc18 y hvc19 sensores
            "--console", "hvc", "--hvc-count", "30", "--hvc-log", &logcat, "--sensors", "18,19",
            // red: Cuttlefish reserva la primera tarjeta para su modem simulado y marca la segunda (eth1) como
            // restringida; la tercera (eth2) es una Ethernet corriente y es la que da internet a las aplicaciones
            "--touch", "--audio", &audio, "--nics", "3", "--share", &share,
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    // traza de QEMU: con que formato de pixel crea el invitado sus imagenes de pantalla
    let trace = format!("{}/qemu-traza.txt", out);
    start.extend(["--", "-trace", "virtio_gpu_cmd_res_create_2d", "-D", &trace].iter().map(|s| s.to_string()));
    let start_ref: Vec<&str> = start.iter().map(String::as_str).collect();
    assert_eq!(m.emu(&start_ref).0, 0, "weft start");
    m.start = start.clone();

    let t0 = Instant::now();
    let (mut booted, mut adb_ok, mut shots) = (false, false, 0);
    while t0.elapsed() < Duration::from_secs(limit) {
        std::thread::sleep(Duration::from_secs(10));
        if m.quiet(&["status"]).0 != 0 {
            println!("la maquina termino");
            break;
        }
        // wait-adb: adbd responde y sys.boot_completed=1 (0); si adbd responde pero Android no termino, el mensaje lo dice
        let (c, v) = ar(&["wait-adb", "--timeout", "6"]);
        let responde = c == 0 || v.contains("sys.boot_completed=");
        adb_ok |= responde;
        let last = m.quiet(&["serial"]).1.lines().last().unwrap_or("").chars().take(150).collect::<String>();
        println!("[{} s] adb={} wait-adb={:?} | {}", t0.elapsed().as_secs(), responde, v.trim().chars().take(70).collect::<String>(), last);
        if c == 0 {
            booted = true;
            break;
        }
        if t0.elapsed().as_secs() / 60 >= shots {
            m.emu(&["screenshot", &format!("{}/arranque-{}.png", out, shots)]);
            shots += 1;
        }
    }
    let mut info = format!("arranco={} segundos={}\nadb={}\n", booted, t0.elapsed().as_secs(), adb_ok);
    info += &format!("estado: {}\n", m.quiet(&["status"]).1.trim());
    if adb_ok {
        for p in ["ro.build.version.release", "ro.build.version.sdk", "ro.product.cpu.abi", "ro.hardware.egl", "ro.hardware.gralloc", "ro.hardware.vulkan", "sys.boot_completed"] {
            info += &format!("{}={}\n", p, sh(&format!("getprop {}", p)));
        }
        std::fs::write(format!("{}/getprop.txt", out), sh("getprop")).unwrap();
        info += &format!("particiones: {}\n", sh("ls /dev/block/by-name/ | tr '\\n' ' '"));
    }
    (m, booted, info)
}

/// Comprobaciones con Android ya arrancado: pantalla, entrada desde weft, red y audio.
fn checks(m: &Machine, info: &mut String) {
    let out = &m.out;
    info.push_str(&format!("sensores: {}\n", sh("dumpsys sensorservice | grep -c -i -E 'accelerometer|gyroscope'")));
    // formato real de la imagen que Android entrega a la pantalla virtual (hace falta ser root para leerlo)
    adb_root();
    std::fs::write(
        format!("{}/pantalla-drm.txt", out),
        sh("cat /sys/kernel/debug/dri/0/framebuffer 2>&1 | head -n 40; echo; cat /sys/kernel/debug/dri/0/state 2>&1 | head -n 60; echo; dumpsys SurfaceFlinger 2>/dev/null | grep -i -E 'format|pixel|client.?target|dataspace' | head -n 40; echo; logcat -d | grep -i -E 'drm|minigbm|gralloc' | head -n 60"),
    )
    .unwrap();
    std::thread::sleep(Duration::from_secs(8));
    m.emu(&["screenshot", &format!("{}/bloqueo.png", out)]);
    sh("input keyevent KEYCODE_WAKEUP; wm dismiss-keyguard; am start -a android.settings.SETTINGS");
    std::thread::sleep(Duration::from_secs(15));
    std::fs::write(format!("{}/screencap.png", out), sh_bytes("screencap -p")).unwrap();
    m.emu(&["screenshot", &format!("{}/ajustes.png", out)]);
    info.push_str(&format!("actividad: {}\n", sh("dumpsys activity activities 2>/dev/null | grep -m1 ResumedActivity")));

    // --- entrada desde weft: se observan los eventos que recibe el kernel de Android mientras se envian
    std::fs::write(format!("{}/dispositivos-entrada.txt", out), sh("getevent -lp")).unwrap();
    let focus = || sh("dumpsys window 2>/dev/null | grep -m1 mCurrentFocus");
    let watch = || std::thread::spawn(|| sh("timeout 7 getevent -l"));
    let before = focus();
    let w = watch();
    std::thread::sleep(Duration::from_secs(2));
    m.emu(&["tap", "0.5", "0.21"]); // fila "Network & internet" de Ajustes
    let ev = w.join().unwrap();
    std::fs::write(format!("{}/eventos-toque.txt", out), &ev).unwrap();
    let down = ev.contains("ABS_MT_POSITION_X") && ev.contains("ABS_MT_TRACKING_ID   00000001");
    let up = ev.contains("ABS_MT_TRACKING_ID   ffffffff");
    std::thread::sleep(Duration::from_secs(4));
    std::fs::write(
        format!("{}/entrada-android.txt", out),
        sh("dumpsys input | grep -n -A45 \"Device .*: QEMU Virtio MultiTouch\" | head -n 80; echo; dumpsys input | grep -i -E 'viewport|isActive|DisplayViewport' | head -n 20; echo; logcat -d | grep -E 'InputReader|InputDispatcher' | tail -n 60"),
    )
    .unwrap();
    m.emu(&["screenshot", &format!("{}/tras-toque.png", out)]);
    let after = focus();
    println!("foco antes: {}\nfoco despues: {}", before, after);
    info.push_str(&format!("toque: contacto={} suelta={} pantalla_cambio={}\n", down, up, before != after));

    let w = watch();
    std::thread::sleep(Duration::from_secs(2));
    m.emu(&["key", "a", "shift-b"]);
    let ev = w.join().unwrap();
    std::fs::write(format!("{}/eventos-teclado.txt", out), &ev).unwrap();
    info.push_str(&format!("teclado: eventos={}\n", ev.contains("KEY_A") && ev.contains("KEY_LEFTSHIFT") && ev.contains("KEY_B")));

    // --- red (modo usuario de QEMU): direccion, puerta de enlace y una peticion HTTP real
    let ip = sh("ip -4 -o addr show scope global | awk '{print $2\" \"$4}' | tr '\\n' ';'");
    let ping = sh("ping -c 2 -W 3 10.0.4.2 2>&1 | grep -c 'bytes from'");
    let http = sh("printf 'GET / HTTP/1.0\\r\\nHost: example.com\\r\\n\\r\\n' | nc -w 8 example.com 80 2>&1 | head -n 1");
    info.push_str(&format!("red: direcciones={} respuestas_ping={} http={:?}\n", ip, ping, http));
    std::fs::write(
        format!("{}/red.txt", out),
        sh("ip -4 addr; echo; dumpsys ethernet 2>&1 | head -n 80; echo; logcat -d | grep -E 'Ethernet|IpClient|DhcpClient|eth[12]' | tail -n 80; echo; dumpsys connectivity 2>/dev/null | grep -i -E 'NetworkAgentInfo|Active default|ETHERNET' | head -n 20"),
    )
    .unwrap();

    // --- carpeta compartida con el anfitrion: cada operacion se hace en un lado y se comprueba en el otro
    {
        let host = format!("{}/compartida", out);
        let g = "/mnt/vendor/shared";
        let hp = |n: &str| format!("{}/{}", host, n);
        let exists = |n: &str| std::path::Path::new(&hp(n)).exists();
        let mounted = sh("mount | grep -c ' type virtiofs '") != "0";
        let mut ops: Vec<(&str, bool)> = vec![("montada", mounted)];
        // anfitrion -> Android
        ops.push(("leer_archivo_del_anfitrion", sh(&format!("cat {}/desde-anfitrion.txt", g)).contains("hola desde el anfitrion")));
        std::fs::create_dir_all(hp("dir-anfitrion/sub")).unwrap();
        std::fs::write(hp("dir-anfitrion/sub/dato.txt"), "anidado").unwrap();
        ops.push(("ver_directorio_del_anfitrion", sh(&format!("cat {}/dir-anfitrion/sub/dato.txt", g)) == "anidado"));
        // Android -> anfitrion
        sh(&format!("echo 'hola desde android' > {}/desde-android.txt", g));
        ops.push(("crear_archivo", std::fs::read_to_string(hp("desde-android.txt")).unwrap_or_default().contains("hola desde android")));
        sh(&format!("echo 'segunda linea' >> {}/desde-android.txt", g));
        ops.push(("anadir_a_archivo", std::fs::read_to_string(hp("desde-android.txt")).unwrap_or_default().lines().count() == 2));
        sh(&format!("echo 'sobrescrito' > {}/desde-anfitrion.txt", g));
        ops.push(("sobrescribir_archivo", std::fs::read_to_string(hp("desde-anfitrion.txt")).unwrap_or_default().trim() == "sobrescrito"));
        sh(&format!("mv {}/desde-android.txt {}/renombrado.txt", g, g));
        ops.push(("renombrar_archivo", exists("renombrado.txt") && !exists("desde-android.txt")));
        sh(&format!("rm {}/renombrado.txt", g));
        ops.push(("borrar_archivo", !exists("renombrado.txt")));
        sh(&format!("mkdir -p {}/dir-android/a/b && echo x > {}/dir-android/a/b/f.txt", g, g));
        ops.push(("crear_directorios", std::fs::read_to_string(hp("dir-android/a/b/f.txt")).unwrap_or_default().trim() == "x"));
        sh(&format!("mv {}/dir-android/a {}/dir-android/movido", g, g));
        ops.push(("renombrar_directorio", exists("dir-android/movido/b/f.txt") && !exists("dir-android/a")));
        let rmdir_full = sh(&format!("rmdir {}/dir-android 2>&1", g));
        ops.push(("rmdir_no_borra_directorio_con_contenido", exists("dir-android") && !rmdir_full.is_empty()));
        sh(&format!("rm -r {}/dir-android", g));
        ops.push(("borrar_directorio_con_contenido", !exists("dir-android")));
        sh(&format!("mkdir {}/vacio && rmdir {}/vacio", g, g));
        ops.push(("crear_y_borrar_directorio_vacio", !exists("vacio")));
        // borrar desde Android lo que creo el anfitrion
        sh(&format!("rm -r {}/dir-anfitrion", g));
        ops.push(("borrar_directorio_del_anfitrion", !exists("dir-anfitrion")));
        // archivo grande: 8 MiB con contenido verificable, ida y vuelta
        let big: Vec<u8> = (0..8usize << 20).map(|i| (i * 31 + i / 4096) as u8).collect();
        std::fs::write(hp("grande.bin"), &big).unwrap();
        let sum_host = big.iter().fold(0u64, |a, b| a.wrapping_mul(31).wrapping_add(*b as u64));
        sh(&format!("cp {}/grande.bin {}/copia.bin", g, g));
        let back = std::fs::read(hp("copia.bin")).unwrap_or_default();
        let sum_back = back.iter().fold(0u64, |a, b| a.wrapping_mul(31).wrapping_add(*b as u64));
        ops.push(("archivo_grande_ida_y_vuelta", back.len() == big.len() && sum_back == sum_host));
        ops.push(("tamano_visto_en_android", sh(&format!("stat -c %s {}/grande.bin", g)) == (8usize << 20).to_string()));
        let _ = std::fs::remove_file(hp("grande.bin"));
        let _ = std::fs::remove_file(hp("copia.bin"));
        // lo borrado en el anfitrion desaparece en Android (con hasta 1 s de retraso: es lo que dura la cache)
        std::thread::sleep(Duration::from_secs(2));
        ops.push(("borrado_del_anfitrion_visible", sh(&format!("ls {}/grande.bin 2>&1", g)).contains("No such file")));
        let failed: Vec<&str> = ops.iter().filter(|o| !o.1).map(|o| o.0).collect();
        info.push_str(&format!("carpeta: {} de {} operaciones correctas{}\n", ops.len() - failed.len(), ops.len(), if failed.is_empty() { String::new() } else { format!("; fallan: {}", failed.join(", ")) }));
        std::fs::write(format!("{}/carpeta.txt", out), sh(&format!("mount | grep -E 'virtiofs|/mnt/vendor/shared'; ls -laZ {} 2>&1; logcat -d | grep -i -E 'virtiofs|avc.*shared' | tail -n 20", g))).unwrap();
        let _ = std::fs::copy(format!("{}/cf/virtiofs0.log", m.dir), format!("{}/virtiofs.txt", out));
    }

    // --- audio: lo que suena en Android se graba en audio.wav
    sh("cmd notification post -t prueba etiqueta 'hola' >/dev/null 2>&1; input keyevent KEYCODE_VOLUME_UP; input keyevent KEYCODE_VOLUME_UP");
    std::thread::sleep(Duration::from_secs(6));
    let wav = std::fs::read(format!("{}/audio.wav", out)).unwrap_or_default();
    let sound = wav.len() > 44 && wav[44..].iter().any(|b| *b != 0);
    info.push_str(&format!("audio: bytes_grabados={} hubo_sonido={}\n", wav.len(), sound));
}

/// Frecuencia dominante (por cruces por cero) del tramo sonoro de una grabacion WAV de 16 bits a partir de `from`.
fn wav_tone_hz(wav: &[u8], from: usize) -> Option<f64> {
    if wav.len() < 44 + 4 {
        return None;
    }
    let channels = u16::from_le_bytes([wav[22], wav[23]]).max(1) as usize;
    let rate = u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]) as f64;
    let frame = 2 * channels;
    let start = 44 + (from.saturating_sub(44) / frame) * frame;
    let s: Vec<i32> = wav.get(start..)?.chunks_exact(frame).map(|c| i16::from_le_bytes([c[0], c[1]]) as i32).collect();
    // umbral relativo al pico: el volumen con que suena depende del ajuste de Android
    // se mide en el medio segundo con mas energia: asi no cuentan otros sonidos del sistema (avisos, teclas)
    let win = (rate / 2.0) as usize;
    if s.len() < win || win == 0 {
        return None;
    }
    let step = win / 5;
    let energy = |a: usize| s[a..a + win].iter().map(|v| (*v as i64) * (*v as i64)).sum::<i64>();
    let best = (0..=(s.len() - win) / step).map(|k| k * step).max_by_key(|a| energy(*a))?;
    let seg = &s[best..best + win];
    if seg.iter().map(|v| v.abs()).max()? < 200 {
        return None;
    }
    let crossings = seg.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count() as f64;
    Some(crossings / 2.0 / (seg.len() as f64 / rate))
}

/// Cabecera de una imagen PPM (P6): (ancho, alto, posicion de los datos).
fn ppm_header(d: &[u8]) -> Option<(usize, usize, usize)> {
    let mut i = 0;
    let mut vals = Vec::new();
    while vals.len() < 4 {
        while d.get(i)?.is_ascii_whitespace() {
            i += 1;
        }
        let s = i;
        while !d.get(i)?.is_ascii_whitespace() {
            i += 1;
        }
        vals.push(std::str::from_utf8(&d[s..i]).ok()?.to_string());
    }
    (vals[0] == "P6").then_some(())?;
    Some((vals[1].parse().ok()?, vals[2].parse().ok()?, i + 1))
}

#[test]
fn cabecera_ppm() {
    assert_eq!(ppm_header(b"P6\n720 1348\n255\nXYZ"), Some((720, 1348, 16)));
    assert_eq!(ppm_header(b"P5\n1 1\n255\n"), None);
}

#[test]
fn frecuencia_de_un_tono() {
    // 1 s de silencio y 1 s de tono de 1000 Hz, estereo, 44100 Hz
    let mut wav = vec![0u8; 44];
    wav[22] = 2;
    wav[24..28].copy_from_slice(&44100u32.to_le_bytes());
    for i in 0..88200usize {
        let v = if i < 44100 { 0i16 } else { ((2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 44100.0).sin() * 20000.0) as i16 };
        wav.extend_from_slice(&v.to_le_bytes());
        wav.extend_from_slice(&v.to_le_bytes());
    }
    let hz = wav_tone_hz(&wav, 44).unwrap();
    assert!((hz - 1000.0).abs() < 5.0, "{}", hz);
    assert_eq!(wav_tone_hz(&wav[..44 + 4 * 44100], 44), None);
}

/// Instala y ejecuta el banco de pruebas (una aplicacion real que calcula, dibuja en 2D, OpenGL y Vulkan, suena y
/// recibe entrada) y recoge sus resultados. `tag` distingue la variante (x86_64 nativa o arm64 traducida).
fn banco(m: &Machine, info: &mut String, apk: &str, abi: &str, tag: &str) -> String {
    let out = &m.out;
    sh("pm uninstall rs.weft.banco >/dev/null 2>&1");
    let inst = adb(&["install", "-r", "--abi", abi, apk]);
    info.push_str(&format!("banco[{}]: instalacion={:?}\n", tag, inst.lines().last().unwrap_or("").chars().take(100).collect::<String>()));
    let wav_before = std::fs::metadata(format!("{}/audio.wav", out)).map(|x| x.len() as usize).unwrap_or(44);
    sh("logcat -c; input keyevent KEYCODE_WAKEUP; wm dismiss-keyguard; am start -n rs.weft.banco/.Main");
    let t0 = Instant::now();
    // los casos de bajo nivel corren en el proceso ":bajo" de la app: si muere, la app espera a que Android anote la salida
    // (hasta 10 s por caso) y sigue, asi que "fin" llega igual
    // (vulkan tambien corre ahi, con hasta 60 s)
    while t0.elapsed() < Duration::from_secs(240) {
        std::thread::sleep(Duration::from_secs(5));
        if sh("logcat -d -s banco:I | grep -c 'BANCO fin='") != "0" {
            break;
        }
    }
    // entrada: un toque y una tecla sobre la propia aplicacion, que los devuelve por el registro
    // (la ventana puede tardar un instante en aceptar entrada tras dibujarse: se reintenta el toque)
    for intento in 1..=4 {
        m.emu(&["tap", "0.25", "0.5"]);
        std::thread::sleep(Duration::from_secs(2));
        if sh("logcat -d -s banco:I | grep -c 'BANCO toque=baja'") != "0" {
            info.push_str(&format!("banco[{}]: toque recibido al intento {}\n", tag, intento));
            break;
        }
    }
    m.emu(&["key", "a"]);
    std::thread::sleep(Duration::from_secs(2));
    m.emu(&["screenshot", &format!("{}/banco-{}.png", out, tag)]);
    let (mut verdict, mut colors) = screen_colors(m);
    if verdict == "no reconocidos" {
        // la pantalla puede estar a medio dibujar: un segundo intento
        std::thread::sleep(Duration::from_secs(4));
        (verdict, colors) = screen_colors(m);
    }
    let log = sh("logcat -d -s banco:I | grep BANCO");
    let wav = std::fs::read(format!("{}/audio.wav", out)).unwrap_or_default();
    let hz = wav_tone_hz(&wav, wav_before);
    let mut res = String::new();
    for l in log.lines() {
        if let Some(x) = l.split("BANCO ").nth(1) {
            res.push_str(&format!("banco[{}]: {}\n", tag, x.trim()));
        }
    }
    let peak = wav.get(wav_before.max(44)..).map_or(0, |d| d.chunks_exact(2).map(|c| (i16::from_le_bytes([c[0], c[1]]) as i32).abs()).max().unwrap_or(0));
    res.push_str(&format!("banco[{}]: tono_medido_hz={} (grabacion: {} bytes nuevos, pico {})\n", tag, hz.map_or("ninguno".to_string(), |h| format!("{:.0}", h)), wav.len().saturating_sub(wav_before), peak));
    res.push_str(&format!("banco[{}]: pantalla colores={} muestras={:?}\n", tag, verdict, colors));
    std::fs::write(format!("{}/banco-{}.txt", out, tag), sh("logcat -d | grep -E 'banco|AndroidRuntime|heddle|DEBUG' | tail -n 300")).unwrap();
    info.push_str(&res);
    res
}

/// Un solo caso del banco (modo `caso` de la app: se ejecuta en el proceso ":bajo", como en la bateria completa) con la
/// aplicacion recien instalada, sin entrada ni pantalla. Las lineas quedan como `banco[TAG]: caso=...` en la transcripcion.
fn banco_case(m: &Machine, info: &mut String, apk: &str, abi: &str, tag: &str, case: &str) {
    sh("pm uninstall rs.weft.banco >/dev/null 2>&1");
    let inst = adb(&["install", "-r", "--abi", abi, apk]);
    info.push_str(&format!("banco[{}]: instalacion={:?}\n", tag, inst.lines().last().unwrap_or("").chars().take(100).collect::<String>()));
    sh(&format!("logcat -c; input keyevent KEYCODE_WAKEUP; wm dismiss-keyguard; am start -n rs.weft.banco/.Main --es modo caso --es caso {}", case));
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(120) {
        std::thread::sleep(Duration::from_secs(3));
        if sh("logcat -d -s banco:I | grep -c 'BANCO fin='") != "0" {
            break;
        }
    }
    for l in sh("logcat -d -s banco:I | grep BANCO").lines() {
        if let Some(x) = l.split("BANCO ").nth(1) {
            info.push_str(&format!("banco[{}]: {}\n", tag, x.trim()));
        }
    }
    std::fs::write(format!("{}/banco-{}.txt", m.out, tag), sh("logcat -d | grep -E 'banco|AndroidRuntime|heddle|DEBUG|vulkan|Vulkan' | tail -n 300")).unwrap();
    sh("am force-stop rs.weft.banco");
}

/// Variante del banco arm64 con heddle sin `debug.heddle.vulkan` (su valor por defecto): solo el caso vulkan.
const ARM_VK_DEFAULT: &str = "arm64-vulkan-defecto";

/// Veredicto del caso vulkan del banco. `x86`: hubo variante x86_64 (nativa); `arm`: hubo variantes arm64 (heddle).
/// Devuelve (avisos, fallos). En x86_64 y en arm64 con `debug.heddle.vulkan=1` vale OK; OMITIDO (el dispositivo no
/// ofrece Vulkan) es un aviso; cualquier otra cosa, o no tener resultado, es un fallo. El resultado de arm64 sin la
/// propiedad (el defecto de heddle) nunca falla: es siempre un aviso, para decidir si se activa por defecto.
fn vulkan_verdict(info: &str, x86: bool, arm: bool) -> (Vec<String>, Vec<String>) {
    let (mut warns, mut fails) = (Vec::new(), Vec::new());
    let mut need = |tag: &str, what: &str| match banco_result(info, tag, "vulkan") {
        Some(("OK", _)) => {}
        Some(("OMITIDO", d)) => warns.push(format!("banco {}: vulkan=OMITIDO {} (el dispositivo no ofrece Vulkan: el caso no se probo)", what, d).replace("  ", " ")),
        Some((r, d)) => fails.push(format!("banco {}: vulkan={} {}", what, r, d).trim_end().to_string()),
        None => fails.push(format!("banco {}: vulkan sin resultado", what)),
    };
    if x86 {
        need("x86_64", "x86_64");
    }
    if arm {
        need("arm64", "arm64 (heddle, debug.heddle.vulkan=1)");
        warns.push(match banco_result(info, ARM_VK_DEFAULT, "vulkan") {
            Some((r, d)) => format!("banco arm64 (heddle, sin debug.heddle.vulkan, por defecto): vulkan={} {}", r, d).trim_end().to_string(),
            None => "banco arm64 (heddle, sin debug.heddle.vulkan, por defecto): vulkan sin resultado".to_string(),
        });
    }
    (warns, fails)
}

#[test]
fn veredicto_vulkan() {
    let info = "banco[x86_64]: vulkan=OK triangulo=255,128,64\nbanco[arm64]: vulkan=OMITIDO motivo=sin_vulkan dispositivos=0\n\
                banco[arm64-vulkan-defecto]: vulkan=FALLO sin libvulkan.so (dlopen: x)\n";
    let (w, f) = vulkan_verdict(info, true, true);
    assert!(f.is_empty(), "{:?}", f);
    assert_eq!(w.len(), 2);
    assert!(w[0].contains("arm64 (heddle, debug.heddle.vulkan=1): vulkan=OMITIDO motivo=sin_vulkan dispositivos=0"), "{}", w[0]);
    assert!(w[1].contains("por defecto): vulkan=FALLO sin libvulkan.so"), "{}", w[1]);
    // con la propiedad, un fallo o un proceso caido hacen fallar; sin resultado tambien
    let (_, f) = vulkan_verdict("banco[arm64]: vulkan=CAIDO senal=11\n", false, true);
    assert_eq!(f, vec!["banco arm64 (heddle, debug.heddle.vulkan=1): vulkan=CAIDO senal=11".to_string()]);
    let (w, f) = vulkan_verdict("", true, false);
    assert!(w.is_empty());
    assert_eq!(f, vec!["banco x86_64: vulkan sin resultado".to_string()]);
    // el defecto de heddle nunca falla, aunque de OK
    let (w, f) = vulkan_verdict("banco[arm64]: vulkan=OK a\nbanco[arm64-vulkan-defecto]: vulkan=OK b\n", false, true);
    assert!(f.is_empty());
    assert_eq!(w, vec!["banco arm64 (heddle, sin debug.heddle.vulkan, por defecto): vulkan=OK b".to_string()]);
}

/// Publica los avisos de Vulkan (`::warning`, linea `aviso:` y resumen de la tarea en el CI) y devuelve los fallos.
fn vulkan_report(info: &mut String, x86: bool, arm: bool) -> Vec<String> {
    let (warns, fails) = vulkan_verdict(info, x86, arm);
    for l in &warns {
        println!("::warning title=Vulkan::{}", l);
        info.push_str(&format!("aviso: {}\n", l));
    }
    if let (false, Ok(path)) = (warns.is_empty(), std::env::var("GITHUB_STEP_SUMMARY")) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(path) {
            let _ = writeln!(f, "### Vulkan en el banco\n");
            for l in &warns {
                let _ = writeln!(f, "- {}", l);
            }
        }
    }
    fails
}

/// Actividad nativa del banco: `android.app.NativeActivity` cuyo manifiesto declara `android.app.lib_name=banconativa` y
/// `android.app.func_name=banco_nativa_crear` (banco/jni/nativa.c; la biblioteca no exporta `ANativeActivity_onCreate`).
/// Corre en el proceso ":nativa" de la app. En arm64, heddle reconoce ese punto de entrada leyendo los metaData de las
/// actividades por JNI cuando ART se lo pide sin firma.
const NATIVA: &str = "rs.weft.banco/android.app.NativeActivity";

/// Instala el banco, abre su NativeActivity, toca el centro de la pantalla y mira su color. Lineas en la transcripcion:
/// `banco[TAG]: nativa_crear=...`, `nativa_ciclo=...`, `nativa=...`, `nativa_entrada=...` (de la app) y `nativa_pantalla=...`.
fn banco_nativa(m: &Machine, info: &mut String, apk: &str, abi: &str, tag: &str) {
    sh("pm uninstall rs.weft.banco >/dev/null 2>&1");
    let inst = adb(&["install", "-r", "--abi", abi, apk]);
    info.push_str(&format!("banco[{}]: nativa_instalacion={:?}\n", tag, inst.lines().last().unwrap_or("").chars().take(100).collect::<String>()));
    sh(&format!("logcat -c; input keyevent KEYCODE_WAKEUP; wm dismiss-keyguard; am start -n {}", NATIVA));
    let hay = |p: &str| sh(&format!("logcat -d -s banco:I | grep -c 'BANCO {}'", p)) != "0";
    // el resumen llega cuando hubo onStart, onResume, ventana, foco y cola de entrada
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(60) && !hay("nativa=") {
        std::thread::sleep(Duration::from_secs(2));
    }
    if hay("nativa=") {
        for intento in 1..=4 {
            m.emu(&["tap", "0.5", "0.5"]);
            std::thread::sleep(Duration::from_secs(2));
            if hay("nativa_entrada=") {
                info.push_str(&format!("banco[{}]: nativa toque recibido al intento {}\n", tag, intento));
                break;
            }
        }
    }
    let (mut verdict, mut color) = nativa_screen(m);
    if verdict != "verde" {
        std::thread::sleep(Duration::from_secs(3));
        (verdict, color) = nativa_screen(m);
    }
    for l in sh("logcat -d -s banco:I | grep 'BANCO nativa'").lines() {
        if let Some(x) = l.split("BANCO ").nth(1) {
            info.push_str(&format!("banco[{}]: {}\n", tag, x.trim()));
        }
    }
    info.push_str(&format!("banco[{}]: nativa_pantalla={} muestra={:?}\n", tag, verdict, color));
    // heddle anota los puntos de entrada que leyo del manifiesto
    let manifest = sh("logcat -d -s heddle | grep -m1 'puntos de entrada del manifiesto'");
    if !manifest.is_empty() {
        info.push_str(&format!("heddle: nativa {}\n", manifest.split("NativeActivity: ").nth(1).unwrap_or(&manifest).trim()));
    }
    std::fs::write(format!("{}/banco-nativa-{}.txt", m.out, tag), sh("logcat -d | grep -E 'banco|AndroidRuntime|heddle|DEBUG|NativeActivity' | tail -n 300")).unwrap();
    sh("am force-stop rs.weft.banco");
}

/// Color del centro de la pantalla con la actividad nativa abierta (la pinta entera de 40,200,40).
fn nativa_screen(m: &Machine) -> (&'static str, Option<Rgb>) {
    let ppm = format!("{}/nativa.ppm", m.out);
    m.quiet(&["screenshot", &ppm]);
    let color = std::fs::read(&ppm).ok().and_then(|d| {
        let (w, h, off) = ppm_header(&d)?;
        let i = off + (h / 2 * w + w / 2) * 3;
        d.get(i..i + 3).map(|p| (p[0], p[1], p[2]))
    });
    let _ = std::fs::remove_file(&ppm);
    match color {
        Some((r, g, b)) if g > 150 && r < 110 && b < 110 => ("verde", color),
        _ => ("otra", color),
    }
}

/// Veredicto de la actividad nativa en cada variante (`tags`: etiqueta y descripcion). Devuelve (avisos, fallos). Falla si
/// falta el resumen `nativa=OK` (no se llamo al punto de entrada del manifiesto o no llegaron los callbacks) o, con el
/// resumen, si la cola de entrada no recibio el toque. Que la pantalla no muestre el verde es un aviso.
fn nativa_verdict(info: &str, tags: &[(&str, &str)]) -> (Vec<String>, Vec<String>) {
    let (mut warns, mut fails) = (Vec::new(), Vec::new());
    for (tag, what) in tags {
        match banco_result(info, tag, "nativa") {
            Some(("OK", _)) => {
                if banco_result(info, tag, "nativa_entrada").is_none() {
                    fails.push(format!("banco {}: la cola de entrada de la NativeActivity no recibio el toque", what));
                }
            }
            Some((r, d)) => fails.push(format!("banco {}: nativa={} {}", what, r, d).trim_end().to_string()),
            None => fails.push(match banco_result(info, tag, "nativa_crear") {
                None => format!("banco {}: NativeActivity sin resultado (no se llamo a banco_nativa_crear, el android.app.func_name del manifiesto)", what),
                Some((r, d)) => format!("banco {}: NativeActivity sin resumen de ciclo de vida (nativa_crear={} {}", what, r, d).trim_end().to_string() + ")",
            }),
        }
        if banco_result(info, tag, "nativa_pantalla").is_some_and(|r| r.0 != "verde") {
            let d = banco_result(info, tag, "nativa_pantalla").map_or("", |r| r.1);
            warns.push(format!("banco {}: la pantalla no muestra el verde de la NativeActivity ({})", what, d));
        }
    }
    (warns, fails)
}

#[test]
fn veredicto_nativa() {
    let info = "banco[x86_64]: nativa_crear=OK func=banco_nativa_crear\nbanco[x86_64]: nativa=OK abi=x86_64 crear=1\n\
                banco[x86_64]: nativa_entrada=OK tipo=toque\nbanco[x86_64]: nativa_pantalla=verde muestra=Some((40, 200, 40))\n\
                banco[arm64]: nativa_pantalla=otra muestra=Some((0, 0, 0))\n";
    let (w, f) = nativa_verdict(info, &[("x86_64", "x86_64"), ("arm64", "arm64 (heddle)")]);
    assert_eq!(f, vec!["banco arm64 (heddle): NativeActivity sin resultado (no se llamo a banco_nativa_crear, el android.app.func_name del manifiesto)".to_string()]);
    assert_eq!(w, vec!["banco arm64 (heddle): la pantalla no muestra el verde de la NativeActivity (muestra=Some((0, 0, 0)))".to_string()]);
    let info = "banco[arm64]: nativa_crear=OK func=banco_nativa_crear\nbanco[arm64]: nativa=OK abi=arm64\n";
    let (_, f) = nativa_verdict(info, &[("arm64", "arm64 (heddle)")]);
    assert_eq!(f, vec!["banco arm64 (heddle): la cola de entrada de la NativeActivity no recibio el toque".to_string()]);
    let (_, f) = nativa_verdict("banco[arm64]: nativa_crear=FALLO jni=0x0\n", &[("arm64", "arm64 (heddle)")]);
    assert_eq!(f, vec!["banco arm64 (heddle): NativeActivity sin resumen de ciclo de vida (nativa_crear=FALLO jni=0x0)".to_string()]);
    let (_, f) = nativa_verdict("banco[arm64]: nativa=FALLO crear=0 dibujo=1\nbanco[arm64]: nativa_entrada=OK\n", &[("arm64", "arm64 (heddle)")]);
    assert_eq!(f, vec!["banco arm64 (heddle): nativa=FALLO crear=0 dibujo=1".to_string()]);
}

/// Publica los avisos de la actividad nativa (como `vulkan_report`) y devuelve los fallos.
fn nativa_report(info: &mut String, x86: bool, arm: bool) -> Vec<String> {
    let mut tags = Vec::new();
    if x86 {
        tags.push(("x86_64", "x86_64"));
    }
    if arm {
        tags.push(("arm64", "arm64 (heddle)"));
    }
    let (warns, fails) = nativa_verdict(info, &tags);
    for l in &warns {
        println!("::warning title=NativeActivity::{}", l);
        info.push_str(&format!("aviso: {}\n", l));
    }
    if let (false, Ok(path)) = (warns.is_empty() && fails.is_empty(), std::env::var("GITHUB_STEP_SUMMARY")) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(path) {
            let _ = writeln!(f, "### NativeActivity del banco (android.app.func_name propio)\n");
            for l in fails.iter().chain(&warns) {
                let _ = writeln!(f, "- {}", l);
            }
        }
    }
    fails
}

/// Casos de bajo nivel del banco. Corren en el proceso ":bajo" de la app (servicio `Bajo`): si ese proceso muere, la
/// linea es `caso=CAIDO senal=N motivo=M pid=P` y el resto del banco sigue.
const BAJO_NIVEL: [&str; 4] = ["fallo_recuperable", "codigo_propio", "mascara_senales", "jni_registrado"];

/// Resultado de un caso del banco en la transcripcion (`banco[TAG]: caso=RESULTADO detalle`): la primera palabra
/// (`OK`, `FALLO`, `CAIDO`) y el detalle, o None si el caso no aparece (la app murio antes o no se ejecuto).
fn banco_result<'a>(info: &'a str, tag: &str, case: &str) -> Option<(&'a str, &'a str)> {
    let pre = format!("banco[{}]: {}=", tag, case);
    let rest = info.lines().find_map(|l| l.strip_prefix(pre.as_str()))?;
    Some(rest.split_once(' ').unwrap_or((rest, "")))
}

#[test]
fn resultado_del_banco() {
    let info = "banco[x86_64]: codigo_propio=OK valores=1,2,3\nbanco[arm64]: codigo_propio=CAIDO senal=11 motivo=crash_nativo pid=9\nbanco[arm64]: fin=OK\n";
    assert_eq!(banco_result(info, "x86_64", "codigo_propio"), Some(("OK", "valores=1,2,3")));
    assert_eq!(banco_result(info, "arm64", "codigo_propio"), Some(("CAIDO", "senal=11 motivo=crash_nativo pid=9")));
    assert_eq!(banco_result(info, "arm64", "fin"), Some(("OK", "")));
    assert_eq!(banco_result(info, "arm64", "mascara_senales"), None);
    assert_eq!(banco_result(info, "arm6", "codigo_propio"), None);
}

/// Casos de bajo nivel que pasan en x86_64 (sin traductor) y no en arm64 (con heddle). Con WEFT_HEDDLE_REF vacio (la
/// ultima release de heddle) no hacen fallar la prueba: quedan como aviso de GitHub Actions (`::warning`, en la salida
/// estandar), como linea `aviso:` de la transcripcion y, en el CI, en el resumen de la tarea (GITHUB_STEP_SUMMARY). Con
/// WEFT_HEDDLE_REF se exigen (ver primer_arranque_desde_cero).
fn low_level_warnings(info: &mut String) {
    if std::env::var("WEFT_HEDDLE_REF").is_ok_and(|r| !r.is_empty()) {
        return;
    }
    let lines = low_level_gaps(info);
    for l in &lines {
        println!("::warning title=heddle (bajo nivel)::{}", l);
        info.push_str(&format!("aviso: {}\n", l));
    }
    if let (false, Ok(path)) = (lines.is_empty(), std::env::var("GITHUB_STEP_SUMMARY")) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(path) {
            let _ = writeln!(f, "### Avisos: casos de bajo nivel en arm64 (heddle, ultima release)\n");
            for l in &lines {
                let _ = writeln!(f, "- {}", l);
            }
        }
    }
}

/// Lineas de aviso de `low_level_warnings`: una por caso OK en x86_64 y no OK en arm64.
fn low_level_gaps(info: &str) -> Vec<String> {
    BAJO_NIVEL
        .iter()
        .filter(|c| banco_result(info, "x86_64", c).is_some_and(|r| r.0 == "OK"))
        .filter_map(|c| match banco_result(info, "arm64", c) {
            Some(("OK", _)) => None,
            Some((r, d)) => Some(format!("banco arm64 (heddle): {}={} {} (en x86_64: OK)", c, r, d).trim_end().replace("  ", " ")),
            None => Some(format!("banco arm64 (heddle): {} sin resultado (en x86_64: OK)", c)),
        })
        .collect()
}

#[test]
fn avisos_de_bajo_nivel() {
    let info = "banco[x86_64]: fallo_recuperable=OK a\nbanco[x86_64]: codigo_propio=OK b\nbanco[x86_64]: mascara_senales=FALLO c\n\
                banco[x86_64]: jni_registrado=OK d\nbanco[arm64]: fallo_recuperable=OK a\nbanco[arm64]: codigo_propio=CAIDO senal=11\n\
                banco[arm64]: mascara_senales=FALLO c\n";
    assert_eq!(
        low_level_gaps(info),
        vec!["banco arm64 (heddle): codigo_propio=CAIDO senal=11 (en x86_64: OK)".to_string(), "banco arm64 (heddle): jni_registrado sin resultado (en x86_64: OK)".to_string()]
    );
    assert!(low_level_gaps("").is_empty());
}

/// Espera a que adbd responda y Android declare el arranque completo (wait-adb). Devuelve los segundos, o None si no llego.
fn wait_boot(limit: u64) -> Option<u64> {
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(limit) {
        std::thread::sleep(Duration::from_secs(5));
        if ar(&["wait-adb", "--timeout", "6"]).0 == 0 {
            return Some(t0.elapsed().as_secs());
        }
    }
    None
}

/// Orden del adb propio de weft (install, push, root, remount, disable-verity, reboot...), con su resultado en la transcripcion.
fn adb(args: &[&str]) -> String {
    let r = ar(args);
    println!("$ weft {} -> {} {}", args.join(" "), r.0, r.1.trim().chars().take(300).collect::<String>());
    r.1
}

/// Reinicia Android y espera a que vuelva (el adb propio abre una conexion nueva en cada orden).
fn reboot(info: &mut String, tag: &str) -> bool {
    adb(&["reboot"]);
    std::thread::sleep(Duration::from_secs(8));
    let r = wait_boot(300);
    info.push_str(&format!("reinicio ({}): {}\n", tag, r.map_or("NO VOLVIO".to_string(), |s| format!("{} s", s))));
    r.is_some()
}

/// adbd como root (`root` espera a que adbd vuelva); se reintenta por si la conexion tarda.
fn adb_root() {
    for _ in 0..5 {
        adb(&["root"]);
        if sh("id -u") == "0" {
            return;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// Instala heddle como traductor de ARM64 del sistema y ejecuta una aplicacion cuya biblioteca nativa es solo ARM64.
/// WEFT_BRIDGE_LIB = libheddle.so (x86_64 Android), WEFT_ARM_APK = banco de pruebas compilado para arm64.
fn heddle(m: &Machine, info: &mut String) {
    let (Ok(lib), Ok(apk)) = (std::env::var("WEFT_BRIDGE_LIB"), std::env::var("WEFT_ARM_APK")) else {
        info.push_str("heddle: no se probo (faltan WEFT_BRIDGE_LIB y WEFT_ARM_APK)\n");
        return;
    };
    // antes de instalar el traductor, Android debe rechazar la aplicacion: no trae biblioteca para x86_64
    let pre = adb(&["install", "-r", &apk]);
    info.push_str(&format!("heddle: instalacion_sin_traductor={:?}\n", pre.replace('\n', " ").chars().rev().take(110).collect::<String>().chars().rev().collect::<String>()));
    sh("pm uninstall rs.weft.banco >/dev/null 2>&1");

    // particion de sistema modificable: desactivar la verificacion, reiniciar y remontar
    adb_root();
    adb(&["disable-verity"]);
    if !reboot(info, "tras desactivar la verificacion") {
        return;
    }
    adb_root();
    let rem = adb(&["remount"]);
    info.push_str(&format!("heddle: remount={:?}\n", rem.lines().last().unwrap_or("").chars().take(120).collect::<String>()));
    adb(&["push", &lib, "/system/lib64/libheddle.so"]);
    sh("chmod 644 /system/lib64/libheddle.so");
    sh("sed -i '/^ro.dalvik.vm.native.bridge=/d;/^ro.dalvik.vm.isa.arm64=/d;/^ro.product.cpu.abilist=/d;/^ro.product.cpu.abilist64=/d' /vendor/build.prop");
    sh("printf 'ro.dalvik.vm.native.bridge=libheddle.so\\nro.dalvik.vm.isa.arm64=x86_64\\nro.product.cpu.abilist=x86_64,arm64-v8a\\nro.product.cpu.abilist64=x86_64,arm64-v8a\\n' >> /vendor/build.prop");
    info.push_str(&format!("heddle: archivo={:?}\n", sh("ls -l /system/lib64/libheddle.so | cut -c1-60; grep -c native.bridge=libheddle /vendor/build.prop")));
    if !reboot(info, "tras instalar el traductor") {
        return;
    }
    adb_root();
    sh("setenforce 0");
    info.push_str(&format!("heddle: propiedades={:?}\n", sh("getprop ro.dalvik.vm.native.bridge; getprop ro.product.cpu.abilist")));
    // Vulkan con la configuracion por defecto de heddle (sin la propiedad: no reenvia libvulkan.so). Va antes de
    // cualquier setprop, para que la propiedad no exista; su resultado solo se informa (vulkan_report)
    info.push_str(&format!("heddle: debug.heddle.vulkan antes del caso por defecto=[{}]\n", sh("getprop debug.heddle.vulkan")));
    banco_case(m, info, &apk, "arm64-v8a", ARM_VK_DEFAULT, "vulkan");
    // con la propiedad, heddle reenvia Vulkan; heddle la lee al iniciar cada proceso (banco reinstala la app)
    sh("setprop debug.heddle.vulkan 1");
    banco(m, info, &apk, "arm64-v8a", "arm64");
    // antes de banco_nativa, que vacia el registro
    info.push_str(&format!("heddle: build={:?}\n", sh("logcat -d -s heddle | grep -o 'build=[0-9a-f]*' | head -n1")));
    // NativeActivity con android.app.func_name y lib_name propios: heddle debe reconocer el punto de entrada por el manifiesto
    banco_nativa(m, info, &apk, "arm64-v8a", "arm64");
}

/// Color RGB de un punto de la pantalla.
type Rgb = (u8, u8, u8);

/// Colores que muestra la pantalla con el banco de pruebas abierto (cuadrado rojo a la izquierda, azul a la derecha).
fn screen_colors(m: &Machine) -> (&'static str, Option<(Rgb, Rgb)>) {
    let ppm = format!("{}/colores.ppm", m.out);
    m.quiet(&["screenshot", &ppm]);
    let colors = std::fs::read(&ppm).ok().and_then(|d| {
        let (w, h, off) = ppm_header(&d)?;
        let px = |x: usize, y: usize| d.get(off + (y * w + x) * 3..off + (y * w + x) * 3 + 3).map(|p| (p[0], p[1], p[2]));
        Some((px(w / 6, h * 173 / 200)?, px(w * 4 / 5, h * 173 / 200)?))
    });
    let _ = std::fs::remove_file(&ppm);
    let verdict = match colors {
        Some(((r, _, b), (r2, _, b2))) if r > 150 && b < 110 && b2 > 150 && r2 < 130 => "correctos",
        Some(((r, _, b), (r2, _, b2))) if b > 150 && r < 110 && r2 > 150 && b2 < 130 => "rojo y azul intercambiados",
        _ => "no reconocidos",
    };
    (verdict, colors)
}

/// Relanza la maquina con otros parametros de arranque y mira los colores de la pantalla. Sirve para encontrar
/// la configuracion del compositor de Android que da colores correctos sobre la GPU virtio de QEMU.
fn finish(m: &Machine, name: &str, info: &str) {
    m.emu(&["screenshot", &format!("{}/final.png", m.out)]);
    println!("{}", info);
    std::fs::write(format!("{}/{}.txt", m.out, name), info).unwrap();
    let log = m.quiet(&["serial"]).1;
    let tail: Vec<&str> = log.lines().rev().take(4000).collect();
    std::fs::write(format!("{}/consola.txt", m.out), tail.into_iter().rev().collect::<Vec<_>>().join("\n")).unwrap();
    let _ = std::fs::copy(format!("{}/cf/qemu.log", m.dir), format!("{}/qemu.log", m.out));
    let _ = std::fs::copy(format!("{}/cf/sensors.log", m.dir), format!("{}/sensores.txt", m.out));
    m.emu(&["stop", "--timeout", "10"]);
}

fn asserts(booted: bool, info: &str, limit: u64) {
    assert!(booted, "Cuttlefish no termino de arrancar en {} s", limit);
    assert!(info.contains("ro.product.cpu.abi=x86_64"));
    assert!(info.contains("contacto=true suelta=true"), "el toque no llego completo a Android");
    assert!(info.contains("teclado: eventos=true"), "las teclas no llegaron a Android");
    assert!(info.contains("hubo_sonido=true"), "no se grabo sonido");
}

/// Primer arranque desde cero, sin ninguna herramienta de Google: weft ensambla el disco y arranca.
#[test]
fn primer_arranque_desde_cero() {
    if std::env::var("WEFT_CF_FIRST").as_deref() != Ok("1") {
        eprintln!("(omitida: define WEFT_CF_FIRST=1)");
        return;
    }
    let (images, out) = (env("WEFT_CF_IMAGES"), env("WEFT_OUT"));
    let limit: u64 = std::env::var("WEFT_LIMIT").ok().and_then(|x| x.parse().ok()).unwrap_or(600);
    std::fs::create_dir_all(&out).unwrap();
    let img = |n: &str| format!("{}/{}", images, n);
    let disk = format!("{}/disco.img", out);
    // las mismas particiones que arma el lanzador de Google; las ranuras A y B comparten contenido
    let mut parts: Vec<String> = vec!["misc=1M".into(), "metadata=64M".into(), "frp=1M".into()];
    for (name, file) in [
        ("boot", "boot.img"), ("init_boot", "init_boot.img"), ("vendor_boot", "vendor_boot.img"), ("vbmeta", "vbmeta.img"), ("vbmeta_system", "vbmeta_system.img"),
        ("vbmeta_system_dlkm", "vbmeta_system_dlkm.img"), ("vbmeta_vendor_dlkm", "vbmeta_vendor_dlkm.img"),
    ] {
        for slot in ["_a", "_b"] {
            parts.push(format!("{}{}={}", name, slot, img(file)));
        }
    }
    parts.push(format!("super={}", img("super.img")));
    parts.push(format!("userdata={}", img("userdata.img")));
    let mut args: Vec<String> = vec!["make-disk".into(), "--out".into(), disk.clone()];
    for p in &parts {
        args.extend(["--part".to_string(), p.clone()]);
    }
    let t = Instant::now();
    let (c, o) = run(BIN, &args.iter().map(String::as_str).collect::<Vec<_>>());
    println!("make-disk ({} s): {}", t.elapsed().as_secs(), o.trim());
    assert_eq!(c, 0, "weft make-disk");

    let profile = format!("{}/perfiles/cuttlefish.bootconfig", env!("CARGO_MANIFEST_DIR"));
    let (m, booted, mut info) = boot(&out, &images, std::slice::from_ref(&disk), &[profile], limit);
    let mut vulkan_fails: Vec<String> = Vec::new();
    let mut nativa_fails: Vec<String> = Vec::new();
    if booted {
        // weft no emula un controlador Bluetooth: si queda encendido, a los 3 minutos el servicio de Android
        // aborta ("Can't start HAL") y su aviso tapa la pantalla. Se apaga desde Android; el ajuste persiste.
        info.push_str(&format!("bluetooth: apagado={:?}\n", sh("cmd bluetooth_manager disable; settings put global bluetooth_on 0; settings get global bluetooth_on")));
        checks(&m, &mut info);
        if let Ok(apk) = std::env::var("WEFT_BANCO_X86") {
            banco(&m, &mut info, &apk, "x86_64", "x86_64");
            banco_nativa(&m, &mut info, &apk, "x86_64", "x86_64");
        }
        heddle(&m, &mut info);
        low_level_warnings(&mut info);
        let fails = vulkan_report(&mut info, std::env::var("WEFT_BANCO_X86").is_ok(), std::env::var("WEFT_BRIDGE_LIB").is_ok() && std::env::var("WEFT_ARM_APK").is_ok());
        vulkan_fails.extend(fails);
        let arm = std::env::var("WEFT_BRIDGE_LIB").is_ok() && std::env::var("WEFT_ARM_APK").is_ok();
        nativa_fails.extend(nativa_report(&mut info, std::env::var("WEFT_BANCO_X86").is_ok(), arm));
        // aceleracion por GPU: sin indicar --gpu, weft debe haber elegido gfxstream (QEMU propio) y Android
        // debe estar dibujando con ANGLE sobre el Vulkan del anfitrion
        info.push_str(&format!("gpu: egl={} sf=[{}]\n", sh("getprop ro.hardware.egl"), sh("dumpsys SurfaceFlinger 2>/dev/null | grep -m1 -E 'GLES: '").chars().take(140).collect::<String>()));
    }
    finish(&m, "primer-arranque", &info);
    let _ = std::fs::remove_file(&disk);
    asserts(booted, &info, limit);
    if std::env::var("WEFT_QEMU_DIR").is_ok() {
        assert!(info.lines().any(|l| l.starts_with("gpu: egl=angle") && l.contains("GFXStream")), "Android no esta usando la GPU por gfxstream");
    }
    if std::env::var("WEFT_BANCO_X86").is_ok() {
        for t in ["biblioteca=OK abi=x86_64", "calculo_java=OK", "calculo_nativo=OK", "hilos_nativos=OK", "dibujo_2d=OK", "opengl=OK", "archivos=OK", "sensores=OK", "red=OK", "sonido=OK"] {
            assert!(info.contains(&format!("banco[x86_64]: {}", t)), "banco x86_64: falta {}", t);
        }
        assert!(info.contains("banco[x86_64]: toque=baja") && info.contains("fuente=tactil"), "el toque no llego a la aplicacion");
        assert!(info.contains("banco[x86_64]: tecla=KEYCODE_A"), "la tecla no llego a la aplicacion");
        assert!(info.contains("banco[x86_64]: pantalla colores=correctos"), "la pantalla no muestra los colores correctos");
    }
    if std::env::var("WEFT_BRIDGE_LIB").is_ok() {
        for t in ["biblioteca=OK abi=arm64", "calculo_nativo=OK", "hilos_nativos=OK"] {
            assert!(info.contains(&format!("banco[arm64]: {}", t)), "banco arm64 (heddle): falta {}", t);
        }
        // casos de bajo nivel (fallo recuperable, codigo propio, mascara de senales, JNI registrado): se exigen al probar
        // una version de heddle de una rama (WEFT_HEDDLE_REF, CI lanzado a mano), y solo los que pasan sin traductor
        // (variante x86_64): la ultima release que usa el CI de cada push puede no soportarlos todavia, y entonces solo
        // avisan (low_level_warnings)
        if std::env::var("WEFT_HEDDLE_REF").is_ok_and(|r| !r.is_empty()) {
            for case in BAJO_NIVEL {
                if banco_result(&info, "x86_64", case).is_some_and(|r| r.0 == "OK") {
                    let arm = banco_result(&info, "arm64", case);
                    assert!(arm.is_some_and(|r| r.0 == "OK"), "banco arm64 (heddle): {}={}", case, arm.map_or("(sin resultado)".to_string(), |r| format!("{} {}", r.0, r.1)));
                }
            }
        }
    }
    // vulkan: OK u OMITIDO en x86_64 y en arm64 con debug.heddle.vulkan=1 (ver vulkan_verdict)
    assert!(vulkan_fails.is_empty(), "Vulkan: {}", vulkan_fails.join("; "));
    // NativeActivity con punto de entrada propio: se exige en x86_64 y en arm64 (heddle), tambien con la ultima release
    assert!(nativa_fails.is_empty(), "NativeActivity: {}", nativa_fails.join("; "));
}

/// Discos ya ensamblados (y arrancados una vez) por el lanzador de Google.
#[test]
fn cuttlefish_arranca() {
    if std::env::var("WEFT_CF").as_deref() != Ok("1") {
        eprintln!("(omitida: define WEFT_CF=1)");
        return;
    }
    let (inst, images, out) = (env("WEFT_CF_INSTANCE"), env("WEFT_CF_IMAGES"), env("WEFT_OUT"));
    let limit: u64 = std::env::var("WEFT_LIMIT").ok().and_then(|x| x.parse().ok()).unwrap_or(600);
    let disk = |n: &str| format!("{}/{}", inst, n);
    let disks = [disk("overlay.img"), disk("persistent_composite_overlay.img"), disk("sdcard_overlay.img")];
    // parametros generados por el lanzador de Google, mas lo que anade su cargador de arranque
    let extra = format!("{}/extra.bootconfig", out);
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(&extra, "androidboot.slot_suffix=_a\nandroidboot.force_normal_boot=1\nandroidboot.verifiedbootstate=orange\n").unwrap();
    let (m, booted, mut info) = boot(&out, &images, &disks, &[disk("internal/bootconfig"), extra], limit);
    if booted {
        checks(&m, &mut info);
    }
    finish(&m, "cuttlefish", &info);
    asserts(booted, &info, limit);
}
