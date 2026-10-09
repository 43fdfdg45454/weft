//! Textos para la gente y textos tecnicos: un solo interruptor por proceso.
//!
//! DECISION: la interfaz (ventana, barra, configuracion y todo lo que ella muestra: progreso de las operaciones, resultados,
//! errores, estado, salida del doctor) NO nombra productos ni proveedores (ni Cuttlefish, KernelSU, heddle, gfxstream,
//! ci.android.com...). La salida de las ordenes de consola (`root status`, `bridge status`, `doctor`...), el README y los
//! archivos de informe SI los nombran, porque ahi el detalle tecnico es lo que se busca.
//!
//! COMO: los modulos de operacion (root, puente, imagen, compartir, doctor...) producen su texto en el punto de uso con
//! `elige(generico, tecnico)`: el nombre del proveedor no esta en el texto que ve la interfaz sino en su rama tecnica. El
//! modo lo fija el proceso: la ventana (`fijar_interfaz(true)` al arrancar) y las ordenes que lanza (variable de entorno
//! `WEFT_SALIDA=interfaz`) producen la rama generica; una orden de consola normal, la tecnica. Lo que no es texto de
//! operacion (estado, campos, notas fijas) lo arma la interfaz con palabras genericas por construccion.
//!
//! DEFENSA EN PROFUNDIDAD: un mensaje que llegue a la interfaz por un camino imprevisto (el error de una herramienta, la salida
//! de una orden del invitado) pasa por `limpiar`, que sustituye cualquier nombre prohibido (como palabra suelta: `checksum` no
//! nombra a nadie aunque lleve dentro `ksu`). Una prueba recorre la interfaz en
//! todos sus estados y exige que ni siquiera haga falta (que `limpiar` no cambie nada de lo que producen los modulos).
//!
//! CATALOGO: ningun texto para la persona esta en el codigo: los de la interfaz (ventana, barra, configuracion, avisos,
//! errores y la pantalla de fin), los de operacion con dos caras (las dos ramas de `elige`) y los de la linea de ordenes
//! (ayuda, salida y errores de cada orden, errores de validacion de la configuracion y de las carpetas compartidas, ayuda de
//! cada clave de configuracion) estan en un catalogo por idioma (`textos/es.rs`), por clave: `tx!(clave)` da el texto,
//! `txf!(clave, a, b)` lo da con sus huecos rellenos y `clave!(clave)` deja la clave en una tabla constante para traducirla al
//! usarla (con `texto`, o con `parte_fija` para reconocer un mensaje ya formado). Otro idioma es otro archivo de catalogo
//! con las mismas claves (ver `textos/es.rs`); el codigo no cambia. Hoy solo hay espanol, que es tambien el de reserva. Las
//! pruebas exigen que toda clave que usa el codigo exista y que no sobre ninguna, y que no quede ningun texto visible fuera
//! del catalogo (criterio en `no_quedan_textos_visibles_fuera_del_catalogo`). Los textos tecnicos de consola, que pueden
//! nombrar componentes, van en secciones `*_tec`; el resto no nombra a nadie.
//!
//! Este modulo no depende de nada de weft.

mod es;

use std::cell::Cell;
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// Catalogos de la interfaz: (codigo de idioma, entradas). El primero es el de reserva y tiene todas las claves.
const CATALOGOS: &[(&str, &[(&str, &str)])] = &[("es", es::TEXTOS)];

/// Posicion en `CATALOGOS` del idioma del escritorio (`LC_ALL`, `LC_MESSAGES` o `LANG`, por este orden, como gettext); el de
/// reserva si no hay catalogo para el.
fn idioma() -> usize {
    static I: OnceLock<usize> = OnceLock::new();
    *I.get_or_init(|| {
        let var = ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()));
        var.and_then(|v| idioma_de(&v)).unwrap_or(0)
    })
}

/// Catalogo para un valor de locale (`es_AR.UTF-8`, `en`...): por el idioma, sin la region ni la codificacion. Pura.
fn idioma_de(locale: &str) -> Option<usize> {
    let codigo = locale.split(['_', '.', '@']).next().unwrap_or("").to_ascii_lowercase();
    CATALOGOS.iter().position(|(c, _)| *c == codigo)
}

fn tablas() -> &'static [HashMap<&'static str, &'static str>] {
    static T: OnceLock<Vec<HashMap<&'static str, &'static str>>> = OnceLock::new();
    T.get_or_init(|| CATALOGOS.iter().map(|(_, e)| e.iter().copied().collect()).collect())
}

/// El texto de una clave del catalogo en el idioma de la interfaz (o en el de reserva si ese idioma no la tiene). Una clave
/// que no existe (no deberia pasar: lo comprueban las pruebas) sale tal cual, para que se note sin romper nada.
pub fn texto(clave: &'static str) -> &'static str {
    let t = tablas();
    t[idioma()].get(clave).or_else(|| t[0].get(clave)).copied().unwrap_or(clave)
}

/// El texto de una clave con sus huecos rellenos: `{}` toma el argumento siguiente y `{N}` el de la posicion N; `{{` y `}}`
/// son llaves. Un hueco sin argumento queda vacio.
pub fn formato(clave: &'static str, args: &[&dyn Display]) -> String {
    rellenar(texto(clave), args)
}

/// El trozo fijo mas largo del texto de una clave (lo que hay entre sus huecos, sin los espacios ni la puntuacion de los
/// bordes): para reconocer en un mensaje ya formado uno que salio de esa clave, en cualquier idioma. P. ej. de
/// `"{} ya esta conectado ({})"`, `"ya esta conectado"`.
pub fn parte_fija(clave: &'static str) -> &'static str {
    let t = texto(clave);
    let mut mejor = "";
    let mut resto = t;
    loop {
        let fin = resto.find(['{', '}']).unwrap_or(resto.len());
        let trozo = resto[..fin].trim_matches(|c: char| !c.is_alphanumeric());
        if trozo.len() > mejor.len() {
            mejor = trozo;
        }
        match resto[fin..].find('}') {
            Some(j) if fin < resto.len() => resto = &resto[fin + j + 1..],
            _ => break,
        }
    }
    mejor
}

fn rellenar(t: &str, args: &[&dyn Display]) -> String {
    let mut out = String::with_capacity(t.len() + 16);
    let mut siguiente = 0;
    let mut resto = t;
    while let Some(i) = resto.find(['{', '}']) {
        out.push_str(&resto[..i]);
        let tras = &resto[i..];
        if tras.starts_with("{{") || tras.starts_with("}}") {
            out.push_str(&tras[..1]);
            resto = &tras[2..];
            continue;
        }
        match (tras.starts_with('{'), tras.find('}')) {
            (true, Some(j)) => {
                let dentro = &tras[1..j];
                let n = if dentro.is_empty() {
                    siguiente += 1;
                    Some(siguiente - 1)
                } else {
                    dentro.parse::<usize>().ok()
                };
                match n {
                    Some(n) => {
                        if let Some(a) = args.get(n) {
                            out.push_str(&a.to_string());
                        }
                    }
                    None => out.push_str(&tras[..=j]),
                }
                resto = &tras[j + 1..];
            }
            _ => {
                out.push_str(&tras[..1]);
                resto = &tras[1..];
            }
        }
    }
    out.push_str(resto);
    out
}

/// Huecos de un texto del catalogo: cuantos argumentos pide (el mayor `{N}` + 1, o cuantos `{}`). Para las pruebas.
#[cfg(test)]
fn huecos(t: &str) -> usize {
    let (mut seguidos, mut maximo, mut resto) = (0usize, 0usize, t.replace("{{", "").replace("}}", ""));
    while let Some(i) = resto.find('{') {
        let j = resto[i..].find('}').map_or(resto.len(), |j| i + j);
        match resto[i + 1..j].parse::<usize>() {
            Ok(n) => maximo = maximo.max(n + 1),
            Err(_) => seguidos += 1,
        }
        resto = resto[(j + 1).min(resto.len())..].to_string();
    }
    seguidos.max(maximo)
}

/// El texto de una clave del catalogo (`&'static str`). La clave es un literal: la prueba del catalogo la encuentra.
macro_rules! tx {
    ($k:literal) => {
        $crate::textos::texto($k)
    };
}

/// El texto de una clave del catalogo con sus huecos rellenos (`String`), como `format!` con `{}`.
macro_rules! txf {
    ($k:literal $(, $a:expr)+ $(,)?) => {
        $crate::textos::formato($k, &[$(&$a as &dyn ::std::fmt::Display),+])
    };
}

/// Una clave del catalogo tal cual (para tablas constantes, que se traducen al usarlas con `texto`).
macro_rules! clave {
    ($k:literal) => {
        $k
    };
}

pub(crate) use {clave, tx, txf};

static INTERFAZ: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// forzado por hilo (solo pruebas): Some(true) = interfaz, Some(false) = consola
    static FORZADO: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Variable de entorno con que la ventana avisa a las ordenes que lanza de que su salida es para la interfaz.
pub const VARIABLE: &str = "WEFT_SALIDA";

/// Fija el modo del proceso: true = el texto es para la interfaz (generico).
pub fn fijar_interfaz(b: bool) {
    INTERFAZ.store(b, Ordering::Relaxed);
}

/// Toma el modo de la variable de entorno `WEFT_SALIDA=interfaz` (para las ordenes que lanza la ventana).
pub fn del_entorno() {
    if std::env::var(VARIABLE).is_ok_and(|v| v == "interfaz") {
        fijar_interfaz(true);
    }
}

/// ¿El texto que se produce ahora es para la interfaz?
pub fn interfaz() -> bool {
    FORZADO.with(|f| f.get()).unwrap_or_else(|| INTERFAZ.load(Ordering::Relaxed))
}

/// Ejecuta `f` produciendo texto de interfaz en este hilo (para las pruebas) o, con `false`, de consola. Tambien lo usan
/// las partes que escriben archivos de informe: ahi el detalle tecnico es lo que importa aunque lo lance la ventana.
pub fn con_modo<R>(interfaz: bool, f: impl FnOnce() -> R) -> R {
    let antes = FORZADO.with(|x| x.replace(Some(interfaz)));
    let r = f();
    FORZADO.with(|x| x.set(antes));
    r
}

/// El texto generico si la salida es para la interfaz; si no, el tecnico.
pub fn elige(generico: &str, tecnico: &str) -> String {
    if interfaz() {
        generico.to_string()
    } else {
        tecnico.to_string()
    }
}

/// Palabras que la interfaz no puede mostrar (sin distinguir mayusculas): productos, proveedores y nombres de componentes.
/// Cuentan como palabras sueltas (ver `como_palabra`): por eso estan tambien las formas compuestas que se escriben pegadas
/// (`ksunext`, `kernelsunext`, `libheddle`, `libgfxstream`...), que la palabra corta ya no encuentra dentro de ellas.
pub const PROHIBIDAS: [&str; 19] = [
    "kernelsu",
    "kernelsunext",
    "ksud",
    "ksu",
    "ksunext",
    "rifsxd",
    "weishu",
    "magisk",
    "zygisk",
    "cuttlefish",
    "heddle",
    "libheddle",
    "ci.android.com",
    "gfxstream",
    "libgfxstream",
    "rutabaga",
    "librutabaga",
    "late-load",
    "lkm",
];

/// Lo que pone `limpiar` en lugar de cada palabra prohibida (clave del catalogo).
const SUSTITUTO: &str = clave!("textos.componente_de_terceros");

/// Primera aparicion (posicion en bytes, desde `desde`) de la palabra `p` en `b` (ya en minusculas) como palabra suelta: sin
/// una letra ni un digito ASCII pegados por ninguno de los dos lados. Todo lo demas es limite: el principio y el final, el
/// espacio, la puntuacion, una letra no ASCII y tambien `_`, `.`, `/` y `-`, para que los nombres de archivo y de paquete
/// (`libheddle.so`, `KernelSU-Next`, `me.weishu.kernelsu`, `ksu_x`) se sigan encontrando. Asi `checksum` (lleva `ksu`) o
/// `vbmeta_system_dlkm.img` (lleva `lkm`) no se tocan: la `c` y la `d` van pegadas.
fn como_palabra(b: &str, p: &str, desde: usize) -> Option<usize> {
    let bytes = b.as_bytes();
    let mut i = desde;
    while let Some(j) = b.get(i..).and_then(|resto| resto.find(p)) {
        let (ini, fin) = (i + j, i + j + p.len());
        let suelta_antes = ini == 0 || !bytes[ini - 1].is_ascii_alphanumeric();
        let suelta_despues = fin == bytes.len() || !bytes[fin].is_ascii_alphanumeric();
        if suelta_antes && suelta_despues {
            return Some(ini);
        }
        // las palabras empiezan por un caracter ASCII: el byte siguiente es principio de caracter
        i = ini + 1;
    }
    None
}

/// La primera palabra prohibida que aparezca en `t` como palabra suelta (sin distinguir mayusculas; ver `como_palabra`), si hay.
/// La usan las pruebas de la interfaz de varios modulos.
#[cfg_attr(not(test), allow(dead_code))]
pub fn prohibida_en(t: &str) -> Option<&'static str> {
    let b = t.to_ascii_lowercase();
    PROHIBIDAS.iter().copied().find(|p| como_palabra(&b, p, 0).is_some())
}

/// Red de seguridad para un mensaje dinamico que llega a la interfaz: cambia cada palabra prohibida (suelta, como en
/// `prohibida_en`) por «componente de terceros». No se usa con rutas (son del usuario).
pub fn limpiar(t: &str) -> String {
    let mut s = t.to_string();
    // las mas largas primero para no dejar restos (libheddle antes que heddle, kernelsu antes que ksu...)
    let mut ps: Vec<&str> = PROHIBIDAS.to_vec();
    ps.sort_by_key(|p| std::cmp::Reverse(p.len()));
    for p in ps {
        // las palabras son ASCII: to_ascii_lowercase conserva la longitud en bytes, asi que los indices valen para `s`; se
        // sigue buscando detras de cada sustitucion
        let mut desde = 0;
        while let Some(i) = como_palabra(&s.to_ascii_lowercase(), p, desde) {
            let sustituto = texto(SUSTITUTO);
            s.replace_range(i..i + p.len(), sustituto);
            desde = i + sustituto.len();
        }
    }
    s
}

/// Origen de una descarga sin el nombre del proyecto: solo el sitio. `github.com/ALGUIEN/proyecto/...` -> GitHub;
/// `ci.android.com` -> el servidor de compilaciones de Android.
pub fn sitio_generico(origen: &str) -> String {
    let o = origen.to_lowercase();
    if o.contains("github.com") {
        "GitHub".to_string()
    } else if o.contains("ci.android.com") {
        tx!("textos.el_servidor_de_compilaciones_de_android").to_string()
    } else {
        tx!("textos.un_servidor_de_terceros").to_string()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn el_modo_se_elige_por_hilo_y_por_proceso() {
        assert_eq!(con_modo(true, || elige("generico", "tecnico")), "generico");
        assert_eq!(con_modo(false, || elige("generico", "tecnico")), "tecnico");
        // anidados: vuelve al anterior
        con_modo(true, || {
            assert!(interfaz());
            con_modo(false, || assert!(!interfaz()));
            assert!(interfaz());
        });
        // sin forzar, manda el proceso (en las pruebas es consola salvo que otra prueba lo cambie: no se toca aqui)
        assert!(FORZADO.with(|f| f.get()).is_none());
    }

    #[test]
    fn limpiar_cambia_las_palabras_prohibidas() {
        assert_eq!(limpiar("todo en orden"), "todo en orden");
        assert_eq!(limpiar("descargando KernelSU-Next (5 MiB)"), "descargando componente de terceros-Next (5 MiB)");
        let sucio = "ksud falla; libheddle.so de HEDDLE y Cuttlefish con gfxstream en ci.android.com tras late-load (me.weishu.kernelsu, Magisk, Zygisk, rutabaga, LKM)";
        let limpio = limpiar(sucio);
        assert!(prohibida_en(&limpio).is_none(), "{}", limpio);
        assert!(limpio.contains("componente de terceros"));
        assert_eq!(prohibida_en("Sin problemas"), None);
        // las formas pegadas tienen su propia entrada (la palabra corta ya no las encuentra por dentro)
        assert_eq!(prohibida_en("KSUnext"), Some("ksunext"));
        assert_eq!(prohibida_en("struct KernelSuNext"), Some("kernelsunext"));
        assert_eq!(prohibida_en("falta libgfxstream_backend.so"), Some("libgfxstream"));
        assert_eq!(prohibida_en("librutabaga_gfx_ffi.so.0"), Some("librutabaga"));
    }

    /// Solo palabras sueltas: una palabra prohibida pegada a una letra o un digito ASCII es parte de otra palabra.
    #[test]
    fn limpiar_compara_palabras_y_no_trozos() {
        // "checksum" lleva "ksu" y "vbmeta_system_dlkm.img" lleva "lkm": no se tocan (caso real: lo que falta de una imagen)
        for t in ["checksum", "el checksum no coincide", "vbmeta_system_dlkm.img", "la imagen no trae: vbmeta_system_dlkm.img, vbmeta_vendor_dlkm.img (checksum)"] {
            assert_eq!(prohibida_en(t), None, "{}", t);
            assert_eq!(limpiar(t), t);
        }
        // pegadas a una letra o un digito por cualquiera de los lados: tampoco
        for t in ["xksu", "ksu2", "lkms", "heddles", "aheddle"] {
            assert_eq!(prohibida_en(t), None, "{}", t);
        }
        // los separadores de nombres de archivo y de paquete si son limite
        assert_eq!(limpiar("ksud falla"), "componente de terceros falla");
        assert_eq!(limpiar("libheddle.so"), "componente de terceros.so");
        assert_eq!(limpiar("KernelSU-Next"), "componente de terceros-Next");
        assert_eq!(limpiar("me.weishu.kernelsu"), "me.componente de terceros.componente de terceros");
        assert_eq!(limpiar("/data/adb/ksu_x"), "/data/adb/componente de terceros_x");
        assert_eq!(limpiar("modo LKM)"), "modo componente de terceros)");
        // una letra no ASCII al lado tambien es limite
        assert_eq!(prohibida_en("«heddle»"), Some("heddle"));
        // varias apariciones, y una suelta detras de una pegada
        assert_eq!(limpiar("ksu, KSU y checksum ksu"), "componente de terceros, componente de terceros y checksum componente de terceros");
        assert_eq!(como_palabra("checksum ksu", "ksu", 0), Some(9));
        assert_eq!(como_palabra("ksu", "ksu", 1), None);
    }

    #[test]
    fn sitios_genericos() {
        assert_eq!(sitio_generico("github.com/KernelSU-Next/KernelSU-Next/releases"), "GitHub");
        assert_eq!(sitio_generico("github.com/43fdfdg45454/heddle/releases"), "GitHub");
        assert_eq!(sitio_generico("ci.android.com"), "el servidor de compilaciones de Android");
        assert_eq!(sitio_generico("ejemplo.org"), "un servidor de terceros");
        for o in ["github.com/x/y", "ci.android.com", "otro"] {
            assert!(prohibida_en(&sitio_generico(o)).is_none());
        }
    }

    // -----------------------------------------------------------------------------------------------------------
    // catalogo

    #[test]
    fn huecos_del_catalogo() {
        let (a, b): (&dyn Display, &dyn Display) = (&"uno", &2);
        assert_eq!(rellenar("{} y {}", &[a, b]), "uno y 2");
        assert_eq!(rellenar("{1} antes que {0}", &[a, b]), "2 antes que uno");
        assert_eq!(rellenar("{{literal}} {}", &[a]), "{literal} uno");
        assert_eq!(rellenar("sin argumento: {}.", &[]), "sin argumento: .");
        assert_eq!(rellenar("{x} raro", &[a]), "{x} raro");
        assert_eq!(rellenar("sin huecos", &[a]), "sin huecos");
        assert_eq!(rellenar("Fijo {} %.", &[&75]), "Fijo 75 %.");
        assert_eq!((huecos("a {} b {}"), huecos("{1} {0} {1}"), huecos("nada {{}}"), huecos("")), (2, 2, 0, 0));
        // la macro hace lo mismo que format! con `{}`
        assert_eq!(txf!("controles.fijo", 75), format!("Fijo {} %.", 75));
        assert_eq!(tx!("comun.cancelar"), "Cancelar");
        // el idioma sale del locale: el codigo sin region ni codificacion; uno sin catalogo usa el de reserva
        assert_eq!(idioma_de("es_AR.UTF-8"), Some(0));
        assert_eq!(idioma_de("es"), Some(0));
        assert_eq!(idioma_de("xx_YY.UTF-8"), None);
        assert_eq!(idioma_de("C"), None);
    }

    /// Una aparicion de `tx!`, `txf!` o `clave!` en el codigo: (macro, clave, argumentos tras la clave, archivo y linea).
    struct Uso {
        macro_: String,
        clave: String,
        args: usize,
        donde: String,
    }

    #[derive(Debug, PartialEq)]
    enum Tok {
        Ident(String),
        Punct(char),
        Cadena(String),
    }

    /// Fichas de un fuente de Rust (identificadores, puntuacion y cadenas, con su linea), sin comentarios. Basta para encontrar
    /// las macros del catalogo y contar sus argumentos.
    fn fichas(src: &str) -> Vec<(Tok, usize)> {
        let c: Vec<char> = src.chars().collect();
        let (mut i, mut linea, mut out) = (0usize, 1usize, Vec::new());
        while i < c.len() {
            let x = c[i];
            if x == '\n' {
                linea += 1;
                i += 1;
            } else if x == '/' && c.get(i + 1) == Some(&'/') {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
            } else if x == '/' && c.get(i + 1) == Some(&'*') {
                i += 2;
                while i < c.len() && !(c[i] == '*' && c.get(i + 1) == Some(&'/')) {
                    linea += (c[i] == '\n') as usize;
                    i += 1;
                }
                i += 2;
            } else if x == '"' || (x == 'r' && matches!(c.get(i + 1), Some('"') | Some('#')) && !c[..i].last().is_some_and(|p| p.is_alphanumeric() || *p == '_')) {
                let crudo = x == 'r';
                let mut j = i + crudo as usize;
                let hashes = c[j..].iter().take_while(|h| **h == '#').count();
                j += hashes + 1;
                let mut t = String::new();
                while j < c.len() {
                    if !crudo && c[j] == '\\' {
                        t.push(c[j]);
                        t.push(*c.get(j + 1).unwrap_or(&' '));
                        j += 2;
                        continue;
                    }
                    if c[j] == '"' && (0..hashes).all(|k| c.get(j + 1 + k) == Some(&'#')) {
                        break;
                    }
                    linea += (c[j] == '\n') as usize;
                    t.push(c[j]);
                    j += 1;
                }
                out.push((Tok::Cadena(t), linea));
                i = j + 1 + hashes;
            } else if x == '\'' && (c.get(i + 2) == Some(&'\'') || c.get(i + 1) == Some(&'\\')) {
                // caracter (no un tiempo de vida)
                let fin = c[i + 2..].iter().position(|y| *y == '\'').map_or(c.len(), |p| i + 2 + p);
                i = fin + 1;
            } else if x.is_alphanumeric() || x == '_' {
                let ini = i;
                while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                    i += 1;
                }
                out.push((Tok::Ident(c[ini..i].iter().collect()), linea));
            } else {
                if !x.is_whitespace() {
                    out.push((Tok::Punct(x), linea));
                }
                i += 1;
            }
        }
        out
    }

    fn usos_en(nombre: &str, src: &str) -> Vec<Uso> {
        let f = fichas(src);
        let mut v = Vec::new();
        for i in 0..f.len() {
            let (Tok::Ident(m), linea) = &f[i] else { continue };
            if !matches!(m.as_str(), "tx" | "txf" | "clave") || f.get(i + 1).map(|x| &x.0) != Some(&Tok::Punct('!')) || f.get(i + 2).map(|x| &x.0) != Some(&Tok::Punct('(')) {
                continue;
            }
            let Some((Tok::Cadena(k), _)) = f.get(i + 3) else { continue };
            // argumentos: comas al primer nivel hasta el parentesis que cierra (sin contar una coma final)
            let (mut prof, mut comas, mut j, mut ultima_coma) = (0i32, 0usize, i + 4, false);
            while let Some((t, _)) = f.get(j) {
                match t {
                    Tok::Punct('(' | '[' | '{') => prof += 1,
                    Tok::Punct(')' | ']' | '}') if prof == 0 => break,
                    Tok::Punct(')' | ']' | '}') => prof -= 1,
                    Tok::Punct(',') if prof == 0 => comas += 1,
                    _ => {}
                }
                ultima_coma = matches!(t, Tok::Punct(','));
                j += 1;
            }
            v.push(Uso { macro_: m.clone(), clave: k.clone(), args: comas - ultima_coma as usize, donde: format!("{}:{}", nombre, linea) });
        }
        v
    }

    /// Todos los fuentes de `src/` salvo los catalogos.
    fn fuentes() -> Vec<(String, String)> {
        fn recorrer(d: &std::path::Path, out: &mut Vec<(String, String)>) {
            let mut es: Vec<_> = std::fs::read_dir(d).unwrap().flatten().map(|e| e.path()).collect();
            es.sort();
            for p in es {
                if p.is_dir() {
                    if p.file_name().is_some_and(|n| n != "textos") {
                        recorrer(&p, out);
                    }
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push((p.display().to_string(), std::fs::read_to_string(&p).unwrap()));
                }
            }
        }
        let mut v = Vec::new();
        recorrer(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut v);
        v
    }

    /// Toda clave que usa el codigo existe en el catalogo de reserva con los huecos que le dan (`tx!` y `clave!` ninguno,
    /// `txf!` tantos como argumentos) y no sobra ninguna clave sin usar.
    #[test]
    fn cada_clave_usada_existe_y_no_sobra_ninguna() {
        let mut usos = Vec::new();
        for (n, src) in fuentes() {
            usos.extend(usos_en(&n, &src));
        }
        assert!(usos.len() > 1500, "se encontraron pocos usos: {}", usos.len());
        let reserva = &tablas()[0];
        let mut errores = Vec::new();
        for u in &usos {
            match reserva.get(u.clave.as_str()) {
                None => errores.push(format!("{}: la clave {:?} no esta en el catalogo", u.donde, u.clave)),
                // `clave!` deja la clave para usarla despues (`texto`, `formato` o `parte_fija`): sus huecos los
                // comprueba quien la usa
                Some(_) if u.macro_ == "clave" => {}
                Some(t) => {
                    let pide = if u.macro_ == "txf" { u.args } else { 0 };
                    if huecos(t) != pide || (u.macro_ == "txf" && pide == 0) {
                        errores.push(format!("{}: {}!({:?}) con {} argumentos para {:?}", u.donde, u.macro_, u.clave, pide, t));
                    }
                }
            }
        }
        let usadas: std::collections::BTreeSet<&str> = usos.iter().map(|u| u.clave.as_str()).collect();
        for (k, _) in CATALOGOS[0].1 {
            if !usadas.contains(k) {
                errores.push(format!("la clave {:?} del catalogo no la usa nadie", k));
            }
        }
        assert!(errores.is_empty(), "{}", errores.join("\n"));
        // el lector de fuentes no se deja enganar por comentarios, cadenas ni caracteres
        let u = usos_en("x", "// tx!(\"comentada\")\nlet a = \"tx!(\\\"en cadena\\\")\"; let c = '('; txf!(\"k\", f(a, b), [1, 2],); clave!(\"z\")");
        let v: Vec<(&str, &str, usize)> = u.iter().map(|u| (u.macro_.as_str(), u.clave.as_str(), u.args)).collect();
        assert_eq!(v, vec![("txf", "k", 2), ("clave", "z", 0)]);
    }

    /// Cada catalogo tiene las mismas claves que el de reserva, sin repetidas, con los mismos huecos y sin nombrar a nadie,
    /// salvo las secciones tecnicas (`*_tec`): textos que solo salen por la consola (la rama tecnica de `elige`, la ayuda y
    /// la salida de las ordenes), donde el detalle tecnico es lo que se busca. Ninguna clave nombra a nadie.
    #[test]
    fn los_catalogos_son_completos_y_genericos() {
        let reserva: std::collections::BTreeMap<&str, &str> = CATALOGOS[0].1.iter().copied().collect();
        assert!(reserva.len() > 1300);
        // las secciones tecnicas existen y son minoria: lo que ve la interfaz no esta en ellas
        let tecnicas = reserva.keys().filter(|k| k.split('.').next().is_some_and(|s| s.ends_with("_tec"))).count();
        assert!(tecnicas > 0 && tecnicas * 4 < reserva.len(), "{} claves tecnicas de {}", tecnicas, reserva.len());
        for (idioma, entradas) in CATALOGOS {
            let mapa: std::collections::BTreeMap<&str, &str> = entradas.iter().copied().collect();
            assert_eq!(mapa.len(), entradas.len(), "{}: hay claves repetidas", idioma);
            assert_eq!(mapa.keys().collect::<Vec<_>>(), reserva.keys().collect::<Vec<_>>(), "{}: no tiene las mismas claves", idioma);
            for (k, t) in entradas.iter() {
                assert_eq!(huecos(t), huecos(reserva[k]), "{}: {} cambia los huecos", idioma, k);
                assert!(!t.trim().is_empty(), "{}: {} vacia", idioma, k);
                let tecnica = k.split('.').next().is_some_and(|s| s.ends_with("_tec"));
                assert!(tecnica || prohibida_en(t).is_none(), "{}: {} nombra {:?}: {}", idioma, k, prohibida_en(t), t);
                assert!(prohibida_en(k).is_none(), "{}: la clave {} nombra {:?}", idioma, k, prohibida_en(k));
                assert!(k.split('.').count() == 2 && k.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.'), "clave mal formada: {}", k);
            }
        }
    }

    // -----------------------------------------------------------------------------------------------------------
    // textos visibles fuera del catalogo

    /// ¿Parece prosa para una persona? Dos palabras seguidas (solo letras, separadas por un espacio) con al menos 5 letras
    /// entre las dos: `"no se pudo"`, `"lectura y escritura"`. No lo son una clave (`share.uid`), una ruta, un argumento
    /// (`--adb tcp`), un formato sin palabras (`{}: {}`) ni una sola palabra.
    fn es_prosa(t: &str) -> bool {
        let palabras: Vec<&str> = t.split(' ').collect();
        palabras.windows(2).any(|w| {
            let letras = |p: &str| !p.is_empty() && p.chars().all(char::is_alphabetic);
            letras(w[0]) && letras(w[1]) && w[0].chars().count() + w[1].chars().count() >= 5
        })
    }

    /// Macros cuyo texto es para quien programa (fallos internos, comprobaciones) o que comparan en lugar de mostrar.
    const MACROS_INTERNAS: &[&str] = &["assert", "assert_eq", "assert_ne", "debug_assert", "debug_assert_eq", "unreachable", "panic", "todo", "unimplemented", "matches"];

    /// Funciones cuyo argumento no se muestra: comparan o buscan en un texto (el de otra herramienta), son ordenes para el
    /// invitado o para el equipo (`shell`, `arg`, `orden_terminal`...), nombres de ordenes de weft (`val`) o datos binarios.
    const FUNCIONES_INTERNAS: &[&str] = &[
        "expect", "contains", "starts_with", "ends_with", "find", "rfind", "split", "split_once", "strip_prefix", "strip_suffix", "replace", "trim_start_matches",
        "trim_end_matches", "shell", "shell_una_vez", "shell_texto", "sh", "orden_terminal", "arg", "args", "val", "copy_from_slice", "guid", "traza",
    ];

    /// Prefijos de los registros internos de depuracion (`eprintln!` a window.log y a la salida de errores de procesos de
    /// servicio): no son mensajes para la persona sino para diagnosticar.
    const REGISTROS: &[&str] = &["ventana: ", "gamepad: ", "gancho de pruebas: ", "QEMU respondio"];

    /// Primeras palabras de las ordenes de shell (del invitado o del equipo) que el codigo arma como texto.
    const ORDENES: &[&str] = &[
        "echo", "cmd", "test", "wm", "getprop", "setprop", "settings", "pm", "am", "mount", "cat", "ls", "rm", "mkdir", "cp", "chmod", "chcon", "dumpsys", "logcat",
        "md5sum", "stat", "su", "sh", "svc", "grep", "pidof", "for", "if", "[",
    ];

    /// ¿Es una orden de shell? Por su primera palabra o por llevar sintaxis de shell.
    fn parece_orden(t: &str) -> bool {
        ["$(", "&&", "||", "2>/dev/null", "; then", "; do", "| grep"].iter().any(|x| t.contains(x)) || ORDENES.contains(&t.split(' ').next().unwrap_or(""))
    }

    /// Marca en el fuente de un texto que no es para la persona y que las reglas no reconocen solas (contenido de archivos
    /// o de protocolos, textos del sistema en ingles que se buscan para traducirlos, registros de depuracion...). Va en un
    /// comentario con el motivo: al final de la linea del texto o en una linea propia antes de la sentencia o del elemento,
    /// y entonces vale hasta su final (`;`, `,` o la llave que lo cierra).
    const MARCA: &str = "texto-interno";

    /// Lineas (desde 1) que la marca `texto-interno` excluye en `src`, sobre las fichas de su codigo (`fichas`).
    fn lineas_marcadas(src: &str, f: &[(Tok, usize)]) -> std::collections::BTreeSet<usize> {
        let mut out = std::collections::BTreeSet::new();
        for (n, l) in src.lines().enumerate() {
            let Some(c) = l.find("//").filter(|c| l[*c..].contains(MARCA)) else { continue };
            let linea = n + 1;
            if !l[..c].trim().is_empty() {
                out.insert(linea);
                continue;
            }
            // linea propia: desde la siguiente ficha hasta el final de su sentencia o elemento
            let Some(ini) = f.iter().position(|(_, l)| *l > linea) else { continue };
            let mut prof = 0i32;
            for (t, l) in &f[ini..] {
                out.insert(*l);
                match t {
                    Tok::Punct('(' | '[' | '{') => prof += 1,
                    Tok::Punct(')' | ']' | '}') => {
                        prof -= 1;
                        if prof <= 0 && matches!(t, Tok::Punct('}')) {
                            break;
                        }
                        if prof < 0 {
                            break;
                        }
                    }
                    Tok::Punct(';' | ',') if prof == 0 => break,
                    _ => {}
                }
            }
        }
        out
    }

    /// La macro o la funcion cuyos parentesis encierran la ficha `i` (la mas cercana), si hay: ("tx", true) para `tx!(...)`,
    /// ("contains", false) para `.contains(...)`.
    fn llamada(f: &[(Tok, usize)], i: usize) -> Option<(&str, bool)> {
        let mut prof = 0;
        for j in (0..i).rev() {
            match &f[j].0 {
                Tok::Punct(')' | ']' | '}') => prof += 1,
                Tok::Punct('(' | '[' | '{') if prof > 0 => prof -= 1,
                Tok::Punct(_) if prof > 0 => {}
                Tok::Punct('(') => {
                    return match (f.get(j.wrapping_sub(1)).map(|x| &x.0), f.get(j.wrapping_sub(2)).map(|x| &x.0)) {
                        (Some(Tok::Punct('!')), Some(Tok::Ident(m))) => Some((m.as_str(), true)),
                        (Some(Tok::Ident(n)), _) => Some((n.as_str(), false)),
                        _ => None,
                    };
                }
                Tok::Punct('[' | '{') => return None,
                _ => {}
            }
        }
        None
    }

    /// Textos para la persona que quedan en el codigo de `src` (sin pruebas ni comentarios): (linea, texto).
    fn textos_fuera(src: &str) -> Vec<(usize, String)> {
        let codigo = sin_pruebas_ni_comentarios(src);
        let f = fichas(&codigo);
        let marcadas = lineas_marcadas(src, &f);
        let mut out = Vec::new();
        for (i, (t, linea)) in f.iter().enumerate() {
            let Tok::Cadena(c) = t else { continue };
            if !es_prosa(c) || parece_orden(c) || marcadas.contains(linea) {
                continue;
            }
            match llamada(&f, i) {
                Some((m, true)) if MACROS_INTERNAS.contains(&m) => continue,
                Some((m, true)) if (m == "eprintln" || m == "eprint") && REGISTROS.iter().any(|r| c.starts_with(r)) => continue,
                Some((n, false)) if FUNCIONES_INTERNAS.contains(&n) => continue,
                _ => {}
            }
            out.push((*linea, c.clone()));
        }
        out
    }

    /// Ningun texto para la persona queda fuera del catalogo: ni en la ventana ni en la linea de ordenes (ayuda, salida,
    /// avisos y errores de cada orden), ni en los errores de validacion de la configuracion y de las carpetas compartidas, ni
    /// en las dos ramas de `elige`.
    ///
    /// Criterio: en el codigo de `src/` (sin pruebas ni comentarios), una cadena que parece prosa (`es_prosa`: dos palabras
    /// seguidas) es un texto visible y debe estar en el catalogo, salvo que sea
    /// - el mensaje de un fallo interno o una comparacion (`MACROS_INTERNAS`, `FUNCIONES_INTERNAS`: `assert!`, `expect`,
    ///   `contains`...);
    /// - una orden para el invitado o para el equipo (`parece_orden`, o el argumento de `shell`, `arg`...), o el nombre de
    ///   una orden de weft (`val`, `orden_terminal`);
    /// - un registro interno de depuracion (`eprintln!` con un prefijo de `REGISTROS`);
    /// - un texto marcado con `// texto-interno: motivo` (contenido de archivos y de protocolos, textos del sistema en ingles
    ///   que se buscan para traducirlos, el gancho de pruebas de la ventana).
    ///
    /// Las palabras sueltas (`"detenida"`, `"compilacion: {}"`) no las ve esta prueba: las visibles se migraron a mano; las
    /// que quedan son identificadores, valores de configuracion o salida para programas (`clave=valor`).
    #[test]
    fn no_quedan_textos_visibles_fuera_del_catalogo() {
        let mut errores = Vec::new();
        let mut revisados = 0;
        for (nombre, src) in fuentes() {
            revisados += 1;
            for (linea, t) in textos_fuera(&src) {
                errores.push(format!("{}:{}: texto visible fuera del catalogo: {:?}", nombre, linea, t));
            }
        }
        assert!(revisados >= 40, "se revisaron pocos fuentes: {}", revisados);
        assert!(errores.is_empty(), "{} textos (pasarlos al catalogo con tx!/txf!/clave!, o marcarlos con `// {}: motivo` si no son para la persona):\n{}", errores.len(), MARCA, errores.join("\n"));
    }

    /// El detector distingue lo que se ve de lo que no.
    #[test]
    fn el_detector_de_textos_visibles() {
        let src = r#"
fn a() -> Result<(), String> {
    println!("{}", tx!("x.y"));
    eprintln!("aviso: no se pudo abrir {}", 1);
    let b = if x { "lectura y escritura" } else { "ro" };
    Err("--adb: se espera {}".into())
}
fn b() {
    assert!(x, "nunca pasa esto");
    if e.contains("no respondio a tiempo") {}
    shell("getprop sys.boot_completed");
    let c = "echo hola && echo adios";
    eprintln!("ventana: cuadro descartado");
    let d = "share.uid"; let e = "{}: {}"; let f = "detenida";
    let g = "cabecera de archivo"; // texto-interno: contenido de un archivo
    // texto-interno: tabla de textos del sistema
    const T: &[&str] = &[
        "No such file or directory",
        "Permission denied",
    ];
    let h = "esto si se ve";
}
#[cfg(test)]
mod tests {
    fn p() { let x = "texto de una prueba"; }
}
"#;
        let v: Vec<String> = textos_fuera(src).into_iter().map(|(l, t)| format!("{}:{}", l, t)).collect();
        assert_eq!(v, vec!["4:aviso: no se pudo abrir {}", "5:lectura y escritura", "6:--adb: se espera {}", "21:esto si se ve"]);
    }

    /// Codigo de un fuente de Rust sin comentarios y sin los elementos marcados con `#[cfg(test)]` (cada uno hasta su
    /// cierre: un bloque por equilibrio de llaves o un `;`/`,` fuera de parentesis; las llaves de cadenas, caracteres y
    /// comentarios no cuentan). Conserva los saltos de linea, asi que los numeros de linea siguen valiendo.
    pub(crate) fn sin_pruebas_ni_comentarios(src: &str) -> String {
        let c: Vec<char> = src.chars().collect();
        let marca: Vec<char> = "#[cfg(test)]".chars().collect();
        let ident = |i: usize| c.get(i).is_some_and(|x| x.is_alphanumeric() || *x == '_');
        // fin (exclusivo) del literal que empieza en `i` (cadena, cadena cruda o caracter), si empieza uno
        let literal = |i: usize| -> Option<usize> {
            let cerrar = |mut j: usize, hashes: usize, escapes: bool| -> usize {
                while j < c.len() {
                    if escapes && c[j] == '\\' {
                        j += 2;
                        continue;
                    }
                    if c[j] == '"' && (0..hashes).all(|k| c.get(j + 1 + k) == Some(&'#')) {
                        return j + 1 + hashes;
                    }
                    j += 1;
                }
                c.len()
            };
            match c[i] {
                '"' => Some(cerrar(i + 1, 0, true)),
                'r' if !ident(i.wrapping_sub(1)) || (i >= 1 && c[i - 1] == 'b' && !ident(i.wrapping_sub(2))) => {
                    let h = c[i + 1..].iter().take_while(|x| **x == '#').count();
                    (c.get(i + 1 + h) == Some(&'"')).then(|| cerrar(i + 2 + h, h, false))
                }
                '\'' if c.get(i + 1) == Some(&'\\') => c.get(i + 3..).and_then(|r| r.iter().position(|x| *x == '\'')).map(|p| i + 4 + p),
                '\'' if c.get(i + 2) == Some(&'\'') => Some(i + 3),
                _ => None,
            }
        };
        let mut out = String::new();
        // dentro de un elemento de prueba: profundidad de (), [] y {} desde su comienzo
        let mut saltando: Option<i32> = None;
        let mut i = 0;
        while i < c.len() {
            // comentarios: fuera (se conservan los saltos de linea)
            if c[i] == '/' && c.get(i + 1) == Some(&'/') {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            if c[i] == '/' && c.get(i + 1) == Some(&'*') {
                let mut nivel = 0;
                while i < c.len() {
                    if c[i] == '/' && c.get(i + 1) == Some(&'*') {
                        nivel += 1;
                        i += 2;
                    } else if c[i] == '*' && c.get(i + 1) == Some(&'/') {
                        nivel -= 1;
                        i += 2;
                        if nivel == 0 {
                            break;
                        }
                    } else {
                        if c[i] == '\n' {
                            out.push('\n');
                        }
                        i += 1;
                    }
                }
                continue;
            }
            if let Some(fin) = literal(i) {
                for x in &c[i..fin] {
                    if saltando.is_none() || *x == '\n' {
                        out.push(*x);
                    }
                }
                i = fin;
                continue;
            }
            if saltando.is_none() && c[i..].starts_with(&marca) {
                saltando = Some(0);
                i += marca.len();
                continue;
            }
            match saltando {
                None => out.push(c[i]),
                Some(d) => {
                    if c[i] == '\n' {
                        out.push('\n');
                    }
                    saltando = match c[i] {
                        '(' | '[' | '{' => Some(d + 1),
                        // fin del bloque del elemento
                        '}' if d == 1 => None,
                        // cierre de lo que contiene al elemento (un campo o una variante al final): se conserva
                        ')' | ']' | '}' if d == 0 => {
                            out.push(c[i]);
                            None
                        }
                        ')' | ']' | '}' => Some(d - 1),
                        ';' | ',' if d == 0 => None,
                        _ => Some(d),
                    };
                }
            }
            i += 1;
        }
        out
    }

    /// El recorte quita cada elemento de prueba entero (aunque lleve llaves en cadenas o caracteres) y deja todo lo que
    /// viene despues: una palabra prohibida tras el primer `#[cfg(test)]` se sigue encontrando.
    #[test]
    fn el_recorte_de_pruebas_no_se_come_el_resto_del_fuente() {
        let prohibida = concat!("mag", "isk");
        let src = format!(
            "fn a() -> &'static str {{ \"a\" }}\n#[cfg(test)]\npub fn b(x: [u8; 2]) {{ let s = \"}}\"; let c = '{{'; let r = r#\"}}\"#; }}\nfn c() {{ \"{p}\" }} // {p} en un comentario\n#[cfg(test)]\nuse algo::{{x, y}};\n/* {{ comentario */ struct S {{ a: u8, #[cfg(test)] b: u8, c: u8 }}\n#[cfg(test)]\nmod tests {{\n    fn d() {{ '\\''; \"{p}\"; }}\n}}\nfn e<'a>(x: &'a str) {{ \"{p}\" }}\n",
            p = prohibida
        );
        let r = sin_pruebas_ni_comentarios(&src);
        assert_eq!(r.lines().count(), src.lines().count(), "{}", r);
        // lo que sigue a cada elemento de prueba se conserva
        assert!(r.contains("fn a()") && r.contains("fn c()") && r.contains("struct S") && r.contains("a: u8,") && r.contains("c: u8 }") && r.contains("fn e<'a>"), "{}", r);
        // los elementos de prueba y los comentarios, no
        assert!(!r.contains("fn b") && !r.contains("use algo") && !r.contains("b: u8") && !r.contains("mod tests") && !r.contains("fn d") && !r.contains("comentario"), "{}", r);
        // la palabra aparece en `c` y en `e` (no en el comentario ni en la prueba `d`)
        let lineas: Vec<&str> = r.lines().filter(|l| crate::textos::prohibida_en(l).is_some()).collect();
        assert_eq!(lineas.len(), 2, "{:?}", lineas);
        assert!(lineas[0].contains("fn c()") && lineas[1].contains("fn e"), "{:?}", lineas);
        // el recorte antiguo (cortar en el primer `#[cfg(test)]`) no la habria visto
        let antiguo = src.split("#[cfg(test)]").next().unwrap();
        assert!(crate::textos::prohibida_en(antiguo).is_none());
        // y sin ninguna marca el codigo queda igual (sin comentarios)
        assert_eq!(sin_pruebas_ni_comentarios("fn f() { g(\"// no es comentario\") } // si\n"), "fn f() { g(\"// no es comentario\") } \n");
    }
}
