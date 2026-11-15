//! Pestaña "Salida": Grabación · Música y sincronía · Reactivo al audio.

use super::{PanelEvent, PanelState};
use crate::config::{AudioSource, AudioTarget, BeatAction, SimParams, FRAME_PRESETS};
use crate::ui_kit::{self as k, slider_row};
use crate::ui_theme::{self as t, icon};
use egui::{vec2, RichText};

pub(super) fn show(
    ui: &mut egui::Ui,
    params: &mut SimParams,
    st: &mut PanelState,
    events: &mut Vec<PanelEvent>,
) {
    // ===================== Grabación =====================
    k::card(ui, "rec", icon::H_REC, "Grabación", true, |ui| {
        let (ic, label) = if st.recording {
            (icon::STOP, "Detener grabación")
        } else {
            (icon::REC, "Grabar vídeo")
        };
        if k::danger_button(ui, ic, label, st.recording)
            .on_hover_text("R")
            .clicked()
        {
            events.push(PanelEvent::ToggleRecord);
        }
        let mut sf = st.show_frame;
        if k::switch_row(ui, "Mostrar encuadre de grabación", &mut sf, Some("G")).changed() {
            events.push(PanelEvent::ToggleFrame);
        }
        k::sub_label(ui, "Tamaño");
        let mut fp = st.frame_preset;
        let opts: Vec<(usize, &str)> = FRAME_PRESETS
            .iter()
            .enumerate()
            .map(|(i, (name, _, _))| (i, *name))
            .collect();
        if k::chips(ui, &mut fp, &opts) {
            events.push(PanelEvent::SetFramePreset(fp));
        }
        k::row(ui, "Salida", |ui| {
            ui.label(
                RichText::new(format!("{}×{} px", st.frame_w, st.frame_h))
                    .font(t::mono(12.0))
                    .color(t::TEXT_1),
            )
        });
        if k::button(ui, icon::CROSSHAIRS, "Centrar encuadre en la vista").clicked() {
            events.push(PanelEvent::CenterFrame);
        }
        k::hint(
            ui,
            "Arrastra el recuadro (mover) o una esquina (redimensionar) en el lienzo.",
        );
        k::divider(ui);
        k::sub_label(ui, "Carpeta");
        if k::button(ui, icon::FOLDER, "Carpeta de guardado…").clicked() {
            events.push(PanelEvent::PickVideoDir);
        }
        k::hint(
            ui,
            if st.video_dir.is_empty() {
                "Carpeta: (directorio actual)".to_string()
            } else {
                format!("Carpeta: {}", st.video_dir)
            },
        );
    });

    // ===================== Música y sincronía =====================
    // Música: se mezcla en el .mp4 al grabar (recortada a la duración).
    k::card(
        ui,
        "music",
        icon::H_MUSIC,
        "Música y sincronía",
        false,
        |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if k::button(ui, icon::MUSIC, "Música…").clicked() {
                    events.push(PanelEvent::PickMusic);
                }
                if !st.music_path.is_empty()
                    && ui
                        .add(k::icon_button(icon::RELEASE).danger())
                        .on_hover_text("Quitar música")
                        .clicked()
                {
                    st.music_path.clear();
                }
            });
            k::hint(
                ui,
                if st.music_path.is_empty() {
                    "Sin música (vídeo mudo).".to_string()
                } else {
                    let name = std::path::Path::new(&st.music_path)
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| st.music_path.clone());
                    format!("{} {name}", icon::MUSIC)
                },
            );

            // --- Sincronía con la música (envolvente + beats de la pista) ---
            if !st.music_path.is_empty() {
                k::divider(ui);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    if k::button(ui, icon::H_AUDIO, "Analizar pista")
                        .on_hover_text("Extrae la envolvente y los beats (unos segundos)")
                        .clicked()
                    {
                        events.push(PanelEvent::MusicAnalyze);
                    }
                    let pi = if st.music_previewing {
                        icon::STOP
                    } else {
                        icon::PLAY
                    };
                    if k::button(ui, pi, "Preescucha")
                        .on_hover_text("Reproducir la pista en vivo (sincronía aproximada)")
                        .clicked()
                    {
                        events.push(PanelEvent::MusicPreviewToggle);
                    }
                });
                ui.label(
                    RichText::new(if st.music_analyzed {
                        let bpm = st
                            .music_bpm
                            .map(|b| format!(" · ~{b:.0} BPM"))
                            .unwrap_or_default();
                        let m = (st.music_duration / 60.0) as u32;
                        let s = st.music_duration as u32 % 60;
                        format!("{} beats{bpm} · {m}:{s:02}", st.music_onsets)
                    } else {
                        "Pista sin analizar.".to_string()
                    })
                    .font(t::mono(11.0))
                    .color(if st.music_analyzed {
                        t::TEXT_2
                    } else {
                        t::TEXT_3
                    }),
                );
                k::switch_row(
                    ui,
                    "Sincronizar con la música",
                    &mut st.music_sync.enabled,
                    None,
                );
                if st.music_sync.enabled {
                    k::nested(ui, |ui| {
                        if !st.music_analyzed {
                            k::warn(ui, "Analiza la pista para que la sincronía actúe.");
                        }
                        k::sub_label(ui, "En cada beat");
                        k::chips_tips(
                            ui,
                            &mut st.music_sync.beat_action,
                            &[
                                (BeatAction::None, "Nada"),
                                (BeatAction::Pulse, "Pulso"),
                                (BeatAction::RandomizeMatrix, "Aleatorizar matriz"),
                                (BeatAction::NextScene, "Escena siguiente"),
                                (BeatAction::Shockwave, "Onda de choque"),
                            ],
                            &[(4, "Empuje radial desde el centro en cada beat (motor GPU)")],
                        );
                        if st.music_sync.beat_action != BeatAction::None {
                            let mut div = st.music_sync.beat_divisor.max(1) as i32;
                            if slider_row("Cada N beats", &mut div, 1..=16)
                                .show(ui)
                                .changed()
                            {
                                st.music_sync.beat_divisor = div as u32;
                            }
                        }
                        if matches!(
                            st.music_sync.beat_action,
                            BeatAction::Pulse | BeatAction::Shockwave
                        ) {
                            slider_row(
                                "Fuerza del pulso",
                                &mut st.music_sync.pulse_gain,
                                0.0..=4.0,
                            )
                            .show(ui);
                        }
                        k::switch_row(
                            ui,
                            "La envolvente modula (según 'Reactivo al audio')",
                            &mut st.music_sync.envelope_drive,
                            None,
                        )
                        .on_hover_text(
                            "Usa el objetivo e intensidad de la sección 'Reactivo al audio', \
                         pero con la pista en lugar del micrófono.",
                        );
                        if st.music_previewing {
                            slider_row(
                                "Latencia preescucha (s)",
                                &mut st.music_sync.latency_offset,
                                -0.5..=0.5,
                            )
                            .decimals(2)
                            .show(ui);
                        }
                        k::hint(
                        ui,
                        "Grabando, la sincronía es exacta en el .mp4; la preescucha es aproximada.",
                    );
                    });
                }
            }
        },
    );

    // ===================== Audio =====================
    k::card_switch(
        ui,
        "audio",
        icon::H_AUDIO,
        "Reactivo al audio",
        &mut params.audio_reactive,
        |ui| {
            k::sub_label(ui, "Fuente");
            k::segmented_tips(
                ui,
                &mut params.audio_source,
                &[
                    (AudioSource::Mic, "Micrófono"),
                    (AudioSource::System, "Sistema"),
                ],
                &[(
                    1,
                    "Lo que suena en el equipo (monitor de PulseAudio/PipeWire)",
                )],
            );
            k::sub_label(ui, "Modula");
            k::chips_tips(
                ui,
                &mut params.audio_target,
                &[
                    (AudioTarget::Speed, "Velocidad"),
                    (AudioTarget::Force, "Fuerza"),
                    (AudioTarget::Brightness, "Brillo"),
                    (AudioTarget::Size, "Tamaño"),
                    (AudioTarget::Bloom, "Resplandor"),
                ],
                &[(4, "Requiere el bloom activo (Apariencia)")],
            );
            slider_row("Intensidad", &mut params.audio_intensity, 0.0..=4.0).show(ui);
            k::switch_row(ui, "Bandas → colores", &mut params.audio_bands, None).on_hover_text(
                "Cada banda ilumina sus colores: graves = rojo/amarillo, \
                 medios = verde/cian, agudos = azul/magenta (motor GPU)",
            );
            k::hint(
                ui,
                if params.audio_source == AudioSource::System {
                    "Captura el audio del sistema (lo que suena en el equipo)."
                } else {
                    "Usa el micrófono/entrada por defecto del sistema."
                },
            );
        },
    );
}
