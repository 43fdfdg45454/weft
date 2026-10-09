//! `weft launch`: lo que ejecuta el icono del menu (y sirve en consola). Usa la imagen ya instalada de la maquina (o la
//! unica), arma el disco si falta (lo hace `start`) y arranca con la ventana propia, todo con los directorios estandar.
//! Se queda en primer plano mientras la maquina vive y su ventana sigue abierta (dentro de Flatpak, el sandbox termina
//! cuando termina este proceso).
//! Este modulo decide los argumentos de `start` (funcion pura, con pruebas) y deja constancia de lo que `launch` no puede
//! mostrar cuando se abre desde el icono (sin terminal): registro `launch.log` de la maquina y notificacion de escritorio.
//! La orden vive en main.rs.

use crate::config::Config;
use crate::rutas::{nombres, Rutas};
use std::path::{Path, PathBuf};
use crate::textos::{tx, txf};

/// Texto que sustituye al identificador de la aplicacion de Flatpak cuando no se conoce.
const ID_DESCONOCIDO: &str = "IDENTIFICADOR_DE_LA_APLICACION";

/// Mensaje cuando no hay ninguna imagen instalada: como agregar una desde un archivo (weft no descarga nada). `flatpak`:
/// el identificador de la aplicacion si corre dentro de Flatpak (ver `id_flatpak`), para dar las ordenes listas para copiar.
pub fn mensaje_sin_imagen(flatpak: Option<String>) -> String {
    let mut t = String::from(tx!("lanzar.todavia_no_hay_ninguna_imagen_de_android"));
    if let Some(id) = flatpak {
        t.push_str(&txf!("lanzar.dentro_de_flatpak_la_aplicacion_no_tiene", id));
    }
    t.push_str(tx!("lanzar.despues_vuelve_a_abrir_la_aplicacion"));
    t
}

/// Identificador de la aplicacion dentro de Flatpak: la variable FLATPAK_ID (`env`) o la clave `name` de la seccion
/// `[Application]` de /.flatpak-info (`info`). Dentro de Flatpak sin ninguna de las dos, un texto que lo indica; fuera de
/// Flatpak (sin variable ni archivo), None. Pura.
pub fn id_flatpak_de(env: Option<String>, info: Option<String>) -> Option<String> {
    if let Some(id) = env.filter(|v| !v.trim().is_empty()) {
        return Some(id.trim().to_string());
    }
    let info = info?;
    let mut en_app = false;
    for l in info.lines().map(str::trim) {
        if l.starts_with('[') {
            en_app = l == "[Application]";
        } else if let Some(v) = l.strip_prefix("name=").filter(|_| en_app) {
            if !v.trim().is_empty() {
                return Some(v.trim().to_string());
            }
        }
    }
    Some(ID_DESCONOCIDO.to_string())
}

/// `id_flatpak_de` con el entorno y el archivo de este proceso.
pub fn id_flatpak() -> Option<String> {
    id_flatpak_de(std::env::var("FLATPAK_ID").ok(), std::fs::read_to_string("/.flatpak-info").ok())
}

/// Fecha y hora UTC de un instante (segundos desde 1970): `AAAA-MM-DD HH:MM:SS UTC`. Pura.
pub fn fecha_utc(epoch: u64) -> String {
    let (dias, resto) = ((epoch / 86400) as i64, epoch % 86400);
    // fecha civil desde los dias desde 1970-01-01 (algoritmo de Howard Hinnant)
    let z = dias + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC", y, m, d, resto / 3600, resto % 3600 / 60, resto % 60)
}

/// Segundos desde 1970 de ahora.
pub fn ahora() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Entrada del registro de `launch`: la fecha entre corchetes y el texto (que puede tener varias lineas). Pura.
pub fn entrada_de_registro(fecha: &str, texto: &str) -> String {
    format!("[{}] weft launch\n{}\n\n", fecha, texto.trim_end())
}

/// Cuerpo de la notificacion de escritorio: las primeras lineas con texto del mensaje (las que dicen que pasa y que hacer).
/// Pura.
pub fn cuerpo_notificacion(texto: &str, lineas: usize) -> String {
    texto.lines().map(str::trim).filter(|l| !l.is_empty()).take(lineas).collect::<Vec<_>>().join("\n")
}

/// Tamano a partir del cual `launch.log` se rota a `launch.log.1` antes de anadir otra entrada.
const REGISTRO_MAXIMO: u64 = 256 * 1024;

/// Ruta del registro de `launch` de la maquina: `<registros>/machines/<maquina>/launch.log`.
pub fn ruta_registro(r: &Rutas, maquina: &str) -> PathBuf {
    r.registros_maquina(maquina).join(nombres::REGISTRO_LANZAR)
}

/// Anade `texto` con la fecha al registro de `launch` (crea la carpeta; si el registro crecio demasiado, el anterior pasa
/// a `launch.log.1`). Desde el icono no hay terminal: es donde queda lo que `launch` dijo.
pub fn registrar(ruta: &Path, texto: &str) -> Result<(), String> {
    use std::io::Write;
    if let Some(d) = ruta.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {}", d.display(), e))?;
    }
    if std::fs::metadata(ruta).is_ok_and(|m| m.len() > REGISTRO_MAXIMO) {
        let _ = crate::vm::rotar_registro(ruta);
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(ruta).map_err(|e| format!("{}: {}", ruta.display(), e))?;
    f.write_all(entrada_de_registro(&fecha_utc(ahora()), texto).as_bytes()).map_err(|e| format!("{}: {}", ruta.display(), e))
}

/// Audio por defecto del arranque segun los servidores de sonido que haya en `xdg_runtime`.
pub fn audio_por_defecto(existe: &dyn Fn(&str) -> bool) -> &'static str {
    if existe("pipewire-0") {
        "pipewire"
    } else if existe("pulse/native") {
        "pa"
    } else {
        "none"
    }
}

/// Argumentos de `start` para `launch`: los que dio el usuario, mas los valores por defecto de `launch` que el usuario no dio
/// ni fijo en `config` o en el perfil de dispositivo. `dado` son los argumentos que siguen a `launch`; `recursos`, los nucleos
/// y la memoria de `config` y del perfil de dispositivo (`dispositivo::recursos`, None = sin fijar).
pub fn argumentos_de_arranque(dado: &[String], id: &str, cfg: &Config, recursos: (Option<u32>, Option<u32>), audio: &str) -> Vec<String> {
    let mut v: Vec<String> = dado.to_vec();
    let tiene = |v: &[String], f: &str| v.iter().any(|x| x == f);
    let pon = |v: &mut Vec<String>, f: &str, valor: String| {
        if !tiene(v, f) {
            v.push(f.to_string());
            v.push(valor);
        }
    };
    pon(&mut v, "--image", id.to_string());
    pon(&mut v, "--display", "window".into());
    pon(&mut v, "--pointer", "multitouch".into());
    pon(&mut v, "--audio", audio.to_string());
    // memoria y nucleos: lo de `config` o del perfil de dispositivo si esta puesto (start lo toma de ahi); si no, valores
    // comodos para un telefono (start usaria 2048 y 2)
    if recursos.1.is_none() {
        pon(&mut v, "--mem", "4096".into());
    }
    if recursos.0.is_none() {
        pon(&mut v, "--cpus", "4".into());
    }
    if cfg.resolucion().is_none() {
        pon(&mut v, "--resolution", "720x1348".into());
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argumentos_por_defecto_y_el_usuario_manda() {
        let cfg = Config::nueva();
        let v = argumentos_de_arranque(&[], "phone-x86_64-1", &cfg, (None, None), "none");
        let s = v.join(" ");
        for esperado in ["--image phone-x86_64-1", "--display window", "--pointer multitouch", "--audio none", "--mem 4096", "--cpus 4", "--resolution 720x1348"] {
            assert!(s.contains(esperado), "{} no esta en {}", esperado, s);
        }
        // lo que da el usuario no se repite ni se pisa
        let d: Vec<String> = ["--mem", "2048", "--display", "none"].iter().map(|x| x.to_string()).collect();
        let s = argumentos_de_arranque(&d, "x", &cfg, (None, None), "none").join(" ");
        assert!(s.contains("--mem 2048") && !s.contains("--mem 4096") && s.contains("--display none") && !s.contains("--display window"), "{}", s);
        // lo que esta en `config` tampoco se pisa
        let mut c = Config::nueva();
        c.set("maquina.ram", "6144").unwrap();
        c.set("pantalla.resolucion", "800x1400").unwrap();
        let s = argumentos_de_arranque(&[], "x", &c, crate::dispositivo::recursos(&c, None), "none").join(" ");
        assert!(!s.contains("--mem") && !s.contains("--resolution") && s.contains("--cpus 4"), "{}", s);
        // los del perfil de dispositivo tampoco (start los toma de ahi)
        let mut c = Config::nueva();
        c.set("dispositivo.perfil", "generico-a55").unwrap();
        let cat = crate::dispositivo::Catalogo::cargar(None);
        let s = argumentos_de_arranque(&[], "x", &c, crate::dispositivo::recursos(&c, cat.elegido(&c).unwrap()), "none").join(" ");
        assert!(!s.contains("--mem") && !s.contains("--cpus"), "{}", s);
    }

    #[test]
    fn audio_segun_los_servidores() {
        assert_eq!(audio_por_defecto(&|n| n == "pipewire-0"), "pipewire");
        assert_eq!(audio_por_defecto(&|n| n == "pulse/native"), "pa");
        assert_eq!(audio_por_defecto(&|_| false), "none");
    }

    #[test]
    fn el_mensaje_sin_imagen_dice_como_agregarla() {
        let t = mensaje_sin_imagen(None);
        assert!(t.contains("image add") && t.contains("por tu cuenta") && !t.contains("flatpak"));
        // dentro de Flatpak, las ordenes llevan el identificador real de la aplicacion, listas para copiar
        let f = mensaje_sin_imagen(Some("io.github.ejemplo.weft".into()));
        assert!(f.contains("flatpak override --user --filesystem=CARPETA io.github.ejemplo.weft\n"), "{}", f);
        assert!(f.contains("flatpak run --command=weft io.github.ejemplo.weft image add"), "{}", f);
        assert!(!f.contains(ID_DESCONOCIDO));
    }

    #[test]
    fn identificador_de_flatpak() {
        let info = "[Application]\nname=io.github.ejemplo.weft\nruntime=runtime/org.freedesktop.Platform/x86_64/24.08\n\n[Instance]\nname=otro\n";
        // la variable manda; si no, el archivo; fuera de Flatpak, nada
        assert_eq!(id_flatpak_de(Some("org.a.B".into()), Some(info.into())).as_deref(), Some("org.a.B"));
        assert_eq!(id_flatpak_de(None, Some(info.into())).as_deref(), Some("io.github.ejemplo.weft"));
        assert_eq!(id_flatpak_de(Some(String::new()), Some(info.into())).as_deref(), Some("io.github.ejemplo.weft"));
        assert_eq!(id_flatpak_de(None, None), None);
        // dentro de Flatpak sin saber el id: el texto que lo indica (el `name` de otra seccion no vale)
        assert_eq!(id_flatpak_de(None, Some("[Instance]\nname=otro\n".into())).as_deref(), Some(ID_DESCONOCIDO));
    }

    #[test]
    fn registro_de_launch() {
        assert_eq!(fecha_utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(fecha_utc(1_791_417_600 + 3600 + 62), "2026-10-08 01:01:02 UTC");
        assert_eq!(fecha_utc(951_782_400), "2000-02-29 00:00:00 UTC");
        let e = entrada_de_registro("2026-10-08 01:01:02 UTC", "linea 1\nlinea 2\n");
        assert_eq!(e, "[2026-10-08 01:01:02 UTC] weft launch\nlinea 1\nlinea 2\n\n");
        let t = mensaje_sin_imagen(None);
        let c = cuerpo_notificacion(&t, 3);
        assert_eq!(c.lines().count(), 3);
        assert!(c.starts_with("Todavia no hay ninguna imagen") && c.contains("weft image add"), "{}", c);
        // en disco: se anade (no se pisa) y crea la carpeta
        let d = std::env::temp_dir().join(format!("weft-launch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let r = Rutas { datos: d.join("d"), config: d.join("c"), cache: d.join("k"), registros: d.join("s"), ejecucion: d.join("r"), estado_propio: false };
        let ruta = ruta_registro(&r, "m1");
        assert_eq!(ruta, d.join("s/machines/m1/launch.log"));
        registrar(&ruta, "primero").unwrap();
        registrar(&ruta, "segundo").unwrap();
        let txt = std::fs::read_to_string(&ruta).unwrap();
        assert!(txt.starts_with('[') && txt.contains("UTC] weft launch\nprimero\n") && txt.contains("\nsegundo\n"), "{}", txt);
        // demasiado grande: pasa a launch.log.1 y empieza otro
        std::fs::write(&ruta, vec![b'x'; REGISTRO_MAXIMO as usize + 1]).unwrap();
        registrar(&ruta, "tercero").unwrap();
        assert!(std::fs::read_to_string(&ruta).unwrap().contains("tercero"));
        assert_eq!(std::fs::metadata(d.join("s/machines/m1/launch.log.1")).unwrap().len(), REGISTRO_MAXIMO + 1);
        let _ = std::fs::remove_dir_all(&d);
    }
}
