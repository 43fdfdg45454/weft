//! Gestos y atajos de la ventana, como funciones puras (sin SDL ni D-Bus) para poder probarlos sin maquina:
//! rueda del raton -> deslizamiento con el lapiz, Ctrl+clic -> pellizco con dos contactos y atajos de teclado.
//! La ventana (window.rs) y la orden interna `dbus-input` envian las operaciones tal cual salen de aqui.
//!
//! Tambien decide que teclas son atajos de la ventana y cuales van al invitado (`FiltroAtajos`), lleva los modificadores
//! apretados en el teclado real (`Mods`), los atajos propios de la ventana que config.rs aun no tiene como `AccionAtajo`
//! (`AtajoExtra`) y el nombre de las teclas segun la distribucion del teclado del equipo (`nombre_combo`).

use crate::config::Combo;
use crate::textos::tx;
use std::collections::BTreeSet;
use std::sync::OnceLock;

/// Una operacion de entrada hacia la pantalla D-Bus de QEMU (interfaces org.qemu.Display1.*).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    /// Keyboard.Press / Release con numero de tecla de QEMU (qnum)
    Key(u32, bool),
    /// Mouse.SetAbsPosition (pixeles de la pantalla del invitado)
    Abs(u32, u32),
    /// Mouse.Press / Release (boton de QEMU: 0 = izquierdo)
    Btn(u32, bool),
    /// MultiTouch.SendEvent(tipo, ranura, x, y)
    Touch(MtKind, u64, f64, f64),
}

/// InputMultiTouchType de QEMU (qapi/ui.json): begin, update, end, cancel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MtKind {
    Begin = 0,
    Update = 1,
    End = 2,
    Cancel = 3,
}

// numeros de tecla de QEMU (qnum = codigo XT del conjunto 1, con 0x80 sumado a las teclas extendidas E0)
pub const QNUM_CTRL_IZQ: u32 = 0x1d;
pub const QNUM_CTRL_DER: u32 = 0x9d;
pub const QNUM_ALT_IZQ: u32 = 0x38;
pub const QNUM_ALT_DER: u32 = 0xb8;
pub const QNUM_META_IZQ: u32 = 0xdb;
pub const QNUM_TAB: u32 = 0x0f;
/// E0 6A (ac_back), E0 32 (ac_home), E0 2E / E0 30 (volumen)
pub const QNUM_ATRAS: u32 = 0xea;
pub const QNUM_INICIO: u32 = 0xb2;
pub const QNUM_VOL_BAJAR: u32 = 0xae;
pub const QNUM_VOL_SUBIR: u32 = 0xb0;
/// E0 5D: la tecla de menu de los teclados de PC (KEY_COMPOSE en Linux), que Android toma como MENU
pub const QNUM_MENU: u32 = 0xdd;
/// E0 5E: encendido ACPI (KEY_POWER): Android apaga o enciende la pantalla (despierta el aparato)
pub const QNUM_ENCENDIDO: u32 = 0xde;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Atajo {
    Atras,
    Inicio,
    Recientes,
    VolBajar,
    VolSubir,
    Rotar,
    Menu,
    /// encendido / despertar (KEY_POWER)
    Encendido,
}

/// Operaciones de un atajo de teclas (None para Rotar, que no es una tecla). Recientes: ningun codigo de tecla de
/// QEMU llega al invitado como KEY_APPSELECT (se probaron los 252 numeros de tecla); Android abre la vista de
/// tareas con Meta+Tab del teclado fisico (Alt+Tab solo la muestra mientras Alt esta pulsado).
pub fn teclas_de(a: Atajo) -> Option<Vec<Op>> {
    let toque = |q| vec![Op::Key(q, true), Op::Key(q, false)];
    Some(match a {
        Atajo::Atras => toque(QNUM_ATRAS),
        Atajo::Inicio => toque(QNUM_INICIO),
        Atajo::VolBajar => toque(QNUM_VOL_BAJAR),
        Atajo::VolSubir => toque(QNUM_VOL_SUBIR),
        Atajo::Recientes => vec![Op::Key(QNUM_META_IZQ, true), Op::Key(QNUM_TAB, true), Op::Key(QNUM_TAB, false), Op::Key(QNUM_META_IZQ, false)],
        Atajo::Menu => toque(QNUM_MENU),
        Atajo::Encendido => toque(QNUM_ENCENDIDO),
        Atajo::Rotar => return None,
    })
}

// ---------------------------------------------------------------------------------------------------------------
// atajos propios de la ventana y sus claves de configuracion

/// Atajos de la ventana que se guardan en `config` con su clave (config::CLAVES, como los demas ajustes) pero que no son
/// una `AccionAtajo` de config.rs: no los dispara `Config::atajo_para`, sino la ventana (ver `Ajustes::atajo_extra_para`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtajoExtra {
    /// la siguiente tecla (o combinacion) llega a Android aunque sea un atajo
    PasarTecla,
    PantallaCompleta,
    /// encendido / despertar de Android (KEY_POWER)
    Encendido,
    /// menu de Android
    Menu,
    /// lleva el teclado a la barra superior (sus pestanas se recorren y se activan con el teclado)
    Barra,
}

impl AtajoExtra {
    pub const TODOS: [AtajoExtra; 5] = [AtajoExtra::PasarTecla, AtajoExtra::PantallaCompleta, AtajoExtra::Encendido, AtajoExtra::Menu, AtajoExtra::Barra];

    pub fn clave(self) -> &'static str {
        match self {
            AtajoExtra::PasarTecla => "atajo.pasar_tecla",
            AtajoExtra::PantallaCompleta => "atajo.pantalla_completa",
            AtajoExtra::Encendido => "atajo.encendido",
            AtajoExtra::Menu => "atajo.menu",
            AtajoExtra::Barra => "atajo.barra",
        }
    }

    pub fn etiqueta(self) -> &'static str {
        match self {
            AtajoExtra::PasarTecla => tx!("atajo.pasar_tecla"),
            AtajoExtra::PantallaCompleta => tx!("atajo.pantalla_completa"),
            AtajoExtra::Encendido => tx!("atajo.encendido"),
            AtajoExtra::Menu => tx!("atajo.menu"),
            AtajoExtra::Barra => tx!("atajo.barra"),
        }
    }
}

/// Interruptor de la seccion Atajos: con `si` ninguna tecla es un atajo y todas llegan a Android.
pub const ATAJOS_DESACTIVADOS: &str = "atajos.desactivados";
/// Que hacen los botones derecho y central del raton con `--pointer multitouch` (ver `boton_tactil`; sus valores son
/// config::BOTONES_RATON).
pub const RATON_DERECHO: &str = "raton.derecho";
pub const RATON_CENTRAL: &str = "raton.central";

// ---------------------------------------------------------------------------------------------------------------
// nombres de las teclas segun la distribucion del teclado

type Nombres = Box<dyn Fn(u32) -> Option<String> + Send + Sync>;
static NOMBRES: OnceLock<Nombres> = OnceLock::new();

/// La ventana registra aqui (una vez, con SDL ya iniciado) como se llama cada tecla en la distribucion del teclado del
/// equipo: `sc` (codigo HID) -> nombre. Sin registro (pruebas, ordenes de consola) valen los nombres de config.rs.
pub fn registrar_nombres(f: Nombres) {
    let _ = NOMBRES.set(f);
}

/// ¿La tecla escribe un caracter (letras, numeros y signos)? Solo esas cambian de nombre con la distribucion: en un
/// teclado espanol la tecla del `;` es la Ñ y la del `=` es la ¡. Las demas (F1, flechas, RePag...) conservan su nombre.
fn escribe(sc: u32) -> bool {
    matches!(sc, 4..=39 | 45..=56 | 100)
}

/// Texto visible de una combinacion con el nombre de la tecla que da `nombre` (la distribucion real) para las teclas que
/// escriben; si no da nada (o algo raro), el de config.rs. El archivo sigue guardando el codigo de la tecla. Pura.
pub fn nombre_combo_con(c: &Combo, nombre: &dyn Fn(u32) -> Option<String>) -> String {
    let base = Combo { sc: c.sc, ctrl: false, alt: false, mayus: false }.visible();
    let tecla = if escribe(c.sc) {
        nombre(c.sc).map(|n| n.trim().to_string()).filter(|n| !n.is_empty() && n.chars().count() <= 3 && !n.chars().any(char::is_control)).map(|n| n.to_uppercase()).unwrap_or(base)
    } else {
        base
    };
    let mut s = String::new();
    if c.ctrl {
        s.push_str("Ctrl+");
    }
    if c.alt {
        s.push_str("Alt+");
    }
    if c.mayus {
        s.push_str("Mayús+");
    }
    s.push_str(&tecla);
    s
}

/// Texto visible de una combinacion con la distribucion del teclado registrada por la ventana.
pub fn nombre_combo(c: &Combo) -> String {
    match NOMBRES.get() {
        Some(f) => nombre_combo_con(c, f.as_ref()),
        None => c.visible(),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// teclado: atajos o invitado

/// ¿Es un modificador (Ctrl, Mayus, Alt, Super; izquierdo o derecho)? Codigos HID 224..=231.
pub fn es_modificador(sc: u32) -> bool {
    (224..=231).contains(&sc)
}

/// Modificadores apretados en el teclado real (un bit por codigo HID 224..=231), los haya reenviado la ventana al
/// invitado o no: el pellizco y Ctrl+rueda los consultan aqui, no en las teclas que tiene apretadas el invitado.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods(u8);

impl Mods {
    pub fn tecla(&mut self, sc: u32, abajo: bool) {
        if es_modificador(sc) {
            let bit = 1u8 << (sc - 224);
            if abajo {
                self.0 |= bit;
            } else {
                self.0 &= !bit;
            }
        }
    }
    pub fn ctrl(self) -> bool {
        self.0 & 0x11 != 0
    }
    pub fn mayus(self) -> bool {
        self.0 & 0x22 != 0
    }
    pub fn alt(self) -> bool {
        self.0 & 0x44 != 0
    }
    /// ¿Esta apretado el modificador del pellizco (`pellizco.modificador`: `ctrl` o `alt`)?
    pub fn pellizco(self, modificador: &str) -> bool {
        if modificador == "alt" {
            self.alt()
        } else {
            self.ctrl()
        }
    }
}

/// Teclas del modificador del pellizco (izquierda y derecha, numero de tecla de QEMU).
pub fn qnums_pellizco(modificador: &str) -> [u32; 2] {
    if modificador == "alt" {
        [QNUM_ALT_IZQ, QNUM_ALT_DER]
    } else {
        [QNUM_CTRL_IZQ, QNUM_CTRL_DER]
    }
}

/// Al empezar un pellizco: las teclas del modificador que el invitado tiene apretadas, que hay que soltarle (si no, Android
/// veria Ctrl o Alt apretado durante todo el gesto). Su soltado real ya no se reenvia: el invitado no las tiene.
pub fn soltar_para_pellizco(apretadas: &BTreeSet<u32>, modificador: &str) -> Vec<u32> {
    qnums_pellizco(modificador).into_iter().filter(|q| apretadas.contains(q)).collect()
}

/// Que hacer con una tecla (fuera de la pantalla de configuracion).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destino<A> {
    /// tecla de un atajo de la ventana: no llega al invitado. `Some` al pulsarla (no al repetir ni al soltar)
    Atajo(Option<A>),
    /// llega al invitado
    Invitado,
}

/// Decide si cada tecla es un atajo de la ventana (que se traga, pulsacion, repeticion y soltado) o va al invitado.
/// Recuerda las teclas tragadas para tragarse su soltado aunque entretanto cambien los modificadores o los atajos (sin
/// eso, soltar F1 con Ctrl aun apretado dejaba la tecla anotada y el soltado de la siguiente pulsacion simple no llegaba
/// al invitado: la tecla quedaba apretada en Android), y el modo "pasar la siguiente tecla".
#[derive(Debug, Default)]
pub struct FiltroAtajos {
    tragadas: BTreeSet<u32>,
    /// la siguiente tecla que no sea un modificador va al invitado aunque sea un atajo (lo arma el atajo `PasarTecla`)
    pub pasar: bool,
}

impl FiltroAtajos {
    /// `atajo`: el atajo que corresponde a la pulsacion con los modificadores de ahora (None si no hay); `activos`: los
    /// atajos estan activos (el interruptor "Atajos desactivados" los apaga todos).
    pub fn tecla<A>(&mut self, sc: u32, abajo: bool, repetida: bool, atajo: Option<A>, activos: bool) -> Destino<A> {
        if !abajo {
            // el soltado de una tecla tragada se traga siempre, cambien o no los modificadores, y la olvida
            return if self.tragadas.remove(&sc) { Destino::Atajo(None) } else { Destino::Invitado };
        }
        if repetida {
            // la repeticion sigue a la pulsacion (la del invitado la hace el propio invitado)
            return if self.tragadas.contains(&sc) { Destino::Atajo(None) } else { Destino::Invitado };
        }
        // una pulsacion nueva de una tecla que seguia anotada (se perdio su soltado: foco, pantalla de configuracion)
        // se decide de nuevo
        self.tragadas.remove(&sc);
        if es_modificador(sc) {
            return Destino::Invitado;
        }
        if self.pasar {
            self.pasar = false;
            return Destino::Invitado;
        }
        match atajo {
            Some(a) if activos => {
                self.tragadas.insert(sc);
                Destino::Atajo(Some(a))
            }
            _ => Destino::Invitado,
        }
    }

    /// La ventana perdio el foco: los soltados de lo apretado no llegaran.
    pub fn olvidar(&mut self) {
        self.tragadas.clear();
        self.pasar = false;
    }

    #[cfg(test)]
    fn tragadas(&self) -> Vec<u32> {
        self.tragadas.iter().copied().collect()
    }
}

// ---------------------------------------------------------------------------------------------------------------
// botones del raton y rueda con Ctrl

/// Atajo de un valor de `raton.derecho` / `raton.central` (None: `none` o algo desconocido).
pub fn atajo_de_boton(valor: &str) -> Option<Atajo> {
    match valor {
        "atras" => Some(Atajo::Atras),
        "inicio" => Some(Atajo::Inicio),
        "recientes" => Some(Atajo::Recientes),
        "menu" => Some(Atajo::Menu),
        _ => None,
    }
}

/// Con `--pointer multitouch` una pantalla tactil no tiene botones derecho ni central: pulsarlos dispara un atajo de
/// Android (por defecto derecho = Atras, central = Inicio). `b`: boton de QEMU (1 central, 2 derecho). None: nada (ni
/// llega al invitado).
pub fn boton_tactil(b: u32, derecho: &str, central: &str) -> Option<Atajo> {
    match b {
        2 => atajo_de_boton(derecho),
        1 => atajo_de_boton(central),
        _ => None,
    }
}

/// Ctrl+rueda = zoom de la ventana: acumula la rueda vertical (`dy` > 0 hacia arriba, que acerca) y devuelve un paso
/// (+1 acercar, -1 alejar, 0 nada) cada vez que se completa uno; las ruedas finas (paneles tactiles) dan fracciones.
pub fn paso_zoom(acum: &mut f64, dy: f64) -> i32 {
    if !dy.is_finite() {
        return 0;
    }
    // cambiar de sentido descarta lo acumulado en el otro
    if *acum * dy < 0.0 {
        *acum = 0.0;
    }
    *acum += dy;
    if *acum >= 1.0 {
        *acum = 0.0;
        1
    } else if *acum <= -1.0 {
        *acum = 0.0;
        -1
    } else {
        0
    }
}

/// Orientaciones del aparato (como Surface.ROTATION_*): 0 vertical, 1 apaisado (arriba a la izquierda), 2 vertical
/// invertido, 3 apaisado inverso. Acelerometro (m/s2) que las produce: la gravedad medida en los ejes del aparato.
pub fn acelerometro(rot: u32) -> (f64, f64, f64) {
    match rot % 4 {
        0 => (0.0, 9.81, 0.0),
        1 => (9.81, 0.0, 0.0),
        2 => (0.0, -9.81, 0.0),
        _ => (-9.81, 0.0, 0.0),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// raton -> un contacto de pantalla tactil

/// Con `--pointer multitouch` Android no tiene un puntero absoluto: lo unico que recibe es la pantalla tactil
/// (virtio-multitouch). Este traductor convierte las operaciones de raton de la ventana (posicion, boton izquierdo)
/// en un solo contacto en la ranura 0: pulsar = Begin, mover con el boton pulsado = Update, soltar = End. Sin boton
/// pulsado el movimiento no se envia (una pantalla tactil no tiene cursor flotante) y los demas botones se ignoran (la
/// ventana convierte antes el derecho y el central en atajos de Android: ver `boton_tactil`). Las coordenadas ya son las
/// del panel.
#[derive(Default, Debug)]
pub struct Tactil {
    activo: bool,
    pos: (f64, f64),
}

impl Tactil {
    pub fn traducir(&mut self, op: Op) -> Vec<Op> {
        match op {
            Op::Abs(x, y) => {
                self.pos = (x as f64, y as f64);
                if self.activo {
                    vec![Op::Touch(MtKind::Update, 0, self.pos.0, self.pos.1)]
                } else {
                    vec![]
                }
            }
            Op::Btn(0, true) if !self.activo => {
                self.activo = true;
                vec![Op::Touch(MtKind::Begin, 0, self.pos.0, self.pos.1)]
            }
            Op::Btn(0, false) if self.activo => {
                self.activo = false;
                vec![Op::Touch(MtKind::End, 0, self.pos.0, self.pos.1)]
            }
            Op::Btn(..) => vec![],
            otra => vec![otra],
        }
    }
}

/// Lleva una operacion de la VISTA (lo que muestra la ventana, girada `rot`) al panel del invitado. Solo cambian
/// las que llevan coordenadas.
pub fn mapear(op: Op, rot: u32, panel: (u32, u32)) -> Op {
    use crate::pantalla::a_panel;
    let acota = |v: f64, max: u32| v.round().clamp(0.0, max.saturating_sub(1) as f64);
    match op {
        Op::Abs(x, y) => {
            let (px, py) = a_panel(rot, panel, (x as f64, y as f64));
            Op::Abs(acota(px, panel.0) as u32, acota(py, panel.1) as u32)
        }
        Op::Touch(k, slot, x, y) => {
            let (px, py) = a_panel(rot, panel, (x, y));
            Op::Touch(k, slot, acota(px, panel.0), acota(py, panel.1))
        }
        otra => otra,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// rueda -> deslizamiento

/// Largo del deslizamiento por cada paso de rueda, como fraccion de la dimension de la pantalla.
pub const SWIPE_POR_PASO: f64 = 0.10;
/// Tope del deslizamiento: fraccion de la dimension.
pub const SWIPE_MAX: f64 = 0.60;
/// Numero de posiciones intermedias.
pub const SWIPE_PUNTOS: u32 = 8;

/// Deslizamiento con el lapiz que sustituye a `pasos` pasos de rueda en el punto `cursor` de una pantalla de
/// `size`. `pasos` > 0 = rueda hacia arriba (SDL: alejarse del usuario) = el dedo baja (se ve lo de arriba);
/// vertical por defecto; en horizontal `pasos` > 0 = rueda a la derecha = el dedo va a la izquierda.
/// Devuelve (milisegundos desde el inicio, operacion): posicion, boton izquierdo, SWIPE_PUNTOS posiciones
/// repartidas en 100-150 ms y suelta. El trazo se desplaza, sin cambiar su largo, hasta caber en la pantalla.
pub fn swipe(cursor: (i32, i32), horizontal: bool, pasos: f64, size: (u32, u32)) -> Vec<(u32, Op)> {
    swipe_con(cursor, horizontal, pasos, size, SWIPE_POR_PASO, SWIPE_MAX)
}

/// Como `swipe` con el largo por paso y el tope (fracciones de la pantalla) de la configuracion (`rueda.paso`, `rueda.tope`).
pub fn swipe_con(cursor: (i32, i32), horizontal: bool, pasos: f64, size: (u32, u32), por_paso: f64, tope: f64) -> Vec<(u32, Op)> {
    let (w, h) = (size.0.max(1) as i32, size.1.max(1) as i32);
    let dim = if horizontal { w } else { h } as f64;
    let largo = (pasos.abs() * por_paso * dim).min(tope * dim).round();
    let signo = if horizontal { -pasos.signum() } else { pasos.signum() };
    let desp = (signo * largo) as i32;
    let (c, max) = if horizontal { (cursor.0, w - 1) } else { (cursor.1, h - 1) };
    let c = c.clamp(0, max);
    let ini = if (c + desp) < 0 {
        -desp
    } else if c + desp > max {
        max - desp
    } else {
        c
    }
    .clamp(0, max);
    let dur = (100.0 + 10.0 * pasos.abs()).min(150.0) as u32;
    let punto = |along: i32| -> Op {
        let (x, y) = if horizontal { (along, cursor.1.clamp(0, h - 1)) } else { (cursor.0.clamp(0, w - 1), along) };
        Op::Abs(x as u32, y as u32)
    };
    let mut v = vec![(0, punto(ini)), (0, Op::Btn(0, true))];
    for i in 1..=SWIPE_PUNTOS {
        v.push((dur * i / SWIPE_PUNTOS, punto(ini + (desp as f64 * i as f64 / SWIPE_PUNTOS as f64).round() as i32)));
    }
    v.push((dur + 10, Op::Btn(0, false)));
    v
}

// ---------------------------------------------------------------------------------------------------------------
// Ctrl+clic -> pellizco

/// Separacion inicial de cada contacto respecto al ancla, como fraccion del ancho de la pantalla.
pub const PELLIZCO_V0: f64 = 0.08;

/// Posiciones de los dos contactos: el ancla fija el centro; el vector del contacto 0 es v0 (horizontal) mas lo
/// que se movio el cursor desde el clic; el contacto 1 es su simetrico. Acercar/alejar el cursor cambia la
/// separacion y moverlo de lado lo gira. Cada contacto se acota a la pantalla.
pub fn pellizco(ancla: (f64, f64), cursor: (f64, f64), size: (u32, u32)) -> [(f64, f64); 2] {
    let (w, h) = (size.0 as f64, size.1 as f64);
    let v = (PELLIZCO_V0 * w + cursor.0 - ancla.0, cursor.1 - ancla.1);
    let c = |x: f64, y: f64| (x.clamp(0.0, (w - 1.0).max(0.0)), y.clamp(0.0, (h - 1.0).max(0.0)));
    [c(ancla.0 + v.0, ancla.1 + v.1), c(ancla.0 - v.0, ancla.1 - v.1)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    fn abs(v: &[(u32, Op)]) -> Vec<(u32, u32)> {
        v.iter().filter_map(|(_, o)| if let Op::Abs(x, y) = o { Some((*x, *y)) } else { None }).collect()
    }

    #[test]
    fn atajos_y_teclas() {
        assert_eq!(teclas_de(Atajo::Atras).unwrap(), vec![Op::Key(0xea, true), Op::Key(0xea, false)]);
        assert_eq!(teclas_de(Atajo::VolSubir).unwrap()[0], Op::Key(0xb0, true));
        assert!(teclas_de(Atajo::Rotar).is_none());
        // Meta se suelta al final
        assert_eq!(teclas_de(Atajo::Recientes).unwrap().last(), Some(&Op::Key(QNUM_META_IZQ, false)));
    }

    #[test]
    fn acelerometro_por_orientacion() {
        assert_eq!(acelerometro(0), (0.0, 9.81, 0.0));
        assert_eq!(acelerometro(5), acelerometro(1));
        assert_eq!(acelerometro(3).0, -9.81);
    }

    #[test]
    fn deslizamiento_vertical() {
        let v = swipe((360, 700), false, 2.0, (720, 1348));
        // 2 pasos = 20 % de 1348 = 270 px, el dedo baja (rueda arriba)
        let p = abs(&v);
        assert_eq!(p.first(), Some(&(360, 700)));
        assert_eq!(p.last(), Some(&(360, 970)));
        assert_eq!(p.len() as u32, SWIPE_PUNTOS + 1);
        assert!(p.windows(2).all(|w| w[0].1 <= w[1].1 && w[0].0 == w[1].0));
        // orden: posicion, pulsar, movimientos, soltar; tiempos crecientes y dentro de 100-160 ms
        assert_eq!(v[1].1, Op::Btn(0, true));
        assert_eq!(v.last().unwrap().1, Op::Btn(0, false));
        assert!(v.windows(2).all(|w| w[0].0 <= w[1].0));
        let t = v.last().unwrap().0;
        assert!((110..=160).contains(&t), "{}", t);
        // rueda hacia abajo: sube
        let p = abs(&swipe((360, 700), false, -2.0, (720, 1348)));
        assert_eq!(p.last(), Some(&(360, 430)));
    }

    #[test]
    fn deslizamiento_acotado() {
        // tope 60 % y el trazo se desplaza para caber sin cambiar de largo
        let p = abs(&swipe((100, 1300), false, 50.0, (720, 1348)));
        let (a, b) = (p.first().unwrap().1, p.last().unwrap().1);
        assert_eq!(b as i32 - a as i32, (0.6 * 1348.0f64).round() as i32);
        assert!(p.iter().all(|&(x, y)| x == 100 && y < 1348));
        let p = abs(&swipe((100, 5), false, -50.0, (720, 1348)));
        assert_eq!(p.first().unwrap().1, 809);
        assert_eq!(p.last().unwrap().1, 0);
        assert!(p.iter().all(|&(_, y)| y < 1348));
        assert!(p.last().unwrap().1 < p.first().unwrap().1);
        // cursor fuera de la pantalla: se acota
        let p = abs(&swipe((9000, -4), true, 1.0, (720, 1348)));
        assert!(p.iter().all(|&(x, y)| x < 720 && y == 0));
    }

    #[test]
    fn deslizamiento_configurable() {
        // 20 % por paso y tope del 30 %: 2 pasos serian 40 %, queda en 30 % de 1348 = 404
        let p = abs(&swipe_con((360, 700), false, 2.0, (720, 1348), 0.20, 0.30));
        assert_eq!(p.last().unwrap().1 as i32 - p.first().unwrap().1 as i32, 404);
        // 5 % por paso: 2 pasos = 10 % de 1348 = 135
        let p = abs(&swipe_con((360, 700), false, 2.0, (720, 1348), 0.05, 0.60));
        assert_eq!(p.last().unwrap().1 as i32 - p.first().unwrap().1 as i32, 135);
        // por defecto es lo mismo que `swipe`
        assert_eq!(swipe((360, 700), false, 2.0, (720, 1348)), swipe_con((360, 700), false, 2.0, (720, 1348), SWIPE_POR_PASO, SWIPE_MAX));
    }

    #[test]
    fn deslizamiento_horizontal() {
        // rueda a la derecha (x > 0): el dedo va a la izquierda
        let p = abs(&swipe((500, 600), true, 1.0, (720, 1348)));
        assert_eq!(p.first(), Some(&(500, 600)));
        assert_eq!(p.last(), Some(&(428, 600)));
    }

    #[test]
    fn pellizco_geometria() {
        let [a, b] = pellizco((360.0, 600.0), (360.0, 600.0), (720, 1348));
        // sin mover el cursor: contactos separados 2 * 8 % del ancho, simetricos respecto al ancla
        assert!((a.0 - 360.0 - 57.6).abs() < 1e-9 && (b.0 - 360.0 + 57.6).abs() < 1e-9);
        assert_eq!((a.1, b.1), (600.0, 600.0));
        // alejar el cursor aumenta la separacion; moverlo en vertical gira
        let [a, b] = pellizco((360.0, 600.0), (460.0, 600.0), (720, 1348));
        assert!((a.0 - b.0 - 2.0 * 157.6).abs() < 1e-9);
        let [a, b] = pellizco((360.0, 600.0), (360.0, 700.0), (720, 1348));
        assert!((a.1 - 700.0).abs() < 1e-9 && (b.1 - 500.0).abs() < 1e-9);
        assert_eq!((a.0 + b.0) / 2.0, 360.0);
        // acotado a la pantalla
        let [a, b] = pellizco((10.0, 10.0), (-500.0, -500.0), (720, 1348));
        assert!(a.0 >= 0.0 && a.1 >= 0.0 && b.0 <= 719.0 && b.1 <= 1347.0);
    }

    #[test]
    fn raton_a_toque() {
        let mut t = Tactil::default();
        assert!(t.traducir(Op::Abs(10, 20)).is_empty()); // sin boton: no hay cursor flotante
        assert_eq!(t.traducir(Op::Btn(0, true)), vec![Op::Touch(MtKind::Begin, 0, 10.0, 20.0)]);
        assert_eq!(t.traducir(Op::Abs(30, 40)), vec![Op::Touch(MtKind::Update, 0, 30.0, 40.0)]);
        assert!(t.traducir(Op::Btn(2, true)).is_empty()); // otros botones se ignoran
        assert_eq!(t.traducir(Op::Btn(0, false)), vec![Op::Touch(MtKind::End, 0, 30.0, 40.0)]);
        assert!(t.traducir(Op::Btn(0, false)).is_empty()); // soltar sin pulsar
        assert!(t.traducir(Op::Abs(1, 1)).is_empty());
        assert_eq!(t.traducir(Op::Key(5, true)), vec![Op::Key(5, true)]); // el resto pasa tal cual
        // un deslizamiento de la rueda se vuelve un solo contacto: Begin, 8 Update, End
        let mut t = Tactil::default();
        let ops: Vec<Op> = swipe((360, 700), false, 2.0, (720, 1348)).into_iter().flat_map(|(_, o)| t.traducir(o)).collect();
        assert_eq!(ops.len() as u32, SWIPE_PUNTOS + 2);
        assert!(matches!(ops[0], Op::Touch(MtKind::Begin, 0, _, _)) && matches!(ops.last(), Some(Op::Touch(MtKind::End, 0, _, _))));
    }

    #[test]
    fn mapeo_a_panel() {
        // un toque en la esquina superior izquierda de la ventana girada (vista de 1348x720) llega a la esquina
        // superior derecha del panel
        assert_eq!(mapear(Op::Abs(0, 0), 1, (720, 1348)), Op::Abs(719, 0));
        assert_eq!(mapear(Op::Touch(MtKind::Begin, 1, 1347.0, 719.0), 1, (720, 1348)), Op::Touch(MtKind::Begin, 1, 0.0, 1347.0));
        assert_eq!(mapear(Op::Abs(7, 9), 0, (720, 1348)), Op::Abs(7, 9));
        assert_eq!(mapear(Op::Btn(0, true), 3, (720, 1348)), Op::Btn(0, true));
        // fuera de la vista se acota al panel
        assert_eq!(mapear(Op::Abs(5000, 5000), 3, (720, 1348)), Op::Abs(719, 0));
        // un deslizamiento hacia abajo en la vista apaisada es horizontal en el panel
        let ops = swipe((600, 300), false, 2.0, vista_de(1));
        let p: Vec<Op> = ops.iter().map(|(_, o)| mapear(*o, 1, (720, 1348))).collect();
        let abs: Vec<(u32, u32)> = p.iter().filter_map(|o| if let Op::Abs(x, y) = o { Some((*x, *y)) } else { None }).collect();
        assert!(abs.iter().all(|a| a.1 == abs[0].1) && abs.last().unwrap().0 < abs[0].0, "{:?}", abs);
    }

    fn vista_de(rot: u32) -> (u32, u32) {
        crate::pantalla::vista(rot, (720, 1348))
    }

    #[test]
    fn menu_y_encendido() {
        assert_eq!(teclas_de(Atajo::Menu).unwrap(), vec![Op::Key(QNUM_MENU, true), Op::Key(QNUM_MENU, false)]);
        assert_eq!(teclas_de(Atajo::Encendido).unwrap(), vec![Op::Key(0xde, true), Op::Key(0xde, false)]);
    }

    const F1: u32 = 58;
    const B: u32 = 5;
    const CTRL: u32 = 224;

    /// Soltar la tecla de un atajo con el modificador aun apretado (o con otro atajo ya) la olvida igual: la siguiente
    /// pulsacion simple de esa tecla llega entera al invitado (su soltado no se traga: no queda apretada en Android).
    #[test]
    fn el_soltado_de_un_atajo_no_deja_la_tecla_anotada() {
        let mut f = FiltroAtajos::default();
        // Ctrl+B es un atajo: Ctrl llega al invitado, B se traga
        assert_eq!(f.tecla(CTRL, true, false, None::<u8>, true), Destino::Invitado);
        assert_eq!(f.tecla(B, true, false, Some(1u8), true), Destino::Atajo(Some(1)));
        // la repeticion de B tambien se traga
        assert_eq!(f.tecla(B, true, true, None::<u8>, true), Destino::Atajo(None));
        // se suelta B con Ctrl aun apretado: se traga y se olvida
        assert_eq!(f.tecla(B, false, false, None::<u8>, true), Destino::Atajo(None));
        assert!(f.tragadas().is_empty());
        assert_eq!(f.tecla(CTRL, false, false, None::<u8>, true), Destino::Invitado);
        // B sola: pulsacion y soltado llegan al invitado
        assert_eq!(f.tecla(B, true, false, None::<u8>, true), Destino::Invitado);
        assert_eq!(f.tecla(B, false, false, None::<u8>, true), Destino::Invitado);
        // al reves: se suelta Ctrl primero y despues B (ya sin el atajo): tambien se traga y se olvida
        f.tecla(CTRL, true, false, None::<u8>, true);
        assert_eq!(f.tecla(B, true, false, Some(1u8), true), Destino::Atajo(Some(1)));
        f.tecla(CTRL, false, false, None::<u8>, true);
        assert_eq!(f.tecla(B, false, false, None::<u8>, true), Destino::Atajo(None));
        assert!(f.tragadas().is_empty());
    }

    /// Una tecla que ya iba al invitado no se vuelve atajo a medias: con B apretada, apretar Ctrl hace que las repeticiones
    /// de B coincidan con Ctrl+B, pero B sigue siendo del invitado (su soltado llega). Una pulsacion nueva de una tecla
    /// anotada (se perdio su soltado) se decide de nuevo.
    #[test]
    fn repeticiones_y_pulsaciones_perdidas() {
        let mut f = FiltroAtajos::default();
        assert_eq!(f.tecla(B, true, false, None::<u8>, true), Destino::Invitado);
        f.tecla(CTRL, true, false, None::<u8>, true);
        // la ventana no busca atajos en las repeticiones
        assert_eq!(f.tecla(B, true, true, None::<u8>, true), Destino::Invitado);
        assert_eq!(f.tecla(B, false, false, None::<u8>, true), Destino::Invitado);
        // F1 tragada cuyo soltado no llego (la pantalla de configuracion se lo quedo): la pulsacion siguiente sin atajo va
        // al invitado y su soltado tambien
        assert_eq!(f.tecla(F1, true, false, Some(2u8), true), Destino::Atajo(Some(2)));
        assert_eq!(f.tecla(F1, true, false, None::<u8>, true), Destino::Invitado);
        assert_eq!(f.tecla(F1, false, false, None::<u8>, true), Destino::Invitado);
        // perder el foco olvida lo tragado
        f.tecla(F1, true, false, Some(2u8), true);
        f.olvidar();
        assert!(f.tragadas().is_empty());
        assert_eq!(f.tecla(F1, false, false, None::<u8>, true), Destino::Invitado);
    }

    /// "Pasar la siguiente tecla": la tecla siguiente que no sea un modificador llega al invitado aunque sea un atajo (con
    /// su repeticion y su soltado); despues los atajos vuelven a funcionar.
    #[test]
    fn pasar_la_siguiente_tecla() {
        let mut f = FiltroAtajos { pasar: true, ..FiltroAtajos::default() };
        // los modificadores no la gastan (Ctrl para mandar Ctrl+B)
        assert_eq!(f.tecla(CTRL, true, false, None::<u8>, true), Destino::Invitado);
        assert!(f.pasar);
        assert_eq!(f.tecla(B, true, false, Some(1u8), true), Destino::Invitado);
        assert!(!f.pasar);
        assert_eq!(f.tecla(B, true, true, None::<u8>, true), Destino::Invitado);
        assert_eq!(f.tecla(B, false, false, None::<u8>, true), Destino::Invitado);
        f.tecla(CTRL, false, false, None::<u8>, true);
        // la siguiente vez Ctrl+B vuelve a ser el atajo
        f.tecla(CTRL, true, false, None::<u8>, true);
        assert_eq!(f.tecla(B, true, false, Some(1u8), true), Destino::Atajo(Some(1)));
        // perder el foco desarma el modo
        f.pasar = true;
        f.olvidar();
        assert!(!f.pasar);
    }

    /// Con los atajos desactivados todo llega al invitado; lo que ya se trago antes sigue tragandose hasta su soltado.
    #[test]
    fn atajos_desactivados() {
        let mut f = FiltroAtajos::default();
        assert_eq!(f.tecla(F1, true, false, Some(2u8), true), Destino::Atajo(Some(2)));
        assert_eq!(f.tecla(F1, false, false, None::<u8>, false), Destino::Atajo(None));
        assert_eq!(f.tecla(F1, true, false, Some(2u8), false), Destino::Invitado);
        assert_eq!(f.tecla(F1, true, true, None::<u8>, false), Destino::Invitado);
        assert_eq!(f.tecla(F1, false, false, None::<u8>, false), Destino::Invitado);
        assert!(f.tragadas().is_empty());
    }

    #[test]
    fn modificadores_del_teclado_real() {
        let mut m = Mods::default();
        assert!(!m.ctrl() && !m.alt() && !m.mayus());
        m.tecla(228, true); // Ctrl derecho
        m.tecla(4, true); // una letra no cuenta
        assert!(m.ctrl() && !m.alt() && m.pellizco("ctrl") && !m.pellizco("alt"));
        m.tecla(226, true); // Alt izquierdo
        m.tecla(229, true); // Mayus derecha
        assert!(m.alt() && m.mayus() && m.pellizco("alt"));
        m.tecla(228, false);
        assert!(!m.ctrl() && m.alt());
        m.tecla(226, false);
        m.tecla(229, false);
        assert_eq!(m, Mods::default());
        assert!(es_modificador(224) && es_modificador(231) && !es_modificador(232) && !es_modificador(58));
    }

    /// Al empezar el pellizco se sueltan en el invitado las teclas del modificador que tiene apretadas (y solo esas).
    #[test]
    fn el_pellizco_suelta_su_modificador() {
        let apretadas: BTreeSet<u32> = [QNUM_CTRL_IZQ, QNUM_ALT_DER, 0x1e].into_iter().collect();
        assert_eq!(soltar_para_pellizco(&apretadas, "ctrl"), vec![QNUM_CTRL_IZQ]);
        assert_eq!(soltar_para_pellizco(&apretadas, "alt"), vec![QNUM_ALT_DER]);
        assert!(soltar_para_pellizco(&BTreeSet::new(), "ctrl").is_empty());
        assert_eq!(qnums_pellizco("otra cosa"), [QNUM_CTRL_IZQ, QNUM_CTRL_DER]);
    }

    /// Con la pantalla tactil, boton derecho = Atras y central = Inicio por defecto; configurables y `none` = nada.
    #[test]
    fn botones_del_raton_en_la_pantalla_tactil() {
        let defecto = |k: &str| config::clave(k).unwrap().defecto;
        let validar = |k: &str, v: &str| config::Config::default().set(k, v);
        let (der, cen) = (defecto(RATON_DERECHO), defecto(RATON_CENTRAL));
        assert_eq!(boton_tactil(2, der, cen), Some(Atajo::Atras));
        assert_eq!(boton_tactil(1, der, cen), Some(Atajo::Inicio));
        assert_eq!(boton_tactil(0, der, cen), None);
        assert_eq!(boton_tactil(5, der, cen), None);
        assert_eq!(boton_tactil(2, "recientes", "menu"), Some(Atajo::Recientes));
        assert_eq!(boton_tactil(1, "recientes", "menu"), Some(Atajo::Menu));
        assert_eq!(boton_tactil(2, "none", "none"), None);
        for v in config::BOTONES_RATON {
            assert_eq!(validar(RATON_DERECHO, v).as_deref(), Ok(*v));
            assert_eq!(atajo_de_boton(v).is_none(), *v == "none");
        }
        assert_eq!(validar(RATON_CENTRAL, "ninguno").as_deref(), Ok("none"));
        assert!(validar(RATON_CENTRAL, "saltar").is_err());
    }

    /// Ctrl+rueda: un paso de zoom por paso de rueda; las ruedas finas suman fracciones y cambiar de sentido empieza de cero.
    #[test]
    fn ctrl_rueda_da_pasos_de_zoom() {
        let mut a = 0.0;
        assert_eq!(paso_zoom(&mut a, 1.0), 1);
        assert_eq!(paso_zoom(&mut a, -1.0), -1);
        assert_eq!(paso_zoom(&mut a, 3.0), 1);
        assert_eq!(a, 0.0);
        assert_eq!(paso_zoom(&mut a, 0.4), 0);
        assert_eq!(paso_zoom(&mut a, 0.4), 0);
        assert_eq!(paso_zoom(&mut a, 0.4), 1);
        assert_eq!(paso_zoom(&mut a, 0.6), 0);
        assert_eq!(paso_zoom(&mut a, -0.6), 0);
        assert_eq!(a, -0.6);
        assert_eq!(paso_zoom(&mut a, f64::NAN), 0);
        assert_eq!(paso_zoom(&mut a, -0.5), -1);
    }

    #[test]
    fn atajos_extra_y_claves_de_la_ventana() {
        // son claves de config.rs, con el mismo defecto que aqui
        let defecto = |k: &str| config::clave(k).map(|c| c.defecto);
        assert_eq!(defecto("atajo.pasar_tecla"), Some("Ctrl+Alt+F"));
        assert_eq!(defecto("atajo.pantalla_completa"), Some("F11"));
        assert_eq!(defecto("atajo.encendido"), Some("ninguno"));
        assert_eq!(defecto("atajo.menu"), Some("ninguno"));
        assert_eq!(defecto(ATAJOS_DESACTIVADOS), Some("no"));
        // los defectos son validos y no chocan con los de las acciones de config.rs
        let cfg = crate::config::Config::nueva();
        for e in AtajoExtra::TODOS {
            let d = defecto(e.clave()).unwrap();
            assert_eq!(config::clave(e.clave()).unwrap().tipo, config::Tipo::Tecla, "{:?}", e);
            assert!(d == "ninguno" || cfg.usa_la_tecla(d, None).is_none(), "{:?}", e);
        }
        let mut c = crate::config::Config::nueva();
        assert_eq!(c.set("atajo.menu", "ctrl+alt+m").as_deref(), Ok("Ctrl+Alt+M"));
        assert!(c.set("atajo.menu", "M").is_err());
        assert_eq!(c.set(ATAJOS_DESACTIVADOS, "1").as_deref(), Ok("si"));
    }

    /// El nombre visible de las teclas que escriben sigue la distribucion del teclado; las demas, y lo que no se pueda
    /// nombrar, conservan el de config.rs.
    #[test]
    fn nombres_segun_la_distribucion() {
        // un teclado espanol: la tecla del `;` es la Ñ y la del `=` es la ¡; la Q de un teclado frances es la A
        let es = |sc: u32| -> Option<String> {
            match sc {
                51 => Some("ñ".into()),
                46 => Some("¡".into()),
                20 => Some("A".into()),
                58 => Some("Tecla rara".into()),
                5 => Some("".into()),
                6 => Some("Keypad Enter".into()),
                _ => None,
            }
        };
        let c = |sc, ctrl, alt, mayus| Combo { sc, ctrl, alt, mayus };
        assert_eq!(nombre_combo_con(&c(51, true, false, false), &es), "Ctrl+Ñ");
        assert_eq!(nombre_combo_con(&c(46, true, false, false), &es), "Ctrl+¡");
        assert_eq!(nombre_combo_con(&c(20, true, true, true), &es), "Ctrl+Alt+Mayús+A");
        // F1 no escribe: su nombre no cambia
        assert_eq!(nombre_combo_con(&c(58, false, false, false), &es), "F1");
        // vacio, largo o sin nombre: el de la tabla
        assert_eq!(nombre_combo_con(&c(5, true, false, false), &es), "Ctrl+B");
        assert_eq!(nombre_combo_con(&c(6, true, false, false), &es), "Ctrl+C");
        assert_eq!(nombre_combo_con(&c(7, true, false, false), &es), "Ctrl+D");
        // sin distribucion registrada (pruebas): lo mismo que config.rs
        let r = c(75, true, false, true);
        assert_eq!(nombre_combo_con(&r, &|_| None), r.visible());
    }
}
