//! Directorios de weft: donde viven las imagenes, los discos, la configuracion, la cache y el estado de ejecucion.
//!
//! ESTANDAR: XDG (y por tanto Flatpak, donde las variables XDG apuntan a ~/.var/app/<id>/...):
//!   datos      $XDG_DATA_HOME/weft      images/<id>/ (imagenes instaladas) y machines/<maquina>/disks/<id>.img (discos)
//!   config     $XDG_CONFIG_HOME/weft    machines/<maquina>/config y profiles/*.profile (perfiles de imagen del usuario)
//!   cache      $XDG_CACHE_HOME/weft     downloads/ (zip de imagenes), boot/<id>/ (kernel y discos RAM extraidos), root/ y bridge/
//!   registros  $XDG_STATE_HOME/weft     machines/<maquina>/android-log.txt (lo que Android escribe en la consola de registro)
//!   ejecucion  $XDG_RUNTIME_DIR/weft    <maquina>/ (sockets, pid, registros de QEMU y de la ventana)
//! Sin las variables XDG se usan los valores por defecto de la especificacion (~/.local/share, ~/.config, ~/.cache,
//! ~/.local/state; el estado de ejecucion cae en el directorio temporal).
//!
//! RUTA PROPIA: opciones globales `--root DIR` (todo bajo una carpeta: images/, machines/, profiles/, cache/, state/),
//! `--data-dir`, `--config-dir`, `--cache-dir`, `--logs-dir` y `--state-dir`; cada una tiene su variable de entorno
//! (WEFT_ROOT, WEFT_DATA_DIR, WEFT_CONFIG_DIR, WEFT_CACHE_DIR, WEFT_LOGS_DIR,
//! WEFT_STATE_DIR). `main` convierte las opciones en variables de entorno: asi las ordenes internas que lanza
//! (ventana, sensores...) y `restart` heredan lo mismo. Ademas, las claves `dir.datos` y `dir.cache` de `config`
//! cambian el directorio de datos y de cache de esa maquina.
//!
//! MODO DE ESTADO PROPIO: con `--state-dir` a una carpeta PROPIA (distinta del estado de ejecucion estandar que resolveria el
//! proceso sin esa opcion) y sin `--root` ni `--config-dir`, la configuracion y la cache de la maquina siguen dentro de
//! `<state-dir>/<maquina>/`: lo usan las pruebas y el CI. Si `--state-dir` apunta
//! justo al estado de ejecucion estandar no es el modo de estado propio: es lo que reciben las ordenes internas que lanzan `start` y
//! la ventana (`--state-dir <ejecucion> --name N`), que asi resuelven las mismas carpetas que el padre.
//!
//! Los NOMBRES de lo que crea el programa van en ingles (images, machines, disks, boot, downloads, profiles, android-log.txt,
//! disk.img...). Este modulo no toca la maquina: solo calcula rutas (la funcion `resolver` es pura y se prueba con un entorno falso).

use std::path::{Path, PathBuf};
use crate::textos::{clave, txf};

pub const VAR_ROOT: &str = "WEFT_ROOT";
pub const VAR_DATOS: &str = "WEFT_DATA_DIR";
pub const VAR_CONFIG: &str = "WEFT_CONFIG_DIR";
pub const VAR_CACHE: &str = "WEFT_CACHE_DIR";
pub const VAR_REGISTROS: &str = "WEFT_LOGS_DIR";
pub const VAR_ESTADO: &str = "WEFT_STATE_DIR";

/// Opciones globales de linea de ordenes y la variable de entorno en que se convierten.
pub const OPCIONES: &[(&str, &str)] =
    &[("--root", VAR_ROOT), ("--data-dir", VAR_DATOS), ("--config-dir", VAR_CONFIG), ("--cache-dir", VAR_CACHE), ("--logs-dir", VAR_REGISTROS), ("--state-dir", VAR_ESTADO)];

/// Nombres de archivos y carpetas (ingles).
pub mod nombres {
    pub const IMAGENES: &str = "images";
    pub const MAQUINAS: &str = "machines";
    pub const DISCOS: &str = "disks";
    /// bases de solo lectura de los discos de copia en escritura (ver cow.rs)
    pub const BASES: &str = "bases";
    pub const PERFILES: &str = "profiles";
    /// perfiles de dispositivo del usuario (ver dispositivo.rs)
    pub const DISPOSITIVOS: &str = "devices";
    pub const DESCARGAS: &str = "downloads";
    pub const ARRANQUE_CACHE: &str = "boot";
    pub const CONFIG: &str = "config";
    pub const REGISTRO_ANDROID: &str = "android-log.txt";
    pub const INFO_IMAGEN: &str = "image.info";
    pub const MARCA_BUILD: &str = ".weft-build";
    pub const EXT_PERFIL: &str = "profile";
    /// registro de `weft launch` (lo que no se ve al abrir desde el icono, sin terminal)
    pub const REGISTRO_LANZAR: &str = "launch.log";
    /// causa y ultimas lineas de QEMU cuando la ventana termina porque la maquina se cayo
    pub const ULTIMO_FALLO: &str = "ultimo-fallo.txt";
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rutas {
    pub datos: PathBuf,
    pub config: PathBuf,
    pub cache: PathBuf,
    pub registros: PathBuf,
    /// carpeta que contiene un directorio por maquina con sus sockets y registros de ejecucion
    pub ejecucion: PathBuf,
    /// modo de estado propio: configuracion y cache de la maquina dentro de `<ejecucion>/<maquina>/`
    pub estado_propio: bool,
}

extern "C" {
    fn getuid() -> u32;
}

/// ¿Corre dentro de un sandbox de Flatpak? (`/.flatpak-info` o la variable FLATPAK_ID)
pub fn en_flatpak() -> bool {
    std::env::var("FLATPAK_ID").is_ok_and(|v| !v.is_empty()) || Path::new("/.flatpak-info").exists()
}

fn algo(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.is_empty())
}

impl Rutas {
    /// Resuelve las rutas con el entorno dado (`env`). Err si no hay HOME ni variables XDG que sirvan.
    pub fn resolver(env: &dyn Fn(&str) -> Option<String>, uid: u32) -> Result<Rutas, String> {
        let e = |k: &str| algo(env(k));
        let raiz = e(VAR_ROOT).map(PathBuf::from);
        let home = e("HOME").map(PathBuf::from);
        // directorio XDG: variable (ruta absoluta; la especificacion ignora las relativas) o su valor por defecto bajo HOME
        let xdg = |var: &str, rel: &str| -> Result<PathBuf, String> {
            if let Some(v) = e(var).filter(|v| v.starts_with('/')) {
                return Ok(PathBuf::from(v).join("weft"));
            }
            match &home {
                Some(h) => Ok(h.join(rel).join("weft")),
                None => Err(txf!("rutas.no_hay_ni_home_indica_una_carpeta_con", var)),
            }
        };
        let propio = |var: &str| e(var).map(PathBuf::from);
        let datos = match propio(VAR_DATOS).or_else(|| raiz.clone()) {
            Some(d) => d,
            None => xdg("XDG_DATA_HOME", ".local/share")?,
        };
        let config = match propio(VAR_CONFIG).or_else(|| raiz.clone()) {
            Some(d) => d,
            None => xdg("XDG_CONFIG_HOME", ".config")?,
        };
        let cache = match propio(VAR_CACHE).or_else(|| raiz.as_ref().map(|r| r.join("cache"))) {
            Some(d) => d,
            None => xdg("XDG_CACHE_HOME", ".cache")?,
        };
        let registros = match propio(VAR_REGISTROS).or_else(|| raiz.clone()) {
            Some(d) => d,
            None => xdg("XDG_STATE_HOME", ".local/state")?,
        };
        let estado_propio = propio(VAR_ESTADO);
        let estandar = ejecucion_estandar(&e, uid);
        let ejecucion = match estado_propio.clone().or_else(|| raiz.as_ref().map(|r| r.join("state"))) {
            Some(d) => d,
            None => estandar.clone(),
        };
        // modo de estado propio: solo --state-dir, sin raiz ni carpeta de configuracion, y a una carpeta propia (si es la de
        // ejecucion estandar es una orden interna con las carpetas del padre, no el modo de estado propio)
        let estado_propio = es_modo_estado_propio(estado_propio.as_deref(), &estandar, raiz.is_some() || propio(VAR_CONFIG).is_some());
        Ok(Rutas { datos, config, cache, registros, ejecucion, estado_propio })
    }

    /// Variables de entorno con las que otro proceso resuelve estas mismas carpetas aunque solo reciba `--state-dir`:
    /// las hereda cada orden interna que lanza `start` (y, de esta, las de la ventana). En el modo de estado propio no hay nada que
    /// fijar: la carpeta de estado ya lo determina todo, y fijar WEFT_CONFIG_DIR sacaria al hijo de ese modo.
    pub fn variables(&self) -> Vec<(&'static str, PathBuf)> {
        if self.estado_propio {
            return Vec::new();
        }
        vec![(VAR_DATOS, self.datos.clone()), (VAR_CONFIG, self.config.clone()), (VAR_CACHE, self.cache.clone()), (VAR_REGISTROS, self.registros.clone())]
    }

    /// Cambia datos y cache segun las claves `dir.datos` y `dir.cache` de la configuracion (valor `auto` = no cambiar).
    pub fn con_claves(mut self, datos: &str, cache: &str) -> Rutas {
        if datos != "auto" && !datos.is_empty() {
            self.datos = PathBuf::from(datos);
        }
        if cache != "auto" && !cache.is_empty() {
            self.cache = PathBuf::from(cache);
        }
        self
    }

    // --- datos
    pub fn imagenes(&self) -> PathBuf {
        self.datos.join(nombres::IMAGENES)
    }
    pub fn imagen(&self, id: &str) -> PathBuf {
        self.imagenes().join(id)
    }
    pub fn dir_discos(&self, maquina: &str) -> PathBuf {
        self.datos.join(nombres::MAQUINAS).join(maquina).join(nombres::DISCOS)
    }
    /// Disco de una maquina para una imagen (un disco por imagen: cambiar de imagen no borra los datos de la otra).
    pub fn disco(&self, maquina: &str, imagen_id: &str) -> PathBuf {
        self.dir_discos(maquina).join(format!("{}.img", imagen_id))
    }
    /// Carpeta de las bases de una imagen (`<datos>/bases/ID/`).
    pub fn dir_bases(&self, imagen_id: &str) -> PathBuf {
        self.datos.join(nombres::BASES).join(imagen_id)
    }
    /// Base de solo lectura de los discos de copia en escritura de una imagen con un tamano de datos (`24G`, `img`...):
    /// la comparten todas las maquinas (ver cow.rs).
    pub fn base_disco(&self, imagen_id: &str, datos: &str) -> PathBuf {
        self.dir_bases(imagen_id).join(format!("{}.img", datos))
    }

    // --- configuracion
    pub fn config_maquina(&self, maquina: &str) -> PathBuf {
        if self.estado_propio {
            self.ejecucion.join(maquina).join(nombres::CONFIG)
        } else {
            self.config.join(nombres::MAQUINAS).join(maquina).join(nombres::CONFIG)
        }
    }
    pub fn dir_config_maquinas(&self) -> PathBuf {
        self.config.join(nombres::MAQUINAS)
    }
    pub fn perfiles_usuario(&self) -> PathBuf {
        self.config.join(nombres::PERFILES)
    }
    /// Perfiles de dispositivo del usuario (`*.device`, ver dispositivo.rs).
    pub fn dispositivos_usuario(&self) -> PathBuf {
        self.config.join(nombres::DISPOSITIVOS)
    }

    // --- cache
    pub fn descargas(&self) -> PathBuf {
        self.cache.join(nombres::DESCARGAS)
    }
    pub fn cache_arranque(&self, id: &str) -> PathBuf {
        self.cache.join(nombres::ARRANQUE_CACHE).join(id)
    }
    /// Cache de un componente (`root`, `bridge`) de la maquina de `estado` (`<ejecucion>/<maquina>`). En el modo de estado propio
    /// sigue dentro del estado de la maquina con su propio nombre (`nombre`: ksu, puente).
    pub fn cache_de(&self, estado: &Path, sub: &str, nombre: &str) -> PathBuf {
        if self.estado_propio {
            estado.join(nombre)
        } else {
            self.cache.join(sub)
        }
    }

    // --- registros
    /// Carpeta de registros de una maquina (`<registros>/machines/<maquina>`): registro de Android, de `launch` y del
    /// ultimo fallo.
    pub fn registros_maquina(&self, maquina: &str) -> PathBuf {
        self.registros.join(nombres::MAQUINAS).join(maquina)
    }
    pub fn registro_android(&self, maquina: &str) -> PathBuf {
        self.registros_maquina(maquina).join(nombres::REGISTRO_ANDROID)
    }
}

/// Estado de ejecucion estandar (sin `--root` ni `--state-dir`): `$XDG_RUNTIME_DIR/weft` (dentro de Flatpak,
/// `$XDG_RUNTIME_DIR/app/<id>/weft`) o, sin la variable, el directorio temporal con el uid.
fn ejecucion_estandar(e: &dyn Fn(&str) -> Option<String>, uid: u32) -> PathBuf {
    match e("XDG_RUNTIME_DIR").filter(|v| v.starts_with('/')) {
        // dentro de Flatpak, XDG_RUNTIME_DIR es privado de cada `flatpak run` salvo `app/<id>`, que es lo unico que
        // comparten las instancias de la misma aplicacion (comprobado): el estado de ejecucion va ahi
        Some(d) => match e("FLATPAK_ID") {
            Some(id) => PathBuf::from(d).join("app").join(id).join("weft"),
            None => PathBuf::from(d).join("weft"),
        },
        None => std::env::temp_dir().join(format!("weft-{}", uid)),
    }
}

/// ¿Modo estado_propio? Solo con una carpeta de estado propia (`estado`) que no sea la de ejecucion estandar (`estandar`) y sin
/// `--root` ni `--config-dir` (`otra_raiz`). Se comparan normalizadas: `.`, `..` y barras repetidas no cuentan.
fn es_modo_estado_propio(estado: Option<&Path>, estandar: &Path, otra_raiz: bool) -> bool {
    match estado {
        Some(e) if !otra_raiz => normalizar(e) != normalizar(estandar),
        _ => false,
    }
}

/// Las rutas del proceso. Sin HOME ni variables XDG (entorno vacio) los directorios de datos, configuracion, cache y
/// registros caen en subcarpetas de la carpeta actual (nunca en /tmp) y el estado de ejecucion sigue su regla de siempre;
/// se avisa por consola una sola vez (`paths` y `doctor` tambien lo muestran: `carpetas_relativas`).
pub fn actual() -> Rutas {
    let env = |k: &str| std::env::var(k).ok();
    match Rutas::resolver(&env, unsafe { getuid() }) {
        Ok(r) => r,
        Err(e) => {
            AVISADO_CARPETAS_RELATIVAS.call_once(|| eprintln!("{}", txf!("cli.aviso", aviso_carpetas_relativas(&e))));
            let r = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let e = |k: &str| algo(env(k));
            let estado = e(VAR_ESTADO).map(PathBuf::from);
            let estandar = ejecucion_estandar(&e, unsafe { getuid() });
            let estado_propio = es_modo_estado_propio(estado.as_deref(), &estandar, false);
            let ejecucion = estado.unwrap_or(estandar);
            Rutas { datos: r.join("weft-data"), config: r.join("weft-config"), cache: r.join("weft-cache"), registros: r.join("weft-logs"), ejecucion, estado_propio }
        }
    }
}

/// Para avisar una sola vez por proceso de que se usan carpetas relativas (`actual` se llama muchas veces).
static AVISADO_CARPETAS_RELATIVAS: std::sync::Once = std::sync::Once::new();

/// Texto del aviso cuando no se pueden resolver las carpetas estandar (`error`: el de `Rutas::resolver`) y se usan las
/// relativas al directorio actual. Quien lo imprime le pone el prefijo "aviso: ". Pura.
pub fn aviso_carpetas_relativas(error: &str) -> String {
    txf!("rutas.mientras_tanto_se_usan_carpetas", error)
}

/// El aviso de `aviso_carpetas_relativas` si este proceso esta en ese caso (sin HOME ni variables XDG que sirvan); None si
/// las carpetas estandar se resuelven.
pub fn carpetas_relativas() -> Option<String> {
    Rutas::resolver(&|k: &str| std::env::var(k).ok(), unsafe { getuid() }).err().map(|e| aviso_carpetas_relativas(&e))
}

const ELIGE_OTRO: &str = clave!("rutas.elige_otro_con_state_dir_o_define_xdg");

/// Deja listo un directorio de estado de ejecucion de weft (`dir`: la carpeta de ejecucion estandar o la de una maquina,
/// con sus sockets y pids) para el usuario `uid` antes de escribir nada en el: lo crea solo para el (0700) o, si ya existe,
/// comprueba que sea suyo, que no sea un enlace simbolico y que nadie mas pueda entrar (si es suyo pero deja entrar a
/// otros, se cierra). Importa sobre todo cuando cae en el directorio temporal compartido (sin XDG_RUNTIME_DIR): ahi
/// cualquier otro usuario pudo haber creado antes `weft-<uid>`, o un enlace, y quedarse con los sockets. Ver
/// `asegurar_estado`.
fn asegurar_ejecucion_de(dir: &Path, uid: u32) -> Result<(), String> {
    use std::io::ErrorKind;
    use std::os::unix::fs::DirBuilderExt;
    let con_ruta = |e: std::io::Error| txf!("rutas.directorio_de_ejecucion", dir.display(), e);
    match std::fs::symlink_metadata(dir) {
        Ok(m) => comprobar_ejecucion(dir, &m, uid),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            // los padres (p. ej. la raiz de --root) con los permisos normales; la carpeta de ejecucion, solo nuestra
            if let Some(p) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(p).map_err(|e| txf!("rutas.directorio_de_ejecucion", p.display(), e))?;
            }
            match std::fs::DirBuilder::new().mode(0o700).create(dir) {
                Ok(()) => Ok(()),
                // otra orden de weft lo creo en este instante (o alguien se adelanto): vale solo si pasa la comprobacion
                Err(e) if e.kind() == ErrorKind::AlreadyExists => std::fs::symlink_metadata(dir).map_err(con_ruta).and_then(|m| comprobar_ejecucion(dir, &m, uid)),
                Err(e) => Err(con_ruta(e)),
            }
        }
        Err(e) => Err(con_ruta(e)),
    }
}

/// Comprueba un directorio de ejecucion que ya existe (`m`: sus metadatos sin seguir enlaces) y, si deja entrar a otros, lo
/// cierra.
fn comprobar_ejecucion(dir: &Path, m: &std::fs::Metadata, uid: u32) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    comprobar_propia(dir, m, uid)?;
    if m.mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| txf!("rutas.no_se_pudieron_cerrar_a_otros_usuarios", dir.display(), e, crate::textos::texto(ELIGE_OTRO)))?;
    }
    Ok(())
}

/// Un directorio de ejecucion que ya existe tiene que ser un directorio de verdad (no un enlace simbolico) y de `uid`.
fn comprobar_propia(dir: &Path, m: &std::fs::Metadata, uid: u32) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    if m.file_type().is_symlink() {
        return Err(txf!("rutas.el_directorio_de_ejecucion_es_un_enlace", dir.display(), crate::textos::texto(ELIGE_OTRO)));
    }
    if !m.is_dir() {
        return Err(txf!("rutas.el_directorio_de_ejecucion_no_es_un", dir.display(), crate::textos::texto(ELIGE_OTRO)));
    }
    if m.uid() != uid {
        return Err(txf!("rutas.el_directorio_de_ejecucion_pertenece_a", dir.display(), m.uid(), crate::textos::texto(ELIGE_OTRO)));
    }
    Ok(())
}

/// ¿Es `base` la carpeta de ejecucion estandar de este proceso, la que weft elige por su cuenta ($XDG_RUNTIME_DIR/weft o, sin
/// esa variable, la del directorio temporal con el uid), y no una que dio el usuario (--state-dir, --root)? Las ordenes
/// internas la reciben con --state-dir, y tambien cuenta. Se comparan normalizadas.
pub fn es_ejecucion_estandar(base: &Path) -> bool {
    let env = |k: &str| algo(std::env::var(k).ok());
    base.is_absolute() && normalizar(base) == normalizar(&ejecucion_estandar(&env, unsafe { getuid() }))
}

/// La carpeta que contiene a la de una maquina (`dir`), si tiene.
fn base_de(dir: &Path) -> Option<&Path> {
    dir.parent().filter(|p| !p.as_os_str().is_empty())
}

/// Deja lista la carpeta de estado de una maquina (`dir`: `<ejecucion>/<maquina>`) antes de que `start` escriba en ella.
/// La de la maquina es de weft: se crea solo para este usuario o se comprueba y se cierra (`asegurar_ejecucion_de`). La que la
/// contiene se trata igual si es la de ejecucion estandar (puede caer en el directorio temporal compartido); si la eligio
/// el usuario (--state-dir, --root) es suya: se crea si falta, se siguen sus enlaces y no se tocan sus permisos.
pub fn asegurar_estado(dir: &Path) -> Result<(), String> {
    asegurar_estado_de(dir, base_de(dir).is_some_and(es_ejecucion_estandar), unsafe { getuid() })
}

/// `asegurar_estado` para el usuario `uid` (`base_estandar`: la carpeta que contiene a `dir` es la de ejecucion estandar).
fn asegurar_estado_de(dir: &Path, base_estandar: bool, uid: u32) -> Result<(), String> {
    if let Some(base) = base_de(dir) {
        if base_estandar {
            asegurar_ejecucion_de(base, uid)?;
        } else {
            std::fs::create_dir_all(base).map_err(|e| txf!("rutas.carpeta_de_estado", base.display(), e))?;
        }
    }
    asegurar_ejecucion_de(dir, uid)
}

/// Comprueba, sin crear nada ni cambiar permisos, la carpeta de estado de una maquina (`dir`) antes de que una orden lea o
/// escriba en ella (pid, helpers.pid, sockets, ventana): si existe, tiene que ser un directorio de este usuario y no un
/// enlace simbolico; y lo mismo la que la contiene si es la de ejecucion estandar. Lo que todavia no existe no es un
/// error (lo crea `start`, con `asegurar_estado`).
pub fn comprobar_estado(dir: &Path) -> Result<(), String> {
    comprobar_estado_de(dir, base_de(dir).is_some_and(es_ejecucion_estandar), unsafe { getuid() })
}

/// `comprobar_estado` para el usuario `uid`.
fn comprobar_estado_de(dir: &Path, base_estandar: bool, uid: u32) -> Result<(), String> {
    let mirar = |d: &Path| match std::fs::symlink_metadata(d) {
        Ok(m) => comprobar_propia(d, &m, uid),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(txf!("rutas.directorio_de_ejecucion", d.display(), e)),
    };
    if let Some(base) = base_de(dir).filter(|_| base_estandar) {
        mirar(base)?;
    }
    mirar(dir)
}

/// Nombre de la maquina de un directorio de estado (`<ejecucion>/<maquina>`).
pub fn maquina_de(estado: &Path) -> String {
    estado.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "default".into())
}

/// Aplica una opcion global de rutas (`--root X`...): la deja en el entorno del proceso, que heredan los procesos hijos.
pub fn fijar_opcion(opcion: &str, valor: &str) -> Result<bool, String> {
    let Some((_, var)) = OPCIONES.iter().find(|(o, _)| *o == opcion) else { return Ok(false) };
    if valor.is_empty() {
        return Err(txf!("rutas.falta_la_carpeta", opcion));
    }
    // ruta absoluta: los procesos hijos pueden tener otro directorio de trabajo
    let abs = if Path::new(valor).is_absolute() { PathBuf::from(valor) } else { std::env::current_dir().map_err(|e| e.to_string())?.join(valor) };
    let limpia = normalizar(&abs);
    std::env::set_var(var, &limpia);
    Ok(true)
}

/// Quita `.` y `..` de una ruta absoluta sin tocar el disco.
pub fn normalizar(p: &Path) -> PathBuf {
    let mut v: Vec<std::ffi::OsString> = Vec::new();
    for c in p.components() {
        use std::path::Component::*;
        match c {
            CurDir => {}
            ParentDir => {
                v.pop();
            }
            RootDir => v.clear(),
            Normal(s) => v.push(s.to_os_string()),
            Prefix(_) => {}
        }
    }
    let mut r = PathBuf::from("/");
    for s in v {
        r.push(s);
    }
    r
}

/// Ruta escrita por una persona en un campo de la ventana, lista para usar: sin espacios alrededor ni comillas que la
/// envuelvan (las que deja copiar una ruta desde el gestor de archivos), con `~` y `~/...` cambiados por `home` y una URI
/// `file://` (pegar o soltar desde el gestor de archivos) convertida en ruta local. `~usuario` se deja tal cual. Pura.
pub fn ruta_escrita(t: &str, home: Option<&str>) -> String {
    let mut t = t.trim();
    for q in ['\'', '"'] {
        if t.len() >= 2 && t.starts_with(q) && t.ends_with(q) {
            t = &t[1..t.len() - 1];
        }
    }
    if t.starts_with("file://") {
        if let Some(r) = ruta_de_uri(t) {
            return r;
        }
    }
    match home.filter(|h| !h.is_empty()) {
        Some(h) if t == "~" => h.to_string(),
        Some(h) if t.starts_with("~/") => format!("{}/{}", h.trim_end_matches('/'), &t[2..]),
        _ => t.to_string(),
    }
}

/// Ruta local de una URI `file://` (con escapes `%XX`; `file://localhost/...` vale igual). None si no es una URI de un
/// archivo local o un escape esta mal formado. Pura.
pub fn ruta_de_uri(uri: &str) -> Option<String> {
    let resto = uri.trim().strip_prefix("file://")?;
    let resto = resto.strip_prefix("localhost").unwrap_or(resto);
    if !resto.starts_with('/') {
        return None;
    }
    let b = resto.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok().filter(|s| !s.contains('\0'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rutas_escritas_y_uris() {
        let h = Some("/home/ana");
        assert_eq!(ruta_escrita("~", h), "/home/ana");
        assert_eq!(ruta_escrita("  ~/Descargas/imagen.zip ", h), "/home/ana/Descargas/imagen.zip");
        assert_eq!(ruta_escrita("~otra/x", h), "~otra/x");
        assert_eq!(ruta_escrita("~/x", None), "~/x");
        assert_eq!(ruta_escrita("'/tmp/con espacio'", h), "/tmp/con espacio");
        assert_eq!(ruta_escrita("\"~/a b\"", h), "/home/ana/a b");
        assert_eq!(ruta_escrita("file:///home/ana/Mi%20imagen.zip", h), "/home/ana/Mi imagen.zip");
        assert_eq!(ruta_escrita("relativa/x", h), "relativa/x");
        assert_eq!(ruta_de_uri("file://localhost/a/%C3%B1"), Some("/a/ñ".to_string()));
        assert_eq!(ruta_de_uri("file://equipo/a"), None);
        assert_eq!(ruta_de_uri("https://x/a"), None);
        assert_eq!(ruta_de_uri("file:///a%2"), None);
        assert_eq!(ruta_de_uri("file:///a%00b"), None);
    }

    fn entorno(pares: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let v: Vec<(String, String)> = pares.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k: &str| v.iter().find(|(a, _)| a == k).map(|(_, b)| b.clone())
    }

    #[test]
    fn xdg_con_variables() {
        let r = Rutas::resolver(
            &entorno(&[("XDG_DATA_HOME", "/x/d"), ("XDG_CONFIG_HOME", "/x/c"), ("XDG_CACHE_HOME", "/x/k"), ("XDG_STATE_HOME", "/x/s"), ("XDG_RUNTIME_DIR", "/x/r"), ("HOME", "/h")]),
            1000,
        )
        .unwrap();
        assert_eq!(r.datos, PathBuf::from("/x/d/weft"));
        assert_eq!(r.config, PathBuf::from("/x/c/weft"));
        assert_eq!(r.cache, PathBuf::from("/x/k/weft"));
        assert_eq!(r.registros, PathBuf::from("/x/s/weft"));
        assert_eq!(r.ejecucion, PathBuf::from("/x/r/weft"));
        assert!(!r.estado_propio);
        assert_eq!(r.imagen("a"), PathBuf::from("/x/d/weft/images/a"));
        assert_eq!(r.disco("m", "a"), PathBuf::from("/x/d/weft/machines/m/disks/a.img"));
        assert_eq!(r.base_disco("a", "24G"), PathBuf::from("/x/d/weft/bases/a/24G.img"));
        assert_eq!(r.config_maquina("m"), PathBuf::from("/x/c/weft/machines/m/config"));
        assert_eq!(r.perfiles_usuario(), PathBuf::from("/x/c/weft/profiles"));
        assert_eq!(r.dispositivos_usuario(), PathBuf::from("/x/c/weft/devices"));
        assert_eq!(r.registro_android("m"), PathBuf::from("/x/s/weft/machines/m/android-log.txt"));
        // los demas registros de la maquina van en la misma carpeta
        assert_eq!(r.registros_maquina("m").join(nombres::REGISTRO_LANZAR), PathBuf::from("/x/s/weft/machines/m/launch.log"));
        assert_eq!(r.registros_maquina("m").join(nombres::ULTIMO_FALLO), PathBuf::from("/x/s/weft/machines/m/ultimo-fallo.txt"));
        assert_eq!(r.descargas(), PathBuf::from("/x/k/weft/downloads"));
        assert_eq!(r.cache_arranque("a"), PathBuf::from("/x/k/weft/boot/a"));
    }

    #[test]
    fn xdg_por_defecto_bajo_home_y_variables_relativas_ignoradas() {
        let r = Rutas::resolver(&entorno(&[("HOME", "/h"), ("XDG_DATA_HOME", "relativa"), ("XDG_RUNTIME_DIR", "/run/user/1")]), 1000).unwrap();
        assert_eq!(r.datos, PathBuf::from("/h/.local/share/weft"));
        assert_eq!(r.config, PathBuf::from("/h/.config/weft"));
        assert_eq!(r.cache, PathBuf::from("/h/.cache/weft"));
        assert_eq!(r.registros, PathBuf::from("/h/.local/state/weft"));
        assert_eq!(r.ejecucion, PathBuf::from("/run/user/1/weft"));
        // sin HOME ni variables: error claro
        assert!(Rutas::resolver(&entorno(&[]), 1000).unwrap_err().contains("--root"));
        // sin XDG_RUNTIME_DIR: directorio temporal con el uid
        let t = Rutas::resolver(&entorno(&[("HOME", "/h")]), 1234).unwrap();
        assert!(t.ejecucion.to_string_lossy().ends_with("weft-1234"));
    }

    #[test]
    fn dentro_de_flatpak_las_variables_xdg_apuntan_al_directorio_de_la_aplicacion() {
        let h = "/home/u/.var/app/org.ejemplo.App";
        let r = Rutas::resolver(
            &entorno(&[("XDG_DATA_HOME", &format!("{}/data", h)), ("XDG_CONFIG_HOME", &format!("{}/config", h)), ("XDG_CACHE_HOME", &format!("{}/cache", h)), ("XDG_STATE_HOME", &format!("{}/.local/state", h)), ("XDG_RUNTIME_DIR", "/run/user/1000")]),
            1000,
        )
        .unwrap();
        assert!(r.datos.starts_with(h) && r.config.starts_with(h) && r.cache.starts_with(h) && r.registros.starts_with(h));
    }

    #[test]
    fn dentro_de_flatpak_el_estado_de_ejecucion_va_en_app_id() {
        let r = Rutas::resolver(&entorno(&[("HOME", "/h"), ("XDG_RUNTIME_DIR", "/run/user/1000"), ("FLATPAK_ID", "org.ejemplo.App")]), 1000).unwrap();
        assert_eq!(r.ejecucion, PathBuf::from("/run/user/1000/app/org.ejemplo.App/weft"));
        // una ruta propia sigue mandando
        let p = Rutas::resolver(&entorno(&[("HOME", "/h"), ("XDG_RUNTIME_DIR", "/run/user/1000"), ("FLATPAK_ID", "org.ejemplo.App"), (VAR_ESTADO, "/t/e")]), 1000).unwrap();
        assert_eq!(p.ejecucion, PathBuf::from("/t/e"));
    }

    #[test]
    fn raiz_propia_lo_reune_todo_y_state_dir_solo_es_modo_estado_propio() {
        let r = Rutas::resolver(&entorno(&[(VAR_ROOT, "/t/r"), ("HOME", "/h")]), 1).unwrap();
        assert_eq!(r.datos, PathBuf::from("/t/r"));
        assert_eq!(r.config, PathBuf::from("/t/r"));
        assert_eq!(r.cache, PathBuf::from("/t/r/cache"));
        assert_eq!(r.registros, PathBuf::from("/t/r"));
        assert_eq!(r.ejecucion, PathBuf::from("/t/r/state"));
        assert!(!r.estado_propio);
        assert_eq!(r.imagen("x"), PathBuf::from("/t/r/images/x"));
        assert_eq!(r.config_maquina("m"), PathBuf::from("/t/r/machines/m/config"));
        // una carpeta concreta manda sobre la raiz
        let c = Rutas::resolver(&entorno(&[(VAR_ROOT, "/t/r"), (VAR_CACHE, "/t/c")]), 1).unwrap();
        assert_eq!(c.cache, PathBuf::from("/t/c"));
        // solo --state-dir: configuracion y cache dentro del estado, como antes
        let a = Rutas::resolver(&entorno(&[(VAR_ESTADO, "/t/estado"), ("HOME", "/h")]), 1).unwrap();
        assert!(a.estado_propio);
        assert_eq!(a.ejecucion, PathBuf::from("/t/estado"));
        assert_eq!(a.config_maquina("prueba"), PathBuf::from("/t/estado/prueba/config"));
        assert_eq!(a.cache_de(Path::new("/t/estado/prueba"), "root", "ksu"), PathBuf::from("/t/estado/prueba/ksu"));
        assert_eq!(r.cache_de(Path::new("/t/r/state/prueba"), "root", "ksu"), PathBuf::from("/t/r/cache/root"));
        // state-dir junto con root: no es el modo de estado propio
        assert!(!Rutas::resolver(&entorno(&[(VAR_ESTADO, "/t/e"), (VAR_ROOT, "/t/r")]), 1).unwrap().estado_propio);
        // state-dir junto con config-dir: tampoco
        let c = Rutas::resolver(&entorno(&[(VAR_ESTADO, "/t/e"), (VAR_CONFIG, "/t/c"), ("HOME", "/h")]), 1).unwrap();
        assert!(!c.estado_propio);
        assert_eq!(c.config_maquina("m"), PathBuf::from("/t/c/machines/m/config"));
    }

    #[test]
    fn state_dir_igual_al_de_ejecucion_estandar_no_es_modo_estado_propio() {
        // lo que reciben las ordenes internas de `start` y de la ventana (`--state-dir <ejecucion> --name N`) en el modo
        // estandar: deben resolver lo mismo que el padre (configuracion en config/machines, cache en la cache)
        let base = &[("HOME", "/h"), ("XDG_RUNTIME_DIR", "/run/user/1"), ("XDG_CONFIG_HOME", "/x/c")];
        let padre = Rutas::resolver(&entorno(base), 1).unwrap();
        assert!(!padre.estado_propio);
        assert_eq!(padre.ejecucion, PathBuf::from("/run/user/1/weft"));
        let mut env = base.to_vec();
        env.push((VAR_ESTADO, "/run/user/1/weft"));
        let hijo = Rutas::resolver(&entorno(&env), 1).unwrap();
        assert!(!hijo.estado_propio);
        assert_eq!(hijo, padre);
        assert_eq!(hijo.config_maquina("default"), PathBuf::from("/x/c/weft/machines/default/config"));
        assert_eq!(hijo.cache_de(Path::new("/run/user/1/weft/default"), "root", "ksu"), PathBuf::from("/h/.cache/weft/root"));
        // la comparacion no se fija en `.`, `..` ni barras repetidas
        let mut env = base.to_vec();
        env.push((VAR_ESTADO, "/run/user/1//otro/../weft/."));
        assert!(!Rutas::resolver(&entorno(&env), 1).unwrap().estado_propio);
        // otra carpeta de estado (las pruebas y el CI): el modo de estado propio de siempre
        let mut env = base.to_vec();
        env.push((VAR_ESTADO, "/run/user/1/weft-otro"));
        let a = Rutas::resolver(&entorno(&env), 1).unwrap();
        assert!(a.estado_propio);
        assert_eq!(a.config_maquina("e2e"), PathBuf::from("/run/user/1/weft-otro/e2e/config"));
        // dentro de Flatpak el estado estandar es app/<id>/weft, y con el tampoco es el modo de estado propio
        let fp = &[("HOME", "/h"), ("XDG_RUNTIME_DIR", "/run/user/1"), ("FLATPAK_ID", "org.ejemplo.App"), (VAR_ESTADO, "/run/user/1/app/org.ejemplo.App/weft")];
        assert!(!Rutas::resolver(&entorno(fp), 1).unwrap().estado_propio);
        // sin XDG_RUNTIME_DIR el estandar es el directorio temporal con el uid
        let tmp = std::env::temp_dir().join("weft-7");
        let t = Rutas::resolver(&entorno(&[("HOME", "/h"), (VAR_ESTADO, &tmp.to_string_lossy())]), 7).unwrap();
        assert!(!t.estado_propio);
        assert!(Rutas::resolver(&entorno(&[("HOME", "/h"), (VAR_ESTADO, &tmp.to_string_lossy())]), 8).unwrap().estado_propio);
    }

    #[test]
    fn variables_para_las_ordenes_internas() {
        // modo estandar: las cuatro carpetas resueltas, para que el hijo no dependa de su propio entorno
        let r = Rutas::resolver(&entorno(&[("HOME", "/h"), ("XDG_RUNTIME_DIR", "/run/user/1")]), 1).unwrap();
        let v = r.variables();
        assert_eq!(v, vec![(VAR_DATOS, PathBuf::from("/h/.local/share/weft")), (VAR_CONFIG, PathBuf::from("/h/.config/weft")), (VAR_CACHE, PathBuf::from("/h/.cache/weft")), (VAR_REGISTROS, PathBuf::from("/h/.local/state/weft"))]);
        // con ellas en el entorno y --state-dir, el hijo resuelve exactamente lo mismo que el padre
        let mut env: Vec<(String, String)> = v.iter().map(|(k, p)| (k.to_string(), p.to_string_lossy().into_owned())).collect();
        env.push((VAR_ESTADO.to_string(), "/run/user/1/weft".to_string()));
        let pares: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(Rutas::resolver(&entorno(&pares), 1).unwrap(), r);
        // con --root, lo mismo
        let r = Rutas::resolver(&entorno(&[(VAR_ROOT, "/t/r")]), 1).unwrap();
        assert_eq!(r.variables()[1], (VAR_CONFIG, PathBuf::from("/t/r")));
        // modo de estado propio: nada (fijar WEFT_CONFIG_DIR sacaria al hijo del modo de estado propio)
        let a = Rutas::resolver(&entorno(&[(VAR_ESTADO, "/t/estado"), ("HOME", "/h")]), 1).unwrap();
        assert!(a.estado_propio);
        assert!(a.variables().is_empty());
    }

    #[test]
    fn las_claves_de_config_cambian_datos_y_cache() {
        let r = Rutas::resolver(&entorno(&[(VAR_ROOT, "/t/r")]), 1).unwrap();
        let c = r.clone().con_claves("auto", "auto");
        assert_eq!(c, r);
        let c = r.con_claves("/otro/datos", "/otro/cache");
        assert_eq!(c.imagen("a"), PathBuf::from("/otro/datos/images/a"));
        assert_eq!(c.descargas(), PathBuf::from("/otro/cache/downloads"));
    }

    #[test]
    fn aviso_de_carpetas_relativas() {
        let e = Rutas::resolver(&entorno(&[]), 1000).unwrap_err();
        let a = aviso_carpetas_relativas(&e);
        // el motivo (el error de resolver, con su remedio) y lo que se usa mientras tanto
        assert!(a.starts_with("no hay XDG_DATA_HOME ni HOME: indica una carpeta con --root"), "{}", a);
        assert!(a.contains("carpetas relativas al directorio actual: weft-data, weft-config, weft-cache y weft-logs"), "{}", a);
        assert!(!a.starts_with("aviso"));
        // en un entorno con HOME no hay aviso (el de las pruebas lo tiene; si no, lo hay)
        assert_eq!(carpetas_relativas().is_some(), Rutas::resolver(&|k: &str| std::env::var(k).ok(), 1).is_err());
    }

    #[test]
    fn el_directorio_de_ejecucion_es_privado_y_propio() {
        use std::os::unix::fs::PermissionsExt;
        let uid = unsafe { getuid() };
        let d = std::env::temp_dir().join(format!("weft-ejecucion-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let modo = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        // no existe (ni su padre): se crea solo para este usuario
        let nuevo = d.join("padre/weft");
        asegurar_ejecucion_de(&nuevo, uid).unwrap();
        assert!(nuevo.is_dir());
        assert_eq!(modo(&nuevo), 0o700);
        // existe, es nuestro y deja entrar a otros: se cierra
        let abierto = d.join("abierto");
        std::fs::create_dir(&abierto).unwrap();
        std::fs::set_permissions(&abierto, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert_eq!(modo(&abierto), 0o777);
        asegurar_ejecucion_de(&abierto, uid).unwrap();
        assert_eq!(modo(&abierto), 0o700);
        // ya correcto: nada cambia
        asegurar_ejecucion_de(&abierto, uid).unwrap();
        assert_eq!(modo(&abierto), 0o700);
        // de otro usuario (se simula preguntando por otro uid): error claro con el remedio
        let e = asegurar_ejecucion_de(&abierto, uid.wrapping_add(1)).unwrap_err();
        assert!(e.contains("pertenece a otro usuario") && e.contains("--state-dir") && e.contains("XDG_RUNTIME_DIR"), "{}", e);
        // un enlace simbolico, aunque apunte a una carpeta nuestra: error
        let enlace = d.join("enlace");
        std::os::unix::fs::symlink(&abierto, &enlace).unwrap();
        let e = asegurar_ejecucion_de(&enlace, uid).unwrap_err();
        assert!(e.contains("es un enlace simbolico") && e.contains("--state-dir"), "{}", e);
        assert_eq!(modo(&abierto), 0o700);
        // un archivo con ese nombre: error
        let archivo = d.join("archivo");
        std::fs::write(&archivo, b"").unwrap();
        assert!(asegurar_ejecucion_de(&archivo, uid).unwrap_err().contains("no es un directorio"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// `start` solo cierra (0700) y exige propias las carpetas que weft elige: la de ejecucion estandar y la de la maquina.
    /// Una carpeta de estado que dio el usuario (--state-dir, --root) puede ser un enlace y no se le tocan los permisos.
    #[test]
    fn la_carpeta_de_estado_del_usuario_no_se_toca() {
        use std::os::unix::fs::PermissionsExt;
        let uid = unsafe { getuid() };
        let d = std::env::temp_dir().join(format!("weft-estado-usuario-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let modo = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        // --state-dir ~/vms, con ~/vms enlace a otra carpeta (abierta al grupo): se sigue el enlace y no se cierra
        let real = d.join("datos/vms");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
        let vms = d.join("vms");
        std::os::unix::fs::symlink(&real, &vms).unwrap();
        asegurar_estado_de(&vms.join("default"), false, uid).unwrap();
        assert_eq!(modo(&real), 0o755, "se cerraron los permisos de la carpeta del usuario");
        // la de la maquina si es de weft: solo nuestra
        assert_eq!(modo(&real.join("default")), 0o700);
        // y otra vez, ya creada: igual (comprobar tambien la acepta)
        asegurar_estado_de(&vms.join("default"), false, uid).unwrap();
        comprobar_estado_de(&vms.join("default"), false, uid).unwrap();
        // una carpeta del usuario que no existe se crea (con sus permisos de siempre)
        asegurar_estado_de(&d.join("nueva/m"), false, uid).unwrap();
        assert!(d.join("nueva/m").is_dir());
        // la de ejecucion estandar si se cierra, y no puede ser un enlace
        let estandar = d.join("estandar");
        std::fs::create_dir(&estandar).unwrap();
        std::fs::set_permissions(&estandar, std::fs::Permissions::from_mode(0o755)).unwrap();
        asegurar_estado_de(&estandar.join("m"), true, uid).unwrap();
        assert_eq!((modo(&estandar), modo(&estandar.join("m"))), (0o700, 0o700));
        let e = asegurar_estado_de(&vms.join("default"), true, uid).unwrap_err();
        assert!(e.contains("enlace simbolico"), "{}", e);
        // la de la maquina nunca puede ser un enlace, ni de otro usuario
        std::os::unix::fs::symlink(real.join("default"), d.join("nueva/enlace")).unwrap();
        assert!(asegurar_estado_de(&d.join("nueva/enlace"), false, uid).unwrap_err().contains("enlace simbolico"));
        assert!(asegurar_estado_de(&vms.join("default"), false, uid.wrapping_add(1)).unwrap_err().contains("otro usuario"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Antes de leer o escribir en la carpeta de estado (cualquier orden, no solo `start`): si existe tiene que ser propia y
    /// no un enlace, y la de ejecucion estandar tambien. Sin crear nada ni cambiar permisos; lo que no existe vale.
    #[test]
    fn toda_orden_comprueba_la_carpeta_de_estado() {
        use std::os::unix::fs::PermissionsExt;
        let uid = unsafe { getuid() };
        let d = std::env::temp_dir().join(format!("weft-comprobar-estado-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        // nada existe: vale y no se crea nada
        comprobar_estado_de(&d.join("base/m"), true, uid).unwrap();
        assert!(!d.join("base").exists());
        // propias: valen y no se tocan los permisos
        std::fs::create_dir_all(d.join("base/m")).unwrap();
        std::fs::set_permissions(d.join("base/m"), std::fs::Permissions::from_mode(0o775)).unwrap();
        comprobar_estado_de(&d.join("base/m"), true, uid).unwrap();
        assert_eq!(std::fs::metadata(d.join("base/m")).unwrap().permissions().mode() & 0o777, 0o775);
        // de otro usuario (simulado), la base estandar o la de la maquina: error con el remedio
        let e = comprobar_estado_de(&d.join("base/m"), true, uid.wrapping_add(1)).unwrap_err();
        assert!(e.contains("pertenece a otro usuario") && e.contains(&d.join("base").display().to_string()) && e.contains("XDG_RUNTIME_DIR"), "{}", e);
        // la carpeta de la maquina como enlace (p. ej. a una carpeta preparada por otro): error
        std::os::unix::fs::symlink(d.join("base/m"), d.join("base/enlace")).unwrap();
        assert!(comprobar_estado_de(&d.join("base/enlace"), false, uid).unwrap_err().contains("enlace simbolico"));
        // la base estandar como enlace: error; una base del usuario como enlace: vale
        std::os::unix::fs::symlink(d.join("base"), d.join("otra")).unwrap();
        assert!(comprobar_estado_de(&d.join("otra/m"), true, uid).unwrap_err().contains("enlace simbolico"));
        comprobar_estado_de(&d.join("otra/m"), false, uid).unwrap();
        // la estandar de este proceso se reconoce (tambien escrita de otra forma); una cualquiera no, ni una relativa
        let estandar = ejecucion_estandar(&|k: &str| algo(std::env::var(k).ok()), uid);
        assert!(es_ejecucion_estandar(&estandar) && es_ejecucion_estandar(&estandar.join("x/..")));
        assert!(!es_ejecucion_estandar(&d) && !es_ejecucion_estandar(Path::new("weft")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn normalizar_quita_puntos() {
        assert_eq!(normalizar(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
        assert_eq!(normalizar(Path::new("/a/b/..")), PathBuf::from("/a"));
        assert_eq!(maquina_de(Path::new("/x/y/prueba")), "prueba");
    }
}
