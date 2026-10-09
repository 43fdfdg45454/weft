//! Prueba de extremo a extremo con QEMU real. Se activa con WEFT_E2E=1 y necesita:
//!   WEFT_KERNEL  kernel Linux x86_64 (bzImage)        WEFT_INITRD  invitado de ci/make-guest.sh
//!   WEFT_ACCEL   kvm | tcg | auto (por defecto auto)
//! Todo se maneja con la linea de comandos de weft, igual que lo haria una persona.

use std::process::Command;

struct Out {
    code: i32,
    text: String,
}

fn emu(dir: &str, args: &[&str]) -> Out {
    let o = Command::new(env!("CARGO_BIN_EXE_weft")).args(["--state-dir", dir, "--name", "e2e"]).args(args).output().expect("ejecutar weft");
    let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    println!("$ weft {} -> {}\n{}", args.join(" "), o.status.code().unwrap_or(-1), text.trim_end());
    Out { code: o.status.code().unwrap_or(-1), text }
}

fn ok(dir: &str, args: &[&str]) -> String {
    let o = emu(dir, args);
    assert_eq!(o.code, 0, "fallo: weft {}", args.join(" "));
    o.text
}

#[test]
fn maquina_completa() {
    if std::env::var("WEFT_E2E").as_deref() != Ok("1") {
        eprintln!("(prueba con QEMU omitida: define WEFT_E2E=1)");
        return;
    }
    let kernel = std::env::var("WEFT_KERNEL").expect("WEFT_KERNEL");
    let initrd = std::env::var("WEFT_INITRD").expect("WEFT_INITRD");
    let accel = std::env::var("WEFT_ACCEL").unwrap_or_else(|_| "auto".into());
    let tmp = std::env::temp_dir().join(format!("weft-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let dir = tmp.to_str().unwrap();
    let start = ["start", "--kernel", &kernel, "--initrd", &initrd, "--append", "console=tty1 console=ttyS0 panic=-1", "--mem", "512", "--accel", &accel];

    // 1) arranque y consola serie
    let s = ok(dir, &start);
    if accel == "kvm" {
        assert!(s.contains("aceleracion=kvm"), "se esperaba KVM");
    }
    let r = emu(dir, &["wait-serial", "WEFT-BOOT-OK", "--timeout", "180"]);
    if r.code != 0 {
        emu(dir, &["serial"]);
        emu(dir, &["kill"]);
        panic!("el invitado no arranco");
    }

    // 2) estado
    let s = ok(dir, &["status"]);
    assert!(s.contains("estado=running"));
    if accel == "kvm" {
        assert!(s.contains("kvm=si"));
    }
    assert!(ok(dir, &["qmp", "query-version"]).contains("qemu"));
    assert!(emu(dir, &start).code != 0, "un segundo arranque debe rechazarse");

    // 3) ordenes dentro del invitado, con su salida y su codigo
    assert!(ok(dir, &["sh", "echo hola-$((6*7))"]).contains("hola-42"));
    assert_eq!(emu(dir, &["sh", "false"]).code, 1);
    assert_eq!(emu(dir, &["sh", "(exit 7)"]).code, 7);
    assert!(ok(dir, &["sh", "cat /proc/cpuinfo | grep -c processor"]).contains('2'));

    // 4) teclado emulado: lo escrito llega al invitado
    ok(dir, &["type", "weft_Key-1\n"]);
    ok(dir, &["wait-serial", "WEFT-KEY:weft_Key-1", "--timeout", "20"]);
    ok(dir, &["key", "a", "shift-b", "ret"]);
    ok(dir, &["wait-serial", "WEFT-KEY:aB", "--timeout", "20"]);

    // 5) pantalla y puntero
    let shot = tmp.join("pantalla.ppm");
    let s = ok(dir, &["screenshot", shot.to_str().unwrap()]);
    assert!(s.contains("captura ") && !s.contains("colores=1\n"), "la pantalla deberia tener texto");
    let png = tmp.join("pantalla.png");
    assert!(ok(dir, &["screenshot", png.to_str().unwrap()]).contains(" png"));
    if let Ok(keep) = std::env::var("WEFT_KEEP") {
        let _ = std::fs::copy(&png, keep);
    }
    ok(dir, &["tap", "0.5", "0.5"]);
    assert!(emu(dir, &["tap", "2", "0"]).code != 0);

    // 6) apagado desde el invitado
    ok(dir, &["serial-send", "poweroff -f"]);
    ok(dir, &["wait-exit", "--timeout", "40"]);
    assert_eq!(emu(dir, &["status"]).code, 1);

    // 7) apagado desde fuera (el invitado de prueba no atiende el boton: se cierra la maquina)
    ok(dir, &start);
    ok(dir, &["wait-serial", "WEFT-BOOT-OK", "--timeout", "180"]);
    ok(dir, &["stop", "--timeout", "3"]);
    assert_eq!(emu(dir, &["status"]).code, 1);
    let _ = std::fs::remove_dir_all(&tmp);
}
