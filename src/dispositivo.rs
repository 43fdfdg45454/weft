//! Perfiles de dispositivo: lo que la maquina ANUNCIA a Android y a las apps ARM (CPU, extensiones, identidad del producto,
//! tamano de pagina) y los recursos de la maquina virtual (nucleos y memoria). No confundir con los perfiles de IMAGEN
//! (`perfil.rs`: como se arranca una familia de imagenes).
//!
//! DECISION: el perfil describe lo que se anuncia, no lo que el traductor ARM implementa. weft no comprueba ni corrige el
//! perfil contra el traductor instalado: se puede crear cualquiera (una extension anunciada y no implementada hara que la
//! app que la use reciba SIGILL, como en un procesador real sin ella). weft acepta cualquier traductor: no lleva una lista
//! propia de lo que implementa. Si el traductor instalado PUBLICA que extensiones soporta (archivo `cpu-features` junto a
//! su biblioteca, ver `CPU_FEATURES`), `weft bridge install` lo copia al estado de la maquina y la pantalla y `device show`
//! avisan (solo informativo) de las anunciadas que no estan; sin ese archivo no hay aviso.
//!
//! FORMATO (`*.device`, clave=valor, `#` solo al principio de linea): `dispositivo.id`, `dispositivo.nombre`,
//! `dispositivo.descripcion`, `dispositivo.hereda` (toma todas las claves de otro perfil y las de este las sustituyen),
//! `producto.fabricante|marca|modelo|dispositivo|nombre` (`ro.product.manufacturer|brand|model|device|name`; vacio = el de
//! la imagen), `maquina.nucleos` y `maquina.ram` (MiB; `auto` = lo que diga la configuracion o el arranque), `pagina` (KiB
//! anunciados a las apps ARM: 4 o 16) y la CPU: `cpu.nombre`, `cpu.hardware`, `cpu.base`, `cpu.midr`, `cpu.revidr`,
//! `cpu.ctr`, `cpu.dczid`, `cpu.features` (nombres de `/proc/cpuinfo` de Linux arm64 separados por espacios o comas: una
//! lista absoluta, o relativa a `cpu.base` si todos llevan `+` o `-`) y los registros `cpu.id_aa64pfr0`... (64 bits; ver
//! `REGISTROS`). Una clave o un valor desconocido rechaza el
//! perfil con su numero de linea. Hay perfiles integrados (`perfiles/dispositivos/*.device`, incrustados) y del usuario
//! en `$XDG_CONFIG_HOME/weft/devices/` (mismo id: manda el del usuario).
//!
//! CONTRATO CON EL TRADUCTOR (docs/perfil-cpu.md de heddle): `/system/etc/heddle/cpu.conf` (clave=valor, `#`
//! comentarios) con `nombre`, `hardware`, `base`, `midr`, `revidr`, `ctr`, `dczid`, `features` (lista con comas, con o sin
//! `+`/`-`) y `id_aa64*`; solo se escriben las que el perfil define (lo que falta lo pone la base, Cortex-A78 si no hay).
//! El traductor anuncia exactamente eso aunque no lo implemente (registra en logcat cuales no implementa) y lo lee una vez
//! por proceso, al crear el primer hilo ARM.
//!
//! IDENTIDAD: las lineas `ro.product.<campo>` y `ro.product.<particion>.<campo>` que ya hay en los build.prop de la imagen
//! (`ARCHIVOS_PROP`) se SUSTITUYEN en su sitio por las del perfil (solo los campos que el perfil define; `ro.product.cpu.*`
//! y las demas no se tocan); las que la imagen no tiene se agregan al final del build.prop de su particion (las generales,
//! al de product o, sin el, al de system). Ahi mismo van `debug.heddle.page_size` y la marca `ro.weft.dispositivo=ID:FIRMA`,
//! que permite saber con un solo `getprop` (sin root) si Android ya arranco con el perfil elegido. Las lineas originales
//! de cada clave que weft cambio o agrego quedan en `<archivo>.weft-orig` (`EXT_ORIG`), en el propio invitado (viven y
//! mueren con el disco que modifican): quitar el perfil o cambiarlo por otro las repone exactamente antes de nada. Sin
//! validar aun en Android real (ver README).
//!
//! RECURSOS (precedencia): `--cpus`/`--mem` > `maquina.cpus`/`maquina.ram` de la configuracion (si no son `auto`) >
//! `maquina.nucleos`/`maquina.ram` del perfil de dispositivo > el defecto del arranque (2/2048 con `start`, 4/4096 con
//! `launch`).
//!
//! APLICAR: `aplicar` (vigilado como el traductor: si Android no vuelve a arrancar, restaura los archivos anteriores) trabaja
//! sobre el rasgo `Invitado`, y se prueba con un invitado simulado. Nada de esto corre en caliente: se lee al arrancar la
//! maquina, al abrir la seccion de la configuracion y tras cada arranque de Android (un `getprop`).

use crate::config::Config;
use crate::perfil::Origen;
use crate::textos::{elige, tx, txf};
use std::path::{Path, PathBuf};

pub const FORMATO: u32 = 1;
/// Valor de `dispositivo.perfil` sin perfil: Android con la identidad y la CPU que traen la imagen y el traductor.
pub const NINGUNO: &str = "ninguno";
/// Extension de los archivos de perfil de dispositivo.
pub const EXT: &str = "device";
pub const CPU_CONF: &str = "/system/etc/heddle/cpu.conf";
pub const CPU_CONF_DIR: &str = "/system/etc/heddle";
pub const PROP_PRODUCT: &str = "/product/etc/build.prop";
pub const PROP_SYSTEM: &str = "/system/build.prop";
/// build.prop de cada particion que init lee (Android 10+), en su orden: (particion, archivo). `bootimage` no tiene uno
/// escribible (va en el ramdisk): sus `ro.product.bootimage.*` no se agregan.
pub const ARCHIVOS_PROP: [(&str, &str); 8] = [
    ("system", PROP_SYSTEM),
    ("system_ext", "/system_ext/etc/build.prop"),
    ("vendor", "/vendor/build.prop"),
    ("odm", "/odm/etc/build.prop"),
    ("vendor_dlkm", "/vendor_dlkm/etc/build.prop"),
    ("odm_dlkm", "/odm_dlkm/etc/build.prop"),
    ("system_dlkm", "/system_dlkm/etc/build.prop"),
    ("product", PROP_PRODUCT),
];
/// Sufijo del archivo con las lineas originales que weft cambio en un build.prop (junto a el, en el invitado).
pub const EXT_ORIG: &str = ".weft-orig";
/// Marcas del bloque de la version anterior de este modulo (se quita si se encuentra).
const MARCA_INICIO: &str = "# weft-dispositivo: inicio";
const MARCA_FIN: &str = "# weft-dispositivo: fin";
pub const PROP_MARCA: &str = "ro.weft.dispositivo";
pub const PROP_PAGINA: &str = "debug.heddle.page_size";
/// Archivos temporales en el invitado (se copian con `cat` sobre el destino para conservar dueno, modo y etiqueta).
const TMP: &str = "/data/local/tmp/weft-dispositivo";
/// Particiones con su propia copia de `ro.product.<particion>.*` (Android 10+).
pub const PARTICIONES: [&str; 9] = ["bootimage", "odm", "odm_dlkm", "product", "system", "system_dlkm", "system_ext", "vendor", "vendor_dlkm"];
/// Archivo de texto que un traductor ARM puede traer junto a su biblioteca (mismo directorio que el .so) con las
/// extensiones que soporta: nombres como los de la linea Features de /proc/cpuinfo de Linux, separados por espacios o
/// saltos de linea, `#` comentario hasta el final de la linea.
pub const CPU_FEATURES: &str = "cpu-features";
/// Copia de `CPU_FEATURES` del traductor instalado en el estado de la maquina (la pone `weft bridge install`).
pub const ARCHIVO_SOPORTADAS: &str = "traductor-cpu-features";
/// Limite de espera de `sys.boot_completed` tras aplicar.
pub const LIMITE_ARRANQUE_S: u64 = 120;
const ESPERA_INICIAL_S: u64 = 300;
const ESPERA_RESTAURAR_S: u64 = 240;
const SONDEO_S: u64 = 5;
/// Archivo del estado de la maquina con la marca que fallo al aplicarse: la sincronizacion tras el arranque no la reintenta
/// (si no, un perfil que impide arrancar reiniciaria Android sin fin).
pub const ARCHIVO_FALLO: &str = "dispositivo-fallo";

/// Extensiones de `/proc/cpuinfo` de Linux 6.6 arm64 (`hwcap_str`), en el orden de los bits de AT_HWCAP y AT_HWCAP2: las
/// que el traductor sabe anunciar (sin las de SME que dependen de ID_AA64SMFR0_EL1). Para mostrarlas y ordenarlas: un
/// perfil puede nombrar otras (el traductor las ignora con un aviso en logcat).
pub const EXTENSIONES: &[&str] = &[
    "fp", "asimd", "evtstrm", "aes", "pmull", "sha1", "sha2", "crc32", "atomics", "fphp", "asimdhp", "cpuid", "asimdrdm", "jscvt", "fcma", "lrcpc", "dcpop", "sha3", "sm3", "sm4", "asimddp", "sha512", "sve",
    "asimdfhm", "dit", "uscat", "ilrcpc", "flagm", "ssbs", "sb", "paca", "pacg", "dcpodp", "sve2", "sveaes", "svepmull", "svebitperm", "svesha3", "svesm4", "flagm2", "frint", "svei8mm", "svef32mm",
    "svef64mm", "svebf16", "i8mm", "bf16", "dgh", "rng", "bti", "mte", "ecv", "afp", "rpres", "mte3", "sme", "wfxt", "ebf16", "sveebf16", "cssc", "rprfm", "sve2p1", "mops", "hbc",
];

/// Como se da la lista de extensiones de un perfil.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModoExtensiones {
    /// sin `cpu.features`: las de `cpu.base`
    DeLaBase,
    /// lista absoluta: exactamente esas
    Propia,
    /// cambios (`+x`/`-x`) sobre las de `cpu.base`
    Cambios,
}

/// Archivos integrados: (id, contenido).
const INTEGRADOS: &[(&str, &str)] = &[
    ("generico-a78", include_str!("../perfiles/dispositivos/generico-a78.device")),
    ("generico-a55", include_str!("../perfiles/dispositivos/generico-a55.device")),
    ("generico-x1", include_str!("../perfiles/dispositivos/generico-x1.device")),
];

/// Perfil que se toma como base al crear uno nuevo sin `--from`.
pub const ID_BASE: &str = "generico-a78";

/// Campos de identidad: (clave del archivo, sufijo de `ro.product.`).
pub const IDENTIDAD: [(&str, &str); 5] = [("producto.fabricante", "manufacturer"), ("producto.marca", "brand"), ("producto.modelo", "model"), ("producto.dispositivo", "device"), ("producto.nombre", "name")];

/// Claves que admite un perfil, en el orden en que se escriben.
pub const CLAVES: &[&str] = &[
    "dispositivo.formato",
    "dispositivo.id",
    "dispositivo.hereda",
    "dispositivo.nombre",
    "dispositivo.descripcion",
    "producto.fabricante",
    "producto.marca",
    "producto.modelo",
    "producto.dispositivo",
    "producto.nombre",
    "maquina.nucleos",
    "maquina.ram",
    "pagina",
    "cpu.nombre",
    "cpu.hardware",
    "cpu.base",
    "cpu.midr",
    "cpu.revidr",
    "cpu.ctr",
    "cpu.dczid",
    "cpu.features",
    "cpu.id_aa64pfr0",
    "cpu.id_aa64pfr1",
    "cpu.id_aa64zfr0",
    "cpu.id_aa64dfr0",
    "cpu.id_aa64isar0",
    "cpu.id_aa64isar1",
    "cpu.id_aa64isar2",
    "cpu.id_aa64mmfr0",
    "cpu.id_aa64mmfr1",
    "cpu.id_aa64mmfr2",
];

/// Registros de identificacion que el perfil puede fijar enteros (sustituyen a lo que `features` haya hecho en ellos),
/// en el orden de `cpu.conf`.
pub const REGISTROS: [&str; 10] = ["id_aa64pfr0", "id_aa64pfr1", "id_aa64zfr0", "id_aa64dfr0", "id_aa64isar0", "id_aa64isar1", "id_aa64isar2", "id_aa64mmfr0", "id_aa64mmfr1", "id_aa64mmfr2"];

/// Modelos del catalogo del traductor que valen como `cpu.base` (tambien sin `cortex-`).
pub const BASES: [&str; 7] = ["cortex-a53", "cortex-a55", "cortex-a76", "cortex-a77", "cortex-a78", "cortex-x1", "max"];

/// Identificador de un perfil de dispositivo: como los de imagen (`perfil::validar_id`) y distinto de `ninguno`.
pub fn validar_id(id: &str) -> Result<(), String> {
    if id == NINGUNO {
        return Err(txf!("dispositivo.id_reservado", NINGUNO));
    }
    crate::perfil::validar_id(id)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dispositivo {
    pub id: String,
    pub nombre: String,
    pub descripcion: String,
    pub origen: Origen,
    /// archivo del usuario de donde salio (None: integrado o aun sin guardar)
    pub archivo: Option<PathBuf>,
    /// ro.product.manufacturer, brand, model, device, name (en el orden de `IDENTIDAD`); vacio = el de la imagen
    pub producto: [String; 5],
    pub nucleos: Option<u32>,
    pub ram_mb: Option<u32>,
    /// KiB: 4 o 16
    pub pagina_kib: u32,
    pub cpu_nombre: String,
    pub midr: Option<u32>,
    pub revidr: Option<u32>,
    /// linea `Hardware` de /proc/cpuinfo (vacio: sin ella)
    pub hardware: String,
    pub ctr: Option<u64>,
    pub dczid: Option<u64>,
    /// registros `REGISTROS`, en su orden
    pub registros: [Option<u64>; 10],
    /// None: las de la base; Some: la lista (absoluta, o relativa a la base si todas llevan `+` o `-`)
    pub features: Option<Vec<String>>,
    pub base: String,
}

/// Una linea del archivo: clave, valor y numero de linea.
type Par = (String, String, usize);

fn pares(texto: &str) -> Result<Vec<Par>, String> {
    let mut v = Vec::new();
    for (n, l) in texto.lines().enumerate() {
        let t = l.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let (k, x) = t.split_once('=').ok_or_else(|| txf!("perfil.linea_se_espera_clave_valor", n + 1, format!("{:?}", t)))?;
        v.push((k.trim().to_string(), x.trim().to_string(), n + 1));
    }
    Ok(v)
}

/// El padre sustituido clave a clave por el hijo (sin `dispositivo.hereda` ni `dispositivo.id` del padre).
fn fusionar(padre: Vec<Par>, hijo: Vec<Par>) -> Vec<Par> {
    let mut r: Vec<Par> = padre.into_iter().filter(|(k, _, _)| k != "dispositivo.hereda" && k != "dispositivo.id").collect();
    for h in hijo {
        if h.0 == "dispositivo.hereda" {
            continue;
        }
        match r.iter().position(|(k, _, _)| *k == h.0) {
            Some(i) => r[i] = h,
            None => r.push(h),
        }
    }
    r
}

fn expandir(texto: &str, buscar_padre: &dyn Fn(&str) -> Option<String>, nivel: u32) -> Result<Vec<Par>, String> {
    let propios = pares(texto)?;
    if let Some((_, padre_id, linea)) = propios.iter().find(|(k, _, _)| k == "dispositivo.hereda") {
        if nivel >= 4 {
            return Err(tx!("dispositivo.hereda_demasiados_niveles").into());
        }
        validar_id(padre_id).map_err(|e| txf!("dispositivo.linea", linea, e))?;
        let tp = buscar_padre(padre_id).ok_or_else(|| txf!("dispositivo.linea_no_existe_el_padre", linea, format!("{:?}", padre_id)))?;
        let padre = expandir(&tp, buscar_padre, nivel + 1).map_err(|e| txf!("dispositivo.en_el_padre", format!("{:?}", padre_id), e))?;
        return Ok(fusionar(padre, propios));
    }
    Ok(propios)
}

/// Numero en hexadecimal (`0x...`) o decimal, con `_` como separador (como lo lee el traductor).
fn numero(v: &str) -> Option<u64> {
    let v: String = v.chars().filter(|c| *c != '_').collect();
    match v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        Some(h) if !h.is_empty() && h.len() <= 16 => u64::from_str_radix(h, 16).ok(),
        Some(_) => None,
        None => v.parse().ok(),
    }
}

/// ¿Es relativa la lista de extensiones (todas con `+` o `-`)?
pub fn es_relativa(l: &[String]) -> bool {
    !l.is_empty() && l.iter().all(|x| x.starts_with(['+', '-']))
}

/// Un texto que va tal cual a una propiedad o a cpu.conf: una sola linea de caracteres visibles, sin comillas simples ni
/// barras invertidas (van entre comillas en el shell del invitado) y de 91 bytes como mucho (PROP_VALUE_MAX - 1).
pub fn texto_valido(v: &str) -> bool {
    v.len() <= 91 && v.chars().all(|c| !c.is_control() && c != '\'' && c != '\\')
}

/// Lista de extensiones de un texto (separadas por espacios o comas, cada una con `+` o `-` delante si se quiere), sin
/// repetidas (la ultima de un nombre manda, como en el traductor) y en el orden canonico: las de `EXTENSIONES` en su
/// orden y despues las demas por orden alfabetico.
pub fn lista_extensiones(v: &str) -> Result<Vec<String>, String> {
    let mut l: Vec<String> = Vec::new();
    for x in v.split([' ', ',', '\t']).filter(|x| !x.is_empty()) {
        let nombre = x.trim_start_matches(['+', '-']);
        if nombre.len() + 1 < x.len() || nombre.is_empty() || nombre.len() > 32 || !nombre.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) {
            return Err(txf!("dispositivo.extension_no_valida", format!("{:?}", x)));
        }
        l.retain(|y| y.trim_start_matches(['+', '-']) != nombre);
        l.push(x.to_string());
    }
    ordenar_extensiones(&mut l);
    Ok(l)
}

pub fn ordenar_extensiones(l: &mut [String]) {
    l.sort_by_key(|x| {
        let n = x.trim_start_matches(['+', '-']);
        (EXTENSIONES.iter().position(|k| *k == n).unwrap_or(usize::MAX), n.to_string())
    });
}

impl Dispositivo {
    /// Perfil vacio con ese id (lo que vale cada clave que el archivo no da).
    pub fn vacio(id: &str) -> Dispositivo {
        Dispositivo {
            id: id.to_string(),
            nombre: String::new(),
            descripcion: String::new(),
            origen: Origen::Usuario,
            archivo: None,
            producto: Default::default(),
            nucleos: None,
            ram_mb: None,
            pagina_kib: 4,
            cpu_nombre: String::new(),
            midr: None,
            revidr: None,
            hardware: String::new(),
            ctr: None,
            dczid: None,
            registros: [None; 10],
            features: None,
            base: String::new(),
        }
    }

    /// Lee un perfil. `buscar_padre` devuelve el TEXTO de otro perfil por su id (para `dispositivo.hereda`).
    pub fn parse(texto: &str, origen: Origen, buscar_padre: &dyn Fn(&str) -> Option<String>) -> Result<Dispositivo, String> {
        let todos = expandir(texto, buscar_padre, 0)?;
        let mut d = Dispositivo::vacio("");
        d.origen = origen;
        for (k, v, n) in &todos {
            match k.as_str() {
                "dispositivo.formato" => {
                    if v.parse::<u32>().ok() != Some(FORMATO) {
                        return Err(txf!("dispositivo.linea_formato", n, v, FORMATO));
                    }
                }
                "dispositivo.id" => {
                    validar_id(v).map_err(|e| txf!("dispositivo.linea", n, e))?;
                    d.id = v.clone();
                }
                _ => d.fijar(k, v).map_err(|e| txf!("dispositivo.linea", n, e))?,
            }
        }
        if d.id.is_empty() {
            return Err(tx!("dispositivo.falta_el_id").into());
        }
        if d.nombre.is_empty() {
            d.nombre = d.id.clone();
        }
        Ok(d)
    }

    /// Pone una clave validandola (lo usan la lectura del archivo, `weft device set` y la pantalla de configuracion).
    /// `dispositivo.id`, `dispositivo.hereda` y `dispositivo.formato` no se cambian asi.
    pub fn fijar(&mut self, k: &str, v: &str) -> Result<(), String> {
        let v = v.trim();
        let texto = |v: &str| -> Result<String, String> {
            if texto_valido(v) {
                Ok(v.to_string())
            } else {
                Err(txf!("dispositivo.texto_no_valido", k))
            }
        };
        let auto = |v: &str, min: u32, max: u32| -> Result<Option<u32>, String> {
            if v.is_empty() || v == "auto" {
                return Ok(None);
            }
            match v.parse::<u32>() {
                Ok(n) if (min..=max).contains(&n) => Ok(Some(n)),
                _ => Err(txf!("dispositivo.se_espera_auto_o_de_a", k, min, max)),
            }
        };
        match k {
            "dispositivo.nombre" => self.nombre = texto(v)?,
            // la descripcion no va al invitado: solo una linea
            "dispositivo.descripcion" => {
                if v.chars().any(char::is_control) {
                    return Err(txf!("dispositivo.texto_no_valido", k));
                }
                self.descripcion = v.to_string();
            }
            "maquina.nucleos" => self.nucleos = auto(v, 1, 128)?,
            "maquina.ram" => self.ram_mb = auto(v, 512, 1048576)?,
            "pagina" => {
                self.pagina_kib = match v {
                    "4" => 4,
                    "16" => 16,
                    _ => return Err(tx!("dispositivo.pagina_4_o_16").into()),
                }
            }
            "cpu.nombre" => self.cpu_nombre = texto(v)?,
            "cpu.midr" | "cpu.revidr" => {
                let n = if v.is_empty() { None } else { Some(numero(v).and_then(|n| u32::try_from(n).ok()).ok_or_else(|| txf!("dispositivo.se_espera_un_numero_de_bits", k, 32))?) };
                if k == "cpu.midr" {
                    self.midr = n;
                } else {
                    self.revidr = n;
                }
            }
            "cpu.hardware" => self.hardware = texto(v)?,
            _ if k == "cpu.ctr" || k == "cpu.dczid" || k.strip_prefix("cpu.").is_some_and(|r| REGISTROS.contains(&r.trim_end_matches("_el1"))) => {
                let n = if v.is_empty() { None } else { Some(numero(v).ok_or_else(|| txf!("dispositivo.se_espera_un_numero_de_bits", k, 64))?) };
                match k {
                    "cpu.ctr" => self.ctr = n,
                    "cpu.dczid" => self.dczid = n,
                    _ => {
                        let r = k["cpu.".len()..].trim_end_matches("_el1");
                        if let Some(i) = REGISTROS.iter().position(|x| *x == r) {
                            self.registros[i] = n;
                        }
                    }
                }
            }
            "cpu.features" => self.features = Some(lista_extensiones(v)?),
            "cpu.base" => {
                let v = v.to_ascii_lowercase();
                if !v.is_empty() && !BASES.iter().any(|b| *b == v || b.strip_prefix("cortex-") == Some(v.as_str())) {
                    return Err(txf!("dispositivo.base_desconocida", BASES.join(", ")));
                }
                self.base = v;
            }
            _ => match IDENTIDAD.iter().position(|(c, _)| *c == k) {
                Some(i) => self.producto[i] = texto(v)?,
                None => return Err(txf!("dispositivo.clave_desconocida", k)),
            },
        }
        Ok(())
    }

    /// Valor de una clave como se escribe en el archivo.
    pub fn valor(&self, k: &str) -> String {
        let auto = |n: Option<u32>| n.map_or("auto".to_string(), |n| n.to_string());
        match k {
            "dispositivo.formato" => FORMATO.to_string(),
            "dispositivo.id" => self.id.clone(),
            "dispositivo.hereda" => String::new(),
            "dispositivo.nombre" => self.nombre.clone(),
            "dispositivo.descripcion" => self.descripcion.clone(),
            "maquina.nucleos" => auto(self.nucleos),
            "maquina.ram" => auto(self.ram_mb),
            "pagina" => self.pagina_kib.to_string(),
            "cpu.nombre" => self.cpu_nombre.clone(),
            "cpu.midr" => self.midr.map(|n| format!("{:#010x}", n)).unwrap_or_default(),
            "cpu.revidr" => self.revidr.map(|n| format!("{:#010x}", n)).unwrap_or_default(),
            "cpu.hardware" => self.hardware.clone(),
            "cpu.ctr" => self.ctr.map(|n| format!("{:#x}", n)).unwrap_or_default(),
            "cpu.dczid" => self.dczid.map(|n| format!("{:#x}", n)).unwrap_or_default(),
            "cpu.features" => self.features.as_ref().map(|l| l.join(" ")).unwrap_or_default(),
            "cpu.base" => self.base.clone(),
            _ => match k.strip_prefix("cpu.").and_then(|r| REGISTROS.iter().position(|x| *x == r)) {
                Some(i) => self.registros[i].map(|n| format!("{:#018x}", n)).unwrap_or_default(),
                None => IDENTIDAD.iter().position(|(c, _)| *c == k).map(|i| self.producto[i].clone()).unwrap_or_default(),
            },
        }
    }

    /// El archivo completo (sin herencia: todas las claves con su valor efectivo). Leerlo da el mismo perfil.
    pub fn a_texto(&self) -> String {
        // texto-interno: cabecera del archivo de perfil (el usuario lo edita a mano)
        let mut s = String::from("# Perfil de dispositivo de weft (formato: README, «Perfiles de dispositivo»).\n");
        for k in CLAVES {
            if *k == "dispositivo.hereda" || (*k == "cpu.features" && self.features.is_none()) {
                continue;
            }
            s.push_str(&format!("{}={}\n", k, self.valor(k)));
        }
        s
    }

    /// Lineas de `cpu.conf` (sin comentarios), con las claves del contrato y solo las que el perfil define.
    fn lineas_cpu(&self) -> Vec<String> {
        let mut v = Vec::new();
        for (k, x) in [("nombre", &self.cpu_nombre), ("hardware", &self.hardware), ("base", &self.base)] {
            if !x.is_empty() {
                v.push(format!("{}={}", k, x));
            }
        }
        for (k, x) in [("midr", self.midr), ("revidr", self.revidr)] {
            if let Some(n) = x {
                v.push(format!("{}={:#010x}", k, n));
            }
        }
        for (k, x) in [("ctr", self.ctr), ("dczid", self.dczid)] {
            if let Some(n) = x {
                v.push(format!("{}={:#x}", k, n));
            }
        }
        // una lista vacia (cambios sobre la base sin ninguno) es lo mismo que no darla
        if let Some(f) = self.features.as_ref().filter(|f| !f.is_empty()) {
            v.push(format!("features={}", f.join(",")));
        }
        for (k, x) in REGISTROS.iter().zip(self.registros) {
            if let Some(n) = x {
                v.push(format!("{}={:#018x}", k, n));
            }
        }
        v
    }

    /// Contenido de `cpu.conf`.
    pub fn cpu_conf(&self) -> String {
        // texto-interno: comentario del archivo que lee el traductor
        let mut s = format!("# generado por weft (perfil de dispositivo {}): lo reescribe `weft device apply`\n", self.id);
        for l in self.lineas_cpu() {
            s.push_str(&l);
            s.push('\n');
        }
        s
    }

    /// Propiedades que el perfil fija en los build.prop (clave, valor), sin la marca: la identidad que define (general y de
    /// cada particion) y el tamano de pagina.
    fn propiedades(&self) -> Vec<(String, String)> {
        let mut v = Vec::new();
        for (i, (_, sufijo)) in IDENTIDAD.iter().enumerate() {
            let x = &self.producto[i];
            if x.is_empty() {
                continue;
            }
            v.push((format!("ro.product.{}", sufijo), x.clone()));
            for p in PARTICIONES {
                v.push((format!("ro.product.{}.{}", p, sufijo), x.clone()));
            }
        }
        v.push((PROP_PAGINA.to_string(), (self.pagina_kib * 1024).to_string()));
        v
    }

    /// Firma de lo que se escribe en el invitado (cpu.conf y propiedades): 12 cifras hexadecimales.
    pub fn firma(&self) -> String {
        let mut t = self.lineas_cpu().join("\n");
        t.push_str("\n--\n");
        t.push_str(&self.propiedades().iter().map(|(k, v)| format!("{}={}", k, v)).collect::<Vec<_>>().join("\n"));
        crate::md5::de_bytes(t.as_bytes())[..12].to_string()
    }

    /// Valor de `ro.weft.dispositivo` con este perfil aplicado.
    pub fn marca(&self) -> String {
        format!("{}:{}", self.id, self.firma())
    }

    /// Todas las propiedades que fija en los build.prop, con la marca la primera (clave, valor).
    pub fn propiedades_con_marca(&self) -> Vec<(String, String)> {
        let mut v = vec![(PROP_MARCA.to_string(), self.marca())];
        v.extend(self.propiedades());
        v
    }

    pub fn modo_extensiones(&self) -> ModoExtensiones {
        match &self.features {
            None => ModoExtensiones::DeLaBase,
            Some(l) if l.is_empty() || es_relativa(l) => ModoExtensiones::Cambios,
            Some(_) => ModoExtensiones::Propia,
        }
    }

    /// Cambia el modo de la lista conservando lo que se pueda: de cambios a propia quedan las añadidas (mas lo que Linux
    /// siempre da: fp, asimd, evtstrm y cpuid); de propia a cambios, cada una como `+x`.
    pub fn poner_modo_extensiones(&mut self, m: ModoExtensiones) {
        if m == self.modo_extensiones() {
            return;
        }
        let actuales: Vec<String> = self.features.iter().flatten().filter(|x| !x.starts_with('-')).map(|x| x.trim_start_matches('+').to_string()).collect();
        self.features = match m {
            ModoExtensiones::DeLaBase => None,
            ModoExtensiones::Cambios => Some(actuales.into_iter().map(|x| format!("+{}", x)).collect()),
            ModoExtensiones::Propia => {
                let mut l: Vec<String> = ["fp", "asimd", "evtstrm", "cpuid"].iter().map(|x| x.to_string()).collect();
                l.extend(actuales.into_iter().filter(|x| !["fp", "asimd", "evtstrm", "cpuid"].contains(&x.as_str())));
                ordenar_extensiones(&mut l);
                Some(l)
            }
        };
    }

    /// Estado de una extension en la lista: None si no se nombra, Some('=') en una lista propia, Some('+'/'-') en una de
    /// cambios.
    pub fn estado_extension(&self, n: &str) -> Option<char> {
        let x = self.features.iter().flatten().find(|x| x.trim_start_matches(['+', '-']) == n)?;
        Some(x.chars().next().filter(|c| *c == '+' || *c == '-').unwrap_or('='))
    }

    /// Pulsar una extension en la pantalla: en una lista propia la pone o la quita; en una de cambios pasa por nada, `+x` y
    /// `-x`; sin lista, empieza una de cambios con `+x`.
    pub fn alternar_extension(&mut self, n: &str) {
        let estado = self.estado_extension(n);
        let l = self.features.get_or_insert_with(Vec::new);
        l.retain(|x| x.trim_start_matches(['+', '-']) != n);
        let nuevo = match estado {
            None if l.is_empty() || es_relativa(l) => Some(format!("+{}", n)),
            None => Some(n.to_string()),
            Some('+') => Some(format!("-{}", n)),
            _ => None,
        };
        if let Some(x) = nuevo {
            l.push(x);
        }
        ordenar_extensiones(l);
    }

    /// Extensiones que el perfil anuncia (o añade a la base) y que no estan en `soportadas` (lo que publica el traductor
    /// instalado, `soportadas_de_maquina`). Informativo; sin lista propia no se sabe (dependen de la base) y queda vacio.
    pub fn sin_soporte(&self, soportadas: &[String]) -> Vec<String> {
        self.features.iter().flatten().filter(|x| !x.starts_with('-')).map(|x| x.trim_start_matches('+')).filter(|x| !soportadas.iter().any(|s| s == x)).map(str::to_string).collect()
    }

    /// Extensiones anunciadas que no estan en la lista de Linux (informativo: se anuncian igual).
    pub fn desconocidas(&self) -> Vec<String> {
        self.features.iter().flatten().map(|x| x.trim_start_matches(['+', '-'])).filter(|x| !EXTENSIONES.contains(x)).map(str::to_string).collect()
    }

    /// Resumen para `weft device show` y la pantalla: una linea por dato.
    pub fn resumen(&self) -> Vec<(String, String)> {
        let auto = |n: Option<u32>, u: &str| n.map_or(tx!("dispositivo.auto").to_string(), |n| format!("{}{}", n, u));
        let mut v = vec![
            (tx!("dispositivo.r_origen").to_string(), if self.origen == Origen::Integrado { tx!("dispositivo.integrado").to_string() } else { tx!("dispositivo.del_usuario").to_string() }),
            (tx!("dispositivo.r_cpu").to_string(), if self.cpu_nombre.is_empty() { tx!("dispositivo.sin_definir").to_string() } else { self.cpu_nombre.clone() }),
            ("MIDR".to_string(), self.valor("cpu.midr")),
            (tx!("dispositivo.r_nucleos").to_string(), auto(self.nucleos, "")),
            (tx!("dispositivo.r_ram").to_string(), auto(self.ram_mb, " MiB")),
            (tx!("dispositivo.r_pagina").to_string(), format!("{} KiB", self.pagina_kib)),
        ];
        let ident: Vec<&str> = self.producto.iter().filter(|x| !x.is_empty()).map(String::as_str).collect();
        v.push((tx!("dispositivo.r_identidad").to_string(), if ident.is_empty() { tx!("dispositivo.la_de_la_imagen").to_string() } else { ident.join(" / ") }));
        v.push((tx!("dispositivo.r_extensiones").to_string(), self.features.as_ref().map_or(tx!("dispositivo.sin_definir").to_string(), |f| f.join(" "))));
        v
    }
}

// ---------------------------------------------------------------------------------------------------------------
// catalogo

pub struct Catalogo {
    pub dispositivos: Vec<Dispositivo>,
    /// perfiles del usuario que no se pudieron leer: (archivo, motivo)
    pub rechazados: Vec<(String, String)>,
    pub avisos: Vec<String>,
}

fn texto_integrado(id: &str) -> Option<String> {
    INTEGRADOS.iter().find(|(n, _)| *n == id).map(|(_, t)| t.to_string())
}

impl Catalogo {
    /// Integrados mas los `*.device` de la carpeta del usuario (si existe).
    pub fn cargar(carpeta_usuario: Option<&Path>) -> Catalogo {
        let mut textos_usuario: Vec<(PathBuf, String)> = Vec::new();
        let mut rechazados = Vec::new();
        if let Some(d) = carpeta_usuario {
            if let Ok(rd) = std::fs::read_dir(d) {
                let mut nombres: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == EXT)).collect();
                nombres.sort();
                for p in nombres {
                    match std::fs::read_to_string(&p) {
                        Ok(t) => textos_usuario.push((p, t)),
                        Err(e) => rechazados.push((p.display().to_string(), e.to_string())),
                    }
                }
            }
        }
        let id_de = |t: &str| pares(t).ok().and_then(|ps| ps.into_iter().find(|(k, _, _)| k == "dispositivo.id").map(|(_, v, _)| v));
        // el padre de un perfil del usuario: otro del usuario con ese id (que no sea el mismo texto) o el integrado
        let padre = |id: &str| -> Option<String> { textos_usuario.iter().find(|(_, t)| id_de(t).as_deref() == Some(id)).map(|(_, t)| t.clone()).or_else(|| texto_integrado(id)) };
        let mut dispositivos: Vec<Dispositivo> = Vec::new();
        let mut avisos = Vec::new();
        for (id, t) in INTEGRADOS {
            match Dispositivo::parse(t, Origen::Integrado, &texto_integrado) {
                Ok(d) => dispositivos.push(d),
                Err(e) => rechazados.push((txf!("dispositivo.integrado_id", id), e)),
            }
        }
        for (archivo, t) in &textos_usuario {
            let nombre = archivo.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match Dispositivo::parse(t, Origen::Usuario, &padre) {
                Ok(mut d) => {
                    d.archivo = Some(archivo.clone());
                    if let Some(i) = dispositivos.iter().position(|x| x.id == d.id) {
                        avisos.push(txf!("dispositivo.reemplaza_a", format!("{:?}", d.id), nombre));
                        dispositivos[i] = d;
                    } else {
                        dispositivos.push(d);
                    }
                }
                Err(e) => rechazados.push((nombre, e)),
            }
        }
        Catalogo { dispositivos, rechazados, avisos }
    }

    pub fn buscar(&self, id: &str) -> Option<&Dispositivo> {
        self.dispositivos.iter().find(|d| d.id == id)
    }

    /// El perfil que eligio la maquina (`dispositivo.perfil`): Ok(None) sin perfil; Err si no existe.
    pub fn elegido(&self, cfg: &Config) -> Result<Option<&Dispositivo>, String> {
        let id = cfg.get("dispositivo.perfil");
        if id == NINGUNO {
            return Ok(None);
        }
        self.buscar(&id).map(Some).ok_or_else(|| txf!("dispositivo.no_existe", format!("{:?}", id)))
    }

    /// Un id libre a partir de `base`: `base-copia`, `base-copia-2`...
    pub fn id_libre(&self, base: &str) -> String {
        let base: String = base.chars().take(50).collect();
        let mut id = format!("{}-copia", base);
        let mut n = 2;
        while self.buscar(&id).is_some() {
            id = format!("{}-copia-{}", base, n);
            n += 1;
        }
        id
    }
}

/// Carpeta de los perfiles de dispositivo del usuario.
pub fn carpeta_usuario(r: &crate::rutas::Rutas) -> PathBuf {
    r.dispositivos_usuario()
}

/// Guarda un perfil del usuario (en su archivo, o en `<carpeta>/<id>.device` si es nuevo) de forma atomica. Devuelve la ruta.
pub fn guardar(carpeta: &Path, d: &mut Dispositivo) -> Result<PathBuf, String> {
    if d.origen != Origen::Usuario {
        return Err(tx!("dispositivo.los_integrados_no_se_cambian").into());
    }
    validar_id(&d.id)?;
    let ruta = d.archivo.clone().unwrap_or_else(|| carpeta.join(format!("{}.{}", d.id, EXT)));
    if let Some(p) = ruta.parent() {
        std::fs::create_dir_all(p).map_err(|e| format!("{}: {}", p.display(), e))?;
    }
    let tmp = ruta.with_extension("device.tmp");
    std::fs::write(&tmp, d.a_texto()).map_err(|e| format!("{}: {}", tmp.display(), e))?;
    std::fs::rename(&tmp, &ruta).map_err(|e| format!("{}: {}", ruta.display(), e))?;
    d.archivo = Some(ruta.clone());
    Ok(ruta)
}

/// Copia `origen` con otro id (y nombre) como perfil del usuario, y la guarda.
pub fn duplicar(carpeta: &Path, cat: &Catalogo, origen: &str, id: &str, nombre: Option<&str>) -> Result<Dispositivo, String> {
    validar_id(id)?;
    if cat.buscar(id).is_some() {
        return Err(txf!("dispositivo.ya_existe", format!("{:?}", id)));
    }
    let o = cat.buscar(origen).ok_or_else(|| txf!("dispositivo.no_existe", format!("{:?}", origen)))?;
    let mut d = o.clone();
    d.id = id.to_string();
    d.origen = Origen::Usuario;
    d.archivo = None;
    d.nombre = match nombre {
        Some(n) => {
            if !texto_valido(n) {
                return Err(txf!("dispositivo.texto_no_valido", "dispositivo.nombre"));
            }
            n.to_string()
        }
        None => txf!("dispositivo.copia_de", o.nombre),
    };
    if !texto_valido(&d.nombre) {
        d.nombre = id.to_string();
    }
    guardar(carpeta, &mut d)?;
    Ok(d)
}

/// Borra un perfil del usuario (su archivo).
pub fn borrar(d: &Dispositivo) -> Result<(), String> {
    match (&d.origen, &d.archivo) {
        (Origen::Usuario, Some(p)) => std::fs::remove_file(p).map_err(|e| format!("{}: {}", p.display(), e)),
        _ => Err(tx!("dispositivo.los_integrados_no_se_borran").into()),
    }
}

/// Nucleos y memoria para el arranque segun la precedencia de este modulo, sin las opciones de la linea de ordenes:
/// (nucleos, MiB), None = el defecto del arranque.
pub fn recursos(cfg: &Config, d: Option<&Dispositivo>) -> (Option<u32>, Option<u32>) {
    (cfg.entero("maquina.cpus").or(d.and_then(|d| d.nucleos)), cfg.entero("maquina.ram").or(d.and_then(|d| d.ram_mb)))
}

/// `recursos` leyendo el catalogo (al arrancar): un perfil que no existe no impide arrancar, se avisa.
pub fn recursos_de_maquina(cfg: &Config, r: &crate::rutas::Rutas) -> ((Option<u32>, Option<u32>), Option<String>) {
    if cfg.get("dispositivo.perfil") == NINGUNO {
        return (recursos(cfg, None), None);
    }
    let cat = Catalogo::cargar(Some(&carpeta_usuario(r)));
    match cat.elegido(cfg) {
        Ok(d) => (recursos(cfg, d), None),
        Err(e) => (recursos(cfg, None), Some(e)),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// extensiones que publica el traductor instalado

/// Extensiones de un archivo `cpu-features` (`CPU_FEATURES`): separadas por espacios o saltos de linea, `#` comentario
/// hasta el final de la linea.
pub fn leer_soportadas(texto: &str) -> Vec<String> {
    texto.lines().flat_map(|l| l.split('#').next().unwrap_or("").split_whitespace()).map(str::to_string).collect()
}

/// Lo que publica el traductor instalado en la maquina del estado `estado` (None: no publica nada, y no se avisa).
pub fn soportadas_de_maquina(estado: &Path) -> Option<Vec<String>> {
    std::fs::read_to_string(estado.join(ARCHIVO_SOPORTADAS)).ok().map(|t| leer_soportadas(&t))
}

/// Tras instalar el traductor `lib`: copia su `cpu-features` (si lo trae junto a la biblioteca) al estado de la maquina,
/// o borra la copia anterior si no lo trae. Devuelve si quedo copia.
pub fn copiar_soportadas(lib: &Path, estado: &Path) -> Result<bool, String> {
    let destino = estado.join(ARCHIVO_SOPORTADAS);
    let origen = lib.parent().map(|d| d.join(CPU_FEATURES));
    match origen.filter(|o| o.is_file()) {
        Some(o) => std::fs::copy(&o, &destino).map(|_| true).map_err(|e| format!("{}: {}", o.display(), e)),
        None => olvidar_soportadas(estado).map(|_| false),
    }
}

/// Borra la copia de `cpu-features` del estado (el traductor se quito o se desconoce cual quedo).
pub fn olvidar_soportadas(estado: &Path) -> Result<(), String> {
    let destino = estado.join(ARCHIVO_SOPORTADAS);
    match std::fs::remove_file(&destino) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("{}: {}", destino.display(), e)),
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// las propiedades en los build.prop

/// `texto` sin el bloque de la version anterior de este modulo (si lo hay; uno sin marca de fin llega hasta el final).
fn sin_bloque(texto: &str) -> String {
    let mut out = String::with_capacity(texto.len());
    let mut dentro = false;
    for l in texto.split_inclusive('\n') {
        let t = l.trim_end();
        if !dentro && t.starts_with(MARCA_INICIO) {
            dentro = true;
            continue;
        }
        if dentro {
            if t.starts_with(MARCA_FIN) {
                dentro = false;
            }
            continue;
        }
        out.push_str(l);
    }
    out
}

/// Clave de una linea de build.prop (None: vacia, comentario o sin `=`).
fn clave_de(l: &str) -> Option<&str> {
    let t = l.trim();
    if t.is_empty() || t.starts_with('#') {
        return None;
    }
    t.split_once('=').map(|(k, _)| k.trim())
}

/// Lineas originales de un build.prop que cambio un perfil (contenido de `<archivo>.weft-orig`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Originales {
    /// (clave, sus lineas en la imagen tal cual y en su orden; vacio = la imagen no la tenia y weft la agrego)
    pub claves: Vec<(String, Vec<String>)>,
    /// el archivo de la imagen no acababa en salto de linea (weft lo puso para agregar)
    pub sin_salto_final: bool,
}

const SIN_SALTO_FINAL: &str = "#sin-salto-final";

impl Originales {
    /// Formato: comentarios al principio, `#sin-salto-final` si hace falta, y por cada clave una linea `=clave` seguida
    /// de sus lineas originales (una linea de build.prop nunca empieza por `=`).
    pub fn leer(t: &str) -> Originales {
        let mut o = Originales::default();
        for l in t.split_inclusive('\n').map(|l| l.strip_suffix('\n').unwrap_or(l)) {
            if let Some(k) = l.strip_prefix('=') {
                o.claves.push((k.to_string(), Vec::new()));
            } else if let Some((_, ls)) = o.claves.last_mut() {
                ls.push(l.to_string());
            } else if l == SIN_SALTO_FINAL {
                o.sin_salto_final = true;
            }
        }
        o
    }

    pub fn a_texto(&self) -> String {
        // texto-interno: cabecera del archivo que weft deja en el invitado
        let mut s = String::from("# weft: lineas originales de este build.prop que cambio un perfil de dispositivo (las repone `weft device remove`)\n");
        if self.sin_salto_final {
            s.push_str(SIN_SALTO_FINAL);
            s.push('\n');
        }
        for (k, ls) in &self.claves {
            s.push('=');
            s.push_str(k);
            s.push('\n');
            for l in ls {
                s.push_str(l);
                s.push('\n');
            }
        }
        s
    }

    /// `texto` con las lineas originales en su sitio: la i-esima linea de cada clave vuelve a ser la i-esima original,
    /// las que sobran (agregadas por weft) se quitan y las originales que falten van al final.
    pub fn reponer(&self, texto: &str) -> String {
        let mut cuenta = vec![0usize; self.claves.len()];
        let mut out = String::with_capacity(texto.len());
        for l in texto.split_inclusive('\n') {
            let cuerpo = l.strip_suffix('\n').unwrap_or(l);
            match clave_de(cuerpo).and_then(|k| self.claves.iter().position(|(c, _)| c == k)) {
                Some(i) => {
                    if let Some(orig) = self.claves[i].1.get(cuenta[i]) {
                        out.push_str(orig);
                        if l.ends_with('\n') {
                            out.push('\n');
                        }
                    }
                    cuenta[i] += 1;
                }
                None => out.push_str(l),
            }
        }
        for (i, (_, ls)) in self.claves.iter().enumerate() {
            for orig in ls.iter().skip(cuenta[i]) {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(orig);
                out.push('\n');
            }
        }
        if self.sin_salto_final && out.ends_with('\n') {
            out.pop();
        }
        out
    }
}

/// `texto` con cada linea de las claves de `fijar` sustituida en su sitio por `clave=valor`, y las de `agregar` al final;
/// devuelve tambien las lineas originales de las claves que cambio o agrego.
fn sustituir(texto: &str, fijar: &[(String, String)], agregar: &[&(String, String)]) -> (String, Originales) {
    let mut o = Originales::default();
    let mut out = String::with_capacity(texto.len() + 64);
    for l in texto.split_inclusive('\n') {
        let cuerpo = l.strip_suffix('\n').unwrap_or(l);
        match clave_de(cuerpo).and_then(|k| fijar.iter().find(|(c, _)| c == k)) {
            Some((k, v)) => {
                match o.claves.iter_mut().find(|(c, _)| c == k) {
                    Some((_, ls)) => ls.push(cuerpo.to_string()),
                    None => o.claves.push((k.clone(), vec![cuerpo.to_string()])),
                }
                out.push_str(&format!("{}={}", k, v));
                if l.ends_with('\n') {
                    out.push('\n');
                }
            }
            None => out.push_str(l),
        }
    }
    for (k, v) in agregar {
        if o.claves.iter().any(|(c, _)| c == k) {
            continue;
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
            o.sin_salto_final = true;
        }
        out.push_str(&format!("{}={}\n", k, v));
        o.claves.push((k.clone(), Vec::new()));
    }
    (out, o)
}

/// Un build.prop del invitado: ruta, contenido y el de su `.weft-orig` (None si no existe).
#[derive(Clone, Debug, PartialEq)]
pub struct ArchivoProp {
    pub ruta: String,
    pub texto: String,
    pub orig: Option<String>,
}

/// Los build.prop como quedan con `d` (None: como los dejo la imagen), cada uno con su `.weft-orig`. Primero se reponen
/// las lineas que cambio un perfil anterior (y se quita el bloque de la version anterior), asi que cambiar de perfil o
/// quitarlo parte siempre de la imagen.
pub fn plan_props(props: &[ArchivoProp], d: Option<&Dispositivo>) -> Vec<ArchivoProp> {
    let bases: Vec<String> = props.iter().map(|a| sin_bloque(&Originales::leer(a.orig.as_deref().unwrap_or("")).reponer(&a.texto))).collect();
    let deseadas = d.map(|d| d.propiedades_con_marca()).unwrap_or_default();
    // las generales, la pagina y la marca se agregan al de product o, sin el, al de system (o al ultimo que haya)
    let principal = props.iter().position(|a| a.ruta == PROP_PRODUCT).or_else(|| props.iter().position(|a| a.ruta == PROP_SYSTEM)).unwrap_or(props.len().saturating_sub(1));
    // donde se agrega cada clave que no esta en ninguno: la de una particion en el build.prop de esa particion (si existe)
    let casa = |k: &str| -> Option<usize> {
        match PARTICIONES.iter().find(|p| k.starts_with(&format!("ro.product.{}.", p))) {
            Some(p) => ARCHIVOS_PROP.iter().find(|(q, _)| q == p).and_then(|(_, ruta)| props.iter().position(|a| a.ruta == *ruta)),
            None => Some(principal),
        }
    };
    let falta = |k: &str| !bases.iter().any(|b| b.split('\n').any(|l| clave_de(l) == Some(k)));
    props
        .iter()
        .zip(&bases)
        .enumerate()
        .map(|(i, (a, base))| {
            let agregar: Vec<&(String, String)> = deseadas.iter().filter(|(k, _)| casa(k) == Some(i) && falta(k)).collect();
            let (texto, o) = sustituir(base, &deseadas, &agregar);
            ArchivoProp { ruta: a.ruta.clone(), texto, orig: if o.claves.is_empty() { None } else { Some(o.a_texto()) } }
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------------------------
// aplicar en el invitado

pub trait Invitado {
    fn esperar_arranque(&mut self, limite_s: u64) -> Result<(), String>;
    fn root(&mut self) -> Result<(), String>;
    fn remontar(&mut self) -> Result<(), String>;
    /// `disable-verity`; Ok(true) si hay que reiniciar para que surta efecto
    fn desactivar_verity(&mut self) -> Result<bool, String>;
    fn reiniciar(&mut self) -> Result<(), String>;
    fn arranque_completo(&mut self) -> bool;
    fn pausa(&mut self, s: u64);
    fn ahora(&self) -> f64;
    /// orden de shell: (codigo de salida, salida estandar)
    fn sh(&mut self, cmd: &str) -> Result<(i32, String), String>;
    /// deja `contenido` en el archivo `remoto` del invitado
    fn subir(&mut self, contenido: &[u8], remoto: &str) -> Result<(), String>;
}

#[derive(Clone, Debug, PartialEq)]
pub enum Resultado {
    /// Android ya arranco con este perfil (o sin ninguno, y no hay nada que quitar)
    SinCambios,
    Aplicado(Vec<String>),
    /// escrito pero sin reiniciar (`--no-reboot`): vale desde el proximo arranque de Android
    SinReiniciar,
    /// Android no volvio a arrancar con el perfil y se devolvieron los archivos anteriores (codigo 2)
    Restaurado(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// antes de cambiar nada (codigo 1)
    Previo(String),
    /// fallo y la restauracion tambien (codigo 3)
    Grave(String),
}

pub fn codigo(r: &Result<Resultado, Error>) -> i32 {
    match r {
        Ok(Resultado::Restaurado(_)) => 2,
        Ok(_) => 0,
        Err(Error::Previo(_)) => 1,
        Err(Error::Grave(_)) => 3,
    }
}

pub fn describir(r: &Result<Resultado, Error>) -> String {
    match r {
        Ok(Resultado::SinCambios) => tx!("dispositivo.sin_cambios").to_string(),
        Ok(Resultado::Aplicado(n)) => txf!("dispositivo.aplicado", n.join("\n")).trim_end().to_string(),
        Ok(Resultado::SinReiniciar) => tx!("dispositivo.sin_reiniciar").to_string(),
        Ok(Resultado::Restaurado(m)) => txf!("dispositivo.restaurado", m),
        Err(Error::Previo(e)) => txf!("puente.error_nada_cambio", e),
        Err(Error::Grave(e)) => txf!("puente.error_grave", e),
    }
}

/// Lo que hay en el invitado: los build.prop que existen (en el orden de `ARCHIVOS_PROP`) y cpu.conf (None si no existe).
#[derive(Clone, Debug, PartialEq)]
struct Archivos {
    props: Vec<ArchivoProp>,
    cpu: Option<String>,
}

/// Separador de la salida de `orden_leer` (al principio de linea).
const SEP: &str = "@@weft@@ ";

/// Orden que vuelca cada build.prop que exista con su `.weft-orig`, cpu.conf y una marca de fin.
fn orden_leer() -> String {
    let lista: Vec<&str> = ARCHIVOS_PROP.iter().map(|(_, r)| *r).collect();
    format!(
        "for f in {l}; do [ -f $f ] || continue; echo \"{s}prop $f\"; cat $f; echo; if [ -f $f{o} ]; then echo \"{s}orig $f\"; cat $f{o}; echo; fi; done; if [ -f {c} ]; then echo \"{s}cpu\"; cat {c}; echo; fi; echo \"{s}fin\"",
        l = lista.join(" "),
        s = SEP,
        o = EXT_ORIG,
        c = CPU_CONF
    )
}

fn parsear_archivos(o: &str) -> Result<Archivos, String> {
    let inesperada = || tx!("dispositivo.respuesta_inesperada").to_string();
    // cada `cat` va seguido de un `echo`: el salto de linea antes del separador no es del archivo
    let texto = format!("\n{}", o);
    let sep = format!("\n{}", SEP);
    let mut a = Archivos { props: Vec::new(), cpu: None };
    let mut fin = false;
    for t in texto.split(sep.as_str()).skip(1) {
        let (cabeza, cuerpo) = t.split_once('\n').unwrap_or((t, ""));
        match cabeza.split_once(' ') {
            Some(("prop", ruta)) if ruta.starts_with('/') => a.props.push(ArchivoProp { ruta: ruta.to_string(), texto: cuerpo.to_string(), orig: None }),
            Some(("orig", ruta)) => match a.props.last_mut() {
                Some(p) if p.ruta == ruta => p.orig = Some(cuerpo.to_string()),
                _ => return Err(inesperada()),
            },
            _ if cabeza == "cpu" => a.cpu = Some(cuerpo.to_string()),
            _ if cabeza == "fin" => {
                fin = true;
                break;
            }
            _ => return Err(inesperada()),
        }
    }
    if !fin || a.props.is_empty() {
        return Err(inesperada());
    }
    Ok(a)
}

fn leer(inv: &mut dyn Invitado) -> Result<Archivos, String> {
    let (c, o) = inv.sh(&orden_leer())?;
    if c != 0 {
        return Err(txf!("dispositivo.no_se_pudo_leer", c));
    }
    parsear_archivos(&o)
}

/// Sube `contenido` a un temporal y anade a `cmd` la copia sobre `destino` (con `cat`: conserva dueno, modo y etiqueta).
fn poner(inv: &mut dyn Invitado, cmd: &mut Vec<String>, contenido: &str, destino: &str) -> Result<(), String> {
    let t = format!("{}-{}", TMP, cmd.len());
    inv.subir(contenido.as_bytes(), &t)?;
    cmd.push(format!("cat {} > {}", t, destino));
    cmd.push(format!("rm -f {}", t));
    Ok(())
}

/// Lleva los archivos del invitado de `desde` (lo que hay) a `hacia`: solo escribe los que cambian. Una orden de shell.
fn escribir(inv: &mut dyn Invitado, desde: &Archivos, hacia: &Archivos) -> Result<(), String> {
    let mut cmd: Vec<String> = Vec::new();
    for (a, b) in desde.props.iter().zip(&hacia.props) {
        if a.texto != b.texto {
            poner(inv, &mut cmd, &b.texto, &b.ruta)?;
        }
        if a.orig != b.orig {
            let o = format!("{}{}", b.ruta, EXT_ORIG);
            match &b.orig {
                Some(t) => {
                    poner(inv, &mut cmd, t, &o)?;
                    cmd.push(format!("chmod 644 {}", o));
                }
                None => cmd.push(format!("rm -f {}", o)),
            }
        }
    }
    if desde.cpu != hacia.cpu {
        match &hacia.cpu {
            Some(c) => {
                cmd.push(format!("mkdir -p {}", CPU_CONF_DIR));
                poner(inv, &mut cmd, c, CPU_CONF)?;
                cmd.push(format!("chmod 755 {} && chmod 644 {} && restorecon -R {}", CPU_CONF_DIR, CPU_CONF, CPU_CONF_DIR));
            }
            None => cmd.push(format!("rm -f {} && {{ rmdir {} 2>/dev/null; true; }}", CPU_CONF, CPU_CONF_DIR)),
        }
    }
    if cmd.is_empty() {
        return Ok(());
    }
    cmd.push("sync".into());
    let cmd = cmd.join(" && ");
    let (c, o) = inv.sh(&cmd)?;
    if c != 0 {
        return Err(crate::puente::error_orden(&cmd, c, &o));
    }
    Ok(())
}

/// La marca con que arranco Android (`ro.weft.dispositivo`; vacia sin perfil). No necesita root.
pub fn marca_en_android(inv: &mut dyn Invitado) -> Result<String, String> {
    inv.sh(&format!("getprop {}", PROP_MARCA)).map(|(_, o)| o.trim().to_string())
}

/// Root y particiones del sistema escribibles; la primera vez desactiva la verificacion (y reinicia si hace falta).
fn preparar_escritura(inv: &mut dyn Invitado, prog: &dyn Fn(&str)) -> Result<(), String> {
    inv.root()?;
    if inv.remontar().is_ok() {
        return Ok(());
    }
    prog(tx!("dispositivo.desactivando_verificacion"));
    if inv.desactivar_verity()? {
        prog(tx!("dispositivo.reiniciando_para_verificacion"));
        inv.reiniciar()?;
        inv.esperar_arranque(ESPERA_INICIAL_S)?;
        inv.root()?;
    }
    inv.remontar()
}

/// Aplica `deseado` (None: quita lo de weft) en el Android en marcha y, con `reiniciar`, reinicia y vigila el arranque.
/// Si Android no vuelve a arrancar en `limite_s`, restaura los archivos de antes y reinicia otra vez.
pub fn aplicar(inv: &mut dyn Invitado, deseado: Option<&Dispositivo>, reiniciar: bool, limite_s: u64, prog: &dyn Fn(&str)) -> Result<Resultado, Error> {
    let previo = |e: String| Error::Previo(e);
    prog(tx!("puente.comprobando_que_android_responde"));
    inv.esperar_arranque(ESPERA_INICIAL_S).map_err(|e| previo(txf!("puente.android_no_responde_por_adb", e)))?;
    let quiere = deseado.map(|d| d.marca()).unwrap_or_default();
    let tiene = marca_en_android(inv).map_err(previo)?;
    preparar_escritura(inv, prog).map_err(previo)?;
    let antes = leer(inv).map_err(previo)?;
    let despues = Archivos { props: plan_props(&antes.props, deseado), cpu: deseado.map(|d| d.cpu_conf()) };
    let iguales = despues == antes;
    if iguales && tiene == quiere {
        return Ok(Resultado::SinCambios);
    }
    if !iguales {
        prog(tx!("dispositivo.escribiendo"));
        // una escritura a medias (varios archivos) se deshace antes de informar
        if let Err(e) = escribir(inv, &antes, &despues) {
            return Err(match leer(inv).and_then(|ahora| escribir(inv, &ahora, &antes)) {
                Ok(()) => Error::Previo(e),
                Err(e2) => Error::Grave(txf!("dispositivo.fallo_y_restauracion", e, e2)),
            });
        }
    }
    if !reiniciar {
        return Ok(Resultado::SinReiniciar);
    }
    prog(&txf!("dispositivo.reiniciando_y_vigilando", limite_s));
    let t0 = inv.ahora();
    let arranco = inv.reiniciar().is_ok() && {
        let mut ok = false;
        while inv.ahora() - t0 < limite_s as f64 {
            if inv.arranque_completo() {
                ok = true;
                break;
            }
            inv.pausa(SONDEO_S);
        }
        ok
    };
    if !arranco {
        let motivo = txf!("dispositivo.no_arranco_en", limite_s);
        prog(&txf!("puente.fallo", motivo));
        let restaurar = (|| -> Result<(), String> {
            // Android no termino de arrancar, pero adbd ya responde (arranca mucho antes): root, remount y los archivos de antes
            prog(tx!("puente.restaurando_lo_anterior"));
            preparar_escritura(inv, prog)?;
            escribir(inv, &despues, &antes)?;
            prog(tx!("puente.reiniciando_android_tras_restaurar"));
            inv.reiniciar()?;
            inv.esperar_arranque(ESPERA_RESTAURAR_S)
        })();
        return match restaurar {
            Ok(()) => Ok(Resultado::Restaurado(motivo)),
            Err(e) => Err(Error::Grave(txf!("dispositivo.fallo_y_restauracion", motivo, e))),
        };
    }
    let mut notas = Vec::new();
    let ahora = marca_en_android(inv).unwrap_or_default();
    if ahora != quiere {
        notas.push(elige(tx!("dispositivo.aviso_la_marca_no_coincide"), &txf!("dispositivo_tec.aviso_la_marca_no_coincide", PROP_MARCA, format!("{:?}", ahora), format!("{:?}", quiere))));
    }
    Ok(Resultado::Aplicado(notas))
}

/// Tras un arranque de Android: si arranco con otro perfil que el elegido, lo aplica (reiniciando). Una marca que ya fallo
/// (archivo `ARCHIVO_FALLO` del estado) no se reintenta: la reintenta `weft device apply`. Devuelve una linea de informe o
/// None si no habia nada que hacer.
pub fn sincronizar(inv: &mut dyn Invitado, estado: &Path, cfg: &Config, r: &crate::rutas::Rutas, prog: &dyn Fn(&str)) -> Option<String> {
    let cat = Catalogo::cargar(Some(&carpeta_usuario(r)));
    let deseado = match cat.elegido(cfg) {
        Ok(d) => d,
        Err(e) => return Some(e),
    };
    let quiere = deseado.map(|d| d.marca()).unwrap_or_default();
    let tiene = marca_en_android(inv).ok()?;
    if tiene == quiere {
        return None;
    }
    let fallo = estado.join(ARCHIVO_FALLO);
    if std::fs::read_to_string(&fallo).is_ok_and(|t| t.trim() == quiere) {
        return Some(tx!("dispositivo.pendiente_por_fallo").to_string());
    }
    let r = aplicar(inv, deseado, true, LIMITE_ARRANQUE_S, prog);
    if matches!(r, Ok(Resultado::Restaurado(_)) | Err(Error::Grave(_))) {
        let _ = std::fs::write(&fallo, format!("{}\n", quiere));
    } else if r.is_ok() {
        let _ = std::fs::remove_file(&fallo);
    }
    Some(describir(&r))
}

/// `sincronizar` con el adb propio de la maquina del estado `estado` (lo que hacen la ventana, `apply-settings` y `restart
/// --wait` tras cada arranque). Sin perfil elegido y sin nada en Android cuesta un `getprop`.
pub fn sincronizar_maquina(estado: &Path, cid: u32, cfg: &Config, prog: &dyn Fn(&str)) -> Option<String> {
    let r = crate::rutas::actual().con_claves(&cfg.get("dir.datos"), &cfg.get("dir.cache"));
    let mut inv = InvitadoAdb::nuevo(cid, r.cache_de(estado, "device", "dispositivo"));
    sincronizar(&mut inv, estado, cfg, &r, prog)
}

/// El invitado real: adb propio (delegando en el del traductor lo que comparten).
pub struct InvitadoAdb {
    base: crate::puente::InvitadoAdb,
    /// carpeta local para los archivos que se suben
    tmp: PathBuf,
}

impl InvitadoAdb {
    pub fn nuevo(cid: u32, tmp: PathBuf) -> InvitadoAdb {
        InvitadoAdb { base: crate::puente::InvitadoAdb::nuevo(cid, tmp.clone()), tmp }
    }
}

impl Invitado for InvitadoAdb {
    fn esperar_arranque(&mut self, limite_s: u64) -> Result<(), String> {
        crate::puente::Invitado::esperar_arranque(&mut self.base, limite_s)
    }
    fn root(&mut self) -> Result<(), String> {
        crate::puente::Invitado::root(&mut self.base)
    }
    fn remontar(&mut self) -> Result<(), String> {
        crate::puente::Invitado::remontar(&mut self.base)
    }
    fn desactivar_verity(&mut self) -> Result<bool, String> {
        crate::puente::Invitado::desactivar_verity(&mut self.base)
    }
    fn reiniciar(&mut self) -> Result<(), String> {
        crate::puente::Invitado::reiniciar(&mut self.base)
    }
    fn arranque_completo(&mut self) -> bool {
        crate::puente::Invitado::arranque_completo(&mut self.base)
    }
    fn pausa(&mut self, s: u64) {
        crate::puente::Invitado::pausa(&mut self.base, s)
    }
    fn ahora(&self) -> f64 {
        crate::puente::Invitado::ahora(&self.base)
    }
    fn sh(&mut self, cmd: &str) -> Result<(i32, String), String> {
        let (c, o, e) = crate::adb::conectar(self.base.cid, 20)?.shell_texto(cmd)?;
        Ok((c, if c == 0 { o } else { format!("{}{}", o, e) }))
    }
    fn subir(&mut self, contenido: &[u8], remoto: &str) -> Result<(), String> {
        std::fs::create_dir_all(&self.tmp).map_err(|e| format!("{}: {}", self.tmp.display(), e))?;
        let local = self.tmp.join(remoto.rsplit('/').next().unwrap_or("weft-tmp"));
        std::fs::write(&local, contenido).map_err(|e| format!("{}: {}", local.display(), e))?;
        let r = crate::adb::conectar(self.base.cid, 30)?.push(&local, remoto).map(|_| ());
        let _ = std::fs::remove_file(&local);
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn integrados() -> Catalogo {
        Catalogo::cargar(None)
    }

    fn tmpdir(n: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("weft-disp-{}-{}", n, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn los_integrados_se_leen_y_heredan() {
        let c = integrados();
        assert!(c.rechazados.is_empty(), "{:?}", c.rechazados);
        let a78 = c.buscar("generico-a78").unwrap();
        assert_eq!(a78.midr, Some(0x411fd411));
        assert_eq!((a78.nucleos, a78.ram_mb, a78.pagina_kib), (Some(8), Some(6144), 4));
        assert!(a78.features.as_ref().unwrap().iter().any(|f| f == "asimddp"));
        assert!(a78.desconocidas().is_empty());
        assert_eq!(a78.origen, Origen::Integrado);
        // el X1 hereda las extensiones del A78 y cambia nucleo, nombre y memoria
        let x1 = c.buscar("generico-x1").unwrap();
        assert_eq!(x1.features, a78.features);
        assert_eq!((x1.midr, x1.ram_mb, x1.cpu_nombre.as_str()), (Some(0x411fd440), Some(8192), "Cortex-X1"));
        assert_eq!(x1.id, "generico-x1");
        let a55 = c.buscar("generico-a55").unwrap();
        assert_eq!(a55.midr, Some(0x412fd050));
        assert!(!a55.features.as_ref().unwrap().iter().any(|f| f == "ssbs"));
        // todos tienen id y nombre distintos
        let ids: std::collections::BTreeSet<&str> = c.dispositivos.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids.len(), c.dispositivos.len());
    }

    #[test]
    fn errores_con_su_linea() {
        let p = |t: &str| Dispositivo::parse(t, Origen::Usuario, &texto_integrado);
        assert!(p("dispositivo.id=x\nmaquina.nucleos=0\n").unwrap_err().contains('2'));
        assert!(p("dispositivo.id=x\npagina=8\n").is_err());
        assert!(p("dispositivo.id=x\ncpu.midr=0x1234567890\n").is_err());
        assert!(p("dispositivo.id=x\ncpu.features=fp ASIMD\n").is_err());
        assert!(p("dispositivo.id=x\nclave.rara=1\n").is_err());
        assert!(p("dispositivo.id=x\nlinea sin igual\n").is_err());
        assert!(p("dispositivo.id=ninguno\n").is_err());
        assert!(p("dispositivo.nombre=sin id\n").is_err());
        assert!(p("dispositivo.id=x\ndispositivo.formato=2\n").is_err());
        assert!(p("dispositivo.id=x\nproducto.modelo=con 'comilla'\n").is_err());
        assert!(p("dispositivo.id=x\ndispositivo.hereda=no-existe\n").is_err());
        // herencia en ciclo: se corta
        let ciclo = |id: &str| Some(format!("dispositivo.id={}\ndispositivo.hereda={}\n", id, id));
        assert!(Dispositivo::parse("dispositivo.id=a\ndispositivo.hereda=a\n", Origen::Usuario, &ciclo).is_err());
        // lo minimo: solo el id
        let d = p("dispositivo.id=minimo\n").unwrap();
        assert_eq!((d.nombre.as_str(), d.nucleos, d.features.clone(), d.pagina_kib), ("minimo", None, None, 4));
        assert_eq!(d.cpu_conf().lines().filter(|l| !l.starts_with('#')).count(), 0);
    }

    #[test]
    fn el_texto_se_vuelve_a_leer_igual() {
        for d in &integrados().dispositivos {
            let mut u = d.clone();
            u.origen = Origen::Usuario;
            let t = u.a_texto();
            let leido = Dispositivo::parse(&t, Origen::Usuario, &|_| None).unwrap();
            assert_eq!(leido, u, "{}", t);
            assert!(!t.contains("dispositivo.hereda"));
        }
        // extensiones: sin repetidas y en el orden de Linux, las desconocidas al final
        assert_eq!(lista_extensiones("sve,fp  fp,zzz aes").unwrap(), vec!["fp", "aes", "sve", "zzz"]);
        // lista relativa (todas con + o -): el ultimo de cada nombre gana y el orden ignora el signo
        let r = lista_extensiones("+sve2,-aes +sve,+aes").unwrap();
        assert_eq!(r, vec!["+aes", "+sve", "+sve2"]);
        assert!(es_relativa(&r) && !es_relativa(&lista_extensiones("fp +sve").unwrap()));
        assert!(lista_extensiones("+-fp").is_err() && lista_extensiones("+").is_err());
        let mut d = Dispositivo::vacio("x");
        d.fijar("cpu.features", "+sve -ssbs +rara").unwrap();
        assert_eq!(d.desconocidas(), vec!["rara"]);
        let mut d = Dispositivo::vacio("x");
        d.fijar("cpu.features", "fp otra").unwrap();
        assert_eq!(d.desconocidas(), vec!["otra"]);
        d.fijar("cpu.midr", &0x411fd411u32.to_string()).unwrap();
        assert_eq!(d.valor("cpu.midr"), "0x411fd411");
        d.fijar("cpu.midr", "").unwrap();
        assert_eq!(d.midr, None);
    }

    #[test]
    fn cpu_conf_y_bloque_de_propiedades() {
        let c = integrados();
        let mut d = c.buscar("generico-a78").unwrap().clone();
        let conf = d.cpu_conf();
        let lineas: Vec<&str> = conf.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(lineas[0], "nombre=Cortex-A78");
        assert_eq!(lineas[1], "base=cortex-a78");
        assert_eq!(lineas[2], "midr=0x411fd411");
        assert!(lineas[3].starts_with("features=fp,asimd,evtstrm,aes,"), "{}", lineas[3]);
        assert_eq!(lineas.len(), 4);
        // las claves exactas del contrato de heddle (docs de heddle: perfil-cpu.md), en su orden
        d.fijar("cpu.revidr", "0x10").unwrap();
        d.fijar("cpu.hardware", "Placa X").unwrap();
        d.fijar("cpu.base", "A76").unwrap();
        d.fijar("cpu.ctr", "0x8444_c004").unwrap();
        d.fijar("cpu.dczid", "4").unwrap();
        d.fijar("cpu.id_aa64isar0_el1", "0x0000_1000_0000_0000").unwrap();
        d.fijar("cpu.id_aa64mmfr2", "0x1").unwrap();
        let conf = d.cpu_conf();
        let lineas: Vec<&str> = conf.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(&lineas[..7], ["nombre=Cortex-A78", "hardware=Placa X", "base=a76", "midr=0x411fd411", "revidr=0x00000010", "ctr=0x8444c004", "dczid=0x4"]);
        assert!(lineas[7].starts_with("features="));
        assert_eq!(&lineas[8..], ["id_aa64isar0=0x0000100000000000", "id_aa64mmfr2=0x0000000000000001"]);
        assert_eq!(d.valor("cpu.id_aa64isar0"), "0x0000100000000000");
        assert!(d.fijar("cpu.base", "cortex-a710").is_err() && d.fijar("cpu.id_aa64isar0", "0x1_0000_0000_0000_0000").is_err());
        assert!(d.fijar("cpu.revidr", "0x1_0000_0000").is_err());
        // sin identidad: solo la marca y el tamano de pagina
        let lineas = |d: &Dispositivo| d.propiedades_con_marca().iter().map(|(k, v)| format!("{}={}", k, v)).collect::<Vec<_>>();
        assert_eq!(lineas(&d), vec![format!("{}={}", PROP_MARCA, d.marca()), "debug.heddle.page_size=4096".to_string()]);
        // con modelo y 16 KiB: la propiedad general y la de cada particion
        d.fijar("producto.modelo", "Modelo X 1").unwrap();
        d.fijar("pagina", "16").unwrap();
        let b = lineas(&d);
        for l in ["ro.product.model=Modelo X 1", "ro.product.vendor.model=Modelo X 1", "ro.product.system_ext.model=Modelo X 1", "debug.heddle.page_size=16384"] {
            assert!(b.iter().any(|x| x == l), "{:?}", b);
        }
        assert_eq!(b.iter().filter(|l| l.contains(".model=")).count(), 1 + PARTICIONES.len());
        assert!(!b.iter().any(|l| l.starts_with("ro.product.cpu.")));
        // la firma cambia con lo que se escribe y no con el nombre del perfil
        let f = d.firma();
        d.nombre = "otro".into();
        assert_eq!(d.firma(), f);
        d.fijar("cpu.features", "fp").unwrap();
        assert_ne!(d.firma(), f);
    }

    #[test]
    fn extensiones_desde_la_pantalla() {
        let mut d = Dispositivo::vacio("x");
        assert_eq!(d.modo_extensiones(), ModoExtensiones::DeLaBase);
        // sin lista, pulsar una empieza una lista de cambios; despues pasa por +, - y nada
        d.alternar_extension("sve");
        assert_eq!(d.features, Some(vec!["+sve".to_string()]));
        assert_eq!((d.modo_extensiones(), d.estado_extension("sve")), (ModoExtensiones::Cambios, Some('+')));
        d.alternar_extension("sve");
        assert_eq!(d.estado_extension("sve"), Some('-'));
        assert!(d.sin_soporte(&[]).is_empty(), "lo que se quita no cuenta");
        d.alternar_extension("sve");
        assert_eq!((d.estado_extension("sve"), d.modo_extensiones()), (None, ModoExtensiones::Cambios));
        assert!(!d.cpu_conf().contains("features="), "una lista de cambios vacia no se escribe");
        d.alternar_extension("bf16");
        d.alternar_extension("aes");
        assert_eq!(d.features, Some(vec!["+aes".to_string(), "+bf16".to_string()]));
        let sop = leer_soportadas("# traductor de prueba\nfp asimd aes  # comentario bf16\n\tsha1\n");
        assert_eq!(sop, vec!["fp", "asimd", "aes", "sha1"]);
        assert_eq!(d.sin_soporte(&sop), vec!["bf16"]);
        // a lista propia: lo añadido y lo que Linux siempre da
        d.poner_modo_extensiones(ModoExtensiones::Propia);
        assert_eq!(d.valor("cpu.features"), "fp asimd evtstrm aes cpuid bf16");
        d.alternar_extension("aes");
        d.alternar_extension("sb");
        assert_eq!((d.estado_extension("aes"), d.estado_extension("sb")), (None, Some('=')));
        assert_eq!(d.modo_extensiones(), ModoExtensiones::Propia);
        d.poner_modo_extensiones(ModoExtensiones::Cambios);
        assert!(d.features.as_ref().unwrap().iter().all(|x| x.starts_with('+')) && d.estado_extension("sb") == Some('+'));
        d.poner_modo_extensiones(ModoExtensiones::DeLaBase);
        assert_eq!(d.features, None);
        // el perfil sigue siendo valido tras cada cambio
        d.alternar_extension("sve2");
        assert!(Dispositivo::parse(&d.a_texto(), Origen::Usuario, &|_| None).is_ok());
    }

    fn archivo(ruta: &str, texto: &str) -> ArchivoProp {
        ArchivoProp { ruta: ruta.into(), texto: texto.into(), orig: None }
    }

    #[test]
    fn las_propiedades_se_sustituyen_en_su_sitio_y_se_reponen_exactas() {
        let cat = integrados();
        let mut d = cat.buscar("generico-a55").unwrap().clone();
        d.fijar("producto.modelo", "Modelo X").unwrap();
        d.fijar("producto.marca", "Marca").unwrap();
        let system = "# build.prop\nro.product.system.model=Original\nro.product.cpu.abilist=x86_64\nro.product.system.brand=Img\nro.x=1";
        let vendor = "ro.product.vendor.model=Vend\nro.product.vendor.model=Dup\nro.product.board=b\n";
        let product = "ro.product.product.name=nombre";
        let imagen = vec![archivo(PROP_SYSTEM, system), archivo("/vendor/build.prop", vendor), archivo(PROP_PRODUCT, product)];
        let p = plan_props(&imagen, Some(&d));
        // en su sitio, con las demas lineas intactas (tambien ro.product.cpu.* y las claves que el perfil no define)
        assert_eq!(p[0].texto, "# build.prop\nro.product.system.model=Modelo X\nro.product.cpu.abilist=x86_64\nro.product.system.brand=Marca\nro.x=1");
        assert!(p[1].texto.starts_with("ro.product.vendor.model=Modelo X\nro.product.vendor.model=Modelo X\nro.product.board=b\n"), "{}", p[1].texto);
        assert!(p[2].texto.starts_with("ro.product.product.name=nombre\n"));
        // lo que la imagen no tiene: en su particion; las generales, la pagina y la marca en product
        assert!(p[1].texto.contains("\nro.product.vendor.brand=Marca\n") && !p[1].texto.contains("ro.product.model="));
        assert!(p[2].texto.contains("\nro.product.model=Modelo X\n") && p[2].texto.contains(&format!("\n{}={}\n", PROP_MARCA, d.marca())));
        assert!(p[2].texto.contains("\ndebug.heddle.page_size=4096\n") && p[2].texto.contains("\nro.product.product.model=Modelo X\n"));
        // particiones sin archivo (odm, bootimage...): no se agregan en otro
        assert!(!p.iter().any(|a| a.texto.contains("ro.product.odm.") || a.texto.contains("ro.product.bootimage.")));
        // las originales quedan guardadas
        let o = Originales::leer(p[1].orig.as_deref().unwrap());
        assert_eq!(o.claves[0], ("ro.product.vendor.model".to_string(), vec!["ro.product.vendor.model=Vend".to_string(), "ro.product.vendor.model=Dup".to_string()]));
        assert_eq!(o.claves[1], ("ro.product.vendor.brand".to_string(), vec![]));
        assert!(Originales::leer(p[2].orig.as_deref().unwrap()).sin_salto_final, "product no acababa en salto de linea");
        assert!(!Originales::leer(p[1].orig.as_deref().unwrap()).sin_salto_final);
        // quitar el perfil repone exactamente la imagen (y borra los .weft-orig)
        let quitado = plan_props(&p, None);
        assert_eq!(quitado, imagen);
        // cambiar a otro perfil parte de la imagen: no se acumulan
        let x1 = cat.buscar("generico-x1").unwrap();
        let otro = plan_props(&p, Some(x1));
        assert_eq!(otro.iter().map(|a| a.texto.matches(PROP_MARCA).count()).sum::<usize>(), 1);
        assert!(otro[0].texto.contains("ro.product.system.model=Original") && otro[2].texto.contains(&x1.marca()));
        assert_eq!(plan_props(&otro, None), imagen);
        // aplicar dos veces lo mismo no cambia nada
        assert_eq!(plan_props(&p, Some(&d)), p);
        // sin product: lo general va a system
        let solo = plan_props(&[archivo(PROP_SYSTEM, "ro.build.id=X\n")], Some(&d));
        assert!(solo[0].texto.starts_with("ro.build.id=X\n") && solo[0].texto.contains("\nro.product.model=Modelo X\n") && solo[0].texto.contains("ro.product.system.brand=Marca"));
        // el bloque de la version anterior se quita
        let viejo = archivo(PROP_PRODUCT, "a=1\n# weft-dispositivo: inicio (perfil x)\nro.weft.dispositivo=x:1\n# weft-dispositivo: fin\n");
        assert_eq!(plan_props(&[viejo], None)[0], archivo(PROP_PRODUCT, "a=1\n"));
        assert_eq!(sin_bloque("a=1\n# weft-dispositivo: inicio\nro.x=2\n"), "a=1\n");
        // el formato de .weft-orig se lee igual
        let o = Originales { claves: vec![("k".into(), vec!["k = 1".into(), "k=2".into()]), ("n".into(), vec![])], sin_salto_final: true };
        assert_eq!(Originales::leer(&o.a_texto()), o);
    }

    #[test]
    fn catalogo_del_usuario_duplicar_guardar_y_borrar() {
        let dir = tmpdir("cat");
        let cat = Catalogo::cargar(Some(&dir));
        let d = duplicar(&dir, &cat, "generico-a78", "mio", None).unwrap();
        assert_eq!(d.nombre, txf!("dispositivo.copia_de", "Genérico A78"));
        assert!(dir.join("mio.device").is_file());
        // no se repiten ids ni se pisa uno integrado
        let cat = Catalogo::cargar(Some(&dir));
        assert!(duplicar(&dir, &cat, "generico-a78", "mio", None).is_err());
        assert!(duplicar(&dir, &cat, "generico-a78", "generico-a55", None).is_err());
        assert!(duplicar(&dir, &cat, "no-existe", "otro", None).is_err());
        assert_eq!(cat.id_libre("mio"), "mio-copia");
        let mut m = cat.buscar("mio").unwrap().clone();
        assert_eq!(m.origen, Origen::Usuario);
        m.fijar("maquina.nucleos", "3").unwrap();
        guardar(&dir, &mut m).unwrap();
        assert_eq!(Catalogo::cargar(Some(&dir)).buscar("mio").unwrap().nucleos, Some(3));
        // los integrados ni se guardan ni se borran
        let mut i = cat.buscar("generico-a55").unwrap().clone();
        assert!(guardar(&dir, &mut i).is_err() && borrar(&i).is_err());
        // un archivo roto no impide leer los demas; uno que hereda de otro del usuario
        std::fs::write(dir.join("roto.device"), "dispositivo.id=roto\npagina=3\n").unwrap();
        std::fs::write(dir.join("hijo.device"), "dispositivo.id=hijo\ndispositivo.hereda=mio\npagina=16\n").unwrap();
        let cat = Catalogo::cargar(Some(&dir));
        assert_eq!(cat.rechazados.len(), 1);
        let h = cat.buscar("hijo").unwrap();
        assert_eq!((h.nucleos, h.pagina_kib), (Some(3), 16));
        borrar(cat.buscar("mio").unwrap()).unwrap();
        assert!(Catalogo::cargar(Some(&dir)).buscar("mio").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn precedencia_de_nucleos_y_memoria() {
        let cat = integrados();
        let mut cfg = Config::nueva();
        assert_eq!(cfg.get("dispositivo.perfil"), NINGUNO);
        assert_eq!(cat.elegido(&cfg).unwrap(), None);
        assert_eq!(recursos(&cfg, None), (None, None));
        cfg.set("dispositivo.perfil", "generico-a55").unwrap();
        let d = cat.elegido(&cfg).unwrap();
        assert_eq!(recursos(&cfg, d), (Some(4), Some(4096)));
        // lo explicito de la configuracion manda sobre el perfil
        cfg.set("maquina.cpus", "2").unwrap();
        assert_eq!(recursos(&cfg, d), (Some(2), Some(4096)));
        cfg.set("maquina.ram", "3000").unwrap();
        assert_eq!(recursos(&cfg, d), (Some(2), Some(3000)));
        cfg.set("dispositivo.perfil", "no-esta").unwrap();
        assert!(cat.elegido(&cfg).is_err());
        assert!(cfg.set("dispositivo.perfil", "con espacio").is_err());
    }

    // ---- invitado simulado ------------------------------------------------------------------------------------

    struct Sim {
        t: f64,
        archivos: HashMap<String, String>,
        /// propiedades con que arranco Android (se recalculan al reiniciar)
        marca: String,
        /// Android no termina de arrancar mientras el bloque lleve esta marca
        no_arranca_con: Option<String>,
        arrancado: bool,
        verity: bool,
        reinicios: u32,
        ordenes: Vec<String>,
        /// la escritura de este archivo falla (particion de solo lectura)
        falla_escribir: Option<String>,
    }

    impl Sim {
        fn nuevo() -> Sim {
            let mut archivos = HashMap::new();
            archivos.insert(PROP_PRODUCT.to_string(), "ro.product.product.model=Original\n".to_string());
            archivos.insert(PROP_SYSTEM.to_string(), "ro.build.id=X\nro.product.system.model=Sistema\n".to_string());
            archivos.insert("/vendor/build.prop".to_string(), "ro.product.vendor.model=Vendor\nro.product.cpu.abilist=x86_64,arm64-v8a\n".to_string());
            Sim { t: 0.0, archivos, marca: String::new(), no_arranca_con: None, arrancado: true, verity: true, reinicios: 0, ordenes: Vec::new(), falla_escribir: None }
        }
        /// la marca con que arrancaria: la ultima que fije un build.prop, en el orden de init
        fn marca_de_archivo(&self) -> String {
            ARCHIVOS_PROP.iter().filter_map(|(_, r)| self.archivos.get(*r)).flat_map(|t| t.lines().filter_map(|l| l.strip_prefix(&format!("{}=", PROP_MARCA)).map(|s| s.to_string()))).last().unwrap_or_default()
        }
    }

    impl Invitado for Sim {
        fn esperar_arranque(&mut self, _: u64) -> Result<(), String> {
            if self.arrancado {
                Ok(())
            } else {
                Err("sin arranque".into())
            }
        }
        fn root(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn remontar(&mut self) -> Result<(), String> {
            if self.verity {
                Err("verity".into())
            } else {
                Ok(())
            }
        }
        fn desactivar_verity(&mut self) -> Result<bool, String> {
            self.verity = false;
            Ok(true)
        }
        fn reiniciar(&mut self) -> Result<(), String> {
            self.reinicios += 1;
            self.marca = self.marca_de_archivo();
            self.arrancado = self.no_arranca_con.as_deref() != Some(self.marca.as_str()) || self.marca.is_empty();
            Ok(())
        }
        fn arranque_completo(&mut self) -> bool {
            self.arrancado
        }
        fn pausa(&mut self, s: u64) {
            self.t += s as f64;
        }
        fn ahora(&self) -> f64 {
            self.t
        }
        fn sh(&mut self, cmd: &str) -> Result<(i32, String), String> {
            self.ordenes.push(cmd.to_string());
            if cmd.starts_with("getprop ") {
                return Ok((0, format!("{}\n", self.marca)));
            }
            if cmd == orden_leer() {
                let mut o = String::new();
                for (_, f) in ARCHIVOS_PROP {
                    if let Some(t) = self.archivos.get(f) {
                        o.push_str(&format!("{}prop {}\n{}\n", SEP, f, t));
                        if let Some(t) = self.archivos.get(&format!("{}{}", f, EXT_ORIG)) {
                            o.push_str(&format!("{}orig {}\n{}\n", SEP, f, t));
                        }
                    }
                }
                if let Some(c) = self.archivos.get(CPU_CONF) {
                    o.push_str(&format!("{}cpu\n{}\n", SEP, c));
                }
                o.push_str(&format!("{}fin\n", SEP));
                return Ok((0, o));
            }
            // escritura: `cat TMP > destino` y `rm -f X` unidos con &&; lo demas (chmod, mkdir, sync...) no cambia nada aqui
            for parte in cmd.split(" && ") {
                if let Some((t, d)) = parte.strip_prefix("cat ").and_then(|r| r.split_once(" > ")) {
                    let Some(c) = self.archivos.get(t).cloned() else { return Ok((1, format!("no existe {}", t))) };
                    if self.falla_escribir.as_deref() == Some(d) {
                        return Ok((1, format!("{}: sistema de archivos de solo lectura", d)));
                    }
                    self.archivos.insert(d.to_string(), c);
                } else if let Some(x) = parte.strip_prefix("rm -f ") {
                    self.archivos.remove(x);
                } else if !["mkdir ", "chmod ", "restorecon ", "sync", "{ rmdir "].iter().any(|p| parte.starts_with(p)) {
                    return Ok((1, format!("orden inesperada: {}", parte)));
                }
            }
            Ok((0, String::new()))
        }
        fn subir(&mut self, contenido: &[u8], remoto: &str) -> Result<(), String> {
            self.archivos.insert(remoto.to_string(), String::from_utf8(contenido.to_vec()).unwrap());
            Ok(())
        }
    }

    fn nada(_: &str) {}

    #[test]
    fn aplicar_cambiar_y_quitar() {
        let cat = integrados();
        let mut a78 = cat.buscar("generico-a78").unwrap().clone();
        a78.fijar("producto.modelo", "Modelo A").unwrap();
        let mut s = Sim::nuevo();
        let imagen = s.archivos.clone();
        // la primera vez: verity, reinicio para desactivarla, escritura, reinicio vigilado
        let r = aplicar(&mut s, Some(&a78), true, 120, &nada);
        assert_eq!(r, Ok(Resultado::Aplicado(vec![])));
        assert_eq!(s.reinicios, 2);
        assert_eq!(s.marca, a78.marca());
        assert_eq!(s.archivos[CPU_CONF], a78.cpu_conf());
        // en su sitio en cada build.prop, con las originales al lado
        assert!(s.archivos[PROP_PRODUCT].starts_with("ro.product.product.model=Modelo A\n"), "{}", s.archivos[PROP_PRODUCT]);
        assert!(s.archivos[PROP_SYSTEM].starts_with("ro.build.id=X\nro.product.system.model=Modelo A\n"));
        assert_eq!(s.archivos["/vendor/build.prop"], "ro.product.vendor.model=Modelo A\nro.product.cpu.abilist=x86_64,arm64-v8a\n");
        assert!(s.archivos[&format!("{}{}", PROP_SYSTEM, EXT_ORIG)].contains("\nro.product.system.model=Sistema\n"));
        // otra vez lo mismo: nada (ni reinicio)
        assert_eq!(aplicar(&mut s, Some(&a78), true, 120, &nada), Ok(Resultado::SinCambios));
        assert_eq!(s.reinicios, 2);
        // cambiar de perfil sin reiniciar: escrito, Android sigue con el anterior
        let a55 = cat.buscar("generico-a55").unwrap();
        assert_eq!(aplicar(&mut s, Some(a55), false, 120, &nada), Ok(Resultado::SinReiniciar));
        assert_eq!(s.marca, a78.marca());
        // el A55 no define identidad: vuelven las lineas de la imagen y solo queda lo suyo (pagina y marca)
        assert_eq!(s.archivos["/vendor/build.prop"], imagen["/vendor/build.prop"]);
        assert!(!s.archivos.contains_key(&format!("/vendor/build.prop{}", EXT_ORIG)));
        // aplicar ya con los archivos escritos: solo reinicia
        assert_eq!(aplicar(&mut s, Some(a55), true, 120, &nada), Ok(Resultado::Aplicado(vec![])));
        assert_eq!((s.reinicios, s.marca.clone()), (3, a55.marca()));
        // quitar: la imagen queda exactamente como estaba (sin .weft-orig ni cpu.conf ni temporales)
        assert_eq!(aplicar(&mut s, None, true, 120, &nada), Ok(Resultado::Aplicado(vec![])));
        assert_eq!(s.archivos, imagen);
        assert!(s.marca.is_empty());
        assert_eq!(aplicar(&mut s, None, true, 120, &nada), Ok(Resultado::SinCambios));
    }

    #[test]
    fn sin_particion_product_usa_la_de_system() {
        let cat = integrados();
        let mut s = Sim::nuevo();
        s.archivos.remove(PROP_PRODUCT);
        s.verity = false;
        let x1 = cat.buscar("generico-x1").unwrap();
        assert_eq!(aplicar(&mut s, Some(x1), true, 120, &nada), Ok(Resultado::Aplicado(vec![])));
        assert!(s.archivos[PROP_SYSTEM].starts_with("ro.build.id=X\nro.product.system.model=Sistema\n") && s.archivos[PROP_SYSTEM].contains(&x1.marca()));
        assert!(!s.archivos.contains_key(PROP_PRODUCT));
        assert_eq!(s.reinicios, 1);
    }

    #[test]
    fn una_escritura_que_falla_se_deshace() {
        let mut d = integrados().buscar("generico-a78").unwrap().clone();
        d.fijar("producto.modelo", "Modelo A").unwrap();
        let mut s = Sim::nuevo();
        s.verity = false;
        let antes = s.archivos.clone();
        s.falla_escribir = Some(PROP_PRODUCT.to_string());
        let r = aplicar(&mut s, Some(&d), true, 120, &nada);
        assert!(matches!(r, Err(Error::Previo(_))), "{:?}", r);
        // system y vendor se escribieron antes del fallo y se devolvieron; ningun temporal queda
        s.archivos.retain(|k, _| !k.starts_with(TMP));
        assert_eq!(s.archivos, antes);
        assert_eq!(s.reinicios, 0);
    }

    #[test]
    fn si_android_no_arranca_se_restaura_lo_anterior() {
        let cat = integrados();
        let a78 = cat.buscar("generico-a78").unwrap();
        let mut s = Sim::nuevo();
        s.verity = false;
        s.no_arranca_con = Some(a78.marca());
        let antes = s.archivos.clone();
        let r = aplicar(&mut s, Some(a78), true, 60, &nada);
        assert!(matches!(r, Ok(Resultado::Restaurado(_))), "{:?}", r);
        assert_eq!(codigo(&r), 2);
        assert_eq!(s.archivos, antes);
        assert!(s.arrancado && s.marca.is_empty());
        // la sincronizacion tras el arranque no lo reintenta en bucle
        let estado = tmpdir("sinc");
        let rutas = crate::rutas::Rutas { datos: estado.clone(), config: estado.join("config"), cache: estado.clone(), registros: estado.clone(), ejecucion: estado.clone(), estado_propio: false };
        let mut cfg = Config::nueva();
        cfg.set("dispositivo.perfil", "generico-a78").unwrap();
        let l = sincronizar(&mut s, &estado, &cfg, &rutas, &nada).unwrap();
        assert_eq!(l, describir(&Ok(Resultado::Restaurado(txf!("dispositivo.no_arranco_en", LIMITE_ARRANQUE_S)))));
        assert_eq!(std::fs::read_to_string(estado.join(ARCHIVO_FALLO)).unwrap().trim(), a78.marca());
        let reinicios = s.reinicios;
        assert_eq!(sincronizar(&mut s, &estado, &cfg, &rutas, &nada), Some(tx!("dispositivo.pendiente_por_fallo").to_string()));
        assert_eq!(s.reinicios, reinicios);
        // con otro perfil si se aplica, y despues ya no hay nada que hacer
        cfg.set("dispositivo.perfil", "generico-a55").unwrap();
        assert!(sincronizar(&mut s, &estado, &cfg, &rutas, &nada).is_some());
        assert_eq!(s.marca, cat.buscar("generico-a55").unwrap().marca());
        assert!(!estado.join(ARCHIVO_FALLO).exists());
        assert_eq!(sincronizar(&mut s, &estado, &cfg, &rutas, &nada), None);
        // sin perfil y sin nada en Android: nada (sin root ni escritura: solo el getprop)
        let mut limpio = Sim::nuevo();
        assert_eq!(sincronizar(&mut limpio, &estado, &Config::nueva(), &rutas, &nada), None);
        assert_eq!(limpio.ordenes, vec![format!("getprop {}", PROP_MARCA)]);
        let _ = std::fs::remove_dir_all(&estado);
    }

    #[test]
    fn sin_adb_no_se_toca_nada() {
        let mut s = Sim::nuevo();
        s.arrancado = false;
        let r = aplicar(&mut s, integrados().buscar("generico-a78"), true, 60, &nada);
        assert!(matches!(r, Err(Error::Previo(_))));
        assert_eq!(codigo(&r), 1);
        assert!(s.ordenes.is_empty());
    }

    #[test]
    fn la_lectura_del_invitado() {
        let a = parsear_archivos("@@weft@@ prop /system/build.prop\na=1\nb=2\n\n@@weft@@ prop /product/etc/build.prop\nc=3\n@@weft@@ orig /product/etc/build.prop\n=c\nc=0\n\n@@weft@@ fin\n").unwrap();
        assert_eq!(a.props, vec![archivo(PROP_SYSTEM, "a=1\nb=2\n"), ArchivoProp { ruta: PROP_PRODUCT.into(), texto: "c=3".into(), orig: Some("=c\nc=0\n".into()) }]);
        assert_eq!(a.cpu, None);
        let a = parsear_archivos("@@weft@@ prop /system/build.prop\n\n@@weft@@ cpu\nnombre=x\n\n@@weft@@ fin\n").unwrap();
        assert_eq!((a.props[0].texto.as_str(), a.cpu.as_deref()), ("", Some("nombre=x\n")));
        // sin la marca de fin (salida cortada), sin ningun build.prop o con algo raro: error
        assert!(parsear_archivos("basura").is_err());
        assert!(parsear_archivos("@@weft@@ prop /system/build.prop\na=1\n").is_err());
        assert!(parsear_archivos("@@weft@@ fin\n").is_err());
        assert!(parsear_archivos("@@weft@@ orig /x\na\n@@weft@@ fin\n").is_err());
        assert!(orden_leer().contains(PROP_PRODUCT) && orden_leer().contains(CPU_CONF) && orden_leer().contains(EXT_ORIG));
    }

    #[test]
    fn el_cpu_features_del_traductor() {
        let d = tmpdir("sop");
        let (lib, estado) = (d.join("traductor"), d.join("estado"));
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::create_dir_all(&estado).unwrap();
        let so = lib.join("libtraductor.so");
        std::fs::write(&so, b"x").unwrap();
        // sin archivo junto a la biblioteca: nada que avisar
        assert_eq!(copiar_soportadas(&so, &estado), Ok(false));
        assert_eq!(soportadas_de_maquina(&estado), None);
        std::fs::write(lib.join(CPU_FEATURES), "fp asimd\n# sin sve\naes\n").unwrap();
        assert_eq!(copiar_soportadas(&so, &estado), Ok(true));
        assert_eq!(soportadas_de_maquina(&estado), Some(vec!["fp".to_string(), "asimd".into(), "aes".into()]));
        // un traductor nuevo sin el archivo borra la copia anterior
        std::fs::remove_file(lib.join(CPU_FEATURES)).unwrap();
        assert_eq!(copiar_soportadas(&so, &estado), Ok(false));
        assert_eq!(soportadas_de_maquina(&estado), None);
        assert_eq!(olvidar_soportadas(&estado), Ok(()));
        let _ = std::fs::remove_dir_all(&d);
    }
}
