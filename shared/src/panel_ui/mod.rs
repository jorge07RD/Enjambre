//! Construcción de la UI egui del panel de control, compartida por el panel
//! embebido (proceso `sim`) y el panel en ventana aparte (proceso `panel`).
//!
//! `config_panel` solo dibuja los controles y muta `SimParams`/`PanelState`;
//! las acciones que requieren el contexto de la simulación (crear partículas,
//! mover la cámara, lanzar la ventana...) se devuelven como `PanelEvent` para
//! que cada proceso las resuelva a su manera (localmente o por IPC).

//!
//! Diseño "cristal de instrumento": una lámina de cristal (`ui_kit::glass_slab`)
//! con cabecera fija (estado, transporte y velocidad), barra de 4 pestañas y,
//! debajo, las tarjetas de la pestaña activa. Cada pestaña vive en su módulo.

mod header;
mod tab_look;
mod tab_out;
mod tab_show;
mod tab_sim;

use crate::config::{Brush, InteractionSnapshot, MusicSync, SimParams, Tool, FRAME_PRESETS};
use crate::playlist::{Playlist, SeqPlayback};
use crate::shapes::SavedShape;
use crate::ui_kit::{self, Backdrop, PanelTab};
use serde::{Deserialize, Serialize};

/// Estado de UI no contenido en `SimParams`, más telemetría de solo lectura.
pub struct PanelState {
    pub fill_count: i32,
    pub active_color: usize,
    /// Herramienta del ratón: pincel (pintar) o fuerza (atraer/repeler).
    pub tool: Tool,
    pub brush: Brush,
    pub brush_size: f32,
    /// Alto del lienzo (el mundo). El ancho se deriva del aspecto de la ventana.
    pub canvas_size: f32,
    /// Zoom de la cámara (1 = ajustado a la ventana).
    pub zoom_level: f32,
    pub paused: bool,

    /// Carpeta de guardado de los vídeos (vacío = directorio de trabajo).
    pub video_dir: String,
    /// Pista de música a mezclar en el vídeo (vacío = sin audio).
    pub music_path: String,

    // --- Sincronía con la música ---
    /// Configuración de la sincronía (viaja en `ControlState`).
    pub music_sync: MusicSync,
    /// Resultado del análisis de la pista (telemetría, solo para mostrar).
    pub music_analyzed: bool,
    pub music_duration: f32,
    pub music_onsets: usize,
    pub music_bpm: Option<f32>,
    /// `true` mientras la preescucha está sonando en el `sim`.
    pub music_previewing: bool,

    /// Texto en edición para formar con las partículas.
    pub shape_text: String,
    /// Biblioteca de formas/letras guardadas (la gobierna el `sim`, llega por
    /// telemetría) para aplicarlas con un clic.
    pub saved_shapes: Vec<SavedShape>,
    /// Índice seleccionado en la biblioteca (para recorrerla y resaltarlo).
    pub shape_sel: Option<usize>,
    /// Miniatura de la imagen de forma activa, cacheada junto a la ruta que la
    /// generó (para no releerla/decodificarla en cada frame). `None` si no hay
    /// imagen activa o falló la carga.
    pub shape_image_preview: Option<(String, egui::TextureHandle)>,

    // --- Escenas ---
    /// Nombre en edición para guardar una escena nueva.
    pub scene_name_input: String,
    /// Transición suave al cambiar de escena.
    pub scene_smooth: bool,
    pub scene_transition_duration: f32,
    /// Auto-avance (slideshow) entre escenas y su intervalo (s).
    pub scene_autoplay: bool,
    pub scene_autoplay_interval: f32,
    /// Lista de escenas y predeterminada (las gobierna el `sim` por telemetría).
    pub scenes: Vec<String>,
    pub default_scene: String,

    // --- Secuenciador ---
    /// Playlist en edición. En el panel separado es la copia local (viaja al
    /// `sim` completa con `SeqSetPlaylist`); embebido, refleja la del `sim`.
    pub seq_playlist: Playlist,
    /// Estado de reproducción (telemetría del `sim`, solo para mostrar).
    pub seq_state: SeqPlayback,
    pub seq_idx: usize,
    pub seq_elapsed: f32,
    /// Índice del combo "añadir escena al show".
    pub seq_scene_pick: usize,

    // Telemetría que llega de la simulación (solo para mostrar).
    pub particle_count: usize,
    pub fps: i32,
    /// `true` mientras la simulación está grabando vídeo (rótulo del botón).
    pub recording: bool,
    /// Segundos grabados hasta ahora (0 si no se está grabando). Telemetría
    /// para el contador del HUD; el `sim`/`sim-gpu` lo derivan de
    /// `recorder.frames / REC_FPS` (exacto, no de reloj de pared).
    pub recording_secs: f32,
    /// Estado del recuadro de encuadre (lo evoluciona el ratón en el `sim`).
    pub show_frame: bool,
    pub frame_preset: usize,
    /// Resolución de salida del preset actual (para mostrar).
    pub frame_w: u32,
    pub frame_h: u32,

    /// `true` cuando esta UI corre en la ventana aparte (`panel`); cambia el
    /// botón de separar por uno de reacoplar.
    pub standalone: bool,

    // --- Presentación (solo UI, no viaja por IPC) ---
    /// Pestaña activa del panel.
    pub tab: PanelTab,
    /// Qué hay detrás de la lámina: `Live` en `sim-gpu` (desenfoca la escena
    /// real), `Ambient` en el resto (fondo de manchas de color).
    pub backdrop: Backdrop,
    /// Rect de la lámina de cristal en puntos, escrito por `config_panel` en
    /// cada frame; el host lo usa para desenfocar la escena justo detrás.
    pub glass_rect: Option<egui::Rect>,
}

impl Default for PanelState {
    fn default() -> Self {
        Self {
            fill_count: 2000,
            active_color: 0,
            tool: Tool::Brush,
            brush: Brush::Add,
            brush_size: 14.0,
            canvas_size: 800.0,
            zoom_level: 1.0,
            paused: false,
            video_dir: String::new(),
            music_path: String::new(),
            music_sync: MusicSync::default(),
            music_analyzed: false,
            music_duration: 0.0,
            music_onsets: 0,
            music_bpm: None,
            music_previewing: false,
            shape_text: String::new(),
            saved_shapes: Vec::new(),
            shape_sel: None,
            shape_image_preview: None,
            scene_name_input: String::new(),
            scene_smooth: true,
            scene_transition_duration: 3.0,
            scene_autoplay: false,
            scene_autoplay_interval: 10.0,
            scenes: Vec::new(),
            default_scene: String::new(),
            seq_playlist: Playlist::default(),
            seq_state: SeqPlayback::Stopped,
            seq_idx: 0,
            seq_elapsed: 0.0,
            seq_scene_pick: 0,
            particle_count: 0,
            fps: 0,
            recording: false,
            recording_secs: 0.0,
            show_frame: false,
            frame_preset: 0,
            frame_w: FRAME_PRESETS[0].1,
            frame_h: FRAME_PRESETS[0].2,
            standalone: false,
            tab: PanelTab::Sim,
            backdrop: Backdrop::Ambient,
            glass_rect: None,
        }
    }
}

/// Acciones que el panel pide pero que solo el proceso `sim` puede ejecutar.
#[derive(Clone, Serialize, Deserialize)]
pub enum PanelEvent {
    /// Avanzar un paso y pausar.
    Step,
    /// Vaciar todas las partículas.
    Clear,
    /// Llenar aleatoriamente con N partículas.
    Fill(usize),
    /// Iniciar transición de interacción desde `snap` (cambio de modo, etc.);
    /// respeta el interruptor de "Transición fluida".
    StartTransition(InteractionSnapshot),
    /// Cruce SIEMPRE suave de las reglas (aleatorizar/restablecer la matriz),
    /// aunque la transición fluida global esté apagada.
    MatrixBlend(InteractionSnapshot),
    /// Fijar una nueva velocidad objetivo (transición suave si está activa).
    SetSpeed(f32),
    /// Empezar/detener la grabación de vídeo.
    ToggleRecord,
    /// Mostrar/ocultar el recuadro de encuadre de grabación.
    ToggleFrame,
    /// Elegir el preset de resolución/aspecto del recuadro.
    SetFramePreset(usize),
    /// Recolocar el recuadro en el centro de la vista actual.
    CenterFrame,
    /// Abrir un diálogo nativo para elegir la carpeta de guardado.
    PickVideoDir,
    /// Abrir un diálogo nativo para elegir la pista de música del vídeo.
    PickMusic,
    /// Guardar la configuración actual como escena con este nombre.
    SaveScene(String),
    /// Cargar una escena (transición suave si está activada).
    LoadScene(String),
    /// Marcar una escena como predeterminada (se carga al arrancar).
    SetDefaultScene(String),
    /// Borrar una escena guardada.
    DeleteScene(String),
    /// Cargar la siguiente / anterior escena de la lista (ciclado).
    NextScene,
    PrevScene,
    /// Exportar todas las escenas a un archivo / importar y fusionar.
    ExportScenes,
    ImportScenes,
    /// Ajustar el zoom para que el lienzo entre en la ventana.
    FitCanvas,
    /// Igualar el lienzo a los píxeles de la ventana de simulación (1:1).
    CanvasEqualsScreen,
    /// Abrir el panel en una ventana del SO aparte.
    Detach,
    /// Volver a acoplar el panel dentro de la ventana de simulación.
    Reattach,
    /// Formar un texto con las partículas.
    FormText(String),
    /// Abrir un diálogo para elegir una imagen y formarla.
    FormImagePick,
    /// Formar la imagen del fichero indicado (lo emite el panel separado tras
    /// abrir su propio diálogo).
    FormImagePath(String),
    /// Abrir un diálogo para elegir VARIAS imágenes/vídeos de una vez: todas
    /// se guardan en la biblioteca (nombradas por su fichero) y la última se
    /// aplica de inmediato.
    FormImagesPick,
    /// Rutas elegidas en el diálogo múltiple (lo emite el panel separado tras
    /// abrir su propio diálogo).
    FormImagePaths(Vec<String>),
    /// Soltar la forma actual (volver al modo de interacción).
    ReleaseShape,
    /// Guardar la forma activa (texto o imagen) en la biblioteca.
    SaveShape,
    /// Aplicar una forma guardada de la biblioteca (por nombre).
    ApplyShape(String),
    /// Borrar una forma guardada de la biblioteca.
    DeleteShape(String),
    /// Ocultar el panel acoplado (se recupera con la tecla H).
    HidePanel,
    /// Reemplazar la playlist del secuenciador (edición completa: añadir,
    /// borrar, reordenar, duraciones y opciones). El `sim` la persiste.
    SeqSetPlaylist(Playlist),
    /// Transporte del secuenciador.
    SeqPlay,
    SeqPause,
    SeqStop,
    /// Saltar a la entrada siguiente/anterior de la playlist (con envoltura).
    SeqNext,
    SeqPrev,
    /// Saltar a la entrada `i` de la playlist.
    SeqJump(usize),
    /// Analizar la pista de música elegida (envolvente + beats, en fondo).
    MusicAnalyze,
    /// Arrancar/parar la preescucha de la pista (ffplay en el `sim`).
    MusicPreviewToggle,
}

/// Selecciona la forma `idx` de la biblioteca: la resalta, carga su texto en el
/// campo editable (vacío si es imagen) y pide aplicarla al `sim`.
fn select_shape(
    st: &mut PanelState,
    events: &mut Vec<PanelEvent>,
    shapes: &[SavedShape],
    idx: usize,
) {
    if let Some(s) = shapes.get(idx) {
        st.shape_sel = Some(idx);
        st.shape_text = s.text.clone();
        events.push(PanelEvent::ApplyShape(s.name.clone()));
    }
}

/// Carga y reduce a miniatura la imagen de `path` como textura egui, para
/// mostrarla de referencia junto a los controles de forma. `None` si el
/// fichero no existe o no se pudo decodificar.
fn load_image_preview(ctx: &egui::Context, path: &str) -> Option<egui::TextureHandle> {
    let thumb = image::open(path).ok()?.thumbnail(220, 220).to_rgba8();
    let (w, h) = (thumb.width() as usize, thumb.height() as usize);
    let color_image = egui::ColorImage::from_rgba_unmultiplied([w, h], thumb.as_raw());
    Some(ctx.load_texture(
        "shape_image_preview",
        color_image,
        egui::TextureOptions::LINEAR,
    ))
}

fn egui_color(c: [f32; 3]) -> egui::Color32 {
    egui::Color32::from_rgb(
        (c[0] * 255.0) as u8,
        (c[1] * 255.0) as u8,
        (c[2] * 255.0) as u8,
    )
}

/// Dibuja todo el panel de control. Devuelve los eventos disparados este frame.
pub fn config_panel(
    ui: &mut egui::Ui,
    params: &mut SimParams,
    st: &mut PanelState,
) -> Vec<PanelEvent> {
    let mut events = Vec::new();
    let backdrop = st.backdrop;
    let slab = ui_kit::glass_slab(ui, backdrop, |ui| {
        // Cabecera fija: lo más usado, sin scroll.
        ui_kit::gutter(ui, |ui| {
            header::show(ui, params, st, &mut events);
            ui_kit::tab_bar(ui, &mut st.tab);
        });
        ui.add_space(2.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt(("panel_scroll", st.tab as u8))
            .show(ui, |ui| {
                ui_kit::gutter(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    match st.tab {
                        PanelTab::Sim => tab_sim::show(ui, params, st, &mut events),
                        PanelTab::Look => tab_look::show(ui, params, st, &mut events),
                        PanelTab::Show => tab_show::show(ui, params, st, &mut events),
                        PanelTab::Out => tab_out::show(ui, params, st, &mut events),
                    }
                    ui.add_space(6.0);
                });
            });
    });
    st.glass_rect = Some(slab);
    events
}
