//! Pestaña "Aspecto": Apariencia · Dinámica del color · Lienzo y vista.

use super::{PanelEvent, PanelState};
use crate::config::{RenderStyle, SimParams};
use crate::ui_kit::{self as k, slider_row};
use crate::ui_theme::icon;

pub(super) fn show(
    ui: &mut egui::Ui,
    params: &mut SimParams,
    st: &mut PanelState,
    events: &mut Vec<PanelEvent>,
) {
    // ===================== Apariencia =====================
    k::card(ui, "look", icon::H_LOOK, "Apariencia", true, |ui| {
        slider_row("Tamaño punto", &mut params.point_size, 1.0..=40.0).show(ui);
        slider_row("Brillo", &mut params.brightness, 0.1..=1.0).show(ui);
        k::sub_label(ui, "Estilo");
        k::segmented(
            ui,
            &mut params.style,
            &[
                (RenderStyle::Solid, "Sólido"),
                (RenderStyle::Glow, "Brillo"),
                (RenderStyle::SolidHalo, "Sólido+halo"),
            ],
        );
        k::switch_row(ui, "Estelas de movimiento", &mut params.trails, Some("E"));
        if params.trails {
            k::nested(ui, |ui| {
                // Menor desvanecido = estela más larga; invertimos el slider para
                // que "Longitud" crezca hacia la derecha.
                let mut length = 1.0 - params.trail_fade;
                if slider_row("Longitud de estela", &mut length, 0.5..=0.99)
                    .show(ui)
                    .changed()
                {
                    params.trail_fade = 1.0 - length;
                }
            });
        }
        let mut orient_on = params.orient > 0.5;
        if k::switch_row(
            ui,
            "Orientar según movimiento (flechas)",
            &mut orient_on,
            None,
        )
        .changed()
        {
            params.orient = if orient_on { 1.0 } else { 0.0 };
        }
        k::switch_row(ui, "Bloom (resplandor)", &mut params.bloom, Some("Y"));
        if params.bloom {
            k::nested(ui, |ui| {
                slider_row("Intensidad bloom", &mut params.bloom_intensity, 0.0..=2.0).show(ui);
                slider_row("Radio bloom", &mut params.bloom_radius, 1.5..=12.0).show(ui);
            });
        }
    });

    // ===================== Dinámica del color =====================
    k::card(
        ui,
        "color",
        icon::H_COLOR,
        "Dinámica del color",
        false,
        |ui| {
            k::switch_row(
                ui,
                "Cambio aleatorio de color",
                &mut params.random_color,
                None,
            );
            if params.random_color {
                k::nested(ui, |ui| {
                    slider_row("Ritmo", &mut params.random_color_rate, 0.0..=0.5).show(ui);
                });
            }
            k::switch_row(
                ui,
                "Deriva lenta (color y atracción)",
                &mut params.gradual,
                Some("V"),
            );
            if params.gradual {
                k::nested(ui, |ui| {
                    slider_row("Vel. color", &mut params.gradual_color_speed, 0.0..=0.1)
                        .decimals(3)
                        .show(ui);
                    slider_row(
                        "Vel. atracción",
                        &mut params.gradual_matrix_speed,
                        0.0..=0.1,
                    )
                    .decimals(3)
                    .show(ui);
                });
            }
            k::switch_row(
                ui,
                "Transición fluida (color)",
                &mut params.color_smooth,
                None,
            );
            if params.color_smooth {
                k::nested(ui, |ui| {
                    slider_row(
                        "Duración color (s)",
                        &mut params.color_transition_duration,
                        0.2..=20.0,
                    )
                    .log()
                    .show(ui);
                });
            }
        },
    );

    // ===================== Lienzo y vista =====================
    k::card(
        ui,
        "canvas",
        icon::H_CANVAS,
        "Lienzo y vista",
        false,
        |ui| {
            slider_row("Tamaño lienzo", &mut st.canvas_size, 200.0..=6000.0)
                .log()
                .show(ui);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if k::button(ui, icon::DESKTOP, "Lienzo = pantalla")
                    .on_hover_text("L")
                    .clicked()
                {
                    events.push(PanelEvent::CanvasEqualsScreen);
                }
                k::kbd(ui, "L");
            });
            k::hint(ui, "Menos = más reducido y denso · Más = más espacio");
            k::divider(ui);
            slider_row("Zoom", &mut st.zoom_level, 0.05..=30.0)
                .log()
                .decimals(2)
                .show(ui);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if k::button(ui, icon::FIT, "Ajustar al lienzo")
                    .on_hover_text("Z")
                    .clicked()
                {
                    events.push(PanelEvent::FitCanvas);
                }
                k::kbd(ui, "Z");
            });
            k::hint(ui, "Rueda = zoom · botón derecho = mover");
        },
    );
}
