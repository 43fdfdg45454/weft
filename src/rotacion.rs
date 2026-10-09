//! La ventana propia sigue la rotacion que Android decide por su cuenta (una app que fuerza apaisado, el ajuste de
//! rotacion del usuario...), sin tocar el acelerometro del servicio de sensores.
//!
//! Se lee la rotacion de la pantalla de Android por el adb propio cada 2 s: `dumpsys display | grep -m1
//! mCurrentOrientation` (0..3 = Surface.ROTATION_*; mismo valor que `mCurrentRotation` de `dumpsys window displays`, que
//! se usa de respaldo). Costo medido: ~6 ms por consulta de punta a punta (~3 ms la conexion vsock) contra ~22 ms con
//! `dumpsys window displays`; a una consulta cada 2 s es despreciable (0,3 % de un nucleo del anfitrion y unos pocos ms de
//! CPU del invitado).
//!
//! El hilo se puede apagar con `pantalla.giro_android=no` en `config` (la pantalla de configuracion: "Seguir la rotacion de
//! Android"): entonces no consulta nada y, con la orientacion en `auto`, la ventana queda vertical.
//!
//! La rotacion informada se guarda en `android-rotation` cada vez que cambia. La ventana la usa solo cuando la orientacion
//! pedida es `auto` (pantalla::rotacion_efectiva): con una orientacion fija (`rotate`, F7, panel) manda lo pedido y la
//! ventana queda girada aunque Android no gire; ya no hay retroceso por tiempo.

use crate::adb;
use crate::pantalla;
use std::path::PathBuf;
use std::time::Duration;

pub const PERIODO: Duration = Duration::from_secs(2);

const CONSULTA: &str = "dumpsys display | grep -m1 mCurrentOrientation";
const CONSULTA_RESPALDO: &str = "dumpsys window displays | grep -m1 mCurrentRotation";

/// "mCurrentOrientation=1" o "mCurrentRotation=ROTATION_90" -> 0..3.
pub fn parse(texto: &str) -> Option<u32> {
    for l in texto.lines() {
        if let Some(v) = l.trim().strip_prefix("mCurrentOrientation=") {
            return v.trim().parse::<u32>().ok().filter(|r| *r < 4);
        }
        if let Some(v) = l.trim().strip_prefix("mCurrentRotation=ROTATION_") {
            return pantalla::rot_de_grados(v.trim().parse().ok()?);
        }
    }
    None
}

/// Si hay que anotar la rotacion de Android: solo cuando cambia respecto de la ultima anotada.
pub fn debe_anotar(observada: u32, anotada: Option<u32>) -> bool {
    anotada != Some(observada)
}

/// Bucle del hilo de la ventana: no termina nunca (muere con el proceso). Sin adbd (la maquina arrancando o sin vsock)
/// no hace nada.
pub fn seguir(dir: PathBuf, cid: u32) {
    let mut sin_formato = 0u32;
    loop {
        std::thread::sleep(PERIODO);
        // con "seguir la rotacion de Android" apagado (config: pantalla.giro_android) no se consulta nada
        if !crate::config::Config::cargar(&dir).bool("pantalla.giro_android") {
            continue;
        }
        let consulta = if sin_formato >= 3 { CONSULTA_RESPALDO } else { CONSULTA };
        let Ok((_, salida, _)) = adb::conectar(cid, 5).and_then(|mut c| c.shell_texto(consulta)) else {
            // adbd todavia no escucha o la maquina se esta reiniciando: reintento mas espaciado
            std::thread::sleep(Duration::from_secs(3));
            continue;
        };
        let Some(obs) = parse(&salida) else {
            sin_formato += 1;
            continue;
        };
        if debe_anotar(obs, pantalla::leer_rotacion_android(&dir)) {
            let _ = pantalla::escribir_rotacion_android(&dir, obs);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lectura() {
        assert_eq!(parse("    mCurrentOrientation=1\n"), Some(1));
        assert_eq!(parse("      mCurrentRotation=ROTATION_270\n"), Some(3));
        assert_eq!(parse("      mCurrentRotation=ROTATION_180"), Some(2));
        assert_eq!(parse("mCurrentOrientation=7"), None);
        assert_eq!(parse("mCurrentRotation=ROTATION_45"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("Failed to write while dumping service display: Broken pipe"), None);
    }

    #[test]
    fn decision() {
        // Android gira solo: se anota; lo mismo que ya esta anotado, no
        assert!(debe_anotar(1, Some(0)));
        assert!(debe_anotar(0, None));
        assert!(!debe_anotar(1, Some(1)));
        assert!(!debe_anotar(0, Some(0)));
    }
}
