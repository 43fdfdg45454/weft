//! Cliente adb propio: habla el protocolo de adbd directamente por vsock (CID del invitado, puerto 5555), sin el
//! servidor ni el binario de Google. Un hilo de conexion por orden; un solo servicio abierto a la vez.
//!
//! Protocolo (system/core/adb/protocol.txt): mensajes de 24 bytes (orden, arg0, arg1, longitud, suma de los datos, orden
//! ^ 0xffffffff; todo en little-endian) seguidos de los datos. CNXN abre la sesion, OPEN abre un servicio ("shell:...",
//! "sync:", ...), el invitado contesta OKAY y cada WRTE se confirma con OKAY; CLSE cierra.
//!
//! Servicios usados: `shell,v2,raw:` (salida y errores separados, con codigo de salida; si adbd no ofrece shell_v2,
//! `shell:` sin codigo de salida), `sync:` (STAT, LIST, SEND, RECV; version 1, sin v2), `exec:` (instalacion en flujo),
//! `root:`, `reboot:`, `remount:` y `disable-verity:`.
//!
//! Autenticacion: Cuttlefish userdebug (ro.adb.secure=0) acepta la conexion sin firma. Si adbd pide AUTH (imagen con
//! ro.adb.secure=1), como el adb de Google: firma el testigo (AUTH SIGNATURE) con la clave RSA de 2048 bits de weft
//! (`adbkey` en la carpeta de configuracion, 0600, creada la primera vez que hace falta; ver rsa.rs); si adbd no la
//! conoce, vuelve a pedir y se le manda la clave publica (AUTH RSAPUBLICKEY) para que el usuario la acepte en la pantalla
//! de Android, y se espera su CNXN.

use std::collections::VecDeque;
use std::io::{Read, Write};
use crate::textos::{clave, tx, txf};

pub const A_CNXN: u32 = 0x4e58_4e43;
pub const A_AUTH: u32 = 0x4854_5541;
pub const A_OPEN: u32 = 0x4e45_504f;
pub const A_OKAY: u32 = 0x5941_4b4f;
pub const A_CLSE: u32 = 0x4553_4c43;
pub const A_WRTE: u32 = 0x4554_5257;
pub const A_STLS: u32 = 0x534c_5453;
/// Tipos de AUTH (arg0): testigo de adbd, firma del testigo, clave publica.
pub const AUTH_TOKEN: u32 = 1;
pub const AUTH_SIGNATURE: u32 = 2;
pub const AUTH_RSAPUBLICKEY: u32 = 3;

pub const VERSION: u32 = 0x0100_0001;
pub const MAXDATA: u32 = 1 << 20;
pub const PUERTO: u32 = 5555;
/// Tamano maximo de cualquier campo con longitud del protocolo sync (bloque DATA, nombre de una entrada de LIST, mensaje
/// de FAIL): 64 KiB, como el adbd real. Lo que anuncie mas es un flujo roto y se rechaza antes de reservar memoria.
pub const SYNC_MAX: usize = 64 * 1024;
/// Tope de seguridad para los datos de un mensaje recibido y para un paquete de shell v2.
const MAX_RECIBIDO: usize = 4 << 20;
const BANNER: &str = "host::features=shell_v2,cmd";

/// Suma (de bytes sin signo) de los datos, el campo data_check de la cabecera.
pub fn checksum(data: &[u8]) -> u32 {
    data.iter().fold(0u32, |a, b| a.wrapping_add(*b as u32))
}

/// Mensaje completo: cabecera de 24 bytes mas datos.
pub fn encode(cmd: u32, arg0: u32, arg1: u32, data: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(24 + data.len());
    for x in [cmd, arg0, arg1, data.len() as u32, checksum(data), cmd ^ 0xffff_ffff] {
        v.extend_from_slice(&x.to_le_bytes());
    }
    v.extend_from_slice(data);
    v
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub struct Header {
    pub cmd: u32,
    pub arg0: u32,
    pub arg1: u32,
    pub len: u32,
    pub check: u32,
}

pub fn decode_header(b: &[u8; 24]) -> Result<Header, String> {
    let w = |i: usize| u32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
    let h = Header { cmd: w(0), arg0: w(1), arg1: w(2), len: w(3), check: w(4) };
    if w(5) != h.cmd ^ 0xffff_ffff {
        return Err(txf!("adb.cabecera_adb_no_valida_magic_para_la", format!("{:08x}", w(5)), format!("{:08x}", h.cmd)));
    }
    Ok(h)
}

/// Nombre legible de una orden, para los mensajes de error.
pub fn nombre(cmd: u32) -> String {
    let b = cmd.to_le_bytes();
    if b.iter().all(|c| c.is_ascii_uppercase()) {
        String::from_utf8_lossy(&b).into_owned()
    } else {
        format!("{:08x}", cmd)
    }
}

/// Banner de CNXN del invitado: "device::ro.product.name=...;features=a,b" -> lista de funciones.
pub fn parse_features(banner: &str) -> Vec<String> {
    let b = banner.trim_end_matches('\0');
    match b.split(';').find_map(|p| p.strip_prefix("features=")) {
        Some(f) => f.split(',').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect(),
        None => Vec::new(),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// shell v2: paquetes dentro del flujo (1 byte de id, 4 de longitud little-endian, datos)

// el id 0 es la entrada estandar: weft no la manda (las ordenes van sin entrada, como `adb shell < /dev/null`)
pub const SH_STDOUT: u8 = 1;
pub const SH_STDERR: u8 = 2;
pub const SH_EXIT: u8 = 3;
pub const SH_CLOSE_STDIN: u8 = 4;

pub fn shell_packet(id: u8, data: &[u8]) -> Vec<u8> {
    let mut v = vec![id];
    v.extend_from_slice(&(data.len() as u32).to_le_bytes());
    v.extend_from_slice(data);
    v
}

/// Saca de `buf` todos los paquetes shell v2 completos; deja lo que quede a medias. Err si un paquete anuncia mas de
/// MAX_RECIBIDO bytes: un flujo roto, y esperar a completarlo acumularia en memoria todo lo que llegue.
pub fn parse_shell_packets(buf: &mut Vec<u8>) -> Result<Vec<(u8, Vec<u8>)>, String> {
    let mut out = Vec::new();
    let mut pos = 0;
    while buf.len() - pos >= 5 {
        let len = u32::from_le_bytes([buf[pos + 1], buf[pos + 2], buf[pos + 3], buf[pos + 4]]) as usize;
        if len > MAX_RECIBIDO {
            return Err(txf!("adb.paquete_de_shell_demasiado_grande_en_la", len, MAX_RECIBIDO));
        }
        if buf.len() - pos - 5 < len {
            break;
        }
        out.push((buf[pos], buf[pos + 5..pos + 5 + len].to_vec()));
        pos += 5 + len;
    }
    buf.drain(..pos);
    Ok(out)
}

// ---------------------------------------------------------------------------------------------------------------
// protocolo sync

pub fn sync_req(id: &[u8; 4], arg: &[u8]) -> Vec<u8> {
    let mut v = id.to_vec();
    v.extend_from_slice(&(arg.len() as u32).to_le_bytes());
    v.extend_from_slice(arg);
    v
}

/// Trozo DATA (hasta SYNC_MAX bytes) del protocolo sync.
pub fn sync_data(chunk: &[u8]) -> Vec<u8> {
    sync_req(b"DATA", chunk)
}

pub fn sync_done(mtime: u32) -> Vec<u8> {
    let mut v = b"DONE".to_vec();
    v.extend_from_slice(&mtime.to_le_bytes());
    v
}

/// Entrada de un directorio remoto (LIST). Solo la usan las pruebas (ninguna orden lista carpetas del invitado).
#[cfg(test)]
#[derive(Debug, PartialEq, Clone)]
pub struct Dent {
    pub mode: u32,
    pub size: u32,
    pub mtime: u32,
    pub name: String,
}

pub fn es_dir(mode: u32) -> bool {
    mode & 0o170000 == 0o040000
}

fn u32le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

// ---------------------------------------------------------------------------------------------------------------
// conexion

pub struct Adb<T: Read + Write> {
    io: T,
    pub maxdata: usize,
    pub features: Vec<String>,
    pub banner: String,
    next_id: u32,
    cur: Option<(u32, u32)>,
    queue: VecDeque<Vec<u8>>,
    rbuf: Vec<u8>,
    rpos: usize,
    closed: bool,
    /// servicios que adbd acepto (OKAY a un OPEN) en esta conexion: distingue una orden que llego a Android de una que se
    /// quedo en la puerta (ver `aceptados`)
    aceptados: u32,
}

fn io_err(what: &str, e: std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe => txf!("adb.adbd_cerro_la_conexion", what),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => txf!("adb.adbd_no_respondio_a_tiempo", what),
        _ => format!("{}: {}", what, e),
    }
}

impl<T: Read + Write> Adb<T> {
    /// Abre la sesion (CNXN). Con adbd sin autenticacion contesta CNXN directamente; si pide AUTH se firma con la clave de
    /// weft (ver `handshake_con`), que se lee o se crea en la carpeta de configuracion solo entonces.
    pub fn handshake(io: T) -> Result<Adb<T>, String> {
        Self::handshake_con(io, &|| {
            let ruta = crate::rsa::ruta_clave();
            let (c, nueva) = crate::rsa::cargar_o_crear(&ruta)?;
            if nueva {
                eprintln!("{}", crate::textos::elige(tx!("adb.adb_se_creo_la_clave_de_depuracion_de"), &txf!("adb.adb_clave_rsa_nueva_de_en", ruta.display())));
            }
            Ok(c)
        })
    }

    /// `handshake` con la clave que da `clave` (se llama solo si adbd pide AUTH). Como el adb de Google: al primer testigo
    /// (AUTH TOKEN) se manda su firma (AUTH SIGNATURE); si adbd no conoce la clave manda otro testigo, y entonces se le
    /// manda la clave publica (AUTH RSAPUBLICKEY) y se espera a que el usuario la acepte en Android (llega CNXN) hasta el
    /// limite de lectura de la conexion.
    pub fn handshake_con(io: T, clave: &dyn Fn() -> Result<crate::rsa::Clave, String>) -> Result<Adb<T>, String> {
        let mut a = Adb { io, maxdata: 4096, features: Vec::new(), banner: String::new(), next_id: 1, cur: None, queue: VecDeque::new(), rbuf: Vec::new(), rpos: 0, closed: false, aceptados: 0 };
        a.send(A_CNXN, VERSION, MAXDATA, format!("{}\0", BANNER).as_bytes())?;
        let mut c: Option<crate::rsa::Clave> = None;
        let (mut firmado, mut publica) = (false, false);
        loop {
            let (h, d) = a.recv().map_err(|e| {
                if publica {
                    crate::textos::elige(
                        &txf!("adb.android_no_acepto_la_clave_de_depuracion", e),
                        &txf!("adb.adbd_no_acepto_la_clave_de_a_tiempo", e, format!("{:?}", crate::rsa::COMENTARIO)),
                    )
                } else {
                    e
                }
            })?;
            match h.cmd {
                A_CNXN => {
                    a.maxdata = if h.arg1 == 0 { 4096 } else { (h.arg1 as usize).min(MAXDATA as usize) };
                    a.banner = String::from_utf8_lossy(&d).trim_end_matches('\0').to_string();
                    a.features = parse_features(&a.banner);
                    return Ok(a);
                }
                // con la clave publica ya mandada, adbd espera al usuario; un testigo nuevo no cambia nada
                A_AUTH if h.arg0 == AUTH_TOKEN && publica => {}
                A_AUTH if h.arg0 == AUTH_TOKEN => {
                    if c.is_none() {
                        c = Some(clave().map_err(|e| txf!("adb.adbd_exige_autenticacion_rsa_y_no_se", e))?);
                    }
                    let k = c.as_ref().unwrap();
                    if !firmado {
                        let f = k.firmar_sha1(&d).map_err(|e| txf!("adb.adbd_exige_autenticacion_rsa", e))?;
                        a.send(A_AUTH, AUTH_SIGNATURE, 0, &f)?;
                        firmado = true;
                    } else {
                        // adbd no conoce la clave: la publica, para que el usuario la acepte en el aparato
                        let mut p = k.publica_android(crate::rsa::COMENTARIO).into_bytes();
                        p.push(0);
                        eprintln!(
                            "{}",
                            crate::textos::elige(tx!("adb.adb_acepta_la_depuracion_de_esta"), tx!("adb.adb_adbd_no_conoce_la_clave_de_acepta"))
                        );
                        a.send(A_AUTH, AUTH_RSAPUBLICKEY, 0, &p)?;
                        publica = true;
                    }
                }
                A_AUTH => return Err(txf!("adb.adbd_mando_un_auth_de_tipo_desconocido", h.arg0)),
                A_STLS => return Err(tx!("adb.adbd_pide_tls_adb_inalambrico_no").into()),
                c => return Err(txf!("adb.respuesta_inesperada_de_adbd_al_conectar", nombre(c))),
            }
        }
    }

    pub fn has(&self, f: &str) -> bool {
        self.features.iter().any(|x| x == f)
    }

    /// Cuantos servicios acepto adbd en esta conexion. Tras un error de una orden, dice si la orden llego a ejecutarse (adbd
    /// la acepto y luego se corto la conexion) o si ni siquiera se acepto (adbd no contesto al OPEN o lo rechazo).
    pub fn aceptados(&self) -> u32 {
        self.aceptados
    }

    fn send(&mut self, cmd: u32, a0: u32, a1: u32, data: &[u8]) -> Result<(), String> {
        let m = encode(cmd, a0, a1, data);
        self.io.write_all(&m).and_then(|_| self.io.flush()).map_err(|e| io_err("escritura", e))
    }

    fn recv(&mut self) -> Result<(Header, Vec<u8>), String> {
        let mut h = [0u8; 24];
        self.io.read_exact(&mut h).map_err(|e| io_err("lectura", e))?;
        let hd = decode_header(&h)?;
        if hd.len as usize > MAX_RECIBIDO {
            return Err(txf!("adb.mensaje_adb_demasiado_grande_bytes", hd.len));
        }
        let mut d = vec![0u8; hd.len as usize];
        self.io.read_exact(&mut d).map_err(|e| io_err("lectura", e))?;
        Ok((hd, d))
    }

    /// Abre un servicio. Solo uno a la vez: hay que cerrar el anterior.
    pub fn open(&mut self, service: &str) -> Result<(), String> {
        if self.cur.is_some() {
            return Err(tx!("adb.ya_hay_un_servicio_abierto_en_esta").into());
        }
        let local = self.next_id;
        self.next_id += 1;
        self.queue.clear();
        self.rbuf.clear();
        self.rpos = 0;
        self.closed = false;
        self.send(A_OPEN, local, 0, format!("{}\0", service).as_bytes())?;
        loop {
            let (h, _) = self.recv()?;
            match h.cmd {
                A_OKAY if h.arg1 == local => {
                    self.cur = Some((local, h.arg0));
                    self.aceptados += 1;
                    return Ok(());
                }
                A_CLSE if h.arg1 == local => return Err(txf!("adb.adbd_rechazo_el_servicio", format!("{:?}", service.split(':').next().unwrap_or(service)))),
                _ => {}
            }
        }
    }

    /// Siguiente trozo de datos del servicio; None cuando adbd cierra.
    fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, String> {
        if let Some(d) = self.queue.pop_front() {
            return Ok(Some(d));
        }
        loop {
            let Some((local, remote)) = self.cur.filter(|_| !self.closed) else { return Ok(None) };
            let (h, d) = self.recv()?;
            if h.arg1 != local {
                continue; // resto de un flujo anterior (p. ej. el CLSE con que adbd contesta al nuestro)
            }
            match h.cmd {
                A_WRTE => {
                    self.send(A_OKAY, local, remote, &[])?;
                    if !d.is_empty() {
                        return Ok(Some(d));
                    }
                }
                A_CLSE => {
                    self.closed = true;
                    self.cur = None;
                    let _ = self.send(A_CLSE, local, h.arg0, &[]);
                    return Ok(None);
                }
                _ => {}
            }
        }
    }

    pub fn read_some(&mut self) -> Result<Option<Vec<u8>>, String> {
        if self.rpos < self.rbuf.len() {
            let r = self.rbuf[self.rpos..].to_vec();
            self.rpos = self.rbuf.len();
            return Ok(Some(r));
        }
        self.next_chunk()
    }

    pub fn read_exact(&mut self, n: usize) -> Result<Vec<u8>, String> {
        // la capacidad crece con lo que llega: un `n` absurdo no reserva nada por adelantado
        let mut out = Vec::with_capacity(n.min(SYNC_MAX));
        while out.len() < n {
            if self.rpos >= self.rbuf.len() {
                match self.next_chunk()? {
                    Some(d) => {
                        self.rbuf = d;
                        self.rpos = 0;
                    }
                    None => return Err(tx!("adb.adbd_cerro_el_servicio_antes_de_terminar").into()),
                }
            }
            let take = (n - out.len()).min(self.rbuf.len() - self.rpos);
            out.extend_from_slice(&self.rbuf[self.rpos..self.rpos + take]);
            self.rpos += take;
        }
        Ok(out)
    }

    pub fn read_to_end(&mut self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        while let Some(d) = self.read_some()? {
            out.extend_from_slice(&d);
        }
        Ok(out)
    }

    /// Escribe en el servicio (trozos de a lo sumo `maxdata`, cada uno confirmado con OKAY).
    pub fn write(&mut self, data: &[u8]) -> Result<(), String> {
        for chunk in data.chunks(self.maxdata.max(1)) {
            let Some((local, remote)) = self.cur.filter(|_| !self.closed) else { return Err(tx!("adb.el_servicio_ya_esta_cerrado").into()) };
            self.send(A_WRTE, local, remote, chunk)?;
            loop {
                let (h, d) = self.recv()?;
                if h.arg1 != local {
                    continue;
                }
                match h.cmd {
                    A_OKAY => break,
                    A_WRTE => {
                        self.send(A_OKAY, local, remote, &[])?;
                        if !d.is_empty() {
                            self.queue.push_back(d);
                        }
                    }
                    A_CLSE => {
                        self.closed = true;
                        self.cur = None;
                        return Err(tx!("adb.adbd_cerro_el_servicio_antes_de_recibir").into());
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    pub fn close(&mut self) {
        if let Some((l, r)) = self.cur.take() {
            let _ = self.send(A_CLSE, l, r, &[]);
        }
        self.queue.clear();
        self.rbuf.clear();
        self.rpos = 0;
    }

    // -----------------------------------------------------------------------------------------------------------
    // servicios

    /// Ejecuta `cmd` en el shell del invitado. La salida va a `out` y los errores a `err` (separados con shell v2).
    /// Devuelve el codigo de salida (sin shell v2, adbd no lo informa y se devuelve 0). Si `out` se cierra (tuberia
    /// rota) se corta y se devuelve 141.
    pub fn shell(&mut self, cmd: &str, out: &mut dyn Write, err: &mut dyn Write) -> Result<i32, String> {
        let v2 = self.has("shell_v2");
        if v2 {
            self.open(&format!("shell,v2,raw:{}", cmd))?;
            // sin entrada estandar: como `adb shell < /dev/null`
            // (un comando muy corto puede terminar antes: entonces adbd ya cerro y el error no importa)
            let _ = self.write(&shell_packet(SH_CLOSE_STDIN, &[]));
        } else {
            self.open(&format!("shell:{}", cmd))?;
        }
        let mut code = 0;
        let mut buf: Vec<u8> = Vec::new();
        let mut rota = false;
        while let Some(d) = self.read_some()? {
            if !v2 {
                if out.write_all(&d).and_then(|_| out.flush()).is_err() {
                    rota = true;
                    break;
                }
                continue;
            }
            buf.extend_from_slice(&d);
            let paquetes = match parse_shell_packets(&mut buf) {
                Ok(p) => p,
                Err(e) => {
                    self.close();
                    return Err(e);
                }
            };
            for (id, p) in paquetes {
                let r = match id {
                    SH_STDOUT => out.write_all(&p).and_then(|_| out.flush()),
                    SH_STDERR => err.write_all(&p).and_then(|_| err.flush()),
                    SH_EXIT => {
                        code = p.first().copied().unwrap_or(0) as i32;
                        Ok(())
                    }
                    _ => Ok(()),
                };
                if r.is_err() {
                    rota = true;
                }
            }
            if rota {
                break;
            }
        }
        self.close();
        Ok(if rota { 141 } else { code })
    }

    /// `shell` capturando la salida y los errores como texto.
    pub fn shell_texto(&mut self, cmd: &str) -> Result<(i32, String, String), String> {
        let (mut o, mut e) = (Vec::new(), Vec::new());
        let c = self.shell(cmd, &mut o, &mut e)?;
        Ok((c, String::from_utf8_lossy(&o).into_owned(), String::from_utf8_lossy(&e).into_owned()))
    }

    /// Abre un servicio de texto (root:, reboot:, remount:...) y devuelve lo que contesta hasta que cierra.
    pub fn servicio_texto(&mut self, service: &str) -> Result<String, String> {
        self.open(service)?;
        let r = self.read_to_end();
        self.close();
        Ok(String::from_utf8_lossy(&r?).into_owned())
    }

    fn sync_abrir(&mut self) -> Result<(), String> {
        self.open("sync:")
    }

    fn sync_cerrar(&mut self) {
        let _ = self.write(&sync_req(b"QUIT", &[]));
        self.close();
    }

    /// Campo de `n` bytes del protocolo sync cuya longitud viene de adbd (`que`: lo que es, para el error). Err sin leer
    /// nada si pasa de SYNC_MAX.
    fn sync_campo(&mut self, n: usize, que: &str) -> Result<Vec<u8>, String> {
        if n > SYNC_MAX {
            return Err(txf!("adb.demasiado_largo_en_la_respuesta_de_adbd", que, n, SYNC_MAX));
        }
        self.read_exact(n)
    }

    /// FAIL del protocolo sync tras un id suelto (LIST): lee el mensaje de error.
    #[cfg(test)]
    fn sync_fail(&mut self) -> String {
        match self.read_exact(4).map(|l| u32le(&l, 0) as usize).and_then(|n| self.sync_campo(n, "mensaje de FAIL")) {
            Ok(m) => String::from_utf8_lossy(&m).into_owned(),
            Err(e) => e,
        }
    }

    fn stat_abierto(&mut self, path: &str) -> Result<Option<(u32, u32, u32)>, String> {
        self.write(&sync_req(b"STAT", path.as_bytes()))?;
        let r = self.read_exact(16)?;
        if &r[..4] != b"STAT" {
            return Err(txf!("adb.respuesta_sync_inesperada_a_stat", format!("{:?}", String::from_utf8_lossy(&r[..4]))));
        }
        let (mode, size, mtime) = (u32le(&r, 4), u32le(&r, 8), u32le(&r, 12));
        Ok(if mode == 0 && size == 0 && mtime == 0 { None } else { Some((mode, size, mtime)) })
    }

    /// (modo, tamano, fecha) de un archivo remoto, o None si no existe. Las ordenes usan `stat_abierto` dentro de su
    /// propia sesion sync; esta, con su sesion, solo la usan las pruebas.
    #[cfg(test)]
    pub fn stat(&mut self, path: &str) -> Result<Option<(u32, u32, u32)>, String> {
        self.sync_abrir()?;
        let r = self.stat_abierto(path);
        self.sync_cerrar();
        r
    }

    /// Entradas de una carpeta remota (LIST). Solo la usan las pruebas (comprueban con ella el adbd simulado y los limites
    /// del protocolo sync).
    #[cfg(test)]
    pub fn list(&mut self, path: &str) -> Result<Vec<Dent>, String> {
        self.sync_abrir()?;
        let r = (|| {
            self.write(&sync_req(b"LIST", path.as_bytes()))?;
            let mut v = Vec::new();
            loop {
                let id = self.read_exact(4)?;
                match &id[..] {
                    b"DENT" => {
                        let h = self.read_exact(16)?;
                        let name = self.sync_campo(u32le(&h, 12) as usize, tx!("adb.nombre_de_una_entrada_de_list"))?;
                        v.push(Dent { mode: u32le(&h, 0), size: u32le(&h, 4), mtime: u32le(&h, 8), name: String::from_utf8_lossy(&name).into_owned() });
                    }
                    b"DONE" => {
                        self.read_exact(16)?;
                        return Ok(v);
                    }
                    b"FAIL" => return Err(self.sync_fail()),
                    x => return Err(txf!("adb.respuesta_sync_inesperada_a_list", format!("{:?}", String::from_utf8_lossy(x)))),
                }
            }
        })();
        self.sync_cerrar();
        r
    }

    /// Copia un archivo local al invitado (adb push). Si `remoto` es un directorio, se copia dentro con el mismo
    /// nombre. Devuelve (ruta final, bytes).
    pub fn push(&mut self, local: &std::path::Path, remoto: &str) -> Result<(String, u64), String> {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(local).map_err(|e| format!("{}: {}", local.display(), e))?;
        if !meta.is_file() {
            return Err(txf!("adb.no_es_un_archivo", local.display()));
        }
        let mut f = std::fs::File::open(local).map_err(|e| format!("{}: {}", local.display(), e))?;
        let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as u32);
        let mode = 0o100000 | (meta.permissions().mode() & 0o777);
        self.sync_abrir()?;
        let r = (|| {
            let mut destino = remoto.to_string();
            if let Some((m, _, _)) = self.stat_abierto(remoto)? {
                if es_dir(m) {
                    let nombre = local.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    destino = format!("{}/{}", remoto.trim_end_matches('/'), nombre);
                }
            }
            self.write(&sync_req(b"SEND", format!("{},{}", destino, mode).as_bytes()))?;
            let (mut total, mut buf, mut chunk) = (0u64, Vec::new(), vec![0u8; SYNC_MAX]);
            loop {
                let n = f.read(&mut chunk).map_err(|e| format!("{}: {}", local.display(), e))?;
                if n == 0 {
                    break;
                }
                total += n as u64;
                buf.extend_from_slice(&sync_data(&chunk[..n]));
                if buf.len() >= self.maxdata {
                    self.write(&buf)?;
                    buf.clear();
                }
            }
            buf.extend_from_slice(&sync_done(mtime));
            self.write(&buf)?;
            let h = self.read_exact(8)?;
            match &h[..4] {
                b"OKAY" => Ok((destino, total)),
                b"FAIL" => {
                    let n = u32le(&h, 4) as usize;
                    Err(format!("{}: {}", remoto, String::from_utf8_lossy(&self.sync_campo(n, tx!("adb.mensaje_de_fail"))?)))
                }
                x => Err(txf!("adb.respuesta_sync_inesperada_a_send", format!("{:?}", String::from_utf8_lossy(x)))),
            }
        })();
        self.sync_cerrar();
        r
    }

    /// Copia un archivo del invitado al equipo (adb pull). Devuelve (ruta final, bytes).
    pub fn pull(&mut self, remoto: &str, local: &std::path::Path) -> Result<(std::path::PathBuf, u64), String> {
        let mut destino = local.to_path_buf();
        if destino.is_dir() {
            let n = remoto.trim_end_matches('/').rsplit('/').next().unwrap_or("archivo");
            destino = destino.join(n);
        }
        self.sync_abrir()?;
        let r = (|| {
            match self.stat_abierto(remoto)? {
                None => return Err(txf!("adb.no_existe_en_el_invitado", remoto)),
                Some((m, _, _)) if es_dir(m) => return Err(txf!("adb.es_un_directorio_pull_de_directorios_no", remoto)),
                _ => {}
            }
            let mut f = std::fs::File::create(&destino).map_err(|e| format!("{}: {}", destino.display(), e))?;
            self.write(&sync_req(b"RECV", remoto.as_bytes()))?;
            let mut total = 0u64;
            loop {
                let h = self.read_exact(8)?;
                let n = u32le(&h, 4) as usize;
                match &h[..4] {
                    b"DATA" => {
                        let d = self.sync_campo(n, tx!("adb.bloque_data"))?;
                        f.write_all(&d).map_err(|e| format!("{}: {}", destino.display(), e))?;
                        total += n as u64;
                    }
                    b"DONE" => return Ok((destino.clone(), total)),
                    b"FAIL" => return Err(format!("{}: {}", remoto, String::from_utf8_lossy(&self.sync_campo(n, tx!("adb.mensaje_de_fail"))?))),
                    x => return Err(txf!("adb.respuesta_sync_inesperada_a_recv", format!("{:?}", String::from_utf8_lossy(x)))),
                }
            }
        })();
        self.sync_cerrar();
        if r.is_err() {
            let _ = std::fs::remove_file(&destino);
        }
        r
    }

    /// Instala un APK. Con la funcion `cmd` de adbd: `exec:cmd package install -S TAMANO` y el archivo por el mismo
    /// flujo (como el adb moderno); si no, se copia a /data/local/tmp y se usa `pm install`. Devuelve la respuesta.
    pub fn install(&mut self, apk: &std::path::Path, flags: &[String]) -> Result<String, String> {
        let meta = std::fs::metadata(apk).map_err(|e| format!("{}: {}", apk.display(), e))?;
        let fl = flags.iter().map(|f| format!(" {}", f)).collect::<String>();
        if self.has("cmd") {
            let mut f = std::fs::File::open(apk).map_err(|e| format!("{}: {}", apk.display(), e))?;
            self.open(&format!("exec:cmd package 'install' -S {}{}", meta.len(), fl))?;
            let r = (|| {
                let mut buf = vec![0u8; self.maxdata];
                loop {
                    let n = f.read(&mut buf).map_err(|e| format!("{}: {}", apk.display(), e))?;
                    if n == 0 {
                        break;
                    }
                    self.write(&buf[..n])?;
                }
                self.read_to_end()
            })();
            self.close();
            return Ok(String::from_utf8_lossy(&r?).trim().to_string());
        }
        let tmp = format!("/data/local/tmp/weft-{}.apk", std::process::id());
        self.push(apk, &tmp)?;
        let (_, o, e) = self.shell_texto(&format!("pm install{} {}; rm -f {}", fl, tmp, tmp))?;
        Ok(format!("{}{}", o, e).trim().to_string())
    }
}

// ---------------------------------------------------------------------------------------------------------------
// vsock (sin crates: las llamadas del sistema por extern "C")

extern "C" {
    fn socket(domain: i32, ty: i32, proto: i32) -> i32;
    fn connect(fd: i32, addr: *const u8, len: u32) -> i32;
    fn setsockopt(fd: i32, level: i32, name: i32, val: *const u8, len: u32) -> i32;
    fn close(fd: i32) -> i32;
}

const AF_VSOCK: i32 = 40;
const SOCK_STREAM: i32 = 1;
const SOCK_CLOEXEC: i32 = 0o2000000;
const SOL_SOCKET: i32 = 1;
const SO_RCVTIMEO: i32 = 20;
const SO_SNDTIMEO: i32 = 21;

/// Direccion sockaddr_vm: familia (u16), reservado (u16), puerto (u32), CID (u32), flags y relleno (8 bytes).
pub fn sockaddr_vm(cid: u32, port: u32) -> [u8; 16] {
    let mut a = [0u8; 16];
    a[0..2].copy_from_slice(&(AF_VSOCK as u16).to_ne_bytes());
    a[4..8].copy_from_slice(&port.to_ne_bytes());
    a[8..12].copy_from_slice(&cid.to_ne_bytes());
    a
}

/// Abre un socket vsock hacia (cid, puerto). `secs` = tiempo maximo de conexion y de cada lectura y escritura (0: sin
/// limite).
pub fn vsock(cid: u32, port: u32, secs: u32) -> Result<std::fs::File, String> {
    use std::os::unix::io::FromRawFd;
    unsafe {
        let fd = socket(AF_VSOCK, SOCK_STREAM | SOCK_CLOEXEC, 0);
        if fd < 0 {
            return Err(txf!("adb.vsock_el_kernel_no_ofrece_af_vsock", std::io::Error::last_os_error()));
        }
        // timeval: segundos y microsegundos, dos i64
        let mut tv = [0u8; 16];
        tv[..8].copy_from_slice(&(secs as i64).to_ne_bytes());
        for opt in [SO_RCVTIMEO, SO_SNDTIMEO] {
            setsockopt(fd, SOL_SOCKET, opt, tv.as_ptr(), 16);
        }
        let a = sockaddr_vm(cid, port);
        if connect(fd, a.as_ptr(), 16) != 0 {
            let e = std::io::Error::last_os_error();
            close(fd);
            return Err(txf!("adb.no_se_pudo_conectar_a_vsock_la_maquina", cid, port, e));
        }
        Ok(std::fs::File::from_raw_fd(fd))
    }
}

/// Por donde se llega al adbd del invitado: vsock (CID) o TCP (un puerto de 127.0.0.1 que QEMU reenvia al 5555 del invitado).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    Vsock,
    Tcp(u16),
}

static VIA: std::sync::Mutex<Via> = std::sync::Mutex::new(Via::Vsock);

/// Fija la via del proceso (la leen las ordenes desde el estado de la maquina: ver adbcmd::via_del_estado).
pub fn fijar_via(v: Via) {
    *VIA.lock().unwrap() = v;
}

pub fn via_actual() -> Via {
    *VIA.lock().unwrap()
}

/// Abre un socket TCP a 127.0.0.1:puerto y lo entrega como archivo (el cliente adb es generico sobre el flujo).
pub fn tcp(puerto: u16, secs: u32) -> Result<std::fs::File, String> {
    use std::os::unix::io::{FromRawFd, IntoRawFd};
    let dir = std::net::SocketAddr::from(([127, 0, 0, 1], puerto));
    let lim = std::time::Duration::from_secs(if secs == 0 { 30 } else { secs as u64 });
    let s = std::net::TcpStream::connect_timeout(&dir, lim).map_err(|e| txf!("adb.no_se_pudo_conectar_a_tcp_127_0_0_1_la", puerto, e))?;
    if secs > 0 {
        let t = Some(std::time::Duration::from_secs(secs as u64));
        let _ = s.set_read_timeout(t);
        let _ = s.set_write_timeout(t);
    }
    let _ = s.set_nodelay(true);
    Ok(unsafe { std::fs::File::from_raw_fd(s.into_raw_fd()) })
}

/// ¿Dice el error de `Adb::handshake` (o de `conectar`) que adbd rechaza al adb propio (pide RSA o TLS)? Entonces
/// reintentar no sirve: con esta imagen no habra adb. Pura.
pub fn es_rechazo(e: &str) -> bool {
    use crate::textos::parte_fija;
    [parte_fija(clave!("adb.adbd_exige_autenticacion_rsa")), parte_fija(clave!("adb.adbd_exige_autenticacion_rsa_y_no_se")), tx!("adb.adbd_pide_tls_adb_inalambrico_no")].iter().any(|p| e.starts_with(p))
}

/// Conecta con el adbd del invitado por la via del proceso (vsock CID:5555 o TCP). `secs`: limite de conexion y de cada
/// lectura y escritura, es decir, de inactividad de adbd (0: sin limite; ver adbcmd::INACTIVIDAD_S).
pub fn conectar(cid: u32, secs: u32) -> Result<Adb<std::fs::File>, String> {
    match via_actual() {
        Via::Vsock => Adb::handshake(vsock(cid, PUERTO, secs)?),
        Via::Tcp(p) => Adb::handshake(tcp(p, secs)?),
    }
}

/// Ejecuta una orden de shell y devuelve (codigo, salida, errores).
pub fn shell_una_vez(cid: u32, cmd: &str, secs: u32) -> Result<(i32, String, String), String> {
    conectar(cid, secs)?.shell_texto(cmd)
}

/// Espera a que adbd responda y, con `boot`, a que sys.boot_completed valga 1. Devuelve los segundos que tardo.
pub fn esperar(cid: u32, limite_s: u64, boot: bool) -> Result<f64, String> {
    let t0 = std::time::Instant::now();
    loop {
        // lo ultimo que se vio, para el error si se acaba el tiempo
        let ultimo = match shell_una_vez(cid, if boot { "getprop sys.boot_completed" } else { "true" }, 8) {
            Ok((_, o, _)) if !boot || o.trim() == "1" => return Ok(t0.elapsed().as_secs_f64()),
            Ok((_, o, _)) => format!("sys.boot_completed={:?}", o.trim()),
            Err(e) => e,
        };
        if t0.elapsed().as_secs() >= limite_s {
            return Err(txf!("adb.adbd_no_estuvo_listo_en_s", limite_s, ultimo));
        }
        std::thread::sleep(std::time::Duration::from_millis(1000));
    }
}

/// Pasa adbd a root (como `adb root`) y espera a que vuelva. Devuelve lo que contesto adbd.
pub fn hacer_root(cid: u32) -> Result<String, String> {
    let es_root = |c: &mut Adb<std::fs::File>| c.shell_texto("id -u").map(|(_, o, _)| o.trim() == "0").unwrap_or(false);
    // adbd puede estar reiniciandose (acaba de pasar a root o de arrancar): se reintenta unos segundos
    let t_inicio = std::time::Instant::now();
    let mut c = loop {
        match conectar(cid, 15) {
            Ok(c) => break c,
            Err(e) if t_inicio.elapsed().as_secs() >= 20 => return Err(e),
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(700)),
        }
    };
    if es_root(&mut c) {
        return Ok(tx!("adb.adbd_ya_es_root").into());
    }
    let msg = c.servicio_texto("root:")?;
    drop(c);
    let t0 = std::time::Instant::now();
    loop {
        std::thread::sleep(std::time::Duration::from_millis(800));
        if let Ok(mut n) = conectar(cid, 8) {
            if es_root(&mut n) {
                return Ok(msg.trim().to_string());
            }
        }
        if t0.elapsed().as_secs() > 30 {
            return Err(txf!("adb.adbd_no_volvio_como_root", msg.trim()));
        }
    }
}

/// Abre `/vendor` y el resto de particiones del sistema para escritura (como `adb remount`; el cambio vive en el
/// disco de la maquina y se pierde al reiniciar Android: hay que repetirlo antes de cada escritura). Pasa adbd a root
/// antes y despues. Devuelve el texto que contesto adbd.
pub fn remontar(cid: u32) -> Result<String, String> {
    hacer_root(cid)?;
    let mut c = conectar(cid, 15)?;
    let t = if c.has("remount_shell") {
        let (code, o, e) = c.shell_texto("remount")?;
        let t = format!("{}{}", o, e);
        if code != 0 {
            return Err(txf!("adb.remount_fallo", t.trim()));
        }
        t
    } else {
        c.servicio_texto("remount:")?
    };
    let bajo = t.to_lowercase();
    if bajo.contains("failed") || bajo.contains("error") {
        return Err(txf!("adb.remount_fallo", t.trim()));
    }
    drop(c);
    std::thread::sleep(std::time::Duration::from_millis(500));
    esperar(cid, 60, false)?;
    hacer_root(cid)?;
    Ok(t)
}

/// Reinicia SurfaceFlinger dentro del invitado (`setprop ctl.restart surfaceflinger`, necesita root) y espera a que
/// vuelva con otro pid (reinicio blando del entorno grafico, ~20 s: las apps se cierran; la maquina no se reinicia).
/// Devuelve los segundos que tardo.
pub fn reiniciar_surfaceflinger(cid: u32) -> Result<f64, String> {
    hacer_root(cid)?;
    let pid = |cid| shell_una_vez(cid, "pidof surfaceflinger", 5).map(|(_, o, _)| o.trim().to_string()).unwrap_or_default();
    let antes = pid(cid);
    let t0 = std::time::Instant::now();
    shell_una_vez(cid, "setprop ctl.restart surfaceflinger", 10)?;
    std::thread::sleep(std::time::Duration::from_secs(4));
    loop {
        let ahora = pid(cid);
        if !ahora.is_empty() && ahora != antes {
            std::thread::sleep(std::time::Duration::from_secs(4));
            return Ok(t0.elapsed().as_secs_f64());
        }
        if t0.elapsed().as_secs() > 120 {
            return Err(tx!("adb.surfaceflinger_no_volvio_en_120_s").into());
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::os::unix::net::UnixStream;

    #[test]
    fn mensaje_y_cabecera() {
        let m = encode(A_OPEN, 7, 0, b"shell:ls\0");
        assert_eq!(m.len(), 24 + 9);
        assert_eq!(&m[..4], b"OPEN");
        let h: [u8; 24] = m[..24].try_into().unwrap();
        let d = decode_header(&h).unwrap();
        assert_eq!(d, Header { cmd: A_OPEN, arg0: 7, arg1: 0, len: 9, check: checksum(b"shell:ls\0") });
        assert_eq!(u32le(&m, 20), A_OPEN ^ 0xffff_ffff);
        // CNXN de referencia: version, maxdata
        let c = encode(A_CNXN, VERSION, MAXDATA, b"");
        assert_eq!(&c[..4], b"CNXN");
        assert_eq!(&c[4..8], &[1, 0, 0, 1]);
        assert_eq!(&c[8..12], &[0, 0, 0x10, 0]);
        // magic roto
        let mut malo = h;
        malo[20] ^= 1;
        assert!(decode_header(&malo).is_err());
    }

    #[test]
    fn suma_y_nombres() {
        assert_eq!(checksum(b""), 0);
        assert_eq!(checksum(&[255, 255, 2]), 512);
        assert_eq!(nombre(A_WRTE), "WRTE");
        assert_eq!(nombre(0x1234_5678), "12345678");
        assert_eq!(parse_features("device::ro.product.name=cf;ro.product.model=x;features=shell_v2,cmd,stat_v2\0"), vec!["shell_v2", "cmd", "stat_v2"]);
        assert!(parse_features("device::").is_empty());
    }

    #[test]
    fn paquetes_shell_v2() {
        let mut flujo = Vec::new();
        flujo.extend(shell_packet(SH_STDOUT, b"hola\n"));
        flujo.extend(shell_packet(SH_STDERR, b"mal"));
        flujo.extend(shell_packet(SH_EXIT, &[3]));
        // llega en dos trozos, cortado a mitad del segundo paquete
        let mut buf = flujo[..12].to_vec();
        let a = parse_shell_packets(&mut buf).unwrap();
        assert_eq!(a, vec![(SH_STDOUT, b"hola\n".to_vec())]);
        assert_eq!(buf.len(), 2);
        buf.extend_from_slice(&flujo[12..]);
        let b = parse_shell_packets(&mut buf).unwrap();
        assert_eq!(b, vec![(SH_STDERR, b"mal".to_vec()), (SH_EXIT, vec![3])]);
        assert!(buf.is_empty());
        assert_eq!(shell_packet(SH_CLOSE_STDIN, &[]), vec![4, 0, 0, 0, 0]);
    }

    #[test]
    fn mensajes_sync() {
        assert_eq!(sync_req(b"STAT", b"/a"), b"STAT\x02\0\0\0/a".to_vec());
        assert_eq!(sync_data(b"xyz"), b"DATA\x03\0\0\0xyz".to_vec());
        assert_eq!(sync_done(0x01020304), b"DONE\x04\x03\x02\x01".to_vec());
        assert!(es_dir(0o040755));
        assert!(!es_dir(0o100644));
    }

    #[test]
    fn direccion_vsock() {
        let a = sockaddr_vm(3, 5555);
        assert_eq!(u16::from_ne_bytes([a[0], a[1]]), 40);
        assert_eq!(u32::from_ne_bytes([a[4], a[5], a[6], a[7]]), 5555);
        assert_eq!(u32::from_ne_bytes([a[8], a[9], a[10], a[11]]), 3);
    }

    // ---- adbd simulado: contesta CNXN, shell v2, sync (STAT, LIST, SEND, RECV) sobre un sistema de archivos en memoria

    fn leer_msg(s: &mut UnixStream) -> Option<(Header, Vec<u8>)> {
        let mut h = [0u8; 24];
        s.read_exact(&mut h).ok()?;
        let hd = decode_header(&h).unwrap();
        let mut d = vec![0u8; hd.len as usize];
        s.read_exact(&mut d).ok()?;
        assert_eq!(hd.check, checksum(&d), "suma de datos");
        Some((hd, d))
    }

    /// Un WRTE del simulado hacia el cliente, esperando su OKAY.
    fn escribir(s: &mut UnixStream, local: u32, remote: u32, data: &[u8], maxdata: usize) {
        for c in data.chunks(maxdata) {
            s.write_all(&encode(A_WRTE, remote, local, c)).unwrap();
            // el cliente puede intercalar su propio WRTE (p. ej. cerrar la entrada estandar) antes de confirmar
            loop {
                let (h, _) = leer_msg(s).unwrap();
                match h.cmd {
                    A_OKAY => break,
                    A_WRTE => s.write_all(&encode(A_OKAY, remote, h.arg0, &[])).unwrap(),
                    c => panic!("mensaje inesperado {}", nombre(c)),
                }
            }
        }
    }

    fn adbd(mut s: UnixStream, maxdata: u32, features: &'static str, mut fs: HashMap<String, Vec<u8>>) -> std::thread::JoinHandle<HashMap<String, Vec<u8>>> {
        std::thread::spawn(move || {
            let mut svc = String::new();
            let mut sync_buf: Vec<u8> = Vec::new();
            // SEND en curso: (ruta, datos)
            let mut subiendo: Option<(String, Vec<u8>)> = None;
            let md = maxdata as usize;
            while let Some((h, d)) = leer_msg(&mut s) {
                match h.cmd {
                    A_CNXN => {
                        assert_eq!(h.arg0, VERSION);
                        assert_eq!(h.arg1, MAXDATA);
                        assert!(String::from_utf8_lossy(&d).starts_with("host::"));
                        s.write_all(&encode(A_CNXN, VERSION, maxdata, format!("device::ro.product.name=cf;features={}\0", features).as_bytes())).unwrap();
                    }
                    A_OPEN => {
                        svc = String::from_utf8_lossy(&d).trim_end_matches('\0').to_string();
                        s.write_all(&encode(A_OKAY, 99, h.arg0, &[])).unwrap();
                        if let Some(c) = svc.strip_prefix("shell,v2,raw:") {
                            let mut out = shell_packet(SH_STDOUT, format!("salida de {}\n", c).as_bytes());
                            out.extend(shell_packet(SH_STDERR, b"un error\n"));
                            out.extend(shell_packet(SH_EXIT, &[7]));
                            escribir(&mut s, h.arg0, 99, &out, md);
                            s.write_all(&encode(A_CLSE, 99, h.arg0, &[])).unwrap();
                        } else if let Some(c) = svc.strip_prefix("shell:") {
                            escribir(&mut s, h.arg0, 99, format!("antiguo {}\n", c).as_bytes(), md);
                            s.write_all(&encode(A_CLSE, 99, h.arg0, &[])).unwrap();
                        }
                    }
                    A_WRTE => {
                        s.write_all(&encode(A_OKAY, 99, h.arg0, &[])).unwrap();
                        if svc != "sync:" {
                            continue;
                        }
                        sync_buf.extend_from_slice(&d);
                        // procesa lo que haya llegado: datos de un SEND en curso o peticiones completas
                        loop {
                            if sync_buf.len() < 8 {
                                break;
                            }
                            let id: [u8; 4] = sync_buf[..4].try_into().unwrap();
                            let n = u32le(&sync_buf, 4) as usize;
                            if subiendo.is_some() {
                                match &id {
                                    b"DATA" => {
                                        if sync_buf.len() < 8 + n {
                                            break;
                                        }
                                        assert!(n <= SYNC_MAX);
                                        subiendo.as_mut().unwrap().1.extend_from_slice(&sync_buf[8..8 + n]);
                                        sync_buf.drain(..8 + n);
                                    }
                                    b"DONE" => {
                                        sync_buf.drain(..8);
                                        let (r, dat) = subiendo.take().unwrap();
                                        fs.insert(r, dat);
                                        escribir(&mut s, h.arg0, 99, &sync_req(b"OKAY", &[]), md);
                                    }
                                    x => panic!("sync inesperado durante SEND: {:?}", x),
                                }
                                continue;
                            }
                            if &id == b"QUIT" {
                                sync_buf.drain(..8);
                                continue;
                            }
                            if sync_buf.len() < 8 + n {
                                break;
                            }
                            let arg = sync_buf[8..8 + n].to_vec();
                            sync_buf.drain(..8 + n);
                            let ruta = String::from_utf8_lossy(&arg).into_owned();
                            let mut resp = Vec::new();
                            match &id {
                                b"STAT" => {
                                    let (m, sz) = match fs.get(&ruta) {
                                        Some(f) => (0o100644u32, f.len() as u32),
                                        None if ruta == "/dir" => (0o040755, 4096),
                                        None => (0, 0),
                                    };
                                    resp.extend_from_slice(b"STAT");
                                    for x in [m, sz, if m == 0 { 0 } else { 1234 }] {
                                        resp.extend_from_slice(&x.to_le_bytes());
                                    }
                                }
                                b"LIST" => {
                                    let mut nombres: Vec<_> = fs.iter().filter(|(k, _)| k.starts_with(&format!("{}/", ruta))).collect();
                                    nombres.sort_by_key(|(k, _)| (*k).clone());
                                    for (k, v) in nombres {
                                        let nm = &k[ruta.len() + 1..];
                                        resp.extend_from_slice(b"DENT");
                                        for x in [0o100644u32, v.len() as u32, 1234, nm.len() as u32] {
                                            resp.extend_from_slice(&x.to_le_bytes());
                                        }
                                        resp.extend_from_slice(nm.as_bytes());
                                    }
                                    resp.extend_from_slice(b"DONE");
                                    resp.extend_from_slice(&[0u8; 16]);
                                }
                                b"RECV" => match fs.get(&ruta) {
                                    Some(f) => {
                                        for c in f.chunks(SYNC_MAX) {
                                            resp.extend(sync_data(c));
                                        }
                                        resp.extend(sync_done(0));
                                    }
                                    None => resp.extend(sync_req(b"FAIL", b"No such file or directory")),
                                },
                                b"SEND" => {
                                    let (p, m) = ruta.rsplit_once(',').unwrap();
                                    assert_eq!(m.parse::<u32>().unwrap() & 0o170000, 0o100000);
                                    subiendo = Some((p.to_string(), Vec::new()));
                                }
                                _ => panic!("peticion sync desconocida {:?}", id),
                            }
                            if !resp.is_empty() {
                                escribir(&mut s, h.arg0, 99, &resp, md);
                            }
                        }
                    }
                    A_CLSE => {
                        s.write_all(&encode(A_CLSE, 99, h.arg0, &[])).ok();
                    }
                    _ => {}
                }
            }
            fs
        })
    }

    fn conexion(maxdata: u32, features: &'static str, fs: HashMap<String, Vec<u8>>) -> (Adb<UnixStream>, std::thread::JoinHandle<HashMap<String, Vec<u8>>>) {
        let (a, b) = UnixStream::pair().unwrap();
        let h = adbd(b, maxdata, features, fs);
        (Adb::handshake(a).unwrap(), h)
    }

    fn tmpdir(n: &str) -> std::path::PathBuf {
        let d = std::path::PathBuf::from(format!("{}/adb-prueba-{}-{}", std::env::var("TMPDIR").unwrap_or_else(|_| ".".into()), n, std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn conexion_y_shell_v2() {
        let (mut c, h) = conexion(4096, "shell_v2,cmd", HashMap::new());
        assert_eq!(c.maxdata, 4096);
        assert!(c.has("shell_v2") && c.has("cmd") && !c.has("stat_v2"));
        let (code, o, e) = c.shell_texto("getprop ro.x").unwrap();
        assert_eq!((code, o.as_str(), e.as_str()), (7, "salida de getprop ro.x\n", "un error\n"));
        // una segunda orden en la misma conexion
        let (code, o, _) = c.shell_texto("id").unwrap();
        assert_eq!((code, o.as_str()), (7, "salida de id\n"));
        drop(c);
        h.join().unwrap();
    }

    #[test]
    fn shell_sin_v2() {
        let (mut c, h) = conexion(1 << 20, "cmd", HashMap::new());
        let (code, o, e) = c.shell_texto("ls").unwrap();
        assert_eq!((code, o.as_str(), e.as_str()), (0, "antiguo ls\n", ""));
        drop(c);
        h.join().unwrap();
    }

    fn clave_prueba() -> crate::rsa::Clave {
        crate::rsa::Clave::de_pem(include_str!("../tests/datos/rsa_prueba.pem")).unwrap()
    }

    /// adbd simulado con autenticacion: manda un testigo, comprueba la firma con la clave publica que conoce (`conocida`);
    /// si no la conoce, manda otro testigo, espera la clave publica y la "acepta el usuario" (CNXN). Devuelve los tipos de
    /// AUTH que recibio.
    fn adbd_con_auth(mut b: UnixStream, conocida: bool) -> Vec<u32> {
        let k = clave_prueba();
        let mut tipos = Vec::new();
        let (h, _) = leer_msg(&mut b).unwrap();
        assert_eq!(h.cmd, A_CNXN);
        let testigo: Vec<u8> = (0u8..20).map(|i| i * 13 + 1).collect();
        b.write_all(&encode(A_AUTH, AUTH_TOKEN, 0, &testigo)).unwrap();
        let (h, firma) = leer_msg(&mut b).unwrap();
        assert_eq!((h.cmd, h.arg0, firma.len()), (A_AUTH, AUTH_SIGNATURE, 256));
        tipos.push(h.arg0);
        // RSA_verify(NID_sha1): el bloque abierto termina en DigestInfo(SHA-1) + testigo
        let em = k.abrir(&firma);
        assert!(em.starts_with(&[0, 1, 0xff]) && em.ends_with(&testigo));
        if !conocida {
            b.write_all(&encode(A_AUTH, AUTH_TOKEN, 0, &[9u8; 20])).unwrap();
            let (h, p) = leer_msg(&mut b).unwrap();
            assert_eq!((h.cmd, h.arg0), (A_AUTH, AUTH_RSAPUBLICKEY));
            tipos.push(h.arg0);
            assert_eq!(p.last(), Some(&0));
            assert_eq!(std::str::from_utf8(&p[..p.len() - 1]).unwrap(), k.publica_android("weft"));
            assert!(p.ends_with(b" weft\0"));
        }
        b.write_all(&encode(A_CNXN, VERSION, 4096, b"device::ro.product.name=x;features=shell_v2\0")).unwrap();
        tipos
    }

    #[test]
    fn autenticacion_rsa() {
        // clave ya aceptada en el aparato: firma y CNXN
        let (a, b) = UnixStream::pair().unwrap();
        let t = std::thread::spawn(move || adbd_con_auth(b, true));
        let c = Adb::handshake_con(a, &|| Ok(clave_prueba())).unwrap();
        assert!(c.has("shell_v2"));
        assert_eq!(t.join().unwrap(), vec![AUTH_SIGNATURE]);
        // clave desconocida: firma rechazada, clave publica y espera a que el usuario la acepte
        let (a, b) = UnixStream::pair().unwrap();
        let t = std::thread::spawn(move || adbd_con_auth(b, false));
        assert!(Adb::handshake_con(a, &|| Ok(clave_prueba())).is_ok());
        assert_eq!(t.join().unwrap(), vec![AUTH_SIGNATURE, AUTH_RSAPUBLICKEY]);
        // sin adbd que pida AUTH no se toca la clave
        let (a, mut b) = UnixStream::pair().unwrap();
        let t = std::thread::spawn(move || {
            leer_msg(&mut b).unwrap();
            b.write_all(&encode(A_CNXN, VERSION, 4096, b"device::\0")).unwrap();
        });
        assert!(Adb::handshake_con(a, &|| panic!("no hacia falta la clave")).is_ok());
        t.join().unwrap();
    }

    #[test]
    fn rechaza_auth() {
        // el usuario no acepta la clave: adbd no contesta y vence el limite de lectura, con un error que dice que hacer
        let (a, mut b) = UnixStream::pair().unwrap();
        a.set_read_timeout(Some(std::time::Duration::from_millis(300))).unwrap();
        let t = std::thread::spawn(move || {
            leer_msg(&mut b).unwrap();
            b.write_all(&encode(A_AUTH, AUTH_TOKEN, 0, &[1u8; 20])).unwrap();
            leer_msg(&mut b).unwrap();
            b.write_all(&encode(A_AUTH, AUTH_TOKEN, 0, &[2u8; 20])).unwrap();
            leer_msg(&mut b).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(600));
        });
        let e = Adb::handshake_con(a, &|| Ok(clave_prueba())).err().unwrap();
        assert!(e.contains("no acepto la clave de weft a tiempo") && e.contains("Permitir depuracion"), "{}", e);
        assert!(!es_rechazo(&e));
        t.join().unwrap();
        // sin clave utilizable: es un rechazo (reintentar no sirve), como TLS; no contestar a tiempo o cerrar no lo son
        let (a, mut b) = UnixStream::pair().unwrap();
        let t = std::thread::spawn(move || {
            leer_msg(&mut b).unwrap();
            b.write_all(&encode(A_AUTH, AUTH_TOKEN, 0, &[0u8; 20])).unwrap();
        });
        let e = Adb::handshake_con(a, &|| Err("sin permiso".into())).err().unwrap();
        assert!(e.contains("RSA") && e.contains("sin permiso"), "{}", e);
        t.join().unwrap();
        assert!(es_rechazo(&e));
        let (a, mut b) = UnixStream::pair().unwrap();
        let t = std::thread::spawn(move || {
            leer_msg(&mut b).unwrap();
            b.write_all(&encode(A_STLS, 1, 0, &[])).unwrap();
        });
        assert!(es_rechazo(&Adb::handshake(a).err().unwrap()));
        t.join().unwrap();
        for otro in ["lectura: adbd no respondio a tiempo", "lectura: adbd cerro la conexion", "respuesta inesperada de adbd al conectar: WRTE"] {
            assert!(!es_rechazo(otro), "{}", otro);
        }
    }

    #[test]
    fn push_pull_y_trozos() {
        // maxdata pequeno: obliga a partir escrituras y lecturas en muchos WRTE
        let (mut c, h) = conexion(4096, "shell_v2,cmd", HashMap::new());
        let d = tmpdir("pp");
        // 300 KB con contenido no repetitivo (varios bloques DATA de 64 KB)
        let datos: Vec<u8> = (0..300_000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        std::fs::write(d.join("origen.bin"), &datos).unwrap();
        let (dest, n) = c.push(&d.join("origen.bin"), "/sdcard/x.bin").unwrap();
        assert_eq!((dest.as_str(), n), ("/sdcard/x.bin", 300_000));
        // a un directorio: se anade el nombre
        let (dest2, _) = c.push(&d.join("origen.bin"), "/dir").unwrap();
        assert_eq!(dest2, "/dir/origen.bin");
        assert_eq!(c.stat("/sdcard/x.bin").unwrap().map(|s| s.1), Some(300_000));
        assert_eq!(c.stat("/nada").unwrap(), None);
        let (p, n) = c.pull("/sdcard/x.bin", &d.join("copia.bin")).unwrap();
        assert_eq!((p, n), (d.join("copia.bin"), 300_000));
        assert_eq!(std::fs::read(d.join("copia.bin")).unwrap(), datos);
        let e = c.pull("/no/existe", &d.join("z")).unwrap_err();
        assert!(e.contains("no existe"), "{}", e);
        let l = c.list("/dir").unwrap();
        assert_eq!(l, vec![Dent { mode: 0o100644, size: 300_000, mtime: 1234, name: "origen.bin".into() }]);
        drop(c);
        let fs = h.join().unwrap();
        assert_eq!(fs.get("/sdcard/x.bin"), Some(&datos));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn el_adb_habla_por_tcp_como_por_vsock() {
        // un adbd falso detras de un puerto local (lo que hace el reenvio de QEMU hacia el 5555 del invitado)
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let puerto = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut cab = [0u8; 24];
            std::io::Read::read_exact(&mut s, &mut cab).unwrap();
            assert_eq!(decode_header(&cab).unwrap().cmd, A_CNXN);
            let mut datos = vec![0u8; decode_header(&cab).unwrap().len as usize];
            std::io::Read::read_exact(&mut s, &mut datos).unwrap();
            s.write_all(&encode(A_CNXN, VERSION, 4096, b"device::ro.product.name=x;features=shell_v2,cmd")).unwrap();
        });
        let f = tcp(puerto, 5).unwrap();
        let c = Adb::handshake(f).unwrap();
        assert!(c.has("shell_v2") && c.has("cmd"));
        h.join().unwrap();
        // sin nadie escuchando: error claro que menciona el puerto
        let libre = { let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap(); l.local_addr().unwrap().port() };
        let e = tcp(libre, 2).err().unwrap();
        assert!(e.contains(&format!("127.0.0.1:{}", libre)), "{}", e);
        assert_eq!(Via::Tcp(1), Via::Tcp(1));
    }

    /// Un invitado colgado (adbd acepta la conexion y deja de contestar, al conectar o a mitad de una orden): con un limite
    /// de inactividad la orden falla con un error claro en vez de quedarse colgada para siempre.
    #[test]
    fn un_adbd_colgado_no_cuelga_la_orden() {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let puerto = l.local_addr().unwrap().port();
        let (fin_tx, fin_rx) = std::sync::mpsc::channel::<()>();
        let h = std::thread::spawn(move || {
            // 1) no contesta ni al CNXN
            let (mudo, _) = l.accept().unwrap();
            // 2) contesta al CNXN y al OPEN, y luego nada
            let (mut s, _) = l.accept().unwrap();
            let mut cab = [0u8; 24];
            for _ in 0..2 {
                std::io::Read::read_exact(&mut s, &mut cab).unwrap();
                let h = decode_header(&cab).unwrap();
                let mut d = vec![0u8; h.len as usize];
                std::io::Read::read_exact(&mut s, &mut d).unwrap();
                let r = if h.cmd == A_CNXN { encode(A_CNXN, VERSION, 4096, b"device::features=cmd") } else { encode(A_OKAY, 9, h.arg0, &[]) };
                s.write_all(&r).unwrap();
            }
            // las conexiones siguen abiertas hasta que la prueba termina
            let _ = fin_rx.recv();
            drop((mudo, s));
        });
        let t0 = std::time::Instant::now();
        let e = Adb::handshake(tcp(puerto, 1).unwrap()).err().unwrap();
        assert!(e.contains("no respondio a tiempo"), "{}", e);
        let mut c = Adb::handshake(tcp(puerto, 1).unwrap()).unwrap();
        assert_eq!(c.aceptados(), 0);
        let e = c.shell_texto("sleep 1000").unwrap_err();
        assert!(e.contains("no respondio a tiempo"), "{}", e);
        // la orden llego a adbd (acepto el OPEN) aunque luego no contestara
        assert_eq!(c.aceptados(), 1);
        assert!(t0.elapsed() < std::time::Duration::from_secs(10), "{:?}", t0.elapsed());
        fin_tx.send(()).unwrap();
        h.join().unwrap();
    }

    /// adbd roto a proposito: al servicio que se abre con `pide` o a la peticion sync que empieza por `pide` contesta
    /// `resp` tal cual; a STAT, "archivo normal de 10 bytes".
    fn adbd_crudo(mut s: UnixStream, pide: &'static [u8], resp: Vec<u8>) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            while let Some((h, d)) = leer_msg(&mut s) {
                let r = match h.cmd {
                    A_CNXN => {
                        s.write_all(&encode(A_CNXN, VERSION, 4096, b"device::ro.product.name=x;features=shell_v2,cmd\0")).unwrap();
                        continue;
                    }
                    A_OPEN => {
                        s.write_all(&encode(A_OKAY, 99, h.arg0, &[])).unwrap();
                        if !d.starts_with(pide) {
                            continue;
                        }
                        resp.clone()
                    }
                    A_WRTE => {
                        s.write_all(&encode(A_OKAY, 99, h.arg0, &[])).unwrap();
                        if d.starts_with(b"STAT") {
                            [&b"STAT"[..], &0o100644u32.to_le_bytes(), &10u32.to_le_bytes(), &1u32.to_le_bytes()].concat()
                        } else if d.starts_with(pide) {
                            resp.clone()
                        } else {
                            continue;
                        }
                    }
                    A_CLSE => {
                        s.write_all(&encode(A_CLSE, 99, h.arg0, &[])).ok();
                        continue;
                    }
                    _ => continue,
                };
                for c in r.chunks(4096) {
                    s.write_all(&encode(A_WRTE, 99, h.arg0, c)).unwrap();
                }
            }
        })
    }

    fn con_adbd_crudo(pide: &'static [u8], resp: Vec<u8>) -> (Adb<UnixStream>, std::thread::JoinHandle<()>) {
        let (a, b) = UnixStream::pair().unwrap();
        // si algo falla, error y no una prueba colgada
        a.set_read_timeout(Some(std::time::Duration::from_secs(20))).unwrap();
        let h = adbd_crudo(b, pide, resp);
        (Adb::handshake(a).unwrap(), h)
    }

    /// Las longitudes que manda adbd en el protocolo sync se comprueban contra SYNC_MAX antes de leer (y de reservar): un
    /// nombre de LIST, un mensaje de FAIL o un bloque DATA que anuncian de mas dan un error claro.
    #[test]
    fn longitudes_sync_acotadas() {
        let dent = |largo: u32| [&b"DENT"[..], &0o100644u32.to_le_bytes(), &1u32.to_le_bytes(), &2u32.to_le_bytes(), &largo.to_le_bytes()].concat();
        for largo in [0xFFFF_FFF0u32, SYNC_MAX as u32 + 1] {
            let (mut c, h) = con_adbd_crudo(b"LIST", dent(largo));
            let e = c.list("/sdcard").unwrap_err();
            assert!(e.contains("nombre de una entrada de LIST demasiado largo") && e.contains(&largo.to_string()), "{}", e);
            // la conexion sigue sirviendo para otra orden
            assert_eq!(c.stat("/x").unwrap(), Some((0o100644, 10, 1)));
            drop(c);
            h.join().unwrap();
        }
        // un nombre en el limite todavia se lee
        let mut justo = dent(SYNC_MAX as u32);
        justo.extend(vec![b'n'; SYNC_MAX]);
        justo.extend_from_slice(b"DONE");
        justo.extend_from_slice(&[0u8; 16]);
        let (mut c, h) = con_adbd_crudo(b"LIST", justo);
        let l = c.list("/sdcard").unwrap();
        assert_eq!((l.len(), l[0].name.len()), (1, SYNC_MAX));
        drop(c);
        h.join().unwrap();

        // FAIL: el mensaje entero (no cortado en 4096 bytes) o un error si anuncia de mas
        let largo: String = "x".repeat(5000);
        let (mut c, h) = con_adbd_crudo(b"LIST", sync_req(b"FAIL", largo.as_bytes()));
        assert_eq!(c.list("/sdcard").unwrap_err(), largo);
        drop(c);
        h.join().unwrap();
        let (mut c, h) = con_adbd_crudo(b"LIST", [&b"FAIL"[..], &u32::MAX.to_le_bytes()].concat());
        assert!(c.list("/sdcard").unwrap_err().contains("mensaje de FAIL demasiado largo"));
        drop(c);
        h.join().unwrap();

        // RECV: un bloque DATA de mas de 64 KiB
        let d = tmpdir("crudo");
        let (mut c, h) = con_adbd_crudo(b"RECV", [&b"DATA"[..], &(SYNC_MAX as u32 + 1).to_le_bytes()].concat());
        let e = c.pull("/sdcard/a", &d.join("a")).unwrap_err();
        assert!(e.contains("bloque DATA demasiado largo"), "{}", e);
        assert!(!d.join("a").exists());
        drop(c);
        h.join().unwrap();
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// Un paquete de shell v2 que anuncia mas de MAX_RECIBIDO bytes es un flujo roto: error en vez de acumular sin fin.
    #[test]
    fn paquete_shell_demasiado_grande() {
        let mut buf = shell_packet(SH_STDOUT, b"ok");
        buf.extend_from_slice(&[SH_STDOUT, 0xFF, 0xFF, 0xFF, 0xFF]);
        assert!(parse_shell_packets(&mut buf).unwrap_err().contains("demasiado grande"));
        // en el limite justo se espera al resto
        let mut buf = vec![SH_STDOUT];
        buf.extend_from_slice(&(MAX_RECIBIDO as u32).to_le_bytes());
        assert_eq!(parse_shell_packets(&mut buf).unwrap(), vec![]);
        assert_eq!(buf.len(), 5);
        // y por la conexion: la orden falla con ese error
        let (mut c, h) = con_adbd_crudo(b"shell", vec![SH_STDOUT, 0xFF, 0xFF, 0xFF, 0xFF]);
        let e = c.shell_texto("ls").unwrap_err();
        assert!(e.contains("paquete de shell demasiado grande"), "{}", e);
        drop(c);
        h.join().unwrap();
    }
}
