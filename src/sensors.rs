//! Servicio de sensores para Cuttlefish. En esa imagen el servicio de sensores de Android no simula nada por si
//! mismo: pregunta al anfitrion por dos consolas virtio (control y datos). Sin respuesta, Android no termina de
//! arrancar. Aqui se responde con un telefono en reposo, en vertical.
//!
//! Trama: u32 (orden en los 31 bits bajos, bit alto = respuesta), u32 tamano, datos. Texto terminado en '\n'.

use std::io::{Read, Write};
use crate::textos::tx;

pub const CMD_UPDATE_HAL: u32 = 2;

/// Trazado opcional (WEFT_SENSORS_TRACE=1): una linea por trama en stderr, con la hora en milisegundos. Sirve para ver
/// la conversacion con el HAL de sensores del invitado cuando un arranque se queda esperando a `sensorservice`.
fn traza(msg: impl FnOnce() -> String) {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("WEFT_SENSORS_TRACE").is_some()) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        eprintln!("{}.{:03} {}", t / 1000, t % 1000, msg());
    }
}

/// (identificador, nombre, valores en reposo, se envia de forma continua)
pub const SENSORS: &[(u32, &str, &str, bool)] = &[
    (0, "acceleration", "0:9.81:0", true),
    (1, "gyroscope", "0:0:0", true),
    (2, "magnetic", "0:5.9:-48.4", true),
    (4, "temperature", "25", false),
    (5, "proximity", "1", false),
    (6, "light", "1000", true),
    (7, "pressure", "1013.25", true),
    (8, "humidity", "40", false),
];

pub fn mask() -> u32 {
    SENSORS.iter().fold(0, |m, s| m | 1 << s.0)
}

pub fn frame(cmd: u32, response: bool, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(8 + payload.len());
    f.extend_from_slice(&((cmd & 0x7fff_ffff) | (response as u32) << 31).to_le_bytes());
    f.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    f.extend_from_slice(payload);
    f
}

/// Lee una trama completa. Ok(None) = fin de la conexion.
pub fn read_frame(r: &mut impl Read) -> std::io::Result<Option<(u32, bool, Vec<u8>)>> {
    let mut h = [0u8; 8];
    match r.read_exact(&mut h) {
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        other => other?,
    }
    let w = u32::from_le_bytes([h[0], h[1], h[2], h[3]]);
    let n = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as usize;
    if n > 1 << 20 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, tx!("sensors.trama_de_sensores_demasiado_grande")));
    }
    let mut p = vec![0u8; n];
    r.read_exact(&mut p)?;
    Ok(Some((w & 0x7fff_ffff, w >> 31 != 0, p)))
}

/// Respuesta a una peticion del servicio de Android por el canal de control (solo se atiende "list-sensors").
pub fn control_reply(payload: &[u8]) -> Option<Vec<u8>> {
    payload.starts_with(b"list-sensors").then(|| frame(CMD_UPDATE_HAL, true, format!("{}\n", mask()).as_bytes()))
}

/// Informes de datos: todos los sensores (tras activarse) o solo los continuos.
/// `rot`: orientacion del aparato (0 vertical, 1..3 giros de 90 grados, ver gestos::acelerometro).
pub fn reports(all: bool, rot: u32) -> Vec<u8> {
    let mut out = Vec::new();
    for (_, name, values, continuous) in SENSORS {
        if all || *continuous {
            let values = if *name == "acceleration" && rot % 4 != 0 {
                let (x, y, z) = crate::gestos::acelerometro(rot);
                format!("{}:{}:{}", x, y, z)
            } else {
                values.to_string()
            };
            out.extend_from_slice(&frame(CMD_UPDATE_HAL, true, format!("{}:{}\n", name, values).as_bytes()));
        }
    }
    out
}

/// Atiende los dos canales hasta que se cierran o `alive` devuelve false.
///
/// Cada vez que el invitado pregunta por la lista de sensores (`list-sensors`) empieza una sesion nueva y espera recibir
/// de golpe el valor de TODOS los sensores, incluidos los que no se envian de forma continua (temperatura, proximidad,
/// humedad); sin ellos el HAL no termina de inicializarse y system_server queda bloqueado hasta que lo mata el Watchdog.
/// Se cuenta con una generacion: la rafaga pendiente no se puede perder aunque la pregunta llegue mientras el hilo de datos
/// esta escribiendo (con un indicador de "pendiente" que el hilo de datos borrase tras escribir, se perdia).
pub fn serve<C, D>(mut control: C, control_w: C, mut data: D, alive: impl Fn() -> bool + Send + 'static, rot: impl Fn() -> u32 + Send + 'static) -> std::io::Result<()>
where
    C: Read + Write + Send + 'static,
    D: Write + Send + 'static,
{
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    // 0 = el invitado aun no ha preguntado; despues, numero de preguntas recibidas
    let gen = Arc::new(AtomicU32::new(0));
    let g2 = gen.clone();
    let reporter = std::thread::spawn(move || {
        let mut vista = 0u32;
        while alive() {
            let g = g2.load(Ordering::SeqCst);
            let mut espera = 20; // pasos de 50 ms: la rafaga nueva sale sin esperar al informe periodico
            if g != 0 {
                let todo = g != vista;
                vista = g;
                let informe = reports(todo, rot());
                traza(|| format!("datos> escribe todo={} gen={} bytes={}", todo, g, informe.len()));
                if let Err(e) = data.write_all(&informe).and_then(|_| data.flush()) {
                    traza(|| format!("datos> ERROR {} (el hilo de datos termina)", e)); // texto-interno: traza de depuracion de los sensores
                    break;
                }
                traza(|| format!("datos> escrito gen={}", g));
                if todo {
                    espera = 1;
                }
            }
            for _ in 0..espera {
                if g2.load(Ordering::SeqCst) != vista {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    });
    let mut w = control_w;
    traza(|| "sesion de sensores: servicio iniciado".to_string());
    while let Some((cmd, resp, payload)) = read_frame(&mut control)? {
        traza(|| format!("control< cmd={} resp={} {:?}", cmd, resp, String::from_utf8_lossy(&payload)));
        if let Some(reply) = control_reply(&payload) {
            w.write_all(&reply)?;
            w.flush()?;
            let g = gen.fetch_add(1, Ordering::SeqCst) + 1;
            traza(|| format!("control> respuesta a list-sensors; gen={}", g)); // texto-interno: traza de depuracion de los sensores
        }
    }
    traza(|| "sesion de sensores: el canal de control se cerro".to_string());
    drop(reporter);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    #[test]
    fn tramas() {
        let f = frame(2, true, b"abc");
        assert_eq!(f, [2, 0, 0, 0x80, 3, 0, 0, 0, b'a', b'b', b'c']);
        let mut r = &f[..];
        assert_eq!(read_frame(&mut r).unwrap(), Some((2, true, b"abc".to_vec())));
        assert_eq!(read_frame(&mut r).unwrap(), None);
        assert_eq!(mask(), 0b1_1111_0111);
        assert!(control_reply(b"otra-cosa").is_none());
        assert_eq!(control_reply(b"list-sensors").unwrap(), frame(2, true, b"503\n"));
    }

    #[test]
    fn acelerometro_girado() {
        let trama = |rot| String::from_utf8(read_frame(&mut &reports(false, rot)[..]).unwrap().unwrap().2).unwrap();
        assert_eq!(trama(0), "acceleration:0:9.81:0\n");
        assert_eq!(trama(1), "acceleration:9.81:0:0\n");
        assert_eq!(trama(3), "acceleration:-9.81:0:0\n");
    }

    /// Dialogo completo como lo hace el servicio de sensores de Android: pide la lista y recibe datos.
    #[test]
    fn dialogo_con_el_invitado() {
        let (guest_ctl, host_ctl) = UnixStream::pair().unwrap();
        let (mut guest_data, host_data) = UnixStream::pair().unwrap();
        let host_ctl_w = host_ctl.try_clone().unwrap();
        let t = std::thread::spawn(move || serve(host_ctl, host_ctl_w, host_data, || true, || 0));
        let mut g = guest_ctl.try_clone().unwrap();
        g.write_all(&frame(1, false, b"list-sensors")).unwrap();
        let mut gr = guest_ctl.try_clone().unwrap();
        let (cmd, resp, p) = read_frame(&mut gr).unwrap().unwrap();
        assert_eq!((cmd, resp, p.as_slice()), (2, true, b"503\n".as_slice()));
        // primero llegan todos los sensores; el primero es el acelerometro, en reposo y en vertical
        guest_data.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        let (_, _, p) = read_frame(&mut guest_data).unwrap().unwrap();
        assert_eq!(p, b"acceleration:0:9.81:0\n");
        let mut names = vec![];
        for _ in 1..SENSORS.len() {
            let (_, _, p) = read_frame(&mut guest_data).unwrap().unwrap();
            names.push(String::from_utf8(p).unwrap().split(':').next().unwrap().to_string());
        }
        assert!(names.contains(&"humidity".to_string()) && names.contains(&"pressure".to_string()));
        drop((g, gr, guest_ctl));
        assert!(t.join().unwrap().is_ok());
    }

    /// Escritor lento: simula que el hilo de datos esta escribiendo cuando llega una pregunta nueva del invitado.
    struct Lenta {
        buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
        escribiendo: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }
    impl Write for Lenta {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.escribiendo.store(true, std::sync::atomic::Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(300));
            self.buf.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Regresion: una pregunta que llega mientras se escribe la rafaga anterior no puede perder su propia rafaga
    /// (el HAL se quedaba esperando los sensores no continuos y Android no arrancaba).
    #[test]
    fn pregunta_durante_una_escritura_recibe_su_rafaga() {
        use std::sync::atomic::Ordering;
        use std::sync::{Arc, Mutex};
        let (guest_ctl, host_ctl) = UnixStream::pair().unwrap();
        let host_ctl_w = host_ctl.try_clone().unwrap();
        let buf = Arc::new(Mutex::new(Vec::new()));
        let esc = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let lenta = Lenta { buf: buf.clone(), escribiendo: esc.clone() };
        let t = std::thread::spawn(move || serve(host_ctl, host_ctl_w, lenta, || true, || 0));
        let mut g = guest_ctl.try_clone().unwrap();
        let mut gr = guest_ctl.try_clone().unwrap();
        g.write_all(&frame(1, false, b"list-sensors")).unwrap();
        read_frame(&mut gr).unwrap().unwrap();
        let t0 = std::time::Instant::now();
        while !esc.load(Ordering::SeqCst) && t0.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(esc.load(Ordering::SeqCst), "no empezo a escribir la primera rafaga");
        // segunda pregunta mientras la primera rafaga sigue escribiendose
        g.write_all(&frame(1, false, b"list-sensors")).unwrap();
        read_frame(&mut gr).unwrap().unwrap();
        let cuenta = |b: &Vec<u8>| b.windows(9).filter(|w| *w == b"humidity:").count();
        let t0 = std::time::Instant::now();
        while cuenta(&buf.lock().unwrap()) < 2 && t0.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(cuenta(&buf.lock().unwrap()), 2, "la segunda pregunta no recibio su rafaga de todos los sensores");
        drop((g, gr, guest_ctl));
        assert!(t.join().unwrap().is_ok());
    }
}
