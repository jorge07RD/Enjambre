mod config;
mod grid;
mod render;
mod simulation;

use egui_macroquad::egui;
use macroquad::prelude::*;
use ::rand::Rng;

use config::*;
use render::Renderer;
use simulation::Simulation;

fn window_conf() -> Conf {
    Conf {
        window_title: "Puntos de Atracción".to_owned(),
        window_width: 1280,
        window_height: 800,
        high_dpi: false,
        ..Default::default()
    }
}

#[derive(PartialEq)]
enum Brush {
    Add,
    Erase,
}

fn egui_color(c: Color) -> egui::Color32 {
    egui::Color32::from_rgb(
        (c.r * 255.0) as u8,
        (c.g * 255.0) as u8,
        (c.b * 255.0) as u8,
    )
}

/// Construye la cámara 2D para un nivel de zoom y un punto del mundo centrado.
/// Zoom mayor = se ve una porción más pequeña del mundo = más cerca.
fn make_camera(zoom: f32, target: Vec2) -> Camera2D {
    let vw = screen_width() / zoom;
    let vh = screen_height() / zoom;
    Camera2D::from_display_rect(Rect::new(target.x - vw / 2.0, target.y - vh / 2.0, vw, vh))
}

#[macroquad::main(window_conf)]
async fn main() {
    let mut world = Vec2::new(screen_width(), screen_height());
    let mut sim = Simulation::new(world);
    let mut params = SimParams::default();
    let palette = palette();
    let mut renderer = Renderer::new();
    let mut rng = ::rand::thread_rng();

    let mut paused = false;
    let mut step_once = false;
    let mut fill_count: i32 = 2000;
    let mut active_color: usize = 0;
    let mut brush = Brush::Add;
    let mut brush_size: f32 = 14.0;

    // Lienzo: un único tamaño (alto); el ancho se deriva del aspecto de la
    // ventana para que siempre se parezca a la pantalla.
    let mut canvas_size: f32 = world.y;

    // Cámara: zoom (1 = ajustado a la ventana) y punto del mundo centrado.
    let mut zoom_level: f32 = 1.0;
    let mut pan_target = world * 0.5;
    let mut last_mouse = Vec2::from(mouse_position());

    // Llenado inicial para que haya algo que ver al arrancar.
    sim.spawn_random(fill_count as usize, &mut rng);

    loop {
        // El lienzo mantiene el aspecto de la ventana; su tamaño lo fija un
        // único control. La cámara (zoom/pan) sirve para verlo de cerca o lejos.
        let aspect = screen_width() / screen_height();
        world = Vec2::new(canvas_size * aspect, canvas_size);
        sim.world = world;

        // Física.
        if !paused || step_once {
            sim.apply_dynamics(&mut params, &mut rng, get_frame_time());
            sim.step(&params);
            // La transición avanza con tiempo real (independiente de los FPS).
            params.advance_transition(get_frame_time());
            step_once = false;
        }

        // Panel de control.
        let mut want_pointer = false;
        egui_macroquad::ui(|ctx| {
            want_pointer = ctx.wants_pointer_input();
            egui::SidePanel::right("panel")
                .default_width(310.0)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.heading("Simulación");
                        ui.label(format!("Partículas: {}", sim.particles.len()));
                        ui.label(format!("FPS: {}", get_fps()));
                        ui.horizontal(|ui| {
                            if ui
                                .button(if paused { "▶ Reanudar" } else { "⏸ Pausa" })
                                .clicked()
                            {
                                paused = !paused;
                            }
                            if ui.button("⏭ Paso").clicked() {
                                step_once = true;
                                paused = true;
                            }
                            if ui.button("⟲ Reiniciar").clicked() {
                                sim.clear();
                            }
                        });
                        ui.add(
                            egui::Slider::new(&mut params.time_scale, 0.0..=3.0).text("Velocidad"),
                        );

                        ui.separator();
                        ui.heading("Llenar aleatorio");
                        ui.add(egui::Slider::new(&mut fill_count, 100..=20000).text("Cantidad"));
                        ui.horizontal(|ui| {
                            if ui.button("Llenar").clicked() {
                                sim.spawn_random(fill_count as usize, &mut rng);
                            }
                            if ui.button("Vaciar").clicked() {
                                sim.clear();
                            }
                        });

                        ui.separator();
                        ui.heading("Física");
                        ui.add(egui::Slider::new(&mut params.force, 0.0..=5.0).text("Fuerza"));
                        ui.add(
                            egui::Slider::new(&mut params.r_max, 20.0..=200.0).text("Radio (r_max)"),
                        );
                        ui.add(
                            egui::Slider::new(&mut params.beta, 0.05..=0.9).text("Repulsión (β)"),
                        );
                        ui.add(
                            egui::Slider::new(&mut params.friction, 0.50..=0.99).text("Fricción"),
                        );
                        ui.horizontal(|ui| {
                            ui.label("Bordes:");
                            ui.selectable_value(&mut params.boundary, Boundary::Wrap, "Toroidal");
                            ui.selectable_value(&mut params.boundary, Boundary::Bounce, "Rebote");
                        });

                        ui.separator();
                        ui.heading("Apariencia");
                        ui.add(
                            egui::Slider::new(&mut params.point_size, 1.0..=40.0)
                                .text("Tamaño punto"),
                        );
                        ui.add(
                            egui::Slider::new(&mut params.brightness, 0.1..=1.0).text("Brillo"),
                        );
                        ui.horizontal(|ui| {
                            ui.label("Estilo:");
                            ui.selectable_value(&mut params.style, RenderStyle::Solid, "Sólido");
                            ui.selectable_value(&mut params.style, RenderStyle::Glow, "Brillo");
                            ui.selectable_value(
                                &mut params.style,
                                RenderStyle::SolidHalo,
                                "Sólido+halo",
                            );
                        });

                        ui.separator();
                        ui.heading("Lienzo");
                        ui.add(
                            egui::Slider::new(&mut canvas_size, 200.0..=6000.0)
                                .logarithmic(true)
                                .text("Tamaño"),
                        );
                        ui.label("Menos = más reducido y denso · Más = más espacio");

                        ui.separator();
                        ui.heading("Vista");
                        ui.add(
                            egui::Slider::new(&mut zoom_level, 0.05..=30.0)
                                .logarithmic(true)
                                .text("Zoom"),
                        );
                        if ui.button("Ajustar al lienzo").clicked() {
                            zoom_level = (screen_width() / world.x)
                                .min(screen_height() / world.y)
                                .clamp(0.02, 30.0);
                            pan_target = world * 0.5;
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

                        // Congelamos la interacción ANTES de aplicar cambios
                        // para poder mezclar de forma continua hacia la nueva.
                        let snap_before = params.current_snapshot();
                        let old_mode = params.mode;
                        let mut trigger = false;

                        ui.horizontal(|ui| {
                            ui.selectable_value(
                                &mut params.mode,
                                InteractionMode::SameColorOnly,
                                "Mismo color",
                            );
                            ui.selectable_value(
                                &mut params.mode,
                                InteractionMode::Matrix,
                                "Matriz",
                            );
                            ui.selectable_value(
                                &mut params.mode,
                                InteractionMode::Similarity,
                                "Similitud",
                            );
                        });
                        if params.mode == InteractionMode::SameColorOnly {
                            if ui
                                .checkbox(
                                    &mut params.same_repel_others,
                                    "Repeler colores distintos",
                                )
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
                            ui.add(
                                egui::Slider::new(&mut params.sim_range, 0.02..=0.5)
                                    .text("Tolerancia de color"),
                            );
                        }
                        if params.mode == InteractionMode::Matrix {
                            if ui.button("🎲 Aleatorizar reglas").clicked() {
                                params.randomize_matrix(&mut rng);
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

                        if params.mode != old_mode {
                            trigger = true;
                        }
                        if trigger {
                            params.start_transition(snap_before);
                        }

                        ui.separator();
                        ui.heading("Dinámica del color");
                        ui.checkbox(
                            &mut params.random_color,
                            "Cambio aleatorio de color",
                        );
                        if params.random_color {
                            ui.add(
                                egui::Slider::new(&mut params.random_color_rate, 0.0..=0.5)
                                    .text("Ritmo"),
                            );
                        }
                        ui.checkbox(
                            &mut params.gradual,
                            "Deriva lenta (color y atracción)",
                        );
                        if params.gradual {
                            ui.add(
                                egui::Slider::new(&mut params.gradual_color_speed, 0.0..=0.1)
                                    .text("Vel. color"),
                            );
                            ui.add(
                                egui::Slider::new(&mut params.gradual_matrix_speed, 0.0..=0.1)
                                    .text("Vel. atracción"),
                            );
                        }
                        ui.checkbox(&mut params.color_smooth, "Transición fluida (color)");
                        if params.color_smooth {
                            ui.add(
                                egui::Slider::new(
                                    &mut params.color_transition_duration,
                                    0.2..=20.0,
                                )
                                .logarithmic(true)
                                .text("Duración color (s)"),
                            );
                        }

                        ui.separator();
                        ui.heading("Pintar");
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut brush, Brush::Add, "Añadir");
                            ui.selectable_value(&mut brush, Brush::Erase, "Borrar");
                        });
                        ui.horizontal_wrapped(|ui| {
                            for i in 0..NUM_COLORS {
                                let selected = active_color == i;
                                let label = if selected { "●" } else { "○" };
                                if ui
                                    .add(
                                        egui::Button::new(label).fill(egui_color(palette[i])),
                                    )
                                    .on_hover_text(COLOR_NAMES[i])
                                    .clicked()
                                {
                                    active_color = i;
                                }
                            }
                        });
                        ui.add(egui::Slider::new(&mut brush_size, 2.0..=60.0).text("Brocha"));
                        ui.label("Click/arrastra en el lienzo para pintar o borrar.");
                    });
                });
        });

        // --- Cámara: zoom y desplazamiento ---
        let mouse = Vec2::from(mouse_position());

        // Zoom con la rueda, hacia el cursor (mantiene fijo el punto bajo él).
        let wheel = mouse_wheel().1;
        if wheel != 0.0 && !want_pointer {
            let world_before = make_camera(zoom_level, pan_target).screen_to_world(mouse);
            let factor = if wheel > 0.0 { 1.15 } else { 1.0 / 1.15 };
            zoom_level = (zoom_level * factor).clamp(0.2, 30.0);
            let world_after = make_camera(zoom_level, pan_target).screen_to_world(mouse);
            pan_target += world_before - world_after;
        }

        // Desplazamiento arrastrando con el botón derecho o central. Usamos
        // screen_to_world (que ya tiene en cuenta el volteo de Y de la cámara)
        // para que el punto agarrado quede fijo bajo el cursor.
        if is_mouse_button_down(MouseButton::Right) || is_mouse_button_down(MouseButton::Middle) {
            let cam = make_camera(zoom_level, pan_target);
            pan_target += cam.screen_to_world(last_mouse) - cam.screen_to_world(mouse);
        }
        last_mouse = mouse;

        let camera = make_camera(zoom_level, pan_target);

        // Pintar/borrar (solo fuera del panel) usando coordenadas del mundo.
        if !want_pointer && is_mouse_button_down(MouseButton::Left) {
            let pos = camera.screen_to_world(mouse);
            match brush {
                Brush::Add => {
                    let count = (brush_size / 5.0).max(1.0) as usize;
                    for _ in 0..count {
                        let ang = rng.gen_range(0.0..std::f32::consts::TAU);
                        let rad = rng.gen_range(0.0..brush_size);
                        sim.add(
                            pos + Vec2::new(ang.cos() * rad, ang.sin() * rad),
                            hue_for_index(active_color),
                        );
                    }
                }
                Brush::Erase => sim.erase_near(pos, brush_size),
            }
        }

        // Render del mundo con la cámara, y luego el panel encima.
        set_camera(&camera);
        renderer.draw(&sim, &params);
        // Borde del lienzo (grosor constante en pantalla).
        draw_rectangle_lines(
            0.0,
            0.0,
            world.x,
            world.y,
            2.0 / zoom_level,
            Color::new(0.3, 0.3, 0.35, 1.0),
        );
        set_default_camera();
        egui_macroquad::draw();
        next_frame().await;
    }
}
