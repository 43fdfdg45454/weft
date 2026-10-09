//! Traductor ARM (heddle) como native bridge de Android x86_64: `weft bridge status|install|remove|restore|check`
//! y la seccion "Traductor ARM" de la pantalla de configuracion. Portado del instalador vigilado del banco de pruebas
//! (instalar-lib-vigilado.sh, codigos de salida 0/1/2/3), que era el UNICO modo permitido de instalar bibliotecas.
//!
//! POR QUE ES VIGILADO: el zygote carga libheddle.so al arrancar y un fallo de la biblioteca tumba system_server; a la
//! 4.a muerte Android restablece de fabrica (RescueParty) y se pierden las apps. Por eso `install` nunca deja una
//! candidata sin supervision: instala, reinicia Android, espera `sys.boot_completed` (maximo 90 s) y cuenta las muertes de
//! system_server (mas de una = fallo); si falla, restaura SOLA la biblioteca y las propiedades anteriores y reinicia.
//!
//! MAPEO con el guion (instalar-lib-vigilado.sh) -> funcion `instalar`:
//!   esperar 300 s            -> `esperar_arranque(300)`; si no responde: error previo, codigo 1, nada cambio
//!   copia de la anterior     -> respaldo EN EL INVITADO (`/data/adb/heddle-backup/`: vive y muere con el disco al que
//!                               respalda, que es donde esta el /vendor que se modifica; el guion la guardaba en el host)
//!   primera vez              -> `desactivar_verity` + reinicio solo si adb lo pide (el guion reiniciaba siempre)
//!   remount, push, chmod     -> `remontar`, `poner_biblioteca` (+ restorecon y comprobacion de la etiqueta)
//!   lineas de build.prop     -> `escribir_lineas` (las cuatro propiedades; se asegura el salto de linea final)
//!   reiniciar y vigilar 120  -> `reiniciar` + bucle de 90 s (CLAUDE.md) con `arranque_completo` y `muertes_system_server`
//!                               cada 5 s; mas 30 s de gracia vigilando muertes (el guion no los tenia)
//!   restaurar (codigo 2)     -> `revertir`: biblioteca y lineas anteriores (o, sin puente previo, sin biblioteca y con las
//!                               lineas ORIGINALES, que el guion no restauraba), reinicio y espera de 240 s
//!   restauracion fallida     -> codigo 3
//!   md5 distinto             -> el guion salia con 3 sin restaurar; aqui es un fallo vigilado: se restaura (codigo 2)
//! Codigos: 0 instalado o sin cambios; 1 error previo (nada cambio); 2 fallo vigilado y restaurado; 3 fallo y la
//! restauracion tambien fallo.
//!
//! El nucleo (`instalar`, `revertir`, `quitar`, `comprobar`) trabaja sobre el rasgo `Invitado`, de modo que la maquina de
//! estados se prueba con un invitado simulado (exito, fallo de arranque, dos muertes, misma md5...). Este modulo no toca SDL.

use crate::textos::{self, elige};
use crate::{adb, md5};
use std::path::{Path, PathBuf};
use crate::textos::{clave, tx, txf};

pub const BIBLIOTECA: &str = "/system/lib64/libheddle.so";
pub const RESPALDO: &str = "/data/adb/heddle-backup";
pub const VENDOR_PROP: &str = "/vendor/build.prop";
pub const CLAVES: [&str; 4] = ["ro.dalvik.vm.native.bridge", "ro.dalvik.vm.isa.arm64", "ro.product.cpu.abilist", "ro.product.cpu.abilist64"];
/// Las lineas que activan el puente.
pub const LINEAS_PUENTE: [&str; 4] = ["ro.dalvik.vm.native.bridge=libheddle.so", "ro.dalvik.vm.isa.arm64=x86_64", "ro.product.cpu.abilist=x86_64,arm64-v8a", "ro.product.cpu.abilist64=x86_64,arm64-v8a"];
/// Donde el usuario consigue la biblioteca (informativo: weft no descarga nada).
pub const ORIGEN: &str = "github.com/43fdfdg45454/heddle/releases";
/// Limite de espera de `sys.boot_completed` tras instalar (CLAUDE.md: 90 s).
pub const LIMITE_ARRANQUE_S: u64 = 90;
/// Tiempo extra vigilando muertes de system_server tras boot_completed.
pub const GRACIA_S: u64 = 30;
const ESPERA_RESTAURAR_S: u64 = 240;
const ESPERA_INICIAL_S: u64 = 300;
const SONDEO_S: u64 = 5;

pub const AVISO_RIESGO: &str = clave!("puente.instalar_el_traductor_arm_modifica");

pub type Progreso<'a> = &'a dyn Fn(&str);

pub fn mib(b: u64) -> String {
    format!("{:.1} MiB", b as f64 / 1048576.0)
}

// ---------------------------------------------------------------------------------------------------------------
// el invitado, como rasgo (real: adb propio; en las pruebas: simulado)

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Respaldo {
    /// existe la carpeta de respaldo
    pub hay: bool,
    /// antes de la ultima instalacion habia una biblioteca (y su copia esta guardada)
    pub anterior_habia: bool,
    pub anterior_lineas: Vec<String>,
    /// lineas de las cuatro propiedades antes de configurar el puente por primera vez (None = no se guardaron)
    pub originales: Option<Vec<String>>,
    /// una instalacion empezo y no termino su supervision
    pub marca: bool,
}

pub trait Invitado {
    fn esperar_arranque(&mut self, limite_s: u64) -> Result<(), String>;
    /// solo que adbd responda (aunque Android no haya terminado de arrancar)
    fn esperar_adb(&mut self, limite_s: u64) -> Result<(), String>;
    fn root(&mut self) -> Result<(), String>;
    fn remontar(&mut self) -> Result<(), String>;
    /// `disable-verity`; Ok(true) si adb pide reiniciar para que surta efecto
    fn desactivar_verity(&mut self) -> Result<bool, String>;
    fn reiniciar(&mut self) -> Result<(), String>;
    fn arranque_completo(&mut self) -> bool;
    fn muertes_system_server(&mut self) -> u32;
    fn pausa(&mut self, s: u64);
    /// segundos desde un origen fijo (reloj; el simulado avanza con `pausa`)
    fn ahora(&self) -> f64;
    fn md5_biblioteca(&mut self) -> Result<Option<String>, String>;
    fn puente_configurado(&mut self) -> Result<bool, String>;
    /// lineas de las cuatro propiedades en /vendor/build.prop
    fn lineas_propiedades(&mut self) -> Result<Vec<String>, String>;
    fn leer_respaldo(&mut self) -> Result<Respaldo, String>;
    /// guarda la biblioteca actual (si hay), las lineas y, si `originales` es Some, las originales (solo si faltaban)
    fn guardar_respaldo(&mut self, hay_biblioteca: bool, lineas: &[String], originales: Option<&[String]>) -> Result<(), String>;
    fn marca(&mut self, poner: bool) -> Result<(), String>;
    fn poner_biblioteca(&mut self, local: &Path) -> Result<(), String>;
    fn quitar_biblioteca(&mut self) -> Result<(), String>;
    fn restaurar_biblioteca(&mut self) -> Result<(), String>;
    /// quita las cuatro propiedades de /vendor/build.prop y escribe `lineas`
    fn escribir_lineas(&mut self, lineas: &[String]) -> Result<(), String>;
    fn guardar_registro_fallo(&mut self);
}

// ---------------------------------------------------------------------------------------------------------------
// resultados y codigos de salida

#[derive(Clone, Debug, PartialEq)]
pub enum Resultado {
    /// ya estaba instalada la misma biblioteca y configurado: no se toco nada
    SinCambios(String),
    Instalado { md5: String, muertes: u32, segundos: f64, notas: Vec<String> },
    /// la candidata fallo y se restauro lo anterior (codigo 2)
    Restaurado { motivo: String, notas: Vec<String> },
    Quitado(Vec<String>),
    /// solo `--no-reboot`: instalada pero SIN vigilar (hasta `bridge check`)
    SinVigilar(Vec<String>),
    /// `check` sin nada pendiente
    Sano(Vec<String>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ErrorPuente {
    /// antes de cambiar nada (codigo 1)
    Previo(String),
    /// el fallo y la restauracion fallaron (codigo 3)
    RestauracionFallo(String),
}

/// Codigo de salida (el mismo del guion).
pub fn codigo(r: &Result<Resultado, ErrorPuente>) -> i32 {
    match r {
        Ok(Resultado::Restaurado { .. }) => 2,
        Ok(_) => 0,
        Err(ErrorPuente::Previo(_)) => 1,
        Err(ErrorPuente::RestauracionFallo(_)) => 3,
    }
}

/// Texto para el usuario de un resultado (incluye el motivo del retroceso).
pub fn describir(r: &Result<Resultado, ErrorPuente>) -> String {
    match r {
        Ok(Resultado::SinCambios(t)) => t.clone(),
        Ok(Resultado::Instalado { md5, muertes, segundos, notas }) => txf!("puente.instalacion_ok_md5_instalado_system", &md5[..12.min(md5.len())], muertes, format!("{:.0}", segundos), notas.join("\n")),
        Ok(Resultado::Restaurado { motivo, notas }) => txf!("puente.restaurado_la_candidata_fallo_se", motivo, notas.join("\n")),
        Ok(Resultado::Quitado(n)) => txf!("puente.traductor_arm_quitado", n.join("\n")),
        Ok(Resultado::SinVigilar(n)) => txf!("puente.aviso_instalado_sin_vigilar_el_arranque", n.join("\n")),
        Ok(Resultado::Sano(n)) => n.join("\n"),
        Err(ErrorPuente::Previo(t)) => txf!("puente.error_nada_cambio", t),
        Err(ErrorPuente::RestauracionFallo(t)) => txf!("puente.error_grave", t),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// validacion de la biblioteca

/// Comprobaciones baratas antes de tocar el invitado: ELF de 64 bits, little endian, x86-64, objeto compartido, tamano
/// razonable y el simbolo `NativeBridgeItf` que busca ART. Devuelve el tamano.
pub fn validar_biblioteca(datos: &[u8]) -> Result<u64, String> {
    if datos.len() < 64 || &datos[..4] != b"\x7fELF" {
        return Err(tx!("puente.no_es_un_elf_la_biblioteca_esta_corrupta").into());
    }
    if datos[4] != 2 || datos[5] != 1 {
        return Err(tx!("puente.no_es_un_elf_de_64_bits_little_endian").into());
    }
    let tipo = u16::from_le_bytes([datos[16], datos[17]]);
    let maquina = u16::from_le_bytes([datos[18], datos[19]]);
    if maquina != 0x3E {
        return Err(txf!("puente.no_es_x86_64_e_machine_el_invitado_es", format!("{:#x}", maquina)));
    }
    if tipo != 3 {
        return Err(tx!("puente.no_es_un_objeto_compartido_et_dyn").into());
    }
    if datos.len() < 4096 {
        return Err(tx!("puente.la_biblioteca_es_demasiado_pequena").into());
    }
    if !datos.windows(15).any(|w| w == b"NativeBridgeItf") {
        return Err(tx!("puente.no_exporta_nativebridgeitf_no_es_un").into());
    }
    Ok(datos.len() as u64)
}

pub fn validar_archivo(p: &Path) -> Result<(u64, String), String> {
    let d = std::fs::read(p).map_err(|e| format!("{}: {}", p.display(), e))?;
    let n = validar_biblioteca(&d)?;
    Ok((n, md5::de_bytes(&d)))
}

// ---------------------------------------------------------------------------------------------------------------
// lineas de propiedades

/// Una linea de propiedad segura para ir entre comillas simples en el shell del invitado.
pub fn linea_valida(l: &str) -> bool {
    let Some((k, v)) = l.split_once('=') else { return false };
    !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ',' | ':' | '/' | '-'))
}

pub fn lineas_del_puente() -> Vec<String> {
    LINEAS_PUENTE.iter().map(|s| s.to_string()).collect()
}

fn es_propiedad_del_puente(l: &str) -> bool {
    CLAVES.iter().any(|k| l.starts_with(&format!("{}=", k)))
}

/// Orden de shell que reemplaza las cuatro propiedades de /vendor/build.prop por `lineas`.
pub fn orden_escribir_lineas(lineas: &[String]) -> Result<String, String> {
    if let Some(m) = lineas.iter().find(|l| !linea_valida(l)) {
        return Err(txf!("puente.linea_de_propiedad_no_valida", format!("{:?}", m)));
    }
    let borrar: Vec<String> = CLAVES.iter().map(|k| format!("/^{}=/d", k.replace('.', "\\."))).collect();
    let mut s = format!("sed -i '{}' {p}", borrar.join(";"), p = VENDOR_PROP);
    if !lineas.is_empty() {
        let args: Vec<String> = lineas.iter().map(|l| format!("'{}'", l)).collect();
        // el archivo puede no terminar en salto de linea: se asegura antes de agregar
        s.push_str(&format!(" && {{ [ -z \"$(tail -c1 {p})\" ] || echo >> {p}; }} && printf '%s\\n' {a} >> {p}", p = VENDOR_PROP, a = args.join(" ")));
    }
    Ok(s)
}

// ---------------------------------------------------------------------------------------------------------------
// maquina de estados

fn sin_conexion(e: String) -> ErrorPuente {
    ErrorPuente::Previo(e)
}

/// Espera boot_completed vigilando las muertes de system_server. Ok(muertes maximas) o Err(motivo).
fn vigilar(inv: &mut dyn Invitado, t0: f64, limite_s: u64, gracia_s: u64, prog: Progreso) -> Result<u32, (String, u32)> {
    let mut max_m = 0u32;
    let mut ok = false;
    let mut ultimo_aviso = -100.0f64;
    while inv.ahora() - t0 < limite_s as f64 {
        if inv.arranque_completo() {
            ok = true;
            break;
        }
        max_m = max_m.max(inv.muertes_system_server());
        if max_m > 1 {
            return Err((txf!("puente.system_server_murio_veces_antes_de", max_m), max_m));
        }
        if inv.ahora() - ultimo_aviso >= 15.0 {
            ultimo_aviso = inv.ahora();
            prog(&txf!("puente.vigilando_el_arranque_s_de_muertes_de", format!("{:.0}", inv.ahora() - t0), limite_s, max_m));
        }
        inv.pausa(SONDEO_S);
    }
    if !ok {
        return Err((txf!("puente.sys_boot_completed_no_llego_en_s_muertes", limite_s, max_m), max_m));
    }
    // gracia: system_server puede morir justo despues de boot_completed
    let t1 = inv.ahora();
    max_m = max_m.max(inv.muertes_system_server());
    while max_m <= 1 && inv.ahora() - t1 < gracia_s as f64 {
        inv.pausa(SONDEO_S);
        max_m = max_m.max(inv.muertes_system_server());
    }
    if max_m > 1 {
        return Err((txf!("puente.system_server_murio_veces_tras_arrancar", max_m), max_m));
    }
    Ok(max_m)
}

/// Devuelve lo anterior: biblioteca y propiedades previas (o, sin puente previo, sin biblioteca y con las lineas originales),
/// reinicia y espera que Android arranque. Ok si Android llego a boot_completed.
fn revertir(inv: &mut dyn Invitado, resp: &Respaldo, prog: Progreso) -> Result<Vec<String>, String> {
    let mut notas = Vec::new();
    prog(tx!("puente.restaurando_lo_anterior"));
    inv.root()?;
    let mut remontado = false;
    for _ in 0..3 {
        if inv.remontar().is_ok() {
            remontado = true;
            break;
        }
        inv.pausa(3);
    }
    if !remontado {
        return Err(tx!("puente.no_se_pudo_abrir_vendor_para_escritura").into());
    }
    if resp.anterior_habia {
        inv.restaurar_biblioteca()?;
        inv.escribir_lineas(&resp.anterior_lineas)?;
        notas.push(tx!("puente.restaurada_la_biblioteca_anterior_y_sus").into());
    } else {
        inv.quitar_biblioteca()?;
        match &resp.originales {
            Some(o) => {
                inv.escribir_lineas(o)?;
                notas.push(tx!("puente.no_habia_traductor_biblioteca_quitada_y").into());
            }
            None => {
                inv.escribir_lineas(&[])?;
                notas.push(tx!("puente.no_habia_traductor_biblioteca_y_lineas").into());
            }
        }
    }
    inv.marca(false)?;
    prog(tx!("puente.reiniciando_android_tras_restaurar"));
    inv.reiniciar()?;
    inv.esperar_arranque(ESPERA_RESTAURAR_S).map_err(|e| txf!("puente.tras_restaurar_android_no_llego_a_boot", e))?;
    Ok(notas)
}

/// Instala `lib` (ya validada, con su md5) con vigilancia. `reiniciar=false` es `--no-reboot` (sin vigilancia).
pub fn instalar(inv: &mut dyn Invitado, lib: &Path, md5_local: &str, reiniciar: bool, limite_s: u64, prog: Progreso) -> Result<Resultado, ErrorPuente> {
    prog(tx!("puente.comprobando_que_android_responde"));
    inv.esperar_arranque(ESPERA_INICIAL_S).map_err(|e| sin_conexion(txf!("puente.android_no_responde_por_adb", e)))?;
    inv.root().map_err(sin_conexion)?;
    let resp = inv.leer_respaldo().map_err(sin_conexion)?;
    if resp.marca {
        return Err(ErrorPuente::Previo(tx!("puente.una_instalacion_anterior_no_termino_su").into()));
    }
    let actual = inv.md5_biblioteca().map_err(sin_conexion)?;
    let configurado = inv.puente_configurado().map_err(sin_conexion)?;
    if actual.as_deref() == Some(md5_local) && configurado {
        return Ok(Resultado::SinCambios(txf!("puente.ya_esta_instalada_esta_misma_biblioteca", &md5_local[..12])));
    }
    let lineas_antes = inv.lineas_propiedades().map_err(sin_conexion)?;
    // la copia de lo que hay AHORA: la biblioteca (si hay) y las lineas; las originales, solo si aun no se guardaron
    let originales = if !configurado && resp.originales.is_none() { Some(lineas_antes.clone()) } else { None };
    prog(tx!("puente.guardando_copia_de_lo_anterior_en_el"));
    inv.guardar_respaldo(actual.is_some(), &lineas_antes, originales.as_deref()).map_err(sin_conexion)?;
    let resp = inv.leer_respaldo().map_err(sin_conexion)?;
    if !configurado {
        prog(tx!("puente.configurando_por_primera_vez_disable"));
        let reinicio = inv.desactivar_verity().map_err(|e| sin_conexion(format!("disable-verity: {}", e)))?;
        if reinicio {
            prog(tx!("puente.reiniciando_android_para_que_disable"));
            inv.reiniciar().map_err(sin_conexion)?;
            inv.esperar_arranque(ESPERA_INICIAL_S).map_err(|e| sin_conexion(txf!("puente.no_arranco_tras_disable_verity_nada", e)))?;
            inv.root().map_err(sin_conexion)?;
        }
    }
    inv.remontar().map_err(|e| sin_conexion(format!("remount: {}", e)))?;
    // desde aqui hay cambios: cualquier fallo restaura
    inv.marca(true).map_err(sin_conexion)?;
    let mut notas = Vec::new();
    let instalacion = (|| -> Result<(), String> {
        prog(tx!("puente.copiando_la_biblioteca_a_system_lib64"));
        inv.poner_biblioteca(lib)?;
        inv.escribir_lineas(&lineas_del_puente())?;
        Ok(())
    })();
    if let Err(e) = instalacion {
        return fallo(inv, &resp, txf!("puente.no_se_pudo_instalar", e), prog);
    }
    if !reiniciar {
        notas.push(tx!("puente.las_propiedades_y_la_biblioteca_estan").into());
        return Ok(Resultado::SinVigilar(notas));
    }
    prog(&txf!("puente.reiniciando_y_vigilando_el_arranque_s", limite_s));
    let t0 = inv.ahora();
    if let Err(e) = inv.reiniciar() {
        return fallo(inv, &resp, txf!("puente.no_se_pudo_reiniciar", e), prog);
    }
    match vigilar(inv, t0, limite_s, GRACIA_S, prog) {
        Err((motivo, _)) => fallo(inv, &resp, motivo, prog),
        Ok(muertes) => {
            let _ = inv.root();
            let instalado = inv.md5_biblioteca().ok().flatten().unwrap_or_default();
            if instalado != md5_local {
                return fallo(inv, &resp, txf!("puente.md5_distinto_tras_instalar_instalado", &instalado[..12.min(instalado.len())], &md5_local[..12]), prog);
            }
            if !inv.puente_configurado().unwrap_or(false) {
                return fallo(inv, &resp, tx!("puente.las_propiedades_del_traductor_no").into(), prog);
            }
            let _ = inv.marca(false);
            notas.push(elige(tx!("puente.copia_de_lo_anterior_guardada_en_el"), &txf!("puente.copia_de_lo_anterior_en_del_invitado", RESPALDO)));
            Ok(Resultado::Instalado { md5: instalado, muertes, segundos: inv.ahora() - t0, notas })
        }
    }
}

fn fallo(inv: &mut dyn Invitado, resp: &Respaldo, motivo: String, prog: Progreso) -> Result<Resultado, ErrorPuente> {
    prog(&txf!("puente.fallo", motivo));
    inv.guardar_registro_fallo();
    match revertir(inv, resp, prog) {
        Ok(notas) => Ok(Resultado::Restaurado { motivo, notas }),
        Err(e) => Err(ErrorPuente::RestauracionFallo(txf!("puente.la_instalacion_fallo_y_la_restauracion", motivo, e))),
    }
}

/// `bridge check`: si una instalacion quedo sin vigilar (marca) la supervisa ahora y restaura si falla; si no, solo informa.
pub fn comprobar(inv: &mut dyn Invitado, limite_s: u64, prog: Progreso) -> Result<Resultado, ErrorPuente> {
    // solo adbd: con una candidata mala Android puede no llegar nunca a boot_completed, y justo eso es lo que se vigila
    inv.esperar_adb(120).map_err(|e| sin_conexion(txf!("puente.android_no_responde_por_adb", e)))?;
    inv.root().map_err(sin_conexion)?;
    let resp = inv.leer_respaldo().map_err(sin_conexion)?;
    if !resp.marca {
        let m = inv.muertes_system_server();
        let boot = inv.arranque_completo();
        return Ok(Resultado::Sano(vec![txf!("puente.sys_boot_completed_system_server_murio", if boot { 1 } else { 0 }, m)]));
    }
    prog(tx!("puente.hay_una_instalacion_sin_vigilar"));
    let t0 = inv.ahora();
    match vigilar(inv, t0, limite_s, GRACIA_S, prog) {
        Ok(m) => {
            let _ = inv.marca(false);
            Ok(Resultado::Sano(vec![txf!("puente.la_candidata_arranco_bien_system_server", m)]))
        }
        Err((motivo, _)) => fallo(inv, &resp, motivo, prog),
    }
}

/// `bridge restore`: devuelve lo que habia antes de la ultima instalacion.
pub fn restaurar(inv: &mut dyn Invitado, prog: Progreso) -> Result<Resultado, ErrorPuente> {
    inv.esperar_arranque(ESPERA_INICIAL_S).map_err(|e| sin_conexion(txf!("puente.android_no_responde_por_adb", e)))?;
    inv.root().map_err(sin_conexion)?;
    let resp = inv.leer_respaldo().map_err(sin_conexion)?;
    if !resp.hay {
        return Err(ErrorPuente::Previo(elige(tx!("puente.no_hay_respaldo_en_el_invitado_no_hay"), &txf!("puente.no_hay_respaldo_en_no_hay_nada_que", RESPALDO))));
    }
    match revertir(inv, &resp, prog) {
        Ok(notas) => Ok(Resultado::Quitado(notas)),
        Err(e) => Err(ErrorPuente::RestauracionFallo(e)),
    }
}

/// `bridge remove`: quita la biblioteca y devuelve las propiedades originales (si estaban guardadas; si no, solo quita las lineas).
pub fn quitar(inv: &mut dyn Invitado, reiniciar: bool, prog: Progreso) -> Result<Resultado, ErrorPuente> {
    inv.esperar_arranque(ESPERA_INICIAL_S).map_err(|e| sin_conexion(txf!("puente.android_no_responde_por_adb", e)))?;
    inv.root().map_err(sin_conexion)?;
    let resp = inv.leer_respaldo().map_err(sin_conexion)?;
    let (hay_lib, configurado) = (inv.md5_biblioteca().map_err(sin_conexion)?.is_some(), inv.puente_configurado().map_err(sin_conexion)?);
    if !hay_lib && !configurado {
        return Ok(Resultado::Quitado(vec![tx!("puente.no_habia_traductor_instalado_nada_que").into()]));
    }
    inv.remontar().map_err(|e| sin_conexion(format!("remount: {}", e)))?;
    let mut notas = Vec::new();
    prog(tx!("puente.quitando_la_biblioteca_y_las_propiedades"));
    inv.quitar_biblioteca().map_err(sin_conexion)?;
    match &resp.originales {
        Some(o) => {
            inv.escribir_lineas(o).map_err(sin_conexion)?;
            notas.push(tx!("puente.propiedades_originales_restauradas").to_string());
        }
        None => {
            inv.escribir_lineas(&[]).map_err(sin_conexion)?;
            notas.push(tx!("puente.aviso_las_propiedades_originales_no").to_string());
        }
    }
    if reiniciar {
        prog(tx!("puente.reiniciando_android"));
        inv.reiniciar().map_err(sin_conexion)?;
        match inv.esperar_arranque(ESPERA_RESTAURAR_S) {
            Ok(()) => notas.push(tx!("puente.android_arranco_sin_el_traductor").to_string()),
            Err(e) => return Err(ErrorPuente::RestauracionFallo(txf!("puente.tras_quitar_el_traductor_android_no", e))),
        }
    } else {
        notas.push(tx!("puente.android_no_se_reinicio_no_reboot_el").to_string());
    }
    Ok(Resultado::Quitado(notas))
}

// ---------------------------------------------------------------------------------------------------------------
// estado (lectura)

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Estado {
    pub biblioteca: bool,
    pub md5: Option<String>,
    pub bytes: Option<u64>,
    /// propiedades en ejecucion (getprop)
    pub nb: String,
    pub isa: String,
    pub abilist: String,
    pub abilist64: String,
    /// propiedades escritas en /vendor/build.prop
    pub en_archivo: Vec<String>,
    pub boot: bool,
    pub muertes: u32,
    /// libheddle cargada en el zygote64 (evidencia de que se cargo)
    pub en_zygote: bool,
    /// lineas del registro con la etiqueta heddle
    pub lineas_log: u32,
    pub marca: bool,
    pub respaldo: bool,
}

impl Estado {
    pub fn instalado(&self) -> bool {
        self.biblioteca && self.nb == "libheddle.so"
    }

    pub fn resumen(&self) -> String {
        let si = |b: bool| if b { "si" } else { "no" };
        let mut s = txf!("puente.instalado_biblioteca_md5_ro_dalvik_vm", si(self.instalado()), BIBLIOTECA, si(self.biblioteca), self.bytes.map_or(String::new(), |b| format!(" ({})", mib(b))), self.md5.as_deref().unwrap_or("-"), if self.nb.is_empty() { "(vacia)" } else { &self.nb }, if self.isa.is_empty() { "(vacia)" } else { &self.isa }, if self.abilist.is_empty() { "(vacia)" } else { &self.abilist }, if self.abilist64.is_empty() { "(vacia)" } else { &self.abilist64 });
        s.push_str(&txf!("puente.lineas_en", VENDOR_PROP, if self.en_archivo.is_empty() { "ninguna".to_string() } else { self.en_archivo.join(" ") }));
        s.push_str(&txf!("puente.arranque_sys_boot_completed_muertes_de", if self.boot { 1 } else { 0 }, self.muertes));
        s.push_str(&txf!("puente.cargado", if self.en_zygote { tx!("puente_tec.so_esta_en_el_zygote64") } else if self.lineas_log > 0 { tx!("puente_tec.hay_registro_de_etiqueta") } else { tx!("puente.sin_evidencia_no_hay_una_app_arm") }));
        s.push_str(&txf!("puente.respaldo_en_el_invitado", si(self.respaldo)));
        if self.marca {
            s.push_str(tx!("puente.aviso_una_instalacion_anterior_no"));
        }
        s
    }
}

pub fn orden_estado() -> String {
    format!(
        "echo biblioteca=$([ -f {l} ] && echo 1 || echo 0); echo md5=$(md5sum {l} 2>/dev/null | cut -d' ' -f1); echo bytes=$(stat -c %s {l} 2>/dev/null); \
         echo nb=$(getprop ro.dalvik.vm.native.bridge); echo isa=$(getprop ro.dalvik.vm.isa.arm64); echo abilist=$(getprop ro.product.cpu.abilist); echo abilist64=$(getprop ro.product.cpu.abilist64); \
         echo archivo=$(grep -E '^({k})=' {p} | tr '\\n' ' '); echo boot=$(getprop sys.boot_completed); \
         echo muertes=$(logcat -d -b system 2>/dev/null | grep -c 'FATAL EXCEPTION IN SYSTEM PROCESS'); \
         echo zygote=$(grep -c libheddle /proc/$(pidof zygote64 | cut -d' ' -f1)/maps 2>/dev/null); \
         echo logs=$(logcat -d -s heddle 2>/dev/null | grep -c heddle); echo marca=$([ -e {r}/instalando ] && echo 1 || echo 0); echo respaldo=$([ -d {r} ] && echo 1 || echo 0)",
        l = BIBLIOTECA,
        k = CLAVES.iter().map(|c| c.replace('.', "\\.")).collect::<Vec<_>>().join("|"),
        p = VENDOR_PROP,
        r = RESPALDO
    )
}

pub fn parsear_estado(salida: &str) -> Estado {
    let mut e = Estado::default();
    for l in salida.lines() {
        let Some((k, v)) = l.trim().split_once('=') else { continue };
        let v = v.trim();
        let n = || v.parse::<u32>().unwrap_or(0);
        match k {
            "biblioteca" => e.biblioteca = v == "1",
            "md5" => e.md5 = Some(v).filter(|m| m.len() == 32 && m.chars().all(|c| c.is_ascii_hexdigit())).map(|m| m.to_string()),
            "bytes" => e.bytes = v.parse().ok(),
            "nb" => e.nb = v.into(),
            "isa" => e.isa = v.into(),
            "abilist" => e.abilist = v.into(),
            "abilist64" => e.abilist64 = v.into(),
            "archivo" => e.en_archivo = v.split_whitespace().map(|s| s.to_string()).collect(),
            "boot" => e.boot = v == "1",
            "muertes" => e.muertes = n(),
            "zygote" => e.en_zygote = n() > 0,
            "logs" => e.lineas_log = n(),
            "marca" => e.marca = v == "1",
            "respaldo" => e.respaldo = v == "1",
            _ => {}
        }
    }
    e
}

pub fn estado(cid: u32) -> Result<Estado, String> {
    adb::hacer_root(cid)?;
    let (_, o, _) = adb::conectar(cid, 20)?.shell_texto(&orden_estado())?;
    Ok(parsear_estado(&o))
}

// ---------------------------------------------------------------------------------------------------------------
// biblioteca ya descargada (weft no descarga nada)

/// Primer archivo `libheddle.so` bajo `dir` (la release lo trae en la raiz; se admite una subcarpeta).
pub fn buscar_biblioteca(dir: &Path) -> Option<PathBuf> {
    let mut pendientes = vec![dir.to_path_buf()];
    while let Some(d) = pendientes.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                pendientes.push(p);
            } else if p.file_name().is_some_and(|n| n == "libheddle.so") {
                return Some(p);
            }
        }
    }
    None
}

/// Biblioteca a instalar sin `--file`: `libheddle.so` (o dentro de una subcarpeta) en la carpeta del traductor de la cache.
/// Con `--file`: un archivo .so o una carpeta que lo contenga.
pub fn biblioteca_local(indicada: Option<&str>, cache: &Path) -> Result<PathBuf, String> {
    match indicada {
        Some(f) => {
            let p = PathBuf::from(f);
            if p.is_dir() {
                buscar_biblioteca(&p).ok_or_else(|| elige(&txf!("puente.no_hay_ninguna_biblioteca_del_traductor", f), &txf!("puente_tec.no_hay_so_en_la_carpeta", f)))
            } else if p.is_file() {
                Ok(p)
            } else {
                Err(txf!("puente.no_existe_se_espera_el_archivo_de_la", f))
            }
        }
        None => buscar_biblioteca(cache).ok_or_else(|| {
            elige(
                &txf!("puente.falta_la_biblioteca_del_traductor_arm_no", cache.display()),
                &txf!("puente_tec.falta_so_no_descarga_nada_usa_bridge", cache.display()),
            )
        }),
    }
}

pub fn dir_cache(estado: &Path) -> PathBuf {
    std::env::var("WEFT_PUENTE_DIR").map(PathBuf::from).unwrap_or_else(|_| crate::rutas::actual().cache_de(estado, "bridge", "puente"))
}

// ---------------------------------------------------------------------------------------------------------------
// el invitado real (adb propio)

pub struct InvitadoAdb {
    pub cid: u32,
    /// donde guardar el registro de un fallo (carpeta de la cache del estado)
    pub registro: PathBuf,
    t0: std::time::Instant,
}

impl InvitadoAdb {
    pub fn nuevo(cid: u32, registro: PathBuf) -> InvitadoAdb {
        InvitadoAdb { cid, registro, t0: std::time::Instant::now() }
    }

    fn sh(&self, cmd: &str) -> Result<(i32, String), String> {
        let (c, o, e) = adb::conectar(self.cid, 20)?.shell_texto(cmd)?;
        Ok((c, format!("{}{}", o, e)))
    }

    fn sh_ok(&self, cmd: &str) -> Result<String, String> {
        let (c, o) = self.sh(cmd)?;
        if c != 0 {
            return Err(error_orden(cmd, c, &o));
        }
        Ok(o)
    }
}

/// Error de una orden que fallo en el invitado: en consola lleva la orden y la salida; en la interfaz solo el codigo y la salida limpia.
pub(crate) fn error_orden(cmd: &str, codigo: i32, salida: &str) -> String {
    elige(&txf!("puente.una_orden_en_el_invitado_fallo_codigo", codigo, textos::limpiar(salida.trim())), &txf!("puente.codigo", cmd.chars().take(60).collect::<String>(), codigo, salida.trim()))
}

fn lineas_de(t: &str) -> Vec<String> {
    t.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}

fn guardar_lineas_cmd(archivo: &str, l: &[String]) -> Result<String, String> {
    if let Some(m) = l.iter().find(|x| !linea_valida(x)) {
        return Err(txf!("puente.linea_de_propiedad_no_valida", format!("{:?}", m)));
    }
    Ok(if l.is_empty() { format!(": > {}", archivo) } else { format!("printf '%s\\n' {} > {}", l.iter().map(|x| format!("'{}'", x)).collect::<Vec<_>>().join(" "), archivo) })
}

impl Invitado for InvitadoAdb {
    fn esperar_arranque(&mut self, limite_s: u64) -> Result<(), String> {
        adb::esperar(self.cid, limite_s, true).map(|_| ())
    }
    fn esperar_adb(&mut self, limite_s: u64) -> Result<(), String> {
        adb::esperar(self.cid, limite_s, false).map(|_| ())
    }
    fn root(&mut self) -> Result<(), String> {
        adb::hacer_root(self.cid).map(|_| ())
    }
    fn remontar(&mut self) -> Result<(), String> {
        adb::remontar(self.cid).map(|_| ())
    }
    fn desactivar_verity(&mut self) -> Result<bool, String> {
        adb::hacer_root(self.cid)?;
        let mut c = adb::conectar(self.cid, 20)?;
        let t = if c.has("remount_shell") {
            let (_, o, e) = c.shell_texto("disable-verity")?;
            format!("{}{}", o, e)
        } else {
            c.servicio_texto("disable-verity:")?
        };
        let b = t.to_lowercase();
        if b.contains("failed") || b.contains("error") {
            return Err(t.trim().to_string());
        }
        // solo se omite el reinicio si adb dice claramente que ya estaba desactivada y no cambio nada
        Ok(!(b.contains("already") && !b.contains("successfully")))
    }
    fn reiniciar(&mut self) -> Result<(), String> {
        let mut c = adb::conectar(self.cid, 20)?;
        let _ = c.servicio_texto("reboot:");
        drop(c);
        // adbd sigue contestando unos segundos mientras Android se apaga: no se mira antes
        std::thread::sleep(std::time::Duration::from_secs(12));
        Ok(())
    }
    fn arranque_completo(&mut self) -> bool {
        adb::shell_una_vez(self.cid, "getprop sys.boot_completed", 5).is_ok_and(|(_, o, _)| o.trim() == "1")
    }
    fn muertes_system_server(&mut self) -> u32 {
        adb::shell_una_vez(self.cid, "logcat -d -b system 2>/dev/null | grep -c 'FATAL EXCEPTION IN SYSTEM PROCESS'", 8).ok().and_then(|(_, o, _)| o.lines().next().and_then(|l| l.trim().parse().ok())).unwrap_or(0)
    }
    fn pausa(&mut self, s: u64) {
        std::thread::sleep(std::time::Duration::from_secs(s));
    }
    fn ahora(&self) -> f64 {
        self.t0.elapsed().as_secs_f64()
    }
    fn md5_biblioteca(&mut self) -> Result<Option<String>, String> {
        let (_, o) = self.sh(&format!("[ -f {l} ] && md5sum {l} | cut -d' ' -f1", l = BIBLIOTECA))?;
        let m = o.trim().to_string();
        Ok(Some(m).filter(|m| m.len() == 32))
    }
    fn puente_configurado(&mut self) -> Result<bool, String> {
        let (_, o) = self.sh("getprop ro.dalvik.vm.native.bridge")?;
        Ok(o.trim() == "libheddle.so")
    }
    fn lineas_propiedades(&mut self) -> Result<Vec<String>, String> {
        let (_, o) = self.sh(&format!("grep -E '^({})=' {}", CLAVES.iter().map(|c| c.replace('.', "\\.")).collect::<Vec<_>>().join("|"), VENDOR_PROP))?;
        Ok(lineas_de(&o).into_iter().filter(|l| es_propiedad_del_puente(l)).collect())
    }
    fn leer_respaldo(&mut self) -> Result<Respaldo, String> {
        let r = RESPALDO;
        let (_, o) = self.sh(&format!(
            "echo hay=$([ -d {r} ] && echo 1 || echo 0); echo habia=$([ -f {r}/anterior.habia ] && [ -f {r}/anterior.so ] && echo 1 || echo 0); echo marca=$([ -e {r}/instalando ] && echo 1 || echo 0); \
             echo tiene_original=$([ -f {r}/original.lineas ] && echo 1 || echo 0); echo ---anterior; cat {r}/anterior.lineas 2>/dev/null; echo ---original; cat {r}/original.lineas 2>/dev/null"
        ))?;
        Ok(parsear_respaldo(&o))
    }
    fn guardar_respaldo(&mut self, hay_biblioteca: bool, lineas: &[String], originales: Option<&[String]>) -> Result<(), String> {
        let r = RESPALDO;
        let mut c = format!("mkdir -p {r} && rm -f {r}/anterior.so {r}/anterior.habia", r = r);
        if hay_biblioteca {
            c.push_str(&format!(" && cp {l} {r}/anterior.so && echo 1 > {r}/anterior.habia", l = BIBLIOTECA, r = r));
        }
        c.push_str(&format!(" && {}", guardar_lineas_cmd(&format!("{}/anterior.lineas", r), lineas)?));
        if let Some(o) = originales {
            c.push_str(&format!(" && {}", guardar_lineas_cmd(&format!("{}/original.lineas", r), o)?));
        }
        c.push_str(" && sync");
        self.sh_ok(&c).map(|_| ())
    }
    fn marca(&mut self, poner: bool) -> Result<(), String> {
        self.sh_ok(&if poner { format!("mkdir -p {r} && echo 1 > {r}/instalando", r = RESPALDO) } else { format!("rm -f {}/instalando", RESPALDO) }).map(|_| ())
    }
    fn poner_biblioteca(&mut self, local: &Path) -> Result<(), String> {
        adb::conectar(self.cid, 30)?.push(local, BIBLIOTECA).map(|_| ())?;
        // permisos y etiqueta de SELinux: la que lleva el resto de /system/lib64
        self.sh_ok(&format!("chmod 644 {l} && restorecon {l}", l = BIBLIOTECA))?;
        let (_, o) = self.sh(&format!("ls -Z {}", BIBLIOTECA))?;
        if !o.contains("system_lib_file") {
            self.sh_ok(&format!("chcon u:object_r:system_lib_file:s0 {}", BIBLIOTECA))?;
        }
        Ok(())
    }
    fn quitar_biblioteca(&mut self) -> Result<(), String> {
        self.sh_ok(&format!("rm -f {}", BIBLIOTECA)).map(|_| ())
    }
    fn restaurar_biblioteca(&mut self) -> Result<(), String> {
        self.sh_ok(&format!("cp {r}/anterior.so {l} && chmod 644 {l} && restorecon {l}", r = RESPALDO, l = BIBLIOTECA)).map(|_| ())
    }
    fn escribir_lineas(&mut self, lineas: &[String]) -> Result<(), String> {
        self.sh_ok(&orden_escribir_lineas(lineas)?).map(|_| ())
    }
    fn guardar_registro_fallo(&mut self) {
        if let Ok((_, o)) = self.sh("logcat -d -b system,crash 2>/dev/null | tail -n 300") {
            let _ = std::fs::create_dir_all(&self.registro);
            let _ = std::fs::write(self.registro.join("fallo-logcat.txt"), o);
        }
    }
}

pub fn parsear_respaldo(salida: &str) -> Respaldo {
    let mut r = Respaldo::default();
    let mut zona = 0; // 0 cabecera, 1 anterior, 2 original
    let mut tiene_original = false;
    let (mut ant, mut orig) = (Vec::new(), Vec::new());
    for l in salida.lines() {
        let t = l.trim();
        match t {
            "---anterior" => {
                zona = 1;
                continue;
            }
            "---original" => {
                zona = 2;
                continue;
            }
            _ => {}
        }
        match zona {
            0 => {
                if let Some((k, v)) = t.split_once('=') {
                    match k {
                        "hay" => r.hay = v == "1",
                        "habia" => r.anterior_habia = v == "1",
                        "marca" => r.marca = v == "1",
                        "tiene_original" => tiene_original = v == "1",
                        _ => {}
                    }
                }
            }
            1 => {
                if linea_valida(t) {
                    ant.push(t.to_string())
                }
            }
            _ => {
                if linea_valida(t) {
                    orig.push(t.to_string())
                }
            }
        }
    }
    r.anterior_lineas = ant;
    r.originales = if tiene_original { Some(orig) } else { None };
    r
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;

    // ---- invitado simulado ------------------------------------------------------------------------------------

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Candidata {
        Buena,
        /// boot_completed no llega nunca
        SinArranque,
        /// system_server muere N veces antes de arrancar
        Muertes(u32),
        /// arranca, pero muere N veces despues de boot_completed
        MuertesTrasArrancar(u32),
    }

    struct Sim {
        t: f64,
        lib: Option<String>,
        lineas: Vec<String>,
        respaldo: Respaldo,
        respaldo_so: Option<String>,
        marca: bool,
        boot_en: Option<f64>,
        muertes: u32,
        muertes_en: Option<f64>,
        modo_de: HashMap<String, Candidata>,
        reinicios: u32,
        acciones: Vec<String>,
        sin_adb: bool,
        verity_pide_reinicio: bool,
        md5_tras_push: Option<String>,
        remount_falla: bool,
    }

    impl Sim {
        fn nuevo() -> Sim {
            Sim {
                t: 0.0,
                lib: None,
                lineas: vec!["ro.product.cpu.abilist=x86_64".into(), "ro.product.cpu.abilist64=x86_64".into()],
                respaldo: Respaldo::default(),
                respaldo_so: None,
                marca: false,
                boot_en: Some(0.0),
                muertes: 0,
                muertes_en: None,
                modo_de: HashMap::new(),
                reinicios: 0,
                acciones: Vec::new(),
                sin_adb: false,
                verity_pide_reinicio: true,
                md5_tras_push: None,
                remount_falla: false,
            }
        }
        fn con_puente(mut self, md5: &str) -> Sim {
            self.lib = Some(md5.into());
            self.lineas = lineas_del_puente();
            self
        }
        fn configurado(&self) -> bool {
            self.lineas.iter().any(|l| l == "ro.dalvik.vm.native.bridge=libheddle.so")
        }
    }

    impl Invitado for Sim {
        fn esperar_arranque(&mut self, limite: u64) -> Result<(), String> {
            if self.sin_adb {
                return Err("sin respuesta".into());
            }
            match self.boot_en {
                Some(b) if b - self.t <= limite as f64 => {
                    self.t = self.t.max(b);
                    Ok(())
                }
                _ => {
                    self.t += limite as f64;
                    Err(format!("adbd no estuvo listo en {} s", limite))
                }
            }
        }
        fn esperar_adb(&mut self, _limite: u64) -> Result<(), String> {
            if self.sin_adb {
                Err("sin respuesta".into())
            } else {
                Ok(())
            }
        }
        fn root(&mut self) -> Result<(), String> {
            self.acciones.push("root".into());
            Ok(())
        }
        fn remontar(&mut self) -> Result<(), String> {
            self.acciones.push("remount".into());
            if self.remount_falla {
                Err("remount fallo".into())
            } else {
                Ok(())
            }
        }
        fn desactivar_verity(&mut self) -> Result<bool, String> {
            self.acciones.push("disable-verity".into());
            Ok(self.verity_pide_reinicio)
        }
        fn reiniciar(&mut self) -> Result<(), String> {
            self.reinicios += 1;
            self.acciones.push("reiniciar".into());
            self.t += 12.0;
            let modo = self.lib.as_ref().and_then(|m| self.modo_de.get(m)).copied().filter(|_| self.configurado()).unwrap_or(Candidata::Buena);
            self.muertes = 0;
            self.muertes_en = None;
            self.boot_en = match modo {
                Candidata::SinArranque => None,
                Candidata::Muertes(n) => {
                    self.muertes = n;
                    None
                }
                Candidata::MuertesTrasArrancar(n) => {
                    self.muertes_en = Some(self.t + 25.0);
                    self.muertes = n;
                    Some(self.t + 20.0)
                }
                Candidata::Buena => Some(self.t + 25.0),
            };
            Ok(())
        }
        fn arranque_completo(&mut self) -> bool {
            self.boot_en.is_some_and(|b| self.t >= b)
        }
        fn muertes_system_server(&mut self) -> u32 {
            match self.muertes_en {
                Some(en) if self.t < en => 0,
                _ => self.muertes,
            }
        }
        fn pausa(&mut self, s: u64) {
            self.t += s as f64;
        }
        fn ahora(&self) -> f64 {
            self.t
        }
        fn md5_biblioteca(&mut self) -> Result<Option<String>, String> {
            Ok(self.lib.clone())
        }
        fn puente_configurado(&mut self) -> Result<bool, String> {
            Ok(self.configurado())
        }
        fn lineas_propiedades(&mut self) -> Result<Vec<String>, String> {
            Ok(self.lineas.clone())
        }
        fn leer_respaldo(&mut self) -> Result<Respaldo, String> {
            let mut r = self.respaldo.clone();
            r.marca = self.marca;
            Ok(r)
        }
        fn guardar_respaldo(&mut self, hay: bool, lineas: &[String], originales: Option<&[String]>) -> Result<(), String> {
            self.acciones.push("respaldo".into());
            self.respaldo.hay = true;
            self.respaldo.anterior_habia = hay;
            self.respaldo_so = if hay { self.lib.clone() } else { None };
            self.respaldo.anterior_lineas = lineas.to_vec();
            if let Some(o) = originales {
                self.respaldo.originales = Some(o.to_vec());
            }
            Ok(())
        }
        fn marca(&mut self, poner: bool) -> Result<(), String> {
            self.marca = poner;
            Ok(())
        }
        fn poner_biblioteca(&mut self, local: &Path) -> Result<(), String> {
            self.acciones.push("push".into());
            self.lib = Some(self.md5_tras_push.clone().unwrap_or_else(|| local.to_string_lossy().into_owned()));
            Ok(())
        }
        fn quitar_biblioteca(&mut self) -> Result<(), String> {
            self.acciones.push("quitar-lib".into());
            self.lib = None;
            Ok(())
        }
        fn restaurar_biblioteca(&mut self) -> Result<(), String> {
            self.acciones.push("restaurar-lib".into());
            self.lib = self.respaldo_so.clone();
            Ok(())
        }
        fn escribir_lineas(&mut self, l: &[String]) -> Result<(), String> {
            self.acciones.push(format!("lineas({})", l.len()));
            self.lineas.retain(|x| !es_propiedad_del_puente(x));
            self.lineas.extend(l.iter().cloned());
            Ok(())
        }
        fn guardar_registro_fallo(&mut self) {
            self.acciones.push("registro".into());
        }
    }

    fn sin_prog(_: &str) {}

    #[test]
    fn exito_con_el_puente_ya_configurado() {
        let mut s = Sim::nuevo().con_puente("a".repeat(32).as_str());
        s.respaldo.originales = Some(vec!["ro.product.cpu.abilist=x86_64".into()]);
        let md5 = "b".repeat(32);
        s.md5_tras_push = Some(md5.clone());
        let r = instalar(&mut s, Path::new("lib"), &md5, true, 90, &sin_prog);
        assert_eq!(codigo(&r), 0, "{:?}", r);
        let Ok(Resultado::Instalado { md5: m, muertes, .. }) = &r else { panic!("{:?}", r) };
        assert_eq!((m, *muertes), (&md5, 0));
        // un solo reinicio (ya estaba configurado: sin disable-verity), con la copia de la anterior y la marca quitada
        assert_eq!(s.reinicios, 1);
        assert!(!s.acciones.contains(&"disable-verity".to_string()) && !s.marca);
        assert!(s.respaldo.anterior_habia && s.respaldo_so.as_deref() == Some("a".repeat(32).as_str()));
        assert_eq!(s.lib.as_deref(), Some(md5.as_str()));
        assert!(describir(&r).starts_with("INSTALACION OK"));
    }

    #[test]
    fn primera_vez_hace_disable_verity_y_guarda_las_lineas_originales() {
        let mut s = Sim::nuevo();
        let md5 = "c".repeat(32);
        s.md5_tras_push = Some(md5.clone());
        let r = instalar(&mut s, Path::new("lib"), &md5, true, 90, &sin_prog);
        assert_eq!(codigo(&r), 0, "{:?}", r);
        assert_eq!(s.reinicios, 2, "disable-verity + instalacion");
        assert_eq!(s.respaldo.originales, Some(vec!["ro.product.cpu.abilist=x86_64".to_string(), "ro.product.cpu.abilist64=x86_64".to_string()]));
        assert!(!s.respaldo.anterior_habia);
        assert!(s.configurado());
        // disable-verity antes de copiar la biblioteca, y la candidata se vigila con un reinicio propio
        let pos = |n: &str| s.acciones.iter().position(|a| a == n).unwrap();
        assert!(pos("disable-verity") < pos("push"));
        // verity ya desactivada: no se reinicia de mas
        let mut s2 = Sim::nuevo();
        s2.verity_pide_reinicio = false;
        s2.md5_tras_push = Some(md5.clone());
        assert_eq!(codigo(&instalar(&mut s2, Path::new("lib"), &md5, true, 90, &sin_prog)), 0);
        assert_eq!(s2.reinicios, 1);
    }

    #[test]
    fn misma_md5_no_hace_nada() {
        let md5 = "d".repeat(32);
        let mut s = Sim::nuevo().con_puente(&md5);
        let r = instalar(&mut s, Path::new("lib"), &md5, true, 90, &sin_prog);
        assert!(matches!(r, Ok(Resultado::SinCambios(_))) && codigo(&r) == 0);
        assert_eq!(s.reinicios, 0);
        assert!(!s.acciones.iter().any(|a| matches!(a.as_str(), "push" | "remount" | "respaldo" | "reiniciar")));
        // misma biblioteca pero sin las propiedades: si se instala
        let mut s = Sim::nuevo();
        s.lib = Some(md5.clone());
        s.md5_tras_push = Some(md5.clone());
        assert_eq!(codigo(&instalar(&mut s, Path::new("lib"), &md5, true, 90, &sin_prog)), 0);
        assert!(s.reinicios >= 1);
    }

    #[test]
    fn fallo_de_arranque_restaura_la_anterior() {
        let (vieja, mala) = ("a".repeat(32), "e".repeat(32));
        let mut s = Sim::nuevo().con_puente(&vieja);
        s.modo_de.insert(mala.clone(), Candidata::SinArranque);
        s.md5_tras_push = Some(mala.clone());
        let r = instalar(&mut s, Path::new("lib"), &mala, true, 90, &sin_prog);
        assert_eq!(codigo(&r), 2, "{:?}", r);
        let Ok(Resultado::Restaurado { motivo, .. }) = &r else { panic!() };
        assert!(motivo.contains("boot_completed no llego en 90 s"), "{}", motivo);
        // quedo la anterior y configurada, Android arranco otra vez y no queda marca
        assert_eq!(s.lib.as_deref(), Some(vieja.as_str()));
        assert!(s.configurado() && !s.marca && s.arranque_completo());
        assert_eq!(s.reinicios, 2, "instalar y restaurar");
        assert!(s.acciones.contains(&"registro".to_string()) && s.acciones.contains(&"restaurar-lib".to_string()));
        assert!(describir(&r).starts_with("RESTAURADO") && describir(&r).contains("boot_completed"));
        // el limite se respeta: no se espera mas de 90 s de vigilancia
        assert!(s.t < 400.0, "t={}", s.t);
    }

    #[test]
    fn dos_muertes_de_system_server_restauran() {
        let mala = "f".repeat(32);
        let mut s = Sim::nuevo();
        s.verity_pide_reinicio = false;
        s.modo_de.insert(mala.clone(), Candidata::Muertes(2));
        s.md5_tras_push = Some(mala.clone());
        let r = instalar(&mut s, Path::new("lib"), &mala, true, 90, &sin_prog);
        assert_eq!(codigo(&r), 2, "{:?}", r);
        let Ok(Resultado::Restaurado { motivo, .. }) = &r else { panic!() };
        assert!(motivo.contains("murio 2 veces"));
        // sin puente previo: sin biblioteca y con las lineas originales
        assert_eq!(s.lib, None);
        assert!(!s.configurado());
        assert_eq!(s.lineas, vec!["ro.product.cpu.abilist=x86_64".to_string(), "ro.product.cpu.abilist64=x86_64".to_string()]);
    }

    #[test]
    fn una_muerte_se_tolera_pero_dos_tras_arrancar_no() {
        let md5 = "1".repeat(32);
        let mut s = Sim::nuevo();
        s.verity_pide_reinicio = false;
        s.modo_de.insert(md5.clone(), Candidata::Muertes(1));
        s.md5_tras_push = Some(md5.clone());
        // una muerte y luego Android arranca: la sim deja boot_en None con Muertes, se prueba con arranque y una muerte
        s.modo_de.insert(md5.clone(), Candidata::MuertesTrasArrancar(1));
        let r = instalar(&mut s, Path::new("lib"), &md5, true, 90, &sin_prog);
        assert_eq!(codigo(&r), 0, "una muerte no es fallo: {:?}", r);
        let Ok(Resultado::Instalado { muertes, .. }) = r else { panic!() };
        assert_eq!(muertes, 1);
        // dos muertes justo despues de boot_completed: la gracia las ve
        let md5 = "2".repeat(32);
        let mut s = Sim::nuevo();
        s.verity_pide_reinicio = false;
        s.modo_de.insert(md5.clone(), Candidata::MuertesTrasArrancar(2));
        s.md5_tras_push = Some(md5.clone());
        let r = instalar(&mut s, Path::new("lib"), &md5, true, 90, &sin_prog);
        assert_eq!(codigo(&r), 2, "{:?}", r);
        let Ok(Resultado::Restaurado { motivo, .. }) = r else { panic!() };
        assert!(motivo.contains("tras arrancar"));
    }

    #[test]
    fn md5_distinta_tras_instalar_es_fallo_vigilado() {
        let md5 = "3".repeat(32);
        let mut s = Sim::nuevo();
        s.verity_pide_reinicio = false;
        s.md5_tras_push = Some("4".repeat(32));
        let r = instalar(&mut s, Path::new("lib"), &md5, true, 90, &sin_prog);
        assert_eq!(codigo(&r), 2, "{:?}", r);
        assert!(describir(&r).contains("md5 distinto"));
        assert_eq!(s.lib, None);
    }

    #[test]
    fn sin_adb_es_error_previo_y_no_cambia_nada() {
        let mut s = Sim::nuevo();
        s.sin_adb = true;
        let r = instalar(&mut s, Path::new("lib"), &"5".repeat(32), true, 90, &sin_prog);
        assert_eq!(codigo(&r), 1);
        assert!(matches!(r, Err(ErrorPuente::Previo(_))));
        assert!(s.acciones.is_empty() && s.reinicios == 0);
        // remount que falla antes de instalar: error previo, sin marca y sin biblioteca
        let mut s = Sim::nuevo();
        s.verity_pide_reinicio = false;
        s.remount_falla = true;
        let r = instalar(&mut s, Path::new("lib"), &"5".repeat(32), true, 90, &sin_prog);
        assert_eq!(codigo(&r), 1);
        assert!(!s.acciones.contains(&"push".to_string()) && !s.marca);
    }

    #[test]
    fn restauracion_fallida_es_codigo_3() {
        let (vieja, mala) = ("a".repeat(32), "6".repeat(32));
        let mut s = Sim::nuevo().con_puente(&vieja);
        s.modo_de.insert(mala.clone(), Candidata::SinArranque);
        s.md5_tras_push = Some(mala.clone());
        // la anterior tambien es mala: tras restaurar Android no llega
        s.modo_de.insert(vieja.clone(), Candidata::SinArranque);
        let r = instalar(&mut s, Path::new("lib"), &mala, true, 90, &sin_prog);
        assert_eq!(codigo(&r), 3, "{:?}", r);
        assert!(describir(&r).starts_with("ERROR GRAVE") && describir(&r).contains("restauracion tambien"));
    }

    #[test]
    fn sin_reiniciar_queda_sin_vigilar_y_check_lo_supervisa() {
        let md5 = "7".repeat(32);
        let mut s = Sim::nuevo();
        s.verity_pide_reinicio = false;
        s.md5_tras_push = Some(md5.clone());
        let r = instalar(&mut s, Path::new("lib"), &md5, false, 90, &sin_prog);
        assert!(matches!(r, Ok(Resultado::SinVigilar(_))) && s.marca);
        assert!(describir(&r).contains("SIN VIGILAR"));
        // otra instalacion encima se niega mientras la marca siga
        let r2 = instalar(&mut s, Path::new("lib"), &"8".repeat(32), true, 90, &sin_prog);
        assert!(matches!(r2, Err(ErrorPuente::Previo(ref t)) if t.contains("check")));
        // el siguiente arranque es bueno: check confirma
        s.reiniciar().unwrap();
        let c = comprobar(&mut s, 90, &sin_prog);
        assert!(matches!(c, Ok(Resultado::Sano(_))) && !s.marca, "{:?}", c);
        // y si el arranque es malo, check restaura
        let mut s = Sim::nuevo();
        s.verity_pide_reinicio = false;
        s.modo_de.insert(md5.clone(), Candidata::MuertesTrasArrancar(3));
        s.md5_tras_push = Some(md5.clone());
        instalar(&mut s, Path::new("lib"), &md5, false, 90, &sin_prog).unwrap();
        s.reiniciar().unwrap();
        let c = comprobar(&mut s, 90, &sin_prog);
        assert_eq!(codigo(&c), 2, "{:?}", c);
        assert_eq!(s.lib, None);
        assert!(!s.marca);
    }

    #[test]
    fn quitar_devuelve_las_propiedades_originales() {
        let mut s = Sim::nuevo();
        s.verity_pide_reinicio = false;
        let md5 = "9".repeat(32);
        s.md5_tras_push = Some(md5.clone());
        let originales = s.lineas.clone();
        assert_eq!(codigo(&instalar(&mut s, Path::new("lib"), &md5, true, 90, &sin_prog)), 0);
        assert!(s.configurado());
        let r = quitar(&mut s, true, &sin_prog);
        assert!(matches!(r, Ok(Resultado::Quitado(_))), "{:?}", r);
        assert_eq!(s.lib, None);
        assert_eq!(s.lineas, originales);
        // sin las originales guardadas: se quitan las lineas y se avisa
        let mut s = Sim::nuevo().con_puente(&md5);
        let r = quitar(&mut s, false, &sin_prog);
        assert!(describir(&r).contains("no estaban guardadas") && s.lineas.is_empty() && s.reinicios == 0);
        // sin nada instalado
        let mut s = Sim::nuevo();
        assert!(describir(&quitar(&mut s, true, &sin_prog)).contains("nada que quitar") && s.reinicios == 0);
    }

    #[test]
    fn restaurar_devuelve_lo_anterior() {
        let (vieja, nueva) = ("a".repeat(32), "b".repeat(32));
        let mut s = Sim::nuevo().con_puente(&vieja);
        s.verity_pide_reinicio = false;
        s.md5_tras_push = Some(nueva.clone());
        assert_eq!(codigo(&instalar(&mut s, Path::new("lib"), &nueva, true, 90, &sin_prog)), 0);
        assert_eq!(s.lib.as_deref(), Some(nueva.as_str()));
        let r = restaurar(&mut s, &sin_prog);
        assert!(matches!(r, Ok(Resultado::Quitado(_))), "{:?}", r);
        assert_eq!(s.lib.as_deref(), Some(vieja.as_str()));
        // sin respaldo no hay que restaurar
        let mut s = Sim::nuevo();
        assert_eq!(codigo(&restaurar(&mut s, &sin_prog)), 1);
    }

    /// TODOS los textos que el traductor ARM puede dar a la interfaz (progreso y resultados) en cada escenario simulado:
    /// exito, primera vez, sin cambios, fallo de arranque, dos muertes, md5 distinta, sin adb, remount, sin vigilar, quitar,
    /// restaurar, comprobar, restauracion fallida. Solo para la prueba que recorre la interfaz entera (src/ajustes.rs).
    pub(crate) fn todos_los_textos() -> Vec<String> {
        use std::cell::RefCell;
        let log: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let prog = |t: &str| log.borrow_mut().push(t.to_string());
        let mut resultados: Vec<Result<Resultado, ErrorPuente>> = Vec::new();
        let lib = Path::new("lib");
        let (md5a, md5b) = ("a".repeat(32), "b".repeat(32));
        // exito con el puente ya configurado y primera vez (disable-verity)
        let mut s = Sim::nuevo().con_puente(&md5a);
        s.respaldo.originales = Some(vec!["ro.product.cpu.abilist=x86_64".into()]);
        s.md5_tras_push = Some(md5b.clone());
        resultados.push(instalar(&mut s, lib, &md5b, true, 90, &prog));
        let mut s = Sim::nuevo();
        s.md5_tras_push = Some(md5b.clone());
        resultados.push(instalar(&mut s, lib, &md5b, true, 90, &prog));
        // sin cambios
        let mut s = Sim::nuevo().con_puente(&md5b);
        resultados.push(instalar(&mut s, lib, &md5b, true, 90, &prog));
        // fallos vigilados: sin arranque, dos muertes, dos muertes tras arrancar, md5 distinta
        for modo in [Candidata::SinArranque, Candidata::Muertes(2), Candidata::MuertesTrasArrancar(2)] {
            let mut s = Sim::nuevo().con_puente(&md5a);
            s.modo_de.insert(md5b.clone(), modo);
            s.md5_tras_push = Some(md5b.clone());
            resultados.push(instalar(&mut s, lib, &md5b, true, 90, &prog));
        }
        let mut s = Sim::nuevo();
        s.verity_pide_reinicio = false;
        s.md5_tras_push = Some("4".repeat(32));
        resultados.push(instalar(&mut s, lib, &md5b, true, 90, &prog));
        // restauracion fallida (la anterior tambien es mala)
        let mut s = Sim::nuevo().con_puente(&md5a);
        s.modo_de.insert(md5b.clone(), Candidata::SinArranque);
        s.modo_de.insert(md5a.clone(), Candidata::SinArranque);
        s.md5_tras_push = Some(md5b.clone());
        resultados.push(instalar(&mut s, lib, &md5b, true, 90, &prog));
        // errores previos: sin adb, remount, marca pendiente
        let mut s = Sim::nuevo();
        s.sin_adb = true;
        resultados.push(instalar(&mut s, lib, &md5b, true, 90, &prog));
        let mut s = Sim::nuevo();
        s.verity_pide_reinicio = false;
        s.remount_falla = true;
        resultados.push(instalar(&mut s, lib, &md5b, true, 90, &prog));
        // sin vigilar y check (bueno y malo), y otra instalacion encima
        let mut s = Sim::nuevo();
        s.verity_pide_reinicio = false;
        s.md5_tras_push = Some(md5b.clone());
        resultados.push(instalar(&mut s, lib, &md5b, false, 90, &prog));
        resultados.push(instalar(&mut s, lib, &md5a, true, 90, &prog));
        s.reiniciar().unwrap();
        resultados.push(comprobar(&mut s, 90, &prog));
        let mut s = Sim::nuevo();
        s.verity_pide_reinicio = false;
        s.modo_de.insert(md5b.clone(), Candidata::MuertesTrasArrancar(3));
        s.md5_tras_push = Some(md5b.clone());
        let _ = instalar(&mut s, lib, &md5b, false, 90, &prog);
        s.reiniciar().unwrap();
        resultados.push(comprobar(&mut s, 90, &prog));
        let mut s = Sim::nuevo();
        resultados.push(comprobar(&mut s, 90, &prog));
        // quitar: con originales, sin ellas, sin reiniciar, sin nada instalado
        let mut s = Sim::nuevo();
        s.verity_pide_reinicio = false;
        s.md5_tras_push = Some(md5b.clone());
        let _ = instalar(&mut s, lib, &md5b, true, 90, &prog);
        resultados.push(quitar(&mut s, true, &prog));
        let mut s = Sim::nuevo().con_puente(&md5b);
        resultados.push(quitar(&mut s, false, &prog));
        resultados.push(quitar(&mut Sim::nuevo(), true, &prog));
        // restaurar: con respaldo y sin el
        let mut s = Sim::nuevo().con_puente(&md5a);
        s.verity_pide_reinicio = false;
        s.md5_tras_push = Some(md5b.clone());
        let _ = instalar(&mut s, lib, &md5b, true, 90, &prog);
        resultados.push(restaurar(&mut s, &prog));
        resultados.push(restaurar(&mut Sim::nuevo(), &prog));
        // mensajes sueltos de los errores graves
        resultados.push(Err(ErrorPuente::RestauracionFallo("tras restaurar Android no llego a boot_completed".into())));
        let mut v = log.into_inner();
        for r in &resultados {
            v.push(describir(r));
        }
        v.push(crate::textos::texto(AVISO_RIESGO).to_string());
        v.push(error_orden("[ -f /system/lib64/libheddle.so ] && md5sum /system/lib64/libheddle.so", 1, "libheddle.so: Permission denied"));
        v
    }

    // ---- piezas puras -----------------------------------------------------------------------------------------

    fn elf(maquina: u16, tipo: u16, con_simbolo: bool, largo: usize) -> Vec<u8> {
        let mut d = vec![0u8; largo];
        d[..4].copy_from_slice(b"\x7fELF");
        d[4] = 2;
        d[5] = 1;
        d[16..18].copy_from_slice(&tipo.to_le_bytes());
        d[18..20].copy_from_slice(&maquina.to_le_bytes());
        if con_simbolo {
            d[100..115].copy_from_slice(b"NativeBridgeItf");
        }
        d
    }

    #[test]
    fn validacion_de_la_biblioteca() {
        assert_eq!(validar_biblioteca(&elf(0x3E, 3, true, 8192)), Ok(8192));
        assert!(validar_biblioteca(b"basura").unwrap_err().contains("no es un ELF"));
        assert!(validar_biblioteca(&elf(0xB7, 3, true, 8192)).unwrap_err().contains("x86-64"));
        assert!(validar_biblioteca(&elf(0x3E, 2, true, 8192)).unwrap_err().contains("ET_DYN"));
        assert!(validar_biblioteca(&elf(0x3E, 3, false, 8192)).unwrap_err().contains("NativeBridgeItf"));
        assert!(validar_biblioteca(&elf(0x3E, 3, true, 200)).unwrap_err().contains("pequena"));
        let mut e32 = elf(0x3E, 3, true, 8192);
        e32[4] = 1;
        assert!(validar_biblioteca(&e32).unwrap_err().contains("64 bits"));
    }

    #[test]
    fn lineas_y_ordenes_de_propiedades() {
        assert!(LINEAS_PUENTE.iter().all(|l| linea_valida(l)));
        assert!(!linea_valida("a=b;rm -rf /") && !linea_valida("sin igual") && !linea_valida("a b=c") && !linea_valida("=x") && !linea_valida("k='x'"));
        let o = orden_escribir_lineas(&lineas_del_puente()).unwrap();
        assert!(o.starts_with("sed -i '/^ro\\.dalvik\\.vm\\.native\\.bridge=/d;") && o.contains("/vendor/build.prop") && o.contains("printf '%s\\n' 'ro.dalvik.vm.native.bridge=libheddle.so'"));
        assert!(o.contains("tail -c1"), "asegura el salto de linea final");
        let vacia = orden_escribir_lineas(&[]).unwrap();
        assert!(vacia.starts_with("sed -i") && !vacia.contains("printf"));
        assert!(orden_escribir_lineas(&["x=1;reboot".to_string()]).is_err());
        assert!(guardar_lineas_cmd("/d/f", &["a=b".into()]).unwrap().contains("> /d/f"));
        assert_eq!(guardar_lineas_cmd("/d/f", &[]).unwrap(), ": > /d/f");
    }

    #[test]
    fn estado_de_la_salida() {
        let salida = "biblioteca=1\nmd5=5e4f26d507a3d4b7a7a3b0e3f1f2c3d4\nbytes=10135552\nnb=libheddle.so\nisa=x86_64\nabilist=x86_64,arm64-v8a\nabilist64=x86_64,arm64-v8a\narchivo=ro.dalvik.vm.native.bridge=libheddle.so ro.dalvik.vm.isa.arm64=x86_64 \nboot=1\nmuertes=0\nzygote=1\nlogs=4\nmarca=0\nrespaldo=1\n";
        let e = parsear_estado(salida);
        assert!(e.instalado() && e.boot && e.en_zygote && e.respaldo && !e.marca);
        assert_eq!(e.bytes, Some(10135552));
        assert_eq!(e.en_archivo.len(), 2);
        let r = e.resumen();
        assert!(r.contains("instalado: si") && r.contains("zygote64") && r.contains("sys.boot_completed=1"));
        let v = parsear_estado("biblioteca=0\nmd5=\nnb=\nboot=0\nmuertes=3\nzygote=0\nlogs=0\nmarca=1\n");
        assert!(!v.instalado() && v.md5.is_none() && v.muertes == 3);
        assert!(v.resumen().contains("no termino su supervision") && v.resumen().contains("sin evidencia"));
        assert_eq!(parsear_estado("basura"), Estado::default());
        assert_eq!(parsear_estado("md5=xyz").md5, None);
        let o = orden_estado();
        assert!(o.contains(BIBLIOTECA) && o.contains("zygote64") && o.contains(RESPALDO) && o.contains("FATAL EXCEPTION IN SYSTEM PROCESS"));
    }

    #[test]
    fn respaldo_de_la_salida() {
        let r = parsear_respaldo("hay=1\nhabia=1\nmarca=0\ntiene_original=1\n---anterior\nro.dalvik.vm.native.bridge=libheddle.so\nbasura con espacios\n---original\nro.product.cpu.abilist=x86_64\n");
        assert!(r.hay && r.anterior_habia && !r.marca);
        assert_eq!(r.anterior_lineas, vec!["ro.dalvik.vm.native.bridge=libheddle.so".to_string()]);
        assert_eq!(r.originales, Some(vec!["ro.product.cpu.abilist=x86_64".to_string()]));
        let v = parsear_respaldo("hay=0\nhabia=0\nmarca=1\ntiene_original=0\n---anterior\n---original\n");
        assert!(!v.hay && v.marca && v.originales.is_none());
    }

    #[test]
    fn busqueda_de_la_biblioteca_local() {
        let d = PathBuf::from(format!("{}/puente-{}", std::env::var("TMPDIR").unwrap_or_else(|_| ".".into()), std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sub")).unwrap();
        assert_eq!(buscar_biblioteca(&d), None);
        std::fs::write(d.join("sub/libheddle.so"), b"x").unwrap();
        assert_eq!(buscar_biblioteca(&d), Some(d.join("sub/libheddle.so")));
        // --file admite el archivo o una carpeta; sin --file, la carpeta de la cache; si falta, el mensaje dice que no se descarga
        let f = d.join("sub/libheddle.so").to_string_lossy().into_owned();
        assert_eq!(biblioteca_local(Some(&f), &d.join("vacia")), Ok(d.join("sub/libheddle.so")));
        assert_eq!(biblioteca_local(Some(&d.to_string_lossy()), &d.join("vacia")), Ok(d.join("sub/libheddle.so")));
        assert_eq!(biblioteca_local(None, &d), Ok(d.join("sub/libheddle.so")));
        assert!(biblioteca_local(None, &d.join("vacia")).unwrap_err().contains("no descarga nada"));
        assert!(biblioteca_local(Some("/no/existe"), &d).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn codigos_de_salida_como_el_guion() {
        let ok: Result<Resultado, ErrorPuente> = Ok(Resultado::Quitado(vec![]));
        assert_eq!(codigo(&ok), 0);
        assert_eq!(codigo(&Ok(Resultado::Restaurado { motivo: "x".into(), notas: vec![] })), 2);
        assert_eq!(codigo(&Err(ErrorPuente::Previo("x".into()))), 1);
        assert_eq!(codigo(&Err(ErrorPuente::RestauracionFallo("x".into()))), 3);
    }

    #[test]
    fn aviso_de_riesgo_dice_lo_esencial() {
        let aviso = crate::textos::texto(AVISO_RIESGO);
        assert!(aviso.contains("reinicia Android") && aviso.contains("restablece de fábrica") && aviso.contains("restaura sola") && aviso.contains("No validado"));
    }
}
