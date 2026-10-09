//! Perfiles de imagen de Android: como se arranca una familia de imagenes, como DATO declarativo (clave=valor).
//!
//! Hay perfiles integrados en el ejecutable (`perfiles/*.profile`, incrustados) y perfiles del usuario en
//! `$XDG_CONFIG_HOME/weft/profiles/*.profile` (se agregan copiando un archivo, sin recompilar; mismo id: manda el del
//! usuario con aviso). La logica (QEMU, GPT, bootconfig, adb) sigue en Rust y se elige con palabras cerradas: el archivo no
//! contiene codigo. Un valor o clave desconocida rechaza el perfil con el numero de linea; nunca se adivina.
//!
//! Formato: una clave por linea, `#` solo al principio de linea (comentario). Claves repetibles: `detectar.requiere`,
//! `detectar.puntua` (cada linea admite varias condiciones separadas por espacios), `disco.particion`,
//! `disco.particion_ab` y `disco.datos` (un solo orden para las tres: el del archivo) y `bc.CLAVE=VALOR` (parametro de
//! arranque de Android). `perfil.hereda=ID` toma todo de otro perfil: las claves de un solo valor se sobrescriben, un grupo
//! repetible que el hijo define reemplaza al del padre y `bc.*` se sobrescribe clave a clave.
//!
//! Este modulo no toca la maquina ni el disco salvo `preparar_arranque` (extrae kernel y discos RAM a la cache).

use crate::gpt;
use crate::vm::Accel;
use std::path::{Path, PathBuf};
use crate::textos::{tx, txf};

pub const FORMATO: u32 = 1;

/// Archivos incrustados en el ejecutable: (nombre, contenido).
const INTEGRADOS: &[(&str, &str)] = &[("phone-x86_64", include_str!("../perfiles/phone-x86_64.profile")), ("car-x86_64", include_str!("../perfiles/car-x86_64.profile"))];
/// Archivos que un perfil integrado puede nombrar en `arranque.bootconfig_archivo`.
const ASSETS: &[(&str, &[u8])] = &[("cuttlefish.bootconfig", include_bytes!("../perfiles/cuttlefish.bootconfig"))];

/// Perfil que se usa cuando una orden no puede detectar uno (carpetas de imagen sueltas con `disk`).
pub const ID_DEFECTO: &str = "phone-x86_64";

/// Identificador de imagen o de perfil: letras, numeros, `.`, `_` y `-`; no empieza por punto ni es `auto`.
pub fn validar_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 64 || id.starts_with('.') || id == "auto" || !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
        return Err(txf!("perfil.un_identificador_lleva_letras_numeros_o", format!("{:?}", id)));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arq {
    X86_64,
    Arm64,
}

impl Arq {
    pub fn parse(t: &str) -> Option<Arq> {
        match t {
            "x86_64" => Some(Arq::X86_64),
            "arm64" => Some(Arq::Arm64),
            _ => None,
        }
    }
    pub fn nombre(self) -> &'static str {
        match self {
            Arq::X86_64 => "x86_64",
            Arq::Arm64 => "arm64",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cond {
    Archivo(String),
    Kernel(Arq),
    AndroidInfo(String, String),
    VendorBootconfig(String, String),
}

impl Cond {
    fn parse(t: &str) -> Result<Cond, String> {
        let (tipo, v) = t.split_once(':').ok_or_else(|| txf!("perfil.condicion_se_espera_tipo_valor_archivo", format!("{:?}", t)))?;
        let par = |v: &str| -> Result<(String, String), String> {
            let (k, x) = v.split_once('=').ok_or_else(|| txf!("perfil.condicion_se_espera_clave_valor", format!("{:?}", t)))?;
            if k.is_empty() {
                return Err(txf!("perfil.condicion_clave_vacia", format!("{:?}", t)));
            }
            Ok((k.to_string(), x.to_string()))
        };
        match tipo {
            "archivo" if !v.is_empty() && !v.contains('/') => Ok(Cond::Archivo(v.to_string())),
            "archivo" => Err(txf!("perfil.condicion_nombre_de_archivo_no_valido", format!("{:?}", t))),
            "kernel" => Arq::parse(v).map(Cond::Kernel).ok_or_else(|| txf!("perfil.condicion_arquitectura_desconocida_x86", format!("{:?}", t))),
            "android_info" => par(v).map(|(k, x)| Cond::AndroidInfo(k, x)),
            "vendor_bootconfig" => par(v).map(|(k, x)| Cond::VendorBootconfig(k, x)),
            otro => Err(txf!("perfil.condicion_tipo_desconocido_archivo", format!("{:?}", t), format!("{:?}", otro))),
        }
    }

    /// La condicion en lenguaje claro (para explicar por que un perfil no encaja).
    pub fn texto(&self) -> String {
        match self {
            Cond::Archivo(f) => txf!("perfil.falta_el_archivo", f),
            Cond::Kernel(a) => txf!("perfil.el_kernel_no_es", a.nombre()),
            Cond::AndroidInfo(k, v) => txf!("perfil.android_info_txt_no_dice", k, v),
            Cond::VendorBootconfig(k, v) => txf!("perfil.el_arranque_del_fabricante_no_declara", k, v),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ParteDisco {
    /// particion vacia: nombre y tamano (`1M`)
    Vacia(String, String),
    /// particion con el contenido de un archivo de la imagen
    Archivo(String, String),
    /// nombres: cada uno va en NOMBRE_a y NOMBRE_b con el contenido de NOMBRE.img
    Ab(Vec<String>),
    /// particion de datos: nombre y archivo (`userdata.img`) que se usa con `disk.data=img`; si no, queda vacia
    Datos(String, String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origen {
    Integrado,
    Usuario,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Perfil {
    pub id: String,
    pub nombre: String,
    pub descripcion: String,
    /// advertencia que `start` e `image info` muestran (limitaciones conocidas de la familia)
    pub aviso: String,
    pub familia: String,
    pub arquitectura: Arq,
    pub origen: Origen,
    pub requiere: Vec<Cond>,
    pub puntua: Vec<Cond>,
    pub ranura_pci: u32,
    pub disco: Vec<ParteDisco>,
    pub boot: String,
    pub init_boot: String,
    pub vendor_boot: String,
    pub cmdline_extra: String,
    pub bootconfig_archivo: Option<String>,
    pub bc: Vec<String>,
    pub acel: Accel,
    pub consola_hvc: bool,
    pub hvc: u32,
    pub registro: Option<u32>,
    pub sensores: Option<(u32, u32)>,
    pub tactil: bool,
    pub tarjetas: u32,
    pub gpu_opciones: String,
    /// intenciones de grafico que la imagen admite: hardware (aceleracion por hardware) y software
    pub motores: Vec<String>,
    pub ram_minima: u32,
    pub tipos_maquina: Vec<String>,
    pub adb_transportes: Vec<String>,
    pub adb_puerto: u32,
    pub adb_tcp_red: u32,
    pub cap: Vec<(String, String)>,
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

/// Grupo de una clave para la herencia: las del mismo grupo se reemplazan juntas.
fn grupo(k: &str) -> Option<&'static str> {
    match k {
        "detectar.requiere" => Some("requiere"),
        "detectar.puntua" => Some("puntua"),
        "disco.particion" | "disco.particion_ab" | "disco.datos" => Some("disco"),
        _ => None,
    }
}

fn fusionar(padre: Vec<Par>, hijo: Vec<Par>) -> Vec<Par> {
    let mut r: Vec<Par> = padre.into_iter().filter(|(k, _, _)| k != "perfil.hereda").collect();
    let grupos_hijo: Vec<&str> = hijo.iter().filter_map(|(k, _, _)| grupo(k)).collect();
    r.retain(|(k, _, _)| grupo(k).map_or(true, |g| !grupos_hijo.contains(&g)));
    for h in hijo {
        if h.0 == "perfil.hereda" {
            continue;
        }
        if grupo(&h.0).is_none() {
            if let Some(i) = r.iter().position(|(k, _, _)| *k == h.0) {
                r[i] = h;
                continue;
            }
        }
        r.push(h);
    }
    r
}

fn entero(v: &str, clave: &str, linea: usize, min: u32, max: u32) -> Result<u32, String> {
    let n: u32 = v.parse().map_err(|_| txf!("perfil.linea_se_espera_un_numero_entero", linea, clave))?;
    if !(min..=max).contains(&n) {
        return Err(txf!("perfil.linea_fuera_de_rango_a", linea, clave, min, max));
    }
    Ok(n)
}

fn si_no(v: &str, clave: &str, linea: usize) -> Result<bool, String> {
    match v {
        "si" => Ok(true),
        "no" => Ok(false),
        _ => Err(txf!("perfil.linea_se_espera_si_o_no", linea, clave)),
    }
}

fn nombre_particion(n: &str) -> bool {
    !n.is_empty() && n.len() <= 36 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn nombre_archivo(n: &str) -> bool {
    !n.is_empty() && !n.contains('/') && !n.starts_with('.') && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

impl Perfil {
    /// Lee un perfil. `buscar_padre` devuelve el TEXTO de otro perfil por su id (para `perfil.hereda`).
    pub fn parse(texto: &str, origen: Origen, buscar_padre: &dyn Fn(&str) -> Option<String>) -> Result<Perfil, String> {
        Perfil::parse_nivel(texto, origen, buscar_padre, 0)
    }

    fn parse_nivel(texto: &str, origen: Origen, buscar_padre: &dyn Fn(&str) -> Option<String>, nivel: u32) -> Result<Perfil, String> {
        let propios = pares(texto)?;
        let mut todos = propios.clone();
        if let Some((_, padre_id, linea)) = propios.iter().find(|(k, _, _)| k == "perfil.hereda") {
            if nivel >= 4 {
                return Err(tx!("perfil.perfil_hereda_demasiados_niveles_o_un").into());
            }
            validar_id(padre_id).map_err(|e| txf!("perfil.linea_perfil_hereda", linea, e))?;
            let tp = buscar_padre(padre_id).ok_or_else(|| txf!("perfil.linea_perfil_hereda_no_existe_el_perfil", linea, format!("{:?}", padre_id)))?;
            // el padre se expande a sus propios pares (con su herencia) volviendo a leer su texto
            let padre = Perfil::expandir(&tp, buscar_padre, nivel + 1).map_err(|e| txf!("perfil.perfil", format!("{:?}", padre_id), e))?;
            todos = fusionar(padre, propios);
        }
        Perfil::interpretar(&todos, origen)
    }

    fn expandir(texto: &str, buscar_padre: &dyn Fn(&str) -> Option<String>, nivel: u32) -> Result<Vec<Par>, String> {
        let propios = pares(texto)?;
        if let Some((_, padre_id, linea)) = propios.iter().find(|(k, _, _)| k == "perfil.hereda") {
            if nivel >= 4 {
                return Err(tx!("perfil.perfil_hereda_demasiados_niveles_o_un").into());
            }
            validar_id(padre_id).map_err(|e| txf!("perfil.linea_perfil_hereda", linea, e))?;
            let tp = buscar_padre(padre_id).ok_or_else(|| txf!("perfil.linea_perfil_hereda_no_existe_el_perfil", linea, format!("{:?}", padre_id)))?;
            let padre = Perfil::expandir(&tp, buscar_padre, nivel + 1)?;
            return Ok(fusionar(padre, propios));
        }
        Ok(propios)
    }

    fn interpretar(todos: &[Par], origen: Origen) -> Result<Perfil, String> {
        let mut p = Perfil {
            id: String::new(),
            nombre: String::new(),
            descripcion: String::new(),
            aviso: String::new(),
            familia: String::new(),
            arquitectura: Arq::X86_64,
            origen,
            requiere: Vec::new(),
            puntua: Vec::new(),
            ranura_pci: 4,
            disco: Vec::new(),
            boot: "boot.img".into(),
            init_boot: "init_boot.img".into(),
            vendor_boot: "vendor_boot.img".into(),
            cmdline_extra: String::new(),
            bootconfig_archivo: None,
            bc: Vec::new(),
            acel: Accel::Auto,
            consola_hvc: true,
            hvc: 0,
            registro: None,
            sensores: None,
            tactil: false,
            tarjetas: 1,
            gpu_opciones: String::new(),
            motores: vec!["hardware".into(), "software".into()],
            ram_minima: 2048,
            tipos_maquina: vec!["q35".into(), "pc".into()],
            adb_transportes: vec!["vsock".into(), "tcp".into()],
            adb_puerto: 5555,
            adb_tcp_red: 0,
            cap: Vec::new(),
        };
        let (mut tiene_modo, mut tiene_disco_modo) = (false, false);
        for (k, v, n) in todos {
            let n = *n;
            let k = k.as_str();
            let v = v.as_str();
            match k {
                "perfil.id" => {
                    validar_id(v).map_err(|e| txf!("perfil.linea_perfil_id", n, e))?;
                    p.id = v.to_string();
                }
                "perfil.nombre" => p.nombre = v.to_string(),
                "perfil.descripcion" => p.descripcion = v.to_string(),
                "perfil.aviso" => p.aviso = v.to_string(),
                "perfil.familia" => p.familia = v.to_string(),
                "perfil.formato" => {
                    if entero(v, k, n, 1, 1000)? != FORMATO {
                        return Err(txf!("perfil.linea_perfil_formato_este_programa", n, v, FORMATO));
                    }
                }
                "perfil.arquitectura" => p.arquitectura = Arq::parse(v).ok_or_else(|| txf!("perfil.linea_perfil_arquitectura_se_espera_x86", n))?,
                "detectar.requiere" | "detectar.puntua" => {
                    for t in v.split_whitespace() {
                        let c = Cond::parse(t).map_err(|e| txf!("perfil.linea", n, e))?;
                        if k == "detectar.requiere" { p.requiere.push(c) } else { p.puntua.push(c) }
                    }
                }
                "disco.modo" => {
                    if v != "gpt" {
                        return Err(txf!("perfil.linea_disco_modo_solo_se_admite_gpt_un", n, v));
                    }
                    tiene_disco_modo = true;
                }
                "disco.ranura_pci" => p.ranura_pci = entero(v, k, n, 2, 26)?,
                "disco.particion" => {
                    let (nom, r) = v.split_once(':').ok_or_else(|| txf!("perfil.linea_disco_particion_se_espera_nombre", n))?;
                    if !nombre_particion(nom) {
                        return Err(txf!("perfil.linea_disco_particion_nombre_de", n, format!("{:?}", nom)));
                    }
                    let es_tamano = r.chars().last().is_some_and(|c| matches!(c, 'K' | 'M' | 'G')) && r[..r.len() - 1].parse::<u64>().is_ok();
                    if es_tamano {
                        p.disco.push(ParteDisco::Vacia(nom.to_string(), r.to_string()));
                    } else if nombre_archivo(r) {
                        p.disco.push(ParteDisco::Archivo(nom.to_string(), r.to_string()));
                    } else {
                        return Err(txf!("perfil.linea_disco_particion_no_es_un_tamano", n, format!("{:?}", r)));
                    }
                }
                "disco.particion_ab" => {
                    let nombres: Vec<String> = v.split_whitespace().map(|s| s.to_string()).collect();
                    if nombres.is_empty() || !nombres.iter().all(|s| nombre_particion(s)) {
                        return Err(txf!("perfil.linea_disco_particion_ab_se_espera_una", n));
                    }
                    p.disco.push(ParteDisco::Ab(nombres));
                }
                "disco.datos" => {
                    let (nom, f) = v.split_once(':').ok_or_else(|| txf!("perfil.linea_disco_datos_se_espera_nombre", n))?;
                    if !nombre_particion(nom) || !nombre_archivo(f) {
                        return Err(txf!("perfil.linea_disco_datos_valor_no_valido", n));
                    }
                    p.disco.push(ParteDisco::Datos(nom.to_string(), f.to_string()));
                }
                "arranque.modo" => {
                    if v != "directo-android" {
                        return Err(txf!("perfil.linea_arranque_modo_solo_se_admite", n, v));
                    }
                    tiene_modo = true;
                }
                "arranque.boot" | "arranque.init_boot" | "arranque.vendor_boot" => {
                    if !nombre_archivo(v) {
                        return Err(txf!("perfil.linea_nombre_de_archivo_no_valido", n, k));
                    }
                    match k {
                        "arranque.boot" => p.boot = v.to_string(),
                        "arranque.init_boot" => p.init_boot = v.to_string(),
                        _ => p.vendor_boot = v.to_string(),
                    }
                }
                "arranque.cmdline_extra" => p.cmdline_extra = v.to_string(),
                "arranque.bootconfig_archivo" => {
                    if !nombre_archivo(v) {
                        return Err(txf!("perfil.linea_arranque_bootconfig_archivo_nombre", n));
                    }
                    p.bootconfig_archivo = Some(v.to_string());
                }
                "aceleracion" => {
                    p.acel = match v {
                        "auto" => Accel::Auto,
                        "kvm" => Accel::Kvm,
                        "tcg" => Accel::Tcg,
                        _ => return Err(txf!("perfil.linea_aceleracion_se_espera_auto_kvm_o", n)),
                    }
                }
                "consola" => {
                    p.consola_hvc = match v {
                        "hvc" => true,
                        "serial" => false,
                        _ => return Err(txf!("perfil.linea_consola_se_espera_hvc_o_serial", n)),
                    }
                }
                "consola.hvc" => p.hvc = entero(v, k, n, 0, 30)?,
                "consola.registro" => p.registro = Some(entero(v, k, n, 1, 30)?),
                "sensores" => {
                    p.sensores = if v == "ninguno" {
                        None
                    } else {
                        let (a, b) = v.split_once(',').ok_or_else(|| txf!("perfil.linea_sensores_se_espera_control_datos_o", n))?;
                        Some((entero(a, k, n, 1, 30)?, entero(b, k, n, 1, 30)?))
                    }
                }
                "entrada.tactil" => p.tactil = si_no(v, k, n)?,
                "red.tarjetas" => p.tarjetas = entero(v, k, n, 1, 4)?,
                "gpu.opciones" => {
                    if v.chars().any(|c| c.is_control() || c == ' ') {
                        return Err(txf!("perfil.linea_gpu_opciones_propiedades_separadas", n));
                    }
                    p.gpu_opciones = v.to_string();
                }
                "gpu.motores" => {
                    let t: Vec<String> = v.split_whitespace().map(|s| s.to_string()).collect();
                    if t.is_empty() || !t.iter().all(|s| s == "hardware" || s == "software") {
                        return Err(txf!("perfil.linea_gpu_motores_se_espera_hardware", n));
                    }
                    p.motores = t;
                }
                "maquina.ram_minima" => p.ram_minima = entero(v, k, n, 64, 1 << 20)?,
                "maquina.tipos" => {
                    let t: Vec<String> = v.split_whitespace().map(|s| s.to_string()).collect();
                    if t.is_empty() || !t.iter().all(|s| s == "q35" || s == "pc") {
                        return Err(txf!("perfil.linea_maquina_tipos_se_espera_q35_pc_o", n));
                    }
                    p.tipos_maquina = t;
                }
                "adb.transportes" => {
                    let t: Vec<String> = v.split_whitespace().map(|s| s.to_string()).collect();
                    if t.is_empty() || !t.iter().all(|s| s == "vsock" || s == "tcp") {
                        return Err(txf!("perfil.linea_adb_transportes_se_espera_vsock", n));
                    }
                    p.adb_transportes = t;
                }
                "adb.puerto" => p.adb_puerto = entero(v, k, n, 1, 65535)?,
                "adb.tcp_red" => p.adb_tcp_red = entero(v, k, n, 0, 3)?,
                _ if k.starts_with("capacidad.") => {
                    let nombre = &k["capacidad.".len()..];
                    let ok: &[&str] = match nombre {
                        "root" => &["modulo-gki", "parche-ramdisk", "ninguna"],
                        "carpetas" => &["init-rc", "fstab", "ninguna"],
                        "traductor" => &["native-bridge", "ninguna"],
                        "mandos" => &["virtio-input-host", "ninguna"],
                        "resolucion" => &["drm-surfaceflinger", "ninguna"],
                        _ => return Err(txf!("perfil.linea_capacidad_desconocida_root", n, format!("{:?}", nombre))),
                    };
                    if !ok.contains(&v) {
                        return Err(txf!("perfil.linea_se_espera", n, k, ok.join(", ")));
                    }
                    p.cap.retain(|(c, _)| c != nombre);
                    p.cap.push((nombre.to_string(), v.to_string()));
                }
                _ if k.starts_with("bc.") => {
                    let clave = &k[3..];
                    if clave.is_empty() || clave.contains(char::is_whitespace) {
                        return Err(txf!("perfil.linea_bc_clave_clave_no_valida", n));
                    }
                    let l = format!("{}={}", clave, v);
                    match p.bc.iter().position(|x| x.split('=').next() == Some(clave)) {
                        Some(i) => p.bc[i] = l,
                        None => p.bc.push(l),
                    }
                }
                otra => return Err(txf!("perfil.linea_clave_desconocida_no_se_adivina", n, format!("{:?}", otra))),
            }
        }
        if p.id.is_empty() {
            return Err(tx!("perfil.falta_perfil_id").into());
        }
        if p.nombre.is_empty() {
            return Err(tx!("perfil.falta_perfil_nombre").into());
        }
        if p.familia.is_empty() {
            p.familia = p.id.clone();
        }
        if !tiene_modo {
            return Err(tx!("perfil.falta_arranque_modo_directo_android").into());
        }
        if !tiene_disco_modo {
            return Err(tx!("perfil.falta_disco_modo_gpt").into());
        }
        if !p.disco.iter().any(|d| matches!(d, ParteDisco::Datos(..))) {
            return Err(tx!("perfil.falta_disco_datos_nombre_archivo_la").into());
        }
        if p.consola_hvc {
            if let Some(r) = p.registro {
                if r > p.hvc {
                    return Err(txf!("perfil.consola_registro_esta_fuera_de_consola", r, p.hvc));
                }
            }
            if let Some((a, b)) = p.sensores {
                if a == b || a > p.hvc || b > p.hvc {
                    return Err(tx!("perfil.sensores_las_dos_consolas_deben_ser").into());
                }
            }
        } else if p.hvc != 0 || p.sensores.is_some() || p.registro.is_some() {
            return Err(tx!("perfil.consola_hvc_consola_registro_y_sensores").into());
        }
        Ok(p)
    }

    /// Archivos .img que necesita la imagen para armar el disco y arrancar: los de las particiones y los de arranque.
    pub fn archivos_requeridos(&self) -> Vec<String> {
        let mut v: Vec<String> = vec![self.boot.clone(), self.init_boot.clone(), self.vendor_boot.clone()];
        for d in &self.disco {
            match d {
                ParteDisco::Archivo(_, f) | ParteDisco::Datos(_, f) => v.push(f.clone()),
                ParteDisco::Ab(ns) => v.extend(ns.iter().map(|n| format!("{}.img", n))),
                ParteDisco::Vacia(..) => {}
            }
        }
        let mut vistos = Vec::new();
        v.retain(|f| {
            let nuevo = !vistos.contains(f);
            vistos.push(f.clone());
            nuevo
        });
        v
    }

    /// Particiones del disco en el orden del perfil. `datos`: tamano de la particion de datos (`24G`) o `None` para usar su archivo.
    pub fn partes(&self, imagen: &Path, datos: Option<&str>) -> Result<Vec<gpt::Part>, String> {
        let faltan: Vec<String> = self.archivos_requeridos().into_iter().filter(|f| !imagen.join(f).is_file()).collect();
        if !faltan.is_empty() {
            return Err(txf!("perfil.a_la_imagen_de_le_faltan", imagen.display(), faltan.join(", ")));
        }
        let ruta = |f: &str| imagen.join(f).to_string_lossy().into_owned();
        let mut specs: Vec<String> = Vec::new();
        for d in &self.disco {
            match d {
                ParteDisco::Vacia(n, t) => specs.push(format!("{}={}", n, t)),
                ParteDisco::Archivo(n, f) => specs.push(format!("{}={}", n, ruta(f))),
                ParteDisco::Ab(ns) => {
                    for n in ns {
                        for s in ["_a", "_b"] {
                            specs.push(format!("{}{}={}", n, s, ruta(&format!("{}.img", n))));
                        }
                    }
                }
                ParteDisco::Datos(n, f) => specs.push(match datos {
                    None => format!("{}={}", n, ruta(f)),
                    Some(t) => format!("{}={}", n, t),
                }),
            }
        }
        specs.iter().map(|s| gpt::parse_part(s)).collect()
    }

    /// Lineas de bootconfig del perfil: las del archivo que nombra (si hay) y despues las `bc.*`.
    pub fn bootconfig_propio(&self, carpeta_usuario: Option<&Path>) -> Result<Vec<String>, String> {
        let mut v = Vec::new();
        if let Some(a) = &self.bootconfig_archivo {
            let raw = match ASSETS.iter().find(|(n, _)| n == a) {
                Some((_, b)) if self.origen == Origen::Integrado => b.to_vec(),
                _ => match carpeta_usuario {
                    Some(d) => std::fs::read(d.join(a)).map_err(|e| format!("arranque.bootconfig_archivo: {}: {}", d.join(a).display(), e))?,
                    None => ASSETS.iter().find(|(n, _)| n == a).map(|(_, b)| b.to_vec()).ok_or_else(|| txf!("perfil.arranque_bootconfig_archivo_no_se", a))?,
                },
            };
            v.extend(crate::vm::bootconfig_lines(&raw));
        }
        v.extend(self.bc.iter().cloned());
        Ok(v)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// catalogo: integrados y del usuario

pub struct Catalogo {
    pub perfiles: Vec<Perfil>,
    /// perfiles de usuario que no se pudieron leer: (archivo, motivo)
    pub rechazados: Vec<(String, String)>,
    /// avisos (un perfil de usuario que reemplaza a uno integrado...)
    pub avisos: Vec<String>,
}

fn texto_integrado(id: &str) -> Option<String> {
    INTEGRADOS.iter().find(|(n, _)| *n == id).map(|(_, t)| t.to_string())
}

impl Catalogo {
    pub fn integrados() -> Catalogo {
        Catalogo::cargar(None)
    }

    /// Integrados mas los `*.profile` de la carpeta del usuario (si existe).
    pub fn cargar(carpeta_usuario: Option<&Path>) -> Catalogo {
        let mut textos_usuario: Vec<(String, String)> = Vec::new();
        let mut rechazados = Vec::new();
        if let Some(d) = carpeta_usuario {
            if let Ok(rd) = std::fs::read_dir(d) {
                let mut nombres: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == crate::rutas::nombres::EXT_PERFIL)).collect();
                nombres.sort();
                for p in nombres {
                    match std::fs::read_to_string(&p) {
                        Ok(t) => textos_usuario.push((p.file_name().unwrap().to_string_lossy().into_owned(), t)),
                        Err(e) => rechazados.push((p.display().to_string(), e.to_string())),
                    }
                }
            }
        }
        // el texto de un padre: primero el del usuario con ese id (si ya se leyo bien), si no el integrado
        let id_de = |t: &str| pares(t).ok().and_then(|ps| ps.into_iter().find(|(k, _, _)| k == "perfil.id").map(|(_, v, _)| v));
        let padre = |id: &str| -> Option<String> { textos_usuario.iter().find(|(_, t)| id_de(t).as_deref() == Some(id)).map(|(_, t)| t.clone()).or_else(|| texto_integrado(id)) };
        let mut perfiles: Vec<Perfil> = Vec::new();
        let mut avisos = Vec::new();
        for (id, t) in INTEGRADOS {
            match Perfil::parse(t, Origen::Integrado, &|i| texto_integrado(i)) {
                Ok(p) => perfiles.push(p),
                Err(e) => rechazados.push((txf!("perfil.integrado", id), e)),
            }
        }
        for (archivo, t) in &textos_usuario {
            match Perfil::parse(t, Origen::Usuario, &padre) {
                Ok(p) => {
                    if let Some(i) = perfiles.iter().position(|x| x.id == p.id) {
                        avisos.push(txf!("perfil.el_perfil_de_usuario_reemplaza_al", format!("{:?}", p.id), archivo, format!("{:?}", if perfiles[i].origen == Origen::Integrado { "integrado" } else { tx!("catalogo.de_usuario") })));
                        perfiles[i] = p;
                    } else {
                        perfiles.push(p);
                    }
                }
                Err(e) => rechazados.push((archivo.clone(), e)),
            }
        }
        Catalogo { perfiles, rechazados, avisos }
    }

    pub fn buscar(&self, id: &str) -> Option<&Perfil> {
        self.perfiles.iter().find(|p| p.id == id)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// arranque directo: lo que hace un cargador de arranque, con cache

pub struct Arranque {
    pub kernel: PathBuf,
    pub initrd: PathBuf,
    pub cmdline: String,
    /// bootconfig del fabricante (de vendor_boot)
    pub bootconfig_vendor: Vec<String>,
}

fn sello(imagen: &Path, archivos: &[&str]) -> String {
    let mut s = String::new();
    for f in archivos {
        match std::fs::metadata(imagen.join(f)) {
            Ok(m) => {
                let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
                s.push_str(&format!("{} {} {}\n", f, m.len(), t));
            }
            Err(_) => s.push_str(&format!("{} -\n", f)),
        }
    }
    s
}


/// Escribe en `cache` kernel, initrd.img, cmdline.txt y bootconfig.txt de la imagen (lo que hacia `unpack-boot` en el guion).
/// Si ya estan hechos con los mismos archivos de entrada (tamano y fecha) no repite el trabajo.
pub fn preparar_arranque(imagen: &Path, p: &Perfil, cache: &Path) -> Result<Arranque, String> {
    let entradas = [p.boot.as_str(), p.init_boot.as_str(), p.vendor_boot.as_str()];
    let marca = sello(imagen, &entradas);
    let salida = |n: &str| cache.join(n);
    let listo = ["kernel", "initrd.img", "cmdline.txt", "bootconfig.txt"].iter().all(|n| salida(n).is_file()) && std::fs::read_to_string(salida(".stamp")).is_ok_and(|t| t == marca);
    if !listo {
        let rd = |f: &str| std::fs::read(imagen.join(f)).map_err(|e| format!("{}: {}", imagen.join(f).display(), e));
        let boot = rd(&p.boot)?;
        let init = if imagen.join(&p.init_boot).is_file() { Some(rd(&p.init_boot)?) } else { None };
        let vend = if imagen.join(&p.vendor_boot).is_file() { Some(rd(&p.vendor_boot)?) } else { None };
        let d = crate::bootimg::desempaquetar(&boot, init.as_deref(), vend.as_deref())?;
        std::fs::create_dir_all(cache).map_err(|e| format!("{}: {}", cache.display(), e))?;
        let wr = |n: &str, b: &[u8]| std::fs::write(salida(n), b).map_err(|e| format!("{}: {}", salida(n).display(), e));
        let _ = std::fs::remove_file(salida(".stamp"));
        wr("kernel", &d.kernel)?;
        wr("initrd.img", &d.initrd)?;
        wr("cmdline.txt", d.cmdline.as_bytes())?;
        wr("bootconfig.txt", format!("{}\n", d.bootconfig).as_bytes())?;
        wr(".stamp", marca.as_bytes())?;
    }
    let cmdline = std::fs::read_to_string(salida("cmdline.txt")).map_err(|e| e.to_string())?.trim().to_string();
    let bc = std::fs::read(salida("bootconfig.txt")).map_err(|e| e.to_string())?;
    Ok(Arranque { kernel: salida("kernel"), initrd: salida("initrd.img"), cmdline, bootconfig_vendor: crate::vm::bootconfig_lines(&bc) })
}

/// Lo que el perfil fija en `vm::Config` y en el bootconfig (las opciones de la linea de ordenes mandan sobre esto).
#[derive(Debug, PartialEq)]
pub struct Receta {
    pub append: String,
    pub bootconfig: Vec<String>,
    pub disk_slot: u32,
    pub console_hvc: bool,
    pub hvc_count: u32,
    pub hvc_log: Option<u32>,
    pub sensors: Option<(u32, u32)>,
    pub touch: bool,
    pub nics: u32,
    pub gpu_opts: String,
    pub accel: Accel,
}

impl Perfil {
    pub fn receta(&self, a: &Arranque, carpeta_usuario: Option<&Path>) -> Result<Receta, String> {
        let append = format!("{} {}", a.cmdline, self.cmdline_extra).trim().to_string();
        let mut bootconfig = a.bootconfig_vendor.clone();
        bootconfig.extend(self.bootconfig_propio(carpeta_usuario)?);
        Ok(Receta {
            append,
            bootconfig,
            disk_slot: self.ranura_pci,
            console_hvc: self.consola_hvc,
            hvc_count: self.hvc,
            hvc_log: self.registro,
            sensors: self.sensores,
            touch: self.tactil,
            nics: self.tarjetas,
            gpu_opts: self.gpu_opciones.clone(),
            accel: self.acel.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn integrado(id: &str) -> Perfil {
        Catalogo::integrados().buscar(id).unwrap_or_else(|| panic!("no hay {}", id)).clone()
    }

    #[test]
    fn los_perfiles_integrados_se_leen() {
        let c = Catalogo::integrados();
        assert!(c.rechazados.is_empty(), "{:?}", c.rechazados);
        let f = integrado("phone-x86_64");
        assert_eq!((f.ranura_pci, f.hvc, f.registro, f.sensores, f.tarjetas, f.tactil), (4, 30, Some(2), Some((18, 19)), 3, true));
        assert_eq!(f.gpu_opciones, "edid=off");
        assert_eq!(f.acel, Accel::Kvm);
        assert_eq!(f.arquitectura, Arq::X86_64);
        assert_eq!(f.archivos_requeridos().len(), 9, "{:?}", f.archivos_requeridos());
        // los de siempre: boot, init_boot, vendor_boot, vbmeta*, super y userdata
        let r = f.archivos_requeridos();
        for n in ["boot.img", "init_boot.img", "vendor_boot.img", "vbmeta.img", "vbmeta_system.img", "vbmeta_system_dlkm.img", "vbmeta_vendor_dlkm.img", "super.img", "userdata.img"] {
            assert!(r.contains(&n.to_string()), "{}", n);
        }
        let b = f.bootconfig_propio(None).unwrap();
        assert_eq!(b, crate::vm::bootconfig_lines(include_bytes!("../perfiles/cuttlefish.bootconfig")));
        assert!(b.contains(&"androidboot.boot_devices=pci0000:00/0000:00:04.0".to_string()));
    }

    #[test]
    fn el_perfil_del_coche_hereda_del_movil_y_cambia_lo_que_difiere() {
        let m = integrado("phone-x86_64");
        let c = integrado("car-x86_64");
        assert_eq!(c.id, "car-x86_64");
        // el coche lleva una particion de intercambio (1G) justo antes de la de datos: los datos pasan de la 19 a la 20
        assert_eq!(c.disco.len(), m.disco.len() + 1);
        let n = c.disco.len();
        assert_eq!(c.disco[..n - 2], m.disco[..n - 2]);
        assert_eq!(c.disco[n - 2], ParteDisco::Vacia("swap".into(), "1G".into()));
        assert_eq!(c.disco[n - 1], m.disco[n - 2]);
        let d = std::env::temp_dir().join(format!("ar-coche-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        for f in c.archivos_requeridos() {
            std::fs::write(d.join(f), vec![1u8; 4096]).unwrap();
        }
        let nombres: Vec<String> = c.partes(&d, Some("4G")).unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!((nombres[17].as_str(), nombres[18].as_str(), nombres[19].as_str(), nombres.len()), ("super", "swap", "userdata", 20));
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(c.sensores, m.sensores);
        // el coche solo admite adb por vsock (su red no obtiene direccion); el movil, vsock y TCP por la tarjeta 2
        assert_eq!((c.adb_transportes.clone(), m.adb_transportes.clone(), m.adb_tcp_red), (vec!["vsock".to_string()], vec!["vsock".to_string(), "tcp".to_string()], 2));
        assert!(c.aviso.contains("vehiculo") && m.aviso.is_empty());
        assert_eq!(c.requiere, m.requiere);
        assert_eq!(c.puntua, vec![Cond::AndroidInfo("config".into(), "auto".into()), Cond::Archivo("init_boot.img".into())]);
        assert_eq!(c.bootconfig_archivo, m.bootconfig_archivo);
    }

    fn base() -> String {
        "perfil.id=x\nperfil.nombre=X\narranque.modo=directo-android\ndisco.modo=gpt\ndisco.datos=userdata:userdata.img\n".to_string()
    }
    fn lee(t: &str) -> Result<Perfil, String> {
        Perfil::parse(t, Origen::Usuario, &|_| None)
    }

    #[test]
    fn errores_con_numero_de_linea_y_sin_adivinar() {
        assert!(lee(&base()).is_ok());
        let e = lee(&format!("{}clave.rara=1\n", base())).unwrap_err();
        assert!(e.contains("linea 6") && e.contains("clave.rara"), "{}", e);
        let e = lee(&format!("{}red.tarjetas=9\n", base())).unwrap_err();
        assert!(e.contains("linea 6") && e.contains("fuera de rango"), "{}", e);
        assert!(lee(&format!("{}sin igual\n", base())).unwrap_err().contains("linea 6"));
        assert!(lee(&format!("{}perfil.arquitectura=mips\n", base())).unwrap_err().contains("x86_64 o arm64"));
        assert!(lee(&format!("{}detectar.requiere=kernel:x86\n", base())).unwrap_err().contains("arquitectura"));
        assert!(lee(&format!("{}detectar.requiere=cosa:1\n", base())).unwrap_err().contains("tipo desconocido"));
        assert!(lee(&format!("{}sensores=3,3\nconsola.hvc=5\n", base())).unwrap_err().contains("distintas"));
        assert!(lee(&format!("{}origen.plantilla=https://x\n", base())).is_err());
        assert!(lee("perfil.id=x\n").unwrap_err().contains("perfil.nombre"));
        assert!(lee("perfil.id=auto\nperfil.nombre=X\n").unwrap_err().contains("identificador"));
        assert!(lee(&base().replace("directo-android", "iso")).unwrap_err().contains("directo-android"));
        assert!(lee(&base().replace("disco.datos=userdata:userdata.img\n", "")).unwrap_err().contains("disco.datos"));
        assert!(lee(&format!("{}capacidad.root=magia\n", base())).unwrap_err().contains("capacidad.root"));
    }

    #[test]
    fn otra_arquitectura_se_admite_en_el_modelo() {
        let p = lee(&format!("{}perfil.arquitectura=arm64\naceleracion=tcg\n", base())).unwrap();
        assert_eq!((p.arquitectura, p.acel), (Arq::Arm64, Accel::Tcg));
    }

    #[test]
    fn herencia_sobrescribe_claves_y_reemplaza_grupos() {
        let padre = format!("{}red.tarjetas=3\nbc.a.b=1\nbc.c.d=2\ndetectar.requiere=archivo:x.img\n", base());
        let hijo = "perfil.id=h\nperfil.nombre=H\nperfil.hereda=padre\nred.tarjetas=1\nbc.a.b=9\nbc.e.f=3\ndetectar.requiere=archivo:y.img\n";
        let buscar = |i: &str| if i == "padre" { Some(padre.clone()) } else { None };
        let p = Perfil::parse(hijo, Origen::Usuario, &buscar).unwrap();
        assert_eq!(p.id, "h");
        assert_eq!(p.tarjetas, 1);
        assert_eq!(p.bc, vec!["a.b=9", "c.d=2", "e.f=3"]);
        assert_eq!(p.requiere, vec![Cond::Archivo("y.img".into())]);
        // un padre inexistente o un ciclo se rechazan
        assert!(Perfil::parse(hijo, Origen::Usuario, &|_| None).unwrap_err().contains("no existe"));
        let ciclo = "perfil.id=c\nperfil.nombre=C\nperfil.hereda=c\n";
        assert!(Perfil::parse(ciclo, Origen::Usuario, &|_| Some(ciclo.to_string())).is_err());
    }

    #[test]
    fn particiones_en_el_orden_del_perfil() {
        let d = std::env::temp_dir().join(format!("ar-perfil-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = integrado("phone-x86_64");
        for n in f.archivos_requeridos() {
            std::fs::write(d.join(&n), vec![1u8; 4096]).unwrap();
        }
        let p = f.partes(&d, Some("4G")).unwrap();
        let nombres: Vec<&str> = p.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(&nombres[..7], ["misc", "metadata", "frp", "boot_a", "boot_b", "init_boot_a", "init_boot_b"]);
        assert_eq!(&nombres[nombres.len() - 2..], ["super", "userdata"]);
        assert_eq!(nombres.len(), 3 + 14 + 2);
        assert!(p.last().unwrap().source.is_none());
        let p2 = f.partes(&d, None).unwrap();
        assert!(p2.last().unwrap().source.is_some());
        std::fs::remove_file(d.join("super.img")).unwrap();
        assert!(f.partes(&d, None).unwrap_err().contains("super.img"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn el_catalogo_junta_integrados_y_usuario_y_avisa_si_se_reemplaza() {
        let d = std::env::temp_dir().join(format!("ar-perfiles-usuario-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("mio.profile"), format!("{}perfil.hereda=phone-x86_64\nred.tarjetas=2\n", base().replace("perfil.id=x", "perfil.id=mio"))).unwrap();
        std::fs::write(d.join("roto.profile"), "perfil.id=roto\nmala linea\n").unwrap();
        std::fs::write(d.join("phone.profile"), std::fs::read_to_string("perfiles/phone-x86_64.profile").unwrap().replace("red.tarjetas=3", "red.tarjetas=2")).unwrap();
        std::fs::write(d.join("nada.txt"), "no es un perfil").unwrap();
        let c = Catalogo::cargar(Some(&d));
        assert_eq!(c.buscar("mio").unwrap().tarjetas, 2);
        assert_eq!(c.buscar("mio").unwrap().origen, Origen::Usuario);
        assert_eq!(c.rechazados.len(), 1);
        assert!(c.rechazados[0].0.contains("roto.profile") && c.rechazados[0].1.contains("linea 2"));
        assert_eq!(c.buscar("phone-x86_64").unwrap().tarjetas, 2);
        assert_eq!(c.avisos.len(), 1);
        assert!(c.buscar("car-x86_64").is_some());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn el_arranque_extraido_se_reutiliza_y_se_rehace_si_cambian_las_entradas() {
        use crate::bootimg::tests::{fake_boot, fake_vendor_boot};
        let d = std::env::temp_dir().join(format!("ar-arranque-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (img, cache) = (d.join("img"), d.join("cache"));
        std::fs::create_dir_all(&img).unwrap();
        std::fs::write(img.join("boot.img"), fake_boot(b"KERNEL", b"RD", "a=1")).unwrap();
        std::fs::write(img.join("init_boot.img"), fake_boot(b"", b"GENERIC", "")).unwrap();
        std::fs::write(img.join("vendor_boot.img"), fake_vendor_boot(b"VENDOR", "v=2", "androidboot.x=1\n")).unwrap();
        let p = integrado("phone-x86_64");
        let a = preparar_arranque(&img, &p, &cache).unwrap();
        assert_eq!(std::fs::read(&a.kernel).unwrap(), b"KERNEL");
        assert_eq!(std::fs::read(&a.initrd).unwrap(), b"VENDORGENERIC");
        assert_eq!(a.cmdline, "v=2 a=1");
        assert_eq!(a.bootconfig_vendor, vec!["androidboot.x=1"]);
        // sin cambios: no se reescribe (se nota porque el kernel tocado a mano sobrevive)
        std::fs::write(cache.join("kernel"), b"TOCADO").unwrap();
        preparar_arranque(&img, &p, &cache).unwrap();
        assert_eq!(std::fs::read(cache.join("kernel")).unwrap(), b"TOCADO");
        // otro init_boot (otro tamano): se rehace
        std::fs::write(img.join("init_boot.img"), fake_boot(b"", &[b'G'; 6000], "")).unwrap();
        let a = preparar_arranque(&img, &p, &cache).unwrap();
        assert_eq!(std::fs::read(&a.kernel).unwrap(), b"KERNEL");
        assert_eq!(std::fs::read(&a.initrd).unwrap().len(), 6 + 6000);
        // la receta reproduce lo del guion
        let r = p.receta(&a, None).unwrap();
        assert_eq!(r.append, "v=2 a=1 console=hvc0 earlycon=uart8250,io,0x3f8 pnpacpi=off");
        assert_eq!(r.bootconfig[0], "androidboot.x=1");
        assert_eq!((r.disk_slot, r.hvc_count, r.hvc_log, r.sensors, r.nics, r.gpu_opts.as_str()), (4, 30, Some(2), Some((18, 19)), 3, "edid=off"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn identificadores() {
        for ok in ["phone-x86_64", "a", "x.y_z-1"] {
            assert!(validar_id(ok).is_ok(), "{}", ok);
        }
        for mal in ["", ".x", "auto", "a b", "a/b", "ñ", &"x".repeat(65)] {
            assert!(validar_id(mal).is_err(), "{}", mal);
        }
    }
}
