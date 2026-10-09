//! Tipos de dibujo compartidos por el panel lateral y la pantalla de configuracion, y las formas que no son texto
//! (esquinas redondeadas e iconos), rasterizadas por software con suavizado (supermuestreo 4x4) en mascaras de alfa de
//! 8 bits. Ni SDL ni la maquina intervienen aqui: `window.rs` sube las mascaras a una textura y las dibuja con el color
//! pedido (mascara blanca + modulacion de color + mezcla por alfa, que es la mezcla correcta con alfa no premultiplicado).
//!
//! Las posiciones y tamanos de la interfaz estan en "dp" (puntos de la ventana); las mascaras se rasterizan al tamano
//! FISICO (dp x factor de escala del escritorio), nunca se escalan despues.

use crate::fuente::Estilo;

/// Rectangulo en coordenadas de la ventana (puntos). Mismo formato que SDL_FRect.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct R {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl R {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> R {
        R { x, y, w, h }
    }
    pub fn contiene(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
    /// Rectangulo reducido `d` por cada lado.
    pub fn reducir(&self, d: f32) -> R {
        R::new(self.x + d, self.y + d, (self.w - 2.0 * d).max(0.0), (self.h - 2.0 * d).max(0.0))
    }
    /// Interseccion (con ancho y alto 0 si no se tocan).
    pub fn cruzar(&self, o: &R) -> R {
        let (x0, y0) = (self.x.max(o.x), self.y.max(o.y));
        let (x1, y1) = ((self.x + self.w).min(o.x + o.w), (self.y + self.h).min(o.y + o.h));
        R::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
    }
    #[cfg(test)]
    pub fn se_cruza(&self, o: &R) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color(pub u8, pub u8, pub u8, pub u8);

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color(r, g, b, 255)
    }
    pub const fn alfa(self, a: u8) -> Color {
        Color(self.0, self.1, self.2, a)
    }
    /// Luminancia relativa (WCAG), para comprobar el contraste.
    #[cfg(test)]
    pub fn luminancia(self) -> f64 {
        let l = |v: u8| {
            let c = v as f64 / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * l(self.0) + 0.7152 * l(self.1) + 0.0722 * l(self.2)
    }
}

/// Relacion de contraste WCAG entre dos colores opacos (4,5 = AA para texto normal).
#[cfg(test)]
pub fn contraste(a: Color, b: Color) -> f64 {
    let (la, lb) = (a.luminancia(), b.luminancia());
    let (alto, bajo) = if la > lb { (la, lb) } else { (lb, la) };
    (alto + 0.05) / (bajo + 0.05)
}

/// `arriba` (con su alfa) mezclado sobre `abajo` (opaco), como lo mezcla la ventana.
#[cfg(test)]
pub fn mezclar(arriba: Color, abajo: Color) -> Color {
    let a = arriba.3 as f32 / 255.0;
    let m = |x: u8, y: u8| (x as f32 * a + y as f32 * (1.0 - a)).round() as u8;
    Color::rgb(m(arriba.0, abajo.0), m(arriba.1, abajo.1), m(arriba.2, abajo.2))
}

/// Un texto o un icono de un dibujo: el color con que se pinta y el del fondo que queda debajo.
#[cfg(test)]
#[derive(Clone, Debug)]
pub struct ParDeColor {
    /// el texto (o el nombre del icono)
    pub que: String,
    pub frente: Color,
    pub fondo: Color,
    pub icono: bool,
}

/// Cada texto e icono visible de un dibujo (dentro del recorte vigente) con su color y el del fondo sobre el que cae: los
/// rectangulos pintados antes que contienen su punto de muestra, mezclados por alfa sobre `base`. El punto de muestra de un
/// texto es su borde izquierdo a media altura de la linea (`alto`: alto de linea de cada estilo); el de un icono, su centro.
#[cfg(test)]
pub fn pares_de_color(p: &[Pint], base: Color, alto: &dyn Fn(Estilo) -> f32) -> Vec<ParDeColor> {
    let mut out = Vec::new();
    let mut recorte: Option<R> = None;
    let fondo_en = |hasta: usize, x: f32, y: f32| {
        let mut rec: Option<R> = None;
        let mut c = base;
        for q in &p[..hasta] {
            match q {
                Pint::Recorte(r) => rec = *r,
                Pint::Rect { r, c: cr, .. } if r.contiene(x, y) && rec.map_or(true, |k| k.contiene(x, y)) => c = mezclar(*cr, c),
                _ => {}
            }
        }
        c
    };
    for (i, q) in p.iter().enumerate() {
        let (que, x, y, frente, icono) = match q {
            Pint::Recorte(r) => {
                recorte = *r;
                continue;
            }
            Pint::Rect { .. } => continue,
            Pint::Texto { x, y, t, e, c } => (t.clone(), *x + 1.0, *y + alto(*e) / 2.0, *c, false),
            Pint::Icono { k, r, c } => (format!("{:?}", k), r.x + r.w / 2.0, r.y + r.h / 2.0, *c, true),
        };
        if que.trim().is_empty() || recorte.is_some_and(|k| !k.contiene(x, y)) {
            continue;
        }
        let fondo = fondo_en(i, x, y);
        out.push(ParDeColor { que, frente: mezclar(frente, fondo), fondo, icono });
    }
    out
}

/// Los pares de un dibujo que no llegan al contraste AA de WCAG: 4,5 el texto y 3 los iconos (componentes graficos). Se
/// exime solo lo deshabilitado (texto apagado sobre un control apagado), como hace WCAG.
#[cfg(test)]
pub fn fallos_de_contraste(pares: &[ParDeColor]) -> Vec<String> {
    pares
        .iter()
        .filter(|p| !(p.frente == tema::p().texto_apagado && p.fondo == tema::p().control_apagado))
        .filter_map(|p| {
            let (c, minimo) = (contraste(p.frente, p.fondo), if p.icono { 3.0 } else { 4.5 });
            (c < minimo).then(|| format!("{:?} {:?} sobre {:?}: {:.2} < {}", p.que, p.frente, p.fondo, c, minimo))
        })
        .collect()
}


/// Temas de la interfaz: dos paletas (oscura, la de siempre, y clara) con contraste AA comprobado en las pruebas, y medidas
/// comunes. Rejilla de 8 dp. El dibujo pide los colores del tema vigente con `tema::p()` (`tema::p().texto`...).
///
/// El tema vigente es uno por proceso (`fijar`, lo decide la ventana: `ventana.tema` de `config` y, en automatico, la
/// preferencia del escritorio por el portal; ver `Variante::elegir`). Las pruebas lo fuerzan por hilo con `con`, como
/// `textos::con_modo`, para no pisarse entre si.
pub mod tema {
    use super::Color;
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU8, Ordering};

    /// Los colores de un tema.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Paleta {
        /// lo que rodea a la pantalla de Android (fuera de la barra y de los dialogos)
        pub lienzo: Color,
        pub fondo: Color,
        /// dialogos y tarjetas
        pub superficie: Color,
        /// controles (botones, chips, campos)
        pub control: Color,
        pub control_hover: Color,
        pub control_apagado: Color,
        pub borde: Color,
        pub texto: Color,
        /// texto secundario (ayudas, etiquetas, la tecla de un boton): AA tambien sobre un control con el raton encima
        pub texto2: Color,
        /// texto de un control deshabilitado (no exige contraste AA)
        pub texto_apagado: Color,
        pub acento: Color,
        pub acento_hover: Color,
        /// acento como texto o como trazo (tecla por capturar, anillo de foco, pestana abierta): AA tambien sobre un control
        /// con hover
        pub acento_claro: Color,
        pub sobre_acento: Color,
        pub aviso: Color,
        pub error: Color,
        pub exito: Color,
        pub peligro: Color,
        pub peligro_hover: Color,
    }

    /// El tema oscuro: los colores de siempre.
    pub const OSCURO: Paleta = Paleta {
        lienzo: Color::rgb(0, 0, 0),
        fondo: Color::rgb(30, 32, 37),
        superficie: Color::rgb(38, 41, 48),
        control: Color::rgb(52, 57, 66),
        control_hover: Color::rgb(68, 75, 89),
        control_apagado: Color::rgb(42, 45, 52),
        borde: Color::rgb(70, 76, 88),
        texto: Color::rgb(232, 235, 240),
        texto2: Color::rgb(184, 190, 203),
        texto_apagado: Color::rgb(118, 124, 136),
        acento: Color::rgb(38, 98, 176),
        acento_hover: Color::rgb(52, 116, 200),
        acento_claro: Color::rgb(142, 191, 255),
        sobre_acento: Color::rgb(255, 255, 255),
        aviso: Color::rgb(236, 190, 90),
        error: Color::rgb(255, 128, 128),
        exito: Color::rgb(124, 214, 154),
        peligro: Color::rgb(122, 48, 48),
        peligro_hover: Color::rgb(158, 62, 62),
    };

    /// El tema claro: fondo gris muy claro, dialogos blancos y texto casi negro; los tonos de aviso, error y exito, y el acento
    /// como texto, oscurecidos para que lleguen a AA sobre blanco. El hover de un boton de acento oscurece en lugar de aclarar.
    pub const CLARO: Paleta = Paleta {
        lienzo: Color::rgb(222, 225, 230),
        fondo: Color::rgb(240, 242, 245),
        superficie: Color::rgb(255, 255, 255),
        control: Color::rgb(232, 235, 240),
        control_hover: Color::rgb(214, 220, 229),
        control_apagado: Color::rgb(243, 244, 246),
        borde: Color::rgb(184, 190, 201),
        texto: Color::rgb(24, 27, 33),
        texto2: Color::rgb(72, 79, 92),
        texto_apagado: Color::rgb(118, 124, 136),
        acento: Color::rgb(31, 94, 178),
        acento_hover: Color::rgb(24, 76, 150),
        acento_claro: Color::rgb(22, 82, 165),
        sobre_acento: Color::rgb(255, 255, 255),
        aviso: Color::rgb(130, 82, 0),
        error: Color::rgb(176, 32, 32),
        exito: Color::rgb(18, 112, 56),
        peligro: Color::rgb(184, 40, 40),
        peligro_hover: Color::rgb(150, 30, 30),
    };

    /// radio de las esquinas de controles y tarjetas (dp)
    pub const RADIO: f32 = 6.0;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Variante {
        Oscuro,
        Claro,
    }

    impl Variante {
        pub fn paleta(self) -> &'static Paleta {
            match self {
                Variante::Oscuro => &OSCURO,
                Variante::Claro => &CLARO,
            }
        }

        /// El tema que toca: `preferencia` es `ventana.tema` (`auto`, `claro` u `oscuro`) y `portal` el `color-scheme` del
        /// escritorio (org.freedesktop.appearance: 1 prefiere oscuro, 2 prefiere claro, 0 sin preferencia), si se pudo leer.
        /// En automatico solo una preferencia clara expresa da el tema claro: sin preferencia, sin portal o con un valor
        /// desconocido queda el oscuro de siempre. Pura.
        pub fn elegir(preferencia: &str, portal: Option<u32>) -> Variante {
            match preferencia {
                "claro" => Variante::Claro,
                "oscuro" => Variante::Oscuro,
                _ if portal == Some(2) => Variante::Claro,
                _ => Variante::Oscuro,
            }
        }
    }

    static ACTUAL: AtomicU8 = AtomicU8::new(0);

    thread_local! {
        /// forzado por hilo (solo pruebas)
        static FORZADO: Cell<Option<Variante>> = const { Cell::new(None) };
    }

    /// Fija el tema del proceso. true si cambio.
    pub fn fijar(v: Variante) -> bool {
        ACTUAL.swap(v as u8, Ordering::Relaxed) != v as u8
    }

    /// El tema vigente.
    pub fn variante() -> Variante {
        FORZADO.with(|f| f.get()).unwrap_or(if ACTUAL.load(Ordering::Relaxed) == Variante::Claro as u8 { Variante::Claro } else { Variante::Oscuro })
    }

    /// La paleta del tema vigente.
    pub fn p() -> &'static Paleta {
        variante().paleta()
    }

    /// Ejecuta `f` con el tema `v` en este hilo (para las pruebas).
    #[cfg(test)]
    pub fn con<R>(v: Variante, f: impl FnOnce() -> R) -> R {
        let antes = FORZADO.with(|x| x.replace(Some(v)));
        let r = f();
        FORZADO.with(|x| x.set(antes));
        r
    }
}

/// Tamano del texto de la interfaz (`ventana.texto`, en %): amplia por igual la barra, la pantalla de configuracion y los
/// avisos de la ventana (texto, controles y margenes: la maquetacion en dp no cambia), no la pantalla de Android. Es un
/// valor del proceso, como el tema; la ventana lo fija al leer la configuracion.
pub mod tamano {
    use std::sync::atomic::{AtomicU32, Ordering};

    static PCT: AtomicU32 = AtomicU32::new(100);

    /// El porcentaje de un valor de `ventana.texto` (config::TAMANOS_TEXTO); otro valor, 100. Pura.
    pub fn de_config(v: &str) -> u32 {
        v.trim().trim_end_matches('%').trim().parse::<u32>().ok().filter(|p| crate::config::TAMANOS_TEXTO.iter().any(|t| t.parse() == Ok(*p))).unwrap_or(100)
    }

    /// Fija el porcentaje. true si cambio.
    pub fn fijar(pct: u32) -> bool {
        PCT.swap(pct.clamp(50, 400), Ordering::Relaxed) != pct.clamp(50, 400)
    }

    /// Factor de la interfaz: 1 = 100 %. Un dp de la interfaz mide `factor()` dp de la ventana.
    pub fn factor() -> f32 {
        PCT.load(Ordering::Relaxed) as f32 / 100.0
    }
}

/// Iconos vectoriales (no dependen de ninguna fuente ni de emojis).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icono {
    Engranaje,
    Cerrar,
    Check,
    Mas,
    Menos,
    Circulo,
    /// triangulo con una exclamacion (mensaje de aviso)
    Alerta,
    /// circulo con un aspa (mensaje de error)
    Fallo,
}

/// Un elemento del dibujo, en el orden en que se pinta.
#[derive(Clone, Debug, PartialEq)]
pub enum Pint {
    /// Rectangulo relleno con esquinas de `radio` dp (0 = rectas).
    Rect { r: R, c: Color, radio: f32 },
    /// Texto con la esquina superior izquierda de su linea en (x, y) (dp).
    Texto { x: f32, y: f32, t: String, e: Estilo, c: Color },
    /// Icono centrado en `r` (cuadrado).
    Icono { k: Icono, r: R, c: Color },
    /// Recorte del dibujo siguiente (None = sin recorte).
    Recorte(Option<R>),
}

// ---------------------------------------------------------------------------------------------------------------
// mascaras

/// Lo que se rasteriza en una mascara de alfa.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mascara {
    /// Cuarto de disco de radio `r` px (esquina superior izquierda de un rectangulo redondeado), r x r px.
    Esquina(u32),
    /// Icono en un cuadrado de `px` px.
    Icono(Icono, u32),
}

/// Distancia de (px, py) al segmento (ax, ay)-(bx, by).
fn dist_seg(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 == 0.0 { 0.0 } else { (((px - a.0) * dx + (py - a.1) * dy) / l2).clamp(0.0, 1.0) };
    let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// Trazo de polilinea con grosor total `g` (coordenadas normalizadas de -1 a 1).
fn trazo(u: f32, v: f32, pts: &[(f32, f32)], g: f32) -> bool {
    pts.windows(2).any(|s| dist_seg(u, v, s[0], s[1]) <= g / 2.0)
}

/// ¿El punto (u, v) en [-1, 1]^2 esta dentro del icono?
fn dentro(k: Icono, u: f32, v: f32) -> bool {
    match k {
        Icono::Engranaje => {
            let d = (u * u + v * v).sqrt();
            if d < 0.30 {
                return false; // agujero central
            }
            if d <= 0.68 {
                return true; // cuerpo
            }
            // 8 dientes trapezoidales
            let a = v.atan2(u).rem_euclid(std::f32::consts::TAU);
            let fase = (a / (std::f32::consts::TAU / 8.0)).fract();
            let medio = (fase - 0.5).abs(); // 0 en el centro del diente
            let limite = if d <= 0.80 { 0.26 } else { 0.20 }; // el diente se afina hacia la punta
            d <= 0.94 && medio < limite
        }
        Icono::Cerrar => trazo(u, v, &[(-0.5, -0.5), (0.5, 0.5)], 0.22) || trazo(u, v, &[(0.5, -0.5), (-0.5, 0.5)], 0.22),
        Icono::Check => trazo(u, v, &[(-0.6, 0.02), (-0.2, 0.46), (0.62, -0.42)], 0.24),
        Icono::Mas => trazo(u, v, &[(-0.6, 0.0), (0.6, 0.0)], 0.2) || trazo(u, v, &[(0.0, -0.6), (0.0, 0.6)], 0.2),
        Icono::Menos => trazo(u, v, &[(-0.6, 0.0), (0.6, 0.0)], 0.2),
        Icono::Circulo => u * u + v * v <= 0.85 * 0.85,
        Icono::Alerta => {
            // triangulo con la punta arriba (lados de la punta a las esquinas de abajo) y la exclamacion calada
            let dentro_tri = (-0.86..=0.78).contains(&v) && u.abs() <= (v + 0.86) / 1.64 * 0.95;
            let palo = u.abs() <= 0.11 && (-0.36..=0.26).contains(&v);
            let punto = u * u + (v - 0.52).powi(2) <= 0.12 * 0.12;
            dentro_tri && !palo && !punto
        }
        Icono::Fallo => u * u + v * v <= 0.9 * 0.9 && !trazo(u, v, &[(-0.38, -0.38), (0.38, 0.38)], 0.2) && !trazo(u, v, &[(0.38, -0.38), (-0.38, 0.38)], 0.2),
    }
}

/// Rasteriza una mascara: (ancho, alto, alfa de 8 bits fila a fila). Supermuestreo de 4x4 por pixel.
pub fn rasterizar(m: Mascara) -> (usize, usize, Vec<u8>) {
    const SS: usize = 4;
    let (w, h) = match m {
        Mascara::Esquina(r) => (r.max(1) as usize, r.max(1) as usize),
        Mascara::Icono(_, n) => (n.max(1) as usize, n.max(1) as usize),
    };
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut n = 0;
            for sy in 0..SS {
                for sx in 0..SS {
                    let (fx, fy) = ((x * SS + sx) as f32 + 0.5, (y * SS + sy) as f32 + 0.5);
                    let dentro = match m {
                        Mascara::Esquina(r) => {
                            // centro del disco en la esquina inferior derecha de la mascara
                            let r = r.max(1) as f32 * SS as f32;
                            (fx - r).powi(2) + (fy - r).powi(2) <= r * r
                        }
                        Mascara::Icono(k, _) => dentro(k, fx / (w * SS) as f32 * 2.0 - 1.0, fy / (h * SS) as f32 * 2.0 - 1.0),
                    };
                    if dentro {
                        n += 1;
                    }
                }
            }
            out[y * w + x] = (n * 255 / (SS * SS)) as u8;
        }
    }
    (w, h, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangulos() {
        let a = R::new(0.0, 0.0, 10.0, 10.0);
        let b = R::new(5.0, 5.0, 10.0, 10.0);
        assert_eq!(a.cruzar(&b), R::new(5.0, 5.0, 5.0, 5.0));
        assert!(a.se_cruza(&b));
        assert!(!a.se_cruza(&R::new(10.0, 0.0, 5.0, 5.0)));
        assert_eq!(a.cruzar(&R::new(20.0, 20.0, 1.0, 1.0)).w, 0.0);
        assert_eq!(a.reducir(2.0), R::new(2.0, 2.0, 6.0, 6.0));
        assert!(a.contiene(0.0, 0.0) && !a.contiene(10.0, 5.0));
    }

    #[test]
    fn contraste_wcag() {
        assert!((contraste(Color::rgb(0, 0, 0), Color::rgb(255, 255, 255)) - 21.0).abs() < 1e-6);
        assert!((contraste(Color::rgb(255, 255, 255), Color::rgb(255, 255, 255)) - 1.0).abs() < 1e-9);
    }

    /// Los pares de colores de la interfaz cumplen AA (4,5 el texto normal) en los dos temas.
    #[test]
    fn paleta_con_contraste_aa() {
        for (tn, t) in [("oscuro", tema::OSCURO), ("claro", tema::CLARO)] {
            paleta_aa(tn, &t);
        }
    }

    fn paleta_aa(tn: &str, t: &tema::Paleta) {
        let pares = [
            ("texto/fondo", t.texto, t.fondo),
            ("texto/superficie", t.texto, t.superficie),
            ("texto/control", t.texto, t.control),
            ("texto/hover", t.texto, t.control_hover),
            ("texto2/superficie", t.texto2, t.superficie),
            ("texto2/control", t.texto2, t.control),
            ("texto2/fondo", t.texto2, t.fondo),
            ("sobre acento", t.sobre_acento, t.acento),
            ("sobre acento hover", t.sobre_acento, t.acento_hover),
            ("sobre peligro", t.sobre_acento, t.peligro),
            ("sobre peligro hover", t.sobre_acento, t.peligro_hover),
            ("aviso/superficie", t.aviso, t.superficie),
            ("error/superficie", t.error, t.superficie),
            ("exito/superficie", t.exito, t.superficie),
            ("acento claro/superficie", t.acento_claro, t.superficie),
            ("acento claro/control", t.acento_claro, t.control),
            // con el raton encima de un control (boton secundario, pestana, chip de atajo o campo)
            ("texto2/hover", t.texto2, t.control_hover),
            ("acento claro/hover", t.acento_claro, t.control_hover),
            ("aviso/hover", t.aviso, t.control_hover),
            // la entrada de la navegacion con el raton encima (hover al 63 % sobre la superficie)
            ("texto/hover de la navegacion", t.texto, mezclar(t.control_hover.alfa(160), t.superficie)),
            // un control apagado sigue legible aunque no se exija (su texto es t.texto_apagado): la tecla y los valores
            ("texto/control apagado", t.texto, t.control_apagado),
            ("texto2/control apagado", t.texto2, t.control_apagado),
            // mensajes en linea y textos de estado sobre el fondo, los controles y la barra
            ("aviso/control", t.aviso, t.control),
            ("error/control", t.error, t.control),
            ("exito/control", t.exito, t.control),
            ("aviso/fondo", t.aviso, t.fondo),
            ("error/fondo", t.error, t.fondo),
            ("exito/fondo", t.exito, t.fondo),
            ("acento claro/fondo", t.acento_claro, t.fondo),
            ("texto/fondo de un campo editado", t.texto, t.fondo),
        ];
        for (n, a, b) in pares {
            let c = contraste(a, b);
            assert!(c >= 4.5, "{}: {}: contraste {:.2} < 4,5", tn, n, c);
        }
        // iconos y piezas graficas (3:1): el punto gris de la maquina en pausa, la bola del interruptor y el anillo de foco
        // (acento claro) sobre todo lo que puede rodear
        let graficos = [
            ("pausa/superficie", t.texto_apagado, t.superficie),
            ("bola/control", t.texto2, t.control),
            ("bola/hover", t.texto2, t.control_hover),
            ("foco/fondo", t.acento_claro, t.fondo),
            ("foco/superficie", t.acento_claro, t.superficie),
            ("foco/control", t.acento_claro, t.control),
            ("foco/hover", t.acento_claro, t.control_hover),
            ("foco/control apagado", t.acento_claro, t.control_apagado),
        ];
        for (n, a, b) in graficos {
            assert!(contraste(a, b) >= 3.0, "{}: {}: {:.2}", tn, n, contraste(a, b));
        }
    }

    /// El recorrido de un dibujo encuentra el fondo real de cada texto (rectangulos mezclados por alfa, respetando el
    /// recorte) y marca lo que no llega a AA, salvo lo deshabilitado.
    #[test]
    fn pares_de_color_de_un_dibujo() {
        for v in [tema::Variante::Oscuro, tema::Variante::Claro] {
            tema::con(v, || pares_de_un_dibujo(v.paleta()));
        }
    }

    fn pares_de_un_dibujo(t: &tema::Paleta) {
        let alto = |_: Estilo| 16.0;
        let p = vec![
            Pint::Rect { r: R::new(0.0, 0.0, 100.0, 100.0), c: t.superficie, radio: 0.0 },
            Pint::Texto { x: 10.0, y: 10.0, t: "legible".into(), e: Estilo::Cuerpo, c: t.texto },
            Pint::Rect { r: R::new(0.0, 40.0, 50.0, 20.0), c: t.control_apagado, radio: 4.0 },
            Pint::Texto { x: 5.0, y: 42.0, t: "apagado".into(), e: Estilo::Cuerpo, c: t.texto_apagado },
            Pint::Texto { x: 60.0, y: 42.0, t: "gris".into(), e: Estilo::Cuerpo, c: t.texto_apagado },
            Pint::Rect { r: R::new(0.0, 70.0, 100.0, 30.0), c: Color(0, 0, 0, 128), radio: 0.0 },
            Pint::Icono { k: Icono::Check, r: R::new(10.0, 75.0, 12.0, 12.0), c: t.sobre_acento },
            Pint::Recorte(Some(R::new(0.0, 0.0, 100.0, 30.0))),
            Pint::Texto { x: 10.0, y: 80.0, t: "recortado".into(), e: Estilo::Cuerpo, c: t.texto_apagado },
            Pint::Recorte(None),
            Pint::Texto { x: 10.0, y: 200.0, t: "  ".into(), e: Estilo::Cuerpo, c: t.texto_apagado },
        ];
        let pares = pares_de_color(&p, Color::rgb(0, 0, 0), &alto);
        let nombres: Vec<&str> = pares.iter().map(|x| x.que.as_str()).collect();
        assert_eq!(nombres, vec!["legible", "apagado", "gris", "Check"]);
        assert_eq!((pares[0].fondo, pares[1].fondo, pares[2].fondo), (t.superficie, t.control_apagado, t.superficie));
        // el velo negro al 50 % oscurece la superficie
        assert_eq!(pares[3].fondo, mezclar(Color(0, 0, 0, 128), t.superficie));
        let f = fallos_de_contraste(&pares);
        assert_eq!(f.len(), 1, "{:?}", f);
        assert!(f[0].contains("gris"), "{:?}", f);
        assert_eq!(mezclar(Color(255, 255, 255, 255), t.fondo), Color::rgb(255, 255, 255));
        assert_eq!(mezclar(Color(255, 255, 255, 0), t.fondo), t.fondo);
    }

    /// El tema que toca: lo fijado en `ventana.tema` manda; en automatico, solo un "prefiere claro" del escritorio da el
    /// claro (sin preferencia, sin portal o con un valor desconocido, el oscuro de siempre).
    #[test]
    fn eleccion_del_tema() {
        use tema::Variante::{Claro, Oscuro};
        for portal in [None, Some(0), Some(1), Some(2), Some(7)] {
            assert_eq!(tema::Variante::elegir("claro", portal), Claro);
            assert_eq!(tema::Variante::elegir("oscuro", portal), Oscuro);
        }
        assert_eq!(tema::Variante::elegir("auto", Some(2)), Claro);
        for portal in [None, Some(0), Some(1), Some(3), Some(u32::MAX)] {
            assert_eq!(tema::Variante::elegir("auto", portal), Oscuro, "{:?}", portal);
        }
        // el oscuro es exactamente el de antes y el forzado de las pruebas solo vale dentro de `con`
        assert_eq!(tema::Variante::Oscuro.paleta(), &tema::OSCURO);
        assert_eq!(tema::con(Claro, || *tema::p()), tema::CLARO);
        assert_eq!(tema::con(Oscuro, || tema::con(Claro, tema::variante)), Claro);
    }

    #[test]
    fn esquina_redondeada() {
        let (w, h, a) = rasterizar(Mascara::Esquina(6));
        assert_eq!((w, h, a.len()), (6, 6, 36));
        // la esquina de la caja (arriba a la izquierda) queda vacia y el pico hacia el centro, lleno
        assert!(a[0] < 40, "{}", a[0]);
        assert_eq!(a[5 * 6 + 5], 255);
        // el borde es suave: hay valores intermedios
        assert!(a.iter().any(|v| *v > 20 && *v < 235));
        // simetria respecto a la diagonal
        for y in 0..6 {
            for x in 0..6 {
                assert_eq!(a[y * 6 + x], a[x * 6 + y]);
            }
        }
    }

    #[test]
    fn iconos_con_forma() {
        for k in [Icono::Engranaje, Icono::Cerrar, Icono::Check, Icono::Mas, Icono::Menos, Icono::Circulo, Icono::Alerta, Icono::Fallo] {
            let (w, h, a) = rasterizar(Mascara::Icono(k, 16));
            assert_eq!(a.len(), w * h);
            let lleno = a.iter().filter(|v| **v > 127).count();
            assert!(lleno > 12 && lleno < 16 * 16, "{:?} cubre {} px", k, lleno);
        }
        // el engranaje tiene agujero central y dientes (esquina vacia, borde con relieve)
        let (_, _, g) = rasterizar(Mascara::Icono(Icono::Engranaje, 32));
        assert_eq!(g[16 * 32 + 16], 0, "agujero");
        assert_eq!(g[0], 0);
        assert!(g[16 * 32 + 24] > 200, "cuerpo");
        // los iconos de los mensajes se distinguen por la forma, no solo por el color: el aviso es un triangulo (esquinas de
        // arriba vacias) con la exclamacion calada; el error, un circulo con el aspa calada; el exito, una marca
        let (_, _, alerta) = rasterizar(Mascara::Icono(Icono::Alerta, 32));
        assert_eq!(alerta[2 * 32 + 2], 0, "esquina del triangulo");
        assert_eq!(alerta[16 * 32 + 16], 0, "palo de la exclamacion");
        assert!(alerta[24 * 32 + 8] > 200, "cuerpo del triangulo");
        let (_, _, fallo) = rasterizar(Mascara::Icono(Icono::Fallo, 32));
        assert_eq!(fallo[16 * 32 + 16], 0, "centro del aspa");
        assert!(fallo[4 * 32 + 16] > 200 && fallo[16 * 32 + 4] > 200, "disco");
        let (_, _, check) = rasterizar(Mascara::Icono(Icono::Check, 32));
        assert!(alerta != fallo && fallo != check && alerta != check);
    }
}
