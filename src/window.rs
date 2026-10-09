//! Ventana propia de weft (`--display window`): QEMU entrega la pantalla por D-Bus (`-display dbus,p2p=yes`)
//! y este proceso la muestra con SDL3, cargada en tiempo de ejecucion (sin dependencias de compilacion).
//!
//! Por que no las ventanas de QEMU: con gfxstream, QEMU cambia en cada cuadro la superficie de la consola pasando
//! por una imagen "Display output is not active". SDL la repinta en el acto (parpadeo) o, con OpenGL, aborta si el
//! invitado entrega RGBA. La pantalla D-Bus solo transmite pixeles cuando el cuadro real ya esta copiado, de modo
//! que esa imagen intermedia nunca llega aqui, y el formato de pixel lo convierte SDL. Ademas la resolucion la
//! decide el dispositivo (--resolution): la ventana nunca se la impone.
//!
//! Entrada: el raton se envia como posicion absoluta + botones (lo recibe el puntero de --pointer) y el teclado
//! como codigos de tecla de QEMU.

use crate::barra::Barra;
use crate::dbus::{self, Arg, Conn, Msg, R};
use crate::formas::{self, Mascara, Pint};
use crate::fuente::{Atlas, Estilo, Tipografia};
use crate::gestos::{self, Atajo, AtajoExtra, Destino, FiltroAtajos, Mods, MtKind, Op, Tactil};
use crate::ajustes::{self, Ajustes, Datos, Efecto, Tecla};
use crate::config::{AccionAtajo, Config};
use crate::vista::{self, Accion, Info, ModoZoom, Servicios, Sondeo};
use crate::pantalla::{self, Resolucion};
use crate::json::V;
use crate::qmp::Qmp;
use crate::textos::tx;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::ffi::{c_void, CString};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use crate::textos::txf;

const CONSOLE: &str = "/org/qemu/Display1/Console_0";

// ---------------------------------------------------------------------------------------------------------------
// SDL3 (solo las funciones usadas; valores de SDL 3.2, estables en toda la serie 3.x)

const SDL_INIT_VIDEO: u32 = 0x20;
const SDL_WINDOW_RESIZABLE: u64 = 0x20;
const SDL_WINDOW_HIGH_PIXEL_DENSITY: u64 = 0x2000;
const SDL_TEXTUREACCESS_STATIC: i32 = 0;
const SDL_TEXTUREACCESS_STREAMING: i32 = 1;
const SDL_BLENDMODE_BLEND: u32 = 1;
/// SDL_PIXELFORMAT_ARGB8888: en memoria B, G, R, A
const SDL_PIXELFORMAT_ARGB8888: u32 = 0x16362004;
const SDL_LOGICAL_PRESENTATION_LETTERBOX: i32 = 2;
const EV_QUIT: u32 = 0x100;
const EV_WINDOW_SHOWN: u32 = 0x202;
const EV_WINDOW_HIDDEN: u32 = 0x203;
const EV_WINDOW_EXPOSED: u32 = 0x204;
const EV_WINDOW_RESIZED: u32 = 0x206;
const EV_WINDOW_PIXEL_SIZE_CHANGED: u32 = 0x207;
const EV_WINDOW_MINIMIZED: u32 = 0x209;
const EV_WINDOW_MAXIMIZED: u32 = 0x20a;
const EV_WINDOW_RESTORED: u32 = 0x20b;
const EV_WINDOW_MOUSE_LEAVE: u32 = 0x20d;
const EV_WINDOW_FOCUS_LOST: u32 = 0x20f;
const EV_WINDOW_CLOSE_REQUESTED: u32 = 0x210;
const EV_WINDOW_ENTER_FULLSCREEN: u32 = 0x217;
const EV_WINDOW_LEAVE_FULLSCREEN: u32 = 0x218;
/// el dibujador perdio sus texturas (cambio de GPU, reinicio del controlador): hay que volver a subir el cuadro
const EV_RENDER_TARGETS_RESET: u32 = 0x2000;
const EV_RENDER_DEVICE_RESET: u32 = 0x2001;
const EV_KEY_DOWN: u32 = 0x300;
const EV_KEY_UP: u32 = 0x301;
const EV_TEXT_INPUT: u32 = 0x303;
const EV_MOUSE_MOTION: u32 = 0x400;
const EV_MOUSE_BUTTON_DOWN: u32 = 0x401;
const EV_MOUSE_BUTTON_UP: u32 = 0x402;
const EV_MOUSE_WHEEL: u32 = 0x403;
/// SDL_EVENT_DROP_FILE: se solto un archivo sobre la ventana (SDL_DropEvent: la ruta en `data`, desplazamiento 40)
const EV_DROP_FILE: u32 = 0x1000;
/// evento propio: llego un cuadro o termino la conexion
const EV_USER: u32 = 0x8000;
/// Tamano minimo de la ventana en dp (la pantalla de configuracion necesita sitio para dibujarse; mas estrecha, las
/// cajas de texto quedarian sin ancho).
pub const VENTANA_MIN: (i32, i32) = (360, 480);

/// SDL_Event: union de 128 bytes; los campos se leen por desplazamiento (ver SDL_events.h)
#[repr(C, align(8))]
struct Event([u8; 128]);

impl Event {
    fn u32(&self, off: usize) -> u32 {
        u32::from_ne_bytes(self.0[off..off + 4].try_into().unwrap())
    }
    fn f32(&self, off: usize) -> f32 {
        f32::from_ne_bytes(self.0[off..off + 4].try_into().unwrap())
    }
    fn kind(&self) -> u32 {
        self.u32(0)
    }
}

#[repr(C)]
#[derive(Default)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

extern "C" {
    fn dlopen(name: *const i8, flags: i32) -> *mut c_void;
    fn dlsym(h: *mut c_void, name: *const i8) -> *mut c_void;
}

type P = *mut c_void;

#[derive(Clone, Copy)]
struct Sdl {
    set_hint: unsafe extern "C" fn(*const i8, *const i8) -> bool,
    init: unsafe extern "C" fn(u32) -> bool,
    quit: unsafe extern "C" fn(),
    get_error: unsafe extern "C" fn() -> *const i8,
    primary_display: unsafe extern "C" fn() -> u32,
    usable_bounds: unsafe extern "C" fn(u32, *mut Rect) -> bool,
    create_window: unsafe extern "C" fn(*const i8, i32, i32, u64) -> P,
    create_renderer: unsafe extern "C" fn(P, *const i8) -> P,
    set_vsync: unsafe extern "C" fn(P, i32) -> bool,
    create_texture: unsafe extern "C" fn(P, u32, i32, i32, i32) -> P,
    destroy_texture: unsafe extern "C" fn(P),
    update_texture: unsafe extern "C" fn(P, *const Rect, *const c_void, i32) -> bool,
    logical: unsafe extern "C" fn(P, i32, i32, i32) -> bool,
    clear: unsafe extern "C" fn(P) -> bool,
    render_texture: unsafe extern "C" fn(P, P, *const c_void, *const c_void) -> bool,
    /// SDL_RenderTextureRotated(renderer, textura, origen, destino, angulo en grados (horario), centro, volteo)
    render_rotated: unsafe extern "C" fn(P, P, *const c_void, *const c_void, f64, *const c_void, i32) -> bool,
    set_window_size: unsafe extern "C" fn(P, i32, i32) -> bool,
    set_min_size: unsafe extern "C" fn(P, i32, i32) -> bool,
    /// SDL_SetRenderDrawColor(renderer, r, g, b, a) y SDL_RenderFillRects(renderer, SDL_FRect*, cantidad)
    set_color: unsafe extern "C" fn(P, u8, u8, u8, u8) -> bool,
    fill_rects: unsafe extern "C" fn(P, *const c_void, i32) -> bool,
    /// pruebas (ventana-captura): SDL_RenderReadPixels, SDL_ConvertSurface y SDL_DestroySurface
    read_pixels: unsafe extern "C" fn(P, *const c_void) -> P,
    convert_surface: unsafe extern "C" fn(P, u32) -> P,
    destroy_surface: unsafe extern "C" fn(P),
    present: unsafe extern "C" fn(P) -> bool,
    to_logical: unsafe extern "C" fn(P, f32, f32, *mut f32, *mut f32) -> bool,
    wait_event: unsafe extern "C" fn(*mut Event, i32) -> bool,
    poll_event: unsafe extern "C" fn(*mut Event) -> bool,
    push_event: unsafe extern "C" fn(*mut Event) -> bool,
    window_size: unsafe extern "C" fn(P, *mut i32, *mut i32) -> bool,
    /// mezcla por alfa del dibujo y de las texturas, modulacion de color y alfa, filtro, recorte y densidad de pixeles
    set_draw_blend: unsafe extern "C" fn(P, u32) -> bool,
    tex_blend: unsafe extern "C" fn(P, u32) -> bool,
    tex_color: unsafe extern "C" fn(P, u8, u8, u8) -> bool,
    tex_alpha: unsafe extern "C" fn(P, u8) -> bool,
    tex_scale: unsafe extern "C" fn(P, i32) -> bool,
    set_clip: unsafe extern "C" fn(P, *const Rect) -> bool,
    pixel_density: unsafe extern "C" fn(P) -> f32,
    /// entrada de texto (los caracteres de los campos de texto de la pantalla de configuracion llegan como eventos de texto)
    start_text: unsafe extern "C" fn(P) -> bool,
    stop_text: unsafe extern "C" fn(P) -> bool,
    /// SDL_SetWindowFullscreen(ventana, pantalla completa)
    set_fullscreen: unsafe extern "C" fn(P, bool) -> bool,
    /// SDL_EnableScreenSaver / SDL_DisableScreenSaver
    enable_screensaver: unsafe extern "C" fn() -> bool,
    disable_screensaver: unsafe extern "C" fn() -> bool,
    /// nombre de las teclas en la distribucion del teclado: SDL_GetKeyFromScancode(codigo, modificadores, para eventos)
    /// y SDL_GetKeyName(tecla)
    key_from_scancode: unsafe extern "C" fn(i32, u16, bool) -> u32,
    key_name: unsafe extern "C" fn(u32) -> *const i8,
    clipboard_text: unsafe extern "C" fn() -> *mut i8,
    free: unsafe extern "C" fn(*mut c_void),
}

impl Sdl {
    // cada simbolo de SDL se convierte al tipo de su campo (el destino lo fija la estructura)
    #[allow(clippy::missing_transmute_annotations)]
    fn load() -> Result<Sdl, String> {
        let h = ["libSDL3.so.0", "libSDL3.so"]
            .iter()
            .map(|n| {
                let c = CString::new(*n).unwrap();
                unsafe { dlopen(c.as_ptr(), 2 /* RTLD_NOW */) }
            })
            .find(|h| !h.is_null())
            .ok_or(tx!("window.no_se_encontro_la_biblioteca_sdl3"))?;
        macro_rules! f {
            ($n:literal) => {{
                let c = CString::new($n).unwrap();
                let p = unsafe { dlsym(h, c.as_ptr()) };
                if p.is_null() {
                    return Err(txf!("window.sdl3_sin_la_funcion", $n));
                }
                unsafe { std::mem::transmute(p) }
            }};
        }
        Ok(Sdl {
            set_hint: f!("SDL_SetHint"),
            init: f!("SDL_Init"),
            quit: f!("SDL_Quit"),
            get_error: f!("SDL_GetError"),
            primary_display: f!("SDL_GetPrimaryDisplay"),
            usable_bounds: f!("SDL_GetDisplayUsableBounds"),
            create_window: f!("SDL_CreateWindow"),
            create_renderer: f!("SDL_CreateRenderer"),
            set_vsync: f!("SDL_SetRenderVSync"),
            create_texture: f!("SDL_CreateTexture"),
            destroy_texture: f!("SDL_DestroyTexture"),
            update_texture: f!("SDL_UpdateTexture"),
            logical: f!("SDL_SetRenderLogicalPresentation"),
            clear: f!("SDL_RenderClear"),
            render_texture: f!("SDL_RenderTexture"),
            render_rotated: f!("SDL_RenderTextureRotated"),
            set_window_size: f!("SDL_SetWindowSize"),
            set_min_size: f!("SDL_SetWindowMinimumSize"),
            set_color: f!("SDL_SetRenderDrawColor"),
            fill_rects: f!("SDL_RenderFillRects"),
            read_pixels: f!("SDL_RenderReadPixels"),
            convert_surface: f!("SDL_ConvertSurface"),
            destroy_surface: f!("SDL_DestroySurface"),
            present: f!("SDL_RenderPresent"),
            to_logical: f!("SDL_RenderCoordinatesFromWindow"),
            wait_event: f!("SDL_WaitEventTimeout"),
            poll_event: f!("SDL_PollEvent"),
            push_event: f!("SDL_PushEvent"),
            window_size: f!("SDL_GetWindowSize"),
            set_draw_blend: f!("SDL_SetRenderDrawBlendMode"),
            tex_blend: f!("SDL_SetTextureBlendMode"),
            tex_color: f!("SDL_SetTextureColorMod"),
            tex_alpha: f!("SDL_SetTextureAlphaMod"),
            tex_scale: f!("SDL_SetTextureScaleMode"),
            set_clip: f!("SDL_SetRenderClipRect"),
            pixel_density: f!("SDL_GetWindowPixelDensity"),
            start_text: f!("SDL_StartTextInput"),
            stop_text: f!("SDL_StopTextInput"),
            set_fullscreen: f!("SDL_SetWindowFullscreen"),
            enable_screensaver: f!("SDL_EnableScreenSaver"),
            disable_screensaver: f!("SDL_DisableScreenSaver"),
            key_from_scancode: f!("SDL_GetKeyFromScancode"),
            key_name: f!("SDL_GetKeyName"),
            clipboard_text: f!("SDL_GetClipboardText"),
            free: f!("SDL_free"),
        })
    }

    fn error(&self) -> String {
        let p = unsafe { (self.get_error)() };
        if p.is_null() {
            return String::new();
        }
        unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }

    /// Tamano de la ventana en dp (puntos de la ventana divididos por el factor de la interfaz, normalmente 1).
    fn tam_dp(&self, win: P, w: &mut i32, h: &mut i32) {
        unsafe { (self.window_size)(win, w, h) };
        let k = ui_k();
        if k != 1.0 {
            (*w, *h) = ((*w as f32 / k).round() as i32, (*h as f32 / k).round() as i32);
        }
    }

    fn fijar_tam_dp(&self, win: P, w: i32, h: i32) {
        let k = ui_k();
        unsafe { (self.set_window_size)(win, (w as f32 * k).round() as i32, (h as f32 * k).round() as i32) };
    }

    /// Texto del portapapeles del escritorio (vacio si no hay).
    fn portapapeles(&self) -> String {
        let p = unsafe { (self.clipboard_text)() };
        if p.is_null() {
            return String::new();
        }
        let t = unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned();
        unsafe { (self.free)(p as *mut c_void) };
        t
    }

    fn wake(&self) {
        let mut e = Event([0; 128]);
        e.0[0..4].copy_from_slice(&EV_USER.to_ne_bytes());
        unsafe { (self.push_event)(&mut e) };
    }
}

/// Preferencia de color del escritorio sin conocer (sin bus, sin portal o aun sin respuesta).
const ESQUEMA_DESCONOCIDO: u32 = u32::MAX;

/// Lanza la escucha de la preferencia de color del escritorio (`color-scheme` del portal de ajustes) en un hilo: el valor
/// queda en lo devuelto (ESQUEMA_DESCONOCIDO mientras no se sepa) y cada cambio despierta la ventana con `wake`.
fn escuchar_esquema(wake: Arc<dyn Fn() + Send + Sync>) -> Arc<std::sync::atomic::AtomicU32> {
    let valor = Arc::new(std::sync::atomic::AtomicU32::new(ESQUEMA_DESCONOCIDO));
    let v = valor.clone();
    std::thread::spawn(move || {
        let r = dbus::vigilar_esquema(&mut |x| {
            v.store(x, std::sync::atomic::Ordering::Relaxed);
            wake();
        });
        if let Err(e) = r {
            eprintln!("ventana: preferencia de color del escritorio: {} (en automatico, sin ella, tema oscuro)", e);
        }
    });
    valor
}

/// El tema que toca con la configuracion y lo ultimo que dijo el escritorio.
fn tema_de(cfg: &Config, esquema: &std::sync::atomic::AtomicU32) -> formas::tema::Variante {
    let e = esquema.load(std::sync::atomic::Ordering::Relaxed);
    formas::tema::Variante::elegir(&cfg.get("ventana.tema"), (e != ESQUEMA_DESCONOCIDO).then_some(e))
}

/// Factor de ampliacion de TODA la interfaz (ventana, panel y dispositivo) sobre los puntos de SDL, solo para probar la
/// tipografia a otra escala: WEFT_ESCALA_UI=2 hace que 1 dp = 2 puntos de la ventana, sin tocar el escritorio. Por
/// defecto 1 (la densidad de pixeles de SDL en pantallas HiDPI ya la maneja la tipografia).
fn ui_k() -> f32 {
    static K: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("WEFT_ESCALA_UI").ok().and_then(|v| v.parse::<f32>().ok()).filter(|k| (1.0..=4.0).contains(k)).unwrap_or(1.0))
}

// ---------------------------------------------------------------------------------------------------------------
// conversiones

/// Formato de pixman (el que informa QEMU) -> formato de SDL3. Los formatos con alfa se muestran como opacos.
pub fn sdl_format(pixman: u32) -> Option<u32> {
    Some(match pixman {
        0x20020888 | 0x20028888 => 0x16161804, // x8r8g8b8 / a8r8g8b8 -> XRGB8888
        0x20030888 | 0x20038888 => 0x16561804, // x8b8g8r8 / a8b8g8r8 -> XBGR8888
        0x20080888 | 0x20088888 => 0x16661804, // b8g8r8x8 / b8g8r8a8 -> BGRX8888
        0x20090888 | 0x20098888 => 0x16261804, // r8g8b8x8 / r8g8b8a8 -> RGBX8888
        0x10020565 => 0x15151002,              // r5g6b5 -> RGB565
        _ => return None,
    })
}

/// Codigo de tecla de SDL (uso HID de USB) -> numero de tecla de QEMU (conjunto 1 de PC; 0x80 | codigo para las
/// teclas extendidas E0).
pub fn qnum(sc: u32) -> Option<u32> {
    const LETTERS: [u32; 26] = [
        0x1e, 0x30, 0x2e, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31, 0x18, 0x19, 0x10, 0x13, 0x1f, 0x14, 0x16, 0x2f, 0x11, 0x2d, 0x15, 0x2c,
    ];
    Some(match sc {
        4..=29 => LETTERS[(sc - 4) as usize],
        30..=38 => sc - 30 + 0x02, // 1..9
        39 => 0x0b,                // 0
        40 => 0x1c,                // Intro
        41 => 0x01,                // Esc
        42 => 0x0e,                // Retroceso
        43 => 0x0f,                // Tab
        44 => 0x39,                // Espacio
        45 => 0x0c,
        46 => 0x0d,
        47 => 0x1a,
        48 => 0x1b,
        49 | 50 => 0x2b,
        51 => 0x27,
        52 => 0x28,
        53 => 0x29,
        54 => 0x33,
        55 => 0x34,
        56 => 0x35,
        57 => 0x3a,                // Bloq Mayus
        58..=67 => sc - 58 + 0x3b, // F1..F10
        68 => 0x57,                // F11
        69 => 0x58,                // F12
        70 => 0xb7,                // Impr Pant
        71 => 0x46,                // Bloq Despl
        72 => 0xc6,                // Pausa
        73 => 0xd2,                // Insert
        74 => 0xc7,                // Inicio
        75 => 0xc9,                // Re Pag
        76 => 0xd3,                // Supr
        77 => 0xcf,                // Fin
        78 => 0xd1,                // Av Pag
        79 => 0xcd,                // derecha
        80 => 0xcb,                // izquierda
        81 => 0xd0,                // abajo
        82 => 0xc8,                // arriba
        83 => 0x45,                // Bloq Num
        84 => 0xb5,                // teclado numerico /
        85 => 0x37,
        86 => 0x4a,
        87 => 0x4e,
        88 => 0x9c, // Intro del teclado numerico
        89 => 0x4f,
        90 => 0x50,
        91 => 0x51,
        92 => 0x4b,
        93 => 0x4c,
        94 => 0x4d,
        95 => 0x47,
        96 => 0x48,
        97 => 0x49,
        98 => 0x52,
        99 => 0x53,
        100 => 0x56, // tecla < > de los teclados europeos
        101 => 0xdd, // menu
        224 => 0x1d, // Ctrl izq
        225 => 0x2a, // Mayus izq
        226 => 0x38, // Alt izq
        227 => 0xdb, // Super izq
        228 => 0x9d, // Ctrl der
        229 => 0x36, // Mayus der
        230 => 0xb8, // AltGr
        231 => 0xdc, // Super der
        _ => return None,
    })
}

/// Boton de SDL -> boton de QEMU (InputButton: izquierdo 0, medio 1, derecho 2, lateral 5, extra 6).
fn qbutton(b: u8) -> Option<u32> {
    Some(match b {
        1 => 0,
        2 => 1,
        3 => 2,
        4 => 5,
        5 => 6,
        _ => return None,
    })
}

/// Imagen "Display output is not active." que QEMU crea (ui/console.c, qemu_create_placeholder_surface) y envia
/// completa cada vez que el dispositivo cambia de superficie: con gfxstream, en cada cuadro, justo antes del cuadro
/// real. Se reconoce por su construccion: formato x8r8g8b8, todo en cero salvo la franja de texto de 16 filas
/// centrada (fila (h/16 - 1)/2 en unidades de 16), que tiene algun pixel encendido. Un cuadro real con esa forma
/// se veria igual que el aviso, asi que descartarlo no pierde nada.
pub fn is_placeholder(w: u32, h: u32, stride: u32, fmt: u32, px: &[u8]) -> bool {
    if fmt != 0x20020888 || h < 16 || w < 8 {
        return false;
    }
    let (row_len, stride, h) = (w as usize * 4, stride as usize, h as usize);
    let band = ((h / 16).saturating_sub(1) / 2) * 16;
    let row = |y: usize| &px[y * stride..y * stride + row_len];
    let zero = |r: &[u8]| r.iter().all(|b| *b == 0);
    // primero las filas que en un cuadro real casi nunca son negras (arriba y abajo)
    (0..band).chain(band + 16..h).all(|y| zero(row(y))) && !(band..band + 16).all(|y| zero(row(y)))
}

/// Por que un Scanout no se puede usar: los datos no cubren `stride * h` filas, o el stride no llega al ancho de una
/// fila (`w` pixeles de `bpp` bytes, con bpp del formato de pixman: bits por pixel en el byte alto). None si esta bien.
pub fn scanout_invalido(w: u32, h: u32, stride: u32, fmt: u32, datos: usize) -> Option<String> {
    let bpp = ((fmt >> 24) / 8) as usize;
    if (stride as usize) * (h as usize) > datos {
        return Some(txf!("window.datos_incompletos_bytes_para_stride_x", datos, stride, h));
    }
    if (stride as usize) < (w as usize) * bpp {
        return Some(txf!("window.stride_menor_que_el_ancho_de_fila_x", stride, w, bpp));
    }
    None
}

// ---------------------------------------------------------------------------------------------------------------
// cuadro compartido entre el hilo D-Bus y la ventana

#[derive(Default)]
struct Frame {
    w: u32,
    h: u32,
    stride: u32,
    fmt: u32,
    /// pixeles: buf[off..]. La ventana los comparte (Arc) mientras sube el cuadro a su textura, sin copiarlos ni tener
    /// tomado el cerrojo: un Update que llegue entretanto copia el bufer (`Arc::make_mut`) y la ventana sigue con el suyo
    buf: Arc<Vec<u8>>,
    off: usize,
    gen: u64,
    /// llego un cuadro (Scanout, Update o Disable) que la ventana aun no subio a su textura (ver `tomar_cuadro`)
    nuevo: bool,
    /// rotacion de la vista (0..3), la fija el bucle de la ventana; la lee el gancho de pruebas
    rot: u32,
    /// esquina (x, y) y escala de la pantalla del dispositivo dentro de la ventana; botones del panel (nombre y
    /// rectangulo): los fija el bucle al dibujar y los lee el gancho de pruebas
    dev: (f32, f32, f32),
    botones: Vec<(String, [f32; 4])>,
    fin: Option<String>,
    /// contadores para WEFT_WINDOW_STATS
    scanouts: u64,
    updates: u64,
    placeholders: u64,
    /// cuadros mal formados que se descartaron (ver `scanout_invalido`)
    descartados: u64,
}

type Shared = Arc<Mutex<Frame>>;

/// Un cuadro para subir a la textura, tomado del compartido sin copiar los pixeles (ver `Frame::buf`).
struct Copia {
    w: u32,
    h: u32,
    stride: u32,
    fmt: u32,
    buf: Arc<Vec<u8>>,
    off: usize,
}

/// El cuadro para subir a la textura si llego uno nuevo desde la ultima vez (o si `forzar`: el dibujador perdio la
/// textura), y baja la bandera. None si no hay nada nuevo o no hay cuadro (pantalla desactivada): un redibujo solo de la
/// interfaz (barra, configuracion, raton) no vuelve a subir la pantalla entera.
fn tomar_cuadro(f: &mut Frame, forzar: bool) -> Option<Copia> {
    if !(f.nuevo || forzar) {
        return None;
    }
    f.nuevo = false;
    if f.buf.is_empty() {
        return None;
    }
    Some(Copia { w: f.w, h: f.h, stride: f.stride, fmt: f.fmt, buf: f.buf.clone(), off: f.off })
}

/// Identificador de la aplicacion para el escritorio (SDL_APP_ID: icono y agrupacion de ventanas): el de Flatpak dentro de
/// Flatpak (FLATPAK_ID), si no WEFT_APP_ID, si no `weft`.
pub fn app_id(flatpak: Option<String>, propio: Option<String>) -> String {
    [flatpak, propio].into_iter().flatten().map(|s| s.trim().to_string()).find(|s| !s.is_empty() && !s.contains('\0')).unwrap_or_else(|| "weft".into())
}

/// ¿Se ve la ventana tras este evento? Minimizada u oculta no se sube ni se presenta nada; restaurada, mostrada,
/// maximizada o descubierta, si. Cualquier otro evento no lo cambia.
fn visible_tras(k: u32, visible: bool) -> bool {
    match k {
        EV_WINDOW_MINIMIZED | EV_WINDOW_HIDDEN => false,
        EV_WINDOW_RESTORED | EV_WINDOW_SHOWN | EV_WINDOW_MAXIMIZED | EV_WINDOW_EXPOSED => true,
        _ => visible,
    }
}

/// El salvapantallas del equipo puede saltar (SDL lo impide por defecto mientras hay una ventana) salvo con un mando
/// conectado a la maquina: jugando solo con el mando no se toca el teclado ni el raton.
fn permitir_salvapantallas(mandos: usize) -> bool {
    mandos == 0
}

/// Atiende las llamadas que QEMU le hace al objeto Listener.
fn listener_loop(mut s: UnixStream, frame: Shared, sdl: Sdl) -> String {
    let mut w = match s.try_clone() {
        Ok(c) => Conn::new(c),
        Err(e) => return e.to_string(),
    };
    loop {
        let mut m = match dbus::read_msg(&mut s) {
            Ok(Some(m)) => m,
            Ok(None) => return tx!("window.qemu_cerro_la_pantalla").into(),
            Err(e) => return e,
        };
        if m.kind != dbus::METHOD_CALL {
            continue;
        }
        let mut reply = Msg::reply_to(&m);
        let r = handle(&mut m, &frame);
        match r {
            Ok(true) => sdl.wake(),
            Ok(false) => {}
            Err(e) => reply = Msg::error_to(&m, "org.freedesktop.DBus.Error.NotSupported", &e),
        }
        if m.iface == "org.freedesktop.DBus.Properties" && m.member == "GetAll" {
            // QEMU consulta que interfaces opcionales ofrece la ventana: ninguna (sin memoria compartida ni DMABUF)
            reply.set_body(&[Arg::DictStrArr(vec![("Interfaces".into(), vec![])])]);
        }
        if m.wants_reply() {
            if let Err(e) = w.send(&reply) {
                return e;
            }
        }
    }
}

/// Aplica una llamada al cuadro. Ok(true) si hay que redibujar.
fn handle(m: &mut Msg, frame: &Shared) -> Result<bool, String> {
    match (m.iface.as_str(), m.member.as_str()) {
        ("org.qemu.Display1.Listener", "Scanout") => {
            let mut r = R::new(&m.body);
            let (w, h, stride, fmt) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?);
            let d = r.bytes()?;
            let mut f = frame.lock().unwrap();
            // un cuadro mal formado se descarta (se queda el anterior): leerlo se saldria de los datos
            if let Some(por) = scanout_invalido(w, h, stride, fmt, d.len()) {
                f.descartados += 1;
                if f.descartados <= 3 || f.descartados % 100 == 0 {
                    eprintln!("ventana: cuadro descartado ({}): Scanout {}x{} {}", f.descartados, w, h, por);
                }
                return Err(format!("Scanout: {}", por));
            }
            if is_placeholder(w, h, stride, fmt, &m.body[d.clone()]) {
                // se queda el cuadro anterior en pantalla
                f.placeholders += 1;
                return Ok(false);
            }
            f.w = w;
            f.h = h;
            f.stride = stride;
            f.fmt = fmt;
            f.off = d.start;
            f.buf = Arc::new(std::mem::take(&mut m.body));
            f.gen += 1;
            f.nuevo = true;
            f.scanouts += 1;
            Ok(true)
        }
        ("org.qemu.Display1.Listener", "Update") => {
            let mut r = R::new(&m.body);
            let (x, y, w, h, stride, fmt) = (r.i32()?, r.i32()?, r.i32()?, r.i32()?, r.u32()? as usize, r.u32()?);
            let d = r.bytes()?;
            let mut f = frame.lock().unwrap();
            let bpp = ((fmt >> 24) / 8) as usize;
            if f.buf.is_empty() || fmt != f.fmt || x < 0 || y < 0 || w <= 0 || h <= 0 || (x + w) as u32 > f.w || (y + h) as u32 > f.h {
                return Ok(false);
            }
            let (x, y, w, h) = (x as usize, y as usize, w as usize, h as usize);
            if stride * (h - 1) + w * bpp > d.len() {
                return Err(tx!("window.update_datos_incompletos").into());
            }
            let (fs, off) = (f.stride as usize, f.off);
            // si la ventana aun sube el cuadro anterior, aqui se copia (y ella sigue con el suyo)
            let buf = Arc::make_mut(&mut f.buf);
            for row in 0..h {
                let src = d.start + row * stride;
                let dst = off + (y + row) * fs + x * bpp;
                buf[dst..dst + w * bpp].copy_from_slice(&m.body[src..src + w * bpp]);
            }
            f.gen += 1;
            f.nuevo = true;
            f.updates += 1;
            Ok(true)
        }
        ("org.qemu.Display1.Listener", "Disable") => {
            let mut f = frame.lock().unwrap();
            f.buf = Arc::default();
            f.gen += 1;
            f.nuevo = true;
            Ok(true)
        }
        ("org.qemu.Display1.Listener", "MouseSet") | ("org.qemu.Display1.Listener", "CursorDefine") => Ok(false),
        ("org.freedesktop.DBus.Properties", "GetAll") | ("org.freedesktop.DBus.Peer", "Ping") => Ok(false),
        (i, n) => Err(format!("{}.{} no implementado en la ventana de weft", i, n)), // texto-interno: respuesta de error D-Bus para QEMU
    }
}

/// Entrega a QEMU un socket para su pantalla D-Bus y autentica: devuelve el extremo nuestro, listo para hablar.
fn dial(qmp_path: &str, fdname: &str) -> Result<UnixStream, String> {
    let (mut ours, theirs) = UnixStream::pair().map_err(|e| e.to_string())?;
    // el control de QEMU atiende una sola conexion a la vez: reintentar mientras otro cliente lo usa
    let t0 = Instant::now();
    let mut q = loop {
        match Qmp::connect(qmp_path) {
            Ok(q) => break q,
            Err(e) if t0.elapsed() > Duration::from_secs(20) => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    };
    q.exec_fd("getfd", V::obj(&[("fdname", V::s(fdname))]), theirs.as_raw_fd())?;
    q.exec("add_client", Some(V::obj(&[("protocol", V::s("@dbus-display")), ("fdname", V::s(fdname))])))?;
    drop(q);
    drop(theirs);
    dbus::auth_client(&mut ours)?;
    Ok(ours)
}

/// Conexion con la pantalla D-Bus de QEMU: socket entregado por QMP y registro del Listener.
fn connect(qmp_path: &str, frame: Shared, sdl: Sdl) -> Result<Conn, String> {
    let ours = dial(qmp_path, "weft-ventana")?;
    let mut conn = Conn::new(ours.try_clone().map_err(|e| e.to_string())?);
    // QEMU emite senales y respuestas por esta conexion: se leen siempre para no llenar el socket. Si QEMU la cierra
    // (admite un solo cliente D-Bus: otro cliente que se conecte, p. ej. la orden interna `dbus-input`, desaloja a
    // este), la entrada ya no llega a la maquina: la ventana termina avisando en vez de quedarse muda.
    let fr = frame.clone();
    std::thread::spawn(move || {
        let mut r = ours;
        while let Ok(Some(m)) = dbus::read_msg(&mut r) {
            if m.kind == dbus::ERROR {
                let mut b = R::new(&m.body);
                eprintln!("QEMU respondio con error: {} {}", m.error, b.str().unwrap_or_default());
            }
        }
        let mut f = fr.lock().unwrap();
        if f.fin.is_none() {
            f.fin = Some(tx!("window.qemu_cerro_la_conexion_de_entrada_otro").into());
        }
        drop(f);
        sdl.wake();
    });
    let (mut lis, lis_theirs) = UnixStream::pair().map_err(|e| e.to_string())?;
    let f2 = frame.clone();
    std::thread::spawn(move || {
        let why = match dbus::auth_client(&mut lis) {
            Ok(()) => listener_loop(lis, f2.clone(), sdl),
            Err(e) => e,
        };
        f2.lock().unwrap().fin = Some(why);
        sdl.wake();
    });
    let mut m = Msg::call(CONSOLE, "org.qemu.Display1.Console", "RegisterListener");
    m.set_body(&[Arg::Fd(0)]);
    conn.send_fd(&m, lis_theirs.as_raw_fd())?;
    Ok(conn)
}

fn input(c: &mut Conn, iface: &str, member: &str, args: &[Arg]) {
    let mut m = Msg::call(CONSOLE, iface, member);
    m.flags = dbus::NO_REPLY;
    m.set_body(args);
    let _ = c.send(&m);
}

/// Envia una operacion de entrada a la pantalla D-Bus.
fn send_op(c: &mut Conn, op: Op) {
    const KB: &str = "org.qemu.Display1.Keyboard";
    const MOUSE: &str = "org.qemu.Display1.Mouse";
    match op {
        Op::Key(q, down) => input(c, KB, if down { "Press" } else { "Release" }, &[Arg::U32(q)]),
        Op::Abs(x, y) => input(c, MOUSE, "SetAbsPosition", &[Arg::U32(x), Arg::U32(y)]),
        Op::Btn(b, down) => input(c, MOUSE, if down { "Press" } else { "Release" }, &[Arg::U32(b)]),
        Op::Touch(k, slot, x, y) => input(c, "org.qemu.Display1.MultiTouch", "SendEvent", &[Arg::U32(k as u32), Arg::U64(slot), Arg::F64(x), Arg::F64(y)]),
    }
}

/// Salida de entrada hacia la maquina: lleva las operaciones de la VISTA (lo que muestra la ventana, girada) al panel
/// del invitado y, con `--pointer multitouch`, convierte el raton en un contacto de la pantalla tactil.
struct Salida {
    conn: Conn,
    rot: u32,
    panel: (u32, u32),
    tactil: Option<Tactil>,
}

impl Salida {
    fn op(&mut self, op: Op) {
        let op = gestos::mapear(op, self.rot, self.panel);
        match &mut self.tactil {
            Some(t) => {
                for o in t.traducir(op) {
                    send_op(&mut self.conn, o);
                }
            }
            None => send_op(&mut self.conn, op),
        }
    }
}

/// Gancho de pruebas (WEFT_WINDOW_INJECT=1): un socket en el directorio de estado por el que la orden interna
/// `window-inject` empuja eventos de SDL como si vinieran del teclado y del raton reales, para ejercitar el bucle
/// de eventos de la ventana (atajos, rueda, Ctrl+clic) sin tocar los dispositivos del equipo. Lineas:
///   key SC down|up [ctrl] [alt] [mayus]   (SC: codigo HID de SDL, p. ej. 58 = F1; con los modificadores indicados)
///   move X Y              (X, Y en pixeles de la VISTA, es decir de lo que muestra la ventana: con la pantalla girada
///                          90/270 grados mide alto x ancho)
///   btn N down|up X Y     (N de SDL: 1 izquierdo)
///   wheel DX DY X Y
/// y, para la barra superior y la configuracion, en coordenadas de la VENTANA (puntos):
///   wmove X Y             wbtn N down|up X Y             wwheel DX DY X Y
///   pclick NOMBRE         clic izquierdo en el centro del boton con ese nombre: con la configuracion cerrada solo hay
///                         `config` (la pestana Configuracion de la barra superior); con la pantalla de configuracion
///                         abierta, sus controles: a-cerrar, a-nav-SECCION (controles, general, atajos, entrada, pantalla,
///                         maquina, imagen, compartir, root, puente, diagnostico, acerca), a-ctl-ACCION (atras, inicio,
///                         recientes, vol-, vol+, rotar, rot0..rot270, rotauto, zoom+, zoom-, ajustar, 1:1, captura),
///                         a-mando-N, a-tog-CLAVE, a-opt-CLAVE-N, a-paso-CLAVE-mas|menos, a-campo-ancho|alto|densidad|cpus|ram,
///                         a-atajo-ACCION, a-btn-... (a-btn-reiniciar-android, a-btn-apagar, a-btn-reinicio-completo,
///                         a-btn-confirmar, a-btn-cancelar...))
///   text CADENA           evento de texto (campos de texto de la pantalla de configuracion)
/// La lista de botones y su posicion es la del ultimo dibujo; tras un cambio de pestana o de tamano hay que dejar
/// pasar un segundo antes del siguiente `pclick`. La orden `window-inject` vuelve a los 300 ms: las lineas se siguen
/// procesando en la ventana (cuenta lo que dura cada `wait`).
fn inject_thread(sdl: Sdl, _win: usize, frame: Shared, path: String) {
    use std::io::{BufRead, BufReader};
    let _ = std::fs::remove_file(&path);
    let l = match std::os::unix::net::UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => return eprintln!("gancho de pruebas: {}", e),
    };
    for conn in l.incoming().flatten() {
        for line in BufReader::new(conn).lines().map_while(Result::ok) {
            let t: Vec<&str> = line.split_whitespace().collect();
            let n = |i: usize| t.get(i).and_then(|x| x.parse::<f32>().ok()).unwrap_or(0.0);
            // posicion de la vista del dispositivo dentro de la ventana, tal como la dejo el ultimo dibujo
            let (dev, botones) = {
                let f = frame.lock().unwrap();
                (f.dev, f.botones.clone())
            };
            let k = ui_k();
            let win_xy = |x: f32, y: f32| ((dev.0 + x * dev.2 + 0.5 * dev.2) * k, (dev.1 + y * dev.2 + 0.5 * dev.2) * k);
            let mut e = Event([0; 128]);
            let put32 = |e: &mut Event, off: usize, v: u32| e.0[off..off + 4].copy_from_slice(&v.to_ne_bytes());
            let putf = |e: &mut Event, off: usize, v: f32| e.0[off..off + 4].copy_from_slice(&v.to_ne_bytes());
            match t.first().copied() {
                Some("key") => {
                    put32(&mut e, 0, if t.get(2) == Some(&"down") { EV_KEY_DOWN } else { EV_KEY_UP });
                    put32(&mut e, 24, n(1) as u32);
                    // modificadores opcionales: ctrl, alt, mayus (SDL_Keymod en el campo `mod`)
                    let mut km = 0u16;
                    for m in &t[3.min(t.len())..] {
                        km |= match *m {
                            "ctrl" => 0x0040,
                            "alt" => 0x0100,
                            "mayus" | "shift" => 0x0001,
                            _ => 0,
                        };
                    }
                    e.0[32..34].copy_from_slice(&km.to_ne_bytes());
                }
                // text CADENA: evento de texto (lo que llega al escribir en un campo de texto de la pantalla de configuracion)
                Some("text") => {
                    let cadena = line.trim_start().strip_prefix("text").unwrap_or("").trim_start();
                    let Ok(c) = CString::new(cadena) else { continue };
                    // el puntero debe vivir hasta que la ventana lea el evento: se deja filtrar (solo pruebas)
                    let p = Box::leak(c.into_boxed_c_str()).as_ptr() as u64;
                    put32(&mut e, 0, EV_TEXT_INPUT);
                    e.0[24..32].copy_from_slice(&p.to_ne_bytes());
                }
                Some("move") => {
                    let (x, y) = win_xy(n(1), n(2));
                    put32(&mut e, 0, EV_MOUSE_MOTION);
                    putf(&mut e, 28, x);
                    putf(&mut e, 32, y);
                }
                Some("btn") => {
                    let (x, y) = win_xy(n(3), n(4));
                    put32(&mut e, 0, if t.get(2) == Some(&"down") { EV_MOUSE_BUTTON_DOWN } else { EV_MOUSE_BUTTON_UP });
                    e.0[24] = n(1) as u8;
                    putf(&mut e, 28, x);
                    putf(&mut e, 32, y);
                }
                Some("wheel") => {
                    let (x, y) = win_xy(n(3), n(4));
                    put32(&mut e, 0, EV_MOUSE_WHEEL);
                    putf(&mut e, 24, n(1));
                    putf(&mut e, 28, n(2));
                    putf(&mut e, 36, x);
                    putf(&mut e, 40, y);
                }
                // coordenadas de la VENTANA (puntos), para el panel lateral: wmove X Y / wbtn N down|up X Y / wwheel DX DY X Y
                Some("wmove") => {
                    put32(&mut e, 0, EV_MOUSE_MOTION);
                    putf(&mut e, 28, n(1) * k);
                    putf(&mut e, 32, n(2) * k);
                }
                Some("wbtn") => {
                    put32(&mut e, 0, if t.get(2) == Some(&"down") { EV_MOUSE_BUTTON_DOWN } else { EV_MOUSE_BUTTON_UP });
                    e.0[24] = n(1) as u8;
                    putf(&mut e, 28, n(3) * k);
                    putf(&mut e, 32, n(4) * k);
                }
                Some("wwheel") => {
                    put32(&mut e, 0, EV_MOUSE_WHEEL);
                    putf(&mut e, 24, n(1));
                    putf(&mut e, 28, n(2));
                    putf(&mut e, 36, n(3) * k);
                    putf(&mut e, 40, n(4) * k);
                }
                // pclick NOMBRE [down|up]: clic izquierdo en el centro del boton del panel con ese nombre (ver `botones`)
                Some("pclick") => {
                    let Some((_, r)) = botones.iter().find(|(nom, _)| Some(nom.as_str()) == t.get(1).copied()) else {
                        eprintln!("gancho de pruebas: no hay un boton {:?} visible", t.get(1));
                        continue;
                    };
                    let (x, y) = ((r[0] + r[2] / 2.0) * k, (r[1] + r[3] / 2.0) * k);
                    for (kind, b) in [(EV_MOUSE_MOTION, 0u8), (EV_MOUSE_BUTTON_DOWN, 1), (EV_MOUSE_BUTTON_UP, 1)] {
                        let mut e2 = Event([0; 128]);
                        put32(&mut e2, 0, kind);
                        e2.0[24] = b;
                        putf(&mut e2, 28, x);
                        putf(&mut e2, 32, y);
                        unsafe { (sdl.push_event)(&mut e2) };
                        std::thread::sleep(Duration::from_millis(60));
                    }
                    continue;
                }
                Some("wait") => {
                    std::thread::sleep(Duration::from_millis(n(1) as u64));
                    continue;
                }
                _ => continue,
            }
            unsafe { (sdl.push_event)(&mut e) };
        }
    }
}

/// Orden interna `window-inject LINEA...`: manda lineas al gancho de pruebas de la ventana (ver inject_thread).
pub fn inject_cli(state_dir: &std::path::Path, lines: &[String]) -> Result<i32, String> {
    use std::io::Write;
    let mut s = UnixStream::connect(state_dir.join("window-inject.sock")).map_err(|e| format!("gancho de pruebas (arranca la ventana con WEFT_WINDOW_INJECT=1): {}", e))?; // texto-interno: orden interna de pruebas
    for l in lines {
        writeln!(s, "{}", l).map_err(|e| e.to_string())?;
    }
    std::thread::sleep(Duration::from_millis(300));
    Ok(0)
}

/// Orden interna `dbus-input` (pruebas): envia entrada a la pantalla D-Bus de la maquina sin abrir ventana.
/// AVISO: QEMU admite un solo cliente D-Bus y el nuevo desaloja al anterior: con la ventana abierta, esta orden
/// la deja sin entrada (la ventana termina). Usarla solo con la maquina arrancada sin ventana, o relanzar la ventana.
///   key QNUM...                        pulsa y suelta cada tecla (numero de tecla de QEMU, p. ej. 0xb2)
///   seq d:QNUM,u:QNUM,w:MS...         secuencia de pulsar/soltar/esperar
///   atajo atras|inicio|recientes|vol-|vol+|menu|encendido   la secuencia de teclas del atajo de la ventana
///   swipe X Y v|h PASOS ANCHO ALTO     lo que hace la rueda (deslizamiento con el lapiz)
///   pinch X Y DX DY ANCHO ALTO         pellizco: ancla en (X,Y), el cursor se mueve (DX,DY) en pasos
///   introspect [RUTA]                  imprime la introspeccion D-Bus de la consola
pub fn input_cli(qmp_path: &str, a: &mut dyn Iterator<Item = String>) -> Result<i32, String> {
    eprintln!("{}", tx!("window.aviso_esta_conexion_desaloja_a_la"));
    let mut sock = dial(qmp_path, "weft-entrada")?;
    let mut conn = Conn::new(sock.try_clone().map_err(|e| e.to_string())?);
    let args: Vec<String> = a.collect();
    let num = |i: usize| -> Result<f64, String> {
        let t = args.get(i).ok_or("faltan argumentos")?; // texto-interno: orden interna de depuracion (dbus-input)
        match t.strip_prefix("0x") {
            Some(h) => u32::from_str_radix(h, 16).map(|v| v as f64).map_err(|e| e.to_string()),
            None => t.parse::<f64>().map_err(|e| format!("{}: {}", t, e)),
        }
    };
    let play = |conn: &mut Conn, ops: &[(u32, Op)]| {
        let t0 = Instant::now();
        for (ms, op) in ops {
            let due = Duration::from_millis(*ms as u64);
            if let Some(w) = due.checked_sub(t0.elapsed()) {
                std::thread::sleep(w);
            }
            send_op(conn, *op);
        }
    };
    match args.first().map(|s| s.as_str()) {
        Some("key") => {
            let lista: Vec<String> = args[1..].iter().flat_map(|t| t.split([',', ' ']).filter(|x| !x.is_empty()).map(String::from)).collect();
            for t in lista {
                let q = match t.strip_prefix("0x") {
                    Some(h) => u32::from_str_radix(h, 16).map_err(|e| e.to_string())?,
                    None => t.parse::<u32>().map_err(|e| format!("{}: {}", t, e))?,
                };
                play(&mut conn, &[(0, Op::Key(q, true)), (30, Op::Key(q, false))]);
            }
        }
        Some("seq") => {
            // secuencia de teclas: d:QNUM (pulsar), u:QNUM (soltar), w:MS (esperar)
            let mut t = 0u32;
            let mut ops = Vec::new();
            for tok in args[1..].iter().flat_map(|a| a.split(',')) {
                let (k, v) = tok.split_once(':').ok_or("seq: se espera d:QNUM, u:QNUM o w:MS")?; // texto-interno: orden interna de depuracion (dbus-input)
                let n = match v.strip_prefix("0x") {
                    Some(h) => u32::from_str_radix(h, 16).map_err(|e| e.to_string())?,
                    None => v.parse::<u32>().map_err(|e| e.to_string())?,
                };
                match k {
                    "d" => ops.push((t, Op::Key(n, true))),
                    "u" => ops.push((t, Op::Key(n, false))),
                    "w" => t += n,
                    _ => return Err("seq: d, u o w".into()),
                }
                t += 20;
            }
            play(&mut conn, &ops);
        }
        Some("atajo") => {
            let at = match args.get(1).map(|s| s.as_str()) {
                Some("atras") => Atajo::Atras,
                Some("inicio") => Atajo::Inicio,
                Some("recientes") => Atajo::Recientes,
                Some("vol-") => Atajo::VolBajar,
                Some("vol+") => Atajo::VolSubir,
                Some("menu") => Atajo::Menu,
                Some("encendido") => Atajo::Encendido,
                _ => return Err("atajo: atras, inicio, recientes, vol-, vol+, menu, encendido".into()),
            };
            let ops: Vec<(u32, Op)> = gestos::teclas_de(at).unwrap().into_iter().enumerate().map(|(i, o)| (i as u32 * 30, o)).collect();
            play(&mut conn, &ops);
        }
        Some("swipe") => {
            let h = args.get(3).is_some_and(|s| s == "h");
            let ops = gestos::swipe((num(1)? as i32, num(2)? as i32), h, num(4)?, (num(5)? as u32, num(6)? as u32));
            for (ms, op) in &ops {
                println!("{:4} ms {:?}", ms, op);
            }
            play(&mut conn, &ops);
        }
        Some("pinch") => {
            let (ax, ay, dx, dy, w, h) = (num(1)?, num(2)?, num(3)?, num(4)?, num(5)? as u32, num(6)? as u32);
            let t = |ops: &mut Vec<(u32, Op)>, ms: u32, k: MtKind, p: [(f64, f64); 2]| {
                for (i, c) in p.iter().enumerate() {
                    ops.push((ms, Op::Touch(k, i as u64, c.0, c.1)));
                }
            };
            let mut ops = Vec::new();
            t(&mut ops, 0, MtKind::Begin, gestos::pellizco((ax, ay), (ax, ay), (w, h)));
            for i in 1..=10u32 {
                let f = i as f64 / 10.0;
                t(&mut ops, i * 30, MtKind::Update, gestos::pellizco((ax, ay), (ax + dx * f, ay + dy * f), (w, h)));
            }
            t(&mut ops, 330, MtKind::End, gestos::pellizco((ax, ay), (ax + dx, ay + dy), (w, h)));
            for (ms, op) in &ops {
                println!("{:4} ms {:?}", ms, op);
            }
            play(&mut conn, &ops);
        }
        Some("introspect") => {
            let mut m = Msg::call(args.get(1).map_or(CONSOLE, |s| s.as_str()), "org.freedesktop.DBus.Introspectable", "Introspect");
            m.set_body(&[]);
            let serial = conn.send(&m)?;
            sock.set_read_timeout(Some(Duration::from_secs(5))).ok();
            loop {
                let r = dbus::read_msg(&mut sock)?.ok_or(tx!("window.la_pantalla_cerro_la_conexion"))?;
                if r.reply_serial == serial && r.kind == dbus::METHOD_RETURN {
                    println!("{}", R::new(&r.body).str()?);
                    break;
                }
                if r.reply_serial == serial && r.kind == dbus::ERROR {
                    return Err(format!("{}: {}", r.error, R::new(&r.body).str().unwrap_or_default()));
                }
            }
        }
        _ => return Err("dbus-input: key, atajo, swipe, pinch o introspect".into()), // texto-interno: orden interna de depuracion (dbus-input)
    }
    // dar tiempo a que QEMU procese el ultimo mensaje antes de cerrar
    std::thread::sleep(Duration::from_millis(200));
    Ok(0)
}

/// F7: da un paso la orientacion pedida (0 -> 90 -> 180 -> 270 -> auto; archivo `orientation` del estado, que sigue el
/// servicio de sensores y esta ventana).
fn rotar(dir: &std::path::Path) {
    let _ = pantalla::escribir_orientacion(dir, pantalla::leer_orientacion(dir).siguiente());
}

/// Estado de los gestos sinteticos de la ventana: operaciones programadas en el tiempo (atajos y deslizamientos de
/// la rueda), el pellizco con Ctrl+clic y la rueda pendiente.
#[derive(Default)]
struct Gestos {
    /// (instante, operacion, de un deslizamiento)
    cola: VecDeque<(Instant, Op, bool)>,
    /// el lapiz sintetico esta pulsado
    boton: bool,
    /// ancla y contactos del pellizco en curso
    pellizco: Option<(f64, f64)>,
    cursor: Option<(i32, i32)>,
    size: (u32, u32),
    /// pasos de rueda acumulados (horizontal, vertical)
    pend: (f64, f64),
    /// largo de un paso de rueda y tope del gesto, como fraccion de la pantalla (`rueda.paso`, `rueda.tope`)
    paso: f64,
    tope: f64,
}

/// Por debajo de esto la rueda se acumula sin deslizar: un trazo muy corto lo tomaria Android por un toque.
const RUEDA_MIN: f64 = 0.25;

impl Gestos {
    /// Milisegundos que se puede esperar a un evento antes de la siguiente operacion programada.
    fn espera(&self) -> i32 {
        match self.cola.front() {
            Some((t, _, _)) => t.saturating_duration_since(Instant::now()).as_millis().min(250) as i32,
            None => 250,
        }
    }

    fn deslizando(&self) -> bool {
        self.cola.iter().any(|c| c.2) || self.boton
    }

    /// Programa operaciones (milisegundos relativos) a continuacion de lo ya programado.
    fn programar(&mut self, ops: Vec<(u32, Op)>, _tag: u8) {
        let base = self.cola.back().map_or(Instant::now(), |c| c.0).max(Instant::now());
        for (ms, op) in ops {
            self.cola.push_back((base + Duration::from_millis(ms as u64), op, false));
        }
    }

    /// Envia lo que ya toca.
    fn avanzar(&mut self, out: &mut Salida) {
        let ahora = Instant::now();
        while self.cola.front().is_some_and(|c| c.0 <= ahora) {
            let (_, op, desliz) = self.cola.pop_front().unwrap();
            if desliz {
                match op {
                    Op::Btn(0, true) => self.boton = true,
                    Op::Btn(0, false) => self.boton = false,
                    _ => {}
                }
            }
            out.op(op);
        }
        self.empezar_rueda();
    }

    /// Cancela un deslizamiento en curso (el raton real tiene prioridad) soltando el lapiz si estaba pulsado.
    fn abortar(&mut self, out: &mut Salida) {
        self.cola.retain(|c| !c.2);
        if self.boton {
            self.boton = false;
            out.op(Op::Btn(0, false));
        }
        self.pend = (0.0, 0.0);
        if self.pellizco.is_some() {
            self.pellizco = None;
            for slot in 0..2 {
                out.op(Op::Touch(MtKind::Cancel, slot, 0.0, 0.0));
            }
        }
    }

    fn rueda(&mut self, h: f64, v: f64) {
        self.pend.0 += h;
        self.pend.1 += v;
        self.empezar_rueda();
    }

    fn empezar_rueda(&mut self) {
        if self.deslizando() || self.size.0 == 0 {
            return;
        }
        let (h, v) = self.pend;
        let horizontal = h.abs() > v.abs();
        let pasos = if horizontal { h } else { v };
        if pasos.abs() < RUEDA_MIN {
            return;
        }
        self.pend = (0.0, 0.0);
        let cursor = self.cursor.unwrap_or((self.size.0 as i32 / 2, self.size.1 as i32 / 2));
        let plan = gestos::swipe_con(cursor, horizontal, pasos, self.size, self.paso, self.tope);
        let fin = plan.last().map_or(0, |p| p.0);
        let base = Instant::now();
        for (ms, op) in plan {
            self.cola.push_back((base + Duration::from_millis(ms as u64), op, true));
        }
        // el cursor real vuelve a su sitio
        self.cola.push_back((base + Duration::from_millis(fin as u64 + 15), Op::Abs(cursor.0 as u32, cursor.1 as u32), true));
    }

    fn iniciar_pellizco(&mut self, out: &mut Salida, cursor: (f64, f64)) {
        self.pellizco = Some(cursor);
        for (i, p) in gestos::pellizco(cursor, cursor, self.size).iter().enumerate() {
            out.op(Op::Touch(MtKind::Begin, i as u64, p.0, p.1));
        }
    }

    fn mover_pellizco(&mut self, out: &mut Salida, cursor: (f64, f64)) {
        if let Some(ancla) = self.pellizco {
            for (i, p) in gestos::pellizco(ancla, cursor, self.size).iter().enumerate() {
                out.op(Op::Touch(MtKind::Update, i as u64, p.0, p.1));
            }
        }
    }

    fn soltar_pellizco(&mut self, out: &mut Salida, cursor: (f64, f64)) {
        if let Some(ancla) = self.pellizco.take() {
            for (i, p) in gestos::pellizco(ancla, cursor, self.size).iter().enumerate() {
                out.op(Op::Touch(MtKind::End, i as u64, p.0, p.1));
            }
        }
    }
}

/// Como termino la ventana.
pub enum Exit {
    /// el usuario cerro la ventana
    Closed,
    /// la maquina termino (o cerro la pantalla)
    Ended(String),
}

/// La maquina se cayo de verdad sin que nadie lo pidiera: lo decide `fin_inesperado` con el mismo criterio con que la
/// ventana muestra su aviso (no cuenta un apagado pedido, ni una pantalla perdida con QEMU vivo). Lleva lo que habia en
/// los registros de ESTA ejecucion en ese momento (un arranque nuevo los rota).
#[derive(Clone, Debug, PartialEq)]
pub struct Fallo {
    /// lo que dijo la conexion con la pantalla
    pub causa: String,
    /// lo ultimo de qemu.log
    pub qemu_log: String,
    /// lo ultimo de events.log
    pub eventos: String,
}

/// Gancho de pruebas (con WEFT_WINDOW_INJECT=1): guarda en `ruta` (PPM) lo que la ventana acaba de dibujar. Se
/// pide creando el archivo `window-capture.request` del estado con la ruta dentro.
fn capturar_ventana(sdl: &Sdl, ren: P, ruta: &str) -> Result<(u32, u32), String> {
    unsafe {
        let s = (sdl.read_pixels)(ren, std::ptr::null());
        if s.is_null() {
            return Err(format!("SDL_RenderReadPixels: {}", sdl.error()));
        }
        let c = (sdl.convert_surface)(s, 0x16161804); // XRGB8888: en memoria B, G, R, X
        (sdl.destroy_surface)(s);
        if c.is_null() {
            return Err(format!("SDL_ConvertSurface: {}", sdl.error()));
        }
        let b = c as *const u8;
        let (w, h, pitch) = (*(b.add(8) as *const i32), *(b.add(12) as *const i32), *(b.add(16) as *const i32));
        let px = *(b.add(24) as *const *const u8);
        let mut out = format!("P6\n{} {}\n255\n", w, h).into_bytes();
        for y in 0..h as usize {
            let row = std::slice::from_raw_parts(px.add(y * pitch as usize), w as usize * 4);
            for p in row.chunks_exact(4) {
                out.extend_from_slice(&[p[2], p[1], p[0]]);
            }
        }
        (sdl.destroy_surface)(c);
        std::fs::write(ruta, out).map_err(|e| e.to_string())?;
        Ok((w as u32, h as u32))
    }
}

/// F1/F2/F3/F5/F6 mandan la secuencia de teclas del atajo; F7 gira un paso. Lo usan las teclas y los botones del panel.
fn lanzar_atajo(g: &mut Gestos, dir: &std::path::Path, at: Atajo) {
    match gestos::teclas_de(at) {
        Some(ops) => g.programar(ops.into_iter().enumerate().map(|(i, o)| (i as u32 * 25, o)).collect(), 0),
        None => rotar(dir),
    }
}

/// Lo que el atlas guarda: un glifo (estilo, tamano fisico, caracter) o una mascara (esquina redondeada, icono).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Clave {
    G(Estilo, u32, char),
    M(Mascara),
}

/// Dibuja las primitivas de `formas::Pint` (rectangulos redondeados, texto, iconos) con SDL. Glifos y mascaras viven en
/// UNA textura (atlas) blanca con el alfa de la cobertura: se tine con la modulacion de color y se mezcla por alfa, que
/// es la mezcla correcta para texto con suavizado sobre cualquier fondo. Todo se rasteriza al tamano FISICO (puntos x
/// densidad de pixeles de la ventana) y se coloca en la rejilla de pixeles; nada se escala.
struct Pintor {
    sdl: Sdl,
    ren: P,
    tex: P,
    atlas: Atlas,
    mapa: HashMap<Clave, (i32, i32, i32, i32)>,
    /// modulacion de color/alfa vigente en la textura
    tinte: Option<(u8, u8, u8, u8)>,
    /// tamano del texto (formas::tamano): lo pintado viene en dp de la interfaz y se dibuja a `u` dp de ventana por dp
    u: f32,
}

const ATLAS_LADO: i32 = 1024;

impl Pintor {
    fn nuevo(sdl: Sdl, ren: P) -> Result<Pintor, String> {
        let tex = unsafe { (sdl.create_texture)(ren, SDL_PIXELFORMAT_ARGB8888, SDL_TEXTUREACCESS_STATIC, ATLAS_LADO, ATLAS_LADO) };
        if tex.is_null() {
            return Err(txf!("window.sdl3_no_se_pudo_crear_la_textura_de", sdl.error()));
        }
        unsafe {
            (sdl.tex_blend)(tex, SDL_BLENDMODE_BLEND);
            (sdl.tex_scale)(tex, 0); // vecino mas cercano: 1 texel = 1 pixel
            (sdl.set_draw_blend)(ren, SDL_BLENDMODE_BLEND);
        }
        Ok(Pintor { sdl, ren, tex, atlas: Atlas::nuevo(ATLAS_LADO, ATLAS_LADO), mapa: HashMap::new(), tinte: None, u: 1.0 })
    }

    /// El dibujador perdio el contenido de las texturas: el atlas vuelve a empezar (cada pieza se sube al usarla).
    fn olvidar(&mut self) {
        self.mapa.clear();
        self.atlas.vaciar();
    }

    /// Posicion en el atlas de una pieza (la sube si todavia no esta). Si el atlas se llena lo vacia y vuelve a empezar.
    fn pieza(&mut self, clave: Clave, w: i32, h: i32, alfa: &dyn Fn() -> Vec<u8>) -> Option<(i32, i32, i32, i32)> {
        if let Some(r) = self.mapa.get(&clave) {
            return Some(*r);
        }
        if w <= 0 || h <= 0 {
            return None;
        }
        let (x, y) = match self.atlas.reservar(w, h) {
            Some(p) => p,
            None => {
                self.mapa.clear();
                self.atlas.vaciar();
                self.atlas.reservar(w, h)?
            }
        };
        let a = alfa();
        let mut px = Vec::with_capacity(a.len() * 4);
        for v in a {
            px.extend_from_slice(&[255, 255, 255, v]);
        }
        let r = Rect { x, y, w, h };
        unsafe { (self.sdl.update_texture)(self.tex, &r, px.as_ptr() as *const c_void, w * 4) };
        let e = (x, y, w, h);
        self.mapa.insert(clave, e);
        Some(e)
    }

    fn tintar(&mut self, c: formas::Color) {
        if self.tinte != Some((c.0, c.1, c.2, c.3)) {
            unsafe {
                (self.sdl.tex_color)(self.tex, c.0, c.1, c.2);
                (self.sdl.tex_alpha)(self.tex, c.3);
            }
            self.tinte = Some((c.0, c.1, c.2, c.3));
        }
    }

    fn rellenar(&mut self, r: formas::R, c: formas::Color) {
        if r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        unsafe {
            (self.sdl.set_color)(self.ren, c.0, c.1, c.2, c.3);
            (self.sdl.fill_rects)(self.ren, &r as *const formas::R as *const c_void, 1);
        }
    }

    /// Copia una pieza del atlas a `dst` (dp) con la inversion pedida (1 horizontal, 2 vertical).
    fn copiar(&mut self, src: (i32, i32, i32, i32), dst: formas::R, volteo: i32) {
        let s = formas::R::new(src.0 as f32, src.1 as f32, src.2 as f32, src.3 as f32);
        unsafe { (self.sdl.render_rotated)(self.ren, self.tex, &s as *const formas::R as *const c_void, &dst as *const formas::R as *const c_void, 0.0, std::ptr::null(), volteo) };
    }

    /// `esc`: pixeles fisicos por dp de la interfaz; `el`: pixeles fisicos por dp de la ventana (las coordenadas de SDL).
    fn rect(&mut self, r: formas::R, c: formas::Color, radio: f32, esc: f32, el: f32) {
        let px = |v: f32| (v * esc).round();
        let (x0, y0, x1, y1) = (px(r.x), px(r.y), px(r.x + r.w), px(r.y + r.h));
        let (w, h) = (x1 - x0, y1 - y0);
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let d = |x: f32, y: f32, w: f32, h: f32| formas::R::new(x / el, y / el, w / el, h / el);
        let rad = px(radio).min((w / 2.0).floor()).min((h / 2.0).floor());
        if rad < 1.0 {
            return self.rellenar(d(x0, y0, w, h), c);
        }
        let n = rad as i32;
        self.rellenar(d(x0 + rad, y0, w - 2.0 * rad, rad), c);
        self.rellenar(d(x0, y0 + rad, w, h - 2.0 * rad), c);
        self.rellenar(d(x0 + rad, y1 - rad, w - 2.0 * rad, rad), c);
        if let Some(p) = self.pieza(Clave::M(Mascara::Esquina(n as u32)), n, n, &|| formas::rasterizar(Mascara::Esquina(n as u32)).2) {
            self.tintar(c);
            self.copiar(p, d(x0, y0, rad, rad), 0);
            self.copiar(p, d(x1 - rad, y0, rad, rad), 1);
            self.copiar(p, d(x0, y1 - rad, rad, rad), 2);
            self.copiar(p, d(x1 - rad, y1 - rad, rad, rad), 3);
        }
    }

    fn texto(&mut self, tipo: &Tipografia, x: f32, y: f32, t: &str, e: Estilo, c: formas::Color) {
        let esc = tipo.escala();
        let el = esc / self.u;
        let m = tipo.metricas(e);
        let (x0, base) = ((x * esc).round() as i32, (y * esc).round() as i32 + m.asc);
        let px = tipo.clave_px(e);
        self.tintar(c);
        for (dx, ch, g) in tipo.trazar(t, e) {
            if g.w <= 0 || g.h <= 0 {
                continue;
            }
            let Some(p) = self.pieza(Clave::G(e, px, ch), g.w, g.h, &|| g.alfa.clone()) else { continue };
            let dst = formas::R::new((x0 + dx) as f32 / el, (base - g.top) as f32 / el, g.w as f32 / el, g.h as f32 / el);
            self.copiar(p, dst, 0);
        }
    }

    fn icono(&mut self, k: formas::Icono, r: formas::R, c: formas::Color, esc: f32, el: f32) {
        let n = ((r.w.min(r.h)) * esc).round().max(1.0) as i32;
        let (cx, cy) = ((r.x + r.w / 2.0) * esc, (r.y + r.h / 2.0) * esc);
        let (x0, y0) = ((cx - n as f32 / 2.0).round(), (cy - n as f32 / 2.0).round());
        let m = Mascara::Icono(k, n as u32);
        if let Some(p) = self.pieza(Clave::M(m), n, n, &|| formas::rasterizar(m).2) {
            self.tintar(c);
            self.copiar(p, formas::R::new(x0 / el, y0 / el, n as f32 / el, n as f32 / el), 0);
        }
    }

    /// Ejecuta las primitivas en orden.
    fn pintar(&mut self, tipo: &Tipografia, lista: &[Pint]) {
        let (esc, u) = (tipo.escala(), self.u);
        let el = esc / u;
        for p in lista {
            match p {
                Pint::Rect { r, c, radio } => self.rect(*r, *c, *radio, esc, el),
                Pint::Texto { x, y, t, e, c } => self.texto(tipo, *x, *y, t, *e, *c),
                Pint::Icono { k, r, c } => self.icono(*k, *r, *c, esc, el),
                Pint::Recorte(r) => {
                    let rc = r.map(|r| Rect { x: (r.x * u).round() as i32, y: (r.y * u).round() as i32, w: (r.w * u).round() as i32, h: (r.h * u).round() as i32 });
                    unsafe { (self.sdl.set_clip)(self.ren, rc.as_ref().map_or(std::ptr::null(), |r| r as *const Rect)) };
                }
            }
        }
        unsafe { (self.sdl.set_clip)(self.ren, std::ptr::null()) };
    }
}

/// Abre la ventana y la mantiene hasta que se cierra o termina la maquina.
/// `al_fallar`: se llama una vez, en cuanto la ventana sabe que la maquina se cayo (ver `Fallo`), antes de mostrar el
/// aviso y esperar a que se cierre.
pub fn serve(qmp_path: &str, title: &str, size: (u32, u32), al_fallar: &dyn Fn(&Fallo)) -> Result<Exit, String> {
    // todo lo que esta ventana muestra (progreso, resultados, errores, estado, doctor) usa el texto generico
    crate::textos::fijar_interfaz(true);
    let sdl = Sdl::load()?;
    let c = |s: &str| CString::new(s).unwrap();
    let id = app_id(std::env::var("FLATPAK_ID").ok(), std::env::var("WEFT_APP_ID").ok());
    unsafe {
        (sdl.set_hint)(c("SDL_APP_ID").as_ptr(), c(&id).as_ptr());
        // SDL impide el salvapantallas mientras hay una ventana: se permite, y la ventana lo vuelve a impedir solo con un
        // mando conectado a la maquina (ver `permitir_salvapantallas`)
        (sdl.set_hint)(c("SDL_VIDEO_ALLOW_SCREENSAVER").as_ptr(), c("1").as_ptr());
        if !(sdl.init)(SDL_INIT_VIDEO) {
            return Err(format!("SDL3: {}", sdl.error()));
        }
    }
    // la interfaz nombra las teclas de los atajos como las rotula el teclado del equipo (en config sigue su codigo)
    let (de_codigo, nombre_de) = (sdl.key_from_scancode, sdl.key_name);
    gestos::registrar_nombres(Box::new(move |sc| {
        let k = unsafe { de_codigo(sc as i32, 0, false) };
        if k == 0 {
            return None;
        }
        let p = unsafe { nombre_de(k) };
        (!p.is_null()).then(|| unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned())
    }));
    let mut area = Rect::default();
    unsafe { (sdl.usable_bounds)((sdl.primary_display)(), &mut area) };
    // el area util en dp (con WEFT_ESCALA_UI la interfaz mide k veces mas por punto)
    (area.w, area.h) = ((area.w as f32 / ui_k()).round() as i32, (area.h as f32 / ui_k()).round() as i32);
    let state_dir = std::path::Path::new(qmp_path).parent().map(|d| d.to_path_buf()).unwrap_or_default();
    // configuracion (archivo `config` del estado): atajos, zoom inicial y el resto de ajustes de la ventana
    let mut cfg = Config::cargar(&state_dir);
    // tamano del texto de la interfaz (la barra entra en el tamano inicial de la ventana)
    formas::tamano::fijar(formas::tamano::de_config(&cfg.get("ventana.texto")));
    for a in &cfg.avisos {
        eprintln!("ventana: {}", a);
    }
    let mut cfg_mtime = mtime_config(&state_dir);
    let rot0 = pantalla::rotacion_ventana(&state_dir, cfg.bool("pantalla.giro_android"));
    let v0 = pantalla::vista(rot0, size);
    // zoom: el «Zoom inicial» de `config` (ajustar o un porcentaje fijo); los atajos y Controles lo cambian solo en esta
    // ventana, sin guardarlo
    let mut zoom = vista::zoom_de_config(&cfg);
    let escala_modo = |m: ModoZoom| match m {
        ModoZoom::Fijo(p) => Some(p as f32 / 100.0),
        ModoZoom::Ajustar => None,
    };
    let ((ww, wh), _) = vista::ventana_para(v0, escala_modo(zoom), (area.w, area.h));
    let win = unsafe { (sdl.create_window)(c(title).as_ptr(), (ww as f32 * ui_k()).round() as i32, (wh as f32 * ui_k()).round() as i32, SDL_WINDOW_RESIZABLE | SDL_WINDOW_HIGH_PIXEL_DENSITY) };
    if win.is_null() {
        return Err(txf!("window.sdl3_no_se_pudo_crear_la_ventana", sdl.error()));
    }
    fijar_minimo(&sdl, win, &area);
    let ren = unsafe { (sdl.create_renderer)(win, std::ptr::null()) };
    if ren.is_null() {
        return Err(txf!("window.sdl3_no_se_pudo_crear_el_dibujador", sdl.error()));
    }
    unsafe { (sdl.set_vsync)(ren, 1) };
    // tipografia de la interfaz: fuente del sistema a la medida fisica (puntos x densidad de pixeles de la ventana)
    let escala_ui = move |win: P| -> f32 {
        let d = unsafe { (sdl.pixel_density)(win) };
        (if d.is_finite() && d >= 0.5 { d } else { 1.0 }) * ui_k()
    };
    let tipo = Tipografia::nueva(escala_ui(win) * formas::tamano::factor());
    eprintln!("ventana: fuente {} (escala {})", tipo.nombre(), tipo.escala());
    let mut pintor = Pintor::nuevo(sdl, ren)?;

    let frame: Shared = Arc::new(Mutex::new(Frame { rot: rot0, ..Frame::default() }));
    let conn = connect(qmp_path, frame.clone(), sdl)?;
    // con --pointer multitouch el raton es un contacto de la pantalla tactil (el estado lo anota `start`)
    let tactil = std::fs::read_to_string(state_dir.join("pointer")).is_ok_and(|t| t.trim() == "multitouch");
    let mut out = Salida { conn, rot: rot0, panel: size, tactil: if tactil { Some(Tactil::default()) } else { None } };
    if std::env::var("WEFT_WINDOW_INJECT").is_ok_and(|v| v == "1") {
        let (f2, w2) = (frame.clone(), win as usize);
        let sock = std::path::Path::new(qmp_path).with_file_name("window-inject.sock").to_string_lossy().into_owned();
        std::thread::spawn(move || inject_thread(sdl, w2, f2, sock));
    }
    let mut barra = Barra::default();
    let servicios = Servicios::new(&state_dir, Arc::new(move || sdl.wake()));
    // tema: la preferencia de color del escritorio (portal de ajustes), que se escucha mientras dure la ventana
    let esquema = escuchar_esquema(Arc::new(move || sdl.wake()));
    formas::tema::fijar(tema_de(&cfg, &esquema));
    let mut ajustes = Ajustes::nuevo();
    ajustes.perfiles_dir = Some(crate::dispositivo::carpeta_usuario(&crate::rutas::actual().con_claves(&cfg.get("dir.datos"), &cfg.get("dir.cache"))));
    ajustes.estado_maquina = Some(state_dir.clone());
    // que teclas son atajos de la ventana (y se traga hasta su soltado) y cuales van al invitado; modificadores apretados
    // en el teclado real (los haya recibido el invitado o no)
    let mut filtro = FiltroAtajos::default();
    let mut mods = Mods::default();
    let mut cfg_sucia = false;
    // minimizada u oculta: no se sube ni se presenta nada; pantalla completa; salvapantallas permitido; textura perdida;
    // Ctrl+rueda acumulada
    let (mut visible, mut completa, mut salvapantallas, mut forzar_subida, mut rueda_zoom) = (true, false, true, false, 0f64);
    // el usuario pidio cerrar la ventana: si el apagado por el proceso hijo no se puede lanzar, se cierra como antes
    let mut cerrando = false;
    // la maquina termino (o se perdio su pantalla) sin que nadie lo pidiera: aviso modal con el final de qemu.log y el
    // motivo de la conexion; la ventana sigue viva hasta que se pulsa Cerrar
    let mut despedida: Option<(vista::Despedida, String)> = None;

    let (mut tex, mut tex_key): (P, (u32, u32, u32)) = (std::ptr::null_mut(), (0, 0, 0));
    let (mut last_gen, mut dirty) = (0u64, true);
    let mut keys: BTreeSet<u32> = BTreeSet::new();
    let mut buttons: BTreeSet<u32> = BTreeSet::new();
    let mut ev = Event([0; 128]);
    let mut g = Gestos { paso: gestos::SWIPE_POR_PASO, tope: gestos::SWIPE_MAX, ..Gestos::default() };
    // tamano de la ventana para el que se fijo la presentacion logica, y ultimo tamano de la pantalla virtual visto
    let (mut logica, mut panel_visto) = ((0i32, 0i32), (0u32, 0u32));
    let mut t_archivos = Instant::now();
    let mut t_animar = Instant::now();
    let mut texto_activo = false;
    let inyectable = std::env::var("WEFT_WINDOW_INJECT").is_ok_and(|v| v == "1");
    let mut captura: Option<String> = None;
    // WEFT_WINDOW_STATS=SEGUNDOS: cada tantos segundos anota en el registro los cuadros recibidos y mostrados
    let stats = std::env::var("WEFT_WINDOW_STATS").ok().and_then(|v| v.parse::<u64>().ok()).filter(|s| *s > 0).map(Duration::from_secs);
    // mostrados: redibujos presentados (tambien los solo de la interfaz); subidos: cuadros de la maquina subidos a la textura
    let (mut t_stats, mut shown, mut subidos, mut prev) = (Instant::now(), 0u64, 0u64, (0u64, 0u64, 0u64));
    let exit = 'main: loop {
        let wait = g.espera();
        let mut got = unsafe { (sdl.wait_event)(&mut ev, wait) };
        // acciones de la ventana (atajos, rotacion, zoom, captura) que resuelve tras vaciar los eventos
        let mut acciones: Vec<Accion> = Vec::new();
        // efectos de la pantalla de configuracion y claves de configuracion que cambiaron (por ella o por editar el archivo)
        let mut efectos: Vec<Efecto> = Vec::new();
        let mut cambios: Vec<String> = Vec::new();
        // la X o Alt+F4 (la peticion de cierre y, por ser la unica ventana, tambien el evento de salida de SDL) y la
        // salida sola (SIGINT o SIGTERM, que SDL convierte en ese evento)
        let (mut cierre_pedido, mut salida_pedida) = (false, false);
        while got {
            let k = ev.kind();
            // estado de la ventana (tambien con el aviso de fin abierto): visible, pantalla completa, textura perdida
            let se_veia = visible;
            visible = visible_tras(k, visible);
            match k {
                EV_WINDOW_ENTER_FULLSCREEN => completa = true,
                EV_WINDOW_LEAVE_FULLSCREEN => completa = false,
                EV_RENDER_TARGETS_RESET => forzar_subida = true,
                EV_RENDER_DEVICE_RESET => {
                    // las texturas se perdieron: la del cuadro se vuelve a crear y el atlas de glifos se rellena al pintar
                    (forzar_subida, tex_key) = (true, (0, 0, 0));
                    pintor.olvidar();
                }
                _ => {}
            }
            if (visible && !se_veia) || matches!(k, EV_WINDOW_ENTER_FULLSCREEN | EV_WINDOW_LEAVE_FULLSCREEN | EV_RENDER_TARGETS_RESET | EV_RENDER_DEVICE_RESET) {
                dirty = true;
            }
            if let Some((d, why)) = despedida.as_mut() {
                // aviso de fin inesperado: modal, solo responde su boton (la maquina ya no recibe nada)
                match k {
                    EV_QUIT | EV_WINDOW_CLOSE_REQUESTED => break 'main Exit::Ended(why.clone()),
                    EV_KEY_DOWN if ev.0[37] == 0 && d.tecla(ev.u32(24)) => break 'main Exit::Ended(why.clone()),
                    EV_MOUSE_MOTION | EV_MOUSE_BUTTON_DOWN | EV_MOUSE_BUTTON_UP => {
                        let (mut mx, mut my) = (0f32, 0f32);
                        unsafe { (sdl.to_logical)(ren, ev.f32(28), ev.f32(32), &mut mx, &mut my) };
                        let (mx, my) = punto_ui(mx, my);
                        let (vent, left) = (ventana_ui(&sdl, win), qbutton(ev.0[24]) == Some(0));
                        if k == EV_MOUSE_MOTION {
                            dirty |= d.mover(vent, &tipo, mx, my);
                        } else if left && k == EV_MOUSE_BUTTON_DOWN {
                            d.presionar(vent, &tipo, mx, my);
                            dirty = true;
                        } else if left {
                            if d.soltar(vent, &tipo, mx, my) {
                                break 'main Exit::Ended(why.clone());
                            }
                            dirty = true;
                        }
                    }
                    EV_WINDOW_EXPOSED | EV_WINDOW_RESIZED | EV_WINDOW_PIXEL_SIZE_CHANGED | EV_USER => dirty = true,
                    _ => {}
                }
                got = unsafe { (sdl.poll_event)(&mut ev) };
                continue;
            }
            match k {
                EV_WINDOW_CLOSE_REQUESTED => cierre_pedido = true,
                EV_QUIT => salida_pedida = true,
                EV_WINDOW_EXPOSED | EV_WINDOW_RESIZED | EV_WINDOW_PIXEL_SIZE_CHANGED | EV_USER => dirty = true,
                EV_WINDOW_MOUSE_LEAVE => {
                    if ajustes.salir() | barra.salir() {
                        dirty = true;
                    }
                }
                EV_WINDOW_FOCUS_LOST => {
                    g.abortar(&mut out);
                    // los soltados de lo apretado ya no llegaran a esta ventana
                    if filtro.pasar {
                        dirty = true;
                    }
                    filtro.olvidar();
                    mods = Mods::default();
                    // al volver, el teclado es de Android
                    if barra.foco.take().is_some() {
                        dirty = true;
                    }
                    // nada queda apretado en el invitado si la ventana pierde el foco
                    for q in std::mem::take(&mut keys) {
                        out.op(Op::Key(q, false));
                    }
                    for b in std::mem::take(&mut buttons) {
                        out.op(Op::Btn(b, false));
                    }
                }
                EV_DROP_FILE => {
                    // un archivo o carpeta soltado sobre la ventana va al campo de ruta que corresponda (ver
                    // `Ajustes::campo_para_soltar`); la pantalla de configuracion se abre en su seccion si hace falta
                    let p = u64::from_ne_bytes(ev.0[40..48].try_into().unwrap()) as *const std::os::raw::c_char;
                    if !p.is_null() {
                        let ruta = unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned();
                        let es_carpeta = std::path::Path::new(&ruta).is_dir();
                        match ajustes.campo_para_soltar(es_carpeta, &ruta) {
                            Some(campo) => {
                                let sec = ajustes::Ajustes::seccion_de(campo);
                                if !ajustes.abierto || ajustes.seccion != sec {
                                    efectos.extend(ajustes.abrir(Some(sec)));
                                }
                                ajustes.poner_ruta(campo, &ruta);
                                eprintln!("ventana: archivo soltado en {:?}", campo);
                            }
                            None => servicios.avisar_barra(tx!("ventana.suelta_zip_o_carpeta")),
                        }
                        dirty = true;
                    }
                }
                EV_TEXT_INPUT => {
                    // texto escrito en un campo de texto de la pantalla de configuracion (el puntero va en el byte 24)
                    if ajustes.abierto {
                        let p = u64::from_ne_bytes(ev.0[24..32].try_into().unwrap()) as *const std::os::raw::c_char;
                        if !p.is_null() {
                            let t = unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned();
                            if ajustes.texto(&t) {
                                dirty = true;
                            }
                        }
                    }
                }
                EV_KEY_DOWN | EV_KEY_UP => {
                    // la repeticion la hace el invitado
                    let repeat = ev.0[37] != 0;
                    let sc = ev.u32(24);
                    let abajo = k == EV_KEY_DOWN;
                    mods.tecla(sc, abajo);
                    // modificadores: los del evento (SDL_Keymod) y los apretados en el teclado real
                    let km = u16::from_ne_bytes([ev.0[32], ev.0[33]]);
                    let ctrl = km & 0x00c0 != 0 || mods.ctrl();
                    let alt = km & 0x0300 != 0 || mods.alt();
                    let mayus = km & 0x0003 != 0 || mods.mayus();
                    if ajustes.abierto && abajo && ajustes.quiere_texto() && ((ctrl && !alt && sc == 25) || (mayus && sc == 73)) {
                        // Ctrl+V o Mayus+Insert en un campo de texto: pega el portapapeles
                        if ajustes.pegar(&sdl.portapapeles()) {
                            dirty = true;
                        }
                    } else if ajustes.abierto {
                        // pantalla de configuracion: nada llega al invitado
                        let datos = datos_ajustes(&cfg, &state_dir, &tipo, &servicios, &frame, zoom);
                        let c = ajustes::Ctx { vent: ventana_ui(&sdl, win), m: &tipo, datos: &datos, ahora: Instant::now() };
                        let (_, ef) = ajustes.tecla(&mut cfg, &c, Tecla { sc, abajo, repetida: repeat, ctrl, alt, mayus });
                        efectos.extend(ef);
                        dirty = true;
                    } else if barra.foco.is_some() {
                        // la barra tiene el teclado: nada nuevo llega a Android; solo el soltado de lo que ya tenia apretado.
                        // Su propio atajo (F10) lo devuelve, como Esc
                        if abajo {
                            let suyo = !repeat && atajo_ventana(&cfg, &ajustes, sc, ctrl, alt, mayus) == Some(AtajoVentana::Extra(AtajoExtra::Barra));
                            if suyo {
                                barra.foco = None;
                            } else if !ctrl && !alt {
                                acciones.extend(barra.tecla(sc, mayus, repeat));
                            }
                        } else if let Some(q) = qnum(sc) {
                            if keys.remove(&q) {
                                out.op(Op::Key(q, false));
                            }
                        }
                        dirty = true;
                    } else {
                        // atajo de la ventana (no llega a la aplicacion: ni al pulsar, ni al repetir, ni al soltar) o tecla
                        // para el invitado; ver `FiltroAtajos`
                        let atajo = if abajo && !repeat { atajo_ventana(&cfg, &ajustes, sc, ctrl, alt, mayus) } else { None };
                        let activos = ajustes.valor(&cfg, gestos::ATAJOS_DESACTIVADOS) != "si";
                        let pasar = filtro.pasar;
                        match filtro.tecla(sc, abajo, repeat, atajo, activos) {
                            Destino::Atajo(Some(AtajoVentana::Config(a))) => atajo_de_ventana(a, &mut acciones),
                            Destino::Atajo(Some(AtajoVentana::Extra(AtajoExtra::Barra))) => {
                                barra.enfocar();
                                dirty = true;
                            }
                            Destino::Atajo(Some(AtajoVentana::Extra(e))) => match accion_extra(e) {
                                Some(a) => acciones.push(a),
                                // la siguiente tecla va a Android aunque sea un atajo (la barra lo avisa)
                                None => filtro.pasar = true,
                            },
                            Destino::Atajo(None) => {}
                            Destino::Invitado => {
                                if let (Some(q), false) = (qnum(sc), repeat && abajo) {
                                    // mientras dura un pellizco, su modificador no llega al invitado (se le solto al empezar)
                                    let del_pellizco = g.pellizco.is_some() && gestos::qnums_pellizco(&cfg.get("pellizco.modificador")).contains(&q);
                                    if abajo {
                                        if !del_pellizco {
                                            keys.insert(q);
                                            out.op(Op::Key(q, true));
                                        }
                                    } else if keys.remove(&q) {
                                        out.op(Op::Key(q, false));
                                    }
                                }
                            }
                        }
                        if filtro.pasar != pasar {
                            dirty = true;
                        }
                    }
                }
                EV_MOUSE_MOTION | EV_MOUSE_BUTTON_DOWN | EV_MOUSE_BUTTON_UP => {
                    let (vista, dis) = ventana_vista(&sdl, win, &frame);
                    let (mut mx, mut my) = (0f32, 0f32);
                    unsafe { (sdl.to_logical)(ren, ev.f32(28), ev.f32(32), &mut mx, &mut my) };
                    let ahora = Instant::now();
                    let left = qbutton(ev.0[24]) == Some(0);
                    // la barra y la configuracion miden en dp de la interfaz (tamano del texto)
                    let (ux, uy) = punto_ui(mx, my);
                    if k == EV_MOUSE_BUTTON_DOWN && barra.foco.take().is_some() {
                        // un clic devuelve el teclado a Android
                        dirty = true;
                    }
                    if ajustes.abierto {
                        // pantalla de configuracion: modal, nada llega al panel ni al invitado
                        let datos = datos_ajustes(&cfg, &state_dir, &tipo, &servicios, &frame, zoom);
                        let c = ajustes::Ctx { vent: ventana_ui(&sdl, win), m: &tipo, datos: &datos, ahora };
                        if k == EV_MOUSE_MOTION {
                            if ajustes.mover(&cfg, &c, ux, uy) {
                                dirty = true;
                            }
                        } else if left && k == EV_MOUSE_BUTTON_DOWN {
                            ajustes.presionar(&cfg, &c, ux, uy);
                            dirty = true;
                        } else if left {
                            efectos.extend(ajustes.soltar(&mut cfg, &c, ux, uy));
                            dirty = true;
                        }
                        got = unsafe { (sdl.poll_event)(&mut ev) };
                        continue;
                    }
                    // la barra superior: sus clics no llegan al invitado
                    let pista = pista_config(&cfg, &ajustes);
                    let consumido = if k == EV_MOUSE_MOTION {
                        if barra.mover(a_ui(dis.barra), false, pista.as_deref(), &tipo, ux, uy) {
                            dirty = true;
                        }
                        barra.pulsado.is_some()
                    } else if k == EV_MOUSE_BUTTON_DOWN && dis.barra.contiene(mx, my) {
                        if left {
                            barra.presionar(a_ui(dis.barra), false, pista.as_deref(), &tipo, ux, uy);
                        }
                        dirty = true;
                        true
                    } else if k == EV_MOUSE_BUTTON_UP && barra.pulsado.is_some() {
                        if left {
                            acciones.extend(barra.soltar(a_ui(dis.barra), false, pista.as_deref(), &tipo, ux, uy));
                        }
                        dirty = true;
                        true
                    } else {
                        false
                    };
                    if !consumido {
                        // todo en coordenadas de la VISTA; la salida las lleva al panel del invitado
                        g.size = vista;
                        let (x, y) = ((mx - dis.dispositivo.x) / dis.escala.max(1e-6), (my - dis.dispositivo.y) / dis.escala.max(1e-6));
                        let inside = vista.0 > 0 && vista::a_vista(&dis, vista, (mx, my)).is_some();
                        if inside {
                            g.cursor = Some((x as i32, y as i32));
                        }
                        // el modificador del pellizco se mira en el teclado real: mientras dura el gesto, el invitado no lo tiene
                        let modificador = cfg.get("pellizco.modificador");
                        let con_modificador = mods.pellizco(&modificador);
                        if g.pellizco.is_some() {
                            // pellizco en curso: el raton solo mueve los contactos
                            if k == EV_MOUSE_MOTION {
                                g.mover_pellizco(&mut out, (x as f64, y as f64));
                            } else if k == EV_MOUSE_BUTTON_UP && left {
                                g.soltar_pellizco(&mut out, (x as f64, y as f64));
                            }
                        } else if k == EV_MOUSE_BUTTON_DOWN && left && con_modificador && inside && !buttons.contains(&0) {
                            g.abortar(&mut out);
                            // Android no debe ver Ctrl (o Alt) apretado durante el gesto: se le suelta ya (su soltado real
                            // no se reenvia, el invitado ya no la tiene)
                            for q in gestos::soltar_para_pellizco(&keys, &modificador) {
                                keys.remove(&q);
                                out.op(Op::Key(q, false));
                            }
                            g.iniciar_pellizco(&mut out, (x as f64, y as f64));
                        } else {
                            // un gesto sintetico de la rueda cede ante el raton real
                            if k == EV_MOUSE_BUTTON_DOWN {
                                g.abortar(&mut out);
                            }
                            if inside && !g.deslizando() {
                                out.op(Op::Abs(x as u32, y as u32));
                            }
                            if k != EV_MOUSE_MOTION {
                                if let Some(b) = qbutton(ev.0[24]) {
                                    if out.tactil.is_some() && b != 0 {
                                        // pantalla tactil: el boton derecho y el central son atajos de Android (por
                                        // defecto Atras e Inicio) y nunca llegan como botones
                                        if k == EV_MOUSE_BUTTON_DOWN && inside {
                                            let (der, cen) = (ajustes.valor(&cfg, gestos::RATON_DERECHO), ajustes.valor(&cfg, gestos::RATON_CENTRAL));
                                            if let Some(a) = gestos::boton_tactil(b, &der, &cen) {
                                                acciones.push(Accion::Atajo(a));
                                            }
                                        }
                                    } else if k == EV_MOUSE_BUTTON_DOWN && inside {
                                        buttons.insert(b);
                                        out.op(Op::Btn(b, true));
                                    } else if k == EV_MOUSE_BUTTON_UP && buttons.remove(&b) {
                                        out.op(Op::Btn(b, false));
                                    }
                                }
                            }
                        }
                    }
                }
                EV_MOUSE_WHEEL => {
                    // la rueda se convierte en un deslizamiento con el lapiz (un lapiz ignora los botones 3/4)
                    let flip = if ev.u32(32) == 1 { -1.0 } else { 1.0 }; // SDL_MOUSEWHEEL_FLIPPED
                    let (dx, dy) = (ev.f32(24) as f64 * flip, ev.f32(28) as f64 * flip);
                    let (vista, dis) = ventana_vista(&sdl, win, &frame);
                    let (mut mx, mut my) = (0f32, 0f32);
                    unsafe { (sdl.to_logical)(ren, ev.f32(36), ev.f32(40), &mut mx, &mut my) };
                    if ajustes.abierto {
                        let datos = datos_ajustes(&cfg, &state_dir, &tipo, &servicios, &frame, zoom);
                        let c = ajustes::Ctx { vent: ventana_ui(&sdl, win), m: &tipo, datos: &datos, ahora: Instant::now() };
                        ajustes.rueda(&c, dy as f32);
                        dirty = true;
                    } else if dis.barra.contiene(mx, my) {
                        // sobre la barra superior la rueda no hace nada y tampoco llega al invitado
                    } else if mods.ctrl() {
                        // Ctrl+rueda: zoom de la ventana (un paso por cada paso de rueda; no llega al invitado)
                        match gestos::paso_zoom(&mut rueda_zoom, dy) {
                            1 => acciones.push(Accion::ZoomMas),
                            -1 => acciones.push(Accion::ZoomMenos),
                            _ => {}
                        }
                    } else {
                        g.size = vista;
                        if let Some((x, y)) = vista::a_vista(&dis, vista, (mx, my)) {
                            g.cursor = Some((x as i32, y as i32));
                        }
                        let shift = mods.mayus();
                        // no se pisa un arrastre real ni un pellizco
                        if !buttons.contains(&0) && g.pellizco.is_none() {
                            // Mayus + rueda vertical = deslizamiento horizontal
                            let (h, v) = if shift { (dx + dy, 0.0) } else { (dx, dy) };
                            let sig = if cfg.bool("rueda.invertir") { -1.0 } else { 1.0 };
                            g.paso = cfg.entero("rueda.paso").unwrap_or(10) as f64 / 100.0;
                            g.tope = cfg.entero("rueda.tope").unwrap_or(60) as f64 / 100.0;
                            g.rueda(h * sig, v * sig);
                        }
                    }
                }
                _ => {}
            }
            got = unsafe { (sdl.poll_event)(&mut ev) };
        }
        if cierre_pedido {
            // cerrar la ventana apaga la maquina, y nunca se cierra de golpe: segun la configuracion se pregunta (en la
            // pantalla de configuracion, seccion Maquina) o se lanza `stop` ya; con una operacion en curso solo se avisa.
            // La barra muestra "Apagando..." hasta que QEMU cierra la pantalla, que termina este proceso.
            let estaba_abierta = ajustes.abierto;
            let datos = datos_ajustes(&cfg, &state_dir, &tipo, &servicios, &frame, zoom);
            let c = ajustes::Ctx { vent: ventana_ui(&sdl, win), m: &tipo, datos: &datos, ahora: Instant::now() };
            efectos.extend(ajustes.cerrar_ventana(&cfg, &c));
            cerrando = true;
            if ajustes.abierto && !estaba_abierta {
                // como al abrirla con su pestana: nada queda apretado en el invitado mientras la pantalla esta abierta
                g.abortar(&mut out);
                for q in std::mem::take(&mut keys) {
                    out.op(Op::Key(q, false));
                }
                for b in std::mem::take(&mut buttons) {
                    out.op(Op::Btn(b, false));
                }
                barra.salir();
                filtro.pasar = false;
            }
            dirty = true;
        } else if salida_pedida {
            // senal al proceso: se sale como siempre y `window-serve` apaga la maquina
            break 'main Exit::Closed;
        }

        // ordenes por archivo: `rotate` (orientacion) y `resolution` (tamano del panel)
        if t_archivos.elapsed() >= Duration::from_millis(100) {
            t_archivos = Instant::now();
            // `config` editado a mano o por `weft config`: se relee y se aplican las diferencias
            let mt = mtime_config(&state_dir);
            if mt != cfg_mtime {
                cfg_mtime = mt;
                let nueva = Config::cargar(&state_dir);
                for a in &nueva.avisos {
                    eprintln!("ventana: {}", a);
                }
                cambios.extend(claves_cambiadas(&cfg, &nueva));
                cfg = nueva;
                dirty = true;
            }
            let rot = pantalla::rotacion_ventana(&state_dir, cfg.bool("pantalla.giro_android"));
            if rot != out.rot {
                g.abortar(&mut out);
                let impar = (rot % 2) != (out.rot % 2);
                let (pw, ph) = {
                    let f = frame.lock().unwrap();
                    (f.w, f.h)
                };
                if impar && pw > 0 {
                    // la ventana sigue a la vista: la nueva forma con la escala que tenia (o el zoom fijo)
                    let (_, dis) = ventana_vista(&sdl, win, &frame);
                    let esc = escala_modo(zoom).unwrap_or(dis.escala);
                    let ((nw, nh), _) = vista::ventana_para(pantalla::vista(rot, (pw, ph)), Some(esc), (area.w, area.h));
                    sdl.fijar_tam_dp(win, nw, nh);
                }
                out.rot = rot;
                g.size = (0, 0);
                g.cursor = None;
                frame.lock().unwrap().rot = rot;
                eprintln!("ventana: orientacion {} grados", pantalla::grados_de_rot(rot));
                dirty = true;
            }
            if inyectable {
                if let Ok(r) = std::fs::read_to_string(state_dir.join("window-capture.request")) {
                    let _ = std::fs::remove_file(state_dir.join("window-capture.request"));
                    captura = Some(r.trim().to_string());
                    dirty = true;
                }
            }
            if let Some(r) = pantalla::tomar_resolucion(&state_dir) {
                eprintln!("ventana: se pide al dispositivo la resolucion {}", r.texto());
                set_ui_info(&mut out.conn, &r);
            }
            // salvapantallas: permitido salvo con un mando conectado a la maquina
            let quiere = permitir_salvapantallas(servicios.mandos_conectados());
            if quiere != salvapantallas {
                salvapantallas = quiere;
                unsafe { if quiere { (sdl.enable_screensaver)() } else { (sdl.disable_screensaver)() } };
                eprintln!("ventana: salvapantallas {}", if quiere { "permitido" } else { tx!("window.impedido_hay_un_mando_conectado") });
            }
        }

        // efectos de la pantalla de configuracion (los botones de Controles piden acciones de la ventana)
        let mut acciones_pendientes: Vec<Accion> = Vec::new();
        for e in std::mem::take(&mut efectos) {
            dirty = true;
            match e {
                Efecto::Cerrar => {}
                Efecto::Cambio(k) => {
                    cfg_sucia = true;
                    cambios.push(k);
                }
                Efecto::Orientacion(o) => {
                    let _ = pantalla::escribir_orientacion(&state_dir, o);
                }
                Efecto::AplicarResolucion => {
                    let (pw, ph) = {
                        let f = frame.lock().unwrap();
                        (f.w, f.h)
                    };
                    let (w, h) = cfg.resolucion().unwrap_or((pw, ph));
                    let spec = match cfg.densidad() {
                        Some(d) => format!("{}x{}@{}", w, h, d),
                        None => format!("{}x{}", w, h),
                    };
                    ajustes.quitar_mensaje("aplicar");
                    servicios.aplicar_resolucion(spec);
                }
                Efecto::AbrirEstado => {
                    // la abre el programa del escritorio (`xdg-open`, sin shell); si no se puede, el aviso lleva la ruta
                    let ruta = std::fs::canonicalize(&state_dir).unwrap_or_else(|_| state_dir.clone());
                    eprintln!("ventana: carpeta de estado: {}", ruta.display());
                    ajustes.quitar_mensaje("estado");
                    servicios.abrir_carpeta(ruta);
                }
                Efecto::ReiniciarAndroid => servicios.reiniciar_android(),
                Efecto::ReinicioCompleto => servicios.reinicio_completo(),
                Efecto::Apagar => {
                    if !servicios.apagar() && cerrando {
                        // el apagado por el proceso hijo no se pudo lanzar: se cierra como antes y `window-serve` apaga
                        break 'main Exit::Closed;
                    }
                }
                Efecto::Mando { path, conectar } => servicios.mando(path, conectar),
                Efecto::Control(a) => acciones_pendientes.push(a),
                Efecto::Comprobar => servicios.comprobar(),
                e @ (Efecto::RootActualizar | Efecto::AlmacenActualizar | Efecto::PuenteActualizar) => refrescar(&servicios, e),
                Efecto::Orden { clave, etiqueta, args } => servicios.orden(clave, etiqueta, args),
                Efecto::CancelarOperacion => servicios.cancelar_operacion(),
                Efecto::Elegir(campo) => servicios.elegir(campo),
            }
        }
        acciones.extend(acciones_pendientes);
        // acciones de la ventana (atajos, rotacion, zoom, captura, abrir la configuracion)
        for a in acciones {
            match a {
                Accion::Atajo(at) => lanzar_atajo(&mut g, &state_dir, at),
                Accion::Rotacion(r) => {
                    let _ = pantalla::escribir_orientacion(&state_dir, pantalla::Orientacion::Fija(r));
                }
                Accion::RotacionAuto => {
                    let _ = pantalla::escribir_orientacion(&state_dir, pantalla::Orientacion::Auto);
                }
                Accion::Captura => servicios.capturar(),
                Accion::PantallaCompleta => {
                    let quiere = !completa;
                    if unsafe { (sdl.set_fullscreen)(win, quiere) } {
                        completa = quiere;
                    } else {
                        eprintln!("ventana: no se pudo cambiar la pantalla completa: {}", sdl.error());
                    }
                    dirty = true;
                }
                Accion::ZoomMas | Accion::ZoomMenos | Accion::Zoom1a1 | Accion::ZoomAjustar => {
                    let (_, dis) = ventana_vista(&sdl, win, &frame);
                    let pct = dis.escala * 100.0;
                    let modo = match a {
                        Accion::ZoomMas => ModoZoom::Fijo(vista::paso_mas(pct)),
                        Accion::ZoomMenos => ModoZoom::Fijo(vista::paso_menos(pct)),
                        Accion::Zoom1a1 => ModoZoom::Fijo(100),
                        _ => ModoZoom::Ajustar,
                    };
                    fijar_zoom(&sdl, win, &frame, &mut zoom, modo, &area);
                    dirty = true;
                }
                Accion::Configuracion => {
                    if ajustes.abierto {
                        ajustes.cerrar();
                    } else {
                        // el invitado no debe quedarse con teclas ni botones apretados mientras la pantalla esta abierta
                        g.abortar(&mut out);
                        for q in std::mem::take(&mut keys) {
                            out.op(Op::Key(q, false));
                        }
                        for b in std::mem::take(&mut buttons) {
                            out.op(Op::Btn(b, false));
                        }
                        barra.salir();
                        barra.foco = None;
                        // la tecla que iba a pasar a Android se queda en nada: la pantalla se lleva el teclado
                        filtro.pasar = false;
                        // abrir la pantalla refresca los datos de la seccion que muestra (como elegirla en la lista)
                        for e in ajustes.abrir(None) {
                            refrescar(&servicios, e);
                        }
                    }
                    dirty = true;
                }
            }
        }
        // claves de configuracion que cambiaron: se aplican en caliente lo que se puede
        for k in std::mem::take(&mut cambios) {
            dirty = true;
            match k.as_str() {
                "zoom" => {
                    let nuevo = vista::zoom_de_config(&cfg);
                    if nuevo != zoom {
                        fijar_zoom(&sdl, win, &frame, &mut zoom, nuevo, &area);
                    }
                }
                "gamepad" => servicios.mandos_auto(cfg.get("gamepad") == "auto"),
                "ventana.texto" => {
                    let antes = ventana_vista(&sdl, win, &frame).1.escala;
                    if formas::tamano::fijar(formas::tamano::de_config(&cfg.get("ventana.texto"))) {
                        // la barra cambia de alto: la ventana se ajusta para que la pantalla de Android siga a su escala
                        fijar_minimo(&sdl, win, &area);
                        let (v, _) = ventana_vista(&sdl, win, &frame);
                        if v.0 > 0 {
                            let ((nw, nh), _) = vista::ventana_para(v, Some(escala_modo(zoom).unwrap_or(antes)), (area.w, area.h));
                            sdl.fijar_tam_dp(win, nw, nh);
                        }
                        g.size = (0, 0);
                    }
                }
                // lo demas (atajos, confirmar, rueda, pellizco, giro de Android) se lee de `cfg` donde se usa
                _ => {}
            }
        }
        // el tema: `ventana.tema` o, en automatico, el escritorio (cambia en caliente con cualquiera de los dos)
        if formas::tema::fijar(tema_de(&cfg, &esquema)) {
            dirty = true;
        }
        if cfg_sucia {
            cfg_sucia = false;
            match cfg.guardar(&state_dir) {
                Ok(()) => {
                    // `guardar` mezcla lo de la ventana con lo que haya en el archivo: lo que otro proceso escribio desde
                    // la ultima lectura (`weft config set`, `image add`, `root enable`...) se aplica ya, como si lo hubiera
                    // visto el sondeo (la hora se toma antes de leer: si cambia en medio, el sondeo lo relee)
                    cfg_mtime = mtime_config(&state_dir);
                    let nueva = Config::cargar(&state_dir);
                    cambios.extend(claves_cambiadas(&cfg, &nueva));
                    cfg = nueva;
                }
                // el archivo no cambio: lo que otro escriba lo ve el sondeo
                Err(e) => eprintln!("ventana: no se pudo guardar la configuracion: {}", e),
            }
        }
        servicios.mirar(match (ajustes.abierto, ajustes.seccion) {
            (true, ajustes::Seccion::Entrada) => Sondeo::Mandos,
            (true, ajustes::Seccion::Maquina) => Sondeo::Maquina,
            _ => Sondeo::Nada,
        });
        if ajustes.abierto && ajustes.seccion == ajustes::Seccion::Acerca {
            let (hecho, en_curso) = servicios.doctor();
            if hecho.is_none() && !en_curso {
                servicios.comprobar();
            }
        }
        // la entrada de texto de SDL solo mientras se escribe en un campo de texto de la pantalla de configuracion
        let quiere_texto = ajustes.quiere_texto();
        if quiere_texto != texto_activo {
            texto_activo = quiere_texto;
            unsafe { if quiere_texto { (sdl.start_text)(win) } else { (sdl.stop_text)(win) } };
        }
        for (k, tono, t) in servicios.tomar_avisos() {
            ajustes.mensaje(&k, tono, &t);
            dirty = true;
        }
        for (campo, ruta) in servicios.tomar_elegidas() {
            eprintln!("ventana: elegido en el selector de archivos para {:?}", campo);
            dirty |= ajustes.poner_ruta(campo, &ruta);
        }
        if t_animar.elapsed() >= Duration::from_millis(250) {
            t_animar = Instant::now();
            if servicios.aviso_vigente() || (ajustes.abierto && ajustes.seccion == ajustes::Seccion::Maquina) || ajustes.animado(Instant::now()) {
                dirty = true;
            }
        }

        // QEMU cerro la pantalla: si nadie lo pidio, aviso modal antes de cerrar; si no, la ventana termina como siempre
        let fin = frame.lock().unwrap().fin.clone();
        if let (Some(why), true) = (fin, despedida.is_none()) {
            match fin_inesperado(&state_dir, &|| cerrando || servicios.cierre_pedido(), &why, Duration::from_millis(1500)) {
                None => break Exit::Ended(why),
                Some((d, fallo)) => {
                    eprintln!("ventana: {} (nadie pidio el cierre: se avisa en la ventana)", why);
                    if let Some(f) = &fallo {
                        al_fallar(f);
                    }
                    ajustes.cerrar();
                    barra.salir();
                    // el aviso de fin se lleva el teclado: la barra no recupera el foco
                    barra.volver = None;
                    despedida = Some((d, why));
                    dirty = true;
                }
            }
        }

        // la configuracion se cerro (Esc, su boton, una confirmacion o el atajo): si se abrio desde la barra con el teclado,
        // el foco vuelve a esa pestana (ver `Barra::seguir_ajustes`)
        if barra.seguir_ajustes(ajustes.abierto) {
            dirty = true;
        }

        g.avanzar(&mut out);
        // lo que se dibuja se toma del cuadro compartido con el cerrojo el minimo tiempo: los pixeles se suben despues,
        // sin el (el hilo D-Bus no espera a la textura), y solo si llego un cuadro nuevo
        let (rot, pw, ph, hay_cuadro, copia) = {
            let mut f = frame.lock().unwrap();
            if let Some(every) = stats {
                if t_stats.elapsed() >= every {
                    let secs = t_stats.elapsed().as_secs_f64();
                    eprintln!(
                        "cuadros en {:.1} s: Scanout={} avisos_descartados={} Update={} mostrados={} ({:.1}/s) subidos={} pantalla={}x{} formato={:#x}", // texto-interno: registro de depuracion (window.log)
                        secs,
                        f.scanouts - prev.0,
                        f.placeholders - prev.2,
                        f.updates - prev.1,
                        shown,
                        shown as f64 / secs,
                        subidos,
                        f.w,
                        f.h,
                        f.fmt
                    );
                    prev = (f.scanouts, f.updates, f.placeholders);
                    (shown, subidos) = (0, 0);
                    t_stats = Instant::now();
                }
            }
            // minimizada u oculta no se sube ni se presenta nada hasta que vuelva a verse (la captura de pruebas, si)
            if (f.gen == last_gen && !dirty && !forzar_subida) || (!visible && captura.is_none()) {
                continue;
            }
            last_gen = f.gen;
            let copia = tomar_cuadro(&mut f, forzar_subida);
            (f.rot, f.w, f.h, !f.buf.is_empty(), copia)
        };
        (dirty, forzar_subida) = (false, false);
        // la presentacion logica es la ventana en puntos: todo se dibuja en coordenadas de ventana
        let (mut cw, mut ch) = (0i32, 0i32);
        sdl.tam_dp(win, &mut cw, &mut ch);
        if (cw, ch) != logica {
            unsafe { (sdl.logical)(ren, cw, ch, SDL_LOGICAL_PRESENTATION_LETTERBOX) };
            logica = (cw, ch);
        }
        if let Some(c) = copia {
            let key = (c.w, c.h, c.fmt);
            if key != tex_key || tex.is_null() {
                if !tex.is_null() {
                    unsafe { (sdl.destroy_texture)(tex) };
                }
                let fmt = sdl_format(c.fmt).ok_or_else(|| txf!("window.formato_de_pixel_no_admitido", format!("{:#x}", c.fmt)))?;
                tex = unsafe { (sdl.create_texture)(ren, fmt, SDL_TEXTUREACCESS_STREAMING, c.w as i32, c.h as i32) };
                if tex.is_null() {
                    return Err(txf!("window.sdl3_no_se_pudo_crear_la_textura", sdl.error()));
                }
                tex_key = key;
            }
            unsafe { (sdl.update_texture)(tex, std::ptr::null(), c.buf[c.off..].as_ptr() as *const c_void, c.stride as i32) };
            subidos += 1;
        }
        if hay_cuadro {
            out.panel = (pw, ph);
            // el invitado cambio el tamano del panel (comando `resolution`): la ventana sigue su proporcion
            if (pw, ph) != panel_visto {
                if panel_visto != (0, 0) {
                    let v = pantalla::vista(rot, (pw, ph));
                    let ((nw, nh), _) = vista::ventana_para(v, escala_modo(zoom), (area.w, area.h));
                    sdl.fijar_tam_dp(win, nw, nh);
                    g.size = (0, 0);
                    g.cursor = None;
                }
                panel_visto = (pw, ph);
            }
        }
        let show = hay_cuadro && !tex.is_null();
        // el tamano del texto amplia la interfaz: su fuente se rasteriza a la medida ampliada (nunca se escala un mapa de bits)
        let u = formas::tamano::factor();
        tipo.fijar_escala(escala_ui(win) * u);
        pintor.u = u;
        let (uw, uh) = (cw as f32 / u, ch as f32 / u);
        let vista = pantalla::vista(rot, if pw > 0 { (pw, ph) } else { size });
        let dis = vista::disenar((cw as f32, ch as f32), vista);
        let ahora = Instant::now();
        let teclado = vista::ModoTeclado::de(ajustes.valor(&cfg, gestos::ATAJOS_DESACTIVADOS) == "si", filtro.pasar);
        let info = servicios.info(Info { rot, escala: dis.escala, orient: pantalla::leer_orientacion(&state_dir), teclado, ..Info::default() });
        unsafe {
            let l = crate::formas::tema::p().lienzo;
            (sdl.set_color)(ren, l.0, l.1, l.2, 255);
            (sdl.clear)(ren);
            if show {
                if rot % 4 == 0 {
                    (sdl.render_texture)(ren, tex, std::ptr::null(), &dis.dispositivo as *const vista::R as *const c_void);
                } else {
                    // la imagen del panel (pw x ph), centrada en la vista y girada alrededor de su centro
                    let (s, d) = (dis.escala, dis.dispositivo);
                    let (w, h) = (pw as f32 * s, ph as f32 * s);
                    let dst = vista::R::new(d.x + (d.w - w) / 2.0, d.y + (d.h - h) / 2.0, w, h);
                    (sdl.render_rotated)(ren, tex, std::ptr::null(), &dst as *const vista::R as *const c_void, pantalla::angulo_sdl(rot), std::ptr::null(), 0);
                }
            }
            // barra superior (debajo de la pantalla de configuracion, que la tapa como a todo lo demas)
            let pista_config = pista_config(&cfg, &ajustes);
            pintor.pintar(&tipo, &barra.dibujar(a_ui(dis.barra), &info, ajustes.abierto, pista_config.as_deref(), &tipo));
            // pantalla de configuracion: capa modal sobre todo lo demas
            let datos = if ajustes.abierto { Some(datos_ajustes(&cfg, &state_dir, &tipo, &servicios, &frame, zoom)) } else { None };
            if let Some(d) = &datos {
                let c = ajustes::Ctx { vent: (uw, uh), m: &tipo, datos: d, ahora };
                pintor.pintar(&tipo, &ajustes.dibujar(&cfg, &c));
            }
            // aviso de fin inesperado: encima de todo
            if let Some((d, _)) = &despedida {
                pintor.pintar(&tipo, &d.dibujar((uw, uh), &tipo));
            }
            // botones visibles (para el gancho de pruebas): el del aviso de fin, los de la pantalla de configuracion si esta
            // abierta o los de la barra, en dp de la ventana
            let botones_visibles = |datos: &Option<Datos>| -> Vec<(String, vista::R)> {
                let v = if let Some((d, _)) = &despedida {
                    vec![(vista::Despedida::BOTON.to_string(), d.boton((uw, uh), &tipo))]
                } else {
                    match datos {
                        Some(d) => ajustes.botones(&cfg, &ajustes::Ctx { vent: (uw, uh), m: &tipo, datos: d, ahora }),
                        None => barra.botones(a_ui(dis.barra), false, pista_config.as_deref(), &tipo),
                    }
                };
                v.into_iter().map(|(n, r)| (n, de_ui(r))).collect()
            };
            if let Some(ruta) = captura.take() {
                let r = capturar_ventana(&sdl, ren, &ruta);
                eprintln!("ventana: captura {} -> {:?}", ruta, r);
                let lista: String = botones_visibles(&datos).iter().map(|(n, r)| format!("{} {} {} {} {}\n", n, r.x, r.y, r.w, r.h)).collect();
                let _ = std::fs::write(format!("{}.botones", ruta), lista);
            }
            (sdl.present)(ren);
            if inyectable {
                let mut fm = frame.lock().unwrap();
                fm.botones = botones_visibles(&datos).into_iter().map(|(n, r)| (n, [r.x, r.y, r.w, r.h])).collect();
            }
        }
        if inyectable {
            let mut fm = frame.lock().unwrap();
            fm.dev = (dis.dispositivo.x, dis.dispositivo.y, dis.escala);
        } else {
            frame.lock().unwrap().dev = (dis.dispositivo.x, dis.dispositivo.y, dis.escala);
        }
        shown += 1;
    };
    unsafe { (sdl.quit)() };
    Ok(exit)
}

/// QEMU cerro la pantalla: None si el fin era de esperar (la ventana pidio apagar o reiniciar, o `events.log` dice que
/// alguien apago la maquina: `weft stop`, `restart`, `weft kill` o el propio Android); si no, el aviso que la ventana muestra
/// antes de cerrarse y, si la maquina termino de verdad (no solo se perdio su pantalla), el `Fallo` para ultimo-fallo.txt.
/// `events-serve` anota el apagado y el final de QEMU por su lado: se le dan unos instantes.
/// `pedido`: la ventana pidio el cierre (se pregunta otra vez tras la espera); `espera`: lo que se aguarda a `events-serve`.
fn fin_inesperado(state_dir: &std::path::Path, pedido: &dyn Fn() -> bool, why: &str, espera: Duration) -> Option<(vista::Despedida, Option<Fallo>)> {
    if pedido() {
        return None;
    }
    let eventos = || vista::cola_de_archivo(&state_dir.join("events.log"), 256 * 1024);
    let t0 = Instant::now();
    let mut ev = eventos();
    while !vista::fin_anotado(&ev) && !vista::cierre_ordenado(&ev) && t0.elapsed() < espera {
        std::thread::sleep(Duration::from_millis(100));
        ev = eventos();
    }
    if vista::cierre_ordenado(&ev) || pedido() {
        return None;
    }
    let en_marcha = !vista::fin_anotado(&ev) && crate::vm::State { dir: state_dir.to_path_buf() }.running();
    let registro = vista::cola_de_archivo(&state_dir.join("qemu.log"), 64 * 1024);
    let aviso = vista::Despedida::nueva(en_marcha, &vista::motivo_del_fin(&ev, why, en_marcha, vista::flatpak_id().as_deref()), &registro);
    // con QEMU vivo solo se perdio la pantalla: la maquina no fallo
    let fallo = (!en_marcha).then(|| Fallo { causa: why.to_string(), qemu_log: registro, eventos: ev });
    Some((aviso, fallo))
}

/// Efectos de la pantalla de configuracion que piden refrescar los datos de una seccion (al elegirla o al reabrirla).
fn refrescar(servicios: &Servicios, e: Efecto) {
    match e {
        Efecto::RootActualizar => servicios.root_actualizar(),
        Efecto::AlmacenActualizar => servicios.almacen_actualizar(),
        Efecto::PuenteActualizar => servicios.puente_actualizar(),
        _ => {}
    }
}

/// Fecha de modificacion del archivo de configuracion.
fn mtime_config(dir: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(Config::ruta(dir)).and_then(|m| m.modified()).ok()
}

/// Claves de `config::CLAVES` cuyo valor difiere entre la configuracion que usa la ventana y la recien leida del archivo:
/// las que hay que aplicar en caliente. Pura.
fn claves_cambiadas(antes: &Config, ahora: &Config) -> Vec<String> {
    crate::config::CLAVES.iter().filter(|k| ahora.get(k.nombre) != antes.get(k.nombre)).map(|k| k.nombre.to_string()).collect()
}

/// Tecla del atajo que abre la configuracion (texto visible, con la distribucion del teclado), para la pestana de la barra
/// superior. Con los atajos desactivados no se muestra: esa tecla va a Android.
fn pista_config(cfg: &Config, ajustes: &Ajustes) -> Option<String> {
    if ajustes.valor(cfg, gestos::ATAJOS_DESACTIVADOS) == "si" {
        return None;
    }
    ajustes.tecla_de(cfg, ajustes::FilaAtajo::Accion(AccionAtajo::Configuracion))
}

/// Un atajo de teclado de la ventana: de `config` o propio de la ventana.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AtajoVentana {
    Config(AccionAtajo),
    Extra(AtajoExtra),
}

/// Atajo que dispara una pulsacion (modificadores exactos): primero los de `config`, despues los propios de la ventana.
fn atajo_ventana(cfg: &Config, ajustes: &Ajustes, sc: u32, ctrl: bool, alt: bool, mayus: bool) -> Option<AtajoVentana> {
    cfg.atajo_para(sc, ctrl, alt, mayus).map(AtajoVentana::Config).or_else(|| ajustes.atajo_extra_para(cfg, sc, ctrl, alt, mayus).map(AtajoVentana::Extra))
}

/// Lo que dispara un atajo propio de la ventana (None para "pasar la siguiente tecla" y "ir a la barra", que solo cambian
/// el estado del teclado).
fn accion_extra(e: AtajoExtra) -> Option<Accion> {
    match e {
        AtajoExtra::PasarTecla => None,
        AtajoExtra::PantallaCompleta => Some(Accion::PantallaCompleta),
        AtajoExtra::Encendido => Some(Accion::Atajo(Atajo::Encendido)),
        AtajoExtra::Menu => Some(Accion::Atajo(Atajo::Menu)),
        // la barra toma el teclado: lo resuelve el bucle de la ventana
        AtajoExtra::Barra => None,
    }
}

/// Lo que dispara una tecla de atajo de la ventana: las mismas acciones de los botones de Controles.
fn atajo_de_ventana(a: AccionAtajo, acciones: &mut Vec<Accion>) {
    use gestos::Atajo as A;
    acciones.push(match a {
        AccionAtajo::Atras => Accion::Atajo(A::Atras),
        AccionAtajo::Inicio => Accion::Atajo(A::Inicio),
        AccionAtajo::Recientes => Accion::Atajo(A::Recientes),
        AccionAtajo::VolMenos => Accion::Atajo(A::VolBajar),
        AccionAtajo::VolMas => Accion::Atajo(A::VolSubir),
        AccionAtajo::Rotar => Accion::Atajo(A::Rotar),
        AccionAtajo::Captura => Accion::Captura,
        AccionAtajo::Configuracion => Accion::Configuracion,
        AccionAtajo::ZoomMas => Accion::ZoomMas,
        AccionAtajo::ZoomMenos => Accion::ZoomMenos,
        AccionAtajo::ZoomAjustar => Accion::ZoomAjustar,
    });
}

/// Fija el modo de zoom y redimensiona la ventana a esa escala (hasta el 95 % del area util).
fn fijar_zoom(sdl: &Sdl, win: P, frame: &Shared, zoom: &mut ModoZoom, modo: ModoZoom, area: &Rect) {
    let (vista, dis) = ventana_vista(sdl, win, frame);
    let esc = match modo {
        ModoZoom::Fijo(p) => p as f32 / 100.0,
        ModoZoom::Ajustar => dis.escala,
    };
    *zoom = modo;
    if vista.0 > 0 {
        let ((nw, nh), _) = vista::ventana_para(vista, Some(esc), (area.w, area.h));
        sdl.fijar_tam_dp(win, nw, nh);
    }
}

/// Tamano minimo de la ventana: VENTANA_MIN en dp de la interfaz (crece con el tamano del texto, para que la configuracion
/// quepa), en puntos.
/// Sin pasar del 90 % del area util de la pantalla del equipo (`area`, en dp de la ventana; 0 si se desconoce).
fn fijar_minimo(sdl: &Sdl, win: P, area: &Rect) {
    let u = formas::tamano::factor();
    let tope = |m: i32, a: i32| if a > 0 { (m as f32 * u).min((a as f32 * 0.9).max(m as f32)) } else { m as f32 * u };
    let (w, h) = (tope(VENTANA_MIN.0, area.w), tope(VENTANA_MIN.1, area.h));
    unsafe { (sdl.set_min_size)(win, (w * ui_k()).round() as i32, (h * ui_k()).round() as i32) };
}

/// Tamano de la ventana en dp de la interfaz (los de la ventana divididos por el tamano del texto, formas::tamano): lo que
/// miden la configuracion y el aviso de fin.
fn ventana_ui(sdl: &Sdl, win: P) -> (f32, f32) {
    let (mut w, mut h) = (0i32, 0i32);
    sdl.tam_dp(win, &mut w, &mut h);
    let u = formas::tamano::factor();
    (w as f32 / u, h as f32 / u)
}

/// Punto de la ventana (dp) -> dp de la interfaz.
fn punto_ui(x: f32, y: f32) -> (f32, f32) {
    let u = formas::tamano::factor();
    (x / u, y / u)
}

/// Rectangulo de la ventana (dp) -> dp de la interfaz.
fn a_ui(r: vista::R) -> vista::R {
    let u = formas::tamano::factor();
    vista::R::new(r.x / u, r.y / u, r.w / u, r.h / u)
}

/// Rectangulo en dp de la interfaz -> dp de la ventana.
fn de_ui(r: vista::R) -> vista::R {
    let u = formas::tamano::factor();
    vista::R::new(r.x * u, r.y * u, r.w * u, r.h * u)
}

/// Lo que la pantalla de configuracion muestra y que no es configuracion.
fn datos_ajustes(cfg: &Config, state_dir: &std::path::Path, tipo: &Tipografia, servicios: &Servicios, frame: &Shared, zoom: ModoZoom) -> Datos {
    let (rot, w, h, escala_vista) = {
        let f = frame.lock().unwrap();
        (f.rot, f.w, f.h, f.dev.2)
    };
    let info = servicios.info(Info::default());
    let (doctor, doctor_en_curso) = servicios.doctor();
    let (root_estado, root_consultando) = servicios.root_estado();
    let (almacen, almacen_consultando) = servicios.almacen();
    let (puente_estado, puente_consultando) = servicios.puente_estado();
    let ruta = |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).display().to_string();
    let _ = cfg;
    Datos {
        version: crate::version(),
        estado_dir: ruta(state_dir),
        config_ruta: Config::ruta(state_dir).display().to_string(),
        fuente: tipo.nombre(),
        escala: tipo.escala(),
        resolucion: (w, h),
        orient: pantalla::leer_orientacion(state_dir),
        rot,
        maquina: info.maquina.clone(),
        doctor,
        doctor_en_curso,
        aplicando: servicios.aplicando_resolucion(),
        root_estado,
        root_consultando,
        operacion: servicios.operacion(),
        cancelable: servicios.operacion_cancelable(),
        virtiofsd: crate::compartir::buscar_virtiofsd().is_some(),
        almacen,
        almacen_consultando,
        // la ventana vive mientras vive la maquina de este estado, que es la que usa el disco
        en_marcha: crate::vm::State { dir: state_dir.to_path_buf() }.running(),
        puente_estado,
        puente_consultando,
        reinicio_completo: servicios.reinicio_completo_ofrecido(),
        reiniciando: servicios.reiniciando(),
        apagando: servicios.apagando(),
        zoom_pct: escala_vista * 100.0,
        zoom_modo: zoom,
        mandos: info.mandos,
        ilegibles: info.ilegibles,
        qmp_ok: info.qmp_ok,
        encendida: info.encendida,
        flatpak: vista::flatpak_id(),
    }
}

/// Vista actual del dispositivo (con la rotacion) y reparto de la ventana.
fn ventana_vista(sdl: &Sdl, win: P, frame: &Shared) -> ((u32, u32), vista::Diseno) {
    let (rot, w, h) = {
        let f = frame.lock().unwrap();
        (f.rot, f.w, f.h)
    };
    let vista = pantalla::vista(rot, (w, h));
    let (mut cw, mut ch) = (0i32, 0i32);
    sdl.tam_dp(win, &mut cw, &mut ch);
    (vista, vista::disenar((cw as f32, ch as f32), vista))
}

/// Pide al dispositivo de pantalla un tamano de panel nuevo (org.qemu.Display1.Console.SetUIInfo). Lo atiende
/// virtio-gpu como si una ventana cambiara de tamano: avisa al invitado, que cambia el modo de la pantalla.
fn set_ui_info(c: &mut Conn, r: &Resolucion) {
    let (wmm, hmm) = r.mm(280);
    let mut m = Msg::call(CONSOLE, "org.qemu.Display1.Console", "SetUIInfo");
    m.set_body(&[Arg::U16(wmm), Arg::U16(hmm), Arg::I32(0), Arg::I32(0), Arg::U32(r.w), Arg::U32(r.h)]);
    // con respuesta: si QEMU la rechaza, el hilo lector deja el error en el registro de la ventana
    let _ = c.send(&m);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatos() {
        assert_eq!(sdl_format(0x20020888), Some(0x16161804));
        assert_eq!(sdl_format(0x20098888), Some(0x16261804)); // RGBA de Android -> RGBX
        assert_eq!(sdl_format(0x20038888), Some(0x16561804));
        assert_eq!(sdl_format(0x1234), None);
    }

    #[test]
    fn teclas() {
        assert_eq!(qnum(4), Some(0x1e)); // a
        assert_eq!(qnum(29), Some(0x2c)); // z
        assert_eq!(qnum(30), Some(0x02)); // 1
        assert_eq!(qnum(39), Some(0x0b)); // 0
        assert_eq!(qnum(40), Some(0x1c)); // Intro
        assert_eq!(qnum(67), Some(0x44)); // F10
        assert_eq!(qnum(82), Some(0xc8)); // arriba
        assert_eq!(qnum(230), Some(0xb8)); // AltGr
        assert_eq!(qnum(0), None);
    }

    /// La imagen de aviso tal como la arma QEMU (fondo negro, texto en la franja central) se reconoce; un cuadro
    /// real o un aviso con otro formato, no.
    #[test]
    fn aviso_de_qemu() {
        let (w, h) = (720u32, 1348u32);
        let mut px = vec![0u8; (w * h * 4) as usize];
        let band = ((h as usize / 16 - 1) / 2) * 16;
        for y in band..band + 16 {
            for x in 200..220 {
                px[(y * w as usize + x) * 4..][..4].copy_from_slice(&[0xaa, 0xaa, 0xaa, 0xff]);
            }
        }
        assert!(is_placeholder(w, h, w * 4, 0x20020888, &px));
        assert!(!is_placeholder(w, h, w * 4, 0x20028888, &px));
        let mut real = px.clone();
        real[4] = 1; // un pixel fuera de la franja
        assert!(!is_placeholder(w, h, w * 4, 0x20020888, &real));
        assert!(!is_placeholder(w, h, w * 4, 0x20020888, &vec![0u8; (w * h * 4) as usize])); // negro total
    }

    /// Update parcial sobre un Scanout previo.
    #[test]
    fn scanout_y_update() {
        let fr: Shared = Arc::new(Mutex::new(Frame::default()));
        let mut s = Msg::call("/org/qemu/Display1/Listener", "org.qemu.Display1.Listener", "Scanout");
        s.body = {
            let mut b = Vec::new();
            for v in [2u32, 2, 8, 0x20020888, 16] {
                b.extend(v.to_le_bytes());
            }
            b.extend([0u8; 16]);
            b
        };
        assert!(handle(&mut s, &fr).unwrap());
        let mut u = Msg::call("/org/qemu/Display1/Listener", "org.qemu.Display1.Listener", "Update");
        u.body = {
            let mut b = Vec::new();
            for v in [1i32, 1, 1, 1] {
                b.extend(v.to_le_bytes());
            }
            for v in [4u32, 0x20020888, 4] {
                b.extend(v.to_le_bytes());
            }
            b.extend([9u8, 9, 9, 9]);
            b
        };
        assert!(handle(&mut u, &fr).unwrap());
        let f = fr.lock().unwrap();
        assert_eq!(&f.buf[f.off..], &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9, 9, 9, 9]);
        assert_eq!(f.gen, 2);
        drop(f);
        let mut x = Msg::call("/org/qemu/Display1/Listener", "org.qemu.Display1.Listener", "ScanoutDMABUF");
        assert!(handle(&mut x, &fr).is_err());
    }

    /// Un Scanout cuyo stride no llega al ancho de una fila (o sin datos para todas las filas) se descarta sin tocar el
    /// cuadro anterior: leerlo se saldria del bufer.
    #[test]
    fn scanout_mal_formado_se_descarta() {
        assert_eq!(scanout_invalido(2, 2, 8, 0x20020888, 16), None);
        assert!(scanout_invalido(2, 2, 8, 0x20020888, 15).unwrap().contains("incompletos"));
        // 4 pixeles de 4 bytes por fila = 16, pero el stride dice 8
        assert!(scanout_invalido(4, 2, 8, 0x20020888, 16).unwrap().contains("stride 8"));
        // con 16 bits por pixel (r5g6b5) ese stride si alcanza
        assert_eq!(scanout_invalido(4, 2, 8, 0x10020565, 16), None);
        let fr: Shared = Arc::new(Mutex::new(Frame::default()));
        let scanout = |w: u32, stride: u32| {
            let mut s = Msg::call("/org/qemu/Display1/Listener", "org.qemu.Display1.Listener", "Scanout");
            s.body = {
                let mut b = Vec::new();
                for v in [w, 2, stride, 0x20020888, 16] {
                    b.extend(v.to_le_bytes());
                }
                b.extend([7u8; 16]);
                b
            };
            s
        };
        // primero un cuadro bueno de 2x2
        assert!(handle(&mut scanout(2, 8), &fr).unwrap());
        // despues uno de 4x2 con stride 8: error, el 2x2 sigue en pantalla y queda contado
        let e = handle(&mut scanout(4, 8), &fr).unwrap_err();
        assert!(e.starts_with("Scanout: stride 8"), "{}", e);
        let f = fr.lock().unwrap();
        assert_eq!((f.w, f.h, f.stride, f.gen, f.scanouts, f.descartados), (2, 2, 8, 1, 1, 1));
        assert_eq!(f.buf.len() - f.off, 16);
    }

    /// La textura solo se sube cuando llega un cuadro nuevo (Scanout, Update): un redibujo de la interfaz no la vuelve a
    /// subir. El cuadro tomado comparte los pixeles sin copiarlos y no cambia si despues llega un Update (copia al escribir).
    #[test]
    fn la_textura_solo_se_sube_con_un_cuadro_nuevo() {
        let fr: Shared = Arc::new(Mutex::new(Frame::default()));
        // sin cuadro: nada que subir, ni forzando
        assert!(tomar_cuadro(&mut fr.lock().unwrap(), true).is_none());
        let mut s = Msg::call("/org/qemu/Display1/Listener", "org.qemu.Display1.Listener", "Scanout");
        s.body = {
            let mut b = Vec::new();
            for v in [2u32, 2, 8, 0x20020888, 16] {
                b.extend(v.to_le_bytes());
            }
            b.extend([1u8; 16]);
            b
        };
        assert!(handle(&mut s, &fr).unwrap());
        let c = tomar_cuadro(&mut fr.lock().unwrap(), false).expect("cuadro nuevo");
        assert_eq!((c.w, c.h, c.stride, c.fmt), (2, 2, 8, 0x20020888));
        assert_eq!(&c.buf[c.off..], &[1u8; 16]);
        // un redibujo sin cuadro nuevo no sube nada; si se perdio la textura, si
        assert!(tomar_cuadro(&mut fr.lock().unwrap(), false).is_none());
        assert!(tomar_cuadro(&mut fr.lock().unwrap(), true).is_some());
        // los pixeles se comparten sin copiarlos
        assert!(Arc::ptr_eq(&c.buf, &fr.lock().unwrap().buf));
        // un Update mientras la ventana aun tiene el cuadro: el compartido se copia y cambia, el de la ventana no
        let mut u = Msg::call("/org/qemu/Display1/Listener", "org.qemu.Display1.Listener", "Update");
        u.body = {
            let mut b = Vec::new();
            for v in [0i32, 0, 1, 1] {
                b.extend(v.to_le_bytes());
            }
            for v in [4u32, 0x20020888, 4] {
                b.extend(v.to_le_bytes());
            }
            b.extend([9u8, 9, 9, 9]);
            b
        };
        assert!(handle(&mut u, &fr).unwrap());
        assert_eq!(&c.buf[c.off..], &[1u8; 16]);
        let c2 = tomar_cuadro(&mut fr.lock().unwrap(), false).expect("el Update es un cuadro nuevo");
        assert_eq!(&c2.buf[c2.off..c2.off + 8], &[9, 9, 9, 9, 1, 1, 1, 1]);
        assert!(!Arc::ptr_eq(&c.buf, &c2.buf));
        // sin nadie mas con el bufer, el siguiente Update escribe en el mismo (no copia)
        drop((c, c2));
        let antes = Arc::as_ptr(&fr.lock().unwrap().buf);
        assert!(handle(&mut u, &fr).unwrap());
        assert_eq!(Arc::as_ptr(&fr.lock().unwrap().buf), antes);
        // Disable: la bandera baja pero no hay nada que subir (la pantalla queda en negro)
        let mut d = Msg::call("/org/qemu/Display1/Listener", "org.qemu.Display1.Listener", "Disable");
        assert!(handle(&mut d, &fr).unwrap());
        let mut f = fr.lock().unwrap();
        assert!(f.nuevo && tomar_cuadro(&mut f, false).is_none() && !f.nuevo && f.buf.is_empty());
    }

    #[test]
    fn identificador_de_la_aplicacion() {
        let s = |t: &str| Some(t.to_string());
        assert_eq!(app_id(None, None), "weft");
        assert_eq!(app_id(s("io.github.weft"), s("otro")), "io.github.weft");
        assert_eq!(app_id(None, s(" mi.weft ")), "mi.weft");
        assert_eq!(app_id(s(""), s("mi.weft")), "mi.weft");
        assert_eq!(app_id(s("  "), None), "weft");
    }

    /// Minimizada u oculta no se dibuja; vuelve a dibujarse al restaurarla, mostrarla, maximizarla o descubrirla.
    #[test]
    fn visibilidad_de_la_ventana() {
        assert!(!visible_tras(EV_WINDOW_MINIMIZED, true));
        assert!(!visible_tras(EV_WINDOW_HIDDEN, true));
        for k in [EV_WINDOW_RESTORED, EV_WINDOW_SHOWN, EV_WINDOW_MAXIMIZED, EV_WINDOW_EXPOSED] {
            assert!(visible_tras(k, false), "{:#x}", k);
        }
        for k in [EV_WINDOW_RESIZED, EV_USER, EV_KEY_DOWN, EV_WINDOW_FOCUS_LOST] {
            assert!(!visible_tras(k, false) && visible_tras(k, true), "{:#x}", k);
        }
        assert!(permitir_salvapantallas(0) && !permitir_salvapantallas(1) && !permitir_salvapantallas(3));
    }

    /// Las teclas de los atajos de la ventana: primero los de `config` y despues los propios; cada propio dispara su accion.
    #[test]
    fn atajos_de_la_ventana() {
        let (cfg, a) = (Config::nueva(), Ajustes::nuevo());
        assert_eq!(atajo_ventana(&cfg, &a, 58, false, false, false), Some(AtajoVentana::Config(AccionAtajo::Atras)));
        assert_eq!(atajo_ventana(&cfg, &a, 68, false, false, false), Some(AtajoVentana::Extra(AtajoExtra::PantallaCompleta)));
        assert_eq!(atajo_ventana(&cfg, &a, 9, true, true, false), Some(AtajoVentana::Extra(AtajoExtra::PasarTecla)));
        assert_eq!(atajo_ventana(&cfg, &a, 9, true, false, false), None);
        assert_eq!(accion_extra(AtajoExtra::PasarTecla), None);
        assert_eq!(accion_extra(AtajoExtra::PantallaCompleta), Some(Accion::PantallaCompleta));
        assert_eq!(accion_extra(AtajoExtra::Encendido), Some(Accion::Atajo(Atajo::Encendido)));
        assert_eq!(accion_extra(AtajoExtra::Menu), Some(Accion::Atajo(Atajo::Menu)));
        // F10 lleva el teclado a la barra (lo resuelve el bucle)
        assert_eq!(atajo_ventana(&cfg, &a, 67, false, false, false), Some(AtajoVentana::Extra(AtajoExtra::Barra)));
        assert_eq!(accion_extra(AtajoExtra::Barra), None);
        // la pista de la pestana Configuracion desaparece con los atajos desactivados
        assert_eq!(pista_config(&cfg, &a).as_deref(), Some("F9"));
    }

    /// Lo que otro proceso escribe en `config` entre dos sondeos de la ventana se aplica tambien cuando la ventana guarda
    /// enseguida: `guardar` lo mezcla en el archivo y la ventana relee y aplica las diferencias (fijar sin mas la hora nueva
    /// del archivo haria que no lo viera nunca).
    #[test]
    fn al_guardar_la_ventana_aplica_lo_que_otros_escribieron() {
        let d = std::env::temp_dir().join(format!("weft-ventana-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let mut cfg = Config::cargar(&d);
        // `weft config set rueda.paso 20` mientras la ventana tiene su copia
        let mut otro = Config::cargar(&d);
        otro.set("rueda.paso", "20").unwrap();
        otro.guardar(&d).unwrap();
        // la ventana cambia otra clave y guarda antes del siguiente sondeo
        cfg.set("zoom", "150").unwrap();
        cfg.guardar(&d).unwrap();
        let nueva = Config::cargar(&d);
        assert_eq!((nueva.get("rueda.paso").as_str(), nueva.get("zoom").as_str()), ("20", "150"));
        assert_eq!(claves_cambiadas(&cfg, &nueva), vec!["rueda.paso".to_string()]);
        // sin cambios de nadie: nada que aplicar
        assert!(claves_cambiadas(&nueva, &Config::cargar(&d)).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Cuando QEMU cierra la pantalla, la ventana solo avisa si nadie pidio el cierre: ni la propia ventana (apagar o
    /// reiniciar) ni nadie mas segun `events.log` (`weft stop`, `restart`, el propio Android). El aviso lleva las ultimas
    /// lineas de qemu.log y, con el final ya anotado, no se espera.
    #[test]
    fn fin_inesperado_solo_si_nadie_pidio_el_cierre() {
        let dir = std::env::temp_dir().join(format!("weft-ventana-fin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let no = || false;
        // sin events.log ni qemu.log: inesperado, con el registro vacio (la espera se agota); la maquina fallo
        let (d, f) = fin_inesperado(&dir, &no, "QEMU cerro la pantalla", Duration::from_millis(150)).expect("aviso");
        assert_eq!(d.titulo, tx!("fin.maquina_cerro_forma_inesperada"));
        assert!(d.lineas.is_empty() && d.motivo.contains("QEMU cerro la pantalla"), "{:?}", d);
        assert_eq!(f, Some(Fallo { causa: "QEMU cerro la pantalla".into(), qemu_log: String::new(), eventos: String::new() }));
        // la ventana pidio apagar o reiniciar: nada que avisar
        assert!(fin_inesperado(&dir, &|| true, "x", Duration::ZERO).is_none());
        // apagado ordenado anotado por events-serve: nada que avisar (y sin esperar)
        std::fs::write(dir.join("events.log"), "1 SHUTDOWN {\"guest\":false,\"reason\":\"host-qmp-quit\"}\n2 FIN QEMU termino tras el aviso de apagado\n").unwrap();
        let t0 = Instant::now();
        assert!(fin_inesperado(&dir, &no, "x", Duration::from_secs(10)).is_none());
        // apagado desde Android
        std::fs::write(dir.join("events.log"), "1 SHUTDOWN {\"guest\":true,\"reason\":\"guest-shutdown\"}\n").unwrap();
        assert!(fin_inesperado(&dir, &no, "x", Duration::from_secs(10)).is_none());
        assert!(t0.elapsed() < Duration::from_secs(5));
        // muerte de golpe: aviso con las ultimas lineas de qemu.log
        std::fs::write(dir.join("events.log"), "2 FIN QEMU termino de golpe, sin aviso de apagado (senal, aborto o cierre forzado)\n").unwrap();
        std::fs::write(dir.join("qemu.log"), (1..=20).map(|i| format!("l{}\n", i)).collect::<String>()).unwrap();
        let t0 = Instant::now();
        let (d, f) = fin_inesperado(&dir, &no, "QEMU cerro la pantalla", Duration::from_secs(10)).expect("aviso");
        assert!(t0.elapsed() < Duration::from_secs(5), "con el final anotado no se espera");
        assert_eq!(d.lineas.len(), vista::LINEAS_REGISTRO);
        assert_eq!((d.lineas[0].as_str(), d.lineas[14].as_str()), ("l6", "l20"));
        assert!(d.motivo.contains("de golpe"), "{}", d.motivo);
        // lo que va a ultimo-fallo.txt son los registros de este momento (los de esta ejecucion)
        let f = f.expect("fallo");
        assert!(f.qemu_log.starts_with("l1\n") && f.qemu_log.ends_with("l20\n") && f.eventos.contains("FIN QEMU termino de golpe"), "{:?}", f);
        // una senal al proceso: aviso y fallo (el mismo criterio para los dos)
        std::fs::write(dir.join("events.log"), "1 SHUTDOWN {\"guest\":false,\"reason\":\"host-signal\"}\n2 FIN QEMU termino tras el aviso de apagado\n").unwrap();
        let (d, f) = fin_inesperado(&dir, &no, "x", Duration::from_secs(10)).expect("aviso");
        assert!(d.motivo.contains("señal") && f.is_some(), "{:?}", d);
        // `weft kill` (o el SIGKILL de `stop`) lo anota: fue pedido, ni aviso ni fallo
        std::fs::write(dir.join("events.log"), "1 KILL pedido por weft kill\n2 FIN QEMU termino de golpe, sin aviso de apagado (senal, aborto o cierre forzado)\n").unwrap();
        assert!(fin_inesperado(&dir, &no, "x", Duration::from_secs(10)).is_none());
        // QEMU sigue vivo (aqui, este mismo proceso con su tiempo de arranque) y solo se perdio la pantalla: aviso, sin fallo
        std::fs::remove_file(dir.join("events.log")).unwrap();
        let st = crate::vm::State { dir: dir.clone() };
        let yo = std::process::id();
        std::fs::write(st.pidfile(), format!("{}\n", yo)).unwrap();
        st.anotar_inicio().unwrap();
        st.anotar_espacio_de_pids();
        let (d, f) = fin_inesperado(&dir, &no, "conexion perdida", Duration::from_millis(150)).expect("aviso");
        assert_eq!((d.titulo.as_str(), f), ("Se perdió la pantalla de la máquina", None));
        for n in ["pid", "pid-start", "pid-ns"] {
            std::fs::remove_file(dir.join(n)).unwrap();
        }
        // la ventana pidio el cierre mientras se esperaba a events-serve: tampoco se avisa (sin events.log)
        let llamadas = std::cell::Cell::new(0);
        let tarde = || {
            llamadas.set(llamadas.get() + 1);
            llamadas.get() > 1
        };
        assert!(fin_inesperado(&dir, &tarde, "x", Duration::from_millis(150)).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
