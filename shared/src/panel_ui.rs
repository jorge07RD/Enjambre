//! Construcción de la UI egui del panel de control, compartida por el panel
//! embebido (proceso `sim`) y el panel en ventana aparte (proceso `panel`).
//!
//! `config_panel` solo dibuja los controles y muta `SimParams`/`PanelState`;
//! las acciones que requieren el contexto de la simulación (crear partículas,
//! mover la cámara, lanzar la ventana...) se devuelven como `PanelEvent` para
//! que cada proceso las resuelva a su manera (localmente o por IPC).

use crate::config::{
    palette, AudioTarget, BoidsScope, Boundary, Brush, InteractionMode, InteractionSnapshot,
    RenderStyle, SimParams, Tool, COLOR_NAMES, FRAME_PRESETS, NUM_COLORS,
};
use crate::shapes::SavedShape;
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

    /// Texto en edición para formar con las partículas.
    pub shape_text: String,
    /// Biblioteca de formas/letras guardadas (la gobierna el `sim`, llega por
    /// telemetría) para aplicarlas con un clic.
    pub saved_shapes: Vec<SavedShape>,
    /// Índice seleccionado en la biblioteca (para recorrerla y resaltarlo).
    pub shape_sel: Option<usize>,

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

    // Telemetría que llega de la simulación (solo para mostrar).
    pub particle_count: usize,
    pub fps: i32,
    /// `true` mientras la simulación está grabando vídeo (rótulo del botón).
    pub recording: bool,
    /// Estado del recuadro de encuadre (lo evoluciona el ratón en el `sim`).
    pub show_frame: bool,
    pub frame_preset: usize,
    /// Resolución de salida del preset actual (para mostrar).
    pub frame_w: u32,
    pub frame_h: u32,

    /// `true` cuando esta UI corre en la ventana aparte (`panel`); cambia el
    /// botón de separar por uno de reacoplar.
    pub standalone: bool,
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
            shape_text: String::new(),
            saved_shapes: Vec::new(),
            shape_sel: None,
            scene_name_input: String::new(),
            scene_smooth: true,
            scene_transition_duration: 3.0,
            scene_autoplay: false,
            scene_autoplay_interval: 10.0,
            scenes: Vec::new(),
            default_scene: String::new(),
            particle_count: 0,
            fps: 0,
            recording: false,
            show_frame: false,
            frame_preset: 0,
            frame_w: FRAME_PRESETS[0].1,
            frame_h: FRAME_PRESETS[0].2,
            standalone: false,
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
    /// Iniciar transición de interacción desde `snap` (cambio de modo, etc.).
    StartTransition(InteractionSnapshot),
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
    let palette = palette();
    let mut events = Vec::new();

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 5.0;

        // ===================== Cabecera fija =====================
        // Título + estado en vivo + transporte + velocidad: lo más usado, sin
        // colapsar. El resto de ajustes van en secciones plegables debajo.
        ui.horizontal(|ui| {
            ui.heading("🐝 Enjambre");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Solo el panel acoplado se puede ocultar (el separado es su
                // propia ventana del SO: basta cerrarla).
                if !st.standalone
                    && ui
                        .button("🙈")
                        .on_hover_text("Ocultar panel (H)")
                        .clicked()
                {
                    events.push(PanelEvent::HidePanel);
                }
                if st.recording {
                    ui.label(
                        egui::RichText::new("● REC")
                            .color(egui::Color32::from_rgb(230, 70, 70))
                            .strong(),
                    );
                }
            });
        });
        ui.label(
            egui::RichText::new(format!("{} partículas · {} FPS", st.particle_count, st.fps))
                .weak(),
        );

        // Separar / reacoplar el panel en su propia ventana.
        if st.standalone {
            if ui.button("⮌ Reacoplar en la ventana (D)").clicked() {
                events.push(PanelEvent::Reattach);
            }
        } else if ui.button("🗗 Separar panel en otra ventana (D)").clicked() {
            events.push(PanelEvent::Detach);
        }

        // Transporte.
        ui.horizontal(|ui| {
            if ui
                .button(if st.paused {
                    "▶ Reanudar"
                } else {
                    "⏸ Pausa"
                })
                .on_hover_text("Espacio")
                .clicked()
            {
                st.paused = !st.paused;
            }
            if ui.button("⏭ Paso").on_hover_text(".").clicked() {
                st.paused = true;
                events.push(PanelEvent::Step);
            }
            if ui.button("⟲ Reiniciar").on_hover_text("C").clicked() {
                events.push(PanelEvent::Clear);
            }
        });

        // --- Velocidad (en %, con cambio suave y atajos rápidos) ---
        let mut pct = params.speed_target * 100.0;
        if ui
            .add(
                egui::Slider::new(&mut pct, 0.0..=300.0)
                    .suffix(" %")
                    .text("Velocidad"),
            )
            .changed()
        {
            params.speed_target = pct / 100.0; // evita que el slider rebote
            events.push(PanelEvent::SetSpeed(pct / 100.0));
        }
        ui.horizontal_wrapped(|ui| {
            for p in (10..=100).step_by(10) {
                let v = p as f32 / 100.0;
                let selected = (params.speed_target - v).abs() < 1e-3;
                if ui.selectable_label(selected, format!("{p}%")).clicked() {
                    params.speed_target = v;
                    events.push(PanelEvent::SetSpeed(v));
                }
            }
        });
        ui.checkbox(&mut params.speed_smooth, "Cambio de velocidad suave");
        if params.speed_smooth {
            ui.add(
                egui::Slider::new(&mut params.speed_transition_duration, 0.1..=10.0)
                    .logarithmic(true)
                    .text("Duración cambio (s)"),
            );
        }
        ui.label(
            egui::RichText::new(format!(
                "Actual {:.0}% → objetivo {:.0}%  ·  teclas 1…0 = 10 %…100 %",
                params.time_scale * 100.0,
                params.speed_target * 100.0
            ))
            .weak()
            .small(),
        );

        ui.add_space(2.0);

        // ===================== Interacción =====================
        egui::CollapsingHeader::new("✨ Interacción")
            .default_open(true)
            .show(ui, |ui| {
                ui.checkbox(&mut params.smooth, "Transición fluida");
                if params.smooth {
                    ui.add(
                        egui::Slider::new(&mut params.transition_duration, 0.2..=60.0)
                            .logarithmic(true)
                            .text("Duración (s)"),
                    );
                    if params.blend < 1.0 {
                        ui.add(egui::ProgressBar::new(params.blend).text("transición"));
                    }
                }

                // Congelamos la interacción ANTES de aplicar cambios para poder
                // mezclar de forma continua hacia la nueva.
                let snap_before = params.current_snapshot();
                let old_mode = params.mode;
                let mut trigger = false;

                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(&mut params.mode, InteractionMode::SameColorOnly, "Mismo color");
                    ui.selectable_value(&mut params.mode, InteractionMode::Matrix, "Matriz");
                    ui.selectable_value(&mut params.mode, InteractionMode::Similarity, "Similitud");
                    ui.selectable_value(&mut params.mode, InteractionMode::Cyclic, "Cíclico");
                    ui.selectable_value(&mut params.mode, InteractionMode::Opposite, "Opuestos");
                    ui.selectable_value(&mut params.mode, InteractionMode::PredatorPrey, "Depredador-presa");
                    ui.selectable_value(&mut params.mode, InteractionMode::SelfRepel, "Repulsión propia");
                    ui.selectable_value(&mut params.mode, InteractionMode::Boids, "Bandada");
                });
                if params.mode == InteractionMode::SameColorOnly {
                    if ui
                        .checkbox(&mut params.same_repel_others, "Repeler colores distintos")
                        .changed()
                    {
                        trigger = true;
                    }
                    if params.same_repel_others {
                        ui.add(
                            egui::Slider::new(&mut params.same_repel_strength, 0.0..=1.0)
                                .text("Fuerza de repulsión"),
                        );
                    }
                }
                if params.mode == InteractionMode::Similarity {
                    ui.label("Atracción según cercanía de color en la rueda.");
                    ui.add(egui::Slider::new(&mut params.sim_range, 0.02..=0.5).text("Tolerancia de color"));
                }
                if params.mode == InteractionMode::Matrix {
                    if ui.button("🎲 Aleatorizar reglas (M)").clicked() {
                        // La matriz es propiedad del panel: la aleatorizamos aquí
                        // mismo para que el nuevo estado fluya al `sim` por `State`
                        // y no lo pise el eco de la matriz anterior.
                        params.randomize_matrix(&mut ::rand::thread_rng());
                        trigger = true;
                    }
                    ui.checkbox(&mut params.auto_randomize, "Auto-aleatorizar sola");
                    if params.auto_randomize {
                        ui.add(
                            egui::Slider::new(&mut params.auto_randomize_interval, 1.0..=60.0)
                                .text("Cada (s)"),
                        );
                    }
                    ui.label("Fila = recibe · Columna = ejerce");
                    egui::Grid::new("matrix").striped(true).show(ui, |ui| {
                        ui.label("");
                        for j in 0..NUM_COLORS {
                            ui.colored_label(egui_color(palette[j]), "■");
                        }
                        ui.end_row();
                        for i in 0..NUM_COLORS {
                            ui.colored_label(egui_color(palette[i]), "■");
                            for j in 0..NUM_COLORS {
                                ui.add(
                                    egui::DragValue::new(&mut params.matrix[i][j])
                                        .speed(0.02)
                                        .range(-1.0..=1.0)
                                        .fixed_decimals(2),
                                );
                            }
                            ui.end_row();
                        }
                    });
                }
                if params.mode == InteractionMode::Cyclic {
                    ui.label("Cada color persigue al siguiente de la rueda y huye del anterior.");
                }
                if params.mode == InteractionMode::Opposite {
                    ui.label("Los colores opuestos en la rueda se atraen; los parecidos se repelen.");
                }
                if params.mode == InteractionMode::PredatorPrey {
                    ui.label("Los colores pares cazan a los impares; las presas huyen en manada.");
                }
                if params.mode == InteractionMode::SelfRepel {
                    ui.label("El mismo color se repele y los distintos se atraen (mezcla homogénea).");
                }
                if params.mode == InteractionMode::Boids {
                    ui.label("Murmuración (Boids): separación, alineación y cohesión.");
                    ui.horizontal(|ui| {
                        ui.label("Bandada:");
                        ui.selectable_value(&mut params.boids_scope, BoidsScope::All, "Todas");
                        ui.selectable_value(&mut params.boids_scope, BoidsScope::SameColor, "Por color");
                        ui.selectable_value(&mut params.boids_scope, BoidsScope::Hybrid, "Híbrido");
                    });
                    ui.add(egui::Slider::new(&mut params.boids_separation, 0.0..=3.0).text("Separación"));
                    ui.add(egui::Slider::new(&mut params.boids_alignment, 0.0..=3.0).text("Alineación"));
                    ui.add(egui::Slider::new(&mut params.boids_cohesion, 0.0..=3.0).text("Cohesión"));
                    if params.boids_scope != BoidsScope::All {
                        ui.add(
                            egui::Slider::new(&mut params.boids_group_avoid, 0.0..=3.0)
                                .text("Evitar otros grupos"),
                        );
                    }
                    ui.add(
                        egui::Slider::new(&mut params.boids_sep_radius, 0.1..=1.0)
                            .text("Radio de separación"),
                    );
                    ui.add(egui::Slider::new(&mut params.boids_cruise, 0.0..=150.0).text("Crucero"));
                }

                if params.mode != old_mode {
                    trigger = true;
                }
                if trigger {
                    events.push(PanelEvent::StartTransition(snap_before));
                }
            });

        // ===================== Física =====================
        egui::CollapsingHeader::new("🌡 Física").show(ui, |ui| {
            ui.add(egui::Slider::new(&mut params.force, 0.0..=5.0).text("Fuerza"));
            ui.add(egui::Slider::new(&mut params.r_max, 20.0..=200.0).text("Radio (r_max)"));
            ui.add(egui::Slider::new(&mut params.beta, 0.05..=0.9).text("Repulsión (β)"));
            ui.add(egui::Slider::new(&mut params.friction, 0.50..=0.99).text("Fricción"));
            ui.horizontal(|ui| {
                ui.label("Bordes:");
                ui.selectable_value(&mut params.boundary, Boundary::Wrap, "Toroidal");
                ui.selectable_value(&mut params.boundary, Boundary::Bounce, "Rebote");
            });
            ui.checkbox(
                &mut params.attract_active,
                "Atraer zonas activas al centro (A)",
            );
            if params.attract_active {
                ui.add(
                    egui::Slider::new(&mut params.attract_active_strength, 0.0..=2.0)
                        .text("Fuerza de recentrado"),
                );
                ui.label("Los grupos densos se acercan suavemente al centro de la vista.");
            }
        });

        // ===================== Apariencia =====================
        egui::CollapsingHeader::new("💡 Apariencia").show(ui, |ui| {
            ui.add(egui::Slider::new(&mut params.point_size, 1.0..=40.0).text("Tamaño punto"));
            ui.add(egui::Slider::new(&mut params.brightness, 0.1..=1.0).text("Brillo"));
            ui.horizontal(|ui| {
                ui.label("Estilo:");
                ui.selectable_value(&mut params.style, RenderStyle::Solid, "Sólido");
                ui.selectable_value(&mut params.style, RenderStyle::Glow, "Brillo");
                ui.selectable_value(&mut params.style, RenderStyle::SolidHalo, "Sólido+halo");
            });
            ui.checkbox(&mut params.trails, "Estelas de movimiento");
            if params.trails {
                // Menor desvanecido = estela más larga; invertimos el slider para
                // que "Longitud" crezca hacia la derecha.
                let mut length = 1.0 - params.trail_fade;
                if ui
                    .add(egui::Slider::new(&mut length, 0.5..=0.99).text("Longitud de estela"))
                    .changed()
                {
                    params.trail_fade = 1.0 - length;
                }
            }
            let mut orient_on = params.orient > 0.5;
            if ui
                .checkbox(&mut orient_on, "Orientar según movimiento (flechas)")
                .changed()
            {
                params.orient = if orient_on { 1.0 } else { 0.0 };
            }
        });

        // ===================== Dinámica del color =====================
        egui::CollapsingHeader::new("🌈 Dinámica del color").show(ui, |ui| {
            ui.checkbox(&mut params.random_color, "Cambio aleatorio de color");
            if params.random_color {
                ui.add(egui::Slider::new(&mut params.random_color_rate, 0.0..=0.5).text("Ritmo"));
            }
            ui.checkbox(&mut params.gradual, "Deriva lenta (color y atracción)");
            if params.gradual {
                ui.add(egui::Slider::new(&mut params.gradual_color_speed, 0.0..=0.1).text("Vel. color"));
                ui.add(
                    egui::Slider::new(&mut params.gradual_matrix_speed, 0.0..=0.1).text("Vel. atracción"),
                );
            }
            ui.checkbox(&mut params.color_smooth, "Transición fluida (color)");
            if params.color_smooth {
                ui.add(
                    egui::Slider::new(&mut params.color_transition_duration, 0.2..=20.0)
                        .logarithmic(true)
                        .text("Duración color (s)"),
                );
            }
        });

        // ===================== Audio =====================
        egui::CollapsingHeader::new("🔊 Reactivo al audio").show(ui, |ui| {
            ui.checkbox(&mut params.audio_reactive, "Reaccionar al sonido");
            if params.audio_reactive {
                ui.horizontal(|ui| {
                    ui.label("Modula:");
                    ui.selectable_value(&mut params.audio_target, AudioTarget::Speed, "Velocidad");
                    ui.selectable_value(&mut params.audio_target, AudioTarget::Force, "Fuerza");
                    ui.selectable_value(&mut params.audio_target, AudioTarget::Brightness, "Brillo");
                });
                ui.add(egui::Slider::new(&mut params.audio_intensity, 0.0..=4.0).text("Intensidad"));
                ui.label("Usa el micrófono/entrada por defecto del sistema.");
            }
        });

        // ===================== Partículas y herramienta =====================
        egui::CollapsingHeader::new("🖱 Partículas y herramienta").show(ui, |ui| {
            ui.add(egui::Slider::new(&mut st.fill_count, 100..=20000).text("Cantidad"));
            ui.horizontal(|ui| {
                if ui.button("Llenar (F)").clicked() {
                    events.push(PanelEvent::Fill(st.fill_count as usize));
                }
                if ui.button("Vaciar (C)").clicked() {
                    events.push(PanelEvent::Clear);
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.selectable_value(&mut st.tool, Tool::Brush, "Pincel");
                ui.selectable_value(&mut st.tool, Tool::Force, "Fuerza");
            });
            if st.tool == Tool::Force {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut params.pointer_repel, true, "Repeler");
                    ui.selectable_value(&mut params.pointer_repel, false, "Atraer");
                });
                ui.add(egui::Slider::new(&mut params.pointer_strength, 0.0..=3.0).text("Fuerza"));
                ui.add(egui::Slider::new(&mut params.pointer_radius, 30.0..=600.0).text("Radio"));
                ui.label("Click/arrastra en el lienzo para atraer o espantar el enjambre.");
            } else {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut st.brush, Brush::Add, "Añadir");
                    ui.selectable_value(&mut st.brush, Brush::Erase, "Borrar");
                });
                ui.horizontal_wrapped(|ui| {
                    for i in 0..NUM_COLORS {
                        let selected = st.active_color == i;
                        let label = if selected { "●" } else { "○" };
                        if ui
                            .add(egui::Button::new(label).fill(egui_color(palette[i])))
                            .on_hover_text(COLOR_NAMES[i])
                            .clicked()
                        {
                            st.active_color = i;
                        }
                    }
                });
                ui.add(egui::Slider::new(&mut st.brush_size, 2.0..=60.0).text("Brocha"));
                ui.label("Click/arrastra en el lienzo para pintar o borrar.");
            }
        });

        // ===================== Mensaje / Forma =====================
        egui::CollapsingHeader::new("💬 Mensaje / Forma").show(ui, |ui| {
            // Escribir un texto y aplicarlo (Enter o el botón).
            ui.horizontal(|ui| {
                let te = ui.add(
                    egui::TextEdit::singleline(&mut st.shape_text)
                        .hint_text("escribe un mensaje")
                        .desired_width(150.0),
                );
                let enter =
                    te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button("✍ Aplicar").clicked() || enter)
                    && !st.shape_text.trim().is_empty()
                {
                    events.push(PanelEvent::FormText(st.shape_text.clone()));
                }
            });
            ui.horizontal(|ui| {
                if ui.button("🖼 Imagen…").clicked() {
                    events.push(PanelEvent::FormImagePick);
                }
                if ui
                    .button("💾 Guardar")
                    .on_hover_text("Guardar en la biblioteca lo que haya en el campo")
                    .clicked()
                {
                    // Si hay texto editado, lo aplicamos antes de guardar para que
                    // se guarde exactamente lo que está en el campo (no lo anterior).
                    if !st.shape_text.trim().is_empty() {
                        events.push(PanelEvent::FormText(st.shape_text.clone()));
                    }
                    events.push(PanelEvent::SaveShape);
                }
                if ui.button("💨 Soltar (S)").clicked() {
                    events.push(PanelEvent::ReleaseShape);
                }
            });
            ui.add(
                egui::Slider::new(&mut params.shape_transition_duration, 0.0..=8.0)
                    .text("Fluidez (s)"),
            );
            ui.add(egui::Slider::new(&mut params.shape_strength, 0.0..=1.0).text("Fijación"));
            ui.checkbox(&mut params.shape_tint, "Teñir de un color");
            if params.shape_tint {
                ui.horizontal_wrapped(|ui| {
                    for i in 0..NUM_COLORS {
                        let selected = params.shape_color == i;
                        let label = if selected { "●" } else { "○" };
                        if ui
                            .add(egui::Button::new(label).fill(egui_color(palette[i])))
                            .on_hover_text(COLOR_NAMES[i])
                            .clicked()
                        {
                            params.shape_color = i;
                        }
                    }
                });
            }
            ui.label(
                egui::RichText::new(
                    "Fluidez = cuánto tarda en formarse/soltarse · más partículas = más legible.",
                )
                .weak()
                .small(),
            );

            // --- Biblioteca de formas guardadas ---
            ui.separator();
            let shapes = st.saved_shapes.clone();
            let n = shapes.len();
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Biblioteca").strong());
                if n > 0 {
                    // Recorrer la biblioteca: carga y aplica el ítem, para ir
                    // preparando frases una tras otra.
                    if ui.button("⏮").on_hover_text("Anterior").clicked() {
                        let cur = st.shape_sel.unwrap_or(0);
                        let idx = (cur + n - 1) % n;
                        select_shape(st, &mut events, &shapes, idx);
                    }
                    if ui.button("⏭").on_hover_text("Siguiente").clicked() {
                        let idx = st.shape_sel.map(|c| (c + 1) % n).unwrap_or(0);
                        select_shape(st, &mut events, &shapes, idx);
                    }
                }
            });
            if n == 0 {
                ui.label(
                    egui::RichText::new("Vacía. Aplica un texto/imagen y pulsa 💾 Guardar.")
                        .weak()
                        .small(),
                );
            } else {
                for (i, s) in shapes.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let icon = if s.is_image() { "🖼" } else { "✍" };
                        // Clic en el ítem: lo selecciona, lo carga en el campo
                        // (para editarlo) y lo aplica.
                        if ui
                            .selectable_label(st.shape_sel == Some(i), format!("{icon} {}", s.name))
                            .on_hover_text("Cargar y aplicar")
                            .clicked()
                        {
                            select_shape(st, &mut events, &shapes, i);
                        }
                        if ui.button("🗑").on_hover_text("Borrar").clicked() {
                            events.push(PanelEvent::DeleteShape(s.name.clone()));
                            if st.shape_sel == Some(i) {
                                st.shape_sel = None;
                            }
                        }
                    });
                }
            }
        });

        // ===================== Escenas =====================
        egui::CollapsingHeader::new("🎞 Escenas").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut st.scene_name_input)
                        .hint_text("nombre")
                        .desired_width(120.0),
                );
                if ui.button("📸 Guardar").clicked() && !st.scene_name_input.trim().is_empty() {
                    events.push(PanelEvent::SaveScene(st.scene_name_input.trim().to_string()));
                }
            });
            ui.checkbox(&mut st.scene_smooth, "Transición suave entre escenas");
            if st.scene_smooth {
                ui.add(
                    egui::Slider::new(&mut st.scene_transition_duration, 0.2..=20.0)
                        .logarithmic(true)
                        .text("Duración (s)"),
                );
            }
            ui.horizontal(|ui| {
                if ui.button("⏮ Anterior (P)").clicked() {
                    events.push(PanelEvent::PrevScene);
                }
                if ui.button("Siguiente ⏭ (N)").clicked() {
                    events.push(PanelEvent::NextScene);
                }
            });
            ui.checkbox(&mut st.scene_autoplay, "Auto-avance (slideshow)");
            if st.scene_autoplay {
                ui.add(
                    egui::Slider::new(&mut st.scene_autoplay_interval, 1.0..=120.0)
                        .logarithmic(true)
                        .text("Cada (s)"),
                );
            }
            ui.horizontal(|ui| {
                if ui.button("⬆ Exportar todas").clicked() {
                    events.push(PanelEvent::ExportScenes);
                }
                if ui.button("⬇ Importar…").clicked() {
                    events.push(PanelEvent::ImportScenes);
                }
            });
            ui.separator();
            if st.scenes.is_empty() {
                ui.label("Aún no hay escenas. Escribe un nombre y pulsa Guardar.");
            } else {
                for name in st.scenes.clone() {
                    let is_def = st.default_scene == name;
                    ui.horizontal(|ui| {
                        if ui.button("▶").on_hover_text("Cargar").clicked() {
                            events.push(PanelEvent::LoadScene(name.clone()));
                        }
                        if ui
                            .button("⟳")
                            .on_hover_text("Actualizar con la configuración actual")
                            .clicked()
                        {
                            // Sobrescribe la escena existente (upsert por nombre).
                            events.push(PanelEvent::SaveScene(name.clone()));
                        }
                        if ui
                            .selectable_label(is_def, "★")
                            .on_hover_text("Predeterminada")
                            .clicked()
                        {
                            events.push(PanelEvent::SetDefaultScene(name.clone()));
                        }
                        if ui.button("🗑").on_hover_text("Borrar").clicked() {
                            events.push(PanelEvent::DeleteScene(name.clone()));
                        }
                        ui.label(&name);
                    });
                }
            }
        });

        // ===================== Grabación =====================
        egui::CollapsingHeader::new("⏺ Grabación").show(ui, |ui| {
            if ui
                .button(if st.recording {
                    "⏹ Detener grabación (R)"
                } else {
                    "⏺ Grabar vídeo (R)"
                })
                .clicked()
            {
                events.push(PanelEvent::ToggleRecord);
            }
            let mut sf = st.show_frame;
            if ui
                .checkbox(&mut sf, "Mostrar encuadre de grabación (G)")
                .changed()
            {
                events.push(PanelEvent::ToggleFrame);
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("Tamaño:");
                for (i, (name, _, _)) in FRAME_PRESETS.iter().enumerate() {
                    if ui
                        .selectable_label(st.frame_preset == i, *name)
                        .clicked()
                    {
                        events.push(PanelEvent::SetFramePreset(i));
                    }
                }
            });
            ui.label(format!("Salida: {}×{} px", st.frame_w, st.frame_h));
            if ui.button("⊹ Centrar encuadre en la vista").clicked() {
                events.push(PanelEvent::CenterFrame);
            }
            ui.label("Arrastra el recuadro (mover) o una esquina (redimensionar) en el lienzo.");
            ui.separator();
            if ui.button("📁 Carpeta de guardado…").clicked() {
                events.push(PanelEvent::PickVideoDir);
            }
            ui.label(if st.video_dir.is_empty() {
                "Carpeta: (directorio actual)".to_string()
            } else {
                format!("Carpeta: {}", st.video_dir)
            });
        });

        // ===================== Lienzo y vista =====================
        egui::CollapsingHeader::new("🖼 Lienzo y vista").show(ui, |ui| {
            ui.add(
                egui::Slider::new(&mut st.canvas_size, 200.0..=6000.0)
                    .logarithmic(true)
                    .text("Tamaño lienzo"),
            );
            if ui.button("📐 Lienzo = pantalla (L)").clicked() {
                events.push(PanelEvent::CanvasEqualsScreen);
            }
            ui.label("Menos = más reducido y denso · Más = más espacio");
            ui.separator();
            ui.add(
                egui::Slider::new(&mut st.zoom_level, 0.05..=30.0)
                    .logarithmic(true)
                    .text("Zoom"),
            );
            if ui.button("Ajustar al lienzo (Z)").clicked() {
                events.push(PanelEvent::FitCanvas);
            }
            ui.label("Rueda = zoom · botón derecho = mover");
        });
    });

    events
}
