//! `weft doctor`: comprueba lo que weft necesita del equipo y lo explica en una tabla OK / AVISO / FALLO.
//! Solo lee: ejecuta `qemu-system-x86_64 --version`, `-display help` y `-device help` (no cambian nada), abre dispositivos
//! en lectura y consulta el espacio libre. Termina con 0 solo si no hay ningun FALLO.

use crate::textos::elige;
use crate::vm::State;
use std::ffi::CString;
use std::os::raw::c_void;
use std::path::{Path, PathBuf};
use std::process::Command;
use crate::textos::{tx, txf};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Nivel {
    Ok,
    Aviso,
    Fallo,
}

pub struct Fila {
    pub nivel: Nivel,
    pub nombre: &'static str,
    pub detalle: String,
}

fn fila(nivel: Nivel, nombre: &'static str, detalle: impl Into<String>) -> Fila {
    Fila { nivel, nombre, detalle: detalle.into() }
}

/// Version mayor de "QEMU emulator version 10.2.2 (qemu-10.2.2-1.fc44)".
pub fn version_qemu(salida: &str) -> Option<(u32, u32)> {
    let v = salida.lines().next()?.split("version ").nth(1)?.split_whitespace().next()?;
    let mut p = v.split('.');
    Some((p.next()?.parse().ok()?, p.next().unwrap_or("0").parse().unwrap_or(0)))
}

/// Si la lista de `-display help` o de `-device help` contiene `nombre` como palabra.
pub fn lista_contiene(salida: &str, nombre: &str) -> bool {
    salida.lines().any(|l| l.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')).any(|w| w == nombre))
}

/// Tabla de texto con columnas alineadas.
pub fn tabla(filas: &[Fila]) -> String {
    let ancho = filas.iter().map(|f| f.nombre.len()).max().unwrap_or(0);
    let mut s = String::new();
    for f in filas {
        let n = match f.nivel {
            Nivel::Ok => "OK   ",
            Nivel::Aviso => "AVISO",
            Nivel::Fallo => "FALLO",
        };
        s.push_str(&format!("{}  {:<w$}  {}\n", n, f.nombre, f.detalle, w = ancho));
    }
    s
}

pub fn codigo_salida(filas: &[Fila]) -> i32 {
    if filas.iter().any(|f| f.nivel == Nivel::Fallo) {
        1
    } else {
        0
    }
}

/// Reemplaza la carpeta personal por ~ para no mostrar el nombre de usuario.
fn corto(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    match std::env::var("HOME") {
        Ok(h) if h.len() > 1 && s.starts_with(&h) => format!("~{}", &s[h.len()..]),
        _ => s,
    }
}

extern "C" {
    fn access(path: *const std::os::raw::c_char, mode: i32) -> i32;
    fn dlopen(name: *const i8, flags: i32) -> *mut c_void;
    fn dlclose(h: *mut c_void) -> i32;
    fn statvfs(path: *const std::os::raw::c_char, buf: *mut u64) -> i32;
}

fn acceso(path: &str, modo: i32) -> bool {
    CString::new(path).is_ok_and(|c| unsafe { access(c.as_ptr(), modo) == 0 })
}

/// Espacio libre (bytes) para un usuario normal en el sistema de archivos de `dir`. struct statvfs de glibc (LP64).
fn espacio_libre(dir: &Path) -> Option<u64> {
    let c = CString::new(dir.to_string_lossy().as_bytes()).ok()?;
    let mut b = [0u64; 16];
    if unsafe { statvfs(c.as_ptr(), b.as_mut_ptr()) } != 0 {
        return None;
    }
    // f_bsize, f_frsize, f_blocks, f_bfree, f_bavail
    Some(b[1].saturating_mul(b[4]))
}

/// Carpeta donde medir el espacio libre para `dir`: ella misma si existe o, si todavia no (no se crea), su antecesora
/// existente mas cercana (donde se creara). None si no existe ninguna. Pura (`existe` dice si una carpeta existe).
fn carpeta_a_medir(dir: &Path, existe: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    dir.ancestors().find(|p| !p.as_os_str().is_empty() && existe(p)).map(Path::to_path_buf)
}

/// La fila del espacio en disco: `libre` (bytes) donde se midio (`medido`), para la carpeta de datos `datos`. Menos de
/// 2 GiB es un fallo (no cabe ni la imagen) y menos de 12 GiB, un aviso. Pura.
fn fila_espacio(libre: Option<u64>, datos: &Path, medido: Option<&Path>) -> Fila {
    let donde = match medido {
        Some(m) if m == datos => txf!("doctor.en_la_carpeta_de_datos", datos.display()),
        Some(m) => txf!("doctor.donde_se_creara_la_carpeta_de_datos", datos.display(), m.display()),
        None => txf!("doctor.para_la_carpeta_de_datos", datos.display()),
    };
    match libre {
        None => fila(Nivel::Aviso, tx!("doctor.espacio_en_disco"), txf!("doctor.no_se_pudo_consultar_el_espacio_libre", donde)),
        Some(b) => {
            let gib = b as f64 / (1u64 << 30) as f64;
            let n = if gib < 2.0 {
                Nivel::Fallo
            } else if gib < 12.0 {
                Nivel::Aviso
            } else {
                Nivel::Ok
            };
            fila(n, tx!("doctor.espacio_en_disco"), txf!("doctor.gib_libres_la_imagen_y_el_disco", format!("{:.1}", gib), donde))
        }
    }
}

/// Por donde se habla con adbd, para la fila de adbd: la via de la maquina (`via`, la de `adb-transport` y `adb-port` del
/// estado) con su CID o su puerto local. Pura.
fn texto_via(via: crate::adb::Via, cid: u32) -> String {
    match via {
        crate::adb::Via::Vsock => format!("vsock:{}", cid),
        crate::adb::Via::Tcp(p) => format!("tcp:127.0.0.1:{}", p),
    }
}

fn buscar_en_path(nombre: &str) -> Option<PathBuf> {
    std::env::var("PATH").ok()?.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join(nombre)).find(|p| p.is_file())
}

fn salida_de(bin: &Path, args: &[&str], ld: Option<&str>) -> Option<String> {
    let mut c = Command::new(bin);
    c.args(args);
    if let Some(l) = ld {
        c.env("LD_LIBRARY_PATH", l);
    }
    let o = c.output().ok()?;
    Some(format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
}

/// Los casos de la fila de aceleracion grafica.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CasoGfx {
    /// el QEMU propio ya la incluye
    Propio,
    /// carpeta gfx/ con las bibliotecas
    Completo,
    /// carpeta gfx/ sin la biblioteca del motor
    SinBackend,
    /// hay gfx/ pero QEMU no trae el dispositivo
    SinDispositivo,
    SinCarpeta,
}

/// La fila de aceleracion grafica: en consola nombra los componentes (gfxstream, rutabaga); en la interfaz, no. `ruta`: carpeta.
pub(crate) fn fila_aceleracion(caso: CasoGfx, ruta: &str) -> Fila {
    let nombre: &'static str = if crate::textos::interfaz() { tx!("doctor.aceleracion_grafica") } else { "gfxstream" };
    match caso {
        CasoGfx::Propio => fila(Nivel::Ok, nombre, tx!("doctor.el_qemu_propio_ya_la_incluye")),
        CasoGfx::Completo => fila(
            Nivel::Ok,
            nombre,
            elige(&txf!("doctor.bibliotecas_de_aceleracion_grafica", ruta), &txf!("doctor_tec.gfx_con_gfx_ffi_so_0_y_backend", ruta)),
        ),
        CasoGfx::SinBackend => fila(
            Nivel::Aviso,
            nombre,
            elige(&txf!("doctor.en_faltan_bibliotecas_de_aceleracion", ruta), &txf!("doctor_tec.en_esta_gfx_ffi_so_0_pero_falta_backend", ruta)),
        ),
        CasoGfx::SinDispositivo => fila(Nivel::Aviso, nombre, elige(tx!("doctor.hay_carpeta_gfx_pero_este_qemu_no_trae"), tx!("doctor_tec.hay_gfx_pero_este_qemu_no_trae_el"))),
        CasoGfx::SinCarpeta => fila(
            Nivel::Aviso,
            nombre,
            elige(
                tx!("doctor.sin_carpeta_gfx_junto_al_ejecutable_ni"),
                tx!("doctor_tec.sin_carpeta_gfx_junto_al_ejecutable_ni"),
            ),
        ),
    }
}

/// Todas las filas posibles de la aceleracion grafica y las del equipo real, para la prueba que recorre la interfaz.
#[cfg(test)]
pub(crate) fn todas_las_filas(st: &State) -> Vec<Fila> {
    let mut v: Vec<Fila> = [CasoGfx::Propio, CasoGfx::Completo, CasoGfx::SinBackend, CasoGfx::SinDispositivo, CasoGfx::SinCarpeta].iter().map(|c| fila_aceleracion(*c, "~/gfx")).collect();
    v.extend(ejecutar(st));
    v
}

pub fn ejecutar(st: &State) -> Vec<Fila> {
    let mut v = Vec::new();
    // --- carpetas: sin HOME ni variables XDG todo cae en subcarpetas del directorio actual (ver rutas.rs)
    if let Some(av) = crate::rutas::carpetas_relativas() {
        v.push(fila(Nivel::Aviso, "carpetas", av));
    }
    // la carpeta de estado de la maquina (y la de ejecucion estandar) tiene que ser propia: si no, las demas ordenes no la usan
    if let Err(e) = crate::rutas::comprobar_estado(&st.dir) {
        v.push(fila(Nivel::Fallo, "carpetas", e));
    }
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok()).and_then(|p| p.parent().map(|d| d.to_string_lossy().into_owned()));
    // --- QEMU: el propio (carpeta qemu/ junto al ejecutable o WEFT_QEMU_DIR) o el del sistema
    let propio = crate::vm::bundled_qemu(std::env::var("WEFT_QEMU_DIR").ok().as_deref(), exe_dir.as_deref());
    let gfx = if propio.is_some() { None } else { crate::vm::gfx_libs(std::env::var("WEFT_GFX_DIR").ok().as_deref(), exe_dir.as_deref()) };
    let (qemu, ld): (Option<PathBuf>, Option<String>) = match &propio {
        Some(d) => (Some(PathBuf::from(format!("{}/bin/qemu-system-x86_64", d))), Some(format!("{}/lib", d))),
        None => (buscar_en_path("qemu-system-x86_64"), gfx.clone()),
    };
    let (mut displays, mut dispositivos) = (String::new(), String::new());
    match &qemu {
        None => v.push(fila(Nivel::Fallo, "qemu-system-x86_64", tx!("doctor.no_esta_en_el_path_ni_hay_un_qemu_propio"))),
        Some(q) => match salida_de(q, &["--version"], ld.as_deref()).and_then(|s| version_qemu(&s).map(|x| (x, s))) {
            None => v.push(fila(Nivel::Fallo, "qemu-system-x86_64", txf!("doctor.no_responde_a_version", corto(q)))),
            Some(((mayor, menor), _)) if mayor < 10 => v.push(fila(Nivel::Fallo, "qemu-system-x86_64", txf!("doctor.version_hace_falta_10_o_mayor", mayor, menor, corto(q)))),
            Some(((mayor, menor), _)) => {
                v.push(fila(Nivel::Ok, "qemu-system-x86_64", txf!("doctor.version", mayor, menor, if propio.is_some() { tx!("doctor.qemu_propio") } else { "" }, corto(q))));
                displays = salida_de(q, &["-display", "help"], ld.as_deref()).unwrap_or_default();
                dispositivos = salida_de(q, &["-device", "help"], ld.as_deref()).unwrap_or_default();
            }
        },
    }
    // --- KVM
    v.push(if crate::vm::kvm_usable() {
        fila(Nivel::Ok, "KVM", tx!("doctor.dev_kvm_accesible"))
    } else if Path::new("/dev/kvm").exists() {
        fila(Nivel::Fallo, "KVM", tx!("doctor.dev_kvm_existe_pero_este_usuario_no"))
    } else {
        fila(Nivel::Fallo, "KVM", tx!("doctor.no_hay_dev_kvm_virtualizacion"))
    });
    // --- vhost-vsock: adb propio (adb-shell, install...), `resolution` y la rotacion automatica de la ventana
    let modulo = Path::new("/sys/module/vhost_vsock").exists();
    v.push(if crate::rutas::en_flatpak() {
        // el filtro seccomp de Flatpak no admite AF_VSOCK: no es un problema, el adb va por TCP (127.0.0.1) y es lo previsto
        fila(Nivel::Ok, "vhost-vsock", tx!("doctor.dentro_de_flatpak_no_se_usa_vsock_el"))
    } else if acceso("/dev/vhost-vsock", 6) {
        fila(Nivel::Ok, "vhost-vsock", tx!("doctor.dev_vhost_vsock_accesible_adb_propio_por"))
    } else if Path::new("/dev/vhost-vsock").exists() {
        fila(Nivel::Aviso, "vhost-vsock", tx!("doctor.dev_vhost_vsock_existe_pero_este_usuario"))
    } else if modulo {
        fila(Nivel::Aviso, "vhost-vsock", tx!("doctor.el_modulo_esta_cargado_pero_falta_dev"))
    } else {
        fila(Nivel::Aviso, "vhost-vsock", tx!("doctor.modulo_vhost_vsock_sin_cargar_modprobe"))
    });
    // --- qemu-ui-dbus (ventana propia)
    if !displays.is_empty() {
        v.push(if lista_contiene(&displays, "dbus") {
            fila(Nivel::Ok, "qemu-ui-dbus", tx!("doctor.qemu_ofrece_la_pantalla_d_bus_display"))
        } else {
            fila(Nivel::Aviso, "qemu-ui-dbus", tx!("doctor.qemu_no_ofrece_display_dbus_paquete_qemu"))
        });
        let otras: Vec<&str> = ["gtk", "sdl"].into_iter().filter(|d| lista_contiene(&displays, d)).collect();
        v.push(if otras.is_empty() {
            if crate::rutas::en_flatpak() {
                fila(Nivel::Ok, tx!("doctor.ventanas_gtk_sdl"), tx!("doctor.dentro_de_flatpak_solo_hay_ventana"))
            } else {
                fila(Nivel::Aviso, tx!("doctor.ventanas_gtk_sdl"), tx!("doctor.qemu_no_trae_ninguna_qemu_ui_gtk_qemu_ui"))
            }
        } else {
            fila(Nivel::Ok, tx!("doctor.ventanas_gtk_sdl"), txf!("doctor.qemu_ofrece", otras.join(", ")))
        });
    }
    // --- SDL3 (ventana propia)
    let sdl = ["libSDL3.so.0", "libSDL3.so"].iter().any(|n| {
        let c = CString::new(*n).unwrap();
        let h = unsafe { dlopen(c.as_ptr(), 1 /* RTLD_LAZY */) };
        if !h.is_null() {
            unsafe { dlclose(h) };
        }
        !h.is_null()
    });
    v.push(if sdl {
        fila(Nivel::Ok, "SDL3", tx!("doctor.libsdl3_so_0_se_carga_display_window"))
    } else {
        fila(Nivel::Aviso, "SDL3", tx!("doctor.no_se_carga_libsdl3_so_0_paquete_sdl3"))
    });
    // --- aceleracion grafica (gfxstream en consola; la interfaz no nombra el componente): dispositivo de QEMU y bibliotecas
    if propio.is_some() {
        v.push(fila_aceleracion(CasoGfx::Propio, ""));
    } else {
        let tiene_dev = dispositivos.is_empty() || lista_contiene(&dispositivos, "virtio-gpu-rutabaga");
        match (&gfx, tiene_dev) {
            (Some(d), true) => {
                let backend = std::fs::read_dir(d).map(|r| r.flatten().any(|e| e.file_name().to_string_lossy().starts_with("libgfxstream_backend"))).unwrap_or(false);
                v.push(fila_aceleracion(if backend { CasoGfx::Completo } else { CasoGfx::SinBackend }, &corto(Path::new(d))));
            }
            (Some(_), false) => v.push(fila_aceleracion(CasoGfx::SinDispositivo, "")),
            (None, _) => v.push(fila_aceleracion(CasoGfx::SinCarpeta, "")),
        }
    }
    // --- virtiofsd: las carpetas compartidas con Android (`share`, pantalla de configuracion)
    v.push(match crate::compartir::buscar_virtiofsd() {
        Some(p) => fila(Nivel::Ok, "virtiofsd", txf!("doctor.carpetas_compartidas_con_android_share", p)),
        None => fila(Nivel::Aviso, "virtiofsd", tx!("doctor.no_esta_instalado_paquete_virtiofsd_sin")),
    });
    // --- mandos de juegos
    let uinput = acceso("/dev/uinput", 6);
    v.push(if uinput {
        fila(Nivel::Ok, "/dev/uinput", "accesible")
    } else if Path::new("/dev/uinput").exists() {
        fila(Nivel::Aviso, "/dev/uinput", tx!("doctor.existe_pero_este_usuario_no_puede"))
    } else {
        fila(Nivel::Aviso, "/dev/uinput", tx!("doctor.no_existe_modulo_uinput_solo_hace_falta"))
    });
    let (mut total, mut leibles) = (0, 0);
    if let Ok(rd) = std::fs::read_dir("/dev/input") {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with("event") {
                total += 1;
                if acceso(&e.path().to_string_lossy(), 4) {
                    leibles += 1;
                }
            }
        }
    }
    v.push(if total == 0 {
        fila(Nivel::Aviso, "/dev/input", tx!("doctor.no_hay_dispositivos_de_entrada"))
    } else if leibles == total {
        fila(Nivel::Ok, "/dev/input", txf!("doctor.dispositivos_event_legibles_mandos_de", total))
    } else if leibles == 0 {
        fila(Nivel::Aviso, "/dev/input", txf!("doctor.ninguno_de_los_dispositivos_event_es", total))
    } else {
        fila(Nivel::Aviso, "/dev/input", txf!("doctor.de_dispositivos_event_legibles_los_demas", leibles, total))
    });
    // --- espacio libre en la carpeta de datos de la maquina (imagenes y discos: unos 12 GiB mas la particion de datos); si
    // aun no existe (no se crea aqui) se mide donde se creara
    let cfg = crate::config::Config::cargar(&st.dir);
    let datos = crate::rutas::actual().con_claves(&cfg.get("dir.datos"), &cfg.get("dir.cache")).datos;
    let datos = std::env::current_dir().map(|d| d.join(&datos)).unwrap_or(datos);
    let medido = carpeta_a_medir(&datos, &|p: &Path| p.is_dir());
    v.push(fila_espacio(medido.as_deref().and_then(espacio_libre), &datos, medido.as_deref()));
    // --- maquina en marcha: adbd, por la via con que arranco (adb-transport y adb-port del estado)
    if st.running() {
        let cid = crate::adbcmd::cid_del_estado(st);
        let via = texto_via(crate::adbcmd::via_del_estado(st), cid);
        v.push(match crate::adb::shell_una_vez(cid, "getprop sys.boot_completed; getprop ro.build.version.release", 5) {
            Ok((_, o, _)) => {
                let mut l = o.lines();
                let (boot, ver) = (l.next().unwrap_or("").trim(), l.next().unwrap_or("").trim());
                if boot == "1" {
                    fila(Nivel::Ok, "adbd", txf!("doctor.responde_por_android_arranque_completo", via, ver))
                } else {
                    fila(Nivel::Aviso, "adbd", txf!("doctor.responde_por_pero_android_aun_esta", via))
                }
            }
            Err(e) => fila(Nivel::Aviso, "adbd", txf!("doctor.la_maquina_esta_en_marcha_pero_adbd_no", via, e)),
        });
    } else {
        v.push(fila(Nivel::Ok, "maquina", tx!("doctor.no_hay_una_maquina_en_marcha_nada_que")));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version() {
        assert_eq!(version_qemu("QEMU emulator version 10.2.2 (qemu-10.2.2-1.fc44)\nCopyright"), Some((10, 2)));
        assert_eq!(version_qemu("QEMU emulator version 9.0.0\n"), Some((9, 0)));
        assert_eq!(version_qemu("QEMU emulator version 11\n"), Some((11, 0)));
        assert_eq!(version_qemu("otra cosa"), None);
        assert_eq!(version_qemu(""), None);
    }

    #[test]
    fn listas() {
        let d = "Available display backend types:\ncocoa\ndbus\ngtk\nsdl\nnone\n";
        assert!(lista_contiene(d, "dbus") && lista_contiene(d, "gtk"));
        assert!(!lista_contiene(d, "db"));
        let dev = "Display devices:\nname \"virtio-gpu-pci\", bus PCI, alias \"virtio-vga\"\nname \"virtio-gpu-rutabaga\", bus virtio-bus\n";
        assert!(lista_contiene(dev, "virtio-gpu-rutabaga"));
        assert!(!lista_contiene(dev, "virtio-gpu-gl"));
    }

    #[test]
    fn salida_y_tabla() {
        let f = vec![fila(Nivel::Ok, "KVM", "bien"), fila(Nivel::Aviso, "vhost-vsock", "mal")];
        assert_eq!(codigo_salida(&f), 0);
        let t = tabla(&f);
        assert!(t.starts_with("OK     KVM          bien\n"), "{:?}", t);
        assert!(t.contains("AVISO  vhost-vsock  mal\n"));
        let g = vec![fila(Nivel::Ok, "a", ""), fila(Nivel::Fallo, "b", "x")];
        assert_eq!(codigo_salida(&g), 1);
        assert_eq!(codigo_salida(&[]), 0);
    }

    #[test]
    fn disco() {
        assert!(espacio_libre(Path::new(".")).is_some_and(|b| b > 0));
    }

    #[test]
    fn el_espacio_se_mide_en_la_carpeta_de_datos_o_donde_se_creara() {
        let hay = ["/", "/home", "/home/maria", "/home/maria/.local"];
        let existe = |p: &Path| hay.iter().any(|h| Path::new(h) == p);
        let datos = Path::new("/home/maria/.local/share/weft");
        // aun no existe: la antecesora existente mas cercana (no se crea nada)
        assert_eq!(carpeta_a_medir(datos, &existe), Some(PathBuf::from("/home/maria/.local")));
        assert_eq!(carpeta_a_medir(Path::new("/home/maria"), &existe), Some(PathBuf::from("/home/maria")));
        assert_eq!(carpeta_a_medir(Path::new("/otra/cosa"), &existe), Some(PathBuf::from("/")));
        assert_eq!(carpeta_a_medir(Path::new("relativa/x"), &|_: &Path| false), None);
        // en el equipo real: una carpeta que no existe se mide en su padre
        let tmp = std::env::temp_dir();
        assert_eq!(carpeta_a_medir(&tmp.join("weft-doctor-no-existe-seguro/datos"), &|p: &Path| p.is_dir()), Some(tmp.clone()));
    }

    #[test]
    fn fila_del_espacio_dice_donde_se_mide() {
        let datos = Path::new("/datos/weft");
        let gib = 1u64 << 30;
        let f = fila_espacio(Some(20 * gib), datos, Some(datos));
        assert_eq!(f.nivel, Nivel::Ok);
        assert!(f.detalle.starts_with("20.0 GiB libres en la carpeta de datos (/datos/weft)"), "{}", f.detalle);
        assert!(!f.detalle.contains("carpeta de trabajo"), "{}", f.detalle);
        let f = fila_espacio(Some(5 * gib), datos, Some(Path::new("/datos")));
        assert_eq!(f.nivel, Nivel::Aviso);
        assert!(f.detalle.contains("donde se creara la carpeta de datos (/datos/weft, medido en /datos)"), "{}", f.detalle);
        assert_eq!(fila_espacio(Some(gib), datos, Some(datos)).nivel, Nivel::Fallo);
        let f = fila_espacio(None, datos, None);
        assert_eq!(f.nivel, Nivel::Aviso);
        assert!(f.detalle.contains("no se pudo consultar") && f.detalle.contains("/datos/weft"), "{}", f.detalle);
    }

    #[test]
    fn la_fila_de_adbd_dice_la_via_real() {
        assert_eq!(texto_via(crate::adb::Via::Vsock, 3), "vsock:3");
        assert_eq!(texto_via(crate::adb::Via::Tcp(15555), 3), "tcp:127.0.0.1:15555");
        // la via sale del estado de la maquina (adb-transport y adb-port)
        let d = std::env::temp_dir().join(format!("weft-doctor-via-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let st = State { dir: d.clone() };
        assert_eq!(texto_via(crate::adbcmd::via_del_estado(&st), 7), "vsock:7");
        std::fs::write(d.join("adb-transport"), "tcp\n").unwrap();
        std::fs::write(d.join("adb-port"), "15601\n").unwrap();
        assert_eq!(texto_via(crate::adbcmd::via_del_estado(&st), 7), "tcp:127.0.0.1:15601");
        let _ = std::fs::remove_dir_all(&d);
    }
}
