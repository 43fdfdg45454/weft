//! MD5 (RFC 1321) sin dependencias. Solo sirve para identificar archivos (la biblioteca del traductor ARM se compara por
//! md5 con la del invitado, que la calcula `md5sum`); no es una proteccion contra manipulacion.

const S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15,
    21, 6, 10, 15, 21,
];

fn k(i: usize) -> u32 {
    // K[i] = floor(2^32 * |sin(i + 1)|)
    ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32
}

pub struct Md5 {
    a: [u32; 4],
    buf: Vec<u8>,
    total: u64,
    k: [u32; 64],
}

impl Md5 {
    pub fn new() -> Md5 {
        let mut kk = [0u32; 64];
        for (i, v) in kk.iter_mut().enumerate() {
            *v = k(i);
        }
        Md5 { a: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476], buf: Vec::with_capacity(64), total: 0, k: kk }
    }

    fn bloque(&mut self, b: &[u8]) {
        let mut m = [0u32; 16];
        for (i, w) in m.iter_mut().enumerate() {
            *w = u32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
        }
        let [mut a, mut bb, mut c, mut d] = self.a;
        for (i, s) in S.iter().enumerate() {
            let (mut f, g) = match i / 16 {
                0 => ((bb & c) | (!bb & d), i),
                1 => ((d & bb) | (!d & c), (5 * i + 1) % 16),
                2 => (bb ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (bb | !d), (7 * i) % 16),
            };
            f = f.wrapping_add(a).wrapping_add(self.k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = bb;
            bb = bb.wrapping_add(f.rotate_left(*s));
        }
        self.a[0] = self.a[0].wrapping_add(a);
        self.a[1] = self.a[1].wrapping_add(bb);
        self.a[2] = self.a[2].wrapping_add(c);
        self.a[3] = self.a[3].wrapping_add(d);
    }

    pub fn escribir(&mut self, datos: &[u8]) {
        self.total += datos.len() as u64;
        let mut d = datos;
        if !self.buf.is_empty() {
            let falta = 64 - self.buf.len();
            let n = falta.min(d.len());
            self.buf.extend_from_slice(&d[..n]);
            d = &d[n..];
            if self.buf.len() == 64 {
                let b = std::mem::take(&mut self.buf);
                self.bloque(&b);
            }
        }
        while d.len() >= 64 {
            self.bloque(&d[..64]);
            d = &d[64..];
        }
        if !d.is_empty() {
            self.buf.extend_from_slice(d);
        }
    }

    pub fn terminar(mut self) -> String {
        let bits = self.total.wrapping_mul(8);
        let mut pad = vec![0x80u8];
        while (self.buf.len() + pad.len()) % 64 != 56 {
            pad.push(0);
        }
        pad.extend_from_slice(&bits.to_le_bytes());
        let total = self.total;
        self.escribir(&pad);
        self.total = total;
        self.a.iter().flat_map(|w| w.to_le_bytes()).map(|b| format!("{:02x}", b)).collect()
    }
}

/// md5 (32 hexadecimales) de un texto o bytes.
pub fn de_bytes(d: &[u8]) -> String {
    let mut m = Md5::new();
    m.escribir(d);
    m.terminar()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vectores_del_rfc() {
        assert_eq!(de_bytes(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(de_bytes(b"a"), "0cc175b9c0f1b6a831c399e269772661");
        assert_eq!(de_bytes(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(de_bytes(b"message digest"), "f96b697d7cb7938d525a2f31aaf161d0");
        assert_eq!(de_bytes(b"abcdefghijklmnopqrstuvwxyz"), "c3fcd3d76192e4007dfb496cca67e13b");
        assert_eq!(de_bytes(b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"), "d174ab98d277d9f5a5611c2c9f419d9f");
        assert_eq!(de_bytes(b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"), "57edf4a22be3c955ac49da2e2107b67a");
    }

    /// Alrededor del borde del relleno (55 bytes caben con la longitud en un bloque; 56 ya necesitan otro; 64 y 65 cruzan
    /// el bloque): de una vez, byte a byte y en dos trozos por cada corte posible, todo igual que `md5sum`.
    #[test]
    fn longitudes_en_el_borde_del_relleno() {
        let esperados = [
            (55, "ef1772b6dff9a122358552954ad0df65"),
            (56, "3b0c8ac703f828b04c6c197006d17218"),
            (57, "652b906d60af96844ebd21b674f35e93"),
            (58, "dc2f2f2462a0d72358b2f99389458606"),
            (59, "762fc2665994b217c52c3c2eb7d9f406"),
            (60, "cc7ed669cf88f201c3297c6a91e1d18d"),
            (61, "cced11f7bbbffea2f718903216643648"),
            (62, "24612f0ce2c9d2cf2b022ef1e027a54f"),
            (63, "b06521f39153d618550606be297466d5"),
            (64, "014842d480b571495a4a0363793f7367"),
            (65, "c743a45e0d2e6a95cb859adae0248435"),
        ];
        for (n, md5sum) in esperados {
            let datos = vec![b'a'; n];
            let entero = de_bytes(&datos);
            assert_eq!(entero, md5sum, "{} bytes", n);
            let mut m = Md5::new();
            for b in &datos {
                m.escribir(std::slice::from_ref(b));
            }
            assert_eq!(m.terminar(), entero, "{} bytes de uno en uno", n);
            for corte in 0..=n {
                let mut m = Md5::new();
                m.escribir(&datos[..corte]);
                m.escribir(&datos[corte..]);
                assert_eq!(m.terminar(), entero, "{} bytes cortados en {}", n, corte);
            }
        }
    }

    #[test]
    fn por_trozos_igual_que_de_una_vez() {
        let datos: Vec<u8> = (0..1000u32).map(|i| (i * 7 + 3) as u8).collect();
        let entero = de_bytes(&datos);
        let mut m = Md5::new();
        for c in datos.chunks(37) {
            m.escribir(c);
        }
        assert_eq!(m.terminar(), entero);
    }
}
