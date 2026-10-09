//! weft NO descarga nada (decision del usuario, 2026-10-07): ni imagenes, ni componentes de root, ni el traductor ARM,
//! ni nada mas; no necesita internet. Esta prueba lee las fuentes de src/ (sin sus modulos de pruebas) y falla si aparece
//! alguna herramienta o direccion de descarga. Quien baja archivos es el usuario (o el guion de desarrollo
//! scripts/dev-fedora.sh, que esta fuera del programa).

use std::path::Path;

/// Palabras que delatan una descarga (en minusculas).
const PROHIBIDAS: &[&str] = &["curl", "wget", "http://", "https://", "ftp://", "reqwest", "ureq"];

/// Marca que abre un modulo de pruebas.
const MARCA: &str = "#[cfg(test)]";

/// Quita de un fuente de Rust todos los bloques `#[cfg(test)] mod X { ... }`, esten donde esten (al final, en medio o
/// varios), cada uno hasta su llave de cierre por equilibrio de llaves (las de cadenas, caracteres y comentarios no
/// cuentan). Cada bloque se cambia por sus saltos de linea, asi que los numeros de linea del resto siguen valiendo.
/// Lo demas marcado con `#[cfg(test)]` (funciones, `mod x;` sin cuerpo) se queda: mejor revisar de mas que de menos.
fn quitar_modulos_de_prueba(src: &str) -> String {
    let c: Vec<char> = src.chars().collect();
    let marca: Vec<char> = MARCA.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        if c[i..].starts_with(&marca) {
            if let Some(fin) = inicio_de_modulo(&c, i + marca.len()).and_then(|llave| cierre(&c, llave)) {
                out.extend(c[i..fin].iter().filter(|x| **x == '\n'));
                i = fin;
                continue;
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

/// Si tras la marca (desde `i`) viene `[atributos] [pub[(..)]] mod nombre {`, la posicion de esa llave.
fn inicio_de_modulo(c: &[char], mut i: usize) -> Option<usize> {
    let blancos = |mut i: usize| {
        while c.get(i).is_some_and(|x| x.is_whitespace()) {
            i += 1;
        }
        i
    };
    let palabra = |i: usize, p: &str| {
        let p: Vec<char> = p.chars().collect();
        c[i.min(c.len())..].starts_with(&p) && !c.get(i + p.len()).is_some_and(|x| x.is_alphanumeric() || *x == '_')
    };
    i = blancos(i);
    // otros atributos entre la marca y el modulo (`#[allow(..)]`)
    while c.get(i) == Some(&'#') && c.get(i + 1) == Some(&'[') {
        let mut nivel = 0;
        i += 1;
        while i < c.len() {
            match c[i] {
                '[' => nivel += 1,
                ']' => nivel -= 1,
                _ => {}
            }
            i += 1;
            if nivel == 0 {
                break;
            }
        }
        i = blancos(i);
    }
    if palabra(i, "pub") {
        i = blancos(i + 3);
        if c.get(i) == Some(&'(') {
            i += c[i..].iter().position(|x| *x == ')')? + 1;
        }
        i = blancos(i);
    }
    if !palabra(i, "mod") {
        return None;
    }
    i = blancos(i + 3);
    let nombre = c[i.min(c.len())..].iter().take_while(|x| x.is_alphanumeric() || **x == '_').count();
    if nombre == 0 {
        return None;
    }
    i = blancos(i + nombre);
    (c.get(i) == Some(&'{')).then_some(i)
}

/// Fin (exclusivo) del bloque cuya llave de apertura esta en `i`, saltando cadenas (tambien crudas), caracteres y
/// comentarios. None si el bloque no se cierra (entonces no se quita nada).
fn cierre(c: &[char], mut i: usize) -> Option<usize> {
    let ident = |j: usize| j < c.len() && (c[j].is_alphanumeric() || c[j] == '_');
    let mut nivel = 0usize;
    while i < c.len() {
        match c[i] {
            '/' if c.get(i + 1) == Some(&'/') => {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '/' if c.get(i + 1) == Some(&'*') => {
                let mut prof = 0;
                while i < c.len() {
                    if c[i] == '/' && c.get(i + 1) == Some(&'*') {
                        prof += 1;
                        i += 2;
                    } else if c[i] == '*' && c.get(i + 1) == Some(&'/') {
                        prof -= 1;
                        i += 2;
                        if prof == 0 {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                continue;
            }
            '"' => {
                i += 1;
                while i < c.len() && c[i] != '"' {
                    i += if c[i] == '\\' { 2 } else { 1 };
                }
            }
            // cadena cruda: r"..", r#".."#, br".."
            'r' if (i == 0 || !ident(i - 1) || (c[i - 1] == 'b' && (i < 2 || !ident(i - 2))))
                && c.get(i + 1 + c[i + 1..].iter().take_while(|x| **x == '#').count()) == Some(&'"') =>
            {
                let h = c[i + 1..].iter().take_while(|x| **x == '#').count();
                i += h + 2;
                while i < c.len() && !(c[i] == '"' && (1..=h).all(|k| c.get(i + k) == Some(&'#'))) {
                    i += 1;
                }
                i += h;
            }
            // caracter con escape ('\'', '\\', '\u{7b}') o simple ('{'); si no, es un tiempo de vida ('a)
            '\'' if c.get(i + 1) == Some(&'\\') => {
                i += 3;
                while i < c.len() && c[i] != '\'' {
                    i += 1;
                }
            }
            '\'' if c.get(i + 2) == Some(&'\'') => i += 2,
            '{' => nivel += 1,
            '}' => {
                nivel -= 1;
                if nivel == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Lineas de un fuente (sin sus modulos de pruebas) que nombran una herramienta o direccion de descarga.
fn hallazgos(nombre: &str, src: &str) -> Vec<String> {
    let mut v = Vec::new();
    for (n, l) in quitar_modulos_de_prueba(src).lines().enumerate() {
        let m = l.to_lowercase();
        if PROHIBIDAS.iter().any(|p| m.contains(p)) {
            v.push(format!("src/{}:{}: {}", nombre, n + 1, l.trim()));
        }
    }
    v
}

fn fuentes() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut v = Vec::new();
    for e in std::fs::read_dir(&dir).unwrap().flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "rs") {
            let t = std::fs::read_to_string(&p).unwrap();
            v.push((p.file_name().unwrap().to_string_lossy().into_owned(), t));
        }
    }
    v
}

#[test]
fn el_programa_no_descarga_nada() {
    let mut todos = Vec::new();
    for (nombre, codigo) in fuentes() {
        todos.extend(hallazgos(&nombre, &codigo));
    }
    assert!(todos.is_empty(), "weft no descarga nada; sobran referencias:\n{}", todos.join("\n"));
}

#[test]
fn las_fuentes_se_leen() {
    // si la carpeta cambiara de sitio, la prueba anterior pasaria en vacio
    assert!(fuentes().len() > 20);
}

#[test]
fn quitar_modulos_de_prueba_al_final() {
    let src = "fn a() {}\n#[cfg(test)]\nmod tests {\n    fn b() { let x = 1; }\n}\n";
    assert_eq!(quitar_modulos_de_prueba(src), "fn a() {}\n\n\n\n\n");
}

#[test]
fn quitar_modulos_de_prueba_en_medio_conserva_lo_de_despues() {
    let src = "fn a() {}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn b() {}\n}\n\npub fn vigilar() { otra(); }\n";
    let r = quitar_modulos_de_prueba(src);
    assert!(!r.contains("mod tests") && !r.contains("fn b"), "{:?}", r);
    assert!(r.contains("pub fn vigilar() { otra(); }"), "{:?}", r);
    // los numeros de linea del resto siguen valiendo
    assert_eq!(r.lines().count(), src.lines().count());
    assert_eq!(r.lines().position(|l| l.contains("vigilar")), src.lines().position(|l| l.contains("vigilar")));
}

#[test]
fn quitar_modulos_de_prueba_varios() {
    let src = "#[cfg(test)]\nmod uno { fn x() {} }\nfn a() {}\n#[cfg(test)]\n#[allow(dead_code)]\npub(crate) mod dos {\n    mod dentro { fn y() {} }\n}\nfn c() {}\n#[cfg(test)]\nfn ayuda() {}\n#[cfg(test)]\nmod externo;\n";
    let r = quitar_modulos_de_prueba(src);
    assert!(!r.contains("mod uno") && !r.contains("mod dos") && !r.contains("dentro"), "{:?}", r);
    assert!(r.contains("fn a() {}") && r.contains("fn c() {}"), "{:?}", r);
    // lo que no es un modulo con cuerpo se queda
    assert!(r.contains("#[cfg(test)]\nfn ayuda() {}") && r.contains("#[cfg(test)]\nmod externo;"), "{:?}", r);
    assert_eq!(r.lines().count(), src.lines().count());
}

#[test]
fn quitar_modulos_de_prueba_no_se_lia_con_llaves_en_cadenas_ni_comentarios() {
    let src = concat!(
        "#[cfg(test)]\nmod tests {\n",
        "    const A: &str = \"}}} \\\" }\";\n",
        "    const B: char = '}';\n",
        "    const C: char = '\\'';\n",
        "    const D: char = '\\u{7d}';\n",
        "    const E: &str = r#\"}\" }\"#;\n",
        "    // } en un comentario\n",
        "    /* } y /* } */ anidado */\n",
        "    fn f<'a>(x: &'a str) -> &'a str { x }\n",
        "}\nfn despues() {}\n",
    );
    let r = quitar_modulos_de_prueba(src);
    assert_eq!(r.trim(), "fn despues() {}", "{:?}", r);
}

#[test]
fn quitar_modulos_de_prueba_sin_cierre_no_quita_nada() {
    let src = "fn a() {}\n#[cfg(test)]\nmod tests {\n    fn b() {}\n";
    assert_eq!(quitar_modulos_de_prueba(src), src);
}

#[test]
fn detecta_una_descarga_puesta_despues_del_modulo_de_pruebas() {
    // antes solo se miraba lo anterior al primer `#[cfg(test)]\nmod tests`: esto pasaba sin verse
    let src = "fn a() {}\n#[cfg(test)]\nmod tests {\n    const U: &str = \"https://ejemplo.org/prueba\";\n}\n\nfn bajar() {\n    let _ = std::process::Command::new(\"curl\").arg(\"https://ejemplo.org/imagen.zip\");\n}\n";
    let h = hallazgos("ejemplo.rs", src);
    assert_eq!(h, vec!["src/ejemplo.rs:8: let _ = std::process::Command::new(\"curl\").arg(\"https://ejemplo.org/imagen.zip\");".to_string()]);
    // y lo que va dentro del modulo de pruebas no cuenta
    assert!(hallazgos("ejemplo.rs", "#[cfg(test)]\nmod tests {\n    const U: &str = \"https://ejemplo.org\";\n}\n").is_empty());
}
