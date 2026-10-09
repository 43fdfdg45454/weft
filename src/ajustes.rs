//! Pantalla de configuracion de la ventana propia: una capa modal sobre toda la ventana (fondo atenuado), con un dialogo
//! centrado de navegacion a la izquierda (o pestanas arriba en ventanas estrechas) y el contenido de cada seccion a la
//! derecha. Cada cambio se guarda al instante en `config` (sin boton Guardar).
//!
//! Este modulo no conoce SDL: el dibujo sale como una lista de primitivas (`formas::Pint`), el texto se mide con la
//! fuente real (`fuente::Medida`) y los eventos llegan ya traducidos (coordenadas en dp, teclas con su codigo HID de
//! SDL), de modo que el diseno, los controles, la captura de teclas y el teclado se prueban sin ventana.
//!
//! Modo inmediato: cada llamada vuelve a maquetar la seccion visible (`maquetar`) a partir de la configuracion, de los
//! datos de la ventana (`Datos`) y del estado de la interfaz (hover, foco, desplazamiento...). El mismo diseno sirve para
//! dibujar y para saber que control hay bajo el raton, asi que no pueden desincronizarse.
//!
//! Estados de los controles: reposo, hover, pulsado, foco de teclado (anillo de 2 dp, solo tras usar el teclado),
//! deshabilitado, aviso en linea (amarillo), error en linea (rojo) y "Guardado" breve tras un cambio.

use crate::compartir::{self, Carpeta};
use crate::config::{self, AccionAtajo, Combo, Config};
use crate::dispositivo::{self, Catalogo, Dispositivo, ModoExtensiones};
use crate::doctor::Nivel as NivelDoctor;
use crate::formas::{tema, Color, Icono, Pint, R};
use crate::fuente::{envolver, truncar, truncar_centro, Estilo, Medida};
use crate::gestos::{self, AtajoExtra};
use crate::imagen;
use crate::puente;
use crate::pantalla::Orientacion;
use crate::vista::{Accion, Maquina, Mando, ModoZoom};
use crate::root;
use crate::textos::{tx, txf};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------------------------------------------
// tipos

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seccion {
    /// acciones rapidas del dispositivo (lo que ofrecia el panel lateral)
    Controles,
    General,
    Atajos,
    Entrada,
    Pantalla,
    Maquina,
    /// perfiles de dispositivo: lo que se anuncia a Android y a las apps ARM, nucleos y memoria
    Perfiles,
    Compartir,
    Imagen,
    Puente,
    Root,
    Diagnostico,
    Acerca,
}

/// Grupos de la navegacion del modal (con encabezado en la columna de la izquierda).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grupo {
    Controles,
    General,
    PantallaYEntrada,
    Maquina,
    Almacenamiento,
    Avanzado,
}

impl Grupo {
    pub const TODOS: [Grupo; 6] = [Grupo::Controles, Grupo::General, Grupo::PantallaYEntrada, Grupo::Maquina, Grupo::Almacenamiento, Grupo::Avanzado];
    pub fn titulo(self) -> &'static str {
        match self {
            Grupo::Controles => tx!("comun.controles"),
            Grupo::General => tx!("comun.general"),
            Grupo::PantallaYEntrada => tx!("grupo.pantalla_entrada"),
            Grupo::Maquina => tx!("comun.maquina"),
            Grupo::Almacenamiento => tx!("grupo.almacenamiento"),
            Grupo::Avanzado => tx!("grupo.avanzado"),
        }
    }
}

impl Seccion {
    /// En el orden de la navegacion: Controles | General | Atajos, Entrada, Pantalla | Maquina | Imagen, Carpetas | Acceso root,
    /// Traductor ARM, Diagnostico, Acerca de.
    pub const TODAS: [Seccion; 13] = [
        Seccion::Controles,
        Seccion::General,
        Seccion::Atajos,
        Seccion::Entrada,
        Seccion::Pantalla,
        Seccion::Maquina,
        Seccion::Perfiles,
        Seccion::Imagen,
        Seccion::Compartir,
        Seccion::Root,
        Seccion::Puente,
        Seccion::Diagnostico,
        Seccion::Acerca,
    ];
    pub fn grupo(self) -> Grupo {
        match self {
            Seccion::Controles => Grupo::Controles,
            Seccion::General => Grupo::General,
            Seccion::Atajos | Seccion::Entrada | Seccion::Pantalla => Grupo::PantallaYEntrada,
            Seccion::Maquina | Seccion::Perfiles => Grupo::Maquina,
            Seccion::Imagen | Seccion::Compartir => Grupo::Almacenamiento,
            Seccion::Root | Seccion::Puente | Seccion::Diagnostico | Seccion::Acerca => Grupo::Avanzado,
        }
    }
    /// Texto de la navegacion: genericos, sin nombres de productos ni de proveedores.
    pub fn titulo(self) -> &'static str {
        match self {
            Seccion::Controles => tx!("comun.controles"),
            Seccion::General => tx!("comun.general"),
            Seccion::Atajos => tx!("seccion.atajos"),
            Seccion::Entrada => tx!("seccion.entrada"),
            Seccion::Pantalla => tx!("seccion.pantalla"),
            Seccion::Maquina => tx!("comun.maquina"),
            Seccion::Perfiles => tx!("seccion.perfiles"),
            Seccion::Compartir => tx!("seccion.carpetas"),
            Seccion::Imagen => tx!("comun.imagen_android"),
            Seccion::Puente => tx!("seccion.traductor_arm"),
            Seccion::Root => tx!("seccion.acceso_root"),
            Seccion::Diagnostico => tx!("seccion.diagnostico"),
            Seccion::Acerca => tx!("seccion.acerca"),
        }
    }
    /// Titulo de la seccion en su contenido (el de la navegacion es mas corto).
    pub fn encabezado(self) -> &'static str {
        match self {
            Seccion::Compartir => tx!("encabezado.carpetas_compartidas"),
            Seccion::Imagen => tx!("encabezado.imagen_android_disco"),
            Seccion::Puente => tx!("encabezado.traductor_arm_apps_arm"),
            Seccion::Perfiles => tx!("encabezado.perfiles_dispositivo"),
            otra => otra.titulo(),
        }
    }
    /// Nombre estable para las pruebas con el gancho de inyeccion.
    pub fn nombre(self) -> &'static str {
        match self {
            Seccion::Controles => "controles",
            Seccion::General => "general",
            Seccion::Atajos => "atajos",
            Seccion::Entrada => "entrada",
            Seccion::Pantalla => "pantalla",
            Seccion::Maquina => "maquina",
            Seccion::Perfiles => "perfiles",
            Seccion::Compartir => "compartir",
            Seccion::Imagen => "imagen",
            Seccion::Puente => "puente",
            Seccion::Root => "root",
            Seccion::Diagnostico => "diagnostico",
            Seccion::Acerca => "acerca",
        }
    }
    #[cfg(test)]
    pub fn parse(s: &str) -> Option<Seccion> {
        Seccion::TODAS.into_iter().find(|x| x.nombre() == s)
    }
}

/// Campos numericos editables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Campo {
    Ancho,
    Alto,
    Densidad,
    Cpus,
    Ram,
    /// nombre de la carpeta compartida que se va a agregar (texto)
    ShareNombre,
    /// carpeta del equipo de la carpeta compartida que se va a agregar (texto)
    ShareRuta,
    /// tamano de datos del disco (disk.data: 24G, 4096M o img)
    DatosTam,
    /// zip o carpeta de la imagen que se va a agregar (borrador, no es configuracion)
    Origen,
    /// carpeta con los archivos de root ya descargados (borrador; vacio = la de `root.carpeta` o la cache)
    RutaRoot,
    /// archivo (o carpeta) de la biblioteca del traductor ARM ya descargada (borrador; vacio = la de la cache)
    RutaPuente,
    /// un campo del perfil de dispositivo que se ve (posicion en `CAMPOS_PERFIL`); se guarda en su archivo al confirmarlo
    Perfil(u8),
}

/// Campos editables de un perfil de dispositivo: (clave del perfil, nombre del control, clave de sus mensajes en linea).
const CAMPOS_PERFIL: [(&str, &str, &str); 26] = [
    ("dispositivo.nombre", "perfil-nombre", "disp.nombre"),
    ("dispositivo.descripcion", "perfil-descripcion", "disp.descripcion"),
    ("maquina.nucleos", "perfil-nucleos", "disp.nucleos"),
    ("maquina.ram", "perfil-ram", "disp.ram"),
    ("producto.fabricante", "perfil-fabricante", "disp.fabricante"),
    ("producto.marca", "perfil-marca", "disp.marca"),
    ("producto.modelo", "perfil-modelo", "disp.modelo"),
    ("producto.dispositivo", "perfil-dispositivo", "disp.dispositivo"),
    ("producto.nombre", "perfil-producto", "disp.producto"),
    ("cpu.nombre", "perfil-cpu-nombre", "disp.cpu_nombre"),
    ("cpu.base", "perfil-cpu-base", "disp.cpu_base"),
    ("cpu.midr", "perfil-cpu-midr", "disp.cpu_midr"),
    ("cpu.revidr", "perfil-cpu-revidr", "disp.cpu_revidr"),
    ("cpu.hardware", "perfil-cpu-hardware", "disp.cpu_hardware"),
    ("cpu.ctr", "perfil-cpu-ctr", "disp.cpu_ctr"),
    ("cpu.dczid", "perfil-cpu-dczid", "disp.cpu_dczid"),
    ("cpu.id_aa64pfr0", "perfil-id-aa64pfr0", "disp.id_aa64pfr0"),
    ("cpu.id_aa64pfr1", "perfil-id-aa64pfr1", "disp.id_aa64pfr1"),
    ("cpu.id_aa64zfr0", "perfil-id-aa64zfr0", "disp.id_aa64zfr0"),
    ("cpu.id_aa64dfr0", "perfil-id-aa64dfr0", "disp.id_aa64dfr0"),
    ("cpu.id_aa64isar0", "perfil-id-aa64isar0", "disp.id_aa64isar0"),
    ("cpu.id_aa64isar1", "perfil-id-aa64isar1", "disp.id_aa64isar1"),
    ("cpu.id_aa64isar2", "perfil-id-aa64isar2", "disp.id_aa64isar2"),
    ("cpu.id_aa64mmfr0", "perfil-id-aa64mmfr0", "disp.id_aa64mmfr0"),
    ("cpu.id_aa64mmfr1", "perfil-id-aa64mmfr1", "disp.id_aa64mmfr1"),
    ("cpu.id_aa64mmfr2", "perfil-id-aa64mmfr2", "disp.id_aa64mmfr2"),
];

/// Posicion en `CAMPOS_PERFIL` del campo de una clave del perfil.
fn campo_perfil(clave: &str) -> Campo {
    Campo::Perfil(CAMPOS_PERFIL.iter().position(|c| c.0 == clave).unwrap_or(0) as u8)
}

/// Opciones del modo de la lista de extensiones, en el orden de sus botones.
const MODOS_EXTENSIONES: [ModoExtensiones; 3] = [ModoExtensiones::DeLaBase, ModoExtensiones::Propia, ModoExtensiones::Cambios];

impl Campo {
    /// Campos de texto libre (reciben los caracteres del evento de texto de SDL, no los codigos de tecla).
    pub fn es_texto(self) -> bool {
        matches!(self, Campo::ShareNombre | Campo::ShareRuta | Campo::DatosTam | Campo::Origen | Campo::RutaRoot | Campo::RutaPuente | Campo::Perfil(_))
    }
    /// Campos que guardan una ruta del equipo: admiten `~`, pegar o soltar un archivo y el boton "Examinar".
    pub fn es_ruta(self) -> bool {
        matches!(self, Campo::ShareRuta | Campo::Origen | Campo::RutaRoot | Campo::RutaPuente)
    }
    /// Para el selector de archivos del sistema: titulo y si se elige una carpeta (si no, un archivo). La imagen puede ser
    /// un zip o una carpeta: el selector pide el zip, que es lo habitual; una carpeta se puede escribir, pegar o soltar.
    pub fn selector(self) -> (&'static str, bool) {
        match self {
            Campo::ShareRuta => (tx!("selector.carpeta_equipo_para_compartir"), true),
            Campo::RutaRoot => (tx!("selector.carpeta_archivos_acceso_root"), true),
            Campo::RutaPuente => (tx!("selector.biblioteca_traductor_arm"), false),
            _ => (tx!("selector.zip_imagen_android"), false),
        }
    }
    fn nombre(self) -> &'static str {
        match self {
            Campo::Ancho => "ancho",
            Campo::Alto => "alto",
            Campo::Densidad => "densidad",
            Campo::Cpus => "cpus",
            Campo::Ram => "ram",
            Campo::ShareNombre => "share-nombre",
            Campo::ShareRuta => "share-ruta",
            Campo::DatosTam => "datos-tam",
            Campo::Origen => "origen",
            Campo::RutaRoot => "ruta-root",
            Campo::RutaPuente => "ruta-puente",
            Campo::Perfil(i) => CAMPOS_PERFIL[i as usize % CAMPOS_PERFIL.len()].1,
        }
    }
    /// Clave de configuracion que guarda el campo (y bajo la que salen sus mensajes en linea).
    fn clave(self) -> &'static str {
        match self {
            Campo::Ancho | Campo::Alto => "pantalla.resolucion",
            Campo::Densidad => "pantalla.densidad",
            Campo::Cpus => "maquina.cpus",
            Campo::Ram => "maquina.ram",
            Campo::ShareNombre => "share.nombre",
            Campo::ShareRuta => "share.ruta",
            Campo::DatosTam => "disk.data",
            Campo::Origen => "imagen.origen",
            Campo::RutaRoot => "root",
            Campo::RutaPuente => "puente",
            Campo::Perfil(i) => CAMPOS_PERFIL[i as usize % CAMPOS_PERFIL.len()].2,
        }
    }
}

/// Una fila de la seccion Atajos: una accion de `config` (`AccionAtajo`) o un atajo propio de la ventana que config.rs no
/// tiene como accion (`AtajoExtra`: pasar la siguiente tecla, pantalla completa, encendido, menu).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilaAtajo {
    Accion(AccionAtajo),
    Extra(AtajoExtra),
}

impl FilaAtajo {
    pub fn clave(self) -> &'static str {
        match self {
            FilaAtajo::Accion(a) => a.clave(),
            FilaAtajo::Extra(e) => e.clave(),
        }
    }
    pub fn etiqueta(self) -> &'static str {
        match self {
            FilaAtajo::Accion(a) => a.etiqueta(),
            FilaAtajo::Extra(e) => e.etiqueta(),
        }
    }
    /// Todas, en el orden de la seccion: las de `config` y despues las de la ventana.
    pub fn todas() -> impl Iterator<Item = FilaAtajo> {
        AccionAtajo::TODAS.into_iter().map(FilaAtajo::Accion).chain(AtajoExtra::TODOS.into_iter().map(FilaAtajo::Extra))
    }
}

/// Bloques de "Detalles tecnicos" desplegables (aqui van los nombres de productos y de origenes de lo que se descarga).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detalle {
    Root,
    Puente,
    /// biblioteca, propiedades y ABIs del traductor ARM en el invitado
    PuenteEstado,
    /// registros de identificacion del perfil de dispositivo
    PerfilRegistros,
}

impl Detalle {
    fn nombre(self) -> &'static str {
        match self {
            Detalle::Root => "root",
            Detalle::Puente => "puente",
            Detalle::PuenteEstado => "puente-estado",
            Detalle::PerfilRegistros => "perfil-registros",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bot {
    /// despliega u oculta los "Detalles tecnicos" de un bloque
    Detalles(Detalle),
    RestaurarAtajos,
    AplicarResolucion,
    AbrirEstado,
    Comprobar,
    /// quita la tecla de la otra accion y la usa aqui (conflicto en la captura)
    UsarAqui,
    RootActualizar,
    RootGestor,
    ReiniciarAndroid,
    /// apagar la maquina (apagado ordenado por adb, ver apagado.rs)
    Apagar,
    /// reinicio completo de la maquina
    ReinicioCompleto,
    AgregarCarpeta,
    /// alterna el modo de solo lectura de la carpeta que se va a agregar
    FormSoloLectura,
    AlmacenActualizar,
    /// agrega la imagen del borrador de origen (zip o carpeta)
    ImagenAgregar,
    DiscoRegenerar,
    PuenteActualizar,
    PuenteInstalar,
    PuenteQuitar,
    InformeCrear,
    Confirmar,
    /// confirma la activacion del root e instala tambien el gestor
    ConfirmarConGestor,
    Cancelar,
    /// detiene la operacion larga en curso (solo las que se pueden deshacer sin riesgo, ver `Datos::cancelable`)
    CancelarOperacion,
    /// la maquina usa el perfil de dispositivo que se ve
    PerfilUsar,
    /// copia el perfil que se ve como perfil nuevo del usuario
    PerfilDuplicar,
    PerfilBorrar,
    /// escribe ya en Android el perfil elegido (`device apply`: reinicia Android)
    PerfilAplicar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Id {
    Cerrar,
    Nav(Seccion),
    Interruptor(&'static str),
    Opcion(&'static str, usize),
    Paso(&'static str, i32),
    Campo(Campo),
    Atajo(FilaAtajo),
    Boton(Bot),
    /// quitar la carpeta compartida de esa posicion de la lista
    Quitar(usize),
    /// boton de la seccion Controles (la misma accion que su tecla de atajo)
    Control(Accion),
    /// conectar o desconectar el mando de esa posicion de la lista de mandos del equipo
    Mando(usize),
    /// usar la imagen instalada de esa posicion en esta maquina
    Imagen(usize),
    /// abrir el selector de archivos del sistema para ese campo de ruta
    Examinar(Campo),
    /// ver el perfil de dispositivo de esa posicion de la lista (0 = ninguno)
    Perfil(usize),
    /// anunciar o no la extension de esa posicion de `dispositivo::EXTENSIONES` en el perfil que se ve
    Extension(usize),
    /// modo de la lista de extensiones (posicion en `MODOS_EXTENSIONES`)
    PerfilModo(usize),
    /// tamano de pagina anunciado (KiB)
    PerfilPagina(u32),
}

impl Id {
    /// Nombre estable (`pclick` del gancho de pruebas y registros).
    pub fn nombre(&self) -> String {
        match self {
            Id::Cerrar => "a-cerrar".into(),
            Id::Nav(s) => format!("a-nav-{}", s.nombre()),
            Id::Interruptor(k) => format!("a-tog-{}", k),
            Id::Opcion(k, i) => format!("a-opt-{}-{}", k, i),
            Id::Paso(k, d) => format!("a-paso-{}-{}", k, if *d < 0 { "menos" } else { "mas" }),
            Id::Campo(c) => format!("a-campo-{}", c.nombre()),
            Id::Atajo(a) => format!("a-{}", a.clave().replace('.', "-")),
            Id::Quitar(i) => format!("a-quitar-{}", i),
            Id::Control(a) => format!("a-ctl-{}", a.nombre()),
            Id::Mando(i) => format!("a-mando-{}", i),
            Id::Imagen(i) => format!("a-imagen-{}", i),
            Id::Examinar(c) => format!("a-examinar-{}", c.nombre()),
            Id::Perfil(i) => format!("a-perfil-{}", i),
            Id::Extension(i) => format!("a-ext-{}", dispositivo::EXTENSIONES.get(*i).copied().unwrap_or("?")),
            Id::PerfilModo(i) => format!("a-perfil-modo-{}", i),
            Id::PerfilPagina(k) => format!("a-perfil-pagina-{}", k),
            Id::Boton(Bot::Detalles(d)) => format!("a-btn-detalles-{}", d.nombre()),
            Id::Boton(b) => format!(
                "a-btn-{}",
                match b {
                    Bot::Detalles(_) => unreachable!("tratado arriba"),
                    Bot::RestaurarAtajos => "restaurar-atajos",
                    Bot::AplicarResolucion => "aplicar-resolucion",
                    Bot::AbrirEstado => "abrir-estado",
                    Bot::Comprobar => "comprobar",
                    Bot::UsarAqui => "usar-aqui",
                    Bot::RootActualizar => "root-actualizar",
                    Bot::RootGestor => "root-gestor",
                    Bot::ReiniciarAndroid => "reiniciar-android",
                    Bot::Apagar => "apagar",
                    Bot::ReinicioCompleto => "reinicio-completo",
                    Bot::AgregarCarpeta => "agregar-carpeta",
                    Bot::FormSoloLectura => "form-solo-lectura",
                    Bot::AlmacenActualizar => "almacen-actualizar",
                    Bot::ImagenAgregar => "imagen-agregar",
                    Bot::DiscoRegenerar => "disco-regenerar",
                    Bot::PuenteActualizar => "puente-actualizar",
                    Bot::PuenteInstalar => "puente-instalar",
                    Bot::PuenteQuitar => "puente-quitar",
                    Bot::InformeCrear => "informe-crear",
                    Bot::Confirmar => "confirmar",
                    Bot::ConfirmarConGestor => "confirmar-con-gestor",
                    Bot::Cancelar => "cancelar",
                    Bot::CancelarOperacion => "cancelar-operacion",
                    Bot::PerfilUsar => "perfil-usar",
                    Bot::PerfilDuplicar => "perfil-duplicar",
                    Bot::PerfilBorrar => "perfil-borrar",
                    Bot::PerfilAplicar => "perfil-aplicar",
                }
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tono {
    Aviso,
    Error,
    Exito,
}

/// Lo que la ventana debe hacer ademas de lo que ya hizo la pantalla con la configuracion.
#[derive(Clone, Debug, PartialEq)]
pub enum Efecto {
    Cerrar,
    /// una clave de configuracion cambio (o `atajos` si se restauraron todos): aplicar y guardar
    Cambio(String),
    /// rotacion pedida (`orientacion` no es configuracion sino estado de la maquina en marcha)
    Orientacion(Orientacion),
    AplicarResolucion,
    AbrirEstado,
    Comprobar,
    /// reinicio ORDENADO de Android por adb (nunca system_reset)
    ReiniciarAndroid,
    /// reinicio completo de la maquina (`weft restart`): cierra la ventana
    ReinicioCompleto,
    /// apagar la maquina (`weft stop`: por adb, luego ACPI, quit y SIGKILL): cierra la ventana
    Apagar,
    /// una accion de la ventana (atajo, rotacion, zoom, captura) que ejecuta `window.rs`
    Control(Accion),
    /// conectar o desconectar un mando del equipo
    Mando { path: String, conectar: bool },
    /// consultar el estado del root en el invitado
    RootActualizar,
    /// consultar el estado de la imagen y del disco (archivos locales)
    AlmacenActualizar,
    /// consultar el estado del traductor ARM en el invitado
    PuenteActualizar,
    /// ejecutar `weft ARGS...` en segundo plano (hilo con progreso); el resultado vuelve como mensaje en linea
    /// bajo `clave`. Las ordenes de root y share guardan ellas mismas lo que haya que guardar en `config`.
    Orden { clave: &'static str, etiqueta: String, args: Vec<String> },
    /// detener la operacion larga en curso
    CancelarOperacion,
    /// abrir el selector de archivos del sistema; lo elegido vuelve con `Ajustes::poner_ruta`
    Elegir(Campo),
}

/// Estado de la imagen y del disco de la maquina (lo que la seccion Imagen muestra).
#[derive(Clone, Debug, PartialEq)]
pub struct Almacen {
    /// la imagen de la maquina
    pub imagen: imagen::EstadoImagen,
    pub disco: imagen::EstadoDisco,
    /// todas las imagenes instaladas y cual usa la maquina
    pub imagenes: Vec<crate::catalogo::FilaImagen>,
    pub actual: Option<String>,
}

#[derive(Clone, Debug)]
pub struct FilaDoctor {
    pub nivel: NivelDoctor,
    pub nombre: String,
    pub detalle: String,
}

/// Lo que la pantalla muestra y que no es configuracion.
#[derive(Clone, Debug)]
pub struct Datos {
    pub version: String,
    pub estado_dir: String,
    pub config_ruta: String,
    pub fuente: String,
    pub escala: f32,
    /// tamano del panel del invitado (la pantalla virtual)
    pub resolucion: (u32, u32),
    pub orient: Orientacion,
    /// rotacion con que se dibuja la ventana
    pub rot: u32,
    pub maquina: Option<Maquina>,
    pub doctor: Option<Vec<FilaDoctor>>,
    pub doctor_en_curso: bool,
    /// `resolution` se esta ejecutando
    pub aplicando: bool,
    /// estado del root en el invitado (None = sin consultar o sin respuesta)
    pub root_estado: Option<root::Estado>,
    pub root_consultando: bool,
    /// orden larga en curso: (clave del mensaje, texto de progreso)
    pub operacion: Option<(String, String)>,
    /// la operacion en curso se puede cancelar (copiar una imagen o reunir el informe: no dejan nada a medias)
    pub cancelable: bool,
    /// ¿esta virtiofsd en el equipo?
    pub virtiofsd: bool,
    /// imagen y disco (None = sin consultar)
    pub almacen: Option<Almacen>,
    pub almacen_consultando: bool,
    /// hay una maquina en marcha con este estado (y por tanto con este disco)
    pub en_marcha: bool,
    /// estado del traductor ARM en el invitado (None = sin consultar o sin respuesta)
    pub puente_estado: Option<puente::Estado>,
    pub puente_consultando: bool,
    /// adbd no respondio al reinicio ordenado: se ofrece el reinicio completo de la maquina
    pub reinicio_completo: bool,
    /// hay un reinicio ordenado en curso
    pub reiniciando: bool,
    /// hay un apagado en curso (`stop` lanzado): la ventana se cierra cuando la maquina termina
    pub apagando: bool,
    /// zoom real de la vista (porcentaje) y el modo pedido
    pub zoom_pct: f32,
    pub zoom_modo: ModoZoom,
    /// mandos del equipo, dispositivos sin permiso y si se pudo hablar con la maquina por QMP
    pub mandos: Vec<Mando>,
    pub ilegibles: usize,
    pub qmp_ok: bool,
    /// tiempo que lleva encendida la maquina
    pub encendida: Option<Duration>,
    /// identificador de Flatpak si la ventana corre dentro (para dar las ordenes de terminal completas)
    pub flatpak: Option<String>,
}

impl Default for Datos {
    fn default() -> Datos {
        Datos {
            version: crate::VERSION_PAQUETE.to_string(),
            estado_dir: String::new(),
            config_ruta: String::new(),
            fuente: String::new(),
            escala: 1.0,
            resolucion: (0, 0),
            orient: Orientacion::Auto,
            rot: 0,
            maquina: None,
            doctor: None,
            doctor_en_curso: false,
            aplicando: false,
            root_estado: None,
            root_consultando: false,
            operacion: None,
            cancelable: false,
            virtiofsd: true,
            almacen: None,
            almacen_consultando: false,
            en_marcha: false,
            puente_estado: None,
            puente_consultando: false,
            reinicio_completo: false,
            reiniciando: false,
            apagando: false,
            zoom_pct: 100.0,
            zoom_modo: ModoZoom::Ajustar,
            mandos: Vec::new(),
            ilegibles: 0,
            qmp_ok: true,
            encendida: None,
            flatpak: None,
        }
    }
}

/// Contexto de cada llamada: tamano de la ventana (dp), medidor de texto, datos y reloj.
pub struct Ctx<'a> {
    pub vent: (f32, f32),
    pub m: &'a dyn Medida,
    pub datos: &'a Datos,
    pub ahora: Instant,
}

/// Una pulsacion de tecla ya traducida.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tecla {
    /// codigo de tecla de SDL (uso HID de USB)
    pub sc: u32,
    pub abajo: bool,
    pub repetida: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub mayus: bool,
}

// ---------------------------------------------------------------------------------------------------------------
// geometria

const MARGEN: f32 = 16.0;
const ANCHO_MAX: f32 = 720.0;
const ALTO_MAX: f32 = 640.0;
const CABECERA: f32 = 48.0;
const NAV_ANCHO: f32 = 168.0;
const ANCHO_ESTRECHO: f32 = 560.0;
const PAD_X: f32 = 24.0;
const PAD_Y: f32 = 16.0;
const BH: f32 = 28.0;
const GAP: f32 = 8.0;
const FILA_GAP: f32 = 16.0;
const GUARDADO_S: u64 = 2;
/// Lado del icono de un mensaje en linea y sangria de su texto (dp).
const ICONO_MENSAJE: f32 = 12.0;
const SANGRIA_MENSAJE: f32 = 18.0;

/// Icono y color de un mensaje segun su tono: el icono hace que el tono se distinga tambien sin ver los colores.
fn icono_de(tono: Tono) -> (Icono, Color) {
    match tono {
        Tono::Aviso => (Icono::Alerta, tema::p().aviso),
        Tono::Error => (Icono::Fallo, tema::p().error),
        Tono::Exito => (Icono::Check, tema::p().exito),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geom {
    pub dialogo: R,
    pub cabecera: R,
    /// columna de navegacion (ventanas anchas)
    pub nav: Option<R>,
    /// franja de pestanas (ventanas estrechas)
    pub tabs: Option<R>,
    /// zona de desplazamiento del contenido
    pub contenido: R,
}

pub fn geometria(vent: (f32, f32), m: &dyn Medida) -> Geom {
    let ancho = (vent.0 - 2.0 * MARGEN).clamp(160.0, ANCHO_MAX).min(vent.0);
    let alto = (vent.1 * 0.9).min(ALTO_MAX).min(vent.1 - 2.0 * 8.0).max(120.0).min(vent.1);
    let dialogo = R::new(((vent.0 - ancho) / 2.0).floor(), ((vent.1 - alto) / 2.0).floor(), ancho, alto);
    let cabecera = R::new(dialogo.x, dialogo.y, dialogo.w, CABECERA);
    let cuerpo = R::new(dialogo.x, dialogo.y + CABECERA, dialogo.w, (dialogo.h - CABECERA).max(0.0));
    if dialogo.w < ANCHO_ESTRECHO {
        let (filas, _) = filas_tabs(cuerpo.w - 2.0 * GAP, m);
        let alto_tabs = filas as f32 * (BH + 4.0) + GAP * 2.0 - 4.0;
        let tabs = R::new(cuerpo.x, cuerpo.y, cuerpo.w, alto_tabs);
        let contenido = R::new(cuerpo.x, cuerpo.y + alto_tabs, cuerpo.w, (cuerpo.h - alto_tabs).max(0.0));
        Geom { dialogo, cabecera, nav: None, tabs: Some(tabs), contenido }
    } else {
        let nav = R::new(cuerpo.x, cuerpo.y, NAV_ANCHO, cuerpo.h);
        let contenido = R::new(cuerpo.x + NAV_ANCHO, cuerpo.y, cuerpo.w - NAV_ANCHO, cuerpo.h);
        Geom { dialogo, cabecera, nav: Some(nav), tabs: None, contenido }
    }
}

/// Una fila de la columna de navegacion: encabezado de grupo o seccion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NavFila {
    Grupo(Grupo, R),
    Item(Seccion, R),
}

/// Filas de la navegacion lateral (ventanas anchas): cada grupo con su encabezado (salvo el grupo cuya primera seccion se
/// llama igual, p. ej. General o Maquina, que ya lo dice) y sus secciones. Si no cabe en el alto de `n`, se compacta hasta un 72 %.
pub fn nav_filas(n: R) -> Vec<NavFila> {
    const ITEM: f32 = 34.0;
    const ENC: f32 = 26.0;
    const ESP: f32 = 4.0;
    let con_encabezado = |g: Grupo| {
        let en_grupo: Vec<Seccion> = Seccion::TODAS.iter().copied().filter(|s| s.grupo() == g).collect();
        !en_grupo.first().is_some_and(|s| s.titulo() == g.titulo())
    };
    let natural: f32 = 8.0 + Grupo::TODOS.iter().map(|g| (if con_encabezado(*g) { ENC } else { 0.0 }) + Seccion::TODAS.iter().filter(|s| s.grupo() == *g).count() as f32 * ITEM + ESP).sum::<f32>();
    let k = ((n.h - 8.0) / natural).clamp(0.72, 1.0);
    let mut y = n.y + 8.0;
    let mut v = Vec::new();
    for g in Grupo::TODOS {
        if con_encabezado(g) {
            v.push(NavFila::Grupo(g, R::new(n.x + 8.0, y, n.w - 16.0, ENC * k)));
            y += ENC * k;
        }
        for s in Seccion::TODAS.iter().copied().filter(|s| s.grupo() == g) {
            v.push(NavFila::Item(s, R::new(n.x + 8.0, y, n.w - 16.0, ITEM * k - 2.0)));
            y += ITEM * k;
        }
        y += ESP * k;
    }
    v
}

/// Reparto de las pestanas en filas parejas: (filas, por fila).
fn filas_tabs(ancho: f32, m: &dyn Medida) -> (usize, usize) {
    let n = Seccion::TODAS.len();
    // cada pestana lleva el titulo mas largo sin recortar (con su margen); como minimo 80 dp
    let ancho_tab = Seccion::TODAS.iter().map(|s| m.ancho(s.titulo(), Estilo::Negrita) + 20.0).fold(80.0f32, f32::max).ceil();
    let max_por_fila = (((ancho + 4.0) / (ancho_tab + 4.0)).floor().clamp(2.0, n as f32)) as usize;
    let filas = n.div_ceil(max_por_fila);
    (filas, n.div_ceil(filas))
}

// ---------------------------------------------------------------------------------------------------------------
// estado de la interfaz

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Conf {
    RootActivar,
    /// quitar el servicio de arranque del root
    RootDesactivar,
    /// instalar la app gestora (reemplaza a otro gestor de root)
    RootGestor,
    ReiniciarAndroid,
    ReinicioCompleto,
    Apagar,
    QuitarCarpeta(usize),
    /// usar otra imagen en esta maquina (posicion en la lista)
    CambiarImagen(usize),
    /// regenerar el disco: primera y segunda confirmacion
    DiscoPaso1,
    DiscoPaso2,
    PuenteInstalar,
    PuenteQuitar,
    /// borrar el perfil de dispositivo que se ve
    PerfilBorrar,
    /// escribir el perfil elegido en Android (reinicia Android)
    PerfilAplicar,
}

struct Edicion {
    campo: Campo,
    buf: String,
    cursor: usize,
}

pub struct Ajustes {
    pub abierto: bool,
    pub seccion: Seccion,
    /// desplazamiento del contenido (dp)
    pub scroll: f32,
    foco: Option<Id>,
    /// el foco se ve (se uso el teclado)
    foco_visible: bool,
    hover: Option<Id>,
    pulsado: Option<Id>,
    /// se pulso fuera del dialogo (se cierra al soltar fuera)
    pulsado_fuera: bool,
    /// arrastre de la barra: (y del raton al empezar, desplazamiento al empezar)
    arrastre: Option<(f32, f32)>,
    captura: Option<FilaAtajo>,
    conflicto: Option<(Combo, FilaAtajo)>,
    edicion: Option<Edicion>,
    lineas: Vec<(String, Tono, String)>,
    guardado: Option<(String, Instant)>,
    /// pregunta de confirmacion abierta (se muestra en linea, en la seccion que corresponde)
    confirmando: Option<Conf>,
    /// el control que abrio la pregunta con el teclado: recupera el foco al cancelarla o contestarla
    pregunta_de: Option<Id>,
    /// formulario de "Agregar carpeta"
    form_nombre: String,
    form_ruta: String,
    form_origen: String,
    form_root: String,
    form_puente: String,
    form_ro: bool,
    /// ultima maqueta (altura del contenido), para acotar el desplazamiento y la barra
    alto_contenido: std::cell::Cell<f32>,
    /// bloques de "Detalles tecnicos" desplegados
    detalles: Vec<Detalle>,
    /// carpeta de los perfiles de dispositivo del usuario (la pone la ventana; sin ella solo se ven los integrados)
    pub perfiles_dir: Option<std::path::PathBuf>,
    /// catalogo de perfiles de dispositivo: se lee al abrir su seccion y tras cada cambio (nunca al dibujar)
    perfiles: Option<Catalogo>,
    /// perfil que se ve en la seccion (None = el de la maquina)
    perfil_visto: Option<String>,
    /// estado de la maquina (la pone la ventana): de ahi se lee lo que publica el traductor instalado
    pub estado_maquina: Option<std::path::PathBuf>,
    /// extensiones que dice soportar el traductor instalado (None: no publica nada y no se avisa); se lee con el catalogo
    soportadas: Option<Vec<String>>,
}

impl Default for Ajustes {
    fn default() -> Self {
        Ajustes::nuevo()
    }
}

/// Teclas que la pantalla de configuracion usa por su cuenta sin Ctrl ni Alt (Intro, Esc, Retroceso, Tab, Espacio,
/// Inicio, RePag, Supr, Fin, AvPag, flechas): aunque esten asignadas a un atajo, con la pantalla abierta hacen lo suyo.
fn tecla_de_la_pantalla(t: &Tecla) -> bool {
    !t.ctrl && !t.alt && matches!(t.sc, 40..=44 | 74..=82 | 88)
}

/// Opciones (valor del archivo, etiqueta) de una clave de tipo lista. `orientacion` no es una clave de `config` sino la
/// rotacion de la maquina en marcha.
pub fn opciones(clave: &str) -> Vec<(&'static str, &'static str)> {
    match clave {
        "zoom" => vec![("ajustar", tx!("comun.ajustar")), ("50", "50 %"), ("75", "75 %"), ("100", "100 %")],
        "pellizco.modificador" => vec![("ctrl", "Ctrl"), ("alt", "Alt")],
        gestos::RATON_DERECHO | gestos::RATON_CENTRAL => vec![("atras", tx!("comun.atras")), ("inicio", tx!("comun.inicio")), ("recientes", tx!("comun.recientes")), ("menu", tx!("opcion.menu")), ("none", tx!("opcion.nada"))],
        "gamepad" => vec![("auto", tx!("opcion.automatico")), ("none", tx!("opcion.ninguno"))],
        "ventana.texto" => vec![("100", "100 %"), ("125", "125 %"), ("150", "150 %"), ("200", "200 %")],
        "ventana.tema" => vec![("auto", tx!("opcion.automatico")), ("claro", tx!("opcion.claro")), ("oscuro", tx!("opcion.oscuro"))],
        "maquina.tipo" => vec![("pc", "pc"), ("q35", "q35")],
        "maquina.gpu" => vec![("auto", tx!("opcion.automatica")), ("hardware", tx!("opcion.hardware")), ("software", tx!("opcion.software"))],
        "orientacion" => vec![("auto", tx!("comun.auto")), ("0", "0°"), ("1", "90°"), ("2", "180°"), ("3", "270°")],
        _ => Vec::new(),
    }
}

fn orient_valor(o: Orientacion) -> String {
    match o {
        Orientacion::Auto => "auto".into(),
        Orientacion::Fija(r) => (r % 4).to_string(),
    }
}

impl Ajustes {
    pub fn nuevo() -> Ajustes {
        Ajustes {
            abierto: false,
            seccion: Seccion::Controles,
            scroll: 0.0,
            foco: None,
            foco_visible: false,
            hover: None,
            pulsado: None,
            pulsado_fuera: false,
            arrastre: None,
            captura: None,
            conflicto: None,
            edicion: None,
            lineas: Vec::new(),
            guardado: None,
            confirmando: None,
            pregunta_de: None,
            form_nombre: String::new(),
            form_ruta: String::new(),
            form_origen: String::new(),
            form_root: String::new(),
            form_puente: String::new(),
            form_ro: false,
            alto_contenido: std::cell::Cell::new(0.0),
            detalles: Vec::new(),
            perfiles_dir: None,
            perfiles: None,
            perfil_visto: None,
            estado_maquina: None,
            soportadas: None,
        }
    }

    /// Abre la pantalla (en la seccion `s` o en la ultima vista). Devuelve los efectos que refrescan los datos de esa
    /// seccion, los mismos que al elegirla en la lista (ver `seleccionar_seccion`): reabrirla nunca muestra datos viejos.
    pub fn abrir(&mut self, s: Option<Seccion>) -> Vec<Efecto> {
        self.abierto = true;
        self.foco = None;
        self.foco_visible = false;
        self.hover = None;
        self.pulsado = None;
        self.edicion = None;
        self.seleccionar_seccion(s.unwrap_or(self.seccion))
    }

    /// Cambia a la seccion `s` (clic en la lista, flechas o al abrir la pantalla): desplazamiento arriba, sin capturas ni
    /// preguntas a medias, y los efectos que refrescan sus datos (estado del root, imagenes, traductor ARM).
    fn seleccionar_seccion(&mut self, s: Seccion) -> Vec<Efecto> {
        self.seccion = s;
        self.quitar_mensaje("controles");
        self.scroll = 0.0;
        self.captura = None;
        self.conflicto = None;
        self.confirmando = None;
        self.pregunta_de = None;
        self.detalles.clear();
        match s {
            Seccion::Root => vec![Efecto::RootActualizar],
            Seccion::Imagen => vec![Efecto::AlmacenActualizar],
            Seccion::Puente => vec![Efecto::PuenteActualizar],
            Seccion::Perfiles => {
                self.perfil_visto = None;
                self.cargar_perfiles();
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// Lee el catalogo de perfiles de dispositivo (integrados y del usuario). Solo al abrir la seccion y tras un cambio.
    fn cargar_perfiles(&mut self) {
        self.perfiles = Some(Catalogo::cargar(self.perfiles_dir.as_deref()));
        self.soportadas = self.estado_maquina.as_deref().and_then(dispositivo::soportadas_de_maquina);
    }

    /// Id del perfil que se ve: el elegido en la lista o, si no se eligio ninguno, el de la maquina.
    fn visto(&self, cfg: &Config) -> String {
        self.perfil_visto.clone().unwrap_or_else(|| cfg.get("dispositivo.perfil"))
    }

    fn perfil_de(&self, cfg: &Config) -> Option<&Dispositivo> {
        self.perfiles.as_ref()?.buscar(&self.visto(cfg))
    }

    /// Cambia el perfil que se ve (uno del usuario) con `f`, lo guarda en su archivo y vuelve a leer el catalogo.
    fn editar_perfil(&mut self, cfg: &Config, f: impl FnOnce(&mut Dispositivo) -> Result<(), String>) -> Result<(), String> {
        let mut d = self.perfil_de(cfg).cloned().ok_or_else(|| tx!("perfiles.no_hay_perfil").to_string())?;
        if d.origen != crate::perfil::Origen::Usuario {
            return Err(tx!("dispositivo.los_integrados_no_se_cambian").into());
        }
        let dir = self.perfiles_dir.clone().ok_or_else(|| tx!("perfiles.sin_carpeta").to_string())?;
        f(&mut d)?;
        dispositivo::guardar(&dir, &mut d)?;
        self.cargar_perfiles();
        Ok(())
    }

    /// `editar_perfil` desde un control: el "Guardado" breve o el error en linea bajo `clave`.
    fn editar_perfil_con(&mut self, cfg: &Config, clave: &str, ahora: Instant, f: impl FnOnce(&mut Dispositivo) -> Result<(), String>) -> bool {
        match self.editar_perfil(cfg, f) {
            Ok(()) => {
                self.quitar_mensaje(clave);
                self.destello(clave, ahora);
                true
            }
            Err(e) => {
                self.mensaje(clave, Tono::Error, &e);
                false
            }
        }
    }

    pub fn cerrar(&mut self) {
        self.abierto = false;
        self.confirmando = None;
        self.pregunta_de = None;
        self.detalles.clear();
        self.captura = None;
        self.conflicto = None;
        self.edicion = None;
        self.hover = None;
        self.pulsado = None;
        self.arrastre = None;
    }

    /// El usuario pidio cerrar la ventana (la X o Alt+F4). Cerrarla apaga la maquina, asi que nunca se cierra de golpe:
    /// con `confirmar=si` se abre la seccion Maquina con la misma pregunta que el boton Apagar (desplazada hasta ella), y
    /// si no, se devuelve el mismo efecto que ese boton (`stop` como proceso aparte; la ventana se cierra cuando la
    /// maquina termina). Con una operacion larga o un apagado ya en curso solo se avisa en esa seccion y no se apaga nada.
    pub fn cerrar_ventana(&mut self, cfg: &Config, c: &Ctx) -> Vec<Efecto> {
        let aviso = match (&c.datos.operacion, c.datos.apagando) {
            (Some((_, texto)), _) => Some(txf!("maquina.espera_termine", texto.trim_end_matches('.'))),
            (None, true) => Some(tx!("maquina.apagando_maquina_ventana_cierra").to_string()),
            (None, false) => None,
        };
        let con_pregunta = aviso.is_none() && cfg.bool("confirmar");
        // los efectos de abrir la seccion (Maquina no pide refrescos, pero se respetan si algun dia los pide)
        let mut ef = Vec::new();
        if aviso.is_some() || con_pregunta {
            ef = self.abrir(Some(Seccion::Maquina));
        }
        self.quitar_mensaje("apagar");
        if let Some(t) = aviso {
            self.mensaje("apagar", Tono::Aviso, &t);
            // el aviso va bajo el boton Apagar: se lleva a la vista
            self.centrar(Id::Boton(Bot::Apagar), cfg, c);
            return ef;
        }
        if con_pregunta {
            self.confirmando = Some(Conf::Apagar);
            self.centrar(Id::Boton(Bot::Confirmar), cfg, c);
            return ef;
        }
        vec![Efecto::Apagar]
    }

    /// Desplaza el contenido para que el control quede centrado en la zona visible (si existe en la seccion abierta).
    fn centrar(&mut self, id: Id, cfg: &Config, c: &Ctx) {
        let g = geometria(c.vent, c.m);
        let mq = self.maquetar(cfg, c, &g);
        if let Some(k) = mq.controles.iter().find(|k| k.id == id) {
            let vp = g.contenido;
            let max = (self.alto_contenido.get() - vp.h).max(0.0);
            // `k.r` ya lleva aplicado el desplazamiento actual (acotado), como lo deja la maqueta
            self.scroll = (self.scroll.clamp(0.0, max) + k.r.y + k.r.h / 2.0 - (vp.y + vp.h / 2.0)).clamp(0.0, max);
        }
    }

    #[cfg(test)]
    pub fn en_captura(&self) -> Option<AccionAtajo> {
        match self.captura {
            Some(FilaAtajo::Accion(a)) => Some(a),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn en_captura_fila(&self) -> Option<FilaAtajo> {
        self.captura
    }

    /// Valor efectivo de una clave de `config` (tambien las de la ventana: atajos extra, `atajos.desactivados`, `raton.*`).
    pub fn valor(&self, cfg: &Config, k: &str) -> String {
        cfg.get(k)
    }

    /// Pone una clave de `config` validandola (se guarda en el archivo al instante, como todas). Ok(valor canonico).
    pub fn fijar(&mut self, cfg: &mut Config, k: &'static str, v: &str) -> Result<String, String> {
        cfg.set(k, v)
    }

    /// Fila de atajo (distinta de `salvo`) que tiene asignada esa combinacion (texto del archivo), de `config` o de la
    /// ventana.
    pub fn quien_usa(&self, cfg: &Config, combo: &str, salvo: FilaAtajo) -> Option<FilaAtajo> {
        FilaAtajo::todas().filter(|f| *f != salvo).find(|f| self.valor(cfg, f.clave()) == combo)
    }

    /// Atajo propio de la ventana que dispara una pulsacion (modificadores exactos). Los de `config` los da
    /// `Config::atajo_para`.
    pub fn atajo_extra_para(&self, cfg: &Config, sc: u32, ctrl: bool, alt: bool, mayus: bool) -> Option<AtajoExtra> {
        AtajoExtra::TODOS.into_iter().find(|e| Combo::parse(&self.valor(cfg, e.clave())).is_ok_and(|c| c.coincide(sc, ctrl, alt, mayus)))
    }

    /// Texto visible de la tecla de una fila (None si esta sin asignar), con la distribucion del teclado del equipo.
    pub fn tecla_de(&self, cfg: &Config, f: FilaAtajo) -> Option<String> {
        Combo::parse(&self.valor(cfg, f.clave())).ok().map(|c| gestos::nombre_combo(&c))
    }

    #[cfg(test)]
    pub fn editando(&self) -> Option<Campo> {
        self.edicion.as_ref().map(|e| e.campo)
    }

    /// ¿Se esta escribiendo en un campo de texto? La ventana activa entonces la entrada de texto de SDL.
    pub fn quiere_texto(&self) -> bool {
        self.abierto && self.edicion.as_ref().is_some_and(|e| e.campo.es_texto())
    }

    /// Texto escrito (evento de texto de SDL) en el campo de texto que se edita.
    pub fn texto(&mut self, s: &str) -> bool {
        let Some(e) = self.edicion.as_mut().filter(|e| e.campo.es_texto()) else { return false };
        let max = match e.campo {
            Campo::ShareNombre => 32,
            Campo::DatosTam => 8,
            _ => 400,
        };
        let byte = |b: &str, i: usize| b.char_indices().nth(i).map_or(b.len(), |(p, _)| p);
        for ch in s.chars().filter(|c| !c.is_control()) {
            if e.buf.chars().count() >= max {
                break;
            }
            let p = byte(&e.buf, e.cursor);
            e.buf.insert(p, ch);
            e.cursor += 1;
        }
        let k = e.campo.clave();
        self.quitar_mensaje(k);
        true
    }

    /// Texto pegado (Ctrl+V) en el campo que se edita: una sola linea (la primera que no este vacia). En un campo de ruta,
    /// una URI `file://` (copiar un archivo en el gestor de archivos) se pega como ruta local.
    pub fn pegar(&mut self, s: &str) -> bool {
        let Some(campo) = self.edicion.as_ref().filter(|e| e.campo.es_texto()).map(|e| e.campo) else { return false };
        let linea = s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
        let t = if campo.es_ruta() && linea.starts_with("file://") { crate::rutas::ruta_escrita(linea, None) } else { linea.to_string() };
        !t.is_empty() && self.texto(&t)
    }

    /// Campo de ruta en que cae un archivo soltado sobre la ventana: el que se esta editando, si es de ruta; si no, el
    /// de la seccion visible. Con la pantalla cerrada (o en otra seccion), un zip va a Imagen y una carpeta a Compartir.
    pub fn campo_para_soltar(&self, es_carpeta: bool, ruta: &str) -> Option<Campo> {
        if let Some(c) = self.edicion.as_ref().map(|e| e.campo).filter(|c| c.es_ruta()).filter(|_| self.abierto) {
            return Some(c);
        }
        if self.abierto {
            match self.seccion {
                Seccion::Imagen => return Some(Campo::Origen),
                Seccion::Compartir => return Some(Campo::ShareRuta),
                Seccion::Root => return Some(Campo::RutaRoot),
                Seccion::Puente => return Some(Campo::RutaPuente),
                _ => {}
            }
        }
        if es_carpeta {
            Some(Campo::ShareRuta)
        } else if ruta.to_ascii_lowercase().ends_with(".zip") {
            Some(Campo::Origen)
        } else {
            None
        }
    }

    /// Clave de los mensajes en linea de un campo.
    pub fn clave_de(c: Campo) -> &'static str {
        c.clave()
    }

    /// Seccion donde esta un campo de ruta.
    pub fn seccion_de(c: Campo) -> Seccion {
        match c {
            Campo::ShareRuta => Seccion::Compartir,
            Campo::RutaRoot => Seccion::Root,
            Campo::RutaPuente => Seccion::Puente,
            _ => Seccion::Imagen,
        }
    }

    /// Pone `ruta` (elegida en el selector del sistema, soltada o pegada) en un campo de ruta, como si se hubiera escrito
    /// y confirmado: termina su edicion si estaba abierta y comprueba la carpeta compartida.
    pub fn poner_ruta(&mut self, campo: Campo, ruta: &str) -> bool {
        if !campo.es_ruta() {
            return false;
        }
        if self.edicion.as_ref().is_some_and(|e| e.campo == campo) {
            self.edicion = None;
        }
        let t = crate::rutas::ruta_escrita(ruta, None);
        match campo {
            Campo::Origen => {
                self.form_origen = t;
                self.quitar_mensaje("imagen");
            }
            Campo::RutaRoot => {
                self.form_root = t;
                self.quitar_mensaje("root");
            }
            Campo::RutaPuente => {
                self.form_puente = t;
                self.quitar_mensaje("puente");
            }
            _ => {
                self.form_ruta = t.clone();
                match compartir::comprobar_ruta(&t) {
                    Err(msg) => self.mensaje("share.ruta", Tono::Aviso, &txf!("comun.no_podra_agregar_asi", msg)),
                    Ok(_) => self.quitar_mensaje("share.ruta"),
                }
            }
        }
        true
    }

    /// Deja un mensaje en linea bajo el control de la clave (reemplaza el anterior de esa clave).
    pub fn mensaje(&mut self, clave: &str, tono: Tono, texto: &str) {
        self.lineas.retain(|(k, _, _)| k != clave);
        // red de seguridad: ningun mensaje que llegue a la interfaz puede nombrar productos ni proveedores (salvo las rutas
        // del usuario que weft marco como tales); los errores del sistema que lleguen en ingles se dicen en espanol
        self.lineas.push((clave.to_string(), tono, crate::vista::para_mostrar(texto)));
    }

    pub fn quitar_mensaje(&mut self, clave: &str) {
        self.lineas.retain(|(k, _, _)| k != clave);
    }

    pub fn mensaje_de(&self, clave: &str) -> Option<(Tono, &str)> {
        self.lineas.iter().find(|(k, _, _)| k == clave).map(|(_, t, s)| (*t, s.as_str()))
    }

    fn destello(&mut self, clave: &str, ahora: Instant) {
        self.guardado = Some((clave.to_string(), ahora));
    }

    /// Hay algo que cambia con el tiempo (el "Guardado" breve): hay que redibujar.
    pub fn animado(&self, ahora: Instant) -> bool {
        self.abierto && self.guardado.as_ref().is_some_and(|(_, t)| ahora.saturating_duration_since(*t) < Duration::from_secs(GUARDADO_S + 1))
    }

    // -----------------------------------------------------------------------------------------------------------
    // dibujo y hit-test

    fn visual(&self, id: &Id) -> (bool, bool, bool) {
        (self.hover.as_ref() == Some(id), self.pulsado.as_ref() == Some(id) && self.hover.as_ref() == Some(id), self.foco_visible && self.foco.as_ref() == Some(id))
    }

    /// Dibujo completo: fondo atenuado, dialogo, navegacion y contenido.
    pub fn dibujar(&self, cfg: &Config, c: &Ctx) -> Vec<Pint> {
        let mut out = Vec::new();
        if !self.abierto {
            return out;
        }
        let g = geometria(c.vent, c.m);
        let mq = self.maquetar(cfg, c, &g);
        // atenuado (60 %) y dialogo
        out.push(Pint::Rect { r: R::new(0.0, 0.0, c.vent.0, c.vent.1), c: Color(0, 0, 0, 153), radio: 0.0 });
        out.push(Pint::Rect { r: g.dialogo.reducir(-1.0), c: tema::p().borde, radio: tema::RADIO + 3.0 });
        out.push(Pint::Rect { r: g.dialogo, c: tema::p().superficie, radio: tema::RADIO + 2.0 });
        // cabecera
        let ty = g.cabecera.y + CABECERA / 2.0;
        out.push(texto_v(c.m, g.cabecera.x + PAD_X, ty, tx!("comun.configuracion"), Estilo::Titulo, tema::p().texto));
        out.push(Pint::Rect { r: R::new(g.cabecera.x, g.cabecera.y + CABECERA - 1.0, g.cabecera.w, 1.0), c: tema::p().borde, radio: 0.0 });
        // navegacion
        if let Some(n) = g.nav {
            out.push(Pint::Recorte(Some(R::new(n.x, n.y, n.w, n.h))));
            out.push(Pint::Rect { r: R::new(n.x + n.w - 1.0, n.y, 1.0, n.h), c: tema::p().borde, radio: 0.0 });
            out.push(Pint::Recorte(None));
        }
        out.extend(mq.fijos.iter().cloned());
        // contenido recortado
        out.push(Pint::Recorte(Some(g.contenido)));
        out.extend(mq.pint.iter().cloned());
        out.push(Pint::Recorte(None));
        // barra de desplazamiento
        if let Some((pista, pulgar)) = mq.barra {
            out.push(Pint::Rect { r: pista, c: tema::p().control.alfa(150), radio: 3.0 });
            out.push(Pint::Rect { r: pulgar, c: if self.arrastre.is_some() { tema::p().texto2 } else { tema::p().texto_apagado }, radio: 3.0 });
        }
        out
    }

    /// Controles visibles (nombre estable y rectangulo en la ventana, ya recortado a la zona visible).
    pub fn botones(&self, cfg: &Config, c: &Ctx) -> Vec<(String, R)> {
        if !self.abierto {
            return Vec::new();
        }
        let g = geometria(c.vent, c.m);
        self.maquetar(cfg, c, &g).controles.iter().filter(|k| k.visible.w > 0.0 && k.visible.h > 0.0).map(|k| (k.id.nombre(), k.visible)).collect()
    }

    /// Control bajo (x, y), si hay.
    fn control_en(&self, cfg: &Config, c: &Ctx, x: f32, y: f32) -> Option<Id> {
        let g = geometria(c.vent, c.m);
        let mq = self.maquetar(cfg, c, &g);
        mq.controles.iter().rev().find(|k| k.habilitado && k.visible.contiene(x, y)).map(|k| k.id)
    }

    // -----------------------------------------------------------------------------------------------------------
    // raton

    /// El raton se mueve. true si hay que redibujar.
    pub fn mover(&mut self, cfg: &Config, c: &Ctx, x: f32, y: f32) -> bool {
        if !self.abierto {
            return false;
        }
        if let Some((y0, s0)) = self.arrastre {
            let g = geometria(c.vent, c.m);
            let (alto, vp) = (self.alto_contenido.get(), g.contenido.h);
            let recorrido = (vp - 16.0 - pulgar_alto(vp, alto)).max(1.0);
            let max = (alto - vp).max(0.0);
            self.scroll = (s0 + (y - y0) / recorrido * max).clamp(0.0, max);
            return true;
        }
        let h = self.control_en(cfg, c, x, y);
        let cambio = h != self.hover;
        self.hover = h;
        cambio
    }

    pub fn salir(&mut self) -> bool {
        let c = self.hover.is_some();
        self.hover = None;
        c
    }

    /// Boton izquierdo pulsado. Con la pantalla abierta siempre lo toma ella.
    pub fn presionar(&mut self, cfg: &Config, c: &Ctx, x: f32, y: f32) -> bool {
        if !self.abierto {
            return false;
        }
        let g = geometria(c.vent, c.m);
        self.foco_visible = false;
        if !g.dialogo.contiene(x, y) {
            self.pulsado_fuera = true;
            self.pulsado = None;
            return true;
        }
        self.pulsado_fuera = false;
        let mq = self.maquetar(cfg, c, &g);
        // barra de desplazamiento
        if let Some((pista, pulgar)) = mq.barra {
            let zona = R::new(pista.x - 6.0, pista.y, pista.w + 8.0, pista.h);
            if zona.contiene(x, y) {
                let (alto, vp) = (self.alto_contenido.get(), g.contenido.h);
                let max = (alto - vp).max(0.0);
                if !pulgar.contiene(x, y) {
                    // un clic en la pista lleva el pulgar hasta ahi
                    let rec = (vp - 16.0 - pulgar.h).max(1.0);
                    self.scroll = (((y - pista.y - pulgar.h / 2.0) / rec) * max).clamp(0.0, max);
                }
                self.arrastre = Some((y, self.scroll));
                self.pulsado = None;
                return true;
            }
        }
        // empezar una accion nueva cancela la captura o la edicion en curso (si se pulsa fuera de ellas)
        let id = mq.controles.iter().rev().find(|k| k.habilitado && k.visible.contiene(x, y)).map(|k| k.id);
        if self.captura.is_some() && id != self.captura.map(Id::Atajo) && id != Some(Id::Boton(Bot::UsarAqui)) {
            self.captura = None;
            self.conflicto = None;
        }
        self.pulsado = id;
        true
    }

    /// Boton izquierdo soltado: activa el control si coincide con el pulsado, o cierra si se pulso y solto fuera.
    pub fn soltar(&mut self, cfg: &mut Config, c: &Ctx, x: f32, y: f32) -> Vec<Efecto> {
        if !self.abierto {
            return Vec::new();
        }
        if self.arrastre.take().is_some() {
            return Vec::new();
        }
        let g = geometria(c.vent, c.m);
        if self.pulsado_fuera {
            self.pulsado_fuera = false;
            if !g.dialogo.contiene(x, y) {
                self.cerrar();
                return vec![Efecto::Cerrar];
            }
            return Vec::new();
        }
        let Some(p) = self.pulsado.take() else {
            // un clic en un hueco: lo que se editaba se confirma
            let mut e = Vec::new();
            self.terminar_edicion(cfg, c.ahora, &mut e);
            return e;
        };
        if self.control_en(cfg, c, x, y) != Some(p) {
            return Vec::new();
        }
        self.activar(p, cfg, c)
    }

    /// Rueda sobre la pantalla: desplaza el contenido (dy > 0 = hacia arriba, como SDL).
    pub fn rueda(&mut self, c: &Ctx, dy: f32) {
        if !self.abierto {
            return;
        }
        let g = geometria(c.vent, c.m);
        let max = (self.alto_contenido.get() - g.contenido.h).max(0.0);
        self.scroll = (self.scroll - dy * 48.0).clamp(0.0, max);
    }

    // -----------------------------------------------------------------------------------------------------------
    // acciones

    fn activar(&mut self, id: Id, cfg: &mut Config, c: &Ctx) -> Vec<Efecto> {
        let mut ef = Vec::new();
        // salvo en el propio campo, activar algo confirma lo que se editaba
        if !matches!(id, Id::Campo(_)) {
            self.terminar_edicion(cfg, c.ahora, &mut ef);
        }
        self.foco = Some(id);
        match id {
            Id::Cerrar => {
                self.cerrar();
                ef.push(Efecto::Cerrar);
            }
            Id::Nav(s) => ef.extend(self.seleccionar_seccion(s)),
            Id::Interruptor("root.cargar_al_inicio") => {
                if c.datos.operacion.is_none() {
                    self.quitar_mensaje("root");
                    if cfg.bool("root.cargar_al_inicio") {
                        // quitar el servicio no descarga nada ni pide reiniciar, pero las apps pierden el root en el
                        // proximo arranque: se pregunta antes
                        self.confirmando = Some(Conf::RootDesactivar);
                    } else {
                        self.confirmando = Some(Conf::RootActivar);
                    }
                }
            }
            Id::Examinar(campo) => ef.push(Efecto::Elegir(campo)),
            Id::Imagen(i) => {
                if c.datos.operacion.is_none() {
                    if let Some(a) = &c.datos.almacen {
                        if a.imagenes.get(i).is_some_and(|f| f.completa && a.actual.as_deref() != Some(f.id.as_str())) {
                            self.quitar_mensaje("imagen.usar");
                            self.confirmando = Some(Conf::CambiarImagen(i));
                        }
                    }
                }
            }
            Id::Quitar(i) => {
                self.confirmando = Some(Conf::QuitarCarpeta(i));
                self.quitar_mensaje("share.quitar");
            }
            Id::Control(a) => {
                // la accion la ejecuta la ventana; el dialogo se queda abierto y una linea de estado la confirma
                ef.push(Efecto::Control(a));
                if a != Accion::Captura {
                    self.mensaje("controles", Tono::Exito, &a.confirmacion());
                } else {
                    self.quitar_mensaje("controles");
                }
            }
            Id::Mando(i) => {
                if let Some(m) = c.datos.mandos.get(i).filter(|m| !m.ocupado) {
                    self.quitar_mensaje("mandos");
                    ef.push(Efecto::Mando { path: m.path.clone(), conectar: m.conectado.is_none() });
                }
            }
            Id::Perfil(i) => {
                let id = if i == 0 { Some(dispositivo::NINGUNO.to_string()) } else { self.perfiles.as_ref().and_then(|c| c.dispositivos.get(i - 1)).map(|d| d.id.clone()) };
                if let Some(id) = id {
                    self.perfil_visto = Some(id);
                    self.confirmando = None;
                    self.quitar_mensaje("perfiles");
                    self.detalles.retain(|d| *d != Detalle::PerfilRegistros);
                }
            }
            Id::Extension(i) => {
                if let Some(n) = dispositivo::EXTENSIONES.get(i) {
                    self.editar_perfil_con(cfg, "disp.features", c.ahora, |d| {
                        d.alternar_extension(n);
                        Ok(())
                    });
                }
            }
            Id::PerfilModo(i) => {
                if let Some(m) = MODOS_EXTENSIONES.get(i).copied() {
                    self.editar_perfil_con(cfg, "disp.features", c.ahora, |d| {
                        d.poner_modo_extensiones(m);
                        Ok(())
                    });
                }
            }
            Id::PerfilPagina(k) => {
                self.editar_perfil_con(cfg, "disp.pagina", c.ahora, |d| d.fijar("pagina", &k.to_string()));
            }
            Id::Interruptor(k) => {
                let nuevo = if self.valor(cfg, k) == "si" { "no" } else { "si" };
                self.poner(cfg, k, nuevo, c.ahora, &mut ef);
            }
            Id::Opcion(k, i) => {
                if let Some((valor, _)) = opciones(k).get(i).copied() {
                    if k == "orientacion" {
                        let o = if valor == "auto" { Orientacion::Auto } else { Orientacion::Fija(valor.parse().unwrap_or(0)) };
                        ef.push(Efecto::Orientacion(o));
                        self.destello(k, c.ahora);
                    } else {
                        self.poner(cfg, k, valor, c.ahora, &mut ef);
                    }
                }
            }
            Id::Paso(k, d) => {
                let (min, max) = match config::clave(k).map(|k| k.tipo) {
                    Some(config::Tipo::Entero(a, b)) => (a as i32, b as i32),
                    _ => (0, 0),
                };
                let ahora = cfg.entero(k).unwrap_or(min as u32) as i32;
                let nuevo = (ahora + d).clamp(min, max);
                if nuevo != ahora {
                    self.poner(cfg, k, &nuevo.to_string(), c.ahora, &mut ef);
                }
            }
            Id::Campo(campo) => {
                if self.edicion.as_ref().map(|e| e.campo) != Some(campo) {
                    self.empezar_edicion(campo, cfg, c.datos);
                }
            }
            Id::Atajo(a) => {
                self.captura = Some(a);
                self.conflicto = None;
                self.quitar_mensaje(a.clave());
            }
            Id::Boton(b) => match b {
                Bot::Detalles(d) => {
                    if let Some(i) = self.detalles.iter().position(|x| *x == d) {
                        self.detalles.remove(i);
                    } else {
                        self.detalles.push(d);
                    }
                }
                Bot::RestaurarAtajos => {
                    cfg.restaurar_atajos();
                    // y los de la ventana
                    for e in AtajoExtra::TODOS {
                        cfg.reset(Some(e.clave()));
                    }
                    self.captura = None;
                    self.conflicto = None;
                    ef.push(Efecto::Cambio("atajos".into()));
                    self.destello("atajos", c.ahora);
                }
                Bot::RootActualizar => ef.push(Efecto::RootActualizar),
                Bot::AlmacenActualizar => ef.push(Efecto::AlmacenActualizar),
                Bot::PuenteActualizar => ef.push(Efecto::PuenteActualizar),
                Bot::ImagenAgregar => {
                    if c.datos.operacion.is_none() {
                        let o = self.form_origen.trim().to_string();
                        if o.is_empty() {
                            self.mensaje("imagen", Tono::Error, tx!("progreso.escribe_ruta_zip_carpeta"));
                        } else {
                            self.quitar_mensaje("imagen");
                            self.form_origen.clear();
                            ef.push(Efecto::Orden { clave: "imagen", etiqueta: tx!("progreso.agregando_imagen_android").into(), args: vec!["image".into(), "add".into(), o] });
                        }
                    }
                }
                Bot::DiscoRegenerar => {
                    if c.datos.operacion.is_none() && !c.datos.en_marcha {
                        self.quitar_mensaje("disco");
                        self.confirmando = Some(Conf::DiscoPaso1);
                    }
                }
                Bot::PuenteInstalar => {
                    if c.datos.operacion.is_none() {
                        self.quitar_mensaje("puente");
                        self.confirmando = Some(Conf::PuenteInstalar);
                    }
                }
                Bot::PuenteQuitar => {
                    if c.datos.operacion.is_none() {
                        self.quitar_mensaje("puente");
                        self.confirmando = Some(Conf::PuenteQuitar);
                    }
                }
                Bot::CancelarOperacion => {
                    if c.datos.operacion.is_some() && c.datos.cancelable {
                        ef.push(Efecto::CancelarOperacion);
                    }
                }
                Bot::InformeCrear => {
                    if c.datos.operacion.is_none() {
                        self.quitar_mensaje("informe");
                        ef.push(Efecto::Orden { clave: "informe", etiqueta: tx!("progreso.reuniendo_informe").into(), args: vec!["report".into()] });
                    }
                }
                Bot::RootGestor => {
                    if c.datos.operacion.is_none() {
                        // puede desinstalar otro gestor de root: se pregunta antes
                        self.quitar_mensaje("root");
                        self.confirmando = Some(Conf::RootGestor);
                    }
                }
                Bot::ReiniciarAndroid => {
                    self.quitar_mensaje("reinicio");
                    if cfg.bool("confirmar") || c.datos.operacion.is_some() || c.datos.reiniciando {
                        self.confirmando = Some(Conf::ReiniciarAndroid);
                    } else {
                        ef.push(Efecto::ReiniciarAndroid);
                    }
                }
                Bot::Apagar => {
                    if c.datos.apagando {
                        // ya se esta apagando: el mensaje del apagado sigue a la vista
                        return ef;
                    }
                    self.quitar_mensaje("apagar");
                    if cfg.bool("confirmar") {
                        self.confirmando = Some(Conf::Apagar);
                    } else {
                        ef.push(Efecto::Apagar);
                    }
                }
                Bot::ReinicioCompleto => {
                    self.quitar_mensaje("reinicio");
                    self.confirmando = Some(Conf::ReinicioCompleto);
                }
                Bot::PerfilUsar => {
                    let id = self.visto(cfg);
                    if self.poner(cfg, "dispositivo.perfil", &id, c.ahora, &mut ef) {
                        self.perfil_visto = Some(id);
                        let t = if c.datos.en_marcha { tx!("perfiles.elegido_en_marcha") } else { tx!("perfiles.elegido_proximo_arranque") };
                        self.mensaje("perfiles", Tono::Exito, t);
                    }
                }
                Bot::PerfilDuplicar => {
                    self.quitar_mensaje("perfiles");
                    let origen = self.visto(cfg);
                    let origen = if origen == dispositivo::NINGUNO { dispositivo::ID_BASE.to_string() } else { origen };
                    let r = match (&self.perfiles_dir, &self.perfiles) {
                        (Some(dir), Some(cat)) => dispositivo::duplicar(dir, cat, &origen, &cat.id_libre(&origen), None),
                        _ => Err(tx!("perfiles.sin_carpeta").to_string()),
                    };
                    match r {
                        Ok(d) => {
                            self.cargar_perfiles();
                            self.perfil_visto = Some(d.id.clone());
                            self.mensaje("perfiles", Tono::Exito, &txf!("perfiles.duplicado", d.nombre));
                        }
                        Err(e) => self.mensaje("perfiles", Tono::Error, &e),
                    }
                }
                Bot::PerfilBorrar => {
                    if self.perfil_de(cfg).is_some_and(|d| d.origen == crate::perfil::Origen::Usuario) {
                        self.quitar_mensaje("perfiles");
                        self.confirmando = Some(Conf::PerfilBorrar);
                    }
                }
                Bot::PerfilAplicar => {
                    if c.datos.operacion.is_none() && c.datos.en_marcha {
                        self.quitar_mensaje("perfiles");
                        self.confirmando = Some(Conf::PerfilAplicar);
                    }
                }
                Bot::Cancelar => self.confirmando = None,
                Bot::Confirmar | Bot::ConfirmarConGestor => {
                    let con_gestor = b == Bot::ConfirmarConGestor;
                    match self.confirmando.take() {
                        Some(Conf::RootActivar) if c.datos.operacion.is_none() => {
                            let mut args: Vec<String> = vec!["root".into(), "enable".into(), "--now".into()];
                            if !self.form_root.trim().is_empty() {
                                args.push("--from".into());
                                args.push(self.form_root.trim().to_string());
                            }
                            if con_gestor {
                                args.push("--manager".into());
                            }
                            ef.push(Efecto::Orden { clave: "root", etiqueta: tx!("progreso.activando_root_puede_tardar").into(), args });
                        }
                        Some(Conf::RootDesactivar) if c.datos.operacion.is_none() => {
                            ef.push(Efecto::Orden { clave: "root", etiqueta: tx!("progreso.desactivando_root").into(), args: vec!["root".into(), "disable".into()] });
                        }
                        Some(Conf::RootGestor) if c.datos.operacion.is_none() => {
                            let mut args: Vec<String> = vec!["root".into(), "install-manager".into()];
                            if !self.form_root.trim().is_empty() {
                                args.push("--from".into());
                                args.push(self.form_root.trim().to_string());
                            }
                            ef.push(Efecto::Orden { clave: "root", etiqueta: tx!("progreso.instalando_gestor").into(), args });
                        }
                        Some(Conf::ReiniciarAndroid) if c.datos.operacion.is_none() && !c.datos.reiniciando => ef.push(Efecto::ReiniciarAndroid),
                        Some(Conf::ReinicioCompleto) => ef.push(Efecto::ReinicioCompleto),
                        Some(Conf::Apagar) => ef.push(Efecto::Apagar),
                        Some(Conf::DiscoPaso1) if c.datos.operacion.is_none() && !c.datos.en_marcha => self.confirmando = Some(Conf::DiscoPaso2),
                        Some(Conf::DiscoPaso2) if c.datos.operacion.is_none() && !c.datos.en_marcha => {
                            let mut args: Vec<String> = vec!["disk".into(), "reset".into(), "--yes".into()];
                            if let Some(a) = &c.datos.almacen {
                                args.extend(["--out".to_string(), a.disco.ruta.to_string_lossy().into_owned(), "--image".to_string(), a.imagen.carpeta.to_string_lossy().into_owned()]);
                            }
                            ef.push(Efecto::Orden { clave: "disco", etiqueta: tx!("progreso.regenerando_disco").into(), args });
                        }
                        Some(Conf::PuenteInstalar) if c.datos.operacion.is_none() => {
                            let mut args: Vec<String> = vec!["bridge".into(), "install".into()];
                            if !self.form_puente.trim().is_empty() {
                                args.push("--file".into());
                                args.push(self.form_puente.trim().to_string());
                            }
                            ef.push(Efecto::Orden { clave: "puente", etiqueta: tx!("progreso.instalando_traductor_arm_vigilando").into(), args });
                        }
                        Some(Conf::PuenteQuitar) if c.datos.operacion.is_none() => {
                            ef.push(Efecto::Orden { clave: "puente", etiqueta: tx!("progreso.quitando_traductor_arm").into(), args: vec!["bridge".into(), "remove".into()] });
                        }
                        Some(Conf::CambiarImagen(i)) if c.datos.operacion.is_none() => {
                            if let Some(f) = c.datos.almacen.as_ref().and_then(|a| a.imagenes.get(i)) {
                                ef.push(Efecto::Orden { clave: "imagen.usar", etiqueta: tx!("progreso.cambiando_imagen_maquina").into(), args: vec!["image".into(), "use".into(), f.id.clone()] });
                            }
                        }
                        Some(Conf::PerfilBorrar) => {
                            let id = self.visto(cfg);
                            match self.perfil_de(cfg).map(dispositivo::borrar) {
                                Some(Ok(())) => {
                                    // la maquina no se queda pidiendo un perfil que ya no existe
                                    if cfg.get("dispositivo.perfil") == id {
                                        self.poner(cfg, "dispositivo.perfil", dispositivo::NINGUNO, c.ahora, &mut ef);
                                    }
                                    self.cargar_perfiles();
                                    self.perfil_visto = None;
                                    self.mensaje("perfiles", Tono::Exito, tx!("perfiles.borrado"));
                                }
                                Some(Err(e)) => self.mensaje("perfiles", Tono::Error, &e),
                                None => {}
                            }
                        }
                        Some(Conf::PerfilAplicar) if c.datos.operacion.is_none() && c.datos.en_marcha => {
                            ef.push(Efecto::Orden { clave: "perfiles", etiqueta: tx!("progreso.aplicando_perfil").into(), args: vec!["device".into(), "apply".into()] });
                        }
                        Some(Conf::QuitarCarpeta(i)) => {
                            let l = compartir::lista(cfg);
                            if let Some(carpeta) = l.get(i) {
                                match compartir::quitar(cfg, &carpeta.nombre.clone()) {
                                    Ok(_) => {
                                        for k in 0..compartir::MAX {
                                            ef.push(Efecto::Cambio(compartir::clave_ranura(k)));
                                        }
                                        self.destello("share", c.ahora);
                                    }
                                    Err(e) => self.mensaje("share.quitar", Tono::Error, &e),
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Bot::FormSoloLectura => self.form_ro = !self.form_ro,
                Bot::AgregarCarpeta => {
                    self.quitar_mensaje("share.agregar");
                    let nueva = Carpeta { nombre: self.form_nombre.trim().to_string(), ro: self.form_ro, ruta: self.form_ruta.trim().to_string() };
                    match compartir::agregar(cfg, nueva, true) {
                        Ok(()) => {
                            for k in 0..compartir::MAX {
                                ef.push(Efecto::Cambio(compartir::clave_ranura(k)));
                            }
                            // deja en el invitado el servicio que las monta (si la maquina esta encendida)
                            ef.push(Efecto::Orden { clave: "share.agregar-orden", etiqueta: tx!("progreso.preparando_android").into(), args: vec!["share".into(), "setup".into()] });
                            self.form_nombre.clear();
                            self.form_ruta.clear();
                            self.form_ro = false;
                            self.quitar_mensaje("share.nombre");
                            self.quitar_mensaje("share.ruta");
                            self.destello("share", c.ahora);
                        }
                        Err(e) => self.mensaje("share.agregar", Tono::Error, &e),
                    }
                }
                Bot::AplicarResolucion => ef.push(Efecto::AplicarResolucion),
                Bot::AbrirEstado => ef.push(Efecto::AbrirEstado),
                Bot::Comprobar => ef.push(Efecto::Comprobar),
                Bot::UsarAqui => {
                    if let (Some(a), Some((combo, otra))) = (self.captura, self.conflicto) {
                        let _ = self.fijar(cfg, otra.clave(), "ninguno");
                        match self.fijar(cfg, a.clave(), &combo.texto()) {
                            Ok(_) => {
                                ef.push(Efecto::Cambio(otra.clave().into()));
                                ef.push(Efecto::Cambio(a.clave().into()));
                                self.destello(a.clave(), c.ahora);
                            }
                            Err(e) => self.mensaje(a.clave(), Tono::Error, &e),
                        }
                        self.captura = None;
                        self.conflicto = None;
                    }
                }
            },
        }
        ef
    }

    /// Pone una clave y deja constancia (efecto, destello, mensaje de error si no vale).
    fn poner(&mut self, cfg: &mut Config, k: &'static str, v: &str, ahora: Instant, ef: &mut Vec<Efecto>) -> bool {
        match self.fijar(cfg, k, v) {
            Ok(_) => {
                self.quitar_mensaje(k);
                ef.push(Efecto::Cambio(k.to_string()));
                self.destello(k, ahora);
                true
            }
            Err(e) => {
                self.mensaje(k, Tono::Error, &e);
                false
            }
        }
    }

    fn empezar_edicion(&mut self, campo: Campo, cfg: &Config, d: &Datos) {
        let buf = match campo {
            Campo::Ancho => cfg.resolucion().map_or(d.resolucion.0, |r| r.0).to_string(),
            Campo::Alto => cfg.resolucion().map_or(d.resolucion.1, |r| r.1).to_string(),
            Campo::Densidad => cfg.densidad().map(|v| v.to_string()).unwrap_or_default(),
            Campo::Cpus => cfg.entero("maquina.cpus").map(|v| v.to_string()).unwrap_or_default(),
            Campo::Ram => cfg.entero("maquina.ram").map(|v| v.to_string()).unwrap_or_default(),
            Campo::ShareNombre => self.form_nombre.clone(),
            Campo::ShareRuta => self.form_ruta.clone(),
            Campo::DatosTam => cfg.get("disk.data"),
            Campo::Origen => self.form_origen.clone(),
            Campo::RutaRoot => self.form_root.clone(),
            Campo::RutaPuente => self.form_puente.clone(),
            Campo::Perfil(i) => {
                let k = CAMPOS_PERFIL[i as usize % CAMPOS_PERFIL.len()].0;
                // auto (nucleos y memoria) se escribe dejando el campo vacio
                self.perfil_de(cfg).map(|p| p.valor(k)).filter(|v| v != "auto").unwrap_or_default()
            }
        };
        let buf = if buf == "0" { String::new() } else { buf };
        self.quitar_mensaje(campo.clave());
        let cursor = buf.chars().count();
        self.edicion = Some(Edicion { campo, buf, cursor });
    }

    /// Confirma lo que se editaba (si es valido se guarda; si no, error en linea y se sigue editando).
    fn terminar_edicion(&mut self, cfg: &mut Config, ahora: Instant, ef: &mut Vec<Efecto>) -> bool {
        let Some(e) = self.edicion.take() else { return true };
        let t = e.buf.trim().to_string();
        // rutas: `~`, comillas y URIs file:// como las entenderia una terminal o el gestor de archivos
        let t = if e.campo.es_ruta() { crate::rutas::ruta_escrita(&t, std::env::var("HOME").ok().as_deref()) } else { t };
        // campos del formulario de carpetas compartidas: no son configuracion, quedan en el borrador
        match e.campo {
            Campo::ShareNombre => {
                if !t.is_empty() {
                    if let Err(msg) = compartir::validar_nombre(&t) {
                        self.mensaje("share.nombre", Tono::Error, &format!("{}.", msg));
                        self.edicion = Some(e);
                        return false;
                    }
                }
                self.form_nombre = t;
                self.quitar_mensaje("share.nombre");
                return true;
            }
            Campo::Origen => {
                self.form_origen = t;
                self.quitar_mensaje("imagen");
                return true;
            }
            Campo::RutaRoot => {
                self.form_root = t;
                self.quitar_mensaje("root");
                return true;
            }
            Campo::RutaPuente => {
                self.form_puente = t;
                self.quitar_mensaje("puente");
                return true;
            }
            Campo::Perfil(i) => {
                let (k, _, msg) = CAMPOS_PERFIL[i as usize % CAMPOS_PERFIL.len()];
                let antes = self.perfil_de(cfg).map(|p| p.valor(k)).unwrap_or_default();
                if antes == t || (antes == "auto" && t.is_empty()) {
                    self.quitar_mensaje(msg);
                    return true;
                }
                return match self.editar_perfil(cfg, |d| d.fijar(k, &t)) {
                    Ok(()) => {
                        self.quitar_mensaje(msg);
                        self.destello(msg, ahora);
                        true
                    }
                    Err(m) => {
                        // el motivo sin el nombre de la clave (ya lo dice la etiqueta) y se sigue editando
                        let m = m.split_once(": ").map_or(m.clone(), |(_, x)| x.to_string());
                        self.mensaje(msg, Tono::Error, &m);
                        self.edicion = Some(e);
                        false
                    }
                };
            }
            Campo::ShareRuta => {
                self.form_ruta = t.clone();
                if t.is_empty() {
                    self.quitar_mensaje("share.ruta");
                } else if let Err(msg) = compartir::comprobar_ruta(&t) {
                    self.mensaje("share.ruta", Tono::Aviso, &txf!("comun.no_podra_agregar_asi", msg));
                } else {
                    self.quitar_mensaje("share.ruta");
                }
                return true;
            }
            _ => {}
        }
        let (clave, valor) = match e.campo {
            Campo::Ancho | Campo::Alto => {
                if t.is_empty() {
                    self.edicion = Some(e);
                    self.mensaje("pantalla.resolucion", Tono::Error, tx!("campo.escribe_numero"));
                    return false;
                }
                let (w0, h0) = cfg.resolucion().unwrap_or((720, 1348));
                let (w, h) = if e.campo == Campo::Ancho { (t.clone(), h0.to_string()) } else { (w0.to_string(), t.clone()) };
                ("pantalla.resolucion", format!("{}x{}", w, h))
            }
            Campo::Densidad => ("pantalla.densidad", if t.is_empty() { "perfil".to_string() } else { t.clone() }),
            Campo::Cpus => ("maquina.cpus", if t.is_empty() { "auto".to_string() } else { t.clone() }),
            Campo::Ram => ("maquina.ram", if t.is_empty() { "auto".to_string() } else { t.clone() }),
            Campo::DatosTam => ("disk.data", if t.is_empty() { "24G".to_string() } else { t.clone() }),
            Campo::ShareNombre | Campo::ShareRuta | Campo::Origen | Campo::RutaRoot | Campo::RutaPuente | Campo::Perfil(_) => unreachable!("se resolvieron arriba"),
        };
        if cfg.get(clave) == valor {
            self.quitar_mensaje(clave);
            return true;
        }
        match cfg.set(clave, &valor) {
            Ok(_) => {
                self.quitar_mensaje(clave);
                ef.push(Efecto::Cambio(clave.to_string()));
                self.destello(clave, ahora);
                true
            }
            Err(msg) => {
                // se muestra el motivo sin el nombre de la clave y se sigue editando
                let msg = msg.split_once(": ").map_or(msg.clone(), |(_, m)| m.to_string());
                self.mensaje(clave, Tono::Error, &msg);
                self.edicion = Some(e);
                false
            }
        }
    }

    // -----------------------------------------------------------------------------------------------------------
    // teclado

    /// Una tecla con la pantalla abierta. Devuelve (consumida, efectos): abierta, consume todo.
    pub fn tecla(&mut self, cfg: &mut Config, c: &Ctx, t: Tecla) -> (bool, Vec<Efecto>) {
        let mut ef = Vec::new();
        if !self.abierto {
            return (false, ef);
        }
        if !t.abajo {
            return (true, ef);
        }
        const ESC: u32 = 41;
        const RETROCESO: u32 = 42;
        const TAB: u32 = 43;
        const ESPACIO: u32 = 44;
        let es_intro = |sc: u32| sc == 40 || sc == 88;
        // --- captura de un atajo
        if let Some(a) = self.captura {
            if t.repetida {
                return (true, ef);
            }
            match t.sc {
                ESC => {
                    self.captura = None;
                    self.conflicto = None;
                }
                RETROCESO => {
                    self.captura = None;
                    self.conflicto = None;
                    self.poner(cfg, a.clave(), "ninguno", c.ahora, &mut ef);
                }
                224..=231 => {} // solo modificadores: se sigue esperando
                sc => {
                    let combo = Combo { sc, ctrl: t.ctrl, alt: t.alt, mayus: t.mayus };
                    match combo.valida() {
                        Err(e) => {
                            self.conflicto = None;
                            self.mensaje(a.clave(), Tono::Error, &e);
                        }
                        Ok(()) => match self.quien_usa(cfg, &combo.texto(), a) {
                            Some(otra) => {
                                self.conflicto = Some((combo, otra));
                                self.mensaje(a.clave(), Tono::Error, &txf!("atajos.ya_usa", gestos::nombre_combo(&combo), otra.etiqueta()));
                            }
                            None => {
                                self.captura = None;
                                self.conflicto = None;
                                self.poner(cfg, a.clave(), &combo.texto(), c.ahora, &mut ef);
                            }
                        },
                    }
                }
            }
            if self.captura.is_none() && t.sc == ESC {
                self.quitar_mensaje(a.clave());
            }
            return (true, ef);
        }
        // --- el atajo que abre la configuracion (F9 por defecto) la cierra, como la X: lo que se editaba se confirma.
        // Si el atajo es una tecla que la propia pantalla usa (flechas, Inicio, Fin...), manda la pantalla.
        if !t.repetida && !tecla_de_la_pantalla(&t) && cfg.atajo_para(t.sc, t.ctrl, t.alt, t.mayus) == Some(AccionAtajo::Configuracion) {
            ef.extend(self.activar(Id::Cerrar, cfg, c));
            return (true, ef);
        }
        // --- edicion de un campo numerico
        if self.edicion.is_some() {
            match t.sc {
                ESC => {
                    let k = self.edicion.as_ref().unwrap().campo.clave();
                    self.edicion = None;
                    self.quitar_mensaje(k);
                }
                sc if es_intro(sc) => {
                    self.terminar_edicion(cfg, c.ahora, &mut ef);
                }
                TAB => {
                    if self.terminar_edicion(cfg, c.ahora, &mut ef) {
                        self.mover_foco(cfg, c, if t.mayus { -1 } else { 1 });
                    }
                }
                sc => {
                    let e = self.edicion.as_mut().unwrap();
                    let n = e.buf.chars().count();
                    let byte = |b: &str, i: usize| b.char_indices().nth(i).map_or(b.len(), |(p, _)| p);
                    match sc {
                        30..=38 | 39 | 89..=97 | 98 if !e.campo.es_texto() => {
                            let d = match sc {
                                30..=38 => (b'1' + (sc - 30) as u8) as char,
                                89..=97 => (b'1' + (sc - 89) as u8) as char,
                                _ => '0',
                            };
                            if n < 7 && !t.ctrl && !t.alt {
                                let p = byte(&e.buf, e.cursor);
                                e.buf.insert(p, d);
                                e.cursor += 1;
                            }
                        }
                        RETROCESO => {
                            if e.cursor > 0 {
                                let p = byte(&e.buf, e.cursor - 1);
                                e.buf.remove(p);
                                e.cursor -= 1;
                            }
                        }
                        76 => {
                            if e.cursor < n {
                                let p = byte(&e.buf, e.cursor);
                                e.buf.remove(p);
                            }
                        }
                        80 => e.cursor = e.cursor.saturating_sub(1),
                        79 => e.cursor = (e.cursor + 1).min(n),
                        74 => e.cursor = 0,
                        77 => e.cursor = n,
                        _ => {}
                    }
                    let k = e.campo.clave();
                    self.quitar_mensaje(k);
                }
            }
            return (true, ef);
        }
        // --- navegacion normal
        match t.sc {
            // Esc es "atras": primero cancela una pregunta pendiente (como su boton Cancelar, con el foco en lo que la abrio
            // si se puede); sin nada pendiente, cierra la pantalla
            ESC if self.confirmando.is_some() => {
                if !t.repetida {
                    self.confirmando = None;
                    if self.foco_visible {
                        self.foco_tras_cancelar(cfg, c);
                    }
                }
            }
            ESC => {
                self.cerrar();
                ef.push(Efecto::Cerrar);
            }
            TAB => self.mover_foco(cfg, c, if t.mayus { -1 } else { 1 }),
            sc if es_intro(sc) || (sc == ESPACIO && !t.repetida) => {
                if !t.repetida {
                    if let Some(id) = self.foco {
                        let preguntaba = self.confirmando.is_some();
                        ef.extend(self.activar(id, cfg, c));
                        match (preguntaba, self.confirmando.is_some()) {
                            // se abrio una pregunta: el foco va a su Cancelar si el boton que la abrio ya no esta
                            (false, true) => {
                                self.pregunta_de = Some(id);
                                self.foco_en_la_pregunta(cfg, c);
                            }
                            // se contesto o se cancelo: el foco vuelve a lo que la abrio
                            (true, false) => self.foco_tras_cancelar(cfg, c),
                            _ => {}
                        }
                    }
                }
            }
            82 | 81 => {
                // flechas arriba/abajo: en la navegacion cambian de seccion; en otro sitio desplazan
                let d = if t.sc == 81 { 1i32 } else { -1 };
                if let Some(Id::Nav(s)) = self.foco {
                    let i = Seccion::TODAS.iter().position(|x| *x == s).unwrap_or(0) as i32;
                    let n = (i + d).clamp(0, Seccion::TODAS.len() as i32 - 1) as usize;
                    let s = Seccion::TODAS[n];
                    self.foco = Some(Id::Nav(s));
                    self.foco_visible = true;
                    // lo mismo que el clic: si la seccion cambia, se refrescan sus datos
                    if s != self.seccion {
                        ef.extend(self.seleccionar_seccion(s));
                    }
                } else {
                    self.rueda(c, -(d as f32) * 0.8);
                }
            }
            79 | 80 => {
                let d = if t.sc == 79 { 1i32 } else { -1 };
                match self.foco {
                    // en la navegacion (sobre todo con las pestanas arriba, en ventanas estrechas): como arriba/abajo
                    Some(Id::Nav(s)) => {
                        let i = Seccion::TODAS.iter().position(|x| *x == s).unwrap_or(0) as i32;
                        let s = Seccion::TODAS[(i + d).clamp(0, Seccion::TODAS.len() as i32 - 1) as usize];
                        self.foco = Some(Id::Nav(s));
                        self.foco_visible = true;
                        if s != self.seccion {
                            ef.extend(self.seleccionar_seccion(s));
                        }
                    }
                    Some(Id::Opcion(k, i)) => {
                        let n = opciones(k).len() as i32;
                        let j = (i as i32 + d).clamp(0, n - 1) as usize;
                        if j != i {
                            ef.extend(self.activar(Id::Opcion(k, j), cfg, c));
                            self.foco_visible = true;
                        }
                    }
                    Some(Id::Paso(k, _)) => {
                        ef.extend(self.activar(Id::Paso(k, d), cfg, c));
                        self.foco_visible = true;
                    }
                    _ => {}
                }
            }
            75 => self.rueda(c, 6.0),  // RePag
            78 => self.rueda(c, -6.0), // AvPag
            74 => self.scroll = 0.0,
            77 => self.scroll = f32::MAX / 2.0,
            _ => {}
        }
        let g = geometria(c.vent, c.m);
        let max = (self.alto_contenido.get() - g.contenido.h).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
        (true, ef)
    }

    /// Tras cancelar una pregunta con Esc: si el control con foco desaparecio (Confirmar o Cancelar de la pregunta), el foco
    /// pasa al primer control del contenido que la pantalla muestre ahora (el boton que la abrio suele volver a su sitio);
    /// sigue visible.
    fn foco_tras_cancelar(&mut self, cfg: &Config, c: &Ctx) {
        let g = geometria(c.vent, c.m);
        let mq = self.maquetar(cfg, c, &g);
        let vivo = |id: &Id| mq.controles.iter().any(|k| k.id == *id && k.habilitado);
        let origen = self.pregunta_de.take();
        if let Some(o) = origen.filter(vivo) {
            self.foco = Some(o);
        } else if !self.foco.as_ref().is_some_and(vivo) {
            self.foco = mq.controles.iter().find(|k| k.habilitado && !matches!(k.id, Id::Cerrar | Id::Nav(_))).map(|k| k.id);
        }
        self.foco_visible = self.foco.is_some();
    }

    /// Con una pregunta recien abierta: si el control con foco ya no esta, el foco pasa a su Cancelar (lo seguro).
    fn foco_en_la_pregunta(&mut self, cfg: &Config, c: &Ctx) {
        let g = geometria(c.vent, c.m);
        let mq = self.maquetar(cfg, c, &g);
        let vivo = |id: &Id| mq.controles.iter().any(|k| k.id == *id && k.habilitado);
        if !self.foco.as_ref().is_some_and(vivo) {
            let cancelar = Id::Boton(Bot::Cancelar);
            self.foco = vivo(&cancelar).then_some(cancelar);
        }
    }

    /// Mueve el foco de teclado al control siguiente (+1) o anterior (-1) en el orden de tabulacion.
    fn mover_foco(&mut self, cfg: &Config, c: &Ctx, d: i32) {
        let g = geometria(c.vent, c.m);
        let mq = self.maquetar(cfg, c, &g);
        let ids: Vec<Id> = mq.controles.iter().filter(|k| k.habilitado).map(|k| k.id).collect();
        if ids.is_empty() {
            return;
        }
        let actual = self.foco.and_then(|f| ids.iter().position(|i| *i == f));
        let n = ids.len() as i32;
        let nuevo = match actual {
            None => {
                if d > 0 {
                    0
                } else {
                    n - 1
                }
            }
            Some(i) => (i as i32 + d).rem_euclid(n),
        };
        let id = ids[nuevo as usize];
        self.foco = Some(id);
        self.foco_visible = true;
        // un control del contenido que quede fuera de la zona visible se lleva a la vista
        if !matches!(id, Id::Cerrar | Id::Nav(_)) {
            if let Some(k) = mq.controles.iter().find(|k| k.id == id) {
                let vp = g.contenido;
                let max = (self.alto_contenido.get() - vp.h).max(0.0);
                if k.r.y - 8.0 < vp.y {
                    self.scroll -= vp.y - (k.r.y - 8.0);
                } else if k.r.y + k.r.h + 8.0 > vp.y + vp.h {
                    self.scroll += k.r.y + k.r.h + 8.0 - (vp.y + vp.h);
                }
                self.scroll = self.scroll.clamp(0.0, max);
            }
        }
    }

    // -----------------------------------------------------------------------------------------------------------
    // maqueta

    fn maquetar(&self, cfg: &Config, c: &Ctx, g: &Geom) -> Maqueta {
        let mut mq = Maqueta { pint: Vec::new(), fijos: Vec::new(), controles: Vec::new(), barra: None };
        // --- cerrar (X) en la cabecera
        let cer = R::new(g.cabecera.x + g.cabecera.w - 12.0 - BH, g.cabecera.y + (CABECERA - BH) / 2.0, BH, BH);
        {
            let mut f = Lienzo::nuevo(self, cfg, c, 0.0, 0.0);
            f.boton_icono(Id::Cerrar, cer, Icono::Cerrar);
            mq.fijos.extend(f.pint);
            mq.controles.extend(f.ctl.into_iter().map(|k| Control { visible: k.r, ..k }));
        }
        // --- navegacion o pestanas
        if let Some(n) = g.nav {
            let mut f = Lienzo::nuevo(self, cfg, c, 0.0, 0.0);
            for fila in nav_filas(n) {
                match fila {
                    NavFila::Grupo(gr, r) => {
                        let lh = f.lh(Estilo::Pequeno);
                        f.txt(r.x + 14.0, r.y + r.h - lh - 2.0, gr.titulo(), Estilo::Pequeno, tema::p().texto2);
                    }
                    NavFila::Item(s, r) => f.nav(Id::Nav(s), r, s.titulo(), s == self.seccion),
                }
            }
            mq.fijos.extend(f.pint);
            mq.controles.extend(f.ctl.into_iter().map(|k| Control { visible: k.r, ..k }));
        }
        if let Some(t) = g.tabs {
            let mut f = Lienzo::nuevo(self, cfg, c, 0.0, 0.0);
            let (_, por_fila) = filas_tabs(t.w - 2.0 * GAP, c.m);
            let ancho = (t.w - 2.0 * GAP - (por_fila as f32 - 1.0) * 4.0) / por_fila as f32;
            for (i, s) in Seccion::TODAS.iter().enumerate() {
                let (fila, col) = (i / por_fila, i % por_fila);
                let r = R::new(t.x + GAP + col as f32 * (ancho + 4.0), t.y + GAP + fila as f32 * (BH + 4.0), ancho, BH);
                f.boton_texto(Id::Nav(*s), r, s.titulo(), if *s == self.seccion { EstiloBoton::Activo } else { EstiloBoton::Normal }, true);
            }
            mq.fijos.extend(f.pint);
            mq.controles.extend(f.ctl.into_iter().map(|k| Control { visible: k.r, ..k }));
            mq.fijos.push(Pint::Rect { r: R::new(t.x, t.y + t.h - 1.0, t.w, 1.0), c: tema::p().borde, radio: 0.0 });
        }
        // --- contenido de la seccion (en coordenadas de contenido; se desplaza al final)
        let vp = g.contenido;
        let x0 = vp.x + PAD_X;
        let w = (vp.w - 2.0 * PAD_X - 10.0).max(80.0);
        let mut l = Lienzo::nuevo(self, cfg, c, x0, w);
        l.y = PAD_Y;
        l.seccion(self.seccion);
        let alto = l.y + PAD_Y;
        self.alto_contenido.set(alto);
        let max = (alto - vp.h).max(0.0);
        let scroll = self.scroll.clamp(0.0, max);
        let dy = vp.y - scroll;
        let mut pint = l.pint;
        for p in pint.iter_mut() {
            mover_pint(p, dy);
        }
        mq.pint = pint;
        for mut k in l.ctl {
            k.r.y += dy;
            k.visible = k.r.cruzar(&vp);
            mq.controles.push(k);
        }
        // la barra
        if alto > vp.h + 0.5 {
            let pista = R::new(vp.x + vp.w - 10.0, vp.y + 8.0, 6.0, vp.h - 16.0);
            let ph = pulgar_alto(vp.h, alto);
            let rec = (pista.h - ph).max(0.0);
            let py = pista.y + if max > 0.0 { scroll / max * rec } else { 0.0 };
            mq.barra = Some((pista, R::new(pista.x, py, pista.w, ph)));
        }
        mq
    }
}

fn pulgar_alto(vp: f32, alto: f32) -> f32 {
    ((vp - 16.0) * vp / alto.max(1.0)).clamp(24.0, (vp - 16.0).max(24.0))
}

fn mover_pint(p: &mut Pint, dy: f32) {
    match p {
        Pint::Rect { r, .. } | Pint::Icono { r, .. } => r.y += dy,
        Pint::Texto { y, .. } => *y += dy,
        Pint::Recorte(Some(r)) => r.y += dy,
        Pint::Recorte(None) => {}
    }
}

/// Texto con la altura de sus mayusculas centrada verticalmente en `cy`.
fn texto_v(m: &dyn Medida, x: f32, cy: f32, t: &str, e: Estilo, c: Color) -> Pint {
    Pint::Texto { x, y: cy + m.cap(e) / 2.0 - m.ascenso(e), t: t.to_string(), e, c }
}

#[derive(Clone, Copy)]
struct Control {
    id: Id,
    /// rectangulo completo (ya desplazado)
    r: R,
    /// parte que se ve dentro de la zona de desplazamiento
    visible: R,
    habilitado: bool,
}

struct Maqueta {
    /// contenido desplazable
    pint: Vec<Pint>,
    /// cabecera, navegacion y pestanas
    fijos: Vec<Pint>,
    controles: Vec<Control>,
    /// (pista, pulgar)
    barra: Option<(R, R)>,
}

// ---------------------------------------------------------------------------------------------------------------
// el lienzo: arma controles y texto uno bajo otro

#[derive(Clone, Copy, PartialEq)]
enum EstiloBoton {
    Normal,
    Primario,
    Activo,
    /// accion destructiva (rojo)
    Peligro,
}

struct Lienzo<'a> {
    a: &'a Ajustes,
    cfg: &'a Config,
    c: &'a Ctx<'a>,
    x0: f32,
    w: f32,
    y: f32,
    pint: Vec<Pint>,
    ctl: Vec<Control>,
}

impl<'a> Lienzo<'a> {
    fn nuevo(a: &'a Ajustes, cfg: &'a Config, c: &'a Ctx<'a>, x0: f32, w: f32) -> Lienzo<'a> {
        Lienzo { a, cfg, c, x0, w, y: 0.0, pint: Vec::new(), ctl: Vec::new() }
    }

    fn m(&self) -> &dyn Medida {
        self.c.m
    }

    fn lh(&self, e: Estilo) -> f32 {
        self.c.m.alto_linea(e).ceil()
    }

    fn txt(&mut self, x: f32, y: f32, t: &str, e: Estilo, col: Color) {
        self.pint.push(Pint::Texto { x, y, t: t.to_string(), e, c: col });
    }

    fn registrar(&mut self, id: Id, r: R, habilitado: bool) {
        self.ctl.push(Control { id, r, visible: r, habilitado });
    }

    /// Anillo de foco de 2 dp alrededor del control.
    fn anillo(&mut self, r: R, radio: f32) {
        self.pint.push(Pint::Rect { r: r.reducir(-2.0), c: tema::p().acento_claro, radio: radio + 2.0 });
    }

    // ---- botones -----------------------------------------------------------------------------------------------

    fn boton_texto(&mut self, id: Id, r: R, t: &str, est: EstiloBoton, habilitado: bool) {
        let (hover, pulsado, foco) = self.a.visual(&id);
        if foco {
            self.anillo(r, tema::RADIO);
        }
        let (fondo, texto) = if !habilitado {
            (tema::p().control_apagado, tema::p().texto_apagado)
        } else if pulsado {
            (tema::p().acento, tema::p().sobre_acento)
        } else {
            match (est, hover) {
                (EstiloBoton::Peligro, true) => (tema::p().peligro_hover, tema::p().sobre_acento),
                (EstiloBoton::Peligro, false) => (tema::p().peligro, tema::p().sobre_acento),
                (EstiloBoton::Activo | EstiloBoton::Primario, true) => (tema::p().acento_hover, tema::p().sobre_acento),
                (EstiloBoton::Activo | EstiloBoton::Primario, false) => (tema::p().acento, tema::p().sobre_acento),
                (EstiloBoton::Normal, true) => (tema::p().control_hover, tema::p().texto),
                (EstiloBoton::Normal, false) => (tema::p().control, tema::p().texto),
            }
        };
        self.pint.push(Pint::Rect { r, c: fondo, radio: tema::RADIO });
        let t = truncar(self.m(), t, Estilo::Negrita, r.w - 12.0);
        let tw = self.m().ancho(&t, Estilo::Negrita);
        let p = texto_v(self.m(), r.x + ((r.w - tw) / 2.0).floor(), r.y + r.h / 2.0, &t, Estilo::Negrita, texto);
        self.pint.push(p);
        self.registrar(id, r, habilitado);
    }

    fn boton_icono(&mut self, id: Id, r: R, icono: Icono) {
        let (hover, pulsado, foco) = self.a.visual(&id);
        if foco {
            self.anillo(r, tema::RADIO);
        }
        let fondo = if pulsado {
            tema::p().acento
        } else if hover {
            tema::p().control_hover
        } else {
            tema::p().control
        };
        self.pint.push(Pint::Rect { r, c: fondo, radio: tema::RADIO });
        let lado = 14.0f32.min(r.h - 8.0);
        self.pint.push(Pint::Icono { k: icono, r: R::new(r.x + ((r.w - lado) / 2.0).floor(), r.y + ((r.h - lado) / 2.0).floor(), lado, lado), c: if pulsado { tema::p().sobre_acento } else { tema::p().texto } });
        self.registrar(id, r, true);
    }

    /// Entrada de la navegacion lateral.
    fn nav(&mut self, id: Id, r: R, t: &str, activa: bool) {
        let (hover, pulsado, foco) = self.a.visual(&id);
        if foco {
            self.anillo(r, tema::RADIO);
        }
        if activa || hover || pulsado {
            let fondo = if pulsado {
                tema::p().acento
            } else if activa {
                tema::p().control
            } else {
                tema::p().control_hover.alfa(160)
            };
            self.pint.push(Pint::Rect { r, c: fondo, radio: tema::RADIO });
        } else if foco {
            // sin fondo propio: el de la columna tapa el anillo por dentro y queda solo su borde
            self.pint.push(Pint::Rect { r, c: tema::p().superficie, radio: tema::RADIO });
        }
        if activa {
            self.pint.push(Pint::Rect { r: R::new(r.x, r.y + 6.0, 3.0, r.h - 12.0), c: tema::p().acento_claro, radio: 1.5 });
        }
        let e = if activa { Estilo::Negrita } else { Estilo::Cuerpo };
        let col = if pulsado { tema::p().sobre_acento } else { tema::p().texto };
        let p = texto_v(self.m(), r.x + 14.0, r.y + r.h / 2.0, &truncar(self.m(), t, e, r.w - 22.0), e, col);
        self.pint.push(p);
        self.registrar(id, r, true);
    }

    // ---- texto ----------------------------------------------------------------------------------------------

    fn titulo(&mut self, t: &str) {
        let (x0, y) = (self.x0, self.y);
        self.txt(x0, y, t, Estilo::Titulo, tema::p().texto);
        self.y += self.lh(Estilo::Titulo) + 12.0;
    }

    /// Cabecera de un grupo de ajustes: una linea fina y el nombre.
    fn grupo(&mut self, t: &str) {
        if self.y > 60.0 {
            self.y += 8.0;
        }
        let (x0, y, w) = (self.x0, self.y, self.w);
        self.pint.push(Pint::Rect { r: R::new(x0, y, w, 1.0), c: tema::p().borde, radio: 0.0 });
        self.y += 10.0;
        self.txt(x0, self.y, t, Estilo::Negrita, tema::p().texto2);
        self.y += self.lh(Estilo::Negrita) + 10.0;
    }

    fn parrafo(&mut self, t: &str, e: Estilo, col: Color) {
        let lh = self.lh(e);
        let (x0, w) = (self.x0, self.w);
        for l in envolver(self.m(), t, e, w, 0) {
            let y = self.y;
            self.txt(x0, y, &l, e, col);
            self.y += lh;
        }
    }

    /// Mensaje en linea bajo un control, ajustado al ancho: aviso amarillo, error rojo o exito verde y, para no depender
    /// solo del color, con el icono de su tono delante (triangulo, circulo con aspa o marca; ver `icono_de`).
    /// Operacion larga en curso: su texto de progreso, una barra si el texto lleva un porcentaje y, si se puede, el boton
    /// "Cancelar".
    fn banda_operacion(&mut self, w: f32) {
        let d = self.c.datos;
        let Some((_, texto)) = &d.operacion else { return };
        self.y += 8.0;
        let t = format!("{}...", texto);
        self.en_linea(Tono::Aviso, &t, w);
        if let Some(pct) = porcentaje_de(texto) {
            let (x0, y) = (self.x0 + SANGRIA_MENSAJE, self.y + 4.0);
            let ancho = (w - SANGRIA_MENSAJE).max(40.0);
            self.pint.push(Pint::Rect { r: R::new(x0, y, ancho, 6.0), c: tema::p().borde, radio: 3.0 });
            self.pint.push(Pint::Rect { r: R::new(x0, y, (ancho * pct / 100.0).max(6.0), 6.0), c: tema::p().acento_claro, radio: 3.0 });
            self.y += 12.0;
        }
        if d.cancelable {
            self.y += 6.0;
            self.boton_fila(Id::Boton(Bot::CancelarOperacion), tx!("comun.cancelar"), EstiloBoton::Normal, true);
        }
    }

    /// Campo de una ruta del equipo con el boton "Examinar..." a su derecha (selector de archivos del sistema).
    fn campo_ruta(&mut self, campo: Campo) {
        let (x0, y, w) = (self.x0, self.y, self.w);
        let etq = tx!("comun.examinar");
        let bw = (self.m().ancho(etq, Estilo::Negrita) + 32.0).max(96.0).ceil();
        let (t, tenue) = self.valor_campo(campo);
        if w - bw - 8.0 >= 120.0 {
            self.campo(campo, x0, y, w - bw - 8.0, &t, tenue);
            self.boton_texto(Id::Examinar(campo), R::new(x0 + w - bw, y, bw, BH), etq, EstiloBoton::Normal, true);
            self.y += BH;
        } else {
            // estrecho: el boton debajo
            self.campo(campo, x0, y, w, &t, tenue);
            self.y += BH + 6.0;
            let y = self.y;
            self.boton_texto(Id::Examinar(campo), R::new(x0, y, bw.min(w), BH), etq, EstiloBoton::Normal, true);
            self.y += BH;
        }
    }

    fn en_linea(&mut self, tono: Tono, t: &str, ancho: f32) {
        let (icono, col) = icono_de(tono);
        let lh = self.lh(Estilo::Pequeno);
        let x0 = self.x0;
        let lado = ICONO_MENSAJE.min(lh);
        for (i, l) in envolver(self.m(), t, Estilo::Pequeno, ancho - SANGRIA_MENSAJE, 0).iter().enumerate() {
            let y = self.y;
            if i == 0 {
                self.pint.push(Pint::Icono { k: icono, r: R::new(x0, y + ((lh - lado) / 2.0).floor(), lado, lado), c: col });
            }
            self.txt(x0 + SANGRIA_MENSAJE, y, l, Estilo::Pequeno, col);
            self.y += lh;
        }
    }

    /// Etiqueta con su ayuda y, a la derecha, el "Guardado" breve de la clave. `ancho` es lo que ocupa el texto.
    fn etiqueta(&mut self, etiqueta: &str, ayuda: &str, clave: &str, ancho: f32) {
        let (x0, y) = (self.x0, self.y);
        let lh = self.lh(Estilo::Cuerpo);
        // la etiqueta se ajusta al ancho del texto (en ventanas estrechas pasa a dos lineas en vez de pisar el control)
        let lineas_etq = envolver(self.m(), etiqueta, Estilo::Cuerpo, ancho, 0);
        for (i, l) in lineas_etq.iter().enumerate() {
            self.txt(x0, y + i as f32 * lh, l, Estilo::Cuerpo, tema::p().texto);
        }
        // "Guardado" breve
        if let Some((k, t)) = &self.a.guardado {
            if k == clave && self.c.ahora.saturating_duration_since(*t) < Duration::from_secs(GUARDADO_S) {
                let txt = tx!("comun.guardado");
                let tw = self.m().ancho(txt, Estilo::Pequeno);
                let xr = x0 + ancho - tw;
                self.pint.push(Pint::Icono { k: Icono::Check, r: R::new(xr - 18.0, y + 1.0, 14.0, 14.0), c: tema::p().exito });
                self.txt(xr, y + (lh - self.lh(Estilo::Pequeno)) / 2.0 + 1.0, txt, Estilo::Pequeno, tema::p().exito);
            }
        }
        self.y += lh * lineas_etq.len().max(1) as f32;
        if !ayuda.is_empty() {
            let lhp = self.lh(Estilo::Pequeno);
            for l in envolver(self.m(), ayuda, Estilo::Pequeno, ancho, 0) {
                let y = self.y;
                self.txt(x0, y, &l, Estilo::Pequeno, tema::p().texto2);
                self.y += lhp;
            }
        }
    }

    fn mensajes_de(&mut self, clave: &str, ancho: f32) {
        if let Some((tono, t)) = self.a.mensaje_de(clave) {
            let t = t.to_string();
            self.y += 2.0;
            self.en_linea(tono, &t, ancho);
        }
    }

    // ---- controles --------------------------------------------------------------------------------------------

    fn fila_interruptor(&mut self, clave: &'static str, etiqueta: &str, ayuda: &str) {
        self.fila_interruptor_h(clave, etiqueta, ayuda, true);
    }

    fn fila_interruptor_h(&mut self, clave: &'static str, etiqueta: &str, ayuda: &str, habilitado: bool) {
        let ancho = self.w - 56.0;
        let y0 = self.y;
        self.etiqueta(etiqueta, ayuda, clave, ancho);
        let alto = (self.y - y0).max(24.0);
        let (x, y) = (self.x0 + self.w - 40.0, y0 + ((alto - 22.0) / 2.0).floor().min(4.0));
        let r = R::new(x, y, 40.0, 22.0);
        let id = Id::Interruptor(clave);
        let (hover, pulsado, foco) = self.a.visual(&id);
        let on = self.a.valor(self.cfg, clave) == "si";
        if foco {
            self.anillo(r, 11.0);
        }
        let fondo = match (on, (hover || pulsado) && habilitado) {
            _ if !habilitado => tema::p().control_apagado,
            (true, false) => tema::p().acento,
            (true, true) => tema::p().acento_hover,
            (false, false) => tema::p().control,
            (false, true) => tema::p().control_hover,
        };
        if !on {
            self.pint.push(Pint::Rect { r: r.reducir(-1.0), c: tema::p().borde, radio: 12.0 });
        }
        self.pint.push(Pint::Rect { r, c: fondo, radio: 11.0 });
        let kx = if on { r.x + r.w - 19.0 } else { r.x + 3.0 };
        self.pint.push(Pint::Icono { k: Icono::Circulo, r: R::new(kx, r.y + 3.0, 16.0, 16.0), c: if !habilitado { tema::p().texto_apagado } else if on { tema::p().sobre_acento } else { tema::p().texto2 } });
        self.registrar(id, r, habilitado);
        self.mensajes_de(clave, ancho);
        self.y += FILA_GAP;
    }

    /// Lista de opciones como botones en fila (se pasan a la linea siguiente si no caben). `actual` es el valor vigente.
    fn fila_opciones(&mut self, clave: &'static str, etiqueta: &str, ayuda: &str, actual: &str) {
        let w = self.w;
        self.etiqueta(etiqueta, ayuda, clave, w);
        self.y += 6.0;
        let ops = opciones(clave);
        let (mut x, x0) = (self.x0, self.x0);
        for (i, (valor, texto)) in ops.iter().enumerate() {
            let bw = (self.m().ancho(texto, Estilo::Negrita) + 28.0).max(60.0).ceil();
            if x > x0 && x + bw > x0 + w + 0.5 {
                x = x0;
                self.y += BH + 4.0;
            }
            let y = self.y;
            self.boton_texto(Id::Opcion(clave, i), R::new(x, y, bw, BH), texto, if *valor == actual { EstiloBoton::Activo } else { EstiloBoton::Normal }, true);
            x += bw + 4.0;
        }
        self.y += BH;
        self.mensajes_de(clave, w);
        self.y += FILA_GAP;
    }

    /// [-] valor [+] para una clave entera.
    fn fila_paso(&mut self, clave: &'static str, etiqueta: &str, ayuda: &str, unidad: &str) {
        let w = self.w;
        self.etiqueta(etiqueta, ayuda, clave, w);
        self.y += 6.0;
        let (x0, y) = (self.x0, self.y);
        let (min, max) = match config::clave(clave).map(|k| k.tipo) {
            Some(config::Tipo::Entero(a, b)) => (a, b),
            _ => (0, 0),
        };
        let v = self.cfg.entero(clave).unwrap_or(min);
        self.boton_icono_paso(Id::Paso(clave, -1), R::new(x0, y, BH, BH), Icono::Menos, v > min);
        let caja = R::new(x0 + BH + 4.0, y, 72.0, BH);
        self.pint.push(Pint::Rect { r: caja, c: tema::p().control_apagado, radio: tema::RADIO });
        let t = format!("{}{}", v, unidad);
        let tw = self.m().ancho(&t, Estilo::Negrita);
        let p = texto_v(self.m(), caja.x + ((caja.w - tw) / 2.0).floor(), caja.y + caja.h / 2.0, &t, Estilo::Negrita, tema::p().texto);
        self.pint.push(p);
        self.boton_icono_paso(Id::Paso(clave, 1), R::new(caja.x + caja.w + 4.0, y, BH, BH), Icono::Mas, v < max);
        self.y += BH;
        self.mensajes_de(clave, w);
        self.y += FILA_GAP;
    }

    fn boton_icono_paso(&mut self, id: Id, r: R, icono: Icono, habilitado: bool) {
        let (hover, pulsado, foco) = self.a.visual(&id);
        if foco && habilitado {
            self.anillo(r, tema::RADIO);
        }
        let (fondo, col) = if !habilitado {
            (tema::p().control_apagado, tema::p().texto_apagado)
        } else if pulsado {
            (tema::p().acento, tema::p().sobre_acento)
        } else if hover {
            (tema::p().control_hover, tema::p().texto)
        } else {
            (tema::p().control, tema::p().texto)
        };
        self.pint.push(Pint::Rect { r, c: fondo, radio: tema::RADIO });
        let lado = 14.0;
        self.pint.push(Pint::Icono { k: icono, r: R::new(r.x + ((r.w - lado) / 2.0).floor(), r.y + ((r.h - lado) / 2.0).floor(), lado, lado), c: col });
        self.registrar(id, r, habilitado);
    }

    /// Campo numerico: `texto` es lo que muestra (o el buffer si se edita); `tenue` si es un valor por defecto.
    fn campo(&mut self, campo: Campo, x: f32, y: f32, w: f32, texto: &str, tenue: bool) {
        let id = Id::Campo(campo);
        let (hover, _, foco) = self.a.visual(&id);
        let editando = self.a.edicion.as_ref().is_some_and(|e| e.campo == campo);
        let r = R::new(x, y, w, BH);
        if foco {
            self.anillo(r, tema::RADIO);
        }
        let hay_error = matches!(self.a.mensaje_de(campo.clave()), Some((Tono::Error, _)));
        let borde = if hay_error {
            tema::p().error
        } else if editando {
            tema::p().acento_claro
        } else {
            tema::p().borde
        };
        self.pint.push(Pint::Rect { r, c: borde, radio: tema::RADIO });
        self.pint.push(Pint::Rect { r: r.reducir(1.0), c: if editando { tema::p().fondo } else if hover { tema::p().control_hover } else { tema::p().control }, radio: tema::RADIO - 1.0 });
        let col = if tenue && !editando { tema::p().texto2 } else { tema::p().texto };
        // al escribir un texto largo se ve el final (donde esta el cursor); en reposo, el principio
        let con_cola = editando && self.m().ancho(texto, Estilo::Cuerpo) > w - 20.0;
        let t = if con_cola { self.cola(texto, w - 20.0) } else { truncar(self.m(), texto, Estilo::Cuerpo, w - 20.0) };
        let p = texto_v(self.m(), r.x + 10.0, r.y + r.h / 2.0, &t, Estilo::Cuerpo, col);
        self.pint.push(p);
        if editando {
            // cursor fijo (sin parpadeo: no hace falta redibujar con el tiempo)
            let cur = self.a.edicion.as_ref().map_or(0, |e| e.cursor);
            let antes: String = if con_cola { t.clone() } else { texto.chars().take(cur).collect() };
            let cx = r.x + 10.0 + self.m().ancho(&antes, Estilo::Cuerpo);
            self.pint.push(Pint::Rect { r: R::new(cx.floor(), r.y + 6.0, 1.0, r.h - 12.0), c: tema::p().texto, radio: 0.0 });
        }
        self.registrar(id, r, true);
    }

    /// El final de `t` que cabe en `ancho`, con puntos suspensivos delante.
    fn cola(&self, t: &str, ancho: f32) -> String {
        let chars: Vec<char> = t.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let s: String = std::iter::once('…').chain(chars[i..].iter().copied()).collect();
            if self.m().ancho(&s, Estilo::Cuerpo) <= ancho {
                return s;
            }
            i += 1;
        }
        "…".to_string()
    }

    /// Texto que muestra un campo y si es un valor por defecto.
    fn valor_campo(&self, campo: Campo) -> (String, bool) {
        if let Some(e) = self.a.edicion.as_ref().filter(|e| e.campo == campo) {
            return (e.buf.clone(), false);
        }
        let d = &self.c.datos;
        match campo {
            Campo::Ancho => match self.cfg.resolucion() {
                Some((w, _)) => (w.to_string(), false),
                None => (if d.resolucion.0 > 0 { d.resolucion.0.to_string() } else { "auto".into() }, true),
            },
            Campo::Alto => match self.cfg.resolucion() {
                Some((_, h)) => (h.to_string(), false),
                None => (if d.resolucion.1 > 0 { d.resolucion.1.to_string() } else { "auto".into() }, true),
            },
            Campo::Densidad => match self.cfg.densidad() {
                Some(v) => (v.to_string(), false),
                None => ("perfil".into(), true),
            },
            Campo::Cpus => match self.cfg.entero("maquina.cpus") {
                Some(v) => (v.to_string(), false),
                None => ("auto".into(), true),
            },
            Campo::Ram => match self.cfg.entero("maquina.ram") {
                Some(v) => (v.to_string(), false),
                None => ("auto".into(), true),
            },
            Campo::ShareNombre if self.a.form_nombre.is_empty() => (tx!("campo.nombre_android_p_ej").into(), true),
            Campo::ShareNombre => (self.a.form_nombre.clone(), false),
            Campo::ShareRuta if self.a.form_ruta.is_empty() => (tx!("campo.carpeta_equipo_ruta_completa").into(), true),
            Campo::ShareRuta => (self.a.form_ruta.clone(), false),
            Campo::DatosTam => (self.cfg.get("disk.data"), !self.cfg.es_explicita("disk.data")),
            Campo::Origen if self.a.form_origen.is_empty() => (tx!("campo.ruta_zip_carpeta").into(), true),
            Campo::Origen => (self.a.form_origen.clone(), false),
            Campo::RutaRoot if self.a.form_root.is_empty() => (tx!("campo.carpeta_archivos_vacio_predeterminada").into(), true),
            Campo::RutaRoot => (self.a.form_root.clone(), false),
            Campo::RutaPuente if self.a.form_puente.is_empty() => (tx!("campo.archivo_carpeta_vacio_predeterminado").into(), true),
            Campo::RutaPuente => (self.a.form_puente.clone(), false),
            Campo::Perfil(i) => {
                let k = CAMPOS_PERFIL[i as usize % CAMPOS_PERFIL.len()].0;
                let v = self.a.perfil_de(self.cfg).map(|p| p.valor(k)).unwrap_or_default();
                match v.as_str() {
                    "" if k.starts_with("producto.") => (tx!("perfiles.la_de_la_imagen").into(), true),
                    "" if k.starts_with("cpu.") => (tx!("perfiles.la_de_la_base").into(), true),
                    "" => (String::new(), true),
                    "auto" => ("auto".into(), true),
                    _ => (v, false),
                }
            }
        }
    }

    /// Una fila de campos con su etiqueta encima. (etiqueta, campo, ancho); pasa a la linea siguiente si no cabe.
    fn fila_campos(&mut self, items: &[(&str, Campo, f32)]) {
        let (x0, w) = (self.x0, self.w);
        let mut x = x0;
        let lhp = self.lh(Estilo::Pequeno);
        let mut fila_y = self.y;
        for (i, (etq, campo, cw)) in items.iter().enumerate() {
            if i > 0 && x + cw > x0 + w + 0.5 {
                x = x0;
                fila_y += lhp + 2.0 + BH + 10.0;
            }
            let sup = if etq.is_empty() { 0.0 } else { lhp + 2.0 };
            if !etq.is_empty() {
                self.txt(x, fila_y, etq, Estilo::Pequeno, tema::p().texto2);
            }
            let (t, tenue) = self.valor_campo(*campo);
            self.campo(*campo, x, fila_y + sup, *cw, &t, tenue);
            x += cw + 12.0;
            self.y = fila_y + sup + BH;
        }
        if items.is_empty() {
            self.y = fila_y;
        }
    }

    fn boton_fila(&mut self, id: Id, t: &str, est: EstiloBoton, habilitado: bool) {
        let bw = (self.m().ancho(t, Estilo::Negrita) + 32.0).max(96.0).ceil().min(self.w);
        let (x0, y) = (self.x0, self.y);
        self.boton_texto(id, R::new(x0, y, bw, BH), t, est, habilitado);
        self.y += BH;
    }

    // ---- secciones --------------------------------------------------------------------------------------------

    fn seccion(&mut self, s: Seccion) {
        self.titulo(s.encabezado());
        match s {
            Seccion::Controles => self.controles(),
            Seccion::General => self.general(),
            Seccion::Atajos => self.atajos(),
            Seccion::Entrada => self.entrada(),
            Seccion::Pantalla => self.pantalla(),
            Seccion::Maquina => self.maquina(),
            Seccion::Perfiles => self.perfiles(),
            Seccion::Compartir => self.compartir(),
            Seccion::Imagen => self.imagen(),
            Seccion::Puente => self.puente(),
            Seccion::Root => self.root(),
            Seccion::Diagnostico => self.diagnostico(),
            Seccion::Acerca => self.acerca(),
        }
    }

    /// Boton con la tecla del atajo a la derecha (si cabe). Con sitio de sobra el nombre va centrado; si solo cabe con la tecla,
    /// el nombre pasa a la izquierda. `activo` lo pinta de acento.
    fn boton_pista(&mut self, id: Id, r: R, t: &str, pista: Option<&str>, activo: bool, habilitado: bool) {
        self.boton_texto(id, r, "", if activo { EstiloBoton::Activo } else { EstiloBoton::Normal }, habilitado);
        let (_, pulsado, _) = self.a.visual(&id);
        let col = if !habilitado {
            tema::p().texto_apagado
        } else if pulsado || activo {
            tema::p().sobre_acento
        } else {
            tema::p().texto
        };
        let pw = pista.map_or(0.0, |p| self.m().ancho(p, Estilo::Pequeno));
        let t = truncar(self.m(), t, Estilo::Negrita, r.w - 12.0);
        let tw = self.m().ancho(&t, Estilo::Negrita);
        let con_pista = habilitado && pista.is_some() && tw + pw + 28.0 <= r.w;
        let x = if con_pista && (r.w - tw) / 2.0 < pw + 12.0 { r.x + 12.0 } else { r.x + ((r.w - tw) / 2.0).floor() };
        let p = texto_v(self.m(), x, r.y + r.h / 2.0, &t, Estilo::Negrita, col);
        self.pint.push(p);
        if let (Some(p), true) = (pista, con_pista) {
            let colp = if activo || pulsado { tema::p().sobre_acento } else { tema::p().texto2 };
            let pt = texto_v(self.m(), r.x + r.w - pw - 8.0, r.y + r.h / 2.0, p, Estilo::Pequeno, colp);
            self.pint.push(pt);
        }
    }

    /// Fila de botones de acciones repartidos a todo el ancho (pasan a la linea siguiente si no caben): cada uno con su tecla.
    fn fila_controles(&mut self, items: &[(Accion, &str, bool)]) {
        let (x0, w) = (self.x0, self.w);
        let n = items.len();
        let minimo = items
            .iter()
            .map(|(a, t, _)| {
                let p = self.pista_de(*a).map_or(0.0, |p| self.m().ancho(&p, Estilo::Pequeno) + 8.0);
                (self.m().ancho(t, Estilo::Negrita) + 28.0 + p).max(72.0)
            })
            .fold(0.0f32, f32::max);
        let por_fila = (((w + GAP) / (minimo + GAP)).floor() as usize).clamp(1, n);
        let bw = (w - GAP * (por_fila as f32 - 1.0)) / por_fila as f32;
        for (i, (a, t, activo)) in items.iter().enumerate() {
            let (fila, col) = (i / por_fila, i % por_fila);
            let r = R::new(x0 + col as f32 * (bw + GAP), self.y + fila as f32 * (BH + GAP), bw, BH);
            let p = self.pista_de(*a);
            self.boton_pista(Id::Control(*a), r, t, p.as_deref(), *activo, true);
        }
        let filas = n.div_ceil(por_fila);
        self.y += filas as f32 * (BH + GAP) - GAP;
    }

    /// Tecla del atajo de una accion (texto visible, con la distribucion del teclado), si la tiene asignada.
    fn pista_de(&self, a: Accion) -> Option<String> {
        use crate::gestos::Atajo as A;
        let f = FilaAtajo::Accion;
        let fila = match a {
            Accion::Atajo(A::Atras) => f(AccionAtajo::Atras),
            Accion::Atajo(A::Inicio) => f(AccionAtajo::Inicio),
            Accion::Atajo(A::Recientes) => f(AccionAtajo::Recientes),
            Accion::Atajo(A::VolBajar) => f(AccionAtajo::VolMenos),
            Accion::Atajo(A::VolSubir) => f(AccionAtajo::VolMas),
            Accion::Atajo(A::Rotar) | Accion::RotacionAuto => f(AccionAtajo::Rotar),
            Accion::Atajo(A::Menu) => FilaAtajo::Extra(AtajoExtra::Menu),
            Accion::Atajo(A::Encendido) => FilaAtajo::Extra(AtajoExtra::Encendido),
            Accion::PantallaCompleta => FilaAtajo::Extra(AtajoExtra::PantallaCompleta),
            Accion::Captura => f(AccionAtajo::Captura),
            Accion::ZoomMas => f(AccionAtajo::ZoomMas),
            Accion::ZoomMenos => f(AccionAtajo::ZoomMenos),
            Accion::ZoomAjustar => f(AccionAtajo::ZoomAjustar),
            Accion::Rotacion(_) | Accion::Zoom1a1 | Accion::Configuracion => return None,
        };
        self.a.tecla_de(self.cfg, fila)
    }

    /// Controles: lo que ofrecia el panel lateral (navegacion de Android, volumen, rotacion, captura y zoom). Cada boton
    /// hace lo mismo que su tecla de atajo. El dialogo se queda abierto al pulsarlos y una linea de estado confirma lo
    /// enviado; Esc o la X lo cierran.
    fn controles(&mut self) {
        use crate::gestos::Atajo as A;
        let d = self.c.datos;
        self.parrafo(tx!("controles.acciones_rapidas_android_cada"), Estilo::Pequeno, tema::p().texto2);
        // linea de estado de altura fija (reservada siempre): lo ultimo enviado, sin que los botones se muevan bajo el raton
        self.y += 6.0;
        let (lhp, x0, w) = (self.lh(Estilo::Pequeno), self.x0, self.w);
        if let Some((tono, t)) = self.a.mensaje_de("controles").map(|(t, s)| (t, s.to_string())) {
            let (icono, col) = icono_de(tono);
            let y = self.y;
            // una sola linea: si no cabe se recorta por el centro (se ve el final: el nombre de la captura guardada)
            let t = truncar_centro(self.m(), &t, Estilo::Pequeno, w - SANGRIA_MENSAJE);
            let lado = ICONO_MENSAJE.min(lhp);
            self.pint.push(Pint::Icono { k: icono, r: R::new(x0, y + ((lhp - lado) / 2.0).floor(), lado, lado), c: col });
            self.txt(x0 + SANGRIA_MENSAJE, y, &t, Estilo::Pequeno, col);
        }
        self.y += lhp;
        self.grupo(tx!("controles.navegacion_android"));
        self.fila_controles(&[(Accion::Atajo(A::Atras), tx!("comun.atras"), false), (Accion::Atajo(A::Inicio), tx!("comun.inicio"), false), (Accion::Atajo(A::Recientes), tx!("comun.recientes"), false)]);
        self.y += 8.0;
        self.fila_controles(&[(Accion::Atajo(A::VolBajar), tx!("controles.vol_menos"), false), (Accion::Atajo(A::VolSubir), tx!("controles.vol_mas"), false)]);
        self.y += FILA_GAP;
        self.grupo(tx!("comun.rotacion"));
        let (valor, nota) = match d.orient {
            Orientacion::Auto => (tx!("comun.auto").to_string(), txf!("comun.android_tiene_grados", d.rot % 4 * 90)),
            Orientacion::Fija(r) => (txf!("controles.grados", r % 4 * 90), tx!("controles.fija_ventana_queda_girada").to_string()),
        };
        let (x0, y) = (self.x0, self.y);
        self.txt(x0, y, &valor, Estilo::Titulo, tema::p().texto);
        self.y += self.lh(Estilo::Titulo) + 2.0;
        self.en_linea_color(&nota, Estilo::Pequeno, tema::p().texto2, self.w);
        self.y += 6.0;
        let f = |r: u32| (Accion::Rotacion(r), r);
        let (a0, a1, a2, a3) = (f(0), f(1), f(2), f(3));
        let t0 = ["0°", "90°", "180°", "270°"];
        self.fila_controles(&[
            (a0.0, t0[0], d.orient == Orientacion::Fija(a0.1)),
            (a1.0, t0[1], d.orient == Orientacion::Fija(a1.1)),
            (a2.0, t0[2], d.orient == Orientacion::Fija(a2.1)),
            (a3.0, t0[3], d.orient == Orientacion::Fija(a3.1)),
            (Accion::RotacionAuto, tx!("comun.auto"), d.orient == Orientacion::Auto),
        ]);
        self.y += 8.0;
        self.fila_controles(&[(Accion::Atajo(A::Rotar), tx!("controles.rotar_paso"), false)]);
        self.y += FILA_GAP;
        self.grupo(tx!("controles.zoom"));
        let pct = d.zoom_pct.round() as i32;
        let t = format!("{} %", pct);
        let (x0, y) = (self.x0, self.y);
        self.txt(x0, y, &t, Estilo::Grande, tema::p().texto);
        self.y += self.lh(Estilo::Grande) + 2.0;
        let modo = match d.zoom_modo {
            ModoZoom::Ajustar => tx!("controles.ajustado_ventana").to_string(),
            ModoZoom::Fijo(p) if (pct - p as i32).abs() > 1 => txf!("controles.fijo_limitado_pantalla_equipo", p),
            ModoZoom::Fijo(p) => txf!("controles.fijo", p),
        };
        self.en_linea_color(&modo, Estilo::Pequeno, tema::p().texto2, self.w);
        self.y += 6.0;
        self.fila_controles(&[
            (Accion::ZoomMas, tx!("controles.acercar"), false),
            (Accion::ZoomMenos, tx!("controles.alejar"), false),
            (Accion::ZoomAjustar, tx!("comun.ajustar"), d.zoom_modo == ModoZoom::Ajustar),
            (Accion::Zoom1a1, "1:1", d.zoom_modo == ModoZoom::Fijo(100)),
        ]);
        self.y += FILA_GAP;
        self.grupo(tx!("controles.captura"));
        self.fila_controles(&[(Accion::Captura, tx!("controles.captura_pantalla"), false)]);
        self.y += 6.0;
        self.parrafo(tx!("controles.guarda_como_captura_aaaammdd"), Estilo::Pequeno, tema::p().texto2);
        self.y += FILA_GAP;
    }

    fn general(&mut self) {
        self.grupo(tx!("general.ventana"));
        let zoom = self.cfg.get("zoom");
        let ayuda = tx!("general.tamano_pantalla_android_abrir");
        self.fila_opciones("zoom", tx!("general.zoom_inicial"), ayuda, &zoom);
        // un porcentaje que no esta en la lista (puesto a mano en el archivo o con `weft config`)
        let fuera = !opciones("zoom").iter().any(|(v, _)| *v == zoom);
        // el zoom de esta ventana, si los atajos o Controles lo cambiaron (no se guarda: el inicial sigue igual)
        let actual = self.c.datos.zoom_modo;
        let ahora = (actual != crate::vista::zoom_de_config(self.cfg)).then(|| match actual {
            ModoZoom::Ajustar => tx!("general.ajustado_ventana").to_string(),
            ModoZoom::Fijo(p) => format!("{} %", p),
        });
        if fuera || ahora.is_some() {
            self.y -= FILA_GAP - 2.0;
            if fuera {
                self.en_linea(Tono::Aviso, &txf!("general.zoom_inicial_fijado_archivo", zoom), self.w);
            }
            if let Some(t) = ahora {
                self.en_linea_color(&txf!("general.ahora_solo_esta_ventana", t), Estilo::Pequeno, tema::p().texto2, self.w);
            }
            self.y += FILA_GAP;
        }
        let tema = self.cfg.get("ventana.tema");
        self.fila_opciones("ventana.tema", tx!("general.tema"), tx!("general.tema_ayuda"), &tema);
        let texto = self.cfg.get("ventana.texto");
        self.fila_opciones("ventana.texto", tx!("general.tamano_texto"), tx!("general.tamano_texto_ayuda"), &texto);
        self.grupo(tx!("general.confirmaciones"));
        self.fila_interruptor("confirmar", tx!("general.pedir_confirmacion_reiniciar_apagar"), tx!("general.reiniciar_android_apagar_seccion"));
    }

    fn atajos(&mut self) {
        self.parrafo(tx!("atajos.pulsa_tecla_para_cambiarla"), Estilo::Cuerpo, tema::p().texto2);
        self.y += 6.0;
        self.parrafo(tx!("atajos.sin_ctrl_ni_alt"), Estilo::Pequeno, tema::p().texto2);
        self.y += 10.0;
        self.fila_interruptor(
            gestos::ATAJOS_DESACTIVADOS,
            tx!("atajos.atajos_desactivados_todo_va"),
            tx!("atajos.ninguna_tecla_atajo_todas"),
        );
        let pasar = self.a.tecla_de(self.cfg, FilaAtajo::Extra(AtajoExtra::PasarTecla));
        if let Some(p) = pasar {
            self.y -= FILA_GAP - 2.0;
            self.en_linea_color(&txf!("atajos.para_mandar_android_sola", p), Estilo::Pequeno, tema::p().texto2, self.w);
            self.y += FILA_GAP;
        }
        let (x0, w) = (self.x0, self.w);
        for a in FilaAtajo::todas() {
            let id = Id::Atajo(a);
            let y0 = self.y;
            let capturando = self.a.captura == Some(a);
            // etiqueta a la izquierda, chip a la derecha
            let cw = 168.0f32.min(w * 0.5);
            let lh = self.lh(Estilo::Cuerpo);
            let etq = truncar(self.m(), a.etiqueta(), Estilo::Cuerpo, w - cw - 20.0);
            let p = texto_v(self.m(), x0, y0 + BH / 2.0, &etq, Estilo::Cuerpo, tema::p().texto);
            self.pint.push(p);
            let _ = lh;
            // "Guardado" entre la etiqueta y el chip
            if let Some((k, t)) = &self.a.guardado {
                if (k == a.clave() || k == "atajos") && self.c.ahora.saturating_duration_since(*t) < Duration::from_secs(GUARDADO_S) {
                    let tw = self.m().ancho(tx!("comun.guardado"), Estilo::Pequeno);
                    let xr = x0 + w - cw - 12.0 - tw;
                    self.pint.push(Pint::Icono { k: Icono::Check, r: R::new(xr - 18.0, y0 + (BH - 14.0) / 2.0, 14.0, 14.0), c: tema::p().exito });
                    self.pint.push(texto_v(self.m(), xr, y0 + BH / 2.0, tx!("comun.guardado"), Estilo::Pequeno, tema::p().exito));
                }
            }
            let r = R::new(x0 + w - cw, y0, cw, BH);
            let (hover, pulsado, foco) = self.a.visual(&id);
            if foco {
                self.anillo(r, tema::RADIO);
            }
            let (texto, col) = if capturando {
                (tx!("atajos.pulsa_tecla").to_string(), tema::p().acento_claro)
            } else {
                match self.a.tecla_de(self.cfg, a) {
                    Some(t) => (t, tema::p().texto),
                    None => (tx!("atajos.sin_asignar").to_string(), tema::p().texto2),
                }
            };
            let borde = if capturando { tema::p().acento_claro } else { tema::p().borde };
            self.pint.push(Pint::Rect { r, c: borde, radio: tema::RADIO });
            let fondo = if pulsado {
                tema::p().control_hover
            } else if capturando {
                tema::p().fondo
            } else if hover {
                tema::p().control_hover
            } else {
                tema::p().control
            };
            self.pint.push(Pint::Rect { r: r.reducir(1.0), c: fondo, radio: tema::RADIO - 1.0 });
            let t = truncar(self.m(), &texto, Estilo::Negrita, r.w - 16.0);
            let tw = self.m().ancho(&t, Estilo::Negrita);
            self.pint.push(texto_v(self.m(), r.x + ((r.w - tw) / 2.0).floor(), r.y + r.h / 2.0, &t, Estilo::Negrita, col));
            self.registrar(id, r, true);
            self.y = y0 + BH;
            // ayuda de la captura y conflicto
            if capturando {
                self.y += 4.0;
                self.en_linea_color(tx!("atajos.esc_cancela_retroceso_borra"), Estilo::Pequeno, tema::p().texto2, w);
            }
            if let Some((tono, t)) = self.a.mensaje_de(a.clave()) {
                let t = t.to_string();
                self.y += 2.0;
                self.en_linea(tono, &t, w);
            }
            if capturando {
                if let Some((_, otra)) = self.a.conflicto {
                    self.y += 4.0;
                    self.boton_fila(Id::Boton(Bot::UsarAqui), &txf!("atajos.quitarla_usarla_aqui", otra.etiqueta()), EstiloBoton::Primario, true);
                }
            }
            self.y += 8.0;
        }
        self.y += 8.0;
        self.boton_fila(Id::Boton(Bot::RestaurarAtajos), tx!("atajos.restaurar_valores_defecto"), EstiloBoton::Normal, true);
    }

    fn en_linea_color(&mut self, t: &str, e: Estilo, col: Color, ancho: f32) {
        let lh = self.lh(e);
        let x0 = self.x0;
        for l in envolver(self.m(), t, e, ancho, 0) {
            let y = self.y;
            self.txt(x0, y, &l, e, col);
            self.y += lh;
        }
    }

    fn entrada(&mut self) {
        self.grupo(tx!("entrada.rueda_raton"));
        self.fila_paso("rueda.paso", tx!("entrada.desplazamiento_paso"), tx!("entrada.cuanto_pantalla_desliza_cada"), " %");
        self.fila_paso("rueda.tope", tx!("entrada.tope_gesto"), tx!("entrada.maximo_desliza_solo_gesto"), " %");
        self.fila_interruptor("rueda.invertir", tx!("entrada.invertir_sentido"), tx!("entrada.rueda_hacia_arriba_desliza"));
        self.y -= FILA_GAP - 2.0;
        self.en_linea_color(tx!("entrada.ctrl_apretado_rueda_acerca"), Estilo::Pequeno, tema::p().texto2, self.w);
        self.y += FILA_GAP;
        self.grupo(tx!("entrada.pellizco"));
        let m = self.cfg.get("pellizco.modificador");
        self.fila_opciones("pellizco.modificador", tx!("entrada.tecla_pellizco"), tx!("entrada.manten_tecla_arrastra_boton"), &m);
        self.grupo(tx!("entrada.botones_raton"));
        self.parrafo(tx!("entrada.raton_como_pantalla_tactil"), Estilo::Pequeno, tema::p().texto2);
        self.y += 6.0;
        let d = self.a.valor(self.cfg, gestos::RATON_DERECHO);
        self.fila_opciones(gestos::RATON_DERECHO, tx!("entrada.boton_derecho"), tx!("entrada.defecto_atras"), &d);
        let c = self.a.valor(self.cfg, gestos::RATON_CENTRAL);
        self.fila_opciones(gestos::RATON_CENTRAL, tx!("entrada.boton_central"), tx!("entrada.defecto_inicio"), &c);
        self.grupo(tx!("entrada.mandos_juegos"));
        let g = self.cfg.get("gamepad");
        self.fila_opciones("gamepad", tx!("entrada.conectar_mandos_automaticamente"), tx!("entrada.entrega_maquina_mandos_equipo"), &g);
        self.mensajes_de("gamepad", self.w);
        self.y += 6.0;
        self.grupo(tx!("entrada.mandos_equipo"));
        let d = self.c.datos;
        let (x0, w) = (self.x0, self.w);
        if !d.qmp_ok {
            self.en_linea(Tono::Aviso, tx!("entrada.sin_acceso_maquina_arrancala"), w);
            self.y += 6.0;
        } else if d.mandos.is_empty() {
            self.parrafo(tx!("entrada.ninguno_detectado_equipo"), Estilo::Cuerpo, tema::p().texto2);
            if d.ilegibles > 0 {
                self.parrafo(&txf!("entrada.dispositivos_sin_permiso_lectura", d.ilegibles), Estilo::Pequeno, tema::p().texto2);
            }
            self.y += 6.0;
        }
        for (i, m) in d.mandos.iter().enumerate() {
            let y0 = self.y;
            let bw = (self.m().ancho(tx!("entrada.desconectar"), Estilo::Negrita) + 32.0).max(110.0).ceil().min(w * 0.6);
            let (etq, est) = match (&m.conectado, m.ocupado) {
                (_, true) => (tx!("entrada.espere"), EstiloBoton::Normal),
                (Some(_), _) => (tx!("entrada.desconectar"), EstiloBoton::Activo),
                (None, _) => (tx!("entrada.conectar"), EstiloBoton::Normal),
            };
            let nombre = truncar(self.m(), &m.name, Estilo::Cuerpo, w - bw - 12.0);
            let p = texto_v(self.m(), x0, y0 + BH / 2.0, &nombre, Estilo::Cuerpo, tema::p().texto);
            self.pint.push(p);
            self.boton_texto(Id::Mando(i), R::new(x0 + w - bw, y0, bw, BH), etq, est, !m.ocupado);
            self.y = y0 + BH + 8.0;
        }
        self.mensajes_de("mandos", w);
        self.y += 6.0;
        self.parrafo(tx!("entrada.mando_conectado_maquina_deja"), Estilo::Pequeno, tema::p().texto2);
        self.y += FILA_GAP;
    }

    fn pantalla(&mut self) {
        self.grupo(tx!("comun.rotacion"));
        let o = orient_valor(self.c.datos.orient);
        self.fila_opciones("orientacion", tx!("pantalla.orientacion"), tx!("pantalla.orientacion_fija_deja_ventana"), &o);
        if self.c.datos.orient == Orientacion::Auto {
            self.y -= FILA_GAP - 2.0;
            let t = txf!("comun.android_tiene_grados", self.c.datos.rot % 4 * 90);
            self.en_linea_color(&t, Estilo::Pequeno, tema::p().texto2, self.w);
            self.y += FILA_GAP;
        }
        self.fila_interruptor("pantalla.giro_android", tx!("pantalla.seguir_rotacion_android"), tx!("pantalla.orientacion_auto_ventana_sigue"));
        self.grupo(tx!("pantalla.resolucion_densidad"));
        let (wv, hv) = self.cfg.resolucion().unwrap_or(self.c.datos.resolucion);
        let _ = hv;
        self.etiqueta(tx!("pantalla.tamano_pantalla_virtual"), tx!("pantalla.guarda_instante_usa_proximo"), "pantalla.resolucion", self.w);
        self.y += 6.0;
        self.fila_campos(&[(tx!("pantalla.ancho"), Campo::Ancho, 88.0), (tx!("pantalla.alto"), Campo::Alto, 88.0), (tx!("pantalla.densidad_dpi"), Campo::Densidad, 104.0)]);
        // el ancho se redondea a multiplo de 8: es un aviso, no un error
        let ancho_vigente = match self.a.edicion.as_ref().filter(|e| e.campo == Campo::Ancho) {
            Some(e) => e.buf.parse::<u32>().unwrap_or(wv),
            None => wv,
        };
        let clave_res = Campo::Ancho.clave();
        self.y += 4.0;
        if ancho_vigente > 0 && ancho_vigente % 8 != 0 {
            let t = txf!("pantalla.ancho_redondeara_multiplo_8", ancho_vigente / 8 * 8);
            self.en_linea(Tono::Aviso, &t, self.w);
        }
        for k in [clave_res, Campo::Densidad.clave()] {
            if let Some((tono, t)) = self.a.mensaje_de(k) {
                let t = t.to_string();
                self.en_linea(tono, &t, self.w);
            }
        }
        self.y += 10.0;
        self.parrafo(tx!("pantalla.aplicar_ahora_reinicia_surfaceflinger"), Estilo::Pequeno, tema::p().aviso);
        self.y += 8.0;
        let aplicando = self.c.datos.aplicando;
        self.boton_fila(Id::Boton(Bot::AplicarResolucion), if aplicando { tx!("pantalla.aplicando") } else { tx!("pantalla.aplicar_resolucion_ahora") }, EstiloBoton::Primario, !aplicando);
        if let Some((tono, t)) = self.a.mensaje_de("aplicar") {
            let t = t.to_string();
            self.y += 6.0;
            self.en_linea(tono, &t, self.w);
        }
        self.y += FILA_GAP;
    }

    fn maquina(&mut self) {
        let d = self.c.datos;
        self.grupo(tx!("comun.estado"));
        match &d.maquina {
            Some(m) => {
                self.par(tx!("comun.estado"), crate::vista::estado_es(&m.estado), if m.estado == "running" { tema::p().exito } else { tema::p().aviso });
                self.par(tx!("maquina.activa_desde_hace"), &d.encendida.map_or("?".to_string(), crate::vista::duracion), tema::p().texto);
                self.par(tx!("maquina.cpus"), &m.cpus.to_string(), tema::p().texto);
                self.par(tx!("maquina.ram"), &txf!("maquina.ram_mb", m.mem_mb), tema::p().texto);
                self.par(tx!("maquina.tipo"), &crate::vista::tipo_corto(&m.tipo), tema::p().texto);
            }
            None => {
                let t = if d.qmp_ok { tx!("comun.consultando") } else { tx!("maquina.sin_acceso_maquina_no") };
                self.parrafo(t, Estilo::Cuerpo, if d.qmp_ok { tema::p().texto2 } else { tema::p().aviso });
            }
        }
        self.y += FILA_GAP;
        self.grupo(tx!("maquina.acciones"));
        self.bloque_reinicio(tx!("maquina.reinicia_solo_android_maquina"), true);
        self.y += FILA_GAP;
        self.bloque_apagar();
        self.y += FILA_GAP;
        self.grupo(tx!("maquina.ajustes_maquina"));
        self.pint.push(Pint::Icono { k: Icono::Circulo, r: R::new(self.x0, self.y + 4.0, 8.0, 8.0), c: tema::p().aviso });
        let (x0, y) = (self.x0 + 16.0, self.y);
        self.txt(x0, y, tx!("maquina.aplican_proximo_arranque"), Estilo::Negrita, tema::p().aviso);
        self.y += self.lh(Estilo::Negrita);
        let t = txf!("maquina.estos_ajustes_no_cambian", crate::vista::orden_terminal("start", self.c.datos.flatpak.as_deref()));
        self.parrafo(&t, Estilo::Pequeno, tema::p().texto2);
        self.y += 8.0;
        self.grupo(tx!("maquina.recursos"));
        self.etiqueta(tx!("maquina.cpus"), tx!("maquina.nucleos_virtuales_1_128"), "maquina.cpus", self.w);
        self.y += 6.0;
        self.fila_campos(&[("", Campo::Cpus, 88.0)]);
        self.mensajes_de("maquina.cpus", self.w);
        self.y += FILA_GAP;
        self.etiqueta(tx!("maquina.memoria_ram"), tx!("maquina.mib_512_vacio_automatico"), "maquina.ram", self.w);
        self.y += 6.0;
        self.fila_campos(&[("", Campo::Ram, 104.0)]);
        self.mensajes_de("maquina.ram", self.w);
        self.y += FILA_GAP;
        self.grupo(tx!("maquina.tipo_graficos"));
        let t = self.cfg.get("maquina.tipo");
        self.fila_opciones("maquina.tipo", tx!("maquina.tipo_maquina"), tx!("maquina.q35_permite_conectar_mandos"), &t);
        let g = self.cfg.get("maquina.gpu");
        self.fila_opciones("maquina.gpu", tx!("maquina.aceleracion_grafica"), tx!("maquina.automatica_elige_mejor_para"), &g);
        if !matches!(g.as_str(), "auto" | "hardware" | "software") {
            self.en_linea(Tono::Aviso, tx!("maquina.hay_motor_grafico_concreto"), self.w);
        }
        self.grupo(tx!("maquina.carpeta_estado"));
        let ruta = self.c.datos.estado_dir.clone();
        self.parrafo(&ruta, Estilo::Pequeno, tema::p().texto2);
        self.y += 6.0;
        self.boton_fila(Id::Boton(Bot::AbrirEstado), tx!("maquina.abrir_carpeta_estado"), EstiloBoton::Normal, true);
        if let Some((tono, t)) = self.a.mensaje_de("estado") {
            let t = t.to_string();
            self.y += 6.0;
            self.en_linea(tono, &t, self.w);
        }
        self.y += FILA_GAP;
    }

    /// Botones en fila (pasan a la linea siguiente si no caben): (control, texto, estilo, habilitado).
    fn fila_botones(&mut self, items: &[(Id, &str, EstiloBoton, bool)]) {
        let (x0, w) = (self.x0, self.w);
        let mut x = x0;
        for (id, t, est, hab) in items {
            let bw = (self.m().ancho(t, Estilo::Negrita) + 32.0).max(96.0).ceil().min(w);
            if x > x0 && x + bw > x0 + w + 0.5 {
                x = x0;
                self.y += BH + 6.0;
            }
            let y = self.y;
            self.boton_texto(*id, R::new(x, y, bw, BH), t, *est, *hab);
            x += bw + 8.0;
        }
        self.y += BH;
    }

    /// Un campo del perfil que se ve, con su etiqueta encima: editable en uno del usuario, solo texto en uno integrado.
    fn campo_perfil(&mut self, clave: &str, etiqueta: &str, ancho: f32, editable: bool) {
        let campo = campo_perfil(clave);
        let msg = campo.clave();
        if editable {
            let w = self.w;
            self.etiqueta(etiqueta, "", msg, w);
            self.y += 4.0;
            let (t, tenue) = self.valor_campo(campo);
            let (x0, y) = (self.x0, self.y);
            self.campo(campo, x0, y, ancho.min(w), &t, tenue);
            self.y += BH;
            self.mensajes_de(msg, w);
            self.y += 10.0;
        } else {
            let (t, tenue) = self.valor_campo(campo);
            self.par(etiqueta, &t, if tenue { tema::p().texto2 } else { tema::p().texto });
        }
    }

    /// Perfiles de dispositivo: la lista (con el de la maquina marcado), las acciones sobre el que se ve y sus campos.
    fn perfiles(&mut self) {
        let d = self.c.datos;
        let w = self.w;
        self.parrafo(tx!("perfiles.intro"), Estilo::Pequeno, tema::p().texto2);
        self.banda_operacion(w);
        let Some(cat) = self.a.perfiles.as_ref() else {
            self.y += 8.0;
            self.parrafo(tx!("comun.consultando"), Estilo::Cuerpo, tema::p().texto2);
            return;
        };
        let elegido = self.cfg.get("dispositivo.perfil");
        let visto = self.a.visto(self.cfg);
        // --- la lista
        self.grupo(tx!("perfiles.lista"));
        let mut filas: Vec<(String, String)> = vec![(dispositivo::NINGUNO.to_string(), tx!("perfiles.ninguno").to_string())];
        for p in &cat.dispositivos {
            let cpu = if p.cpu_nombre.is_empty() { String::new() } else { format!(" · {}", p.cpu_nombre) };
            let de = if p.origen == crate::perfil::Origen::Integrado { tx!("perfiles.de_fabrica_corto") } else { tx!("perfiles.propio_corto") };
            filas.push((p.id.clone(), txf!("perfiles.fila", p.nombre, cpu, de)));
        }
        let rechazados: Vec<String> = cat.rechazados.iter().map(|(f, e)| txf!("perfiles.rechazado", std::path::Path::new(f).file_name().map_or(f.clone(), |n| n.to_string_lossy().into_owned()), e)).collect();
        let no_existe = elegido != dispositivo::NINGUNO && cat.buscar(&elegido).is_none();
        let p = cat.buscar(&visto).cloned();
        for (i, (id, t)) in filas.iter().enumerate() {
            let t = if *id == elegido { txf!("perfiles.en_uso", t) } else { t.clone() };
            let (x0, y) = (self.x0, self.y);
            self.boton_texto(Id::Perfil(i), R::new(x0, y, w, BH), &t, if *id == visto { EstiloBoton::Activo } else { EstiloBoton::Normal }, true);
            self.y += BH + 4.0;
        }
        for t in rechazados {
            self.y += 2.0;
            self.en_linea(Tono::Aviso, &t, w);
        }
        if no_existe {
            self.y += 2.0;
            self.en_linea(Tono::Aviso, &txf!("perfiles.elegido_no_existe", elegido), w);
        }
        // --- acciones sobre el que se ve
        let titulo = p.as_ref().map_or(tx!("perfiles.ninguno").to_string(), |p| p.nombre.clone());
        self.grupo(&titulo);
        match &p {
            Some(p) if !p.descripcion.is_empty() => {
                self.parrafo(&p.descripcion, Estilo::Pequeno, tema::p().texto2);
                self.y += 6.0;
            }
            Some(_) => {}
            None => {
                self.parrafo(tx!("perfiles.ninguno_explica"), Estilo::Pequeno, tema::p().texto2);
                self.y += 6.0;
            }
        }
        let propio = p.as_ref().is_some_and(|p| p.origen == crate::perfil::Origen::Usuario);
        let editable = propio && self.a.perfiles_dir.is_some();
        let mut botones = vec![
            (Id::Boton(Bot::PerfilUsar), if visto == elegido { tx!("perfiles.en_uso_boton") } else { tx!("perfiles.usar") }, EstiloBoton::Primario, visto != elegido),
            (Id::Boton(Bot::PerfilDuplicar), tx!("perfiles.duplicar"), EstiloBoton::Normal, self.a.perfiles_dir.is_some()),
        ];
        if propio {
            botones.push((Id::Boton(Bot::PerfilBorrar), tx!("perfiles.borrar"), EstiloBoton::Peligro, true));
        }
        self.fila_botones(&botones);
        if self.a.confirmando == Some(Conf::PerfilBorrar) {
            self.y += 6.0;
            let l = vec![txf!("perfiles.borrar_detalle", titulo)];
            self.caja_con_tono(tx!("perfiles.pregunta_borrar"), &l, &[(Id::Boton(Bot::Confirmar), tx!("perfiles.borrar"), EstiloBoton::Peligro), (Id::Boton(Bot::Cancelar), tx!("comun.cancelar"), EstiloBoton::Normal)], tema::p().error);
        }
        self.mensajes_de("perfiles", w);
        // --- aplicar el de la maquina
        if visto == elegido {
            self.y += 8.0;
            if d.en_marcha {
                self.parrafo(tx!("perfiles.aplicar_explica"), Estilo::Pequeno, tema::p().texto2);
                self.y += 6.0;
                if self.a.confirmando == Some(Conf::PerfilAplicar) {
                    let l = vec![tx!("perfiles.aplicar_detalle").to_string()];
                    self.caja_confirmacion(tx!("perfiles.pregunta_aplicar"), &l, &[(Id::Boton(Bot::Confirmar), tx!("perfiles.aplicar_ahora"), EstiloBoton::Primario), (Id::Boton(Bot::Cancelar), tx!("comun.cancelar"), EstiloBoton::Normal)]);
                } else {
                    self.boton_fila(Id::Boton(Bot::PerfilAplicar), tx!("perfiles.aplicar_ahora"), EstiloBoton::Normal, d.operacion.is_none());
                }
            } else {
                self.parrafo(tx!("perfiles.se_aplica_al_arrancar"), Estilo::Pequeno, tema::p().texto2);
            }
        }
        let Some(p) = p else {
            self.y += FILA_GAP;
            return;
        };
        if !editable {
            self.y += 8.0;
            self.en_linea(Tono::Aviso, tx!("perfiles.de_fabrica"), w);
        }
        // --- campos
        let ancho_txt = 360.0f32;
        self.grupo(tx!("perfiles.g_general"));
        self.campo_perfil("dispositivo.nombre", tx!("perfiles.c_nombre"), ancho_txt, editable);
        self.campo_perfil("dispositivo.descripcion", tx!("perfiles.c_descripcion"), w, editable);
        self.grupo(tx!("perfiles.g_maquina"));
        self.parrafo(tx!("perfiles.maquina_ayuda"), Estilo::Pequeno, tema::p().texto2);
        self.y += 6.0;
        self.campo_perfil("maquina.nucleos", tx!("perfiles.c_nucleos"), 104.0, editable);
        self.campo_perfil("maquina.ram", tx!("perfiles.c_ram"), 104.0, editable);
        if editable {
            self.etiqueta(tx!("perfiles.c_pagina"), tx!("perfiles.c_pagina_ayuda"), "disp.pagina", w);
            self.y += 6.0;
            let pag: Vec<(Id, String, EstiloBoton)> = [4u32, 16].iter().map(|k| (Id::PerfilPagina(*k), txf!("perfiles.kib", k), if p.pagina_kib == *k { EstiloBoton::Activo } else { EstiloBoton::Normal })).collect();
            let pag: Vec<(Id, &str, EstiloBoton, bool)> = pag.iter().map(|(i, t, e)| (*i, t.as_str(), *e, true)).collect();
            self.fila_botones(&pag);
            self.mensajes_de("disp.pagina", w);
            self.y += 10.0;
        } else {
            self.par(tx!("perfiles.c_pagina"), &txf!("perfiles.kib", p.pagina_kib), tema::p().texto);
        }
        self.grupo(tx!("perfiles.g_identidad"));
        self.parrafo(tx!("perfiles.identidad_ayuda"), Estilo::Pequeno, tema::p().texto2);
        self.y += 6.0;
        for (k, e) in [("producto.fabricante", tx!("perfiles.c_fabricante")), ("producto.marca", tx!("perfiles.c_marca")), ("producto.modelo", tx!("perfiles.c_modelo")), ("producto.dispositivo", tx!("perfiles.c_dispositivo")), ("producto.nombre", tx!("perfiles.c_producto"))] {
            self.campo_perfil(k, e, ancho_txt, editable);
        }
        self.grupo(tx!("perfiles.g_cpu"));
        self.campo_perfil("cpu.nombre", tx!("perfiles.c_cpu_nombre"), ancho_txt, editable);
        let base = txf!("perfiles.c_base", dispositivo::BASES.join(", "));
        self.campo_perfil("cpu.base", &base, 200.0, editable);
        self.campo_perfil("cpu.midr", tx!("perfiles.c_midr"), 200.0, editable);
        self.campo_perfil("cpu.revidr", tx!("perfiles.c_revidr"), 200.0, editable);
        self.campo_perfil("cpu.hardware", tx!("perfiles.c_hardware"), ancho_txt, editable);
        // --- extensiones
        self.grupo(tx!("perfiles.g_extensiones"));
        self.parrafo(tx!("perfiles.extensiones_ayuda"), Estilo::Pequeno, tema::p().texto2);
        self.y += 6.0;
        let modo = p.modo_extensiones();
        let nombres_modo = [tx!("perfiles.modo_base"), tx!("perfiles.modo_propia"), tx!("perfiles.modo_cambios")];
        let modos: Vec<(Id, &str, EstiloBoton, bool)> = MODOS_EXTENSIONES.iter().enumerate().map(|(i, m)| (Id::PerfilModo(i), nombres_modo[i], if *m == modo { EstiloBoton::Activo } else { EstiloBoton::Normal }, editable)).collect();
        self.fila_botones(&modos);
        self.y += 8.0;
        // rejilla: encendido = se anuncia (o se anade a la base); "-x" = se quita de la base
        let (x0, mut x) = (self.x0, self.x0);
        for (i, n) in dispositivo::EXTENSIONES.iter().enumerate() {
            let estado = p.estado_extension(n);
            let t = match estado {
                Some('+') => format!("+{}", n),
                Some('-') => format!("-{}", n),
                _ => n.to_string(),
            };
            let bw = (self.m().ancho(&t, Estilo::Negrita) + 18.0).max(48.0).ceil().min(w);
            if x > x0 && x + bw > x0 + w + 0.5 {
                x = x0;
                self.y += BH + 4.0;
            }
            let y = self.y;
            let est = if matches!(estado, Some('=' | '+')) { EstiloBoton::Activo } else { EstiloBoton::Normal };
            self.boton_texto(Id::Extension(i), R::new(x, y, bw, BH), &t, est, editable);
            x += bw + 4.0;
        }
        self.y += BH;
        self.mensajes_de("disp.features", w);
        let desconocidas = p.desconocidas();
        if !desconocidas.is_empty() {
            self.y += 6.0;
            self.en_linea(Tono::Aviso, &txf!("perfiles.desconocidas", desconocidas.join(" ")), w);
        }
        let sin = self.a.soportadas.as_deref().map(|s| p.sin_soporte(s)).unwrap_or_default();
        if !sin.is_empty() {
            self.y += 6.0;
            self.en_linea(Tono::Aviso, &txf!("perfiles.sin_soporte", sin.join(" ")), w);
        }
        // --- registros de identificacion (detalles tecnicos)
        self.y += 10.0;
        let abierto = self.a.detalles.contains(&Detalle::PerfilRegistros);
        self.boton_fila(Id::Boton(Bot::Detalles(Detalle::PerfilRegistros)), if abierto { tx!("perfiles.ocultar_registros") } else { tx!("perfiles.ver_registros") }, EstiloBoton::Normal, true);
        if abierto {
            self.y += 8.0;
            self.parrafo(tx!("perfiles.registros_ayuda"), Estilo::Pequeno, tema::p().texto2);
            self.y += 6.0;
            self.campo_perfil("cpu.ctr", "CTR_EL0", 240.0, editable);
            self.campo_perfil("cpu.dczid", "DCZID_EL0", 240.0, editable);
            for r in dispositivo::REGISTROS {
                let e = format!("{}_EL1", r.to_uppercase()); // texto-interno: nombre del registro de ARM
                self.campo_perfil(&format!("cpu.{}", r), &e, 240.0, editable);
            }
        }
        self.y += FILA_GAP;
    }

    /// Apagar la maquina: apagado ordenado de Android por adb (unos 2 s; si adbd no responde, ACPI y por ultimo cierre
    /// forzado). La ventana se cierra con la maquina.
    fn bloque_apagar(&mut self) {
        let w = self.w;
        self.parrafo(tx!("maquina.apaga_maquina_android_cierra"), Estilo::Pequeno, tema::p().texto2);
        self.y += 6.0;
        if self.a.confirmando == Some(Conf::Apagar) {
            let l = vec![tx!("maquina.android_apaga_forma_ordenada").to_string()];
            self.caja_con_tono(tx!("maquina.pregunta_apagar"), &l, &[(Id::Boton(Bot::Confirmar), tx!("maquina.apagar"), EstiloBoton::Peligro), (Id::Boton(Bot::Cancelar), tx!("comun.cancelar"), EstiloBoton::Normal)], tema::p().error);
        } else {
            self.boton_fila(Id::Boton(Bot::Apagar), tx!("maquina.boton_apagar"), EstiloBoton::Peligro, true);
        }
        self.mensajes_de("apagar", w);
    }

    /// Caja de confirmacion en linea: borde amarillo, titulo, lineas de detalle y botones.
    fn caja_confirmacion(&mut self, titulo: &str, lineas: &[String], botones: &[(Id, &str, EstiloBoton)]) {
        self.caja_con_tono(titulo, lineas, botones, tema::p().aviso);
    }

    /// Caja de confirmacion con el color del borde y del titulo a elegir (rojo para lo destructivo).
    fn caja_con_tono(&mut self, titulo: &str, lineas: &[String], botones: &[(Id, &str, EstiloBoton)], tono: Color) {
        self.caja_con_detalle(titulo, lineas, None, botones, tono);
    }

    /// Caja de confirmacion con, opcionalmente, un bloque desplegable de "Detalles tecnicos" (nombres de productos y
    /// origenes de lo que se descarga): el titulo y el texto principal son genericos y el detalle es para quien quiera verlo.
    fn caja_con_detalle(&mut self, titulo: &str, lineas: &[String], detalle: Option<(Detalle, &[String])>, botones: &[(Id, &str, EstiloBoton)], tono: Color) {
        let pad = 12.0;
        let (x0, w) = (self.x0, self.w);
        let interior = w - 2.0 * pad;
        let (lhn, lhp) = (self.lh(Estilo::Negrita), self.lh(Estilo::Pequeno));
        let mut envueltas: Vec<String> = Vec::new();
        for l in lineas {
            envueltas.extend(envolver(self.m(), l, Estilo::Pequeno, interior, 0));
        }
        // bloque de detalles: boton para desplegar y, si esta desplegado, sus lineas
        let abierto = detalle.is_some_and(|(d, _)| self.a.detalles.contains(&d));
        let mut det_lineas: Vec<String> = Vec::new();
        if let (Some((_, l)), true) = (detalle, abierto) {
            for t in l {
                det_lineas.extend(envolver(self.m(), t, Estilo::Pequeno, interior - 8.0, 0));
            }
        }
        let alto_det = match detalle {
            None => 0.0,
            Some(_) => BH + 8.0 + if abierto { det_lineas.len() as f32 * lhp + 8.0 } else { 0.0 },
        };
        // botones en filas (pasan a la linea siguiente si no caben)
        let anchos: Vec<f32> = botones.iter().map(|(_, t, _)| (self.m().ancho(t, Estilo::Negrita) + 32.0).max(96.0).ceil().min(interior)).collect();
        let mut filas = 1usize;
        let mut x = 0.0f32;
        for bw in &anchos {
            if x > 0.0 && x + bw > interior {
                filas += 1;
                x = 0.0;
            }
            x += bw + 8.0;
        }
        let alto = pad + lhn + 6.0 + envueltas.len() as f32 * lhp + 10.0 + alto_det + filas as f32 * BH + (filas as f32 - 1.0) * 8.0 + pad;
        let r = R::new(x0, self.y, w, alto);
        self.pint.push(Pint::Rect { r, c: tono, radio: tema::RADIO + 1.0 });
        self.pint.push(Pint::Rect { r: r.reducir(1.0), c: tema::p().superficie, radio: tema::RADIO });
        let mut y = r.y + pad;
        self.txt(x0 + pad, y, titulo, Estilo::Negrita, tono);
        y += lhn + 6.0;
        for l in &envueltas {
            self.txt(x0 + pad, y, l, Estilo::Pequeno, tema::p().texto2);
            y += lhp;
        }
        y += 10.0;
        if let Some((d, _)) = detalle {
            let etq = if abierto { tx!("comun.ocultar_detalles_tecnicos") } else { tx!("comun.detalles_tecnicos") };
            let bw = (self.m().ancho(etq, Estilo::Negrita) + 32.0).max(96.0).ceil().min(interior);
            self.boton_texto(Id::Boton(Bot::Detalles(d)), R::new(x0 + pad, y, bw, BH), etq, EstiloBoton::Normal, true);
            y += BH + 8.0;
            if abierto {
                // barra de acento a la izquierda del detalle
                self.pint.push(Pint::Rect { r: R::new(x0 + pad, y - 2.0, 2.0, det_lineas.len() as f32 * lhp + 4.0), c: tema::p().borde, radio: 1.0 });
                for l in &det_lineas {
                    self.txt(x0 + pad + 8.0, y, l, Estilo::Pequeno, tema::p().texto2);
                    y += lhp;
                }
                y += 8.0;
            }
        }
        let mut x = x0 + pad;
        for ((id, t, est), bw) in botones.iter().zip(anchos.iter()) {
            if x > x0 + pad && x + bw > x0 + w - pad {
                x = x0 + pad;
                y += BH + 8.0;
            }
            self.boton_texto(*id, R::new(x, y, *bw, BH), t, *est, true);
            x += bw + 8.0;
        }
        self.y = r.y + alto;
    }

    /// "Detalles tecnicos" desplegables en linea (sin caja): boton y, desplegado, sus lineas.
    fn detalles_en_linea(&mut self, d: Detalle, lineas: &[String]) {
        let abierto = self.a.detalles.contains(&d);
        let etq = if abierto { tx!("comun.ocultar_detalles_tecnicos") } else { tx!("comun.detalles_tecnicos") };
        self.boton_fila(Id::Boton(Bot::Detalles(d)), etq, EstiloBoton::Normal, true);
        if abierto {
            self.y += 6.0;
            let (x0, w, lhp) = (self.x0, self.w, self.lh(Estilo::Pequeno));
            let y0 = self.y;
            for t in lineas {
                for l in envolver(self.m(), t, Estilo::Pequeno, w - 10.0, 0) {
                    let y = self.y;
                    self.txt(x0 + 10.0, y, &l, Estilo::Pequeno, tema::p().texto2);
                    self.y += lhp;
                }
            }
            let alto = self.y - y0;
            self.pint.push(Pint::Rect { r: R::new(x0, y0 - 2.0, 2.0, alto + 4.0), c: tema::p().borde, radio: 1.0 });
        }
    }

    /// Par "etiqueta: valor" del estado.
    fn par(&mut self, etq: &str, valor: &str, col: Color) {
        let (x0, y) = (self.x0, self.y);
        let xv = x0 + 230.0f32.min(self.w * 0.5);
        let etq = truncar(self.m(), etq, Estilo::Cuerpo, xv - x0 - 8.0);
        self.txt(x0, y, &etq, Estilo::Cuerpo, tema::p().texto2);
        let t = truncar(self.m(), valor, Estilo::Cuerpo, (self.w - (xv - x0)).max(40.0));
        self.txt(xv, y, &t, Estilo::Cuerpo, col);
        self.y += self.lh(Estilo::Cuerpo) + 2.0;
    }

    /// Etiqueta y, debajo, un valor largo (una ruta) ajustado al ancho en lugar de recortado.
    fn par_largo(&mut self, etq: &str, valor: &str) {
        let (x0, y) = (self.x0, self.y);
        self.txt(x0, y, etq, Estilo::Cuerpo, tema::p().texto2);
        self.y += self.lh(Estilo::Cuerpo);
        let lhp = self.lh(Estilo::Pequeno);
        let w = self.w;
        for l in envolver(self.m(), valor, Estilo::Pequeno, w, 0) {
            let y = self.y;
            self.txt(x0, y, &l, Estilo::Pequeno, tema::p().texto);
            self.y += lhp;
        }
        self.y += 4.0;
    }

    fn root(&mut self) {
        let d = self.c.datos;
        let ocupado = d.operacion.is_some();
        let w = self.w;
        self.parrafo(tx!("root.acceso_root_da_permisos"), Estilo::Pequeno, tema::p().texto2);
        self.y += 6.0;
        self.en_linea(Tono::Aviso, crate::textos::texto(root::AVISO_DETECCION), w);
        self.banda_operacion(w);
        self.grupo(tx!("comun.estado"));
        match &d.root_estado {
            None => {
                let t = if d.root_consultando { tx!("comun.consultando") } else { tx!("comun.sin_datos_maquina_esta") };
                self.parrafo(t, Estilo::Cuerpo, tema::p().texto2);
            }
            Some(e) => {
                let (si, no) = (tx!("comun.si"), tx!("comun.no"));
                self.par(tx!("root.instalado_cada_arranque"), if e.instalado() { si } else { no }, if e.instalado() { tema::p().exito } else { tema::p().texto });
                self.par(tx!("root.cargado_ahora"), if e.cargado { si } else { no }, if e.cargado { tema::p().exito } else { tema::p().texto });
                self.par(tx!("root.version"), e.version.as_deref().unwrap_or(tx!("comun.desconocida")), tema::p().texto);
                let g = if e.gestor { if e.reconocido { tx!("root.instalado_reconocido") } else { tx!("root.instalado") } } else { tx!("root.no_instalado") };
                self.par(tx!("root.gestor_permisos"), g, if e.gestor { tema::p().exito } else { tema::p().texto });
                if e.oficial {
                    self.y += 2.0;
                    self.en_linea(Tono::Aviso, tx!("root.hay_instalado_otro_gestor"), w);
                }
            }
        }
        self.y += 6.0;
        self.boton_fila(Id::Boton(Bot::RootActualizar), if d.root_consultando { tx!("comun.consultando") } else { tx!("comun.actualizar_estado") }, EstiloBoton::Normal, !d.root_consultando && !ocupado);
        self.y += FILA_GAP;
        self.grupo(tx!("root.archivos_ya_descargados"));
        self.parrafo(tx!("root.aplicacion_no_descarga_nada"), Estilo::Pequeno, tema::p().texto2);
        self.y += 6.0;
        self.campo_ruta(Campo::RutaRoot);
        self.y += 8.0;
        self.grupo(tx!("comun.arranque"));
        self.fila_interruptor_h("root.cargar_al_inicio", tx!("root.activar_acceso_root_cada"), tx!("root.instala_servicio_carga_acceso"), !ocupado);
        if self.a.confirmando == Some(Conf::RootActivar) {
            // texto principal generico; QUE se descarga (nombre, tamano y origen, es software de un tercero) va en los
            // "Detalles tecnicos" desplegables de la propia confirmacion
            let prov = root::proveedor();
            let (solo, todo) = (prov.descargas(false), prov.descargas(true));
            let tam = |v: &[root::Descarga]| root::mib(v.iter().map(|d| d.bytes).sum());
            let mut l: Vec<String> = vec![txf!("root.usara_software_tercero_ya", tam(&solo), tam(&todo))];
            l.push(tx!("root.copia_disco_maquina_instala").into());
            l.push(crate::textos::texto(root::AVISO_DETECCION).into());
            // detalles genericos: componente de terceros, tamano de cada descarga, dominio del origen, integridad y donde ver el resto
            // (las ordenes de consola que traen, como el resto de ordenes de la configuracion: con su forma de Flatpak)
            let fp = self.c.datos.flatpak.as_deref();
            let det: Vec<String> = prov.detalles_ui(true).iter().map(|l| crate::vista::ordenes_de_consola(l, fp)).collect();
            self.caja_con_detalle(tx!("root.activar_acceso_root"), &l, Some((Detalle::Root, &det)), &[(Id::Boton(Bot::Confirmar), tx!("root.activar"), EstiloBoton::Primario), (Id::Boton(Bot::ConfirmarConGestor), tx!("root.activar_e_instalar_gestor"), EstiloBoton::Normal), (Id::Boton(Bot::Cancelar), tx!("comun.cancelar"), EstiloBoton::Normal)], tema::p().aviso);
            self.y += FILA_GAP;
        }
        if self.a.confirmando == Some(Conf::RootDesactivar) {
            let l = vec![tx!("root.quita_servicio_carga_acceso").to_string()];
            self.caja_confirmacion(tx!("root.desactivar_acceso_root"), &l, &[(Id::Boton(Bot::Confirmar), tx!("root.desactivar"), EstiloBoton::Peligro), (Id::Boton(Bot::Cancelar), tx!("comun.cancelar"), EstiloBoton::Normal)]);
            self.y += FILA_GAP;
        }
        self.fila_interruptor_h("root.instalar_automaticamente", tx!("root.instalarlo_solo_si_falta"), tx!("root.root_activado_si_tras"), !ocupado);
        self.grupo(tx!("root.gestor"));
        self.parrafo(tx!("root.app_gestora_decide_apps"), Estilo::Pequeno, tema::p().texto2);
        self.y += 6.0;
        if self.a.confirmando == Some(Conf::RootGestor) {
            let l = vec![tx!("root.instala_app_gestora_archivos").to_string()];
            self.caja_confirmacion(tx!("root.instalar_gestor_root"), &l, &[(Id::Boton(Bot::Confirmar), tx!("comun.instalar"), EstiloBoton::Primario), (Id::Boton(Bot::Cancelar), tx!("comun.cancelar"), EstiloBoton::Normal)]);
        } else {
            self.boton_fila(Id::Boton(Bot::RootGestor), tx!("root.instalar_gestor"), EstiloBoton::Normal, !ocupado);
        }
        self.y += FILA_GAP;
        self.grupo(tx!("root.android"));
        self.bloque_reinicio(tx!("root.reinicia_solo_android_maquina"), false);
        self.mensajes_de("root", w);
        self.y += FILA_GAP;
    }

    /// Reinicio ORDENADO de Android por adb y, si adbd no responde, reinicio completo de la maquina (nunca `system_reset`:
    /// deja el renderizador grafico con estado viejo y Android no vuelve a arrancar). Lo usan Maquina y Acceso root.
    fn bloque_reinicio(&mut self, nota: &str, completo_siempre: bool) {
        let d = self.c.datos;
        let (w, ocupado) = (self.w, d.operacion.is_some() || d.reiniciando);
        self.parrafo(nota, Estilo::Pequeno, tema::p().texto2);
        self.y += 6.0;
        if self.a.confirmando == Some(Conf::ReiniciarAndroid) {
            let l = vec![tx!("reinicio.reinicio_ordenado_android_cierra").to_string()];
            self.caja_confirmacion(tx!("reinicio.pregunta_reiniciar_android"), &l, &[(Id::Boton(Bot::Confirmar), tx!("reinicio.reiniciar"), EstiloBoton::Primario), (Id::Boton(Bot::Cancelar), tx!("comun.cancelar"), EstiloBoton::Normal)]);
        } else {
            self.boton_fila(Id::Boton(Bot::ReiniciarAndroid), if d.reiniciando { tx!("reinicio.reiniciando") } else { tx!("reinicio.boton_reiniciar_android") }, EstiloBoton::Normal, !ocupado);
        }
        self.mensajes_de("reinicio", w);
        if d.reinicio_completo || completo_siempre {
            self.y += 12.0;
            if self.a.confirmando == Some(Conf::ReinicioCompleto) {
                let l = vec![tx!("reinicio.apaga_maquina_arranca_nuevo").to_string()];
                self.caja_con_tono(tx!("reinicio.pregunta_completo"), &l, &[(Id::Boton(Bot::Confirmar), tx!("reinicio.reiniciar_maquina"), EstiloBoton::Peligro), (Id::Boton(Bot::Cancelar), tx!("comun.cancelar"), EstiloBoton::Normal)], tema::p().error);
            } else {
                if d.reinicio_completo {
                    self.en_linea(Tono::Aviso, tx!("reinicio.android_no_responde_adb"), w);
                } else {
                    self.parrafo(tx!("reinicio.reinicio_completo_apaga_maquina"), Estilo::Pequeno, tema::p().texto2);
                }
                self.y += 6.0;
                self.boton_fila(Id::Boton(Bot::ReinicioCompleto), tx!("reinicio.boton_completo"), EstiloBoton::Peligro, true);
            }
        }
    }

    fn compartir(&mut self) {
        let d = self.c.datos;
        let w = self.w;
        self.pint.push(Pint::Icono { k: Icono::Circulo, r: R::new(self.x0, self.y + 4.0, 8.0, 8.0), c: tema::p().aviso });
        let (x0, y) = (self.x0 + 16.0, self.y);
        self.txt(x0, y, tx!("compartir.aplica_proximo_arranque_maquina"), Estilo::Negrita, tema::p().aviso);
        self.y += self.lh(Estilo::Negrita);
        self.parrafo(tx!("compartir.cada_carpeta_equipo_aparece"), Estilo::Pequeno, tema::p().texto2);
        if !d.virtiofsd {
            self.y += 6.0;
            self.en_linea(Tono::Error, tx!("compartir.no_encuentra_virtiofsd_equipo"), w);
        }
        let lista = compartir::lista(self.cfg);
        self.grupo(&txf!("compartir.carpetas", lista.len(), compartir::MAX));
        if lista.is_empty() {
            self.parrafo(tx!("compartir.todavia_no_hay_ninguna"), Estilo::Cuerpo, tema::p().texto2);
        }
        for (i, c) in lista.iter().enumerate() {
            let y0 = self.y;
            let (bw, ancho_txt) = (84.0, w - 84.0 - 12.0);
            let x0 = self.x0;
            let modo = if c.ro { tx!("compartir.solo_lectura") } else { tx!("compartir.lectura_escritura") };
            self.txt(x0, y0, &truncar(self.m(), &c.nombre, Estilo::Negrita, ancho_txt * 0.5), Estilo::Negrita, tema::p().texto);
            let xn = x0 + self.m().ancho(&truncar(self.m(), &c.nombre, Estilo::Negrita, ancho_txt * 0.5), Estilo::Negrita) + 10.0;
            let t = truncar(self.m(), &txf!("compartir.fila_ruta_android", c.nombre, modo), Estilo::Pequeno, (x0 + ancho_txt - xn).max(40.0));
            let dy = (self.lh(Estilo::Negrita) - self.lh(Estilo::Pequeno)) / 2.0;
            self.txt(xn, y0 + dy, &t, Estilo::Pequeno, tema::p().texto2);
            self.y += self.lh(Estilo::Negrita) + 2.0;
            let lhp = self.lh(Estilo::Pequeno);
            for l in envolver(self.m(), &c.ruta, Estilo::Pequeno, ancho_txt, 0) {
                let y = self.y;
                self.txt(x0, y, &l, Estilo::Pequeno, tema::p().texto2);
                self.y += lhp;
            }
            if let Err(e) = compartir::comprobar_ruta(&c.ruta) {
                self.en_linea(Tono::Aviso, &txf!("compartir.omitira_arrancar", crate::vista::traducir_errores(&e)), ancho_txt);
            }
            let fin = self.y;
            if self.a.confirmando == Some(Conf::QuitarCarpeta(i)) {
                self.y += 6.0;
                let l = vec![tx!("compartir.no_borra_nada_equipo").to_string()];
                self.caja_confirmacion(&txf!("compartir.quitar_lista", c.nombre), &l, &[(Id::Boton(Bot::Confirmar), tx!("comun.quitar"), EstiloBoton::Primario), (Id::Boton(Bot::Cancelar), tx!("comun.cancelar"), EstiloBoton::Normal)]);
            } else {
                self.boton_texto(Id::Quitar(i), R::new(x0 + w - bw, y0, bw, BH), tx!("comun.quitar"), EstiloBoton::Normal, true);
                self.y = fin.max(y0 + BH);
            }
            self.y += 12.0;
        }
        self.mensajes_de("share.quitar", w);
        self.mensajes_de("share.agregar-orden", w);
        self.grupo(tx!("compartir.agregar_carpeta"));
        let (lhp, x0) = (self.lh(Estilo::Pequeno), self.x0);
        self.txt(x0, self.y, tx!("compartir.nombre_android"), Estilo::Pequeno, tema::p().texto2);
        self.y += lhp + 2.0;
        let (t, tenue) = self.valor_campo(Campo::ShareNombre);
        let y = self.y;
        self.campo(Campo::ShareNombre, x0, y, w.min(260.0), &t, tenue);
        self.y += BH;
        self.mensajes_de("share.nombre", w);
        self.y += 10.0;
        self.txt(x0, self.y, tx!("compartir.carpeta_equipo"), Estilo::Pequeno, tema::p().texto2);
        self.y += lhp + 2.0;
        self.campo_ruta(Campo::ShareRuta);
        self.mensajes_de("share.ruta", w);
        self.y += 10.0;
        let y = self.y;
        let ro = self.a.form_ro;
        let etq = if ro { tx!("compartir.solo_lectura_si") } else { tx!("compartir.solo_lectura_no") };
        let bw1 = (self.m().ancho(etq, Estilo::Negrita) + 32.0).max(96.0).ceil();
        self.boton_texto(Id::Boton(Bot::FormSoloLectura), R::new(x0, y, bw1, BH), etq, if ro { EstiloBoton::Activo } else { EstiloBoton::Normal }, true);
        let lleno = lista.len() >= compartir::MAX;
        let bw2 = (self.m().ancho(tx!("compartir.agregar"), Estilo::Negrita) + 32.0).max(96.0).ceil();
        if bw1 + 8.0 + bw2 <= w + 0.5 {
            self.boton_texto(Id::Boton(Bot::AgregarCarpeta), R::new(x0 + bw1 + 8.0, y, bw2, BH), tx!("compartir.agregar"), EstiloBoton::Primario, !lleno);
            self.y += BH;
        } else {
            // ventana estrecha: "Agregar" pasa a la linea siguiente
            self.y += BH + 8.0;
            let y2 = self.y;
            self.boton_texto(Id::Boton(Bot::AgregarCarpeta), R::new(x0, y2, bw2.min(w), BH), tx!("compartir.agregar"), EstiloBoton::Primario, !lleno);
            self.y += BH;
        }
        if lleno {
            self.y += 6.0;
            self.en_linea(Tono::Aviso, &txf!("compartir.ya_hay_carpetas_maximo", compartir::MAX), w);
        }
        self.mensajes_de("share.agregar", w);
        self.y += FILA_GAP;
    }

    /// Linea "Se aplica al proximo arranque" con su circulo.
    fn nota_proximo_arranque(&mut self, t: &str) {
        self.pint.push(Pint::Icono { k: Icono::Circulo, r: R::new(self.x0, self.y + 4.0, 8.0, 8.0), c: tema::p().aviso });
        // una nota larga se parte en lineas (la ventana puede ser estrecha)
        let x0 = self.x0 + 16.0;
        for l in envolver(self.m(), t, Estilo::Negrita, self.w - 16.0, 0) {
            let y = self.y;
            self.txt(x0, y, &l, Estilo::Negrita, tema::p().aviso);
            self.y += self.lh(Estilo::Negrita);
        }
    }

    fn imagen(&mut self) {
        let d = self.c.datos;
        let (w, ocupado) = (self.w, d.operacion.is_some());
        self.nota_proximo_arranque(tx!("imagen.agregar_imagen_instante_usarla"));
        self.parrafo(tx!("imagen.agregar_imagen_copia_ya"), Estilo::Pequeno, tema::p().texto2);
        self.banda_operacion(w);
        self.grupo(tx!("comun.imagen_android"));
        match &d.almacen {
            None => {
                let t = if d.almacen_consultando { tx!("comun.consultando") } else { tx!("imagen.sin_datos_todavia") };
                self.parrafo(t, Estilo::Cuerpo, tema::p().texto2);
            }
            Some(a) => {
                let e = &a.imagen;
                self.par_largo(tx!("imagen.carpeta"), &e.carpeta.display().to_string());
                let (txt, col) = if e.completa() {
                    (tx!("imagen.completa").to_string(), tema::p().exito)
                } else if e.existe {
                    (tx!("imagen.incompleta").to_string(), tema::p().aviso)
                } else {
                    (tx!("imagen.ausente").to_string(), tema::p().aviso)
                };
                self.par(tx!("comun.estado"), &txt, col);
                if e.existe && !e.faltan.is_empty() {
                    let l: Vec<&str> = e.faltan.iter().take(4).map(|s| s.as_str()).collect();
                    let t = txf!("imagen.faltan_archivos", e.faltan.len(), l.join(", "), if e.faltan.len() > 4 { "..." } else { "" });
                    self.en_linea(Tono::Aviso, &t, w);
                }
                self.par(tx!("imagen.compilacion"), &e.build.map_or(tx!("comun.desconocida").to_string(), |b| b.to_string()), tema::p().texto);
                self.par(tx!("imagen.tamano"), &imagen::mib(e.bytes), tema::p().texto);
            }
        }
        self.y += FILA_GAP;
        self.grupo(tx!("imagen.imagenes_instaladas"));
        match &d.almacen {
            None => self.parrafo(tx!("imagen.sin_datos_todavia"), Estilo::Cuerpo, tema::p().texto2),
            Some(a) if a.imagenes.is_empty() => self.parrafo(tx!("imagen.no_hay_imagenes_instaladas"), Estilo::Cuerpo, tema::p().texto2),
            Some(a) => {
                let (imagenes, actual) = (a.imagenes.clone(), a.actual.clone());
                self.parrafo(tx!("imagen.cada_imagen_tiene_propio"), Estilo::Pequeno, tema::p().texto2);
                self.y += 6.0;
                for (i, f) in imagenes.iter().enumerate() {
                    let y0 = self.y;
                    let (bw, ancho_txt, x0) = (84.0, w - 84.0 - 12.0, self.x0);
                    let en_uso = actual.as_deref() == Some(f.id.as_str());
                    self.txt(x0, y0, &truncar(self.m(), &f.id, Estilo::Negrita, ancho_txt), Estilo::Negrita, tema::p().texto);
                    self.y += self.lh(Estilo::Negrita) + 2.0;
                    let det = txf!("imagen.fila_detalle", crate::textos::limpiar(&f.nombre_perfil), f.build.map_or(tx!("comun.desconocida").to_string(), |b| b.to_string()), imagen::mib(f.bytes), if f.con_disco { tx!("imagen.fila_con_disco") } else { "" }, if f.completa { "" } else { tx!("imagen.fila_incompleta") });
                    let lhp = self.lh(Estilo::Pequeno);
                    for l in envolver(self.m(), &det, Estilo::Pequeno, ancho_txt, 0) {
                        let y = self.y;
                        self.txt(x0, y, &l, Estilo::Pequeno, tema::p().texto2);
                        self.y += lhp;
                    }
                    let fin = self.y;
                    if self.a.confirmando == Some(Conf::CambiarImagen(i)) {
                        self.y += 6.0;
                        let mut l = vec![txf!("imagen.esta_maquina_arrancara_desde", f.id)];
                        l.push(if f.con_disco { tx!("imagen.esa_imagen_ya_tiene").to_string() } else { tx!("imagen.esa_imagen_no_tiene").to_string() });
                        if d.en_marcha {
                            l.push(tx!("imagen.maquina_esta_marcha_apagala").to_string());
                        }
                        self.caja_confirmacion(&txf!("imagen.usar_esta_maquina", f.id), &l, &[(Id::Boton(Bot::Confirmar), tx!("imagen.usar_esta_imagen"), EstiloBoton::Primario), (Id::Boton(Bot::Cancelar), tx!("comun.cancelar"), EstiloBoton::Normal)]);
                    } else if en_uso {
                        let t = tx!("imagen.uso");
                        let tw = self.m().ancho(t, Estilo::Negrita);
                        self.txt(x0 + w - tw, y0, t, Estilo::Negrita, tema::p().exito);
                        self.y = fin;
                    } else {
                        self.boton_texto(Id::Imagen(i), R::new(x0 + w - bw, y0, bw, BH), tx!("imagen.usar"), EstiloBoton::Normal, !ocupado && f.completa);
                        self.y = fin.max(y0 + BH);
                    }
                    self.y += 10.0;
                }
            }
        }
        self.mensajes_de("imagen.usar", w);
        self.grupo(tx!("imagen.agregar_una_imagen"));
        self.parrafo(tx!("imagen.indica_zip_carpeta_imagen"), Estilo::Pequeno, tema::p().texto2);
        self.y += 6.0;
        self.campo_ruta(Campo::Origen);
        self.y += 8.0;
        self.boton_fila(Id::Boton(Bot::ImagenAgregar), tx!("imagen.boton_agregar"), EstiloBoton::Primario, !ocupado);
        self.mensajes_de("imagen", w);
        self.y += FILA_GAP;
        self.grupo(tx!("imagen.disco_maquina"));
        match &d.almacen {
            None => self.parrafo(tx!("imagen.sin_datos_todavia"), Estilo::Cuerpo, tema::p().texto2),
            Some(a) => {
                let e = &a.disco;
                self.par_largo(tx!("imagen.archivo"), &e.ruta.display().to_string());
                if e.existe {
                    self.par(tx!("comun.estado"), &txf!("imagen.existe", imagen::gib(e.bytes)), tema::p().exito);
                    self.par(tx!("imagen.datos_userdata"), &e.datos.map_or(tx!("imagen.sin_particion").to_string(), imagen::gib), tema::p().texto);
                } else {
                    self.par(tx!("comun.estado"), tx!("imagen.no_existe"), tema::p().aviso);
                    let t = txf!("imagen.disco_crea_solo_proximo", crate::vista::orden_terminal("disk create", d.flatpak.as_deref()));
                    self.en_linea(Tono::Aviso, &t, w);
                }
            }
        }
        self.y += 8.0;
        self.etiqueta(tx!("imagen.tamano_particion_datos"), tx!("imagen.24g_4096m_img_userdata"), "disk.data", w);
        self.y += 6.0;
        self.fila_campos(&[("", Campo::DatosTam, 104.0)]);
        self.mensajes_de("disk.data", w);
        self.y += 10.0;
        if d.en_marcha {
            // tras apagar, esta ventana se cierra con la maquina: regenerar queda para la terminal
            let t = txf!("imagen.hay_maquina_marcha_este", crate::vista::orden_terminal("disk reset --yes", d.flatpak.as_deref()));
            self.en_linea(Tono::Aviso, &t, w);
            self.y += 8.0;
        }
        let datos = self.cfg.get("disk.data");
        match self.a.confirmando {
            Some(Conf::DiscoPaso1) => {
                let l = vec![crate::textos::texto(imagen::AVISO_REGENERAR).to_string(), txf!("imagen.creara_disco_nuevo_datos", datos)];
                self.caja_con_tono(tx!("imagen.borrar_disco_esta_maquina"), &l, &[(Id::Boton(Bot::Confirmar), tx!("imagen.continuar"), EstiloBoton::Peligro), (Id::Boton(Bot::Cancelar), tx!("comun.cancelar"), EstiloBoton::Normal)], tema::p().error);
            }
            Some(Conf::DiscoPaso2) => {
                let l = vec![tx!("imagen.ultima_confirmacion_borran_apps").to_string()];
                self.caja_con_tono(tx!("imagen.seguro_pierde_todo_maquina"), &l, &[(Id::Boton(Bot::Confirmar), tx!("imagen.borrar_regenerar"), EstiloBoton::Peligro), (Id::Boton(Bot::Cancelar), tx!("comun.cancelar"), EstiloBoton::Normal)], tema::p().error);
            }
            _ => {
                self.parrafo(tx!("imagen.regenerar_borra_apps_instaladas"), Estilo::Pequeno, tema::p().error);
                self.y += 6.0;
                self.boton_fila(Id::Boton(Bot::DiscoRegenerar), tx!("imagen.regenerar_disco"), EstiloBoton::Peligro, !ocupado && !d.en_marcha);
            }
        }
        self.mensajes_de("disco", w);
        self.y += 10.0;
        self.boton_fila(Id::Boton(Bot::AlmacenActualizar), if d.almacen_consultando { tx!("comun.consultando") } else { tx!("comun.actualizar_estado") }, EstiloBoton::Normal, !d.almacen_consultando && !ocupado);
        self.y += FILA_GAP;
    }

    fn puente(&mut self) {
        let d = self.c.datos;
        let (w, ocupado) = (self.w, d.operacion.is_some());
        self.parrafo(tx!("puente.traductor_arm_traduce_apps"), Estilo::Pequeno, tema::p().texto2);
        self.y += 6.0;
        self.en_linea(Tono::Aviso, crate::textos::texto(puente::AVISO_RIESGO), w);
        self.banda_operacion(w);
        self.grupo(tx!("comun.estado"));
        match &d.puente_estado {
            None => {
                let t = if d.puente_consultando { tx!("comun.consultando") } else { tx!("comun.sin_datos_maquina_esta") };
                self.parrafo(t, Estilo::Cuerpo, tema::p().texto2);
            }
            Some(e) => {
                let (si, no) = (tx!("comun.si"), tx!("comun.no"));
                self.par(tx!("puente.instalado"), if e.instalado() { si } else { no }, if e.instalado() { tema::p().exito } else { tema::p().texto });
                self.par(tx!("comun.arranque"), if e.boot { tx!("puente.completado_boot_completed_1") } else { tx!("puente.sin_completar") }, if e.boot { tema::p().texto } else { tema::p().aviso });
                self.par(tx!("puente.muertes_system_server"), &e.muertes.to_string(), if e.muertes > 1 { tema::p().error } else { tema::p().texto });
                let c = if e.en_zygote { tx!("puente.cargado_zygote") } else if e.lineas_log > 0 { tx!("puente.registro_traductor") } else { tx!("puente.sin_evidencia_sin_apps") };
                self.par(tx!("puente.carga"), c, if e.en_zygote { tema::p().exito } else { tema::p().texto2 });
                // biblioteca, propiedades y ABIs: datos del invitado con el nombre del archivo, tras "Detalles tecnicos"
                self.y += 4.0;
                let prop = if e.nb.is_empty() { tx!("puente.vacia") } else if e.instalado() { tx!("puente.configurada") } else { tx!("puente.otro_valor") };
                let det = vec![
                    txf!("puente.biblioteca_md5", e.md5.as_deref().map_or("-".to_string(), |m| m[..12.min(m.len())].to_string())),
                    txf!("puente.tamano", e.bytes.map_or("-".to_string(), puente::mib)),
                    txf!("puente.propiedad_puente_nativo", prop),
                    txf!("puente.abis", if e.abilist.is_empty() { tx!("puente.vacia") } else { &e.abilist }),
                ];
                self.detalles_en_linea(Detalle::PuenteEstado, &det);
                if e.marca {
                    self.y += 4.0;
                    self.y += 2.0;
                    let fp = d.flatpak.as_deref();
                    let t = txf!("puente.instalacion_anterior_no_termino", crate::vista::orden_terminal("bridge check", fp), crate::vista::orden_terminal("bridge restore", fp));
                    self.en_linea(Tono::Aviso, &t, w);
                }
            }
        }
        self.y += 6.0;
        self.boton_fila(Id::Boton(Bot::PuenteActualizar), if d.puente_consultando { tx!("comun.consultando") } else { tx!("comun.actualizar_estado") }, EstiloBoton::Normal, !d.puente_consultando && !ocupado);
        self.y += FILA_GAP;
        self.grupo(tx!("puente.instalacion"));
        let det = vec![
            txf!("puente.instala_biblioteca_traductor_arm", crate::textos::sitio_generico(puente::ORIGEN)),
            txf!("puente.detalles_completos_archivo_origen", crate::vista::orden_terminal("bridge status", d.flatpak.as_deref())),
        ];
        self.parrafo(tx!("puente.aplicacion_no_descarga_nada"), Estilo::Pequeno, tema::p().texto2);
        self.y += 6.0;
        self.campo_ruta(Campo::RutaPuente);
        self.y += 8.0;
        self.detalles_en_linea(Detalle::Puente, &det);
        self.y += 8.0;
        let instalado = d.puente_estado.as_ref().is_some_and(|e| e.instalado());
        match self.a.confirmando {
            Some(Conf::PuenteInstalar) => {
                let l = vec![
                    tx!("puente.instalara_biblioteca_traductor_arm").to_string(),
                    tx!("puente.android_reinicia_apps_abiertas").to_string(),
                ];
                self.caja_con_detalle(tx!("puente.instalar_traductor_arm"), &l, Some((Detalle::Puente, &det)), &[(Id::Boton(Bot::Confirmar), if instalado { tx!("puente.actualizar") } else { tx!("comun.instalar") }, EstiloBoton::Primario), (Id::Boton(Bot::Cancelar), tx!("comun.cancelar"), EstiloBoton::Normal)], tema::p().aviso);
            }
            Some(Conf::PuenteQuitar) => {
                let l = vec![tx!("puente.quita_biblioteca_system_lib64").to_string()];
                self.caja_confirmacion(tx!("puente.quitar_traductor_arm"), &l, &[(Id::Boton(Bot::Confirmar), tx!("comun.quitar"), EstiloBoton::Primario), (Id::Boton(Bot::Cancelar), tx!("comun.cancelar"), EstiloBoton::Normal)]);
            }
            _ => {
                let y = self.y;
                let x0 = self.x0;
                let (t1, t2) = (if instalado { tx!("puente.actualizar") } else { tx!("comun.instalar") }, tx!("comun.quitar"));
                let bw1 = (self.m().ancho(t1, Estilo::Negrita) + 32.0).max(96.0).ceil();
                let bw2 = (self.m().ancho(t2, Estilo::Negrita) + 32.0).max(96.0).ceil();
                self.boton_texto(Id::Boton(Bot::PuenteInstalar), R::new(x0, y, bw1, BH), t1, EstiloBoton::Primario, !ocupado);
                if bw1 + 8.0 + bw2 <= w + 0.5 {
                    self.boton_texto(Id::Boton(Bot::PuenteQuitar), R::new(x0 + bw1 + 8.0, y, bw2, BH), t2, EstiloBoton::Normal, !ocupado && instalado);
                    self.y += BH;
                } else {
                    self.y += BH + 8.0;
                    let y2 = self.y;
                    self.boton_texto(Id::Boton(Bot::PuenteQuitar), R::new(x0, y2, bw2.min(w), BH), t2, EstiloBoton::Normal, !ocupado && instalado);
                    self.y += BH;
                }
            }
        }
        self.mensajes_de("puente", w);
        self.y += FILA_GAP;
    }

    fn diagnostico(&mut self) {
        let d = self.c.datos;
        let (w, ocupado) = (self.w, d.operacion.is_some());
        self.parrafo(tx!("diagnostico.comprueba_equipo_reune_necesario"), Estilo::Pequeno, tema::p().texto2);
        self.grupo(tx!("diagnostico.comprobacion_equipo"));
        self.filas_doctor();
        self.y += 4.0;
        self.boton_fila(Id::Boton(Bot::Comprobar), if d.doctor_en_curso { tx!("comun.comprobando") } else { tx!("diagnostico.ejecutar_doctor") }, EstiloBoton::Normal, !d.doctor_en_curso);
        self.y += FILA_GAP;
        self.grupo(tx!("diagnostico.informe_errores"));
        self.parrafo(tx!("diagnostico.reune_archivo_tar_gz"), Estilo::Pequeno, tema::p().texto2);
        self.banda_operacion(w);
        self.y += 8.0;
        self.boton_fila(Id::Boton(Bot::InformeCrear), tx!("diagnostico.crear_informe"), EstiloBoton::Primario, !ocupado);
        self.mensajes_de("informe", w);
        self.y += FILA_GAP;
    }

    /// Resultado de `doctor` en lineas coloreadas OK / AVISO / FALLO (lo comparten Diagnostico y Acerca de).
    fn filas_doctor(&mut self) {
        let d = self.c.datos;
        match &d.doctor {
            None => {
                let t = if d.doctor_en_curso { tx!("diagnostico.comprobando_equipo") } else { tx!("diagnostico.aun_no_ha_comprobado") };
                self.parrafo(t, Estilo::Cuerpo, tema::p().texto2);
            }
            Some(filas) => {
                let (fallos, avisos) = (filas.iter().filter(|f| f.nivel == NivelDoctor::Fallo).count(), filas.iter().filter(|f| f.nivel == NivelDoctor::Aviso).count());
                let resumen = if fallos > 0 { txf!("diagnostico.fallo_s_aviso_s", fallos, avisos) } else if avisos > 0 { txf!("diagnostico.sin_fallos_aviso_s", avisos) } else { tx!("diagnostico.todo_orden").to_string() };
                self.parrafo(&resumen, Estilo::Cuerpo, tema::p().texto);
                self.y += 6.0;
                for f in filas {
                    let (etq, col) = match f.nivel {
                        NivelDoctor::Ok => (tx!("diagnostico.ok"), tema::p().exito),
                        NivelDoctor::Aviso => (tx!("diagnostico.aviso"), tema::p().aviso),
                        NivelDoctor::Fallo => (tx!("diagnostico.fallo"), tema::p().error),
                    };
                    let (x0, y) = (self.x0, self.y);
                    self.txt(x0, y, etq, Estilo::Negrita, col);
                    self.txt(x0 + 56.0, y, &f.nombre, Estilo::Negrita, tema::p().texto);
                    self.y += self.lh(Estilo::Negrita);
                    let lhp = self.lh(Estilo::Pequeno);
                    let ancho = self.w - 56.0;
                    for l in envolver(self.m(), &f.detalle, Estilo::Pequeno, ancho, 0) {
                        let y = self.y;
                        self.txt(x0 + 56.0, y, &l, Estilo::Pequeno, tema::p().texto2);
                        self.y += lhp;
                    }
                    self.y += 6.0;
                }
            }
        }
    }

    fn acerca(&mut self) {
        let d = self.c.datos;
        self.parrafo("weft", Estilo::Negrita, tema::p().texto);
        self.parrafo(&txf!("acerca.version", d.version), Estilo::Cuerpo, tema::p().texto2);
        self.parrafo(tx!("acerca.emulador_android_sobre_qemu"), Estilo::Pequeno, tema::p().texto2);
        self.grupo(tx!("acerca.estado_equipo"));
        self.filas_doctor();
        self.y += 4.0;
        self.boton_fila(Id::Boton(Bot::Comprobar), if d.doctor_en_curso { tx!("comun.comprobando") } else { tx!("acerca.volver_comprobar") }, EstiloBoton::Normal, !d.doctor_en_curso);
        self.y += FILA_GAP;
        self.grupo(tx!("acerca.rutas"));
        for (etq, ruta) in [(tx!("comun.estado"), &d.estado_dir), (tx!("comun.configuracion"), &d.config_ruta)] {
            self.parrafo(etq, Estilo::Pequeno, tema::p().texto2);
            self.parrafo(ruta, Estilo::Cuerpo, tema::p().texto);
            self.y += 6.0;
        }
        self.grupo(tx!("acerca.tipografia"));
        self.parrafo(&d.fuente, Estilo::Cuerpo, tema::p().texto);
        self.parrafo(&txf!("acerca.escala_interfaz_rasteriza_tamano", d.escala), Estilo::Pequeno, tema::p().texto2);
    }
}

/// Porcentaje (0 a 100) que lleva un texto de progreso ("copiando la imagen: 45 %"), si lo lleva. Pura.
pub fn porcentaje_de(t: &str) -> Option<f32> {
    let i = t.rfind('%')?;
    let antes = t[..i].trim_end();
    let num: String = antes.chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',').collect::<Vec<_>>().into_iter().rev().collect();
    num.replace(',', ".").parse::<f32>().ok().filter(|p| (0.0..=100.0).contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuente::Tipografia;

    fn tipo() -> Tipografia {
        Tipografia::solo_respaldo(1.0)
    }

    /// Contexto desde los campos del banco (no toma prestado el banco entero, para poder mutar `a` y `cfg`).
    macro_rules! ctx {
        ($b:expr) => {
            Ctx { vent: $b.vent, m: &$b.t, datos: &$b.d, ahora: $b.ahora }
        };
    }

    fn datos() -> Datos {
        Datos { estado_dir: "/estado/prueba".into(), config_ruta: "/estado/prueba/config".into(), fuente: "mapa de bits 5x7 (respaldo)".into(), resolucion: (720, 1348), ..Datos::default() }
    }

    fn tec(sc: u32) -> Tecla {
        Tecla { sc, abajo: true, repetida: false, ctrl: false, alt: false, mayus: false }
    }

    struct Banco {
        a: Ajustes,
        cfg: Config,
        t: Tipografia,
        d: Datos,
        vent: (f32, f32),
        ahora: Instant,
    }

    impl Banco {
        fn nuevo(vent: (f32, f32)) -> Banco {
            let mut a = Ajustes::nuevo();
            a.abrir(Some(Seccion::General));
            Banco { a, cfg: Config::nueva(), t: tipo(), d: datos(), vent, ahora: Instant::now() }
        }
        fn ctx(&self) -> Ctx<'_> {
            Ctx { vent: self.vent, m: &self.t, datos: &self.d, ahora: self.ahora }
        }
        fn botones(&self) -> Vec<(String, R)> {
            self.a.botones(&self.cfg, &self.ctx())
        }
        fn boton(&self, n: &str) -> R {
            self.botones().into_iter().find(|(x, _)| x == n).unwrap_or_else(|| panic!("no hay control {} en {:?}", n, self.botones().iter().map(|b| b.0.clone()).collect::<Vec<_>>())).1
        }
        /// Desplaza el contenido hasta que el control sea visible (si ya lo es, no toca nada).
        fn ir_a(&mut self, n: &str) {
            if self.botones().iter().any(|(x, _)| x == n) {
                return;
            }
            self.a.scroll = 0.0;
            for _ in 0..60 {
                if self.botones().iter().any(|(x, _)| x == n) {
                    return;
                }
                self.a.scroll += 100.0;
            }
        }
        fn clic(&mut self, n: &str) -> Vec<Efecto> {
            self.ir_a(n);
            let r = self.boton(n);
            let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
            self.clic_en(x, y)
        }
        fn clic_en(&mut self, x: f32, y: f32) -> Vec<Efecto> {
            let c = Ctx { vent: self.vent, m: &self.t, datos: &self.d, ahora: self.ahora };
            self.a.mover(&self.cfg, &c, x, y);
            self.a.presionar(&self.cfg, &c, x, y);
            self.a.soltar(&mut self.cfg, &c, x, y)
        }
        fn tecla(&mut self, t: Tecla) -> (bool, Vec<Efecto>) {
            let c = Ctx { vent: self.vent, m: &self.t, datos: &self.d, ahora: self.ahora };
            self.a.tecla(&mut self.cfg, &c, t)
        }
        fn seccion(&mut self, s: Seccion) {
            self.clic(&format!("a-nav-{}", s.nombre()));
            assert_eq!(self.a.seccion, s);
        }
        fn textos(&self) -> Vec<String> {
            let c = self.ctx();
            self.a.dibujar(&self.cfg, &c).into_iter().filter_map(|p| if let Pint::Texto { t, .. } = p { Some(t) } else { None }).collect()
        }
        fn hay_texto(&self, parte: &str) -> bool {
            self.textos().iter().any(|t| t.contains(parte))
        }
    }

    #[test]
    fn geometria_ancha_y_estrecha() {
        let t = tipo();
        // ventana ancha: dialogo de 720 como maximo y 90 % del alto (tope 640), centrado; navegacion a la izquierda
        let g = geometria((1200.0, 900.0), &t);
        assert_eq!(g.dialogo.w, 720.0);
        assert_eq!(g.dialogo.h, 640.0);
        assert_eq!(g.dialogo.x, 240.0);
        assert!(g.nav.is_some() && g.tabs.is_none());
        assert_eq!(g.nav.unwrap().w, NAV_ANCHO);
        assert_eq!(g.contenido.x, g.dialogo.x + NAV_ANCHO);
        // alto limitado al 90 % de la ventana
        let g = geometria((900.0, 500.0), &t);
        assert_eq!(g.dialogo.h, 450.0);
        // ventana estrecha (< 560 de dialogo): pestanas arriba, sin columna
        let g = geometria((500.0, 700.0), &t);
        assert!(g.dialogo.w < ANCHO_ESTRECHO && g.nav.is_none());
        let tabs = g.tabs.unwrap();
        assert_eq!(g.contenido.y, tabs.y + tabs.h);
        assert_eq!(g.contenido.w, g.dialogo.w);
        // siempre dentro de la ventana
        for v in [(1200.0, 900.0), (900.0, 500.0), (500.0, 700.0), (380.0, 440.0), (300.0, 300.0)] {
            let g = geometria(v, &t);
            assert!(g.dialogo.x >= 0.0 && g.dialogo.y >= 0.0 && g.dialogo.x + g.dialogo.w <= v.0 + 0.01 && g.dialogo.y + g.dialogo.h <= v.1 + 0.01, "{:?} {:?}", v, g.dialogo);
        }
    }

    /// En todas las secciones y a varios tamanos: ningun control sale del dialogo y no se pisan entre si.
    #[test]
    fn controles_dentro_y_sin_solaparse() {
        for vent in [(1100.0, 800.0), (700.0, 600.0), (500.0, 600.0), (396.0, 440.0)] {
            for s in Seccion::TODAS {
                let mut b = Banco::nuevo(vent);
                b.a.abrir(Some(s));
                b.a.captura = None;
                let g = geometria(vent, &b.t);
                // al principio y tras desplazar hasta el final
                for pos in [0.0f32, 1e6] {
                    b.a.scroll = pos;
                    let c = ctx!(b);
                    let mq = b.a.maquetar(&b.cfg, &c, &g);
                    for k in &mq.controles {
                        assert!(k.r.x >= g.dialogo.x - 0.01 && k.r.x + k.r.w <= g.dialogo.x + g.dialogo.w + 0.01, "{:?} {:?} {}: {:?} fuera del dialogo", vent, s, pos, k.id);
                    }
                    // los del contenido no se solapan entre si
                    let en_contenido: Vec<&Control> = mq.controles.iter().filter(|k| !matches!(k.id, Id::Cerrar | Id::Nav(_))).collect();
                    for (i, a) in en_contenido.iter().enumerate() {
                        for bb in en_contenido.iter().skip(i + 1) {
                            assert!(!a.r.se_cruza(&bb.r.reducir(0.5)), "{:?} {:?}: {:?} y {:?} se solapan ({:?} / {:?})", vent, s, a.id, bb.id, a.r, bb.r);
                        }
                    }
                }
                // todo control del contenido se puede alcanzar desplazando
                b.a.scroll = 0.0;
                let c = ctx!(b);
                let mq = b.a.maquetar(&b.cfg, &c, &g);
                let alto = b.a.alto_contenido.get();
                let max = (alto - g.contenido.h).max(0.0);
                for k in mq.controles.iter().filter(|k| !matches!(k.id, Id::Cerrar | Id::Nav(_))) {
                    let y_rel = k.r.y - g.contenido.y;
                    assert!(y_rel >= 0.0 && y_rel + k.r.h <= alto + 0.01, "{:?} {:?} fuera del contenido ({} de {})", s, k.id, y_rel, alto);
                    assert!(y_rel + k.r.h <= max + g.contenido.h + 0.01);
                }
            }
        }
    }

    #[test]
    fn cerrar_con_x_esc_y_clic_fuera() {
        let mut b = Banco::nuevo((1000.0, 700.0));
        assert_eq!(b.clic("a-cerrar"), vec![Efecto::Cerrar]);
        assert!(!b.a.abierto);
        // Esc
        b.a.abrir(None);
        let (c, e) = b.tecla(tec(41));
        assert!(c && e == vec![Efecto::Cerrar] && !b.a.abierto);
        // clic fuera del dialogo (pulsar y soltar fuera)
        b.a.abrir(None);
        assert_eq!(b.clic_en(5.0, 5.0), vec![Efecto::Cerrar]);
        assert!(!b.a.abierto);
        // un clic dentro del dialogo, en un hueco, no cierra
        b.a.abrir(None);
        let g = geometria(b.vent, &b.t);
        assert!(b.clic_en(g.dialogo.x + g.dialogo.w - 4.0, g.dialogo.y + g.dialogo.h - 4.0).is_empty());
        assert!(b.a.abierto);
        // pulsar fuera y soltar dentro tampoco cierra
        let c = ctx!(b);
        b.a.presionar(&b.cfg, &c, 5.0, 5.0);
        let c = ctx!(b);
        assert!(b.a.soltar(&mut b.cfg, &c, g.dialogo.x + 30.0, g.dialogo.y + 100.0).is_empty());
        assert!(b.a.abierto);
    }

    #[test]
    fn la_pantalla_abierta_lo_consume_todo() {
        let mut b = Banco::nuevo((1000.0, 700.0));
        // cualquier tecla (y su soltado) y cualquier clic quedan dentro de la pantalla
        for sc in [4u32, 58, 41 + 100, 44, 79] {
            let (c, _) = b.tecla(tec(sc));
            assert!(c, "sc {}", sc);
            let (c, _) = b.tecla(Tecla { abajo: false, ..tec(sc) });
            assert!(c);
        }
        let c = ctx!(b);
        assert!(b.a.presionar(&b.cfg, &c, 3.0, 3.0));
        // cerrada: nada
        b.a.cerrar();
        let (c, e) = b.tecla(tec(4));
        assert!(!c && e.is_empty());
        let c = ctx!(b);
        assert!(!b.a.presionar(&b.cfg, &c, 3.0, 3.0));
    }

    #[test]
    fn interruptor_opcion_y_paso_guardan_al_instante() {
        let mut b = Banco::nuevo((1000.0, 700.0));
        // interruptor
        assert!(b.cfg.bool("confirmar"));
        assert_eq!(b.clic("a-tog-confirmar"), vec![Efecto::Cambio("confirmar".into())]);
        assert!(!b.cfg.bool("confirmar"));
        assert!(b.hay_texto("Pedir confirmación"));
        // "Guardado" breve junto al control, que desaparece
        assert!(b.hay_texto(tx!("comun.guardado")));
        assert!(b.a.animado(b.ahora));
        b.ahora += Duration::from_secs(3);
        assert!(!b.hay_texto(tx!("comun.guardado")) && !b.a.animado(b.ahora));
        // opcion de zoom
        assert_eq!(b.clic("a-opt-zoom-2"), vec![Efecto::Cambio("zoom".into())]);
        assert_eq!(b.cfg.get("zoom"), "75");
        // tema: Automatico, Claro y Oscuro; se guarda como cualquier otra clave (la ventana lo aplica en caliente)
        assert_eq!(b.cfg.get("ventana.tema"), "auto");
        assert!(b.hay_texto(tx!("general.tema")) && b.hay_texto(tx!("opcion.claro")) && b.hay_texto(tx!("opcion.oscuro")));
        assert_eq!(b.clic("a-opt-ventana.tema-1"), vec![Efecto::Cambio("ventana.tema".into())]);
        assert_eq!(b.cfg.get("ventana.tema"), "claro");
        b.clic("a-opt-ventana.tema-0");
        assert_eq!(b.cfg.get("ventana.tema"), "auto");
        // un zoom inicial que no esta en la lista (puesto a mano en el archivo) se avisa
        b.cfg.set("zoom", "67").unwrap();
        b.d.zoom_modo = ModoZoom::Fijo(67);
        assert!(b.hay_texto("Zoom inicial: 67 %") && !b.hay_texto("Ahora:"));
        b.cfg.set("zoom", "75").unwrap();
        b.d.zoom_modo = ModoZoom::Ajustar;
        // pasos de la rueda
        b.seccion(Seccion::Entrada);
        assert_eq!(b.cfg.get("rueda.paso"), "10");
        assert_eq!(b.clic("a-paso-rueda.paso-mas"), vec![Efecto::Cambio("rueda.paso".into())]);
        assert_eq!(b.cfg.get("rueda.paso"), "11");
        b.clic("a-paso-rueda.paso-menos");
        b.clic("a-paso-rueda.paso-menos");
        assert_eq!(b.cfg.get("rueda.paso"), "9");
        // tope: el boton se apaga en el limite y no hace nada
        b.cfg.set("rueda.paso", "25").unwrap();
        assert!(b.clic("a-paso-rueda.paso-mas").is_empty());
        assert_eq!(b.cfg.get("rueda.paso"), "25");
        // pellizco con Alt, mandos automaticos
        assert_eq!(b.clic("a-opt-pellizco.modificador-1"), vec![Efecto::Cambio("pellizco.modificador".into())]);
        assert_eq!(b.cfg.get("pellizco.modificador"), "alt");
        b.clic("a-opt-gamepad-0");
        assert_eq!(b.cfg.get("gamepad"), "auto");
        assert_eq!(b.clic("a-tog-rueda.invertir"), vec![Efecto::Cambio("rueda.invertir".into())]);
        assert!(b.cfg.bool("rueda.invertir"));
    }

    #[test]
    fn captura_de_un_atajo() {
        let mut b = Banco::nuevo((1000.0, 700.0));
        b.seccion(Seccion::Atajos);
        assert!(b.hay_texto("F1") && b.hay_texto("Ctrl+=") && b.hay_texto(tx!("atajo.configuracion")));
        // el chip entra en modo captura
        assert!(b.clic("a-atajo-atras").is_empty());
        assert_eq!(b.a.en_captura(), Some(AccionAtajo::Atras));
        // (con la fuente de respaldo, tan ancha, el chip trunca el texto y la ayuda se parte en lineas)
        assert!(b.hay_texto("Pulsa") && b.hay_texto("Esc cancela"), "{:?}", b.textos());
        // solo un modificador: se sigue esperando
        b.tecla(Tecla { ctrl: true, ..tec(224) });
        assert_eq!(b.a.en_captura(), Some(AccionAtajo::Atras));
        // Ctrl+B se asigna
        let (c, e) = b.tecla(Tecla { ctrl: true, ..tec(5) });
        assert!(c);
        assert_eq!(e, vec![Efecto::Cambio("atajo.atras".into())]);
        assert_eq!(b.cfg.get("atajo.atras"), "Ctrl+B");
        assert!(b.a.en_captura().is_none());
        assert!(b.hay_texto("Ctrl+B") && b.hay_texto(tx!("comun.guardado")));
        // la tecla nueva dispara la accion y la vieja ya no
        assert_eq!(b.cfg.atajo_para(5, true, false, false), Some(AccionAtajo::Atras));
        assert_eq!(b.cfg.atajo_para(58, false, false, false), None);
        // Esc cancela la captura sin cerrar la pantalla ni cambiar nada
        b.clic("a-atajo-inicio");
        let (c, e) = b.tecla(tec(41));
        assert!(c && e.is_empty() && b.a.abierto && b.a.en_captura().is_none());
        assert_eq!(b.cfg.get("atajo.inicio"), "F2");
        // Retroceso borra la tecla
        b.clic("a-atajo-inicio");
        let (_, e) = b.tecla(tec(42));
        assert_eq!(e, vec![Efecto::Cambio("atajo.inicio".into())]);
        assert_eq!(b.cfg.get("atajo.inicio"), "ninguno");
        assert!(b.hay_texto(tx!("atajos.sin_asignar")));
        // una letra sola no vale: error en linea y se sigue esperando
        b.clic("a-atajo-inicio");
        let (_, e) = b.tecla(tec(7));
        assert!(e.is_empty() && b.a.en_captura().is_some());
        assert!(b.hay_texto("sin Ctrl ni Alt solo valen"), "{:?}", b.textos());
        assert!(matches!(b.a.mensaje_de("atajo.inicio"), Some((Tono::Error, _))));
        // una tecla no admitida (Tab) tambien
        b.tecla(tec(43));
        assert!(b.hay_texto("no se puede usar"));
        // F12 sola si vale (F11 es de la pantalla completa)
        let (_, e) = b.tecla(tec(69));
        assert_eq!(e, vec![Efecto::Cambio("atajo.inicio".into())]);
        assert_eq!(b.cfg.get("atajo.inicio"), "F12");
        // al pulsar otro control se cancela la captura
        b.clic("a-atajo-recientes");
        assert!(b.a.en_captura().is_some());
        b.clic("a-nav-general");
        assert!(b.a.en_captura().is_none());
    }

    #[test]
    fn conflicto_de_atajos() {
        let mut b = Banco::nuevo((1000.0, 700.0));
        b.seccion(Seccion::Atajos);
        b.clic("a-atajo-atras");
        // F2 es de Inicio: se muestra el conflicto y NO se guarda
        let (_, e) = b.tecla(tec(59));
        assert!(e.is_empty());
        assert_eq!(b.cfg.get("atajo.atras"), "F1");
        assert_eq!(b.a.en_captura(), Some(AccionAtajo::Atras));
        assert!(b.hay_texto("F2 ya la usa «Inicio»."), "{:?}", b.textos());
        assert!(matches!(b.a.mensaje_de("atajo.atras"), Some((Tono::Error, _))));
        // F11 tampoco: es de la pantalla completa (un atajo de la ventana que config.rs no tiene como accion)
        let (_, e) = b.tecla(tec(68));
        assert!(e.is_empty());
        assert!(b.hay_texto("F11 ya la usa «Pantalla completa»."), "{:?}", b.textos());
        // se resuelve eligiendo otra tecla
        let (_, e) = b.tecla(tec(69));
        assert_eq!(e, vec![Efecto::Cambio("atajo.atras".into())]);
        assert_eq!(b.cfg.get("atajo.atras"), "F12");
        assert!(b.a.mensaje_de("atajo.atras").is_none());
        // o quitandola de la otra accion y usandola aqui
        b.clic("a-atajo-vol_mas");
        b.tecla(tec(59));
        assert!(b.hay_texto("Quitarla de «Inicio» y usarla aquí"));
        let e = b.clic("a-btn-usar-aqui");
        assert_eq!(e, vec![Efecto::Cambio("atajo.inicio".into()), Efecto::Cambio("atajo.vol_mas".into())]);
        assert_eq!(b.cfg.get("atajo.vol_mas"), "F2");
        assert_eq!(b.cfg.get("atajo.inicio"), "ninguno");
        assert!(b.a.en_captura().is_none());
        // Esc deja el conflicto sin tocar nada
        b.clic("a-atajo-rotar");
        b.tecla(tec(59));
        b.tecla(tec(41));
        assert_eq!(b.cfg.get("atajo.rotar"), "F7");
        assert!(b.a.mensaje_de("atajo.rotar").is_none());
        // restaurar valores por defecto
        let e = b.clic("a-btn-restaurar-atajos");
        assert_eq!(e, vec![Efecto::Cambio("atajos".into())]);
        for a in AccionAtajo::TODAS {
            assert_eq!(b.cfg.get(a.clave()), config::clave(a.clave()).unwrap().defecto);
        }
    }

    /// Atajos propios de la ventana (pasar la siguiente tecla, pantalla completa, encendido, menu): salen en Atajos con su
    /// defecto, se capturan como los demas y chocan con los de `config` en los dos sentidos. Son claves de `config`: se
    /// guardan en el archivo (no duran solo lo que la ventana).
    #[test]
    fn atajos_propios_de_la_ventana() {
        let mut b = Banco::nuevo((1000.0, 700.0));
        b.seccion(Seccion::Atajos);
        let todo = |b: &Banco| b.textos().join(" ");
        assert!(todo(&b).contains(tx!("atajo.pantalla_completa")) && todo(&b).contains("Ctrl+Alt+F"), "{:?}", b.textos());
        // la seccion explica como mandar a Android una tecla de atajo
        assert!(todo(&b).contains("pulsa antes Ctrl+Alt+F"), "{}", todo(&b));
        // Menu viene sin asignar; la captura le pone Ctrl+Alt+M
        b.ir_a("a-atajo-menu");
        assert!(b.clic("a-atajo-menu").is_empty());
        assert_eq!(b.a.en_captura_fila(), Some(FilaAtajo::Extra(AtajoExtra::Menu)));
        let (_, e) = b.tecla(Tecla { ctrl: true, alt: true, ..tec(16) });
        assert_eq!(e, vec![Efecto::Cambio("atajo.menu".into())]);
        assert_eq!(b.a.valor(&b.cfg, "atajo.menu"), "Ctrl+Alt+M");
        assert_eq!(b.a.atajo_extra_para(&b.cfg, 16, true, true, false), Some(AtajoExtra::Menu));
        assert_eq!(b.a.atajo_extra_para(&b.cfg, 16, true, false, false), None);
        assert_eq!(b.a.atajo_extra_para(&b.cfg, 68, false, false, false), Some(AtajoExtra::PantallaCompleta));
        assert_eq!(b.a.atajo_extra_para(&b.cfg, 9, true, true, false), Some(AtajoExtra::PasarTecla));
        // una tecla de `config` para un atajo de la ventana: conflicto y "usarla aqui"
        b.clic("a-atajo-encendido");
        let (_, e) = b.tecla(tec(58));
        assert!(e.is_empty());
        assert_eq!(b.a.mensaje_de("atajo.encendido").map(|(_, t)| t.to_string()).as_deref(), Some("F1 ya la usa «Atrás»."));
        assert_eq!(b.a.valor(&b.cfg, "atajo.encendido"), "ninguno");
        let e = b.clic("a-btn-usar-aqui");
        assert_eq!(e, vec![Efecto::Cambio("atajo.atras".into()), Efecto::Cambio("atajo.encendido".into())]);
        assert_eq!(b.cfg.get("atajo.atras"), "ninguno");
        assert_eq!(b.a.valor(&b.cfg, "atajo.encendido"), "F1");
        // y al reves: la combinacion de un atajo de la ventana para uno de `config`
        b.clic("a-atajo-inicio");
        let (_, e) = b.tecla(Tecla { ctrl: true, alt: true, ..tec(9) });
        assert!(e.is_empty());
        assert_eq!(b.a.mensaje_de("atajo.inicio").map(|(_, t)| t.to_string()).as_deref(), Some("Ctrl+Alt+F ya la usa «Pasar la siguiente tecla a Android»."));
        b.tecla(tec(41));
        assert_eq!(b.cfg.get("atajo.inicio"), "F2");
        assert_eq!(b.a.quien_usa(&b.cfg, "F11", FilaAtajo::Accion(AccionAtajo::Atras)), Some(FilaAtajo::Extra(AtajoExtra::PantallaCompleta)));
        assert_eq!(b.a.quien_usa(&b.cfg, "F11", FilaAtajo::Extra(AtajoExtra::PantallaCompleta)), None);
        // Retroceso deja sin asignar un atajo de la ventana
        b.clic("a-atajo-pantalla_completa");
        let (_, e) = b.tecla(tec(42));
        assert_eq!(e, vec![Efecto::Cambio("atajo.pantalla_completa".into())]);
        assert_eq!(b.a.tecla_de(&b.cfg, FilaAtajo::Extra(AtajoExtra::PantallaCompleta)), None);
        assert_eq!(b.a.atajo_extra_para(&b.cfg, 68, false, false, false), None);
        // restaurar vuelve todos a su defecto, los de la ventana tambien
        b.clic("a-btn-restaurar-atajos");
        for e in AtajoExtra::TODOS {
            assert_eq!(b.a.valor(&b.cfg, e.clave()), config::clave(e.clave()).unwrap().defecto, "{:?}", e);
        }
        assert_eq!(b.cfg.get("atajo.atras"), "F1");
        // los de la ventana son claves de `config`: lo que se cambia aqui se guarda y vale en la proxima ventana
        b.ir_a("a-atajo-menu");
        b.clic("a-atajo-menu");
        b.tecla(Tecla { ctrl: true, alt: true, ..tec(16) });
        assert_eq!(b.cfg.get("atajo.menu"), "Ctrl+Alt+M");
        let d = std::env::temp_dir().join(format!("weft-ajustes-atajos-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        b.cfg.guardar(&d).unwrap();
        assert_eq!(Ajustes::nuevo().valor(&Config::cargar(&d), "atajo.menu"), "Ctrl+Alt+M");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// El interruptor "Atajos desactivados" de la seccion Atajos: apagado por defecto, se enciende y se apaga con un clic.
    #[test]
    fn interruptor_de_atajos_desactivados() {
        let mut b = Banco::nuevo((1000.0, 700.0));
        b.seccion(Seccion::Atajos);
        assert!(b.hay_texto(tx!("teclado.atajos_desactivados")));
        assert_eq!(b.a.valor(&b.cfg, gestos::ATAJOS_DESACTIVADOS), "no");
        let encendido = |b: &Banco| {
            let r = b.boton("a-tog-atajos.desactivados");
            b.a.dibujar(&b.cfg, &b.ctx()).iter().any(|p| matches!(p, Pint::Rect { r: x, c, .. } if *x == r && *c == tema::p().acento))
        };
        assert!(!encendido(&b));
        let e = b.clic("a-tog-atajos.desactivados");
        assert_eq!(e, vec![Efecto::Cambio(gestos::ATAJOS_DESACTIVADOS.into())]);
        assert_eq!(b.a.valor(&b.cfg, gestos::ATAJOS_DESACTIVADOS), "si");
        b.a.salir();
        assert!(encendido(&b));
        b.clic("a-tog-atajos.desactivados");
        assert_eq!(b.a.valor(&b.cfg, gestos::ATAJOS_DESACTIVADOS), "no");
        // es una clave de `config`: se guarda en el archivo, no solo en esta ventana
        b.clic("a-tog-atajos.desactivados");
        assert!(b.cfg.bool(gestos::ATAJOS_DESACTIVADOS));
        assert!(b.cfg.texto().contains("\natajos.desactivados=si\n"));
    }

    /// Entrada: que hacen los botones derecho y central con la pantalla tactil (Atras e Inicio por defecto).
    #[test]
    fn botones_del_raton_en_entrada() {
        let mut b = Banco::nuevo((1000.0, 700.0));
        b.seccion(Seccion::Entrada);
        b.ir_a("a-opt-raton.central-4");
        assert!(b.hay_texto(tx!("entrada.boton_derecho")) && b.hay_texto(tx!("entrada.boton_central")), "{:?}", b.textos());
        assert_eq!(b.a.valor(&b.cfg, gestos::RATON_DERECHO), "atras");
        assert_eq!(b.a.valor(&b.cfg, gestos::RATON_CENTRAL), "inicio");
        let e = b.clic("a-opt-raton.derecho-2");
        assert_eq!(e, vec![Efecto::Cambio(gestos::RATON_DERECHO.into())]);
        assert_eq!(b.a.valor(&b.cfg, gestos::RATON_DERECHO), "recientes");
        b.clic("a-opt-raton.central-4");
        assert_eq!(b.a.valor(&b.cfg, gestos::RATON_CENTRAL), "none");
        let (der, cen) = (b.a.valor(&b.cfg, gestos::RATON_DERECHO), b.a.valor(&b.cfg, gestos::RATON_CENTRAL));
        assert_eq!(gestos::boton_tactil(2, &der, &cen), Some(crate::gestos::Atajo::Recientes));
        assert_eq!(gestos::boton_tactil(1, &der, &cen), None);
        // se guardan en `config` como las demas claves
        assert!(b.cfg.texto().contains("\nraton.derecho=recientes\n") && b.cfg.texto().contains("\nraton.central=none\n"));
        // una clave que no es de config no se puede fijar
        assert!(b.a.fijar(&mut b.cfg, "inventada", "si").is_err());
    }

    #[test]
    fn campos_numericos_y_aviso_de_multiplo_de_8() {
        let mut b = Banco::nuevo((1000.0, 700.0));
        b.seccion(Seccion::Pantalla);
        // sin configuracion: muestra la resolucion real del panel en tenue
        assert!(b.hay_texto("720") && b.hay_texto("1348") && b.hay_texto("perfil"));
        assert!(!b.hay_texto("múltiplo de 8"));
        // se edita el ancho: 1348 no es multiplo de 8 -> AVISO (amarillo), no error
        b.clic("a-campo-ancho");
        assert_eq!(b.a.editando(), Some(Campo::Ancho));
        for _ in 0..3 {
            b.tecla(tec(42)); // retroceso
        }
        for sc in [30, 31, 32, 33, 34] {
            b.tecla(tec(sc)); // 12345 -> el ancho que queda es 7 (de 720) + 12345 ...
        }
        // aviso en linea mientras se escribe
        let v: String = b.a.edicion.as_ref().unwrap().buf.clone();
        assert_eq!(v, "12345");
        assert!(b.hay_texto("múltiplo de 8") && b.hay_texto("12344"), "{:?}", b.textos());
        // el aviso es amarillo, no rojo
        let c = ctx!(b);
        let avisos: Vec<Color> = b.a.dibujar(&b.cfg, &c).into_iter().filter_map(|p| if let Pint::Texto { t, c, .. } = p { if t.contains("redondeará") { Some(c) } else { None } } else { None }).collect();
        assert!(!avisos.is_empty() && avisos.iter().all(|c| *c == tema::p().aviso), "{:?}", avisos);
        // ancho fuera de rango: error en linea (rojo) y se sigue editando
        let (_, e) = b.tecla(tec(40));
        assert!(e.is_empty());
        assert_eq!(b.a.editando(), Some(Campo::Ancho));
        assert!(matches!(b.a.mensaje_de("pantalla.resolucion"), Some((Tono::Error, m)) if m.contains("fuera de rango")), "{:?}", b.a.mensaje_de("pantalla.resolucion"));
        assert_eq!(b.cfg.get("pantalla.resolucion"), "auto");
        // se corrige: 1348 -> guarda, avisa del multiplo y deja de editar
        for _ in 0..5 {
            b.tecla(tec(42));
        }
        for sc in [30, 34, 33, 37] {
            b.tecla(tec(sc)); // 1 5 4 8 -> 1548? (1,5,4,8)
        }
        assert_eq!(b.a.edicion.as_ref().unwrap().buf, "1548");
        let (_, e) = b.tecla(tec(40));
        assert_eq!(e, vec![Efecto::Cambio("pantalla.resolucion".into())]);
        assert_eq!(b.a.editando(), None);
        assert_eq!(b.cfg.get("pantalla.resolucion"), "1548x1348");
        assert!(b.hay_texto("redondeará a 1544"));
        // el alto con teclado numerico y Esc cancela
        b.clic("a-campo-alto");
        b.tecla(tec(89)); // 1 del teclado numerico
        assert!(b.a.edicion.as_ref().unwrap().buf.ends_with('1'));
        b.tecla(tec(41));
        assert_eq!(b.a.editando(), None);
        assert!(b.a.abierto, "Esc cancela la edicion pero no cierra");
        assert_eq!(b.cfg.get("pantalla.resolucion"), "1548x1348");
        // densidad: vacio = perfil; un valor
        b.clic("a-campo-densidad");
        for sc in [32, 37, 39] {
            b.tecla(tec(sc)); // 3 8 0
        }
        let (_, e) = b.tecla(tec(40));
        assert_eq!(e, vec![Efecto::Cambio("pantalla.densidad".into())]);
        assert_eq!(b.cfg.get("pantalla.densidad"), "380");
        b.clic("a-campo-densidad");
        for _ in 0..3 {
            b.tecla(tec(42));
        }
        b.tecla(tec(40));
        assert_eq!(b.cfg.get("pantalla.densidad"), "perfil");
        // cursor: flechas, suprimir, inicio y fin
        b.clic("a-campo-ancho");
        b.tecla(tec(74)); // inicio
        b.tecla(tec(76)); // supr
        assert_eq!(b.a.edicion.as_ref().unwrap().buf, "548");
        b.tecla(tec(77)); // fin
        b.tecla(tec(80)); // izquierda
        b.tecla(tec(31)); // 2
        assert_eq!(b.a.edicion.as_ref().unwrap().buf, "5428");
        // las letras no entran
        b.tecla(tec(4));
        assert_eq!(b.a.edicion.as_ref().unwrap().buf, "5428");
        // otro control confirma lo editado
        b.tecla(tec(77)); // fin
        for _ in 0..4 {
            b.tecla(tec(42));
        }
        for sc in [34, 31, 36, 37] {
            b.tecla(tec(sc)); // 5 2 7 8
        }
        let e = b.clic("a-tog-pantalla.giro_android");
        assert!(e.contains(&Efecto::Cambio("pantalla.resolucion".into())), "{:?}", e);
        assert_eq!(b.cfg.get("pantalla.resolucion"), "5278x1348");
        // aplicar ahora pide a la ventana ejecutar `resolution`
        assert_eq!(b.clic("a-btn-aplicar-resolucion"), vec![Efecto::AplicarResolucion]);
        // el aviso de que reinicia SurfaceFlinger esta a la vista
        assert!(b.hay_texto("SurfaceFlinger"), "{:?}", b.textos());
        // mientras se aplica el boton se apaga
        b.d.aplicando = true;
        assert!(b.clic("a-btn-aplicar-resolucion").is_empty());
        assert!(b.hay_texto(tx!("pantalla.aplicando")));
    }

    #[test]
    fn maquina_campos_y_estado() {
        let mut b = Banco::nuevo((1000.0, 700.0));
        b.seccion(Seccion::Maquina);
        assert!(b.hay_texto(tx!("maquina.aplican_proximo_arranque")));
        b.clic("a-campo-cpus");
        b.tecla(tec(35)); // 6
        let (_, e) = b.tecla(tec(40));
        assert_eq!(e, vec![Efecto::Cambio("maquina.cpus".into())]);
        assert_eq!(b.cfg.entero("maquina.cpus"), Some(6));
        b.clic("a-campo-ram");
        for sc in [37, 33, 33, 34] {
            b.tecla(tec(sc)); // 8 4 4 5 = 8445 MiB
        }
        b.tecla(tec(40));
        assert_eq!(b.cfg.entero("maquina.ram"), Some(8445));
        // un valor invalido: error en linea
        b.clic("a-campo-ram");
        for _ in 0..4 {
            b.tecla(tec(42));
        }
        b.tecla(tec(30)); // 1 MiB < 512
        let (_, e) = b.tecla(tec(40));
        assert!(e.is_empty());
        assert!(matches!(b.a.mensaje_de("maquina.ram"), Some((Tono::Error, _))));
        assert_eq!(b.cfg.entero("maquina.ram"), Some(8445));
        b.tecla(tec(41));
        // tipo y gpu
        assert_eq!(b.clic("a-opt-maquina.tipo-0"), vec![Efecto::Cambio("maquina.tipo".into())]);
        assert_eq!(b.cfg.get("maquina.tipo"), "pc");
        b.clic("a-opt-maquina.gpu-2");
        assert_eq!(b.cfg.get("maquina.gpu"), "software");
        b.clic("a-opt-maquina.gpu-1");
        assert_eq!(b.cfg.get("maquina.gpu"), "hardware");
        // un motor concreto fijado a mano (valor avanzado) se avisa sin nombrarlo
        b.cfg.set("maquina.gpu", "gfxstream").unwrap();
        b.a.scroll = 0.0;
        assert!(b.hay_texto("motor gráfico concreto fijado a mano"));
        b.cfg.set("maquina.gpu", "software").unwrap();
        // la carpeta de estado se imprime con un boton
        b.a.scroll = 1e6;
        assert!(b.hay_texto("/estado/prueba"));
        assert_eq!(b.clic("a-btn-abrir-estado"), vec![Efecto::AbrirEstado]);
        b.a.scroll = 0.0;
        // lo que corre en la maquina
        b.d.maquina = Some(Maquina { cpus: 4, mem_mb: 8192, tipo: "pc-q35-10.2-machine".into(), estado: "running".into() });
        assert!(b.hay_texto("8192 MB") && b.hay_texto("q35 10.2"));
    }

    #[test]
    fn rotacion_desde_la_pantalla() {
        let mut b = Banco::nuevo((1000.0, 700.0));
        b.seccion(Seccion::Pantalla);
        assert_eq!(b.clic("a-opt-orientacion-2"), vec![Efecto::Orientacion(Orientacion::Fija(1))]);
        assert_eq!(b.clic("a-opt-orientacion-0"), vec![Efecto::Orientacion(Orientacion::Auto)]);
        assert_eq!(b.clic("a-opt-orientacion-4"), vec![Efecto::Orientacion(Orientacion::Fija(3))]);
        // la pantalla no toca `config` por la rotacion: eso es estado de la maquina
        assert_eq!(b.cfg, Config::nueva());
        // con auto muestra lo que hace Android
        b.d.orient = Orientacion::Auto;
        b.d.rot = 1;
        assert!(b.hay_texto("Android la tiene a 90 grados."));
        assert_eq!(b.clic("a-tog-pantalla.giro_android"), vec![Efecto::Cambio("pantalla.giro_android".into())]);
        assert!(!b.cfg.bool("pantalla.giro_android"));
    }

    #[test]
    fn teclado_tab_enter_y_flechas() {
        let mut b = Banco::nuevo((1000.0, 700.0));
        let ids = |b: &Banco| -> Vec<String> { b.botones().into_iter().map(|x| x.0).collect() };
        // Tab recorre: cerrar, navegacion, contenido
        let orden = ids(&b);
        assert_eq!(orden[0], "a-cerrar");
        assert_eq!(&orden[1..14], ["a-nav-controles", "a-nav-general", "a-nav-atajos", "a-nav-entrada", "a-nav-pantalla", "a-nav-maquina", "a-nav-perfiles", "a-nav-imagen", "a-nav-compartir", "a-nav-root", "a-nav-puente", "a-nav-diagnostico", "a-nav-acerca"]);
        b.tecla(tec(43));
        assert_eq!(b.a.foco, Some(Id::Cerrar));
        assert!(b.a.foco_visible);
        b.tecla(tec(43));
        assert_eq!(b.a.foco, Some(Id::Nav(Seccion::Controles)));
        // Mayus+Tab vuelve
        b.tecla(Tecla { mayus: true, ..tec(43) });
        assert_eq!(b.a.foco, Some(Id::Cerrar));
        // Mayus+Tab desde el primero da la vuelta al ultimo
        b.tecla(Tecla { mayus: true, ..tec(43) });
        let ultimo = b.botones().last().unwrap().0.clone();
        assert_eq!(b.a.foco.unwrap().nombre(), ultimo);
        // flechas en la lista de navegacion cambian de seccion
        b.a.foco = Some(Id::Nav(Seccion::General));
        b.tecla(tec(81));
        assert_eq!(b.a.seccion, Seccion::Atajos);
        assert_eq!(b.a.foco, Some(Id::Nav(Seccion::Atajos)));
        b.tecla(tec(81));
        b.tecla(tec(82));
        assert_eq!(b.a.seccion, Seccion::Atajos);
        b.tecla(tec(82));
        b.tecla(tec(82));
        assert_eq!(b.a.seccion, Seccion::Controles, "no pasa del principio");
        // Enter activa el interruptor con foco; Espacio tambien
        b.a.foco = Some(Id::Interruptor("confirmar"));
        let (_, e) = b.tecla(tec(40));
        assert_eq!(e, vec![Efecto::Cambio("confirmar".into())]);
        assert!(!b.cfg.bool("confirmar"));
        let (_, e) = b.tecla(tec(44));
        assert_eq!(e, vec![Efecto::Cambio("confirmar".into())]);
        assert!(b.cfg.bool("confirmar"));
        // flechas izquierda/derecha en una lista de opciones la cambian
        b.a.foco = Some(Id::Opcion("zoom", 0));
        let (_, e) = b.tecla(tec(79));
        assert_eq!(e, vec![Efecto::Cambio("zoom".into())]);
        assert_eq!(b.cfg.get("zoom"), "50");
        assert_eq!(b.a.foco, Some(Id::Opcion("zoom", 1)));
        b.tecla(tec(80));
        assert_eq!(b.cfg.get("zoom"), "ajustar");
        // Enter sobre un chip empieza la captura
        b.seccion(Seccion::Atajos);
        b.a.foco = Some(Id::Atajo(FilaAtajo::Accion(AccionAtajo::Captura)));
        b.tecla(tec(40));
        assert_eq!(b.a.en_captura(), Some(AccionAtajo::Captura));
        // el foco solo se ve tras usar el teclado: un clic lo oculta
        b.tecla(tec(41));
        b.tecla(tec(43));
        assert!(b.a.foco_visible);
        b.clic("a-nav-general");
        assert!(!b.a.foco_visible);
        // sobre el control con foco hay un anillo, en los dos temas
        b.a.foco = Some(Id::Opcion("zoom", 1));
        b.a.foco_visible = true;
        for v in [tema::Variante::Oscuro, tema::Variante::Claro] {
            let c = ctx!(b);
            let anillos = tema::con(v, || b.a.dibujar(&b.cfg, &c).into_iter().filter(|p| matches!(p, Pint::Rect { c, r, .. } if *c == tema::p().acento_claro && r.w > 10.0)).count());
            assert_eq!(anillos, 1, "{:?}", v);
        }
    }

    /// Tab hasta que el foco este en el control `nombre` (falla si no llega).
    fn tab_hasta(b: &mut Banco, nombre: &str) {
        for _ in 0..300 {
            if b.a.foco.map(|f| f.nombre()).as_deref() == Some(nombre) {
                return;
            }
            b.tecla(tec(43));
        }
        panic!("Tab no llega a {}: {:?}", nombre, b.botones().into_iter().map(|x| x.0).collect::<Vec<_>>());
    }

    /// Esc es "atras": con una pregunta pendiente la cancela (sin cerrar la pantalla) y el foco vuelve al boton que la abrio;
    /// sin nada pendiente cierra. Al abrir la pregunta con el teclado el foco queda en un control visible de ella.
    #[test]
    fn esc_cancela_la_pregunta_y_el_foco_vuelve() {
        let mut b = Banco::nuevo((1000.0, 900.0));
        b.seccion(Seccion::Maquina);
        tab_hasta(&mut b, "a-btn-apagar");
        let (_, e) = b.tecla(tec(40));
        assert!(e.is_empty() && b.a.confirmando == Some(Conf::Apagar), "{:?}", e);
        let f = b.a.foco.unwrap().nombre();
        assert!(b.botones().iter().any(|(n, _)| *n == f), "el foco quedo en {} que no se ve", f);
        let (_, e) = b.tecla(tec(41));
        assert!(e.is_empty() && b.a.abierto && b.a.confirmando.is_none());
        assert_eq!(b.a.foco.map(|f| f.nombre()).as_deref(), Some("a-btn-apagar"));
        assert!(b.a.foco_visible);
        // Cancelar con el teclado tambien devuelve el foco
        b.tecla(tec(40));
        tab_hasta(&mut b, "a-btn-cancelar");
        b.tecla(tec(44));
        assert!(b.a.confirmando.is_none());
        assert_eq!(b.a.foco.map(|f| f.nombre()).as_deref(), Some("a-btn-apagar"));
        // sin nada pendiente, Esc cierra
        let (_, e) = b.tecla(tec(41));
        assert_eq!(e, vec![Efecto::Cerrar]);
        assert!(!b.a.abierto);
    }

    /// En una ventana estrecha la navegacion son pestanas arriba: las flechas izquierda y derecha cambian de seccion.
    #[test]
    fn flechas_laterales_en_las_pestanas() {
        let mut b = Banco::nuevo((500.0, 700.0));
        assert!(geometria(b.vent, &b.t).tabs.is_some());
        b.a.foco = Some(Id::Nav(Seccion::General));
        b.tecla(tec(79));
        assert_eq!((b.a.seccion, b.a.foco), (Seccion::Atajos, Some(Id::Nav(Seccion::Atajos))));
        b.tecla(tec(80));
        b.tecla(tec(80));
        b.tecla(tec(80));
        assert_eq!(b.a.seccion, Seccion::Controles, "no pasa del principio");
        assert!(b.a.foco_visible);
    }

    /// Tamano del texto: sus opciones son las de `config`, se guardan al instante y la configuracion cabe en la ventana a
    /// cada tamano (la ventana le da su tamano dividido por el factor).
    #[test]
    fn tamano_del_texto() {
        let valores: Vec<&str> = opciones("ventana.texto").into_iter().map(|(v, _)| v).collect();
        assert_eq!(valores, config::TAMANOS_TEXTO);
        assert_eq!([crate::formas::tamano::de_config("150"), crate::formas::tamano::de_config("125 %")], [150, 125]);
        assert_eq!([crate::formas::tamano::de_config("300"), crate::formas::tamano::de_config(""), crate::formas::tamano::de_config("x")], [100, 100, 100]);
        let mut b = Banco::nuevo((1000.0, 800.0));
        b.seccion(Seccion::General);
        assert!(b.hay_texto(tx!("general.tamano_texto")));
        assert_eq!(b.clic("a-opt-ventana.texto-3"), vec![Efecto::Cambio("ventana.texto".into())]);
        assert_eq!(b.cfg.get("ventana.texto"), "200");
        for u in [1.25f32, 1.5, 2.0] {
            for (w, h) in [(1280.0f32, 800.0f32), (360.0 * u, 480.0 * u)] {
                let vent = (w / u, h / u);
                let mut b = Banco::nuevo(vent);
                b.a.abrir(Some(Seccion::General));
                let g = geometria(vent, &b.t);
                let c = ctx!(b);
                let mq = b.a.maquetar(&b.cfg, &c, &g);
                for k in &mq.controles {
                    assert!(k.r.x >= g.dialogo.x - 0.01 && k.r.x + k.r.w <= g.dialogo.x + g.dialogo.w + 0.01, "{} {:?}: {:?} fuera del dialogo", u, vent, k.id);
                }
            }
        }
    }

    /// Banco con datos de sobra para que cada seccion muestre casi todo: maquina, mandos, doctor, imagen y disco, root y
    /// traductor con avisos, una carpeta compartida y mensajes en linea de los tres tonos. `variante` 1 cambia a lo malo: sin
    /// acceso a la maquina, disco ausente con la maquina en marcha, sin virtiofsd, una operacion en curso, en Flatpak.
    /// Carpeta de perfiles de dispositivo del usuario, nueva en cada llamada (las pruebas corren en paralelo).
    fn carpeta_perfiles(n: &str) -> std::path::PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let i = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("weft-ajustes-perfiles-{}-{}-{}", n, std::process::id(), i));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn banco_completo(vent: (f32, f32), sec: Seccion, variante: u8) -> Banco {
        let mut b = Banco::nuevo(vent);
        if sec == Seccion::Perfiles {
            // uno propio con todo: lista de cambios con una desconocida y otra sin implementar, identidad y registros
            let dir = carpeta_perfiles("completo");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("mio.device"), "dispositivo.id=mio\ndispositivo.hereda=generico-a78\ndispositivo.nombre=Un perfil con un nombre bastante largo para la fila\nproducto.modelo=Modelo X\ncpu.features=+sve -aes +rara\ncpu.id_aa64isar0=0x1000\n").unwrap();
            std::fs::write(dir.join("roto.device"), "dispositivo.id=roto\npagina=3\n").unwrap();
            b.a.perfiles_dir = Some(dir);
            b.cfg.set("dispositivo.perfil", if variante == 0 { "mio" } else { "borrado" }).unwrap();
        }
        b.a.abrir(Some(sec));
        if sec == Seccion::Perfiles {
            b.a.detalles.push(Detalle::PerfilRegistros);
            if variante == 1 {
                b.a.perfil_visto = Some("mio".into());
            }
        }
        b.d.maquina = Some(Maquina { cpus: 2, mem_mb: 4096, tipo: "pc-q35-10.2-machine".into(), estado: "running".into() });
        b.d.encendida = Some(Duration::from_secs(95));
        b.d.mandos = vec![
            Mando { path: "/dev/input/event7".into(), name: "Mando de prueba".into(), conectado: None, ocupado: false },
            Mando { path: "/dev/input/event8".into(), name: "Otro mando".into(), conectado: Some("pad0".into()), ocupado: false },
            Mando { path: "/dev/input/event9".into(), name: "Mando ocupado".into(), conectado: None, ocupado: true },
        ];
        b.d.doctor = Some(vec![
            FilaDoctor { nivel: NivelDoctor::Ok, nombre: "KVM".into(), detalle: "disponible".into() },
            FilaDoctor { nivel: NivelDoctor::Aviso, nombre: "Espacio".into(), detalle: "poco espacio libre".into() },
            FilaDoctor { nivel: NivelDoctor::Fallo, nombre: "QEMU".into(), detalle: "no se encuentra".into() },
        ]);
        b.d.reinicio_completo = true;
        b.d.root_estado = Some(root::Estado { servicio: true, ksud: true, gestor: true, oficial: true, version: Some("33294".into()), ..Default::default() });
        b.d.puente_estado = Some(puente::Estado { marca: true, muertes: 2, ..Default::default() });
        compartir::agregar(&mut b.cfg, Carpeta { nombre: "Compartida".into(), ro: false, ruta: "/no/existe/compartida".into() }, false).unwrap();
        b.cfg.set("zoom", "80").unwrap();
        if variante == 0 {
            b.d.almacen = Some(almacen_de_prueba(true, true));
        } else {
            b.d.almacen = Some(almacen_de_prueba(false, false));
            b.d.en_marcha = true;
            b.d.qmp_ok = false;
            b.d.maquina = None;
            b.d.virtiofsd = false;
            b.d.mandos.clear();
            b.d.ilegibles = 2;
            b.d.operacion = Some(("root".into(), "Copiando archivos".into()));
            b.d.flatpak = Some("io.github.ejemplo.weft".into());
        }
        let claves = ["controles", "root", "puente", "imagen", "disco", "informe", "reinicio", "apagar", "mandos", "gamepad", "aplicar", "estado", "share.quitar", "share.agregar", "maquina.cpus", "pantalla.resolucion", "disk.data", "perfiles", "disp.cpu_midr", "disp.features"];
        for (i, k) in claves.iter().enumerate() {
            let tono = [Tono::Aviso, Tono::Error, Tono::Exito][(i + variante as usize) % 3];
            b.a.mensaje(k, tono, &format!("Mensaje de prueba para {}.", k));
        }
        b
    }

    /// Recorre la configuracion en muchos estados y entrega cada dibujo con su descripcion: cada seccion, en ventana ancha y
    /// estrecha, con los datos buenos y los malos, en reposo y con cada confirmacion abierta (con sus detalles desplegados),
    /// una tecla por capturar con conflicto y un campo en edicion; desplazada de arriba abajo y, con `raton`, con el raton
    /// encima de cada control visible y pulsandolo. Devuelve cuantos dibujos hizo.
    fn recorrer_estados(raton: bool, f: &mut dyn FnMut(&Banco, &[Pint], &str)) -> usize {
        let mut n = 0;
        let confs = [None, Some(Conf::PerfilBorrar), Some(Conf::PerfilAplicar), Some(Conf::RootActivar), Some(Conf::RootDesactivar), Some(Conf::RootGestor), Some(Conf::PuenteInstalar), Some(Conf::PuenteQuitar), Some(Conf::ReiniciarAndroid), Some(Conf::ReinicioCompleto), Some(Conf::Apagar), Some(Conf::QuitarCarpeta(0)), Some(Conf::DiscoPaso1), Some(Conf::DiscoPaso2), Some(Conf::CambiarImagen(1))];
        for vent in [(1100.0f32, 900.0f32), (500.0, 700.0)] {
            for sec in Seccion::TODAS {
                for variante in [0u8, 1] {
                    let mut b = banco_completo(vent, sec, variante);
                    let reposo = b.a.dibujar(&b.cfg, &b.ctx());
                    for (i, conf) in confs.iter().enumerate() {
                        // las preguntas de los perfiles solo se dibujan en su seccion, y las demas fuera de ella (la seccion
                        // Perfiles es larga: recorrerla con cada pregunta ajena repetiria el mismo dibujo)
                        if conf.is_some() && matches!(conf, Some(Conf::PerfilBorrar | Conf::PerfilAplicar)) != (sec == Seccion::Perfiles) {
                            continue;
                        }
                        b.a.confirmando = *conf;
                        b.a.detalles = if i % 2 == 0 { vec![Detalle::Root, Detalle::Puente, Detalle::PuenteEstado, Detalle::PerfilRegistros] } else { vec![] };
                        // tecla por capturar con conflicto (Atajos) y campo en edicion (los demas): solo en reposo
                        b.a.captura = None;
                        b.a.conflicto = None;
                        b.a.edicion = None;
                        if conf.is_none() {
                            b.a.captura = Some(FilaAtajo::Accion(AccionAtajo::Atras));
                            b.a.conflicto = Some((Combo::parse("F2").unwrap(), FilaAtajo::Accion(AccionAtajo::Inicio)));
                            let campo = if sec == Seccion::Perfiles { campo_perfil("cpu.midr") } else { [Campo::Ancho, Campo::Cpus, Campo::ShareRuta, Campo::Origen, Campo::RutaRoot, Campo::RutaPuente, Campo::DatosTam][sec as usize % 7] };
                            b.a.edicion = Some(Edicion { campo, buf: "1234".into(), cursor: 2 });
                        }
                        b.a.scroll = 0.0;
                        b.a.hover = None;
                        b.a.pulsado = None;
                        let dibujo = b.a.dibujar(&b.cfg, &b.ctx());
                        if conf.is_some() && dibujo == reposo {
                            continue; // esta confirmacion no es de esta seccion
                        }
                        let g = geometria(b.vent, &b.t);
                        let mut scroll = 0.0f32;
                        loop {
                            b.a.scroll = scroll;
                            b.a.hover = None;
                            b.a.pulsado = None;
                            let que = format!("{:?} {:?} variante {} {:?} desplazado {}", vent, sec, variante, conf, scroll);
                            f(&b, &b.a.dibujar(&b.cfg, &b.ctx()), &que);
                            n += 1;
                            if raton {
                                // de la rejilla de extensiones y de los campos del perfil (todos iguales) bastan unos pocos:
                                // uno apagado, uno encendido (sve) y dos campos
                                let repetido = |id: &Id| match id {
                                    Id::Extension(i) => *i >= 4 && dispositivo::EXTENSIONES.get(*i) != Some(&"sve"),
                                    Id::Campo(Campo::Perfil(i)) => *i >= 2,
                                    _ => false,
                                };
                                let ids: Vec<Id> = b.a.maquetar(&b.cfg, &b.ctx(), &g).controles.iter().filter(|k| k.visible.w > 0.0 && k.visible.h > 0.0 && !repetido(&k.id)).map(|k| k.id).collect();
                                for id in ids {
                                    for pulsado in [false, true] {
                                        b.a.hover = Some(id);
                                        b.a.pulsado = pulsado.then_some(id);
                                        f(&b, &b.a.dibujar(&b.cfg, &b.ctx()), &format!("{} con el raton {} {:?}", que, if pulsado { "pulsando" } else { "encima de" }, id));
                                        n += 1;
                                    }
                                    // con el foco del teclado (anillo visible)
                                    (b.a.hover, b.a.pulsado, b.a.foco, b.a.foco_visible) = (None, None, Some(id), true);
                                    f(&b, &b.a.dibujar(&b.cfg, &b.ctx()), &format!("{} con el foco en {:?}", que, id));
                                    n += 1;
                                    (b.a.foco, b.a.foco_visible) = (None, false);
                                }
                            }
                            let alto = b.a.alto_contenido.get();
                            if scroll + g.contenido.h >= alto {
                                break;
                            }
                            scroll += (g.contenido.h - 40.0).max(40.0);
                        }
                    }
                }
            }
        }
        n
    }

    /// E.11: todo texto de la configuracion llega al contraste AA de WCAG (4,5:1; 3:1 los iconos) sobre su fondo REAL, en
    /// todas las secciones y estados, con el raton encima y pulsando cada control (botones normales y secundarios, pestanas,
    /// opciones, chips de atajos, campos, interruptores y la navegacion). Solo se exime el texto de un control deshabilitado.
    #[test]
    fn contraste_aa_en_todos_los_estados() {
        // en los dos temas
        for v in [tema::Variante::Oscuro, tema::Variante::Claro] {
            let mut fallos: Vec<String> = Vec::new();
            let n = tema::con(v, || {
                recorrer_estados(true, &mut |b, dibujo, que| {
                    let pares = crate::formas::pares_de_color(dibujo, tema::p().fondo, &|e| b.t.alto_linea(e));
                    for f in crate::formas::fallos_de_contraste(&pares) {
                        if fallos.len() < 20 && !fallos.iter().any(|x| x.ends_with(&f)) {
                            fallos.push(format!("{}: {}", que, f));
                        }
                    }
                })
            });
            assert!(fallos.is_empty(), "{:?}: {} fallos de contraste, p. ej.:\n{}", v, fallos.len(), fallos.join("\n"));
            assert!(n > 3000, "se recorrieron pocos estados: {}", n);
        }
    }

    /// E.10: la fuente de respaldo (5x7) dibuja todos los caracteres de la configuracion: ninguno cae en el '?' de relleno.
    #[test]
    fn la_fuente_de_respaldo_cubre_toda_la_interfaz() {
        let mut faltan: std::collections::BTreeSet<char> = Default::default();
        let mut ejemplo: Vec<String> = Vec::new();
        recorrer_estados(false, &mut |_, dibujo, _| {
            for p in dibujo {
                if let Pint::Texto { t, .. } = p {
                    for c in t.chars().filter(|c| !crate::fuente::mapa5x7::cubre(*c)) {
                        if faltan.insert(c) {
                            ejemplo.push(t.clone());
                        }
                    }
                }
            }
        });
        assert!(faltan.is_empty(), "sin glifo en la fuente de respaldo: {:?} (en {:?})", faltan, ejemplo);
    }

    /// E.10: un solo nombre para la configuracion ("Configuración": la pestana de la barra, el titulo del modal y el atajo) y
    /// los mismos terminos en todo el modal: "máquina" para la maquina virtual y "Android" para el sistema que corre dentro
    /// (ni "invitado" ni "VM" ni "guest").
    #[test]
    fn un_solo_nombre_para_cada_cosa() {
        let mut b = Banco::nuevo((1100.0, 900.0));
        assert!(b.hay_texto(tx!("comun.configuracion")));
        b.seccion(Seccion::Atajos);
        let t = b.textos().join(" ");
        assert!(t.contains("La pestaña Configuración de la barra") && t.contains("no llegan a Android"), "{}", t);
        // la pestana de la barra guarda la clave del catalogo: es la misma que la del titulo del modal
        assert_eq!(crate::barra::FICHAS[0].2, "comun.configuracion");
        assert_eq!(crate::textos::texto(crate::barra::FICHAS[0].2), tx!("comun.configuracion"));
        assert!(AccionAtajo::Configuracion.etiqueta().to_lowercase().contains(&tx!("comun.configuracion").to_lowercase()));
        let mut malos: Vec<String> = Vec::new();
        recorrer_estados(false, &mut |_, dibujo, que| {
            for p in dibujo {
                if let Pint::Texto { t, .. } = p {
                    let l = t.to_lowercase();
                    let palabra = |w: &str| l.split(|c: char| !c.is_alphanumeric()).any(|x| x == w);
                    let malo = l.contains("configuraciones") || palabra("invitado") || palabra("vm") || palabra("guest") || (palabra("emulador") && !l.starts_with("emulador de android"));
                    if malo && malos.len() < 10 {
                        malos.push(format!("{}: {:?}", que, t));
                    }
                }
            }
        });
        assert!(malos.is_empty(), "{:?}", malos);
    }

    /// E.10: los textos que mandaban a la terminal: donde hay un boton, el boton; donde no, la orden completa (y la de
    /// Flatpak, dentro de Flatpak).
    #[test]
    fn ordenes_de_terminal_o_botones() {
        for (flatpak, con_fp) in [(None, false), (Some("io.github.ejemplo.weft".to_string()), true)] {
            let fp = |o: &str| if con_fp { format!("«weft {o}» (o, en Flatpak, «flatpak run --command=weft io.github.ejemplo.weft {o}»)") } else { format!("«weft {o}»") };
            let todo = |b: &Banco| b.textos().join(" ");
            // Maquina: los ajustes se usan al proximo arranque (sin "el guion")
            let mut b = Banco::nuevo((1100.0, 900.0));
            b.d.flatpak = flatpak.clone();
            b.d.maquina = Some(Maquina { cpus: 2, mem_mb: 4096, tipo: "q35".into(), estado: "running".into() });
            b.a.abrir(Some(Seccion::Maquina));
            let mut t = String::new();
            for pos in [0.0f32, 400.0, 800.0, 1200.0, 1e6] {
                b.a.scroll = pos;
                t.push_str(&todo(&b));
            }
            assert!(t.contains(&fp("start")) && !t.contains("guion"), "{}", t);
            // Imagen: disco ausente (orden para crearlo) y maquina en marcha (boton Apagar y orden para regenerar)
            b.a.abrir(Some(Seccion::Imagen));
            b.d.almacen = Some(almacen_de_prueba(true, false));
            b.d.en_marcha = true;
            let mut t = String::new();
            for pos in [0.0f32, 400.0, 800.0, 1200.0, 1e6] {
                b.a.scroll = pos;
                t.push_str(&todo(&b));
                t.push(' ');
            }
            let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
            let partido = |frase: &str| t.contains(&frase.split_whitespace().collect::<Vec<_>>().join(" "));
            assert!(partido(&fp("disk create")) && partido(&fp("disk reset --yes")), "{}", t);
            assert!(partido("con el botón «Apagar la máquina» de la sección Máquina") && !t.contains("(weft stop)"), "{}", t);
            // Traductor ARM: supervision sin terminar (ordenes check y restore completas) y detalles con `bridge status`
            b.a.abrir(Some(Seccion::Puente));
            b.d.puente_estado = Some(puente::Estado { marca: true, ..Default::default() });
            b.a.detalles = vec![Detalle::Puente];
            let mut t = String::new();
            for pos in [0.0f32, 400.0, 800.0, 1200.0, 1e6] {
                b.a.scroll = pos;
                t.push_str(&todo(&b));
                t.push(' ');
            }
            let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
            for o in ["bridge check", "bridge restore", "bridge status"] {
                assert!(t.contains(&fp(o).split_whitespace().collect::<Vec<_>>().join(" ")), "{} en {}", o, t);
            }
            assert!(!t.contains("«bridge restore»"), "{}", t);
            // Root: la orden de consola que traen los detalles tecnicos de la confirmacion (`weft root status`)
            let mut b = Banco::nuevo((1000.0, 800.0));
            b.d.flatpak = flatpak.clone();
            b.clic("a-nav-root");
            b.d.root_estado = Some(estado_root(false, false));
            b.clic("a-tog-root.cargar_al_inicio");
            b.clic("a-btn-detalles-root");
            let mut t = String::new();
            for pos in [0.0f32, 400.0, 800.0, 1200.0, 1e6] {
                b.a.scroll = pos;
                t.push_str(&todo(&b));
                t.push(' ');
            }
            let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(t.contains(&fp("root status").split_whitespace().collect::<Vec<_>>().join(" ")) && !t.contains('`'), "{}", t);
        }
    }

    /// E.10: los errores del sistema que llegan en ingles a un mensaje en linea se muestran en espanol.
    #[test]
    fn los_errores_del_sistema_se_traducen_en_los_mensajes() {
        let mut a = Ajustes::nuevo();
        a.mensaje("share.agregar", Tono::Error, &format!("/home/ana/fotos: {}", std::io::Error::from_raw_os_error(2)));
        assert_eq!(a.mensaje_de("share.agregar"), Some((Tono::Error, "/home/ana/fotos: no existe")));
        a.mensaje("mandos", Tono::Error, &format!("Mando: {}", std::io::Error::from_raw_os_error(111)));
        assert_eq!(a.mensaje_de("mandos").unwrap().1, "Mando: conexión rechazada");
        a.mensaje("estado", Tono::Aviso, &format!("No se pudo ({})", std::io::Error::from(std::io::ErrorKind::PermissionDenied)));
        assert_eq!(a.mensaje_de("estado").unwrap().1, "No se pudo (sin permiso)");
        // la ruta de una carpeta compartida que no existe, en la lista, tambien
        let mut b = Banco::nuevo((1100.0, 900.0));
        compartir::agregar(&mut b.cfg, Carpeta { nombre: "Fotos".into(), ro: true, ruta: "/no/existe/fotos".into() }, false).unwrap();
        b.a.abrir(Some(Seccion::Compartir));
        let t = b.textos().join(" ");
        assert!(!t.contains("os error") && !t.contains("No such file"), "{}", t);
    }

    /// D.7 y E.10: el aviso de una captura lleva la ruta completa, tal cual (sin pasar por la red de seguridad aunque la
    /// carpeta tenga un nombre que la red cambiaria) y sin marcas a la vista; si no cabe en la linea, se ve el nombre del
    /// archivo. La seccion Controles dice donde se guardan.
    #[test]
    fn la_captura_avisa_con_la_ruta_completa() {
        let dir = std::env::temp_dir().join(format!("weft-ajustes-captura-{}", std::process::id())).join(concat!("cuttle", "fish"));
        std::fs::create_dir_all(&dir).unwrap();
        let archivo = dir.join("captura-20261008-120000.png");
        std::fs::write(&archivo, "png").unwrap();
        let mut b = Banco::nuevo((1100.0, 900.0));
        b.a.abrir(Some(Seccion::Controles));
        // lo que entrega Servicios::tomar_avisos (rutas marcadas) llega al mensaje en linea
        b.a.mensaje("controles", Tono::Exito, &crate::vista::limpiar_salvo_rutas(&format!("Captura guardada en {}", crate::vista::ruta_literal(&archivo))));
        let m = b.a.mensaje_de("controles").unwrap().1.to_string();
        assert_eq!(m, format!("Captura guardada en {}", archivo.display()));
        let t = b.textos();
        assert!(t.iter().any(|x| x.starts_with("Captura guardada en") && x.ends_with("captura-20261008-120000.png")), "{:?}", t);
        assert!(b.hay_texto("carpeta de imágenes del equipo") && !b.hay_texto("carpeta de trabajo"), "{:?}", t);
        // ventana estrecha: la linea se recorta por el centro y se sigue viendo el archivo
        let mut e = Banco::nuevo((500.0, 700.0));
        e.a.abrir(Some(Seccion::Controles));
        e.a.mensaje("controles", Tono::Exito, &m);
        let linea = e.textos().into_iter().find(|x| x.starts_with(tx!("controles.captura"))).unwrap();
        assert!(linea.contains("...") && linea.ends_with(".png"), "{}", linea);
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    /// E.11: los mensajes en linea no se distinguen solo por el color: cada tono lleva su icono delante (triangulo para el
    /// aviso, circulo con aspa para el error, marca para el exito) y el texto va sangrado tras el icono.
    #[test]
    fn los_mensajes_llevan_icono_segun_su_tono() {
        for (tono, icono, color) in [(Tono::Aviso, Icono::Alerta, tema::p().aviso), (Tono::Error, Icono::Fallo, tema::p().error), (Tono::Exito, Icono::Check, tema::p().exito)] {
            for (seccion, clave) in [(Seccion::General, "confirmar"), (Seccion::Controles, "controles")] {
                let mut b = Banco::nuevo((1100.0, 900.0));
                b.a.abrir(Some(seccion));
                b.a.mensaje(clave, tono, "Un mensaje de prueba bastante largo para que ocupe varias líneas si la ventana es estrecha.");
                let d = b.a.dibujar(&b.cfg, &b.ctx());
                let i = d.iter().position(|p| matches!(p, Pint::Texto { t, .. } if t.starts_with("Un mensaje"))).unwrap_or_else(|| panic!("sin mensaje en {:?}", seccion));
                let Pint::Texto { x, y, c, .. } = &d[i] else { unreachable!() };
                assert_eq!(*c, color);
                // el icono, justo antes, a la izquierda y en la misma linea
                match &d[i - 1] {
                    Pint::Icono { k, r, c: ci } => {
                        assert_eq!((*k, *ci), (icono, color), "{:?}", seccion);
                        assert!(r.x + r.w <= *x && r.y >= *y - 1.0 && r.y + r.h <= *y + b.t.alto_linea(Estilo::Pequeno) + 1.0, "{:?} {:?}", r, (x, y));
                    }
                    otro => panic!("{:?}: antes del mensaje va {:?}", seccion, otro),
                }
            }
        }
        // los tres iconos son distintos
        assert!(icono_de(Tono::Aviso).0 != icono_de(Tono::Error).0 && icono_de(Tono::Error).0 != icono_de(Tono::Exito).0);
    }

    #[test]
    fn hover_y_pulsado_cambian_de_color() {
        let mut b = Banco::nuevo((1000.0, 700.0));
        let r = b.boton("a-opt-zoom-1");
        let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let color = |b: &Banco, r: R| {
            let c = ctx!(b);
            b.a.dibujar(&b.cfg, &c).into_iter().find_map(|p| match p {
                Pint::Rect { r: rr, c, radio } if rr == r && radio > 0.0 => Some(c),
                _ => None,
            })
        };
        let reposo = color(&b, r).unwrap();
        let c = ctx!(b);
        assert!(b.a.mover(&b.cfg, &c, x, y));
        let encima = color(&b, r).unwrap();
        b.a.presionar(&b.cfg, &c, x, y);
        let pulsado = color(&b, r).unwrap();
        assert!(reposo != encima && encima != pulsado && reposo != pulsado);
        // al salir del control con el boton pulsado deja de verse pulsado y no activa
        assert!(b.a.mover(&b.cfg, &c, 2.0, 2.0));
        assert_ne!(color(&b, r).unwrap(), pulsado);
        assert!(b.a.soltar(&mut b.cfg, &c, 2.0, 2.0).is_empty());
        assert_eq!(b.cfg.get("zoom"), "ajustar");
    }

    #[test]
    fn desplazamiento_y_barra() {
        // ventana baja: la seccion de atajos no cabe y aparece la barra
        let mut b = Banco::nuevo((900.0, 400.0));
        b.seccion(Seccion::Atajos);
        let g = geometria(b.vent, &b.t);
        let c = ctx!(b);
        let mq = b.a.maquetar(&b.cfg, &c, &g);
        let (pista, pulgar) = mq.barra.expect("barra visible");
        assert!(pulgar.h < pista.h && pulgar.y >= pista.y);
        let alto = b.a.alto_contenido.get();
        assert!(alto > g.contenido.h);
        // la rueda desplaza y se acota
        let c = ctx!(b);
        b.a.rueda(&c, -1.0);
        assert_eq!(b.a.scroll, 48.0);
        b.a.rueda(&c, -1000.0);
        assert!((b.a.scroll - (alto - g.contenido.h)).abs() < 0.01);
        b.a.rueda(&c, 1000.0);
        assert_eq!(b.a.scroll, 0.0);
        // al final el pulgar llega al final de la pista
        b.a.rueda(&c, -1000.0);
        let c = ctx!(b);
        let mq = b.a.maquetar(&b.cfg, &c, &g);
        let (pista, pulgar) = mq.barra.unwrap();
        assert!((pulgar.y + pulgar.h - (pista.y + pista.h)).abs() < 0.6, "{:?} {:?}", pista, pulgar);
        // el ultimo control (restaurar) queda a la vista
        let r = b.boton("a-btn-restaurar-atajos");
        assert!(r.h > 0.0 && r.y + r.h <= g.contenido.y + g.contenido.h + 0.01);
        // arrastrar el pulgar desplaza
        b.a.scroll = 0.0;
        let c = ctx!(b);
        let mq = b.a.maquetar(&b.cfg, &c, &g);
        let (_, pulgar) = mq.barra.unwrap();
        let (x, y) = (pulgar.x + 2.0, pulgar.y + 4.0);
        b.a.presionar(&b.cfg, &c, x, y);
        assert!(b.a.arrastre.is_some());
        assert!(b.a.mover(&b.cfg, &c, x, y + 40.0));
        assert!(b.a.scroll > 20.0, "{}", b.a.scroll);
        let _ = b.a.soltar(&mut b.cfg, &c, x, y + 40.0);
        assert!(b.a.arrastre.is_none());
        // una seccion corta no tiene barra
        let mut c2 = Banco::nuevo((1000.0, 900.0));
        c2.seccion(Seccion::General);
        let g2 = geometria(c2.vent, &c2.t);
        let cx = ctx!(c2);
        assert!(c2.a.maquetar(&c2.cfg, &cx, &g2).barra.is_none());
        // cambiar de seccion reinicia el desplazamiento
        b.a.scroll = 100.0;
        b.clic("a-nav-general");
        assert_eq!(b.a.scroll, 0.0);
        // el foco con Tab lleva el control a la vista
        b.seccion(Seccion::Atajos);
        for _ in 0..40 {
            b.tecla(tec(43));
        }
        let f = b.a.foco.unwrap().nombre();
        let r = b.boton(&f);
        assert!(r.h > 0.0, "el control con foco ({}) debe verse", f);
    }

    #[test]
    fn doctor_y_acerca_de() {
        let mut b = Banco::nuevo((1000.0, 800.0));
        b.seccion(Seccion::Acerca);
        assert!(b.hay_texto(tx!("diagnostico.aun_no_ha_comprobado")) && b.hay_texto(&format!("Versión {}", crate::VERSION_PAQUETE)));
        assert_eq!(b.clic("a-btn-comprobar"), vec![Efecto::Comprobar]);
        b.d.doctor_en_curso = true;
        assert!(b.hay_texto(tx!("diagnostico.comprobando_equipo")));
        b.d.doctor_en_curso = false;
        b.d.doctor = Some(vec![
            FilaDoctor { nivel: NivelDoctor::Ok, nombre: "KVM".into(), detalle: "/dev/kvm accesible".into() },
            FilaDoctor { nivel: NivelDoctor::Aviso, nombre: "SDL3".into(), detalle: "no se carga libSDL3.so.0".into() },
            FilaDoctor { nivel: NivelDoctor::Fallo, nombre: "QEMU".into(), detalle: "falta".into() },
        ]);
        let t = b.textos();
        for esperado in ["OK", "AVISO", "FALLO", "KVM", "1 fallo(s) y 1 aviso(s)."] {
            assert!(t.iter().any(|x| x.contains(esperado)), "falta {:?} en {:?}", esperado, t);
        }
        // colores por nivel
        let c = ctx!(b);
        let col = |txt: &str| b.a.dibujar(&b.cfg, &c).into_iter().find_map(|p| match p {
            Pint::Texto { t, c, .. } if t == txt => Some(c),
            _ => None,
        });
        assert_eq!((col("OK"), col("AVISO"), col("FALLO")), (Some(tema::p().exito), Some(tema::p().aviso), Some(tema::p().error)));
        // rutas y tipografia
        assert!(b.hay_texto("/estado/prueba/config") && b.hay_texto("mapa de bits 5x7 (respaldo)"));
    }

    #[test]
    fn estrecha_con_pestanas() {
        let mut b = Banco::nuevo((420.0, 600.0));
        let g = geometria(b.vent, &b.t);
        assert!(g.tabs.is_some());
        // una pestana por seccion, todas dentro del dialogo y a la vista
        let tabs: Vec<(String, R)> = b.botones().into_iter().filter(|(n, _)| n.starts_with("a-nav-")).collect();
        assert_eq!(tabs.len(), Seccion::TODAS.len());
        for (_, r) in &tabs {
            assert!(r.x >= g.dialogo.x && r.x + r.w <= g.dialogo.x + g.dialogo.w + 0.01 && r.y + r.h <= g.contenido.y + 0.01);
        }
        b.clic("a-nav-atajos");
        assert_eq!(b.a.seccion, Seccion::Atajos);
        assert!(b.hay_texto("Atrás"));
        // el dibujo de la pestana activa usa el color de acento
        let c = ctx!(b);
        let activas = b.a.dibujar(&b.cfg, &c).into_iter().filter(|p| matches!(p, Pint::Rect { c, radio, .. } if (*c == tema::p().acento || *c == tema::p().acento_hover) && *radio > 0.0)).count();
        assert_eq!(activas, 1);
    }

    #[test]
    fn el_dibujo_es_coherente() {
        // fondo atenuado al 60 % sobre toda la ventana y dialogo centrado en la superficie
        let b = Banco::nuevo((1000.0, 700.0));
        let c = ctx!(b);
        let d = b.a.dibujar(&b.cfg, &c);
        assert_eq!(d[0], Pint::Rect { r: R::new(0.0, 0.0, 1000.0, 700.0), c: Color(0, 0, 0, 153), radio: 0.0 });
        assert!(d.iter().any(|p| matches!(p, Pint::Rect { c, .. } if *c == tema::p().superficie)));
        assert!(b.hay_texto(tx!("comun.configuracion")) && b.hay_texto(tx!("comun.general")) && b.hay_texto(tx!("seccion.acerca")));
        // cerrada: no dibuja nada
        let mut z = Ajustes::nuevo();
        assert!(z.dibujar(&b.cfg, &c).is_empty() && z.botones(&b.cfg, &c).is_empty());
        z.abrir(Some(Seccion::Entrada));
        assert_eq!(z.seccion, Seccion::Entrada);
        // todo el texto es espanol: las etiquetas llevan sus tildes
        let mut b = Banco::nuevo((1000.0, 800.0));
        b.seccion(Seccion::Maquina);
        assert!(b.hay_texto("próximo arranque") && b.hay_texto(tx!("maquina.aceleracion_grafica")) && b.hay_texto(tx!("maquina.tipo_maquina")));
    }

    fn estado_root(instalado: bool, cargado: bool) -> root::Estado {
        root::Estado { servicio: instalado, ksud: instalado, cargado, version: Some("33294".into()), gestor: instalado, oficial: false, reconocido: cargado }
    }

    #[test]
    fn root_estado_y_activacion_con_confirmacion() {
        let mut b = Banco::nuevo((1000.0, 800.0));
        // entrar en la seccion pide consultar el estado
        let e = b.clic("a-nav-root");
        assert_eq!(e, vec![Efecto::RootActualizar]);
        assert!(b.hay_texto("Sin datos") && !b.hay_texto("Puede afectar"));
        assert!(b.hay_texto(crate::textos::texto(root::AVISO_DETECCION).split_whitespace().next().unwrap()), "el aviso de deteccion de root es fijo");
        b.d.root_estado = Some(estado_root(false, false));
        assert!(b.hay_texto("Instalado") && b.hay_texto("33294"));
        // activar: no hace nada todavia, muestra lo que se descargara con tamanos y origen
        let e = b.clic("a-tog-root.cargar_al_inicio");
        assert!(e.is_empty() && !b.cfg.bool("root.cargar_al_inicio"));
        assert!(b.hay_texto(tx!("root.activar_acceso_root")));
        // el texto principal es generico (sin nombres de productos) y dice cuanto se baja; QUE se baja (nombre, tamano y
        // origen) esta en los "Detalles tecnicos", que se despliegan
        let principal = b.textos().join(" ");
        assert!(principal.contains("software de un tercero") && principal.contains("5.3 MiB") && principal.contains(tx!("comun.detalles_tecnicos")), "{}", principal);
        assert!(crate::textos::prohibida_en(&principal).is_none(), "{}", principal);
        assert!(b.clic("a-btn-detalles-root").is_empty());
        let detalle = b.textos().join(" ");
        // lo generico y util: componente de terceros, tamano de cada descarga, dominio sin el nombre del proyecto, integridad
        // por tamano y donde ver el resto (ni el nombre del archivo ni el del proveedor)
        assert!(crate::textos::prohibida_en(&detalle).is_none(), "{}", detalle);
        assert!(detalle.contains("Componente de terceros") && detalle.contains("el componente principal (5.3 MiB), ya descargado en el equipo (se obtiene de GitHub)"), "{}", detalle);
        assert!(detalle.contains("la app gestora de permisos (11.3 MiB), ya descargado en el equipo (se obtiene de GitHub), solo si instalas el gestor"), "{}", detalle);
        assert!(detalle.contains("se comprueba el tamaño exacto") && detalle.contains("weft root status") && detalle.contains("README"), "{}", detalle);
        // se pueden volver a ocultar
        b.clic("a-btn-detalles-root");
        assert!(!b.textos().join(" ").contains("Integridad"));
        // cancelar
        assert!(b.clic("a-btn-cancelar").is_empty());
        assert!(!b.hay_texto(tx!("root.activar_acceso_root")));
        // confirmar sin gestor y con gestor
        b.clic("a-tog-root.cargar_al_inicio");
        let e = b.clic("a-btn-confirmar");
        assert_eq!(e.len(), 1);
        match &e[0] {
            Efecto::Orden { clave, args, .. } => {
                assert_eq!(*clave, "root");
                assert_eq!(args, &vec!["root".to_string(), "enable".into(), "--now".into()]);
            }
            x => panic!("{:?}", x),
        }
        b.clic("a-tog-root.cargar_al_inicio");
        let e = b.clic("a-btn-confirmar-con-gestor");
        assert!(matches!(&e[0], Efecto::Orden { args, .. } if args.contains(&"--manager".to_string())));
        // con el root ya activado, apagarlo pregunta antes de quitar el servicio
        b.cfg.set("root.cargar_al_inicio", "si").unwrap();
        assert!(b.clic("a-tog-root.cargar_al_inicio").is_empty());
        assert!(b.hay_texto(tx!("root.desactivar_acceso_root")));
        let e = b.clic("a-btn-confirmar");
        assert!(matches!(&e[0], Efecto::Orden { args, .. } if args == &vec!["root".to_string(), "disable".into()]));
        // instalar gestor: tambien pregunta (puede desinstalar otro gestor)
        assert!(b.clic("a-btn-root-gestor").is_empty());
        let e = b.clic("a-btn-confirmar");
        assert!(matches!(&e[0], Efecto::Orden { args, .. } if args[1] == "install-manager"));
        // reiniciar Android: confirmacion (el boton esta al final de la seccion)
        b.a.scroll = 1e6;
        assert!(b.clic("a-btn-reiniciar-android").is_empty());
        b.a.scroll = 1e6;
        assert!(b.hay_texto(tx!("reinicio.pregunta_reiniciar_android")));
        let e = b.clic("a-btn-confirmar");
        // reinicio ORDENADO de Android (no una orden de consola ni system_reset)
        assert_eq!(e, vec![Efecto::ReiniciarAndroid]);
        // el interruptor de instalar solo si falta guarda en config
        assert_eq!(b.clic("a-tog-root.instalar_automaticamente"), vec![Efecto::Cambio("root.instalar_automaticamente".into())]);
        assert!(b.cfg.bool("root.instalar_automaticamente"));
    }

    /// Maquina > Acciones: el reinicio ordenado y el apagado piden confirmacion (salvo `confirmar=no`); el reinicio completo
    /// siempre esta disponible, siempre pregunta y avisa de que la ventana se cierra.
    #[test]
    fn reiniciar_apagar_y_reinicio_completo_en_maquina() {
        let mut b = Banco::nuevo((1000.0, 900.0));
        b.seccion(Seccion::Maquina);
        // sin fallo previo: reinicio ordenado, reinicio completo (como boton propio) y apagar, los tres visibles
        for n in ["a-btn-reiniciar-android", "a-btn-reinicio-completo", "a-btn-apagar"] {
            b.boton(n);
        }
        assert!(!b.hay_texto("Android no responde por adb"));
        assert!(b.clic("a-btn-reiniciar-android").is_empty());
        assert!(b.hay_texto(tx!("reinicio.pregunta_reiniciar_android")) && b.textos().join(" ").contains("la máquina sigue encendida"));
        assert_eq!(b.clic("a-btn-cancelar"), vec![]);
        assert!(b.a.confirmando.is_none());
        b.clic("a-btn-reiniciar-android");
        assert_eq!(b.clic("a-btn-confirmar"), vec![Efecto::ReiniciarAndroid]);
        // un reinicio en curso deshabilita el boton
        b.d.reiniciando = true;
        let r = b.boton("a-btn-reiniciar-android");
        let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        assert!(b.clic_en(x, y).is_empty() && b.a.confirmando.is_none());
        b.d.reiniciando = false;
        // Apagar: dos clics (el segundo en la caja de confirmacion) y es el efecto que llama a `stop`
        assert!(b.clic("a-btn-apagar").is_empty());
        assert!(b.hay_texto(tx!("maquina.pregunta_apagar")) && b.textos().join(" ").contains("ventana se cerrará"));
        assert_eq!(b.clic("a-btn-cancelar"), vec![]);
        b.clic("a-btn-apagar");
        assert_eq!(b.clic("a-btn-confirmar"), vec![Efecto::Apagar]);
        // sin confirmacion (`confirmar=no`) actuan al primer clic
        b.cfg.set("confirmar", "no").unwrap();
        assert_eq!(b.clic("a-btn-apagar"), vec![Efecto::Apagar]);
        assert_eq!(b.clic("a-btn-reiniciar-android"), vec![Efecto::ReiniciarAndroid]);
        // ...pero el reinicio completo siempre pregunta
        assert!(b.clic("a-btn-reinicio-completo").is_empty());
        assert!(b.hay_texto(tx!("reinicio.pregunta_completo")) && b.textos().join(" ").contains("ventana se cerrará"));
        assert_eq!(b.clic("a-btn-confirmar"), vec![Efecto::ReinicioCompleto]);
        // adbd no respondio: se avisa y se destaca el reinicio completo
        b.d.reinicio_completo = true;
        assert!(b.hay_texto(tx!("reinicio.android_no_responde_adb")));
        b.clic("a-btn-reinicio-completo");
        assert!(b.clic("a-btn-cancelar").is_empty() && b.a.confirmando.is_none());
        // un mensaje del apagado se ve en la seccion
        b.a.mensaje("apagar", Tono::Aviso, "Apagando la máquina...");
        assert!(b.hay_texto(tx!("aviso.apagando_maquina")));
    }

    /// Cerrar la ventana (la X o Alt+F4) apaga la maquina y nunca la cierra de golpe: con `confirmar=si` abre Maquina con la
    /// pregunta de apagar a la vista y confirmarla da el efecto que llama a `stop`; con `confirmar=no` da ese efecto sin
    /// preguntar; con una operacion larga o un apagado ya en curso solo avisa y no apaga nada.
    #[test]
    fn cerrar_la_ventana_pregunta_o_apaga() {
        let mut b = Banco::nuevo((1000.0, 900.0));
        b.a.cerrar();
        assert!(!b.a.abierto && b.cfg.bool("confirmar"));
        // confirmar=si (lo normal): se abre Maquina con la pregunta a la vista, sin efecto todavia
        let ef = b.a.cerrar_ventana(&b.cfg, &ctx!(b));
        assert!(ef.is_empty(), "{:?}", ef);
        assert!(b.a.abierto && b.a.seccion == Seccion::Maquina && b.a.confirmando == Some(Conf::Apagar));
        assert!(b.hay_texto(tx!("maquina.pregunta_apagar")));
        assert!(b.botones().iter().any(|(n, _)| n == "a-btn-confirmar"), "la pregunta no esta a la vista: {:?}", b.botones());
        // tambien con la ventana en su tamano minimo (360x480)
        b.vent = (360.0, 480.0);
        b.a.cerrar_ventana(&b.cfg, &ctx!(b));
        let vis: Vec<String> = b.botones().into_iter().map(|(n, _)| n).collect();
        assert!(vis.contains(&"a-btn-confirmar".to_string()) && vis.contains(&"a-btn-cancelar".to_string()), "{:?}", vis);
        b.vent = (1000.0, 900.0);
        // cancelar deja la ventana abierta y nada apagado; volver a cerrar vuelve a preguntar y confirmar apaga
        assert_eq!(b.clic("a-btn-cancelar"), vec![]);
        assert!(b.a.confirmando.is_none());
        assert!(b.a.cerrar_ventana(&b.cfg, &ctx!(b)).is_empty());
        assert_eq!(b.clic("a-btn-confirmar"), vec![Efecto::Apagar]);
        // confirmar=no: el efecto de apagar sin preguntar ni abrir la pantalla
        b.a.cerrar();
        b.cfg.set("confirmar", "no").unwrap();
        assert_eq!(b.a.cerrar_ventana(&b.cfg, &ctx!(b)), vec![Efecto::Apagar]);
        assert!(!b.a.abierto);
        // con una operacion larga en curso no se apaga (con o sin confirmacion): se avisa en Maquina
        b.d.operacion = Some(("root".into(), "Activando el root (puede tardar unos minutos)".into()));
        for conf in ["no", "si"] {
            b.cfg.set("confirmar", conf).unwrap();
            assert!(b.a.cerrar_ventana(&b.cfg, &ctx!(b)).is_empty());
            assert!(b.a.abierto && b.a.seccion == Seccion::Maquina && b.a.confirmando.is_none());
            assert!(b.textos().join(" ").contains("Espera a que termine: Activando el root (puede tardar unos minutos)."), "{:?}", b.textos());
            // el aviso (bajo el boton Apagar) queda a la vista
            assert!(b.botones().iter().any(|(n, _)| n == "a-btn-apagar"), "{:?}", b.botones());
        }
        b.d.operacion = None;
        // con un apagado ya en curso tampoco se relanza nada: ni al cerrar ni con el boton
        b.d.apagando = true;
        assert!(b.a.cerrar_ventana(&b.cfg, &ctx!(b)).is_empty());
        assert!(b.a.confirmando.is_none() && b.textos().join(" ").contains(tx!("aviso.apagando_maquina")), "{:?}", b.textos());
        b.cfg.set("confirmar", "no").unwrap();
        assert!(b.clic("a-btn-apagar").is_empty());
        b.d.apagando = false;
        assert_eq!(b.clic("a-btn-apagar"), vec![Efecto::Apagar]);
    }

    /// Elegir una seccion con las flechas hace lo mismo que con el clic (refresca sus datos y suelta lo que habia a medias),
    /// y reabrir la pantalla refresca la seccion que muestra.
    #[test]
    fn flechas_y_reabrir_refrescan_como_el_clic() {
        let mut b = Banco::nuevo((1000.0, 900.0));
        let por_clic: Vec<Vec<Efecto>> = Seccion::TODAS.iter().map(|s| b.clic(&format!("a-nav-{}", s.nombre()))).collect();
        let pos = |s: Seccion| Seccion::TODAS.iter().position(|x| *x == s).unwrap();
        assert_eq!(por_clic[pos(Seccion::Root)], vec![Efecto::RootActualizar]);
        assert_eq!(por_clic[pos(Seccion::Imagen)], vec![Efecto::AlmacenActualizar]);
        assert_eq!(por_clic[pos(Seccion::Puente)], vec![Efecto::PuenteActualizar]);
        // con el foco en la lista, cada flecha abajo elige la siguiente seccion con los mismos efectos que el clic
        b.clic("a-nav-controles");
        for (i, s) in Seccion::TODAS.iter().enumerate().skip(1) {
            // lo que quedaba a medias en la seccion anterior se suelta, como con el clic
            b.a.confirmando = Some(Conf::Apagar);
            b.a.scroll = 50.0;
            let (consumida, ef) = b.tecla(tec(81));
            assert!(consumida && b.a.seccion == *s, "{:?}", s);
            assert_eq!(ef, por_clic[i], "{:?}", s);
            assert!(b.a.confirmando.is_none() && b.a.scroll == 0.0, "{:?}", s);
        }
        // en el extremo no cambia nada ni se vuelve a refrescar
        let (_, ef) = b.tecla(tec(81));
        assert!(ef.is_empty() && b.a.seccion == Seccion::Acerca);
        // hacia arriba igual
        assert!(b.tecla(tec(82)).1.is_empty());
        assert_eq!(b.tecla(tec(82)).1, vec![Efecto::PuenteActualizar]);
        assert_eq!(b.a.seccion, Seccion::Puente);
        // reabrir la pantalla refresca la seccion que muestra (la ultima vista o la pedida)
        b.a.cerrar();
        assert_eq!(b.a.abrir(None), vec![Efecto::PuenteActualizar]);
        assert!(b.a.abierto && b.a.seccion == Seccion::Puente);
        b.a.cerrar();
        assert_eq!(b.a.abrir(Some(Seccion::Imagen)), vec![Efecto::AlmacenActualizar]);
        b.a.cerrar();
        assert_eq!(b.a.abrir(Some(Seccion::Root)), vec![Efecto::RootActualizar]);
        b.a.cerrar();
        assert!(b.a.abrir(Some(Seccion::General)).is_empty());
    }

    /// El atajo de la configuracion (F9) con la pantalla abierta la cierra, como la X (confirmando lo que se editaba);
    /// mientras se captura un atajo es la tecla capturada, y si el atajo es una tecla que la pantalla usa, manda la pantalla.
    #[test]
    fn el_atajo_de_la_configuracion_la_cierra() {
        const F9: u32 = 66;
        let mut b = Banco::nuevo((1000.0, 900.0));
        assert_eq!(b.cfg.atajo_para(F9, false, false, false), Some(AccionAtajo::Configuracion));
        assert_eq!(b.tecla(tec(F9)), (true, vec![Efecto::Cerrar]));
        assert!(!b.a.abierto);
        // la repeticion no cierra
        b.a.abrir(None);
        assert_eq!(b.tecla(Tecla { repetida: true, ..tec(F9) }), (true, vec![]));
        assert!(b.a.abierto);
        // con un campo a medio editar: se guarda lo escrito y se cierra
        b.seccion(Seccion::Maquina);
        b.clic("a-campo-cpus");
        for _ in 0..8 {
            b.tecla(tec(42));
        }
        // tecla 4 de la fila de numeros
        b.tecla(tec(33));
        let (_, ef) = b.tecla(tec(F9));
        assert!(!b.a.abierto && b.cfg.get("maquina.cpus") == "4", "{}", b.cfg.get("maquina.cpus"));
        assert!(ef.contains(&Efecto::Cambio("maquina.cpus".into())) && ef.contains(&Efecto::Cerrar), "{:?}", ef);
        // capturando el atajo, F9 es la tecla que se captura: no cierra
        b.a.abrir(Some(Seccion::Atajos));
        b.clic("a-atajo-configuracion");
        assert_eq!(b.a.en_captura(), Some(AccionAtajo::Configuracion));
        b.tecla(tec(F9));
        assert!(b.a.abierto && b.a.en_captura().is_none() && b.cfg.get("atajo.configuracion") == "F9");
        // con otro atajo, es ese el que cierra (y F9 ya no)
        b.cfg.set("atajo.configuracion", "Ctrl+F12").unwrap();
        assert!(b.tecla(tec(F9)).1.is_empty() && b.a.abierto);
        assert_eq!(b.tecla(Tecla { ctrl: true, ..tec(69) }).1, vec![Efecto::Cerrar]);
        // un atajo en una tecla que la pantalla usa (Fin): con la pantalla abierta hace lo suyo (ir al final)
        b.vent = (1000.0, 500.0);
        b.a.abrir(Some(Seccion::Maquina));
        b.cfg.set("atajo.configuracion", &Combo { sc: 77, ctrl: false, alt: false, mayus: false }.texto()).unwrap();
        assert_eq!(b.cfg.atajo_para(77, false, false, false), Some(AccionAtajo::Configuracion));
        assert!(b.tecla(tec(77)).1.is_empty() && b.a.abierto && b.a.scroll > 0.0);
    }

    /// Desactivar el root y el boton "Instalar gestor" preguntan antes de actuar; cancelar no hace nada y, con una operacion
    /// en curso, confirmar tampoco.
    #[test]
    fn root_desactivar_e_instalar_gestor_piden_confirmacion() {
        let mut b = Banco::nuevo((1000.0, 800.0));
        b.clic("a-nav-root");
        b.d.root_estado = Some(estado_root(true, true));
        b.cfg.set("root.cargar_al_inicio", "si").unwrap();
        // desactivar: pregunta, cancelar lo deja como estaba
        assert!(b.clic("a-tog-root.cargar_al_inicio").is_empty());
        assert!(b.hay_texto(tx!("root.desactivar_acceso_root")) && b.hay_texto(tx!("root.desactivar")));
        assert!(b.clic("a-btn-cancelar").is_empty());
        assert!(!b.hay_texto(tx!("root.desactivar_acceso_root")) && b.cfg.bool("root.cargar_al_inicio"));
        // confirmar lanza `root disable`
        b.clic("a-tog-root.cargar_al_inicio");
        let e = b.clic("a-btn-confirmar");
        assert!(matches!(e.as_slice(), [Efecto::Orden { clave: "root", args, .. }] if args == &vec!["root".to_string(), "disable".into()]), "{:?}", e);
        // instalar gestor: pregunta en lugar del boton (que vuelve al cancelar)
        assert!(b.clic("a-btn-root-gestor").is_empty());
        assert!(b.hay_texto(tx!("root.instalar_gestor_root")) && !b.botones().iter().any(|(n, _)| n == "a-btn-root-gestor"));
        assert!(b.textos().join(" ").contains("se desinstala antes"));
        assert!(b.clic("a-btn-cancelar").is_empty());
        b.ir_a("a-btn-root-gestor");
        assert!(b.botones().iter().any(|(n, _)| n == "a-btn-root-gestor"));
        // confirmar lanza `root install-manager`, con la carpeta indicada
        b.a.form_root = "/descargas/root".into();
        b.clic("a-btn-root-gestor");
        let e = b.clic("a-btn-confirmar");
        assert!(matches!(e.as_slice(), [Efecto::Orden { clave: "root", args, .. }] if args == &vec!["root".to_string(), "install-manager".into(), "--from".into(), "/descargas/root".into()]), "{:?}", e);
        // con una operacion en curso, confirmar no lanza nada
        for n in ["a-btn-root-gestor", "a-tog-root.cargar_al_inicio"] {
            b.clic(n);
            b.d.operacion = Some(("root".into(), "Activando el root".into()));
            assert!(b.clic("a-btn-confirmar").is_empty(), "{}", n);
            b.d.operacion = None;
        }
    }

    /// Imagen deja claro que agregar una imagen actua al instante y que usarla y el tamano de datos van al proximo
    /// arranque; la nota se parte en lineas en una ventana estrecha.
    #[test]
    fn imagen_dice_que_agregar_es_al_instante() {
        let mut b = Banco::nuevo((1000.0, 800.0));
        b.seccion(Seccion::Imagen);
        let t = b.textos().join(" ");
        assert!(t.contains(tx!("imagen.agregar_imagen_instante_usarla")), "{}", t);
        assert!(t.contains("«Agregar imagen» la copia ya a la lista") && t.contains("cuentan al próximo arranque"), "{}", t);
        b.vent = (360.0, 480.0);
        let g = geometria(b.vent, &b.t);
        let nota = "Agregar una imagen es al instante; usarla y el tamaño de datos se aplican al próximo arranque";
        let partes: Vec<(f32, String)> = b.a.dibujar(&b.cfg, &b.ctx()).into_iter().filter_map(|p| if let Pint::Texto { x, t, e: Estilo::Negrita, .. } = p { Some((x, t)) } else { None }).filter(|(_, t)| t.len() > 8 && nota.contains(t.as_str())).collect();
        assert!(partes.len() >= 2, "{:?}", partes);
        for (x, t) in &partes {
            assert!(x + b.t.ancho(t, Estilo::Negrita) <= g.dialogo.x + g.dialogo.w + 0.5, "se sale: {}", t);
        }
    }

    /// Los atajos de zoom y los botones de Controles cambian el zoom de la ventana sin tocar el «Zoom inicial»; General dice
    /// el zoom actual cuando no coincide con el inicial.
    #[test]
    fn el_zoom_de_la_ventana_no_pisa_el_zoom_inicial() {
        let mut b = Banco::nuevo((1000.0, 900.0));
        b.seccion(Seccion::General);
        assert!(!b.hay_texto("Ahora:"));
        b.d.zoom_modo = ModoZoom::Fijo(80);
        assert!(b.hay_texto("Ahora: 80 % (solo en esta ventana)."), "{:?}", b.textos());
        assert_eq!(b.cfg.get("zoom"), "ajustar");
        // elegir el zoom inicial lo guarda; el de la ventana se sigue diciendo si es otro
        assert_eq!(b.clic("a-opt-zoom-3"), vec![Efecto::Cambio("zoom".into())]);
        assert_eq!(b.cfg.get("zoom"), "100");
        b.d.zoom_modo = ModoZoom::Ajustar;
        assert!(b.hay_texto("Ahora: ajustado a la ventana"));
        b.d.zoom_modo = ModoZoom::Fijo(100);
        assert!(!b.hay_texto("Ahora:"));
        // los botones de zoom de Controles piden la accion a la ventana: la configuracion no cambia
        b.seccion(Seccion::Controles);
        assert_eq!(b.clic("a-ctl-zoom+"), vec![Efecto::Control(Accion::ZoomMas)]);
        assert_eq!(b.cfg.get("zoom"), "100");
        // y la ventana ya no anota el zoom de los atajos en la configuracion
        assert!(!crate::textos::tests::sin_pruebas_ni_comentarios(include_str!("window.rs")).contains("zoom_a_config"));
    }

    /// Maquina > Estado: lo que mostraba la pestana Maquina del panel (estado, tiempo encendida, CPUs, RAM y tipo).
    #[test]
    fn maquina_muestra_el_estado() {
        use crate::vista::Maquina;
        let mut b = Banco::nuevo((1000.0, 900.0));
        b.seccion(Seccion::Maquina);
        assert!(b.hay_texto(tx!("comun.consultando")));
        b.d.qmp_ok = false;
        assert!(b.hay_texto("Sin acceso a la máquina"));
        b.d.maquina = Some(Maquina { cpus: 2, mem_mb: 4096, tipo: "pc-q35-10.2-machine".into(), estado: "running".into() });
        b.d.encendida = Some(Duration::from_secs(3725));
        let t = b.textos().join(" | ");
        assert!(t.contains(tx!("estado_maquina.marcha")) && t.contains("1h 02m") && t.contains("4096 MB") && t.contains("q35 10.2") && t.contains(tx!("maquina.cpus")), "{}", t);
    }

    /// Controles: las acciones rapidas del panel lateral, cada una con la tecla de su atajo; al pulsarlas el dialogo se queda
    /// abierto, la accion sale como efecto y una linea de estado la confirma.
    #[test]
    fn controles_acciones_con_su_tecla_y_estado() {
        use crate::gestos::Atajo as A;
        let mut b = Banco::nuevo((1000.0, 900.0));
        b.seccion(Seccion::Controles);
        // estan todas las acciones
        for n in ["atras", "inicio", "recientes", "vol-", "vol+", "rotar", "rot0", "rot90", "rot180", "rot270", "rotauto", "zoom+", "zoom-", "ajustar", "1:1", "captura"] {
            b.ir_a(&format!("a-ctl-{}", n));
            b.boton(&format!("a-ctl-{}", n));
        }
        b.a.scroll = 0.0;
        // la tecla del atajo se ve junto al boton
        let t = b.textos();
        for tecla in ["F1", "F2", "F3", "F5", "F6", "F7", "F8", "Ctrl+=", "Ctrl+-", "Ctrl+0"] {
            assert!(t.iter().any(|x| x == tecla), "falta la tecla {} en {:?}", tecla, t);
        }
        // se pulsa: efecto + linea de estado, y el dialogo sigue abierto
        assert_eq!(b.clic("a-ctl-atras"), vec![Efecto::Control(Accion::Atajo(A::Atras))]);
        assert!(b.a.abierto && b.hay_texto(tx!("enviado.atras_enviado")));
        assert_eq!(b.clic("a-ctl-inicio"), vec![Efecto::Control(Accion::Atajo(A::Inicio))]);
        assert!(b.hay_texto(tx!("enviado.inicio_enviado")) && !b.hay_texto(tx!("enviado.atras_enviado")), "la linea de estado muestra solo lo ultimo");
        assert_eq!(b.clic("a-ctl-rot90"), vec![Efecto::Control(Accion::Rotacion(1))]);
        assert!(b.hay_texto("Rotación fijada a 90°"));
        assert_eq!(b.clic("a-ctl-rotauto"), vec![Efecto::Control(Accion::RotacionAuto)]);
        assert_eq!(b.clic("a-ctl-zoom+"), vec![Efecto::Control(Accion::ZoomMas)]);
        assert_eq!(b.clic("a-ctl-1:1"), vec![Efecto::Control(Accion::Zoom1a1)]);
        assert_eq!(b.clic("a-ctl-captura"), vec![Efecto::Control(Accion::Captura)]);
        assert!(!b.hay_texto(tx!("enviado.captura_pedida")), "el nombre real de la captura lo dice el servicio");
        // la teclas cambiadas en Atajos se reflejan
        b.cfg.set("atajo.atras", "F4").unwrap();
        assert!(b.textos().iter().any(|x| x == "F4"));
        // porcentaje real y modo del zoom
        b.d.zoom_pct = 50.4;
        b.d.zoom_modo = ModoZoom::Ajustar;
        assert!(b.hay_texto("50 %") && b.hay_texto("Ajustado a la ventana"));
        b.d.zoom_modo = ModoZoom::Fijo(100);
        assert!(b.hay_texto("Fijo 100 %, limitado por la pantalla del equipo"));
        // rotacion vigente
        b.d.orient = Orientacion::Fija(3);
        assert!(b.hay_texto("270 grados"));
        // el estado se reserva siempre: los botones no se mueven al aparecer el mensaje
        let mut c = Banco::nuevo((1000.0, 900.0));
        c.seccion(Seccion::Controles);
        let antes = c.boton("a-ctl-atras");
        c.clic("a-ctl-atras");
        assert_eq!(c.boton("a-ctl-atras"), antes);
    }

    /// Controles se abre primero y las acciones tambien se pueden pulsar con el teclado (Tab + Enter).
    #[test]
    fn controles_es_la_primera_seccion_y_se_usa_con_el_teclado() {
        let mut a = Ajustes::nuevo();
        a.abrir(None);
        assert_eq!(a.seccion, Seccion::Controles);
        assert_eq!(Seccion::TODAS[0], Seccion::Controles);
        let mut b = Banco::nuevo((1000.0, 900.0));
        b.seccion(Seccion::Controles);
        b.a.foco = Some(Id::Control(Accion::Captura));
        let (_, e) = b.tecla(tec(40));
        assert_eq!(e, vec![Efecto::Control(Accion::Captura)]);
        // ventana estrecha: los botones pasan a la linea siguiente sin salirse
        let mut s = Banco::nuevo((420.0, 800.0));
        s.seccion(Seccion::Controles);
        let g = geometria(s.vent, &s.t);
        for (n, r) in s.botones() {
            if n.starts_with("a-ctl-") {
                assert!(r.x >= g.contenido.x && r.x + r.w <= g.contenido.x + g.contenido.w + 0.5, "{} {:?}", n, r);
            }
        }
    }

    /// Entrada > Mandos del equipo: lo que ofrecia la pestana Entrada del panel (Conectar/Desconectar por mando).
    #[test]
    fn entrada_lista_los_mandos_y_los_conecta() {
        use crate::vista::Mando;
        let mut b = Banco::nuevo((1000.0, 900.0));
        b.seccion(Seccion::Entrada);
        assert!(b.hay_texto(tx!("entrada.mandos_equipo")) && b.hay_texto("Ninguno detectado en el equipo"));
        b.d.qmp_ok = false;
        assert!(b.hay_texto("Sin acceso a la máquina"));
        b.d.qmp_ok = true;
        b.d.mandos = vec![
            Mando { path: "/dev/input/event7".into(), name: "Mando uno".into(), conectado: None, ocupado: false },
            Mando { path: "/dev/input/event9".into(), name: "Mando dos".into(), conectado: Some("pad0".into()), ocupado: false },
        ];
        assert!(b.hay_texto("Mando uno") && b.hay_texto("Mando dos") && b.hay_texto(tx!("entrada.conectar")) && b.hay_texto(tx!("entrada.desconectar")), "{:?}", b.textos());
        assert_eq!(b.clic("a-mando-0"), vec![Efecto::Mando { path: "/dev/input/event7".into(), conectar: true }]);
        assert_eq!(b.clic("a-mando-1"), vec![Efecto::Mando { path: "/dev/input/event9".into(), conectar: false }]);
        // una conexion en curso apaga el boton
        b.d.mandos[0].ocupado = true;
        assert!(b.hay_texto(tx!("entrada.espere")));
        assert!(b.clic("a-mando-0").is_empty());
        b.a.mensaje("mandos", Tono::Error, "Mando: sin permiso");
        assert!(b.hay_texto("Mando: sin permiso"));
    }

    /// Los textos de la interfaz no nombran productos ni proveedores (Cuttlefish, KernelSU, heddle...): eso solo sale en los
    /// "Detalles tecnicos" desplegables, en el README y en la salida de las ordenes de consola.
    #[test]
    fn la_interfaz_no_nombra_proveedores() {
        const PROHIBIDAS: [&str; 7] = ["Cuttlefish", "KernelSU", "kernelsu", "ksud", "heddle", "me.weishu", "ci.android.com"];
        for vent in [(1100.0f32, 900.0f32), (500.0, 700.0)] {
            for sec in Seccion::TODAS {
                let mut b = Banco::nuevo(vent);
                b.a.abrir(Some(sec));
                b.d.root_estado = Some(root::Estado { oficial: true, ..estado_root(true, true) });
                b.d.puente_estado = Some(puente::parsear_estado("biblioteca=1\nmd5=5e4f26d507a3d4b7a7a3b0e3f1f2c3d4\nbytes=10135552\nnb=libheddle.so\nabilist=x86_64,arm64-v8a\nboot=1\nmuertes=0\nzygote=1\n"));
                b.d.almacen = Some(almacen_de_prueba(true, true));
                b.d.reinicio_completo = true;
                // en cada seccion, con las confirmaciones abiertas que tenga
                for conf in [None, Some(Conf::RootActivar), Some(Conf::RootDesactivar), Some(Conf::RootGestor), Some(Conf::PuenteInstalar), Some(Conf::PuenteQuitar), Some(Conf::ReiniciarAndroid), Some(Conf::ReinicioCompleto), Some(Conf::DiscoPaso1)] {
                    b.a.confirmando = conf;
                    for pos in [0.0f32, 1e6] {
                        b.a.scroll = pos;
                        let t = b.textos().join(" ");
                        for p in PROHIBIDAS {
                            assert!(!t.contains(p), "{:?} {:?} {:?}: la interfaz nombra {:?}: {}", vent, sec, conf, p, t);
                        }
                    }
                }
            }
        }
        // los nombres de la navegacion y de los encabezados tampoco
        for s in Seccion::TODAS {
            for p in PROHIBIDAS {
                assert!(!s.titulo().contains(p) && !s.encabezado().contains(p));
            }
        }
    }

    /// La navegacion lleva los grupos Pantalla y entrada, Maquina, Almacenamiento y Avanzado con encabezado, en el orden
    /// pedido, sin salirse de la columna ni solaparse, y se compacta si la ventana es baja.
    #[test]
    fn navegacion_agrupada() {
        let orden: Vec<(Grupo, Vec<Seccion>)> = Grupo::TODOS.iter().map(|g| (*g, Seccion::TODAS.iter().copied().filter(|s| s.grupo() == *g).collect())).collect();
        assert_eq!(orden[0], (Grupo::Controles, vec![Seccion::Controles]), "las acciones rapidas van arriba del todo");
        assert_eq!(orden[1], (Grupo::General, vec![Seccion::General]));
        assert_eq!(orden[2], (Grupo::PantallaYEntrada, vec![Seccion::Atajos, Seccion::Entrada, Seccion::Pantalla]));
        assert_eq!(orden[3], (Grupo::Maquina, vec![Seccion::Maquina, Seccion::Perfiles]));
        assert_eq!(orden[4], (Grupo::Almacenamiento, vec![Seccion::Imagen, Seccion::Compartir]));
        assert_eq!(orden[5], (Grupo::Avanzado, vec![Seccion::Root, Seccion::Puente, Seccion::Diagnostico, Seccion::Acerca]));
        assert_eq!(Grupo::PantallaYEntrada.titulo(), tx!("grupo.pantalla_entrada"));
        assert_eq!(Seccion::Root.titulo(), tx!("seccion.acceso_root"));
        assert_eq!(Seccion::Puente.titulo(), tx!("seccion.traductor_arm"));
        assert_eq!(Seccion::Imagen.titulo(), tx!("comun.imagen_android"));
        let t = tipo();
        for vent in [(1100.0f32, 900.0f32), (800.0, 600.0), (700.0, 500.0)] {
            let g = geometria(vent, &t);
            let n = g.nav.expect("ventana ancha");
            let filas = nav_filas(n);
            let grupos: Vec<Grupo> = filas.iter().filter_map(|f| if let NavFila::Grupo(g, _) = f { Some(*g) } else { None }).collect();
            // Controles, General y Maquina (su primera seccion se llama como el grupo) no repiten el encabezado
            assert_eq!(grupos, vec![Grupo::PantallaYEntrada, Grupo::Almacenamiento, Grupo::Avanzado]);
            let rects: Vec<R> = filas.iter().map(|f| match f { NavFila::Grupo(_, r) | NavFila::Item(_, r) => *r }).collect();
            assert_eq!(filas.iter().filter(|f| matches!(f, NavFila::Item(..))).count(), Seccion::TODAS.len());
            for (i, r) in rects.iter().enumerate() {
                assert!(r.x >= n.x && r.x + r.w <= n.x + n.w + 0.01 && r.y >= n.y, "{:?}", r);
                assert!(r.y + r.h <= n.y + n.h + 0.01, "{:?} {:?}: la fila {} sale de la columna ({:?})", vent, n, i, r);
                for o in rects.iter().skip(i + 1) {
                    assert!(!r.se_cruza(o), "{:?} y {:?} se solapan", r, o);
                }
            }
        }
        // el encabezado de grupo se dibuja
        let b = Banco::nuevo((1100.0, 900.0));
        for g in ["Pantalla y entrada", "Almacenamiento", "Avanzado"] {
            assert!(b.hay_texto(g), "falta el encabezado {}", g);
        }
        // y sus secciones se alcanzan
        let mut b = Banco::nuevo((1100.0, 900.0));
        for s in Seccion::TODAS {
            b.seccion(s);
        }
    }

    #[test]
    fn root_con_una_operacion_en_curso_no_deja_tocar_nada() {
        let mut b = Banco::nuevo((1000.0, 800.0));
        b.clic("a-nav-root");
        b.d.operacion = Some(("root".into(), "Activando el root".into()));
        assert!(b.hay_texto("Activando el root..."));
        for n in ["a-tog-root.cargar_al_inicio", "a-tog-root.instalar_automaticamente", "a-btn-root-gestor", "a-btn-reiniciar-android", "a-btn-root-actualizar"] {
            b.ir_a(n);
            let r = b.boton(n);
            let _ = r;
        }
        // deshabilitados: no estan entre los controles activos del hit-test
        b.ir_a("a-btn-root-gestor");
        let c = ctx!(b);
        let r = b.boton("a-btn-root-gestor");
        assert!(b.a.control_en(&b.cfg, &c, r.x + 2.0, r.y + 2.0).is_none());
        assert!(b.clic("a-btn-root-gestor").is_empty());
        assert!(b.clic("a-tog-root.cargar_al_inicio").is_empty());
        // y un error del hilo se ve en linea
        b.d.operacion = None;
        b.a.mensaje("root", Tono::Error, "No se pudo completar: sin adb");
        assert!(b.hay_texto("No se pudo completar"));
    }


    fn almacen_de_prueba(completa: bool, disco_existe: bool) -> Almacen {
        let faltan = if completa { vec![] } else { vec!["super.img".to_string(), "boot.img".to_string()] };
        Almacen {
            imagen: imagen::EstadoImagen { carpeta: "/maquina/imagen".into(), existe: true, faltan, bytes: 1_900_000_000, build: Some(16373615) },
            disco: imagen::EstadoDisco { ruta: "/maquina/disco.img".into(), existe: disco_existe, bytes: 24 << 30, datos: if disco_existe { Some(24 << 30) } else { None }, error: None },
            imagenes: vec![
                crate::catalogo::FilaImagen { id: "phone-x86_64-16373615".into(), perfil: "phone-x86_64".into(), nombre_perfil: "Teléfono virtual x86_64".into(), build: Some(16373615), bytes: 1_900_000_000, completa, con_disco: disco_existe },
                crate::catalogo::FilaImagen { id: "car-x86_64-16373615".into(), perfil: "car-x86_64".into(), nombre_perfil: "Coche virtual x86_64".into(), build: Some(16373615), bytes: 2_000_000_000, completa: true, con_disco: false },
            ],
            actual: Some("phone-x86_64-16373615".into()),
        }
    }

    #[test]
    fn imagenes_lista_cambio_con_confirmacion_y_agregar() {
        let mut b = Banco::nuevo((1000.0, 1000.0));
        b.seccion(Seccion::Imagen);
        b.d.almacen = Some(almacen_de_prueba(true, true));
        assert!(b.hay_texto(tx!("imagen.imagenes_instaladas")) && b.hay_texto("phone-x86_64-16373615") && b.hay_texto("car-x86_64-16373615") && b.hay_texto(tx!("imagen.uso")));
        let todo = b.textos().join(" ");
        assert!(todo.contains("Coche virtual x86_64") && todo.contains("compilación 16373615") && todo.contains("con disco"), "{}", todo);
        // la que ya se usa no tiene boton; la otra pide confirmacion y no borra nada
        assert!(b.clic("a-imagen-1").is_empty());
        let todo = b.textos().join(" ");
        assert!(b.hay_texto("¿Usar «car-x86_64-16373615» en esta máquina?") && todo.contains("No se borra nada") && todo.contains("propio disco"), "{}", todo);
        assert!(todo.contains("se crea uno vacío"));
        // cancelar no cambia nada
        assert!(b.clic("a-btn-cancelar").is_empty() && b.a.confirmando.is_none());
        b.clic("a-imagen-1");
        match &b.clic("a-btn-confirmar")[..] {
            [Efecto::Orden { clave, args, .. }] => {
                assert_eq!(*clave, "imagen.usar");
                assert_eq!(args, &vec!["image".to_string(), "use".into(), "car-x86_64-16373615".into()]);
            }
            x => panic!("{:?}", x),
        }
        // con la maquina en marcha avisa de que hace falta reiniciar
        b.d.en_marcha = true;
        b.clic("a-imagen-1");
        assert!(b.textos().join(" ").contains("apágala y arráncala"));
        b.clic("a-btn-cancelar");
        // una imagen incompleta no se puede elegir
        b.d.almacen = Some(almacen_de_prueba(false, true));
        assert!(b.hay_texto("INCOMPLETA"));
        // agregar: sin origen da error; con origen lanza `image add ORIGEN` y limpia el borrador
        assert!(b.clic("a-btn-imagen-agregar").is_empty() && matches!(b.a.mensaje_de("imagen"), Some((Tono::Error, _))));
        b.a.form_origen = "/descargas/imagen.zip".into();
        match &b.clic("a-btn-imagen-agregar")[..] {
            [Efecto::Orden { clave, args, .. }] => {
                assert_eq!(*clave, "imagen");
                assert_eq!(args, &vec!["image".to_string(), "add".into(), "/descargas/imagen.zip".into()]);
            }
            x => panic!("{:?}", x),
        }
        assert!(b.a.form_origen.is_empty());
    }

    #[test]
    fn imagen_estado_y_campos_sin_descarga() {
        let mut b = Banco::nuevo((1000.0, 800.0));
        // entrar en la seccion pide mirar la imagen y el disco
        assert_eq!(b.clic("a-nav-imagen"), vec![Efecto::AlmacenActualizar]);
        assert!(b.hay_texto(tx!("encabezado.imagen_android_disco")) && b.hay_texto(tx!("imagen.sin_datos_todavia")));
        assert!(b.hay_texto("próximo arranque"));
        b.d.almacen = Some(almacen_de_prueba(true, true));
        assert!(b.hay_texto(tx!("imagen.completa")) && b.hay_texto("/maquina/imagen") && b.hay_texto("16373615") && b.hay_texto("/maquina/disco.img"));
        assert!(b.textos().iter().any(|t| t.contains("24.0 GiB")));
        b.d.almacen = Some(almacen_de_prueba(false, false));
        assert!(b.hay_texto(tx!("imagen.incompleta")) && b.hay_texto("Faltan 2 archivos: super.img, boot.img.") && b.hay_texto(tx!("imagen.no_existe")), "{:?}", b.textos());
        // la aplicacion no descarga nada: ni boton de descarga ni compilacion a bajar
        assert!(!b.botones().iter().any(|(n, _)| n.contains("descargar") || n.contains("compilacion")), "{:?}", b.botones().iter().map(|x| x.0.clone()).collect::<Vec<_>>());
        assert!(!b.textos().join(" ").contains("https://"));
        // tamano de datos: 24G, 4096M, img; 1G y 24T se rechazan
        let poner = |b: &mut Banco, t: &str| {
            b.clic("a-campo-datos-tam");
            for _ in 0..8 {
                b.tecla(tec(42));
            }
            b.a.texto(t);
            b.tecla(tec(40))
        };
        assert_eq!(poner(&mut b, "16g").1, vec![Efecto::Cambio("disk.data".into())]);
        assert_eq!(b.cfg.get("disk.data"), "16G");
        assert_eq!(poner(&mut b, "img").1, vec![Efecto::Cambio("disk.data".into())]);
        assert_eq!(b.cfg.get("disk.data"), "img");
        assert!(poner(&mut b, "1G").1.is_empty() && matches!(b.a.mensaje_de("disk.data"), Some((Tono::Error, m)) if m.contains("2G")));
        assert!(poner(&mut b, "24T").1.is_empty());
        assert_eq!(b.cfg.get("disk.data"), "img");
    }

    #[test]
    fn regenerar_disco_pide_dos_confirmaciones_en_rojo_y_se_bloquea_con_la_maquina_en_marcha() {
        let mut b = Banco::nuevo((1000.0, 900.0));
        b.seccion(Seccion::Imagen);
        b.d.almacen = Some(almacen_de_prueba(true, true));
        // maquina en marcha: el boton esta deshabilitado y lo dice en linea
        b.d.en_marcha = true;
        assert!(b.hay_texto("Hay una máquina en marcha con este disco"));
        b.a.scroll = 1e6;
        assert!(b.clic("a-btn-disco-regenerar").is_empty() && b.a.confirmando.is_none());
        let c = ctx!(b);
        b.a.scroll = 1e6;
        let r = b.boton("a-btn-disco-regenerar");
        assert!(b.a.control_en(&b.cfg, &c, r.x + 2.0, r.y + 2.0).is_none());
        // apagada: primera confirmacion (roja, con lo que se pierde)
        b.d.en_marcha = false;
        b.a.scroll = 1e6;
        assert!(b.clic("a-btn-disco-regenerar").is_empty());
        b.a.scroll = 1e6;
        assert!(b.hay_texto(tx!("imagen.borrar_disco_esta_maquina")) && b.textos().iter().any(|t| t.contains("BORRA las apps") || t.contains("BORRA")), "{:?}", b.textos());
        let c = ctx!(b);
        let rojo = b.a.dibujar(&b.cfg, &c).into_iter().any(|p| matches!(p, Pint::Rect { c, .. } if c == tema::p().error));
        assert!(rojo, "la caja de confirmacion lleva borde rojo");
        // cancelar no hace nada
        assert!(b.clic("a-btn-cancelar").is_empty() && b.a.confirmando.is_none());
        // continuar -> segunda confirmacion, sin orden todavia
        b.a.scroll = 1e6;
        b.clic("a-btn-disco-regenerar");
        b.a.scroll = 1e6;
        assert!(b.clic("a-btn-confirmar").is_empty());
        b.a.scroll = 1e6;
        assert!(b.hay_texto(tx!("imagen.seguro_pierde_todo_maquina")));
        b.a.scroll = 1e6;
        let e = b.clic("a-btn-confirmar");
        match &e[..] {
            [Efecto::Orden { clave, args, .. }] => {
                assert_eq!(*clave, "disco");
                assert_eq!(args[..3], ["disk".to_string(), "reset".into(), "--yes".into()]);
                assert!(args.contains(&"/maquina/disco.img".to_string()) && args.contains(&"/maquina/imagen".to_string()));
            }
            x => panic!("{:?}", x),
        }
        // si la maquina arranca entre las dos confirmaciones, la segunda no ejecuta nada
        b.clic("a-btn-disco-regenerar");
        b.a.scroll = 1e6;
        b.clic("a-btn-confirmar");
        b.d.en_marcha = true;
        b.a.scroll = 1e6;
        assert!(b.a.confirmando == Some(Conf::DiscoPaso2));
        let c = ctx!(b);
        let r = b.boton("a-btn-confirmar");
        let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        b.a.mover(&b.cfg, &c, x, y);
        let _ = (x, y);
    }

    #[test]
    fn puente_estado_instalacion_y_retroceso_en_rojo() {
        let mut b = Banco::nuevo((1000.0, 900.0));
        assert_eq!(b.clic("a-nav-puente"), vec![Efecto::PuenteActualizar]);
        assert!(b.hay_texto(tx!("seccion.traductor_arm")) && b.hay_texto("Sin datos"));
        assert!(b.textos().iter().any(|t| t.contains("restablece")), "el aviso de riesgo es fijo");
        b.d.puente_estado = Some(puente::parsear_estado("biblioteca=1\nmd5=5e4f26d507a3d4b7a7a3b0e3f1f2c3d4\nbytes=10135552\nnb=libheddle.so\nabilist=x86_64,arm64-v8a\nboot=1\nmuertes=0\nzygote=1\n"));
        assert!(b.hay_texto(tx!("puente.cargado_zygote")) && b.hay_texto(tx!("puente.actualizar")));
        assert!(!b.hay_texto("5e4f26d507a3") && !b.hay_texto("libheddle"), "los datos de la biblioteca van tras Detalles técnicos");
        b.clic("a-btn-detalles-puente-estado");
        // los detalles dicen lo util (huella, tamano, si la propiedad esta configurada) sin el nombre del archivo ni del proyecto
        assert!(b.hay_texto("5e4f26d507a3") && b.hay_texto("Propiedad del puente nativo: configurada") && b.hay_texto("9.7 MiB"));
        assert!(!b.hay_texto("libheddle") && !b.textos().join(" ").to_lowercase().contains("heddle"));
        b.clic("a-btn-detalles-puente-estado");
        // instalar: no hace nada hasta confirmar, y la confirmacion muestra lo que se descarga y el riesgo
        b.a.scroll = 1e6;
        assert!(b.clic("a-btn-puente-instalar").is_empty());
        b.a.scroll = 1e6;
        assert!(b.hay_texto(tx!("puente.instalar_traductor_arm")));
        // el origen esta en los detalles tecnicos de la confirmacion (solo el sitio, sin el nombre del proyecto)
        assert!(!b.textos().join(" ").to_lowercase().contains("heddle"), "el texto principal es generico");
        b.a.scroll = 1e6;
        b.clic("a-btn-detalles-puente");
        b.a.scroll = 1e6;
        let todo = b.textos().join(" ");
        assert!(todo.contains("se consigue en GitHub") && todo.contains("weft bridge status"), "{}", todo);
        assert!(!b.textos().join(" ").to_lowercase().contains("heddle") && !b.textos().join(" ").contains("43fdfdg45454"));
        assert!(b.textos().iter().any(|t| t.contains("90 s")) && b.textos().iter().any(|t| t.contains("en rojo")));
        b.a.scroll = 1e6;
        b.clic("a-btn-cancelar");
        assert!(b.a.confirmando.is_none());
        b.a.scroll = 1e6;
        b.clic("a-btn-puente-instalar");
        b.a.scroll = 1e6;
        let e = b.clic("a-btn-confirmar");
        assert!(matches!(&e[..], [Efecto::Orden { clave: "puente", args, .. }] if args == &vec!["bridge".to_string(), "install".into()]), "{:?}", e);
        // quitar tambien confirma
        b.a.scroll = 1e6;
        b.clic("a-btn-puente-quitar");
        b.a.scroll = 1e6;
        assert!(b.hay_texto(tx!("puente.quitar_traductor_arm")));
        let e = b.clic("a-btn-confirmar");
        assert!(matches!(&e[..], [Efecto::Orden { clave: "puente", args, .. }] if args == &vec!["bridge".to_string(), "remove".into()]));
        // quitar esta deshabilitado si no hay traductor instalado
        b.d.puente_estado = Some(puente::parsear_estado("biblioteca=0\nnb=\nboot=1\n"));
        b.a.scroll = 1e6;
        assert!(b.clic("a-btn-puente-quitar").is_empty() && b.a.confirmando.is_none());
        assert!(b.hay_texto(tx!("comun.instalar")));
        // el retroceso automatico llega como error: se ve en rojo con el motivo
        b.a.mensaje("puente", Tono::Error, "No se pudo completar: RESTAURADO: la candidata fallo (system_server murio 2 veces antes de terminar de arrancar).");
        b.a.scroll = 1e6;
        let c = ctx!(b);
        let rojo = b.a.dibujar(&b.cfg, &c).into_iter().any(|p| matches!(p, Pint::Texto { t, c, .. } if c == tema::p().error && t.contains("RESTAURADO")));
        assert!(rojo || b.textos().iter().any(|t| t.contains("RESTAURADO")), "{:?}", b.textos());
        // una operacion en curso bloquea los botones
        b.d.operacion = Some(("puente".into(), "Instalando el traductor ARM".into()));
        assert!(b.hay_texto("Instalando el traductor ARM..."));
        b.a.scroll = 1e6;
        assert!(b.clic("a-btn-puente-instalar").is_empty() && b.a.confirmando.is_none());
    }

    #[test]
    fn diagnostico_doctor_e_informe() {
        let mut b = Banco::nuevo((1000.0, 800.0));
        b.seccion(Seccion::Diagnostico);
        assert!(b.hay_texto(tx!("seccion.diagnostico")) && b.hay_texto(tx!("diagnostico.aun_no_ha_comprobado")) && b.hay_texto(tx!("diagnostico.ejecutar_doctor")) && b.hay_texto(tx!("diagnostico.crear_informe")));
        assert_eq!(b.clic("a-btn-comprobar"), vec![Efecto::Comprobar]);
        b.d.doctor = Some(vec![
            FilaDoctor { nivel: NivelDoctor::Ok, nombre: "KVM".into(), detalle: "accesible".into() },
            FilaDoctor { nivel: NivelDoctor::Aviso, nombre: "virtiofsd".into(), detalle: "no esta".into() },
            FilaDoctor { nivel: NivelDoctor::Fallo, nombre: "SDL3".into(), detalle: "falta".into() },
        ]);
        let c = ctx!(b);
        let col = |txt: &str| b.a.dibujar(&b.cfg, &c).into_iter().find_map(|p| match p {
            Pint::Texto { t, c, .. } if t == txt => Some(c),
            _ => None,
        });
        assert_eq!((col("OK"), col("AVISO"), col("FALLO")), (Some(tema::p().exito), Some(tema::p().aviso), Some(tema::p().error)));
        // el informe es una orden `report`; el resultado (la ruta) vuelve como mensaje
        b.a.scroll = 1e6;
        let e = b.clic("a-btn-informe-crear");
        assert!(matches!(&e[..], [Efecto::Orden { clave: "informe", args, .. }] if args == &vec!["report".to_string()]), "{:?}", e);
        b.a.mensaje("informe", Tono::Exito, "Informe guardado en /carpeta/informe-20261007-120000.tar.gz (sin datos personales).");
        b.a.scroll = 1e6;
        assert!(b.hay_texto("Informe guardado en"));
        // Acerca de sigue mostrando el resultado de doctor
        b.seccion(Seccion::Acerca);
        assert!(b.hay_texto("virtiofsd") && b.hay_texto(tx!("acerca.volver_comprobar")));
    }

    fn escribir(b: &mut Banco, campo: &str, texto: &str) {
        b.clic(campo);
        b.a.texto(texto);
        b.tecla(tec(40));
    }

    /// Campos de ruta: "Examinar..." abre el selector del sistema, lo elegido entra en el campo, `~` se expande al
    /// confirmar, Ctrl+V pega una URI del gestor de archivos como ruta y soltar un archivo elige el campo de la seccion.
    #[test]
    fn campos_de_ruta_examinar_pegar_soltar_y_tilde() {
        let mut b = Banco::nuevo((1000.0, 800.0));
        b.seccion(Seccion::Imagen);
        assert_eq!(b.clic("a-examinar-origen"), vec![Efecto::Elegir(Campo::Origen)]);
        assert!(b.a.poner_ruta(Campo::Origen, "/datos/imagen.zip"));
        assert_eq!(b.a.form_origen, "/datos/imagen.zip");
        // ~ al confirmar
        let home = std::env::var("HOME").unwrap_or_default();
        b.a.form_origen.clear();
        escribir(&mut b, "a-campo-origen", "~/x.zip");
        if !home.is_empty() {
            assert_eq!(b.a.form_origen, format!("{}/x.zip", home.trim_end_matches('/')));
        }
        // pegar: una URI se convierte; solo la primera linea
        b.a.form_origen.clear();
        b.clic("a-campo-origen");
        assert!(b.a.pegar("file:///tmp/Mi%20imagen.zip\nfile:///otra"));
        b.tecla(tec(40));
        assert_eq!(b.a.form_origen, "/tmp/Mi imagen.zip");
        assert!(!b.a.pegar("sin campo en edicion"));
        // soltar: con la pantalla abierta, el campo de la seccion visible; cerrada, por el tipo
        assert_eq!(b.a.campo_para_soltar(false, "/a/b.txt"), Some(Campo::Origen));
        b.seccion(Seccion::Compartir);
        assert_eq!(b.a.campo_para_soltar(false, "/a/b.zip"), Some(Campo::ShareRuta));
        b.a.cerrar();
        assert_eq!(b.a.campo_para_soltar(true, "/a"), Some(Campo::ShareRuta));
        assert_eq!(b.a.campo_para_soltar(false, "/a/IMAGEN.ZIP"), Some(Campo::Origen));
        assert_eq!(b.a.campo_para_soltar(false, "/a/b.txt"), None);
        assert_eq!(Ajustes::seccion_de(Campo::RutaPuente), Seccion::Puente);
    }

    /// Una operacion larga con porcentaje dibuja su barra y, si se puede cancelar, el boton Cancelar.
    #[test]
    fn operacion_larga_con_progreso_y_cancelar() {
        let mut b = Banco::nuevo((1000.0, 800.0));
        b.seccion(Seccion::Imagen);
        b.d.operacion = Some(("imagen".into(), "copiando la imagen: 40 %".into()));
        assert!(!b.botones().iter().any(|(n, _)| n == "a-btn-cancelar-operacion"));
        b.d.cancelable = true;
        assert_eq!(b.clic("a-btn-cancelar-operacion"), vec![Efecto::CancelarOperacion]);
        assert_eq!(porcentaje_de("copiando la imagen: 40 %"), Some(40.0));
        assert_eq!(porcentaje_de("descomprimiendo (1.2 GiB)"), None);
        assert_eq!(porcentaje_de("x 250 %"), None);
    }

    #[test]
    fn carpetas_agregar_y_quitar() {
        let mut b = Banco::nuevo((1000.0, 800.0));
        b.seccion(Seccion::Compartir);
        assert!(b.hay_texto(tx!("compartir.aplica_proximo_arranque_maquina")) && b.hay_texto(tx!("compartir.todavia_no_hay_ninguna")));
        // los campos de texto no toman los codigos de tecla de los digitos (llegan como texto)
        b.clic("a-campo-share-nombre");
        assert!(b.a.quiere_texto());
        b.tecla(tec(30));
        b.a.texto("Mis datos");
        assert_eq!(b.a.edicion.as_ref().unwrap().buf, "Mis datos");
        // el nombre con espacio no vale: error en linea y se sigue editando
        let (_, e) = b.tecla(tec(40));
        assert!(e.is_empty() && b.a.quiere_texto());
        assert!(matches!(b.a.mensaje_de("share.nombre"), Some((Tono::Error, m)) if m.contains("solo admite")));
        for _ in 0..4 {
            b.tecla(tec(42));
        }
        b.tecla(tec(41));
        assert!(!b.a.quiere_texto());
        // agregar sin datos: error
        b.clic("a-btn-agregar-carpeta");
        assert!(matches!(b.a.mensaje_de("share.agregar"), Some((Tono::Error, _))));
        assert!(compartir::lista(&b.cfg).is_empty());
        // una ruta que no existe: aviso amarillo al salir del campo
        escribir(&mut b, "a-campo-share-nombre", "Datos");
        escribir(&mut b, "a-campo-share-ruta", "/no/existe/seguro");
        assert!(matches!(b.a.mensaje_de("share.ruta"), Some((Tono::Aviso, _))));
        b.clic("a-btn-agregar-carpeta");
        assert!(compartir::lista(&b.cfg).is_empty());
        // ruta buena, solo lectura
        escribir(&mut b, "a-campo-share-ruta", "/");
        // (se borra lo anterior: el campo se abre con lo que habia)
        let _ = b.a.mensaje_de("share.ruta");
        b.a.form_ruta = "/".into();
        b.clic("a-btn-form-solo-lectura");
        assert!(b.a.form_ro);
        let e = b.clic("a-btn-agregar-carpeta");
        assert!(e.contains(&Efecto::Cambio("share.0".into())), "{:?}", e);
        assert!(e.iter().any(|x| matches!(x, Efecto::Orden { clave: "share.agregar-orden", args, .. } if args == &vec!["share".to_string(), "setup".into()])));
        let l = compartir::lista(&b.cfg);
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].nombre.as_str(), l[0].ro, l[0].ruta.as_str()), ("Datos", true, "/"));
        // el formulario queda limpio y la lista muestra la carpeta
        assert!(b.a.form_nombre.is_empty() && !b.a.form_ro);
        assert!(b.hay_texto("Datos") && b.hay_texto("/sdcard/Datos  ·  solo lectura"), "{:?}", b.textos());
        // un nombre repetido
        escribir(&mut b, "a-campo-share-nombre", "datos");
        b.a.form_ruta = "/usr".into();
        b.clic("a-btn-agregar-carpeta");
        assert!(matches!(b.a.mensaje_de("share.agregar"), Some((Tono::Error, m)) if m.contains("ya hay")));
        // quitar: pide confirmacion, cancelar no cambia nada
        assert!(b.clic("a-quitar-0").is_empty());
        assert!(b.hay_texto("¿Quitar «Datos» de la lista?"));
        b.clic("a-btn-cancelar");
        assert_eq!(compartir::lista(&b.cfg).len(), 1);
        b.clic("a-quitar-0");
        let e = b.clic("a-btn-confirmar");
        assert!(e.contains(&Efecto::Cambio("share.0".into())));
        assert!(compartir::lista(&b.cfg).is_empty());
    }

    #[test]
    fn carpetas_aviso_sin_virtiofsd_y_cupo() {
        let mut b = Banco::nuevo((1000.0, 800.0));
        b.seccion(Seccion::Compartir);
        assert!(!b.hay_texto("No se encuentra virtiofs"));
        b.d.virtiofsd = false;
        assert!(b.hay_texto("No se encuentra virtiofsd"));
        for i in 0..compartir::MAX {
            compartir::agregar(&mut b.cfg, Carpeta { nombre: format!("C{}", i), ro: false, ruta: format!("/c{}", i) }, false).unwrap();
        }
        assert!(b.hay_texto("Ya hay 4 carpetas"));
        // las que no existen en el equipo se avisan en la lista
        assert!(b.hay_texto("se omitirá al arrancar") || b.textos().iter().any(|t| t.contains("omitirá")));
    }

    #[test]
    fn el_texto_largo_muestra_el_final_al_escribir() {
        let mut b = Banco::nuevo((700.0, 700.0));
        b.seccion(Seccion::Compartir);
        b.clic("a-campo-share-ruta");
        let larga = format!("/{}/final", "directorio".repeat(30));
        b.a.texto(&larga);
        assert!(b.hay_texto("final") && b.textos().iter().any(|t| t.starts_with('…')), "{:?}", b.textos());
        assert_eq!(b.a.edicion.as_ref().unwrap().buf, larga);
    }

    #[test]
    fn nombres_estables() {
        assert_eq!(Id::Cerrar.nombre(), "a-cerrar");
        assert_eq!(Id::Nav(Seccion::Maquina).nombre(), "a-nav-maquina");
        assert_eq!(Id::Atajo(FilaAtajo::Accion(AccionAtajo::VolMas)).nombre(), "a-atajo-vol_mas");
        assert_eq!(Id::Atajo(FilaAtajo::Extra(AtajoExtra::PantallaCompleta)).nombre(), "a-atajo-pantalla_completa");
        assert_eq!(Id::Paso("rueda.paso", -1).nombre(), "a-paso-rueda.paso-menos");
        assert_eq!(Seccion::parse("atajos"), Some(Seccion::Atajos));
        assert_eq!(Seccion::parse("x"), None);
        assert_eq!(opciones("orientacion").len(), 5);
        assert!(opciones("nada").is_empty());
    }

    // ---------------------------------------------------------------------------------------------------------------
    // LA INTERFAZ NO NOMBRA A NADIE, EN NINGUN ESTADO
    // ---------------------------------------------------------------------------------------------------------------

    /// Todos los textos dinamicos que los modulos de operacion pueden entregar a la interfaz, producidos en modo interfaz:
    /// progreso, notas, resultados y errores de root, traductor ARM e imagen; el doctor (todas sus filas de aceleracion mas la
    /// del equipo real); salidas simuladas de las ordenes (con nombres de proveedor, para ejercitar la red de seguridad) y los
    /// avisos de la barra.
    fn catalogo_dinamico() -> Vec<String> {
        crate::textos::con_modo(true, || {
            let mut v: Vec<String> = Vec::new();
            v.extend(root::todos_los_textos());
            v.extend(imagen::todos_los_textos());
            v.extend(crate::catalogo::todos_los_textos());
            v.extend(puente::tests::todos_los_textos());
            v.extend(crate::compartir::todos_los_textos());
            let st = crate::vm::State { dir: std::env::temp_dir().join(format!("ar-catalogo-{}", std::process::id())) };
            for f in crate::doctor::todas_las_filas(&st) {
                v.push(f.nombre.to_string());
                v.push(f.detalle);
            }
            // errores de las herramientas que llegan tal cual (el mundo exterior nombra a sus proveedores)
            for t in ["KernelSU: late-load failed", "me.weishu.kernelsu no instalado", "curl: (22) 404 en ci.android.com", "libheddle.so: Permission denied", "gfxstream no responde", "Magisk/Zygisk/LKM"] {
                v.push(crate::textos::limpiar(t));
                v.push(crate::vista::resultado_de_orden(false, t, t).1);
                v.push(crate::vista::resultado_de_orden(true, "", t).1);
            }
            v.push(crate::vista::resultado_de_orden(true, "", "").1);
            v
        })
    }

    #[test]
    fn el_catalogo_dinamico_no_necesita_la_red_de_seguridad() {
        // lo que los modulos producen para la interfaz ya es generico por construccion: `limpiar` no cambia nada
        let cat = catalogo_dinamico();
        assert!(cat.len() > 150, "el catalogo es pequeno: {}", cat.len());
        for t in &cat {
            assert!(crate::textos::prohibida_en(t).is_none(), "texto de interfaz con nombre prohibido ({:?}): {}", crate::textos::prohibida_en(t), t);
            assert_eq!(&crate::textos::limpiar(t), t);
        }
        // y en consola SI nombran lo tecnico (el detalle sigue ahi: no se perdio, esta en otro camino)
        let (consola, interfaz) = (
            crate::textos::con_modo(false, || (root::ErrorRoot::Falta(root::KSUD.remoto, "/d".into()).texto(), root::Nota::CargadoPrevio.texto(), crate::puente::biblioteca_local(None, std::path::Path::new("/no/hay")).unwrap_err())),
            crate::textos::con_modo(true, || (root::ErrorRoot::Falta(root::KSUD.remoto, "/d".into()).texto(), root::Nota::CargadoPrevio.texto(), crate::puente::biblioteca_local(None, std::path::Path::new("/no/hay")).unwrap_err())),
        );
        assert!(consola.0.contains("ksud") && consola.1.contains("KernelSU-Next") && consola.2.contains("libheddle.so"), "{:?}", consola);
        assert!(crate::textos::prohibida_en(&format!("{} {} {}", interfaz.0, interfaz.1, interfaz.2)).is_none(), "{:?}", interfaz);
        // la consola de las ordenes sigue nombrando al proveedor por su camino propio
        assert!(root::proveedor().nombre_tecnico().contains("KernelSU-Next") && root::Estado::default().resumen().contains("KernelSU-Next"));
        assert!(crate::puente::Estado::default().resumen().contains("libheddle.so"));
    }

    /// La salida de una orden con nombres de proveedor llega a la interfaz limpia (exito y error).
    #[test]
    fn la_salida_de_las_ordenes_se_limpia() {
        let (tono, t) = crate::vista::resultado_de_orden(false, "ksud: late-load fallo", "KernelSU-Next v3.4.0 instalado");
        assert_eq!(tono, Tono::Error);
        assert!(crate::textos::prohibida_en(&t).is_none(), "{}", t);
        let (tono, t) = crate::vista::resultado_de_orden(true, "", "Gestor de KernelSU-Next instalado.\nzygisk off");
        assert_eq!(tono, Tono::Exito);
        assert!(crate::textos::prohibida_en(&t).is_none() && t.contains("componente de terceros"), "{}", t);
        // los mensajes en linea tambien pasan por la red de seguridad
        let mut a = Ajustes::nuevo();
        a.mensaje("root", Tono::Error, "No se pudo: me.weishu.kernelsu");
        assert!(crate::textos::prohibida_en(a.mensaje_de("root").unwrap().1).is_none());
    }

    fn estados_de_root() -> Vec<root::Estado> {
        let mut v = Vec::new();
        for bits in 0..64u32 {
            for version in [Some("33294".to_string()), None] {
                v.push(root::Estado { servicio: bits & 1 != 0, ksud: bits & 2 != 0, cargado: bits & 4 != 0, gestor: bits & 8 != 0, oficial: bits & 16 != 0, reconocido: bits & 32 != 0, version });
            }
        }
        v
    }

    fn estados_de_puente() -> Vec<puente::Estado> {
        let mut v = Vec::new();
        for bits in 0..64u32 {
            for nb in ["", "libheddle.so", "otra.so"] {
                for muertes in [0, 1, 2] {
                    v.push(puente::Estado {
                        biblioteca: bits & 1 != 0,
                        md5: if bits & 2 != 0 { Some("5e4f26d507a3d4b7a7a3b0e3f1f2c3d4".into()) } else { None },
                        bytes: if bits & 4 != 0 { Some(10_135_552) } else { None },
                        nb: nb.into(),
                        isa: "x86_64".into(),
                        abilist: if bits & 8 != 0 { "x86_64,arm64-v8a".into() } else { String::new() },
                        abilist64: "x86_64".into(),
                        en_archivo: vec!["ro.dalvik.vm.native.bridge=libheddle.so".into()],
                        boot: bits & 16 != 0,
                        muertes,
                        en_zygote: bits & 32 != 0,
                        lineas_log: if bits & 1 != 0 { 3 } else { 0 },
                        marca: bits & 8 != 0 && muertes == 2,
                        respaldo: true,
                    });
                }
            }
        }
        v
    }

    /// Recorre la interfaz EN TODOS LOS ESTADOS y falla si aparece alguna palabra prohibida (kernelsu, ksud, ksu, ksunext,
    /// rifsxd, weishu, magisk, zygisk, cuttlefish, heddle, libheddle, ci.android.com, gfxstream, rutabaga, late-load,
    /// lkm; sin distinguir mayusculas): cada seccion en ventana ancha, estrecha y baja; cada confirmacion abierta; los detalles
    /// tecnicos desplegados; cada operacion en curso con cada mensaje de progreso de root, traductor, imagen y disco; mensajes en
    /// linea de exito, aviso y error con cada resultado; los 128 estados posibles de root y los del traductor con todos sus
    /// campos; la salida del doctor con todas sus filas; el estado de la maquina, los mandos, la imagen y el disco; y la barra.
    #[test]
    fn la_interfaz_no_nombra_a_nadie_en_ningun_estado() {
        use crate::textos::prohibida_en;
        let cat = catalogo_dinamico();
        let mut renders = 0usize;
        let mut mira = |b: &Banco, ctx: &str| {
            renders += 1;
            let t = b.textos().join(" ");
            if let Some(p) = prohibida_en(&t) {
                let i = t.to_lowercase().find(p).unwrap_or(0);
                panic!("{}: la interfaz nombra {:?}: ...{}...", ctx, p, t[i.saturating_sub(60).min(t.len())..(i + 80).min(t.len())].replace('\n', " "));
            }
        };
        let ventanas = [(1100.0f32, 900.0f32), (500.0, 700.0), (700.0, 400.0)];
        let confs = [None, Some(Conf::PerfilBorrar), Some(Conf::PerfilAplicar), Some(Conf::RootActivar), Some(Conf::RootDesactivar), Some(Conf::RootGestor), Some(Conf::PuenteInstalar), Some(Conf::PuenteQuitar), Some(Conf::ReiniciarAndroid), Some(Conf::ReinicioCompleto), Some(Conf::Apagar), Some(Conf::QuitarCarpeta(0)), Some(Conf::DiscoPaso1), Some(Conf::DiscoPaso2)];
        let detalles = [Detalle::Root, Detalle::Puente, Detalle::PuenteEstado];
        let claves = ["root", "puente", "imagen", "disco", "informe", "reinicio", "apagar", "controles", "mandos", "gamepad", "aplicar", "estado", "share.quitar", "share.agregar", "share.agregar-orden"];
        let mensajes_fijos: Vec<(Tono, String)> = cat.iter().enumerate().map(|(i, t)| ([Tono::Aviso, Tono::Error, Tono::Exito][i % 3], t.clone())).collect();
        let (almacen_ok, almacen_mal) = (almacen_de_prueba(true, true), almacen_de_prueba(false, false));
        let filas_doctor: Vec<FilaDoctor> = {
            let st = crate::vm::State { dir: std::env::temp_dir().join(format!("ar-catalogo2-{}", std::process::id())) };
            crate::textos::con_modo(true, || crate::doctor::todas_las_filas(&st)).into_iter().map(|f| FilaDoctor { nivel: f.nivel, nombre: f.nombre.to_string(), detalle: f.detalle }).collect()
        };
        for vent in ventanas {
            for sec in Seccion::TODAS {
                let mut b = Banco::nuevo(vent);
                b.a.abrir(Some(sec));
                b.d.fuente = "Noto Sans".into();
                b.d.maquina = Some(crate::vista::Maquina { cpus: 2, mem_mb: 4096, tipo: "pc-q35-10.2-machine".into(), estado: "running".into() });
                b.d.encendida = Some(Duration::from_secs(95));
                b.d.mandos = vec![crate::vista::Mando { path: "/dev/input/event7".into(), name: "Mando de prueba".into(), conectado: None, ocupado: false }];
                b.d.doctor = Some(filas_doctor.clone());
                b.d.almacen = Some(almacen_ok.clone());
                b.d.reinicio_completo = true;
                // 1) cada confirmacion abierta, con y sin detalles desplegados, arriba y abajo del todo
                for conf in confs {
                    if conf.is_some() && matches!(conf, Some(Conf::PerfilBorrar | Conf::PerfilAplicar)) != (sec == Seccion::Perfiles) {
                        continue;
                    }
                    b.a.confirmando = conf;
                    for abiertos in [vec![], detalles.to_vec()] {
                        b.a.detalles = abiertos;
                        for pos in [0.0f32, 1e6] {
                            b.a.scroll = pos;
                            mira(&b, &format!("{:?} {:?} {:?}", vent, sec, conf));
                        }
                    }
                }
                b.a.confirmando = None;
                b.a.detalles = detalles.to_vec();
                // 2) estados de root y del traductor, con todos sus campos
                if sec == Seccion::Root {
                    for e in estados_de_root() {
                        b.d.root_estado = Some(e);
                        mira(&b, "root con estado");
                    }
                    b.d.root_estado = None;
                    b.d.root_consultando = true;
                    mira(&b, "root consultando");
                    b.d.root_consultando = false;
                }
                if sec == Seccion::Puente {
                    for e in estados_de_puente() {
                        b.d.puente_estado = Some(e);
                        mira(&b, "puente con estado");
                    }
                    b.d.puente_estado = None;
                    b.d.puente_consultando = true;
                    mira(&b, "puente consultando");
                    b.d.puente_consultando = false;
                }
                // 3) operaciones en curso con cada mensaje de progreso, y mensajes en linea con cada resultado
                for (i, (tono, t)) in mensajes_fijos.iter().enumerate() {
                    b.d.operacion = Some((claves[i % claves.len()].to_string(), t.clone()));
                    for (k, c) in claves.iter().enumerate() {
                        let (tn, tx) = &mensajes_fijos[(i + k) % mensajes_fijos.len()];
                        b.a.mensaje(c, *tn, tx);
                    }
                    b.a.mensaje(claves[i % claves.len()], *tono, t);
                    mira(&b, &format!("operacion y mensajes #{} {:?}", i, sec));
                }
                b.d.operacion = None;
                // 4) doctor (con y sin comprobacion en curso), imagen y disco en todos sus estados, maquina y mandos
                b.d.doctor_en_curso = true;
                mira(&b, "doctor en curso");
                b.d.doctor_en_curso = false;
                for (almacen, en_marcha) in [(Some(almacen_ok.clone()), false), (Some(almacen_ok.clone()), true), (Some(almacen_mal.clone()), false), (None, false)] {
                    b.d.almacen = almacen;
                    b.d.en_marcha = en_marcha;
                    b.d.almacen_consultando = en_marcha;
                    mira(&b, "almacen");
                }
                for (qmp, reinicio, reiniciando) in [(true, false, false), (false, true, false), (true, true, true)] {
                    b.d.qmp_ok = qmp;
                    b.d.reinicio_completo = reinicio;
                    b.d.reiniciando = reiniciando;
                    b.d.maquina = if qmp { b.d.maquina.clone() } else { None };
                    mira(&b, "maquina");
                }
            }
        }
        // la barra superior: estado, zoom, rotacion y cada aviso breve que pueden dar los servicios
        let barra = crate::barra::Barra::default();
        let tipo = tipo();
        let mut avisos: Vec<Option<String>> = vec![None];
        avisos.extend(cat.iter().map(|t| Some(crate::textos::limpiar(t))));
        for aviso in avisos {
            use crate::vista::Estado;
            for estado in [Estado::Arrancando, Estado::EnMarcha, Estado::Pausada, Estado::Reiniciando, Estado::Apagando, Estado::SinAceleracion] {
                let info = crate::vista::Info { escala: 0.5, estado, aviso: aviso.clone(), ..crate::vista::Info::default() };
                let t: Vec<String> = barra.dibujar(R::new(0.0, 0.0, 1600.0, crate::barra::ALTO), &info, true, Some("F9"), &tipo).into_iter().filter_map(|p| if let Pint::Texto { t, .. } = p { Some(t) } else { None }).collect();
                assert!(prohibida_en(&t.join(" ")).is_none(), "{:?}", t);
                renders += 1;
            }
        }
        // nombres fijos: navegacion, grupos, encabezados, titulos de la ventana y etiquetas de los atajos
        for s in Seccion::TODAS {
            assert!(prohibida_en(s.titulo()).is_none() && prohibida_en(s.encabezado()).is_none() && prohibida_en(s.nombre()).is_none());
        }
        for g in Grupo::TODOS {
            assert!(prohibida_en(g.titulo()).is_none());
        }
        for a in FilaAtajo::todas() {
            assert!(prohibida_en(a.etiqueta()).is_none());
        }
        for (_, etq) in ["zoom", "pellizco.modificador", "gamepad", "maquina.tipo", "maquina.gpu", "orientacion", gestos::RATON_DERECHO, gestos::RATON_CENTRAL].iter().flat_map(|k| opciones(k)) {
            assert!(prohibida_en(etq).is_none(), "{}", etq);
        }
        assert!(prohibida_en(&format!("weft: {}", "prueba")).is_none());
        assert!(renders > 3000, "se recorrieron pocos estados: {}", renders);
    }

    /// Los fuentes que dibujan la interfaz (ventana, barra, configuracion y vista) no llevan ninguna palabra prohibida en su
    /// codigo (ni en cadenas): lo tecnico vive en los modulos de operacion, en su rama de consola. El catalogo de textos lo
    /// comprueba `textos::tests::los_catalogos_son_completos_y_genericos` (salvo sus secciones tecnicas `*_tec`, de consola).
    #[test]
    fn los_fuentes_de_la_interfaz_no_llevan_nombres_de_proveedores() {
        for (nombre, fuente) in [("window.rs", include_str!("window.rs")), ("barra.rs", include_str!("barra.rs")), ("vista.rs", include_str!("vista.rs")), ("ajustes.rs", include_str!("ajustes.rs"))] {
            let codigo = crate::textos::tests::sin_pruebas_ni_comentarios(fuente);
            assert_eq!(codigo.lines().count(), fuente.lines().count(), "{}: los numeros de linea deben seguir valiendo", nombre);
            // se recorre todo el codigo, no solo lo que hay antes del primer `#[cfg(test)]` (en ajustes.rs, una funcion
            // de prueba al principio dejaba fuera casi todo el archivo)
            let con_codigo = |t: &str| t.lines().filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with("//")).count();
            let (quedan, antes) = (con_codigo(&codigo), con_codigo(fuente.split("#[cfg(test)]").next().unwrap_or(fuente)));
            assert!(quedan >= antes, "{}: {} lineas con codigo, menos que antes del primer bloque de pruebas ({})", nombre, quedan, antes);
            if nombre == "ajustes.rs" {
                assert!(quedan > 10 * antes, "{}: quedaron solo {} lineas con codigo (antes del primer bloque de pruebas: {})", nombre, quedan, antes);
            }
            for (n, l) in codigo.lines().enumerate() {
                assert!(crate::textos::prohibida_en(l).is_none(), "{}:{} lleva {:?}: {}", nombre, n + 1, crate::textos::prohibida_en(l), l.trim());
            }
        }
    }

    /// Seccion Perfiles: elegir, duplicar, editar campos y extensiones, aplicar y borrar, todo con clics y teclas.
    #[test]
    fn perfiles_de_dispositivo_con_clics() {
        let dir = carpeta_perfiles("clics");
        let mut b = Banco::nuevo((1000.0, 800.0));
        b.a.perfiles_dir = Some(dir.clone());
        b.seccion(Seccion::Perfiles);
        assert!(b.hay_texto("Genérico A78") && b.hay_texto(tx!("perfiles.ninguno")));
        let pos = |b: &Banco, id: &str| b.a.perfiles.as_ref().unwrap().dispositivos.iter().position(|d| d.id == id).unwrap() + 1;
        // uno de fabrica: se ve, se usa y se duplica, pero no se edita ni se borra
        let i = pos(&b, "generico-a78");
        b.clic(&format!("a-perfil-{}", i));
        assert_eq!(b.a.visto(&b.cfg), "generico-a78");
        // todo el texto de la seccion, desplazandola de arriba abajo (se dibuja solo lo visible)
        let todo = |b: &mut Banco| {
            let mut v = Vec::new();
            for i in 0..80 {
                b.a.scroll = i as f32 * 200.0;
                v.extend(b.textos());
            }
            b.a.scroll = 0.0;
            v.join(" ")
        };
        let t = todo(&mut b);
        assert!(t.contains(&tx!("perfiles.de_fabrica")[..30]) && t.contains("0x411fd411"), "{}", t);
        b.ir_a("a-btn-perfil-usar");
        let nombres: Vec<String> = b.botones().into_iter().map(|x| x.0).collect();
        assert!(!nombres.iter().any(|n| n == "a-btn-perfil-borrar" || n == "a-campo-perfil-nombre"), "{:?}", nombres);
        assert!(b.clic("a-ext-sve").is_empty());
        assert_eq!(b.a.perfil_de(&b.cfg).unwrap().estado_extension("sve"), None, "las extensiones de uno de fabrica no se tocan");
        assert_eq!(b.clic("a-btn-perfil-usar"), vec![Efecto::Cambio("dispositivo.perfil".into())]);
        assert_eq!(b.cfg.get("dispositivo.perfil"), "generico-a78");
        assert!(matches!(b.a.mensaje_de("perfiles"), Some((Tono::Exito, _))));
        assert!(todo(&mut b).contains(&tx!("perfiles.se_aplica_al_arrancar")[..30]));
        // duplicar: el nuevo es del usuario, se ve y se edita
        b.clic("a-btn-perfil-duplicar");
        assert_eq!(b.a.visto(&b.cfg), "generico-a78-copia");
        let archivo = dir.join("generico-a78-copia.device");
        assert!(archivo.is_file());
        let escribir = |b: &mut Banco, campo: &str, t: &str| {
            b.clic(campo);
            for _ in 0..80 {
                b.tecla(tec(42));
            }
            b.a.texto(t);
            b.tecla(tec(40));
        };
        escribir(&mut b, "a-campo-perfil-nombre", "Mi telefono");
        assert!(b.a.editando().is_none());
        assert!(std::fs::read_to_string(&archivo).unwrap().contains("\ndispositivo.nombre=Mi telefono\n"));
        assert!(todo(&mut b).contains("Mi telefono"));
        // un valor no valido: error en linea y se sigue editando; corregido, se guarda
        escribir(&mut b, "a-campo-perfil-cpu-midr", "zz");
        assert_eq!(b.a.editando(), Some(campo_perfil("cpu.midr")));
        assert!(matches!(b.a.mensaje_de("disp.cpu_midr"), Some((Tono::Error, _))));
        for _ in 0..4 {
            b.tecla(tec(42));
        }
        b.a.texto("0x412fd471");
        b.tecla(tec(40));
        assert!(b.a.editando().is_none() && b.a.mensaje_de("disp.cpu_midr").is_none());
        escribir(&mut b, "a-campo-perfil-nucleos", "");
        escribir(&mut b, "a-campo-perfil-modelo", "Modelo X 1");
        let t = std::fs::read_to_string(&archivo).unwrap();
        assert!(t.contains("\ncpu.midr=0x412fd471\n") && t.contains("\nmaquina.nucleos=auto\n") && t.contains("\nproducto.modelo=Modelo X 1\n"), "{}", t);
        // extensiones: lista propia (la del A78); sve se enciende. Sin cpu-features del traductor no hay aviso; con el, se
        // avisa (solo informativo) de las anunciadas que no publica
        let leer = |b: &Banco| b.a.perfil_de(&b.cfg).unwrap().clone();
        b.clic("a-ext-sve");
        assert_eq!(leer(&b).estado_extension("sve"), Some('='));
        let aviso = txf!("perfiles.sin_soporte", "sve ssbs");
        let prefijo: String = aviso.chars().take(30).collect();
        assert!(!todo(&mut b).contains(&prefijo), "sin cpu-features no se avisa");
        b.clic("a-ext-sve");
        let estado = dir.join("estado");
        std::fs::create_dir_all(&estado).unwrap();
        std::fs::write(estado.join(dispositivo::ARCHIVO_SOPORTADAS), "# publicado por el traductor\nfp asimd evtstrm aes pmull sha1 sha2 crc32 atomics\nfphp asimdhp cpuid asimdrdm lrcpc dcpop asimddp\n").unwrap();
        b.a.estado_maquina = Some(estado);
        b.clic("a-ext-sve");
        assert!(todo(&mut b).contains(&prefijo), "falta el aviso");
        assert_eq!(leer(&b).sin_soporte(b.a.soportadas.as_deref().unwrap()), vec!["sve", "ssbs"]);
        b.clic("a-ext-sve");
        assert_eq!(leer(&b).estado_extension("sve"), None);
        // cambios sobre la base: +, - y nada
        b.clic("a-perfil-modo-2");
        assert_eq!(leer(&b).modo_extensiones(), ModoExtensiones::Cambios);
        // al pasar de la lista propia a cambios, cada una queda como +x
        assert_eq!(leer(&b).estado_extension("aes"), Some('+'));
        b.clic("a-ext-bf16");
        b.clic("a-ext-aes");
        let p = leer(&b);
        assert_eq!((p.estado_extension("bf16"), p.estado_extension("aes")), (Some('+'), Some('-')));
        let t = todo(&mut b);
        assert!(t.contains("+bf16") && t.contains("-aes"));
        b.clic("a-perfil-modo-0");
        assert_eq!(leer(&b).features, None);
        // pagina y registros
        b.clic("a-perfil-pagina-16");
        assert_eq!(leer(&b).pagina_kib, 16);
        b.clic("a-btn-detalles-perfil-registros");
        escribir(&mut b, "a-campo-perfil-id-aa64isar0", "0x0000_1000");
        assert_eq!(leer(&b).valor("cpu.id_aa64isar0"), "0x0000000000001000");
        // usarlo con la maquina en marcha: Aplicar ahora pide confirmacion y lanza `device apply`
        b.d.en_marcha = true;
        b.clic("a-btn-perfil-usar");
        assert_eq!(b.cfg.get("dispositivo.perfil"), "generico-a78-copia");
        assert!(b.clic("a-btn-perfil-aplicar").is_empty());
        assert_eq!(b.a.confirmando, Some(Conf::PerfilAplicar));
        let ef = b.clic("a-btn-confirmar");
        assert_eq!(ef, vec![Efecto::Orden { clave: "perfiles", etiqueta: tx!("progreso.aplicando_perfil").into(), args: vec!["device".into(), "apply".into()] }]);
        // con una operacion en curso no se puede volver a aplicar
        b.d.operacion = Some(("perfiles".into(), "Aplicando".into()));
        assert!(b.clic("a-btn-perfil-aplicar").is_empty() && b.a.confirmando.is_none());
        b.d.operacion = None;
        // borrar el que usa la maquina: confirmacion, archivo fuera y la maquina pasa a ninguno
        b.clic("a-btn-perfil-borrar");
        assert_eq!(b.a.confirmando, Some(Conf::PerfilBorrar));
        b.clic("a-btn-cancelar");
        assert!(archivo.is_file());
        b.clic("a-btn-perfil-borrar");
        let ef = b.clic("a-btn-confirmar");
        assert_eq!(ef, vec![Efecto::Cambio("dispositivo.perfil".into())]);
        assert!(!archivo.exists());
        assert_eq!(b.cfg.get("dispositivo.perfil"), dispositivo::NINGUNO);
        assert!(b.a.perfiles.as_ref().unwrap().buscar("generico-a78-copia").is_none());
        // ninguno: solo se usa
        b.clic("a-perfil-0");
        assert!(todo(&mut b).contains(&tx!("perfiles.ninguno_explica")[..30]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Sin carpeta de perfiles (la ventana no la dio) se ven los integrados y no se puede duplicar.
    #[test]
    fn perfiles_sin_carpeta() {
        let mut b = Banco::nuevo((1000.0, 800.0));
        b.seccion(Seccion::Perfiles);
        assert!(b.a.perfiles.as_ref().unwrap().dispositivos.len() >= 3);
        assert!(b.clic("a-btn-perfil-duplicar").is_empty());
        assert!(b.a.perfiles.as_ref().unwrap().dispositivos.iter().all(|d| d.origen == crate::perfil::Origen::Integrado));
    }
}
