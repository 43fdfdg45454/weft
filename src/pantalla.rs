//! Geometria de la presentacion de la pantalla: rotacion de la VENTANA y resolucion del panel, como funciones
//! puras (sin SDL, D-Bus ni maquina) mas los pocos archivos del directorio de estado por los que las ordenes
//! `rotate` y `resolution` hablan con la ventana y con el servicio de sensores.
//!
//! Modelo. El panel virtual (el scanout de virtio-gpu) tiene siempre la forma que fija la resolucion (p. ej. 720x1348,
//! vertical). Cuando Android gira (Surface.ROTATION_90 = aparato girado a la izquierda, apaisado), compone su
//! contenido ya girado dentro de ese mismo panel, como en un movil real: la imagen del panel queda de lado (el
//! contenido rotado 90 grados a la derecha por cada paso de rotacion). Un emulador muestra lo que veria quien gira
//! el aparato: la ventana dibuja la imagen del panel girada 90 grados a la izquierda por cada paso (la vista
//! apaisada mide alto x ancho) y lleva el raton, el toque y el pellizco de la vista al panel con la transformacion
//! inversa. Android recibe siempre coordenadas del panel (como un panel tactil real, que no sabe de rotaciones) y las
//! gira el mismo segun la orientacion de la pantalla (touch.orientationAware).

use std::path::Path;
use crate::textos::{tx, txf};

/// Rotaciones de Android: 0, 1 (90), 2 (180), 3 (270), como Surface.ROTATION_*.
pub fn rot_de_grados(g: u32) -> Option<u32> {
    match g {
        0 => Some(0),
        90 => Some(1),
        180 => Some(2),
        270 => Some(3),
        _ => None,
    }
}

pub fn grados_de_rot(r: u32) -> u32 {
    (r % 4) * 90
}

/// Tamano de la vista (lo que muestra la ventana) para un panel de `panel` = (ancho, alto): con 90 y 270 grados
/// se intercambian.
pub fn vista(rot: u32, panel: (u32, u32)) -> (u32, u32) {
    if rot % 2 == 1 {
        (panel.1, panel.0)
    } else {
        panel
    }
}

/// Angulo (grados, sentido horario, como SDL_RenderTextureRotated) con que se dibuja la imagen del panel: 90
/// grados a la izquierda por cada paso de rotacion.
pub fn angulo_sdl(rot: u32) -> f64 {
    match rot % 4 {
        0 => 0.0,
        1 => 270.0,
        2 => 180.0,
        _ => 90.0,
    }
}

/// Punto de la vista -> punto del panel. Las coordenadas son indices de pixel (el primero es 0, el ultimo ancho-1).
pub fn a_panel(rot: u32, panel: (u32, u32), p: (f64, f64)) -> (f64, f64) {
    let (w, h) = (panel.0 as f64 - 1.0, panel.1 as f64 - 1.0);
    match rot % 4 {
        0 => p,
        1 => (w - p.1, p.0),
        2 => (w - p.0, h - p.1),
        _ => (p.1, h - p.0),
    }
}

/// Punto del panel -> punto de la vista (inversa de `a_panel`; solo la usan las pruebas).
#[cfg(test)]
pub fn de_panel(rot: u32, panel: (u32, u32), p: (f64, f64)) -> (f64, f64) {
    let (w, h) = (panel.0 as f64 - 1.0, panel.1 as f64 - 1.0);
    match rot % 4 {
        0 => p,
        1 => (p.1, w - p.0),
        2 => (w - p.0, h - p.1),
        _ => (h - p.1, p.0),
    }
}

/// Resolucion pedida: ANCHOxALTO con densidad opcional (@DPI).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Resolucion {
    pub w: u32,
    pub h: u32,
    pub dpi: Option<u32>,
}

pub const LADO_MIN: u32 = 64;
pub const LADO_MAX: u32 = 8192;

pub fn parse_resolucion(s: &str) -> Result<Resolucion, String> {
    let s = s.trim();
    let (dim, dpi) = match s.split_once('@') {
        Some((d, p)) => (d, Some(p)),
        None => (s, None),
    };
    let (w, h) = dim.split_once(['x', 'X']).ok_or(tx!("pantalla.se_espera_anchoxalto_dpi_p_ej_1348x720"))?;
    let lado = |t: &str, n: &str| -> Result<u32, String> {
        let v: u32 = t.parse().map_err(|_| txf!("pantalla.no_valido", n, format!("{:?}", t)))?;
        if !(LADO_MIN..=LADO_MAX).contains(&v) {
            return Err(txf!("pantalla.fuera_de_rango", n, LADO_MIN, LADO_MAX, v));
        }
        Ok(v)
    };
    let dpi = match dpi {
        None => None,
        Some(p) => {
            let v: u32 = p.parse().map_err(|_| txf!("pantalla.densidad_no_valida", format!("{:?}", p)))?;
            if !(72..=960).contains(&v) {
                return Err(txf!("pantalla.densidad_fuera_de_rango_72_960", v));
            }
            Some(v)
        }
    };
    Ok(Resolucion { w: lado(w, "ancho")?, h: lado(h, "alto")?, dpi })
}

impl Resolucion {
    pub fn texto(&self) -> String {
        match self.dpi {
            Some(d) => format!("{}x{}@{}", self.w, self.h, d),
            None => format!("{}x{}", self.w, self.h),
        }
    }

    /// Tamano fisico (mm) del panel para el dispositivo (solo informativo: sin EDID no se usa).
    pub fn mm(&self, dpi_por_defecto: u32) -> (u16, u16) {
        let d = self.dpi.unwrap_or(dpi_por_defecto).max(1) as f64;
        let mm = |px: u32| (px as f64 * 25.4 / d).round().clamp(1.0, 65535.0) as u16;
        (mm(self.w), mm(self.h))
    }
}

// ---------------------------------------------------------------------------------------------------------------
// archivos del directorio de estado

/// Orientacion pedida (archivo `orientation`): fija en una de las cuatro rotaciones o `auto`.
///
/// REGLA. Con una orientacion FIJA (`rotate 90`, F7, el panel) la ventana queda girada esos grados aunque Android no gire
/// (giro automatico apagado, aplicacion que no gira): manda lo pedido, sin retroceso. El servicio de sensores reporta ese
/// giro al acelerometro, y Android lo sigue si puede. Con `auto` (el valor por defecto) manda Android: la ventana gira
/// con lo que Android decide por su cuenta (una app que fuerza apaisado, el ajuste de rotacion del usuario) y el
/// acelerometro vuelve a "aparato derecho" (0), como un movil sobre la mesa. Si ademas se desactiva "seguir la rotacion de
/// Android" (configuracion `giro_android`), `auto` se queda en vertical.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Orientacion {
    #[default]
    Auto,
    Fija(u32),
}

impl Orientacion {
    /// Texto del archivo: `auto` o 0..3.
    pub fn texto(self) -> String {
        match self {
            Orientacion::Auto => "auto".to_string(),
            Orientacion::Fija(r) => (r % 4).to_string(),
        }
    }

    pub fn parse(t: &str) -> Option<Orientacion> {
        let t = t.trim();
        if t.eq_ignore_ascii_case("auto") {
            return Some(Orientacion::Auto);
        }
        t.parse::<u32>().ok().map(|n| Orientacion::Fija(n % 4))
    }

    /// Paso de F7 / `rotate` sin argumento: 0 -> 90 -> 180 -> 270 -> auto -> 0.
    pub fn siguiente(self) -> Orientacion {
        match self {
            Orientacion::Fija(3) => Orientacion::Auto,
            Orientacion::Fija(r) => Orientacion::Fija(r + 1),
            Orientacion::Auto => Orientacion::Fija(0),
        }
    }

    /// Rotacion (0..3) que debe reportar el acelerometro del servicio de sensores.
    pub fn para_sensores(self) -> u32 {
        match self {
            Orientacion::Auto => 0,
            Orientacion::Fija(r) => r % 4,
        }
    }

    /// Descripcion para mensajes: `auto` o `90 grados (fija)`.
    pub fn descripcion(self) -> String {
        match self {
            Orientacion::Auto => tx!("pantalla.auto_la_decide_android").to_string(),
            Orientacion::Fija(r) => txf!("pantalla.grados_fija", grados_de_rot(r)),
        }
    }
}

/// Orientacion pedida del archivo `orientation` (`auto` si no existe o no se entiende).
pub fn leer_orientacion(dir: &Path) -> Orientacion {
    std::fs::read_to_string(dir.join("orientation")).ok().and_then(|t| Orientacion::parse(&t)).unwrap_or(Orientacion::Auto)
}

/// Fija la orientacion pedida (la sigue el acelerometro del servicio de sensores y la ventana). Escritura atomica.
pub fn escribir_orientacion(dir: &Path, o: Orientacion) -> std::io::Result<()> {
    let tmp = dir.join("orientation.tmp");
    std::fs::write(&tmp, format!("{}\n", o.texto()))?;
    std::fs::rename(&tmp, dir.join("orientation"))
}

/// Rotacion que Android informo por su cuenta (archivo `android-rotation`, lo escribe `rotacion::seguir` al leer la
/// pantalla de Android por adb). None si no existe.
pub fn leer_rotacion_android(dir: &Path) -> Option<u32> {
    Some(std::fs::read_to_string(dir.join("android-rotation")).ok()?.trim().parse::<u32>().ok()? % 4)
}

pub fn escribir_rotacion_android(dir: &Path, rot: u32) -> std::io::Result<()> {
    let tmp = dir.join("android-rotation.tmp");
    std::fs::write(&tmp, format!("{}\n", rot % 4))?;
    std::fs::rename(&tmp, dir.join("android-rotation"))
}

/// Rotacion con que se dibuja la ventana: la fija pedida o, con `auto`, la de Android (vertical si Android no ha
/// informado o si `seguir_android` esta apagado).
pub fn rotacion_efectiva(pedida: Orientacion, android: Option<u32>, seguir_android: bool) -> u32 {
    match pedida {
        Orientacion::Fija(r) => r % 4,
        Orientacion::Auto if seguir_android => android.unwrap_or(0) % 4,
        Orientacion::Auto => 0,
    }
}

/// Rotacion con que debe dibujar la ventana segun los archivos del estado.
pub fn rotacion_ventana(dir: &Path, seguir_android: bool) -> u32 {
    rotacion_efectiva(leer_orientacion(dir), leer_rotacion_android(dir), seguir_android)
}

/// Pide a la ventana que cambie la resolucion del panel: archivo `requested-resolution`, que la ventana consume.
pub fn pedir_resolucion(dir: &Path, r: &Resolucion) -> std::io::Result<()> {
    let tmp = dir.join("requested-resolution.tmp");
    std::fs::write(&tmp, format!("{}\n", r.texto()))?;
    std::fs::rename(&tmp, dir.join("requested-resolution"))
}

/// Toma (y borra) la peticion de resolucion pendiente, si hay una.
pub fn tomar_resolucion(dir: &Path) -> Option<Resolucion> {
    let f = dir.join("requested-resolution");
    let t = std::fs::read_to_string(&f).ok()?;
    let _ = std::fs::remove_file(&f);
    parse_resolucion(&t).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: (u32, u32) = (720, 1348);

    #[test]
    fn grados() {
        assert_eq!(rot_de_grados(90), Some(1));
        assert_eq!(rot_de_grados(270), Some(3));
        assert_eq!(rot_de_grados(45), None);
        assert_eq!(grados_de_rot(5), 90);
        assert_eq!(vista(0, P), P);
        assert_eq!(vista(1, P), (1348, 720));
        assert_eq!(vista(2, P), P);
        assert_eq!(vista(3, P), (1348, 720));
        assert_eq!([angulo_sdl(0), angulo_sdl(1), angulo_sdl(2), angulo_sdl(3)], [0.0, 270.0, 180.0, 90.0]);
    }

    /// Las esquinas de la vista van a las esquinas correctas del panel. Con 90 grados (ROTATION_90) la imagen del
    /// panel se ve girada a la izquierda: la esquina superior izquierda de la vista es la superior DERECHA del panel.
    #[test]
    fn esquinas() {
        let (w, h) = (719.0, 1347.0);
        // rot 0: identidad
        assert_eq!(a_panel(0, P, (0.0, 0.0)), (0.0, 0.0));
        // rot 1: vista 1348x720
        let v = vista(1, P);
        let (vw, vh) = (v.0 as f64 - 1.0, v.1 as f64 - 1.0);
        assert_eq!(a_panel(1, P, (0.0, 0.0)), (w, 0.0)); // sup-izq vista -> sup-der panel
        assert_eq!(a_panel(1, P, (vw, 0.0)), (w, h)); // sup-der vista -> inf-der panel
        assert_eq!(a_panel(1, P, (vw, vh)), (0.0, h)); // inf-der vista -> inf-izq panel
        assert_eq!(a_panel(1, P, (0.0, vh)), (0.0, 0.0)); // inf-izq vista -> sup-izq panel
        // rot 2: 180
        assert_eq!(a_panel(2, P, (0.0, 0.0)), (w, h));
        assert_eq!(a_panel(2, P, (w, h)), (0.0, 0.0));
        // rot 3: la vista gira a la derecha
        assert_eq!(a_panel(3, P, (0.0, 0.0)), (0.0, h)); // sup-izq vista -> inf-izq panel
        assert_eq!(a_panel(3, P, (vw, 0.0)), (0.0, 0.0));
        assert_eq!(a_panel(3, P, (vw, vh)), (w, 0.0));
        assert_eq!(a_panel(3, P, (0.0, vh)), (w, h));
    }

    #[test]
    fn ida_y_vuelta() {
        for rot in 0..4 {
            let v = vista(rot, P);
            for p in [(0.0, 0.0), (5.0, 7.0), (v.0 as f64 - 1.0, v.1 as f64 - 1.0), (300.0, 100.0)] {
                let q = a_panel(rot, P, p);
                assert!(q.0 >= 0.0 && q.0 <= 719.0 && q.1 >= 0.0 && q.1 <= 1347.0, "rot {} {:?} -> {:?}", rot, p, q);
                assert_eq!(de_panel(rot, P, q), p, "rot {}", rot);
            }
        }
    }

    /// El centro de la vista es el centro del panel y un desplazamiento horizontal de la vista a 90 grados es
    /// vertical en el panel (lo que hace la rueda con Mayus en una pantalla apaisada).
    #[test]
    fn ejes() {
        let c = a_panel(1, P, (673.0, 359.0));
        assert_eq!(c, (360.0, 673.0));
        let (a, b) = (a_panel(1, P, (100.0, 50.0)), a_panel(1, P, (200.0, 50.0)));
        assert_eq!(a.0, b.0);
        assert_eq!(b.1 - a.1, 100.0);
    }

    #[test]
    fn resolucion() {
        assert_eq!(parse_resolucion("1348x720"), Ok(Resolucion { w: 1348, h: 720, dpi: None }));
        assert_eq!(parse_resolucion(" 1348X720@320 "), Ok(Resolucion { w: 1348, h: 720, dpi: Some(320) }));
        assert_eq!(parse_resolucion("720x1348@280").unwrap().texto(), "720x1348@280");
        assert!(parse_resolucion("1348").is_err());
        assert!(parse_resolucion("axb").is_err());
        assert!(parse_resolucion("10x720").is_err());
        assert!(parse_resolucion("720x99999").is_err());
        assert!(parse_resolucion("720x1348@5").is_err());
        assert!(parse_resolucion("720x1348@").is_err());
        assert_eq!(parse_resolucion("720x1348@254").unwrap().mm(280), (72, 135));
        assert_eq!(parse_resolucion("1000x1000").unwrap().mm(254), (100, 100));
    }

    #[test]
    fn orientacion_pedida() {
        assert_eq!(Orientacion::parse("auto"), Some(Orientacion::Auto));
        assert_eq!(Orientacion::parse(" AUTO\n"), Some(Orientacion::Auto));
        assert_eq!(Orientacion::parse("1"), Some(Orientacion::Fija(1)));
        assert_eq!(Orientacion::parse("5"), Some(Orientacion::Fija(1)));
        assert_eq!(Orientacion::parse("x"), None);
        for o in [Orientacion::Auto, Orientacion::Fija(0), Orientacion::Fija(3)] {
            assert_eq!(Orientacion::parse(&o.texto()), Some(o));
        }
        // el ciclo de F7: 0 -> 90 -> 180 -> 270 -> auto -> 0
        let mut o = Orientacion::Fija(0);
        let mut v = vec![o];
        for _ in 0..5 {
            o = o.siguiente();
            v.push(o);
        }
        assert_eq!(v, vec![Orientacion::Fija(0), Orientacion::Fija(1), Orientacion::Fija(2), Orientacion::Fija(3), Orientacion::Auto, Orientacion::Fija(0)]);
        // el acelerometro: fija = esa rotacion; auto = aparato derecho
        assert_eq!(Orientacion::Fija(2).para_sensores(), 2);
        assert_eq!(Orientacion::Auto.para_sensores(), 0);
    }

    /// La regla: fija manda sobre Android (sin retroceso); auto sigue a Android si esta activado.
    #[test]
    fn rotacion_que_manda() {
        // fija: lo pedido, diga lo que diga Android (que no gira) y este o no activado seguirlo
        assert_eq!(rotacion_efectiva(Orientacion::Fija(1), Some(0), true), 1);
        assert_eq!(rotacion_efectiva(Orientacion::Fija(1), None, false), 1);
        assert_eq!(rotacion_efectiva(Orientacion::Fija(0), Some(3), true), 0);
        // auto: Android
        assert_eq!(rotacion_efectiva(Orientacion::Auto, Some(3), true), 3);
        assert_eq!(rotacion_efectiva(Orientacion::Auto, None, true), 0);
        // auto sin seguir a Android: vertical
        assert_eq!(rotacion_efectiva(Orientacion::Auto, Some(3), false), 0);
    }

    #[test]
    fn archivos_de_rotacion() {
        let d = std::path::PathBuf::from(format!("{}/pantalla-rot-{}", std::env::var("TMPDIR").unwrap_or_else(|_| ".".into()), std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        // sin archivos: auto y vertical
        assert_eq!(leer_orientacion(&d), Orientacion::Auto);
        assert_eq!(leer_rotacion_android(&d), None);
        assert_eq!(rotacion_ventana(&d, true), 0);
        // fija: la ventana queda a 90 aunque Android informe 0 (no hay retroceso por tiempo)
        escribir_rotacion_android(&d, 0).unwrap();
        escribir_orientacion(&d, Orientacion::Fija(1)).unwrap();
        assert_eq!(rotacion_ventana(&d, true), 1);
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(rotacion_ventana(&d, true), 1);
        // auto: Android manda
        escribir_orientacion(&d, Orientacion::Auto).unwrap();
        escribir_rotacion_android(&d, 3).unwrap();
        assert_eq!(rotacion_ventana(&d, true), 3);
        assert_eq!(rotacion_ventana(&d, false), 0);
        assert_eq!(std::fs::read_to_string(d.join("orientation")).unwrap(), "auto\n");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn archivos_de_estado() {
        let d = std::path::PathBuf::from(format!("{}/pantalla-prueba-{}", std::env::var("TMPDIR").unwrap_or_else(|_| ".".into()), std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(leer_orientacion(&d), Orientacion::Auto);
        escribir_orientacion(&d, Orientacion::Fija(3)).unwrap();
        assert_eq!(leer_orientacion(&d), Orientacion::Fija(3));
        escribir_orientacion(&d, Orientacion::Fija(5)).unwrap();
        assert_eq!(leer_orientacion(&d), Orientacion::Fija(1));
        assert!(tomar_resolucion(&d).is_none());
        let r = parse_resolucion("1348x720@300").unwrap();
        pedir_resolucion(&d, &r).unwrap();
        assert_eq!(tomar_resolucion(&d), Some(r));
        assert!(tomar_resolucion(&d).is_none());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
