//! Carpetas compartidas con Android: carpetas del equipo que Android ve dentro de su almacenamiento interno
//! (`/sdcard/NOMBRE`) y que una app normal (por ejemplo Material Files, sin root) puede listar, leer, crear y borrar.
//!
//! DISENO (medido en la maquina, ver BITACORA.md):
//!   * cada carpeta es un dispositivo virtiofs propio (etiqueta `arshare0`..`arshare3`) servido por un virtiofsd sin
//!     privilegios; la lista vive en `config` (claves `share.0`..`share.3`, formato `NOMBRE:rw|ro:/ruta`);
//!   * dentro del invitado la monta el propio init, con un servicio estatico `/vendor/etc/init/arshare.rc` (sin
//!     scripts, en /system/etc/init para que corra como `init`): `mount virtiofs arshareN /data/media/0/NOMBRE ...`. Los nombres y el modo viajan por el bootconfig
//!     del kernel (`androidboot.arshareN=NOMBRE`, `androidboot.arshareNf=ro|rw`), que Android expone como
//!     `ro.boot.arshareN`: asi cambiar la lista no exige tocar el invitado;
//!   * `/sdcard` es un FUSE que sirve MediaProvider sobre `/data/media`; el montaje se propaga a su espacio de nombres.
//!     SELinux (medido con el dominio de init): init solo puede montar con `defcontext=media_rw_data_file` (los
//!     archivos sin etiqueta propia pasan a ser `media_rw_data_file`, que MediaProvider puede usar); `context=` y
//!     `fscontext=` los niega (relabelto), y no puede montar sobre un directorio `media_rw_data_file` (mounton): el
//!     punto de montaje se etiqueta `shell_data_file` una vez con adb root (`preparar_y_montar`). Sin parche de politica;
//!   * permisos: MediaProvider abre los archivos con su propio uid (sin capacidades), de modo que el dueno de los
//!     archivos que ve debe ser ese uid. virtiofsd traduce el uid/gid del dueno de la carpeta del equipo al uid de
//!     MediaProvider (`--translate-uid map:...`). Ese uid NO es estable entre instalaciones (10101 en la imagen de
//!     prueba): `share status` lo lee por adb y lo guarda en `share.uid`; si difiere del usado en el arranque actual, las
//!     escrituras fallan hasta reiniciar la maquina.
//!
//! Este modulo no toca SDL ni la maquina: modelo, validacion, argumentos de arranque e instalacion en el invitado.

use crate::adb;
use crate::config::Config;
use std::path::Path;
use crate::textos::{tx, txf};

pub const MAX: usize = 4;
/// En /system y no en /vendor: init ejecuta las acciones de los .rc de /vendor con el dominio `vendor_init`, que no puede
/// ni mirar `/data/media` (avc denied search media_userdir_file, medido); las de /system corren como `init`.
pub const RC_RUTA: &str = "/system/etc/init/arshare.rc";
/// Uid de MediaProvider si no se ha aprendido otro.
pub const UID_DEFECTO: u32 = 10101;
pub const PAQUETE_MEDIA: &str = "com.android.providers.media.module";

/// Nombres que Android usa para sus carpetas de siempre: no se pueden tapar.
const RESERVADOS: &[&str] = &[
    "android", "alarms", "audiobooks", "dcim", "documents", "download", "movies", "music", "notifications", "pictures", "podcasts", "recordings", "ringtones", "lost.dir",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Carpeta {
    pub nombre: String,
    pub ro: bool,
    pub ruta: String,
}

impl Carpeta {
    pub fn texto(&self) -> String {
        format!("{}:{}:{}", self.nombre, if self.ro { "ro" } else { "rw" }, self.ruta)
    }

    /// Lee `NOMBRE:rw|ro:/ruta` (la ruta puede llevar `:`).
    pub fn parse(t: &str) -> Result<Carpeta, String> {
        let t = t.trim();
        let mut it = t.splitn(3, ':');
        let (n, m, r) = (it.next().unwrap_or(""), it.next(), it.next());
        let (Some(m), Some(r)) = (m, r) else { return Err(txf!("compartir.se_espera_nombre_rw_ro_ruta", format!("{:?}", t))) };
        validar_nombre(n)?;
        let ro = match m {
            "rw" => false,
            "ro" => true,
            x => return Err(txf!("compartir.el_modo_es_rw_o_ro", format!("{:?}", x))),
        };
        validar_ruta_texto(r)?;
        Ok(Carpeta { nombre: n.to_string(), ro, ruta: r.to_string() })
    }
}

pub fn validar_nombre(n: &str) -> Result<(), String> {
    if n.is_empty() || n.chars().count() > 32 {
        return Err(tx!("compartir.el_nombre_va_de_1_a_32_caracteres").into());
    }
    if !n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
        return Err(tx!("compartir.el_nombre_solo_admite_letras_numeros").into());
    }
    if n.starts_with('.') {
        return Err(tx!("compartir.el_nombre_no_puede_empezar_por_punto").into());
    }
    if RESERVADOS.contains(&n.to_ascii_lowercase().as_str()) {
        return Err(txf!("compartir.es_una_carpeta_propia_de_android", format!("{:?}", n)));
    }
    Ok(())
}

fn validar_ruta_texto(r: &str) -> Result<(), String> {
    if !r.starts_with('/') {
        return Err(tx!("compartir.la_ruta_de_la_carpeta_del_equipo_debe").into());
    }
    if r.contains(['#', '\n', '\r', '\0']) {
        return Err(tx!("compartir.la_ruta_no_puede_llevar_ni_saltos_de").into());
    }
    Ok(())
}

/// Valida y normaliza el valor de una clave `share.N` (para `config`): `ninguna` o `NOMBRE:rw|ro:/ruta`.
pub fn normalizar(v: &str) -> Result<String, String> {
    let v = v.trim();
    if matches!(v.to_lowercase().as_str(), "" | "ninguna" | "ninguno" | "none") {
        return Ok("ninguna".into());
    }
    Carpeta::parse(v).map(|c| c.texto())
}

/// Comprueba que la carpeta del equipo existe y es una carpeta.
pub fn comprobar_ruta(r: &str) -> Result<(), String> {
    validar_ruta_texto(r)?;
    match std::fs::metadata(r) {
        Ok(m) if m.is_dir() => Ok(()),
        Ok(_) => Err(txf!("compartir.no_es_una_carpeta", r)),
        Err(e) => Err(format!("{}: {}", r, e)),
    }
}

/// Donde buscar virtiofsd (el servicio de archivos del anfitrion que necesitan las carpetas compartidas).
pub fn buscar_virtiofsd() -> Option<String> {
    ["/app/libexec/virtiofsd", "/usr/libexec/virtiofsd", "/usr/lib/qemu/virtiofsd", "/usr/lib/virtiofsd", "/usr/bin/virtiofsd"].iter().find(|p| Path::new(p).exists()).map(|p| p.to_string())
}

pub fn clave_ranura(i: usize) -> String {
    format!("share.{}", i)
}

/// Las carpetas de `config`, en orden de ranura (las ranuras vacias se saltan).
pub fn lista(cfg: &Config) -> Vec<Carpeta> {
    (0..MAX).filter_map(|i| Carpeta::parse(&cfg.get(&clave_ranura(i))).ok()).collect()
}

/// Escribe la lista compactada en las ranuras (las que sobran quedan en `ninguna`).
pub fn guardar(cfg: &mut Config, l: &[Carpeta]) -> Result<(), String> {
    if l.len() > MAX {
        return Err(txf!("compartir.como_mucho_carpetas_compartidas", MAX));
    }
    for i in 0..MAX {
        let v = l.get(i).map_or("ninguna".to_string(), |c| c.texto());
        cfg.set(&clave_ranura(i), &v)?;
    }
    Ok(())
}

/// Agrega una carpeta (valida nombre, ruta existente, repetidos y cupo). Con `existe=false` no mira el disco (pruebas).
pub fn agregar(cfg: &mut Config, c: Carpeta, existe: bool) -> Result<(), String> {
    validar_nombre(&c.nombre)?;
    if existe {
        comprobar_ruta(&c.ruta)?;
    } else {
        validar_ruta_texto(&c.ruta)?;
    }
    let mut l = lista(cfg);
    if l.iter().any(|x| x.nombre.eq_ignore_ascii_case(&c.nombre)) {
        return Err(txf!("compartir.ya_hay_una_carpeta_compartida_llamada", format!("{:?}", c.nombre)));
    }
    if l.iter().any(|x| x.ruta == c.ruta) {
        return Err(tx!("compartir.esa_carpeta_del_equipo_ya_esta").into());
    }
    if l.len() >= MAX {
        return Err(txf!("compartir.como_mucho_carpetas_compartidas", MAX));
    }
    l.push(c);
    guardar(cfg, &l)
}

pub fn quitar(cfg: &mut Config, nombre: &str) -> Result<Carpeta, String> {
    let mut l = lista(cfg);
    let i = l.iter().position(|x| x.nombre.eq_ignore_ascii_case(nombre)).ok_or_else(|| txf!("compartir.no_hay_una_carpeta_compartida_llamada", format!("{:?}", nombre)))?;
    let c = l.remove(i);
    guardar(cfg, &l)?;
    Ok(c)
}

pub fn listado(l: &[Carpeta]) -> String {
    if l.is_empty() {
        return tx!("compartir.no_hay_carpetas_compartidas_share_add").into();
    }
    let mut s = String::new();
    for c in l {
        let falta = if comprobar_ruta(&c.ruta).is_err() { tx!("compartir.aviso_la_carpeta_no_existe_en_el_equipo") } else { "" };
        s.push_str(&format!("{:<20} {}  /sdcard/{}  <-  {}{}\n", c.nombre, if c.ro { tx!("compartir.solo_lectura") } else { tx!("compartir.lectura_y_escritura") }, c.nombre, c.ruta, falta));
    }
    s
}

/// Uid de MediaProvider que se usara al arrancar: `share.uid` de la configuracion (aprendido o fijado) o el defecto.
pub fn uid_configurado(cfg: &Config) -> u32 {
    cfg.entero("share.uid").unwrap_or(UID_DEFECTO)
}

/// Lo que `start` necesita por cada carpeta: etiqueta virtiofs, carpeta, solo lectura y traduccion de dueno.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Servicio {
    pub etiqueta: String,
    pub ruta: String,
    pub ro: bool,
    /// (uid del anfitrion, gid del anfitrion) que se presentan al invitado como el uid de MediaProvider
    pub dueno: Option<(u32, u32)>,
    pub uid_invitado: u32,
}

pub fn etiqueta(i: usize) -> String {
    format!("arshare{}", i)
}

/// Parametros de arranque de Android (bootconfig) para la lista.
pub fn bootconfig(l: &[Carpeta]) -> Vec<String> {
    let mut v = Vec::new();
    for (i, c) in l.iter().enumerate() {
        v.push(format!("androidboot.arshare{}=\"{}\"", i, c.nombre));
        v.push(format!("androidboot.arshare{}f=\"{}\"", i, if c.ro { "ro" } else { "rw" }));
    }
    v
}

/// Argumentos de virtiofsd para la traduccion del dueno (vacios si no se conoce el dueno).
pub fn args_traduccion(s: &Servicio) -> Vec<String> {
    match s.dueno {
        Some((u, g)) => vec!["--translate-uid".into(), format!("map:{}:{}:1", s.uid_invitado, u), "--translate-gid".into(), format!("map:{}:{}:1", s.uid_invitado, g)],
        None => Vec::new(),
    }
}

pub fn servicios(l: &[Carpeta], uid_invitado: u32) -> Vec<Servicio> {
    use std::os::unix::fs::MetadataExt;
    l.iter()
        .enumerate()
        .map(|(i, c)| Servicio {
            etiqueta: etiqueta(i),
            ruta: c.ruta.clone(),
            ro: c.ro,
            dueno: std::fs::metadata(&c.ruta).ok().map(|m| (m.uid(), m.gid())),
            uid_invitado,
        })
        .collect()
}

/// Servicio de init (estatico) que monta las carpetas al terminar el arranque. No crea el directorio (`mkdir` de init le
/// pondria `media_rw_data_file`, sobre el que init no puede montar): lo deja `preparar_y_montar`. Opciones de montaje
/// medidas con el dominio de init: solo `defcontext` (init no puede `relabelto` otros tipos; con `context=` o `fscontext=`
/// el montaje se niega).
/// El `wait` espera a que vold prepare el almacenamiento emulado (`Bind mounting /data/media to /mnt/pass_through`, unos
/// 100 ms despues de boot_completed en esta imagen): un montaje hecho antes no se propaga a lo que ve MediaProvider
/// (medido: la carpeta aparecia vacia y sin permiso). El hilo `vigilar` de la ventana y `share setup` lo corrigen igual.
pub fn rc() -> String {
    // texto-interno: contenido del servicio de init del invitado
    let mut s = String::from("# Carpetas compartidas de weft: monta cada dispositivo virtiofs arshareN en /data/media/0/NOMBRE (MediaProvider\n# lo sirve en /sdcard/NOMBRE). Los nombres llegan por el bootconfig (androidboot.arshareN). Instalado por weft.\n");
    for i in 0..MAX {
        s.push_str(&format!(
            "\non property:sys.boot_completed=1 && property:ro.boot.arshare{i}=*\n    wait /mnt/pass_through/0/emulated/0 10\n    mount virtiofs arshare{i} /data/media/0/${{ro.boot.arshare{i}}} nosuid nodev noatime ${{ro.boot.arshare{i}f}} defcontext=u:object_r:media_rw_data_file:s0\n"
        ));
    }
    s
}

/// Etiqueta del punto de montaje mientras no hay nada montado: la unica que a la vez permite a init montar encima
/// (`mounton`) y no es la de /data/media (medido con el dominio de init).
pub const ETIQUETA_PUNTO: &str = "u:object_r:shell_data_file:s0";

/// Orden de shell (como root en el invitado) que, para cada carpeta que este arranque trae (`ro.boot.arshareN`), prepara
/// el punto de montaje y monta si init no lo hizo. Escribe una linea `NOMBRE: estado` por carpeta.
fn orden_montar() -> String {
    format!(
        "for i in 0 1 2 3; do N=$(getprop ro.boot.arshare$i); [ -n \"$N\" ] || continue; F=$(getprop ro.boot.arshare${{i}}f); [ \"$F\" = ro ] || F=rw; D=/data/media/0/$N; \
         if grep -q \" /mnt/pass_through/0/emulated/0/$N virtiofs\" /proc/mounts; then echo \"$N: montada\"; continue; fi; \
         grep -q \" $D virtiofs\" /proc/mounts && umount $D 2>/dev/null; \
         mkdir -p $D && chown media_rw:media_rw $D && chmod 770 $D && chcon {et} $D; \
         if mount -t virtiofs -o nosuid,nodev,noatime,$F,defcontext=u:object_r:media_rw_data_file:s0 arshare$i $D 2>/dev/null; then echo \"$N: montada ahora\"; else echo \"$N: NO se pudo montar (el dispositivo arshare$i no esta en esta maquina: se agrego despues de arrancar)\"; fi; done",
        et = ETIQUETA_PUNTO
    )
}

/// Deja listos los puntos de montaje y monta las carpetas del arranque actual que falten. Devuelve el informe.
pub fn preparar_y_montar(cid: u32) -> Result<String, String> {
    adb::hacer_root(cid)?;
    let (_, o, e) = adb::conectar(cid, 15)?.shell_texto(&orden_montar())?;
    Ok(format!("{}{}", o, e))
}

/// ¿Esta instalado el servicio de montaje en el invitado?
pub fn rc_instalado(cid: u32) -> Result<bool, String> {
    adb::hacer_root(cid)?;
    let (_, o, _) = adb::conectar(cid, 15)?.shell_texto(&format!("[ -f {} ] && echo 1 || echo 0", RC_RUTA))?;
    Ok(o.trim() == "1")
}

/// Instala (o reemplaza) el servicio de montaje en el invitado; hace `remount` (se pierde al reiniciar Android, el
/// archivo no). Devuelve true si cambio algo.
pub fn instalar_rc(cid: u32, tmp: &Path, forzar: bool) -> Result<bool, String> {
    adb::hacer_root(cid)?;
    let (_, actual, _) = adb::conectar(cid, 15)?.shell_texto(&format!("cat {} 2>/dev/null", RC_RUTA))?;
    if !forzar && actual.replace('\r', "") == rc() {
        return Ok(false);
    }
    adb::remontar(cid)?;
    std::fs::create_dir_all(tmp).map_err(|e| e.to_string())?;
    let local = tmp.join("arshare.rc");
    std::fs::write(&local, rc()).map_err(|e| e.to_string())?;
    adb::conectar(cid, 15)?.push(&local, RC_RUTA)?;
    let (code, o, e) = adb::conectar(cid, 15)?.shell_texto(&format!("chmod 644 {rc}; chcon u:object_r:system_file:s0 {rc}; ls -lZ {rc}", rc = RC_RUTA))?;
    if code != 0 {
        return Err(txf!("compartir.no_se_pudo_etiquetar", RC_RUTA, o, e));
    }
    Ok(true)
}

/// Uid de MediaProvider en el invitado.
pub fn aprender_uid(cid: u32) -> Result<u32, String> {
    adb::hacer_root(cid)?;
    let (_, o, _) = adb::conectar(cid, 15)?.shell_texto(&format!("stat -c %u /data/data/{}", PAQUETE_MEDIA))?;
    o.trim().parse().map_err(|_| txf!("compartir.no_se_pudo_leer_el_uid_de_mediaprovider", format!("{:?}", o.trim())))
}

/// Como se dice que una carpeta esta (o no) montada en el invitado.
pub fn texto_montaje(montada: bool) -> &'static str {
    if montada {
        tx!("compartir.montada_en_sdcard")
    } else {
        tx!("compartir.no_montada_se_monta_al_arrancar_android")
    }
}

/// Textos de este modulo que llegan a la interfaz (para la prueba que la recorre entera).
#[cfg(test)]
pub(crate) fn todos_los_textos() -> Vec<String> {
    let l = vec![Carpeta { nombre: "Shared".into(), ro: false, ruta: "/mi/carpeta".into() }, Carpeta { nombre: "Fotos".into(), ro: true, ruta: "/mis/fotos".into() }];
    let mut v = vec![listado(&l), listado(&[]), texto_montaje(true).to_string(), texto_montaje(false).to_string()];
    for e in [validar_nombre("DCIM"), validar_nombre(""), validar_nombre(".x"), comprobar_ruta("/no/existe/seguro"), comprobar_ruta("relativa")] {
        v.push(e.err().unwrap_or_default());
    }
    v
}

/// Informe del estado dentro del invitado: que esta montado y con que dueno.
pub fn estado_invitado(cid: u32, l: &[Carpeta]) -> Result<String, String> {
    adb::hacer_root(cid)?;
    let mut s = String::new();
    let (_, o, _) = adb::conectar(cid, 15)?.shell_texto("mount | grep virtiofs | grep /data/media")?;
    for c in l {
        let montada = o.lines().any(|x| x.contains(&format!("/data/media/0/{} ", c.nombre)));
        s.push_str(&format!("{}: {}\n", c.nombre, texto_montaje(montada)));
    }
    Ok(s)
}

/// Hilo de la ventana: cada vez que el invitado arranca (cambia `boot_id`), tras boot_completed deja montadas las carpetas
/// compartidas del arranque, aplica los ajustes de Android (`apply-settings`) y, si esta pedido, comprueba el root. Es la red de seguridad del servicio de init (que corre
/// antes de que vold termine de preparar el almacenamiento): asi un reinicio de Android no deja nada a medias.
pub fn vigilar(dir: std::path::PathBuf, cid: u32) {
    let mut ultimo: Option<String> = None;
    loop {
        std::thread::sleep(std::time::Duration::from_secs(5));
        let Ok((_, o, _)) = adb::conectar(cid, 5).and_then(|mut c| c.shell_texto("cat /proc/sys/kernel/random/boot_id")) else { continue };
        let id = o.trim().to_string();
        if id.is_empty() || ultimo.as_deref() == Some(id.as_str()) {
            continue;
        }
        ultimo = Some(id);
        if adb::esperar(cid, 120, true).is_err() {
            ultimo = None;
            continue;
        }
        std::thread::sleep(std::time::Duration::from_secs(6));
        let cfg = Config::cargar(&dir);
        match preparar_y_montar(cid) {
            Ok(t) if !t.trim().is_empty() => eprintln!("{}", txf!("compartir.carpetas_compartidas", t.trim().replace('\n', "; "))),
            Ok(_) => {}
            Err(e) => eprintln!("{}", txf!("compartir.carpetas_compartidas", e)),
        }
        // Bluetooth apagado y raton como toque (idempotente)
        match crate::aplicar::aplicar(&crate::vm::State { dir: dir.clone() }, cid, &cfg, 60) {
            Ok(l) => eprintln!("{}", txf!("compartir.ajustes_de_android", l.join("; "))),
            Err(e) => eprintln!("{}", txf!("compartir.ajustes_de_android", e)),
        }
        if cfg.bool("root.cargar_al_inicio") {
            let r = crate::root::proveedor().asegurar(cid, &dir, &cfg, &|_| {});
            match &r {
                Ok(t) => eprintln!("root: {}", t),
                Err(e) => eprintln!("root: {}", e),
            }
            // la ventana lo muestra en la barra y en Acceso root (vista::aviso_root_auto); antes solo iba a window.log
            let _ = std::fs::write(dir.join(crate::vista::ROOT_AUTO), linea_root_auto(&r));
        }
        // perfil de dispositivo: si Android arranco con otro que el elegido, se aplica (reinicia Android: la vuelta siguiente
        // de este bucle vera el arranque nuevo y ya no habra nada que hacer). Lo ultimo, porque puede reiniciar
        if let Some(l) = crate::dispositivo::sincronizar_maquina(&dir, cid, &cfg, &|_| {}) {
            eprintln!("{}", txf!("compartir.perfil_de_dispositivo", l.replace('\n', "; ")));
        }
    }
}

/// Linea del archivo `root-auto` con el resultado del root automatico: `ok TEXTO` o `error TEXTO`, en una sola linea
/// (es lo que lee `vista::aviso_root_auto`). Pura.
pub fn linea_root_auto(r: &Result<String, String>) -> String {
    let (clase, texto) = match r {
        Ok(t) => ("ok", t),
        Err(e) => ("error", e),
    };
    format!("{} {}\n", clase, texto.split_whitespace().collect::<Vec<_>>().join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El resultado del root automatico llega a la barra: la linea que escribe el vigilante es la que entiende la ventana.
    #[test]
    fn el_root_automatico_se_anota_para_la_ventana() {
        use crate::ajustes::Tono;
        let ok = linea_root_auto(&Ok("cargado\n(ya estaba instalado)".into()));
        assert_eq!(ok, "ok cargado (ya estaba instalado)\n");
        assert!(matches!(crate::vista::aviso_root_auto(&ok), Some((Tono::Exito, t)) if t.contains("cargado (ya estaba instalado)")));
        let err = linea_root_auto(&Err("no hay archivos\nen la carpeta".into()));
        assert_eq!(err, "error no hay archivos en la carpeta\n");
        assert!(matches!(crate::vista::aviso_root_auto(&err), Some((Tono::Error, t)) if t.contains("no hay archivos en la carpeta")));
        assert_eq!(crate::vista::aviso_root_auto(&linea_root_auto(&Ok(String::new()))), None);
    }

    #[test]
    fn nombres_validos_e_invalidos() {
        for ok in ["Shared", "mis-datos", "a.b_c", "X"] {
            assert!(validar_nombre(ok).is_ok(), "{}", ok);
        }
        for mal in ["", ".oculta", "con espacio", "a/b", "tildé", "DCIM", "download", "Android", &"x".repeat(33)] {
            assert!(validar_nombre(mal).is_err(), "{}", mal);
        }
    }

    #[test]
    fn texto_y_lectura() {
        let c = Carpeta::parse("Shared:rw:/home/u/mi:carpeta").unwrap();
        assert_eq!(c, Carpeta { nombre: "Shared".into(), ro: false, ruta: "/home/u/mi:carpeta".into() });
        assert_eq!(Carpeta::parse(&c.texto()).unwrap(), c);
        assert!(Carpeta::parse("Shared:ro:/x").unwrap().ro);
        for mal in ["Shared", "Shared:rw", "Shared:xx:/x", "Shared:rw:relativa", "Shared:rw:/a#b", ":rw:/x"] {
            assert!(Carpeta::parse(mal).is_err(), "{}", mal);
        }
        assert_eq!(normalizar("  ninguna ").unwrap(), "ninguna");
        assert_eq!(normalizar("").unwrap(), "ninguna");
    }

    #[test]
    fn lista_en_config() {
        let mut cfg = Config::nueva();
        assert!(lista(&cfg).is_empty());
        agregar(&mut cfg, Carpeta { nombre: "A".into(), ro: false, ruta: "/x/a".into() }, false).unwrap();
        agregar(&mut cfg, Carpeta { nombre: "B".into(), ro: true, ruta: "/x/b".into() }, false).unwrap();
        assert!(agregar(&mut cfg, Carpeta { nombre: "a".into(), ro: false, ruta: "/x/c".into() }, false).is_err(), "nombre repetido sin distinguir mayusculas");
        assert!(agregar(&mut cfg, Carpeta { nombre: "C".into(), ro: false, ruta: "/x/b".into() }, false).is_err(), "ruta repetida");
        assert_eq!(lista(&cfg).len(), 2);
        assert_eq!(quitar(&mut cfg, "A").unwrap().ruta, "/x/a");
        // la lista se compacta: B pasa a la ranura 0
        assert_eq!(cfg.get("share.0"), "B:ro:/x/b");
        assert_eq!(cfg.get("share.1"), "ninguna");
        assert!(quitar(&mut cfg, "zzz").is_err());
        for i in 0..MAX {
            agregar(&mut cfg, Carpeta { nombre: format!("N{}", i), ro: false, ruta: format!("/y/{}", i) }, false).ok();
        }
        assert_eq!(lista(&cfg).len(), MAX);
        assert!(agregar(&mut cfg, Carpeta { nombre: "Z".into(), ro: false, ruta: "/z".into() }, false).unwrap_err().contains("como mucho"));
    }

    #[test]
    fn ruta_que_no_existe() {
        assert!(comprobar_ruta("/no/existe/seguro").is_err());
        assert!(comprobar_ruta("/").is_ok());
        assert!(comprobar_ruta("/proc/self/stat").unwrap_err().contains("no es una carpeta"));
        let mut cfg = Config::nueva();
        assert!(agregar(&mut cfg, Carpeta { nombre: "A".into(), ro: false, ruta: "/no/existe/seguro".into() }, true).is_err());
        assert!(lista(&cfg).is_empty());
    }

    #[test]
    fn bootconfig_y_traduccion() {
        let l = vec![Carpeta { nombre: "A".into(), ro: false, ruta: "/x".into() }, Carpeta { nombre: "B".into(), ro: true, ruta: "/y".into() }];
        assert_eq!(bootconfig(&l), vec!["androidboot.arshare0=\"A\"", "androidboot.arshare0f=\"rw\"", "androidboot.arshare1=\"B\"", "androidboot.arshare1f=\"ro\""]);
        let s = Servicio { etiqueta: "arshare0".into(), ruta: "/x".into(), ro: false, dueno: Some((1000, 1001)), uid_invitado: 10101 };
        assert_eq!(args_traduccion(&s), vec!["--translate-uid", "map:10101:1000:1", "--translate-gid", "map:10101:1001:1"]);
        assert!(args_traduccion(&Servicio { dueno: None, ..s }).is_empty());
        assert_eq!(servicios(&l, 5)[1].etiqueta, "arshare1");
    }

    #[test]
    fn el_servicio_de_init_cubre_todas_las_ranuras() {
        let r = rc();
        for i in 0..MAX {
            assert!(r.contains(&format!("property:ro.boot.arshare{}=*", i)));
            assert!(r.contains(&format!("mount virtiofs arshare{} /data/media/0/${{ro.boot.arshare{}}}", i, i)));
        }
        assert!(r.contains("defcontext=u:object_r:media_rw_data_file:s0") && !r.contains("fscontext") && !r.contains("mkdir"));
        assert!(orden_montar().contains(ETIQUETA_PUNTO) && orden_montar().contains("ro.boot.arshare$i"));
        assert_eq!(r.matches("on property:").count(), MAX);
    }

    #[test]
    fn uid_por_defecto_y_aprendido() {
        let mut cfg = Config::nueva();
        assert_eq!(uid_configurado(&cfg), UID_DEFECTO);
        cfg.set("share.uid", "10234").unwrap();
        assert_eq!(uid_configurado(&cfg), 10234);
    }
}
