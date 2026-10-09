//! Reinicio seguro de la maquina y de Android.
//!
//! POR QUE NO `system_reset`: reinicia el hardware virtual sin que Android cierre sus contextos graficos. El renderizador
//! gfxstream del anfitrion conserva el estado de la sesion anterior (contextos y anillos address-space) y el Android nuevo
//! no consigue hablar con el: SurfaceFlinger se queda esperando para siempre y el arranque nunca termina (reproducido 2 de
//! 2 veces; ver BITACORA.md, 2026-10-07). Solo un apagado completo y un arranque nuevo limpian el renderizador. Por eso la
//! interfaz NO ofrece (ni llama a) `system_reset`; hay dos vias:
//!
//!   1. REINICIO ORDENADO de Android por el adb propio (lo mismo que `weft reboot`): Android cierra sus contextos y
//!      reinicia sin que la maquina se apague.
//!   2. REINICIO COMPLETO de la maquina (`weft restart`): apagado y arranque nuevo con los mismos argumentos con los
//!      que se arranco (se guardan en el archivo `start-args` del estado al hacer `start`). Es la via cuando adbd no responde.
//!
//! Este modulo no depende de SDL: lo usan la orden `restart`, el boton Reiniciar del panel y la pantalla de configuracion.

use crate::adb;
use std::path::Path;
use crate::textos::{tx, txf};

/// Segundos que se espera a que adbd conteste antes de dar el reinicio ordenado por imposible.
pub const ADB_ESPERA_S: u32 = 4;

/// Variables de entorno que se recuerdan para el reinicio completo (ademas de las que empiezan por WEFT_ o SDL_):
/// lo que necesita la ventana y el audio para salir en el mismo escritorio.
pub const ENTORNO_GUARDADO: &[&str] = &["WAYLAND_DISPLAY", "DISPLAY", "XDG_RUNTIME_DIR", "XAUTHORITY", "XDG_SESSION_TYPE", "DBUS_SESSION_BUS_ADDRESS"];

/// Que hacer ante una peticion de reiniciar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// la maquina no esta en marcha: no hay nada que reiniciar
    MaquinaApagada,
    /// reinicio ordenado de Android por adb
    Ordenado,
    /// adbd no responde: ofrecer el reinicio completo de la maquina (con confirmacion)
    OfrecerCompleto,
}

/// Decision de una peticion de reiniciar: `adb_responde` es el resultado de intentar hablar con adbd.
pub fn decidir(maquina_en_marcha: bool, adb_responde: bool) -> Plan {
    match (maquina_en_marcha, adb_responde) {
        (false, _) => Plan::MaquinaApagada,
        (true, true) => Plan::Ordenado,
        (true, false) => Plan::OfrecerCompleto,
    }
}

/// Reinicio ordenado de Android: el servicio `reboot:` de adbd (lo que hace `weft reboot`). Falla si adbd no
/// responde en `ADB_ESPERA_S` s; en ese caso la interfaz ofrece el reinicio completo.
pub fn ordenado(cid: u32) -> Result<(), String> {
    let mut c = adb::conectar(cid, ADB_ESPERA_S)?;
    // adbd cierra la conexion al reiniciar: un error de lectura aqui es lo normal
    let _ = c.servicio_texto("reboot:");
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// la linea de arranque guardada

/// Con que se arranco la maquina: argumentos de `start`, carpeta de trabajo y entorno que importa.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Arranque {
    pub cwd: String,
    /// argumentos de `start` tal como se dieron (sin la palabra `start`)
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

pub const ARCHIVO: &str = "start-args";

impl Arranque {
    /// Recuerda lo que esta en el entorno de la orden `start`: WEFT_*, SDL_* y las variables de escritorio.
    pub fn del_entorno(cwd: &str, args: &[String], entorno: impl Iterator<Item = (String, String)>) -> Arranque {
        let env = entorno.filter(|(k, _)| k.starts_with("WEFT_") || k.starts_with("SDL_") || ENTORNO_GUARDADO.contains(&k.as_str())).collect();
        Arranque { cwd: cwd.to_string(), args: args.to_vec(), env }
    }

    /// Texto del archivo: registros separados por NUL (un argumento puede llevar cualquier caracter salvo NUL).
    pub fn serializar(&self) -> Vec<u8> {
        let mut v: Vec<u8> = Vec::new();
        let mut reg = |t: String| {
            v.extend_from_slice(t.as_bytes());
            v.push(0);
        };
        reg("weft-arranque 1".to_string());
        reg(format!("cwd={}", self.cwd));
        for (k, val) in &self.env {
            reg(format!("env={}={}", k, val));
        }
        for a in &self.args {
            reg(format!("arg={}", a));
        }
        v
    }

    pub fn parsear(datos: &[u8]) -> Result<Arranque, String> {
        let mut it = datos.split(|b| *b == 0).filter(|r| !r.is_empty()).map(|r| String::from_utf8_lossy(r).into_owned());
        if it.next().as_deref() != Some("weft-arranque 1") {
            return Err(tx!("reinicio.el_archivo_start_args_no_tiene_el").into());
        }
        let mut a = Arranque::default();
        let mut cwd = false;
        for r in it {
            if let Some(c) = r.strip_prefix("cwd=") {
                a.cwd = c.to_string();
                cwd = true;
            } else if let Some(e) = r.strip_prefix("env=") {
                let (k, v) = e.split_once('=').ok_or(tx!("reinicio.archivo_start_args_variable_sin_valor"))?;
                a.env.push((k.to_string(), v.to_string()));
            } else if let Some(x) = r.strip_prefix("arg=") {
                a.args.push(x.to_string());
            }
        }
        if !cwd {
            return Err(tx!("reinicio.el_archivo_start_args_no_dice_la_carpeta").into());
        }
        Ok(a)
    }

    pub fn guardar(&self, dir: &Path) -> Result<(), String> {
        let tmp = dir.join(format!("{}.tmp", ARCHIVO));
        std::fs::write(&tmp, self.serializar()).and_then(|_| std::fs::rename(&tmp, dir.join(ARCHIVO))).map_err(|e| txf!("reinicio.no_se_pudo_guardar_la_linea_de_arranque", e))
    }

    pub fn leer(dir: &Path) -> Result<Arranque, String> {
        let d = std::fs::read(dir.join(ARCHIVO)).map_err(|_| tx!("reinicio.no_hay_linea_de_arranque_guardada_esta").to_string())?;
        Arranque::parsear(&d)
    }

    /// Variables que hay que poner antes de arrancar: las guardadas que la orden `restart` no trae ya en su entorno
    /// (lo que se da al reiniciar manda sobre lo guardado).
    pub fn faltantes(&self, actual: impl Fn(&str) -> bool) -> Vec<(String, String)> {
        self.env.iter().filter(|(k, _)| !actual(k)).cloned().collect()
    }

    /// El arranque pidio la ventana propia.
    pub fn con_ventana(&self) -> bool {
        self.args.windows(2).any(|w| w[0] == "--display" && w[1] == "window")
    }
}

/// El arranque fallo por la carrera conocida del CID de vsock: justo tras apagar, el kernel tarda un instante en
/// soltar el CID y QEMU dice "unable to set guest cid: Address already in use".
pub fn es_carrera_de_cid(mensaje: &str) -> bool {
    let m = mensaje.to_lowercase();
    m.contains("unable to set guest cid") || (m.contains("vhost-vsock") && m.contains("address already in use"))
}

/// El arranque fallo porque el puerto local del adb por TCP ya estaba ocupado: el reenvio de QEMU (`hostfwd`) dice "Could
/// not set up host forwarding rule" (con ": Address already in use" en las versiones nuevas). Pasa si otro programa toma
/// el puerto entre que `start` lo ve libre y QEMU lo abre. Hace falta la prueba del reenvio en la misma linea: un "Address
/// already in use" de otra cosa (el CID de vsock, ver `es_carrera_de_cid`; el socket de virtiofsd o de VNC) no cuenta.
pub fn es_puerto_ocupado(mensaje: &str) -> bool {
    mensaje.to_lowercase().lines().any(|l| l.contains("could not set up host forwarding") || (l.contains("hostfwd") && l.contains("address already in use")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Los dos fallos conocidos de arranque con "Address already in use" no se confunden: el del CID de vsock y el del
    /// puerto del adb por TCP.
    #[test]
    fn fallos_de_cid_y_de_puerto() {
        let cid = "QEMU no arranco (exit status: 1):\nqemu-system-x86_64: -device vhost-vsock-pci,guest-cid=3: vhost-vsock: unable to set guest cid: Address already in use";
        assert!(es_carrera_de_cid(cid) && !es_puerto_ocupado(cid));
        let viejo = "QEMU no arranco (exit status: 1):\nqemu-system-x86_64: -netdev user,id=n2,hostfwd=tcp:127.0.0.1:15555-:5555: Could not set up host forwarding rule 'tcp:127.0.0.1:15555-:5555'";
        let nuevo = format!("{}: Address already in use", viejo);
        for m in [viejo, nuevo.as_str()] {
            assert!(es_puerto_ocupado(m) && !es_carrera_de_cid(m), "{}", m);
        }
        assert!(!es_puerto_ocupado("QEMU no arranco (exit status: 1):\nqemu-system-x86_64: could not open disk image"));
        assert!(!es_carrera_de_cid("Could not set up host forwarding rule"));
        // con la opcion del reenvio y el error en la misma linea (otra forma de decirlo)
        assert!(es_puerto_ocupado("qemu-system-x86_64: -netdev user,id=n0,hostfwd=tcp:127.0.0.1:5600-:5555: Address already in use"));
        // un "Address already in use" de otra cosa no es el puerto del adb, aunque el reenvio aparezca en otra linea
        for otro in [
            "virtiofsd no arranco:\n[ERROR virtiofsd] Error creating listener: Address already in use (os error 98)",
            "QEMU no arranco (exit status: 1):\nqemu-system-x86_64: -vnc unix:/r/m/vnc.sock: Failed to bind socket: Address already in use",
            "QEMU no arranco (exit status: 1):\nqemu-system-x86_64: aviso: -netdev user,id=n0,hostfwd=tcp:127.0.0.1:5600-:5555\nqemu-system-x86_64: -chardev socket,id=c0,path=/r/m/qmp.sock: Address already in use",
        ] {
            assert!(!es_puerto_ocupado(otro), "{}", otro);
        }
    }

    #[test]
    fn decision_de_reinicio() {
        assert_eq!(decidir(false, false), Plan::MaquinaApagada);
        assert_eq!(decidir(false, true), Plan::MaquinaApagada);
        assert_eq!(decidir(true, true), Plan::Ordenado);
        assert_eq!(decidir(true, false), Plan::OfrecerCompleto);
    }

    #[test]
    fn la_linea_de_arranque_ida_y_vuelta() {
        let args: Vec<String> = ["--kernel", "arranque/kernel", "--display", "window", "--append", "a b\tc\nd", "--resolution", "720x1348"].iter().map(|s| s.to_string()).collect();
        let entorno = vec![
            ("WEFT_WINDOW_INJECT".to_string(), "1".to_string()),
            ("SDL_VIDEO_DRIVER".to_string(), "offscreen".to_string()),
            ("WAYLAND_DISPLAY".to_string(), "wayland-0".to_string()),
            ("HOME".to_string(), "/no/se/guarda".to_string()),
            ("PATH".to_string(), "/tampoco".to_string()),
        ];
        let a = Arranque::del_entorno("/trabajo", &args, entorno.into_iter());
        // solo se recuerda lo que importa
        assert_eq!(a.env.len(), 3);
        assert!(a.env.iter().all(|(k, _)| k != "HOME" && k != "PATH"));
        let b = Arranque::parsear(&a.serializar()).unwrap();
        assert_eq!(a, b);
        // un argumento con espacios, tabuladores y saltos de linea sobrevive
        assert_eq!(b.args[5], "a b\tc\nd");
        assert!(b.con_ventana());
        let sin = Arranque { args: vec!["--display".into(), "none".into()], ..b.clone() };
        assert!(!sin.con_ventana());
    }

    #[test]
    fn archivo_ilegible_o_incompleto() {
        assert!(Arranque::parsear(b"basura").is_err());
        assert!(Arranque::parsear(b"weft-arranque 1\0arg=--x\0").is_err(), "sin carpeta de trabajo");
        assert!(Arranque::parsear(b"weft-arranque 1\0cwd=/x\0env=SIN_VALOR\0").is_err());
        assert!(Arranque::parsear(b"otro-arranque 1\0cwd=/x\0arg=--x\0").is_err(), "encabezado desconocido");
        let ok = Arranque::parsear(b"weft-arranque 1\0cwd=/x\0").unwrap();
        assert!(ok.args.is_empty() && ok.cwd == "/x");
    }

    #[test]
    fn lo_guardado_no_pisa_lo_que_ya_hay() {
        let a = Arranque { cwd: "/x".into(), args: vec![], env: vec![("SDL_VIDEO_DRIVER".into(), "offscreen".into()), ("WEFT_GFX_DIR".into(), "/g".into())] };
        let f = a.faltantes(|k| k == "SDL_VIDEO_DRIVER");
        assert_eq!(f, vec![("WEFT_GFX_DIR".to_string(), "/g".to_string())]);
        assert_eq!(a.faltantes(|_| false).len(), 2);
    }

    #[test]
    fn carrera_del_cid() {
        assert!(es_carrera_de_cid("QEMU no arranco (exit status: 1):\nqemu-system-x86_64: -device vhost-vsock-pci,guest-cid=3: vhost-vsock: unable to set guest cid: Address already in use"));
        assert!(!es_carrera_de_cid("QEMU no arranco: no se encontro el disco"));
    }

    /// La interfaz nunca debe volver a mandar `system_reset`: ni las acciones, ni la pantalla de configuracion, ni la ventana.
    #[test]
    fn la_interfaz_no_manda_system_reset() {
        let aguja = concat!("\"system", "_reset\"");
        for (nombre, fuente) in [("vista.rs", include_str!("vista.rs")), ("ajustes.rs", include_str!("ajustes.rs")), ("window.rs", include_str!("window.rs")), ("barra.rs", include_str!("barra.rs"))] {
            assert!(!fuente.contains(aguja), "{} manda system_reset", nombre);
        }
    }

    #[test]
    fn guardar_y_leer_en_disco() {
        let d = std::env::temp_dir().join(format!("ar-reinicio-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        assert!(Arranque::leer(&d).is_err());
        let a = Arranque { cwd: "/w".into(), args: vec!["--mem".into(), "4096".into()], env: vec![] };
        a.guardar(&d).unwrap();
        assert_eq!(Arranque::leer(&d).unwrap(), a);
        let _ = std::fs::remove_dir_all(&d);
    }
}
