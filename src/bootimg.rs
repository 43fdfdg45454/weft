//! Imagenes de arranque de Android (boot.img, init_boot.img, vendor_boot.img), versiones de cabecera 3 y 4.
//! Permite arrancar el kernel directamente, sin cargador de arranque: se extraen kernel, discos RAM, linea de
//! comandos y configuracion de arranque.

use crate::textos::{tx, txf};
fn u32at(d: &[u8], o: usize) -> Result<u32, String> {
    d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(|| tx!("bootimg.imagen_de_arranque_truncada_2").to_string())
}

fn slice<'a>(d: &'a [u8], off: usize, len: usize, what: &str) -> Result<&'a [u8], String> {
    d.get(off..off.checked_add(len).ok_or("desbordamiento")?).ok_or_else(|| txf!("bootimg.imagen_de_arranque_truncada", what))
}

fn cstr(b: &[u8]) -> String {
    String::from_utf8_lossy(&b[..b.iter().position(|c| *c == 0).unwrap_or(b.len())]).trim().to_string()
}

fn pages(n: usize, page: usize) -> usize {
    n.div_ceil(page) * page
}

#[derive(Debug, PartialEq)]
pub struct Boot<'a> {
    pub version: u32,
    pub kernel: &'a [u8],
    pub ramdisk: &'a [u8],
    pub cmdline: String,
}

/// boot.img o init_boot.img (cabecera "ANDROID!", versiones 3 y 4: paginas de 4096 bytes).
pub fn parse_boot(d: &[u8]) -> Result<Boot<'_>, String> {
    if d.get(0..8) != Some(b"ANDROID!") {
        return Err(tx!("bootimg.no_es_una_imagen_de_arranque_de_android").into());
    }
    let version = u32at(d, 40)?;
    if version != 3 && version != 4 {
        return Err(txf!("bootimg.version_de_cabecera_no_soportada_solo_3", version));
    }
    const PAGE: usize = 4096;
    let (ksize, rsize) = (u32at(d, 8)? as usize, u32at(d, 12)? as usize);
    let cmdline = cstr(slice(d, 44, 1536, "cmdline")?);
    let koff = PAGE;
    let roff = koff + pages(ksize, PAGE);
    Ok(Boot { version, kernel: slice(d, koff, ksize, "kernel")?, ramdisk: slice(d, roff, rsize, "ramdisk")?, cmdline })
}

#[derive(Debug, PartialEq)]
pub struct VendorBoot<'a> {
    pub version: u32,
    pub ramdisk: &'a [u8],
    pub cmdline: String,
    pub bootconfig: String,
}

/// vendor_boot.img (cabecera "VNDRBOOT", versiones 3 y 4).
pub fn parse_vendor_boot(d: &[u8]) -> Result<VendorBoot<'_>, String> {
    if d.get(0..8) != Some(b"VNDRBOOT") {
        return Err(tx!("bootimg.no_es_una_imagen_vendor_boot_falta").into());
    }
    let version = u32at(d, 8)?;
    if version != 3 && version != 4 {
        return Err(txf!("bootimg.version_de_cabecera_no_soportada_solo_3", version));
    }
    let page = u32at(d, 12)? as usize;
    if page == 0 || page > 1 << 20 {
        return Err(tx!("bootimg.tamano_de_pagina_no_valido").into());
    }
    let rsize = u32at(d, 24)? as usize;
    let cmdline = cstr(slice(d, 28, 2048, "cmdline")?);
    // 2076 tags_addr, 2080 name[16], 2096 header_size, 2100 dtb_size, 2104 dtb_addr (u64)
    let header_size = u32at(d, 2096)? as usize;
    let dtb_size = u32at(d, 2100)? as usize;
    let roff = pages(header_size, page);
    let ramdisk = slice(d, roff, rsize, tx!("bootimg.vendor_ramdisk"))?;
    let mut bootconfig = String::new();
    if version == 4 {
        // 2112 vendor_ramdisk_table_size, 2116 entry_num, 2120 entry_size, 2124 bootconfig_size
        let table = u32at(d, 2112)? as usize;
        let bsize = u32at(d, 2124)? as usize;
        let boff = roff + pages(rsize, page) + pages(dtb_size, page) + pages(table, page);
        bootconfig = cstr(slice(d, boff, bsize, "bootconfig")?);
    }
    Ok(VendorBoot { version, ramdisk, cmdline, bootconfig })
}

/// Lo que hace un cargador de arranque con boot, init_boot y vendor_boot: kernel, discos RAM concatenados (primero el del
/// fabricante, despues el generico), linea de comandos (la de vendor_boot y la de boot) y bootconfig de vendor_boot.
#[derive(Debug, PartialEq)]
pub struct Desempaquetado {
    pub kernel: Vec<u8>,
    pub initrd: Vec<u8>,
    pub cmdline: String,
    pub bootconfig: String,
}

pub fn desempaquetar(boot: &[u8], init_boot: Option<&[u8]>, vendor_boot: Option<&[u8]>) -> Result<Desempaquetado, String> {
    let b = parse_boot(boot)?;
    if b.kernel.is_empty() {
        return Err(tx!("bootimg.boot_img_no_contiene_kernel").into());
    }
    let mut cmdline = b.cmdline.clone();
    let mut bootconfig = String::new();
    let mut initrd: Vec<u8> = Vec::new();
    if let Some(v) = vendor_boot {
        let vb = parse_vendor_boot(v)?;
        initrd.extend_from_slice(vb.ramdisk);
        cmdline = format!("{} {}", vb.cmdline, cmdline).trim().to_string();
        bootconfig = vb.bootconfig;
    }
    let mut generic = b.ramdisk.to_vec();
    if let Some(i) = init_boot {
        generic = parse_boot(i)?.ramdisk.to_vec();
    }
    initrd.extend_from_slice(&generic);
    Ok(Desempaquetado { kernel: b.kernel.to_vec(), initrd, cmdline, bootconfig })
}

/// Arquitectura del kernel de un boot.img: bzImage x86 de 64 bits (`HdrS` en 0x202 y XLF_KERNEL_64 en xloadflags), Image de
/// ARM64 (`ARM\x64` en 0x38) o desconocida (p. ej. comprimida).
pub fn arquitectura_kernel(kernel: &[u8]) -> Option<&'static str> {
    if kernel.get(0x202..0x206) == Some(b"HdrS") {
        let xl = kernel.get(0x236..0x238).map(|b| u16::from_le_bytes([b[0], b[1]])).unwrap_or(0);
        return Some(if xl & 1 == 1 { "x86_64" } else { "x86" });
    }
    if kernel.get(0x38..0x3c) == Some(b"ARM\x64") {
        return Some("arm64");
    }
    None
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn fake_boot(kernel: &[u8], ramdisk: &[u8], cmdline: &str) -> Vec<u8> {
        let mut d = vec![0u8; 4096];
        d[0..8].copy_from_slice(b"ANDROID!");
        d[8..12].copy_from_slice(&(kernel.len() as u32).to_le_bytes());
        d[12..16].copy_from_slice(&(ramdisk.len() as u32).to_le_bytes());
        d[40..44].copy_from_slice(&4u32.to_le_bytes());
        d[44..44 + cmdline.len()].copy_from_slice(cmdline.as_bytes());
        for part in [kernel, ramdisk] {
            d.extend_from_slice(part);
            d.resize(pages(d.len(), 4096), 0);
        }
        d
    }

    pub fn fake_vendor_boot(ramdisk: &[u8], cmdline: &str, bootconfig: &str) -> Vec<u8> {
        let page = 4096usize;
        let mut d = vec![0u8; 2128];
        d[0..8].copy_from_slice(b"VNDRBOOT");
        d[8..12].copy_from_slice(&4u32.to_le_bytes());
        d[12..16].copy_from_slice(&(page as u32).to_le_bytes());
        d[24..28].copy_from_slice(&(ramdisk.len() as u32).to_le_bytes());
        d[28..28 + cmdline.len()].copy_from_slice(cmdline.as_bytes());
        d[2096..2100].copy_from_slice(&2128u32.to_le_bytes());
        d[2100..2104].copy_from_slice(&5u32.to_le_bytes()); // dtb de 5 bytes
        d[2112..2116].copy_from_slice(&108u32.to_le_bytes()); // tabla de discos RAM
        d[2124..2128].copy_from_slice(&(bootconfig.len() as u32).to_le_bytes());
        d.resize(pages(d.len(), page), 0);
        for part in [ramdisk, b"DTB!!".as_slice(), &[7u8; 108], bootconfig.as_bytes()] {
            d.extend_from_slice(part);
            d.resize(pages(d.len(), page), 0);
        }
        d
    }

    #[test]
    fn boot_y_vendor_boot() {
        let k = vec![0xAAu8; 5000];
        let img = fake_boot(&k, b"RAMDISK", "console=x");
        let b = parse_boot(&img).unwrap();
        assert_eq!((b.version, b.kernel, b.ramdisk, b.cmdline.as_str()), (4, k.as_slice(), b"RAMDISK".as_slice(), "console=x"));
        let init = fake_boot(b"", b"GENERIC", "");
        assert_eq!(parse_boot(&init).unwrap().ramdisk, b"GENERIC");
        assert!(parse_boot(&img[..6000]).is_err());
        assert!(parse_boot(b"nada").is_err());

        let v = fake_vendor_boot(&vec![3u8; 9000], "a=1 b=2", "androidboot.hardware=cutf_cvm\nx.y=1\n");
        let p = parse_vendor_boot(&v).unwrap();
        assert_eq!(p.ramdisk, vec![3u8; 9000].as_slice());
        assert_eq!(p.cmdline, "a=1 b=2");
        assert_eq!(p.bootconfig, "androidboot.hardware=cutf_cvm\nx.y=1");
        assert!(parse_vendor_boot(&img).is_err());
    }
}
