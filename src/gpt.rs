//! Ensamblado de un disco con tabla de particiones GPT a partir de archivos sueltos (una particion por archivo).
//! Android localiza sus particiones por el nombre GPT, asi que la imagen descargada (super.img, userdata.img,
//! vbmeta*.img...) hay que presentarla como un unico disco.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use crate::textos::{tx, txf};

pub const SECTOR: u64 = 512;
const ALIGN: u64 = 1 << 20;
const ENTRIES: u64 = 128;
const ENTRY_SIZE: u64 = 128;
/// GUID de tipo "datos de Linux" (0FC63DAF-8483-4772-8E79-3D69D8477DE4) en el orden de bytes de GPT.
const TYPE_LINUX: [u8; 16] = [0xAF, 0x3D, 0xC6, 0x0F, 0x83, 0x84, 0x72, 0x47, 0x8E, 0x79, 0x3D, 0x69, 0xD8, 0x47, 0x7D, 0xE4];

pub fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for b in data {
        c ^= *b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}

/// Identificador estable derivado de un texto (no hace falta azar: basta con que sean distintos y repetibles).
fn guid(seed: &str) -> [u8; 16] {
    let mut g = [0u8; 16];
    for (i, salt) in ["a", "b", "c", "d"].iter().enumerate() {
        g[i * 4..i * 4 + 4].copy_from_slice(&crc32(format!("{}:{}", salt, seed).as_bytes()).to_le_bytes());
    }
    g[7] = (g[7] & 0x0F) | 0x40;
    g[8] = (g[8] & 0x3F) | 0x80;
    g
}

#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub name: String,
    /// archivo con el contenido (None = particion vacia)
    pub source: Option<String>,
    pub bytes: u64,
}

#[derive(Debug, PartialEq)]
pub struct Layout {
    /// (primer sector, ultimo sector) de cada particion
    pub ranges: Vec<(u64, u64)>,
    pub total_sectors: u64,
}

/// `n` redondeado hacia arriba a un multiplo de `a`; None si no cabe en 64 bits.
fn up(n: u64, a: u64) -> Option<u64> {
    n.div_ceil(a).checked_mul(a)
}

pub fn layout(parts: &[Part]) -> Result<Layout, String> {
    if parts.is_empty() || parts.len() as u64 > ENTRIES {
        return Err(tx!("gpt.el_disco_necesita_entre_1_y_128").into());
    }
    let mut off = ALIGN;
    let mut ranges = Vec::new();
    for p in parts {
        if p.bytes == 0 {
            return Err(txf!("gpt.la_particion_esta_vacia", p.name));
        }
        if p.name.is_empty() || p.name.encode_utf16().count() > 36 {
            return Err(txf!("gpt.nombre_de_particion_no_valido", format!("{:?}", p.name)));
        }
        let grande = || txf!("gpt.la_particion_es_demasiado_grande_bytes", p.name, p.bytes);
        let fin = up(p.bytes, SECTOR).and_then(|size| off.checked_add(size)).ok_or_else(grande)?;
        ranges.push((off / SECTOR, fin / SECTOR - 1));
        off = up(fin, ALIGN).ok_or_else(grande)?;
    }
    // copia de respaldo al final: 32 sectores de entradas + 1 de cabecera
    let total = off / SECTOR + ENTRIES * ENTRY_SIZE / SECTOR + 1;
    // el disco se mide en bytes (tamano del archivo, desplazamientos): tiene que caber en 64 bits
    total.checked_mul(SECTOR).ok_or(tx!("gpt.las_particiones_no_caben_en_un_disco"))?;
    Ok(Layout { ranges, total_sectors: total })
}

fn entries(parts: &[Part], l: &Layout) -> Vec<u8> {
    let mut e = vec![0u8; (ENTRIES * ENTRY_SIZE) as usize];
    for (i, (p, (first, last))) in parts.iter().zip(&l.ranges).enumerate() {
        let o = i * ENTRY_SIZE as usize;
        e[o..o + 16].copy_from_slice(&TYPE_LINUX);
        e[o + 16..o + 32].copy_from_slice(&guid(&format!("part:{}:{}", i, p.name)));
        e[o + 32..o + 40].copy_from_slice(&first.to_le_bytes());
        e[o + 40..o + 48].copy_from_slice(&last.to_le_bytes());
        for (k, u) in p.name.encode_utf16().enumerate() {
            e[o + 56 + 2 * k..o + 58 + 2 * k].copy_from_slice(&u.to_le_bytes());
        }
    }
    e
}

fn header(l: &Layout, entries_crc: u32, primary: bool) -> Vec<u8> {
    let last = l.total_sectors - 1;
    let ent_sectors = ENTRIES * ENTRY_SIZE / SECTOR;
    let (my, other, ent_lba) = if primary { (1, last, 2) } else { (last, 1, last - ent_sectors) };
    let mut h = vec![0u8; SECTOR as usize];
    h[0..8].copy_from_slice(b"EFI PART");
    h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
    h[12..16].copy_from_slice(&92u32.to_le_bytes());
    h[24..32].copy_from_slice(&my.to_le_bytes());
    h[32..40].copy_from_slice(&other.to_le_bytes());
    h[40..48].copy_from_slice(&(ALIGN / SECTOR).to_le_bytes()); // primer sector utilizable
    h[48..56].copy_from_slice(&(last - ent_sectors - 1).to_le_bytes()); // ultimo sector utilizable
    h[56..72].copy_from_slice(&guid("disco weft"));
    h[72..80].copy_from_slice(&ent_lba.to_le_bytes());
    h[80..84].copy_from_slice(&(ENTRIES as u32).to_le_bytes());
    h[84..88].copy_from_slice(&(ENTRY_SIZE as u32).to_le_bytes());
    h[88..92].copy_from_slice(&entries_crc.to_le_bytes());
    let c = crc32(&h[0..92]);
    h[16..20].copy_from_slice(&c.to_le_bytes());
    h
}

fn protective_mbr(total_sectors: u64) -> Vec<u8> {
    let mut m = vec![0u8; SECTOR as usize];
    m[446 + 1..446 + 4].copy_from_slice(&[0x00, 0x02, 0x00]);
    m[446 + 4] = 0xEE;
    m[446 + 5..446 + 8].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
    m[446 + 8..446 + 12].copy_from_slice(&1u32.to_le_bytes());
    m[446 + 12..446 + 16].copy_from_slice(&((total_sectors - 1).min(0xFFFF_FFFF) as u32).to_le_bytes());
    m[510] = 0x55;
    m[511] = 0xAA;
    m
}

/// Copia `src` en `dst` a partir de `offset`, sin escribir los bloques que son todo ceros (el destino ya es
/// un archivo disperso del tamano final). Devuelve los bytes realmente escritos.
fn sparse_copy(src: &mut File, dst: &mut File, offset: u64, len: u64) -> std::io::Result<u64> {
    const CHUNK: usize = 1 << 20;
    let mut buf = vec![0u8; CHUNK];
    let (mut done, mut written) = (0u64, 0u64);
    src.seek(SeekFrom::Start(0))?;
    while done < len {
        let want = CHUNK.min((len - done) as usize);
        src.read_exact(&mut buf[..want])?;
        if buf[..want].iter().any(|b| *b != 0) {
            dst.seek(SeekFrom::Start(offset + done))?;
            dst.write_all(&buf[..want])?;
            written += want as u64;
        }
        done += want as u64;
    }
    Ok(written)
}

const SIMG_MAGIC: [u8; 4] = [0x3A, 0xFF, 0x26, 0xED];

/// (tamano de cabecera, tamano de cabecera de trozo, tamano de bloque, bloques totales, trozos) de una imagen dispersa.
type CabeceraSimg = (u64, u64, u64, u64, u32);

/// Cabecera de una imagen dispersa de Android (formato "simg", el que usa fastboot), o None si el archivo no lo es.
fn simg_header(f: &mut File) -> std::io::Result<Option<CabeceraSimg>> {
    let mut h = [0u8; 28];
    f.seek(SeekFrom::Start(0))?;
    if f.read_exact(&mut h).is_err() || h[0..4] != SIMG_MAGIC {
        return Ok(None);
    }
    let u16at = |i: usize| u16::from_le_bytes([h[i], h[i + 1]]) as u64;
    let u32at = |i: usize| u32::from_le_bytes([h[i], h[i + 1], h[i + 2], h[i + 3]]);
    let (major, hdr, chunk_hdr, blk, blocks, chunks) = (u16at(4), u16at(8), u16at(10), u32at(12) as u64, u32at(16) as u64, u32at(20));
    if major != 1 || hdr < 28 || chunk_hdr < 12 || blk == 0 || blk % 4 != 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, tx!("gpt.cabecera_de_imagen_dispersa_no_valida")));
    }
    Ok(Some((hdr, chunk_hdr, blk, blocks, chunks)))
}

/// Tamano ya expandido de una imagen dispersa de Android, o None si el archivo no lo es.
pub fn simg_size(path: &str) -> Result<Option<u64>, String> {
    let mut f = File::open(path).map_err(|e| format!("{}: {}", path, e))?;
    Ok(simg_header(&mut f).map_err(|e| format!("{}: {}", path, e))?.map(|(_, _, blk, blocks, _)| blk * blocks))
}

/// Expande una imagen dispersa de Android dentro de `dst` a partir de `offset`. Los trozos "sin datos" y los
/// rellenos con ceros no se escriben (el destino ya es un archivo disperso). Devuelve los bytes escritos.
fn simg_copy(src: &mut File, dst: &mut File, offset: u64, len: u64) -> std::io::Result<u64> {
    let bad = |m: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, m.to_string());
    let (hdr, chunk_hdr, blk, _, chunks) = simg_header(src)?.ok_or_else(|| bad(tx!("gpt.no_es_una_imagen_dispersa")))?;
    src.seek(SeekFrom::Start(hdr))?;
    const CHUNK: usize = 1 << 20;
    let mut buf = vec![0u8; CHUNK];
    let (mut pos, mut written) = (0u64, 0u64);
    for _ in 0..chunks {
        let mut ch = [0u8; 12];
        src.read_exact(&mut ch)?;
        let kind = u16::from_le_bytes([ch[0], ch[1]]);
        let out_bytes = u32::from_le_bytes([ch[4], ch[5], ch[6], ch[7]]) as u64 * blk;
        let data = (u32::from_le_bytes([ch[8], ch[9], ch[10], ch[11]]) as u64).checked_sub(chunk_hdr).ok_or_else(|| bad(tx!("gpt.trozo_mas_corto_que_su_cabecera")))?;
        if chunk_hdr > 12 {
            src.seek(SeekFrom::Current((chunk_hdr - 12) as i64))?;
        }
        if kind != 0xCAC4 && pos + out_bytes > len {
            return Err(bad(tx!("gpt.la_imagen_dispersa_es_mas_grande_de_lo")));
        }
        match kind {
            // datos tal cual
            0xCAC1 => {
                if data != out_bytes {
                    return Err(bad(tx!("gpt.trozo_de_datos_con_tamano_incoherente")));
                }
                let mut done = 0u64;
                while done < out_bytes {
                    let want = CHUNK.min((out_bytes - done) as usize);
                    src.read_exact(&mut buf[..want])?;
                    if buf[..want].iter().any(|b| *b != 0) {
                        dst.seek(SeekFrom::Start(offset + pos + done))?;
                        dst.write_all(&buf[..want])?;
                        written += want as u64;
                    }
                    done += want as u64;
                }
            }
            // relleno: un patron de 4 bytes repetido
            0xCAC2 => {
                let mut pat = [0u8; 4];
                src.read_exact(&mut pat)?;
                if data > 4 {
                    src.seek(SeekFrom::Current((data - 4) as i64))?;
                }
                if pat != [0, 0, 0, 0] {
                    for c in buf.chunks_mut(4) {
                        c.copy_from_slice(&pat);
                    }
                    dst.seek(SeekFrom::Start(offset + pos))?;
                    let mut done = 0u64;
                    while done < out_bytes {
                        let want = CHUNK.min((out_bytes - done) as usize);
                        dst.write_all(&buf[..want])?;
                        done += want as u64;
                    }
                    written += out_bytes;
                }
            }
            // sin datos, o suma de comprobacion (no ocupa espacio en la salida)
            0xCAC3 | 0xCAC4 => {
                src.seek(SeekFrom::Current(data as i64))?;
            }
            _ => return Err(bad(tx!("gpt.tipo_de_trozo_desconocido"))),
        }
        if kind != 0xCAC4 {
            pos += out_bytes;
        }
    }
    Ok(written)
}

/// Escribe el disco completo. Devuelve (tamano total, bytes de datos escritos).
pub fn assemble(out: &str, parts: &[Part]) -> Result<(u64, u64), String> {
    let l = layout(parts)?;
    let total = l.total_sectors * SECTOR;
    let mut f = File::create(out).map_err(|e| format!("{}: {}", out, e))?;
    f.set_len(total).map_err(|e| format!("{}: {}", out, e))?;
    let ent = entries(parts, &l);
    let crc = crc32(&ent);
    let io = |e: std::io::Error| format!("{}: {}", out, e);
    f.write_all(&protective_mbr(l.total_sectors)).map_err(io)?;
    f.write_all(&header(&l, crc, true)).map_err(io)?;
    f.write_all(&ent).map_err(io)?;
    let mut written = 0u64;
    for (p, (first, _)) in parts.iter().zip(&l.ranges) {
        if let Some(src) = &p.source {
            let mut s = File::open(src).map_err(|e| format!("{}: {}", src, e))?;
            // las imagenes dispersas de Android (super.img tal como se publica) se expanden al copiarlas
            if simg_header(&mut s).map_err(|e| format!("{}: {}", src, e))?.is_some() {
                written += simg_copy(&mut s, &mut f, first * SECTOR, p.bytes).map_err(|e| format!("{}: {}", src, e))?;
                continue;
            }
            written += sparse_copy(&mut s, &mut f, first * SECTOR, p.bytes).map_err(|e| format!("{}: {}", src, e))?;
        }
    }
    f.seek(SeekFrom::Start(total - SECTOR - ent.len() as u64)).map_err(io)?;
    f.write_all(&ent).map_err(io)?;
    f.write_all(&header(&l, crc, false)).map_err(io)?;
    Ok((total, written))
}

/// "nombre=archivo" o "nombre=TAMANO" (64M, 1G, 4096K o bytes) -> particion.
pub fn parse_part(spec: &str) -> Result<Part, String> {
    let (name, v) = spec.split_once('=').ok_or(txf!("gpt.particion_se_espera_nombre_archivo_o", format!("{:?}", spec)))?;
    let size = |t: &str| -> Option<Result<u64, String>> {
        let (num, mul) = match t.chars().last()? {
            'K' => (&t[..t.len() - 1], 1u64 << 10),
            'M' => (&t[..t.len() - 1], 1 << 20),
            'G' => (&t[..t.len() - 1], 1 << 30),
            _ => (t, 1),
        };
        // un numero que se pasa de 64 bits al multiplicar es un tamano, no un archivo: error propio
        let n = num.parse::<u64>().ok()?;
        Some(n.checked_mul(mul).ok_or(txf!("gpt.particion_tamano_demasiado_grande", format!("{:?}", spec))))
    };
    if let Some(bytes) = size(v) {
        return Ok(Part { name: name.to_string(), source: None, bytes: bytes? });
    }
    let bytes = match simg_size(v)? {
        Some(expanded) => expanded,
        None => std::fs::metadata(v).map_err(|e| format!("{}: {}", v, e))?.len(),
    };
    Ok(Part { name: name.to_string(), source: Some(v.to_string()), bytes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suma_de_comprobacion() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn disco_completo() {
        let dir = std::env::temp_dir().join(format!("weft-gpt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = |n: &str| dir.join(n).to_str().unwrap().to_string();
        let mut big = vec![0u8; 3 << 20];
        big[(2 << 20) + 5] = 0x77; // datos solo en el tercer megabyte
        std::fs::write(p("super.img"), &big).unwrap();
        std::fs::write(p("vbmeta.img"), b"AVB0contenido").unwrap();
        let parts = vec![
            parse_part("misc=1M").unwrap(),
            parse_part(&format!("vbmeta_a={}", p("vbmeta.img"))).unwrap(),
            parse_part(&format!("super={}", p("super.img"))).unwrap(),
        ];
        assert_eq!(parts[0], Part { name: "misc".into(), source: None, bytes: 1 << 20 });
        let l = layout(&parts).unwrap();
        assert_eq!(l.ranges, vec![(2048, 4095), (4096, 4096), (6144, 6144 + 6143)]);
        let (total, written) = assemble(&p("disco.img"), &parts).unwrap();
        assert_eq!(total, l.total_sectors * 512);
        assert_eq!(written, 13 + (1 << 20), "solo se escriben los bloques con datos");

        let d = std::fs::read(p("disco.img")).unwrap();
        assert_eq!(&d[510..512], &[0x55, 0xAA]);
        assert_eq!(d[446 + 4], 0xEE);
        // cabecera primaria: firma, suma propia y suma de las entradas
        let h = &d[512..1024];
        assert_eq!(&h[0..8], b"EFI PART");
        let mut hc = h[0..92].to_vec();
        hc[16..20].fill(0);
        assert_eq!(u32::from_le_bytes(h[16..20].try_into().unwrap()), crc32(&hc));
        let ent = &d[1024..1024 + 128 * 128];
        assert_eq!(u32::from_le_bytes(h[88..92].try_into().unwrap()), crc32(ent));
        // nombre de la tercera particion en UTF-16 y su contenido en el sitio correcto
        let name: Vec<u16> = ent[2 * 128 + 56..2 * 128 + 56 + 10].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(String::from_utf16(&name).unwrap(), "super");
        assert_eq!(&d[4096 * 512..4096 * 512 + 4], b"AVB0");
        assert_eq!(d[6144 * 512 + (2 << 20) + 5], 0x77);
        // copia de respaldo al final, apuntando a la primaria
        let b = &d[d.len() - 512..];
        assert_eq!(&b[0..8], b"EFI PART");
        assert_eq!(u64::from_le_bytes(b[32..40].try_into().unwrap()), 1);
        assert_eq!(&d[d.len() - 512 - 128 * 128..d.len() - 512], ent);

        assert!(parse_part("sin-igual").is_err());
        assert!(layout(&[]).is_err());
        // imagen dispersa de Android: 4 bloques de 4096 -> datos, relleno, hueco, datos
        let mut sp: Vec<u8> = Vec::new();
        sp.extend(SIMG_MAGIC);
        for v in [1u16, 0, 28, 12] {
            sp.extend(v.to_le_bytes());
        }
        for v in [4096u32, 4, 5, 0] {
            sp.extend(v.to_le_bytes());
        }
        let chunk = |sp: &mut Vec<u8>, kind: u16, blocks: u32, data: &[u8]| {
            sp.extend(kind.to_le_bytes());
            sp.extend(0u16.to_le_bytes());
            sp.extend(blocks.to_le_bytes());
            sp.extend((12 + data.len() as u32).to_le_bytes());
            sp.extend(data);
        };
        chunk(&mut sp, 0xCAC1, 1, &[0x11u8; 4096]);
        chunk(&mut sp, 0xCAC2, 1, &[0xAA, 0xBB, 0xCC, 0xDD]);
        chunk(&mut sp, 0xCAC3, 1, &[]);
        chunk(&mut sp, 0xCAC4, 0, &[0, 0, 0, 0]);
        chunk(&mut sp, 0xCAC1, 1, &[0x22u8; 4096]);
        std::fs::write(p("simg.img"), &sp).unwrap();
        assert_eq!(simg_size(&p("simg.img")).unwrap(), Some(16384));
        assert_eq!(simg_size(&p("vbmeta.img")).unwrap(), None);
        let sparts = [parse_part("misc=1M").unwrap(), parse_part(&format!("super={}", p("simg.img"))).unwrap()];
        assert_eq!(sparts[1].bytes, 16384);
        let sl = layout(&sparts).unwrap();
        assemble(&p("d2.img"), &sparts).unwrap();
        let d2 = std::fs::read(p("d2.img")).unwrap();
        let o = (sl.ranges[1].0 * 512) as usize;
        assert!(d2[o..o + 4096].iter().all(|b| *b == 0x11));
        assert_eq!(&d2[o + 4096..o + 4104], &[0xAA, 0xBB, 0xCC, 0xDD, 0xAA, 0xBB, 0xCC, 0xDD]);
        assert_eq!(&d2[o + 8188..o + 8192], &[0xAA, 0xBB, 0xCC, 0xDD]);
        assert!(d2[o + 8192..o + 12288].iter().all(|b| *b == 0));
        assert!(d2[o + 12288..o + 16384].iter().all(|b| *b == 0x22));
        // truncada: error, no un disco a medias dado por bueno
        std::fs::write(p("simg-rota.img"), &sp[..sp.len() - 100]).unwrap();
        assert!(assemble(&p("d3.img"), &[parse_part(&format!("x={}", p("simg-rota.img"))).unwrap()]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Tamanos en el limite de 64 bits: error claro en vez de desbordar (que en una compilacion optimizada daria la
    /// vuelta en silencio y armaria una tabla sin sentido).
    #[test]
    fn tamanos_en_el_limite() {
        let parte = |bytes: u64| Part { name: "x".into(), source: None, bytes };
        // 2^34 G = 2^64 bytes: no cabe
        let e = parse_part("x=17179869184G").unwrap_err();
        assert!(e.contains("demasiado grande"), "{}", e);
        assert_eq!(parse_part("x=17179869183G").unwrap().bytes, (17179869183u64) << 30);
        assert!(parse_part("x=18014398509481984K").is_err());
        assert_eq!(parse_part("x=18446744073709551615").unwrap().bytes, u64::MAX);
        // redondear a sectores, sumar el desplazamiento o alinear a 1 MiB se pasarian de 64 bits
        for bytes in [u64::MAX, u64::MAX - 511, u64::MAX - (1 << 20), (u64::MAX - (1 << 20)) / 512 * 512 - 512] {
            let e = layout(&[parte(bytes)]).unwrap_err();
            assert!(e.contains("demasiado grande"), "{}: {}", bytes, e);
        }
        // cada una cabe, pero juntas no
        assert!(layout(&[parte(1 << 63), parte(1 << 63)]).is_err());
        // en el limite justo todavia es una tabla valida
        let l = layout(&[parte((1u64 << 63) - (1 << 21))]).unwrap();
        assert_eq!(l.ranges, vec![(2048, 2048 + ((1u64 << 63) - (1 << 21)) / 512 - 1)]);
        assert!(l.total_sectors.checked_mul(SECTOR).is_some());
        // un disco normal grande no cambia
        assert_eq!(layout(&[parte(1 << 40)]).unwrap().ranges, vec![(2048, 2048 + (1u64 << 31) - 1)]);
    }
}
