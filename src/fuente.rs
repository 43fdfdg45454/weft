//! Tipografia de la interfaz de la ventana propia: una fuente real del sistema, con tildes y enes.
//!
//! La fuente es la sans-serif de interfaz del escritorio (fontconfig: `sans-serif`; en KDE sobre Fedora, Noto Sans), en
//! regular y en semibold para los titulos, rasterizada con FreeType a la medida FISICA (puntos x factor de escala de la
//! ventana): nunca se escala un mapa de bits. Las dos bibliotecas se cargan en tiempo de ejecucion (`dlopen`), como SDL3:
//! weft sigue sin dependencias de compilacion. Si faltan (o no hay ninguna fuente con tildes) queda como ultimo
//! recurso la fuente de mapa de bits 5x7 de antes, con un aviso en el registro de la ventana.
//!
//! Este modulo no conoce SDL: entrega glifos (cobertura de 8 bits con su posicion y avance), mide texto en dp (puntos)
//! para que la interfaz alinee y trunque con el ancho REAL, y reparte los glifos en un atlas (`Atlas`) cuya textura sube
//! `window.rs`. Los glifos se cachean por (estilo, tamano fisico, caracter).

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{c_char, c_long, c_ulong, c_void, CStr, CString};
use std::rc::Rc;
use crate::textos::{tx, txf};

/// Estilos de texto. Tamanos en dp (puntos de la ventana; a escala 1 = pixeles).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Estilo {
    /// 12 dp: etiquetas, notas y textos secundarios
    Pequeno,
    /// 13 dp: cuerpo
    Cuerpo,
    /// 13 dp semibold: etiquetas de controles y chips
    Negrita,
    /// 16 dp semibold: titulos
    Titulo,
    /// 26 dp: cifras grandes (porcentaje de zoom)
    Grande,
}

impl Estilo {
    pub fn dp(self) -> f32 {
        match self {
            Estilo::Pequeno => 12.0,
            Estilo::Cuerpo | Estilo::Negrita => 13.0,
            Estilo::Titulo => 16.0,
            Estilo::Grande => 26.0,
        }
    }
    pub fn semibold(self) -> bool {
        matches!(self, Estilo::Negrita | Estilo::Titulo)
    }
    /// Tamano fisico en pixeles para el factor de escala dado.
    pub fn px(self, escala: f32) -> u32 {
        (self.dp() * escala).round().max(6.0) as u32
    }
    /// Escala entera de la fuente de mapa de bits (solo el respaldo).
    fn k_respaldo(self) -> u32 {
        match self {
            Estilo::Pequeno => 1,
            Estilo::Cuerpo | Estilo::Negrita | Estilo::Titulo => 2,
            Estilo::Grande => 4,
        }
    }
}

/// Un glifo rasterizado: cobertura de 8 bits fila a fila (`w` x `h`), posicion respecto al punto de la linea base
/// (`left` hacia la derecha, `top` hacia arriba) y avance del cursor, todo en pixeles fisicos.
#[derive(Clone, Debug, PartialEq)]
pub struct Glifo {
    pub w: i32,
    pub h: i32,
    pub left: i32,
    pub top: i32,
    pub avance: f32,
    pub alfa: Vec<u8>,
}

/// Metricas verticales de un estilo, en pixeles fisicos: ascenso y descenso (positivo) desde la linea base, y altura
/// de las mayusculas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metricas {
    pub asc: i32,
    pub desc: i32,
    pub cap: i32,
}

/// Lo que necesita la interfaz para alinear y truncar texto, en dp.
pub trait Medida {
    fn ancho(&self, t: &str, e: Estilo) -> f32;
    /// Alto de la caja de una linea.
    fn alto_linea(&self, e: Estilo) -> f32;
    /// Distancia de la linea base al borde superior de la caja.
    fn ascenso(&self, e: Estilo) -> f32;
    /// Altura de las mayusculas.
    fn cap(&self, e: Estilo) -> f32;
}

/// Acorta `t` con "..." para que quepa en `max` dp (el texto original si ya cabe).
pub fn truncar(m: &dyn Medida, t: &str, e: Estilo, max: f32) -> String {
    if m.ancho(t, e) <= max {
        return t.to_string();
    }
    let cs: Vec<char> = t.chars().collect();
    for n in (0..cs.len()).rev() {
        let s: String = cs[..n].iter().collect::<String>().trim_end().to_string() + "...";
        if m.ancho(&s, e) <= max {
            return s;
        }
    }
    String::new()
}

/// Acorta `t` quitando su parte central (con "...") para que quepa en `max` dp: se ven el principio y el final, lo que
/// importa de una frase que acaba en una ruta (el nombre del archivo). El original si ya cabe; si ni el final cabe, el
/// recorte de `truncar`.
pub fn truncar_centro(m: &dyn Medida, t: &str, e: Estilo, max: f32) -> String {
    if m.ancho(t, e) <= max {
        return t.to_string();
    }
    let cs: Vec<char> = t.chars().collect();
    // se quita un tramo cada vez mas largo; queda un tercio delante y dos detras (el final suele ser lo importante)
    for quitar in 1..cs.len() {
        let ini = (cs.len() - quitar) / 3;
        let s: String = cs[..ini].iter().collect::<String>() + "..." + &cs[ini + quitar..].iter().collect::<String>();
        if m.ancho(&s, e) <= max {
            return s;
        }
    }
    truncar(m, t, e, max)
}

/// Parte `t` en lineas de a lo sumo `max` dp (por palabras; una palabra mas larga que la linea se corta). Con `max_lineas`
/// la ultima se trunca con "...". Sin ancho (`max` cero, negativo o NaN: un dialogo mas estrecho que sus margenes) no
/// cabe nada, asi que se devuelve el texto entero en una linea (o ninguna si esta vacio) en vez de cortarlo letra a letra.
pub fn envolver(m: &dyn Medida, t: &str, e: Estilo, max: f32, max_lineas: usize) -> Vec<String> {
    let mut lineas: Vec<String> = Vec::new();
    let mut actual = String::new();
    if max.is_nan() || max <= 0.0 {
        let entero: Vec<&str> = t.split_whitespace().collect();
        if !entero.is_empty() {
            lineas.push(entero.join(" "));
        }
        return lineas;
    }
    for palabra in t.split_whitespace() {
        let prueba = if actual.is_empty() { palabra.to_string() } else { format!("{} {}", actual, palabra) };
        if m.ancho(&prueba, e) <= max {
            actual = prueba;
            continue;
        }
        if !actual.is_empty() {
            lineas.push(std::mem::take(&mut actual));
        }
        // palabra mas larga que una linea: se corta por caracteres (al menos uno por linea, y nunca sobre lo vacio)
        let mut resto = palabra.to_string();
        while !resto.is_empty() && m.ancho(&resto, e) > max {
            let cs: Vec<char> = resto.chars().collect();
            let mut n = cs.len().saturating_sub(1).max(1);
            while n > 1 && m.ancho(&cs[..n].iter().collect::<String>(), e) > max {
                n -= 1;
            }
            lineas.push(cs[..n].iter().collect());
            resto = cs[n..].iter().collect();
        }
        actual = resto;
    }
    if !actual.is_empty() {
        lineas.push(actual);
    }
    if max_lineas > 0 && lineas.len() > max_lineas {
        lineas.truncate(max_lineas);
        let ultima = lineas.pop().unwrap();
        lineas.push(truncar(m, &format!("{}...", ultima.trim_end_matches('.')), e, max));
    }
    lineas
}

// ---------------------------------------------------------------------------------------------------------------
// fuente de mapa de bits 5x7 (ultimo recurso)

/// Fuente 5x7 incrustada (ASCII imprimible, 0x20..=0x7E, mas los signos del espanol que no tienen letra base: los de
/// apertura de pregunta y exclamacion, el grado, las comillas angulares, el punto medio y la ene). Cada glifo son 5
/// columnas; el bit 0 de cada columna es la fila de arriba. Sin vocales con tilde: `base` reduce las que lleguen (a, e, i,
/// o, u, u dieresis) a su letra. Era la unica fuente del panel; queda como respaldo cuando FreeType o fontconfig no estan.
/// Glifos ASCII: la fuente 5x7 clasica de los ejemplos de pantallas LCD (glcdfont de Adafruit-GFX, licencia BSD de
/// Adafruit Industries); solo datos de glifos, sin codigo. Los otros ocho son propios.
pub mod mapa5x7 {
    pub const PRIMERO: u32 = 0x20;
    pub const ULTIMO: u32 = 0x7e;
    /// Ancho de la celda de un caracter (5 columnas + 1 de separacion) y alto del glifo, en pixeles de la fuente.
    pub const CELDA: usize = 6;
    pub const ALTO: usize = 7;

    #[rustfmt::skip]
    static GLIFOS: [[u8; 5]; 95] = [
        [0x00, 0x00, 0x00, 0x00, 0x00], // ' '
        [0x00, 0x00, 0x5f, 0x00, 0x00], // !
        [0x00, 0x07, 0x00, 0x07, 0x00], // "
        [0x14, 0x7f, 0x14, 0x7f, 0x14], // #
        [0x24, 0x2a, 0x7f, 0x2a, 0x12], // $
        [0x23, 0x13, 0x08, 0x64, 0x62], // %
        [0x36, 0x49, 0x55, 0x22, 0x50], // &
        [0x00, 0x05, 0x03, 0x00, 0x00], // '
        [0x00, 0x1c, 0x22, 0x41, 0x00], // (
        [0x00, 0x41, 0x22, 0x1c, 0x00], // )
        [0x14, 0x08, 0x3e, 0x08, 0x14], // *
        [0x08, 0x08, 0x3e, 0x08, 0x08], // +
        [0x00, 0x50, 0x30, 0x00, 0x00], // ,
        [0x08, 0x08, 0x08, 0x08, 0x08], // -
        [0x00, 0x60, 0x60, 0x00, 0x00], // .
        [0x20, 0x10, 0x08, 0x04, 0x02], // /
        [0x3e, 0x51, 0x49, 0x45, 0x3e], // 0
        [0x00, 0x42, 0x7f, 0x40, 0x00], // 1
        [0x42, 0x61, 0x51, 0x49, 0x46], // 2
        [0x21, 0x41, 0x45, 0x4b, 0x31], // 3
        [0x18, 0x14, 0x12, 0x7f, 0x10], // 4
        [0x27, 0x45, 0x45, 0x45, 0x39], // 5
        [0x3c, 0x4a, 0x49, 0x49, 0x30], // 6
        [0x01, 0x71, 0x09, 0x05, 0x03], // 7
        [0x36, 0x49, 0x49, 0x49, 0x36], // 8
        [0x06, 0x49, 0x49, 0x29, 0x1e], // 9
        [0x00, 0x36, 0x36, 0x00, 0x00], // :
        [0x00, 0x56, 0x36, 0x00, 0x00], // ;
        [0x08, 0x14, 0x22, 0x41, 0x00], // <
        [0x14, 0x14, 0x14, 0x14, 0x14], // =
        [0x00, 0x41, 0x22, 0x14, 0x08], // >
        [0x02, 0x01, 0x51, 0x09, 0x06], // ?
        [0x32, 0x49, 0x79, 0x41, 0x3e], // @
        [0x7e, 0x11, 0x11, 0x11, 0x7e], // A
        [0x7f, 0x49, 0x49, 0x49, 0x36], // B
        [0x3e, 0x41, 0x41, 0x41, 0x22], // C
        [0x7f, 0x41, 0x41, 0x22, 0x1c], // D
        [0x7f, 0x49, 0x49, 0x49, 0x41], // E
        [0x7f, 0x09, 0x09, 0x09, 0x01], // F
        [0x3e, 0x41, 0x49, 0x49, 0x7a], // G
        [0x7f, 0x08, 0x08, 0x08, 0x7f], // H
        [0x00, 0x41, 0x7f, 0x41, 0x00], // I
        [0x20, 0x40, 0x41, 0x3f, 0x01], // J
        [0x7f, 0x08, 0x14, 0x22, 0x41], // K
        [0x7f, 0x40, 0x40, 0x40, 0x40], // L
        [0x7f, 0x02, 0x0c, 0x02, 0x7f], // M
        [0x7f, 0x04, 0x08, 0x10, 0x7f], // N
        [0x3e, 0x41, 0x41, 0x41, 0x3e], // O
        [0x7f, 0x09, 0x09, 0x09, 0x06], // P
        [0x3e, 0x41, 0x51, 0x21, 0x5e], // Q
        [0x7f, 0x09, 0x19, 0x29, 0x46], // R
        [0x46, 0x49, 0x49, 0x49, 0x31], // S
        [0x01, 0x01, 0x7f, 0x01, 0x01], // T
        [0x3f, 0x40, 0x40, 0x40, 0x3f], // U
        [0x1f, 0x20, 0x40, 0x20, 0x1f], // V
        [0x3f, 0x40, 0x38, 0x40, 0x3f], // W
        [0x63, 0x14, 0x08, 0x14, 0x63], // X
        [0x07, 0x08, 0x70, 0x08, 0x07], // Y
        [0x61, 0x51, 0x49, 0x45, 0x43], // Z
        [0x00, 0x7f, 0x41, 0x41, 0x00], // [
        [0x02, 0x04, 0x08, 0x10, 0x20], // \
        [0x00, 0x41, 0x41, 0x7f, 0x00], // ]
        [0x04, 0x02, 0x01, 0x02, 0x04], // ^
        [0x40, 0x40, 0x40, 0x40, 0x40], // _
        [0x00, 0x01, 0x02, 0x04, 0x00], // `
        [0x20, 0x54, 0x54, 0x54, 0x78], // a
        [0x7f, 0x48, 0x44, 0x44, 0x38], // b
        [0x38, 0x44, 0x44, 0x44, 0x20], // c
        [0x38, 0x44, 0x44, 0x48, 0x7f], // d
        [0x38, 0x54, 0x54, 0x54, 0x18], // e
        [0x08, 0x7e, 0x09, 0x01, 0x02], // f
        [0x0c, 0x52, 0x52, 0x52, 0x7e], // g
        [0x7f, 0x08, 0x04, 0x04, 0x78], // h
        [0x00, 0x44, 0x7d, 0x40, 0x00], // i
        [0x20, 0x40, 0x44, 0x3d, 0x00], // j
        [0x00, 0x7f, 0x10, 0x28, 0x44], // k
        [0x00, 0x41, 0x7f, 0x40, 0x00], // l
        [0x7c, 0x04, 0x18, 0x04, 0x78], // m
        [0x7c, 0x08, 0x04, 0x04, 0x78], // n
        [0x38, 0x44, 0x44, 0x44, 0x38], // o
        [0x7c, 0x14, 0x14, 0x14, 0x08], // p
        [0x0c, 0x12, 0x12, 0x12, 0x7e], // q
        [0x7c, 0x08, 0x04, 0x04, 0x08], // r
        [0x48, 0x54, 0x54, 0x54, 0x20], // s
        [0x04, 0x3f, 0x44, 0x40, 0x20], // t
        [0x3c, 0x40, 0x40, 0x20, 0x7c], // u
        [0x1c, 0x20, 0x40, 0x20, 0x1c], // v
        [0x3c, 0x40, 0x30, 0x40, 0x3c], // w
        [0x44, 0x28, 0x10, 0x28, 0x44], // x
        [0x0c, 0x50, 0x50, 0x50, 0x3c], // y
        [0x44, 0x64, 0x54, 0x4c, 0x44], // z
        [0x00, 0x08, 0x36, 0x41, 0x00], // {
        [0x00, 0x00, 0x7f, 0x00, 0x00], // |
        [0x00, 0x41, 0x36, 0x08, 0x00], // }
        [0x10, 0x08, 0x08, 0x10, 0x08], // ~
    ];

    /// Signos del espanol fuera de ASCII que no se pueden reducir a otra letra (dibujo propio, mismo formato).
    #[rustfmt::skip]
    pub const EXTRA: [(char, [u8; 5]); 8] = [
        ('¿', [0x30, 0x48, 0x45, 0x40, 0x20]), // '?' girado
        ('¡', [0x00, 0x00, 0x7d, 0x00, 0x00]), // '!' girado
        ('°', [0x06, 0x09, 0x09, 0x06, 0x00]), // anillo arriba
        ('«', [0x08, 0x14, 0x2a, 0x14, 0x22]), // doble angulo hacia la izquierda
        ('»', [0x22, 0x14, 0x2a, 0x14, 0x08]),
        ('·', [0x00, 0x18, 0x18, 0x00, 0x00]), // punto a media altura
        ('ñ', [0x7c, 0x0a, 0x05, 0x06, 0x79]), // 'n' con la tilde encima
        ('Ñ', [0x7c, 0x0a, 0x11, 0x22, 0x7d]), // 'N' de 5 filas con la tilde encima
    ];

    /// Letra o signo base de un caracter sin glifo propio (vocales con tilde o dieresis, puntos suspensivos, signo de
    /// multiplicar, rayas y comillas tipograficas).
    pub fn base(c: char) -> char {
        match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'Á' | 'À' | 'Ä' | 'Â' => 'A',
            'É' | 'È' | 'Ë' | 'Ê' => 'E',
            'Í' | 'Ì' | 'Ï' | 'Î' => 'I',
            'Ó' | 'Ò' | 'Ö' | 'Ô' => 'O',
            'Ú' | 'Ù' | 'Ü' | 'Û' => 'U',
            '…' => '.',
            '×' => 'x',
            '–' | '—' => '-',
            '‘' | '’' => '\'',
            '“' | '”' => '"',
            c => c,
        }
    }

    /// Glifo de un caracter ASCII imprimible o de uno de `EXTRA` (None para lo demas).
    pub fn glifo(c: char) -> Option<[u8; 5]> {
        let n = c as u32;
        if (PRIMERO..=ULTIMO).contains(&n) {
            Some(GLIFOS[(n - PRIMERO) as usize])
        } else {
            EXTRA.iter().find(|(e, _)| *e == c).map(|(_, g)| *g)
        }
    }

    /// ¿La fuente dibuja `c` (con glifo propio o el de su letra base) sin caer en el '?' de relleno?
    #[cfg(test)]
    pub fn cubre(c: char) -> bool {
        glifo(base(c)).is_some()
    }
}

// ---------------------------------------------------------------------------------------------------------------
// FreeType y fontconfig por dlopen

extern "C" {
    fn dlopen(name: *const c_char, flags: i32) -> *mut c_void;
    fn dlsym(h: *mut c_void, name: *const c_char) -> *mut c_void;
}

type P = *mut c_void;

fn abrir(nombres: &[&str]) -> Option<P> {
    nombres.iter().map(|n| unsafe { dlopen(CString::new(*n).unwrap().as_ptr(), 2 /* RTLD_NOW */) }).find(|h| !h.is_null())
}

fn simbolo<T: Copy>(h: P, nombre: &str) -> Option<T> {
    let p = unsafe { dlsym(h, CString::new(nombre).unwrap().as_ptr()) };
    if p.is_null() {
        None
    } else {
        Some(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&p) })
    }
}

const FT_LOAD_NO_BITMAP: i32 = 1 << 3;
const FT_LOAD_RENDER: i32 = 1 << 2;
/// FT_LOAD_TARGET_LIGHT: autoajuste solo vertical, sin deformar el ancho de las letras
const FT_LOAD_TARGET_LIGHT: i32 = 1 << 16;

// Desplazamientos de FT_FaceRec / FT_GlyphSlotRec / FT_Bitmap en 64 bits (ABI estable de FreeType 2.x; las pruebas
// rasterizan una 'A' y comprueban que los valores tienen sentido).
const FACE_UPEM: usize = 136; // FT_UShort units_per_EM
const FACE_ASC: usize = 138; // FT_Short ascender
const FACE_DESC: usize = 140; // FT_Short descender
const FACE_GLYPH: usize = 152; // FT_GlyphSlot glyph
const SLOT_ADV_X: usize = 128; // FT_Vector advance (26.6)
const SLOT_ROWS: usize = 152; // FT_Bitmap: rows (u32), width (u32), pitch (i32), buffer (ptr), pixel_mode (u8 en +26)
const SLOT_WIDTH: usize = 156;
const SLOT_PITCH: usize = 160;
const SLOT_BUFFER: usize = 168;
const SLOT_MODE: usize = 178;
const SLOT_LEFT: usize = 192; // FT_Int bitmap_left
const SLOT_TOP: usize = 196; // FT_Int bitmap_top

unsafe fn leer<T: Copy>(p: P, off: usize) -> T {
    ((p as *const u8).add(off) as *const T).read_unaligned()
}

struct FtFns {
    init: unsafe extern "C" fn(*mut P) -> i32,
    new_face: unsafe extern "C" fn(P, *const c_char, c_long, *mut P) -> i32,
    set_px: unsafe extern "C" fn(P, u32, u32) -> i32,
    load_char: unsafe extern "C" fn(P, c_ulong, i32) -> i32,
    char_index: unsafe extern "C" fn(P, c_ulong) -> u32,
}

struct Cara {
    ptr: P,
    /// tamano fisico fijado (0 = ninguno)
    px: u32,
    ruta: String,
}

struct Ft {
    f: FtFns,
    regular: Cara,
    semibold: Cara,
}

/// Rutas de las fuentes habituales por si fontconfig no esta o no contesta.
const RUTAS_CONOCIDAS: &[(&str, &str)] = &[
    ("/usr/share/fonts/google-noto/NotoSans-Regular.ttf", "/usr/share/fonts/google-noto/NotoSans-SemiBold.ttf"),
    ("/usr/share/fonts/noto/NotoSans-Regular.ttf", "/usr/share/fonts/noto/NotoSans-SemiBold.ttf"),
    ("/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/noto/NotoSans-SemiBold.ttf"),
    ("/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf", "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans-Bold.ttf"),
    ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"),
    ("/usr/share/fonts/liberation-sans/LiberationSans-Regular.ttf", "/usr/share/fonts/liberation-sans/LiberationSans-Bold.ttf"),
    ("/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf", "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf"),
];

/// Pregunta a fontconfig por una fuente (`sans-serif`, `sans-serif:semibold`...): ruta del archivo e indice de la cara.
fn fontconfig(patron: &str) -> Option<(String, i32)> {
    let h = abrir(&["libfontconfig.so.1", "libfontconfig.so"])?;
    type Parse = unsafe extern "C" fn(*const u8) -> P;
    type Subst = unsafe extern "C" fn(P, P, i32) -> i32;
    type Defecto = unsafe extern "C" fn(P);
    type Match = unsafe extern "C" fn(P, P, *mut i32) -> P;
    type GetStr = unsafe extern "C" fn(P, *const c_char, i32, *mut *const u8) -> i32;
    type GetInt = unsafe extern "C" fn(P, *const c_char, i32, *mut i32) -> i32;
    type Destruir = unsafe extern "C" fn(P);
    let parse: Parse = simbolo(h, "FcNameParse")?;
    let subst: Subst = simbolo(h, "FcConfigSubstitute")?;
    let defecto: Defecto = simbolo(h, "FcDefaultSubstitute")?;
    let buscar: Match = simbolo(h, "FcFontMatch")?;
    let get_str: GetStr = simbolo(h, "FcPatternGetString")?;
    let get_int: GetInt = simbolo(h, "FcPatternGetInteger")?;
    let destruir: Destruir = simbolo(h, "FcPatternDestroy")?;
    let nombre = CString::new(patron).ok()?;
    unsafe {
        let pat = parse(nombre.as_ptr() as *const u8);
        if pat.is_null() {
            return None;
        }
        subst(std::ptr::null_mut(), pat, 0 /* FcMatchPattern */);
        defecto(pat);
        let mut res = 0i32;
        let m = buscar(std::ptr::null_mut(), pat, &mut res);
        destruir(pat);
        if m.is_null() {
            return None;
        }
        let (mut archivo, mut indice) = (std::ptr::null::<u8>(), 0i32);
        let ok = get_str(m, c"file".as_ptr(), 0, &mut archivo) == 0 && !archivo.is_null();
        let ruta = if ok { Some(CStr::from_ptr(archivo as *const c_char).to_string_lossy().into_owned()) } else { None };
        if get_int(m, c"index".as_ptr(), 0, &mut indice) != 0 {
            indice = 0;
        }
        destruir(m);
        ruta.map(|r| (r, indice))
    }
}

impl Ft {
    /// Carga FreeType y las dos caras. Err con el motivo si no se puede (el llamador usa el respaldo 5x7).
    fn cargar() -> Result<Ft, String> {
        let h = abrir(&["libfreetype.so.6", "libfreetype.so"]).ok_or(tx!("fuente.no_se_carga_libfreetype_so_6"))?;
        let f = FtFns {
            init: simbolo(h, "FT_Init_FreeType").ok_or(tx!("fuente.freetype_sin_ft_init_freetype"))?,
            new_face: simbolo(h, "FT_New_Face").ok_or(tx!("fuente.freetype_sin_ft_new_face"))?,
            set_px: simbolo(h, "FT_Set_Pixel_Sizes").ok_or(tx!("fuente.freetype_sin_ft_set_pixel_sizes"))?,
            load_char: simbolo(h, "FT_Load_Char").ok_or(tx!("fuente.freetype_sin_ft_load_char"))?,
            char_index: simbolo(h, "FT_Get_Char_Index").ok_or(tx!("fuente.freetype_sin_ft_get_char_index"))?,
        };
        let mut lib: P = std::ptr::null_mut();
        if unsafe { (f.init)(&mut lib) } != 0 {
            return Err(tx!("fuente.ft_init_freetype_fallo").into());
        }
        // candidatos en orden: lo que dice fontconfig y despues las rutas conocidas
        let mut candidatos: Vec<(String, i32, String, i32)> = Vec::new();
        if let (Some(r), s) = (fontconfig("sans-serif:lang=es"), fontconfig("sans-serif:lang=es:semibold")) {
            let s = s.unwrap_or_else(|| r.clone());
            candidatos.push((r.0, r.1, s.0, s.1));
        }
        for (r, s) in RUTAS_CONOCIDAS {
            if std::path::Path::new(r).exists() {
                let s = if std::path::Path::new(s).exists() { s } else { r };
                candidatos.push((r.to_string(), 0, s.to_string(), 0));
            }
        }
        let abrir_cara = |ruta: &str, indice: i32| -> Option<P> {
            let c = CString::new(ruta).ok()?;
            let mut cara: P = std::ptr::null_mut();
            if unsafe { (f.new_face)(lib, c.as_ptr(), indice as c_long, &mut cara) } != 0 || cara.is_null() {
                return None;
            }
            // debe tener las letras del espanol; si no, no sirve para esta interfaz
            let tiene = |ch: char| unsafe { (f.char_index)(cara, ch as c_ulong) } != 0;
            if "aeAEñÑáéíóúü".chars().all(tiene) {
                Some(cara)
            } else {
                None
            }
        };
        for (r, ri, s, si) in candidatos {
            let Some(reg) = abrir_cara(&r, ri) else { continue };
            let (semi, ruta_s) = match abrir_cara(&s, si) {
                Some(c) => (c, s),
                None => (reg, r.clone()),
            };
            return Ok(Ft { f, regular: Cara { ptr: reg, px: 0, ruta: r }, semibold: Cara { ptr: semi, px: 0, ruta: ruta_s } });
        }
        Err(tx!("fuente.no_hay_ninguna_fuente_sans_serif_con").into())
    }

    fn cara(&mut self, semibold: bool) -> &mut Cara {
        if semibold {
            &mut self.semibold
        } else {
            &mut self.regular
        }
    }

    fn fijar(&mut self, semibold: bool, px: u32) {
        let set = self.f.set_px;
        let c = self.cara(semibold);
        if c.px != px {
            unsafe { set(c.ptr, 0, px) };
            c.px = px;
        }
    }

    fn rasterizar(&mut self, e: Estilo, px: u32, ch: char) -> Option<Glifo> {
        self.fijar(e.semibold(), px);
        let (load, indice) = (self.f.load_char, self.f.char_index);
        let c = self.cara(e.semibold());
        unsafe {
            if ch != ' ' && indice(c.ptr, ch as c_ulong) == 0 {
                return None;
            }
            if load(c.ptr, ch as c_ulong, FT_LOAD_RENDER | FT_LOAD_TARGET_LIGHT | FT_LOAD_NO_BITMAP) != 0 {
                return None;
            }
            let slot: P = leer(c.ptr, FACE_GLYPH);
            if slot.is_null() {
                return None;
            }
            let (rows, width): (u32, u32) = (leer(slot, SLOT_ROWS), leer(slot, SLOT_WIDTH));
            let pitch: i32 = leer(slot, SLOT_PITCH);
            let buf: *const u8 = leer(slot, SLOT_BUFFER);
            let modo: u8 = leer(slot, SLOT_MODE);
            let avance: c_long = leer(slot, SLOT_ADV_X);
            let (left, top): (i32, i32) = (leer(slot, SLOT_LEFT), leer(slot, SLOT_TOP));
            let (w, h) = (width as usize, rows as usize);
            // sanidad: un glifo no pasa de varias veces el tamano pedido
            if w > 4 * px as usize + 8 || h > 4 * px as usize + 8 || (w * h > 0 && (buf.is_null() || modo != 2)) {
                return None;
            }
            let mut alfa = vec![0u8; w * h];
            for y in 0..h {
                let fila = if pitch < 0 { (h - 1 - y) * (-pitch) as usize } else { y * pitch as usize };
                for x in 0..w {
                    alfa[y * w + x] = refuerzo(*buf.add(fila + x));
                }
            }
            Some(Glifo { w: w as i32, h: h as i32, left, top, avance: avance as f32 / 64.0, alfa })
        }
    }

    fn metricas(&mut self, e: Estilo, px: u32) -> Metricas {
        self.fijar(e.semibold(), px);
        let c = self.cara(e.semibold());
        let (upem, asc, desc): (u16, i16, i16) = unsafe { (leer(c.ptr, FACE_UPEM), leer(c.ptr, FACE_ASC), leer(c.ptr, FACE_DESC)) };
        let k = px as f32 / upem.max(1) as f32;
        let (asc, desc) = ((asc as f32 * k).ceil() as i32, (-(desc as f32) * k).ceil() as i32);
        Metricas { asc, desc, cap: 0 }
    }
}

/// Texto claro sobre fondo oscuro: la mezcla en el espacio de color de la pantalla (sin linealizar) adelgaza los trazos
/// claros, asi que se refuerza algo la cobertura de los bordes (gamma 0,8). 0 y 255 no cambian.
fn refuerzo(c: u8) -> u8 {
    if c == 0 || c == 255 {
        return c;
    }
    (255.0 * (c as f32 / 255.0).powf(0.8)).round() as u8
}

// ---------------------------------------------------------------------------------------------------------------
// Tipografia

struct Interno {
    ft: Option<Ft>,
    escala: f32,
    glifos: HashMap<(Estilo, u32, char), Rc<Glifo>>,
    metricas: HashMap<(Estilo, u32), Metricas>,
    nombre: String,
}

/// Tipografia de la ventana: FreeType si esta (o el respaldo 5x7), con cache de glifos y medicion en dp.
/// Se usa solo desde el hilo de la ventana.
pub struct Tipografia {
    i: RefCell<Interno>,
}

/// Glifo de la fuente 5x7 con escala entera `k`.
fn glifo_respaldo(c: char, k: u32) -> Glifo {
    let k = k.max(1) as usize;
    let g = mapa5x7::glifo(mapa5x7::base(c)).or_else(|| mapa5x7::glifo('?')).unwrap();
    let (w, h) = (5 * k, mapa5x7::ALTO * k);
    let mut alfa = vec![0u8; w * h];
    for (col, bits) in g.iter().enumerate() {
        for fila in 0..mapa5x7::ALTO {
            if bits >> fila & 1 != 0 {
                for dy in 0..k {
                    for dx in 0..k {
                        alfa[(fila * k + dy) * w + col * k + dx] = 255;
                    }
                }
            }
        }
    }
    Glifo { w: w as i32, h: h as i32, left: 0, top: h as i32, avance: (mapa5x7::CELDA * k) as f32, alfa }
}

impl Tipografia {
    /// Intenta FreeType + fontconfig; si no, el respaldo 5x7 (con un aviso por la salida de errores).
    pub fn nueva(escala: f32) -> Tipografia {
        // WEFT_SIN_FUENTE=1 fuerza el respaldo (para probarlo en un equipo que si tiene fuentes)
        if std::env::var("WEFT_SIN_FUENTE").is_ok_and(|v| v == "1") {
            eprintln!("{}", tx!("fuente.aviso_sin_fuente_1_se_usa_la_fuente_de"));
            return Tipografia::solo_respaldo(escala);
        }
        match Ft::cargar() {
            Ok(ft) => {
                let nombre = format!("{} (FreeType)", ft.regular.ruta);
                Tipografia { i: RefCell::new(Interno { ft: Some(ft), escala: escala.max(0.5), glifos: HashMap::new(), metricas: HashMap::new(), nombre }) }
            }
            Err(e) => {
                eprintln!("{}", txf!("fuente.aviso_sin_fuente_del_sistema_se_usa_la", e));
                Tipografia::solo_respaldo(escala)
            }
        }
    }

    /// Solo la fuente de mapa de bits (determinista; para las pruebas y como ultimo recurso).
    pub fn solo_respaldo(escala: f32) -> Tipografia {
        Tipografia { i: RefCell::new(Interno { ft: None, escala: escala.max(0.5), glifos: HashMap::new(), metricas: HashMap::new(), nombre: tx!("fuente.mapa_de_bits_5x7_respaldo").into() }) }
    }

    #[cfg(test)]
    pub fn es_respaldo(&self) -> bool {
        self.i.borrow().ft.is_none()
    }

    pub fn nombre(&self) -> String {
        self.i.borrow().nombre.clone()
    }

    pub fn escala(&self) -> f32 {
        self.i.borrow().escala
    }

    /// Cambia el factor de escala (la ventana paso a otra pantalla): los glifos se rasterizan de nuevo a la medida.
    pub fn fijar_escala(&self, escala: f32) {
        let mut i = self.i.borrow_mut();
        let e = escala.max(0.5);
        if (e - i.escala).abs() > 1e-3 {
            i.escala = e;
        }
    }

    /// Tamano con que se rasteriza `e` (clave del cache): pixeles fisicos con FreeType; con el respaldo, la escala entera
    /// de la fuente de mapa de bits.
    pub fn clave_px(&self, e: Estilo) -> u32 {
        let i = self.i.borrow();
        if i.ft.is_some() {
            e.px(i.escala)
        } else {
            e.k_respaldo() * (i.escala.round().max(1.0) as u32)
        }
    }

    pub fn glifo(&self, e: Estilo, c: char) -> Rc<Glifo> {
        let px = self.clave_px(e);
        let mut i = self.i.borrow_mut();
        if let Some(g) = i.glifos.get(&(e, px, c)) {
            return g.clone();
        }
        let g = match i.ft.as_mut() {
            Some(ft) => ft.rasterizar(e, px, c).or_else(|| ft.rasterizar(e, px, '?')).unwrap_or_else(|| glifo_respaldo(c, 2)),
            None => glifo_respaldo(c, px),
        };
        let g = Rc::new(g);
        i.glifos.insert((e, px, c), g.clone());
        g
    }

    pub fn metricas(&self, e: Estilo) -> Metricas {
        let px = self.clave_px(e);
        if let Some(m) = self.i.borrow().metricas.get(&(e, px)) {
            return *m;
        }
        let cap = self.glifo(e, 'H').top;
        let mut i = self.i.borrow_mut();
        let m = match i.ft.as_mut() {
            Some(ft) => Metricas { cap, ..ft.metricas(e, px) },
            None => Metricas { asc: (mapa5x7::ALTO as u32 * px) as i32, desc: px as i32, cap: (mapa5x7::ALTO as u32 * px) as i32 },
        };
        i.metricas.insert((e, px), m);
        m
    }

    /// Reparte `t` en glifos: (x en pixeles fisicos desde el inicio del texto, caracter, glifo). La posicion se redondea al
    /// pixel pero el cursor avanza con el avance fraccionario, asi el ancho total no acumula error.
    pub fn trazar(&self, t: &str, e: Estilo) -> Vec<(i32, char, Rc<Glifo>)> {
        let mut pen = 0f32;
        let mut v = Vec::with_capacity(t.len());
        for c in t.chars() {
            let g = self.glifo(e, c);
            v.push((pen.round() as i32 + g.left, c, g.clone()));
            pen += g.avance;
        }
        v
    }
}

impl Medida for Tipografia {
    fn ancho(&self, t: &str, e: Estilo) -> f32 {
        let esc = self.escala();
        t.chars().map(|c| self.glifo(e, c).avance).sum::<f32>() / esc
    }
    fn alto_linea(&self, e: Estilo) -> f32 {
        let m = self.metricas(e);
        (m.asc + m.desc) as f32 / self.escala()
    }
    fn ascenso(&self, e: Estilo) -> f32 {
        self.metricas(e).asc as f32 / self.escala()
    }
    fn cap(&self, e: Estilo) -> f32 {
        self.metricas(e).cap as f32 / self.escala()
    }
}

// ---------------------------------------------------------------------------------------------------------------
// atlas

/// Reparto de una textura en estantes horizontales (cada pieza lleva 1 px de margen para que ningun muestreo se salga).
pub struct Atlas {
    pub w: i32,
    pub h: i32,
    x: i32,
    y: i32,
    alto_fila: i32,
}

impl Atlas {
    pub fn nuevo(w: i32, h: i32) -> Atlas {
        Atlas { w, h, x: 0, y: 0, alto_fila: 0 }
    }

    /// Esquina (x, y) para una pieza de `w` x `h`, o None si ya no cabe (hay que vaciar el atlas).
    pub fn reservar(&mut self, w: i32, h: i32) -> Option<(i32, i32)> {
        let (pw, ph) = (w + 1, h + 1);
        if pw > self.w || ph > self.h {
            return None;
        }
        if self.x + pw > self.w {
            self.x = 0;
            self.y += self.alto_fila;
            self.alto_fila = 0;
        }
        if self.y + ph > self.h {
            return None;
        }
        let p = (self.x, self.y);
        self.x += pw;
        self.alto_fila = self.alto_fila.max(ph);
        Some(p)
    }

    pub fn vaciar(&mut self) {
        self.x = 0;
        self.y = 0;
        self.alto_fila = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn respaldo_mide_con_la_celda() {
        let t = Tipografia::solo_respaldo(1.0);
        // Cuerpo = escala 2 del 5x7: celda de 12 px por caracter
        assert_eq!(t.ancho("abc", Estilo::Cuerpo), 36.0);
        assert_eq!(t.ancho("", Estilo::Cuerpo), 0.0);
        assert_eq!(t.ancho("abc", Estilo::Pequeno), 18.0);
        assert_eq!(t.alto_linea(Estilo::Cuerpo), 16.0);
        assert_eq!(t.ascenso(Estilo::Cuerpo), 14.0);
        assert!(t.es_respaldo());
        // con escala 2 las medidas en dp no cambian (la fuente se rasteriza al doble)
        let t2 = Tipografia::solo_respaldo(2.0);
        assert_eq!(t2.ancho("abc", Estilo::Cuerpo), 36.0);
        assert_eq!(t2.glifo(Estilo::Cuerpo, 'a').h, 28);
    }

    #[test]
    fn respaldo_sin_tildes_ni_simbolos() {
        let t = Tipografia::solo_respaldo(1.0);
        assert_eq!(*t.glifo(Estilo::Cuerpo, 'é'), *t.glifo(Estilo::Cuerpo, 'e'));
        assert_eq!(*t.glifo(Estilo::Cuerpo, 'Ú'), *t.glifo(Estilo::Cuerpo, 'U'));
        assert_eq!(*t.glifo(Estilo::Cuerpo, '—'), *t.glifo(Estilo::Cuerpo, '-'));
        // la ene si tiene glifo propio (no es una n)
        assert_ne!(*t.glifo(Estilo::Cuerpo, 'Ñ'), *t.glifo(Estilo::Cuerpo, 'N'));
        assert_ne!(*t.glifo(Estilo::Cuerpo, 'ñ'), *t.glifo(Estilo::Cuerpo, 'n'));
        // un caracter sin glifo sale como ?
        assert_eq!(*t.glifo(Estilo::Cuerpo, '€'), *t.glifo(Estilo::Cuerpo, '?'));
        assert!(!mapa5x7::cubre('€') && mapa5x7::cubre('é') && mapa5x7::cubre('¿') && mapa5x7::cubre('a'));
        let g = t.glifo(Estilo::Cuerpo, 'I');
        assert_eq!((g.w, g.h, g.top, g.avance), (10, 14, 14, 12.0));
        assert!(g.alfa.iter().all(|v| *v == 0 || *v == 255) && g.alfa.contains(&255));
    }

    #[test]
    fn cada_glifo_existe() {
        for n in mapa5x7::PRIMERO..=mapa5x7::ULTIMO {
            let c = char::from_u32(n).unwrap();
            let g = mapa5x7::glifo(c).unwrap_or_else(|| panic!("falta el glifo {:?}", c));
            if c == ' ' {
                assert_eq!(g, [0; 5]);
            } else {
                assert!(g.iter().any(|b| *b != 0), "glifo vacio: {:?}", c);
                assert!(g.iter().all(|b| *b < 0x80), "glifo {:?} con mas de 7 filas", c);
            }
        }
        assert!(mapa5x7::glifo('\u{7f}').is_none() && mapa5x7::glifo('\u{1f}').is_none() && mapa5x7::glifo('é').is_none());
        // los signos propios: con algo dibujado, en 7 filas, y sin letra base que los tape
        for (c, g) in mapa5x7::EXTRA {
            assert_eq!(mapa5x7::glifo(c), Some(g), "{:?}", c);
            assert_eq!(mapa5x7::base(c), c, "{:?} no debe reducirse a otra letra", c);
            assert!(g.iter().any(|b| *b != 0) && g.iter().all(|b| *b < 0x80), "glifo {:?}", c);
        }
    }

    /// Una pregunta, una exclamacion, grados, comillas angulares, el separador de la barra y la ene salen con su dibujo,
    /// no con el '?' de relleno (que solo aparece donde hay un '?' de verdad).
    #[test]
    fn respaldo_dibuja_los_signos_del_espanol() {
        let t = Tipografia::solo_respaldo(1.0);
        let relleno = t.glifo(Estilo::Cuerpo, '?');
        for texto in ["¿Apagar la máquina?", "¡Listo!", "Rotación 90°", "«Ajustar» la adapta", "Android en marcha  ·  100 %", "Año: AÑO, ñandú", "Configuración"] {
            for (_, c, g) in t.trazar(texto, Estilo::Cuerpo) {
                assert!(mapa5x7::cubre(c), "{:?} en {:?} no tiene glifo", c, texto);
                if c != '?' {
                    assert_ne!(*g, *relleno, "{:?} en {:?} salio como '?'", c, texto);
                }
            }
        }
        // los de apertura son los de cierre girados: distintos entre si y del de cierre
        assert_ne!(*t.glifo(Estilo::Cuerpo, '¿'), *relleno);
        assert_ne!(*t.glifo(Estilo::Cuerpo, '¡'), *t.glifo(Estilo::Cuerpo, '!'));
        assert_ne!(*t.glifo(Estilo::Cuerpo, '«'), *t.glifo(Estilo::Cuerpo, '»'));
        // y miden lo mismo que cualquier otro caracter (celda fija)
        assert_eq!(t.ancho("¿·°", Estilo::Cuerpo), t.ancho("abc", Estilo::Cuerpo));
    }

    #[test]
    fn glifos_distintos() {
        let mut vistos: Vec<([u8; 5], char)> = Vec::new();
        let todos = (mapa5x7::PRIMERO..=mapa5x7::ULTIMO).map(|n| char::from_u32(n).unwrap()).chain(mapa5x7::EXTRA.iter().map(|(c, _)| *c));
        for c in todos {
            let g = mapa5x7::glifo(c).unwrap();
            if let Some((_, otro)) = vistos.iter().find(|(x, _)| *x == g) {
                panic!("{:?} y {:?} tienen el mismo glifo", c, otro);
            }
            vistos.push((g, c));
        }
    }

    #[test]
    fn trunca_con_puntos_suspensivos() {
        let t = Tipografia::solo_respaldo(1.0);
        assert_eq!(truncar(&t, "corto", Estilo::Cuerpo, 100.0), "corto");
        // 6 caracteres de 12 dp = 72; "Un mando largo" (14) no cabe en 72: 3 letras + "..."
        let r = truncar(&t, "Un mando largo", Estilo::Cuerpo, 72.0);
        assert_eq!(r, "Un...");
        assert!(t.ancho(&r, Estilo::Cuerpo) <= 72.0 && t.ancho(&r, Estilo::Cuerpo) > 0.0);
        // el resultado siempre cabe
        for w in [10.0, 40.0, 80.0, 150.0] {
            let r = truncar(&t, "Configuración de la máquina", Estilo::Cuerpo, w);
            assert!(t.ancho(&r, Estilo::Cuerpo) <= w, "{} no cabe en {}", r, w);
        }
        assert_eq!(truncar(&t, "abc", Estilo::Cuerpo, 5.0), "");
    }

    /// Recortar por el centro deja ver el principio y el final (el nombre del archivo de una ruta) y siempre cabe.
    #[test]
    fn trunca_por_el_centro() {
        let t = Tipografia::solo_respaldo(1.0);
        assert_eq!(truncar_centro(&t, "corto", Estilo::Cuerpo, 100.0), "corto");
        let largo = "Captura guardada en /home/ana/Imágenes/captura-20261008-120000.png";
        for w in [120.0f32, 240.0, 400.0, 600.0] {
            let r = truncar_centro(&t, largo, Estilo::Cuerpo, w);
            assert!(t.ancho(&r, Estilo::Cuerpo) <= w, "{:?} no cabe en {}", r, w);
            assert!(r.contains("...") && r.starts_with("Ca") && r.ends_with(".png"), "{:?}", r);
            if w >= 600.0 {
                assert!(r.ends_with("/captura-20261008-120000.png") && r.starts_with("Captura guardad"), "{:?}", r);
            }
        }
        assert!(t.ancho(&truncar_centro(&t, largo, Estilo::Cuerpo, 30.0), Estilo::Cuerpo) <= 30.0);
        assert_eq!(truncar_centro(&t, "abc", Estilo::Cuerpo, 5.0), "");
    }

    #[test]
    fn envuelve_por_palabras() {
        let t = Tipografia::solo_respaldo(1.0);
        // lineas de 8 caracteres (96 dp)
        let l = envolver(&t, "uno dos tres cuatro", Estilo::Cuerpo, 96.0, 0);
        assert_eq!(l, vec!["uno dos", "tres", "cuatro"]);
        assert!(l.iter().all(|x| t.ancho(x, Estilo::Cuerpo) <= 96.0));
        // palabra mas larga que la linea: se corta
        let l = envolver(&t, "abcdefghijkl", Estilo::Cuerpo, 60.0, 0);
        assert!(l.len() >= 2 && l.iter().all(|x| t.ancho(x, Estilo::Cuerpo) <= 60.0), "{:?}", l);
        assert_eq!(l.concat(), "abcdefghijkl");
        // limite de lineas: la ultima termina en ...
        let l = envolver(&t, "uno dos tres cuatro cinco seis", Estilo::Cuerpo, 96.0, 2);
        assert_eq!(l.len(), 2);
        assert!(l[1].ends_with("..."), "{:?}", l);
        assert!(t.ancho(&l[1], Estilo::Cuerpo) <= 96.0);
        assert!(envolver(&t, "", Estilo::Cuerpo, 96.0, 3).is_empty());
    }

    /// Sin ancho (un dialogo mas estrecho que sus margenes) no hay panico: el texto sale entero en una linea, y vacio, nada.
    /// Con un ancho menor que una letra tampoco: una letra por linea.
    #[test]
    fn envuelve_sin_ancho_sin_panico() {
        let t = Tipografia::solo_respaldo(1.0);
        for max in [0.0f32, -1.0, -16.0, f32::NAN, f32::NEG_INFINITY] {
            assert_eq!(envolver(&t, "uno  dos tres", Estilo::Cuerpo, max, 0), vec!["uno dos tres"], "max {}", max);
            assert_eq!(envolver(&t, "palabra", Estilo::Cuerpo, max, 2), vec!["palabra"], "max {}", max);
            assert!(envolver(&t, "   ", Estilo::Cuerpo, max, 0).is_empty(), "max {}", max);
        }
        // mas estrecho que una letra (12 dp): una letra por linea, todas las letras
        let l = envolver(&t, "abc de", Estilo::Cuerpo, 5.0, 0);
        assert_eq!(l, vec!["a", "b", "c", "d", "e"]);
        // y el limite de lineas sigue valiendo
        assert_eq!(envolver(&t, "abc de", Estilo::Cuerpo, 5.0, 2).len(), 2);
    }

    #[test]
    fn atlas_reparte_estantes() {
        let mut a = Atlas::nuevo(32, 16);
        assert_eq!(a.reservar(10, 8), Some((0, 0)));
        assert_eq!(a.reservar(10, 6), Some((11, 0)));
        // el tercero ya no cabe en la fila: pasa a la siguiente (alto de la fila mas alta + margen)
        assert_eq!(a.reservar(12, 5), Some((0, 9)));
        assert_eq!(a.reservar(40, 2), None); // mas ancha que el atlas
        // se llena y avisa
        let mut n = 0;
        while a.reservar(10, 6).is_some() {
            n += 1;
            assert!(n < 100);
        }
        a.vaciar();
        assert_eq!(a.reservar(31, 15), Some((0, 0)));
    }

    /// Con FreeType y alguna fuente en el sistema (en el contenedor de pruebas puede no haber): el glifo 'A' tiene las
    /// medidas de una mayuscula de 13 px, las tildes existen y la medida crece con el tamano.
    #[test]
    fn freetype_si_esta() {
        let t = Tipografia::nueva(1.0);
        if t.es_respaldo() {
            eprintln!("sin FreeType o sin fuentes: se omite");
            return;
        }
        let a = t.glifo(Estilo::Cuerpo, 'A');
        assert!((6..=12).contains(&a.w) && (8..=11).contains(&a.h) && (8..=11).contains(&a.top), "A: {:?}", (a.w, a.h, a.top));
        assert!(a.avance > 6.0 && a.avance < 12.0, "avance {}", a.avance);
        assert!(a.alfa.iter().any(|v| *v > 200) && a.alfa.iter().any(|v| *v > 0 && *v < 200), "sin suavizado");
        // tildes y enes: glifos propios, distintos de la letra base y de '?'
        for c in ['á', 'é', 'í', 'ó', 'ú', 'ü', 'ñ', 'Ñ', 'Á'] {
            let g = t.glifo(Estilo::Cuerpo, c);
            assert!(g.w > 0 && g.h > 0, "{:?}", c);
            assert_ne!(*g, *t.glifo(Estilo::Cuerpo, '?'), "{:?} salio como ?", c);
        }
        assert_ne!(*t.glifo(Estilo::Cuerpo, 'é'), *t.glifo(Estilo::Cuerpo, 'e'));
        // el espacio no dibuja pero avanza
        let e = t.glifo(Estilo::Cuerpo, ' ');
        assert!(e.avance > 2.0 && e.alfa.iter().all(|v| *v == 0));
        // mide: mas grande el titulo que el cuerpo; el semibold es un poco mas ancho
        let (c, ti, p) = (t.ancho("Configuración", Estilo::Cuerpo), t.ancho("Configuración", Estilo::Titulo), t.ancho("Configuración", Estilo::Pequeno));
        assert!(p < c && c < ti, "{} {} {}", p, c, ti);
        assert!(t.ancho("Configuración", Estilo::Negrita) >= c);
        // metricas: la linea cabe el cuerpo y las mayusculas son mas bajas que el ascenso
        let m = t.metricas(Estilo::Cuerpo);
        assert!(m.asc > m.cap && m.cap >= 8 && m.desc >= 2 && m.desc <= 5, "{:?}", m);
        // a escala 2 se rasteriza al doble (no se escala): mismo ancho en dp, glifo del doble de alto
        let t2 = Tipografia::nueva(2.0);
        let a2 = t2.glifo(Estilo::Cuerpo, 'A');
        assert!((a2.h - 2 * a.h).abs() <= 2, "{} vs {}", a2.h, a.h);
        let (w1, w2) = (t.ancho("Atajos del teclado", Estilo::Cuerpo), t2.ancho("Atajos del teclado", Estilo::Cuerpo));
        assert!((w1 - w2).abs() / w1 < 0.06, "{} vs {}", w1, w2);
        // el glifo no se sale de su caja
        assert_eq!(a.alfa.len(), (a.w * a.h) as usize);
    }
}
