//! Cabecera fija: marca, estado en vivo, separar/ocultar, transporte y velocidad.

use super::{PanelEvent, PanelState};
use crate::config::SimParams;
use crate::ui_kit::{self as k, slider_row};
use crate::ui_theme::{self as t, icon};
use egui::{vec2, Align, Layout, RichText, Sense};

/// "12345" → "12 345" (separador de miles fino, legible en mono).
fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push('\u{202f}');
        }
        out.push(ch);
    }
    out
}

pub(super) fn show(
    ui: &mut egui::Ui,
    params: &mut SimParams,
    st: &mut PanelState,
    events: &mut Vec<PanelEvent>,
) {
    // --- Marca + acciones de ventana ---
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Enjambre")
                .font(t::strong(17.0))
                .color(t::TEXT_1),
        );
        // Punto de estado: verde = corriendo, ámbar = en pausa.
        let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
        let c = if st.paused { t::WARN } else { t::OK };
        ui.painter().circle_filled(rect.center(), 3.5, c);
        ui.painter()
            .circle_filled(rect.center(), 6.0, c.gamma_multiply(0.18));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            // Solo el panel acoplado se puede ocultar (el separado es su
            // propia ventana del SO: basta cerrarla).
            if !st.standalone && k::icon_btn(ui, icon::HIDE, "Ocultar panel (H)").clicked() {
                events.push(PanelEvent::HidePanel);
            }
            // Separar / reacoplar el panel en su propia ventana.
            if st.standalone {
                if k::icon_btn(ui, icon::REATTACH, "Reacoplar en la ventana (D)").clicked() {
                    events.push(PanelEvent::Reattach);
                }
            } else if k::icon_btn(ui, icon::DETACH, "Separar panel en otra ventana (D)").clicked() {
                events.push(PanelEvent::Detach);
            }
        });
    });

    // --- Telemetría ---
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        k::stat_chip(
            ui,
            &format!("{} partículas", thousands(st.particle_count)),
            None,
        );
        let fps_c = if st.fps >= 50 {
            t::OK
        } else if st.fps >= 30 {
            t::WARN
        } else {
            t::DANGER
        };
        k::stat_chip(ui, &format!("{} FPS", st.fps), Some(fps_c));
        if st.recording {
            k::rec_chip(ui, st.recording_secs);
        }
    });

    // --- Transporte + velocidad ---
    k::plain_card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let bw = ((ui.available_width() - 12.0) / 3.0).floor();
            let (pi, pl) = if st.paused {
                (icon::PLAY, "Reanudar")
            } else {
                (icon::PAUSE, "Pausa")
            };
            if k::button_w(ui, pi, pl, bw, true)
                .on_hover_text("Espacio")
                .clicked()
            {
                st.paused = !st.paused;
            }
            if k::button_w(ui, icon::STEP, "Paso", bw, false)
                .on_hover_text(".")
                .clicked()
            {
                st.paused = true;
                events.push(PanelEvent::Step);
            }
            if k::button_w(ui, icon::RESET, "Reiniciar", bw, false)
                .on_hover_text("C")
                .clicked()
            {
                events.push(PanelEvent::Clear);
            }
        });

        // Velocidad (en %, con cambio suave y atajos rápidos).
        let mut pct = params.speed_target * 100.0;
        if slider_row("Velocidad", &mut pct, 0.0..=300.0)
            .suffix(" %")
            .decimals(0)
            .show(ui)
            .changed()
        {
            params.speed_target = pct / 100.0; // evita que el slider rebote
            events.push(PanelEvent::SetSpeed(pct / 100.0));
        }
        const PRESETS: [&str; 10] = ["10", "20", "30", "40", "50", "60", "70", "80", "90", "100"];
        let mut sel = (0..10).find(|&i| (params.speed_target - (i + 1) as f32 / 10.0).abs() < 1e-3);
        let opts: Vec<(Option<usize>, &str)> = (0..10).map(|i| (Some(i), PRESETS[i])).collect();
        let tips: Vec<(usize, &str)> = (0..10)
            .map(|i| (i, "Teclas 1…0 = 10\u{a0}%…100\u{a0}%"))
            .collect();
        if k::segmented_tips(ui, &mut sel, &opts, &tips) {
            if let Some(i) = sel {
                let v = (i + 1) as f32 / 10.0;
                params.speed_target = v;
                events.push(PanelEvent::SetSpeed(v));
            }
        }
        // Cambio suave en una sola fila: etiqueta · duración · interruptor.
        // Lo de la derecha va primero; la etiqueta se recorta con lo que sobre.
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add(k::toggle(&mut params.speed_smooth));
                if params.speed_smooth {
                    ui.scope(|ui| {
                        ui.style_mut().override_font_id = Some(t::mono(12.0));
                        ui.spacing_mut().interact_size.y = 20.0;
                        ui.add(
                            egui::DragValue::new(&mut params.speed_transition_duration)
                                .range(0.1..=10.0)
                                .speed(0.05)
                                .max_decimals(1)
                                .suffix(" s"),
                        )
                        .on_hover_text("Duración cambio (s)");
                    });
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Label::new(
                                RichText::new("Cambio de velocidad suave").color(t::TEXT_1),
                            )
                            .truncate()
                            .sense(Sense::click()),
                        )
                        .clicked()
                    {
                        params.speed_smooth = !params.speed_smooth;
                    }
                });
            });
        });
        // Solo mientras la velocidad está transitando: de dónde a dónde va.
        if (params.time_scale - params.speed_target).abs() > 0.005 {
            k::hint(
                ui,
                format!(
                    "Actual {:.0}% → objetivo {:.0}%",
                    params.time_scale * 100.0,
                    params.speed_target * 100.0
                ),
            );
        }
    });
}
