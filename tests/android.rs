//! Arranque de una imagen de Android x86_64 de Google (tipo SDK) con weft sobre QEMU estandar, comprobado por el adb
//! propio de weft (wait-adb, adb-shell, logcat; por vsock, sin adb de Google). Hace falta una imagen cuyo adbd escuche
//! por vsock y sin autenticacion (como Cuttlefish); el CI no ejecuta esta prueba (la cubre tests/cuttlefish.rs).
//! Se activa con WEFT_ANDROID=1:
//!   WEFT_IMAGE  directorio de la imagen (kernel-ranchu, ramdisk.img, system.img, vendor.img...)
//!   WEFT_WORK   directorio con los discos de trabajo: cache.img, userdata.img, encryptionkey.img
//!   WEFT_OUT    directorio para resultados      WEFT_GPU  dispositivo grafico (por defecto virtio-pci)
//!   WEFT_BOOTCONFIG_EXTRA  archivo con parametros de arranque adicionales (opcional)

use std::process::Command;
use std::time::{Duration, Instant};

fn run(bin: &str, args: &[&str]) -> (i32, String) {
    match Command::new(bin).args(args).output() {
        Ok(o) => (o.status.code().unwrap_or(-1), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))),
        Err(e) => (-1, e.to_string()),
    }
}

/// Parametros de arranque verificado que acompanan a la imagen: lineas `param: "clave=valor"`.
fn verified_boot_params(text: &str) -> Vec<String> {
    text.lines().filter_map(|l| l.trim().strip_prefix("param:")).map(|v| v.trim().trim_matches('"').to_string()).filter(|v| !v.is_empty()).collect()
}

/// `clave=valor` -> linea de bootconfig con el valor entre comillas.
fn bc(kv: &str) -> String {
    match kv.split_once('=') {
        Some((k, v)) => format!("{}=\"{}\"", k.trim(), v.trim().trim_matches('"')),
        None => kv.to_string(),
    }
}

#[test]
fn parametros_de_arranque_verificado() {
    let p = verified_boot_params("major_version: 1\nparam: \"androidboot.vbmeta.size=6720\"\n  param: \"dm=1 vroot none\"\n");
    assert_eq!(p, vec!["androidboot.vbmeta.size=6720", "dm=1 vroot none"]);
    assert_eq!(bc("a.b=x y"), "a.b=\"x y\"");
}

#[test]
fn android_arranca() {
    if std::env::var("WEFT_ANDROID").as_deref() != Ok("1") {
        eprintln!("(prueba con Android omitida: define WEFT_ANDROID=1)");
        return;
    }
    let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("falta {}", k));
    let (img, work, out) = (env("WEFT_IMAGE"), env("WEFT_WORK"), env("WEFT_OUT"));
    let gpu = std::env::var("WEFT_GPU").unwrap_or_else(|_| "virtio-pci".into());
    let limit: u64 = std::env::var("WEFT_LIMIT").ok().and_then(|x| x.parse().ok()).unwrap_or(480);
    std::fs::create_dir_all(&out).unwrap();
    let dir = format!("{}/estado", out);

    // --- parametros de arranque: los mismos que usa el emulador oficial, sin los que dependen de sus dispositivos propios
    let mut boot: Vec<String> = vec![
        bc("androidboot.boot_devices=pci0000:00/0000:00:03.0 pci0000:00/0000:00:06.0"),
        bc("androidboot.hardware=ranchu"),
        bc("androidboot.qemu=1"),
        bc("androidboot.serialno=WEFT0001"),
        bc("androidboot.dalvik.vm.heapsize=576m"),
        bc("androidboot.debug.hwui.renderer=skiagl"),
        bc("androidboot.opengles.version=196608"),
        bc("androidboot.qemu.vsync=60"),
        bc("androidboot.qemu.skin=1080x2400"),
        bc("androidboot.logcat=*:V"),
        bc("androidboot.console=ttyS0"),
    ];
    let mut cmdline = String::from("8250.nr_uarts=1 clocksource=pit no_timer_check console=ttyS0,38400 loop.max_part=7 printk.devkmsg=on");
    if let Ok(t) = std::fs::read_to_string(format!("{}/kernel_cmdline.txt", img)) {
        cmdline = format!("{} {}", t.trim(), cmdline);
    }
    let vb = std::fs::read_to_string(format!("{}/VerifiedBootParams.textproto", img)).unwrap_or_default();
    for p in verified_boot_params(&vb) {
        if p.starts_with("androidboot.") {
            boot.push(bc(&p));
        } else {
            cmdline = format!("{} {}", cmdline, p);
        }
    }
    if let Ok(f) = std::env::var("WEFT_BOOTCONFIG_EXTRA") {
        for l in std::fs::read_to_string(&f).unwrap_or_default().lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            // un parametro adicional sustituye al que tenga la misma clave
            let key = l.split('=').next().unwrap_or("").trim().to_string();
            boot.retain(|b| b.split('=').next().unwrap_or("").trim() != key);
            boot.push(bc(l));
        }
    }
    let bcfile = format!("{}/bootconfig.txt", out);
    std::fs::write(&bcfile, boot.join("\n") + "\n").unwrap();
    std::fs::write(format!("{}/cmdline.txt", out), &cmdline).unwrap();

    let emu = |args: &[&str]| {
        let mut a = vec!["--state-dir", dir.as_str(), "--name", "android"];
        a.extend_from_slice(args);
        let r = run(env!("CARGO_BIN_EXE_weft"), &a);
        println!("$ weft {} -> {}\n{}", args.join(" "), r.0, r.1.trim_end().chars().take(3000).collect::<String>());
        r
    };
    // orden de shell en Android por el adb propio (CID 3 por vsock)
    let adbsh = |c: &str| run(env!("CARGO_BIN_EXE_weft"), &["--state-dir", &dir, "--name", "android", "adb-shell", c]);
    let hvc1 = format!("file,id=h1,path={}/logcat-hvc1.txt", out);
    let (sys, vendor) = (format!("{}/system.img,ro", img), format!("{}/vendor.img,ro", img));
    let (cache, data, key) = (format!("{}/cache.img", work), format!("{}/userdata.img", work), format!("{}/encryptionkey.img", work));
    let (kernel, initrd) = (format!("{}/kernel-ranchu", img), format!("{}/ramdisk.img", img));
    let mut start: Vec<&str> = vec!["start", "--machine", "pc", "--kernel", &kernel, "--initrd", &initrd, "--append", &cmdline, "--bootconfig-file", &bcfile];
    // mismo orden de discos que el emulador oficial: vda sistema, vdb cache, vdc datos, vdd metadatos, vde vendor
    start.extend_from_slice(&["--disk", &sys, "--disk", &cache, "--disk", &data, "--disk", &key, "--disk", &vendor]);
    start.extend_from_slice(&["--mem", "4096", "--cpus", "4", "--accel", "kvm", "--gpu", &gpu, "--vsock-cid", "3"]);
    // consolas virtuales: hvc0 (sin uso) y hvc1, por donde Android vuelca su registro
    start.extend_from_slice(&["--", "-device", "virtio-serial-pci", "-chardev", "null,id=h0", "-device", "virtconsole,chardev=h0", "-chardev", &hvc1, "-device", "virtconsole,chardev=h1"]);
    assert_eq!(emu(&start).0, 0, "weft start");

    let t0 = Instant::now();
    let (mut booted, mut adb_ok, mut shots) = (false, false, 0);
    while t0.elapsed() < Duration::from_secs(limit) {
        std::thread::sleep(Duration::from_secs(10));
        if run(env!("CARGO_BIN_EXE_weft"), &["--state-dir", &dir, "--name", "android", "status"]).0 != 0 {
            println!("la maquina termino");
            break;
        }
        // wait-adb: 0 si adbd responde y sys.boot_completed=1; si responde pero Android no termino, el mensaje lo dice
        let (c, v) = run(env!("CARGO_BIN_EXE_weft"), &["--state-dir", &dir, "--name", "android", "wait-adb", "--timeout", "6"]);
        let responde = c == 0 || v.contains("sys.boot_completed=");
        adb_ok |= responde;
        println!("[{} s] adb={} wait-adb={:?}", t0.elapsed().as_secs(), responde, v.trim().chars().take(80).collect::<String>());
        if t0.elapsed().as_secs() / 60 >= shots {
            emu(&["screenshot", &format!("{}/arranque-{}.png", out, shots)]);
            shots += 1;
        }
        if c == 0 {
            booted = true;
            break;
        }
    }

    // --- resultados, pase lo que pase
    let mut info = format!("arranco={} adb={} segundos={}\n", booted, adb_ok, t0.elapsed().as_secs());
    info += &format!("estado: {}\n", emu(&["status"]).1.trim());
    if adb_ok {
        for p in ["ro.build.version.release", "ro.build.version.sdk", "ro.product.cpu.abi", "ro.hardware", "ro.hardware.egl", "ro.hardware.gralloc", "init.svc.surfaceflinger", "init.svc.zygote", "sys.boot_completed"] {
            info += &format!("{}={}\n", p, adbsh(&format!("getprop {}", p)).1.trim());
        }
        std::fs::write(format!("{}/getprop.txt", out), adbsh("getprop").1).unwrap();
        std::fs::write(format!("{}/logcat-adb.txt", out), run(env!("CARGO_BIN_EXE_weft"), &["--state-dir", &dir, "--name", "android", "logcat", "-d", "-t", "3000"]).1).unwrap();
        let q = "ls -l /dev/dri /vendor/lib64/hw /vendor/bin/hw /vendor/lib64/egl /apex 2>&1; ps -A -o NAME | sort | tr '\\n' ' '; cat /proc/modules | cut -d' ' -f1 | sort | tr '\\n' ' '";
        std::fs::write(format!("{}/invitado.txt", out), adbsh(q).1).unwrap();
        if booted {
            std::thread::sleep(Duration::from_secs(20));
            let png = Command::new(env!("CARGO_BIN_EXE_weft")).args(["--state-dir", &dir, "--name", "android", "adb-shell", "screencap -p"]).output().map(|o| o.stdout).unwrap_or_default();
            std::fs::write(format!("{}/screencap.png", out), png).unwrap();
        }
    } else {
        // sin adb: lo que se pueda por el interprete de la consola serie
        let q = "getprop | grep -E 'adb|boot|hardware|egl|init.svc' ; ls -l /dev/dri /vendor/lib64/hw /vendor/bin/hw 2>&1; ip addr";
        std::fs::write(format!("{}/consola-diagnostico.txt", out), emu(&["sh", q, "--timeout", "20"]).1).unwrap();
    }
    println!("{}", info);
    std::fs::write(format!("{}/android.txt", out), &info).unwrap();
    emu(&["screenshot", &format!("{}/final.png", out)]);
    std::fs::write(format!("{}/serie.txt", out), run(env!("CARGO_BIN_EXE_weft"), &["--state-dir", &dir, "--name", "android", "serial"]).1).unwrap();
    let _ = std::fs::copy(format!("{}/qemu.log", dir), format!("{}/qemu.log", out));
    emu(&["stop", "--timeout", "5"]);
    assert!(booted, "Android no termino de arrancar en {} s", limit);
}
