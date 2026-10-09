//! Apagado de la maquina: primero ORDENADO por el adb propio, despues el boton ACPI y por ultimo `quit` y SIGKILL.
//!
//! POR QUE: el Android de la imagen de prueba ignora el apagado ACPI (`system_powerdown`: nadie en el invitado atiende el
//! boton de encendido), de modo que `stop` y `restart` esperaban el tiempo completo (20-30 s) y terminaban con `quit`, que
//! cierra QEMU sin que Android haya vaciado sus escrituras. Con un apagado pedido a Android por adb, init cierra los
//! servicios, sincroniza y desmonta los discos y apaga el hardware virtual (ACPI S5 del kernel), con lo que QEMU termina
//! solo y rapido, sin perder datos.
//!
//! Escalera (la misma para `stop`, `restart` y el boton Apagar de la interfaz, que lanza `weft stop`):
//!   1. apagado ordenado por adb (`ADB_ESPERA_S` para que adbd conteste) y espera a que QEMU termine;
//!   2. si adbd no responde o QEMU no termina: boton de apagado ACPI (`system_powerdown`) y espera;
//!   3. QMP `quit`;
//!   4. SIGKILL.
//!
//! Este modulo no toca SDL ni QMP: solo sabe pedir el apagado por adb y decidir el plan; `main.rs` ejecuta la escalera.

use crate::adb;
use crate::textos::txf;

/// Segundos que se espera a que adbd conteste antes de pasar al apagado ACPI.
pub const ADB_ESPERA_S: u32 = 4;

/// Formas de pedirle a Android que se apague por adb. Todas acaban en `sys.powerctl=shutdown` de init; cambia quien lo pone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metodo {
    /// servicio `reboot:` de adbd con argumento `shutdown`
    ServicioReboot,
    /// `svc power shutdown`: el PowerManager de Android (cierra el sistema por la ruta de la interfaz)
    SvcPower,
    /// `reboot -p`
    RebootP,
    /// `setprop sys.powerctl shutdown`
    Powerctl,
}

impl Metodo {
    pub const TODOS: [Metodo; 4] = [Metodo::ServicioReboot, Metodo::SvcPower, Metodo::RebootP, Metodo::Powerctl];

    pub fn nombre(self) -> &'static str {
        match self {
            Metodo::ServicioReboot => "reboot-servicio",
            Metodo::SvcPower => "svc-power",
            Metodo::RebootP => "reboot-p",
            Metodo::Powerctl => "powerctl",
        }
    }

    pub fn parse(s: &str) -> Option<Metodo> {
        Metodo::TODOS.into_iter().find(|m| m.nombre() == s)
    }
}

/// Metodo por defecto (medido; ver BITACORA.md). `WEFT_APAGADO=NOMBRE` fuerza otro, solo para pruebas.
pub const DEFECTO: Metodo = Metodo::Powerctl;

pub fn metodo_en_uso() -> Metodo {
    std::env::var("WEFT_APAGADO").ok().and_then(|v| Metodo::parse(&v)).unwrap_or(DEFECTO)
}

/// Pide a Android que se apague por adb con el metodo dado. Falla si adbd no responde en `ADB_ESPERA_S` o si la orden
/// fallo de forma explicita (ver `interpretar`): solo un Ok significa que el apagado quedo pedido.
pub fn pedir(cid: u32, m: Metodo) -> Result<(), String> {
    pedir_en(&mut adb::conectar(cid, ADB_ESPERA_S)?, m)
}

/// `pedir` sobre una conexion con adbd ya abierta (las pruebas le dan un adbd simulado).
fn pedir_en<T: std::io::Read + std::io::Write>(c: &mut adb::Adb<T>, m: Metodo) -> Result<(), String> {
    let shell = |c: &mut adb::Adb<T>, orden: &str| c.shell_texto(orden).map(|(codigo, o, e)| (codigo, format!("{}{}", o, e)));
    let r = match m {
        // el servicio no da codigo: adbd contesta "reboot (...) failed" si no pudo
        Metodo::ServicioReboot => c.servicio_texto("reboot:shutdown").map(|t| (if t.to_lowercase().contains("failed") { 1 } else { 0 }, t)),
        Metodo::SvcPower => shell(c, "svc power shutdown"),
        Metodo::RebootP => shell(c, "reboot -p"),
        Metodo::Powerctl => shell(c, "setprop sys.powerctl shutdown"),
    };
    interpretar(r, c.aceptados() > 0)
}

/// Que significa lo que contesto adbd a la orden de apagado (`r`: Ok con el codigo de salida y el texto si la orden
/// termino, Err si no; `aceptada`: adbd acepto el servicio, es decir, la orden llego a Android). Codigo 0: pedido. Otro
/// codigo: la orden fallo (p. ej. `setprop` sin permiso) y el apagado NO quedo pedido. Sin codigo porque adbd cerro la
/// conexion (o dejo de contestar) despues de aceptar la orden: lo normal al apagarse, adbd muere con el sistema, y cuenta
/// como pedido. Si adbd no llego a aceptarla (no contesto al OPEN o rechazo el servicio), no se pidio nada. Pura.
pub fn interpretar(r: Result<(i32, String), String>, aceptada: bool) -> Result<(), String> {
    match r {
        Ok((0, _)) => Ok(()),
        Ok((codigo, t)) => Err(txf!("apagado.la_orden_de_apagado_fallo_codigo", codigo, if t.trim().is_empty() { String::new() } else { format!(": {}", t.trim()) })),
        Err(e) if !aceptada => Err(e),
        Err(_) => Ok(()),
    }
}

/// Un paso de la escalera.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paso {
    Adb,
    Acpi,
    Quit,
    Kill,
}

/// Orden de los pasos y cuanto se espera tras cada uno. `adb_pidio`: el paso 1 consiguio pedir el apagado (`pedir` dio Ok:
/// la orden se acepto, no basta con que adbd conectara);
/// si QEMU no termino pese a eso, el boton ACPI solo se espera poco porque el sistema ya esta apagandose. `secs` es el
/// limite de la espera tras cada paso de apagado (como `--timeout`).
pub fn plan(secs: u64, adb_pidio: bool) -> Vec<(Paso, u64)> {
    vec![(Paso::Adb, secs), (Paso::Acpi, if adb_pidio { secs.min(5) } else { secs }), (Paso::Quit, 5), (Paso::Kill, 5)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nombres_y_defecto() {
        for m in Metodo::TODOS {
            assert_eq!(Metodo::parse(m.nombre()), Some(m));
        }
        assert_eq!(Metodo::parse("otro"), None);
        assert!(Metodo::TODOS.contains(&DEFECTO));
    }

    /// Solo cuenta como pedido lo que adbd acepto: codigo 0, o la conexion cortada al apagarse; un codigo de error o un
    /// servicio rechazado no (y entonces el boton ACPI tiene el tiempo completo).
    #[test]
    fn resultado_de_la_orden_de_apagado() {
        assert_eq!(interpretar(Ok((0, String::new())), true), Ok(()));
        // setprop sin permiso: codigo 1 y el motivo en el error
        let e = interpretar(Ok((1, "Failed to set property 'sys.powerctl' to 'shutdown'.\n".into())), true).unwrap_err();
        assert!(e.contains("codigo 1") && e.contains("Failed to set property"), "{}", e);
        assert_eq!(interpretar(Ok((255, " ".into())), true), Err("la orden de apagado fallo (codigo 255)".to_string()));
        // adbd murio con el sistema justo despues de aceptar la orden: pedido
        assert_eq!(interpretar(Err("lectura: adbd cerro la conexion".into()), true), Ok(()));
        assert_eq!(interpretar(Err("lectura: adbd no respondio a tiempo".into()), true), Ok(()));
        assert_eq!(interpretar(Err("adbd cerro el servicio antes de terminar la respuesta".into()), true), Ok(()));
        // adbd no acepto la orden (la rechazo, o conecto pero no contesto al OPEN): no se pidio nada
        assert!(interpretar(Err("adbd rechazo el servicio \"shell,v2,raw\"".into()), false).is_err());
        assert_eq!(interpretar(Err("lectura: adbd no respondio a tiempo".into()), false), Err("lectura: adbd no respondio a tiempo".to_string()));
        assert!(interpretar(Err("lectura: adbd cerro la conexion".into()), false).is_err());
    }

    /// Lo que hace el adbd simulado con la orden de apagado.
    #[derive(Clone, Copy)]
    enum Adbd {
        /// la ejecuta y termina con este codigo (y este texto en stderr, o en la respuesta del servicio `reboot:`)
        Termina(u8, &'static str),
        /// la acepta y muere enseguida (lo normal: adbd cae con el sistema)
        MuereTrasAceptar,
        /// conecta pero no contesta al OPEN (adbd colgado)
        NoContesta,
        /// rechaza el servicio
        Rechaza,
    }

    /// adbd simulado con shell v2 por un extremo de `UnixStream::pair` (como las pruebas de adb.rs).
    fn adbd(mut s: std::os::unix::net::UnixStream, modo: Adbd) -> std::thread::JoinHandle<()> {
        use std::io::{Read, Write};
        std::thread::spawn(move || loop {
            let mut cab = [0u8; 24];
            if s.read_exact(&mut cab).is_err() {
                return;
            }
            let h = adb::decode_header(&cab).unwrap();
            let mut d = vec![0u8; h.len as usize];
            if s.read_exact(&mut d).is_err() {
                return;
            }
            let r = match (h.cmd, modo) {
                (adb::A_CNXN, _) => adb::encode(adb::A_CNXN, adb::VERSION, 4096, b"device::ro.product.name=x;features=shell_v2,cmd\0"),
                (adb::A_OPEN, Adbd::NoContesta) => continue,
                (adb::A_OPEN, Adbd::Rechaza) => adb::encode(adb::A_CLSE, 0, h.arg0, &[]),
                (adb::A_OPEN, Adbd::MuereTrasAceptar) => {
                    let _ = s.write_all(&adb::encode(adb::A_OKAY, 9, h.arg0, &[]));
                    return;
                }
                (adb::A_OPEN, Adbd::Termina(codigo, texto)) => {
                    let datos = if d.starts_with(b"reboot:") { texto.as_bytes().to_vec() } else { [adb::shell_packet(adb::SH_STDERR, texto.as_bytes()), adb::shell_packet(adb::SH_EXIT, &[codigo])].concat() };
                    [adb::encode(adb::A_OKAY, 9, h.arg0, &[]), adb::encode(adb::A_WRTE, 9, h.arg0, &datos), adb::encode(adb::A_CLSE, 9, h.arg0, &[])].concat()
                }
                _ => continue,
            };
            if s.write_all(&r).is_err() {
                return;
            }
        })
    }

    /// La orden de apagado contra un adbd simulado: solo cuenta como pedida si adbd la acepto y no fallo (B.15: antes
    /// bastaba con que adbd conectara, aunque `setprop` fallara, y el boton ACPI solo tenia 5 s).
    #[test]
    fn pedir_contra_un_adbd_simulado() {
        let probar = |m: Metodo, modo: Adbd| {
            let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
            // el limite de `pedir` (ADB_ESPERA_S), mas corto para la prueba
            a.set_read_timeout(Some(std::time::Duration::from_millis(500))).unwrap();
            let h = adbd(b, modo);
            let mut c = adb::Adb::handshake(a).unwrap();
            let r = pedir_en(&mut c, m);
            drop(c);
            h.join().unwrap();
            r
        };
        assert_eq!(probar(Metodo::Powerctl, Adbd::Termina(0, "")), Ok(()));
        let e = probar(Metodo::Powerctl, Adbd::Termina(1, "Failed to set property 'sys.powerctl' to 'shutdown'.")).unwrap_err();
        assert!(e.contains("codigo 1") && e.contains("Failed to set property"), "{}", e);
        assert_eq!(probar(Metodo::SvcPower, Adbd::MuereTrasAceptar), Ok(()));
        assert!(probar(Metodo::Powerctl, Adbd::NoContesta).unwrap_err().contains("no respondio a tiempo"));
        assert!(probar(Metodo::RebootP, Adbd::Rechaza).unwrap_err().contains("rechazo el servicio"));
        // el servicio reboot: no da codigo; "failed" en su respuesta es un fallo
        assert_eq!(probar(Metodo::ServicioReboot, Adbd::Termina(0, "")), Ok(()));
        assert!(probar(Metodo::ServicioReboot, Adbd::Termina(0, "reboot (shutdown) failed: Operation not permitted")).unwrap_err().contains("codigo 1"));
    }

    #[test]
    fn escalera_del_apagado() {
        // sin respuesta de adbd: el ACPI espera el tiempo completo
        assert_eq!(plan(20, false), vec![(Paso::Adb, 20), (Paso::Acpi, 20), (Paso::Quit, 5), (Paso::Kill, 5)]);
        // adbd pidio el apagado y QEMU no termino: el ACPI solo da 5 s mas
        assert_eq!(plan(20, true)[1], (Paso::Acpi, 5));
        assert_eq!(plan(3, true)[1], (Paso::Acpi, 3));
        // el orden nunca cambia: adb, ACPI, quit, kill
        let orden: Vec<Paso> = plan(1, false).into_iter().map(|p| p.0).collect();
        assert_eq!(orden, vec![Paso::Adb, Paso::Acpi, Paso::Quit, Paso::Kill]);
    }
}
