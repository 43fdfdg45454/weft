//! Root persistente con KernelSU-Next (modo LKM) dentro del invitado, sin tocar imagenes ni el kernel.
//!
//! Que instala `root enable`, todo por el adb propio:
//!   * `/data/adb/ksud-boot`: copia del ksud de KernelSU-Next (`late-load` borra `/data/adb/ksud` al terminar, por eso
//!     el servicio usa su propia copia);
//!   * `/vendor/etc/init/ksunext.rc`: un servicio de init que ejecuta `ksud-boot late-load` al terminar el arranque
//!     (`sys.boot_completed=1`). Se lanza con el shell del sistema porque SELinux no deja que init ejecute un binario de
//!     `/data` directamente. `/vendor` es un overlay que vive en el disco de la maquina: sobrevive a los reinicios de
//!     Android y de la maquina, y se pierde si se regenera el disco.
//!   * opcionalmente el gestor (APK de KernelSU-Next) para conceder root a cada app.
//!
//! ABSTRACCION: la interfaz (pantalla de configuracion, panel) y las ordenes dependen de `ProveedorRoot`, no del nombre
//! de un producto concreto. Hoy hay UNA implementacion (`KernelSuNext`); la interfaz solo dice "Acceso root" y el proveedor
//! concreto (nombre, version, de donde baja, que instala) sale de `ProveedorRoot::nombre_tecnico` y de `descargas`, que la
//! confirmacion muestra como "Detalles tecnicos" (es una descarga de un tercero y el usuario debe poder verla).
//!
//! Este modulo no toca SDL: lo usan la orden `root` y la seccion "Acceso root" de la pantalla de configuracion.

use crate::adb;
use crate::textos::{self, elige};
use std::path::{Path, PathBuf};
use crate::textos::{clave, tx, txf};

pub const VERSION: &str = "v3.4.0";
pub const ORIGEN: &str = "github.com/KernelSU-Next/KernelSU-Next/releases";
pub const RC_RUTA: &str = "/vendor/etc/init/ksunext.rc";
pub const KSUD_BOOT: &str = "/data/adb/ksud-boot";
pub const PAQUETE_GESTOR: &str = "com.rifsxd.ksunext";
pub const PAQUETE_OFICIAL: &str = "me.weishu.kernelsu";

/// Contenido verificado del servicio de init.
// texto-interno: contenido del servicio de init del invitado
pub const RC: &str = "# KernelSU-Next (LKM): carga el modulo al terminar el arranque. Instalado por weft (`weft root enable`).
service ksunext_load /system/bin/sh -c \"/data/adb/ksud-boot late-load\"
    user root
    group root
    seclabel u:r:su:s0
    oneshot
    disabled

on property:sys.boot_completed=1
    start ksunext_load
";

pub struct Archivo {
    pub remoto: &'static str,
    pub local: &'static str,
    pub bytes: u64,
}

impl Archivo {
    /// Como se llama el archivo para la gente (la interfaz no nombra al proveedor).
    pub fn nombre_ui(&self) -> &'static str {
        if self.remoto.ends_with(".apk") {
            tx!("root.la_app_gestora")
        } else {
            tx!("root.el_componente_de_root")
        }
    }
}

pub const KSUD: Archivo = Archivo { remoto: "ksud-x86_64-linux-android", local: "next-ksud-x86_64-linux-android", bytes: 5_583_488 };
pub const APK: Archivo = Archivo { remoto: "KernelSU_Next_v3.4.0_33294-release.apk", local: "KernelSU_Next_v3.4.0_33294-release.apk", bytes: 11_857_955 };

pub const AVISO_DETECCION: &str = clave!("root.con_root_cargado_las_apps_que_detectan");

/// Un archivo que se usara (ya descargado por el usuario), para mostrarlo antes de pedir confirmacion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Descarga {
    pub nombre: String,
    pub bytes: u64,
    pub origen: String,
    /// solo hace falta si se pide instalar el gestor
    pub solo_gestor: bool,
}

/// Texto generico de las descargas para la interfaz (ver `ProveedorRoot::detalles_ui`).
fn detalles_ui(con_gestor: bool) -> Vec<String> {
    let mut v = vec![txf!("root.componente_de_terceros_para_dar_acceso", VERSION.trim_start_matches('v'))];
    for d in descargas(con_gestor) {
        let que = if d.solo_gestor { tx!("root.la_app_gestora_de_permisos") } else { tx!("root.el_componente_principal") };
        v.push(txf!("root.hace_falta_ya_descargado_en_el_equipo_se", que, mib(d.bytes), textos::sitio_generico(&d.origen), if d.solo_gestor { tx!("root.solo_si_instalas_el_gestor") } else { "" }));
    }
    v.push(tx!("root.no_descarga_nada_indica_la_carpeta_donde").to_string());
    v.push(tx!("root.se_copian_al_disco_de_la_maquina_datos").to_string());
    v.push(tx!("root.los_detalles_completos_proveedor_version").to_string());
    v
}

fn descargas(con_gestor: bool) -> Vec<Descarga> {
    let mut v = vec![Descarga { nombre: KSUD.remoto.to_string(), bytes: KSUD.bytes, origen: ORIGEN.to_string(), solo_gestor: false }];
    if con_gestor {
        v.push(Descarga { nombre: APK.remoto.to_string(), bytes: APK.bytes, origen: ORIGEN.to_string(), solo_gestor: true });
    }
    v
}

/// Lo que la interfaz y las ordenes necesitan de un proveedor de root persistente en el invitado.
pub trait ProveedorRoot: Sync {
    /// Nombre tecnico completo (consola, registro y "Detalles tecnicos"): producto, version y modo de instalacion.
    fn nombre_tecnico(&self) -> String;
    /// Archivos que hacen falta, ya descargados por el usuario (con `con_gestor`, tambien el gestor).
    fn descargas(&self, con_gestor: bool) -> Vec<Descarga>;
    /// Lo mismo para la interfaz, SIN nombrar al proveedor: que es (componente de terceros), tamano de cada descarga, dominio
    /// del origen, integridad comprobada y donde ver los detalles completos.
    fn detalles_ui(&self, con_gestor: bool) -> Vec<String>;
    /// Consulta el estado en el invitado.
    fn estado(&self, cid: u32) -> Result<Estado, String>;
    /// Instala el servicio de arranque (y opcionalmente el gestor); devuelve lo que paso. `cache`: carpeta con los archivos
    /// que bajo el usuario (`--from`, `root.carpeta`...: solo se lee); lo que weft genera va a la cache de root de la
    /// maquina de `estado_dir` (ver `ruta_rc_generado`).
    fn habilitar(&self, cid: u32, estado_dir: &Path, cache: &Path, op: &Opciones, prog: Progreso) -> Result<String, String>;
    /// Quita el servicio de arranque.
    fn deshabilitar(&self, cid: u32, prog: Progreso) -> Result<String, String>;
    fn instalar_gestor(&self, cid: u32, cache: &Path, prog: Progreso) -> Result<String, String>;
    /// `root.cargar_al_inicio` / `root.instalar_automaticamente`: comprueba tras arrancar (e instala si esta pedido).
    fn asegurar(&self, cid: u32, estado_dir: &Path, cfg: &crate::config::Config, prog: Progreso) -> Result<String, String>;
}

/// KernelSU-Next v3.4.0 en modo LKM: la unica implementacion actual.
pub struct KernelSuNext;

impl ProveedorRoot for KernelSuNext {
    fn nombre_tecnico(&self) -> String {
        txf!("root_tec.next_modulo_cargable_modo", VERSION)
    }
    fn descargas(&self, con_gestor: bool) -> Vec<Descarga> {
        descargas(con_gestor)
    }
    fn detalles_ui(&self, con_gestor: bool) -> Vec<String> {
        detalles_ui(con_gestor)
    }
    fn estado(&self, cid: u32) -> Result<Estado, String> {
        estado(cid)
    }
    fn habilitar(&self, cid: u32, estado_dir: &Path, cache: &Path, op: &Opciones, prog: Progreso) -> Result<String, String> {
        habilitar(cid, estado_dir, cache, op, prog)
    }
    fn deshabilitar(&self, cid: u32, prog: Progreso) -> Result<String, String> {
        deshabilitar(cid, prog)
    }
    fn instalar_gestor(&self, cid: u32, cache: &Path, prog: Progreso) -> Result<String, String> {
        instalar_gestor(cid, cache, prog)
    }
    fn asegurar(&self, cid: u32, estado_dir: &Path, cfg: &crate::config::Config, prog: Progreso) -> Result<String, String> {
        asegurar(cid, estado_dir, cfg, prog)
    }
}

/// El proveedor de root en uso.
pub fn proveedor() -> &'static dyn ProveedorRoot {
    &KernelSuNext
}

pub fn mib(b: u64) -> String {
    format!("{:.1} MiB", b as f64 / 1048576.0)
}

pub type Progreso<'a> = &'a dyn Fn(&str);

/// Pasos de progreso de las operaciones de root. El texto de cada uno tiene su cara de interfaz (generica) y su cara de consola.
#[derive(Clone, Debug, PartialEq)]
pub enum Paso {
    Comprobando,
    Copiando,
    AbriendoSistema,
    InstalandoServicio,
    CargandoModulo,
    InstalandoGestor,
    Reiniciando,
    EsperandoArranque,
}

impl Paso {
    /// Un ejemplo de cada paso (para recorrerlos en las pruebas).
    #[cfg(test)]
    pub const TODOS: [Paso; 8] = [Paso::Comprobando, Paso::Copiando, Paso::AbriendoSistema, Paso::InstalandoServicio, Paso::CargandoModulo, Paso::InstalandoGestor, Paso::Reiniciando, Paso::EsperandoArranque];

    pub fn texto(&self) -> String {
        match self {
            Paso::Comprobando => tx!("root.comprobando_la_maquina").into(),
            Paso::Copiando => elige(tx!("root.copiando_el_componente_de_root_al"), tx!("root_tec.copiando_al_invitado")),
            Paso::AbriendoSistema => tx!("root.abriendo_vendor_para_escritura_remount").into(),
            Paso::InstalandoServicio => tx!("root.instalando_el_servicio_de_arranque").into(),
            Paso::CargandoModulo => elige(tx!("root.cargando_el_modulo_de_root"), tx!("root_tec.cargando_el_modulo_late_load")),
            Paso::InstalandoGestor => tx!("root.instalando_el_gestor").into(),
            Paso::Reiniciando => tx!("puente.reiniciando_android").into(),
            Paso::EsperandoArranque => tx!("root.esperando_a_que_android_termine_de").into(),
        }
    }
}

/// Errores de las operaciones de root (con su cara generica de interfaz y su cara de consola).
#[derive(Clone, Debug, PartialEq)]
pub enum ErrorRoot {
    /// falta el archivo (nombre remoto) en la carpeta indicada
    Falta(&'static str, String),
    /// el tamano del archivo (bytes) no coincide con el publicado
    Tamano(&'static str, u64),
    /// el gestor no se pudo instalar: salida del instalador (solo se ve en consola)
    Gestor(String),
    /// el servicio no quedo con la etiqueta de seguridad correcta: salida (solo en consola)
    Etiqueta(String),
    SinCargarTrasReiniciar,
}

impl ErrorRoot {
    /// Un error de cada clase, para la prueba que recorre los textos de la interfaz (`todos_los_textos`).
    #[cfg(test)]
    pub fn ejemplos() -> Vec<ErrorRoot> {
        vec![
            ErrorRoot::Falta(KSUD.remoto, "/descargas".into()),
            ErrorRoot::Falta(APK.remoto, "/descargas".into()),
            ErrorRoot::Tamano(KSUD.remoto, 1234),
            ErrorRoot::Tamano(APK.remoto, 99),
            ErrorRoot::Gestor("Failure [INSTALL_FAILED] me.weishu.kernelsu".into()),
            ErrorRoot::Etiqueta("u:object_r:ksunext".into()),
            ErrorRoot::SinCargarTrasReiniciar,
        ]
    }

    pub fn texto(&self) -> String {
        let archivo = |remoto: &str| if remoto.ends_with(".apk") { &APK } else { &KSUD };
        match self {
            ErrorRoot::Falta(remoto, dir) => {
                let a = archivo(remoto);
                elige(
                    &txf!("root.falta_en_la_carpeta_no_descarga_nada", a.nombre_ui(), mib(a.bytes), dir),
                    &txf!("root.falta_bytes_o_con_el_nombre_en_no", a.remoto, a.bytes, a.local, dir),
                )
            }
            ErrorRoot::Tamano(remoto, n) => {
                let a = archivo(remoto);
                elige(&txf!("root.el_tamano_bytes_no_coincide_con_el_2", a.nombre_ui(), n, a.bytes), &txf!("root.el_tamano_bytes_no_coincide_con_el", remoto, n, a.bytes))
            }
            ErrorRoot::Gestor(salida) => elige(tx!("root.no_se_pudo_instalar_el_gestor_de"), &txf!("root.no_se_pudo_instalar_el_gestor", salida.trim())),
            ErrorRoot::Etiqueta(salida) => elige(tx!("root.no_se_pudo_dejar_el_servicio_de_arranque"), &txf!("root.no_se_pudo_dejar_el_servicio_con_la", salida.trim())),
            ErrorRoot::SinCargarTrasReiniciar => elige(tx!("root.el_modulo_de_root_no_se_cargo_solo_tras"), tx!("root.el_modulo_no_se_cargo_solo_tras")),
        }
    }
}

/// Todos los textos de progreso, de notas y de error de este modulo (para la prueba que recorre la interfaz).
#[cfg(test)]
pub(crate) fn todos_los_textos() -> Vec<String> {
    let mut v: Vec<String> = Paso::TODOS.iter().map(|p| p.texto()).collect();
    v.extend(Nota::ejemplos().iter().map(|n| n.texto()));
    v.extend(ErrorRoot::ejemplos().iter().map(|e| e.texto()));
    v.push(crate::textos::texto(AVISO_DETECCION).to_string());
    v.extend(proveedor().detalles_ui(true));
    v.extend(proveedor().detalles_ui(false));
    v
}

/// Notas del resultado de las operaciones de root (lo que paso, para quien lo lee al terminar).
#[derive(Clone, Debug, PartialEq)]
pub enum Nota {
    ServicioInstalado,
    YaCargado,
    CargadoAhora,
    /// el modulo no aparecio tras cargarlo; la salida de la herramienta solo se ve en consola
    NoAparecio(String),
    /// habia un modulo de root cargado de antes: hace falta reiniciar Android
    CargadoPrevio,
    ProximoArranque,
    /// el gestor oficial se desinstalo; lo que dijo el sistema solo se ve en consola
    GestorAnteriorQuitado(String),
    GestorInstalado,
    TrasReiniciar(bool),
    ServicioQuitado(bool),
    Aviso,
}

impl Nota {
    /// Una nota de cada clase, para la prueba que recorre los textos de la interfaz (`todos_los_textos`).
    #[cfg(test)]
    pub fn ejemplos() -> Vec<Nota> {
        vec![
            Nota::ServicioInstalado,
            Nota::YaCargado,
            Nota::CargadoAhora,
            Nota::NoAparecio("KernelSU: late-load failed".into()),
            Nota::CargadoPrevio,
            Nota::ProximoArranque,
            Nota::GestorAnteriorQuitado("Success me.weishu.kernelsu".into()),
            Nota::GestorInstalado,
            Nota::TrasReiniciar(true),
            Nota::TrasReiniciar(false),
            Nota::ServicioQuitado(true),
            Nota::ServicioQuitado(false),
            Nota::Aviso,
        ]
    }

    pub fn texto(&self) -> String {
        match self {
            Nota::ServicioInstalado => tx!("root.servicio_de_arranque_instalado").into(),
            Nota::YaCargado => elige(tx!("root.el_modulo_de_root_ya_estaba_cargado"), tx!("root.el_modulo_ya_estaba_cargado")),
            Nota::CargadoAhora => elige(tx!("root.modulo_de_root_cargado_ahora"), tx!("root.modulo_cargado_ahora")),
            Nota::NoAparecio(salida) => elige(tx!("root.el_modulo_de_root_no_aparecio_tras"), &txf!("root_tec.el_modulo_no_aparecio_tras_late_load_se", salida.trim())),
            Nota::CargadoPrevio => elige(tx!("root.hay_otro_modulo_de_root_cargado_de_antes"), tx!("root_tec.hay_un_modulo_cargado_de_antes_para_usar")),
            Nota::ProximoArranque => elige(tx!("root.se_cargara_en_el_proximo_arranque_de_2"), tx!("root.se_cargara_en_el_proximo_arranque_de")),
            Nota::GestorAnteriorQuitado(salida) => elige(tx!("root.se_desinstalo_el_gestor_anterior_que_no"), &txf!("root.gestor_oficial_desinstalado", salida.trim())),
            Nota::GestorInstalado => elige(tx!("root.gestor_de_permisos_instalado"), tx!("root_tec.gestor_de_next_instalado")),
            Nota::TrasReiniciar(cargado) => elige(&txf!("root.tras_reiniciar_modulo_de_root_cargado", if *cargado { "sí" } else { "NO" }), &txf!("root.tras_reiniciar_modulo_cargado", if *cargado { "si" } else { "NO" })),
            Nota::ServicioQuitado(modulo_sigue) => {
                if *modulo_sigue {
                    elige(tx!("root.servicio_quitado_el_modulo_cargado_sigue_2"), tx!("root.servicio_quitado_el_modulo_cargado_sigue"))
                } else {
                    tx!("root.servicio_quitado").into()
                }
            }
            Nota::Aviso => crate::textos::texto(AVISO_DETECCION).into(),
        }
    }
}

/// Carpeta con los archivos de root ya descargados por el usuario: WEFT_KSU_DIR, `root/` de la cache estandar (ver rutas.rs) o, en el modo de estado propio
/// (solo --state-dir), `ksu` dentro del estado.
pub fn dir_cache(estado: &Path) -> PathBuf {
    std::env::var("WEFT_KSU_DIR").map(PathBuf::from).unwrap_or_else(|_| crate::rutas::actual().cache_de(estado, "root", "ksu"))
}

/// Carpeta de los archivos de root: la clave `root.carpeta` de la configuracion (si no es `auto`) o `dir_cache`.
pub fn carpeta(estado: &Path, cfg: &crate::config::Config) -> PathBuf {
    let c = cfg.get("root.carpeta");
    if c != "auto" && !c.is_empty() {
        PathBuf::from(c)
    } else {
        dir_cache(estado)
    }
}

/// Busca el archivo ya descargado en `dir` (con su nombre publicado o el local) y comprueba su tamano exacto.
pub fn buscar(dir: &Path, a: &Archivo) -> Result<PathBuf, String> {
    for n in [a.remoto, a.local] {
        let p = dir.join(n);
        match std::fs::metadata(&p) {
            Ok(m) if m.is_file() && m.len() == a.bytes => return Ok(p),
            Ok(m) if m.is_file() => return Err(ErrorRoot::Tamano(a.remoto, m.len()).texto()),
            _ => {}
        }
    }
    Err(ErrorRoot::Falta(a.remoto, dir.display().to_string()).texto())
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Estado {
    /// el servicio de init esta instalado
    pub servicio: bool,
    /// la copia de ksud para el arranque existe
    pub ksud: bool,
    /// hay un modulo `kernelsu` cargado ahora
    pub cargado: bool,
    pub version: Option<String>,
    /// el gestor de KernelSU-Next esta instalado
    pub gestor: bool,
    /// el gestor oficial (me.weishu.kernelsu) esta instalado: no es el que corresponde al modulo de Next
    pub oficial: bool,
    /// el modulo reconocio a un gestor (mensaje del kernel)
    pub reconocido: bool,
}

impl Estado {
    pub fn instalado(&self) -> bool {
        self.servicio && self.ksud
    }

    pub fn resumen(&self) -> String {
        let si = |b: bool| if b { "si" } else { "no" };
        txf!("root_tec.instalado_para_cada_arranque_modulo", si(self.instalado()), si(self.cargado), self.version.as_deref().unwrap_or(tx!("comun.desconocida")), si(self.gestor), if self.oficial { tx!("root_tec.ademas_esta_instalado_el_oficial_me_que") } else { "" }, si(self.reconocido))
    }
}

fn orden_estado() -> String {
    format!(
        "echo servicio=$([ -f {rc} ] && echo 1 || echo 0); echo ksud=$([ -x {k} ] && echo 1 || echo 0); echo cargado=$(grep -c '^kernelsu ' /proc/modules); \
         echo version=$({k} debug version 2>/dev/null | head -n 1); echo gestor=$(pm path {g} >/dev/null 2>&1 && echo 1 || echo 0); \
         echo oficial=$(pm path {o} >/dev/null 2>&1 && echo 1 || echo 0); echo reconocido=$(dmesg 2>/dev/null | grep -c -E 'KernelSU: (Crowning|manager pkg)')",
        rc = RC_RUTA,
        k = KSUD_BOOT,
        g = PAQUETE_GESTOR,
        o = PAQUETE_OFICIAL
    )
}

pub fn parsear_estado(salida: &str) -> Estado {
    let mut e = Estado::default();
    for l in salida.lines() {
        let Some((k, v)) = l.trim().split_once('=') else { continue };
        let v = v.trim();
        match k {
            "servicio" => e.servicio = v == "1",
            "ksud" => e.ksud = v == "1",
            "cargado" => e.cargado = v.parse::<u32>().is_ok_and(|n| n > 0),
            "version" => e.version = Some(v).filter(|s| !s.is_empty() && !s.contains("rror") && !s.contains("nvalid")).map(|s| s.rsplit(':').next().unwrap_or(s).trim().to_string()),
            "gestor" => e.gestor = v == "1",
            "oficial" => e.oficial = v == "1",
            "reconocido" => e.reconocido = v.parse::<u32>().is_ok_and(|n| n > 0),
            _ => {}
        }
    }
    e
}

fn estado(cid: u32) -> Result<Estado, String> {
    adb::hacer_root(cid)?;
    let (_, o, _) = adb::conectar(cid, 15)?.shell_texto(&orden_estado())?;
    Ok(parsear_estado(&o))
}

/// Como `estado`, pero con un solo vistazo (sin tocar adbd: si no es root, algunos datos salen vacios).
pub struct Opciones {
    /// cargar el modulo ahora, sin esperar al proximo reinicio de Android
    pub ahora: bool,
    pub gestor: bool,
    pub reiniciar: bool,
}

fn sh(cid: u32, cmd: &str) -> Result<(i32, String), String> {
    let (c, o, e) = adb::conectar(cid, 15)?.shell_texto(cmd)?;
    Ok((c, format!("{}{}", o, e)))
}

/// Donde se genera el servicio de arranque antes de subirlo al invitado: la cache de root de la maquina de `estado`
/// (`<cache>/root`, o `<estado>/ksu` en el modo de estado propio), nunca la carpeta que indico el usuario (`--from`, `root.carpeta`
/// o WEFT_KSU_DIR: de ahi solo se leen sus descargas). Pura.
pub fn ruta_rc_generado(r: &crate::rutas::Rutas, estado: &Path) -> PathBuf {
    r.cache_de(estado, "root", "ksu").join("ksunext.rc")
}

/// Activa el root en cada arranque. `cache`: carpeta con las descargas del usuario (solo se lee). Devuelve un texto con lo
/// que paso (avisos incluidos).
fn habilitar(cid: u32, estado_dir: &Path, cache: &Path, op: &Opciones, prog: Progreso) -> Result<String, String> {
    let mut notas: Vec<String> = Vec::new();
    // primero los archivos (weft no descarga nada): si faltan se dice enseguida, sin esperar a la maquina
    let ksud = buscar(cache, &KSUD)?;
    let apk = if op.gestor { Some(buscar(cache, &APK)?) } else { None };
    prog(&Paso::Comprobando.texto());
    adb::esperar(cid, 60, true)?;
    adb::hacer_root(cid)?;
    let previo = estado(cid)?;
    prog(&Paso::Copiando.texto());
    let mut c = adb::conectar(cid, 15)?;
    c.shell_texto("mkdir -p /data/adb")?;
    c.push(&ksud, KSUD_BOOT)?;
    drop(c);
    sh(cid, &format!("chmod 755 {}", KSUD_BOOT))?;
    prog(&Paso::AbriendoSistema.texto());
    adb::remontar(cid)?;
    let rc_local = ruta_rc_generado(&crate::rutas::actual(), estado_dir);
    if let Some(d) = rc_local.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {}", d.display(), e))?;
    }
    std::fs::write(&rc_local, RC).map_err(|e| format!("{}: {}", rc_local.display(), e))?;
    prog(&Paso::InstalandoServicio.texto());
    adb::conectar(cid, 15)?.push(&rc_local, RC_RUTA)?;
    let (code, o) = sh(cid, &format!("chmod 644 {rc}; chcon u:object_r:vendor_configs_file:s0 {rc}; ls -lZ {rc}", rc = RC_RUTA))?;
    if code != 0 {
        return Err(ErrorRoot::Etiqueta(o).texto());
    }
    notas.push(Nota::ServicioInstalado.texto());
    if op.ahora {
        if previo.cargado {
            notas.push(Nota::YaCargado.texto());
        } else {
            prog(&Paso::CargandoModulo.texto());
            let (_, o) = sh(cid, &format!("{} late-load 2>&1 | tail -n 3", KSUD_BOOT))?;
            let mut ok = false;
            for _ in 0..15 {
                if estado(cid).is_ok_and(|e| e.cargado) {
                    ok = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            if ok {
                notas.push(Nota::CargadoAhora.texto());
            } else {
                notas.push(Nota::NoAparecio(o).texto());
            }
        }
    } else if previo.cargado {
        notas.push(Nota::CargadoPrevio.texto());
    } else {
        notas.push(Nota::ProximoArranque.texto());
    }
    if let Some(apk) = apk {
        instalar_apk(cid, &apk, prog, &mut notas)?;
    }
    if op.reiniciar {
        prog(&Paso::Reiniciando.texto());
        let _ = reiniciar_y_esperar(cid, prog)?;
        let e = estado(cid)?;
        notas.push(Nota::TrasReiniciar(e.cargado).texto());
        if !e.cargado {
            return Err(format!("{}\n{}", notas.join("\n"), ErrorRoot::SinCargarTrasReiniciar.texto()));
        }
    }
    notas.push(Nota::Aviso.texto());
    Ok(notas.join("\n"))
}

fn instalar_apk(cid: u32, apk: &Path, prog: Progreso, notas: &mut Vec<String>) -> Result<(), String> {
    prog(&Paso::InstalandoGestor.texto());
    let e = estado(cid)?;
    if e.oficial {
        let (_, o) = sh(cid, &format!("cmd package uninstall {}", PAQUETE_OFICIAL))?;
        notas.push(Nota::GestorAnteriorQuitado(o).texto());
    }
    let r = adb::conectar(cid, 15)?.install(apk, &["-r".to_string()])?;
    if !r.contains("Success") {
        return Err(ErrorRoot::Gestor(r).texto());
    }
    notas.push(Nota::GestorInstalado.texto());
    Ok(())
}

fn instalar_gestor(cid: u32, cache: &Path, prog: Progreso) -> Result<String, String> {
    adb::esperar(cid, 60, true)?;
    adb::hacer_root(cid)?;
    let apk = buscar(cache, &APK)?;
    let mut notas = Vec::new();
    instalar_apk(cid, &apk, prog, &mut notas)?;
    Ok(notas.join("\n"))
}

fn deshabilitar(cid: u32, prog: Progreso) -> Result<String, String> {
    adb::esperar(cid, 60, true)?;
    adb::hacer_root(cid)?;
    let cargado = estado(cid)?.cargado;
    prog(&Paso::AbriendoSistema.texto());
    adb::remontar(cid)?;
    sh(cid, &format!("rm -f {} {}", RC_RUTA, KSUD_BOOT))?;
    let e = estado(cid)?;
    if e.servicio {
        return Err(tx!("root.no_se_pudo_quitar_el_servicio").into());
    }
    Ok(Nota::ServicioQuitado(cargado).texto())
}

/// `root.cargar_al_inicio`: tras arrancar, comprueba que el root este instalado y, si falta y
/// `root.instalar_automaticamente` esta activo, lo instala (y lo carga ya). Devuelve lo que paso.
fn asegurar(cid: u32, estado_dir: &Path, cfg: &crate::config::Config, prog: Progreso) -> Result<String, String> {
    if !cfg.bool("root.cargar_al_inicio") {
        return Ok(tx!("root.root_cargar_al_inicio_esta_desactivado").into());
    }
    adb::esperar(cid, 180, true)?;
    let e = estado(cid)?;
    if e.instalado() {
        Ok(txf!("root.root_instalado", if e.cargado { tx!("root.modulo_cargado") } else { tx!("root.el_modulo_se_esta_cargando_o_necesita") }))
    } else if cfg.bool("root.instalar_automaticamente") {
        habilitar(cid, estado_dir, &carpeta(estado_dir, cfg), &Opciones { ahora: true, gestor: false, reiniciar: false }, prog)
    } else {
        Ok(tx!("root.aviso_root_cargar_al_inicio_esta_activo").into())
    }
}

/// Reinicia Android por el adb propio y espera a boot_completed y 30 s mas (reglas de estabilidad del arranque).
pub fn reiniciar_y_esperar(cid: u32, prog: Progreso) -> Result<f64, String> {
    let t0 = std::time::Instant::now();
    {
        let mut c = adb::conectar(cid, 15)?;
        let _ = c.servicio_texto("reboot:");
    }
    std::thread::sleep(std::time::Duration::from_secs(12));
    prog(&Paso::EsperandoArranque.texto());
    adb::esperar(cid, 120, true)?;
    std::thread::sleep(std::time::Duration::from_secs(30));
    Ok(t0.elapsed().as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estado_de_la_salida() {
        let e = parsear_estado("servicio=1\nksud=1\ncargado=1\nversion=Kernel Version: 33294\ngestor=1\noficial=0\nreconocido=2\n");
        assert!(e.instalado() && e.cargado && e.gestor && !e.oficial && e.reconocido);
        assert_eq!(e.version.as_deref(), Some("33294"));
        let v = parsear_estado("servicio=0\nksud=1\ncargado=0\nversion=\ngestor=0\noficial=1\nreconocido=0\n");
        assert!(!v.instalado() && !v.cargado && v.version.is_none() && v.oficial);
        assert!(v.resumen().contains("oficial"));
        // salida rota: nada se da por cierto
        assert_eq!(parsear_estado("basura"), Estado::default());
        assert_eq!(parsear_estado("version=error: algo").version, None);
    }

    #[test]
    fn rc_y_descargas() {
        assert!(RC.contains("service ksunext_load /system/bin/sh -c \"/data/adb/ksud-boot late-load\""));
        assert!(RC.contains("seclabel u:r:su:s0") && RC.contains("on property:sys.boot_completed=1"));
        assert_eq!(proveedor().descargas(false).len(), 1);
        let d = proveedor().descargas(true);
        assert_eq!(d[1].bytes, 11_857_955);
        assert!(d[1].solo_gestor && !d[0].solo_gestor);
        assert!(proveedor().nombre_tecnico().contains("KernelSU-Next"));
        assert_eq!(mib(1048576), "1.0 MiB");
    }

    #[test]
    fn los_archivos_se_buscan_en_la_carpeta_sin_descargar_nada() {
        let d = std::path::PathBuf::from(format!("{}/root-{}", std::env::var("TMPDIR").unwrap_or_else(|_| ".".into()), std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        // falta: el mensaje dice que no se descarga y donde buscar (con y sin nombre tecnico segun la cara)
        let e = buscar(&d, &KSUD).unwrap_err();
        assert!(e.contains("no descarga nada") && e.contains(&d.display().to_string()), "{}", e);
        // tamano distinto: se rechaza
        std::fs::write(d.join(KSUD.remoto), b"corto").unwrap();
        assert!(buscar(&d, &KSUD).unwrap_err().contains("no coincide"));
        // con el tamano exacto, por el nombre publicado o por el local
        std::fs::write(d.join(KSUD.remoto), vec![0u8; KSUD.bytes as usize]).unwrap();
        assert_eq!(buscar(&d, &KSUD), Ok(d.join(KSUD.remoto)));
        std::fs::rename(d.join(KSUD.remoto), d.join(KSUD.local)).unwrap();
        assert_eq!(buscar(&d, &KSUD), Ok(d.join(KSUD.local)));
        // la carpeta de la configuracion manda sobre la cache
        let mut cfg = crate::config::Config::nueva();
        cfg.set("root.carpeta", "/otra/carpeta").unwrap();
        assert_eq!(carpeta(Path::new("/estado"), &cfg), PathBuf::from("/otra/carpeta"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// El servicio de arranque se genera en la cache de root de la maquina y se sube desde ahi: nunca en la carpeta de
    /// descargas que indico el usuario (`--from`), que solo se lee.
    #[test]
    fn el_servicio_se_genera_en_la_cache_y_no_en_from() {
        let entorno = |pares: &'static [(&'static str, &'static str)]| move |k: &str| pares.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string());
        let r = crate::rutas::Rutas::resolver(&entorno(&[("HOME", "/h"), ("XDG_RUNTIME_DIR", "/run/user/1")]), 1).unwrap();
        let estado = r.ejecucion.join("m");
        let p = ruta_rc_generado(&r, &estado);
        assert_eq!(p, PathBuf::from("/h/.cache/weft/root/ksunext.rc"));
        assert!(p.starts_with(&r.cache));
        // ni la carpeta de --from ni root.carpeta cuentan: la ruta no depende de ellas
        let mut cfg = crate::config::Config::nueva();
        cfg.set("root.carpeta", "/descargas/root").unwrap();
        assert!(!p.starts_with(carpeta(&estado, &cfg)));
        // con la cache propia (--cache-dir) tambien va ahi
        let c = crate::rutas::Rutas::resolver(&entorno(&[("HOME", "/h"), (crate::rutas::VAR_CACHE, "/c")]), 1).unwrap();
        assert_eq!(ruta_rc_generado(&c, &c.ejecucion.join("m")), PathBuf::from("/c/root/ksunext.rc"));
        // modo de estado propio (solo --state-dir): la cache de la maquina vive en su estado
        let a = crate::rutas::Rutas::resolver(&entorno(&[(crate::rutas::VAR_ESTADO, "/t/estado"), ("HOME", "/h")]), 1).unwrap();
        assert!(a.estado_propio);
        assert_eq!(ruta_rc_generado(&a, Path::new("/t/estado/m")), PathBuf::from("/t/estado/m/ksu/ksunext.rc"));
    }

    #[test]
    fn la_orden_de_estado_usa_las_rutas() {
        let o = orden_estado();
        assert!(o.contains(RC_RUTA) && o.contains(KSUD_BOOT) && o.contains(PAQUETE_GESTOR) && o.contains(PAQUETE_OFICIAL));
    }
}
