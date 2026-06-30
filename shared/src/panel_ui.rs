//! Construcción de la UI egui del panel de control, compartida por el panel
//! embebido (proceso `sim`) y el panel en ventana aparte (proceso `panel`).
//!
//! `config_panel` solo dibuja los controles y muta `SimParams`/`PanelState`;
//! las acciones que requieren el contexto de la simulación (crear partículas,
//! mover la cámara, lanzar la ventana...) se devuelven como `PanelEvent` para
//! que cada proceso las resuelva a su manera (localmente o por IPC).

use crate::config::{
    palette, Boundary, Brush, InteractionMode, InteractionSnapshot, RenderStyle, SimParams,
    COLOR_NAMES, NUM_COLORS,
};
use serde::{Deserialize, Serialize};

/// Estado de UI no contenido en `SimParams`, más telemetría de solo lectura.
pub struct PanelState {
    pub fill_count: i32,
    pub active_color: usize,
    pub brush: Brush,
    pub brush_size: f32,
    /// Alto del lienzo (el mundo). El ancho se deriva del aspecto de la ventana.
    pub canvas_size: f32,
    /// Zoom de la cámara (1 = ajustado a la ventana).
    pub zoom_level: f32,
    pub paused: bool,

    // Telemetría que llega de la simulación (solo para mostrar).
    pub particle_count: usize,
    pub fps: i32,

    /// `true` cuando esta UI corre en la ventana aparte (`panel`); cambia el
    /// botón de separar por uno de reacoplar.
    pub standalone: bool,
}

impl Default for PanelState {
    fn default() -> Self {
        Self {
            fill_count: 2000,
            active_color: 0,
            brush: Brush::Add,
            brush_size: 14.0,
            canvas_size: 800.0,
            zoom_level: 1.0,
            paused: false,
            particle_count: 0,
            fps: 0,
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
    /// Ajustar el zoom para que el lienzo entre en la ventana.
    FitCanvas,
    /// Igualar el lienzo a los píxeles de la ventana de simulación (1:1).
    CanvasEqualsScreen,
    /// Abrir el panel en una ventana del SO aparte.
    Detach,
    /// Volver a acoplar el panel dentro de la ventana de simulación.
    Reattach,
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
        ui.heading("Simulación");

        // Separar / reacoplar el panel.
        if st.standalone {
            if ui.button("⮌ Reacoplar en la ventana").clicked() {
                events.push(PanelEvent::Reattach);
            }
        } else if ui.button("🗗 Separar panel en otra ventana").clicked() {
            events.push(PanelEvent::Detach);
        }

        ui.label(format!("Partículas: {}", st.particle_count));
        ui.label(format!("FPS: {}", st.fps));
        ui.horizontal(|ui| {
            if ui
                .button(if st.paused { "▶ Reanudar" } else { "⏸ Pausa" })
                .clicked()
            {
                st.paused = !st.paused;
            }
            if ui.button("⏭ Paso").clicked() {
                st.paused = true;
                events.push(PanelEvent::Step);
            }
            if ui.button("⟲ Reiniciar").clicked() {
                events.push(PanelEvent::Clear);
            }
        });
        // --- Velocidad (en %, con cambio suave y atajos rápidos) ---
        ui.checkbox(&mut params.speed_smooth, "Cambio de velocidad suave");
        if params.speed_smooth {
            ui.add(
                egui::Slider::new(&mut params.speed_transition_duration, 0.1..=10.0)
                    .logarithmic(true)
                    .text("Duración cambio (s)"),
            );
        }
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
        // Botones rápidos del 10 % al 100 %.
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
        ui.label(format!(
            "Actual: {:.0}%  ·  objetivo: {:.0}%",
            params.time_scale * 100.0,
            params.speed_target * 100.0
        ));

        ui.separator();
        ui.heading("Llenar aleatorio");
        ui.add(egui::Slider::new(&mut st.fill_count, 100..=20000).text("Cantidad"));
        ui.horizontal(|ui| {
            if ui.button("Llenar").clicked() {
                events.push(PanelEvent::Fill(st.fill_count as usize));
            }
            if ui.button("Vaciar").clicked() {
                events.push(PanelEvent::Clear);
            }
        });

        ui.separator();
        ui.heading("Física");
        ui.add(egui::Slider::new(&mut params.force, 0.0..=5.0).text("Fuerza"));
        ui.add(egui::Slider::new(&mut params.r_max, 20.0..=200.0).text("Radio (r_max)"));
        ui.add(egui::Slider::new(&mut params.beta, 0.05..=0.9).text("Repulsión (β)"));
        ui.add(egui::Slider::new(&mut params.friction, 0.50..=0.99).text("Fricción"));
        ui.horizontal(|ui| {
            ui.label("Bordes:");
            ui.selectable_value(&mut params.boundary, Boundary::Wrap, "Toroidal");
            ui.selectable_value(&mut params.boundary, Boundary::Bounce, "Rebote");
        });

        ui.separator();
        ui.heading("Apariencia");
        ui.add(egui::Slider::new(&mut params.point_size, 1.0..=40.0).text("Tamaño punto"));
        ui.add(egui::Slider::new(&mut params.brightness, 0.1..=1.0).text("Brillo"));
        ui.horizontal(|ui| {
            ui.label("Estilo:");
            ui.selectable_value(&mut params.style, RenderStyle::Solid, "Sólido");
            ui.selectable_value(&mut params.style, RenderStyle::Glow, "Brillo");
            ui.selectable_value(&mut params.style, RenderStyle::SolidHalo, "Sólido+halo");
        });

        ui.separator();
        ui.heading("Lienzo");
        ui.add(
            egui::Slider::new(&mut st.canvas_size, 200.0..=6000.0)
                .logarithmic(true)
                .text("Tamaño"),
        );
        if ui.button("📐 Lienzo = pantalla").clicked() {
            events.push(PanelEvent::CanvasEqualsScreen);
        }
        ui.label("Menos = más reducido y denso · Más = más espacio");

        ui.separator();
        ui.heading("Vista");
        ui.add(
            egui::Slider::new(&mut st.zoom_level, 0.05..=30.0)
                .logarithmic(true)
                .text("Zoom"),
        );
        if ui.button("Ajustar al lienzo").clicked() {
            events.push(PanelEvent::FitCanvas);
        }
        ui.label("Rueda = zoom · botón derecho = mover");

        ui.separator();
        ui.heading("Interacción");
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

        // Congelamos la interacción ANTES de aplicar cambios para poder mezclar
        // de forma continua hacia la nueva.
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
            if ui.button("🎲 Aleatorizar reglas").clicked() {
                // La matriz es propiedad del panel: la aleatorizamos aquí mismo
                // para que el nuevo estado fluya al `sim` por `State` y no lo
                // pise el eco de la matriz anterior (bug en modo separado).
                params.randomize_matrix(&mut ::rand::thread_rng());
                trigger = true;
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

        if params.mode != old_mode {
            trigger = true;
        }
        if trigger {
            events.push(PanelEvent::StartTransition(snap_before));
        }

        ui.separator();
        ui.heading("Dinámica del color");
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

        ui.separator();
        ui.heading("Pintar");
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
    });

    events
}
