//! Pestaña "Simulación": Interacción · Física · Partículas y herramienta.

use super::{egui_color, PanelEvent, PanelState};
use crate::config::{
    palette, BoidsScope, Boundary, Brush, InteractionMode, SimParams, Tool, COLOR_NAMES, NUM_COLORS,
};
use crate::ui_kit::{self as k, slider_row};
use crate::ui_theme::icon;
use egui::Color32;

pub(super) fn show(
    ui: &mut egui::Ui,
    params: &mut SimParams,
    st: &mut PanelState,
    events: &mut Vec<PanelEvent>,
) {
    let pal = palette();
    let colors: Vec<Color32> = (0..NUM_COLORS).map(|i| egui_color(pal[i])).collect();

    // ===================== Interacción =====================
    k::card(
        ui,
        "interact",
        icon::H_INTERACT,
        "Interacción",
        true,
        |ui| {
            k::switch_row(ui, "Transición fluida", &mut params.smooth, None);
            if params.smooth {
                k::nested(ui, |ui| {
                    slider_row("Duración (s)", &mut params.transition_duration, 0.2..=60.0)
                        .log()
                        .show(ui);
                    if params.blend < 1.0 {
                        ui.add(
                            egui::ProgressBar::new(params.blend)
                                .text("transición")
                                .fill(crate::ui_theme::ACCENT.gamma_multiply(0.7))
                                .desired_height(16.0),
                        );
                    }
                });
            }

            // Congelamos la interacción ANTES de aplicar cambios para poder
            // mezclar de forma continua hacia la nueva.
            let snap_before = params.current_snapshot();
            let old_mode = params.mode;
            let mut trigger = false;

            k::sub_label(ui, "Modo");
            k::chips(
                ui,
                &mut params.mode,
                &[
                    (InteractionMode::SameColorOnly, "Mismo color"),
                    (InteractionMode::Matrix, "Matriz"),
                    (InteractionMode::Similarity, "Similitud"),
                    (InteractionMode::Cyclic, "Cíclico"),
                    (InteractionMode::Opposite, "Opuestos"),
                    (InteractionMode::PredatorPrey, "Depredador-presa"),
                    (InteractionMode::SelfRepel, "Repulsión propia"),
                    (InteractionMode::Boids, "Bandada"),
                ],
            );
            if params.mode == InteractionMode::SameColorOnly {
                if k::switch_row(
                    ui,
                    "Repeler colores distintos",
                    &mut params.same_repel_others,
                    None,
                )
                .changed()
                {
                    trigger = true;
                }
                if params.same_repel_others {
                    k::nested(ui, |ui| {
                        slider_row(
                            "Fuerza de repulsión",
                            &mut params.same_repel_strength,
                            0.0..=1.0,
                        )
                        .show(ui);
                    });
                }
            }
            if params.mode == InteractionMode::Similarity {
                k::hint(ui, "Atracción según cercanía de color en la rueda.");
                slider_row("Tolerancia de color", &mut params.sim_range, 0.02..=0.5).show(ui);
            }
            if params.mode == InteractionMode::Matrix {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if k::button(ui, icon::RANDOM, "Aleatorizar reglas")
                        .on_hover_text("M")
                        .clicked()
                    {
                        // La matriz es propiedad del panel: la aleatorizamos aquí
                        // mismo para que el nuevo estado fluya al `sim` por `State`
                        // y no lo pise el eco de la matriz anterior. El cruce
                        // (`MatrixBlend`) es suave siempre.
                        params.randomize_matrix(&mut ::rand::thread_rng());
                        events.push(PanelEvent::MatrixBlend(snap_before));
                    }
                    k::kbd(ui, "M");
                    if k::button(ui, icon::RESET, "Restablecer")
                        .on_hover_text("Matriz por defecto: cada color se atrae a sí mismo")
                        .clicked()
                    {
                        params.reset_matrix();
                        events.push(PanelEvent::MatrixBlend(snap_before));
                    }
                });
                k::switch_row(
                    ui,
                    "Auto-aleatorizar sola",
                    &mut params.auto_randomize,
                    Some("X"),
                );
                if params.auto_randomize {
                    k::nested(ui, |ui| {
                        slider_row("Cada (s)", &mut params.auto_randomize_interval, 1.0..=60.0)
                            .show(ui);
                    });
                }
                k::matrix_editor(ui, &mut params.matrix, &colors, &COLOR_NAMES);
                k::hint(ui, "Fila = recibe · Columna = ejerce");
            }
            if params.mode == InteractionMode::Cyclic {
                k::hint(
                    ui,
                    "Cada color persigue al siguiente de la rueda y huye del anterior.",
                );
            }
            if params.mode == InteractionMode::Opposite {
                k::hint(
                    ui,
                    "Los colores opuestos en la rueda se atraen; los parecidos se repelen.",
                );
            }
            if params.mode == InteractionMode::PredatorPrey {
                k::hint(
                    ui,
                    "Los colores pares cazan a los impares; las presas huyen en manada.",
                );
            }
            if params.mode == InteractionMode::SelfRepel {
                k::hint(
                    ui,
                    "El mismo color se repele y los distintos se atraen (mezcla homogénea).",
                );
            }
            if params.mode == InteractionMode::Boids {
                k::hint(
                    ui,
                    "Murmuración (Boids): separación, alineación y cohesión.",
                );
                k::sub_label(ui, "Bandada");
                k::segmented(
                    ui,
                    &mut params.boids_scope,
                    &[
                        (BoidsScope::All, "Todas"),
                        (BoidsScope::SameColor, "Por color"),
                        (BoidsScope::Hybrid, "Híbrido"),
                    ],
                );
                slider_row("Separación", &mut params.boids_separation, 0.0..=3.0).show(ui);
                slider_row("Alineación", &mut params.boids_alignment, 0.0..=3.0).show(ui);
                slider_row("Cohesión", &mut params.boids_cohesion, 0.0..=3.0).show(ui);
                if params.boids_scope != BoidsScope::All {
                    slider_row(
                        "Evitar otros grupos",
                        &mut params.boids_group_avoid,
                        0.0..=3.0,
                    )
                    .show(ui);
                }
                slider_row(
                    "Radio de separación",
                    &mut params.boids_sep_radius,
                    0.1..=1.0,
                )
                .show(ui);
                slider_row("Crucero", &mut params.boids_cruise, 0.0..=150.0).show(ui);
            }

            if params.mode != old_mode {
                trigger = true;
            }
            if trigger {
                events.push(PanelEvent::StartTransition(snap_before));
            }
        },
    );

    // ===================== Física =====================
    k::card(ui, "physics", icon::H_PHYSICS, "Física", false, |ui| {
        slider_row("Fuerza", &mut params.force, 0.0..=5.0).show(ui);
        slider_row("Radio (r_max)", &mut params.r_max, 20.0..=200.0).show(ui);
        slider_row("Repulsión (β)", &mut params.beta, 0.05..=0.9).show(ui);
        slider_row("Fricción", &mut params.friction, 0.50..=0.99).show(ui);
        k::row(ui, "Bordes", |ui| k::kbd(ui, "B"));
        k::segmented_tips(
            ui,
            &mut params.boundary,
            &[(Boundary::Wrap, "Toroidal"), (Boundary::Bounce, "Rebote")],
            &[(0, "Alternar con B"), (1, "Alternar con B")],
        );
        k::switch_row(
            ui,
            "Disolver aglomeraciones densas",
            &mut params.anti_clump,
            Some("U"),
        )
        .on_hover_text("U · compara con/sin al vuelo");
        if params.anti_clump {
            k::nested(ui, |ui| {
                slider_row(
                    "Umbral (× densidad media)",
                    &mut params.anti_clump_factor,
                    1.5..=10.0,
                )
                .show(ui);
                slider_row(
                    "Fuerza de dispersión",
                    &mut params.anti_clump_strength,
                    0.0..=3.0,
                )
                .show(ui);
                k::hint(
                    ui,
                    "Las bolas hiperpobladas se dispersan con suavidad en vez de vibrar.",
                );
            });
        }
        k::switch_row(
            ui,
            "Atraer zonas activas al centro",
            &mut params.attract_active,
            Some("A"),
        );
        if params.attract_active {
            k::nested(ui, |ui| {
                slider_row(
                    "Fuerza de recentrado",
                    &mut params.attract_active_strength,
                    0.0..=2.0,
                )
                .show(ui);
                k::hint(
                    ui,
                    "Los grupos densos se acercan suavemente al centro de la vista.",
                );
            });
        }
    });

    // ===================== Partículas y herramienta =====================
    k::card(
        ui,
        "tool",
        icon::H_TOOL,
        "Partículas y herramienta",
        false,
        |ui| {
            slider_row("Cantidad", &mut st.fill_count, 100..=20000).show(ui);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if k::button(ui, icon::FILL, "Llenar")
                    .on_hover_text("F")
                    .clicked()
                {
                    events.push(PanelEvent::Fill(st.fill_count as usize));
                }
                k::kbd(ui, "F");
                ui.add_space(4.0);
                if k::button(ui, icon::ERASE, "Vaciar")
                    .on_hover_text("C")
                    .clicked()
                {
                    events.push(PanelEvent::Clear);
                }
                k::kbd(ui, "C");
            });
            k::divider(ui);
            k::sub_label(ui, "Herramienta del ratón");
            k::segmented(
                ui,
                &mut st.tool,
                &[(Tool::Brush, "Pincel"), (Tool::Force, "Fuerza")],
            );
            if st.tool == Tool::Force {
                k::segmented(
                    ui,
                    &mut params.pointer_repel,
                    &[(true, "Repeler"), (false, "Atraer")],
                );
                slider_row("Fuerza", &mut params.pointer_strength, 0.0..=3.0).show(ui);
                slider_row("Radio", &mut params.pointer_radius, 30.0..=600.0).show(ui);
                k::hint(
                    ui,
                    "Click/arrastra en el lienzo para atraer o espantar el enjambre.",
                );
            } else {
                k::segmented(
                    ui,
                    &mut st.brush,
                    &[(Brush::Add, "Añadir"), (Brush::Erase, "Borrar")],
                );
                k::swatches(ui, &mut st.active_color, &colors, &COLOR_NAMES);
                slider_row("Brocha", &mut st.brush_size, 2.0..=60.0).show(ui);
                k::hint(ui, "Click/arrastra en el lienzo para pintar o borrar.");
            }
        },
    );
}
