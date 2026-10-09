//! Vista de la ventana propia (`--display window`): geometria (barra superior y pantalla del dispositivo), zoom, acciones
//! de la interfaz y servicios de fondo (mandos, maquina, captura, apagado, reinicios, ordenes largas).
//!
//! La ventana ya no tiene panel lateral: solo la barra superior (`barra.rs`) y la pantalla del dispositivo, que ocupa el
//! resto. Todo lo que ofrecia el panel (Atras, Inicio, Recientes, volumen, rotacion, captura, zoom, mandos, estado de la
//! maquina, reiniciar y apagar) vive en la seccion Controles, Entrada y Maquina de la pantalla de configuracion
//! (`ajustes.rs`), que se abre desde la pestana Configuracion de la barra (o con F9).
//!
//! Este modulo no conoce SDL. Las acciones de fondo las hace `Servicios` en hilos propios para no bloquear la ventana:
//! QEMU admite un solo cliente QMP a la vez y algunas esperan segundos.
//!
//! Un clic sobre la barra nunca llega al invitado: `window.rs` no lo manda a la pantalla tactil.

use crate::config::Config;
use crate::formas::{tema, Color, Pint};
use crate::fuente::{envolver, truncar, Estilo, Medida};
use crate::gamepad;
use crate::gestos::Atajo;
use crate::json::V;
use crate::qmp::Qmp;
use crate::textos::{clave, texto, tx, txf};
use crate::vm::{self, State};
pub use crate::formas::R;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------------------------------------------
// acciones

/// Lo que dispara un control de la ventana (tecla de atajo, pestana de la barra o boton de la seccion Controles).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accion {
    /// Atras, Inicio, Recientes, Vol-, Vol+ y Rotar (un paso): exactamente lo de F1, F2, F3, F5, F6 y F7.
    Atajo(Atajo),
    /// Fija la rotacion (0..3, como Surface.ROTATION_*): la orden `rotate`.
    Rotacion(u32),
    /// la rotacion la decide Android
    RotacionAuto,
    /// abre la pantalla de configuracion (la emite la pestana Configuracion de la barra superior y el atajo F9)
    Configuracion,
    ZoomMas,
    ZoomMenos,
    ZoomAjustar,
    Zoom1a1,
    Captura,
    /// entra o sale de la pantalla completa (atajo F11 por defecto)
    PantallaCompleta,
}

impl Accion {
    /// Nombre estable (para las pruebas con el gancho de inyeccion y para el registro).
    pub fn nombre(&self) -> String {
        match self {
            Accion::Atajo(a) => match a {
                Atajo::Atras => "atras",
                Atajo::Inicio => "inicio",
                Atajo::Recientes => "recientes",
                Atajo::VolBajar => "vol-",
                Atajo::VolSubir => "vol+",
                Atajo::Rotar => "rotar",
                Atajo::Menu => "menu",
                Atajo::Encendido => "encendido",
            }
            .into(),
            Accion::Rotacion(r) => format!("rot{}", r * 90),
            Accion::RotacionAuto => "rotauto".into(),
            Accion::Configuracion => "config".into(),
            Accion::ZoomMas => "zoom+".into(),
            Accion::ZoomMenos => "zoom-".into(),
            Accion::ZoomAjustar => "ajustar".into(),
            Accion::Zoom1a1 => "1:1".into(),
            Accion::Captura => "captura".into(),
            Accion::PantallaCompleta => "pantalla-completa".into(),
        }
    }

    /// Frase de confirmacion para la linea de estado de la seccion Controles.
    pub fn confirmacion(&self) -> String {
        match self {
            Accion::Atajo(a) => match a {
                Atajo::Atras => tx!("enviado.atras_enviado"),
                Atajo::Inicio => tx!("enviado.inicio_enviado"),
                Atajo::Recientes => tx!("enviado.recientes_enviado"),
                Atajo::VolBajar => tx!("enviado.volumen_menos"),
                Atajo::VolSubir => tx!("enviado.volumen_mas"),
                Atajo::Rotar => tx!("enviado.rotacion_cambiada_paso"),
                Atajo::Menu => tx!("enviado.menu_enviado"),
                Atajo::Encendido => tx!("enviado.encendido_enviado"),
            }
            .into(),
            Accion::Rotacion(r) => txf!("enviado.rotacion_fijada", r % 4 * 90),
            Accion::RotacionAuto => tx!("enviado.rotacion_decide_android").into(),
            Accion::Configuracion => String::new(),
            Accion::ZoomMas => tx!("enviado.zoom_acercado").into(),
            Accion::ZoomMenos => tx!("enviado.zoom_alejado").into(),
            Accion::ZoomAjustar => tx!("enviado.zoom_ajustado_ventana").into(),
            Accion::Zoom1a1 => tx!("enviado.zoom_1_1").into(),
            Accion::Captura => tx!("enviado.captura_pedida").into(),
            Accion::PantallaCompleta => tx!("enviado.pantalla_completa_cambiada").into(),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// zoom y geometria

/// Modo de zoom: `Ajustar` (la imagen se ajusta a lo que mida la ventana) o `Fijo(porcentaje)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ModoZoom {
    Ajustar,
    Fijo(u32),
}

pub const PASOS: [u32; 10] = [25, 33, 50, 67, 75, 100, 125, 150, 200, 300];

/// Siguiente paso de zoom por encima de `pct` (el ultimo si ya se paso).
pub fn paso_mas(pct: f32) -> u32 {
    PASOS.iter().copied().find(|p| *p as f32 > pct + 0.5).unwrap_or(PASOS[PASOS.len() - 1])
}

/// Paso de zoom inmediatamente por debajo de `pct` (el primero si ya esta abajo).
pub fn paso_menos(pct: f32) -> u32 {
    PASOS.iter().rev().copied().find(|p| (*p as f32) < pct - 0.5).unwrap_or(PASOS[0])
}


/// Alto y ancho minimos de la ventana (dp): la barra suma su alto al minimo de alto.
pub const ANCHO_MIN: f32 = 320.0;
pub const ALTO_MIN: f32 = 240.0;

/// Reparto de la ventana: la barra superior (a todo el ancho, `barra::alto()` dp) y la pantalla del dispositivo (centrada y
/// sin deformar en el espacio que queda bajo la barra).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Diseno {
    pub barra: R,
    pub dispositivo: R,
    /// pixeles de ventana por pixel de la vista del dispositivo
    pub escala: f32,
}

/// `win` es el tamano de la ventana; `vista` el de lo que muestra el dispositivo (ya girado).
pub fn disenar(win: (f32, f32), vista: (u32, u32)) -> Diseno {
    let bh = crate::barra::alto().min(win.1.max(0.0));
    let (rw, rh) = (win.0.max(0.0), (win.1 - bh).max(0.0));
    let (vw, vh) = (vista.0.max(1) as f32, vista.1.max(1) as f32);
    let s = (rw / vw).min(rh / vh);
    let (w, h) = (vw * s, vh * s);
    Diseno { barra: R::new(0.0, 0.0, win.0.max(0.0), bh), dispositivo: R::new((rw - w) / 2.0, bh + (rh - h) / 2.0, w, h), escala: s }
}

/// Punto de la ventana -> punto de la vista del dispositivo (indices de pixel como numeros reales), o None si cae fuera
/// de la imagen del dispositivo (en la barra o en las bandas).
pub fn a_vista(d: &Diseno, vista: (u32, u32), p: (f32, f32)) -> Option<(f32, f32)> {
    if d.escala <= 0.0 || !d.dispositivo.contiene(p.0, p.1) {
        return None;
    }
    let (x, y) = ((p.0 - d.dispositivo.x) / d.escala, (p.1 - d.dispositivo.y) / d.escala);
    if x < 0.0 || y < 0.0 || x >= vista.0 as f32 || y >= vista.1 as f32 {
        return None;
    }
    Some((x, y))
}

/// Tamano de ventana para una vista del dispositivo: (ancho, alto) y la escala resultante. `escala`: Some(s) pide esa
/// escala (hasta el 95 % del area util); None ajusta como el arranque (sin pasar de 1:1 ni del 90 % del area util).
/// `area` es el area util de la pantalla del equipo (0 si se desconoce). El alto incluye la barra superior.
pub fn ventana_para(vista: (u32, u32), escala: Option<f32>, area: (i32, i32)) -> ((i32, i32), f32) {
    let bh = crate::barra::alto();
    let (vw, vh) = (vista.0.max(1) as f32, vista.1.max(1) as f32);
    let mut s = escala.unwrap_or(1.0).max(0.01);
    if area.0 > 0 && area.1 > 0 {
        let k = if escala.is_some() { 0.95 } else { 0.9 };
        s = s.min((area.0 as f32 * k).max(1.0) / vw).min(((area.1 as f32 * k - bh).max(1.0)) / vh);
    }
    let w = (vw * s).round().max(ANCHO_MIN);
    let h = (vh * s + bh).round().max(ALTO_MIN + bh);
    ((w as i32, h as i32), s)
}

/// Modo de zoom que dice la configuracion (clave `zoom`).
pub fn zoom_de_config(c: &Config) -> ModoZoom {
    match c.get("zoom").as_str() {
        "ajustar" => ModoZoom::Ajustar,
        n => n.parse::<u32>().map_or(ModoZoom::Ajustar, ModoZoom::Fijo),
    }
}

/// Anota el modo de zoom en la configuracion. true si algo cambio (hay que guardar el archivo). Solo lo usan las pruebas:
/// el zoom que cambian los atajos y Controles es de la ventana abierta y no pisa el «Zoom inicial» (clave `zoom`).
#[cfg(test)]
pub fn zoom_a_config(z: ModoZoom, c: &mut Config) -> bool {
    let t = match z {
        ModoZoom::Ajustar => "ajustar".to_string(),
        ModoZoom::Fijo(p) => p.to_string(),
    };
    if c.get("zoom") != t {
        let _ = c.set("zoom", &t);
        return true;
    }
    false
}

// ---------------------------------------------------------------------------------------------------------------
// datos que muestra la interfaz

#[derive(Clone, Debug, PartialEq)]
pub struct Mando {
    pub path: String,
    pub name: String,
    /// id del dispositivo de QEMU si esta conectado a la maquina
    pub conectado: Option<String>,
    /// hay una conexion o desconexion en curso (el invitado tarda unos segundos en confirmarla)
    pub ocupado: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Maquina {
    pub cpus: usize,
    pub mem_mb: u64,
    pub tipo: String,
    pub estado: String,
}

/// Estado de la maquina que resume el indicador de la barra superior (texto y color del punto). Lo decide
/// `estado_de` con lo que sonda el hilo de fondo (`query-status` por QMP, `sys.boot_completed` por el adb propio y si la
/// maquina va con KVM) y con lo que la propia ventana lanzo (reinicio ordenado, apagado).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Estado {
    /// QEMU corre pero Android no ha terminado de arrancar (o aun no se pudo consultar)
    #[default]
    Arrancando,
    EnMarcha,
    /// QEMU tiene las CPU paradas (pausa, suspension, fallo del invitado)
    Pausada,
    Reiniciando,
    Apagando,
    /// sin KVM: la maquina va por emulacion (TCG), mucho mas lenta
    SinAceleracion,
}

impl Estado {
    /// Texto del indicador. "Android en marcha" se conserva tal cual (lo buscan las pruebas de la ventana).
    pub fn texto(self) -> &'static str {
        match self {
            Estado::Arrancando => tx!("estado.arrancando_android"),
            Estado::EnMarcha => tx!("estado.android_marcha"),
            Estado::Pausada => tx!("estado.maquina_pausa"),
            Estado::Reiniciando => tx!("estado.reiniciando_android"),
            Estado::Apagando => tx!("estado.apagando"),
            Estado::SinAceleracion => tx!("estado.sin_aceleracion_lento"),
        }
    }

    /// Color del punto: verde en marcha, ambar mientras cambia (arrancando, reiniciando, apagando) o va lenta, gris en pausa.
    pub fn color(self) -> Color {
        match self {
            Estado::EnMarcha => tema::p().exito,
            Estado::Pausada => tema::p().texto_apagado,
            Estado::Arrancando | Estado::Reiniciando | Estado::Apagando | Estado::SinAceleracion => tema::p().aviso,
        }
    }
}

/// Estado para la barra. `qemu`: lo que dice `query-status` (None si no se pudo consultar); `arrancado`: Android dio
/// `sys.boot_completed=1`; `tcg`: la maquina va sin KVM. Lo que pidio la ventana (apagar, reiniciar) manda sobre lo
/// sondeado, y un QEMU parado (en pausa, suspendido o con el invitado caido) sobre el resto.
pub fn estado_de(qemu: Option<&str>, arrancado: bool, reiniciando: bool, apagando: bool, tcg: bool) -> Estado {
    if apagando {
        return Estado::Apagando;
    }
    if reiniciando {
        return Estado::Reiniciando;
    }
    match qemu {
        Some("shutdown") => Estado::Apagando,
        Some("paused" | "suspended" | "prelaunch" | "guest-panicked" | "internal-error" | "io-error" | "inmigrate" | "postmigrate" | "finish-migrate" | "save-vm" | "restore-vm" | "watchdog") => Estado::Pausada,
        _ if tcg => Estado::SinAceleracion,
        _ if arrancado => Estado::EnMarcha,
        _ => Estado::Arrancando,
    }
}

/// Tiempo con QEMU en marcha sin que adbd conteste ni una vez tras el que la barra deja de esperar a adb: la maquina no
/// tiene adb propio (sin vsock ni puerto TCP), o el invitado no es Android. adbd arranca en los primeros segundos del
/// arranque de Android, tambien en el primero (que formatea /data).
pub const SIN_ADB_TRAS: Duration = Duration::from_secs(180);

/// ¿No se puede saber por adb si Android termino de arrancar? adbd rechazo al adb propio (`rechazo`: pide RSA o TLS) o, con
/// QEMU en marcha desde hace `en_marcha` (SIN_ADB_TRAS o mas), no contesto nunca (`contesto`). Entonces la barra dice que
/// esta en marcha en vez de quedarse para siempre en "Arrancando Android...". Pura.
pub fn sin_adb(rechazo: bool, contesto: bool, en_marcha: Option<Duration>) -> bool {
    rechazo || (!contesto && en_marcha.is_some_and(|d| d >= SIN_ADB_TRAS))
}

/// `start --accel tcg` en los argumentos guardados del arranque (`start-args`): la maquina va sin aceleracion aunque
/// no se pueda preguntar a QEMU.
pub fn tcg_pedido(args: &[String]) -> bool {
    args.windows(2).any(|w| w[0] == "--accel" && w[1] == "tcg") || args.iter().any(|a| a == "--accel=tcg")
}

/// Todo lo que la barra y la configuracion muestran y que no es suyo.
#[derive(Clone, Debug, Default)]
pub struct Info {
    /// rotacion actual (0..3) con que se dibuja la ventana
    pub rot: u32,
    /// orientacion pedida (fija o auto)
    pub orient: crate::pantalla::Orientacion,
    /// escala actual de la vista (ventana por pixel del dispositivo): el zoom que se ve
    pub escala: f32,
    pub mandos: Vec<Mando>,
    pub ilegibles: usize,
    /// se pudo hablar con la maquina por QMP la ultima vez
    pub qmp_ok: bool,
    pub maquina: Option<Maquina>,
    pub encendida: Option<Duration>,
    /// lo que la barra dice de la maquina (ver `estado_de`)
    pub estado: Estado,
    /// adbd no respondio al pedir el reinicio ordenado: se ofrece el reinicio completo de la maquina
    pub reinicio_completo: bool,
    /// aviso breve (captura guardada, mando conectado...) que la barra muestra unos segundos
    pub aviso: Option<String>,
    /// el mismo aviso con cada ruta reducida a su ultimo nombre, para cuando entero no cabe en la barra (None: no lleva rutas)
    pub aviso_corto: Option<String>,
    /// los atajos de teclado estan desactivados o la siguiente tecla va a Android (lo pone la ventana)
    pub teclado: ModoTeclado,
}

/// Lo que la barra avisa del teclado mientras dure: sin atajos (todo llega a Android) o la siguiente tecla pasando a
/// Android aunque sea un atajo (atajo "Pasar la siguiente tecla a Android").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModoTeclado {
    /// lo normal: los atajos de la ventana funcionan
    #[default]
    Atajos,
    Desactivados,
    PasarSiguiente,
}

impl ModoTeclado {
    /// Texto de la etiqueta de la barra (None: no hay nada que avisar).
    pub fn texto(self) -> Option<&'static str> {
        match self {
            ModoTeclado::Atajos => None,
            ModoTeclado::Desactivados => Some(tx!("teclado.atajos_desactivados")),
            ModoTeclado::PasarSiguiente => Some(tx!("teclado.siguiente_tecla_android")),
        }
    }

    /// Con los atajos desactivados manda eso; si no, si la siguiente tecla pasa a Android.
    pub fn de(desactivados: bool, pasar: bool) -> ModoTeclado {
        if desactivados {
            ModoTeclado::Desactivados
        } else if pasar {
            ModoTeclado::PasarSiguiente
        } else {
            ModoTeclado::Atajos
        }
    }
}

/// `5m 12s`, `1h 02m`, `42s`.
pub fn duracion(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}h {:02}m", s / 3600, s % 3600 / 60)
    } else if s >= 60 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{}s", s)
    }
}

/// Estado de QEMU (`query-status`) en espanol.
pub fn estado_es(e: &str) -> &str {
    match e {
        "running" => tx!("estado_maquina.marcha"),
        "paused" => tx!("estado_maquina.pausa"),
        "prelaunch" => tx!("estado_maquina.preparando"),
        "shutdown" => tx!("estado_maquina.apagada"),
        "guest-panicked" => tx!("estado_maquina.android_fallo"),
        "suspended" => tx!("estado_maquina.suspendida"),
        "internal-error" | "io-error" => tx!("estado_maquina.error"),
        otro => otro,
    }
}

/// `pc-q35-10.2-machine` -> `q35 10.2`.
pub fn tipo_corto(t: &str) -> String {
    t.trim_end_matches("-machine").trim_start_matches("pc-").replace('-', " ")
}

/// Nombre de un archivo de captura por la fecha UTC: `captura-AAAAMMDD-HHMMSS.png`.
pub fn nombre_captura(epoch: u64) -> String {
    let (dias, resto) = ((epoch / 86400) as i64, epoch % 86400);
    // fecha civil desde dias desde 1970-01-01 (algoritmo de Howard Hinnant)
    let z = dias + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("captura-{:04}{:02}{:02}-{:02}{:02}{:02}.png", y, m, d, resto / 3600, resto % 3600 / 60, resto % 60)
}

/// Carpeta de imagenes del usuario (XDG): la variable `XDG_PICTURES_DIR` o su linea de `user-dirs.dirs` (`contenido`;
/// `XDG_PICTURES_DIR="$HOME/Imagenes"` o una ruta absoluta). None si no esta, no es absoluta o es la carpeta personal
/// (asi la desactiva xdg-user-dirs). Pura.
pub fn carpeta_imagenes(env: &dyn Fn(&str) -> Option<String>, contenido: Option<&str>) -> Option<PathBuf> {
    let home = env("HOME").filter(|h| h.starts_with('/'));
    let valor = env("XDG_PICTURES_DIR").filter(|v| !v.trim().is_empty()).or_else(|| {
        contenido?.lines().map(str::trim).filter(|l| !l.starts_with('#')).find_map(|l| l.strip_prefix("XDG_PICTURES_DIR=")).map(|v| v.trim().trim_matches('"').to_string())
    })?;
    let ruta = match valor.strip_prefix("$HOME") {
        Some(resto) if resto.is_empty() || resto.starts_with('/') => PathBuf::from(home.clone()?).join(resto.trim_start_matches('/')),
        Some(_) => return None,
        None if valor.starts_with('/') => PathBuf::from(&valor),
        None => return None,
    };
    let ruta = crate::rutas::normalizar(&ruta);
    (home.map_or(true, |h| ruta != crate::rutas::normalizar(Path::new(&h)))).then_some(ruta)
}

/// Donde puede ir una captura, por orden de preferencia: la carpeta de imagenes del usuario (si la hay) y la carpeta de
/// datos de la maquina (`<datos>/machines/<maquina>/capturas`). Pura.
pub fn carpetas_de_captura(imagenes: Option<PathBuf>, datos: &Path, maquina: &str) -> Vec<PathBuf> {
    let propia = datos.join(crate::rutas::nombres::MAQUINAS).join(maquina).join("capturas");
    imagenes.into_iter().chain(std::iter::once(propia)).collect()
}

/// `carpeta_imagenes` con el entorno de este proceso y su `user-dirs.dirs` (en `$XDG_CONFIG_HOME` o en `~/.config`).
fn carpeta_imagenes_del_usuario() -> Option<PathBuf> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let config = env("XDG_CONFIG_HOME").filter(|v| v.starts_with('/')).map(PathBuf::from).or_else(|| env("HOME").map(|h| Path::new(&h).join(".config")));
    let contenido = config.and_then(|c| std::fs::read_to_string(c.join("user-dirs.dirs")).ok());
    carpeta_imagenes(&env, contenido.as_deref())
}

/// Carpeta de datos de la maquina del estado `dir` (la clave `dir.datos` de su configuracion manda sobre la estandar).
fn datos_de_la_maquina(dir: &Path) -> PathBuf {
    let cfg = Config::cargar(dir);
    crate::rutas::actual().con_claves(&cfg.get("dir.datos"), &cfg.get("dir.cache")).datos
}

/// La primera carpeta de `candidatas` que existe (o se puede crear) y admite escribir en ella (dentro de Flatpak la carpeta
/// de imagenes no se ve sin permiso expreso). Err con el motivo de la ultima si ninguna sirve.
pub fn elegir_carpeta(candidatas: &[PathBuf]) -> Result<PathBuf, String> {
    let mut error = tx!("error.no_hay_ninguna_carpeta").to_string();
    for c in candidatas {
        let prueba = c.join(format!(".weft-escritura-{}", std::process::id()));
        let r = std::fs::create_dir_all(c).and_then(|_| std::fs::OpenOptions::new().write(true).create_new(true).open(&prueba)).map(|_| {
            let _ = std::fs::remove_file(&prueba);
        });
        match r {
            Ok(()) => return Ok(c.clone()),
            Err(e) => error = format!("{}: {}", ruta_literal(c), error_io(&e)),
        }
    }
    Err(error)
}

// ---------------------------------------------------------------------------------------------------------------
// textos para la interfaz: rutas, errores del sistema y ordenes de terminal

/// Marcas (aislantes bidireccionales de Unicode, invisibles) que encierran en un mensaje una ruta del usuario que weft
/// escribe tal cual: `para_mostrar` no la pasa por `textos::limpiar` ni por la traduccion de errores, y quita las marcas.
pub const RUTA_INI: char = '\u{2068}';
pub const RUTA_FIN: char = '\u{2069}';

/// `p` entre las marcas de ruta literal.
pub fn ruta_literal(p: &Path) -> String {
    format!("{}{}{}", RUTA_INI, p.display(), RUTA_FIN)
}

/// Trozos de `t`: (texto, es una ruta literal). Solo cuenta como ruta lo que va entre las dos marcas, es absoluto y existe
/// (un texto ajeno con marcas no se salta asi la red de seguridad); lo demas, marcas sueltas incluidas, es texto normal.
fn trozos(t: &str) -> Vec<(String, bool)> {
    let mut v = Vec::new();
    let mut resto = t;
    while let Some(i) = resto.find(RUTA_INI) {
        let tras = &resto[i + RUTA_INI.len_utf8()..];
        let Some(j) = tras.find(RUTA_FIN) else { break };
        let ruta = &tras[..j];
        let valida = ruta.starts_with('/') && !ruta.contains(RUTA_INI) && Path::new(ruta).exists();
        v.push((resto[..i].to_string(), false));
        v.push((if valida { ruta.to_string() } else { format!("{}{}{}", RUTA_INI, ruta, RUTA_FIN) }, valida));
        resto = &tras[j + RUTA_FIN.len_utf8()..];
    }
    v.push((resto.to_string(), false));
    v
}

/// `textos::limpiar` sobre todo menos las rutas literales, que siguen marcadas (para quien muestre el texto despues).
pub fn limpiar_salvo_rutas(t: &str) -> String {
    trozos(t).into_iter().map(|(s, ruta)| if ruta { format!("{}{}{}", RUTA_INI, s, RUTA_FIN) } else { crate::textos::limpiar(&s) }).collect()
}

/// Texto listo para la interfaz: lo que no es ruta literal pasa por `textos::limpiar`, por `traducir_errores` y por
/// `ordenes_de_consola` (con el Flatpak de este proceso); las rutas quedan tal cual. Sin marcas.
pub fn para_mostrar(t: &str) -> String {
    let flatpak = flatpak_id();
    trozos(t)
        .into_iter()
        .map(|(s, ruta)| if ruta { s } else { ordenes_de_consola(&traducir_errores(&crate::textos::limpiar(&s)), flatpak.as_deref()).replace([RUTA_INI, RUTA_FIN], "") })
        .collect()
}

/// Como `para_mostrar`, pero cada ruta literal se acorta a su ultimo nombre (`.../captura-....png`): para la barra, donde una
/// ruta entera no suele caber. None si `t` no lleva rutas.
pub fn para_mostrar_corto(t: &str) -> Option<String> {
    let v = trozos(t);
    v.iter().any(|(_, r)| *r).then(|| {
        v.into_iter()
            .map(|(s, ruta)| if ruta { format!(".../{}", Path::new(&s).file_name().map_or(s.clone(), |n| n.to_string_lossy().into_owned())) } else { para_mostrar(&s) })
            .collect()
    })
}

/// Errores del sistema que llegan en ingles (texto de `std::io::Error`): (numero de error, texto original, clave de su
/// traduccion en el catalogo de textos).
// texto-interno: textos del sistema en ingles que se buscan para traducirlos
const ERRORES_SO: &[(i32, &str, &str)] = &[
    (1, "Operation not permitted", clave!("error.operacion_no_permitida")),
    (2, "No such file or directory", clave!("error.no_existe")),
    (3, "No such process", clave!("error.proceso_no_existe")),
    (4, "Interrupted system call", clave!("error.llamada_interrumpida")),
    (5, "Input/output error", clave!("error.error_entrada_salida")),
    (6, "No such device or address", clave!("error.dispositivo_no_existe")),
    (11, "Resource temporarily unavailable", clave!("error.recurso_no_disponible_ahora")),
    (12, "Cannot allocate memory", clave!("error.no_hay_memoria_suficiente")),
    (13, "Permission denied", clave!("error.sin_permiso")),
    (16, "Device or resource busy", clave!("error.dispositivo_recurso_ocupado")),
    (17, "File exists", clave!("error.ya_existe")),
    (18, "Invalid cross-device link", clave!("error.no_puede_mover_entre")),
    (19, "No such device", clave!("error.dispositivo_no_existe")),
    (20, "Not a directory", clave!("error.no_carpeta")),
    (21, "Is a directory", clave!("error.carpeta")),
    (22, "Invalid argument", clave!("error.argumento_no_valido")),
    (24, "Too many open files", clave!("error.demasiados_archivos_abiertos")),
    (26, "Text file busy", clave!("error.archivo_uso")),
    (27, "File too large", clave!("error.archivo_demasiado_grande")),
    (28, "No space left on device", clave!("error.no_queda_espacio_disco")),
    (30, "Read-only file system", clave!("error.sistema_archivos_solo_lectura")),
    (32, "Broken pipe", clave!("error.conexion_cortada")),
    (36, "File name too long", clave!("error.nombre_archivo_demasiado_largo")),
    (39, "Directory not empty", clave!("error.carpeta_no_esta_vacia")),
    (40, "Too many levels of symbolic links", clave!("error.demasiados_enlaces_simbolicos_encadenados")),
    (95, "Operation not supported", clave!("error.operacion_no_admitida")),
    (98, "Address already in use", clave!("error.direccion_ya_uso")),
    (104, "Connection reset by peer", clave!("error.conexion_corto")),
    (110, "Connection timed out", clave!("error.agoto_tiempo_conexion")),
    (111, "Connection refused", clave!("error.conexion_rechazada")),
    (113, "No route to host", clave!("error.sin_ruta_equipo")),
    (122, "Disk quota exceeded", clave!("error.cuota_disco_superada")),
];

/// Lo que escribe `std::io::Error` cuando no viene del sistema sino de su clase (`ErrorKind`), y la clave de su traduccion.
// texto-interno: textos del sistema en ingles que se buscan para traducirlos
const ERRORES_CLASE: &[(&str, &str)] = &[
    ("entity not found", clave!("error.no_existe")),
    ("permission denied", clave!("error.sin_permiso")),
    ("entity already exists", clave!("error.ya_existe")),
    ("not a directory", clave!("error.no_carpeta")),
    ("is a directory", clave!("error.carpeta")),
    ("directory not empty", clave!("error.carpeta_no_esta_vacia")),
    ("read-only filesystem or storage medium", clave!("error.sistema_archivos_solo_lectura")),
    ("no storage space", clave!("error.no_queda_espacio_disco")),
    ("connection refused", clave!("error.conexion_rechazada")),
    ("connection reset", clave!("error.conexion_corto")),
    ("timed out", clave!("error.agoto_tiempo_espera")),
    ("broken pipe", clave!("error.conexion_cortada")),
    ("unexpected end of file", clave!("error.fin_archivo_inesperado")),
    ("failed to fill whole buffer", clave!("error.fin_archivo_inesperado")),
    ("invalid input parameter", clave!("error.dato_no_valido")),
];

/// Traduce los errores del sistema en ingles que aparezcan en `t`: `Texto (os error N)` de los numeros conocidos y los textos
/// de clase de `std::io::Error` (`entity not found`...) cuando son el mensaje entero (tras `: `, `(` o al principio, y hasta
/// el final, `)`, `.`, `,` o `;`). Lo demas no se toca. Pura.
pub fn traducir_errores(t: &str) -> String {
    let mut s = t.to_string();
    for (n, ingles, es) in ERRORES_SO {
        s = s.replace(&format!("{} (os error {})", ingles, n), texto(es));
    }
    for (ingles, es) in ERRORES_CLASE {
        let es = texto(es);
        let mut desde = 0;
        while let Some(i) = s[desde..].find(ingles).map(|i| i + desde) {
            let (antes, fin) = (&s[..i], i + ingles.len());
            let empieza = antes.is_empty() || antes.ends_with(": ") || antes.ends_with('(');
            let acaba = s[fin..].is_empty() || s[fin..].starts_with([')', '.', ',', ';']);
            if empieza && acaba {
                s.replace_range(i..fin, es);
                desde = i + es.len();
            } else {
                desde = fin;
            }
        }
    }
    s
}

/// Un error de E/S en espanol: por su numero de error si es uno conocido, si no por su clase, y si no su texto.
pub fn error_io(e: &std::io::Error) -> String {
    use std::io::ErrorKind as K;
    if let Some(es) = e.raw_os_error().and_then(|n| ERRORES_SO.iter().find(|(m, _, _)| *m == n)).map(|(_, _, es)| texto(es)) {
        return es.to_string();
    }
    let por_clase = match e.kind() {
        K::NotFound => tx!("error.no_existe"),
        K::PermissionDenied => tx!("error.sin_permiso"),
        K::AlreadyExists => tx!("error.ya_existe"),
        K::ConnectionRefused => tx!("error.conexion_rechazada"),
        K::TimedOut => tx!("error.agoto_tiempo_espera"),
        K::UnexpectedEof => tx!("error.fin_archivo_inesperado"),
        _ => return traducir_errores(&e.to_string()),
    };
    // un error con mensaje propio (no del sistema) conserva el mensaje
    if e.raw_os_error().is_none() && e.get_ref().is_some() {
        return traducir_errores(&e.to_string());
    }
    por_clase.to_string()
}

/// Identificador de Flatpak de este proceso (`FLATPAK_ID`), si corre dentro de Flatpak.
pub fn flatpak_id() -> Option<String> {
    std::env::var("FLATPAK_ID").ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// Una orden de terminal de weft para un texto de la interfaz: «weft ARGS» y, dentro de Flatpak (`flatpak`: su
/// identificador), tambien la forma con que se lanza ahi. Pura.
pub fn orden_terminal(args: &str, flatpak: Option<&str>) -> String {
    match flatpak {
        Some(id) => txf!("comun.orden_terminal_flatpak", args, id),
        None => txf!("comun.orden_terminal", args),
    }
}

/// Las ordenes de consola entre acentos graves que traen los textos de otros modulos (`` `weft ARGS` ``, pensados para la
/// terminal) se escriben como el resto de la interfaz: con `orden_terminal` (y su forma de Flatpak, dentro de Flatpak).
/// Lo demas entre acentos graves, y un acento suelto, se deja como esta. Pura.
pub fn ordenes_de_consola(t: &str, flatpak: Option<&str>) -> String {
    let mut s = String::with_capacity(t.len());
    let mut resto = t;
    while let Some(i) = resto.find("`weft ") {
        let tras = &resto[i + 1..];
        let Some(j) = tras.find('`') else { break };
        let args = tras["weft ".len()..j].trim();
        s.push_str(&resto[..i]);
        if args.is_empty() {
            s.push_str(&resto[i..i + 1 + j + 1]);
        } else {
            s.push_str(&orden_terminal(args, flatpak));
        }
        resto = &tras[j + 1..];
    }
    s.push_str(resto);
    s
}

/// Ordenes largas de la ventana que se pueden cancelar sin riesgo: agregar una imagen (trabaja en una carpeta parcial que
/// solo se publica al final, y la siguiente vez se limpia) y reunir el informe (solo lee). Las demas (root, traductor,
/// disco) cambian el invitado o el disco a mitad de camino y no se cortan. Pura.
pub fn cancelable(clave: &str) -> bool {
    matches!(clave, "imagen" | "informe")
}

/// Duracion para un texto de progreso: "45 s", "3 min 05 s". Pura.
pub fn duracion_corta(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{} s", s)
    } else {
        format!("{} min {:02} s", s / 60, s % 60)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// servicios de fondo

/// Estado compartido entre los hilos de fondo y la ventana.
#[derive(Default)]
struct Compartido {
    mandos: Vec<(String, String, Option<String>)>,
    ilegibles: usize,
    qmp_ok: bool,
    maquina: Option<Maquina>,
    /// aviso breve para la barra superior (texto, cuando)
    aviso: Option<(String, Instant)>,
    /// mando -> (estado buscado: conectado?, desde cuando)
    ocupados: HashMap<String, (bool, Instant)>,
    /// resultado de `weft doctor` (None = aun no se pidio) y si se esta ejecutando
    doctor: Option<Vec<crate::ajustes::FilaDoctor>>,
    doctor_en_curso: bool,
    /// `resolution` en curso
    aplicando: bool,
    /// mensajes para la pantalla de configuracion (clave, tono, texto), que los recoge `tomar_avisos`
    avisos: Vec<(String, crate::ajustes::Tono, String)>,
    /// estado del root en el invitado y si se esta consultando
    root_estado: Option<crate::root::Estado>,
    root_consultando: bool,
    /// orden larga en curso (root, share): (clave del mensaje, texto de progreso)
    operacion: Option<(String, String)>,
    /// la orden en curso: desde cuando, su pid (lider de su grupo de procesos) si se puede cancelar, y si se cancelo
    operacion_desde: Option<Instant>,
    operacion_pid: Option<u32>,
    operacion_cancelada: bool,
    /// rutas elegidas en el selector de archivos del sistema, para la pantalla de configuracion (campo, ruta)
    elegidas: Vec<(crate::ajustes::Campo, String)>,
    /// hay un selector de archivos abierto
    eligiendo: bool,
    /// estado de la imagen y del disco (archivos locales) y si se esta consultando
    almacen: Option<crate::ajustes::Almacen>,
    almacen_consultando: bool,
    /// estado del traductor ARM en el invitado y si se esta consultando
    puente_estado: Option<crate::puente::Estado>,
    puente_consultando: bool,
    /// reinicio ordenado de Android en curso (desde cuando)
    reiniciando: Option<Instant>,
    /// adbd no respondio: se ofrece el reinicio completo de la maquina
    ofrecer_completo: bool,
    /// `stop` lanzado como proceso aparte y aun sin fallar: la maquina se esta apagando
    apagando: bool,
    /// la ventana pidio que la maquina termine (apagado o reinicio completo lanzados): su fin no es inesperado
    cierre_pedido: bool,
    /// lo ultimo que dijo `query-status` (None: no se pudo consultar), si Android dio boot_completed y si va sin KVM
    qemu_estado: Option<String>,
    arrancado: bool,
    tcg: bool,
    /// adb para la barra: desde cuando corre QEMU (primer `running` visto), si adbd contesto alguna vez y si rechazo al adb
    /// propio (pide RSA o TLS). Sin adb no se puede saber si Android termino de arrancar (ver `sin_adb`)
    en_marcha_desde: Option<Instant>,
    adb_contesto: bool,
    adb_rechazo: bool,
    /// el estado que vio el ultimo sondeo (para despertar a la ventana cuando cambia, tambien solo por el paso del tiempo)
    estado_visto: Estado,
    /// mandos conectados a la maquina (lo sondea el estado cada pocos segundos): con alguno, el salvapantallas del
    /// equipo no salta mientras se juega solo con el mando
    pads: usize,
}

impl Compartido {
    /// Lo que la barra dice de la maquina con lo sondeado (`estado_de`); sin adb (`sin_adb`), en marcha en cuanto QEMU corre.
    fn estado(&self) -> Estado {
        self.estado_tras(self.en_marcha_desde.map(|t| t.elapsed()))
    }

    /// `estado` con QEMU en marcha desde hace `en_marcha`.
    fn estado_tras(&self, en_marcha: Option<Duration>) -> Estado {
        let sin = sin_adb(self.adb_rechazo, self.adb_contesto, en_marcha);
        estado_de(self.qemu_estado.as_deref(), self.arrancado || sin, self.reiniciando.is_some(), self.apagando, self.tcg)
    }
}

/// Segundos que la barra superior muestra un aviso.
pub const AVISO_S: u64 = 6;

/// Que consulta el hilo de sondeo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sondeo {
    Nada = 0,
    /// mandos del equipo (cada 2 s)
    Mandos = 1,
    /// datos de la maquina por QMP (cada 5 s)
    Maquina = 2,
}

/// Lo que tarda como maximo en notarse una conexion o desconexion de un mando antes de dar el boton por libre.
const OCUPADO_S: u64 = 20;

pub struct Servicios {
    dir: PathBuf,
    comp: Arc<Mutex<Compartido>>,
    /// que consulta el hilo de sondeo: 0 nada, 1 mandos, 2 maquina
    vista: Arc<AtomicU8>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Servicios {
    /// `dir`: directorio de estado de la maquina. `wake`: despierta el bucle de la ventana para redibujar.
    pub fn new(dir: &Path, wake: Arc<dyn Fn() + Send + Sync>) -> Servicios {
        let s = Servicios { dir: dir.to_path_buf(), comp: Arc::new(Mutex::new(Compartido::default())), vista: Arc::new(AtomicU8::new(0)), wake };
        let (comp, vista, wake, d) = (s.comp.clone(), s.vista.clone(), s.wake.clone(), s.dir.clone());
        std::thread::spawn(move || sondear(&d, &comp, &vista, &wake));
        s
    }

    /// Que debe consultar el hilo de sondeo segun la seccion visible de la configuracion.
    pub fn mirar(&self, p: Sondeo) {
        self.vista.store(p as u8, Ordering::Relaxed);
    }

    /// Completa `base` con lo que saben los hilos de fondo.
    pub fn info(&self, mut base: Info) -> Info {
        let mut c = self.comp.lock().unwrap();
        let ahora = Instant::now();
        let conectados: Vec<(String, Option<String>)> = c.mandos.iter().map(|(p, _, id)| (p.clone(), id.clone())).collect();
        c.ocupados.retain(|p, (objetivo, desde)| {
            let actual = conectados.iter().find(|(q, _)| q == p).map(|(_, id)| id.is_some());
            actual != Some(*objetivo) && ahora.saturating_duration_since(*desde) < Duration::from_secs(OCUPADO_S)
        });
        base.mandos = c.mandos.iter().map(|(p, n, id)| Mando { path: p.clone(), name: n.clone(), conectado: id.clone(), ocupado: c.ocupados.contains_key(p) }).collect();
        base.ilegibles = c.ilegibles;
        base.qmp_ok = c.qmp_ok;
        base.estado = c.estado();
        base.reinicio_completo = c.ofrecer_completo;
        base.maquina = c.maquina.clone();
        // el aviso pasa por la red de seguridad (salvo las rutas del usuario que weft marco) y con los errores del sistema en espanol
        let aviso = c.aviso.as_ref().filter(|(_, t)| ahora.saturating_duration_since(*t) < Duration::from_secs(AVISO_S)).map(|(t, _)| t.clone());
        base.aviso = aviso.as_deref().map(para_mostrar);
        base.aviso_corto = aviso.as_deref().and_then(para_mostrar_corto);
        base.encendida = std::fs::metadata(self.dir.join("pid")).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
        base
    }

    /// Estado del doctor para la pantalla de configuracion: (filas, en curso).
    pub fn doctor(&self) -> (Option<Vec<crate::ajustes::FilaDoctor>>, bool) {
        let c = self.comp.lock().unwrap();
        (c.doctor.clone(), c.doctor_en_curso)
    }

    /// Lanza `doctor` en un hilo (solo lee; ejecuta `qemu --version` y consulta dispositivos).
    pub fn comprobar(&self) {
        {
            let mut c = self.comp.lock().unwrap();
            if c.doctor_en_curso {
                return;
            }
            c.doctor_en_curso = true;
        }
        let (comp, wake, dir) = (self.comp.clone(), self.wake.clone(), self.dir.clone());
        std::thread::spawn(move || {
            let filas: Vec<crate::ajustes::FilaDoctor> = crate::doctor::ejecutar(&State { dir })
                .into_iter()
                .map(|f| crate::ajustes::FilaDoctor { nivel: f.nivel, nombre: crate::textos::limpiar(f.nombre), detalle: crate::textos::limpiar(&f.detalle) })
                .collect();
            let mut c = comp.lock().unwrap();
            c.doctor = Some(filas);
            c.doctor_en_curso = false;
            drop(c);
            wake();
        });
    }

    pub fn root_estado(&self) -> (Option<crate::root::Estado>, bool) {
        let c = self.comp.lock().unwrap();
        (c.root_estado.clone(), c.root_consultando)
    }

    pub fn almacen(&self) -> (Option<crate::ajustes::Almacen>, bool) {
        let c = self.comp.lock().unwrap();
        (c.almacen.clone(), c.almacen_consultando)
    }

    pub fn puente_estado(&self) -> (Option<crate::puente::Estado>, bool) {
        let c = self.comp.lock().unwrap();
        (c.puente_estado.clone(), c.puente_consultando)
    }

    /// Mira la imagen y el disco de la maquina (archivos locales; lee la cabecera GPT del disco) en un hilo.
    pub fn almacen_actualizar(&self) {
        {
            let mut c = self.comp.lock().unwrap();
            if c.almacen_consultando {
                return;
            }
            c.almacen_consultando = true;
        }
        let (comp, wake, dir) = (self.comp.clone(), self.wake.clone(), self.dir.clone());
        std::thread::spawn(move || {
            let (img, disco) = crate::imagen::rutas_de_la_maquina(&dir);
            let (imagenes, actual) = crate::catalogo::filas_para(&dir);
            let a = crate::ajustes::Almacen { imagen: crate::imagen::estado_imagen(&img), disco: crate::imagen::estado_disco(&disco), imagenes, actual };
            let mut c = comp.lock().unwrap();
            c.almacen = Some(a);
            c.almacen_consultando = false;
            drop(c);
            wake();
        });
    }

    /// Consulta el estado del traductor ARM en el invitado (adb propio) en un hilo.
    pub fn puente_actualizar(&self) {
        {
            let mut c = self.comp.lock().unwrap();
            if c.puente_consultando {
                return;
            }
            c.puente_consultando = true;
        }
        let (comp, wake, dir) = (self.comp.clone(), self.wake.clone(), self.dir.clone());
        std::thread::spawn(move || {
            let st = State { dir };
            let r = crate::puente::estado(crate::adbcmd::cid_del_estado(&st));
            let mut c = comp.lock().unwrap();
            c.puente_consultando = false;
            match r {
                Ok(e) => c.puente_estado = Some(e),
                Err(e) => {
                    c.puente_estado = None;
                    c.avisos.push(("puente".to_string(), crate::ajustes::Tono::Aviso, txf!("aviso.no_pudo_consultar_estado", crate::textos::limpiar(&e))));
                }
            }
            drop(c);
            wake();
        });
    }

    /// Orden larga en curso: (clave, texto de progreso con el tiempo que lleva a partir de los 5 s).
    pub fn operacion(&self) -> Option<(String, String)> {
        let c = self.comp.lock().unwrap();
        let (k, t) = c.operacion.clone()?;
        let pasado = c.operacion_desde.map(|d| d.elapsed()).unwrap_or_default();
        Some((k, if pasado >= Duration::from_secs(5) { format!("{} ({})", t, duracion_corta(pasado)) } else { t }))
    }

    /// ¿Se puede cancelar la orden en curso? Solo las que no dejan nada a medias (ver `cancelable`).
    pub fn operacion_cancelable(&self) -> bool {
        let c = self.comp.lock().unwrap();
        c.operacion_pid.is_some() && !c.operacion_cancelada
    }

    /// Cancela la orden en curso (SIGTERM a su grupo de procesos: tambien a la herramienta que descomprime). El resultado
    /// llega como siempre, como mensaje en linea, diciendo que se cancelo.
    pub fn cancelar_operacion(&self) {
        let mut c = self.comp.lock().unwrap();
        if let (Some(pid), false) = (c.operacion_pid, c.operacion_cancelada) {
            c.operacion_cancelada = true;
            if let Some(op) = c.operacion.as_mut() {
                op.1 = tx!("aviso.cancelando").into();
            }
            drop(c);
            // al grupo entero (pid negativo); vm::signal solo admite procesos sueltos
            extern "C" {
                fn kill(pid: i32, sig: i32) -> i32;
            }
            if pid > 1 {
                unsafe { kill(-(pid as i32), 15) };
            }
            (self.wake)();
        }
    }

    /// Abre el selector de archivos del sistema (portal de escritorio, tambien dentro de Flatpak) para un campo de ruta,
    /// en un hilo. Lo elegido se recoge con `tomar_elegidas`; si no hay portal, el aviso lo dice bajo el campo.
    pub fn elegir(&self, campo: crate::ajustes::Campo) {
        {
            let mut c = self.comp.lock().unwrap();
            if c.eligiendo {
                return;
            }
            c.eligiendo = true;
        }
        let (comp, wake) = (self.comp.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let (titulo, carpeta) = campo.selector();
            let r = crate::dbus::elegir(titulo, carpeta);
            let mut c = comp.lock().unwrap();
            c.eligiendo = false;
            match r {
                Ok(Some(ruta)) => c.elegidas.push((campo, ruta)),
                Ok(None) => {}
                Err(e) => c.avisos.push((
                    crate::ajustes::Ajustes::clave_de(campo).to_string(),
                    crate::ajustes::Tono::Aviso,
                    txf!("aviso.no_pudo_abrir_selector", e),
                )),
            }
            drop(c);
            wake();
        });
    }

    pub fn tomar_elegidas(&self) -> Vec<(crate::ajustes::Campo, String)> {
        std::mem::take(&mut self.comp.lock().unwrap().elegidas)
    }

    /// Consulta el estado del root en el invitado (adb propio) en un hilo.
    pub fn root_actualizar(&self) {
        {
            let mut c = self.comp.lock().unwrap();
            if c.root_consultando {
                return;
            }
            c.root_consultando = true;
        }
        let (comp, wake, dir) = (self.comp.clone(), self.wake.clone(), self.dir.clone());
        std::thread::spawn(move || {
            let st = State { dir };
            let cid = crate::adbcmd::cid_del_estado(&st);
            let r = crate::root::proveedor().estado(cid);
            let mut c = comp.lock().unwrap();
            c.root_consultando = false;
            match r {
                Ok(e) => c.root_estado = Some(e),
                Err(e) => {
                    c.root_estado = None;
                    c.avisos.push(("root".to_string(), crate::ajustes::Tono::Aviso, txf!("aviso.no_pudo_consultar_estado", crate::textos::limpiar(&e))));
                }
            }
            drop(c);
            wake();
        });
    }

    /// Ejecuta `weft ARGS...` (con el mismo estado) en un hilo, con el progreso que la orden escribe en su salida de
    /// error (lineas `... texto`). Al terminar deja el resultado como mensaje en linea bajo `clave`.
    pub fn orden(&self, clave: &'static str, etiqueta: String, args: Vec<String>) {
        {
            let mut c = self.comp.lock().unwrap();
            if c.operacion.is_some() {
                return;
            }
            c.operacion = Some((clave.to_string(), etiqueta.clone()));
            (c.operacion_desde, c.operacion_pid, c.operacion_cancelada) = (Some(Instant::now()), None, false);
        }
        let (comp, wake, dir) = (self.comp.clone(), self.wake.clone(), self.dir.clone());
        std::thread::spawn(move || {
            use crate::ajustes::Tono;
            use std::io::{BufRead, Read};
            let (base, nombre) = (dir.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(), dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            let hijo = std::env::current_exe().map_err(|e| e.to_string()).and_then(|exe| {
                let mut cmd = std::process::Command::new(exe);
                // las carpetas de la ventana pasan a la orden (datos, config, cache, registros)
                for (k, v) in crate::rutas::actual().variables() {
                    cmd.env(k, v);
                }
                // en su propio grupo de procesos: cancelar llega tambien a lo que la orden lance (unzip, 7z)
                std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
                cmd.args(["--state-dir", &base, "--name", &nombre])
                    .args(&args)
                    // la salida de la orden es para la interfaz: texto generico (ver src/textos.rs)
                    .env(crate::textos::VARIABLE, "interfaz")
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .map_err(|e| e.to_string())
            });
            let (tono, texto) = match hijo {
                Err(e) => (Tono::Error, txf!("aviso.no_pudo_ejecutar", e)),
                Ok(mut h) => {
                    if cancelable(clave) {
                        comp.lock().unwrap().operacion_pid = Some(h.id());
                        wake();
                    }
                    let mut salida = h.stdout.take().unwrap();
                    let lector = std::thread::spawn(move || {
                        let mut t = String::new();
                        let _ = salida.read_to_string(&mut t);
                        t
                    });
                    let mut errores = String::new();
                    if let Some(se) = h.stderr.take() {
                        for l in std::io::BufReader::new(se).lines().map_while(Result::ok) {
                            if let Some(p) = l.strip_prefix("... ") {
                                let mut c = comp.lock().unwrap();
                                if let Some(op) = c.operacion.as_mut() {
                                    op.1 = crate::textos::limpiar(p.trim());
                                }
                                drop(c);
                                wake();
                            } else if !l.trim().is_empty() {
                                errores.push_str(l.trim());
                                errores.push('\n');
                            }
                        }
                    }
                    let ok = h.wait().map(|s| s.success()).unwrap_or(false);
                    let sal = lector.join().unwrap_or_default();
                    if comp.lock().unwrap().operacion_cancelada && !ok {
                        (Tono::Aviso, tx!("aviso.cancelado_no_cambio_nada").to_string())
                    } else {
                        resultado_de_orden(ok, &errores, &sal)
                    }
                }
            };
            let mut c = comp.lock().unwrap();
            c.operacion = None;
            (c.operacion_desde, c.operacion_pid, c.operacion_cancelada) = (None, None, false);
            c.avisos.push((clave.to_string(), tono, crate::textos::limpiar(&texto)));
            drop(c);
            wake();
            if clave == "imagen" || clave == "disco" || clave == "imagen.usar" {
                // la imagen o el disco cambiaron: se vuelve a mirar
                let (imagenes, actual) = crate::catalogo::filas_para(&dir);
                let a = crate::ajustes::Almacen {
                    imagen: crate::imagen::estado_imagen(&crate::imagen::rutas_de_la_maquina(&dir).0),
                    disco: crate::imagen::estado_disco(&crate::imagen::rutas_de_la_maquina(&dir).1),
                    imagenes,
                    actual,
                };
                comp.lock().unwrap().almacen = Some(a);
                wake();
            }
            if clave == "puente" {
                // el traductor cambio (o se restauro): se vuelve a consultar
                let st = State { dir: dir.clone() };
                let r = crate::puente::estado(crate::adbcmd::cid_del_estado(&st));
                comp.lock().unwrap().puente_estado = r.ok();
                wake();
            }
            if clave == "root" {
                // el estado cambio: se vuelve a consultar
                let st = State { dir };
                let r = crate::root::proveedor().estado(crate::adbcmd::cid_del_estado(&st));
                let mut c = comp.lock().unwrap();
                c.root_estado = r.ok();
                drop(c);
                wake();
            }
        });
    }

    /// El reinicio completo se esta ofreciendo (adbd no respondio al reinicio ordenado).
    pub fn reinicio_completo_ofrecido(&self) -> bool {
        self.comp.lock().unwrap().ofrecer_completo
    }

    pub fn reiniciando(&self) -> bool {
        self.comp.lock().unwrap().reiniciando.is_some()
    }

    /// Hay un apagado en curso (`apagar` lanzo `stop` y este no ha fallado).
    pub fn apagando(&self) -> bool {
        self.comp.lock().unwrap().apagando
    }

    /// La ventana pidio que la maquina termine (apagado o reinicio completo lanzados y sin fallar): cuando QEMU cierre
    /// la pantalla no hay que avisar de nada.
    pub fn cierre_pedido(&self) -> bool {
        self.comp.lock().unwrap().cierre_pedido
    }

    /// Mandos conectados a la maquina segun el ultimo sondeo del estado (cada pocos segundos; 0 sin acceso a QEMU).
    pub fn mandos_conectados(&self) -> usize {
        self.comp.lock().unwrap().pads
    }

    /// Abre la carpeta `ruta` con el programa del escritorio (`xdg-open`, sin shell) en un hilo. El resultado va a la
    /// barra y a la seccion Acerca de; si no se pudo, el aviso lleva la ruta para copiarla a mano.
    pub fn abrir_carpeta(&self, ruta: PathBuf) {
        let (comp, wake) = (self.comp.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let r = std::process::Command::new("xdg-open").arg(&ruta).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
            let (tono, t) = resultado_abrir_carpeta(&r, &ruta);
            if tono != crate::ajustes::Tono::Exito {
                // en el registro, sin las marcas invisibles de la ruta
                eprintln!("ventana: {}", t.replace([RUTA_INI, RUTA_FIN], ""));
            }
            Servicios::decir(&comp, &wake, "estado", tono, t);
        });
    }

    /// REINICIO ORDENADO de Android por el adb propio (lo mismo que `weft reboot`), con aviso de cuando vuelve a estar
    /// listo. Si adbd no responde en pocos segundos (o Android no termina de arrancar), deja ofrecido el reinicio completo de
    /// la maquina. Nunca usa `system_reset`: ver src/reinicio.rs.
    pub fn reiniciar_android(&self) {
        {
            let mut c = self.comp.lock().unwrap();
            if c.reiniciando.is_some() {
                return;
            }
            c.reiniciando = Some(Instant::now());
            c.ofrecer_completo = false;
        }
        let (comp, wake, dir) = (self.comp.clone(), self.wake.clone(), self.dir.clone());
        std::thread::spawn(move || {
            use crate::ajustes::Tono;
            let st = State { dir };
            let cid = crate::adbcmd::cid_del_estado(&st);
            let r = crate::reinicio::ordenado(cid);
            let decir = |tono: Tono, t: String, ofrecer: Option<bool>, fin: bool| {
                let mut c = comp.lock().unwrap();
                c.aviso = Some((t.clone(), Instant::now()));
                c.avisos.push(("reinicio".to_string(), tono, t));
                if let Some(o) = ofrecer {
                    c.ofrecer_completo = o;
                }
                if fin {
                    c.reiniciando = None;
                }
                drop(c);
                wake();
            };
            match crate::reinicio::decidir(st.running(), r.is_ok()) {
                crate::reinicio::Plan::MaquinaApagada => decir(Tono::Aviso, tx!("aviso.maquina_no_esta_marcha").into(), None, true),
                crate::reinicio::Plan::OfrecerCompleto => decir(Tono::Aviso, txf!("aviso.android_no_responde_adb", r.err().unwrap_or_default()), Some(true), true),
                crate::reinicio::Plan::Ordenado => {
                    decir(Tono::Exito, tx!("aviso.reiniciando_android_reinicio_ordenado").into(), None, false);
                    // adbd tarda unos segundos en caer; sin la pausa se leeria el boot_completed del Android que se va
                    std::thread::sleep(Duration::from_secs(12));
                    match crate::adb::esperar(cid, 150, true) {
                        Ok(_) => decir(Tono::Exito, tx!("aviso.android_volvio_arrancar").into(), None, true),
                        Err(_) => decir(Tono::Aviso, tx!("aviso.android_no_termino_arrancar").into(), Some(true), true),
                    }
                }
            }
        });
    }

    /// REINICIO COMPLETO de la maquina: lanza `weft restart` como proceso aparte (sobrevive a esta ventana, que se
    /// cierra cuando la maquina se apaga) con la misma linea de arranque. Esta ventana desaparece y se abre otra.
    pub fn reinicio_completo(&self) {
        use std::os::unix::process::CommandExt;
        let dir = self.dir.clone();
        {
            let mut c = self.comp.lock().unwrap();
            c.ofrecer_completo = false;
            c.aviso = Some((tx!("aviso.reinicio_completo_esta_ventana").to_string(), Instant::now()));
        }
        (self.wake)();
        let (base, nombre) = (dir.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(), dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        let log = std::fs::OpenOptions::new().create(true).write(true).truncate(true).open(dir.join("restart.log"));
        let r = std::env::current_exe().map_err(|e| e.to_string()).and_then(|exe| {
            let mut cmd = std::process::Command::new(exe);
            for (k, v) in crate::rutas::actual().variables() {
                cmd.env(k, v);
            }
            cmd.args(["--state-dir", &base, "--name", &nombre, "restart", "--timeout", "20"]).stdin(std::process::Stdio::null());
            match log.and_then(|l| l.try_clone().map(|l2| (l, l2))) {
                Ok((l, l2)) => cmd.stdout(l).stderr(l2),
                Err(_) => cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()),
            };
            // grupo de procesos propio: no cae con la ventana
            cmd.process_group(0).spawn().map(|_| ()).map_err(|e| e.to_string())
        });
        match r {
            Ok(()) => self.comp.lock().unwrap().cierre_pedido = true,
            Err(e) => {
                let mut c = self.comp.lock().unwrap();
                c.aviso = Some((txf!("aviso.no_pudo_lanzar_reinicio", e), Instant::now()));
                c.avisos.push(("reinicio".to_string(), crate::ajustes::Tono::Error, txf!("aviso.no_pudo_lanzar_reinicio", e)));
                drop(c);
                (self.wake)();
            }
        }
    }

    pub fn aplicando_resolucion(&self) -> bool {
        self.comp.lock().unwrap().aplicando
    }

    /// Mensajes para la pantalla de configuracion (se entregan una sola vez). Pasan por la red de seguridad salvo las rutas
    /// del usuario marcadas (`ruta_literal`), que siguen marcadas: `ajustes::mensaje` las muestra tal cual.
    pub fn tomar_avisos(&self) -> Vec<(String, crate::ajustes::Tono, String)> {
        std::mem::take(&mut self.comp.lock().unwrap().avisos).into_iter().map(|(k, tono, t)| (k, tono, limpiar_salvo_rutas(&t))).collect()
    }

    fn avisar_ajustes(comp: &Arc<Mutex<Compartido>>, wake: &Arc<dyn Fn() + Send + Sync>, clave: &str, tono: crate::ajustes::Tono, texto: String) {
        comp.lock().unwrap().avisos.push((clave.to_string(), tono, texto));
        wake();
    }

    /// `Aplicar resolucion ahora`: ejecuta `weft resolution ANCHOxALTO[@DPI]` (pide el modo, reinicia SurfaceFlinger
    /// y aplica la densidad; tarda unos 10-20 s y cierra las apps). La orden guarda ademas la resolucion en `config`.
    pub fn aplicar_resolucion(&self, spec: String) {
        {
            let mut c = self.comp.lock().unwrap();
            if c.aplicando {
                return;
            }
            c.aplicando = true;
        }
        let (comp, wake, dir) = (self.comp.clone(), self.wake.clone(), self.dir.clone());
        std::thread::spawn(move || {
            use crate::ajustes::Tono;
            let (base, nombre) = (dir.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(), dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            let res = std::env::current_exe().map_err(|e| e.to_string()).and_then(|exe| {
                let mut cmd = std::process::Command::new(exe);
                for (k, v) in crate::rutas::actual().variables() {
                    cmd.env(k, v);
                }
                cmd.args(["--state-dir", &base, "--name", &nombre, "resolution", &spec])
                    .stdin(std::process::Stdio::null())
                    .output()
                    .map_err(|e| e.to_string())
            });
            let (tono, texto) = match res {
                Ok(o) if o.status.success() => (Tono::Exito, txf!("aviso.resolucion_aplicada", spec)),
                Ok(o) => {
                    let sal = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
                    let ultima = sal.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or(tx!("aviso.sin_detalle")).trim().to_string();
                    (Tono::Error, txf!("aviso.no_pudo_aplicar", ultima))
                }
                Err(e) => (Tono::Error, txf!("aviso.no_pudo_aplicar", e)),
            };
            let mut c = comp.lock().unwrap();
            c.aplicando = false;
            c.avisos.push(("aplicar".to_string(), tono, texto));
            drop(c);
            wake();
        });
    }

    /// Aplica en caliente la conexion automatica de mandos (`gamepad` de la configuracion): arranca o detiene el servicio
    /// `gamepad-serve`. Necesita que la maquina se arrancara con puertos de reserva (`start --gamepad`); si no, avisa
    /// de que queda para el proximo arranque.
    pub fn mandos_auto(&self, auto: bool) {
        let (comp, wake, dir) = (self.comp.clone(), self.wake.clone(), self.dir.clone());
        std::thread::spawn(move || {
            use crate::ajustes::Tono;
            let st = State { dir: dir.clone() };
            let en_marcha = gamepad::servicios_en_marcha(&st);
            if auto {
                let puertos = Qmp::connect(&st.qmp()).and_then(|mut q| gamepad::tiene_puertos(&mut q));
                match puertos {
                    Ok(false) => return Servicios::avisar_ajustes(&comp, &wake, "gamepad", Tono::Aviso, tx!("aviso.maquina_arranco_sin_puertos").into()),
                    Err(e) => return Servicios::avisar_ajustes(&comp, &wake, "gamepad", Tono::Aviso, txf!("aviso.no_pudo_consultar_maquina", e)),
                    Ok(true) => {}
                }
                if !en_marcha.is_empty() {
                    return Servicios::avisar_ajustes(&comp, &wake, "gamepad", Tono::Exito, tx!("aviso.conexion_automatica_activa").into());
                }
                let r = std::env::current_exe().map_err(|e| e.to_string()).and_then(|exe| {
                    let (base, nombre) = (dir.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(), dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                    let log = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("gamepad.log")).map_err(|e| e.to_string())?;
                    let mut cmd = std::process::Command::new(exe);
                    for (k, v) in crate::rutas::actual().variables() {
                        cmd.env(k, v);
                    }
                    cmd.args(["--state-dir", &base, "--name", &nombre, "gamepad-serve", "auto"])
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(log)
                        .spawn()
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                });
                match r {
                    Ok(()) => Servicios::avisar_ajustes(&comp, &wake, "gamepad", Tono::Exito, tx!("aviso.conexion_automatica_activada_conectan").into()),
                    Err(e) => Servicios::avisar_ajustes(&comp, &wake, "gamepad", Tono::Error, txf!("aviso.no_pudo_iniciar_servicio", e)),
                }
            } else {
                for pid in &en_marcha {
                    vm::signal(*pid, 15);
                }
                Servicios::avisar_ajustes(&comp, &wake, "gamepad", Tono::Aviso, tx!("aviso.conexion_automatica_desactivada_mandos").into());
            }
        });
    }

    /// Aviso breve en la barra superior (lo ve tambien quien tiene la configuracion cerrada).
    pub fn avisar_barra(&self, t: &str) {
        self.comp.lock().unwrap().aviso = Some((t.to_string(), Instant::now()));
        (self.wake)();
    }

    /// Hay un aviso breve en la barra (hay que redibujar para que desaparezca a su hora).
    pub fn aviso_vigente(&self) -> bool {
        self.comp.lock().unwrap().aviso.as_ref().is_some_and(|(_, t)| t.elapsed() < Duration::from_secs(AVISO_S + 1))
    }

    /// Deja un mensaje: aviso breve en la barra superior (visible aunque la configuracion este cerrada) y linea en la
    /// seccion `clave` de la configuracion.
    fn decir(comp: &Arc<Mutex<Compartido>>, wake: &Arc<dyn Fn() + Send + Sync>, clave: &str, tono: crate::ajustes::Tono, t: String) {
        let mut c = comp.lock().unwrap();
        c.aviso = Some((t.clone(), Instant::now()));
        c.avisos.push((clave.to_string(), tono, t));
        drop(c);
        wake();
    }

    /// Captura de pantalla: archivo `captura-AAAAMMDD-HHMMSS.png` en la carpeta de imagenes del usuario (XDG) o, si no la
    /// hay o no se puede escribir en ella, en la carpeta de datos de la maquina (`carpetas_de_captura`). El aviso lleva la
    /// ruta completa (sin pasar por la red de seguridad: es una carpeta del usuario).
    pub fn capturar(&self) {
        let (comp, wake, dir) = (self.comp.clone(), self.wake.clone(), self.dir.clone());
        let qmp = self.dir.join("qmp.sock").to_string_lossy().into_owned();
        std::thread::spawn(move || {
            use crate::ajustes::Tono;
            let ahora = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let nombre = nombre_captura(ahora);
            let carpetas = carpetas_de_captura(carpeta_imagenes_del_usuario(), &datos_de_la_maquina(&dir), &crate::rutas::maquina_de(&dir));
            let r = elegir_carpeta(&carpetas).and_then(|c| vm::captura(&qmp, &c.join(&nombre).to_string_lossy()));
            match r {
                Ok((ruta, _)) => {
                    eprintln!("ventana: captura guardada en {}", ruta);
                    Servicios::decir(&comp, &wake, "controles", Tono::Exito, txf!("aviso.captura_guardada", ruta_literal(Path::new(&ruta))));
                }
                Err(e) => Servicios::decir(&comp, &wake, "controles", Tono::Error, txf!("aviso.no_pudo_hacer_captura", e)),
            }
        });
    }

    /// APAGADO de la maquina: lanza `weft stop` como proceso aparte (sobrevive a esta ventana, que se cierra cuando
    /// QEMU termina). Es el mismo codigo que la orden de consola: apagado ordenado por adb, despues ACPI, `quit` y SIGKILL
    /// (src/apagado.rs). Devuelve si el proceso se lanzo (o ya habia un apagado en curso): mientras tanto `apagando`
    /// queda anotado y la barra lo muestra; si `stop` termina con error, se levanta el estado y se avisa.
    pub fn apagar(&self) -> bool {
        use std::os::unix::process::CommandExt;
        if self.apagando() {
            return true;
        }
        let dir = self.dir.clone();
        let (base, nombre) = (dir.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(), dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        let log = std::fs::OpenOptions::new().create(true).write(true).truncate(true).open(dir.join("stop.log"));
        let r = std::env::current_exe().map_err(|e| e.to_string()).and_then(|exe| {
            let mut cmd = std::process::Command::new(exe);
            for (k, v) in crate::rutas::actual().variables() {
                cmd.env(k, v);
            }
            cmd.args(["--state-dir", &base, "--name", &nombre, "stop", "--timeout", "20"]).stdin(std::process::Stdio::null());
            match log.and_then(|l| l.try_clone().map(|l2| (l, l2))) {
                Ok((l, l2)) => cmd.stdout(l).stderr(l2),
                Err(_) => cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()),
            };
            cmd.process_group(0).spawn().map_err(|e| e.to_string())
        });
        match r {
            Ok(mut hijo) => {
                {
                    let mut c = self.comp.lock().unwrap();
                    c.apagando = true;
                    c.cierre_pedido = true;
                }
                Servicios::decir(&self.comp, &self.wake, "apagar", crate::ajustes::Tono::Aviso, tx!("aviso.apagando_maquina").into());
                // si `stop` falla, la maquina sigue en marcha: se deja de mostrar "Apagando..." y se avisa
                let (comp, wake) = (self.comp.clone(), self.wake.clone());
                std::thread::spawn(move || {
                    let ok = hijo.wait().is_ok_and(|e| e.success());
                    if !ok {
                        {
                            let mut c = comp.lock().unwrap();
                            c.apagando = false;
                            c.cierre_pedido = false;
                        }
                        Servicios::decir(&comp, &wake, "apagar", crate::ajustes::Tono::Error, tx!("aviso.no_pudo_apagar_maquina").into());
                    }
                });
                true
            }

            Err(e) => {
                Servicios::decir(&self.comp, &self.wake, "apagar", crate::ajustes::Tono::Error, txf!("aviso.no_pudo_apagar", e));
                false
            }
        }
    }

    /// Conecta o desconecta un mando del equipo a la maquina (mismo codigo que `gamepad attach|detach`).
    pub fn mando(&self, path: String, conectar: bool) {
        let (comp, wake, dir) = (self.comp.clone(), self.wake.clone(), self.dir.clone());
        comp.lock().unwrap().ocupados.insert(path.clone(), (conectar, Instant::now()));
        std::thread::spawn(move || {
            use crate::ajustes::Tono;
            let st = State { dir };
            let r = if conectar {
                gamepad::conectar_a_mano(&st, &path)
            } else {
                let (pads, _) = gamepad::enumerar();
                gamepad::desconectar_a_mano(&st, &path, &pads)
            };
            match r {
                Ok(m) => Servicios::decir(&comp, &wake, "mandos", Tono::Exito, m),
                Err(e) => {
                    comp.lock().unwrap().ocupados.remove(&path);
                    Servicios::decir(&comp, &wake, "mandos", Tono::Error, txf!("aviso.mando", e));
                }
            }
        });
    }
}

/// Resultado de una orden larga para la interfaz: el tono y la frase (lo que escribio en su salida, en una linea, o "Hecho."; en
/// caso de fallo, con el motivo). Es la red de seguridad: lo que la orden diga pasa por `textos::limpiar`.
pub(crate) fn resultado_de_orden(ok: bool, errores: &str, salida: &str) -> (crate::ajustes::Tono, String) {
    use crate::ajustes::Tono;
    let resumen = |t: &str| {
        let v: Vec<&str> = t.lines().map(|l| l.trim().trim_end_matches('.')).filter(|l| !l.is_empty()).collect();
        let mut r = v.join(". ");
        if let Some(c) = r.chars().next() {
            r = c.to_uppercase().chain(r.chars().skip(1)).collect::<String>() + ".";
        }
        r
    };
    if ok {
        let r = resumen(salida);
        (Tono::Exito, crate::textos::limpiar(&if r.is_empty() { tx!("aviso.hecho").to_string() } else { r }))
    } else {
        let r = resumen(&format!("{}\n{}", errores, salida));
        (Tono::Error, crate::textos::limpiar(&txf!("aviso.no_pudo_completar", if r.is_empty() { tx!("aviso.sin_detalle").to_string() } else { r })))
    }
}

/// Mensaje para la barra y la seccion Maquina tras intentar abrir la carpeta de estado con `xdg-open`: si no se pudo,
/// lleva la ruta para abrirla a mano.
pub(crate) fn resultado_abrir_carpeta(r: &std::io::Result<std::process::ExitStatus>, ruta: &Path) -> (crate::ajustes::Tono, String) {
    use crate::ajustes::Tono;
    match r {
        Ok(s) if s.success() => (Tono::Exito, tx!("aviso.carpeta_estado_abierta").into()),
        Ok(s) => (Tono::Aviso, txf!("aviso.no_pudo_abrir_carpeta", s.code().map_or(tx!("aviso.senal").to_string(), |c| c.to_string()), ruta_literal(ruta))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Tono::Aviso, txf!("aviso.no_hay_programa_para", ruta_literal(ruta))),
        Err(e) => (Tono::Aviso, txf!("aviso.no_pudo_abrir_carpeta_2", error_io(e), ruta_literal(ruta))),
    }
}

/// Cada cuanto se mira el estado de la maquina para la barra (siempre, este o no abierta la configuracion).
const ESTADO_CADA: Duration = Duration::from_secs(3);

/// Archivo del estado donde el vigilante del arranque (`compartir::vigilar`) deja el resultado del root automatico: una
/// linea `ok TEXTO` o `error TEXTO`. La ventana lo muestra en la barra y en la seccion Acceso root cuando cambia.
pub const ROOT_AUTO: &str = "root-auto";

/// Hilo de sondeo: el estado de la maquina para la barra cada 3 s (siempre); mandos cada 2 s con la seccion Entrada a la
/// vista; datos de la maquina cada 5 s con Maquina o Controles.
fn sondear(dir: &Path, comp: &Arc<Mutex<Compartido>>, vista: &Arc<AtomicU8>, wake: &Arc<dyn Fn() + Send + Sync>) {
    let qmp = dir.join("qmp.sock").to_string_lossy().into_owned();
    let cid = crate::adbcmd::cid_del_estado(&State { dir: dir.to_path_buf() });
    // `start --accel tcg`: sin aceleracion aunque QEMU no conteste (si contesta, `query-kvm` manda)
    if crate::reinicio::Arranque::leer(dir).is_ok_and(|a| tcg_pedido(&a.args)) {
        comp.lock().unwrap().tcg = true;
    }
    let mut ultimo: HashMap<u8, Instant> = HashMap::new();
    let mut anterior = 0u8;
    let mut t_estado: Option<Instant> = None;
    let mut root_auto_visto: Option<SystemTime> = std::fs::metadata(dir.join(ROOT_AUTO)).and_then(|m| m.modified()).ok();
    loop {
        std::thread::sleep(Duration::from_millis(250));
        if t_estado.map_or(true, |t| t.elapsed() >= ESTADO_CADA) {
            t_estado = Some(Instant::now());
            if sondear_estado(&qmp, cid, comp) {
                wake();
            }
            // el resultado del root automatico de este arranque (lo deja el vigilante del arranque)
            let mt = std::fs::metadata(dir.join(ROOT_AUTO)).and_then(|m| m.modified()).ok();
            if mt.is_some() && mt != root_auto_visto {
                root_auto_visto = mt;
                if let Some((tono, t)) = aviso_root_auto(&std::fs::read_to_string(dir.join(ROOT_AUTO)).unwrap_or_default()) {
                    Servicios::decir(comp, wake, "root", tono, t);
                }
            }
        }
        let v = vista.load(Ordering::Relaxed);
        if v == 0 {
            anterior = 0;
            continue;
        }
        let cada = Duration::from_secs(if v == 1 { 2 } else { 5 });
        if v == anterior && ultimo.get(&v).is_some_and(|t| t.elapsed() < cada) {
            continue;
        }
        anterior = v;
        ultimo.insert(v, Instant::now());
        if v == 1 {
            let (pads, ilegibles) = gamepad::enumerar();
            let con = Qmp::connect(&qmp).and_then(|mut q| gamepad::conectados(&mut q));
            let mut c = comp.lock().unwrap();
            let ok = con.is_ok();
            let con = con.unwrap_or_default();
            let nuevos: Vec<(String, String, Option<String>)> =
                pads.iter().map(|p| (p.path.clone(), p.name.clone(), con.iter().find(|(_, e)| *e == p.path).map(|(id, _)| id.clone()))).collect();
            let cambio = c.mandos != nuevos || c.ilegibles != ilegibles || c.qmp_ok != ok;
            c.mandos = nuevos;
            c.ilegibles = ilegibles;
            c.qmp_ok = ok;
            drop(c);
            if cambio {
                wake();
            }
        } else {
            let m = maquina_info(&qmp);
            let mut c = comp.lock().unwrap();
            c.qmp_ok = m.is_ok();
            let nuevo = m.ok();
            let cambio = c.maquina != nuevo;
            c.maquina = nuevo;
            drop(c);
            if cambio {
                wake();
            }
        }
    }
}

/// Un paso del sondeo del estado para la barra: `query-status` y `query-kvm` por QMP y, con QEMU en marcha,
/// `sys.boot_completed` por el adb propio (unos ms; sin adbd, Android esta arrancando, salvo que adbd rechace al adb propio
/// o no conteste nunca: `sin_adb`). Deja lo visto en `Compartido` y devuelve si el estado resumido cambio (hay que
/// redibujar).
fn sondear_estado(qmp: &str, cid: u32, comp: &Arc<Mutex<Compartido>>) -> bool {
    let q = Qmp::connect(qmp).and_then(|mut q| {
        let estado = q.exec("query-status", None)?.get("status").and_then(|s| s.as_str()).unwrap_or("?").to_string();
        let kvm = q.exec("query-kvm", None).ok().and_then(|v| v.get("enabled").and_then(|b| b.as_bool()));
        let pads = gamepad::conectados(&mut q).map_or(0, |v| v.len());
        Ok((estado, kvm, pads))
    });
    // (arrancado, adbd contesto, adbd rechazo al adb propio)
    let (arrancado, contesto, rechazo) = match &q {
        Ok((e, _, _)) if e == "running" => match crate::adb::conectar(cid, 3) {
            Ok(mut a) => (a.shell_texto("getprop sys.boot_completed").is_ok_and(|(_, o, _)| o.trim() == "1"), true, false),
            Err(e) => (false, false, crate::adb::es_rechazo(&e)),
        },
        _ => (false, false, false),
    };
    let mut c = comp.lock().unwrap();
    match q {
        Ok((e, kvm, pads)) => {
            if e == "running" && c.en_marcha_desde.is_none() {
                c.en_marcha_desde = Some(Instant::now());
            }
            c.qemu_estado = Some(e);
            if let Some(k) = kvm {
                c.tcg = !k;
            }
            c.pads = pads;
        }
        Err(_) => {
            c.qemu_estado = None;
            c.pads = 0;
        }
    }
    c.arrancado = arrancado;
    c.adb_contesto |= contesto;
    c.adb_rechazo = rechazo || (c.adb_rechazo && !contesto);
    let ahora = c.estado();
    std::mem::replace(&mut c.estado_visto, ahora) != ahora
}

/// Aviso para la barra y la seccion Acceso root con lo que dejo el root automatico de este arranque (archivo
/// `root-auto`: `ok TEXTO` o `error TEXTO`). Un `ok` sin texto no dice nada; lo demas pasa por `textos::limpiar`.
pub fn aviso_root_auto(contenido: &str) -> Option<(crate::ajustes::Tono, String)> {
    use crate::ajustes::Tono;
    let l = contenido.lines().find(|l| !l.trim().is_empty())?.trim();
    let (clase, texto) = l.split_once(char::is_whitespace).unwrap_or((l, ""));
    let texto = crate::textos::limpiar(texto.trim());
    match clase {
        "ok" if texto.is_empty() => None,
        "ok" => Some((Tono::Exito, txf!("aviso.root_arrancar", texto))),
        "error" => Some((Tono::Error, txf!("aviso.root_automatico_fallo", if texto.is_empty() { tx!("aviso.sin_detalle").to_string() } else { texto }))),
        _ => None,
    }
}

/// Estado, CPUs, memoria y tipo de la maquina por QMP (cuatro ordenes baratas sobre una sola conexion).
fn maquina_info(qmp: &str) -> Result<Maquina, String> {
    let mut q = Qmp::connect(qmp)?;
    let estado = q.exec("query-status", None)?.get("status").and_then(|s| s.as_str()).unwrap_or("?").to_string();
    let cpus = q.exec("query-cpus-fast", None)?.as_arr().map_or(0, |a| a.len());
    let mem = q.exec("query-memory-size-summary", None)?;
    let mem_mb = match mem.get("base-memory") {
        Some(V::Num(n)) => (*n / 1048576.0).round() as u64,
        _ => 0,
    };
    let tipo = q
        .exec("qom-get", Some(V::obj(&[("path", V::s("/machine")), ("property", V::s("type"))])))
        .ok()
        .and_then(|t| t.as_str().map(|s| s.to_string()))
        .unwrap_or_default();
    Ok(Maquina { cpus, mem_mb, tipo, estado })
}

// ---------------------------------------------------------------------------------------------------------------
// fin inesperado de la maquina

/// Lineas que muestra la caja de despedida del registro de QEMU.
pub const LINEAS_REGISTRO: usize = 15;

/// Las ultimas `n` lineas no vacias de un registro, recortadas y pasadas por `textos::limpiar`.
pub fn ultimas_lineas(registro: &str, n: usize) -> Vec<String> {
    let todas: Vec<&str> = registro.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    todas[todas.len().saturating_sub(n)..].iter().map(|l| crate::textos::limpiar(l)).collect()
}

/// QEMU termino porque alguien se lo pidio (segun `events.log`, que escribe `events-serve`): un `SHUTDOWN` por orden
/// del anfitrion (`weft stop` o `restart`: `quit` por QMP) o del propio invitado (apagado desde Android o por adb), o un
/// cierre forzado pedido (`weft kill`, o el ultimo paso de `weft stop`: ver `linea_cierre_forzado`). Un panico del
/// invitado, una senal al proceso que nadie anoto o ningun aviso de apagado (muerte de golpe) no cuentan como pedido.
/// Es el criterio de la ventana para avisar y para anotar ultimo-fallo.txt.
pub fn cierre_ordenado(eventos: &str) -> bool {
    if eventos.lines().any(|l| l.split_whitespace().nth(1) == Some(CIERRE_FORZADO)) {
        return true;
    }
    eventos.lines().rev().filter_map(|l| l.split_once(" SHUTDOWN ")).next().is_some_and(|(_, datos)| {
        let d = datos.to_ascii_lowercase();
        ["host-qmp-quit", "guest-shutdown", "host-ui", "host-qmp-system-reset", "guest-reset"].iter().any(|r| d.contains(r))
    })
}

/// Marca de `events.log` de un cierre forzado pedido (SIGKILL a QEMU): QEMU no avisa de nada al recibirlo.
const CIERRE_FORZADO: &str = "KILL";

/// Linea que `weft kill` (y `weft stop` antes de su SIGKILL) anaden a `events.log` (`ts`: segundos desde 1970; `por`: quien
/// lo pidio): con ella la ventana sabe que el final fue pedido. Pura.
pub fn linea_cierre_forzado(ts: u64, por: &str) -> String {
    format!("{} {} {}", ts, CIERRE_FORZADO, por)
}

/// `events-serve` anoto que QEMU termino (linea `FIN`): el proceso ya no existe.
pub fn fin_anotado(eventos: &str) -> bool {
    eventos.lines().any(|l| l.split_whitespace().nth(1) == Some("FIN"))
}

/// Explicacion para la gente de por que termino la maquina, segun `events.log` (`en_marcha`: QEMU sigue vivo y solo se
/// perdio la conexion con su pantalla; `conexion`: lo que dijo esa conexion, que va al final entre parentesis; `flatpak`: el
/// identificador de Flatpak, para dar la orden de apagado completa).
pub fn motivo_del_fin(eventos: &str, conexion: &str, en_marcha: bool, flatpak: Option<&str>) -> String {
    if en_marcha {
        return txf!("fin.maquina_sigue_marcha_pero", conexion, orden_terminal("stop", flatpak));
    }
    let razon = eventos.lines().rev().find_map(|l| l.split_once(" SHUTDOWN ")).map(|(_, d)| d.to_ascii_lowercase());
    let panico = eventos.lines().any(|l| l.split_whitespace().nth(1) == Some("GUEST_PANICKED")) || razon.as_deref().is_some_and(|r| r.contains("guest-panic"));
    if panico {
        tx!("fin.android_sufrio_fallo_grave").to_string()
    } else if razon.as_deref().is_some_and(|r| r.contains("host-error")) {
        tx!("fin.maquina_detuvo_error_emulador").to_string()
    } else if razon.as_deref().is_some_and(|r| r.contains("host-signal")) {
        tx!("fin.emulador_recibio_senal_terminacion").to_string()
    } else if fin_anotado(eventos) {
        tx!("fin.maquina_termino_golpe_sin").to_string()
    } else {
        txf!("fin.maquina_termino_sin_pidiera", conexion)
    }
}

/// Lo ultimo de un archivo de registro (hasta `max` bytes, desde el principio de una linea), sin fallar si no existe o
/// no es UTF-8.
pub fn cola_de_archivo(ruta: &Path, max: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(ruta) else {
        return String::new();
    };
    let largo = f.metadata().map_or(0, |m| m.len());
    let cortado = largo > max && f.seek(SeekFrom::Start(largo - max)).is_ok();
    let mut b = Vec::new();
    let _ = f.read_to_end(&mut b);
    let t = String::from_utf8_lossy(&b).into_owned();
    match (cortado, t.split_once('\n')) {
        // la primera linea puede venir a medias
        (true, Some((_, resto))) => resto.to_string(),
        _ => t,
    }
}

/// Caja modal que la ventana muestra cuando la maquina termina sin que el usuario lo pidiera: el motivo, las ultimas
/// lineas de `qemu.log` y un boton Cerrar. La ventana sigue viva hasta que se pulsa (o Intro, Espacio o Esc). No conoce
/// SDL: calcula la geometria y entrega el dibujo como `Pint`, como la barra.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Despedida {
    pub titulo: String,
    pub motivo: String,
    pub lineas: Vec<String>,
    pub hover: bool,
    pub pulsado: bool,
}

/// Margenes y medidas de la caja de despedida (dp).
const DESP_ANCHO: f32 = 560.0;
const DESP_PAD: f32 = 16.0;
const DESP_BH: f32 = 28.0;

impl Despedida {
    /// `en_marcha`: la maquina sigue viva (solo se perdio la pantalla); `motivo`: lo que dijo la conexion; `registro`: el
    /// contenido de `qemu.log` (se quedan sus ultimas lineas, limpias de nombres de proveedores).
    pub fn nueva(en_marcha: bool, motivo: &str, registro: &str) -> Despedida {
        let titulo = if en_marcha { tx!("fin.perdio_pantalla_maquina") } else { tx!("fin.maquina_cerro_forma_inesperada") };
        Despedida { titulo: titulo.to_string(), motivo: para_mostrar(motivo), lineas: ultimas_lineas(registro, LINEAS_REGISTRO), hover: false, pulsado: false }
    }

    /// Nombre estable del boton (para el gancho de pruebas de la ventana).
    pub const BOTON: &'static str = "fin-cerrar";

    /// Caja, lineas del registro que caben y boton: (caja, lineas visibles, boton).
    fn geometria(&self, vent: (f32, f32), m: &dyn Medida) -> (R, Vec<String>, R) {
        let w = DESP_ANCHO.min(vent.0 - 2.0 * DESP_PAD).max(0.0);
        let interior = w - 2.0 * DESP_PAD;
        let (lht, lhc, lhp) = (m.alto_linea(Estilo::Titulo).ceil(), m.alto_linea(Estilo::Cuerpo).ceil(), m.alto_linea(Estilo::Pequeno).ceil());
        let motivo = envolver(m, &self.motivo, Estilo::Cuerpo, interior, 3);
        // alto fijo: titulo, motivo, etiqueta del registro, boton y nota
        let fijo = DESP_PAD + lht + 8.0 + motivo.len() as f32 * lhc + 10.0 + lhp + 4.0 + 12.0 + DESP_BH + 8.0 + lhp + DESP_PAD;
        let sitio = ((vent.1 - 2.0 * DESP_PAD - fijo) / lhp).floor().max(0.0) as usize;
        let n = self.lineas.len().min(sitio);
        let lineas: Vec<String> = self.lineas[self.lineas.len() - n..].iter().map(|l| truncar(m, l, Estilo::Pequeno, interior - 8.0)).collect();
        let h = fijo + n as f32 * lhp;
        let r = R::new(((vent.0 - w) / 2.0).round(), ((vent.1 - h) / 2.0).max(DESP_PAD).round(), w, h);
        let bw = (m.ancho(tx!("fin.cerrar"), Estilo::Negrita) + 32.0).max(96.0).ceil().min(interior.max(0.0));
        let boton = R::new(r.x + DESP_PAD, r.y + h - DESP_PAD - lhp - 8.0 - DESP_BH, bw, DESP_BH);
        (r, lineas, boton)
    }

    /// Rectangulo del boton Cerrar en la ventana.
    pub fn boton(&self, vent: (f32, f32), m: &dyn Medida) -> R {
        self.geometria(vent, m).2
    }

    /// El raton se mueve a (x, y). true si cambio el resaltado.
    pub fn mover(&mut self, vent: (f32, f32), m: &dyn Medida, x: f32, y: f32) -> bool {
        let h = self.boton(vent, m).contiene(x, y);
        let cambio = h != self.hover;
        self.hover = h;
        cambio
    }

    pub fn presionar(&mut self, vent: (f32, f32), m: &dyn Medida, x: f32, y: f32) {
        self.pulsado = self.boton(vent, m).contiene(x, y);
    }

    /// Boton izquierdo soltado: true si hay que cerrar la ventana (se pulso y se solto sobre Cerrar).
    pub fn soltar(&mut self, vent: (f32, f32), m: &dyn Medida, x: f32, y: f32) -> bool {
        let p = std::mem::take(&mut self.pulsado);
        p && self.boton(vent, m).contiene(x, y)
    }

    /// Tecla pulsada (codigo de SDL): Intro, Intro del teclado numerico, Espacio y Esc cierran.
    pub fn tecla(&self, sc: u32) -> bool {
        matches!(sc, 40 | 88 | 44 | 41)
    }

    /// Dibujo: fondo atenuado, caja con borde rojo, titulo, motivo, registro y boton.
    pub fn dibujar(&self, vent: (f32, f32), m: &dyn Medida) -> Vec<Pint> {
        let (r, lineas, boton) = self.geometria(vent, m);
        let (lht, lhc, lhp) = (m.alto_linea(Estilo::Titulo).ceil(), m.alto_linea(Estilo::Cuerpo).ceil(), m.alto_linea(Estilo::Pequeno).ceil());
        let interior = r.w - 2.0 * DESP_PAD;
        let mut out = vec![
            Pint::Rect { r: R::new(0.0, 0.0, vent.0, vent.1), c: Color(0, 0, 0, 153), radio: 0.0 },
            Pint::Rect { r: r.reducir(-1.0), c: tema::p().error, radio: tema::RADIO + 3.0 },
            Pint::Rect { r, c: tema::p().superficie, radio: tema::RADIO + 2.0 },
        ];
        let (x, mut y) = (r.x + DESP_PAD, r.y + DESP_PAD);
        out.push(Pint::Texto { x, y, t: truncar(m, &self.titulo, Estilo::Titulo, interior), e: Estilo::Titulo, c: tema::p().error });
        y += lht + 8.0;
        for l in envolver(m, &self.motivo, Estilo::Cuerpo, interior, 3) {
            out.push(Pint::Texto { x, y, t: l, e: Estilo::Cuerpo, c: tema::p().texto });
            y += lhc;
        }
        y += 10.0;
        let etq = if lineas.is_empty() { tx!("fin.registro_maquina_qemu_log") } else { tx!("fin.ultimas_lineas_registro_maquina") };
        out.push(Pint::Texto { x, y, t: truncar(m, etq, Estilo::Pequeno, interior), e: Estilo::Pequeno, c: tema::p().texto2 });
        y += lhp + 4.0;
        if !lineas.is_empty() {
            out.push(Pint::Rect { r: R::new(x, y - 2.0, 2.0, lineas.len() as f32 * lhp + 4.0), c: tema::p().borde, radio: 1.0 });
        }
        for l in &lineas {
            out.push(Pint::Texto { x: x + 8.0, y, t: l.clone(), e: Estilo::Pequeno, c: tema::p().texto2 });
            y += lhp;
        }
        let fondo = if self.pulsado && self.hover {
            tema::p().acento
        } else if self.hover {
            tema::p().acento_hover
        } else {
            tema::p().acento
        };
        out.push(Pint::Rect { r: boton, c: fondo, radio: tema::RADIO });
        let t = tx!("fin.cerrar");
        let tx = boton.x + (boton.w - m.ancho(t, Estilo::Negrita)) / 2.0;
        let ty = boton.y + boton.h / 2.0 + m.cap(Estilo::Negrita) / 2.0 - m.ascenso(Estilo::Negrita);
        out.push(Pint::Texto { x: tx, y: ty, t: t.to_string(), e: Estilo::Negrita, c: tema::p().sobre_acento });
        let nota = tx!("fin.cerrar_termina_esta_ventana");
        // texto secundario (no deshabilitado): con contraste AA
        out.push(Pint::Texto { x, y: boton.y + boton.h + 8.0, t: truncar(m, nota, Estilo::Pequeno, interior), e: Estilo::Pequeno, c: tema::p().texto2 });
        out
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solo_se_cancelan_las_ordenes_que_no_dejan_nada_a_medias() {
        assert!(cancelable("imagen") && cancelable("informe"));
        for k in ["root", "puente", "disco", "imagen.usar", "share"] {
            assert!(!cancelable(k), "{}", k);
        }
        assert_eq!(duracion_corta(Duration::from_secs(45)), "45 s");
        assert_eq!(duracion_corta(Duration::from_secs(185)), "3 min 05 s");
    }

    #[test]
    fn la_ventana_es_barra_mas_dispositivo() {
        let bh = crate::barra::ALTO;
        // ajustada: la barra arriba, a todo el ancho; el dispositivo ocupa todo lo demas, justo debajo
        let d = disenar((720.0, 1348.0 + bh), (720, 1348));
        assert_eq!(d.barra, R::new(0.0, 0.0, 720.0, bh));
        assert_eq!(d.dispositivo, R::new(0.0, bh, 720.0, 1348.0));
        assert_eq!(d.escala, 1.0);
        // ventana mas ancha que la imagen: se centra (sin columna a la derecha)
        let c = disenar((1000.0, 1348.0 + bh), (720, 1348));
        assert_eq!(c.dispositivo, R::new(140.0, bh, 720.0, 1348.0));
        // girado (vista 1348x720) en una ventana que no la alcanza: escala menor y bandas arriba y abajo (bajo la barra)
        let g = disenar((674.0, 600.0 + bh), (1348, 720));
        assert_eq!(g.escala, 0.5);
        assert_eq!(g.dispositivo, R::new(0.0, bh + 120.0, 674.0, 360.0));
        // la barra y el dispositivo nunca se solapan y el dispositivo nunca se sale de la ventana
        for (win, vista) in [((936.0, 1382.0), (720, 1348)), ((700.0, 500.0), (1348, 720)), ((300.0, 200.0), (720, 1348)), ((320.0, 30.0), (720, 1348))] {
            let d = disenar(win, vista);
            assert!(d.dispositivo.y >= d.barra.y + d.barra.h - 0.01);
            assert!(d.dispositivo.x >= -0.01 && d.dispositivo.x + d.dispositivo.w <= win.0 + 0.01 && d.dispositivo.y + d.dispositivo.h <= win.1 + 0.01);
        }
    }

    #[test]
    fn de_la_ventana_a_la_vista() {
        let d = disenar((360.0, 700.0), (720, 1348));
        assert!((d.escala - 0.5).abs() < 0.03);
        // la barra y las bandas no son del dispositivo: un clic en la barra no llega al invitado
        assert_eq!(a_vista(&d, (720, 1348), (d.dispositivo.x + 10.0, d.barra.h - 1.0)), None);
        assert_eq!(a_vista(&d, (720, 1348), (d.dispositivo.x + 10.0, 5.0)), None);
        // el primer pixel del dispositivo esta justo bajo la barra
        let p0 = a_vista(&d, (720, 1348), (d.dispositivo.x + 0.1, d.dispositivo.y + 0.1)).unwrap();
        assert!(p0.0 < 1.0 && p0.1 < 1.0);
        let s = d.escala;
        let p = a_vista(&d, (720, 1348), (d.dispositivo.x + 100.0 * s, d.dispositivo.y + 200.0 * s)).unwrap();
        assert!((p.0 - 100.0).abs() < 1e-3 && (p.1 - 200.0).abs() < 1e-3);
        // la ultima fila y columna estan dentro; un pixel mas alla, no
        assert!(a_vista(&d, (720, 1348), (d.dispositivo.x + d.dispositivo.w - 0.01, d.dispositivo.y + d.dispositivo.h - 0.01)).is_some());
        assert_eq!(a_vista(&d, (720, 1348), (d.dispositivo.x - 0.5, d.dispositivo.y + 10.0)), None);
        // los toques siguen bien con rotacion: vista girada 90 -> punto del panel (ver pantalla::a_panel)
        let dg = disenar((674.0, 360.0 + crate::barra::ALTO), (1348, 720));
        let v = a_vista(&dg, (1348, 720), (dg.dispositivo.x + 1.0, dg.dispositivo.y + 1.0)).unwrap();
        assert!(v.0 < 3.0 && v.1 < 3.0);
        let panel = crate::pantalla::a_panel(1, (720, 1348), (v.0 as f64, v.1 as f64));
        assert!(panel.0 > 715.0 && panel.1 < 3.0);
    }

    #[test]
    fn zoom_pasos_y_ventana() {
        assert_eq!(paso_mas(100.0), 125);
        assert_eq!(paso_mas(62.0), 67);
        assert_eq!(paso_mas(300.0), 300);
        assert_eq!(paso_menos(100.0), 75);
        assert_eq!(paso_menos(62.0), 50);
        assert_eq!(paso_menos(10.0), 25);
        let bh = crate::barra::ALTO as i32;
        // 100 % en una pantalla grande: vista + barra
        let ((w, h), s) = ventana_para((720, 1348), Some(1.0), (3840, 2160));
        assert_eq!((w, h, s), (720, 1348 + bh, 1.0));
        // un zoom muy pequeno no baja de los minimos
        let ((w, h), _) = ventana_para((720, 1348), Some(0.1), (3840, 2160));
        assert_eq!((w, h), (ANCHO_MIN as i32, ALTO_MIN as i32 + bh));
        // no cabe en la pantalla del equipo: se reduce hasta el 95 % del area util (la barra incluida)
        let ((w, h), s) = ventana_para((720, 1348), Some(1.0), (2560, 1400));
        assert!(s < 1.0 && h <= (1400.0f32 * 0.95) as i32 + 1 && w as f32 <= 2560.0 * 0.95);
        // ajuste automatico (arranque): nunca por encima de 1:1 ni del 90 %
        let ((_, h), s) = ventana_para((720, 1348), None, (2560, 1400));
        assert!((s - (1400.0 * 0.9 - bh as f32) / 1348.0).abs() < 1e-3 && h <= 1260);
        let (_, s) = ventana_para((720, 1348), None, (3840, 2160));
        assert_eq!(s, 1.0);
        // area desconocida
        let ((w, h), s) = ventana_para((720, 1348), Some(0.5), (0, 0));
        assert_eq!((w, h, s), (360, 674 + bh, 0.5));
        // la ventana pedida da exactamente la escala pedida al disenarla
        let ((w, h), _) = ventana_para((720, 1348), Some(0.5), (3840, 2160));
        let d = disenar((w as f32, h as f32), (720, 1348));
        assert!((d.escala - 0.5).abs() < 0.01);
    }

    #[test]
    fn zoom_en_la_configuracion() {
        let mut c = Config::nueva();
        assert_eq!(zoom_de_config(&c), ModoZoom::Ajustar);
        assert!(zoom_a_config(ModoZoom::Fijo(67), &mut c));
        assert_eq!(c.get("zoom"), "67");
        assert_eq!(zoom_de_config(&c), ModoZoom::Fijo(67));
        // sin cambios no hay nada que guardar
        assert!(!zoom_a_config(ModoZoom::Fijo(67), &mut c));
        assert!(zoom_a_config(ModoZoom::Ajustar, &mut c));
        // y sobrevive al archivo
        zoom_a_config(ModoZoom::Fijo(50), &mut c);
        assert_eq!(zoom_de_config(&Config::parse(&c.texto())), ModoZoom::Fijo(50));
    }

    #[test]
    fn textos_auxiliares() {
        assert_eq!(duracion(Duration::from_secs(42)), "42s");
        assert_eq!(duracion(Duration::from_secs(312)), "5m 12s");
        assert_eq!(duracion(Duration::from_secs(3723)), "1h 02m");
        assert_eq!(tipo_corto("pc-q35-10.2-machine"), "q35 10.2");
        assert_eq!(estado_es("running"), tx!("estado_maquina.marcha"));
        assert_eq!(estado_es("raro"), "raro");
        // E.10: "Android" para el sistema de dentro, nunca "invitado"
        assert_eq!(estado_es("guest-panicked"), tx!("estado_maquina.android_fallo"));
        assert_eq!(tipo_corto("pc-i440fx-10.2-machine"), "i440fx 10.2");
        assert_eq!(nombre_captura(0), "captura-19700101-000000.png");
        assert_eq!(nombre_captura(1_790_000_000), "captura-20260921-141320.png");
        assert_eq!(nombre_captura(951_782_400 + 86399), "captura-20000229-235959.png");
    }

    #[test]
    fn nombres_y_confirmaciones_de_acciones() {
        assert_eq!(Accion::Rotacion(1).nombre(), "rot90");
        assert_eq!(Accion::RotacionAuto.nombre(), "rotauto");
        assert_eq!(Accion::Atajo(Atajo::VolBajar).nombre(), "vol-");
        assert_eq!(Accion::Configuracion.nombre(), "config");
        assert_eq!(Accion::PantallaCompleta.nombre(), "pantalla-completa");
        assert_eq!(Accion::Atajo(Atajo::Menu).nombre(), "menu");
        assert_eq!(Accion::Atajo(Atajo::Encendido).nombre(), "encendido");
        assert_eq!(Accion::Atajo(Atajo::Encendido).confirmacion(), tx!("enviado.encendido_enviado"));
        assert_eq!(Accion::Atajo(Atajo::Atras).confirmacion(), tx!("enviado.atras_enviado"));
        assert_eq!(Accion::Rotacion(3).confirmacion(), "Rotación fijada a 270°");
        // todas las acciones tienen nombre distinto
        let todas = [
            Accion::Atajo(Atajo::Atras),
            Accion::Atajo(Atajo::Inicio),
            Accion::Atajo(Atajo::Recientes),
            Accion::Atajo(Atajo::VolBajar),
            Accion::Atajo(Atajo::VolSubir),
            Accion::Atajo(Atajo::Rotar),
            Accion::Rotacion(0),
            Accion::Rotacion(1),
            Accion::Rotacion(2),
            Accion::Rotacion(3),
            Accion::RotacionAuto,
            Accion::Configuracion,
            Accion::ZoomMas,
            Accion::ZoomMenos,
            Accion::ZoomAjustar,
            Accion::Zoom1a1,
            Accion::Captura,
        ];
        let mut nombres: Vec<String> = todas.iter().map(|a| a.nombre()).collect();
        nombres.sort();
        nombres.dedup();
        assert_eq!(nombres.len(), todas.len());
    }

    /// No hay panel lateral: ningun fuente de la interfaz lo menciona como estructura ni como tecla.
    #[test]
    fn sin_panel_lateral() {
        let agujas = [concat!("Pan", "el::"), concat!("Accion::Ple", "gar"), concat!("pan.", "pleg"), concat!("ANCHO_PLE", "GADO"), concat!("aplicar_ple", "gado")];
        for (nombre, fuente) in [("vista.rs", include_str!("vista.rs")), ("window.rs", include_str!("window.rs")), ("barra.rs", include_str!("barra.rs")), ("ajustes.rs", include_str!("ajustes.rs"))] {
            for a in agujas {
                let en_la_prueba = nombre == "vista.rs" && fuente.matches(a).count() == 1;
                assert!(!fuente.contains(a) || en_la_prueba, "{} sigue usando {}", nombre, a);
            }
        }
    }

    /// El estado de la barra: lo que pidio la ventana manda (apagando sobre reiniciando), despues un QEMU parado, despues
    /// la falta de KVM y por ultimo si Android termino de arrancar.
    #[test]
    fn estado_de_la_maquina_para_la_barra() {
        assert_eq!(estado_de(None, false, false, false, false), Estado::Arrancando);
        assert_eq!(estado_de(Some("running"), false, false, false, false), Estado::Arrancando);
        assert_eq!(estado_de(Some("running"), true, false, false, false), Estado::EnMarcha);
        for s in ["paused", "suspended", "guest-panicked", "internal-error", "io-error", "prelaunch", "watchdog"] {
            assert_eq!(estado_de(Some(s), true, false, false, false), Estado::Pausada, "{}", s);
        }
        assert_eq!(estado_de(Some("shutdown"), true, false, false, false), Estado::Apagando);
        // lo que lanzo la ventana manda sobre lo sondeado
        assert_eq!(estado_de(Some("running"), true, true, false, false), Estado::Reiniciando);
        assert_eq!(estado_de(Some("paused"), false, true, false, false), Estado::Reiniciando);
        assert_eq!(estado_de(Some("running"), true, true, true, true), Estado::Apagando);
        assert_eq!(estado_de(None, false, false, true, false), Estado::Apagando);
        // sin KVM se dice siempre (arrancando o en marcha), pero no tapa una pausa
        assert_eq!(estado_de(Some("running"), true, false, false, true), Estado::SinAceleracion);
        assert_eq!(estado_de(None, false, false, false, true), Estado::SinAceleracion);
        assert_eq!(estado_de(Some("paused"), false, false, false, true), Estado::Pausada);
        // textos y colores del punto
        assert_eq!(Estado::EnMarcha.texto(), tx!("estado.android_marcha"));
        assert_eq!(Estado::SinAceleracion.texto(), tx!("estado.sin_aceleracion_lento"));
        assert_eq!((Estado::EnMarcha.color(), Estado::Pausada.color()), (tema::p().exito, tema::p().texto_apagado));
        for e in [Estado::Arrancando, Estado::Reiniciando, Estado::Apagando, Estado::SinAceleracion] {
            assert_eq!(e.color(), tema::p().aviso, "{:?}", e);
        }
        assert_eq!(Info::default().estado, Estado::Arrancando);
    }

    /// Sin adb propio (sin vsock ni puerto TCP, una imagen que pide RSA, un invitado que no es Android) la barra no se queda
    /// para siempre en "Arrancando Android...": adbd rechazo la conexion, o no contesto nunca en SIN_ADB_TRAS con QEMU en
    /// marcha. Si adbd contesto alguna vez, Android sigue arrancando hasta que diga boot_completed.
    #[test]
    fn sin_adb_la_barra_no_se_queda_arrancando() {
        let poco = Some(Duration::from_secs(20));
        assert!(!sin_adb(false, false, None) && !sin_adb(false, false, poco));
        assert!(sin_adb(false, false, Some(SIN_ADB_TRAS)));
        assert!(!sin_adb(false, true, Some(SIN_ADB_TRAS * 10)), "adbd contesto: Android esta arrancando");
        assert!(sin_adb(true, false, None) && sin_adb(true, false, poco));
        // lo que ve la barra con lo sondeado
        let mut c = Compartido { qemu_estado: Some("running".into()), en_marcha_desde: Some(Instant::now()), ..Compartido::default() };
        assert_eq!(c.estado(), Estado::Arrancando);
        let tarde = Some(SIN_ADB_TRAS + Duration::from_secs(1));
        assert_eq!(c.estado_tras(tarde), Estado::EnMarcha);
        c.adb_contesto = true;
        assert_eq!(c.estado_tras(tarde), Estado::Arrancando);
        c.arrancado = true;
        assert_eq!(c.estado_tras(tarde), Estado::EnMarcha);
        // adbd pide RSA: en marcha desde el principio; una pausa o un apagado siguen mandando
        let mut c = Compartido { qemu_estado: Some("running".into()), adb_rechazo: true, ..Compartido::default() };
        assert_eq!(c.estado(), Estado::EnMarcha);
        c.qemu_estado = Some("paused".into());
        assert_eq!(c.estado(), Estado::Pausada);
        c.apagando = true;
        assert_eq!(c.estado(), Estado::Apagando);
    }

    /// `start --accel tcg` en los argumentos guardados del arranque: la maquina va sin aceleracion.
    #[test]
    fn tcg_en_los_argumentos_del_arranque() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<String>>();
        assert!(tcg_pedido(&a(&["start", "--mem", "4096", "--accel", "tcg"])));
        assert!(tcg_pedido(&a(&["--accel=tcg"])));
        assert!(!tcg_pedido(&a(&["start", "--accel", "kvm"])));
        assert!(!tcg_pedido(&a(&["start", "--accel"])));
        assert!(!tcg_pedido(&a(&["tcg", "--accel"])));
        assert!(!tcg_pedido(&[]));
    }

    /// El resultado del root automatico (archivo `root-auto` del estado) se convierte en un aviso limpio para la barra.
    #[test]
    fn aviso_del_root_automatico() {
        use crate::ajustes::Tono;
        assert_eq!(aviso_root_auto(""), None);
        assert_eq!(aviso_root_auto("ok\n"), None);
        assert_eq!(aviso_root_auto("ok   \n"), None);
        assert_eq!(aviso_root_auto("ok cargado y listo\n"), Some((Tono::Exito, "Root al arrancar: cargado y listo".to_string())));
        assert_eq!(aviso_root_auto("\n\nerror faltan los archivos en /descargas\notra linea\n"), Some((Tono::Error, "El root automático falló: faltan los archivos en /descargas".to_string())));
        assert_eq!(aviso_root_auto("error"), Some((Tono::Error, "El root automático falló: sin detalle".to_string())));
        assert_eq!(aviso_root_auto("raro algo"), None);
        // un nombre de proveedor que se cuele en el detalle no llega a la barra
        let (_, t) = aviso_root_auto(&format!("error {} no se cargo", concat!("Kernel", "SU"))).unwrap();
        assert!(crate::textos::prohibida_en(&t).is_none(), "{}", t);
    }

    /// Mensaje tras abrir la carpeta de estado: si no se pudo, lleva la ruta.
    #[test]
    fn abrir_la_carpeta_de_estado() {
        use crate::ajustes::Tono;
        use std::os::unix::process::ExitStatusExt;
        let ruta = Path::new("/estado/prueba");
        let ok = resultado_abrir_carpeta(&Ok(std::process::ExitStatus::from_raw(0)), ruta);
        assert_eq!(ok, (Tono::Exito, tx!("aviso.carpeta_estado_abierta").to_string()));
        // la ruta va marcada como literal (no pasa por la red de seguridad) y al mostrarla las marcas no se ven
        let (t, m) = resultado_abrir_carpeta(&Ok(std::process::ExitStatus::from_raw(3 << 8)), ruta);
        assert!(t == Tono::Aviso && m.contains("devolvió 3") && m.ends_with(&ruta_literal(ruta)), "{}", m);
        assert!(para_mostrar(&m).ends_with("Está en /estado/prueba"), "{}", m);
        let (_, m) = resultado_abrir_carpeta(&Ok(std::process::ExitStatus::from_raw(9)), ruta);
        assert!(m.contains(tx!("aviso.senal")) && m.contains("/estado/prueba"), "{}", m);
        let (t, m) = resultado_abrir_carpeta(&Err(std::io::Error::from(std::io::ErrorKind::NotFound)), ruta);
        assert!(t == Tono::Aviso && m.starts_with("No hay programa para abrir carpetas") && para_mostrar(&m).ends_with("/estado/prueba"), "{}", m);
        // el error del sistema, en espanol
        let (_, m) = resultado_abrir_carpeta(&Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)), ruta);
        assert!(m.starts_with("No se pudo abrir la carpeta (sin permiso)") && para_mostrar(&m).ends_with("/estado/prueba"), "{}", m);
        let (_, m) = resultado_abrir_carpeta(&Err(std::io::Error::from_raw_os_error(13)), ruta);
        assert!(m.starts_with("No se pudo abrir la carpeta (sin permiso)"), "{}", m);
    }

    /// `events.log`: un apagado pedido (por el anfitrion, por Android o un cierre forzado anotado) no es inesperado; una
    /// muerte de golpe, un panico, una senal o un error si. El motivo que ve la gente sale de ahi.
    #[test]
    fn por_que_termino_la_maquina() {
        let quit = "1700000000 SHUTDOWN {\"guest\":false,\"reason\":\"host-qmp-quit\"}\n1700000001 FIN QEMU termino tras el aviso de apagado\n";
        assert!(cierre_ordenado(quit) && fin_anotado(quit));
        let android = "1 SHUTDOWN {\"guest\":true,\"reason\":\"guest-shutdown\"}\n";
        assert!(cierre_ordenado(android) && !fin_anotado(android));
        let golpe = "1 RESET {}\n2 FIN QEMU termino de golpe, sin aviso de apagado (senal, aborto o cierre forzado)\n";
        assert!(!cierre_ordenado(golpe) && fin_anotado(golpe));
        let panico = "1 GUEST_PANICKED {\"action\":\"pause\"}\n2 SHUTDOWN {\"guest\":true,\"reason\":\"guest-panic\"}\n3 FIN QEMU termino tras el aviso de apagado\n";
        assert!(!cierre_ordenado(panico));
        let senal = "1 SHUTDOWN {\"guest\":false,\"reason\":\"host-signal\"}\n";
        assert!(!cierre_ordenado(senal));
        let error = "1 SHUTDOWN {\"guest\":false,\"reason\":\"host-error\"}\n";
        assert!(!cierre_ordenado(error));
        assert!(!cierre_ordenado("") && !fin_anotado(""));
        // cuenta el ultimo apagado anotado
        assert!(!cierre_ordenado(&format!("{}{}", android, error)));
        assert!(cierre_ordenado(&format!("{}{}", error, android)));
        // una linea que solo menciona FIN en el texto no es el final
        assert!(!fin_anotado("1 SHUTDOWN {\"reason\":\"x FIN\"}\n"));
        // un cierre forzado pedido (`weft kill`, el SIGKILL de `stop`) cuenta como pedido aunque QEMU no avise de nada
        let kill = format!("{}\n2 FIN QEMU termino de golpe, sin aviso de apagado (senal, aborto o cierre forzado)\n", linea_cierre_forzado(1, "pedido por weft kill"));
        assert_eq!(linea_cierre_forzado(1, "pedido por weft kill"), "1 KILL pedido por weft kill");
        assert!(cierre_ordenado(&kill) && fin_anotado(&kill));
        assert!(cierre_ordenado(&format!("{}{}", error, kill)));
        // solo como marca de su columna, no por aparecer en un texto
        assert!(!cierre_ordenado("1 SHUTDOWN {\"reason\":\"KILL host-signal\"}\n"));
        // motivos
        assert!(motivo_del_fin(panico, "QEMU cerro la pantalla", false, None).contains("fallo grave"));
        assert!(motivo_del_fin(golpe, "QEMU cerro la pantalla", false, None).contains("de golpe"));
        assert!(motivo_del_fin(senal, "x", false, None).contains("señal"));
        assert!(motivo_del_fin(error, "x", false, None).contains("error del emulador"));
        assert_eq!(motivo_del_fin("", "QEMU cerro la pantalla", false, None), "La máquina terminó sin que se pidiera apagarla (QEMU cerro la pantalla).");
        let viva = motivo_del_fin("", "otro cliente la desalojo", true, None);
        assert!(viva.starts_with("La máquina sigue en marcha") && viva.contains("(otro cliente la desalojo)") && viva.contains("«weft stop» la apaga"), "{}", viva);
        // dentro de Flatpak, tambien la orden con que se lanza alli
        let viva = motivo_del_fin("", "x", true, Some("org.ejemplo.Weft"));
        assert!(viva.contains("«weft stop» (o, en Flatpak, «flatpak run --command=weft org.ejemplo.Weft stop»)"), "{}", viva);
    }

    /// Las ultimas lineas de un registro (sin vacias, limpias de nombres de proveedores) y la cola de un archivo grande.
    #[test]
    fn ultimas_lineas_del_registro() {
        let reg: String = (1..=30).map(|i| format!("linea {}\n", i)).collect::<String>() + "\n   \n";
        let u = ultimas_lineas(&reg, LINEAS_REGISTRO);
        assert_eq!(u.len(), 15);
        assert_eq!((u[0].as_str(), u[14].as_str()), ("linea 16", "linea 30"));
        assert_eq!(ultimas_lineas("a\n\n  b  \n", 15), vec!["a", "  b"]);
        assert!(ultimas_lineas("", 15).is_empty());
        let l = ultimas_lineas(&format!("{}: fallo del dispositivo\n", concat!("gfx", "stream")), 15);
        assert!(crate::textos::prohibida_en(&l[0]).is_none() && l[0].contains("fallo del dispositivo"), "{:?}", l);
        // cola de un archivo: entero si cabe; si no, desde el principio de una linea; vacio si no existe
        let dir = std::env::temp_dir().join(format!("weft-vista-cola-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("qemu.log");
        assert_eq!(cola_de_archivo(&f, 100), "");
        std::fs::write(&f, "uno\ndos\ntres\n").unwrap();
        assert_eq!(cola_de_archivo(&f, 100), "uno\ndos\ntres\n");
        assert_eq!(cola_de_archivo(&f, 7), "tres\n");
        std::fs::write(&f, b"a\xff\nb\n").unwrap();
        assert_eq!(cola_de_archivo(&f, 100), "a\u{fffd}\nb\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// La caja de fin inesperado: titulo, motivo, las ultimas lineas de qemu.log y un boton Cerrar que solo responde a un
    /// clic completo sobre el (o a Intro, Espacio y Esc). Cabe en ventanas pequenas recortando el registro.
    #[test]
    fn la_despedida_muestra_el_registro_y_se_cierra_con_su_boton() {
        let t = crate::fuente::Tipografia::solo_respaldo(1.0);
        let textos = |d: &Despedida, vent: (f32, f32)| -> Vec<String> { d.dibujar(vent, &t).into_iter().filter_map(|p| if let Pint::Texto { t, .. } = p { Some(t) } else { None }).collect() };
        let reg: String = (1..=40).map(|i| format!("qemu: linea {}\n", i)).collect();
        let mut d = Despedida::nueva(false, "La máquina terminó de golpe.", &reg);
        assert_eq!(d.titulo, tx!("fin.maquina_cerro_forma_inesperada"));
        assert_eq!(d.lineas.len(), LINEAS_REGISTRO);
        let vent = (800.0, 900.0);
        let tx = textos(&d, vent);
        assert!(tx.contains(&"La máquina se cerró de forma inesperada".to_string()) && tx.contains(&"La máquina terminó de golpe.".to_string()), "{:?}", tx);
        assert!(tx.contains(&"qemu: linea 40".to_string()) && tx.contains(&"qemu: linea 26".to_string()) && !tx.contains(&"qemu: linea 25".to_string()), "{:?}", tx);
        assert!(tx.contains(&"Cerrar".to_string()));
        // el boton queda dentro de la ventana
        let b = d.boton(vent, &t);
        assert!(b.x >= 0.0 && b.y >= 0.0 && b.x + b.w <= vent.0 && b.y + b.h <= vent.1, "{:?}", b);
        let (cx, cy) = (b.x + b.w / 2.0, b.y + b.h / 2.0);
        // resaltado al pasar por encima
        assert!(d.mover(vent, &t, cx, cy) && d.hover);
        assert!(!d.mover(vent, &t, cx, cy));
        assert!(d.mover(vent, &t, 1.0, 1.0) && !d.hover);
        // pulsar dentro y soltar fuera, o al reves, no cierra; un clic completo si
        d.presionar(vent, &t, cx, cy);
        assert!(!d.soltar(vent, &t, 1.0, 1.0));
        d.presionar(vent, &t, 1.0, 1.0);
        assert!(!d.soltar(vent, &t, cx, cy));
        d.presionar(vent, &t, cx, cy);
        assert!(d.soltar(vent, &t, cx, cy));
        // teclas: Intro, Intro del teclado numerico, Espacio y Esc
        for sc in [40, 88, 44, 41] {
            assert!(d.tecla(sc), "{}", sc);
        }
        assert!(!d.tecla(4) && !d.tecla(66));
        // ventana baja: caben el boton y menos lineas del registro (las ultimas)
        let baja = (360.0, 220.0);
        let b = d.boton(baja, &t);
        assert!(b.x + b.w <= baja.0 && b.y + b.h <= baja.1, "{:?}", b);
        let vistas: Vec<String> = textos(&d, baja).into_iter().filter(|x| x.starts_with("qemu: linea")).collect();
        assert!(!vistas.is_empty() && vistas.len() < LINEAS_REGISTRO && vistas.last().map(String::as_str) == Some("qemu: linea 40"), "{:?}", vistas);
        // la maquina sigue viva (solo se perdio la pantalla) y sin registro
        let v = Despedida::nueva(true, "x", "");
        assert_eq!(v.titulo, tx!("fin.perdio_pantalla_maquina"));
        assert!(textos(&v, vent).iter().any(|x| x.contains("está vacío")));
        // nada de nombres de proveedores, ni del motivo ni del registro
        let p = Despedida::nueva(false, concat!("gfx", "stream se cayo"), &format!("{}: fallo\n", concat!("ruta", "baga")));
        assert!(textos(&p, vent).iter().all(|x| crate::textos::prohibida_en(x).is_none()), "{:?}", textos(&p, vent));
        // el motivo que da la conexion llega con el error del sistema en espanol
        let e = Despedida::nueva(true, "La máquina sigue en marcha, pero esta ventana perdió la conexión con su pantalla (Connection reset by peer (os error 104)).", "");
        assert!(e.motivo.contains("(la conexión se cortó)") && !e.motivo.contains("os error"), "{}", e.motivo);
    }

    /// Todo lo que dibuja la caja de despedida (con el boton en reposo, con el raton encima y pulsado) tiene contraste AA:
    /// la nota del pie es texto secundario, no deshabilitado.
    #[test]
    fn despedida_con_contraste_aa() {
        let t = crate::fuente::Tipografia::solo_respaldo(1.0);
        let reg: String = (1..=20).map(|i| format!("qemu: linea {}\n", i)).collect();
        let mut d = Despedida::nueva(false, "La máquina terminó de golpe.", &reg);
        use crate::formas::tema;
        for (v, (hover, pulsado)) in [tema::Variante::Oscuro, tema::Variante::Claro].into_iter().flat_map(|v| [(false, false), (true, false), (true, true)].map(|e| (v, e))) {
            d.hover = hover;
            d.pulsado = pulsado;
            for vent in [(800.0f32, 900.0f32), (360.0, 260.0)] {
                let pares = tema::con(v, || crate::formas::pares_de_color(&d.dibujar(vent, &t), tema::p().lienzo, &|e| t.alto_linea(e)));
                assert!(pares.iter().any(|p| p.que.starts_with("Al cerrar")), "{:?}", pares.iter().map(|p| &p.que).collect::<Vec<_>>());
                let f = tema::con(v, || crate::formas::fallos_de_contraste(&pares));
                assert!(f.is_empty(), "{:?}: {:?}", v, f);
            }
        }
    }

    /// La carpeta de imagenes del usuario sale de la variable o de `user-dirs.dirs` (con `$HOME`); sin ella, o si apunta a
    /// la carpeta personal (desactivada), no hay.
    #[test]
    fn carpeta_de_imagenes_del_usuario() {
        let entorno = |pares: &'static [(&'static str, &'static str)]| move |k: &str| pares.iter().find(|(c, _)| *c == k).map(|(_, v)| v.to_string());
        let dirs = "# generado por xdg-user-dirs-update\nXDG_DESKTOP_DIR=\"$HOME/Escritorio\"\nXDG_PICTURES_DIR=\"$HOME/Imágenes\"\n";
        assert_eq!(carpeta_imagenes(&entorno(&[("HOME", "/home/ana")]), Some(dirs)), Some(PathBuf::from("/home/ana/Imágenes")));
        // la variable manda sobre el archivo
        assert_eq!(carpeta_imagenes(&entorno(&[("HOME", "/home/ana"), ("XDG_PICTURES_DIR", "/datos/fotos")]), Some(dirs)), Some(PathBuf::from("/datos/fotos")));
        // ruta absoluta en el archivo
        assert_eq!(carpeta_imagenes(&entorno(&[("HOME", "/home/ana")]), Some("XDG_PICTURES_DIR=\"/mnt/img/\"\n")), Some(PathBuf::from("/mnt/img")));
        // desactivada (la carpeta personal), comentada, relativa, sin HOME para $HOME o sin nada
        for (env, contenido) in [
            (&[("HOME", "/home/ana")][..], Some("XDG_PICTURES_DIR=\"$HOME/\"\n")),
            (&[("HOME", "/home/ana")][..], Some("XDG_PICTURES_DIR=\"$HOME\"\n")),
            (&[("HOME", "/home/ana")][..], Some("# XDG_PICTURES_DIR=\"$HOME/Fotos\"\n")),
            (&[("HOME", "/home/ana")][..], Some("XDG_PICTURES_DIR=\"Fotos\"\n")),
            (&[("HOME", "/home/ana")][..], Some("XDG_PICTURES_DIR=\"$HOMEFotos\"\n")),
            (&[][..], Some(dirs)),
            (&[("HOME", "/home/ana")][..], None),
        ] {
            let e = move |k: &str| env.iter().find(|(c, _)| *c == k).map(|(_, v)| v.to_string());
            assert_eq!(carpeta_imagenes(&e, contenido), None, "{:?} {:?}", env, contenido);
        }
        // candidatas: la de imagenes (si hay) y siempre la de datos de la maquina
        let datos = Path::new("/home/ana/.local/share/weft");
        assert_eq!(carpetas_de_captura(Some(PathBuf::from("/home/ana/Imágenes")), datos, "prueba"), vec![PathBuf::from("/home/ana/Imágenes"), datos.join("machines/prueba/capturas")]);
        assert_eq!(carpetas_de_captura(None, datos, "default"), vec![datos.join("machines/default/capturas")]);
    }

    /// Se elige la primera carpeta en la que se puede escribir (creandola si hace falta); si la primera no sirve (un archivo
    /// donde deberia haber una carpeta), la siguiente; si ninguna, el motivo en espanol. No deja archivos de prueba.
    #[test]
    fn eleccion_de_la_carpeta_de_capturas() {
        let base = std::env::temp_dir().join(format!("weft-capturas-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let (imagenes, propia) = (base.join("Imágenes"), base.join("datos/machines/prueba/capturas"));
        assert_eq!(elegir_carpeta(&[imagenes.clone(), propia.clone()]).unwrap(), imagenes);
        assert!(imagenes.is_dir() && std::fs::read_dir(&imagenes).unwrap().next().is_none(), "sin restos de la prueba de escritura");
        // un archivo con el nombre de la carpeta: se pasa a la siguiente, que se crea
        let archivo = base.join("no-es-carpeta");
        std::fs::write(&archivo, "x").unwrap();
        assert_eq!(elegir_carpeta(&[archivo.join("sub"), propia.clone()]).unwrap(), propia);
        assert!(propia.is_dir());
        let e = elegir_carpeta(&[archivo.join("sub")]).unwrap_err();
        assert!(para_mostrar(&e).ends_with(": no es una carpeta") && para_mostrar(&e).contains("no-es-carpeta/sub"), "{}", e);
        assert!(elegir_carpeta(&[]).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Una ruta del usuario marcada como literal llega entera a la interfaz aunque lleve una palabra que la red de seguridad
    /// cambiaria; el resto del mensaje si pasa por ella. Una ruta que no existe o marcas sueltas no se saltan la red.
    #[test]
    fn rutas_literales_en_los_mensajes() {
        let base = std::env::temp_dir().join(format!("weft-ruta-{}-{}", std::process::id(), concat!("magi", "sk")));
        std::fs::create_dir_all(&base).unwrap();
        let prohibida = concat!("Kernel", "SU");
        let m = format!("{} dice: guardada en {}", prohibida, ruta_literal(&base));
        let visto = para_mostrar(&m);
        assert!(visto.ends_with(&format!("guardada en {}", base.display())), "{}", visto);
        assert!(!visto.contains(RUTA_INI) && !visto.contains(RUTA_FIN) && !visto.contains(prohibida), "{}", visto);
        // limpiar_salvo_rutas conserva las marcas (para quien muestre despues) y limpia lo demas
        let l = limpiar_salvo_rutas(&m);
        assert!(l.contains(&ruta_literal(&base)) && !l.contains(prohibida), "{}", l);
        assert_eq!(para_mostrar(&l), visto);
        // forma corta: la ruta reducida a su ultimo nombre
        let archivo = base.join("captura-20261008-120000.png");
        std::fs::write(&archivo, "png").unwrap();
        let aviso = format!("Captura guardada en {}", ruta_literal(&archivo));
        assert_eq!(para_mostrar(&aviso), format!("Captura guardada en {}", archivo.display()));
        assert_eq!(para_mostrar_corto(&aviso).as_deref(), Some("Captura guardada en .../captura-20261008-120000.png"));
        assert_eq!(para_mostrar_corto("sin rutas"), None);
        // una ruta que no existe, una relativa o marcas sueltas: texto normal, limpio y sin marcas
        for t in [format!("en {}", ruta_literal(Path::new(&format!("/no/existe/{}", prohibida)))), format!("en {}{}{}", RUTA_INI, prohibida, RUTA_FIN), format!("{}{} sin cierre", RUTA_INI, prohibida)] {
            let v = para_mostrar(&t);
            assert!(!v.contains(prohibida) && !v.contains(RUTA_INI) && !v.contains(RUTA_FIN), "{:?} -> {:?}", t, v);
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Los errores del sistema que llegan en ingles se dicen en espanol: por su numero de error, por su clase o por su texto.
    #[test]
    fn errores_del_sistema_en_espanol() {
        use std::io::{Error, ErrorKind};
        // el texto que escribe std::io::Error, tal cual llega desde otros modulos
        let e = format!("/no/existe: {}", Error::from_raw_os_error(2));
        assert_eq!(traducir_errores(&e), "/no/existe: no existe");
        assert_eq!(traducir_errores(&format!("/root: {}", Error::from_raw_os_error(13))), "/root: sin permiso");
        assert_eq!(traducir_errores(&format!("x: {}", Error::from_raw_os_error(17))), "x: ya existe");
        assert_eq!(traducir_errores(&format!("x: {}", Error::from_raw_os_error(20))), "x: no es una carpeta");
        assert_eq!(traducir_errores(&format!("no se pudo conectar ({})", Error::from_raw_os_error(111))), "no se pudo conectar (conexión rechazada)");
        assert_eq!(traducir_errores(&format!("{}", Error::from_raw_os_error(28))), tx!("error.no_queda_espacio_disco"));
        // las clases sin numero, solo como mensaje entero
        assert_eq!(traducir_errores(&format!("x: {}", Error::from(ErrorKind::NotFound))), "x: no existe");
        assert_eq!(traducir_errores(&format!("leyendo ({}).", Error::from(ErrorKind::PermissionDenied))), "leyendo (sin permiso).");
        assert_eq!(traducir_errores("adb: connection timed out after 3 s"), "adb: connection timed out after 3 s");
        assert_eq!(traducir_errores("the entity not found here"), "the entity not found here");
        // un numero desconocido o un texto que no es el del sistema se dejan
        assert_eq!(traducir_errores("algo raro (os error 9999)"), "algo raro (os error 9999)");
        assert_eq!(traducir_errores("Sin errores"), "Sin errores");
        // de un error de E/S: numero, clase o su propio mensaje
        assert_eq!(error_io(&Error::from_raw_os_error(2)), tx!("error.no_existe"));
        assert_eq!(error_io(&Error::from_raw_os_error(21)), tx!("error.carpeta"));
        assert_eq!(error_io(&Error::from(ErrorKind::PermissionDenied)), tx!("error.sin_permiso"));
        assert_eq!(error_io(&Error::from(ErrorKind::AlreadyExists)), tx!("error.ya_existe"));
        assert_eq!(error_io(&Error::new(ErrorKind::NotFound, "falta el socket")), "falta el socket");
        assert_eq!(error_io(&Error::other("otra cosa")), "otra cosa");
        // todos los numeros conocidos se traducen desde su texto real
        for (n, ingles, es) in ERRORES_SO {
            let real = Error::from_raw_os_error(*n).to_string();
            assert!(real.starts_with(ingles), "el texto del sistema para {} es {:?}", n, real);
            assert_eq!(traducir_errores(&real), texto(es), "{}", n);
        }
        // y lo que muestra la configuracion pasa por la traduccion
        assert_eq!(para_mostrar(&format!("No se pudo leer: {}", Error::from_raw_os_error(2))), "No se pudo leer: no existe");
    }

    /// Las ordenes de terminal de la interfaz: «weft ...» y, dentro de Flatpak, tambien la forma con que se lanza alli.
    #[test]
    fn ordenes_de_terminal_con_flatpak() {
        assert_eq!(orden_terminal("disk create", None), "«weft disk create»");
        assert_eq!(orden_terminal("stop", Some("io.github.x.weft")), "«weft stop» (o, en Flatpak, «flatpak run --command=weft io.github.x.weft stop»)");
    }

    /// E.10: las ordenes de consola entre acentos graves que traen los textos de otros modulos salen como el resto de
    /// ordenes de la interfaz (con su forma de Flatpak dentro de Flatpak); lo demas entre acentos graves no se toca.
    #[test]
    fn ordenes_de_consola_en_los_mensajes() {
        let t = "Los detalles se ven con `weft root status` o en el README.";
        assert_eq!(ordenes_de_consola(t, None), "Los detalles se ven con «weft root status» o en el README.");
        assert_eq!(
            ordenes_de_consola(t, Some("io.github.x.weft")),
            "Los detalles se ven con «weft root status» (o, en Flatpak, «flatpak run --command=weft io.github.x.weft root status») o en el README."
        );
        // varias, con otras cosas entre acentos graves y un acento sin cerrar
        assert_eq!(
            ordenes_de_consola("usa `weft stop` y `weft start ...`; `bridge restore` no es de weft", None),
            "usa «weft stop» y «weft start ...»; `bridge restore` no es de weft"
        );
        for igual in ["sin ordenes", "`weft ` vacia", "a medias `weft stop", "`otra cosa`", ""] {
            assert_eq!(ordenes_de_consola(igual, Some("x.y")), igual);
        }
        // y los mensajes de la configuracion pasan por aqui (fuera de Flatpak en las pruebas)
        if flatpak_id().is_none() {
            assert_eq!(para_mostrar("Ejecuta `weft bridge check`."), "Ejecuta «weft bridge check».");
        }
    }
}
