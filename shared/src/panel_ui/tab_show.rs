//! Pestaña "Show": Mensaje / Forma · Escenas · Secuenciador.

use super::{egui_color, load_image_preview, select_shape, PanelEvent, PanelState};
use crate::config::{palette, SimParams, COLOR_NAMES, NUM_COLORS};
use crate::playlist::{PlaylistEntry, SeqPlayback};
use crate::ui_kit::{self as k, icon_button, slider_row};
use crate::ui_theme::{self as t, icon};
use crate::video::is_video_path;
use egui::{vec2, Align, Color32, Layout, RichText};

/// `TextEdit` de una línea con la altura de los controles.
fn text_field<'a>(text: &'a mut String, hint: &'a str, w: f32) -> egui::TextEdit<'a> {
    // `desired_width` no incluye el margen interior (8 px por lado).
    let w = (w - 16.0).max(40.0);
    egui::TextEdit::singleline(text)
        .hint_text(hint)
        .desired_width(w)
        .margin(vec2(8.0, 6.0))
}

pub(super) fn show(
    ui: &mut egui::Ui,
    params: &mut SimParams,
    st: &mut PanelState,
    events: &mut Vec<PanelEvent>,
) {
    let pal = palette();
    let colors: Vec<Color32> = (0..NUM_COLORS).map(|i| egui_color(pal[i])).collect();

    // ===================== Mensaje / Forma =====================
    k::card(ui, "shape", icon::H_SHAPE, "Mensaje / Forma", true, |ui| {
        // Escribir un texto y aplicarlo (Enter o el botón).
        // Botón a la derecha primero; el campo ocupa el resto (su ancho ya
        // descuenta el margen interior, así no desborda la tarjeta).
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let apply = k::primary_button(ui, icon::APPLY, "Aplicar").clicked();
                let te = ui.add(text_field(
                    &mut st.shape_text,
                    "escribe un mensaje",
                    ui.available_width(),
                ));
                let enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (apply || enter) && !st.shape_text.trim().is_empty() {
                    events.push(PanelEvent::FormText(st.shape_text.clone()));
                }
            });
        });
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            if k::button(ui, icon::IMAGE, "Imagen…").clicked() {
                events.push(PanelEvent::FormImagePick);
            }
            if k::button(ui, icon::IMAGE, "Varias…")
                .on_hover_text(
                    "Elegir varias imágenes/vídeos a la vez: se guardan todas en la \
                     biblioteca (la última queda aplicada).",
                )
                .clicked()
            {
                events.push(PanelEvent::FormImagesPick);
            }
            if k::button(ui, icon::SAVE, "Guardar")
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
            if k::button(ui, icon::RELEASE, "Soltar")
                .on_hover_text("S")
                .clicked()
            {
                events.push(PanelEvent::ReleaseShape);
            }
            k::kbd(ui, "S");
        });
        // Miniatura de la foto activa, para comparar contra lo que forman
        // las partículas. Se recarga solo si cambió la ruta.
        if !params.shape_image.is_empty() {
            let stale = st
                .shape_image_preview
                .as_ref()
                .is_none_or(|(p, _)| p != &params.shape_image);
            if stale {
                st.shape_image_preview = load_image_preview(ui.ctx(), &params.shape_image)
                    .map(|tex| (params.shape_image.clone(), tex));
            }
            if let Some((_, tex)) = &st.shape_image_preview {
                let max = vec2(ui.available_width(), 160.0);
                ui.vertical_centered(|ui| {
                    ui.add(
                        egui::Image::new(tex)
                            .max_size(max)
                            .shrink_to_fit()
                            .corner_radius(t::R_CTRL),
                    );
                });
            }
        }
        slider_row(
            "Fluidez (s)",
            &mut params.shape_transition_duration,
            0.0..=8.0,
        )
        .show(ui);
        slider_row("Fijación", &mut params.shape_strength, 0.0..=1.0).show(ui);
        slider_row(
            "Repulsión del fondo",
            &mut params.shape_avoid_gain,
            0.0..=30.0,
        )
        .hint(
            "Cuánto esquiva el resto del enjambre a la figura/foto/vídeo: más alto = se \
                 lee mejor el contenido, menos partículas encima.",
        )
        .show(ui);
        if k::switch_row(ui, "Teñir de un color", &mut params.shape_tint, None).changed()
            && params.shape_tint
        {
            params.shape_photo_color = false; // mutuamente excluyentes
        }
        if params.shape_tint {
            k::nested(ui, |ui| {
                k::swatches(ui, &mut params.shape_color, &colors, &COLOR_NAMES);
            });
        }
        if !params.shape_image.is_empty() {
            if k::switch_row(
                ui,
                "Recrear colores de la foto",
                &mut params.shape_photo_color,
                None,
            )
            .on_hover_text(
                "Cada partícula de la imagen migra hacia su color real (solo imágenes, no texto).",
            )
            .changed()
                && params.shape_photo_color
            {
                params.shape_tint = false; // mutuamente excluyentes
            }
            if is_video_path(&params.shape_image) {
                k::switch_row(
                    ui,
                    "Quitar fondo negro del vídeo",
                    &mut params.video_key_black,
                    None,
                )
                .on_hover_text(
                    "Los píxeles casi negros del vídeo se vuelven transparentes: se ven \
                     las partículas donde no hay contenido (ideal para clips con fondo \
                     negro, p. ej. Manim).",
                );
            }
        }
        k::hint(
            ui,
            "Fluidez = cuánto tarda en formarse/soltarse · más partículas = más legible.",
        );

        // --- Biblioteca de formas guardadas ---
        k::divider(ui);
        let shapes = st.saved_shapes.clone();
        let n = shapes.len();
        ui.horizontal(|ui| {
            k::sub_label(ui, "Biblioteca");
            if n > 0 {
                ui.label(
                    RichText::new(n.to_string())
                        .font(t::mono(11.0))
                        .color(t::TEXT_3),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    // Recorrer la biblioteca: carga y aplica el ítem, para ir
                    // preparando frases una tras otra.
                    if k::icon_btn(ui, icon::NEXT, "Siguiente").clicked() {
                        let idx = st.shape_sel.map(|c| (c + 1) % n).unwrap_or(0);
                        select_shape(st, events, &shapes, idx);
                    }
                    if k::icon_btn(ui, icon::PREV, "Anterior").clicked() {
                        let cur = st.shape_sel.unwrap_or(0);
                        let idx = (cur + n - 1) % n;
                        select_shape(st, events, &shapes, idx);
                    }
                });
            }
        });
        if n == 0 {
            k::hint(
                ui,
                format!(
                    "Vacía. Aplica un texto/imagen y pulsa {} Guardar.",
                    icon::SAVE
                ),
            );
        } else {
            ui.spacing_mut().item_spacing.y = 2.0;
            for (i, s) in shapes.iter().enumerate() {
                let ic = if s.is_image() {
                    icon::IMAGE
                } else {
                    icon::TEXT
                };
                let mut del = false;
                // Clic en el ítem: lo selecciona, lo carga en el campo
                // (para editarlo) y lo aplica.
                let resp = k::list_row(ui, st.shape_sel == Some(i), ic, &s.name, t::TEXT_1, |ui| {
                    if ui
                        .add(icon_button(icon::TRASH).danger())
                        .on_hover_text("Borrar")
                        .clicked()
                    {
                        del = true;
                    }
                });
                if resp.on_hover_text("Cargar y aplicar").clicked() {
                    select_shape(st, events, &shapes, i);
                }
                if del {
                    events.push(PanelEvent::DeleteShape(s.name.clone()));
                    if st.shape_sel == Some(i) {
                        st.shape_sel = None;
                    }
                }
            }
        }
    });

    // ===================== Escenas =====================
    k::card(ui, "scenes", icon::H_SCENES, "Escenas", false, |ui| {
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let save = k::primary_button(ui, icon::CAMERA, "Guardar").clicked();
                ui.add(text_field(
                    &mut st.scene_name_input,
                    "nombre",
                    ui.available_width(),
                ));
                if save && !st.scene_name_input.trim().is_empty() {
                    events.push(PanelEvent::SaveScene(
                        st.scene_name_input.trim().to_string(),
                    ));
                }
            });
        });
        k::switch_row(
            ui,
            "Transición suave entre escenas",
            &mut st.scene_smooth,
            None,
        );
        if st.scene_smooth {
            k::nested(ui, |ui| {
                slider_row(
                    "Duración (s)",
                    &mut st.scene_transition_duration,
                    0.2..=20.0,
                )
                .log()
                .show(ui);
            });
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            if k::button(ui, icon::PREV, "Anterior")
                .on_hover_text("P")
                .clicked()
            {
                events.push(PanelEvent::PrevScene);
            }
            k::kbd(ui, "P");
            ui.add_space(4.0);
            if k::button(ui, icon::NEXT, "Siguiente")
                .on_hover_text("N")
                .clicked()
            {
                events.push(PanelEvent::NextScene);
            }
            k::kbd(ui, "N");
        });
        k::switch_row(ui, "Auto-avance (slideshow)", &mut st.scene_autoplay, None);
        if st.scene_autoplay {
            k::nested(ui, |ui| {
                slider_row("Cada (s)", &mut st.scene_autoplay_interval, 1.0..=120.0)
                    .log()
                    .show(ui);
            });
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            if k::button(ui, icon::UPLOAD, "Exportar todas").clicked() {
                events.push(PanelEvent::ExportScenes);
            }
            if k::button(ui, icon::DOWNLOAD, "Importar…").clicked() {
                events.push(PanelEvent::ImportScenes);
            }
        });
        k::divider(ui);
        if st.scenes.is_empty() {
            k::hint(ui, "Aún no hay escenas. Escribe un nombre y pulsa Guardar.");
        } else {
            ui.spacing_mut().item_spacing.y = 2.0;
            for name in st.scenes.clone() {
                let is_def = st.default_scene == name;
                let ic = if is_def { icon::STAR } else { icon::H_SCENES };
                let resp = k::list_row(ui, false, ic, &name, t::TEXT_1, |ui| {
                    if ui
                        .add(icon_button(icon::TRASH).danger())
                        .on_hover_text("Borrar")
                        .clicked()
                    {
                        events.push(PanelEvent::DeleteScene(name.clone()));
                    }
                    if ui
                        .add(icon_button(icon::STAR).selected(is_def))
                        .on_hover_text("Predeterminada")
                        .clicked()
                    {
                        events.push(PanelEvent::SetDefaultScene(name.clone()));
                    }
                    if k::icon_btn(ui, icon::REPEAT, "Actualizar con la configuración actual")
                        .clicked()
                    {
                        // Sobrescribe la escena existente (upsert por nombre).
                        events.push(PanelEvent::SaveScene(name.clone()));
                    }
                    if k::icon_btn(ui, icon::PLAY, "Cargar").clicked() {
                        events.push(PanelEvent::LoadScene(name.clone()));
                    }
                });
                if resp.on_hover_text("Cargar").clicked() {
                    events.push(PanelEvent::LoadScene(name.clone()));
                }
            }
        }
    });

    // ===================== Secuenciador =====================
    // Playlist de escenas con duración propia por entrada, para montar un
    // "show" reproducible y grabable. La lista la edita el panel y viaja
    // completa al `sim` (`SeqSetPlaylist`); el estado de reproducción llega
    // de vuelta por telemetría (`seq_state`/`seq_idx`/`seq_elapsed`).
    k::card(ui, "seq", icon::H_SEQ, "Secuenciador", false, |ui| {
        let mut changed = false;

        // Transporte del show.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if st.seq_state == SeqPlayback::Playing {
                if ui
                    .add(icon_button(icon::PAUSE).selected(true))
                    .on_hover_text("Pausar el show")
                    .clicked()
                {
                    events.push(PanelEvent::SeqPause);
                }
            } else if k::icon_btn(ui, icon::PLAY, "Reproducir el show").clicked() {
                events.push(PanelEvent::SeqPlay);
            }
            if k::icon_btn(ui, icon::STOP, "Detener (vuelve al principio)").clicked() {
                events.push(PanelEvent::SeqStop);
            }
            if k::icon_btn(ui, icon::PREV, "Entrada anterior").clicked() {
                events.push(PanelEvent::SeqPrev);
            }
            if k::icon_btn(ui, icon::NEXT, "Entrada siguiente").clicked() {
                events.push(PanelEvent::SeqNext);
            }
        });

        let n = st.seq_playlist.entries.len();
        if n > 0 {
            let dur = st
                .seq_playlist
                .entries
                .get(st.seq_idx)
                .map_or(0.0, |e| e.duration);
            ui.label(
                RichText::new(format!(
                    "Entrada {}/{} · {:.0} / {:.0} s · show {:.0} s",
                    (st.seq_idx + 1).min(n),
                    n,
                    st.seq_elapsed,
                    dur,
                    st.seq_playlist.total_duration()
                ))
                .font(t::mono(11.0))
                .color(t::TEXT_3),
            );
        }
        if st.scene_autoplay && st.seq_state == SeqPlayback::Playing {
            k::warn(
                ui,
                "El auto-avance simple queda en espera mientras el show suena.",
            );
        }

        // Lista de entradas (editable). Las operaciones estructurales se
        // recogen y aplican tras el bucle para no mover índices en medio.
        let scenes = st.scenes.clone();
        let mut swap: Option<(usize, usize)> = None;
        let mut delete: Option<usize> = None;
        for i in 0..n {
            let name = st.seq_playlist.entries[i].scene.clone();
            let exists = scenes.contains(&name);
            let active = st.seq_idx == i && st.seq_state != SeqPlayback::Stopped;
            let col = if exists { t::TEXT_1 } else { t::DANGER };
            let resp = k::list_row(
                ui,
                active,
                "",
                &format!("{:>2}  {name}", i + 1),
                col,
                |ui| {
                    if ui
                        .add(icon_button(icon::TRASH).danger())
                        .on_hover_text("Quitar del show")
                        .clicked()
                    {
                        delete = Some(i);
                    }
                    if ui
                        .add_enabled(i + 1 < n, icon_button(icon::DOWN))
                        .on_hover_text("Bajar")
                        .clicked()
                    {
                        swap = Some((i, i + 1));
                    }
                    if ui
                        .add_enabled(i > 0, icon_button(icon::UP))
                        .on_hover_text("Subir")
                        .clicked()
                    {
                        swap = Some((i, i - 1));
                    }
                },
            );
            if resp
                .on_hover_text(if exists {
                    "Saltar a esta entrada"
                } else {
                    "La escena ya no existe (se salta al reproducir)"
                })
                .clicked()
            {
                events.push(PanelEvent::SeqJump(i));
            }
            // Segunda línea: duración y transición propia de la entrada.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.style_mut().override_font_id = Some(t::mono(11.5));
                ui.add_space(12.0);
                ui.label(RichText::new("\u{f017}").color(t::TEXT_3));
                if ui
                    .add(
                        egui::DragValue::new(&mut st.seq_playlist.entries[i].duration)
                            .speed(0.5)
                            .range(0.5..=600.0)
                            .suffix(" s"),
                    )
                    .on_hover_text("Duración total de la entrada (transición incluida)")
                    .changed()
                {
                    changed = true;
                }
                ui.add_space(4.0);
                // Transición propia opcional (si no, la global de Escenas).
                let mut own = st.seq_playlist.entries[i].transition.is_some();
                if ui
                    .add(k::toggle(&mut own))
                    .on_hover_text("Transición propia (si no, la global de Escenas)")
                    .changed()
                {
                    st.seq_playlist.entries[i].transition =
                        own.then_some(st.scene_transition_duration);
                    changed = true;
                }
                if let Some(tr) = st.seq_playlist.entries[i].transition.as_mut() {
                    if ui
                        .add(
                            egui::DragValue::new(tr)
                                .speed(0.1)
                                .range(0.05..=30.0)
                                .suffix(" s"),
                        )
                        .on_hover_text("Duración de la transición")
                        .changed()
                    {
                        changed = true;
                    }
                } else {
                    ui.label(
                        RichText::new("transición global")
                            .size(11.0)
                            .color(t::TEXT_3),
                    );
                }
            });
            ui.add_space(2.0);
        }
        if let Some((a, b)) = swap {
            st.seq_playlist.entries.swap(a, b);
            changed = true;
        }
        if let Some(i) = delete {
            st.seq_playlist.entries.remove(i);
            changed = true;
        }

        // Añadir una escena al show.
        if scenes.is_empty() {
            k::hint(ui, "Guarda escenas primero para poder añadirlas al show.");
        } else {
            st.seq_scene_pick = st.seq_scene_pick.min(scenes.len() - 1);
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if k::primary_button(ui, icon::FILL, "Añadir").clicked() {
                        st.seq_playlist.entries.push(PlaylistEntry {
                            scene: scenes[st.seq_scene_pick].clone(),
                            ..PlaylistEntry::default()
                        });
                        changed = true;
                    }
                    // El combo suma icono y relleno a `width`: se le deja ese margen.
                    let w = (ui.available_width() - 2.0 * ui.spacing().button_padding.x - 20.0)
                        .max(60.0);
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        egui::ComboBox::from_id_salt("seq_add")
                            .width(w)
                            .selected_text(scenes[st.seq_scene_pick].clone())
                            .show_ui(ui, |ui| {
                                for (i, name) in scenes.iter().enumerate() {
                                    ui.selectable_value(&mut st.seq_scene_pick, i, name);
                                }
                            });
                    });
                });
            });
        }

        if k::switch_row(
            ui,
            "Repetir al terminar (loop)",
            &mut st.seq_playlist.loop_at_end,
            None,
        )
        .changed()
        {
            changed = true;
        }
        if k::switch_row(
            ui,
            "Al grabar: empezar el show desde el principio",
            &mut st.seq_playlist.start_on_record,
            None,
        )
        .changed()
        {
            changed = true;
        }
        if st.seq_playlist.start_on_record && !st.seq_playlist.loop_at_end {
            k::hint(
                ui,
                "Sin loop: la grabación se detiene sola al acabar el show.",
            );
        }

        if changed {
            events.push(PanelEvent::SeqSetPlaylist(st.seq_playlist.clone()));
        }
    });
}
