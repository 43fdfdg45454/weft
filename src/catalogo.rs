//! Catalogo de imagenes de Android: detectar de que familia es una imagen (estaticamente, sin arrancar nada), instalarla desde un
//! zip o una carpeta ya descargados, listarla, quitarla y elegir la de cada maquina. Los perfiles (perfil.rs) dicen como se arranca
//! cada familia; este modulo dice cual encaja con una imagen y guarda las imagenes en `<datos>/images/<id>/` (rutas.rs).
//!
//! DECISION (cambio de imagen de una maquina): UN DISCO POR IMAGEN (`<datos>/machines/<maquina>/disks/<id>.img`). Cambiar de imagen
//! solo cambia la clave `image.id`; el disco de la otra imagen no se toca (apps, cuentas y datos siguen ahi si se vuelve) y el de la
//! imagen nueva se crea la primera vez. Razones: (1) regenerar borra datos y obliga a confirmar una perdida; (2) los datos de Android
//! (f2fs cifrado, claves en metadata) dependen de la compilacion: reutilizar un disco con otra imagen no es seguro; (3) el costo es
//! espacio (cada disco ocupa lo escrito, ~3-4 GB mas los datos), que `image remove` recupera con confirmacion.
//!
//! Este modulo no toca SDL ni la maquina: solo mira si una maquina esta en marcha (para negarse a quitar lo que usa).

use crate::config::Config;
use crate::imagen::{self, Herramienta};
use crate::perfil::{self, Arq, Catalogo, Cond, Origen, Perfil};
use crate::rutas::{nombres, Rutas};

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use crate::textos::{tx, txf};

pub type Progreso<'a> = &'a dyn Fn(&str);

// ---------------------------------------------------------------------------------------------------------------
// deteccion

/// Lo que se sabe de una imagen sin arrancarla.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hechos {
    /// nombres de archivo presentes
    pub archivos: Vec<String>,
    pub android_info: BTreeMap<String, String>,
    pub vendor_bootconfig: BTreeMap<String, String>,
    /// arquitectura del kernel de boot.img (x86_64, x86, arm64) si se reconoce
    pub kernel: Option<String>,
}

/// Lee `clave=valor` (una por linea) de un texto.
fn pares_texto(t: &str) -> BTreeMap<String, String> {
    t.lines().filter_map(|l| l.trim().split_once('=')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect()
}

/// Reune los hechos de una carpeta (con `archivos` si ya se sabe el listado, p. ej. el de un zip sin descomprimir).
pub fn hechos_de_carpeta(dir: &Path, archivos: Option<Vec<String>>) -> Hechos {
    let archivos = archivos.unwrap_or_else(|| std::fs::read_dir(dir).map(|rd| rd.filter_map(|e| e.ok()).filter(|e| e.path().is_file()).map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default());
    let mut h = Hechos { archivos, ..Default::default() };
    h.archivos.sort();
    if let Ok(t) = std::fs::read_to_string(dir.join("android-info.txt")) {
        h.android_info = pares_texto(&t);
    }
    if let Ok(d) = std::fs::read(dir.join("boot.img")) {
        if let Ok(b) = crate::bootimg::parse_boot(&d) {
            h.kernel = crate::bootimg::arquitectura_kernel(b.kernel).map(|s| s.to_string());
        }
    }
    if let Ok(d) = std::fs::read(dir.join("vendor_boot.img")) {
        if let Ok(v) = crate::bootimg::parse_vendor_boot(&d) {
            h.vendor_bootconfig = pares_texto(&v.bootconfig);
        }
    }
    h
}

fn cumple(c: &Cond, h: &Hechos) -> bool {
    match c {
        Cond::Archivo(f) => h.archivos.iter().any(|a| a == f),
        Cond::Kernel(a) => h.kernel.as_deref() == Some(a.nombre()),
        Cond::AndroidInfo(k, v) => h.android_info.get(k) == Some(v),
        Cond::VendorBootconfig(k, v) => h.vendor_bootconfig.get(k) == Some(v),
    }
}

/// Resultado de evaluar un perfil contra una imagen.
#[derive(Clone, Debug, PartialEq)]
pub struct Encaje {
    pub perfil: String,
    pub puntos: usize,
    /// condiciones "requiere" que no se cumplen, en lenguaje claro
    pub fallos: Vec<String>,
}

impl Encaje {
    pub fn compatible(&self) -> bool {
        self.fallos.is_empty()
    }
}

pub fn evaluar(p: &Perfil, h: &Hechos) -> Encaje {
    Encaje { perfil: p.id.clone(), puntos: p.puntua.iter().filter(|c| cumple(c, h)).count(), fallos: p.requiere.iter().filter(|c| !cumple(c, h)).map(|c| c.texto()).collect() }
}

/// Resultado de detectar: el perfil elegido o el motivo de que no haya ninguno.
#[derive(Clone, Debug, PartialEq)]
pub enum Deteccion {
    Elegido(String),
    /// varios perfiles empatan: sus ids
    Empate(Vec<String>),
    /// ninguno encaja: el mensaje explica por que
    Ninguno(String),
}

/// Elige el perfil de una imagen. `forzado`: un perfil pedido por el usuario (se comprueba que encaje).
pub fn detectar(cat: &Catalogo, h: &Hechos, forzado: Option<&str>) -> Deteccion {
    if let Some(id) = forzado {
        return match cat.buscar(id) {
            None => Deteccion::Ninguno(txf!("catalogo.no_existe_el_perfil_image_profiles", format!("{:?}", id))),
            Some(p) => {
                let e = evaluar(p, h);
                if e.compatible() {
                    Deteccion::Elegido(id.to_string())
                } else {
                    Deteccion::Ninguno(txf!("catalogo.la_imagen_no_encaja_con_el_perfil", format!("{:?}", id), e.fallos.join("; ")))
                }
            }
        };
    }
    let mut buenos: Vec<Encaje> = cat.perfiles.iter().map(|p| evaluar(p, h)).filter(|e| e.compatible()).collect();
    buenos.sort_by_key(|e| std::cmp::Reverse(e.puntos));
    match buenos.as_slice() {
        [] => Deteccion::Ninguno(explicar_incompatible(cat, h)),
        [uno] => Deteccion::Elegido(uno.perfil.clone()),
        [a, b, ..] if a.puntos > b.puntos => Deteccion::Elegido(a.perfil.clone()),
        todos => {
            let max = todos[0].puntos;
            Deteccion::Empate(todos.iter().filter(|e| e.puntos == max).map(|e| e.perfil.clone()).collect())
        }
    }
}

/// Por que ninguna familia conocida encaja, en lenguaje claro y sin nombrar productos.
pub fn explicar_incompatible(cat: &Catalogo, h: &Hechos) -> String {
    let mut m: Vec<String> = Vec::new();
    match h.kernel.as_deref() {
        Some("arm64") => m.push(tx!("catalogo.el_kernel_es_arm64_este_emulador_ejecuta").into()),
        Some("x86") => m.push(tx!("catalogo.el_kernel_es_x86_de_32_bits_solo_se").into()),
        None if h.archivos.iter().any(|f| f == "boot.img") => m.push(tx!("catalogo.no_se_reconoce_la_arquitectura_del").into()),
        _ => {}
    }
    for f in ["boot.img", "vendor_boot.img", "super.img"] {
        if !h.archivos.iter().any(|a| a == f) {
            m.push(txf!("catalogo.falta", f));
        }
    }
    if h.archivos.iter().any(|f| matches!(f.as_str(), "source.properties" | "kernel-ranchu" | "advancedFeatures.ini")) && !h.archivos.iter().any(|f| f == "boot.img") {
        m.push(tx!("catalogo.parece_una_imagen_de_tipo_emulador_del").into());
    }
    if h.archivos.iter().any(|f| f == "system.img") && !h.archivos.iter().any(|f| f == "super.img") {
        m.push(tx!("catalogo.es_una_imagen_de_tipo_emulador_con").into());
    }
    if h.vendor_bootconfig.is_empty() && h.archivos.iter().any(|f| f == "vendor_boot.img") {
        m.push(tx!("catalogo.el_arranque_del_fabricante_no_trae").into());
    }
    let mejor = cat.perfiles.iter().map(|p| evaluar(p, h)).min_by_key(|e| e.fallos.len());
    if let Some(e) = mejor {
        if !e.fallos.is_empty() {
            m.push(txf!("catalogo.lo_mas_parecido_es_el_perfil_al_que_le", format!("{:?}", e.perfil), e.fallos.join("; ")));
        }
    }
    if m.is_empty() {
        m.push(tx!("catalogo.ningun_perfil_integrado_ni_de_usuario").into());
    }
    txf!("catalogo.imagen_no_compatible_se_pueden_agregar", m.join("; "))
}

// ---------------------------------------------------------------------------------------------------------------
// imagen instalada

#[derive(Clone, Debug, Default, PartialEq)]
pub struct InfoImagen {
    pub id: String,
    pub perfil: String,
    pub build: Option<u32>,
    pub origen: String,
    pub bytes: u64,
    pub instalada: u64,
}

impl InfoImagen {
    pub fn texto(&self) -> String {
        let mut s = format!("# Imagen instalada por weft (se puede editar con cuidado)\nid={}\nperfil={}\n", self.id, self.perfil); // texto-interno: contenido del archivo de la imagen (clave=valor)
        if let Some(b) = self.build {
            s.push_str(&format!("build={}\n", b));
        }
        s.push_str(&format!("origen={}\nbytes={}\ninstalada={}\n", self.origen, self.bytes, self.instalada));
        s
    }

    pub fn parse(t: &str) -> InfoImagen {
        let m = pares_texto(t);
        let g = |k: &str| m.get(k).cloned().unwrap_or_default();
        InfoImagen { id: g("id"), perfil: g("perfil"), build: m.get("build").and_then(|v| v.parse().ok()), origen: g("origen"), bytes: g("bytes").parse().unwrap_or(0), instalada: g("instalada").parse().unwrap_or(0) }
    }
}

pub fn leer_info(dir: &Path) -> Option<InfoImagen> {
    std::fs::read_to_string(dir.join(nombres::INFO_IMAGEN)).ok().map(|t| InfoImagen::parse(&t))
}

fn escribir_info(dir: &Path, i: &InfoImagen) -> Result<(), String> {
    std::fs::write(dir.join(nombres::INFO_IMAGEN), i.texto()).map_err(|e| format!("{}: {}", dir.join(nombres::INFO_IMAGEN).display(), e))
}

/// Compilacion que dejo marcada una descarga (archivo `.weft-build`).
pub fn build_de_carpeta(dir: &Path) -> Option<u32> {
    std::fs::read_to_string(dir.join(nombres::MARCA_BUILD)).ok().and_then(|t| t.trim().parse().ok())
}

#[derive(Clone, Debug)]
pub struct Instalada {
    pub id: String,
    pub carpeta: PathBuf,
    pub info: InfoImagen,
    pub completa: bool,
    pub faltan: Vec<String>,
}

/// Imagenes instaladas (carpetas de `<datos>/images/`), por id.
pub fn listar(r: &Rutas, cat: &Catalogo) -> Vec<Instalada> {
    let mut v = Vec::new();
    let Ok(rd) = std::fs::read_dir(r.imagenes()) else { return v };
    let mut ids: Vec<String> = rd.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| !n.starts_with('.')).collect();
    ids.sort();
    for id in ids {
        let carpeta = r.imagen(&id);
        let info = leer_info(&carpeta).unwrap_or(InfoImagen { id: id.clone(), ..Default::default() });
        let faltan = match cat.buscar(&info.perfil) {
            Some(p) => p.archivos_requeridos().into_iter().filter(|f| !carpeta.join(f).is_file()).collect(),
            None => vec![txf!("catalogo.perfil_desconocido", format!("{:?}", info.perfil))],
        };
        v.push(Instalada { id, carpeta, completa: faltan.is_empty(), faltan, info });
    }
    v
}

pub fn buscar_instalada(r: &Rutas, cat: &Catalogo, id: &str) -> Option<Instalada> {
    listar(r, cat).into_iter().find(|i| i.id == id)
}

/// Perfil de una imagen instalada (el de `image.info`).
pub fn perfil_de<'a>(cat: &'a Catalogo, i: &Instalada) -> Result<&'a Perfil, String> {
    cat.buscar(&i.info.perfil).ok_or_else(|| txf!("catalogo.la_imagen_usa_el_perfil_que_ya_no_existe", format!("{:?}", i.id), format!("{:?}", i.info.perfil)))
}

fn tamano_carpeta(dir: &Path) -> u64 {
    std::fs::read_dir(dir).map(|rd| rd.filter_map(|e| e.ok()).filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum()).unwrap_or(0)
}

// ---------------------------------------------------------------------------------------------------------------
// maquinas: que imagen usa cada una

/// Imagen que usa una maquina: la clave `image.id` de su config; con `auto`, la unica instalada.
pub fn id_de_maquina(cfg: &Config, instaladas: &[Instalada]) -> Result<String, String> {
    let v = cfg.get("image.id");
    if v != "auto" {
        return if instaladas.iter().any(|i| i.id == v) { Ok(v) } else { Err(txf!("catalogo.la_imagen_image_id_no_esta_instalada", format!("{:?}", v))) };
    }
    match instaladas {
        [] => Err(tx!("catalogo.no_hay_ninguna_imagen_de_android").into()),
        [una] => Ok(una.id.clone()),
        varias => Err(txf!("catalogo.hay_varias_imagenes_instaladas_elige_una", varias.iter().map(|i| i.id.as_str()).collect::<Vec<_>>().join(", "))),
    }
}

/// Que hacer con `image.id` de la maquina tras instalar una imagen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrasInstalar {
    /// fijar image.id = la imagen nueva: estaba en `auto` (o apuntaba a una imagen que ya no esta) y la maquina esta
    /// apagada. Sin esto, con dos imagenes y `auto`, el siguiente `launch` fallaria ("hay varias imagenes instaladas")
    Fijar,
    /// la maquina ya usa esa imagen
    YaLaUsa,
    /// la maquina usa otra imagen elegida a mano: no se cambia (se dice como cambiarla)
    UsaOtra(String),
    /// image.id en `auto` y la maquina en marcha con otra imagen instalada (la de su arranque): image.id = esa, para que la
    /// maquina siga igual y `auto` no quede entre dos imagenes; la nueva no se cambia bajo sus pies (se dice como usarla)
    MantenerLaDeAhora(String),
    /// image.id en `auto` y la maquina en marcha sin una imagen instalada conocida: no se cambia (se dice como cambiarla)
    EnMarcha,
}

/// Decide `TrasInstalar`. `image_id`: valor de `image.id` en la config de la maquina (`auto` si no esta); `nuevo`: id de la
/// imagen instalada; `instalada(id)`: si habia una imagen instalada con ese id; `en_marcha`: la imagen con que arranco la
/// maquina si esta en marcha (Some(None) si arranco sin imagen o no se sabe), None si esta apagada. Pura.
pub fn tras_instalar(image_id: &str, nuevo: &str, instalada: &dyn Fn(&str) -> bool, en_marcha: Option<Option<&str>>) -> TrasInstalar {
    let v = image_id.trim();
    if v == nuevo {
        return TrasInstalar::YaLaUsa;
    }
    if !v.is_empty() && v != "auto" && instalada(v) {
        return TrasInstalar::UsaOtra(v.to_string());
    }
    match en_marcha {
        None => TrasInstalar::Fijar,
        Some(Some(ahora)) if ahora == nuevo => TrasInstalar::Fijar,
        Some(Some(ahora)) if instalada(ahora) => TrasInstalar::MantenerLaDeAhora(ahora.to_string()),
        Some(_) => TrasInstalar::EnMarcha,
    }
}

/// Maquinas (nombres) que tienen su config con `image.id` = id (mira todas las del directorio de configuracion).
pub fn maquinas_que_usan(r: &Rutas, id: &str) -> Vec<String> {
    let mut v = Vec::new();
    let base = if r.estado_propio { r.ejecucion.clone() } else { r.dir_config_maquinas() };
    if let Ok(rd) = std::fs::read_dir(&base) {
        for e in rd.filter_map(|e| e.ok()) {
            let n = e.file_name().to_string_lossy().into_owned();
            if let Ok(t) = std::fs::read_to_string(e.path().join(nombres::CONFIG)) {
                if Config::parse(&t).get("image.id") == id {
                    v.push(n);
                }
            }
        }
    }
    v.sort();
    v
}

/// Discos que hay para una imagen: (maquina, ruta, bytes).
pub fn discos_de_imagen(r: &Rutas, id: &str) -> Vec<(String, PathBuf, u64)> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(r.datos.join(nombres::MAQUINAS)) {
        for e in rd.filter_map(|e| e.ok()) {
            let n = e.file_name().to_string_lossy().into_owned();
            let d = r.disco(&n, id);
            if let Ok(m) = std::fs::metadata(&d) {
                v.push((n, d, m.len()));
            }
        }
    }
    v.sort();
    v
}

/// Discos que usan alguna de las bases `bases` (overlays de copia en escritura, ver cow.rs): los de todas las maquinas
/// (`<datos>/machines/*/disks/`) y el disco con que arranco cada estado de ejecucion (`disk.path`, que puede ser otro).
/// Es el recuento de referencias de una base: mientras tenga alguno, no se borra ni se reemplaza.
pub fn usuarios_de_bases(r: &Rutas, bases: &[PathBuf]) -> Vec<PathBuf> {
    let mut candidatos: Vec<PathBuf> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(r.datos.join(nombres::MAQUINAS)) {
        for m in rd.flatten() {
            if let Ok(dd) = std::fs::read_dir(m.path().join(nombres::DISCOS)) {
                candidatos.extend(dd.flatten().map(|e| e.path()).filter(|p| p.is_file()));
            }
        }
    }
    if let Ok(rd) = std::fs::read_dir(&r.ejecucion) {
        for e in rd.flatten() {
            if let Ok(t) = std::fs::read_to_string(e.path().join("disk.path")) {
                candidatos.push(PathBuf::from(t.trim()));
            }
        }
    }
    let mut v: Vec<PathBuf> = candidatos.into_iter().filter(|d| bases.iter().any(|b| crate::cow::usa_base(d, b))).collect();
    v.sort();
    v.dedup();
    v
}

/// Bases de copia en escritura de una imagen (`<datos>/bases/ID/*.img`).
pub fn bases_de_imagen(r: &Rutas, id: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(r.dir_bases(id)).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "img")).collect()).unwrap_or_default();
    v.sort();
    v
}

/// ¿Esta en marcha la maquina `n`? (su estado de ejecucion: el pid guardado, si sigue siendo el QEMU que arranco; ver vm.rs)
pub fn en_marcha(r: &Rutas, n: &str) -> bool {
    crate::vm::State { dir: r.ejecucion.join(n) }.running()
}

// ---------------------------------------------------------------------------------------------------------------
// origenes: zip o carpeta

#[derive(Clone, Debug, PartialEq)]
pub enum Fuente {
    Zip(PathBuf),
    Carpeta(PathBuf),
}

pub fn fuente_de(t: &str) -> Result<Fuente, String> {
    if t.contains("://") {
        return Err(txf!("catalogo.no_descarga_nada_baja_el_archivo_por_tu", t));
    }
    let p = PathBuf::from(t);
    if p.is_dir() {
        Ok(Fuente::Carpeta(p))
    } else if p.is_file() {
        Ok(Fuente::Zip(p))
    } else {
        Err(txf!("catalogo.no_existe_se_espera_la_ruta_de_un_zip_o", t))
    }
}

/// Orden que lista los nombres de un zip.
fn orden_listar(h: Herramienta, zip: &Path) -> (String, Vec<String>) {
    let z = zip.to_string_lossy().into_owned();
    match h {
        Herramienta::Unzip => ("unzip".into(), vec!["-Z1".into(), z]),
        Herramienta::Bsdtar => ("bsdtar".into(), vec!["-tf".into(), z]),
        Herramienta::Python => ("python3".into(), vec!["-c".into(), "import zipfile,sys\nfor n in zipfile.ZipFile(sys.argv[1]).namelist(): print(n)".into(), z]),
    }
}

/// Orden que extrae solo algunos miembros de un zip.
fn orden_extraer(h: Herramienta, zip: &Path, dir: &Path, miembros: &[String]) -> (String, Vec<String>) {
    let (z, d) = (zip.to_string_lossy().into_owned(), dir.to_string_lossy().into_owned());
    match h {
        Herramienta::Unzip => ("unzip".into(), ["-q", "-o"].iter().map(|s| s.to_string()).chain([z, "-d".into(), d]).chain(miembros.iter().cloned()).collect()),
        Herramienta::Bsdtar => ("bsdtar".into(), ["-xf"].iter().map(|s| s.to_string()).chain([z, "-C".into(), d]).chain(miembros.iter().cloned()).collect()),
        Herramienta::Python => (
            "python3".into(),
            ["-c", "import zipfile,sys\nz=zipfile.ZipFile(sys.argv[1])\nn=set(z.namelist())\nfor m in sys.argv[3:]:\n    if m in n: z.extract(m, sys.argv[2])"].iter().map(|s| s.to_string()).chain([z, d]).chain(miembros.iter().cloned()).collect(),
        ),
    }
}

fn ejecutar(prog: &str, args: &[String]) -> Result<std::process::Output, String> {
    Command::new(prog).args(args).stdin(Stdio::null()).output().map_err(|e| txf!("catalogo.no_se_pudo_ejecutar", prog, e))
}

/// Nombres dentro de un zip y, si todos viven bajo una carpeta unica, ese prefijo (`carpeta/`).
pub fn listar_zip(zip: &Path) -> Result<(Vec<String>, String), String> {
    let h = imagen::elegir_herramienta(&imagen::en_path)?;
    let (p, a) = orden_listar(h, zip);
    let o = ejecutar(&p, &a)?;
    if !o.status.success() {
        return Err(txf!("catalogo.no_pudo_leer", p, zip.display(), String::from_utf8_lossy(&o.stderr).trim()));
    }
    let nombres: Vec<String> = String::from_utf8_lossy(&o.stdout).lines().map(|l| l.trim().trim_start_matches("./").to_string()).filter(|l| !l.is_empty() && !l.ends_with('/')).collect();
    Ok(prefijo_comun(nombres))
}

/// Si todos los nombres empiezan por la misma carpeta, la quita. Devuelve (nombres sin prefijo, prefijo).
pub fn prefijo_comun(nombres: Vec<String>) -> (Vec<String>, String) {
    let primero = nombres.first().and_then(|n| n.split_once('/')).map(|(d, _)| format!("{}/", d));
    match primero {
        Some(pre) if nombres.iter().all(|n| n.starts_with(&pre)) => (nombres.iter().map(|n| n[pre.len()..].to_string()).collect(), pre),
        _ => (nombres, String::new()),
    }
}

/// Hechos de un zip sin descomprimirlo entero: extrae solo android-info.txt, boot.img y vendor_boot.img a `tmp`.
pub fn hechos_de_zip(zip: &Path, tmp: &Path) -> Result<Hechos, String> {
    let (nombres, prefijo) = listar_zip(zip)?;
    let h = imagen::elegir_herramienta(&imagen::en_path)?;
    std::fs::create_dir_all(tmp).map_err(|e| format!("{}: {}", tmp.display(), e))?;
    let quiero: Vec<String> = ["android-info.txt", "boot.img", "vendor_boot.img"].iter().filter(|n| nombres.iter().any(|x| x == *n)).map(|n| format!("{}{}", prefijo, n)).collect();
    if !quiero.is_empty() {
        let (p, a) = orden_extraer(h, zip, tmp, &quiero);
        let o = ejecutar(&p, &a)?;
        if !o.status.success() {
            return Err(txf!("catalogo.no_pudo_extraer_de", p, zip.display(), String::from_utf8_lossy(&o.stderr).trim()));
        }
    }
    let base = if prefijo.is_empty() { tmp.to_path_buf() } else { tmp.join(prefijo.trim_end_matches('/')) };
    Ok(hechos_de_carpeta(&base, Some(nombres)))
}

// ---------------------------------------------------------------------------------------------------------------
// instalar

pub struct OpcionesAdd {
    /// zip o carpeta con la imagen (ya en el equipo)
    pub origen: String,
    pub id: Option<String>,
    pub perfil: Option<String>,
}

/// `...-img-16373615.zip` -> 16373615.
pub fn build_de_nombre(nombre: &str) -> Option<u32> {
    let base = nombre.strip_suffix(".zip").unwrap_or(nombre);
    let n = base.rsplit(|c: char| !c.is_ascii_digit()).next()?;
    if n.len() >= 6 {
        n.parse().ok()
    } else {
        None
    }
}

fn limpiar_nombre(t: &str) -> String {
    let s: String = t.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' }).collect();
    s.trim_matches(|c| c == '-' || c == '.').chars().take(48).collect()
}

/// Id de una imagen nueva: `<perfil>-<build>` si se conoce la compilacion, si no `<perfil>-<nombre limpio del origen>`.
pub fn id_por_defecto(perfil: &str, build: Option<u32>, nombre_origen: &str) -> String {
    match build {
        Some(b) => format!("{}-{}", perfil, b),
        None => {
            let l = limpiar_nombre(nombre_origen);
            if l.is_empty() { format!("{}-local", perfil) } else { format!("{}-{}", perfil, l) }
        }
    }
}

fn espacio_o_error(dir: &Path, necesario: u64) -> Result<(), String> {
    if let Some(libre) = imagen::espacio_libre(dir) {
        if libre < necesario {
            return Err(txf!("catalogo.espacio_insuficiente_en_libres_y_hacen", dir.display(), imagen::gib(libre), imagen::gib(necesario)));
        }
    }
    Ok(())
}

/// Copia los archivos de `origen` (sin subcarpetas) a `destino`, contando el progreso por bytes ("copiando la imagen:
/// 45 %"; la ventana lo dibuja como barra).
fn copiar_carpeta(origen: &Path, destino: &Path, prog: Progreso) -> Result<(), String> {
    use std::io::{Read, Write};
    std::fs::create_dir_all(destino).map_err(|e| format!("{}: {}", destino.display(), e))?;
    let archivos: Vec<_> = std::fs::read_dir(origen).map_err(|e| format!("{}: {}", origen.display(), e))?.filter_map(|e| e.ok()).filter(|e| e.path().is_file()).collect();
    let total: u64 = archivos.iter().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum();
    let (mut hecho, mut visto) = (0u64, u64::MAX);
    let mut buf = vec![0u8; 8 << 20];
    for e in archivos {
        let error = |er: std::io::Error| txf!("catalogo.copiando", e.path().display(), er);
        let mut f = std::fs::File::open(e.path()).map_err(error)?;
        let mut g = std::fs::File::create(destino.join(e.file_name())).map_err(error)?;
        loop {
            let n = f.read(&mut buf).map_err(error)?;
            if n == 0 {
                break;
            }
            g.write_all(&buf[..n]).map_err(error)?;
            hecho += n as u64;
            let pct = (hecho * 100).checked_div(total).unwrap_or(100);
            if pct != visto {
                visto = pct;
                prog(&txf!("catalogo.copiando_la_imagen", pct));
            }
        }
    }
    Ok(())
}

/// Bytes de los archivos de `dir` y sus subcarpetas (sin seguir enlaces).
fn tamano_recursivo(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| e.metadata().ok().map(|m| (e.path(), m)))
                .map(|(p, m)| if m.is_dir() { tamano_recursivo(&p) } else if m.is_file() { m.len() } else { 0 })
                .sum()
        })
        .unwrap_or(0)
}

/// Carpetas parciales (`.partial-PID`, `.partial2-PID`) de una instalacion anterior que ya no corre (se cancelo o se
/// corto): las que quedan en `dir` cuyo pid no existe. Pura salvo `vivo`.
pub fn parciales_huerfanas(nombres: &[String], vivo: &dyn Fn(u32) -> bool) -> Vec<String> {
    nombres
        .iter()
        .filter(|n| {
            let pid = n.strip_prefix(".partial-").or_else(|| n.strip_prefix(".partial2-")).and_then(|p| p.parse::<u32>().ok());
            pid.is_some_and(|p| p != std::process::id() && !vivo(p))
        })
        .cloned()
        .collect()
}

/// Instala una imagen. Devuelve (id, texto). Si ya esta instalada con el mismo id y completa, no hace nada (Ok).
pub fn instalar(r: &Rutas, cat: &Catalogo, op: &OpcionesAdd, prog: Progreso) -> Result<(String, String), String> {
    let perfil_forzado = op.perfil.as_deref();
    let fuente = fuente_de(&op.origen)?;
    let nombre_origen = match &fuente {
        Fuente::Zip(p) | Fuente::Carpeta(p) => p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
    };
    let build_origen = build_de_nombre(&nombre_origen).or_else(|| if let Fuente::Carpeta(p) = &fuente { build_de_carpeta(p) } else { None });
    // una imagen ya instalada con el id previsto (perfil forzado): no se copia otra vez
    if let (Some(pid), Some(b), true) = (perfil_forzado, build_origen, op.id.is_none()) {
        let id = id_por_defecto(pid, Some(b), &nombre_origen);
        if let Some(i) = buscar_instalada(r, cat, &id) {
            if i.completa {
                return Ok((id.clone(), txf!("catalogo.la_imagen_compilacion_ya_esta_instalada", id, b, i.carpeta.display())));
            }
        }
    }
    std::fs::create_dir_all(r.imagenes()).map_err(|e| format!("{}: {}", r.imagenes().display(), e))?;
    // lo que dejo una instalacion cancelada o cortada
    let nombres: Vec<String> = std::fs::read_dir(r.imagenes()).map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    for n in parciales_huerfanas(&nombres, &|p| Path::new(&format!("/proc/{}", p)).exists()) {
        let _ = std::fs::remove_dir_all(r.imagenes().join(n));
    }
    let parcial = r.imagenes().join(format!(".partial-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&parcial);
    let limpiar = |e: String| -> String {
        let _ = std::fs::remove_dir_all(&parcial);
        e
    };
    let local: Fuente = fuente;
    // 2) detectar con lo minimo
    let hechos = match &local {
        Fuente::Zip(z) => {
            prog(tx!("catalogo.leyendo_el_contenido_del_zip"));
            hechos_de_zip(z, &parcial.join(".probe")).map_err(limpiar)?
        }
        Fuente::Carpeta(d) => hechos_de_carpeta(d, None),
    };
    let _ = std::fs::remove_dir_all(parcial.join(".probe"));
    let perfil_id = match detectar(cat, &hechos, perfil_forzado) {
        Deteccion::Elegido(p) => p,
        Deteccion::Empate(ps) => {
            return Err(limpiar(txf!("catalogo.la_imagen_encaja_con_varios_perfiles", ps.join(", "))));
        }
        Deteccion::Ninguno(m) => {
            return Err(limpiar(m));
        }
    };
    let perfil = cat.buscar(&perfil_id).unwrap();
    if perfil.arquitectura != Arq::X86_64 {
        prog(&txf!("catalogo.aviso_el_perfil_es_de_otra_arquitectura", perfil.id, perfil.arquitectura.nombre()));
    }
    let id = match &op.id {
        Some(i) => {
            perfil::validar_id(i).map_err(limpiar)?;
            i.clone()
        }
        None => id_por_defecto(&perfil_id, build_origen, &nombre_origen),
    };
    let destino = r.imagen(&id);
    if destino.exists() {
        let i = buscar_instalada(r, cat, &id);
        return if i.as_ref().is_some_and(|i| i.completa) && op.id.is_none() {
            Ok((id.clone(), txf!("catalogo.la_imagen_ya_esta_instalada_en", id, destino.display())))
        } else {
            Err(limpiar(txf!("catalogo.ya_existe_una_imagen_con_el_id_elige", format!("{:?}", id), destino.display(), id)))
        };
    }
    // 3) llevar los archivos a la carpeta parcial
    match &local {
        Fuente::Zip(z) => {
            let h = imagen::elegir_herramienta(&imagen::en_path).map_err(limpiar)?;
            let tam = std::fs::metadata(z).map(|m| m.len()).unwrap_or(0);
            espacio_o_error(&r.imagenes(), (tam * 2).max(4 << 30)).map_err(limpiar)?;
            prog(&imagen::Paso::Descomprimiendo(h.nombre().to_string(), tam).texto());
            std::fs::create_dir_all(&parcial).map_err(|e| e.to_string())?;
            let (p, a) = imagen::orden_descompresion(h, z, &parcial);
            // la herramienta no dice cuanto lleva: se mira lo que ya escribio, cada segundo
            let base = imagen::Paso::Descomprimiendo(h.nombre().to_string(), tam).texto();
            let mut hijo = Command::new(&p).args(&a).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| limpiar(txf!("catalogo.no_se_pudo_ejecutar", p, e)))?;
            let mut err = hijo.stderr.take();
            let lector = std::thread::spawn(move || {
                let mut t = Vec::new();
                if let Some(e) = err.as_mut() {
                    let _ = std::io::Read::read_to_end(e, &mut t);
                }
                t
            });
            let estado = loop {
                match hijo.try_wait() {
                    Ok(Some(st)) => break st,
                    Ok(None) => {}
                    Err(e) => return Err(limpiar(e.to_string())),
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
                prog(&txf!("catalogo.extraidos", base, imagen::mib(tamano_recursivo(&parcial))));
            };
            let o = std::process::Output { status: estado, stdout: Vec::new(), stderr: lector.join().unwrap_or_default() };
            if !o.status.success() {
                return Err(limpiar(txf!("catalogo.no_pudo_descomprimir_la_imagen", p, String::from_utf8_lossy(&o.stderr).trim())));
            }
            // una carpeta unica envolvente: se sube un nivel
            let (_, pre) = listar_zip(z).unwrap_or_default();
            if !pre.is_empty() {
                let dentro = parcial.join(pre.trim_end_matches('/'));
                let tmp2 = r.imagenes().join(format!(".partial2-{}", std::process::id()));
                std::fs::rename(&dentro, &tmp2).and_then(|_| std::fs::remove_dir_all(&parcial)).and_then(|_| std::fs::rename(&tmp2, &parcial)).map_err(|e| limpiar(e.to_string()))?;
            }
        }
        Fuente::Carpeta(d) => {
            espacio_o_error(&r.imagenes(), tamano_carpeta(d) + (256 << 20))?;
            prog(&txf!("catalogo.copiando_la_imagen_de", d.display()));
            copiar_carpeta(d, &parcial, prog).map_err(limpiar)?;
        }
    }
    // 4) comprobar que esta completa para el perfil, anotar y publicar
    let faltan: Vec<String> = perfil.archivos_requeridos().into_iter().filter(|f| !parcial.join(f).is_file()).collect();
    if !faltan.is_empty() {
        return Err(limpiar(txf!("catalogo.la_imagen_no_trae", faltan.join(", "))));
    }
    if let Some(b) = build_origen {
        let _ = std::fs::write(parcial.join(nombres::MARCA_BUILD), format!("{}\n", b));
    }
    let origen = match &local {
        Fuente::Zip(_) => "zip".to_string(),
        _ => "carpeta".to_string(),
    };
    let info = InfoImagen {
        id: id.clone(),
        perfil: perfil_id.clone(),
        build: build_origen,
        origen,
        bytes: tamano_carpeta(&parcial),
        instalada: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
    };
    escribir_info(&parcial, &info).map_err(limpiar)?;
    std::fs::rename(&parcial, &destino).map_err(|e| limpiar(format!("{}: {}", destino.display(), e)))?;
    Ok((id.clone(), txf!("catalogo.imagen_instalada_en_perfil", id, destino.display(), perfil_id, imagen::mib(info.bytes))))
}

/// Quita una imagen. Se niega si una maquina (en marcha o no) la usa. Los discos que haya para ella se borran SOLO con `con_discos`.
pub fn quitar(r: &Rutas, cat: &Catalogo, id: &str, con_discos: bool) -> Result<String, String> {
    let i = buscar_instalada(r, cat, id).ok_or_else(|| txf!("catalogo.no_hay_una_imagen_instalada_con_el_id", format!("{:?}", id)))?;
    let usan = maquinas_que_usan(r, id);
    if !usan.is_empty() {
        return Err(txf!("catalogo.la_imagen_la_usa_la_maquina_cambiala", format!("{:?}", id), usan.join(", ")));
    }
    let discos = discos_de_imagen(r, id);
    if let Some((m, _, _)) = discos.iter().find(|(m, _, _)| en_marcha(r, m)) {
        return Err(txf!("catalogo.la_maquina_esta_en_marcha_con_un_disco", m));
    }
    if !discos.is_empty() && !con_discos {
        let t: Vec<String> = discos.iter().map(|(m, _, b)| format!("{} ({})", m, imagen::gib(*b))).collect();
        return Err(txf!("catalogo.hay_discos_de_maquinas_para_esta_imagen", t.join(", "), id));
    }
    // las bases de copia en escritura de la imagen se borran con ella, pero nunca mientras las use un disco que no se borra
    // aqui (otra ruta elegida con --out, el disco con que arranco un estado): se comprueba antes de borrar nada
    let bases = bases_de_imagen(r, id);
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let se_borran: Vec<PathBuf> = discos.iter().map(|(_, d, _)| canon(d)).collect();
    let otros: Vec<PathBuf> = usuarios_de_bases(r, &bases).into_iter().filter(|d| !se_borran.contains(&canon(d))).collect();
    if !otros.is_empty() {
        return Err(txf!("catalogo.la_base_de_los_discos_de_esta_imagen_la", r.dir_bases(id).display(), otros.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join(", ")));
    }
    for (_, d, _) in &discos {
        std::fs::remove_file(d).map_err(|e| format!("{}: {}", d.display(), e))?;
    }
    if !bases.is_empty() {
        std::fs::remove_dir_all(r.dir_bases(id)).map_err(|e| format!("{}: {}", r.dir_bases(id).display(), e))?;
    }
    let _ = std::fs::remove_dir_all(r.cache_arranque(id));
    std::fs::remove_dir_all(&i.carpeta).map_err(|e| format!("{}: {}", i.carpeta.display(), e))?;
    Ok(txf!("catalogo.imagen_quitada", id, if discos.is_empty() { String::new() } else { txf!("catalogo.y_disco_s", discos.len()) }))
}

/// Descripcion de `image info`.
pub fn describir(r: &Rutas, cat: &Catalogo, i: &Instalada) -> String {
    let mut s = txf!("catalogo.id_carpeta", i.id, i.carpeta.display());
    s.push_str(&txf!("catalogo.perfil_2", i.info.perfil));
    if let Some(p) = cat.buscar(&i.info.perfil) {
        s.push_str(&txf!("catalogo.nombre_del_perfil_familia", p.nombre, if p.origen == Origen::Integrado { "integrado" } else { tx!("catalogo.de_usuario") }, p.familia, p.arquitectura.nombre()));
        if !p.descripcion.trim().is_empty() {
            s.push_str(&txf!("catalogo.descripcion_del_perfil", p.descripcion.trim()));
        }
        if !p.aviso.is_empty() {
            s.push_str(&txf!("catalogo.aviso_del_perfil", p.aviso));
        }
        for (c, v) in &p.cap {
            s.push_str(&txf!("catalogo.capacidad", c, v));
        }
        s.push_str(&txf!("catalogo.motores_graficos_adb_puerto", p.motores.join(" "), p.adb_transportes.join(" "), p.adb_puerto));
    }
    s.push_str(&txf!("catalogo.compilacion_2", i.info.build.map_or(tx!("comun.desconocida").to_string(), |b| b.to_string())));
    s.push_str(&txf!("catalogo.origen_tamano", i.info.origen, imagen::mib(tamano_carpeta(&i.carpeta))));
    s.push_str(&txf!("catalogo.completa", if i.completa { "si".to_string() } else { txf!("catalogo.no_faltan", i.faltan.join(", ")) }));
    let h = hechos_de_carpeta(&i.carpeta, None);
    s.push_str(&format!("kernel: {}\n", h.kernel.as_deref().unwrap_or(tx!("catalogo.no_reconocido"))));
    if let Some(c) = h.android_info.get("config") {
        s.push_str(&txf!("catalogo.android_info_config", c));
    }
    let usan = maquinas_que_usan(r, &i.id);
    s.push_str(&txf!("catalogo.la_usan_image_id", if usan.is_empty() { tx!("catalogo.ninguna_maquina").to_string() } else { usan.join(", ") }));
    for (m, d, b) in discos_de_imagen(r, &i.id) {
        s.push_str(&txf!("catalogo.disco_de_la_maquina", m, d.display(), imagen::gib(b)));
    }
    s
}

pub fn listado(r: &Rutas, cat: &Catalogo, actual: Option<&str>) -> String {
    let v = listar(r, cat);
    if v.is_empty() {
        return txf!("catalogo.no_hay_imagenes_instaladas_en_para", r.imagenes().display());
    }
    let mut s = format!("{:<28} {:<16} {:>10} {:>10}  {}\n", "id", tx!("catalogo.perfil"), tx!("catalogo.compilacion"), tx!("catalogo.tamano"), tx!("catalogo.uso"));
    for i in v {
        let usan = maquinas_que_usan(r, &i.id);
        let mut uso = if usan.is_empty() { String::new() } else { txf!("catalogo.usada_por", usan.join(", ")) };
        if actual == Some(i.id.as_str()) {
            uso = if uso.is_empty() { tx!("catalogo.esta_maquina_2").into() } else { txf!("catalogo.esta_maquina", uso) };
        }
        if !i.completa {
            uso = txf!("catalogo.incompleta", i.faltan.join(", "), uso);
        }
        s.push_str(&format!("{:<28} {:<16} {:>10} {:>10}  {}\n", i.id, i.info.perfil, i.info.build.map_or("-".to_string(), |b| b.to_string()), imagen::mib(tamano_carpeta(&i.carpeta)), uso.trim()));
    }
    s
}

pub fn listado_perfiles(cat: &Catalogo, r: &Rutas) -> String {
    let mut s = String::new();
    for p in &cat.perfiles {
        s.push_str(&format!("{:<20} {:<9} {:<8} {}\n", p.id, if p.origen == Origen::Integrado { "integrado" } else { "usuario" }, p.arquitectura.nombre(), p.nombre));
        // la descripcion (`perfil.descripcion`), debajo del nombre
        if !p.descripcion.trim().is_empty() {
            s.push_str(&format!("{:40}{}\n", "", p.descripcion.trim()));
        }
    }
    for a in &cat.avisos {
        s.push_str(&txf!("catalogo.aviso", a));
    }
    for (f, e) in &cat.rechazados {
        s.push_str(&txf!("catalogo.rechazado", f, e));
    }
    s.push_str(&txf!("catalogo.carpeta_de_perfiles_del_usuario", r.perfiles_usuario().display()));
    s
}

/// Explica una deteccion para `image check`.
pub fn informe_check(cat: &Catalogo, h: &Hechos, forzado: Option<&str>) -> (bool, String) {
    let mut s = String::new();
    s.push_str(&txf!("catalogo.archivos", h.archivos.join(" ")));
    s.push_str(&format!("kernel: {}\n", h.kernel.as_deref().unwrap_or(tx!("catalogo.no_reconocido"))));
    for p in &cat.perfiles {
        let e = evaluar(p, h);
        s.push_str(&txf!("catalogo.perfil_puntos", format!("{:<18}", p.id), if e.compatible() { "ENCAJA" } else { tx!("catalogo.no_encaja") }, e.puntos, if e.compatible() { String::new() } else { format!(": {}", e.fallos.join("; ")) }));
    }
    match detectar(cat, h, forzado) {
        Deteccion::Elegido(p) => {
            s.push_str(&txf!("catalogo.resultado_compatible_perfil", p));
            (true, s)
        }
        Deteccion::Empate(ps) => {
            s.push_str(&txf!("catalogo.resultado_encaja_con_varios_perfiles", ps.join(", ")));
            (false, s)
        }
        Deteccion::Ninguno(m) => {
            s.push_str(&txf!("catalogo.resultado", m));
            (false, s)
        }
    }
}

pub fn cargar_catalogo(r: &Rutas) -> Catalogo {
    Catalogo::cargar(Some(&r.perfiles_usuario()))
}

/// Una imagen instalada tal como la muestra la pantalla de configuracion.
#[derive(Clone, Debug, PartialEq)]
pub struct FilaImagen {
    pub id: String,
    pub perfil: String,
    pub nombre_perfil: String,
    pub build: Option<u32>,
    pub bytes: u64,
    pub completa: bool,
    /// la maquina ya tiene un disco para esta imagen (sus datos se conservan al volver a ella)
    pub con_disco: bool,
}

/// Imagenes instaladas y la que usa la maquina del estado (su config, o la unica instalada).
pub fn filas_para(estado: &Path) -> (Vec<FilaImagen>, Option<String>) {
    let cfg = Config::cargar(estado);
    let r = crate::rutas::actual().con_claves(&cfg.get("dir.datos"), &cfg.get("dir.cache"));
    let cat = cargar_catalogo(&r);
    let inst = listar(&r, &cat);
    let m = crate::rutas::maquina_de(estado);
    let actual = std::fs::read_to_string(estado.join("image-id")).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty() && cfg.get("image.id") == "auto").or_else(|| id_de_maquina(&cfg, &inst).ok());
    let filas = inst
        .iter()
        .map(|i| FilaImagen {
            id: i.id.clone(),
            perfil: i.info.perfil.clone(),
            nombre_perfil: cat.buscar(&i.info.perfil).map_or(i.info.perfil.clone(), |p| p.nombre.clone()),
            build: i.info.build,
            bytes: tamano_carpeta(&i.carpeta),
            completa: i.completa,
            con_disco: r.disco(&m, &i.id).is_file(),
        })
        .collect();
    (filas, actual)
}

#[cfg(test)]
pub(crate) fn todos_los_textos() -> Vec<String> {
    let cat = Catalogo::integrados();
    let h = Hechos { archivos: vec!["boot.img".into()], kernel: Some("arm64".into()), ..Default::default() };
    vec![explicar_incompatible(&cat, &h), informe_check(&cat, &h, None).1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootimg::tests::{fake_boot, fake_vendor_boot};

    /// Solo las parciales de procesos que ya no existen se limpian (nunca la propia ni otra cosa).
    #[test]
    fn parciales_huerfanas_de_instalaciones_cortadas() {
        let yo = std::process::id();
        let n: Vec<String> = [".partial-10".to_string(), ".partial2-11".into(), ".partial-12".into(), format!(".partial-{}", yo), "img-1".into(), ".partial-x".into()].to_vec();
        assert_eq!(parciales_huerfanas(&n, &|p| p == 12), [".partial-10", ".partial2-11"]);
    }

    /// Copiar una carpeta cuenta el progreso por porcentaje y termina en 100 %.
    #[test]
    fn copiar_carpeta_con_progreso() {
        let t = std::env::temp_dir().join(format!("weft-copiar-{}", std::process::id()));
        let (o, d) = (t.join("o"), t.join("d"));
        std::fs::create_dir_all(&o).unwrap();
        std::fs::write(o.join("a"), vec![1u8; 3000]).unwrap();
        std::fs::write(o.join("b"), vec![2u8; 1000]).unwrap();
        let vistos = std::cell::RefCell::new(Vec::new());
        copiar_carpeta(&o, &d, &|t: &str| vistos.borrow_mut().push(t.to_string())).unwrap();
        assert_eq!(std::fs::read(d.join("a")).unwrap().len(), 3000);
        assert_eq!(tamano_recursivo(&t), 8000);
        let v = vistos.into_inner();
        assert_eq!(v.last().map(String::as_str), Some("copiando la imagen: 100 %"));
        assert!(v.iter().all(|x| crate::ajustes::porcentaje_de(x).is_some()));
        let _ = std::fs::remove_dir_all(&t);
    }

    fn dir(n: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ar-cat-{}-{}", n, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// kernel falso de x86_64 (HdrS en 0x202 y xloadflags con el bit de 64 bits) o de ARM64
    fn kernel(arq: &str) -> Vec<u8> {
        let mut k = vec![0u8; 0x400];
        match arq {
            "x86_64" => {
                k[0x202..0x206].copy_from_slice(b"HdrS");
                k[0x236] = 1;
            }
            "x86" => k[0x202..0x206].copy_from_slice(b"HdrS"),
            _ => k[0x38..0x3c].copy_from_slice(b"ARM\x64"),
        }
        k
    }

    fn imagen_falsa(d: &Path, arq: &str, config: &str, bc: &str) {
        let cat = Catalogo::integrados();
        let p = cat.buscar("phone-x86_64").unwrap();
        for n in p.archivos_requeridos() {
            std::fs::write(d.join(&n), vec![2u8; 4096]).unwrap();
        }
        std::fs::write(d.join("boot.img"), fake_boot(&kernel(arq), b"RD", "a=1")).unwrap();
        std::fs::write(d.join("vendor_boot.img"), fake_vendor_boot(b"VENDOR", "v=2", bc)).unwrap();
        std::fs::write(d.join("android-info.txt"), format!("config={}\n", config)).unwrap();
    }

    const BC_CF: &str = "androidboot.hardware=cutf_cvm\nandroidboot.x=1\n";

    #[test]
    fn el_detector_elige_telefono_o_coche_y_explica_lo_incompatible() {
        let cat = Catalogo::integrados();
        let d = dir("det");
        imagen_falsa(&d, "x86_64", "phone", BC_CF);
        assert_eq!(detectar(&cat, &hechos_de_carpeta(&d, None), None), Deteccion::Elegido("phone-x86_64".into()));
        imagen_falsa(&d, "x86_64", "auto", BC_CF);
        assert_eq!(detectar(&cat, &hechos_de_carpeta(&d, None), None), Deteccion::Elegido("car-x86_64".into()));
        // sin android-info.txt empatan: hay que elegir
        std::fs::remove_file(d.join("android-info.txt")).unwrap();
        std::fs::remove_file(d.join("init_boot.img")).unwrap();
        assert!(matches!(detectar(&cat, &hechos_de_carpeta(&d, None), None), Deteccion::Empate(v) if v.len() == 2));
        assert_eq!(detectar(&cat, &hechos_de_carpeta(&d, None), Some("car-x86_64")), Deteccion::Elegido("car-x86_64".into()));
        assert!(matches!(detectar(&cat, &hechos_de_carpeta(&d, None), Some("no-existe")), Deteccion::Ninguno(_)));
        // ARM64: fuera de alcance, con el motivo
        imagen_falsa(&d, "arm64", "phone", BC_CF);
        match detectar(&cat, &hechos_de_carpeta(&d, None), None) {
            Deteccion::Ninguno(m) => assert!(m.contains("ARM64") && m.contains("KVM"), "{}", m),
            o => panic!("{:?}", o),
        }
        // otra familia (sin los parametros del fabricante conocido)
        imagen_falsa(&d, "x86_64", "phone", "androidboot.hardware=otra\n");
        match detectar(&cat, &hechos_de_carpeta(&d, None), None) {
            Deteccion::Ninguno(m) => assert!(m.contains("no compatible") && m.contains("androidboot.hardware=cutf_cvm"), "{}", m),
            o => panic!("{:?}", o),
        }
        // faltan archivos
        std::fs::remove_file(d.join("super.img")).unwrap();
        match detectar(&cat, &hechos_de_carpeta(&d, None), None) {
            Deteccion::Ninguno(m) => assert!(m.contains("falta super.img"), "{}", m),
            o => panic!("{:?}", o),
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    fn rutas_de(d: &Path) -> Rutas {
        Rutas::resolver(&|k| if k == crate::rutas::VAR_ROOT { Some(d.to_string_lossy().into_owned()) } else { None }, 1).unwrap()
    }

    #[test]
    fn instalar_desde_carpeta_listar_usar_y_quitar() {
        let cat = Catalogo::integrados();
        let base = dir("inst");
        let origen = base.join("origen");
        std::fs::create_dir_all(&origen).unwrap();
        imagen_falsa(&origen, "x86_64", "phone", BC_CF);
        std::fs::write(origen.join(".weft-build"), "16373615\n").unwrap();
        let r = rutas_de(&base.join("raiz"));
        let op = OpcionesAdd { origen: origen.to_string_lossy().into_owned(), id: None, perfil: None };
        let (id, t) = instalar(&r, &cat, &op, &|_| {}).unwrap();
        assert_eq!(id, "phone-x86_64-16373615");
        assert!(t.contains("instalada"), "{}", t);
        assert!(r.imagen(&id).join("image.info").is_file());
        let v = listar(&r, &cat);
        assert_eq!(v.len(), 1);
        assert!(v[0].completa);
        assert_eq!(v[0].info.build, Some(16373615));
        // el mismo id otra vez: ya instalada, sin error ni copia
        let (id2, t2) = instalar(&r, &cat, &op, &|_| {}).unwrap();
        assert_eq!(id2, id);
        assert!(t2.contains("ya esta instalada"), "{}", t2);
        // con otro id explicito se instala aparte
        let op2 = OpcionesAdd { id: Some("copia".into()), ..OpcionesAdd { origen: op.origen.clone(), id: None, perfil: None } };
        assert_eq!(instalar(&r, &cat, &op2, &|_| {}).unwrap().0, "copia");
        assert!(instalar(&r, &cat, &op2, &|_| {}).unwrap_err().contains("ya existe"));
        // una imagen no compatible no deja nada a medias
        let mala = base.join("mala");
        std::fs::create_dir_all(&mala).unwrap();
        imagen_falsa(&mala, "arm64", "phone", BC_CF);
        let e = instalar(&r, &cat, &OpcionesAdd { origen: mala.to_string_lossy().into_owned(), id: None, perfil: None }, &|_| {}).unwrap_err();
        assert!(e.contains("ARM64"), "{}", e);
        assert!(!std::fs::read_dir(r.imagenes()).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with(".partial")));
        // la eleccion de la maquina
        let mut cfg = Config::default();
        assert!(id_de_maquina(&cfg, &listar(&r, &cat)).unwrap_err().contains("varias"));
        cfg.set("image.id", "copia").unwrap();
        assert_eq!(id_de_maquina(&cfg, &listar(&r, &cat)).unwrap(), "copia");
        cfg.set("image.id", "no-esta").unwrap();
        assert!(id_de_maquina(&cfg, &listar(&r, &cat)).unwrap_err().contains("no esta instalada"));
        assert!(id_de_maquina(&Config::default(), &[]).unwrap_err().contains("image add"));
        // quitar: la usa una maquina (config en el directorio de configuracion), tiene disco
        let mut c = Config::default();
        c.set("image.id", "copia").unwrap();
        std::fs::create_dir_all(r.config_maquina("uno").parent().unwrap()).unwrap();
        std::fs::write(r.config_maquina("uno"), c.texto()).unwrap();
        assert_eq!(maquinas_que_usan(&r, "copia"), vec!["uno"]);
        assert!(quitar(&r, &cat, "copia", false).unwrap_err().contains("la usa la maquina uno"));
        std::fs::remove_file(r.config_maquina("uno")).unwrap();
        std::fs::create_dir_all(r.dir_discos("uno")).unwrap();
        // copia en escritura: el disco de "uno" es un overlay sobre la base de la imagen, y otro disco suelto tambien la usa
        let b = r.base_disco("copia", "24G");
        std::fs::create_dir_all(b.parent().unwrap()).unwrap();
        std::fs::File::create(&b).unwrap().set_len(1 << 20).unwrap();
        crate::cow::crear(&b, &r.disco("uno", "copia")).unwrap();
        // (el disco con que arranco la maquina "dos", elegido a mano: su estado de ejecucion lo anota en disk.path)
        let suelto = base.join("suelto.img");
        crate::cow::crear(&b, &suelto).unwrap();
        std::fs::create_dir_all(r.ejecucion.join("dos")).unwrap();
        std::fs::write(r.ejecucion.join("dos").join("disk.path"), format!("{}\n", suelto.display())).unwrap();
        assert_eq!(usuarios_de_bases(&r, &bases_de_imagen(&r, "copia")), vec![r.disco("uno", "copia"), suelto.clone()]);
        let e = quitar(&r, &cat, "copia", true).unwrap_err();
        assert!(e.contains("la usan otros discos") && e.contains("suelto.img"), "{}", e);
        assert!(r.disco("uno", "copia").is_file() && b.is_file() && r.imagen("copia").is_dir());
        std::fs::remove_file(&suelto).unwrap();
        std::fs::write(r.disco("uno", "copia"), b"disco").unwrap();
        crate::cow::crear(&b, &r.disco("uno", "copia")).unwrap();
        assert_eq!(discos_de_imagen(&r, "copia").len(), 1);
        let e = quitar(&r, &cat, "copia", false).unwrap_err();
        assert!(e.contains("BORRA") && e.contains("--yes"), "{}", e);
        assert!(r.disco("uno", "copia").is_file() && r.imagen("copia").is_dir());
        assert!(quitar(&r, &cat, "copia", true).unwrap().contains("1 disco"));
        assert!(!r.imagen("copia").exists() && !r.disco("uno", "copia").exists() && !r.dir_bases("copia").exists());
        assert!(quitar(&r, &cat, "copia", true).unwrap_err().contains("no hay una imagen"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn nombres_y_compilaciones() {
        assert_eq!(build_de_nombre("aosp_cf_x86_64_only_phone-img-16373615.zip"), Some(16373615));
        assert_eq!(build_de_nombre("imagen-3.zip"), None);
        assert_eq!(id_por_defecto("phone-x86_64", Some(5), "x"), "phone-x86_64-5");
        assert_eq!(id_por_defecto("phone-x86_64", None, "Mi imagen (v2)"), "phone-x86_64-Mi-imagen--v2");
        assert_eq!(id_por_defecto("p", None, ""), "p-local");
        assert!(perfil::validar_id(&id_por_defecto("phone-x86_64", None, "Mi imagen (v2)")).is_ok());
        let (n, pre) = prefijo_comun(vec!["x/boot.img".into(), "x/super.img".into()]);
        assert_eq!((n, pre.as_str()), (vec!["boot.img".to_string(), "super.img".to_string()], "x/"));
        let (n, pre) = prefijo_comun(vec!["boot.img".into(), "x/super.img".into()]);
        assert_eq!((n.len(), pre.as_str()), (2, ""));
        let i = InfoImagen { id: "a".into(), perfil: "p".into(), build: Some(7), origen: "zip".into(), bytes: 9, instalada: 3 };
        assert_eq!(InfoImagen::parse(&i.texto()), i);
        assert!(fuente_de("https://x/y.zip").unwrap_err().contains("no descarga nada"));
        assert!(fuente_de("/no/existe/seguro").is_err());
    }

    #[test]
    fn los_textos_del_catalogo_son_genericos() {
        for t in crate::textos::con_modo(true, todos_los_textos) {
            assert!(crate::textos::prohibida_en(&t).is_none(), "{}", t);
        }
    }

    /// Tras `image add`: con `auto` (o una imagen que ya no esta) y la maquina apagada se fija la nueva, asi la maquina no
    /// se queda con "hay varias imagenes instaladas"; si eligio otra a mano no se toca; en marcha, tampoco.
    #[test]
    fn imagen_de_la_maquina_tras_instalar() {
        let hay = |id: &str| id == "vieja" || id == "nueva";
        let apagada = None;
        // apagada: con auto, vacio o una imagen que ya no esta, pasa a la nueva
        assert_eq!(tras_instalar("auto", "nueva", &hay, apagada), TrasInstalar::Fijar);
        assert_eq!(tras_instalar("", "nueva", &hay, apagada), TrasInstalar::Fijar);
        assert_eq!(tras_instalar("borrada", "nueva", &hay, apagada), TrasInstalar::Fijar);
        // elegida a mano: no se toca, ni apagada ni en marcha
        assert_eq!(tras_instalar("vieja", "nueva", &hay, apagada), TrasInstalar::UsaOtra("vieja".into()));
        assert_eq!(tras_instalar("vieja", "nueva", &hay, Some(Some("vieja"))), TrasInstalar::UsaOtra("vieja".into()));
        assert_eq!(tras_instalar("nueva", "nueva", &hay, apagada), TrasInstalar::YaLaUsa);
        assert_eq!(tras_instalar("nueva", "nueva", &hay, Some(None)), TrasInstalar::YaLaUsa);
        // en marcha con auto (el caso de la ventana, que solo existe con la maquina en marcha): sigue con la de ahora
        assert_eq!(tras_instalar("auto", "nueva", &hay, Some(Some("vieja"))), TrasInstalar::MantenerLaDeAhora("vieja".into()));
        // ... salvo que la de ahora sea la misma que se acaba de instalar
        assert_eq!(tras_instalar("auto", "nueva", &hay, Some(Some("nueva"))), TrasInstalar::Fijar);
        // en marcha sin imagen conocida (arranque sin imagen, o la de ahora ya no esta): no se toca
        assert_eq!(tras_instalar("auto", "nueva", &hay, Some(None)), TrasInstalar::EnMarcha);
        assert_eq!(tras_instalar("auto", "nueva", &hay, Some(Some("dir-suelta-1234"))), TrasInstalar::EnMarcha);
        // la clave sin poner vale `auto`
        let cfg = Config::default();
        assert_eq!(tras_instalar(&cfg.get("image.id"), "nueva", &hay, apagada), TrasInstalar::Fijar);
    }

    /// `image profiles` e `image info` muestran la descripcion del perfil (`perfil.descripcion`), que antes se leia y no
    /// se usaba.
    #[test]
    fn la_descripcion_del_perfil_se_muestra() {
        let cat = Catalogo::integrados();
        let base = dir("descripcion");
        let r = rutas_de(&base.join("raiz"));
        let p = cat.buscar("phone-x86_64").unwrap();
        assert!(!p.descripcion.is_empty());
        // image profiles: debajo del nombre de su perfil
        let l = listado_perfiles(&cat, &r);
        let lineas: Vec<&str> = l.lines().collect();
        let i = lineas.iter().position(|x| x.starts_with("phone-x86_64 ")).unwrap();
        assert_eq!(lineas[i + 1].trim(), p.descripcion, "{}", l);
        assert!(lineas[i + 1].starts_with(&" ".repeat(40)), "{}", l);
        // image info
        let origen = base.join("origen");
        std::fs::create_dir_all(&origen).unwrap();
        imagen_falsa(&origen, "x86_64", "phone", BC_CF);
        std::fs::write(origen.join(".weft-build"), "16373615\n").unwrap();
        let (id, _) = instalar(&r, &cat, &OpcionesAdd { origen: origen.to_string_lossy().into_owned(), id: None, perfil: None }, &|_| {}).unwrap();
        let inst = buscar_instalada(&r, &cat, &id).unwrap();
        let d = describir(&r, &cat, &inst);
        assert!(d.contains(&format!("descripcion del perfil: {}\n", p.descripcion)), "{}", d);
        // sin descripcion no queda una linea vacia
        let mut sin = Catalogo::integrados();
        for p in &mut sin.perfiles {
            p.descripcion.clear();
        }
        assert!(!describir(&r, &sin, &inst).contains("descripcion del perfil"));
        assert!(listado_perfiles(&sin, &r).lines().all(|x| !x.trim().is_empty() && !x.starts_with(' ')), "{}", listado_perfiles(&sin, &r));
        let _ = std::fs::remove_dir_all(&base);
    }
}
