//! JSON minimo (sin dependencias) para hablar QMP con QEMU.

use std::collections::BTreeMap;
use std::fmt::Write;
use crate::textos::{tx, txf};

#[derive(Clone, Debug, PartialEq)]
pub enum V {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<V>),
    Obj(BTreeMap<String, V>),
}

impl V {
    pub fn obj(pairs: &[(&str, V)]) -> V {
        V::Obj(pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
    }
    pub fn s(x: &str) -> V {
        V::Str(x.to_string())
    }
    pub fn n(x: i64) -> V {
        V::Num(x as f64)
    }
    pub fn get(&self, k: &str) -> Option<&V> {
        match self {
            V::Obj(m) => m.get(k),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            V::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> Option<&Vec<V>> {
        match self {
            V::Arr(a) => Some(a),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            V::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn dump(&self) -> String {
        let mut o = String::new();
        self.write(&mut o);
        o
    }
    fn write(&self, o: &mut String) {
        match self {
            V::Null => o.push_str("null"),
            V::Bool(b) => o.push_str(if *b { "true" } else { "false" }),
            // NaN e infinito no existen en JSON: se escriben como null (si no, el texto no se podria leer)
            V::Num(n) if !n.is_finite() => o.push_str("null"),
            V::Num(n) if n.fract() == 0.0 && n.abs() < 9e15 => {
                let _ = write!(o, "{}", *n as i64);
            }
            V::Num(n) => {
                let _ = write!(o, "{}", n);
            }
            V::Str(s) => {
                o.push('"');
                for c in s.chars() {
                    match c {
                        '"' => o.push_str("\\\""),
                        '\\' => o.push_str("\\\\"),
                        '\n' => o.push_str("\\n"),
                        '\r' => o.push_str("\\r"),
                        '\t' => o.push_str("\\t"),
                        c if (c as u32) < 0x20 => {
                            let _ = write!(o, "\\u{:04x}", c as u32);
                        }
                        c => o.push(c),
                    }
                }
                o.push('"');
            }
            V::Arr(a) => {
                o.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        o.push(',');
                    }
                    v.write(o);
                }
                o.push(']');
            }
            V::Obj(m) => {
                o.push('{');
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        o.push(',');
                    }
                    V::Str(k.clone()).write(o);
                    o.push(':');
                    v.write(o);
                }
                o.push('}');
            }
        }
    }
}

/// Niveles de arreglos y objetos anidados que admite `parse`: QMP no pasa de unos pocos y el analizador es recursivo, asi
/// que un texto mas profundo se rechaza en vez de agotar la pila.
pub const MAX_PROFUNDIDAD: usize = 128;

pub fn parse(text: &str) -> Result<V, String> {
    let mut p = P { b: text.as_bytes(), i: 0, prof: 0 };
    let v = p.value()?;
    p.ws();
    if p.i != p.b.len() {
        return Err(txf!("json.sobra_texto_en_la_posicion", p.i));
    }
    Ok(v)
}

struct P<'a> {
    b: &'a [u8],
    i: usize,
    /// arreglos y objetos abiertos ahora mismo
    prof: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\n' | b'\r' | b'\t') {
            self.i += 1;
        }
    }
    fn lit(&mut self, s: &str, v: V) -> Result<V, String> {
        if self.b[self.i..].starts_with(s.as_bytes()) {
            self.i += s.len();
            Ok(v)
        } else {
            Err(txf!("json.valor_no_valido_en_la_posicion", self.i))
        }
    }
    /// Entra en un arreglo u objeto; Err si pasa de MAX_PROFUNDIDAD.
    fn entrar(&mut self) -> Result<(), String> {
        if self.prof >= MAX_PROFUNDIDAD {
            return Err(txf!("json.json_demasiado_anidado_mas_de_niveles_en", MAX_PROFUNDIDAD, self.i));
        }
        self.prof += 1;
        self.i += 1;
        Ok(())
    }
    fn value(&mut self) -> Result<V, String> {
        self.ws();
        match self.b.get(self.i).copied() {
            None => Err(tx!("json.fin_inesperado").into()),
            Some(b'n') => self.lit("null", V::Null),
            Some(b't') => self.lit("true", V::Bool(true)),
            Some(b'f') => self.lit("false", V::Bool(false)),
            Some(b'"') => self.string().map(V::Str),
            Some(b'[') => {
                self.entrar()?;
                let mut a = Vec::new();
                loop {
                    self.ws();
                    if self.b.get(self.i) == Some(&b']') {
                        self.i += 1;
                        self.prof -= 1;
                        return Ok(V::Arr(a));
                    }
                    if !a.is_empty() {
                        self.expect(b',')?;
                    }
                    a.push(self.value()?);
                }
            }
            Some(b'{') => {
                self.entrar()?;
                let mut m = BTreeMap::new();
                loop {
                    self.ws();
                    if self.b.get(self.i) == Some(&b'}') {
                        self.i += 1;
                        self.prof -= 1;
                        return Ok(V::Obj(m));
                    }
                    if !m.is_empty() {
                        self.expect(b',')?;
                        self.ws();
                    }
                    let k = self.string()?;
                    self.ws();
                    self.expect(b':')?;
                    m.insert(k, self.value()?);
                }
            }
            Some(_) => {
                let s = self.i;
                while self.i < self.b.len() && matches!(self.b[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                    self.i += 1;
                }
                std::str::from_utf8(&self.b[s..self.i]).ok().and_then(|t| t.parse::<f64>().ok()).map(V::Num).ok_or(txf!("json.numero_no_valido_en_la_posicion", s))
            }
        }
    }
    fn expect(&mut self, c: u8) -> Result<(), String> {
        if self.b.get(self.i) == Some(&c) {
            self.i += 1;
            Ok(())
        } else {
            Err(txf!("json.se_esperaba_en_la_posicion", c as char, self.i))
        }
    }
    /// Los 4 hexadecimales de un escape \u (solo digitos: ni signo ni espacios).
    fn hex4(&mut self) -> Result<u32, String> {
        let h = self.b.get(self.i..self.i + 4).ok_or("escape \\u incompleto")?;
        if !h.iter().all(u8::is_ascii_hexdigit) {
            return Err(txf!("json.escape_u_no_valido_en_la_posicion", self.i));
        }
        self.i += 4;
        Ok(h.iter().fold(0, |n, c| n * 16 + (*c as char).to_digit(16).unwrap_or(0)))
    }
    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = Vec::new();
        loop {
            let c = *self.b.get(self.i).ok_or(tx!("json.cadena_sin_cerrar"))?;
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).map_err(|_| tx!("json.utf_8_no_valido").to_string()),
                b'\\' => {
                    let e = *self.b.get(self.i).ok_or(tx!("json.escape_sin_terminar"))?;
                    self.i += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'u' => {
                            let mut cp = self.hex4()?;
                            // fuera del plano basico, UTF-16 lo parte en dos escapes: \ud83d\ude00 es un solo caracter
                            if (0xD800..0xDC00).contains(&cp) && self.b[self.i..].starts_with(b"\\u") {
                                let guarda = self.i;
                                self.i += 2;
                                match self.hex4()? {
                                    bajo @ 0xDC00..=0xDFFF => cp = 0x10000 + ((cp - 0xD800) << 10) + (bajo - 0xDC00),
                                    // no es la segunda mitad: la primera queda suelta y el escape siguiente va aparte
                                    _ => self.i = guarda,
                                }
                            }
                            // una mitad suelta no es un caracter: U+FFFD
                            let ch = char::from_u32(cp).unwrap_or('\u{fffd}');
                            out.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
                        }
                        other => out.push(other),
                    }
                }
                c => out.push(c),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ida_y_vuelta() {
        let t = r#"{"QMP":{"version":{"qemu":{"major":8,"minor":2}},"capabilities":["oob"]},"x":[1,-2.5,true,null,"a\"b\né"]}"#;
        let v = parse(t).unwrap();
        assert_eq!(v.get("QMP").unwrap().get("version").unwrap().get("qemu").unwrap().get("major"), Some(&V::Num(8.0)));
        assert_eq!(parse(&v.dump()).unwrap(), v);
        assert!(parse("{\"a\":1} x").is_err());
        assert!(parse("{\"a\"").is_err());
    }

    /// Un texto muy anidado da error sin agotar la pila (el analizador es recursivo); hasta el limite se lee bien.
    #[test]
    fn anidamiento_profundo() {
        let e = parse(&"[".repeat(100_000)).unwrap_err();
        assert!(e.contains("JSON demasiado anidado"), "{}", e);
        let e = parse(&"{\"a\":".repeat(100_000)).unwrap_err();
        assert!(e.contains("JSON demasiado anidado"), "{}", e);
        let justo = format!("{}{}", "[".repeat(MAX_PROFUNDIDAD), "]".repeat(MAX_PROFUNDIDAD));
        assert!(parse(&justo).is_ok());
        let uno_mas = format!("{}{}", "[".repeat(MAX_PROFUNDIDAD + 1), "]".repeat(MAX_PROFUNDIDAD + 1));
        assert!(parse(&uno_mas).unwrap_err().contains("demasiado anidado"));
        // la profundidad baja al cerrar: muchos hermanos seguidos no cuentan como anidamiento
        let hermanos = format!("[{}[]]", "[[]],".repeat(1000));
        assert!(parse(&hermanos).is_ok());
    }

    #[test]
    fn escapes_unicode() {
        // par sustituto: un solo caracter fuera del plano basico
        assert_eq!(parse(r#""😀""#).unwrap(), V::s("😀"));
        assert_eq!(parse(r#""a😀b""#).unwrap(), V::s("a😀b"));
        assert_eq!(parse(r#""é€""#).unwrap(), V::s("é€"));
        // mitades sueltas: U+FFFD, sin comerse lo que sigue
        assert_eq!(parse(r#""\ud83d""#).unwrap(), V::s("\u{fffd}"));
        assert_eq!(parse(r#""\ude00x""#).unwrap(), V::s("\u{fffd}x"));
        assert_eq!(parse(r#""\ud83dA""#).unwrap(), V::s("\u{fffd}A"));
        assert_eq!(parse(r#""\ud83d\n""#).unwrap(), V::s("\u{fffd}\n"));
        // solo 4 hexadecimales: ni signo ni menos digitos
        assert!(parse(r#""\u+041""#).is_err());
        assert!(parse(r#""\u-041""#).is_err());
        assert!(parse(r#""\u04""#).is_err());
        assert!(parse(r#""\ud83d\u+e00""#).is_err());
        // ida y vuelta de un caracter de 4 bytes
        assert_eq!(parse(&V::s("x😀").dump()).unwrap(), V::s("x😀"));
    }

    /// NaN e infinito no son JSON: se escriben como null y el resultado se vuelve a leer.
    #[test]
    fn numeros_no_finitos() {
        assert_eq!(V::Num(f64::NAN).dump(), "null");
        assert_eq!(V::Num(f64::INFINITY).dump(), "null");
        assert_eq!(V::Num(f64::NEG_INFINITY).dump(), "null");
        let v = V::Arr(vec![V::Num(f64::NAN), V::n(3), V::Num(-2.5)]);
        assert_eq!(v.dump(), "[null,3,-2.5]");
        assert_eq!(parse(&v.dump()).unwrap(), V::Arr(vec![V::Null, V::n(3), V::Num(-2.5)]));
    }
}
