//! Cliente RFB (VNC) minimo: solo sabe pedirle a QEMU un tamano de pantalla.
//!
//! El dispositivo grafico de QEMU le ofrece al invitado la resolucion que le indica la interfaz. La ventana GTK
//! le indica su tamano inicial (640x480) y Android arrancaria con eso; la unica via de control por la que QEMU
//! acepta otro tamano es el mensaje SetDesktopSize de un cliente VNC. weft abre un VNC local (socket Unix
//! en su directorio privado) y envia ese unico mensaje.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;
use crate::textos::tx;

/// Mensaje SetDesktopSize (extension ExtendedDesktopSize) para una sola pantalla.
pub fn set_desktop_size_msg(w: u16, h: u16) -> Vec<u8> {
    let mut m = vec![251u8, 0];
    m.extend(w.to_be_bytes());
    m.extend(h.to_be_bytes());
    m.extend([1u8, 0]); // una pantalla
    m.extend(0u32.to_be_bytes()); // identificador
    m.extend([0u8; 4]); // posicion x, y
    m.extend(w.to_be_bytes());
    m.extend(h.to_be_bytes());
    m.extend(0u32.to_be_bytes()); // indicadores
    m
}

/// Dialogo completo sobre una conexion ya abierta: version 3.8, sin autenticacion, y la peticion de tamano.
pub fn request_size<S: Read + Write>(s: &mut S, w: u16, h: u16) -> Result<(), String> {
    let io = |e: std::io::Error| format!("VNC: {}", e);
    let mut ver = [0u8; 12];
    s.read_exact(&mut ver).map_err(io)?;
    if &ver[0..4] != b"RFB " {
        return Err(tx!("rfb.vnc_el_servidor_no_hablo_rfb").into());
    }
    s.write_all(b"RFB 003.008\n").map_err(io)?;
    let mut n = [0u8; 1];
    s.read_exact(&mut n).map_err(io)?;
    if n[0] == 0 {
        return Err(tx!("rfb.vnc_el_servidor_rechazo_la_conexion").into());
    }
    let mut types = vec![0u8; n[0] as usize];
    s.read_exact(&mut types).map_err(io)?;
    if !types.contains(&1) {
        return Err(tx!("rfb.vnc_el_servidor_exige_autenticacion").into());
    }
    s.write_all(&[1]).map_err(io)?;
    let mut res = [0u8; 4];
    s.read_exact(&mut res).map_err(io)?;
    if res != [0, 0, 0, 0] {
        return Err(tx!("rfb.vnc_el_servidor_no_acepto_la_conexion").into());
    }
    s.write_all(&[1]).map_err(io)?; // conexion compartida
    let mut init = [0u8; 24];
    s.read_exact(&mut init).map_err(io)?;
    let name_len = u32::from_be_bytes([init[20], init[21], init[22], init[23]]) as usize;
    let mut name = vec![0u8; name_len.min(4096)];
    s.read_exact(&mut name).map_err(io)?;
    // codificaciones admitidas: ExtendedDesktopSize (-308) y datos sin comprimir (0)
    let mut enc = vec![2u8, 0, 0, 2];
    enc.extend((-308i32).to_be_bytes());
    enc.extend(0i32.to_be_bytes());
    s.write_all(&enc).map_err(io)?;
    s.write_all(&set_desktop_size_msg(w, h)).map_err(io)?;
    s.flush().map_err(io)
}

/// Pide a QEMU, por su socket VNC, que ofrezca al invitado una pantalla de `w` x `h`.
pub fn set_desktop_size(path: &str, w: u16, h: u16) -> Result<(), String> {
    let mut s = UnixStream::connect(path).map_err(|e| format!("VNC: {}", e))?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    s.set_write_timeout(Some(Duration::from_secs(5))).ok();
    request_size(&mut s, w, h)?;
    // se deja a QEMU un momento para procesar la peticion antes de cerrar (su respuesta no hace falta)
    s.set_read_timeout(Some(Duration::from_millis(400))).ok();
    let mut junk = [0u8; 256];
    let _ = s.read(&mut junk);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn mensaje_de_tamano() {
        let m = set_desktop_size_msg(720, 1348);
        assert_eq!(m.len(), 8 + 16);
        assert_eq!(&m[0..8], &[251, 0, 0x02, 0xD0, 0x05, 0x44, 1, 0]);
        assert_eq!(&m[16..20], &[0x02, 0xD0, 0x05, 0x44]);
    }

    /// Servidor RFB simulado: comprueba el dialogo completo y el mensaje final.
    #[test]
    fn dialogo_con_servidor_simulado() {
        let path = std::env::temp_dir().join(format!("weft-rfb-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let l = UnixListener::bind(&path).unwrap();
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            s.write_all(b"RFB 003.008\n").unwrap();
            let mut v = [0u8; 12];
            s.read_exact(&mut v).unwrap();
            s.write_all(&[1, 1]).unwrap();
            let mut c = [0u8; 1];
            s.read_exact(&mut c).unwrap();
            assert_eq!(c[0], 1);
            s.write_all(&[0, 0, 0, 0]).unwrap();
            s.read_exact(&mut c).unwrap();
            let mut init = vec![0u8; 20];
            init.extend(4u32.to_be_bytes());
            init.extend(b"QEMU");
            s.write_all(&init).unwrap();
            let mut enc = [0u8; 12];
            s.read_exact(&mut enc).unwrap();
            assert_eq!(&enc[0..4], &[2, 0, 0, 2]);
            let mut msg = [0u8; 24];
            s.read_exact(&mut msg).unwrap();
            msg.to_vec()
        });
        set_desktop_size(path.to_str().unwrap(), 720, 1348).unwrap();
        assert_eq!(t.join().unwrap(), set_desktop_size_msg(720, 1348));
        let _ = std::fs::remove_file(&path);
        assert!(set_desktop_size("/no/existe.sock", 1, 1).is_err());
    }
}
