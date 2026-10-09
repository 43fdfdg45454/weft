//! Cliente QMP (protocolo de control de QEMU) sobre un socket Unix.

use crate::json::{self, V};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;
use crate::textos::{tx, txf};

pub struct Qmp {
    r: BufReader<UnixStream>,
    w: UnixStream,
    pub greeting: V,
}

impl Qmp {
    pub fn connect(path: &str) -> Result<Qmp, String> {
        let s = UnixStream::connect(path).map_err(|e| txf!("qmp.no_se_pudo_conectar_al_control_de_la", e))?;
        s.set_read_timeout(Some(Duration::from_secs(20))).ok();
        let w = s.try_clone().map_err(|e| e.to_string())?;
        let mut q = Qmp { r: BufReader::new(s), w, greeting: V::Null };
        q.greeting = q.read_msg()?;
        if q.greeting.get("QMP").is_none() {
            return Err(tx!("qmp.el_socket_no_respondio_con_el_saludo_qmp").into());
        }
        q.exec("qmp_capabilities", None)?;
        Ok(q)
    }

    fn read_msg(&mut self) -> Result<V, String> {
        let mut line = String::new();
        let n = self.r.read_line(&mut line).map_err(|e| txf!("qmp.lectura_qmp", e))?;
        if n == 0 {
            return Err(tx!("qmp.la_maquina_cerro_la_conexion_de_control").into());
        }
        json::parse(line.trim())
    }

    /// Espera sin limite de tiempo el siguiente mensaje (evento) de QEMU. Err cuando QEMU cierra la conexion.
    pub fn next_event(&mut self) -> Result<V, String> {
        self.r.get_ref().set_read_timeout(None).ok();
        self.read_msg()
    }

    /// Ejecuta un comando y devuelve el valor de `return`. Los eventos asincronos se descartan.
    pub fn exec(&mut self, cmd: &str, args: Option<V>) -> Result<V, String> {
        let mut pairs = vec![("execute", V::s(cmd))];
        if let Some(a) = args {
            pairs.push(("arguments", a));
        }
        let msg = V::obj(&pairs).dump();
        self.w.write_all(msg.as_bytes()).and_then(|_| self.w.write_all(b"\n")).map_err(|e| txf!("qmp.escritura_qmp", e))?;
        self.reply(cmd)
    }

    /// Como `exec`, pero el descriptor `fd` viaja junto al comando (lo usan `getfd` y `add-fd`).
    pub fn exec_fd(&mut self, cmd: &str, args: V, fd: std::os::unix::io::RawFd) -> Result<V, String> {
        let msg = format!("{}\n", V::obj(&[("execute", V::s(cmd)), ("arguments", args)]).dump());
        crate::dbus::send_with_fd(&self.w, msg.as_bytes(), fd).map_err(|e| txf!("qmp.escritura_qmp", e))?;
        self.reply(cmd)
    }

    fn reply(&mut self, cmd: &str) -> Result<V, String> {
        loop {
            let m = self.read_msg()?;
            if let Some(r) = m.get("return") {
                return Ok(r.clone());
            }
            if let Some(e) = m.get("error") {
                let d = e.get("desc").and_then(|d| d.as_str()).unwrap_or(tx!("qmp.error_sin_descripcion"));
                return Err(format!("{}: {}", cmd, d));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// Servidor QMP simulado: saludo, eventos intercalados, respuesta y error.
    #[test]
    fn dialogo_con_servidor_simulado() {
        let path = std::env::temp_dir().join(format!("weft-qmp-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let l = UnixListener::bind(&path).unwrap();
        let t = std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let mut w = s.try_clone().unwrap();
            let mut r = BufReader::new(s);
            w.write_all(b"{\"QMP\":{\"version\":{},\"capabilities\":[]}}\n").unwrap();
            let mut seen = Vec::new();
            for _ in 0..3 {
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let v = json::parse(line.trim()).unwrap();
                let c = v.get("execute").unwrap().as_str().unwrap().to_string();
                w.write_all(b"{\"event\":\"RESUME\",\"timestamp\":{}}\n").unwrap();
                if c == "malo" {
                    w.write_all(b"{\"error\":{\"class\":\"GenericError\",\"desc\":\"no existe\"}}\n").unwrap();
                } else if c == "query-status" {
                    w.write_all(b"{\"return\":{\"status\":\"running\",\"running\":true}}\n").unwrap();
                } else {
                    w.write_all(b"{\"return\":{}}\n").unwrap();
                }
                seen.push(c);
            }
            seen
        });
        let mut q = Qmp::connect(path.to_str().unwrap()).unwrap();
        let st = q.exec("query-status", None).unwrap();
        assert_eq!(st.get("status").unwrap().as_str(), Some("running"));
        assert_eq!(q.exec("malo", None).unwrap_err(), "malo: no existe");
        assert_eq!(t.join().unwrap(), vec!["qmp_capabilities", "query-status", "malo"]);
        let _ = std::fs::remove_file(&path);
    }
}
