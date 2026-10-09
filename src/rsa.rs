//! RSA minimo para la autenticacion de adb (sin crates): enteros grandes sin signo, generacion de una clave de 2048 bits
//! (primos con criba de primos pequenos y Miller-Rabin), firma PKCS#1 v1.5 y los formatos de la clave: PEM PKCS#1 para
//! guardarla (`-----BEGIN RSA PRIVATE KEY-----`, el mismo formato que el `adbkey` de Google, que se puede usar con las dos
//! herramientas) y el formato propio de Android para la clave publica (estructura `RSAPublicKey` de mincrypt en base64).
//!
//! Lo que firma adb: adbd manda un testigo de 20 bytes (AUTH TOKEN) y el cliente lo firma como si fuera un resumen SHA-1:
//! EM = 00 01 FF..FF 00 || DigestInfo(SHA-1) || testigo, firma = EM^d mod n (RSA_sign(NID_sha1, ...) en adb).
//!
//! No es una biblioteca criptografica general: no hay tiempo constante (la clave vive en el equipo del usuario y solo firma
//! testigos de adbd) ni otros relleno o tamanos.

use std::cmp::Ordering;
use crate::textos::{tx, txf};

// ---------------------------------------------------------------------------------------------------------------
// enteros grandes

/// Entero sin signo de precision arbitraria: palabras de 32 bits, la menos significativa primero, sin ceros por arriba.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Big(Vec<u32>);

impl Big {
    fn norm(mut v: Vec<u32>) -> Big {
        while v.last() == Some(&0) {
            v.pop();
        }
        Big(v)
    }
    pub fn zero() -> Big {
        Big(Vec::new())
    }
    pub fn from_u64(x: u64) -> Big {
        Big::norm(vec![x as u32, (x >> 32) as u32])
    }
    pub fn is_zero(&self) -> bool {
        self.0.is_empty()
    }
    /// Numero de bits (0 para el cero).
    pub fn bits(&self) -> usize {
        self.0.last().map_or(0, |w| self.0.len() * 32 - w.leading_zeros() as usize)
    }
    pub fn bit(&self, i: usize) -> bool {
        self.0.get(i / 32).is_some_and(|w| (w >> (i % 32)) & 1 == 1)
    }
    /// Desde bytes en orden de red (el mas significativo primero).
    pub fn from_be(b: &[u8]) -> Big {
        let mut v = Vec::with_capacity(b.len().div_ceil(4));
        for c in b.rchunks(4) {
            let mut w = 0u32;
            for x in c {
                w = (w << 8) | *x as u32;
            }
            v.push(w);
        }
        Big::norm(v)
    }
    /// A bytes en orden de red con `len` bytes exactos (ceros a la izquierda). None si no cabe.
    pub fn to_be(&self, len: usize) -> Option<Vec<u8>> {
        if self.bits() > len * 8 {
            return None;
        }
        let mut out = vec![0u8; len];
        for (i, w) in self.0.iter().enumerate() {
            for k in 0..4 {
                let pos = i * 4 + k;
                if pos < len {
                    out[len - 1 - pos] = (w >> (8 * k)) as u8;
                }
            }
        }
        Some(out)
    }
    /// Bytes minimos en orden de red (el cero es un byte 0).
    pub fn to_be_min(&self) -> Vec<u8> {
        self.to_be(self.bits().div_ceil(8).max(1)).unwrap()
    }
    /// Palabras de 32 bits, la menos significativa primero, rellenas hasta `n`.
    pub fn words(&self, n: usize) -> Vec<u32> {
        let mut v = self.0.clone();
        v.resize(n.max(v.len()), 0);
        v
    }
    pub fn add(&self, o: &Big) -> Big {
        let (a, b) = if self.0.len() >= o.0.len() { (&self.0, &o.0) } else { (&o.0, &self.0) };
        let mut r = Vec::with_capacity(a.len() + 1);
        let mut c = 0u64;
        for (i, x) in a.iter().enumerate() {
            let t = *x as u64 + *b.get(i).unwrap_or(&0) as u64 + c;
            r.push(t as u32);
            c = t >> 32;
        }
        r.push(c as u32);
        Big::norm(r)
    }
    /// self - o; exige self >= o.
    pub fn sub(&self, o: &Big) -> Big {
        assert!(*self >= *o, "resta de enteros sin signo negativa");
        let mut r = Vec::with_capacity(self.0.len());
        let mut borrow = 0i64;
        for i in 0..self.0.len() {
            let mut t = self.0[i] as i64 - *o.0.get(i).unwrap_or(&0) as i64 - borrow;
            borrow = 0;
            if t < 0 {
                t += 1 << 32;
                borrow = 1;
            }
            r.push(t as u32);
        }
        Big::norm(r)
    }
    pub fn mul(&self, o: &Big) -> Big {
        if self.is_zero() || o.is_zero() {
            return Big::zero();
        }
        let mut r = vec![0u32; self.0.len() + o.0.len()];
        for (i, a) in self.0.iter().enumerate() {
            let mut c = 0u64;
            for (j, b) in o.0.iter().enumerate() {
                let t = *a as u64 * *b as u64 + r[i + j] as u64 + c;
                r[i + j] = t as u32;
                c = t >> 32;
            }
            r[i + o.0.len()] = c as u32;
        }
        Big::norm(r)
    }
    pub fn shl(&self, n: usize) -> Big {
        if self.is_zero() {
            return Big::zero();
        }
        let (w, b) = (n / 32, n % 32);
        let mut r = vec![0u32; w];
        let mut c = 0u32;
        for x in &self.0 {
            r.push(if b == 0 { *x } else { (x << b) | c });
            c = if b == 0 { 0 } else { x >> (32 - b) };
        }
        r.push(c);
        Big::norm(r)
    }
    pub fn shr(&self, n: usize) -> Big {
        let (w, b) = (n / 32, n % 32);
        if w >= self.0.len() {
            return Big::zero();
        }
        let s = &self.0[w..];
        let r = (0..s.len()).map(|i| if b == 0 { s[i] } else { (s[i] >> b) | s.get(i + 1).map_or(0, |n| n << (32 - b)) }).collect();
        Big::norm(r)
    }
    /// Resto de dividir por una palabra.
    pub fn rem_u32(&self, d: u32) -> u32 {
        let mut r = 0u64;
        for w in self.0.iter().rev() {
            r = ((r << 32) | *w as u64) % d as u64;
        }
        r as u32
    }
    /// (cociente, resto). Algoritmo D de Knuth (con la forma de "Hacker's Delight", divmnu).
    pub fn divrem(&self, v: &Big) -> (Big, Big) {
        assert!(!v.is_zero(), "division por cero");
        if self < v {
            return (Big::zero(), self.clone());
        }
        let n = v.0.len();
        if n == 1 {
            let d = v.0[0] as u64;
            let mut q = vec![0u32; self.0.len()];
            let mut r = 0u64;
            for i in (0..self.0.len()).rev() {
                let t = (r << 32) | self.0[i] as u64;
                q[i] = (t / d) as u32;
                r = t % d;
            }
            return (Big::norm(q), Big::from_u64(r));
        }
        let m = self.0.len() - n;
        let s = v.0[n - 1].leading_zeros();
        let vn = v.shl(s as usize).0;
        let mut un = self.shl(s as usize).0;
        un.resize(self.0.len() + 1, 0);
        let mut q = vec![0u32; m + 1];
        const B: u64 = 1 << 32;
        for j in (0..=m).rev() {
            let num = ((un[j + n] as u64) << 32) | un[j + n - 1] as u64;
            let mut qhat = num / vn[n - 1] as u64;
            let mut rhat = num % vn[n - 1] as u64;
            while qhat >= B || qhat * vn[n - 2] as u64 > ((rhat << 32) | un[j + n - 2] as u64) {
                qhat -= 1;
                rhat += vn[n - 1] as u64;
                if rhat >= B {
                    break;
                }
            }
            // multiplicar y restar
            let mut k: i64 = 0;
            for i in 0..n {
                let p = qhat * vn[i] as u64;
                let t = un[i + j] as i64 - k - (p & 0xFFFF_FFFF) as i64;
                un[i + j] = t as u32;
                k = (p >> 32) as i64 - (t >> 32);
            }
            let t = un[j + n] as i64 - k;
            un[j + n] = t as u32;
            q[j] = qhat as u32;
            if t < 0 {
                // se resto de mas: se suma una vez el divisor
                q[j] = q[j].wrapping_sub(1);
                let mut c = 0u64;
                for i in 0..n {
                    let t = un[i + j] as u64 + vn[i] as u64 + c;
                    un[i + j] = t as u32;
                    c = t >> 32;
                }
                un[j + n] = un[j + n].wrapping_add(c as u32);
            }
        }
        un.truncate(n);
        (Big::norm(q), Big::norm(un).shr(s as usize))
    }
    pub fn rem(&self, m: &Big) -> Big {
        self.divrem(m).1
    }
    /// self^e mod m (de izquierda a derecha: cuadrado y producto).
    pub fn modpow(&self, e: &Big, m: &Big) -> Big {
        let mut r = Big::from_u64(1).rem(m);
        let b = self.rem(m);
        for i in (0..e.bits()).rev() {
            r = r.mul(&r).rem(m);
            if e.bit(i) {
                r = r.mul(&b).rem(m);
            }
        }
        r
    }
}

impl PartialOrd for Big {
    fn partial_cmp(&self, o: &Big) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Big {
    fn cmp(&self, o: &Big) -> Ordering {
        self.0.len().cmp(&o.0.len()).then_with(|| self.0.iter().rev().cmp(o.0.iter().rev()))
    }
}

// ---------------------------------------------------------------------------------------------------------------
// azar y primos

/// Bytes al azar de /dev/urandom.
pub fn azar(n: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut b = vec![0u8; n];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).map_err(|e| format!("/dev/urandom: {}", e))?;
    Ok(b)
}

/// Primos impares pequenos para la criba previa (hasta 2000; los candidatos ya son impares).
fn primos_pequenos() -> Vec<u32> {
    let mut v = Vec::new();
    for n in (3u32..2000).step_by(2) {
        if v.iter().take_while(|p| *p * *p <= n).all(|p| n % p != 0) {
            v.push(n);
        }
    }
    v
}

/// Rondas de Miller-Rabin con bases al azar: con 40 la probabilidad de aceptar un compuesto es menor que 2^-80 (mucho
/// menor en la practica para numeros al azar de 1024 bits).
pub const RONDAS_MR: usize = 40;

/// ¿Es `n` (impar, mayor que 3) probablemente primo? Miller-Rabin con `rondas` bases al azar en [2, n-2].
pub fn miller_rabin(n: &Big, rondas: usize, azar: &mut dyn FnMut(usize) -> Result<Vec<u8>, String>) -> Result<bool, String> {
    let uno = Big::from_u64(1);
    let n1 = n.sub(&uno);
    let s = (0..n1.bits()).take_while(|i| !n1.bit(*i)).count();
    let d = n1.shr(s);
    let n3 = n.sub(&Big::from_u64(3));
    for _ in 0..rondas {
        // a = 2 + (azar mod (n-3)), en [2, n-2]
        let a = Big::from_be(&azar(n.bits().div_ceil(8) + 8)?).rem(&n3).add(&Big::from_u64(2));
        let mut x = a.modpow(&d, n);
        if x == uno || x == n1 {
            continue;
        }
        let mut compuesto = true;
        for _ in 1..s {
            x = x.mul(&x).rem(n);
            if x == n1 {
                compuesto = false;
                break;
            }
        }
        if compuesto {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Primo al azar de `bits` bits con los dos bits altos puestos (el producto de dos tiene exactamente 2*bits bits) y con
/// p-1 primo con `e`.
fn primo(bits: usize, e: u32, azar: &mut dyn FnMut(usize) -> Result<Vec<u8>, String>) -> Result<Big, String> {
    let pequenos = primos_pequenos();
    loop {
        let mut b = azar(bits / 8)?;
        b[0] |= 0xC0;
        let last = b.len() - 1;
        b[last] |= 1;
        let c = Big::from_be(&b);
        if pequenos.iter().any(|p| c.rem_u32(*p) == 0) || c.rem_u32(e) == 1 {
            continue;
        }
        if miller_rabin(&c, RONDAS_MR, azar)? {
            return Ok(c);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// claves

/// Clave privada RSA (los campos de PKCS#1).
#[derive(Clone, Debug, PartialEq)]
pub struct Clave {
    pub n: Big,
    pub e: Big,
    pub d: Big,
    pub p: Big,
    pub q: Big,
    pub dp: Big,
    pub dq: Big,
    pub qinv: Big,
}

pub const E: u32 = 65537;

impl Clave {
    /// Clave nueva de `bits` bits (2048 para adb) con e = 65537.
    pub fn generar(bits: usize, azar: &mut dyn FnMut(usize) -> Result<Vec<u8>, String>) -> Result<Clave, String> {
        let uno = Big::from_u64(1);
        loop {
            let p = primo(bits / 2, E, azar)?;
            let q = primo(bits / 2, E, azar)?;
            if p == q {
                continue;
            }
            let (p, q) = if p > q { (p, q) } else { (q, p) };
            let n = p.mul(&q);
            if n.bits() != bits {
                continue;
            }
            let phi = p.sub(&uno).mul(&q.sub(&uno));
            // d = e^-1 mod phi: d = (1 + k*phi) / e con el k (1 <= k < e) que hace la division exacta
            let fm = phi.rem_u32(E) as u64;
            let Some(k) = (1..E as u64).find(|k| (1 + k * fm) % E as u64 == 0) else { continue };
            let (d, r) = Big::from_u64(1).add(&phi.mul(&Big::from_u64(k))).divrem(&Big::from_u64(E as u64));
            debug_assert!(r.is_zero());
            return Ok(Clave::de_primos(n, d, p, q));
        }
    }

    /// Completa los campos de CRT. `p` y `q` primos (qinv por Fermat: q^(p-2) mod p).
    fn de_primos(n: Big, d: Big, p: Big, q: Big) -> Clave {
        let uno = Big::from_u64(1);
        let dp = d.rem(&p.sub(&uno));
        let dq = d.rem(&q.sub(&uno));
        let qinv = q.modpow(&p.sub(&Big::from_u64(2)), &p);
        Clave { n, e: Big::from_u64(E as u64), d, p, q, dp, dq, qinv }
    }

    /// Bytes del modulo.
    pub fn bytes(&self) -> usize {
        self.n.bits().div_ceil(8)
    }

    /// Firma PKCS#1 v1.5 de un resumen SHA-1 de 20 bytes (lo que hace adb con el testigo de AUTH).
    pub fn firmar_sha1(&self, resumen: &[u8]) -> Result<Vec<u8>, String> {
        const DIGEST_INFO_SHA1: [u8; 15] = [0x30, 0x21, 0x30, 0x09, 0x06, 0x05, 0x2b, 0x0e, 0x03, 0x02, 0x1a, 0x05, 0x00, 0x04, 0x14];
        if resumen.len() != 20 {
            return Err(txf!("rsa.el_testigo_de_adb_debe_tener_20_bytes", resumen.len()));
        }
        let k = self.bytes();
        let t = DIGEST_INFO_SHA1.len() + 20;
        let mut em = vec![0xFFu8; k];
        em[0] = 0;
        em[1] = 1;
        em[k - t - 1] = 0;
        em[k - 20 - DIGEST_INFO_SHA1.len()..k - 20].copy_from_slice(&DIGEST_INFO_SHA1);
        em[k - 20..].copy_from_slice(resumen);
        let m = Big::from_be(&em);
        // CRT: s = s2 + q * (qinv * (s1 - s2) mod p)
        let s1 = m.modpow(&self.dp, &self.p);
        let s2 = m.modpow(&self.dq, &self.q);
        let s2p = s2.rem(&self.p);
        let dif = if s1 >= s2p { s1.sub(&s2p) } else { s1.add(&self.p).sub(&s2p) };
        let h = self.qinv.mul(&dif).rem(&self.p);
        let s = s2.add(&self.q.mul(&h));
        s.to_be(k).ok_or_else(|| tx!("rsa.firma_mas_larga_que_el_modulo").to_string())
    }

    /// Comprueba una firma con la clave publica: s^e mod n (para las pruebas).
    #[cfg(test)]
    pub fn abrir(&self, firma: &[u8]) -> Vec<u8> {
        Big::from_be(firma).modpow(&self.e, &self.n).to_be(self.bytes()).unwrap_or_default()
    }

    /// Clave publica en el formato de Android (`RSAPublicKey` de mincrypt: numero de palabras, n0inv = -1/n[0] mod 2^32, n
    /// y R^2 mod n en palabras de 32 bits little-endian, exponente), en base64, seguida de " " y `comentario`. Es lo que
    /// adb manda en AUTH RSAPUBLICKEY (con un 0 al final) y lo que guarda en adbkey.pub.
    pub fn publica_android(&self, comentario: &str) -> String {
        let palabras = self.bytes() / 4;
        let n = self.n.words(palabras);
        // inverso de n[0] modulo 2^32 por Newton (n[0] es impar)
        let mut x = 1u32;
        for _ in 0..5 {
            x = x.wrapping_mul(2u32.wrapping_sub(n[0].wrapping_mul(x)));
        }
        let rr = Big::from_u64(1).shl(2 * 32 * palabras).rem(&self.n).words(palabras);
        let mut b = Vec::with_capacity(4 * (3 + 2 * palabras));
        b.extend_from_slice(&(palabras as u32).to_le_bytes());
        b.extend_from_slice(&x.wrapping_neg().to_le_bytes());
        for w in n.iter().chain(rr.iter()) {
            b.extend_from_slice(&w.to_le_bytes());
        }
        b.extend_from_slice(&(self.e.words(1)[0]).to_le_bytes());
        format!("{} {}", base64(&b), comentario)
    }

    /// PEM PKCS#1 (`RSA PRIVATE KEY`).
    pub fn pem(&self) -> String {
        let mut cuerpo = der_entero(&Big::zero());
        for x in [&self.n, &self.e, &self.d, &self.p, &self.q, &self.dp, &self.dq, &self.qinv] {
            cuerpo.extend(der_entero(x));
        }
        let mut der = vec![0x30];
        der.extend(der_largo(cuerpo.len()));
        der.extend(cuerpo);
        let b = base64(&der);
        let mut s = String::from("-----BEGIN RSA PRIVATE KEY-----\n"); // texto-interno: formato PEM
        for c in b.as_bytes().chunks(64) {
            s.push_str(std::str::from_utf8(c).unwrap());
            s.push('\n');
        }
        s.push_str("-----END RSA PRIVATE KEY-----\n"); // texto-interno: formato PEM
        s
    }

    /// Lee un PEM PKCS#1 (`RSA PRIVATE KEY`) o PKCS#8 sin cifrar (`PRIVATE KEY`, lo que escribe el adb de Google moderno).
    pub fn de_pem(t: &str) -> Result<Clave, String> {
        let mala = || tx!("rsa.la_clave_no_es_un_pem_rsa_valido").to_string();
        let ini = t.find("-----BEGIN ").ok_or_else(mala)?;
        let tipo_fin = t[ini + 11..].find("-----").ok_or_else(mala)? + ini + 11;
        let tipo = &t[ini + 11..tipo_fin];
        let fin = t.find("-----END ").ok_or_else(mala)?;
        let der = de_base64(&t[tipo_fin + 5..fin]).ok_or_else(mala)?;
        let mut r = Der(&der);
        let mut seq = Der(r.tlv(0x30).ok_or_else(mala)?);
        let der_pkcs1;
        if tipo == "PRIVATE KEY" { // texto-interno: formato PEM
            // PrivateKeyInfo: version, AlgorithmIdentifier, OCTET STRING con la RSAPrivateKey
            seq.tlv(0x02).ok_or_else(mala)?;
            seq.tlv(0x30).ok_or_else(mala)?;
            der_pkcs1 = seq.tlv(0x04).ok_or_else(mala)?.to_vec();
            seq = Der(&der_pkcs1);
            seq = Der(seq.tlv(0x30).ok_or_else(mala)?);
        } else if tipo != "RSA PRIVATE KEY" { // texto-interno: formato PEM
            return Err(txf!("rsa.tipo_de_clave_no_admitido", tipo));
        }
        let mut v = Vec::new();
        for _ in 0..9 {
            v.push(Big::from_be(seq.tlv(0x02).ok_or_else(mala)?));
        }
        let c = Clave { n: v[1].clone(), e: v[2].clone(), d: v[3].clone(), p: v[4].clone(), q: v[5].clone(), dp: v[6].clone(), dq: v[7].clone(), qinv: v[8].clone() };
        if c.p.mul(&c.q) != c.n || c.n.bits() < 512 {
            return Err(mala());
        }
        Ok(c)
    }
}

fn der_largo(n: usize) -> Vec<u8> {
    if n < 0x80 {
        vec![n as u8]
    } else {
        let b: Vec<u8> = (n as u64).to_be_bytes().into_iter().skip_while(|x| *x == 0).collect();
        let mut v = vec![0x80 | b.len() as u8];
        v.extend(b);
        v
    }
}

fn der_entero(x: &Big) -> Vec<u8> {
    let mut b = x.to_be_min();
    if b[0] & 0x80 != 0 {
        b.insert(0, 0);
    }
    let mut v = vec![0x02];
    v.extend(der_largo(b.len()));
    v.extend(b);
    v
}

/// Lector DER minimo: TLV con etiqueta de un byte y largo corto o largo.
struct Der<'a>(&'a [u8]);

impl<'a> Der<'a> {
    fn tlv(&mut self, etiqueta: u8) -> Option<&'a [u8]> {
        let b = self.0;
        if b.len() < 2 || b[0] != etiqueta {
            return None;
        }
        let (largo, cab) = if b[1] < 0x80 {
            (b[1] as usize, 2)
        } else {
            let k = (b[1] & 0x7f) as usize;
            if k == 0 || k > 4 || b.len() < 2 + k {
                return None;
            }
            (b[2..2 + k].iter().fold(0usize, |a, x| (a << 8) | *x as usize), 2 + k)
        };
        let v = b.get(cab..cab.checked_add(largo)?)?;
        self.0 = &b[cab + largo..];
        Some(v)
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            s.push(if i <= c.len() { B64[(n >> (18 - 6 * i)) as usize & 63] as char } else { '=' });
        }
    }
    s
}

/// Base64 (ignora espacios y saltos de linea). None si hay algo que no es base64.
pub fn de_base64(t: &str) -> Option<Vec<u8>> {
    let mut v = Vec::new();
    let (mut acc, mut n) = (0u32, 0);
    for c in t.bytes().filter(|c| !c.is_ascii_whitespace()) {
        if c == b'=' {
            break;
        }
        let x = B64.iter().position(|b| *b == c)? as u32;
        acc = (acc << 6) | x;
        n += 6;
        if n >= 8 {
            n -= 8;
            v.push((acc >> n) as u8);
        }
    }
    Some(v)
}

// ---------------------------------------------------------------------------------------------------------------
// la clave de adb de weft

/// Comentario de la clave publica que se manda a Android (lo que el aparato muestra al pedir permiso). Fijo: sin nombre de
/// usuario ni de equipo.
pub const COMENTARIO: &str = "weft";

/// Archivo de la clave privada de adb de weft en la carpeta de configuracion (`adbkey`; la publica en `adbkey.pub`).
pub fn ruta_clave() -> std::path::PathBuf {
    crate::rutas::actual().config.join("adbkey")
}

/// La clave de adb de weft: la de `ruta` si existe; si no, una nueva de 2048 bits que se guarda ahi (0600, por un temporal
/// que se renombra: otra orden a la vez no lee una clave a medias) junto con `ruta.pub`. Devuelve (clave, si es nueva).
pub fn cargar_o_crear(ruta: &std::path::Path) -> Result<(Clave, bool), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Ok(t) = std::fs::read_to_string(ruta) {
        return Clave::de_pem(&t).map(|c| (c, false)).map_err(|e| format!("{}: {}", ruta.display(), e));
    }
    let c = Clave::generar(2048, &mut azar)?;
    if let Some(d) = ruta.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {}", d.display(), e))?;
    }
    let tmp = ruta.with_extension(format!("parte-{}", std::process::id()));
    let escribir = |p: &std::path::Path, t: &str| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(p)?;
        f.write_all(t.as_bytes())?;
        f.sync_all()
    };
    escribir(&tmp, &c.pem()).map_err(|e| format!("{}: {}", tmp.display(), e))?;
    // si otra orden la creo mientras tanto, se usa la suya (no se reemplaza una clave que el aparato ya pudo aceptar)
    match std::fs::hard_link(&tmp, ruta) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = std::fs::remove_file(&tmp);
            let t = std::fs::read_to_string(ruta).map_err(|e| format!("{}: {}", ruta.display(), e))?;
            return Clave::de_pem(&t).map(|c| (c, false)).map_err(|e| format!("{}: {}", ruta.display(), e));
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("{}: {}", ruta.display(), e));
        }
    }
    let _ = std::fs::remove_file(&tmp);
    let _ = escribir(&ruta.with_extension("pub"), &format!("{}\n", c.publica_android(COMENTARIO)));
    Ok((c, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generador determinista para las pruebas (xorshift).
    fn pseudo(semilla: u64) -> impl FnMut(usize) -> Result<Vec<u8>, String> {
        let mut s = semilla;
        move |n| {
            Ok((0..n)
                .map(|_| {
                    s ^= s << 13;
                    s ^= s >> 7;
                    s ^= s << 17;
                    (s >> 24) as u8
                })
                .collect())
        }
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{:02x}", x)).collect()
    }

    fn de_hex(t: &str) -> Vec<u8> {
        let t: String = t.chars().filter(|c| c.is_ascii_hexdigit()).collect();
        (0..t.len()).step_by(2).map(|i| u8::from_str_radix(&t[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn aritmetica() {
        let a = Big::from_be(&de_hex("0123456789abcdef0011223344556677"));
        assert_eq!(hex(&a.to_be_min()), "0123456789abcdef0011223344556677");
        assert_eq!(a.to_be(18).unwrap().len(), 18);
        assert!(a.to_be(15).is_none());
        assert_eq!(Big::from_u64(0), Big::zero());
        assert_eq!(Big::from_u64(1 << 40).bits(), 41);
        assert_eq!(Big::from_u64(5).shl(70).shr(70), Big::from_u64(5));
        assert_eq!(Big::from_u64(u64::MAX).add(&Big::from_u64(1)), Big::from_u64(1).shl(64));
        assert_eq!(Big::from_u64(1).shl(64).sub(&Big::from_u64(1)), Big::from_u64(u64::MAX));
        // 3^200 mod 1e9+7, con u128
        let mut r: u128 = 1;
        for _ in 0..200 {
            r = r * 3 % 1_000_000_007;
        }
        assert_eq!(Big::from_u64(3).modpow(&Big::from_u64(200), &Big::from_u64(1_000_000_007)), Big::from_u64(r as u64));
        // division: (a*b + r) / b = (a, r) con numeros al azar de muchos tamanos (incluidos los casos raros de qhat)
        let mut g = pseudo(7);
        for i in 0..400 {
            let la = 1 + i % 70;
            let lb = 1 + (i * 7) % 40;
            let mut ba = g(la).unwrap();
            let mut bb = g(lb).unwrap();
            if i % 5 == 0 {
                // palabras llenas de unos: fuerzan la correccion de qhat y la suma de vuelta
                ba.iter_mut().for_each(|x| *x |= 0xF0);
                bb[0] = 0xFF;
            }
            let (a, b) = (Big::from_be(&ba), Big::from_be(&bb));
            if b.is_zero() {
                continue;
            }
            let r = Big::from_be(&g(lb).unwrap()).rem(&b);
            let x = a.mul(&b).add(&r);
            assert_eq!(x.divrem(&b), (a.clone(), r.clone()), "caso {}", i);
            assert!(r < b);
        }
    }

    #[test]
    fn primos() {
        let mut g = pseudo(3);
        // 2^127 - 1 es primo; 2^127 + 1 no (divisible por 3), y un producto de dos primos tampoco
        let m127 = Big::from_u64(1).shl(127).sub(&Big::from_u64(1));
        assert!(miller_rabin(&m127, 20, &mut g).unwrap());
        assert!(!miller_rabin(&Big::from_u64(1).shl(127).add(&Big::from_u64(1)), 20, &mut g).unwrap());
        let p = Big::from_u64(1_000_000_007);
        assert!(miller_rabin(&p, 20, &mut g).unwrap());
        assert!(!miller_rabin(&p.mul(&Big::from_u64(998_244_353)), 20, &mut g).unwrap());
        // numero de Carmichael 561 = 3*11*17
        assert!(!miller_rabin(&Big::from_u64(561), 20, &mut g).unwrap());
        assert_eq!(primos_pequenos()[..5], [3, 5, 7, 11, 13]);
        assert_eq!(primos_pequenos().len(), 302);
    }

    /// Clave de prueba (solo para estas pruebas; generada con `openssl genrsa 2048`).
    const PEM_PRUEBA: &str = include_str!("../tests/datos/rsa_prueba.pem");
    /// Testigo fijo y su firma, calculada con openssl:
    /// `openssl pkeyutl -sign -inkey tests/datos/rsa_prueba.pem -pkeyopt digest:sha1 -in testigo` (PKCS#1 v1.5 con DigestInfo SHA-1).
    const TESTIGO: &str = "000102030405060708090a0b0c0d0e0f10111213";
    const FIRMA_OPENSSL: &str = include_str!("../tests/datos/rsa_prueba.firma");
    /// Clave publica en el formato de Android, calculada aparte con un guion de Python que sigue la estructura de mincrypt.
    const PUBLICA_ANDROID: &str = include_str!("../tests/datos/rsa_prueba.pub");

    #[test]
    fn firma_igual_que_openssl() {
        let c = Clave::de_pem(PEM_PRUEBA).unwrap();
        assert_eq!(c.n.bits(), 2048);
        assert_eq!(c.e, Big::from_u64(65537));
        let f = c.firmar_sha1(&de_hex(TESTIGO)).unwrap();
        assert_eq!(hex(&f), FIRMA_OPENSSL.trim());
        // la firma abierta con la clave publica es el bloque PKCS#1 con el DigestInfo de SHA-1 y el testigo
        let em = c.abrir(&f);
        assert_eq!(&em[..2], &[0, 1]);
        assert!(hex(&em).ends_with(&format!("003021300906052b0e03021a05000414{}", TESTIGO)));
        assert!(c.firmar_sha1(&[0u8; 19]).is_err());
    }

    #[test]
    fn publica_en_formato_android() {
        let c = Clave::de_pem(PEM_PRUEBA).unwrap();
        assert_eq!(c.publica_android("weft"), PUBLICA_ANDROID.trim());
        let b = de_base64(PUBLICA_ANDROID.split(' ').next().unwrap()).unwrap();
        assert_eq!(b.len(), 4 * (3 + 2 * 64));
        assert_eq!(&b[..4], &64u32.to_le_bytes());
    }

    #[test]
    fn pem_ida_y_vuelta() {
        let c = Clave::de_pem(PEM_PRUEBA).unwrap();
        assert_eq!(Clave::de_pem(&c.pem()).unwrap(), c);
        // el mismo DER que el PEM de openssl
        let solo = |t: &str| t.lines().filter(|l| !l.starts_with("-----")).collect::<String>();
        assert_eq!(solo(&c.pem()), solo(PEM_PRUEBA));
        assert!(Clave::de_pem("-----BEGIN RSA PRIVATE KEY-----\nAAAA\n-----END RSA PRIVATE KEY-----\n").is_err());
        assert!(Clave::de_pem("nada").is_err());
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
        assert_eq!(de_base64("YWI=").unwrap(), b"ab");
    }

    /// Una clave nueva: 2048 bits exactos, firma que se abre con la publica, PEM legible otra vez (y por openssl, si esta).
    #[test]
    fn generar_clave() {
        let mut g = pseudo(0x5eed);
        let c = Clave::generar(2048, &mut g).unwrap();
        assert_eq!(c.n.bits(), 2048);
        assert_eq!(c.p.mul(&c.q), c.n);
        let f = c.firmar_sha1(&[7u8; 20]).unwrap();
        assert!(hex(&c.abrir(&f)).ends_with(&"07".repeat(20)));
        // e*d = 1 mod (p-1)(q-1): firmar con d sin CRT da lo mismo
        let em = Big::from_be(&c.abrir(&f));
        assert_eq!(em.modpow(&c.d, &c.n).to_be(256).unwrap(), f);
        let d = std::env::temp_dir().join(format!("weft-rsa-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let k = d.join("adbkey");
        std::fs::write(&k, c.pem()).unwrap();
        if let Ok(o) = std::process::Command::new("openssl").args(["rsa", "-check", "-noout", "-in"]).arg(&k).output() {
            assert!(o.status.success() && String::from_utf8_lossy(&o.stdout).contains("RSA key ok"), "{}", String::from_utf8_lossy(&o.stderr));
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// La clave de weft se crea una vez (0600, con su .pub) y despues se reutiliza.
    #[test]
    fn clave_guardada() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("weft-adbkey-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let k = d.join("conf").join("adbkey");
        let (c, nueva) = cargar_o_crear(&k).unwrap();
        assert!(nueva);
        assert_eq!(std::fs::metadata(&k).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read_to_string(d.join("conf").join("adbkey.pub")).unwrap().trim(), c.publica_android(COMENTARIO));
        let (c2, nueva) = cargar_o_crear(&k).unwrap();
        assert!(!nueva && c2 == c);
        assert!(std::fs::read_dir(d.join("conf")).unwrap().count() == 2, "quedaron temporales");
        let _ = std::fs::remove_dir_all(&d);
    }
}
