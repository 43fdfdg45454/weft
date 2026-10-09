//! Configuracion de weft: un archivo `config` en el directorio de estado, en formato `clave=valor` legible y
//! editable a mano, con comentarios. Lo lee y lo escribe la pantalla de configuracion de la ventana (cada cambio se
//! guarda al instante), la orden `weft config [get CLAVE | set CLAVE VALOR | list | reset]` y lo consultan `start`,
//! `resolution` y el guion.
//!
//! PRECEDENCIA: opcion de la linea de ordenes (o variable WEFT_* del guion) > este archivo > valor por defecto.
//!
//! Formato: una clave por linea; `#` abre un comentario. Se escriben sin comentar solo los valores que difieren del
//! defecto (los demas salen como `#clave=defecto`, para documentar y para que se pueda activar quitando el `#`). Un valor
//! no valido se ignora con un aviso y se usa el valor por defecto. Una clave desconocida se conserva y avisa.
//!
//! `WEFT_CONFIG=RUTA` cambia el archivo. Por defecto es `machines/<maquina>/config` del directorio de configuracion
//! estandar ($XDG_CONFIG_HOME/weft; ver rutas.rs), que sobrevive al cierre de la sesion (el estado de ejecucion no).
//!
//! Este modulo no toca SDL ni la maquina: modelo, validacion, lectura y escritura.

use crate::textos::tx;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use crate::textos::{clave, txf};

// ---------------------------------------------------------------------------------------------------------------
// tipos de clave

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tipo {
    /// `si` / `no`
    Bool,
    /// entero en [min, max]
    Entero(u32, u32),
    /// `auto` o un entero en [min, max]
    EnteroAuto(u32, u32),
    /// una de las palabras
    Enum(&'static [&'static str]),
    /// `ajustar` o un porcentaje de 10 a 800
    Zoom,
    /// una combinacion de teclas (`F1`, `Ctrl+B`) o `ninguno`
    Tecla,
    /// `auto` o ANCHOxALTO
    Resolucion,
    /// `perfil` o una densidad de 72 a 960
    Densidad,
    /// `ninguna` o `NOMBRE:rw|ro:/ruta` (carpeta compartida con Android, ver compartir.rs)
    Carpeta,
    /// `img` (usar userdata.img de la imagen) o un tamano `24G` / `4096M` (minimo 2 GiB, maximo 2048 GiB)
    Tamano,
    /// `auto` o el identificador de una imagen instalada (letras, numeros, `.`, `_`, `-`)
    IdImagen,
    /// `auto` o una ruta absoluta de carpeta
    Ruta,
    /// `ninguno` o el identificador de un perfil de dispositivo (ver dispositivo.rs)
    IdDispositivo,
}

pub struct Clave {
    pub nombre: &'static str,
    pub seccion: &'static str,
    pub tipo: Tipo,
    pub defecto: &'static str,
    /// clave de su ayuda en el catalogo de textos (el texto lo da `ayuda`)
    pub clave_ayuda: &'static str,
    /// solo se aplica al proximo arranque de la maquina
    pub arranque: bool,
}

impl Clave {
    /// La ayuda de la clave (comentario del archivo de configuracion), en el idioma de la interfaz.
    pub fn ayuda(&self) -> &'static str {
        crate::textos::texto(self.clave_ayuda)
    }
}

const fn c(nombre: &'static str, seccion: &'static str, tipo: Tipo, defecto: &'static str, ayuda: &'static str) -> Clave {
    Clave { nombre, seccion, tipo, defecto, clave_ayuda: ayuda, arranque: false }
}

const fn ca(nombre: &'static str, seccion: &'static str, tipo: Tipo, defecto: &'static str, ayuda: &'static str) -> Clave {
    Clave { nombre, seccion, tipo, defecto, clave_ayuda: ayuda, arranque: true }
}

pub const TIPOS_MAQUINA: &[&str] = &["pc", "q35"];
/// Intenciones (auto, hardware, software) y, como valores avanzados para fijar el motor a mano, sus nombres concretos.
/// `none` (sin aceleracion) se normaliza a `software`.
pub const GPUS: &[&str] = &["auto", "hardware", "software", "gfxstream", "virtio-pci"];
pub const MANDOS: &[&str] = &["none", "auto"];
pub const MODIFICADORES: &[&str] = &["ctrl", "alt"];
pub const TRANSPORTES_ADB: &[&str] = &["auto", "vsock", "tcp", "both"];
/// Lo que hacen los botones derecho y central del raton con el puntero tactil (`none`: nada; ver gestos::boton_tactil).
pub const BOTONES_RATON: &[&str] = &["atras", "inicio", "recientes", "menu", "none"];
/// Tema de la ventana (ver formas::tema::Variante::elegir).
pub const TEMAS: &[&str] = &["auto", "claro", "oscuro"];
/// Tamano del texto de la ventana, en % (ver formas::tamano).
pub const TAMANOS_TEXTO: &[&str] = &["100", "125", "150", "200"];

/// Todas las claves, en el orden del archivo.
pub const CLAVES: &[Clave] = &[
    c("zoom", "General", Tipo::Zoom, "ajustar", clave!("config.ayuda_zoom")),
    c("ventana.tema", "General", Tipo::Enum(TEMAS), "auto", clave!("config.ayuda_ventana_tema")),
    c("ventana.texto", "General", Tipo::Enum(TAMANOS_TEXTO), "100", clave!("config.ayuda_ventana_texto")),
    c("confirmar", "General", Tipo::Bool, "si", clave!("config.ayuda_confirmar")),
    c("atajo.atras", "Atajos", Tipo::Tecla, "F1", clave!("config.ayuda_atajo_atras")),
    c("atajo.inicio", "Atajos", Tipo::Tecla, "F2", "Inicio."),
    c("atajo.recientes", "Atajos", Tipo::Tecla, "F3", clave!("config.ayuda_atajo_recientes")),
    c("atajo.vol_menos", "Atajos", Tipo::Tecla, "F5", clave!("config.ayuda_atajo_vol_menos")),
    c("atajo.vol_mas", "Atajos", Tipo::Tecla, "F6", clave!("config.ayuda_atajo_vol_mas")),
    c("atajo.rotar", "Atajos", Tipo::Tecla, "F7", clave!("config.ayuda_atajo_rotar")),
    c("atajo.captura", "Atajos", Tipo::Tecla, "F8", clave!("config.ayuda_atajo_captura")),
    c("atajo.configuracion", "Atajos", Tipo::Tecla, "F9", clave!("config.ayuda_atajo_configuracion")),
    c("atajo.zoom_mas", "Atajos", Tipo::Tecla, "Ctrl+=", clave!("config.ayuda_atajo_zoom_mas")),
    c("atajo.zoom_menos", "Atajos", Tipo::Tecla, "Ctrl+-", clave!("config.ayuda_atajo_zoom_menos")),
    c("atajo.zoom_ajustar", "Atajos", Tipo::Tecla, "Ctrl+0", clave!("config.ayuda_atajo_zoom_ajustar")),
    c("atajo.pasar_tecla", "Atajos", Tipo::Tecla, "Ctrl+Alt+F", clave!("config.ayuda_atajo_pasar_tecla")),
    c("atajo.pantalla_completa", "Atajos", Tipo::Tecla, "F11", clave!("config.ayuda_atajo_pantalla_completa")),
    c("atajo.encendido", "Atajos", Tipo::Tecla, "ninguno", clave!("config.ayuda_atajo_encendido")),
    c("atajo.menu", "Atajos", Tipo::Tecla, "ninguno", clave!("config.ayuda_atajo_menu")),
    c("atajo.barra", "Atajos", Tipo::Tecla, "F10", clave!("config.ayuda_atajo_barra")),
    c("atajos.desactivados", "Atajos", Tipo::Bool, "no", clave!("config.ayuda_atajos_desactivados")),
    c("rueda.paso", "Entrada", Tipo::Entero(5, 25), "10", clave!("config.ayuda_rueda_paso")),
    c("rueda.tope", "Entrada", Tipo::Entero(20, 100), "60", clave!("config.ayuda_rueda_tope")),
    c("rueda.invertir", "Entrada", Tipo::Bool, "no", clave!("config.ayuda_rueda_invertir")),
    c("pellizco.modificador", "Entrada", Tipo::Enum(MODIFICADORES), "ctrl", clave!("config.ayuda_pellizco_modificador")),
    c("raton.derecho", "Entrada", Tipo::Enum(BOTONES_RATON), "atras", clave!("config.ayuda_raton_derecho")),
    c("raton.central", "Entrada", Tipo::Enum(BOTONES_RATON), "inicio", clave!("config.ayuda_raton_central")),
    c("gamepad", "Entrada", Tipo::Enum(MANDOS), "none", clave!("config.ayuda_gamepad")),
    c("pantalla.resolucion", "Pantalla", Tipo::Resolucion, "auto", clave!("config.ayuda_pantalla_resolucion")),
    c("pantalla.densidad", "Pantalla", Tipo::Densidad, "perfil", clave!("config.ayuda_pantalla_densidad")),
    c("pantalla.giro_android", "Pantalla", Tipo::Bool, "si", clave!("config.ayuda_pantalla_giro_android")),
    ca("maquina.cpus", "Maquina", Tipo::EnteroAuto(1, 128), "auto", clave!("config.ayuda_maquina_cpus")),
    ca("maquina.ram", "Maquina", Tipo::EnteroAuto(512, 1048576), "auto", clave!("config.ayuda_maquina_ram")),
    ca("maquina.tipo", "Maquina", Tipo::Enum(TIPOS_MAQUINA), "q35", clave!("config.ayuda_maquina_tipo")),
    ca("maquina.gpu", "Maquina", Tipo::Enum(GPUS), "auto", clave!("config.ayuda_maquina_gpu")),
    ca("dispositivo.perfil", "Maquina", Tipo::IdDispositivo, "ninguno", clave!("config.ayuda_dispositivo_perfil")),
    c("root.cargar_al_inicio", "Root", Tipo::Bool, "no", clave!("config.ayuda_root_cargar_al_inicio")),
    c("root.instalar_automaticamente", "Root", Tipo::Bool, "no", clave!("config.ayuda_root_instalar_automaticamente")),
    c("root.carpeta", "Root", Tipo::Ruta, "auto", clave!("config.ayuda_root_carpeta")),
    ca("share.0", "Compartir", Tipo::Carpeta, "ninguna", clave!("config.ayuda_share_0")),
    ca("share.1", "Compartir", Tipo::Carpeta, "ninguna", clave!("config.ayuda_share_1")),
    ca("share.2", "Compartir", Tipo::Carpeta, "ninguna", clave!("config.ayuda_share_2")),
    ca("share.3", "Compartir", Tipo::Carpeta, "ninguna", clave!("config.ayuda_share_3")),
    ca("share.uid", "Compartir", Tipo::EnteroAuto(1000, 2000000000), "auto", clave!("config.ayuda_share_uid")),
    ca("disk.data", "Imagen", Tipo::Tamano, "24G", clave!("config.ayuda_disk_data")),
    c("disk.cow", "Imagen", Tipo::Bool, "no", clave!("config.ayuda_disk_cow")),
    ca("image.id", "Imagen", Tipo::IdImagen, "auto", clave!("config.ayuda_image_id")),
    ca("adb.transporte", "Imagen", Tipo::Enum(TRANSPORTES_ADB), "auto", clave!("config.ayuda_adb_transporte")),
    ca("adb.puerto", "Imagen", Tipo::EnteroAuto(1024, 65535), "auto", clave!("config.ayuda_adb_puerto")),
    ca("dir.datos", "Imagen", Tipo::Ruta, "auto", clave!("config.ayuda_dir_datos")),
    ca("dir.cache", "Imagen", Tipo::Ruta, "auto", clave!("config.ayuda_dir_cache")),
    c("android.bluetooth", "Android", Tipo::Bool, "no", clave!("config.ayuda_android_bluetooth")),
];

pub fn clave(nombre: &str) -> Option<&'static Clave> {
    CLAVES.iter().find(|k| k.nombre == nombre)
}

// ---------------------------------------------------------------------------------------------------------------
// teclas y atajos

/// Acciones que se pueden asignar a una tecla.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AccionAtajo {
    Atras,
    Inicio,
    Recientes,
    VolMenos,
    VolMas,
    Rotar,
    Captura,
    Configuracion,
    ZoomMas,
    ZoomMenos,
    ZoomAjustar,
}

impl AccionAtajo {
    pub const TODAS: [AccionAtajo; 11] = [
        AccionAtajo::Atras,
        AccionAtajo::Inicio,
        AccionAtajo::Recientes,
        AccionAtajo::VolMenos,
        AccionAtajo::VolMas,
        AccionAtajo::Rotar,
        AccionAtajo::Captura,
        AccionAtajo::Configuracion,
        AccionAtajo::ZoomMas,
        AccionAtajo::ZoomMenos,
        AccionAtajo::ZoomAjustar,
    ];

    pub fn clave(self) -> &'static str {
        match self {
            AccionAtajo::Atras => "atajo.atras",
            AccionAtajo::Inicio => "atajo.inicio",
            AccionAtajo::Recientes => "atajo.recientes",
            AccionAtajo::VolMenos => "atajo.vol_menos",
            AccionAtajo::VolMas => "atajo.vol_mas",
            AccionAtajo::Rotar => "atajo.rotar",
            AccionAtajo::Captura => "atajo.captura",
            AccionAtajo::Configuracion => "atajo.configuracion",
            AccionAtajo::ZoomMas => "atajo.zoom_mas",
            AccionAtajo::ZoomMenos => "atajo.zoom_menos",
            AccionAtajo::ZoomAjustar => "atajo.zoom_ajustar",
        }
    }

    pub fn de_clave(k: &str) -> Option<AccionAtajo> {
        AccionAtajo::TODAS.into_iter().find(|a| a.clave() == k)
    }

    pub fn etiqueta(self) -> &'static str {
        match self {
            AccionAtajo::Atras => tx!("atajo.atras"),
            AccionAtajo::Inicio => tx!("atajo.inicio"),
            AccionAtajo::Recientes => tx!("atajo.recientes"),
            AccionAtajo::VolMenos => tx!("atajo.vol_menos"),
            AccionAtajo::VolMas => tx!("atajo.vol_mas"),
            AccionAtajo::Rotar => tx!("atajo.rotar"),
            AccionAtajo::Captura => tx!("atajo.captura"),
            AccionAtajo::Configuracion => tx!("atajo.configuracion"),
            AccionAtajo::ZoomMas => tx!("atajo.zoom_mas"),
            AccionAtajo::ZoomMenos => tx!("atajo.zoom_menos"),
            AccionAtajo::ZoomAjustar => tx!("atajo.zoom_ajustar"),
        }
    }
}

/// Una tecla con modificadores. `sc` es el codigo de tecla de SDL (uso HID de USB).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Combo {
    pub sc: u32,
    pub ctrl: bool,
    pub alt: bool,
    pub mayus: bool,
}

/// Teclas que se pueden asignar: (codigo, nombre del archivo, nombre para mostrar). No incluye Esc (cierra los dialogos),
/// Retroceso (borra en la captura) ni Tab (recorre los controles).
const TECLAS: &[(u32, &str, &str)] = &[
    (4, "A", "A"),
    (5, "B", "B"),
    (6, "C", "C"),
    (7, "D", "D"),
    (8, "E", "E"),
    (9, "F", "F"),
    (10, "G", "G"),
    (11, "H", "H"),
    (12, "I", "I"),
    (13, "J", "J"),
    (14, "K", "K"),
    (15, "L", "L"),
    (16, "M", "M"),
    (17, "N", "N"),
    (18, "O", "O"),
    (19, "P", "P"),
    (20, "Q", "Q"),
    (21, "R", "R"),
    (22, "S", "S"),
    (23, "T", "T"),
    (24, "U", "U"),
    (25, "V", "V"),
    (26, "W", "W"),
    (27, "X", "X"),
    (28, "Y", "Y"),
    (29, "Z", "Z"),
    (30, "1", "1"),
    (31, "2", "2"),
    (32, "3", "3"),
    (33, "4", "4"),
    (34, "5", "5"),
    (35, "6", "6"),
    (36, "7", "7"),
    (37, "8", "8"),
    (38, "9", "9"),
    (39, "0", "0"),
    (40, "Intro", "Intro"),
    (44, "Espacio", "Espacio"),
    (45, "-", "-"),
    (46, "=", "="),
    (47, "[", "["),
    (48, "]", "]"),
    (49, "\\", "\\"),
    (51, ";", ";"),
    (52, "'", "'"),
    (53, "`", "`"),
    (54, ",", ","),
    (55, ".", "."),
    (56, "/", "/"),
    (58, "F1", "F1"),
    (59, "F2", "F2"),
    (60, "F3", "F3"),
    (61, "F4", "F4"),
    (62, "F5", "F5"),
    (63, "F6", "F6"),
    (64, "F7", "F7"),
    (65, "F8", "F8"),
    (66, "F9", "F9"),
    (67, "F10", "F10"),
    (68, "F11", "F11"),
    (69, "F12", "F12"),
    (73, "Ins", "Ins"),
    (74, "Inicio", "Inicio"),
    (75, "RePag", "RePág"),
    (76, "Supr", "Supr"),
    (77, "Fin", "Fin"),
    (78, "AvPag", "AvPág"),
    (79, "Derecha", "Derecha"),
    (80, "Izquierda", "Izquierda"),
    (81, "Abajo", "Abajo"),
    (82, "Arriba", "Arriba"),
    (104, "F13", "F13"),
    (105, "F14", "F14"),
    (106, "F15", "F15"),
    (107, "F16", "F16"),
    (108, "F17", "F17"),
    (109, "F18", "F18"),
    (110, "F19", "F19"),
    (111, "F20", "F20"),
    (112, "F21", "F21"),
    (113, "F22", "F22"),
    (114, "F23", "F23"),
    (115, "F24", "F24"),
];

/// ¿Una tecla sola (sin Ctrl ni Alt) puede ser un atajo? Solo las que no escriben texto en el invitado.
fn sin_modificador_ok(sc: u32) -> bool {
    matches!(sc, 58..=69 | 104..=115 | 73..=82)
}

/// Minusculas y sin tildes, para comparar nombres.
fn plano(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'á' | 'Á' => 'a',
            'é' | 'É' => 'e',
            'í' | 'Í' => 'i',
            'ó' | 'Ó' => 'o',
            'ú' | 'Ú' => 'u',
            c => c.to_ascii_lowercase(),
        })
        .collect()
}

impl Combo {
    #[cfg(test)]
    pub fn tecla(sc: u32) -> Combo {
        Combo { sc, ctrl: false, alt: false, mayus: false }
    }

    /// ¿Se puede asignar esta combinacion? Err con el motivo.
    pub fn valida(&self) -> Result<(), String> {
        if !TECLAS.iter().any(|t| t.0 == self.sc) {
            return Err(tx!("config.esa_tecla_no_se_puede_usar_como_atajo").into());
        }
        if !self.ctrl && !self.alt && !sin_modificador_ok(self.sc) {
            return Err(tx!("config.sin_ctrl_ni_alt_solo_valen_las_teclas_f1").into());
        }
        Ok(())
    }

    /// Texto del archivo: `Ctrl+Alt+Mayus+F5` (nombres sin tildes).
    pub fn texto(&self) -> String {
        self.nombrar(false)
    }

    /// Texto para mostrar en la interfaz: `Ctrl+Mayús+RePág`.
    pub fn visible(&self) -> String {
        self.nombrar(true)
    }

    fn nombrar(&self, visible: bool) -> String {
        let mut s = String::new();
        if self.ctrl {
            s.push_str("Ctrl+");
        }
        if self.alt {
            s.push_str("Alt+");
        }
        if self.mayus {
            s.push_str(if visible { "Mayús+" } else { "Mayus+" });
        }
        let t = TECLAS.iter().find(|t| t.0 == self.sc);
        s.push_str(match (t, visible) {
            (Some(t), true) => t.2,
            (Some(t), false) => t.1,
            (None, _) => "?",
        });
        s
    }

    /// `F5`, `Ctrl+B`, `ctrl+alt+mayus+f1`, `Ctrl++`... (el ultimo trozo es la tecla; `+` y `-` se escriben tal cual).
    pub fn parse(t: &str) -> Result<Combo, String> {
        let t = t.trim();
        if t.is_empty() {
            return Err(tx!("config.combinacion_vacia").into());
        }
        let mut c = Combo { sc: 0, ctrl: false, alt: false, mayus: false };
        // la tecla es lo que queda tras el ultimo `+` que separa un modificador; "Ctrl++" = Ctrl y la tecla +
        let mut resto = t;
        while let Some(i) = resto.find('+').filter(|i| *i > 0) {
            let (mods, tras) = resto.split_at(i);
            match plano(mods.trim()).as_str() {
                "ctrl" | "control" => c.ctrl = true,
                "alt" => c.alt = true,
                "mayus" | "shift" | "mayusculas" => c.mayus = true,
                otro => return Err(txf!("config.modificador_desconocido_ctrl_alt_mayus", format!("{:?}", otro))),
            }
            resto = &tras[1..];
            if resto.is_empty() {
                return Err(tx!("config.falta_la_tecla_tras_el").into());
            }
        }
        let nombre = plano(resto.trim());
        // la tecla "+" no esta en la tabla (es Mayus+=): se admite el signo = y -
        c.sc = TECLAS.iter().find(|t| plano(t.1) == nombre || plano(t.2) == nombre).map(|t| t.0).ok_or_else(|| txf!("config.tecla_desconocida", format!("{:?}", resto.trim())))?;
        c.valida()?;
        Ok(c)
    }

    /// ¿Coincide con una pulsacion (modificadores exactos)?
    pub fn coincide(&self, sc: u32, ctrl: bool, alt: bool, mayus: bool) -> bool {
        self.sc == sc && self.ctrl == ctrl && self.alt == alt && self.mayus == mayus
    }
}

/// Nombre visible de un valor de atajo del archivo (`ninguno` = "Sin asignar").
pub fn tecla_visible(valor: &str) -> String {
    match Combo::parse(valor) {
        Ok(c) => c.visible(),
        Err(_) => tx!("atajos.sin_asignar").to_string(),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// validacion de valores

fn es_si(t: &str) -> Option<bool> {
    match plano(t.trim()).as_str() {
        "si" | "1" | "true" | "yes" | "on" | "activado" | "verdadero" => Some(true),
        "no" | "0" | "false" | "off" | "desactivado" | "falso" => Some(false),
        _ => None,
    }
}

fn es_auto(t: &str) -> bool {
    matches!(plano(t.trim()).as_str(), "auto" | "automatico" | "")
}

/// Valida y normaliza un valor para el tipo. Ok(valor canonico) o Err(motivo).
pub fn validar(tipo: Tipo, valor: &str) -> Result<String, String> {
    let v = valor.trim();
    match tipo {
        Tipo::Bool => es_si(v).map(|b| if b { "si" } else { "no" }.to_string()).ok_or_else(|| txf!("config.se_espera_si_o_no", format!("{:?}", v))),
        Tipo::Entero(min, max) => {
            let n: u32 = v.parse().map_err(|_| txf!("config.se_espera_un_numero_entero_de_a", format!("{:?}", v), min, max))?;
            if !(min..=max).contains(&n) {
                return Err(txf!("config.fuera_de_rango_a", n, min, max));
            }
            Ok(n.to_string())
        }
        Tipo::EnteroAuto(min, max) => {
            if es_auto(v) {
                return Ok("auto".into());
            }
            validar(Tipo::Entero(min, max), v)
        }
        Tipo::Enum(opciones) => {
            let p = plano(v);
            // acepta "automatico" por "auto" y "ninguno" por "none"
            let p = match p.as_str() {
                "automatico" => "auto".to_string(),
                "ninguno" | "ninguna" => "none".to_string(),
                _ => p,
            };
            // none = sin aceleracion = software
            let p = if p == "none" && opciones.contains(&"software") && !opciones.contains(&"none") { "software".to_string() } else { p };
            opciones.iter().find(|o| **o == p).map(|o| o.to_string()).ok_or_else(|| txf!("config.se_espera", format!("{:?}", v), opciones.join(", ")))
        }
        Tipo::Zoom => {
            if matches!(plano(v).as_str(), "ajustar" | "ajuste" | "auto") {
                return Ok("ajustar".into());
            }
            let n: u32 = v.trim_end_matches('%').trim().parse().map_err(|_| txf!("config.se_espera_ajustar_o_un_porcentaje_de_10", format!("{:?}", v)))?;
            if !(10..=800).contains(&n) {
                return Err(txf!("config.el_zoom_va_de_10_a_800", n));
            }
            Ok(n.to_string())
        }
        Tipo::Tecla => {
            if matches!(plano(v).as_str(), "ninguno" | "ninguna" | "-" | "" | "sin asignar") {
                return Ok("ninguno".into());
            }
            Combo::parse(v).map(|c| c.texto())
        }
        Tipo::Resolucion => {
            if es_auto(v) {
                return Ok("auto".into());
            }
            if v.contains('@') {
                return Err(tx!("config.la_densidad_va_aparte_pantalla_densidad").into());
            }
            let r = crate::pantalla::parse_resolucion(v)?;
            Ok(format!("{}x{}", r.w, r.h))
        }
        Tipo::IdImagen => {
            if es_auto(v) {
                return Ok("auto".into());
            }
            crate::perfil::validar_id(v).map(|_| v.to_string())
        }
        Tipo::Ruta => {
            if es_auto(v) {
                return Ok("auto".into());
            }
            if !v.starts_with('/') || v.contains(['\n', '\r']) {
                return Err(txf!("config.se_espera_auto_o_una_ruta_absoluta", format!("{:?}", v)));
            }
            Ok(crate::rutas::normalizar(Path::new(v)).to_string_lossy().into_owned())
        }
        Tipo::IdDispositivo => {
            if matches!(plano(v).as_str(), "ninguno" | "") {
                return Ok(crate::dispositivo::NINGUNO.into());
            }
            crate::dispositivo::validar_id(v).map(|_| v.to_string())
        }
        Tipo::Carpeta => crate::compartir::normalizar(v),
        Tipo::Tamano => crate::imagen::normalizar_datos(v),
        Tipo::Densidad => {
            if matches!(plano(v).as_str(), "perfil" | "auto" | "") {
                return Ok("perfil".into());
            }
            let n: u32 = v.parse().map_err(|_| txf!("config.se_espera_perfil_o_una_densidad_de_72_a", format!("{:?}", v)))?;
            if !(72..=960).contains(&n) {
                return Err(txf!("config.la_densidad_va_de_72_a_960", n));
            }
            Ok(n.to_string())
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// cerrojo del archivo

extern "C" {
    fn flock(fd: i32, operacion: i32) -> i32;
}
const LOCK_EX: i32 = 2;
const LOCK_NB: i32 = 4;
/// Lo que se espera como mucho a que otro proceso suelte el cerrojo (lo tiene lo que tarda en leer y escribir el archivo).
const ESPERA_CERROJO: std::time::Duration = std::time::Duration::from_secs(5);

/// `ruta` con `sufijo` pegado al nombre (`config` -> `config.lock`).
fn con_sufijo(ruta: &Path, sufijo: &str) -> PathBuf {
    let mut s = ruta.as_os_str().to_owned();
    s.push(sufijo);
    PathBuf::from(s)
}

/// Toma el cerrojo exclusivo (flock) de `ruta`, creandola si hace falta. Se suelta al soltar el archivo devuelto o al
/// terminar el proceso, aunque muera de golpe. Si otro lo tiene mas de ESPERA_CERROJO, error (no se cuelga la ventana).
fn cerrojo(ruta: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::io::AsRawFd;
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(ruta)?;
    let t0 = std::time::Instant::now();
    loop {
        if unsafe { flock(f.as_raw_fd(), LOCK_EX | LOCK_NB) } == 0 {
            return Ok(f);
        }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::WouldBlock && e.kind() != std::io::ErrorKind::Interrupted {
            return Err(e);
        }
        if t0.elapsed() >= ESPERA_CERROJO {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, txf!("config.otro_proceso_tiene_la_configuracion", ruta.display())));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

// ---------------------------------------------------------------------------------------------------------------
// la configuracion

#[derive(Clone, Debug, Default)]
pub struct Config {
    /// valores validados (solo los que se pusieron: lo demas es el defecto)
    vals: BTreeMap<String, String>,
    /// lineas con claves desconocidas, tal cual, para conservarlas al reescribir
    extras: Vec<String>,
    /// avisos de la ultima lectura (valores no validos, claves desconocidas, conflictos)
    pub avisos: Vec<String>,
    /// claves puestas o quitadas desde la lectura (`set`, `reset`...): `guardar` aplica solo estas sobre lo que haya en
    /// el archivo en ese momento, para no pisar lo que otro proceso escribio mientras tanto
    cambios: BTreeSet<String>,
    /// `reset(None)`: se restablecio todo, y `guardar` escribe esta configuracion entera
    todo: bool,
}

impl PartialEq for Config {
    fn eq(&self, o: &Config) -> bool {
        CLAVES.iter().all(|k| self.get(k.nombre) == o.get(k.nombre))
    }
}

impl Config {
    #[cfg(test)]
    pub fn nueva() -> Config {
        Config::default()
    }

    /// Lee el texto de un archivo `config`; lo no valido se ignora y queda en `avisos`.
    pub fn parse(texto: &str) -> Config {
        let mut c = Config::default();
        for (n, l) in texto.lines().enumerate() {
            let t = l.trim();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            let Some((k, v)) = t.split_once('=') else {
                c.avisos.push(txf!("config.config_linea_sin", n + 1, format!("{:?}", t)));
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            match clave(k) {
                None => {
                    c.avisos.push(txf!("config.config_clave_desconocida_linea_se", format!("{:?}", k), n + 1));
                    c.extras.push(t.to_string());
                }
                Some(def) => match validar(def.tipo, v) {
                    Ok(norm) => {
                        c.vals.insert(k.to_string(), norm);
                    }
                    Err(e) => c.avisos.push(txf!("config.config_linea_se_usa", k, v, e, n + 1, def.defecto)),
                },
            }
        }
        c.resolver_conflictos();
        c
    }

    /// Valor efectivo (el puesto o el defecto) de una clave conocida.
    pub fn get(&self, k: &str) -> String {
        match self.vals.get(k) {
            Some(v) => v.clone(),
            None => clave(k).map_or(String::new(), |d| d.defecto.to_string()),
        }
    }

    /// ¿Se puso explicitamente (distinto del defecto)?
    pub fn es_explicita(&self, k: &str) -> bool {
        self.vals.get(k).is_some_and(|v| Some(v.as_str()) != clave(k).map(|d| d.defecto))
    }

    pub fn bool(&self, k: &str) -> bool {
        self.get(k) == "si"
    }

    /// Entero de una clave Entero o EnteroAuto (None si es auto).
    pub fn entero(&self, k: &str) -> Option<u32> {
        self.get(k).parse().ok()
    }

    /// Pone un valor validandolo. Ok(valor canonico). Las teclas no pueden repetirse: Err con quien la usa.
    pub fn set(&mut self, k: &str, valor: &str) -> Result<String, String> {
        let def = clave(k).ok_or_else(|| txf!("config.clave_desconocida_config_list", k))?;
        let norm = validar(def.tipo, valor).map_err(|e| format!("{}: {}", k, e))?;
        if def.tipo == Tipo::Tecla && norm != "ninguno" {
            if let Some(otra) = self.usa_la_tecla(&norm, AccionAtajo::de_clave(k)) {
                return Err(txf!("config.ya_la_usa_2", k, tecla_visible(&norm), otra.etiqueta()));
            }
            // y los atajos propios de la ventana (pasar tecla, pantalla completa...), que no son una AccionAtajo
            if let Some(otra) = CLAVES.iter().find(|o| o.nombre != k && o.tipo == Tipo::Tecla && AccionAtajo::de_clave(o.nombre).is_none() && self.get(o.nombre) == norm) {
                return Err(txf!("config.ya_la_usa", k, tecla_visible(&norm), otra.nombre));
            }
        }
        self.vals.insert(k.to_string(), norm.clone());
        self.cambios.insert(k.to_string());
        Ok(norm)
    }

    /// Quita una clave (vuelve al defecto) o todas.
    pub fn reset(&mut self, k: Option<&str>) {
        match k {
            Some(k) => {
                self.vals.remove(k);
                self.cambios.insert(k.to_string());
            }
            None => {
                self.vals.clear();
                self.extras.clear();
                self.todo = true;
            }
        }
    }

    /// Accion (distinta de `salvo`) que tiene asignada esa combinacion.
    pub fn usa_la_tecla(&self, combo: &str, salvo: Option<AccionAtajo>) -> Option<AccionAtajo> {
        AccionAtajo::TODAS.into_iter().filter(|a| Some(*a) != salvo).find(|a| self.get(a.clave()) == combo)
    }

    /// Combinacion asignada a una accion (None si esta sin asignar).
    pub fn combo(&self, a: AccionAtajo) -> Option<Combo> {
        Combo::parse(&self.get(a.clave())).ok()
    }

    /// Accion que dispara una pulsacion (modificadores exactos).
    pub fn atajo_para(&self, sc: u32, ctrl: bool, alt: bool, mayus: bool) -> Option<AccionAtajo> {
        AccionAtajo::TODAS.into_iter().find(|a| self.combo(*a).is_some_and(|c| c.coincide(sc, ctrl, alt, mayus)))
    }

    /// Restaura todas las teclas de atajo.
    pub fn restaurar_atajos(&mut self) {
        for a in AccionAtajo::TODAS {
            self.vals.remove(a.clave());
            self.cambios.insert(a.clave().to_string());
        }
    }

    /// Si dos acciones comparten tecla (a mano en el archivo), gana la que se puso explicitamente (en orden del archivo)
    /// y las otras quedan sin asignar con un aviso.
    fn resolver_conflictos(&mut self) {
        let mut dueno: BTreeMap<String, AccionAtajo> = BTreeMap::new();
        // primero las explicitas (distintas del defecto), despues las que valen su defecto
        let orden: Vec<AccionAtajo> = AccionAtajo::TODAS.into_iter().filter(|a| self.es_explicita(a.clave())).chain(AccionAtajo::TODAS.into_iter().filter(|a| !self.es_explicita(a.clave()))).collect();
        for a in orden {
            let v = self.get(a.clave());
            if v == "ninguno" {
                continue;
            }
            match dueno.get(&v) {
                None => {
                    dueno.insert(v, a);
                }
                Some(otra) => {
                    self.avisos.push(txf!("config.config_ya_la_usa_queda_sin_asignar", tecla_visible(&v), otra.etiqueta(), a.etiqueta()));
                    self.vals.insert(a.clave().to_string(), "ninguno".into());
                }
            }
        }
    }

    /// Contenido del archivo: encabezado, y por seccion cada clave con su ayuda (`#clave=defecto` si vale el defecto).
    pub fn texto(&self) -> String {
        let mut s = String::from(
            tx!("config.configuracion_clave_valor_una_por_linea"),
        );
        let mut seccion = "";
        for k in CLAVES {
            if k.seccion != seccion {
                seccion = k.seccion;
                s.push_str(&format!("\n# --- {} ---\n", if seccion == "Maquina" { "Máquina" } else { seccion }));
            }
            s.push_str(&format!("# {}\n", k.ayuda()));
            let v = self.get(k.nombre);
            if self.es_explicita(k.nombre) {
                s.push_str(&format!("{}={}\n", k.nombre, v));
            } else {
                s.push_str(&format!("#{}={}\n", k.nombre, k.defecto));
            }
        }
        if !self.extras.is_empty() {
            s.push_str(tx!("config.claves_desconocidas_se_conservan"));
            for l in &self.extras {
                s.push_str(l);
                s.push('\n');
            }
        }
        s
    }

    /// Ruta del archivo de configuracion para un directorio de estado (WEFT_CONFIG manda).
    pub fn ruta(dir: &Path) -> PathBuf {
        match std::env::var("WEFT_CONFIG") {
            Ok(r) if !r.is_empty() => PathBuf::from(r),
            _ => {
                // `dir` es el estado de una maquina (<ejecucion>/<maquina>): su configuracion vive en el directorio de
                // configuracion estandar (ver rutas.rs). Cualquier otra carpeta es su propia carpeta de configuracion.
                let r = crate::rutas::actual();
                if dir.parent() == Some(r.ejecucion.as_path()) {
                    r.config_maquina(&crate::rutas::maquina_de(dir))
                } else {
                    dir.join("config")
                }
            }
        }
    }

    /// Lee la configuracion del estado (por defecto si no existe).
    pub fn cargar(dir: &Path) -> Config {
        std::fs::read_to_string(Config::ruta(dir)).map(|t| Config::parse(&t)).unwrap_or_default()
    }

    /// Guarda los cambios de esta configuracion (las claves puestas o quitadas desde que se leyo) sobre lo que haya ahora
    /// en el archivo: la ventana, `weft config`, `root`, `share` e `image` escriben el mismo archivo, y cada uno parte de
    /// lo que leyo antes. Todo (leer, aplicar y escribir) va con el cerrojo del archivo tomado (`<archivo>.lock`, ver
    /// `cerrojo`), y la escritura es atomica: un temporal propio de este proceso (`<archivo>.tmp.<pid>`) que se renombra.
    /// Tras `reset(None)` se escribe esta configuracion entera.
    pub fn guardar(&self, dir: &Path) -> std::io::Result<()> {
        let ruta = Config::ruta(dir);
        if let Some(p) = ruta.parent() {
            std::fs::create_dir_all(p)?;
        }
        let _cerrojo = cerrojo(&con_sufijo(&ruta, ".lock"))?;
        let texto = if self.todo {
            self.texto()
        } else {
            let mut actual = std::fs::read_to_string(&ruta).map(|t| Config::parse(&t)).unwrap_or_default();
            self.aplicar_cambios(&mut actual);
            actual.texto()
        };
        let tmp = con_sufijo(&ruta, &format!(".tmp.{}", std::process::id()));
        let r = std::fs::write(&tmp, texto).and_then(|_| std::fs::rename(&tmp, &ruta));
        if r.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        r
    }

    /// Pone en `otra` el valor que tienen aqui las claves cambiadas (o las quita si aqui valen el defecto por no estar).
    fn aplicar_cambios(&self, otra: &mut Config) {
        for k in &self.cambios {
            match self.vals.get(k) {
                Some(v) => otra.vals.insert(k.clone(), v.clone()),
                None => otra.vals.remove(k),
            };
        }
    }

    /// Resolucion pedida como (ancho, alto), si no es auto.
    pub fn resolucion(&self) -> Option<(u32, u32)> {
        let v = self.get("pantalla.resolucion");
        let (w, h) = v.split_once('x')?;
        Some((w.parse().ok()?, h.parse().ok()?))
    }

    /// Densidad fija en dpi, si no es `perfil`.
    pub fn densidad(&self) -> Option<u32> {
        self.get("pantalla.densidad").parse().ok()
    }

    /// Listado `clave=valor` (con `  # por defecto: X` cuando difiere).
    pub fn listado(&self) -> String {
        let mut s = String::new();
        for k in CLAVES {
            let v = self.get(k.nombre);
            if v == k.defecto {
                s.push_str(&format!("{}={}\n", k.nombre, v));
            } else {
                s.push_str(&txf!("config.por_defecto", k.nombre, v, k.defecto));
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn los_comentarios_de_config_y_su_listado_son_genericos() {
        use crate::textos::prohibida_en;
        let c = Config::default();
        for k in CLAVES {
            for t in [k.nombre, k.ayuda(), k.defecto, k.seccion] {
                assert!(prohibida_en(t).is_none(), "{}: {:?} nombra {:?}", k.nombre, t, prohibida_en(t));
            }
        }
        for t in [c.texto(), c.listado()] {
            assert!(prohibida_en(&t).is_none(), "{:?}", prohibida_en(&t));
        }
        // aunque se pongan valores no por defecto (el nombre del valor de GPU es del usuario, no de los comentarios)
        let mut c2 = Config::default();
        c2.set("maquina.ram", "4096").unwrap();
        assert!(prohibida_en(&c2.texto().lines().filter(|l| l.starts_with('#')).collect::<Vec<_>>().join("\n")).is_none());
    }

    use super::*;

    fn dir_prueba(n: &str) -> PathBuf {
        let d = PathBuf::from(format!("{}/config-{}-{}", std::env::var("TMPDIR").unwrap_or_else(|_| ".".into()), n, std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn defectos_validos_y_unicos() {
        // cada defecto pasa su propia validacion y no cambia al normalizar
        for k in CLAVES {
            assert_eq!(validar(k.tipo, k.defecto).as_deref(), Ok(k.defecto), "{}", k.nombre);
        }
        // nombres unicos
        for (i, a) in CLAVES.iter().enumerate() {
            assert!(CLAVES.iter().skip(i + 1).all(|b| b.nombre != a.nombre), "{} repetida", a.nombre);
        }
        // las teclas por defecto no chocan entre si
        let c = Config::nueva();
        for a in AccionAtajo::TODAS {
            assert!(c.combo(a).is_some(), "{:?}", a);
            assert_eq!(c.usa_la_tecla(&c.get(a.clave()), Some(a)), None, "{:?} repetida", a);
        }
        assert!(c.avisos.is_empty());
    }

    #[test]
    fn valores_validos_e_invalidos() {
        assert_eq!(validar(Tipo::Bool, " Sí "), Ok("si".into()));
        assert_eq!(validar(Tipo::Bool, "off"), Ok("no".into()));
        assert!(validar(Tipo::Bool, "quizas").is_err());
        assert_eq!(validar(Tipo::Entero(5, 25), "10"), Ok("10".into()));
        assert!(validar(Tipo::Entero(5, 25), "4").is_err());
        assert!(validar(Tipo::Entero(5, 25), "diez").is_err());
        assert_eq!(validar(Tipo::EnteroAuto(1, 128), "Automático"), Ok("auto".into()));
        assert_eq!(validar(Tipo::EnteroAuto(1, 128), "8"), Ok("8".into()));
        assert!(validar(Tipo::EnteroAuto(1, 128), "0").is_err());
        // none = sin aceleracion = software; gfxstream vale (fijado a mano)
        assert_eq!(validar(Tipo::Enum(GPUS), "NONE"), Ok("software".into()));
        assert_eq!(validar(Tipo::Enum(GPUS), "ninguna"), Ok("software".into()));
        assert_eq!(validar(Tipo::Enum(GPUS), "gfxstream"), Ok("gfxstream".into()));
        assert_eq!(validar(Tipo::Enum(GPUS), "Hardware"), Ok("hardware".into()));
        assert!(Config::parse("maquina.gpu=gfxstream\n").avisos.is_empty());
        assert_eq!(Config::parse("maquina.gpu=none\n").get("maquina.gpu"), "software");
        assert!(validar(Tipo::Enum(GPUS), "virgl").is_err());
        assert_eq!(validar(Tipo::Zoom, "75%"), Ok("75".into()));
        assert_eq!(validar(Tipo::Zoom, "Ajustar"), Ok("ajustar".into()));
        assert!(validar(Tipo::Zoom, "5").is_err() && validar(Tipo::Zoom, "900").is_err());
        assert_eq!(validar(Tipo::Resolucion, "720X1348"), Ok("720x1348".into()));
        assert!(validar(Tipo::Resolucion, "720x1348@280").is_err());
        assert!(validar(Tipo::Resolucion, "10x10").is_err());
        assert_eq!(validar(Tipo::Densidad, "280"), Ok("280".into()));
        assert_eq!(validar(Tipo::Densidad, "perfil"), Ok("perfil".into()));
        assert!(validar(Tipo::Densidad, "20").is_err());
    }

    #[test]
    fn claves_de_imagen_disco_y_android() {
        let mut c = Config::nueva();
        assert_eq!((c.get("disk.data").as_str(), c.get("android.bluetooth").as_str(), c.get("root.carpeta").as_str()), ("24G", "no", "auto"));
        assert_eq!(c.set("disk.data", "4g"), Ok("4G".into()));
        assert_eq!(c.set("disk.data", "IMG"), Ok("img".into()));
        assert_eq!(c.set("disk.data", "4096m"), Ok("4096M".into()));
        assert!(c.set("disk.data", "1G").is_err() && c.set("disk.data", "mucho").is_err() && c.set("disk.data", "3000G").is_err());
        assert_eq!(c.get("disk.data"), "4096M");
        assert_eq!(c.set("android.bluetooth", "si"), Ok("si".into()));
        assert!(c.bool("android.bluetooth"));
        // ida y vuelta por el archivo; `disk.data` se avisa como de proximo arranque
        let t = c.texto();
        let d = Config::parse(&t);
        assert!(d.avisos.is_empty() && d.get("disk.data") == "4096M" && d.bool("android.bluetooth"), "{:?}", d.avisos);
        assert!(clave("disk.data").unwrap().arranque && !clave("root.carpeta").unwrap().arranque);
        let mala = Config::parse("disk.data=2T\n");
        assert_eq!(mala.avisos.len(), 1);
        assert_eq!(mala.get("disk.data").as_str(), "24G");
    }

    #[test]
    fn combinaciones_de_teclas() {
        assert_eq!(Combo::parse("F5").unwrap(), Combo::tecla(62));
        let c = Combo::parse("ctrl+alt+mayús+f1").unwrap();
        assert_eq!((c.sc, c.ctrl, c.alt, c.mayus), (58, true, true, true));
        assert_eq!(c.texto(), "Ctrl+Alt+Mayus+F1");
        assert_eq!(c.visible(), "Ctrl+Alt+Mayús+F1");
        assert_eq!(Combo::parse("Ctrl+=").unwrap().sc, 46);
        assert_eq!(Combo::parse("Ctrl+-").unwrap().sc, 45);
        assert_eq!(Combo::parse("Ctrl+RePág").unwrap().texto(), "Ctrl+RePag");
        // una letra sola o con Mayus escribiria texto: hace falta Ctrl o Alt
        assert!(Combo::parse("B").is_err());
        assert!(Combo::parse("Mayus+B").is_err());
        assert!(Combo::parse("Ctrl+B").is_ok() && Combo::parse("Alt+B").is_ok());
        assert!(Combo::parse("Esc").is_err() && Combo::parse("Ctrl+Retroceso").is_err());
        assert!(Combo::parse("Ctrl+").is_err() && Combo::parse("").is_err());
        assert!(Combo::parse("Meta+F1").is_err());
        // ida y vuelta de todas las teclas con Ctrl
        for t in TECLAS {
            let c = Combo { sc: t.0, ctrl: true, alt: false, mayus: false };
            assert_eq!(Combo::parse(&c.texto()), Ok(c), "{}", t.1);
            assert_eq!(Combo::parse(&c.visible()), Ok(c), "{}", t.2);
        }
        assert!(c.coincide(58, true, true, true) && !c.coincide(58, true, true, false) && !c.coincide(59, true, true, true));
        assert_eq!(tecla_visible("Ctrl+B"), "Ctrl+B");
        assert_eq!(tecla_visible("ninguno"), "Sin asignar");
    }

    #[test]
    fn lectura_con_avisos() {
        let c = Config::parse("# comentario\nzoom=75\nrueda.paso=99\nmaquina.cpus=\ncolor=rojo\nlinea sin igual\n#confirmar=no\n");
        assert_eq!(c.get("zoom"), "75");
        assert_eq!(c.get("rueda.paso"), "10", "el valor fuera de rango vuelve al defecto");
        assert_eq!(c.get("maquina.cpus"), "auto");
        assert!(c.bool("confirmar"), "lo comentado no cuenta");
        assert_eq!(c.avisos.len(), 3, "{:?}", c.avisos);
        assert!(c.avisos.iter().any(|a| a.contains("rueda.paso=99")) && c.avisos.iter().any(|a| a.contains("color")) && c.avisos.iter().any(|a| a.contains("sin '='")));
        // la clave desconocida se conserva al reescribir
        assert!(c.texto().contains("color=rojo"));
    }

    #[test]
    fn escritura_y_vuelta() {
        let mut c = Config::nueva();
        c.set("zoom", "50").unwrap();
        c.set("atajo.atras", "Ctrl+B").unwrap();
        c.set("maquina.ram", "16384").unwrap();
        c.set("rueda.invertir", "si").unwrap();
        let t = c.texto();
        // lo que vale el defecto sale comentado y documentado; lo demas, activo
        assert!(t.contains("\nzoom=50\n") && t.contains("\natajo.atras=Ctrl+B\n") && t.contains("\nmaquina.ram=16384\n"));
        assert!(t.contains("\n#atajo.inicio=F2\n") && t.contains("# Atrás."));
        assert!(!t.contains("\nzoom=ajustar\n"));
        let r = Config::parse(&t);
        assert!(r.avisos.is_empty(), "{:?}", r.avisos);
        assert_eq!(r, c);
        for k in CLAVES {
            assert_eq!(r.get(k.nombre), c.get(k.nombre), "{}", k.nombre);
        }
        // poner el defecto lo deja comentado
        let mut d = c.clone();
        d.set("zoom", "ajustar").unwrap();
        assert!(d.texto().contains("\n#zoom=ajustar\n"));
        // reset de una clave y de todo
        d.reset(Some("maquina.ram"));
        assert_eq!(d.get("maquina.ram"), "auto");
        d.reset(None);
        assert_eq!(d, Config::nueva());
        // listado
        assert!(c.listado().contains("zoom=50  # por defecto: ajustar\n") && c.listado().contains("\nconfirmar=si\n"));
    }

    #[test]
    fn set_valida_y_detecta_conflictos() {
        let mut c = Config::nueva();
        assert!(c.set("noexiste", "1").unwrap_err().contains("desconocida"));
        assert!(c.set("rueda.paso", "50").unwrap_err().contains("fuera de rango"));
        assert_eq!(c.get("rueda.paso"), "10", "un set invalido no cambia nada");
        // F2 es de Inicio
        let e = c.set("atajo.atras", "F2").unwrap_err();
        assert!(e.contains("ya la usa «Inicio»"), "{}", e);
        assert_eq!(c.get("atajo.atras"), "F1");
        // la propia accion puede reasignarse a la misma tecla
        assert_eq!(c.set("atajo.atras", "F1"), Ok("F1".into()));
        // liberar y reasignar
        c.set("atajo.inicio", "ninguno").unwrap();
        assert_eq!(c.set("atajo.atras", "F2"), Ok("F2".into()));
        assert_eq!(c.atajo_para(59, false, false, false), Some(AccionAtajo::Atras));
        assert_eq!(c.atajo_para(58, false, false, false), None);
        // exactitud de modificadores: Ctrl+F2 no es F2
        assert_eq!(c.atajo_para(59, true, false, false), None);
        c.restaurar_atajos();
        assert_eq!(c.atajo_para(58, false, false, false), Some(AccionAtajo::Atras));
        assert_eq!(c.atajo_para(46, true, false, false), Some(AccionAtajo::ZoomMas));
        // los atajos propios de la ventana tampoco se pisan con los demas, en ningun sentido
        let e = c.set("atajo.atras", "F11").unwrap_err();
        assert!(e.contains("ya la usa atajo.pantalla_completa"), "{}", e);
        let e = c.set("atajo.menu", "F1").unwrap_err();
        assert!(e.contains("ya la usa «Atrás»"), "{}", e);
        let e = c.set("atajo.menu", "Ctrl+Alt+F").unwrap_err();
        assert!(e.contains("ya la usa atajo.pasar_tecla"), "{}", e);
        assert_eq!(c.set("atajo.menu", "Ctrl+Alt+M"), Ok("Ctrl+Alt+M".into()));
        // la propia clave puede quedarse con su tecla
        assert_eq!(c.set("atajo.pantalla_completa", "F11"), Ok("F11".into()));
    }

    /// Los ajustes de la ventana (atajos propios, interruptor de atajos, botones del raton) son claves de `config` como las
    /// demas: `weft config set` los acepta, se guardan en el archivo y sobreviven a la ventana.
    #[test]
    fn los_ajustes_de_la_ventana_se_guardan() {
        let mut c = Config::nueva();
        for (k, defecto) in [("atajo.pasar_tecla", "Ctrl+Alt+F"), ("atajo.pantalla_completa", "F11"), ("atajo.encendido", "ninguno"), ("atajo.menu", "ninguno"), ("atajos.desactivados", "no"), ("raton.derecho", "atras"), ("raton.central", "inicio")] {
            assert_eq!(c.get(k), defecto, "{}", k);
        }
        assert_eq!(c.set("atajos.desactivados", "1"), Ok("si".into()));
        assert_eq!(c.set("raton.derecho", "Recientes"), Ok("recientes".into()));
        assert_eq!(c.set("raton.central", "ninguno"), Ok("none".into()));
        assert!(c.set("raton.central", "saltar").is_err());
        assert_eq!(c.set("atajo.encendido", "ctrl+alt+p"), Ok("Ctrl+Alt+P".into()));
        assert!(c.set("atajo.encendido", "P").is_err());
        // ida y vuelta por el archivo, sin avisos
        let r = Config::parse(&c.texto());
        assert!(r.avisos.is_empty(), "{:?}", r.avisos);
        assert_eq!((r.get("atajos.desactivados").as_str(), r.get("raton.derecho").as_str(), r.get("raton.central").as_str(), r.get("atajo.encendido").as_str()), ("si", "recientes", "none", "Ctrl+Alt+P"));
        // y escritas a mano tambien valen
        assert!(Config::parse("raton.derecho=menu\natajos.desactivados=si\n").bool("atajos.desactivados"));
        // cada seccion sale una sola vez en el archivo (las claves de una seccion van juntas)
        let t = Config::nueva().texto();
        for s in ["Atajos", "Entrada"] {
            assert_eq!(t.matches(&format!("# --- {} ---", s)).count(), 1, "{}", s);
        }
    }

    #[test]
    fn conflicto_a_mano_gana_lo_escrito() {
        // atajo.atras=F2 choca con el defecto de Inicio: gana lo escrito a mano, Inicio queda sin asignar
        let c = Config::parse("atajo.atras=F2\n");
        assert_eq!(c.get("atajo.atras"), "F2");
        assert_eq!(c.get("atajo.inicio"), "ninguno");
        assert!(c.avisos.iter().any(|a| a.contains("ya la usa «Atrás»") && a.contains("«Inicio» queda sin asignar")), "{:?}", c.avisos);
        // dos escritas a mano iguales: gana la primera del archivo (orden de las claves)
        let c = Config::parse("atajo.captura=Ctrl+K\natajo.rotar=Ctrl+K\n");
        assert_eq!(c.get("atajo.rotar"), "Ctrl+K");
        assert_eq!(c.get("atajo.captura"), "ninguno");
    }

    #[test]
    fn archivo() {
        let d = dir_prueba("arch");
        // sin nada: defecto
        assert_eq!(Config::cargar(&d), Config::nueva());
        let mut c = Config::nueva();
        c.set("zoom", "67").unwrap();
        c.set("pantalla.resolucion", "800x1280").unwrap();
        c.set("pantalla.densidad", "320").unwrap();
        c.guardar(&d).unwrap();
        let c2 = Config::cargar(&d);
        assert_eq!((c2.get("zoom"), c2.resolucion(), c2.densidad()), ("67".to_string(), Some((800, 1280)), Some(320)));
        assert!(c2.avisos.is_empty(), "{:?}", c2.avisos);
        // guardar y releer
        let mut c3 = c2.clone();
        c3.set("zoom", "ajustar").unwrap();
        c3.guardar(&d).unwrap();
        assert_eq!(Config::cargar(&d).get("zoom"), "ajustar");
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// Dos escritores a la vez (la ventana y `weft config set`, aqui dos hilos) que leen, cambian cada uno su clave y
    /// guardan: ninguno pisa lo del otro, y no queda ningun temporal.
    #[test]
    fn escritores_concurrentes_no_se_pisan() {
        let d = dir_prueba("concurrente");
        let _ = std::fs::remove_file(d.join("config"));
        let hilos: Vec<_> = [("maquina.cpus", 1u32), ("rueda.tope", 20u32)]
            .into_iter()
            .map(|(k, base)| {
                let d = d.clone();
                std::thread::spawn(move || {
                    for i in 0..50 {
                        let mut c = Config::cargar(&d);
                        c.set(k, &(base + i).to_string()).unwrap();
                        c.guardar(&d).unwrap();
                    }
                })
            })
            .collect();
        for h in hilos {
            h.join().unwrap();
        }
        let c = Config::cargar(&d);
        assert_eq!((c.get("maquina.cpus").as_str(), c.get("rueda.tope").as_str()), ("50", "69"));
        let mut nombres: Vec<String> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        nombres.sort();
        assert_eq!(nombres, vec!["config", "config.lock"]);
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// `guardar` aplica solo lo que cambio esta configuracion sobre lo que hay en el archivo; tras `reset(None)`, todo.
    #[test]
    fn guardar_conserva_lo_que_escribieron_otros() {
        let d = dir_prueba("fusion");
        let _ = std::fs::remove_file(d.join("config"));
        let mut ventana = Config::cargar(&d);
        // mientras tanto, otro proceso escribe dos claves
        let mut otro = Config::cargar(&d);
        otro.set("zoom", "150").unwrap();
        otro.set("maquina.ram", "4096").unwrap();
        otro.guardar(&d).unwrap();
        // la ventana cambia una clave y quita otra (que vale su defecto)
        ventana.set("rueda.paso", "20").unwrap();
        ventana.reset(Some("maquina.ram"));
        ventana.guardar(&d).unwrap();
        let c = Config::cargar(&d);
        assert_eq!((c.get("zoom").as_str(), c.get("rueda.paso").as_str(), c.get("maquina.ram").as_str()), ("150", "20", "auto"));
        // restaurar los atajos cuenta como cambio de cada uno
        let mut a = Config::cargar(&d);
        a.set("atajo.atras", "F12").unwrap();
        a.guardar(&d).unwrap();
        let mut b = Config::cargar(&d);
        b.restaurar_atajos();
        b.guardar(&d).unwrap();
        assert_eq!(Config::cargar(&d).get("atajo.atras"), "F1");
        // las claves desconocidas escritas a mano se conservan
        std::fs::write(d.join("config"), "zoom=200
color=rojo
").unwrap();
        ventana.guardar(&d).unwrap();
        let t = std::fs::read_to_string(d.join("config")).unwrap();
        assert!(t.contains("\nzoom=200\n") && t.contains("color=rojo") && t.contains("rueda.paso=20"), "{}", t);
        // restablecer todo escribe la configuracion entera (sin lo de los demas)
        let mut r = Config::cargar(&d);
        r.reset(None);
        r.guardar(&d).unwrap();
        let t = std::fs::read_to_string(d.join("config")).unwrap();
        assert_eq!(Config::cargar(&d), Config::nueva());
        assert!(!t.contains("color=rojo"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn resolucion_y_densidad() {
        let mut c = Config::nueva();
        assert_eq!((c.resolucion(), c.densidad()), (None, None));
        c.set("pantalla.resolucion", "1348x720").unwrap();
        c.set("pantalla.densidad", "280").unwrap();
        assert_eq!((c.resolucion(), c.densidad()), (Some((1348, 720)), Some(280)));
        assert_eq!(c.entero("maquina.cpus"), None);
        c.set("maquina.cpus", "6").unwrap();
        assert_eq!(c.entero("maquina.cpus"), Some(6));
    }
}
