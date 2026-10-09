//! D-Bus minimo para hablar con la pantalla D-Bus de QEMU (conexion directa entre pares) y, como extra, mandar una
//! notificacion de escritorio por el bus de sesion.
//!
//! Solo lo necesario para `-display dbus,p2p=yes`: autenticacion EXTERNAL como cliente con paso de descriptores,
//! mensajes en orden little-endian, los tipos basicos que usa la interfaz org.qemu.Display1 y el envio de un
//! descriptor de archivo junto a un mensaje (SCM_RIGHTS), que tambien usa el QMP para `getfd`. Para el bus de sesion:
//! la direccion de DBUS_SESSION_BUS_ADDRESS, `Hello` y una llamada con destino (ver `notificar`), y el selector de
//! archivos del portal de escritorio (ver `elegir`), que tambien funciona dentro de Flatpak.

use std::io::{Read, Write};
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use crate::textos::{tx, txf};

pub const METHOD_CALL: u8 = 1;
pub const METHOD_RETURN: u8 = 2;
pub const ERROR: u8 = 3;
/// Indicador de mensaje: no hace falta respuesta.
pub const NO_REPLY: u8 = 1;

/// Tamano maximo de mensaje aceptado (el de la especificacion: 128 MiB).
const MAX_MSG: usize = 128 << 20;

// ---------------------------------------------------------------------------------------------------------------
// paso de descriptores (SCM_RIGHTS)

#[repr(C)]
struct IoVec {
    base: *const u8,
    len: usize,
}

#[repr(C)]
struct MsgHdr {
    name: *mut u8,
    namelen: u32,
    iov: *const IoVec,
    iovlen: usize,
    control: *mut u8,
    controllen: usize,
    flags: i32,
}

extern "C" {
    fn sendmsg(fd: i32, msg: *const MsgHdr, flags: i32) -> isize;
    fn getuid() -> u32;
}

/// Escribe `data` completo por el socket; el descriptor `fd` viaja con el primer byte.
pub fn send_with_fd(sock: &UnixStream, data: &[u8], fd: RawFd) -> std::io::Result<()> {
    // cmsghdr (len: usize, level: i32, type: i32) + un int, alineado a 8: CMSG_SPACE(4) = 24, CMSG_LEN(4) = 20
    let mut cbuf = [0u8; 24];
    cbuf[0..8].copy_from_slice(&20usize.to_ne_bytes());
    cbuf[8..12].copy_from_slice(&1i32.to_ne_bytes()); // SOL_SOCKET
    cbuf[12..16].copy_from_slice(&1i32.to_ne_bytes()); // SCM_RIGHTS
    cbuf[16..20].copy_from_slice(&fd.to_ne_bytes());
    let iov = IoVec { base: data.as_ptr(), len: data.len() };
    let msg = MsgHdr { name: std::ptr::null_mut(), namelen: 0, iov: &iov, iovlen: 1, control: cbuf.as_mut_ptr(), controllen: cbuf.len(), flags: 0 };
    let n = unsafe { sendmsg(sock.as_raw_fd(), &msg, 0x4000 /* MSG_NOSIGNAL */) };
    if n < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut s = sock;
    s.write_all(&data[n as usize..])
}

// ---------------------------------------------------------------------------------------------------------------
// codificacion

/// Valor de cuerpo de mensaje. Solo los tipos que hacen falta.
#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    /// booleano ("b": un entero de 32 bits que vale 0 o 1)
    Bool(bool),
    /// entero de 16 bits sin signo ("q")
    U16(u16),
    /// entero de 32 bits con signo ("i")
    I32(i32),
    U32(u32),
    /// entero de 64 bits sin signo ("t")
    U64(u64),
    /// numero de coma flotante de 64 bits ("d")
    F64(f64),
    Str(String),
    /// descriptor de archivo: en el mensaje va el indice dentro de la lista de descriptores
    Fd(u32),
    /// diccionario a{sv} cuyos valores son arreglos de texto (as): el unico que hace falta (propiedad Interfaces)
    DictStrArr(Vec<(String, Vec<String>)>),
    /// arreglo de textos ("as")
    StrArr(Vec<String>),
    /// diccionario a{sv} con valores de cualquier tipo de esta lista (cada uno viaja como variante con su firma)
    DictVar(Vec<(String, Arg)>),
    /// variante ("v") con un valor de esta lista (de momento solo la mandan las pruebas, al simular el portal)
    #[cfg_attr(not(test), allow(dead_code))]
    Var(Box<Arg>),
}

impl Arg {
    fn sig(&self) -> &'static str {
        match self {
            Arg::Bool(_) => "b",
            Arg::U16(_) => "q",
            Arg::I32(_) => "i",
            Arg::U32(_) => "u",
            Arg::U64(_) => "t",
            Arg::F64(_) => "d",
            Arg::Str(_) => "s",
            Arg::Fd(_) => "h",
            Arg::DictStrArr(_) | Arg::DictVar(_) => "a{sv}",
            Arg::StrArr(_) => "as",
            Arg::Var(_) => "v",
        }
    }
}

#[derive(Default)]
struct W {
    v: Vec<u8>,
}

impl W {
    fn pad(&mut self, a: usize) {
        while self.v.len() % a != 0 {
            self.v.push(0);
        }
    }
    fn u8(&mut self, x: u8) {
        self.v.push(x);
    }
    fn u16(&mut self, x: u16) {
        self.pad(2);
        self.v.extend(x.to_le_bytes());
    }
    fn u32(&mut self, x: u32) {
        self.pad(4);
        self.v.extend(x.to_le_bytes());
    }
    fn u64(&mut self, x: u64) {
        self.pad(8);
        self.v.extend(x.to_le_bytes());
    }
    fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.v.extend(s.as_bytes());
        self.v.push(0);
    }
    fn sig(&mut self, s: &str) {
        self.v.push(s.len() as u8);
        self.v.extend(s.as_bytes());
        self.v.push(0);
    }
    /// arreglo: longitud (sin el relleno inicial del primer elemento) + elementos
    fn array(&mut self, elem_align: usize, f: impl FnOnce(&mut W)) {
        self.u32(0);
        let at = self.v.len() - 4;
        self.pad(elem_align);
        let start = self.v.len();
        f(self);
        let n = (self.v.len() - start) as u32;
        self.v[at..at + 4].copy_from_slice(&n.to_le_bytes());
    }
    fn str_arr(&mut self, l: &[String]) {
        self.array(4, |w| {
            for s in l {
                w.str(s);
            }
        });
    }
    fn arg(&mut self, a: &Arg) {
        match a {
            Arg::Bool(b) => self.u32(*b as u32),
            Arg::U16(x) => self.u16(*x),
            Arg::I32(x) => self.u32(*x as u32),
            Arg::U32(x) | Arg::Fd(x) => self.u32(*x),
            Arg::U64(x) => self.u64(*x),
            Arg::F64(x) => self.u64(x.to_bits()),
            Arg::Str(s) => self.str(s),
            Arg::DictStrArr(d) => self.array(8, |w| {
                for (k, l) in d {
                    w.pad(8);
                    w.str(k);
                    w.sig("as");
                    w.str_arr(l);
                }
            }),
            Arg::StrArr(l) => self.str_arr(l),
            Arg::DictVar(d) => self.array(8, |w| {
                for (k, v) in d {
                    w.pad(8);
                    w.str(k);
                    w.sig(v.sig());
                    w.arg(v);
                }
            }),
            Arg::Var(v) => {
                self.sig(v.sig());
                self.arg(v);
            }
        }
    }
}

/// Mensaje D-Bus (cabecera ya interpretada y cuerpo crudo).
#[derive(Clone, Debug, Default)]
pub struct Msg {
    pub kind: u8,
    pub flags: u8,
    pub serial: u32,
    pub path: String,
    pub iface: String,
    pub member: String,
    pub error: String,
    /// destino (nombre en el bus); vacio en una conexion directa entre pares
    pub dest: String,
    pub reply_serial: u32,
    pub signature: String,
    pub unix_fds: u32,
    pub body: Vec<u8>,
}

impl Msg {
    pub fn call(path: &str, iface: &str, member: &str) -> Msg {
        Msg { kind: METHOD_CALL, path: path.into(), iface: iface.into(), member: member.into(), ..Default::default() }
    }

    /// Respuesta vacia (o con argumentos) a una llamada recibida.
    pub fn reply_to(call: &Msg) -> Msg {
        Msg { kind: METHOD_RETURN, reply_serial: call.serial, ..Default::default() }
    }

    pub fn error_to(call: &Msg, name: &str, text: &str) -> Msg {
        let mut m = Msg { kind: ERROR, reply_serial: call.serial, error: name.into(), ..Default::default() };
        m.set_body(&[Arg::Str(text.into())]);
        m
    }

    pub fn set_body(&mut self, args: &[Arg]) {
        let mut w = W::default();
        let mut sig = String::new();
        for a in args {
            sig.push_str(a.sig());
            w.arg(a);
        }
        self.signature = sig;
        self.unix_fds = args.iter().filter(|a| matches!(a, Arg::Fd(_))).count() as u32;
        self.body = w.v;
    }

    pub fn wants_reply(&self) -> bool {
        self.kind == METHOD_CALL && self.flags & NO_REPLY == 0
    }

    /// Mensaje completo listo para escribir, con el numero de serie dado.
    pub fn encode(&self, serial: u32) -> Vec<u8> {
        let mut w = W::default();
        w.u8(b'l');
        w.u8(self.kind);
        w.u8(self.flags);
        w.u8(1);
        w.u32(self.body.len() as u32);
        w.u32(serial);
        let field = |w: &mut W, code: u8, sig: &str, f: &dyn Fn(&mut W)| {
            w.pad(8);
            w.u8(code);
            w.sig(sig);
            f(w);
        };
        w.array(8, |w| {
            if !self.path.is_empty() {
                field(w, 1, "o", &|w| w.str(&self.path));
            }
            if !self.iface.is_empty() {
                field(w, 2, "s", &|w| w.str(&self.iface));
            }
            if !self.member.is_empty() {
                field(w, 3, "s", &|w| w.str(&self.member));
            }
            if !self.error.is_empty() {
                field(w, 4, "s", &|w| w.str(&self.error));
            }
            if self.reply_serial != 0 {
                field(w, 5, "u", &|w| w.u32(self.reply_serial));
            }
            if !self.dest.is_empty() {
                field(w, 6, "s", &|w| w.str(&self.dest));
            }
            if !self.signature.is_empty() {
                field(w, 8, "g", &|w| w.sig(&self.signature));
            }
            if self.unix_fds != 0 {
                field(w, 9, "u", &|w| w.u32(self.unix_fds));
            }
        });
        w.pad(8);
        w.v.extend(&self.body);
        w.v
    }
}

// ---------------------------------------------------------------------------------------------------------------
// decodificacion

/// Lector de un cuerpo (o cabecera) con alineacion relativa al inicio del bloque.
pub struct R<'a> {
    b: &'a [u8],
    pub pos: usize,
}

impl<'a> R<'a> {
    pub fn new(b: &'a [u8]) -> R<'a> {
        R { b, pos: 0 }
    }
    fn need(&self, n: usize) -> Result<(), String> {
        if self.pos + n > self.b.len() {
            return Err(tx!("dbus.d_bus_mensaje_truncado").into());
        }
        Ok(())
    }
    fn align(&mut self, a: usize) -> Result<(), String> {
        let p = self.pos.div_ceil(a) * a;
        if p > self.b.len() {
            return Err(tx!("dbus.d_bus_mensaje_truncado").into());
        }
        self.pos = p;
        Ok(())
    }
    pub fn u8(&mut self) -> Result<u8, String> {
        self.need(1)?;
        self.pos += 1;
        Ok(self.b[self.pos - 1])
    }
    pub fn u32(&mut self) -> Result<u32, String> {
        self.align(4)?;
        self.need(4)?;
        let v = u32::from_le_bytes(self.b[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        Ok(v)
    }
    #[cfg(test)]
    pub fn u64(&mut self) -> Result<u64, String> {
        self.align(8)?;
        self.need(8)?;
        let v = u64::from_le_bytes(self.b[self.pos..self.pos + 8].try_into().unwrap());
        self.pos += 8;
        Ok(v)
    }
    #[cfg(test)]
    pub fn f64(&mut self) -> Result<f64, String> {
        self.u64().map(f64::from_bits)
    }
    pub fn i32(&mut self) -> Result<i32, String> {
        self.u32().map(|v| v as i32)
    }
    pub fn str(&mut self) -> Result<String, String> {
        let n = self.u32()? as usize;
        self.need(n + 1)?;
        let s = String::from_utf8_lossy(&self.b[self.pos..self.pos + n]).into_owned();
        self.pos += n + 1;
        Ok(s)
    }
    pub fn sig(&mut self) -> Result<String, String> {
        let n = self.u8()? as usize;
        self.need(n + 1)?;
        let s = String::from_utf8_lossy(&self.b[self.pos..self.pos + n]).into_owned();
        self.pos += n + 1;
        Ok(s)
    }
    /// arreglo de bytes (ay): devuelve el rango dentro del bloque, sin copiar
    pub fn bytes(&mut self) -> Result<std::ops::Range<usize>, String> {
        let n = self.u32()? as usize;
        self.need(n)?;
        self.pos += n;
        Ok(self.pos - n..self.pos)
    }
}

/// Primer byte de un mensaje: su orden de bytes. Solo se admite 'l' (little-endian); 'B' (big-endian) es D-Bus valido
/// pero no esta implementado, y cualquier otro valor es un flujo roto. Pura.
fn orden_de_bytes(b: u8) -> Result<(), String> {
    match b {
        b'l' => Ok(()),
        b'B' => Err(tx!("dbus.d_bus_el_otro_extremo_manda_mensajes_big").into()),
        x => Err(txf!("dbus.d_bus_byte_de_orden_no_valido_al", format!("{:02x}", x))),
    }
}

fn decode_header(fixed: &[u8; 16], fields: &[u8]) -> Result<Msg, String> {
    orden_de_bytes(fixed[0])?;
    let mut m = Msg { kind: fixed[1], flags: fixed[2], serial: u32::from_le_bytes(fixed[8..12].try_into().unwrap()), ..Default::default() };
    // los campos empiezan en el desplazamiento 16 del mensaje: misma alineacion a 8 que aqui
    let mut r = R::new(fields);
    while r.pos < fields.len() {
        r.align(8)?;
        if r.pos >= fields.len() {
            break;
        }
        let code = r.u8()?;
        let sig = r.sig()?;
        match sig.as_str() {
            "o" | "s" => {
                let s = r.str()?;
                match code {
                    1 => m.path = s,
                    2 => m.iface = s,
                    3 => m.member = s,
                    4 => m.error = s,
                    6 => m.dest = s,
                    _ => {}
                }
            }
            "u" => {
                let v = r.u32()?;
                match code {
                    5 => m.reply_serial = v,
                    9 => m.unix_fds = v,
                    _ => {}
                }
            }
            "g" => {
                let s = r.sig()?;
                if code == 8 {
                    m.signature = s;
                }
            }
            other => return Err(txf!("dbus.d_bus_tipo_de_campo_de_cabecera", other)),
        }
    }
    Ok(m)
}

/// Lee un mensaje completo. Ok(None) si el otro extremo cerro la conexion.
pub fn read_msg<S: Read>(s: &mut S) -> Result<Option<Msg>, String> {
    let mut fixed = [0u8; 16];
    match s.read_exact(&mut fixed) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(txf!("dbus.d_bus_lectura", e)),
    }
    // el orden de bytes va antes que las longitudes: leidas en el orden equivocado pediran cualquier cantidad
    orden_de_bytes(fixed[0])?;
    let body_len = u32::from_le_bytes(fixed[4..8].try_into().unwrap()) as usize;
    let fields_len = u32::from_le_bytes(fixed[12..16].try_into().unwrap()) as usize;
    let padded = fields_len.div_ceil(8) * 8;
    if padded + body_len > MAX_MSG {
        return Err(tx!("dbus.d_bus_mensaje_demasiado_grande").into());
    }
    let mut fields = vec![0u8; padded];
    s.read_exact(&mut fields).map_err(|e| txf!("dbus.d_bus_lectura", e))?;
    let mut m = decode_header(&fixed, &fields[..fields_len])?;
    m.body = vec![0u8; body_len];
    s.read_exact(&mut m.body).map_err(|e| txf!("dbus.d_bus_lectura", e))?;
    Ok(Some(m))
}

// ---------------------------------------------------------------------------------------------------------------
// conexion

/// Autenticacion EXTERNAL como cliente, con paso de descriptores, sobre un socket recien conectado.
pub fn auth_client(s: &mut UnixStream) -> Result<(), String> {
    autenticar(s, true)
}

/// Autenticacion EXTERNAL; `con_fds`: negociar tambien el paso de descriptores (la notificacion no lo necesita).
fn autenticar(s: &mut UnixStream, con_fds: bool) -> Result<(), String> {
    let io = |e: std::io::Error| txf!("dbus.d_bus_autenticacion", e);
    let uid: String = unsafe { getuid() }.to_string().bytes().map(|b| format!("{:02x}", b)).collect();
    s.write_all(b"\0").map_err(io)?;
    s.write_all(format!("AUTH EXTERNAL {}\r\n", uid).as_bytes()).map_err(io)?; // texto-interno: protocolo D-Bus
    let line = read_line(s)?;
    if !line.starts_with("OK ") {
        return Err(txf!("dbus.d_bus_autenticacion_rechazada", line));
    }
    if con_fds {
        s.write_all(b"NEGOTIATE_UNIX_FD\r\n").map_err(io)?;
        let line = read_line(s)?;
        if line != "AGREE_UNIX_FD" {
            return Err(txf!("dbus.d_bus_el_otro_extremo_no_acepta", line));
        }
    }
    s.write_all(b"BEGIN\r\n").map_err(io)
}

/// Linea de autenticacion (termina en \r\n), leida byte a byte para no consumir datos del primer mensaje.
fn read_line(s: &mut UnixStream) -> Result<String, String> {
    let mut v = Vec::new();
    let mut c = [0u8; 1];
    while !v.ends_with(b"\r\n") {
        if v.len() > 1024 {
            return Err(tx!("dbus.d_bus_linea_de_autenticacion_demasiado").into());
        }
        let n = s.read(&mut c).map_err(|e| txf!("dbus.d_bus_autenticacion", e))?;
        if n == 0 {
            return Err(tx!("dbus.d_bus_el_otro_extremo_cerro_durante_la").into());
        }
        v.push(c[0]);
    }
    v.truncate(v.len() - 2);
    Ok(String::from_utf8_lossy(&v).into_owned())
}

/// Lado de escritura de una conexion: numera los mensajes.
pub struct Conn {
    pub s: UnixStream,
    serial: u32,
}

impl Conn {
    pub fn new(s: UnixStream) -> Conn {
        Conn { s, serial: 0 }
    }

    /// Envia un mensaje y devuelve su numero de serie.
    pub fn send(&mut self, m: &Msg) -> Result<u32, String> {
        self.serial += 1;
        (&self.s).write_all(&m.encode(self.serial)).map_err(|e| txf!("dbus.d_bus_escritura", e))?;
        Ok(self.serial)
    }

    /// Envia un mensaje que lleva un descriptor (argumento Fd(0)).
    pub fn send_fd(&mut self, m: &Msg, fd: RawFd) -> Result<u32, String> {
        self.serial += 1;
        send_with_fd(&self.s, &m.encode(self.serial), fd).map_err(|e| txf!("dbus.d_bus_escritura", e))?;
        Ok(self.serial)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// bus de sesion: notificacion de escritorio

/// Donde esta el bus de una direccion de D-Bus (DBUS_SESSION_BUS_ADDRESS).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Direccion {
    /// `unix:path=RUTA`
    Ruta(String),
    /// `unix:abstract=NOMBRE` (socket del espacio abstracto de Linux)
    Abstracta(String),
}

/// La primera entrada utilizable de una direccion de D-Bus: `unix:path=...` o `unix:abstract=...`. Las entradas van
/// separadas por `;`, las claves por `,` (`guid=` y demas se ignoran) y los valores pueden llevar escapes `%XX`. Pura.
pub fn direccion_de(texto: &str) -> Option<Direccion> {
    for entrada in texto.split(';') {
        let Some(resto) = entrada.trim().strip_prefix("unix:") else { continue };
        for par in resto.split(',') {
            match par.split_once('=') {
                Some(("path", v)) => return sin_escapes(v).map(Direccion::Ruta),
                Some(("abstract", v)) => return sin_escapes(v).map(Direccion::Abstracta),
                _ => {}
            }
        }
    }
    None
}

/// Valor de una direccion de D-Bus sin sus escapes `%XX`. None si un escape esta mal formado.
fn sin_escapes(v: &str) -> Option<String> {
    let b = v.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn conectar(d: &Direccion) -> std::io::Result<UnixStream> {
    match d {
        Direccion::Ruta(p) => UnixStream::connect(p),
        #[cfg(target_os = "linux")]
        Direccion::Abstracta(n) => {
            use std::os::linux::net::SocketAddrExt;
            UnixStream::connect_addr(&std::os::unix::net::SocketAddr::from_abstract_name(n.as_bytes())?)
        }
        #[cfg(not(target_os = "linux"))]
        Direccion::Abstracta(_) => Err(std::io::Error::new(std::io::ErrorKind::Unsupported, tx!("dbus.socket_abstracto"))),
    }
}

/// Nombre (y interfaz) del servicio de notificaciones de escritorio.
pub const NOTIFICACIONES: &str = "org.freedesktop.Notifications";

/// Llamada `Notify` de org.freedesktop.Notifications (firma susssasa{sv}i): de "weft", sin sustituir otra (0), sin icono,
/// sin acciones ni pistas y con el tiempo de espera que decida el servidor (-1). Pura.
pub fn mensaje_notify(resumen: &str, cuerpo: &str) -> Msg {
    let mut m = Msg::call("/org/freedesktop/Notifications", NOTIFICACIONES, "Notify");
    m.dest = NOTIFICACIONES.into();
    m.set_body(&[Arg::Str("weft".into()), Arg::U32(0), Arg::Str(String::new()), Arg::Str(resumen.into()), Arg::Str(cuerpo.into()), Arg::StrArr(Vec::new()), Arg::DictVar(Vec::new()), Arg::I32(-1)]);
    m
}

/// La misma notificacion por el portal de escritorio (`AddNotification` de org.freedesktop.portal.Notification, firma
/// sa{sv}): la via que un sandbox de Flatpak deja pasar siempre, por si el bus filtrado no deja llegar a `Notify`. Pura.
pub fn mensaje_portal(id: &str, resumen: &str, cuerpo: &str) -> Msg {
    let mut m = Msg::call("/org/freedesktop/portal/desktop", "org.freedesktop.portal.Notification", "AddNotification");
    m.dest = "org.freedesktop.portal.Desktop".into();
    m.set_body(&[Arg::Str(id.into()), Arg::DictVar(vec![("title".into(), Arg::Str(resumen.into())), ("body".into(), Arg::Str(cuerpo.into()))])]);
    m
}

/// `Hello`: lo primero que hay que decirle al bus.
fn mensaje_hello() -> Msg {
    let mut m = Msg::call("/org/freedesktop/DBus", "org.freedesktop.DBus", "Hello");
    m.dest = "org.freedesktop.DBus".into();
    m
}

/// Espera la respuesta a la llamada numero `serie` (lo demas que llegue, como la senal NameAcquired, se descarta): Ok con
/// la respuesta o Err con el nombre del error.
fn respuesta<S: Read>(s: &mut S, serie: u32) -> Result<Msg, String> {
    loop {
        let m = read_msg(s)?.ok_or(tx!("dbus.d_bus_el_bus_cerro_la_conexion"))?;
        if m.reply_serial != serie {
            continue;
        }
        match m.kind {
            METHOD_RETURN => return Ok(m),
            ERROR => return Err(format!("D-Bus: {}", m.error)),
            _ => {}
        }
    }
}

/// Manda una notificacion de escritorio por el bus de `direccion` (ver `direccion_de`): `Notify` de
/// org.freedesktop.Notifications y, si el bus la rechaza (p. ej. un sandbox sin permiso para ese nombre), el portal de
/// notificaciones. `espera`: tiempo maximo de cada lectura y escritura.
pub fn notificar_en(direccion: &str, resumen: &str, cuerpo: &str, espera: std::time::Duration) -> Result<(), String> {
    let d = direccion_de(direccion).ok_or(tx!("dbus.d_bus_direccion_del_bus_de_sesion_no"))?;
    let mut s = conectar(&d).map_err(|e| txf!("dbus.d_bus_bus_de_sesion", e))?;
    s.set_read_timeout(Some(espera)).and_then(|_| s.set_write_timeout(Some(espera))).map_err(|e| format!("D-Bus: {}", e))?;
    autenticar(&mut s, false)?;
    let mut c = Conn::new(s.try_clone().map_err(|e| format!("D-Bus: {}", e))?);
    let hola = c.send(&mensaje_hello())?;
    respuesta(&mut s, hola)?;
    let n = c.send(&mensaje_notify(resumen, cuerpo))?;
    match respuesta(&mut s, n) {
        Ok(_) => Ok(()),
        Err(e) => {
            let p = c.send(&mensaje_portal("weft", resumen, cuerpo))?;
            respuesta(&mut s, p).map(|_| ()).map_err(|e2| format!("{}; portal: {}", e, e2))
        }
    }
}

/// `notificar_en` con el bus de sesion de este proceso (DBUS_SESSION_BUS_ADDRESS). Err si no hay sesion de escritorio.
pub fn notificar(resumen: &str, cuerpo: &str) -> Result<(), String> {
    let dir = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok().filter(|d| !d.is_empty()).ok_or(tx!("dbus.no_hay_bus_de_sesion_dbus_session_bus"))?;
    notificar_en(&dir, resumen, cuerpo, std::time::Duration::from_secs(2))
}

// ---------------------------------------------------------------------------------------------------------------
// bus de sesion: selector de archivos del portal (org.freedesktop.portal.FileChooser)

/// Nombre del portal de escritorio en el bus de sesion.
const PORTAL: &str = "org.freedesktop.portal.Desktop";

/// Ruta del objeto `Request` que el portal crea para una llamada con `handle_token`: el nombre unico del remitente sin
/// los dos puntos y con `_` en lugar de `.`, y el token (ver la documentacion de org.freedesktop.portal.Request). Pura.
pub fn ruta_peticion(nombre_unico: &str, token: &str) -> String {
    format!("/org/freedesktop/portal/desktop/request/{}/{}", nombre_unico.trim_start_matches(':').replace('.', "_"), token)
}

/// `OpenFile` de org.freedesktop.portal.FileChooser (firma ssa{sv}): sin ventana padre, con `titulo`, modal, una sola
/// seleccion y, con `carpeta`, eligiendo carpetas en vez de archivos. Pura.
pub fn mensaje_elegir(titulo: &str, carpeta: bool, token: &str) -> Msg {
    let mut m = Msg::call("/org/freedesktop/portal/desktop", "org.freedesktop.portal.FileChooser", "OpenFile");
    m.dest = PORTAL.into();
    m.set_body(&[
        Arg::Str(String::new()),
        Arg::Str(titulo.into()),
        Arg::DictVar(vec![("handle_token".into(), Arg::Str(token.into())), ("modal".into(), Arg::Bool(true)), ("multiple".into(), Arg::Bool(false)), ("directory".into(), Arg::Bool(carpeta))]),
    ]);
    m
}

/// `AddMatch` para recibir la senal `Response` del objeto `ruta` (las senales solo llegan a quien las pide).
fn mensaje_add_match(ruta: &str) -> Msg {
    let mut m = Msg::call("/org/freedesktop/DBus", "org.freedesktop.DBus", "AddMatch");
    m.dest = "org.freedesktop.DBus".into();
    m.set_body(&[Arg::Str(format!("type='signal',interface='org.freedesktop.portal.Request',member='Response',path='{}'", ruta))]);
    m
}

/// Salta un valor de la firma `sig` (completa: un solo tipo) en `r`. Admite los tipos basicos, arreglos, estructuras,
/// diccionarios y variantes, con un limite de anidamiento.
fn saltar(r: &mut R, sig: &str, nivel: u32) -> Result<(), String> {
    if nivel > 32 {
        return Err(tx!("dbus.d_bus_valor_anidado_demasiado_profundo").into());
    }
    let b = sig.as_bytes();
    match b.first().copied().ok_or(tx!("dbus.d_bus_firma_vacia"))? {
        b'y' => r.u8().map(|_| ()),
        b'n' | b'q' => {
            r.align(2)?;
            r.need(2)?;
            r.pos += 2;
            Ok(())
        }
        b'b' | b'i' | b'u' | b'h' => r.u32().map(|_| ()),
        b'x' | b't' | b'd' => {
            r.align(8)?;
            r.need(8)?;
            r.pos += 8;
            Ok(())
        }
        b's' | b'o' => r.str().map(|_| ()),
        b'g' => r.sig().map(|_| ()),
        b'v' => {
            let s = r.sig()?;
            saltar(r, &s, nivel + 1)
        }
        b'a' => {
            let n = r.u32()? as usize;
            let elem = &sig[1..];
            r.align(alineacion(elem))?;
            let fin = r.pos.checked_add(n).filter(|f| *f <= r.b.len()).ok_or(tx!("dbus.d_bus_arreglo_truncado"))?;
            while r.pos < fin {
                saltar(r, elem, nivel + 1)?;
            }
            Ok(())
        }
        b'(' | b'{' => {
            r.align(8)?;
            for t in tipos_de(&sig[1..sig.len() - 1])? {
                saltar(r, t, nivel + 1)?;
            }
            Ok(())
        }
        x => Err(txf!("dbus.d_bus_tipo_no_admitido_en_la_respuesta", x as char)),
    }
}

/// Alineacion de un tipo de D-Bus (por su primer caracter).
fn alineacion(sig: &str) -> usize {
    match sig.as_bytes().first() {
        Some(b'n' | b'q') => 2,
        Some(b'b' | b'i' | b'u' | b'h' | b's' | b'o' | b'a') => 4,
        Some(b'x' | b't' | b'd' | b'(' | b'{') => 8,
        _ => 1,
    }
}

/// Divide una firma en sus tipos completos ("sa{sv}i" -> ["s", "a{sv}", "i"]).
fn tipos_de(sig: &str) -> Result<Vec<&str>, String> {
    let b = sig.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let ini = i;
        while b.get(i) == Some(&b'a') {
            i += 1;
        }
        match b.get(i) {
            Some(b'(') | Some(b'{') => {
                let mut prof = 0;
                loop {
                    match b.get(i) {
                        Some(b'(') | Some(b'{') => prof += 1,
                        Some(b')') | Some(b'}') => prof -= 1,
                        None => return Err(txf!("dbus.d_bus_firma_mal_formada", sig)),
                        _ => {}
                    }
                    i += 1;
                    if prof == 0 {
                        break;
                    }
                }
            }
            Some(_) => i += 1,
            None => return Err(txf!("dbus.d_bus_firma_mal_formada", sig)),
        }
        out.push(&sig[ini..i]);
    }
    Ok(out)
}

/// Lo que dice la senal `Response` del portal (cuerpo `ua{sv}`): Ok(Some(uri)) con la primera de `uris`, Ok(None) si se
/// cancelo, Err si el portal fallo. Pura.
pub fn respuesta_elegir(body: &[u8]) -> Result<Option<String>, String> {
    let mut r = R::new(body);
    match r.u32()? {
        0 => {}
        1 => return Ok(None),
        n => return Err(txf!("dbus.el_selector_de_archivos_termino_con", n)),
    }
    let n = r.u32()? as usize;
    r.align(8)?;
    let fin = r.pos.checked_add(n).filter(|f| *f <= body.len()).ok_or(tx!("dbus.d_bus_diccionario_truncado"))?;
    while r.pos < fin {
        r.align(8)?;
        let clave = r.str()?;
        let sig = r.sig()?;
        if clave == "uris" && sig == "as" {
            let m = r.u32()? as usize;
            let fin_l = r.pos.checked_add(m).filter(|f| *f <= body.len()).ok_or(tx!("dbus.d_bus_arreglo_truncado"))?;
            if r.pos < fin_l {
                return Ok(Some(r.str()?));
            }
            return Ok(None);
        }
        saltar(&mut r, &sig, 0)?;
    }
    Ok(None)
}

/// Abre el selector de archivos del portal de escritorio por el bus de `direccion` y espera la eleccion (hasta `espera`
/// en total: la persona puede tardar). Ok(Some(ruta)) con la ruta local elegida, Ok(None) si se cancelo.
pub fn elegir_en(direccion: &str, titulo: &str, carpeta: bool, espera: std::time::Duration) -> Result<Option<String>, String> {
    let d = direccion_de(direccion).ok_or(tx!("dbus.d_bus_direccion_del_bus_de_sesion_no"))?;
    let mut s = conectar(&d).map_err(|e| txf!("dbus.d_bus_bus_de_sesion", e))?;
    let corta = std::time::Duration::from_secs(5);
    s.set_read_timeout(Some(corta)).and_then(|_| s.set_write_timeout(Some(corta))).map_err(|e| format!("D-Bus: {}", e))?;
    autenticar(&mut s, false)?;
    let mut c = Conn::new(s.try_clone().map_err(|e| format!("D-Bus: {}", e))?);
    let hola = c.send(&mensaje_hello())?;
    let unico = respuesta(&mut s, hola).and_then(|m| R::new(&m.body).str())?;
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let token = format!("weft{}_{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let esperada = ruta_peticion(&unico, &token);
    // la senal se pide ANTES de la llamada: el portal puede contestar enseguida
    let am = c.send(&mensaje_add_match(&esperada))?;
    respuesta(&mut s, am)?;
    let llamada = c.send(&mensaje_elegir(titulo, carpeta, &token))?;
    let mut ruta = respuesta(&mut s, llamada).and_then(|m| R::new(&m.body).str())?;
    if ruta != esperada {
        // portales antiguos no usan el token: se escucha el objeto que devolvieron
        let am = c.send(&mensaje_add_match(&ruta))?;
        if respuesta(&mut s, am).is_err() {
            ruta = esperada.clone();
        }
    }
    let t0 = std::time::Instant::now();
    s.set_read_timeout(Some(espera)).map_err(|e| format!("D-Bus: {}", e))?;
    loop {
        if t0.elapsed() >= espera {
            return Err(tx!("dbus.el_selector_de_archivos_no_contesto_a").into());
        }
        let m = read_msg(&mut s)?.ok_or(tx!("dbus.d_bus_el_bus_cerro_la_conexion"))?;
        if m.kind == 4 && m.member == "Response" && (m.path == ruta || m.path == esperada) {
            return match respuesta_elegir(&m.body)? {
                Some(uri) => crate::rutas::ruta_de_uri(&uri).map(Some).ok_or_else(|| txf!("dbus.el_selector_devolvio_algo_que_no_es_un", uri)),
                None => Ok(None),
            };
        }
    }
}

/// `elegir_en` con el bus de sesion de este proceso. Err si no hay sesion de escritorio o no hay portal.
pub fn elegir(titulo: &str, carpeta: bool) -> Result<Option<String>, String> {
    let dir = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok().filter(|d| !d.is_empty()).ok_or(tx!("dbus.no_hay_bus_de_sesion_dbus_session_bus"))?;
    elegir_en(&dir, titulo, carpeta, std::time::Duration::from_secs(30 * 60))
}

// ---------------------------------------------------------------------------------------------------------------
// bus de sesion: esquema de color del escritorio (org.freedesktop.portal.Settings)

/// Espacio de nombres y clave de la preferencia de color en el portal de ajustes.
const APARIENCIA: (&str, &str) = ("org.freedesktop.appearance", "color-scheme");

/// Lectura de la preferencia de color: `ReadOne` (portal version 2 en adelante; responde `v`) o, con `antigua`, `Read`
/// (version 1, obsoleta: responde la variante envuelta en otra, `v` con `v` dentro). Pura.
pub fn mensaje_leer_esquema(antigua: bool) -> Msg {
    let mut m = Msg::call("/org/freedesktop/portal/desktop", "org.freedesktop.portal.Settings", if antigua { "Read" } else { "ReadOne" });
    m.dest = PORTAL.into();
    m.set_body(&[Arg::Str(APARIENCIA.0.into()), Arg::Str(APARIENCIA.1.into())]);
    m
}

/// `AddMatch` para la senal `SettingChanged` del portal de ajustes, solo la de la apariencia.
fn mensaje_add_match_ajustes() -> Msg {
    let mut m = Msg::call("/org/freedesktop/DBus", "org.freedesktop.DBus", "AddMatch");
    m.dest = "org.freedesktop.DBus".into();
    m.set_body(&[Arg::Str(format!(
        "type='signal',interface='org.freedesktop.portal.Settings',member='SettingChanged',path='/org/freedesktop/portal/desktop',arg0='{}'",
        APARIENCIA.0
    ))]);
    m
}

/// Lee una variante con un entero (`u` o `i`; las variantes anidadas, como la respuesta de `Read`, se abren): el valor de
/// `color-scheme`. Un entero negativo u otro tipo es un error.
fn entero_en_variante(r: &mut R, nivel: u32) -> Result<u32, String> {
    if nivel > 4 {
        return Err(tx!("dbus.d_bus_variante_anidada_demasiado").into());
    }
    match r.sig()?.as_str() {
        "v" => entero_en_variante(r, nivel + 1),
        "u" => r.u32(),
        "i" => u32::try_from(r.i32()?).map_err(|_| tx!("dbus.d_bus_color_scheme_negativo").to_string()),
        s => Err(txf!("dbus.d_bus_color_scheme_con_un_tipo", s)),
    }
}

/// El valor de la respuesta a `mensaje_leer_esquema` (firma `v`): 0 sin preferencia, 1 prefiere oscuro, 2 prefiere claro
/// (otros valores pasan tal cual: quien los use decide). Pura.
pub fn respuesta_esquema(m: &Msg) -> Result<u32, String> {
    if m.signature != "v" {
        return Err(txf!("dbus.d_bus_respuesta_de_settings_con_firma", m.signature));
    }
    entero_en_variante(&mut R::new(&m.body), 0)
}

/// Si `m` es la senal `SettingChanged` (firma `ssv`) de `color-scheme`, su nuevo valor. Pura.
pub fn senal_esquema(m: &Msg) -> Option<u32> {
    if m.kind != 4 || m.member != "SettingChanged" || m.iface != "org.freedesktop.portal.Settings" || m.signature != "ssv" {
        return None;
    }
    let mut r = R::new(&m.body);
    let (ns, clave) = (r.str().ok()?, r.str().ok()?);
    if (ns.as_str(), clave.as_str()) != APARIENCIA {
        return None;
    }
    entero_en_variante(&mut r, 0).ok()
}

/// Lee la preferencia de color del escritorio por el bus de `direccion` y se queda escuchando sus cambios: llama a `aviso`
/// con el valor inicial y con cada cambio. Solo vuelve con error: sin bus o sin portal (antes del primer aviso), o cuando el
/// bus cierra la conexion. `espera`: tiempo maximo de cada paso hasta el primer aviso (la escucha no tiene limite).
pub fn vigilar_esquema_en(direccion: &str, espera: std::time::Duration, aviso: &mut dyn FnMut(u32)) -> Result<(), String> {
    let d = direccion_de(direccion).ok_or(tx!("dbus.d_bus_direccion_del_bus_de_sesion_no"))?;
    let mut s = conectar(&d).map_err(|e| txf!("dbus.d_bus_bus_de_sesion", e))?;
    s.set_read_timeout(Some(espera)).and_then(|_| s.set_write_timeout(Some(espera))).map_err(|e| format!("D-Bus: {}", e))?;
    autenticar(&mut s, false)?;
    let mut c = Conn::new(s.try_clone().map_err(|e| format!("D-Bus: {}", e))?);
    let hola = c.send(&mensaje_hello())?;
    respuesta(&mut s, hola)?;
    // la senal se pide antes de leer: un cambio entre la lectura y la escucha no se pierde
    let am = c.send(&mensaje_add_match_ajustes())?;
    respuesta(&mut s, am)?;
    let n = c.send(&mensaje_leer_esquema(false))?;
    let valor = match respuesta(&mut s, n) {
        Ok(m) => respuesta_esquema(&m)?,
        // portal de version 1: solo `Read`
        Err(e) => {
            let n = c.send(&mensaje_leer_esquema(true))?;
            respuesta(&mut s, n).and_then(|m| respuesta_esquema(&m)).map_err(|e2| format!("{}; Read: {}", e, e2))?
        }
    };
    aviso(valor);
    s.set_read_timeout(None).map_err(|e| format!("D-Bus: {}", e))?;
    loop {
        let m = read_msg(&mut s)?.ok_or(tx!("dbus.d_bus_el_bus_cerro_la_conexion"))?;
        if let Some(v) = senal_esquema(&m) {
            aviso(v);
        }
    }
}

/// `vigilar_esquema_en` con el bus de sesion de este proceso. Err si no hay sesion de escritorio o no hay portal.
pub fn vigilar_esquema(aviso: &mut dyn FnMut(u32)) -> Result<(), String> {
    let dir = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok().filter(|d| !d.is_empty()).ok_or(tx!("dbus.no_hay_bus_de_sesion_dbus_session_bus"))?;
    vigilar_esquema_en(&dir, std::time::Duration::from_secs(5), aviso)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// OpenFile del portal: firma ssa{sv}, booleanos como enteros de 32 bits y el token en las opciones.
    #[test]
    fn selector_de_archivos_mensaje() {
        let m = mensaje_elegir("Elige", true, "weft1_0");
        assert_eq!((m.dest.as_str(), m.member.as_str(), m.signature.as_str()), ("org.freedesktop.portal.Desktop", "OpenFile", "ssa{sv}"));
        let mut r = R::new(&m.body);
        assert_eq!(r.str().unwrap(), "");
        assert_eq!(r.str().unwrap(), "Elige");
        let n = r.u32().unwrap() as usize;
        r.align(8).unwrap();
        let fin = r.pos + n;
        let mut vistos = Vec::new();
        while r.pos < fin {
            r.align(8).unwrap();
            let k = r.str().unwrap();
            let sig = r.sig().unwrap();
            let v = if sig == "b" { r.u32().unwrap().to_string() } else { r.str().unwrap() };
            vistos.push(format!("{}:{}={}", k, sig, v));
        }
        assert_eq!(vistos, ["handle_token:s=weft1_0", "modal:b=1", "multiple:b=0", "directory:b=1"]);
        assert_eq!(ruta_peticion(":1.42", "weft1_0"), "/org/freedesktop/portal/desktop/request/1_42/weft1_0");
    }

    /// Response(u, a{sv}): se salta lo que no son `uris` (de cualquier tipo) y se toma la primera; 1 es cancelar.
    #[test]
    fn selector_de_archivos_respuesta() {
        let mut w = W::default();
        w.u32(0);
        w.arg(&Arg::DictVar(vec![
            ("choices".into(), Arg::DictStrArr(vec![("x".into(), vec!["y".into()])])),
            ("current_filter".into(), Arg::U64(7)),
            ("uris".into(), Arg::StrArr(vec!["file:///home/a/Mi%20zip.zip".into(), "file:///otro".into()])),
        ]));
        assert_eq!(respuesta_elegir(&w.v).unwrap().as_deref(), Some("file:///home/a/Mi%20zip.zip"));
        let mut w = W::default();
        w.u32(1);
        w.arg(&Arg::DictVar(vec![]));
        assert_eq!(respuesta_elegir(&w.v).unwrap(), None);
        let mut w = W::default();
        w.u32(2);
        assert!(respuesta_elegir(&w.v).is_err());
        assert_eq!(tipos_de("sa{sv}a(ii)i").unwrap(), ["s", "a{sv}", "a(ii)", "i"]);
        // truncado: error, no panico
        let mut w = W::default();
        w.u32(0);
        w.u32(400);
        assert!(respuesta_elegir(&w.v).is_err() || respuesta_elegir(&w.v).unwrap().is_none());
    }

    #[test]
    fn ida_y_vuelta() {
        let mut m = Msg::call("/org/qemu/Display1/Console_0", "org.qemu.Display1.Mouse", "SetAbsPosition");
        m.flags = NO_REPLY;
        m.set_body(&[Arg::U32(359), Arg::U32(1347)]);
        let b = m.encode(7);
        assert_eq!(b.len() % 8, 0);
        let r = read_msg(&mut &b[..]).unwrap().unwrap();
        assert_eq!((r.kind, r.flags, r.serial), (METHOD_CALL, NO_REPLY, 7));
        assert_eq!((r.path.as_str(), r.iface.as_str(), r.member.as_str()), ("/org/qemu/Display1/Console_0", "org.qemu.Display1.Mouse", "SetAbsPosition"));
        assert_eq!(r.signature, "uu");
        let mut rd = R::new(&r.body);
        assert_eq!((rd.u32().unwrap(), rd.u32().unwrap()), (359, 1347));
        assert!(!r.wants_reply());
    }

    /// Un mensaje big-endian se rechaza por su primer byte, antes de leer las longitudes: leidas como little-endian
    /// pedirian 16 MiB de campos que no llegan nunca (y el lector se quedaria esperando).
    #[test]
    fn rechaza_big_endian_antes_de_leer_longitudes() {
        // 'B', llamada, sin banderas, version 1; cuerpo 0, serie 1 y 1 byte de campos, todo big-endian
        let mut b = vec![b'B', METHOD_CALL, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1];
        b.extend_from_slice(&[1, b'o', 0, 0, 0, 0, 0, 0]);
        let mut r = &b[..];
        let e = read_msg(&mut r).unwrap_err();
        assert!(e.contains("big-endian"), "{}", e);
        // solo se consumio la cabecera fija
        assert_eq!(r.len(), b.len() - 16);
        // cualquier otro primer byte: flujo roto
        let mut malo = Msg::call("/a", "b.c", "D").encode(1);
        malo[0] = 0;
        let e = read_msg(&mut &malo[..]).unwrap_err();
        assert!(e.contains("orden no valido"), "{}", e);
        assert!(orden_de_bytes(b'l').is_ok());
    }

    /// SetUIInfo(q width_mm, q height_mm, i x, i y, u width, u height): los enteros de 16 bits se alinean a 2 y los de 32 a 4.
    #[test]
    fn set_ui_info() {
        let mut m = Msg::call("/org/qemu/Display1/Console_0", "org.qemu.Display1.Console", "SetUIInfo");
        m.set_body(&[Arg::U16(72), Arg::U16(135), Arg::I32(0), Arg::I32(-3), Arg::U32(720), Arg::U32(1348)]);
        assert_eq!(m.signature, "qqiiuu");
        assert_eq!(m.body.len(), 20);
        let mut r = R::new(&m.body);
        assert_eq!(&m.body[0..4], &[72, 0, 135, 0]);
        assert_eq!((r.u32().unwrap(), r.u32().unwrap(), r.u32().unwrap(), r.u32().unwrap()), (72 | 135 << 16, 0, (-3i32) as u32, 720));
        assert_eq!(r.u32().unwrap(), 1348);
    }

    /// MultiTouch.SendEvent(u kind, t num_slot, d x, d y): el uint64 y los dobles se alinean a 8.
    #[test]
    fn multitoque() {
        let mut m = Msg::call("/org/qemu/Display1/Console_0", "org.qemu.Display1.MultiTouch", "SendEvent");
        m.set_body(&[Arg::U32(1), Arg::U64(1), Arg::F64(12.5), Arg::F64(700.0)]);
        assert_eq!(m.signature, "utdd");
        assert_eq!(m.body.len(), 32);
        let mut r = R::new(&m.body);
        assert_eq!((r.u32().unwrap(), r.u64().unwrap(), r.f64().unwrap(), r.f64().unwrap()), (1, 1, 12.5, 700.0));
    }

    /// Cuerpo de Scanout tal como lo arma QEMU (uuuuay) y la respuesta con la propiedad Interfaces.
    #[test]
    fn scanout_y_propiedades() {
        let mut body = Vec::new();
        for v in [2u32, 1, 8, 0x20020888] {
            body.extend(v.to_le_bytes());
        }
        body.extend(8u32.to_le_bytes());
        body.extend([1u8, 2, 3, 4, 5, 6, 7, 8]);
        let mut r = R::new(&body);
        let (w, h, stride, fmt) = (r.u32().unwrap(), r.u32().unwrap(), r.u32().unwrap(), r.u32().unwrap());
        let d = r.bytes().unwrap();
        assert_eq!((w, h, stride, fmt), (2, 1, 8, 0x20020888));
        assert_eq!(&body[d], &[1, 2, 3, 4, 5, 6, 7, 8]);

        let call = Msg { kind: METHOD_CALL, serial: 3, ..Default::default() };
        let mut rep = Msg::reply_to(&call);
        rep.set_body(&[Arg::DictStrArr(vec![("Interfaces".into(), vec![])])]);
        assert_eq!(rep.signature, "a{sv}");
        let back = read_msg(&mut &rep.encode(1)[..]).unwrap().unwrap();
        assert_eq!((back.kind, back.reply_serial), (METHOD_RETURN, 3));
        let mut rd = R::new(&back.body);
        let _len = rd.u32().unwrap();
        rd.align(8).unwrap();
        assert_eq!(rd.str().unwrap(), "Interfaces");
        assert_eq!(rd.sig().unwrap(), "as");
        assert_eq!(rd.u32().unwrap(), 0);
    }

    /// Dialogo de autenticacion contra un servidor simulado y envio de un descriptor.
    #[test]
    fn autenticacion_y_descriptor() {
        let (mut a, b) = UnixStream::pair().unwrap();
        let t = std::thread::spawn(move || {
            let mut b = b;
            let mut nul = [0u8; 1];
            b.read_exact(&mut nul).unwrap();
            let l = read_line(&mut b).unwrap();
            assert!(l.starts_with("AUTH EXTERNAL "));
            b.write_all(b"OK 0123456789abcdef0123456789abcdef\r\n").unwrap();
            assert_eq!(read_line(&mut b).unwrap(), "NEGOTIATE_UNIX_FD");
            b.write_all(b"AGREE_UNIX_FD\r\n").unwrap();
            assert_eq!(read_line(&mut b).unwrap(), "BEGIN");
            read_msg(&mut b).unwrap().unwrap()
        });
        auth_client(&mut a).unwrap();
        let (x, _y) = UnixStream::pair().unwrap();
        let mut m = Msg::call("/org/qemu/Display1/Console_0", "org.qemu.Display1.Console", "RegisterListener");
        m.set_body(&[Arg::Fd(0)]);
        Conn::new(a).send_fd(&m, x.as_raw_fd()).unwrap();
        let got = t.join().unwrap();
        assert_eq!((got.member.as_str(), got.signature.as_str(), got.unix_fds), ("RegisterListener", "h", 1));
    }

    /// Notify(s app, u reemplaza, s icono, s resumen, s cuerpo, as acciones, a{sv} pistas, i espera): cabecera con
    /// destino y cuerpo campo a campo, con sus alineaciones (el diccionario vacio se alinea a 8 igualmente).
    #[test]
    fn mensaje_de_notificacion() {
        let m = mensaje_notify("weft: falta una imagen de Android", "linea 1\nlinea 2");
        assert_eq!(m.signature, "susssasa{sv}i");
        let b = m.encode(2);
        let r = read_msg(&mut &b[..]).unwrap().unwrap();
        assert_eq!((r.kind, r.serial), (METHOD_CALL, 2));
        assert_eq!(
            (r.dest.as_str(), r.path.as_str(), r.iface.as_str(), r.member.as_str(), r.signature.as_str()),
            (NOTIFICACIONES, "/org/freedesktop/Notifications", NOTIFICACIONES, "Notify", "susssasa{sv}i")
        );
        assert!(r.wants_reply());
        let mut rd = R::new(&r.body);
        assert_eq!(rd.str().unwrap(), "weft");
        assert_eq!(rd.u32().unwrap(), 0);
        assert_eq!(rd.str().unwrap(), "");
        assert_eq!(rd.str().unwrap(), "weft: falta una imagen de Android");
        assert_eq!(rd.str().unwrap(), "linea 1\nlinea 2");
        // acciones: arreglo vacio
        assert_eq!(rd.u32().unwrap(), 0);
        // pistas: diccionario vacio (longitud 0 y relleno hasta 8)
        assert_eq!(rd.u32().unwrap(), 0);
        rd.align(8).unwrap();
        assert_eq!(rd.i32().unwrap(), -1);
        assert_eq!(rd.pos, r.body.len());
        // los bytes del principio: "weft" con su longitud y su NUL, y el 0 de reemplaza alineado a 4
        assert_eq!(&r.body[..16], &[4, 0, 0, 0, b'w', b'e', b'f', b't', 0, 0, 0, 0, 0, 0, 0, 0]);

        // el portal: AddNotification(s id, a{sv} {title: <s>, body: <s>})
        let p = mensaje_portal("weft", "titulo", "cuerpo");
        assert_eq!((p.signature.as_str(), p.dest.as_str(), p.member.as_str()), ("sa{sv}", "org.freedesktop.portal.Desktop", "AddNotification"));
        let mut rd = R::new(&p.body);
        assert_eq!(rd.str().unwrap(), "weft");
        let _len = rd.u32().unwrap();
        for (k, v) in [("title", "titulo"), ("body", "cuerpo")] {
            rd.align(8).unwrap();
            assert_eq!(rd.str().unwrap(), k);
            assert_eq!(rd.sig().unwrap(), "s");
            assert_eq!(rd.str().unwrap(), v);
        }
        assert_eq!(rd.pos, p.body.len());
    }

    #[test]
    fn direccion_del_bus_de_sesion() {
        assert_eq!(direccion_de("unix:path=/run/user/1000/bus"), Some(Direccion::Ruta("/run/user/1000/bus".into())));
        assert_eq!(direccion_de("unix:path=/run/user/1000/bus,guid=0123abcd"), Some(Direccion::Ruta("/run/user/1000/bus".into())));
        assert_eq!(direccion_de("unix:guid=x,abstract=/tmp/dbus-AbC"), Some(Direccion::Abstracta("/tmp/dbus-AbC".into())));
        // la primera entrada que sirva; escapes %XX
        assert_eq!(direccion_de("tcp:host=localhost,port=1;unix:path=/a%20b/bus"), Some(Direccion::Ruta("/a b/bus".into())));
        assert_eq!(direccion_de("tcp:host=localhost,port=1"), None);
        assert_eq!(direccion_de("unix:path=/a%2"), None);
        assert_eq!(direccion_de(""), None);
    }

    /// Dialogo completo con un bus simulado: autenticacion sin descriptores, Hello, Notify y su respuesta. Si el bus rechaza
    /// Notify (sandbox), se usa el portal.
    #[test]
    fn notificacion_contra_un_bus_simulado() {
        for rechaza in [false, true] {
            let ruta = std::env::temp_dir().join(format!("weft-bus-{}-{}", std::process::id(), rechaza));
            let _ = std::fs::remove_file(&ruta);
            let l = std::os::unix::net::UnixListener::bind(&ruta).unwrap();
            let t = std::thread::spawn(move || {
                let (mut b, _) = l.accept().unwrap();
                let mut nul = [0u8; 1];
                b.read_exact(&mut nul).unwrap();
                assert!(read_line(&mut b).unwrap().starts_with("AUTH EXTERNAL "));
                b.write_all(b"OK 0123456789abcdef0123456789abcdef\r\n").unwrap();
                // sin NEGOTIATE_UNIX_FD: directamente BEGIN
                assert_eq!(read_line(&mut b).unwrap(), "BEGIN");
                let mut w = Conn::new(b.try_clone().unwrap());
                let hola = read_msg(&mut b).unwrap().unwrap();
                assert_eq!((hola.member.as_str(), hola.dest.as_str()), ("Hello", "org.freedesktop.DBus"));
                let mut r = Msg::reply_to(&hola);
                r.dest = ":1.7".into();
                r.set_body(&[Arg::Str(":1.7".into())]);
                w.send(&r).unwrap();
                // una senal cualquiera antes de la respuesta: se descarta
                let mut senal = Msg { kind: 4, path: "/org/freedesktop/DBus".into(), iface: "org.freedesktop.DBus".into(), member: "NameAcquired".into(), ..Default::default() };
                senal.set_body(&[Arg::Str(":1.7".into())]);
                w.send(&senal).unwrap();
                let n = read_msg(&mut b).unwrap().unwrap();
                assert_eq!((n.member.as_str(), n.signature.as_str()), ("Notify", "susssasa{sv}i"));
                let mut recibidos = vec![n.member.clone()];
                if rechaza {
                    w.send(&Msg::error_to(&n, "org.freedesktop.DBus.Error.ServiceUnknown", "no")).unwrap();
                    let p = read_msg(&mut b).unwrap().unwrap();
                    recibidos.push(p.member.clone());
                    w.send(&Msg::reply_to(&p)).unwrap();
                } else {
                    let mut r = Msg::reply_to(&n);
                    r.set_body(&[Arg::U32(9)]);
                    w.send(&r).unwrap();
                }
                recibidos
            });
            let dir = format!("unix:path={},guid=00", ruta.display());
            notificar_en(&dir, "resumen", "cuerpo", std::time::Duration::from_secs(5)).unwrap();
            let recibidos = t.join().unwrap();
            assert_eq!(recibidos, if rechaza { vec!["Notify", "AddNotification"] } else { vec!["Notify"] });
            let _ = std::fs::remove_file(&ruta);
        }
        // sin bus que escuche: error, sin colgarse
        assert!(notificar_en("unix:path=/no/existe/bus", "a", "b", std::time::Duration::from_secs(1)).is_err());
        assert!(notificar_en("tcp:host=x", "a", "b", std::time::Duration::from_secs(1)).is_err());
    }

    /// La preferencia de color del portal: las dos formas de respuesta (`ReadOne` con `v`, `Read` con `v` dentro de `v`),
    /// enteros con y sin signo, y la senal de cambio solo para `color-scheme`.
    #[test]
    fn esquema_de_color_del_portal() {
        let llamada = mensaje_leer_esquema(false);
        assert_eq!((llamada.member.as_str(), llamada.signature.as_str(), llamada.dest.as_str()), ("ReadOne", "ss", PORTAL));
        let mut r = R::new(&llamada.body);
        assert_eq!((r.str().unwrap(), r.str().unwrap()), ("org.freedesktop.appearance".to_string(), "color-scheme".to_string()));
        assert_eq!(mensaje_leer_esquema(true).member, "Read");
        let resp = |a: Arg| {
            let mut m = Msg::reply_to(&llamada);
            m.set_body(&[a]);
            m
        };
        assert_eq!(respuesta_esquema(&resp(Arg::Var(Box::new(Arg::U32(1))))), Ok(1));
        assert_eq!(respuesta_esquema(&resp(Arg::Var(Box::new(Arg::Var(Box::new(Arg::U32(2))))))), Ok(2));
        assert_eq!(respuesta_esquema(&resp(Arg::Var(Box::new(Arg::I32(0))))), Ok(0));
        assert!(respuesta_esquema(&resp(Arg::Var(Box::new(Arg::I32(-1))))).is_err());
        assert!(respuesta_esquema(&resp(Arg::Var(Box::new(Arg::Str("dark".into()))))).is_err());
        assert!(respuesta_esquema(&resp(Arg::U32(1))).is_err());
        // cuerpo truncado: error, sin panico
        let mut corta = resp(Arg::Var(Box::new(Arg::U32(1))));
        corta.body.truncate(5);
        assert!(respuesta_esquema(&corta).is_err());
        let senal = |ns: &str, k: &str, v: Arg| {
            let mut m = Msg { kind: 4, path: "/org/freedesktop/portal/desktop".into(), iface: "org.freedesktop.portal.Settings".into(), member: "SettingChanged".into(), ..Default::default() };
            m.set_body(&[Arg::Str(ns.into()), Arg::Str(k.into()), Arg::Var(Box::new(v))]);
            m
        };
        assert_eq!(senal_esquema(&senal("org.freedesktop.appearance", "color-scheme", Arg::U32(2))), Some(2));
        assert_eq!(senal_esquema(&senal("org.freedesktop.appearance", "accent-color", Arg::U32(2))), None);
        assert_eq!(senal_esquema(&senal("org.gnome.desktop.interface", "color-scheme", Arg::U32(2))), None);
        let mut otra = senal("org.freedesktop.appearance", "color-scheme", Arg::U32(1));
        otra.member = "Otra".into();
        assert_eq!(senal_esquema(&otra), None);
    }

    /// Contra un bus simulado: con un portal de version 1 (`ReadOne` desconocido) se lee con `Read`, se avisa el valor
    /// inicial y despues cada `SettingChanged` de la apariencia (las demas senales se ignoran); al cerrar el bus, error.
    #[test]
    fn vigilar_esquema_contra_un_bus_simulado() {
        for antigua in [false, true] {
            let ruta = std::env::temp_dir().join(format!("weft-bus-esquema-{}-{}", std::process::id(), antigua));
            let _ = std::fs::remove_file(&ruta);
            let l = std::os::unix::net::UnixListener::bind(&ruta).unwrap();
            let t = std::thread::spawn(move || {
                let (mut b, _) = l.accept().unwrap();
                let mut nul = [0u8; 1];
                b.read_exact(&mut nul).unwrap();
                assert!(read_line(&mut b).unwrap().starts_with("AUTH EXTERNAL "));
                b.write_all(b"OK 0123456789abcdef0123456789abcdef\r\n").unwrap();
                assert_eq!(read_line(&mut b).unwrap(), "BEGIN");
                let mut w = Conn::new(b.try_clone().unwrap());
                let hola = read_msg(&mut b).unwrap().unwrap();
                let mut r = Msg::reply_to(&hola);
                r.set_body(&[Arg::Str(":1.9".into())]);
                w.send(&r).unwrap();
                let am = read_msg(&mut b).unwrap().unwrap();
                assert_eq!(am.member, "AddMatch");
                assert!(R::new(&am.body).str().unwrap().contains("member='SettingChanged'"));
                w.send(&Msg::reply_to(&am)).unwrap();
                let uno = read_msg(&mut b).unwrap().unwrap();
                assert_eq!(uno.member, "ReadOne");
                let lectura = if antigua {
                    w.send(&Msg::error_to(&uno, "org.freedesktop.DBus.Error.UnknownMethod", "no")).unwrap();
                    let dos = read_msg(&mut b).unwrap().unwrap();
                    assert_eq!(dos.member, "Read");
                    let mut r = Msg::reply_to(&dos);
                    r.set_body(&[Arg::Var(Box::new(Arg::Var(Box::new(Arg::U32(2)))))]);
                    r
                } else {
                    let mut r = Msg::reply_to(&uno);
                    r.set_body(&[Arg::Var(Box::new(Arg::U32(0)))]);
                    r
                };
                w.send(&lectura).unwrap();
                for (ns, k, v) in [("org.freedesktop.appearance", "accent-color", 7), ("org.freedesktop.appearance", "color-scheme", 1), ("org.freedesktop.appearance", "color-scheme", 2)] {
                    let mut m = Msg { kind: 4, path: "/org/freedesktop/portal/desktop".into(), iface: "org.freedesktop.portal.Settings".into(), member: "SettingChanged".into(), ..Default::default() };
                    m.set_body(&[Arg::Str(ns.into()), Arg::Str(k.into()), Arg::Var(Box::new(Arg::U32(v)))]);
                    w.send(&m).unwrap();
                }
                // el bus se va
            });
            let dir = format!("unix:path={},guid=00", ruta.display());
            let mut vistos = Vec::new();
            let e = vigilar_esquema_en(&dir, std::time::Duration::from_secs(5), &mut |v| vistos.push(v)).unwrap_err();
            t.join().unwrap();
            assert!(e.contains("cerro"), "{}", e);
            assert_eq!(vistos, if antigua { vec![2, 1, 2] } else { vec![0, 1, 2] });
            let _ = std::fs::remove_file(&ruta);
        }
        assert!(vigilar_esquema_en("unix:path=/no/existe/bus", std::time::Duration::from_secs(1), &mut |_| {}).is_err());
    }
}
