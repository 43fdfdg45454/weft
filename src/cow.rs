//! Discos de copia en escritura: el disco de cada maquina es un overlay qcow2 (formato de QEMU) sobre una base raw de
//! solo lectura compartida (el disco GPT armado una vez por imagen y tamano de datos; ver imagen.rs). Crear o reiniciar
//! (volver a fabrica) un disco es escribir un overlay vacio: unos pocos KiB, sin copiar la imagen.
//!
//! El overlay lo escribe weft (sin qemu-img: el paquete no lo trae y no hace falta): una cabecera qcow2 version 3 con la
//! base como archivo de respaldo (formato raw declarado en la extension de cabecera, sin que QEMU lo adivine), su tabla de
//! contadores de referencias y una tabla L1 vacia. Lo que el invitado escribe va al overlay; lo que no ha escrito se lee de
//! la base, que QEMU abre en solo lectura.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use crate::textos::{tx, txf};

const MAGIA: &[u8; 4] = b"QFI\xfb";
/// Extension de cabecera "formato del archivo de respaldo".
const EXT_FORMATO_RESPALDO: u32 = 0xE279_2ACA;
/// Bit de `incompatible_features`: entradas L2 extendidas (subclusteres).
const L2_EXTENDIDO: u64 = 1 << 4;

/// Parametros del overlay. Ver `OPCIONES` (los elegidos con `qemu-img bench`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Opciones {
    /// log2 del tamano de cluster (16 = 64 KiB, 17 = 128 KiB, 21 = 2 MiB)
    pub bits_cluster: u32,
    /// entradas L2 extendidas: cada cluster se divide en 32 subclusteres, y una escritura pequena en un cluster que aun
    /// esta en la base solo copia su subcluster, no el cluster entero
    pub l2_extendido: bool,
    /// reservar los metadatos (tablas L2) y los clusteres de datos al crear el overlay (`preallocation=metadata` de
    /// qemu-img): la primera escritura en un cluster no tiene que asignar nada, solo marcar su subcluster. El archivo es
    /// disperso: ocupa solo los metadatos (unos 4 MiB para 30 GiB) hasta que el invitado escribe
    pub metadatos: bool,
}

/// Los parametros con que weft crea los overlays: los mas rapidos de los medidos con `qemu-img bench` (ver README, "Discos
/// de copia en escritura"). Aun asi el disco completo (raw) es mas rapido: por eso la copia en escritura es opcional.
pub const OPCIONES: Opciones = Opciones { bits_cluster: 17, l2_extendido: true, metadatos: true };

impl Opciones {
    fn cluster(&self) -> u64 {
        1 << self.bits_cluster
    }
    /// Bytes de cada entrada L2.
    fn entrada_l2(&self) -> u64 {
        if self.l2_extendido {
            16
        } else {
            8
        }
    }
    /// Bytes del disco que cubre una tabla L2 (un cluster de entradas).
    fn cubre_l2(&self) -> u64 {
        self.cluster() * (self.cluster() / self.entrada_l2())
    }
    /// Cache L2 que cubre un disco de `bytes` entero (opcion `l2-cache-size` de QEMU), con un minimo de 1 MiB.
    pub fn cache_l2(&self, bytes: u64) -> u64 {
        (bytes.div_ceil(self.cluster()) * self.entrada_l2()).next_multiple_of(1 << 20).max(1 << 20)
    }
}

/// Overlay vacio de `bytes` sobre `respaldo` (el nombre tal como se guarda: relativo a la carpeta del overlay o absoluto):
/// (metadatos, largo total del archivo). Con `o.metadatos` el archivo es disperso y mas largo que los metadatos (los
/// clusteres de datos ya reservados); sin ella, el largo es el de los metadatos. Pura.
///
/// Distribucion: cluster 0 cabecera (con la extension del formato de la base y el nombre de la base), despues la tabla de
/// contadores de referencias, sus bloques (16 bits por cluster), la tabla L1, las tablas L2 y los clusteres de datos (estos
/// dos ultimos solo con `metadatos`).
pub fn overlay(respaldo: &str, bytes: u64, o: Opciones) -> Result<(Vec<u8>, u64), String> {
    if !(14..=21).contains(&o.bits_cluster) {
        return Err(txf!("cow.tamano_de_cluster_no_admitido_2", o.bits_cluster));
    }
    if bytes == 0 || bytes % 512 != 0 {
        return Err(txf!("cow.tamano_de_disco_no_valido_para_un", bytes));
    }
    if o.metadatos && !o.l2_extendido {
        // sin subclusteres, un cluster reservado no puede seguir leyendose de la base (QEMU tampoco lo admite)
        return Err(tx!("cow.la_reserva_de_metadatos_sobre_una_base").into());
    }
    let c = o.cluster();
    let nombre = respaldo.as_bytes();
    // cabecera (104) + extension del formato de respaldo (8 + 8) + fin de extensiones (8), y despues el nombre
    let pos_nombre = 104 + 16 + 8;
    if nombre.is_empty() || nombre.len() > 1023 || pos_nombre + nombre.len() as u64 > c {
        return Err(txf!("cow.ruta_de_la_base_demasiado_larga_para_el", respaldo));
    }
    let datos = if o.metadatos { bytes.div_ceil(c) } else { 0 };
    let por_l2 = c / o.entrada_l2();
    let l2n = datos.div_ceil(por_l2);
    let l1_entradas = bytes.div_ceil(o.cubre_l2());
    let l1c = (l1_entradas * 8).div_ceil(c).max(1);
    // bloques de contadores: cubren todos los clusteres del archivo, ellos y su tabla incluidos
    let por_rb = c / 2;
    let rtc_de = |rbn: u64| (rbn * 8).div_ceil(c).max(1);
    let mut rbn = 1u64;
    loop {
        let total = 1 + rtc_de(rbn) + rbn + l1c + l2n + datos;
        let hace_falta = total.div_ceil(por_rb);
        if hace_falta <= rbn {
            break;
        }
        rbn = hace_falta;
    }
    let rtc = rtc_de(rbn);
    let rt = c;
    let rb0 = rt + rtc * c;
    let l1 = rb0 + rbn * c;
    let l2_0 = l1 + l1c * c;
    let datos0 = l2_0 + l2n * c;
    let clusteres = datos0 / c + datos;
    if l1_entradas > u32::MAX as u64 || datos0 > 1 << 31 {
        return Err(tx!("cow.disco_demasiado_grande_para_el_overlay").into());
    }
    let mut v = vec![0u8; datos0 as usize];
    let be32 = |v: &mut Vec<u8>, o: u64, x: u32| v[o as usize..o as usize + 4].copy_from_slice(&x.to_be_bytes());
    let be64 = |v: &mut Vec<u8>, o: u64, x: u64| v[o as usize..o as usize + 8].copy_from_slice(&x.to_be_bytes());
    v[0..4].copy_from_slice(MAGIA);
    be32(&mut v, 4, 3); // version
    be64(&mut v, 8, pos_nombre);
    be32(&mut v, 16, nombre.len() as u32);
    be32(&mut v, 20, o.bits_cluster);
    be64(&mut v, 24, bytes);
    be32(&mut v, 32, 0); // sin cifrado
    be32(&mut v, 36, l1_entradas as u32);
    be64(&mut v, 40, l1);
    be64(&mut v, 48, rt);
    be32(&mut v, 56, rtc as u32); // clusteres de la tabla de contadores
    be32(&mut v, 60, 0); // instantaneas
    be64(&mut v, 64, 0);
    be64(&mut v, 72, if o.l2_extendido { L2_EXTENDIDO } else { 0 });
    be64(&mut v, 80, 0);
    be64(&mut v, 88, 0);
    be32(&mut v, 96, 4); // contadores de 16 bits
    be32(&mut v, 100, 104); // largo de la cabecera
    be32(&mut v, 104, EXT_FORMATO_RESPALDO);
    be32(&mut v, 108, 3);
    v[112..115].copy_from_slice(b"raw");
    // 120..128: fin de las extensiones (tipo 0, largo 0)
    v[pos_nombre as usize..pos_nombre as usize + nombre.len()].copy_from_slice(nombre);
    for k in 0..rbn {
        be64(&mut v, rt + 8 * k, rb0 + k * c);
    }
    for i in 0..clusteres {
        v[(rb0 + 2 * i) as usize..(rb0 + 2 * i + 2) as usize].copy_from_slice(&1u16.to_be_bytes());
    }
    // con metadatos: cada entrada L1 apunta a su tabla L2 y cada entrada L2 a su cluster de datos ya reservado, con los
    // subclusteres sin asignar (mapa a cero): se siguen leyendo de la base. Contador 1: marca COPIED
    const COPIED: u64 = 1 << 63;
    for k in 0..l2n {
        be64(&mut v, l1 + 8 * k, (l2_0 + k * c) | COPIED);
    }
    for j in 0..datos {
        be64(&mut v, l2_0 + j * o.entrada_l2(), (datos0 + j * c) | COPIED);
    }
    Ok((v, datos0 + datos * c))
}

/// Ruta de `destino` relativa a la carpeta `desde` (las dos absolutas y sin `..`). Pura.
pub fn relativa(desde: &Path, destino: &Path) -> PathBuf {
    let a: Vec<_> = desde.components().collect();
    let b: Vec<_> = destino.components().collect();
    let comun = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut r = PathBuf::new();
    for _ in comun..a.len() {
        r.push("..");
    }
    for c in &b[comun..] {
        r.push(c);
    }
    r
}

/// Escribe en `out` un overlay vacio sobre la base `base` (raw), del tamano de la base. La base se guarda relativa a la
/// carpeta del overlay (QEMU la resuelve desde ahi): el conjunto se puede mover entero (`--root`). Se escribe
/// en un temporal y se renombra: nunca queda un overlay a medias.
pub fn crear(base: &Path, out: &Path) -> Result<(), String> {
    let bytes = std::fs::metadata(base).map_err(|e| format!("{}: {}", base.display(), e))?.len();
    let base_abs = std::fs::canonicalize(base).map_err(|e| format!("{}: {}", base.display(), e))?;
    let dir = out.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let dir_abs = std::fs::canonicalize(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    let nombre = relativa(&dir_abs, &base_abs);
    let (meta, largo) = overlay(&nombre.to_string_lossy(), bytes, OPCIONES)?;
    let tmp = out.with_extension("qcow2.parte");
    let r = std::fs::File::create(&tmp).and_then(|mut f| f.write_all(&meta).and_then(|_| f.set_len(largo)).and_then(|_| f.sync_all()));
    if let Err(e) = r.and_then(|_| std::fs::rename(&tmp, out)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{}: {}", out.display(), e));
    }
    Ok(())
}

/// Datos de la cabecera de un disco qcow2.
#[derive(Clone, Debug, PartialEq)]
pub struct Cabecera {
    /// tamano virtual del disco
    pub bytes: u64,
    /// archivo de respaldo resuelto (relativo a la carpeta del overlay, como hace QEMU); None si no tiene
    pub respaldo: Option<PathBuf>,
}

/// Cabecera de `ruta` si es un disco qcow2; None si no lo es (un disco raw) o no se puede leer.
pub fn cabecera(ruta: &Path) -> Option<Cabecera> {
    let mut f = std::fs::File::open(ruta).ok()?;
    let mut h = [0u8; 32];
    f.read_exact(&mut h).ok()?;
    if &h[0..4] != MAGIA {
        return None;
    }
    let off = u64::from_be_bytes(h[8..16].try_into().ok()?);
    let largo = u32::from_be_bytes(h[16..20].try_into().ok()?) as usize;
    let bytes = u64::from_be_bytes(h[24..32].try_into().ok()?);
    let respaldo = if off == 0 || largo == 0 || largo > 4096 {
        None
    } else {
        let mut n = vec![0u8; largo];
        f.seek(SeekFrom::Start(off)).ok()?;
        f.read_exact(&mut n).ok()?;
        let p = PathBuf::from(String::from_utf8_lossy(&n).into_owned());
        Some(if p.is_absolute() { p } else { ruta.parent().unwrap_or(Path::new(".")).join(p) })
    };
    Some(Cabecera { bytes, respaldo })
}

/// ¿Es `disco` un overlay cuya base es `base`? (compara las rutas canonicas)
pub fn usa_base(disco: &Path, base: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).ok();
    cabecera(disco).and_then(|c| c.respaldo).and_then(|r| canon(&r)).is_some_and(|r| Some(r) == canon(base))
}

/// Bytes que ocupa de verdad un archivo en el disco del equipo (bloques asignados; un archivo disperso ocupa menos que su
/// tamano).
pub fn ocupado(ruta: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(ruta).map_or(0, |m| m.blocks() * 512)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rutas_relativas() {
        assert_eq!(relativa(Path::new("/d/machines/m/disks"), Path::new("/d/bases/img/24G.img")), PathBuf::from("../../../bases/img/24G.img"));
        assert_eq!(relativa(Path::new("/d"), Path::new("/d/b.img")), PathBuf::from("b.img"));
        assert_eq!(relativa(Path::new("/a/b"), Path::new("/c/d")), PathBuf::from("../../c/d"));
    }

    /// La cabecera de un overlay: campos, extension del formato de la base, contadores y L1, y su lectura.
    #[test]
    fn overlay_vacio() {
        let o = Opciones { bits_cluster: 16, l2_extendido: false, metadatos: false };
        // 64 KiB por cluster y 8192 entradas por L2: 512 MiB por entrada L1; 30 GiB = 60 entradas (un cluster)
        let (v, largo) = overlay("base.img", 30 << 30, o).unwrap();
        assert_eq!((v.len() as u64, largo), (4 << 16, 4 << 16));
        let be32 = |o: usize| u32::from_be_bytes(v[o..o + 4].try_into().unwrap());
        let be64 = |o: usize| u64::from_be_bytes(v[o..o + 8].try_into().unwrap());
        assert_eq!(&v[0..4], b"QFI\xfb");
        assert_eq!((be32(4), be32(20), be64(24), be32(36), be64(40), be64(48), be32(56)), (3, 16, 30 << 30, 60, 3 << 16, 1 << 16, 1));
        assert_eq!((be64(72), be32(96), be32(100)), (0, 4, 104));
        assert_eq!((be32(104), be32(108), &v[112..115]), (EXT_FORMATO_RESPALDO, 3, &b"raw"[..]));
        assert_eq!((be64(8), be32(16), &v[128..136]), (128, 8, &b"base.img"[..]));
        assert_eq!(be64(1 << 16), 2 << 16);
        let rc: Vec<u16> = (0..6).map(|i| u16::from_be_bytes([v[(2 << 16) + 2 * i], v[(2 << 16) + 2 * i + 1]])).collect();
        assert_eq!(rc, vec![1, 1, 1, 1, 0, 0]);
        // con L2 extendido (128 KiB): 16 bytes por entrada, 1 GiB por entrada L1
        let sin_meta = Opciones { metadatos: false, ..OPCIONES };
        let (v, _) = overlay("b", 30 << 30, sin_meta).unwrap();
        assert_eq!(u32::from_be_bytes(v[36..40].try_into().unwrap()), 30);
        assert_eq!(u64::from_be_bytes(v[72..80].try_into().unwrap()), L2_EXTENDIDO);
        // con metadatos: 245760 clusteres de datos en 30 tablas L2, 4 bloques de contadores; el archivo llega hasta el
        // ultimo cluster de datos
        let (v, largo) = overlay("b", 30 << 30, OPCIONES).unwrap();
        let be64 = |o: usize| u64::from_be_bytes(v[o..o + 8].try_into().unwrap());
        let c = 1u64 << 17;
        let (l1, rt) = (be64(40), be64(48));
        assert_eq!(rt, c);
        let l2_0 = be64(l1 as usize) & !(1 << 63);
        assert_eq!(v.len() as u64, l2_0 + 30 * c);
        assert_eq!(largo, v.len() as u64 + (30 << 30));
        assert_eq!(be64(l2_0 as usize), v.len() as u64 | 1 << 63);
        assert_eq!(be64(l2_0 as usize + 8), 0);
        assert_eq!(be64(l2_0 as usize + 16), (v.len() as u64 + c) | 1 << 63);
        assert!(overlay("b", 1 << 30, Opciones { l2_extendido: false, ..OPCIONES }).is_err());
        assert!(overlay("b", 1000, OPCIONES).is_err() && overlay("", 1 << 30, OPCIONES).is_err() && overlay("b", 1 << 30, Opciones { bits_cluster: 9, l2_extendido: false, metadatos: false }).is_err());
        assert!(overlay(&"x".repeat(2000), 1 << 30, OPCIONES).is_err());
    }

    #[test]
    fn cache_l2_cubre_el_disco() {
        // 30 GiB con clusteres de 128 KiB y entradas de 16 bytes: 3,75 MiB -> 4 MiB
        assert_eq!(OPCIONES.cache_l2(30 << 30), 4 << 20);
        assert_eq!(OPCIONES.cache_l2(1 << 20), 1 << 20);
    }

    /// Crear un overlay en disco y leerlo: la base se guarda relativa y se resuelve desde la carpeta del overlay.
    #[test]
    fn crear_y_leer() {
        let d = std::env::temp_dir().join(format!("weft-cow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("bases")).unwrap();
        std::fs::create_dir_all(d.join("m/disks")).unwrap();
        let base = d.join("bases/b.img");
        std::fs::File::create(&base).unwrap().set_len(64 << 20).unwrap();
        let ov = d.join("m/disks/x.img");
        crear(&base, &ov).unwrap();
        let c = cabecera(&ov).unwrap();
        assert_eq!(c.bytes, 64 << 20);
        assert_eq!(c.respaldo, Some(d.join("m/disks/../../bases/b.img")));
        assert!(usa_base(&ov, &base) && !usa_base(&base, &base) && !usa_base(&ov, &ov));
        assert_eq!(cabecera(&base), None);
        assert!(!d.join("m/disks/x.qcow2.parte").exists());
        // con qemu-img en el equipo (no hace falta para weft): QEMU lo da por bueno y ve la base y su formato
        if let Ok(o) = std::process::Command::new("qemu-img").args(["check", "-f", "qcow2"]).arg(&ov).output() {
            assert!(o.status.success(), "qemu-img check: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
            let o = std::process::Command::new("qemu-img").args(["info", "-f", "qcow2", "--output=json"]).arg(&ov).output().unwrap();
            let t = String::from_utf8_lossy(&o.stdout);
            assert!(t.contains("\"backing-filename-format\": \"raw\"") && t.contains("\"virtual-size\": 67108864") && t.contains("../../bases/b.img"), "{}", t);
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
