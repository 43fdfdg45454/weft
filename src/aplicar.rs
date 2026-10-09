//! Ajustes dentro de Android que weft aplica por el adb propio tras cada arranque: Bluetooth apagado (no hay controlador
//! emulado y Android avisaria de que "Bluetooth se detuvo") y, con el puntero wacom (ventanas gtk/sdl), los archivos idc que hacen que Android trate la tableta USB como pantalla
//! tactil. `weft apply-settings` es idempotente: lo que ya esta hecho no se repite. Lo llaman el guion y el hilo
//! `vigilar` de la ventana tras cada arranque de Android.
//!
//! Decision: orden propia (y no dentro de `share setup` o `root ensure`) porque no tiene que ver con carpetas ni con root,
//! y porque su contrato es distinto: siempre se aplica, la gobiernan `android.bluetooth` y el puntero con que arranco la
//! maquina (archivo `pointer` del estado).

use crate::adb;
use crate::config::Config;
use crate::qmp::Qmp;
use crate::json::V;
use crate::vm::State;
use std::time::{Duration, Instant};
use crate::textos::{tx, txf};

pub const DIR_IDC: &str = "/data/system/devices/idc";
pub const IDC_TABLETA: &str = "Vendor_056a_Product_0000.idc";
pub const IDC_NOMBRE: &str = "Wacom_Penpartner_Pen.idc";

/// Orden de shell que apaga Bluetooth.
pub fn orden_bluetooth() -> &'static str {
    "cmd bluetooth_manager disable; settings put global bluetooth_on 0"
}

/// ¿Esta ya el idc de la tableta?
pub fn orden_comprobar_idc() -> String {
    format!("test -f {}/{} && echo si", DIR_IDC, IDC_TABLETA)
}

/// Escribe los dos archivos idc con su dueno, modo y etiqueta de SELinux.
pub fn orden_escribir_idc() -> String {
    let i = DIR_IDC;
    format!(
        "mkdir -p {i} && printf 'touch.deviceType = touchScreen\\ntouch.orientationAware = 1\\ndevice.internal = 1\\n' > {i}/{t} && cp {i}/{t} {i}/{n} && chown -R system:system /data/system/devices && chmod 755 /data/system/devices {i} && chmod 644 {i}/*.idc && restorecon -R /data/system/devices",
        i = i,
        t = IDC_TABLETA,
        n = IDC_NOMBRE
    )
}

/// Desenchufa y vuelve a enchufar la tableta USB: Android relee su configuracion al detectarla de nuevo.
pub fn replug_pointer(st: &State) -> Result<(), String> {
    let mut q = Qmp::connect(&st.qmp())?;
    q.exec("device_del", Some(V::obj(&[("id", V::s("ptr0"))])))?;
    let add = V::obj(&[("driver", V::s("usb-wacom-tablet")), ("bus", V::s("usb0.0")), ("id", V::s("ptr0"))]);
    let t0 = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(500));
        match q.exec("device_add", Some(add.clone())) {
            Ok(_) => return Ok(()),
            Err(e) if t0.elapsed() > Duration::from_secs(10) => return Err(e),
            Err(_) => {}
        }
    }
}

/// Puntero con que arranco la maquina (`pointer` del estado), si se sabe.
pub fn puntero(st: &State) -> Option<String> {
    std::fs::read_to_string(st.f("pointer")).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// Aplica los ajustes y devuelve las lineas de informe (`bluetooth: apagado`, `raton como toque: ...`), que el guion
/// muestra tal cual. `espera_s`: cuanto esperar a que Android termine de arrancar.
pub fn aplicar(st: &State, cid: u32, cfg: &Config, espera_s: u64) -> Result<Vec<String>, String> {
    // adbd contesta antes de que Android este listo del todo (y reinicia al pasar a root): un fallo de conexion se reintenta unos
    // segundos en vez de dar el aviso de "no se pudieron aplicar los ajustes" por una carrera de arranque
    let t0 = Instant::now();
    let limite = Duration::from_secs(espera_s.clamp(30, 90));
    let mut intento = 0;
    loop {
        intento += 1;
        match aplicar_una_vez(st, cid, cfg, espera_s) {
            Ok(mut v) => {
                if intento > 1 {
                    v.push(txf!("aplicar.aplicado_en_el_intento_adbd_aun_no", intento));
                }
                return Ok(v);
            }
            Err(e) if t0.elapsed() >= limite => return Err(txf!("aplicar.tras_intentos_en_s", e, intento, format!("{:.0}", t0.elapsed().as_secs_f64()))),
            Err(_) => std::thread::sleep(Duration::from_secs(3)),
        }
    }
}

fn aplicar_una_vez(st: &State, cid: u32, cfg: &Config, espera_s: u64) -> Result<Vec<String>, String> {
    let mut v = Vec::new();
    adb::esperar(cid, espera_s, true)?;
    adb::hacer_root(cid)?;
    if cfg.bool("android.bluetooth") {
        v.push(tx!("aplicar.bluetooth_sin_tocar_android_bluetooth_si").to_string());
    } else {
        adb::conectar(cid, 15)?.shell_texto(orden_bluetooth())?;
        v.push("bluetooth: apagado".to_string());
    }
    if puntero(st).as_deref() == Some("wacom") {
        let (_, o, _) = adb::conectar(cid, 15)?.shell_texto(&orden_comprobar_idc())?;
        if o.trim() == "si" {
            v.push(tx!("aplicar.raton_como_toque_ya_estaba_configurado").to_string());
        } else {
            let (c, _, _) = adb::conectar(cid, 15)?.shell_texto(&orden_escribir_idc())?;
            match if c == 0 { replug_pointer(st) } else { Err(tx!("aplicar.no_se_pudieron_escribir_los_archivos_idc").to_string()) } {
                Ok(()) => v.push(tx!("aplicar.raton_como_toque_configurado").to_string()),
                Err(e) => v.push(txf!("aplicar.aviso_no_se_pudo_configurar_el_raton", e)),
            }
        }
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordenes_de_android() {
        assert!(orden_bluetooth().contains("cmd bluetooth_manager disable") && orden_bluetooth().contains("bluetooth_on 0"));
        assert_eq!(orden_comprobar_idc(), "test -f /data/system/devices/idc/Vendor_056a_Product_0000.idc && echo si");
        let o = orden_escribir_idc();
        for parte in ["touch.deviceType = touchScreen", "touch.orientationAware = 1", "device.internal = 1", "Wacom_Penpartner_Pen.idc", "chown -R system:system", "restorecon -R /data/system/devices"] {
            assert!(o.contains(parte), "{}", parte);
        }
    }

    #[test]
    fn puntero_del_estado() {
        let d = std::env::temp_dir().join(format!("ar-aplicar-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let st = State { dir: d.clone() };
        assert_eq!(puntero(&st), None);
        std::fs::write(d.join("pointer"), "wacom\n").unwrap();
        assert_eq!(puntero(&st).as_deref(), Some("wacom"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
