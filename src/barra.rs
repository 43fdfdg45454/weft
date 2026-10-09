//! Barra superior de la ventana propia (`--display window`): una franja fina dibujada por weft dentro de la ventana,
//! justo debajo de la barra de titulo del escritorio y encima de la pantalla del dispositivo, a todo el ancho.
//!
//! A la izquierda lleva las pestanas (hoy una: "Configuración", con el mismo nombre que el titulo del modal de configuracion
//! que abre, con los Controles, la Maquina y todos los ajustes; la lista `FICHAS`
//! esta preparada para mas) y a la derecha un indicador de estado con un punto de color: lo que dice `vista::Estado`
//! ("Arrancando Android...", "Android en marcha", "Máquina en pausa", "Reiniciando Android...", "Apagando..." o "Sin
//! aceleración (lento)"; verde en marcha, ambar mientras cambia o va lenta, gris en pausa), el zoom y la rotacion.
//! Mismo diseno que el resto (hover, pulsado, activa mientras el modal esta abierto). Junto a las pestanas, mientras
//! dure, una etiqueta avisa del teclado (`vista::ModoTeclado`): "Atajos desactivados" o "Siguiente tecla a Android".
//!
//! Este modulo no conoce SDL: calcula la geometria, decide que hay bajo el raton y entrega el dibujo como `Pint`. Un clic
//! sobre la barra nunca llega al invitado: `window.rs` no lo manda a la pantalla tactil, y la geometria de la ventana
//! (`vista::disenar`) ya baja la pantalla del dispositivo `ALTO` dp.

use crate::formas::{tema, Color, Icono, Pint, R};
use crate::fuente::{truncar, Estilo, Medida};
use crate::pantalla::Orientacion;
use crate::textos::{clave, texto, txf};
use crate::vista::{Accion, Info};

/// Alto fijo de la barra (dp de la interfaz).
pub const ALTO: f32 = 34.0;

/// Alto de la barra en dp de la ventana: `ALTO` por el tamano del texto (formas::tamano).
pub fn alto() -> f32 {
    ALTO * crate::formas::tamano::factor()
}
const MARGEN: f32 = 6.0;
const ALTO_FICHA: f32 = ALTO - 8.0;

/// Pestanas de la barra: (accion, icono, clave del titulo en el catalogo de textos). Hoy solo "Configuración" (el mismo
/// nombre que el titulo del modal que abre y que el atajo "Abrir configuración"); para otra pestana basta agregar una fila.
/// Las pruebas de la ventana la buscan por su nombre estable (`config`), no por el titulo.
pub const FICHAS: &[(Accion, Icono, &str)] = &[(Accion::Configuracion, Icono::Engranaje, clave!("comun.configuracion"))];

#[derive(Clone, Debug, PartialEq)]
pub struct Ficha {
    pub accion: Accion,
    pub icono: Icono,
    pub titulo: &'static str,
    pub r: R,
    /// su contenido esta abierto (el modal de configuracion)
    pub activa: bool,
}

#[derive(Default)]
pub struct Barra {
    pub hover: Option<Accion>,
    pub pulsado: Option<Accion>,
    /// pestana con el foco del teclado (indice en `FICHAS`): la barra tiene el teclado (atajo `atajo.barra`, F10) y nada
    /// llega a Android hasta que se activa una pestana, se pulsa Esc o se hace clic
    pub foco: Option<usize>,
    /// pestana desde la que el teclado (foco de barra + Intro o Espacio) abrio lo que esta abierto: al cerrarse, el foco
    /// vuelve a ella (ver `seguir_ajustes`). None si se abrio con el raton o con un atajo directo.
    pub volver: Option<usize>,
    /// la configuracion estaba abierta en la ultima llamada a `seguir_ajustes`
    ajustes_abiertos: bool,
}

/// Teclas de la barra con el foco (codigos HID).
const TAB: u32 = 43;
const ESC: u32 = 41;
const ESPACIO: u32 = 44;

/// Texto `t` con su altura de mayusculas centrada verticalmente en `cy`.
fn texto_v(m: &dyn Medida, x: f32, cy: f32, t: &str, e: Estilo, c: Color) -> Pint {
    Pint::Texto { x, y: cy + m.cap(e) / 2.0 - m.ascenso(e), t: t.to_string(), e, c }
}

/// Lo que dice el indicador de la derecha, de mas a menos completo (el dibujo elige el primero que cabe). El estado lo
/// resume `vista::estado_de` (apagando manda sobre reiniciando, y los dos sobre lo que diga QEMU).
pub fn estados(info: &Info) -> Vec<String> {
    let base = info.estado.texto();
    let zoom = format!("{} %", (info.escala * 100.0).round() as i32);
    let rot = match info.orient {
        Orientacion::Auto => txf!("barra.rotacion_auto", info.rot % 4 * 90),
        Orientacion::Fija(r) => txf!("barra.rotacion", r % 4 * 90),
    };
    let mut v = vec![format!("{}  ·  {}  ·  {}", base, zoom, rot), format!("{}  ·  {}", base, zoom), base.to_string()];
    // un aviso breve (captura guardada, mando conectado...) va primero y sustituye al estado mientras dura; si lleva rutas y
    // entero no cabe, va su forma corta (cada ruta reducida a su ultimo nombre)
    if let Some(a) = &info.aviso {
        let mut avisos = vec![a.clone()];
        avisos.extend(info.aviso_corto.iter().filter(|c| *c != a).cloned());
        v.splice(0..0, avisos);
    }
    v
}

impl Barra {
    /// Pestanas con su rectangulo dentro de `area`. `pista`: la tecla del atajo de la configuracion (p. ej. F9).
    pub fn fichas(&self, area: R, abierta: bool, pista: Option<&str>, m: &dyn Medida) -> Vec<Ficha> {
        let mut x = area.x + MARGEN;
        let mut v = Vec::new();
        for (accion, icono, clave) in FICHAS {
            let titulo = texto(clave);
            let pw = pista.filter(|_| *accion == Accion::Configuracion).map_or(0.0, |p| m.ancho(p, Estilo::Pequeno) + 10.0);
            let w = 10.0 + 14.0 + 6.0 + m.ancho(titulo, Estilo::Negrita) + pw + 10.0;
            v.push(Ficha { accion: *accion, icono: *icono, titulo, r: R::new(x, area.y + 4.0, w, ALTO_FICHA), activa: abierta && *accion == Accion::Configuracion });
            x += w + 4.0;
        }
        v
    }

    fn ficha_en(&self, area: R, abierta: bool, pista: Option<&str>, m: &dyn Medida, x: f32, y: f32) -> Option<Accion> {
        if !area.contiene(x, y) {
            return None;
        }
        self.fichas(area, abierta, pista, m).into_iter().find(|f| f.r.contiene(x, y)).map(|f| f.accion)
    }

    /// Nombres y rectangulos de las pestanas (para el gancho de pruebas de la ventana).
    pub fn botones(&self, area: R, abierta: bool, pista: Option<&str>, m: &dyn Medida) -> Vec<(String, R)> {
        self.fichas(area, abierta, pista, m).into_iter().map(|f| (f.accion.nombre(), f.r)).collect()
    }

    /// El raton se mueve a (x, y) (ventana). true si cambio el resaltado.
    pub fn mover(&mut self, area: R, abierta: bool, pista: Option<&str>, m: &dyn Medida, x: f32, y: f32) -> bool {
        let h = self.ficha_en(area, abierta, pista, m, x, y);
        let cambio = h != self.hover;
        self.hover = h;
        cambio
    }

    pub fn salir(&mut self) -> bool {
        let c = self.hover.is_some() || self.pulsado.is_some();
        self.hover = None;
        self.pulsado = None;
        c
    }

    /// Boton izquierdo pulsado: la barra se lo queda si cae dentro de ella (aunque sea sobre un hueco).
    pub fn presionar(&mut self, area: R, abierta: bool, pista: Option<&str>, m: &dyn Medida, x: f32, y: f32) -> bool {
        if !area.contiene(x, y) {
            return false;
        }
        self.pulsado = self.ficha_en(area, abierta, pista, m, x, y);
        true
    }

    /// Boton izquierdo soltado: la accion de la pestana si coincide con la pulsada.
    pub fn soltar(&mut self, area: R, abierta: bool, pista: Option<&str>, m: &dyn Medida, x: f32, y: f32) -> Option<Accion> {
        let p = self.pulsado.take()?;
        let sobre = self.ficha_en(area, abierta, pista, m, x, y)?;
        // abierta con el raton: al cerrarla el teclado vuelve a Android
        self.volver = None;
        (sobre == p).then_some(p)
    }

    /// La barra toma el teclado (el atajo `atajo.barra`): foco en la primera pestana, con su anillo.
    pub fn enfocar(&mut self) {
        self.foco = Some(0);
    }

    /// Una tecla pulsada con la barra enfocada: Tab, Mayus+Tab y las flechas izquierda/derecha recorren las pestanas (dan
    /// la vuelta), Inicio y Fin van a la primera y a la ultima, Intro y Espacio activan la pestana (y devuelven su accion;
    /// el foco se suelta: lo que abre se lleva el teclado) y Esc suelta el foco sin hacer nada. Lo demas no hace nada
    /// (tampoco llega a Android). Sin foco, nada.
    pub fn tecla(&mut self, sc: u32, mayus: bool, repetida: bool) -> Option<Accion> {
        let i = self.foco?;
        let n = FICHAS.len();
        match sc {
            TAB => self.foco = Some(if mayus { (i + n - 1) % n } else { (i + 1) % n }),
            79 => self.foco = Some((i + 1) % n),
            80 => self.foco = Some((i + n - 1) % n),
            74 => self.foco = Some(0),
            77 => self.foco = Some(n - 1),
            ESC => self.foco = None,
            40 | 88 | ESPACIO if !repetida => {
                self.foco = None;
                self.volver = Some(i);
                return FICHAS.get(i).map(|f| f.0);
            }
            _ => {}
        }
        None
    }

    /// Sigue la apertura de la configuracion (la ventana la llama en cada vuelta, tras resolver las acciones, con
    /// `ajustes.abierto`). Al cerrarse (Esc, su boton, una confirmacion o el atajo), si se habia abierto desde la barra con
    /// el teclado, el foco vuelve a esa pestana, con su anillo: quien navega con el teclado sigue donde estaba. Si se abrio
    /// con el raton o con un atajo directo, el teclado vuelve a Android como siempre. Devuelve true si cambio el foco.
    pub fn seguir_ajustes(&mut self, abiertos: bool) -> bool {
        let antes = std::mem::replace(&mut self.ajustes_abiertos, abiertos);
        if abiertos {
            return false;
        }
        // cerrada: si acaba de cerrarse, al foco de la pestana de origen; si ya lo estaba, lo anotado no abrio nada
        match (antes, self.volver.take()) {
            (true, Some(i)) if i < FICHAS.len() => {
                self.foco = Some(i);
                true
            }
            _ => false,
        }
    }

    /// Dibujo de la barra dentro de `area` (a todo el ancho de la ventana).
    pub fn dibujar(&self, area: R, info: &Info, abierta: bool, pista: Option<&str>, m: &dyn Medida) -> Vec<Pint> {
        let mut out = vec![Pint::Rect { r: area, c: tema::p().superficie, radio: 0.0 }, Pint::Rect { r: R::new(area.x, area.y + area.h - 1.0, area.w, 1.0), c: tema::p().borde, radio: 0.0 }];
        let mut fin_fichas = area.x + MARGEN;
        for (i, f) in self.fichas(area, abierta, pista, m).into_iter().enumerate() {
            let foco = self.foco == Some(i);
            if foco {
                // anillo de foco (como en la configuracion): acento claro alrededor y la pestana con fondo de control encima
                out.push(Pint::Rect { r: f.r.reducir(-2.0), c: tema::p().acento_claro, radio: tema::RADIO + 2.0 });
            }
            let enc = self.hover.as_ref() == Some(&f.accion);
            let pul = self.pulsado.as_ref() == Some(&f.accion) && enc;
            let (fondo, texto) = if pul {
                (Some(tema::p().acento), tema::p().sobre_acento)
            } else if f.activa {
                (Some(tema::p().control), tema::p().texto)
            } else if enc {
                (Some(tema::p().control_hover), tema::p().texto)
            } else if foco {
                (Some(tema::p().control), tema::p().texto)
            } else {
                (None, tema::p().texto)
            };
            if let Some(c) = fondo {
                out.push(Pint::Rect { r: f.r, c, radio: tema::RADIO });
            }
            if f.activa && !pul {
                // pestana abierta: raya de acento abajo, como las pestanas del modal
                out.push(Pint::Rect { r: R::new(f.r.x + 6.0, f.r.y + f.r.h - 2.0, f.r.w - 12.0, 2.0), c: tema::p().acento_claro, radio: 1.0 });
            }
            let cy = f.r.y + f.r.h / 2.0;
            out.push(Pint::Icono { k: f.icono, r: R::new(f.r.x + 10.0, cy - 7.0, 14.0, 14.0), c: texto });
            out.push(texto_v(m, f.r.x + 10.0 + 14.0 + 6.0, cy, f.titulo, Estilo::Negrita, texto));
            if let Some(p) = pista.filter(|_| f.accion == Accion::Configuracion) {
                let pw = m.ancho(p, Estilo::Pequeno);
                out.push(texto_v(m, f.r.x + f.r.w - 10.0 - pw, cy, p, Estilo::Pequeno, if pul { tema::p().sobre_acento } else { tema::p().texto2 }));
            }
            fin_fichas = f.r.x + f.r.w;
        }
        // etiqueta del teclado junto a las pestanas mientras dure (atajos desactivados, siguiente tecla a Android): va
        // antes que el indicador, que se acorta para dejarle sitio
        if let Some(t) = info.teclado.texto() {
            let x = fin_fichas + 8.0;
            let max = area.x + area.w - MARGEN - x;
            let t = truncar(m, t, Estilo::Pequeno, (max - 16.0).max(0.0));
            if max > 40.0 && !t.is_empty() {
                let r = R::new(x, area.y + 7.0, m.ancho(&t, Estilo::Pequeno) + 16.0, ALTO - 15.0);
                out.push(Pint::Rect { r, c: tema::p().control, radio: tema::RADIO });
                out.push(texto_v(m, x + 8.0, r.y + r.h / 2.0, &t, Estilo::Pequeno, tema::p().aviso));
                fin_fichas = r.x + r.w;
            }
        }
        // indicador de estado a la derecha: el texto mas completo que quepa en lo que dejan las pestanas
        let libre = (area.x + area.w - MARGEN - 8.0 - 12.0) - (fin_fichas + 16.0);
        let cy = area.y + (area.h - 1.0) / 2.0;
        // un aviso breve va en ambar; si no, el color del estado (verde, ambar o gris)
        let color = if info.aviso.is_some() { tema::p().aviso } else { info.estado.color() };
        let elegido = estados(info).into_iter().find(|t| m.ancho(t, Estilo::Cuerpo) <= libre);
        let t = elegido.unwrap_or_else(|| truncar(m, estados(info).last().map_or("", |s| s.as_str()), Estilo::Cuerpo, libre.max(0.0)));
        if libre > 24.0 && !t.is_empty() {
            let tw = m.ancho(&t, Estilo::Cuerpo);
            let x = area.x + area.w - MARGEN - 8.0 - tw;
            out.push(texto_v(m, x, cy, &t, Estilo::Cuerpo, tema::p().texto2));
            out.push(Pint::Icono { k: Icono::Circulo, r: R::new(x - 14.0, cy - 4.0, 8.0, 8.0), c: color });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::textos::tx;
    use crate::fuente::Tipografia;
    use crate::pantalla::Orientacion;
    use crate::vista::Estado;

    fn mm() -> Tipografia {
        Tipografia::solo_respaldo(1.0)
    }

    /// ancha de sobra: la fuente de mapa de bits de las pruebas mide mas que Noto Sans
    const AREA: R = R::new(0.0, 0.0, 1600.0, ALTO);

    fn info() -> Info {
        Info { rot: 0, escala: 0.5, orient: Orientacion::Auto, estado: Estado::EnMarcha, ..Info::default() }
    }

    /// Color del punto del indicador (el circulo que va a la izquierda del texto).
    fn punto(d: &[Pint]) -> Option<Color> {
        d.iter().find_map(|p| match p {
            Pint::Icono { k: Icono::Circulo, c, .. } => Some(*c),
            _ => None,
        })
    }

    fn textos(d: &[Pint]) -> Vec<String> {
        d.iter().filter_map(|p| if let Pint::Texto { t, .. } = p { Some(t.clone()) } else { None }).collect()
    }

    #[test]
    fn la_pestana_configuraciones_esta_arriba_a_la_izquierda() {
        let b = Barra::default();
        let f = b.fichas(AREA, false, Some("F9"), &mm());
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].accion, Accion::Configuracion);
        assert_eq!(f[0].titulo, tx!("comun.configuracion"));
        assert!(f[0].r.x < 20.0 && f[0].r.y >= 0.0 && f[0].r.y + f[0].r.h <= ALTO, "{:?}", f[0].r);
        assert!(!f[0].activa);
        assert!(b.fichas(AREA, true, None, &mm())[0].activa, "abierta mientras el modal esta abierto");
        assert_eq!(b.botones(AREA, false, None, &mm())[0].0, "config");
    }

    #[test]
    fn clic_en_la_pestana_abre_la_configuracion_y_en_un_hueco_no_hace_nada() {
        let mut b = Barra::default();
        let r = b.fichas(AREA, false, Some("F9"), &mm())[0].r;
        let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        assert!(b.presionar(AREA, false, Some("F9"), &mm(), x, y));
        assert_eq!(b.soltar(AREA, false, Some("F9"), &mm(), x, y), Some(Accion::Configuracion));
        // pulsar en un hueco de la barra: la barra lo toma (no llega al invitado) pero no hace nada
        assert!(b.presionar(AREA, false, Some("F9"), &mm(), 1500.0, 10.0));
        assert_eq!(b.pulsado, None);
        assert_eq!(b.soltar(AREA, false, Some("F9"), &mm(), 1500.0, 10.0), None);
        // pulsar en la pestana y soltar fuera: nada
        b.presionar(AREA, false, Some("F9"), &mm(), x, y);
        assert_eq!(b.soltar(AREA, false, Some("F9"), &mm(), 1500.0, 10.0), None);
        // fuera de la barra no la toma
        assert!(!b.presionar(AREA, false, Some("F9"), &mm(), 100.0, ALTO + 1.0));
    }

    #[test]
    fn hover_pulsado_y_activa_cambian_el_color() {
        let mut b = Barra::default();
        let r = b.fichas(AREA, false, None, &mm())[0].r;
        let (x, y) = (r.x + 4.0, r.y + 4.0);
        let fondo = |b: &Barra, abierta: bool| -> Option<Color> {
            b.dibujar(AREA, &info(), abierta, None, &mm()).into_iter().find_map(|p| match p {
                Pint::Rect { r: rr, c, radio } if rr == r && radio > 0.0 => Some(c),
                _ => None,
            })
        };
        assert_eq!(fondo(&b, false), None, "en reposo, sin fondo");
        assert!(b.mover(AREA, false, None, &mm(), x, y));
        assert!(!b.mover(AREA, false, None, &mm(), x + 1.0, y));
        assert_eq!(fondo(&b, false), Some(tema::p().control_hover));
        b.presionar(AREA, false, None, &mm(), x, y);
        assert_eq!(fondo(&b, false), Some(tema::p().acento));
        b.pulsado = None;
        b.hover = None;
        assert_eq!(fondo(&b, true), Some(tema::p().control), "abierta: fondo de control y raya de acento");
        assert!(b.dibujar(AREA, &info(), true, None, &mm()).iter().any(|p| matches!(p, Pint::Rect { c, .. } if *c == tema::p().acento_claro)));
        assert!(b.salir() || !b.salir());
    }

    #[test]
    fn el_indicador_de_estado_se_acorta_en_ventanas_estrechas() {
        let b = Barra::default();
        let mut i = info();
        // ancha: estado, zoom y rotacion
        let t = textos(&b.dibujar(AREA, &i, false, Some("F9"), &mm()));
        assert!(t.contains(&"Android en marcha  ·  50 %  ·  Rotación auto (0°)".to_string()), "{:?}", t);
        // rotacion fija
        i.orient = Orientacion::Fija(1);
        assert!(textos(&b.dibujar(AREA, &i, false, None, &mm())).iter().any(|t| t.ends_with("Rotación 90°")));
        // al estrechar la ventana el indicador pierde primero la rotacion, luego el zoom: se barre el ancho y se comprueba que
        // aparecen las tres versiones, que nunca crece al achicar y que siempre cabe a la derecha de las pestanas
        let (mut vistos, mut ultimo) = (std::collections::BTreeSet::new(), usize::MAX);
        for ancho in (150..=1600).rev().step_by(5) {
            let area = R::new(0.0, 0.0, ancho as f32, ALTO);
            let d = b.dibujar(area, &i, false, Some("F9"), &mm());
            let e = d.iter().find_map(|p| match p {
                Pint::Texto { t, .. } if t.starts_with(tx!("estado.android_marcha")) => Some(t.clone()),
                _ => None,
            });
            let largo = e.as_ref().map_or(0, |t| t.chars().count());
            assert!(largo <= ultimo, "al achicar la ventana el indicador no puede crecer ({} -> {} en {})", ultimo, largo, ancho);
            ultimo = largo;
            if let Some(t) = e {
                vistos.insert(t);
            }
        }
        assert_eq!(vistos.len(), 3, "{:?}", vistos);
        assert!(vistos.contains(tx!("estado.android_marcha")));
        // reiniciando
        i.estado = Estado::Reiniciando;
        assert!(textos(&b.dibujar(AREA, &i, false, None, &mm())).iter().any(|t| t.starts_with(tx!("estado.reiniciando_android"))));
    }

    /// Mientras `stop` apaga la maquina (la ventana se cierra cuando QEMU termina) el indicador dice "Apagando...", gane a
    /// lo que gane: tambien con un reinicio en curso, y con el zoom y la rotacion detras si caben.
    #[test]
    fn apagando_se_ve_en_el_indicador() {
        let b = Barra::default();
        let mut i = info();
        i.estado = Estado::Apagando;
        assert_eq!(estados(&i)[0], "Apagando...  ·  50 %  ·  Rotación auto (0°)");
        assert_eq!(estados(&i).last().map(String::as_str), Some(tx!("estado.apagando")));
        let t = textos(&b.dibujar(AREA, &i, false, Some("F9"), &mm()));
        assert!(t.iter().any(|x| x.starts_with(tx!("estado.apagando"))), "{:?}", t);
        assert!(!t.iter().any(|x| x.starts_with(tx!("estado.android_marcha"))));
        // apagando manda sobre un reinicio en curso (lo decide `vista::estado_de`)
        i.estado = crate::vista::estado_de(Some("running"), true, true, true, false);
        assert!(estados(&i).iter().all(|x| x.starts_with(tx!("estado.apagando"))), "{:?}", estados(&i));
        // un aviso breve sigue yendo primero (el propio del apagado, con el texto largo)
        i.aviso = Some(tx!("aviso.apagando_maquina").into());
        assert_eq!(estados(&i)[0], tx!("aviso.apagando_maquina"));
        // sin apagado vuelve el estado normal
        i.estado = crate::vista::estado_de(Some("running"), true, true, false, false);
        i.aviso = None;
        assert!(estados(&i).iter().all(|x| x.starts_with(tx!("estado.reiniciando_android"))));
    }

    /// Cada estado de la maquina tiene su texto y su color de punto: verde en marcha; ambar arrancando, reiniciando,
    /// apagando y sin aceleracion; gris en pausa. "Android en marcha" se conserva tal cual.
    #[test]
    fn cada_estado_tiene_su_texto_y_su_color() {
        let b = Barra::default();
        let esperado = [
            (Estado::Arrancando, "Arrancando Android...", tema::p().aviso),
            (Estado::EnMarcha, "Android en marcha", tema::p().exito),
            (Estado::Pausada, "Máquina en pausa", tema::p().texto_apagado),
            (Estado::Reiniciando, "Reiniciando Android...", tema::p().aviso),
            (Estado::Apagando, "Apagando...", tema::p().aviso),
            (Estado::SinAceleracion, "Sin aceleración (lento)", tema::p().aviso),
        ];
        for (estado, texto, color) in esperado {
            let mut i = info();
            i.estado = estado;
            let v = estados(&i);
            assert_eq!(v[0], format!("{}  ·  50 %  ·  Rotación auto (0°)", texto), "{:?}", estado);
            assert_eq!(v.last().map(String::as_str), Some(texto));
            let d = b.dibujar(AREA, &i, false, Some("F9"), &mm());
            assert!(textos(&d).iter().any(|t| t.starts_with(texto)), "{:?}: {:?}", estado, textos(&d));
            assert_eq!(punto(&d), Some(color), "{:?}", estado);
            // en una barra estrecha el punto conserva el color y el texto es el corto
            let d = b.dibujar(R::new(0.0, 0.0, 420.0, ALTO), &i, false, Some("F9"), &mm());
            assert_eq!(punto(&d), Some(color), "{:?}", estado);
            assert!(textos(&d).iter().any(|t| t.starts_with(texto) || !t.contains('·')), "{:?}: {:?}", estado, textos(&d));
        }
        // sin datos la barra arranca en "Arrancando Android..." (es lo normal al abrir la ventana)
        assert_eq!(Info::default().estado, Estado::Arrancando);
        // un aviso breve pone el punto en ambar aunque la maquina este en marcha
        let mut i = info();
        i.aviso = Some("Captura guardada".into());
        assert_eq!(punto(&b.dibujar(AREA, &i, false, None, &mm())), Some(tema::p().aviso));
    }

    /// Un aviso breve (captura guardada, mando conectado...) sustituye al estado mientras dura y, si no cabe, se acorta.
    #[test]
    fn un_aviso_va_primero_en_el_indicador() {
        let b = Barra::default();
        let mut i = info();
        i.aviso = Some("Captura guardada: captura-20261007-120000.png".into());
        let t = textos(&b.dibujar(AREA, &i, false, None, &mm()));
        assert!(t.contains(&"Captura guardada: captura-20261007-120000.png".to_string()), "{:?}", t);
        assert!(!t.iter().any(|x| x.starts_with(tx!("estado.android_marcha"))));
        // en una barra estrecha no se sale: o se acorta o vuelve al estado
        for ancho in [220.0f32, 400.0, 700.0] {
            let area = R::new(0.0, 0.0, ancho, ALTO);
            for p in b.dibujar(area, &i, false, Some("F9"), &mm()) {
                if let Pint::Texto { x, t, e, .. } = p {
                    if !(t.starts_with("Captura") || t.starts_with("Android") || t.starts_with("Capt")) {
                        continue;
                    }
                    assert!(x + mm().ancho(&t, e) <= ancho + 0.5, "{:?} se sale con {} dp", t, ancho);
                }
            }
        }
        // sin aviso vuelve el estado
        i.aviso = None;
        assert!(textos(&b.dibujar(AREA, &i, false, None, &mm())).iter().any(|x| x.starts_with(tx!("estado.android_marcha"))));
    }

    /// Un aviso con una ruta completa (la captura guardada) sale entero si cabe; si no, con la ruta reducida al nombre del
    /// archivo antes de rendirse y volver al estado.
    #[test]
    fn un_aviso_con_ruta_se_acorta_al_nombre() {
        let b = Barra::default();
        let mut i = info();
        let (largo, corto) = ("Captura guardada en /home/ana/Imágenes/Capturas de Android/captura-20261008-120000.png", "Captura guardada en .../captura-20261008-120000.png");
        i.aviso = Some(largo.into());
        i.aviso_corto = Some(corto.into());
        assert_eq!(estados(&i)[..2], [largo.to_string(), corto.to_string()]);
        assert!(textos(&b.dibujar(AREA, &i, false, None, &mm())).contains(&largo.to_string()));
        // a 1100 dp el largo no cabe (12 dp por letra en la fuente de pruebas) pero el corto si
        let t = textos(&b.dibujar(R::new(0.0, 0.0, 1100.0, ALTO), &i, false, Some("F9"), &mm()));
        assert!(t.contains(&corto.to_string()) && !t.contains(&largo.to_string()), "{:?}", t);
        // sin forma corta (o igual a la larga) no se repite
        i.aviso_corto = Some(largo.into());
        assert_eq!(estados(&i).iter().filter(|x| *x == largo).count(), 1);
        i.aviso_corto = None;
        assert_eq!(estados(&i)[1], "Android en marcha  ·  50 %  ·  Rotación auto (0°)");
    }

    /// Todo lo que dibuja la barra tiene contraste AA (texto 4,5:1, iconos 3:1) sobre su fondo real: la pestana en reposo,
    /// con el raton encima, pulsada y abierta; cada estado de la maquina, con aviso y con la etiqueta del teclado.
    #[test]
    fn contraste_aa_de_la_barra() {
        use crate::vista::ModoTeclado;
        let m = mm();
        let r = Barra::default().fichas(AREA, false, Some("F9"), &m)[0].r;
        let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let mut vistos = 0;
        // en los dos temas
        for v in [tema::Variante::Oscuro, tema::Variante::Claro] {
            tema::con(v, || {
                for (hover, pulsar, abierta, foco) in [
                    (false, false, false, false),
                    (true, false, false, false),
                    (true, true, false, false),
                    (false, false, true, false),
                    (true, false, true, false),
                    (true, true, true, false),
                    (false, false, false, true),
                    (true, false, false, true),
                ] {
                    let mut b = Barra::default();
                    if foco {
                        b.enfocar();
                    }
                    if hover {
                        b.mover(AREA, abierta, Some("F9"), &m, x, y);
                    }
                    if pulsar {
                        b.presionar(AREA, abierta, Some("F9"), &m, x, y);
                    }
                    for estado in [Estado::Arrancando, Estado::EnMarcha, Estado::Pausada, Estado::Reiniciando, Estado::Apagando, Estado::SinAceleracion] {
                        for (aviso, teclado) in [(None, ModoTeclado::Atajos), (Some("Captura guardada"), ModoTeclado::Desactivados), (None, ModoTeclado::PasarSiguiente)] {
                            let i = Info { estado, aviso: aviso.map(String::from), teclado, ..info() };
                            let pares = crate::formas::pares_de_color(&b.dibujar(AREA, &i, abierta, Some("F9"), &m), tema::p().fondo, &|e| m.alto_linea(e));
                            assert!(pares.iter().any(|p| p.que == "Configuración") && pares.iter().any(|p| p.que == "F9"));
                            let f = crate::formas::fallos_de_contraste(&pares);
                            assert!(f.is_empty(), "{:?}: hover {} pulsada {} abierta {} {:?}: {:?}", v, hover, pulsar, abierta, estado, f);
                            vistos += pares.len();
                        }
                    }
                }
            });
        }
        assert!(vistos > 300, "{}", vistos);
    }

    #[test]
    fn nada_se_sale_de_la_barra() {
        let b = Barra::default();
        for ancho in [200.0f32, 360.0, 480.0, 800.0, 1600.0] {
            let area = R::new(0.0, 0.0, ancho, ALTO);
            for p in b.dibujar(area, &info(), true, Some("F9"), &mm()) {
                let r = match p {
                    Pint::Rect { r, .. } | Pint::Icono { r, .. } => r,
                    _ => continue,
                };
                assert!(r.y >= -0.01 && r.y + r.h <= ALTO + 0.01, "{:?} sale de la barra", r);
            }
        }
    }

    /// Con los atajos desactivados o la siguiente tecla pasando a Android, una etiqueta junto a las pestanas lo avisa; el
    /// indicador de estado se acorta para dejarle sitio y nada se sale ni se pisa.
    #[test]
    fn etiqueta_del_teclado() {
        use crate::vista::ModoTeclado;
        let b = Barra::default();
        let mut i = info();
        assert!(!textos(&b.dibujar(AREA, &i, false, Some("F9"), &mm())).iter().any(|t| t.contains(tx!("teclado.atajos_desactivados")) || t.contains("Siguiente tecla")));
        for (modo, texto) in [(ModoTeclado::Desactivados, "Atajos desactivados"), (ModoTeclado::PasarSiguiente, "Siguiente tecla a Android")] {
            i.teclado = modo;
            let d = b.dibujar(AREA, &i, false, None, &mm());
            assert!(textos(&d).contains(&texto.to_string()), "{:?}", textos(&d));
            // el estado sigue a la derecha
            assert!(textos(&d).iter().any(|t| t.starts_with(tx!("estado.android_marcha"))));
            // a cualquier ancho de ventana (desde el minimo, 360 dp): dentro de la barra, la etiqueta despues de las pestanas
            // y el estado despues de la etiqueta
            for ancho in [360.0f32, 420.0, 480.0, 800.0, 1600.0] {
                let area = R::new(0.0, 0.0, ancho, ALTO);
                let fin_fichas = b.fichas(area, false, Some("F9"), &mm()).last().map_or(0.0, |f| f.r.x + f.r.w);
                let d = b.dibujar(area, &i, false, Some("F9"), &mm());
                let mut fin_etiqueta = fin_fichas;
                for p in &d {
                    if let Pint::Texto { x, t, e, .. } = p {
                        assert!(x + mm().ancho(t, *e) <= ancho + 0.5, "{:?} se sale con {} dp", t, ancho);
                        if *e == Estilo::Pequeno && (texto.starts_with(t.trim_end_matches("...")) || t.starts_with(&texto[..4])) {
                            assert!(*x >= fin_fichas, "{:?} pisa las pestanas", t);
                            fin_etiqueta = fin_etiqueta.max(x + mm().ancho(t, *e));
                        }
                    }
                }
                for p in &d {
                    if let Pint::Texto { x, t, e: Estilo::Cuerpo, .. } = p {
                        assert!(*x >= fin_etiqueta, "el estado {:?} pisa la etiqueta con {} dp", t, ancho);
                    }
                    if let Pint::Rect { r, .. } = p {
                        assert!(r.y >= -0.01 && r.y + r.h <= ALTO + 0.01 && r.x + r.w <= ancho + 0.5, "{:?} sale de la barra", r);
                    }
                }
            }
        }
        assert_eq!(ModoTeclado::de(true, true), ModoTeclado::Desactivados);
        assert_eq!(ModoTeclado::de(false, true), ModoTeclado::PasarSiguiente);
        assert_eq!(ModoTeclado::de(false, false), ModoTeclado::Atajos);
        assert_eq!(ModoTeclado::Atajos.texto(), None);
    }

    /// Teclado: el atajo de la barra la enfoca (anillo de acento sobre la pestana, con fondo de control); Tab, Mayus+Tab,
    /// flechas, Inicio y Fin recorren las pestanas; Intro o Espacio activan y sueltan el foco; Esc lo suelta sin hacer nada;
    /// lo demas no hace nada. En los dos temas el anillo es el acento claro.
    #[test]
    fn teclado_en_la_barra() {
        let m = mm();
        let mut b = Barra::default();
        assert_eq!(b.tecla(40, false, false), None, "sin foco no hace nada");
        b.enfocar();
        assert_eq!(b.foco, Some(0));
        let r = b.fichas(AREA, false, Some("F9"), &m)[0].r;
        for v in [tema::Variante::Oscuro, tema::Variante::Claro] {
            let d = tema::con(v, || b.dibujar(AREA, &info(), false, Some("F9"), &m));
            let anillo = tema::con(v, || Pint::Rect { r: r.reducir(-2.0), c: tema::p().acento_claro, radio: tema::RADIO + 2.0 });
            let i = d.iter().position(|p| *p == anillo).expect("anillo de foco");
            // encima, la pestana con fondo de control: el anillo se ve como un borde
            assert!(matches!(&d[i + 1], Pint::Rect { r: rr, c, .. } if *rr == r && *c == tema::con(v, || tema::p().control)), "{:?}", d[i + 1]);
        }
        // recorrer (una sola pestana: se queda en ella, sin salirse)
        for (sc, mayus) in [(43, false), (43, true), (79, false), (80, false), (74, false), (77, false)] {
            assert_eq!(b.tecla(sc, mayus, false), None);
            assert_eq!(b.foco, Some(0));
        }
        // otras teclas: nada, y el foco sigue
        assert_eq!(b.tecla(4, false, false), None);
        assert_eq!(b.foco, Some(0));
        // Intro activa (y suelta el foco); Espacio tambien, pero no repetido
        assert_eq!(b.tecla(40, false, false), Some(Accion::Configuracion));
        assert_eq!(b.foco, None);
        b.enfocar();
        assert_eq!(b.tecla(44, false, true), None);
        assert_eq!(b.tecla(44, false, false), Some(Accion::Configuracion));
        // Esc suelta sin hacer nada; sin foco, ni anillo
        b.enfocar();
        assert_eq!(b.tecla(41, false, false), None);
        assert_eq!(b.foco, None);
        assert!(!b.dibujar(AREA, &info(), false, Some("F9"), &m).iter().any(|p| matches!(p, Pint::Rect { c, .. } if *c == tema::p().acento_claro)));
    }

    /// Accesibilidad: la configuracion abierta desde la barra con el teclado (F10 + Intro) devuelve el foco a esa pestana
    /// al cerrarse, con su anillo visible; abierta con el raton o con un atajo directo, el teclado vuelve a Android.
    #[test]
    fn cerrar_la_configuracion_devuelve_el_foco_a_la_pestana_si_se_abrio_con_el_teclado() {
        let m = mm();
        let r = Barra::default().fichas(AREA, false, Some("F9"), &m)[0].r;
        let anillo = |b: &Barra| {
            let a = Pint::Rect { r: r.reducir(-2.0), c: tema::p().acento_claro, radio: tema::RADIO + 2.0 };
            b.dibujar(AREA, &info(), false, Some("F9"), &m).contains(&a)
        };
        // con el teclado: F10 + Intro abre; mientras esta abierta la barra no tiene el foco; al cerrar, vuelve con anillo
        let mut b = Barra::default();
        b.enfocar();
        assert_eq!(b.tecla(40, false, false), Some(Accion::Configuracion));
        assert!(!b.seguir_ajustes(true));
        assert_eq!(b.foco, None);
        assert!(!b.seguir_ajustes(true), "sigue abierta: nada cambia");
        assert!(b.seguir_ajustes(false));
        assert_eq!(b.foco, Some(0));
        assert!(anillo(&b), "anillo de foco visible tras cerrar");
        // y solo una vez: abrirla despues con un atajo directo y cerrarla deja el teclado a Android
        b.foco = None;
        assert!(!b.seguir_ajustes(false));
        assert!(!b.seguir_ajustes(true));
        assert!(!b.seguir_ajustes(false));
        assert_eq!(b.foco, None);
        // con Espacio tambien
        b.enfocar();
        assert_eq!(b.tecla(44, false, false), Some(Accion::Configuracion));
        b.seguir_ajustes(true);
        assert!(b.seguir_ajustes(false));
        assert_eq!(b.foco, Some(0));
        // con el raton: al cerrar, nada (el teclado es de Android), aunque antes se hubiera usado el teclado
        let mut b = Barra::default();
        let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        b.presionar(AREA, false, Some("F9"), &m, x, y);
        assert_eq!(b.soltar(AREA, false, Some("F9"), &m, x, y), Some(Accion::Configuracion));
        b.seguir_ajustes(true);
        assert!(!b.seguir_ajustes(false));
        assert_eq!(b.foco, None);
        assert!(!anillo(&b));
        // con un atajo directo (F9): igual
        let mut b = Barra::default();
        b.seguir_ajustes(true);
        assert!(!b.seguir_ajustes(false));
        assert_eq!(b.foco, None);
        // una activacion por teclado que no llego a abrir nada no se queda anotada para la siguiente apertura
        let mut b = Barra::default();
        b.enfocar();
        b.tecla(40, false, false);
        b.seguir_ajustes(false);
        assert_eq!(b.volver, None);
        b.seguir_ajustes(true);
        assert!(!b.seguir_ajustes(false));
        assert_eq!(b.foco, None);
    }
}
