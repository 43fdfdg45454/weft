//! Imagen de Android y disco de la maquina: `weft image list|add|path` y `weft disk create|status|reset`, y
//! la seccion "Imagen" de la pantalla de configuracion. weft NO descarga nada: la imagen (zip o carpeta) ya esta en el
//! equipo; un zip se descomprime con una herramienta del sistema (unzip, bsdtar o `python3 -m zipfile`, en ese orden) y se
//! comprueba que esten los archivos que necesita el disco. Quien la baja es el usuario (o el guion de desarrollo).
//!
//! Disco: GPT propio (gpt.rs): misc/metadata/frp vacias, boot/init_boot/vendor_boot/vbmeta* en _a y _b, super y la
//! particion de datos (userdata.img de la imagen, o vacia de N GiB que Android formatea en el primer arranque).
//!
//! Copia en escritura (opcional, `disk.cow=si`, para los discos que arma weft para una maquina; ver cow.rs): ese GPT se arma una sola vez
//! como base de solo lectura (`<datos>/bases/ID/DATOS.img`) y el disco de cada maquina es un overlay qcow2 sobre ella.
//!
//! Este modulo no toca SDL ni la maquina (solo mira si esta en marcha para negarse a borrar un disco en uso).

use crate::gpt;

use std::path::{Path, PathBuf};
use crate::textos::{clave, tx, txf};

const MINIMO_LIBRE: u64 = 6 << 30;

pub type Progreso<'a> = &'a dyn Fn(&str);

/// Pasos de progreso de la descarga y del disco (cada uno con su texto generico de interfaz y su texto de consola).
#[derive(Clone, Debug, PartialEq)]
pub enum Paso {
    /// descomprimiendo con una herramienta: nombre y tamano del zip
    Descomprimiendo(String, u64),
    /// la particion de datos se crea vacia: tamano
    DatosVacia(String),
    /// poco espacio libre: libres y tamano de datos
    PocoEspacio(String, String),
    ArmandoDisco,
    BorrandoDisco,
    /// armando la base compartida (la primera vez para esa imagen y ese tamano de datos)
    ArmandoBase,
    /// creando el overlay de la maquina sobre la base
    CreandoCopia,
}

impl Paso {
    /// Un paso de cada clase, para la prueba que recorre los textos de la interfaz (`todos_los_textos`).
    #[cfg(test)]
    pub fn ejemplos() -> Vec<Paso> {
        vec![
            Paso::Descomprimiendo("unzip".into(), 1200 << 20),
            Paso::DatosVacia("24G".into()),
            Paso::PocoEspacio("10.0 GiB".into(), "24.0 GiB".into()),
            Paso::ArmandoDisco,
            Paso::BorrandoDisco,
            Paso::ArmandoBase,
            Paso::CreandoCopia,
        ]
    }

    pub fn texto(&self) -> String {
        match self {
            Paso::Descomprimiendo(h, n) => txf!("imagen.descomprimiendo_con", h, mib(*n)),
            Paso::DatosVacia(t) => txf!("imagen.la_particion_de_datos_se_crea_vacia", t),
            Paso::PocoEspacio(libres, datos) => txf!("imagen.aviso_quedan_libres_y_la_particion_de", libres, datos),
            Paso::ArmandoDisco => tx!("imagen.armando_el_disco_gpt").into(),
            Paso::BorrandoDisco => tx!("imagen.borrando_el_disco_anterior").into(),
            Paso::ArmandoBase => tx!("imagen.armando_la_base_del_disco_una_sola_vez").into(),
            Paso::CreandoCopia => tx!("imagen.creando_el_disco_de_la_maquina_sobre_la").into(),
        }
    }
}

/// Todos los textos de progreso y de error de este modulo (para la prueba que recorre la interfaz).
#[cfg(test)]
pub(crate) fn todos_los_textos() -> Vec<String> {
    let mut v: Vec<String> = Paso::ejemplos().iter().map(|p| p.texto()).collect();
    v.push(crate::textos::texto(AVISO_REGENERAR).to_string());
    v
}

pub fn mib(b: u64) -> String {
    format!("{:.1} MiB", b as f64 / 1048576.0)
}

pub fn gib(b: u64) -> String {
    format!("{:.1} GiB", b as f64 / 1073741824.0)
}

// ---------------------------------------------------------------------------------------------------------------
// tamano de la particion de datos

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Datos {
    /// usar userdata.img de la imagen
    Img,
    /// particion vacia: (texto normalizado `24G`/`4096M`, bytes)
    Vacia(String, u64),
}

impl Datos {
    pub fn texto(&self) -> String {
        match self {
            Datos::Img => "img".into(),
            Datos::Vacia(t, _) => t.clone(),
        }
    }
}

/// `img`, `24G` o `4096M` (G y M en mayuscula o minuscula). Minimo 2 GiB, maximo 2048 GiB.
pub fn parsear_datos(v: &str) -> Result<Datos, String> {
    let v = v.trim();
    if v.eq_ignore_ascii_case("img") {
        return Ok(Datos::Img);
    }
    let (num, unidad) = match v.chars().last() {
        Some(c) if c.eq_ignore_ascii_case(&'g') => (&v[..v.len() - 1], 'G'),
        Some(c) if c.eq_ignore_ascii_case(&'m') => (&v[..v.len() - 1], 'M'),
        _ => return Err(txf!("imagen.se_espera_img_o_un_tamano_como_24g_o", format!("{:?}", v))),
    };
    let n: u64 = num.trim().parse().map_err(|_| txf!("imagen.se_espera_img_o_un_tamano_como_24g_o", format!("{:?}", v)))?;
    let bytes = n.checked_mul(if unidad == 'G' { 1 << 30 } else { 1 << 20 }).ok_or(tx!("imagen.tamano_demasiado_grande"))?;
    if bytes < 2 << 30 {
        return Err(txf!("imagen.la_particion_de_datos_necesita_al_menos", v));
    }
    if bytes > 2048u64 << 30 {
        return Err(txf!("imagen.como_mucho_2048g", v));
    }
    Ok(Datos::Vacia(format!("{}{}", n, unidad), bytes))
}

pub fn normalizar_datos(v: &str) -> Result<String, String> {
    parsear_datos(v).map(|d| d.texto())
}

// ---------------------------------------------------------------------------------------------------------------
// herramienta de descompresion

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Herramienta {
    Unzip,
    Bsdtar,
    Python,
}

impl Herramienta {
    pub fn nombre(self) -> &'static str {
        match self {
            Herramienta::Unzip => "unzip",
            Herramienta::Bsdtar => "bsdtar",
            Herramienta::Python => "python3",
        }
    }
}

/// La primera disponible en el orden unzip, bsdtar, python3 (como hacia el guion). `hay` dice si un programa esta en el PATH.
pub fn elegir_herramienta(hay: &dyn Fn(&str) -> bool) -> Result<Herramienta, String> {
    for h in [Herramienta::Unzip, Herramienta::Bsdtar, Herramienta::Python] {
        if hay(h.nombre()) {
            return Ok(h);
        }
    }
    Err(tx!("imagen.no_hay_con_que_descomprimir_la_imagen").into())
}

pub fn en_path(nombre: &str) -> bool {
    std::env::var("PATH").is_ok_and(|p| p.split(':').filter(|d| !d.is_empty()).any(|d| Path::new(d).join(nombre).is_file()))
}

/// Programa y argumentos para descomprimir `zip` en `dir`.
pub fn orden_descompresion(h: Herramienta, zip: &Path, dir: &Path) -> (String, Vec<String>) {
    let (z, d) = (zip.to_string_lossy().into_owned(), dir.to_string_lossy().into_owned());
    match h {
        Herramienta::Unzip => ("unzip".into(), vec!["-q".into(), "-o".into(), z, "-d".into(), d]),
        Herramienta::Bsdtar => ("bsdtar".into(), vec!["-xf".into(), z, "-C".into(), d]),
        Herramienta::Python => ("python3".into(), vec!["-m".into(), "zipfile".into(), "-e".into(), z, d]),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// estado de la imagen

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EstadoImagen {
    pub carpeta: PathBuf,
    pub existe: bool,
    /// archivos requeridos que faltan
    pub faltan: Vec<String>,
    /// suma de los tamanos de los archivos .img
    pub bytes: u64,
    /// compilacion anotada en la carpeta (si se conoce)
    pub build: Option<u32>,
}

impl EstadoImagen {
    pub fn completa(&self) -> bool {
        self.existe && self.faltan.is_empty()
    }
}

/// Archivos .img que necesita la imagen (los del perfil por defecto: las carpetas sueltas de `image list --dir`).
fn necesarios() -> Vec<String> {
    perfil_defecto().archivos_requeridos()
}

pub fn estado_imagen(dir: &Path) -> EstadoImagen {
    let mut e = EstadoImagen { carpeta: dir.to_path_buf(), ..Default::default() };
    e.existe = dir.is_dir();
    if !e.existe {
        e.faltan = necesarios();
        return e;
    }
    for f in necesarios() {
        match std::fs::metadata(dir.join(&f)) {
            Ok(m) if m.is_file() => e.bytes += m.len(),
            _ => e.faltan.push(f),
        }
    }
    e.build = crate::catalogo::build_de_carpeta(dir);
    e
}

pub fn listado(e: &EstadoImagen) -> String {
    let mut s = String::new();
    s.push_str(&txf!("imagen.carpeta_2", e.carpeta.display()));
    s.push_str(&txf!("imagen.presente", if e.completa() { "si" } else if e.existe { "incompleta" } else { "no" }));
    s.push_str(&txf!("catalogo.compilacion_2", e.build.map_or(tx!("comun.desconocida").to_string(), |b| b.to_string())));
    s.push_str(&txf!("imagen.tamano_2", mib(e.bytes)));
    if !e.faltan.is_empty() && e.existe {
        s.push_str(&txf!("imagen.faltan", e.faltan.join(", ")));
    }
    if !e.completa() {
        s.push_str(tx!("imagen.para_instalarla_image_add_zip_carpeta_la"));
    }
    s
}

// ---------------------------------------------------------------------------------------------------------------
// espacio libre

extern "C" {
    fn statvfs(path: *const std::os::raw::c_char, buf: *mut u64) -> i32;
}

/// Espacio libre (bytes) en el sistema de archivos que contiene `dir` (o su primer ancestro existente).
pub fn espacio_libre(dir: &Path) -> Option<u64> {
    let mut d = dir.to_path_buf();
    while !d.exists() {
        if !d.pop() {
            return None;
        }
    }
    let c = std::ffi::CString::new(if d.as_os_str().is_empty() { ".".to_string() } else { d.to_string_lossy().into_owned() }).ok()?;
    let mut b = [0u64; 16];
    if unsafe { statvfs(c.as_ptr(), b.as_mut_ptr()) } != 0 {
        return None;
    }
    Some(b[1].saturating_mul(b[4]))
}

// ---------------------------------------------------------------------------------------------------------------
// disco

/// Particiones del disco con el perfil por defecto (para las pruebas).
#[cfg(test)]
pub fn partes(imagen: &Path, datos: &Datos) -> Result<Vec<gpt::Part>, String> {
    partes_de(&perfil_defecto(), imagen, datos)
}

/// El perfil que se usa cuando no se detecta ninguno (el movil integrado).
pub fn perfil_defecto() -> crate::perfil::Perfil {
    crate::perfil::Catalogo::integrados().buscar(crate::perfil::ID_DEFECTO).expect("perfil integrado por defecto").clone()
}

/// Particiones del disco en el orden del perfil.
pub fn partes_de(p: &crate::perfil::Perfil, imagen: &Path, datos: &Datos) -> Result<Vec<gpt::Part>, String> {
    p.partes(imagen, match datos {
        Datos::Img => None,
        Datos::Vacia(t, _) => Some(t.as_str()),
    })
    .map_err(|e| txf!("imagen.image_add", e))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EstadoDisco {
    pub ruta: PathBuf,
    pub existe: bool,
    pub bytes: u64,
    /// tamano de la particion `userdata` leido de la tabla GPT
    pub datos: Option<u64>,
    pub error: Option<String>,
}

/// Tabla GPT de un disco: (nombre, primer sector, ultimo sector).
pub fn leer_particiones(ruta: &Path) -> Result<Vec<(String, u64, u64)>, String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(ruta).map_err(|e| format!("{}: {}", ruta.display(), e))?;
    let mut h = [0u8; 92];
    f.seek(SeekFrom::Start(gpt::SECTOR)).and_then(|_| f.read_exact(&mut h)).map_err(|e| txf!("imagen.cabecera_gpt", e))?;
    if &h[..8] != b"EFI PART" { // texto-interno: firma de la cabecera GPT
        return Err(tx!("imagen.el_disco_no_tiene_una_tabla_gpt").into());
    }
    let u64a = |o: usize| u64::from_le_bytes(h[o..o + 8].try_into().unwrap());
    let u32a = |o: usize| u32::from_le_bytes(h[o..o + 4].try_into().unwrap());
    let (lba, n, tam) = (u64a(72), u32a(80) as usize, u32a(84) as usize);
    if !(128..=1024).contains(&tam) || !(1..=4096).contains(&n) {
        return Err(tx!("imagen.tabla_gpt_con_valores_raros").into());
    }
    let inicio = lba.checked_mul(gpt::SECTOR).ok_or_else(|| txf!("imagen.tabla_de_particiones_invalida_las", lba))?;
    let mut ent = vec![0u8; n * tam];
    f.seek(SeekFrom::Start(inicio)).and_then(|_| f.read_exact(&mut ent)).map_err(|e| txf!("imagen.entradas_gpt", e))?;
    let mut v = Vec::new();
    for e in ent.chunks(tam) {
        if e[..16].iter().all(|b| *b == 0) {
            continue;
        }
        let (a, b) = (u64::from_le_bytes(e[32..40].try_into().unwrap()), u64::from_le_bytes(e[40..48].try_into().unwrap()));
        let nombre = String::from_utf16_lossy(&e[56..128].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|c| *c != 0).collect::<Vec<u16>>());
        if bytes_de_particion(a, b).is_none() {
            return Err(txf!("imagen.tabla_de_particiones_invalida_la", format!("{:?}", nombre), a, b));
        }
        v.push((nombre, a, b));
    }
    Ok(v)
}

/// Bytes que ocupa una particion del sector `primero` al `ultimo` (ambos incluidos). None si la entrada no tiene sentido:
/// termina antes de empezar o acaba mas alla del ultimo byte que se puede direccionar con 64 bits. Pura.
pub fn bytes_de_particion(primero: u64, ultimo: u64) -> Option<u64> {
    // si el final en bytes cabe, el tamano tambien
    ultimo.checked_add(1)?.checked_mul(gpt::SECTOR)?;
    Some((ultimo.checked_sub(primero)? + 1) * gpt::SECTOR)
}

/// Estado del disco. Un overlay de copia en escritura (cow.rs) da su tamano virtual y la tabla GPT de su base.
pub fn estado_disco(ruta: &Path) -> EstadoDisco {
    let mut e = EstadoDisco { ruta: ruta.to_path_buf(), ..Default::default() };
    let Ok(m) = std::fs::metadata(ruta) else { return e };
    e.existe = m.is_file();
    e.bytes = m.len();
    let mut gpt_de = ruta.to_path_buf();
    if let Some(c) = crate::cow::cabecera(ruta) {
        e.bytes = c.bytes;
        match c.respaldo {
            Some(b) => gpt_de = b,
            None => {
                e.error = Some(tx!("imagen.disco_qcow2_sin_base").into());
                return e;
            }
        }
    }
    if e.existe {
        match leer_particiones(&gpt_de) {
            Ok(p) => e.datos = p.iter().find(|x| x.0 == "userdata").and_then(|x| bytes_de_particion(x.1, x.2)),
            Err(er) => e.error = Some(er),
        }
    }
    e
}

pub fn resumen_disco(e: &EstadoDisco) -> String {
    if !e.existe {
        return txf!("imagen.disco_no_existe_disk_create", e.ruta.display());
    }
    let mut s = txf!("imagen.disco_tamano", e.ruta.display(), gib(e.bytes));
    if let Some(b) = crate::cow::cabecera(&e.ruta).and_then(|c| c.respaldo) {
        let b = crate::rutas::normalizar(&b);
        s.push_str(&txf!("imagen.copia_en_escritura_sobre_la_base_solo", b.display(), if b.is_file() { "" } else { tx!("imagen.aviso_no_existe") }, gib(crate::cow::ocupado(&e.ruta))));
    }
    match (e.datos, &e.error) {
        (Some(d), _) => s.push_str(&txf!("imagen.particion_de_datos_userdata", gib(d))),
        (None, Some(er)) => s.push_str(&txf!("imagen.aviso", er)),
        _ => s.push_str(tx!("imagen.aviso_el_disco_no_tiene_particion")),
    }
    s
}

/// Crea el disco. Si ya existe no lo toca (Ok con aviso). Devuelve (texto, creado). `base`: Some = disco de copia en
/// escritura (un overlay qcow2 sobre esa base, que se arma si falta; ver cow.rs); None = el disco GPT completo (raw).
pub fn crear_disco(perfil: &crate::perfil::Perfil, imagen: &Path, datos: &Datos, out: &Path, base: Option<&Path>, prog: Progreso) -> Result<(String, bool), String> {
    if out.exists() {
        let e = estado_disco(out);
        return Ok((txf!("imagen.el_disco_ya_existe_datos_se_queda_como", out.display(), gib(e.bytes), e.datos.map_or("?".to_string(), gib)), false));
    }
    let Some(base) = base else { return armar_disco(perfil, imagen, datos, out, prog).map(|t| (t, true)) };
    if !base.exists() {
        if let Some(d) = base.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {}", d.display(), e))?;
        }
        prog(&Paso::ArmandoBase.texto());
        armar_base(perfil, imagen, datos, base, prog)?;
    }
    prog(&Paso::CreandoCopia.texto());
    crate::cow::crear(base, out)?;
    Ok((txf!("imagen.disco_creado_copia_en_escritura_sobre_la", out.display(), base.display(), gib(estado_disco(out).bytes)), true))
}

/// Arma la base de los discos de copia en escritura en `base`: el disco GPT completo, en un temporal propio de este proceso
/// que se enlaza con el nombre final solo si no existe (otro arranque a la vez pudo armarla antes: entonces se usa la suya,
/// que es igual) y queda en solo lectura (0444): QEMU la abre en solo lectura y nadie la cambia por descuido.
fn armar_base(perfil: &crate::perfil::Perfil, imagen: &Path, datos: &Datos, base: &Path, prog: Progreso) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let tmp = base.with_extension(format!("img.parte-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let r = armar_disco(perfil, imagen, datos, &tmp, prog)
        .and_then(|_| std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o444)).map_err(|e| format!("{}: {}", tmp.display(), e)))
        .and_then(|_| match std::fs::hard_link(&tmp, base) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(format!("{}: {}", base.display(), e)),
        });
    let _ = std::fs::remove_file(&tmp);
    r
}

/// Arma el disco GPT completo en `out` (via un temporal que se renombra al final). Devuelve el texto del resultado.
fn armar_disco(perfil: &crate::perfil::Perfil, imagen: &Path, datos: &Datos, out: &Path, prog: Progreso) -> Result<String, String> {
    let p = partes_de(perfil, imagen, datos)?;
    if let Some(libre) = espacio_libre(out.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."))) {
        if libre < MINIMO_LIBRE {
            return Err(txf!("imagen.espacio_insuficiente_para_el_disco", gib(libre), gib(MINIMO_LIBRE)));
        }
        if let Datos::Vacia(_, b) = datos {
            if libre < *b {
                prog(&Paso::PocoEspacio(gib(libre), gib(*b)).texto());
            }
        }
    }
    if let Datos::Vacia(t, _) = datos {
        prog(&Paso::DatosVacia(t.clone()).texto());
    }
    prog(&Paso::ArmandoDisco.texto());
    let tmp = out.with_extension("img.parte");
    let (total, escrito) = match gpt::assemble(&tmp.to_string_lossy(), &p) {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    };
    std::fs::rename(&tmp, out).map_err(|e| format!("{}: {}", out.display(), e))?;
    Ok(txf!("imagen.disco_creado_en_total_con_datos_escritos", out.display(), gib(total), mib(escrito), p.len()))
}

/// Borra y regenera el disco (volver a fabrica). Se niega si hay una maquina en marcha de este estado (usa el disco). Las
/// apps y los datos se pierden. Con `base` el disco nuevo es un overlay vacio sobre ella (la base no se toca: es un
/// instante); un disco anterior completo (raw) tambien pasa a overlay.
pub fn regenerar_disco(perfil: &crate::perfil::Perfil, imagen: &Path, datos: &Datos, out: &Path, base: Option<&Path>, en_marcha: bool, prog: Progreso) -> Result<String, String> {
    if en_marcha {
        return Err(tx!("imagen.hay_una_maquina_en_marcha_con_este_disco").into());
    }
    if out.exists() {
        // se comprueba antes de borrar que se podra rearmar
        partes_de(perfil, imagen, datos)?;
        prog(&Paso::BorrandoDisco.texto());
        std::fs::remove_file(out).map_err(|e| format!("{}: {}", out.display(), e))?;
    }
    crear_disco(perfil, imagen, datos, out, base, prog).map(|r| r.0)
}

/// Aviso de lo que se pierde al regenerar, para el CLI y la pantalla.
pub const AVISO_REGENERAR: &str = clave!("imagen.regenerar_el_disco_borra_las_apps");

/// Imagen y disco de la maquina de un estado (para la pantalla de configuracion): la imagen que uso al arrancar (`image-id` del
/// estado) o la de su config (`image.id`, o la unica instalada); su carpeta en `<datos>/images/`; el disco con que arranco
/// (`disk.path` del estado, que escribe `start`) o el de esa imagen para la maquina (`<datos>/machines/<m>/disks/<id>.img`).
/// Devuelve (carpeta de la imagen, disco).
pub fn rutas_de_la_maquina(estado: &Path) -> (PathBuf, PathBuf) {
    let cfg = crate::config::Config::cargar(estado);
    let r = crate::rutas::actual().con_claves(&cfg.get("dir.datos"), &cfg.get("dir.cache"));
    let maquina = crate::rutas::maquina_de(estado);
    let usada = std::fs::read_to_string(estado.join("image-id")).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    let id = usada.or_else(|| {
        let cat = crate::catalogo::cargar_catalogo(&r);
        crate::catalogo::id_de_maquina(&cfg, &crate::catalogo::listar(&r, &cat)).ok()
    });
    let id = id.unwrap_or_else(|| "none".to_string());
    let disco = std::fs::read_to_string(estado.join("disk.path")).ok().map(|t| PathBuf::from(t.trim())).filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| r.disco(&maquina, &id));
    (r.imagen(&id), disco)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir_prueba(n: &str) -> PathBuf {
        let d = PathBuf::from(format!("{}/imagen-{}-{}", std::env::var("TMPDIR").unwrap_or_else(|_| ".".into()), n, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn imagen_falsa(d: &Path) {
        for f in necesarios() {
            std::fs::write(d.join(&f), format!("contenido de {}", f)).unwrap();
        }
    }

    #[test]
    fn tamanos_de_datos() {
        assert_eq!(parsear_datos("24G"), Ok(Datos::Vacia("24G".into(), 24 << 30)));
        assert_eq!(parsear_datos(" 4g "), Ok(Datos::Vacia("4G".into(), 4 << 30)));
        assert_eq!(parsear_datos("4096M"), Ok(Datos::Vacia("4096M".into(), 4096 << 20)));
        assert_eq!(parsear_datos("IMG"), Ok(Datos::Img));
        assert_eq!(normalizar_datos("img").unwrap(), "img");
        assert_eq!(normalizar_datos("16g").unwrap(), "16G");
        for malo in ["", "24", "G", "24T", "1G", "1024M", "0G", "2049G", "-4G", "4,5G", "veinteG"] {
            assert!(parsear_datos(malo).is_err(), "{:?}", malo);
        }
        assert!(parsear_datos("2G").is_ok() && parsear_datos("2048G").is_ok());
    }

    #[test]
    fn eleccion_de_la_herramienta() {
        let solo = |n: &'static str| move |x: &str| x == n;
        assert_eq!(elegir_herramienta(&|_| true), Ok(Herramienta::Unzip));
        assert_eq!(elegir_herramienta(&|x| x != "unzip"), Ok(Herramienta::Bsdtar));
        assert_eq!(elegir_herramienta(&solo("python3")), Ok(Herramienta::Python));
        let e = elegir_herramienta(&|_| false).unwrap_err();
        assert!(e.contains("unzip") && e.contains("bsdtar") && e.contains("python3"));
        let (p, a) = orden_descompresion(Herramienta::Unzip, Path::new("/t/z.zip"), Path::new("/t/imagen"));
        assert_eq!((p.as_str(), a.join(" ")), ("unzip", "-q -o /t/z.zip -d /t/imagen".to_string()));
        let (p, a) = orden_descompresion(Herramienta::Bsdtar, Path::new("z"), Path::new("d"));
        assert_eq!((p.as_str(), a.join(" ")), ("bsdtar", "-xf z -C d".to_string()));
        let (p, a) = orden_descompresion(Herramienta::Python, Path::new("z"), Path::new("d"));
        assert_eq!((p.as_str(), a.join(" ")), ("python3", "-m zipfile -e z d".to_string()));
    }

    #[test]
    fn estado_de_la_imagen() {
        let d = dir_prueba("estado");
        let vacia = estado_imagen(&d);
        assert!(vacia.existe && !vacia.completa() && vacia.faltan.len() == necesarios().len() && vacia.build.is_none());
        let ausente = estado_imagen(&d.join("no-hay"));
        assert!(!ausente.existe && !ausente.completa());
        imagen_falsa(&d);
        std::fs::write(d.join(crate::rutas::nombres::MARCA_BUILD), "16373615\n").unwrap();
        let e = estado_imagen(&d);
        assert!(e.completa() && e.build == Some(16373615) && e.bytes > 0, "{:?}", e);
        assert!(listado(&e).contains("presente: si"));
        std::fs::remove_file(d.join("super.img")).unwrap();
        let e = estado_imagen(&d);
        assert_eq!(e.faltan, vec!["super.img".to_string()]);
        assert!(listado(&e).contains("faltan: super.img") && listado(&e).contains("weft image add"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn el_disco_se_arma_como_el_guion() {
        let d = dir_prueba("disco");
        imagen_falsa(&d);
        let p = partes(&d, &Datos::Vacia("2G".into(), 2 << 30)).unwrap();
        let nombres: Vec<&str> = p.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(&nombres[..3], &["misc", "metadata", "frp"]);
        assert_eq!(nombres.len(), 3 + 14 + 2);
        assert!(nombres.contains(&"vbmeta_vendor_dlkm_b") && nombres.contains(&"boot_a") && nombres[nombres.len() - 2..] == ["super", "userdata"]);
        assert_eq!(p.last().unwrap().bytes, 2 << 30);
        assert!(p.last().unwrap().source.is_none() && p[nombres.len() - 2].source.is_some());
        // con img, la particion de datos sale del archivo
        let pi = partes(&d, &Datos::Img).unwrap();
        assert!(pi.last().unwrap().source.as_deref().unwrap().ends_with("userdata.img"));
        // sin un archivo: error claro
        std::fs::remove_file(d.join("vbmeta_system.img")).unwrap();
        assert!(partes(&d, &Datos::Img).unwrap_err().contains("vbmeta_system.img"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn crear_estado_y_regenerar_el_disco() {
        let d = dir_prueba("ciclo");
        imagen_falsa(&d);
        let out = d.join("disco.img");
        let datos = Datos::Vacia("2G".into(), 2 << 30);
        assert!(!estado_disco(&out).existe);
        let (t, creado) = crear_disco(&perfil_defecto(), &d, &datos, &out, None, &|_| {}).unwrap();
        assert!(creado && t.contains("creado"), "{}", t);
        let e = estado_disco(&out);
        assert!(e.existe && e.datos == Some(2 << 30) && e.error.is_none(), "{:?}", e);
        let parts = leer_particiones(&out).unwrap();
        assert!(parts.iter().any(|p| p.0 == "super") && parts.iter().any(|p| p.0 == "boot_a") && parts.len() == 19);
        assert!(resumen_disco(&e).contains("2.0 GiB"));
        // otra vez: no toca nada
        let (t, creado) = crear_disco(&perfil_defecto(), &d, &Datos::Vacia("3G".into(), 3 << 30), &out, None, &|_| {}).unwrap();
        assert!(!creado && t.contains("ya existe") && estado_disco(&out).datos == Some(2 << 30));
        // regenerar con una maquina en marcha: se niega y no borra
        assert!(regenerar_disco(&perfil_defecto(), &d, &datos, &out, None, true, &|_| {}).unwrap_err().contains("en marcha") && out.exists());
        // sin la imagen completa no borra el disco
        std::fs::remove_file(d.join("boot.img")).unwrap();
        assert!(regenerar_disco(&perfil_defecto(), &d, &datos, &out, None, false, &|_| {}).is_err() && out.exists());
        std::fs::write(d.join("boot.img"), b"x").unwrap();
        let t = regenerar_disco(&perfil_defecto(), &d, &Datos::Vacia("3G".into(), 3 << 30), &out, None, false, &|_| {}).unwrap();
        assert!(t.contains("creado") && estado_disco(&out).datos == Some(3 << 30));
        assert!(!d.join("disco.img.parte").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Copia en escritura: la base se arma una vez (solo lectura) y cada disco es un overlay sobre ella; volver a fabrica
    /// crea otro overlay vacio sin tocar la base; un disco completo anterior tambien pasa a overlay.
    #[test]
    fn disco_de_copia_en_escritura() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir_prueba("cow");
        imagen_falsa(&d);
        let datos = Datos::Vacia("2G".into(), 2 << 30);
        let base = d.join("bases/img/2G.img");
        let (a, b) = (d.join("m1/a.img"), d.join("m2/b.img"));
        std::fs::create_dir_all(a.parent().unwrap()).unwrap();
        std::fs::create_dir_all(b.parent().unwrap()).unwrap();
        let pasos = std::cell::RefCell::new(Vec::new());
        let prog = |t: &str| pasos.borrow_mut().push(t.to_string());
        let (t, creado) = crear_disco(&perfil_defecto(), &d, &datos, &a, Some(&base), &prog).unwrap();
        assert!(creado && t.contains("copia en escritura"), "{}", t);
        assert!(pasos.borrow().iter().any(|p| p.contains("armando la base")));
        assert_eq!(std::fs::metadata(&base).unwrap().permissions().mode() & 0o777, 0o444);
        assert!(crate::cow::usa_base(&a, &base));
        let e = estado_disco(&a);
        assert!(e.existe && e.datos == Some(2 << 30) && e.bytes == std::fs::metadata(&base).unwrap().len() && e.error.is_none(), "{:?}", e);
        assert!(resumen_disco(&e).contains("copia en escritura sobre la base"), "{}", resumen_disco(&e));
        // la segunda maquina reutiliza la base (no la arma otra vez)
        pasos.borrow_mut().clear();
        crear_disco(&perfil_defecto(), &d, &datos, &b, Some(&base), &prog).unwrap();
        assert!(!pasos.borrow().iter().any(|p| p.contains("armando la base")) && crate::cow::usa_base(&b, &base));
        let antes = std::fs::metadata(&base).unwrap().modified().unwrap();
        // volver a fabrica: overlay nuevo, base intacta
        std::fs::write(&a, b"lo que el invitado escribio").unwrap();
        regenerar_disco(&perfil_defecto(), &d, &datos, &a, Some(&base), false, &|_| {}).unwrap();
        assert!(crate::cow::usa_base(&a, &base) && std::fs::metadata(&base).unwrap().modified().unwrap() == antes);
        // un disco completo (raw) de antes sigue valiendo y, al volver a fabrica, pasa a overlay
        let c = d.join("m1/c.img");
        crear_disco(&perfil_defecto(), &d, &datos, &c, None, &|_| {}).unwrap();
        assert!(crate::cow::cabecera(&c).is_none() && estado_disco(&c).datos == Some(2 << 30));
        regenerar_disco(&perfil_defecto(), &d, &datos, &c, Some(&base), false, &|_| {}).unwrap();
        assert!(crate::cow::usa_base(&c, &base));
        assert!(std::fs::read_dir(base.parent().unwrap()).unwrap().count() == 1, "quedaron temporales junto a la base");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn disco_ilegible_o_ausente() {
        let d = dir_prueba("malo");
        let e = estado_disco(&d.join("no.img"));
        assert!(!e.existe && resumen_disco(&e).contains("no existe"));
        std::fs::write(d.join("raro.img"), vec![0u8; 4096]).unwrap();
        let e = estado_disco(&d.join("raro.img"));
        assert!(e.existe && e.datos.is_none() && e.error.is_some());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Una tabla GPT con valores en el limite de 64 bits (entradas fuera de cualquier disco, particiones al reves o que
    /// ocupan todo el espacio de sectores) da "tabla de particiones inválida" en vez de desbordar.
    #[test]
    fn tabla_gpt_con_valores_limite() {
        assert_eq!(bytes_de_particion(2048, 4095), Some(2048 * 512));
        assert_eq!(bytes_de_particion(7, 7), Some(512));
        assert_eq!(bytes_de_particion(10, 9), None);
        assert_eq!(bytes_de_particion(0, u64::MAX), None);
        assert_eq!(bytes_de_particion(0, u64::MAX / 512 - 1), Some(u64::MAX / 512 * 512));
        assert_eq!(bytes_de_particion(0, u64::MAX / 512), None);

        let d = dir_prueba("gpt-limite");
        // MBR vacio, cabecera en el sector 1 y una sola entrada de 128 bytes en el sector `lba`
        let disco = |lba: u64, primero: u64, ultimo: u64| {
            let mut v = vec![0u8; 4 * 512];
            v[512..520].copy_from_slice(b"EFI PART");
            v[512 + 72..512 + 80].copy_from_slice(&lba.to_le_bytes());
            v[512 + 80..512 + 84].copy_from_slice(&1u32.to_le_bytes());
            v[512 + 84..512 + 88].copy_from_slice(&128u32.to_le_bytes());
            let e = 1024;
            v[e] = 0xAF; // tipo distinto de cero: entrada en uso
            v[e + 32..e + 40].copy_from_slice(&primero.to_le_bytes());
            v[e + 40..e + 48].copy_from_slice(&ultimo.to_le_bytes());
            for (k, u) in "userdata".encode_utf16().enumerate() {
                v[e + 56 + 2 * k..e + 58 + 2 * k].copy_from_slice(&u.to_le_bytes());
            }
            let ruta = d.join(format!("gpt-{}-{}-{}.img", lba, primero, ultimo));
            std::fs::write(&ruta, &v).unwrap();
            ruta
        };
        // valida: la particion de datos se mide bien
        let bien = disco(2, 2048, 4095);
        assert_eq!(leer_particiones(&bien).unwrap(), vec![("userdata".to_string(), 2048, 4095)]);
        assert_eq!(estado_disco(&bien).datos, Some(2048 * 512));
        for (lba, primero, ultimo) in [(u64::MAX, 1, 2), (u64::MAX / 512 + 1, 1, 2), (2, 10, 9), (2, 0, u64::MAX), (2, u64::MAX, u64::MAX)] {
            let r = disco(lba, primero, ultimo);
            let err = leer_particiones(&r).unwrap_err();
            assert!(err.contains("tabla de particiones inválida"), "{} {} {}: {}", lba, primero, ultimo, err);
            let e = estado_disco(&r);
            assert!(e.existe && e.datos.is_none() && e.error.as_deref() == Some(err.as_str()), "{:?}", e);
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rutas_de_la_maquina_del_estado() {
        let d = dir_prueba("rutas");
        // sin nada: la imagen "none" y el disco de esa imagen, en la carpeta de datos estandar (nada se crea)
        let (i, k) = rutas_de_la_maquina(&d);
        assert!(i.ends_with("images/none") && k.to_string_lossy().contains("machines/") && k.ends_with("disks/none.img"), "{:?} {:?}", i, k);
        // con lo que dejo `start`: la imagen usada y el disco con que arranco
        std::fs::write(d.join("image-id"), "phone-x86_64-1\n").unwrap();
        std::fs::write(d.join("disk.path"), "/datos/maquina/disk.img\n").unwrap();
        let (i, k) = rutas_de_la_maquina(&d);
        assert!(i.ends_with("images/phone-x86_64-1"));
        assert_eq!(k, PathBuf::from("/datos/maquina/disk.img"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn aviso_de_regenerar_dice_que_se_pierde() {
        let aviso = crate::textos::texto(AVISO_REGENERAR);
        assert!(aviso.contains("BORRA") && aviso.contains("apps") && aviso.contains("datos"));
    }
}
