//! Kit de componentes del panel "cristal de instrumento" (egui 0.31).
//!
//! Piezas sin estado global: la lámina de cristal, tarjetas de sección,
//! interruptores, filas de slider, controles segmentados, chips, muestras de
//! color, filas de lista, la barra de pestañas y el editor de la matriz. Los
//! colores y medidas salen de `ui_theme`.

use crate::ui_theme::{self as t, icon};
use egui::collapsing_header::CollapsingState;
use egui::emath::Numeric;
use egui::epaint::{Mesh, RectShape};
use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Galley, Id, Layout,
    Margin, Painter, Pos2, Rect, Response, RichText, Sense, Shape, Stroke, StrokeKind, Ui,
    UiBuilder, Widget, WidgetInfo, WidgetType,
};
use std::ops::RangeInclusive;
use std::sync::Arc;

/// Qué hay detrás de la lámina de cristal.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Backdrop {
    /// Nada vivo detrás: se pinta un fondo ambiental (manchas de color).
    #[default]
    Ambient,
    /// La escena se ve detrás, desenfocada por el host (`sim-gpu`).
    Live,
}

/// Pestañas del panel.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum PanelTab {
    #[default]
    Sim,
    Look,
    Show,
    Out,
}

/// Margen entre el borde del contenedor y la lámina.
pub const SLAB_MARGIN: f32 = 10.0;

// ------------------------------------------------------------------ utilidades

/// Mezcla lineal de dos colores (premultiplicados).
fn mix(a: Color32, b: Color32, k: f32) -> Color32 {
    let k = k.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k).round() as u8;
    Color32::from_rgba_premultiplied(
        l(a.r(), b.r()),
        l(a.g(), b.g()),
        l(a.b(), b.b()),
        l(a.a(), b.a()),
    )
}

fn white(a: u8) -> Color32 {
    Color32::from_white_alpha(a)
}

fn line(w: f32, c: Color32) -> Stroke {
    Stroke::new(w, c)
}

/// Texto en una línea, recortado con "…" si no cabe en `max_w`.
fn fit_galley(ui: &Ui, text: &str, font: FontId, color: Color32, max_w: f32) -> Arc<Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_w.max(1.0));
    ui.fonts(|f| f.layout_job(job))
}

/// Línea horizontal de 1 px que se desvanece hacia los extremos (bisel).
fn hline_fade(painter: &Painter, x0: f32, x1: f32, y: f32, c: Color32) {
    if x1 <= x0 {
        return;
    }
    let xm = (x0 + x1) * 0.5;
    let mut mesh = Mesh::default();
    let tr = Color32::TRANSPARENT;
    for (x, col) in [(x0, tr), (xm, c), (x1, tr)] {
        mesh.colored_vertex(pos2(x, y), col);
        mesh.colored_vertex(pos2(x, y + 1.0), col);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    mesh.add_triangle(2, 3, 4);
    mesh.add_triangle(3, 5, 4);
    painter.add(Shape::mesh(mesh));
}

/// Mancha radial suave (anillos con alfa decreciente).
fn radial(painter: &Painter, center: Pos2, radius: f32, color: Color32) {
    const SEG: usize = 48;
    const RINGS: [(f32, f32); 4] = [(0.0, 1.0), (0.4, 0.5), (0.72, 0.16), (1.0, 0.0)];
    let mut mesh = Mesh::default();
    mesh.colored_vertex(center, color);
    for &(r, k) in &RINGS[1..] {
        let c = color.gamma_multiply(k);
        for s in 0..SEG {
            let a = s as f32 / SEG as f32 * std::f32::consts::TAU;
            mesh.colored_vertex(center + radius * r * vec2(a.cos(), a.sin()), c);
        }
    }
    let seg = SEG as u32;
    // Centro → primer anillo.
    for s in 0..seg {
        mesh.add_triangle(0, 1 + s, 1 + (s + 1) % seg);
    }
    // Entre anillos.
    for ring in 0..(RINGS.len() as u32 - 2) {
        let a0 = 1 + ring * seg;
        let b0 = a0 + seg;
        for s in 0..seg {
            let (a, an) = (a0 + s, a0 + (s + 1) % seg);
            let (b, bn) = (b0 + s, b0 + (s + 1) % seg);
            mesh.add_triangle(a, b, an);
            mesh.add_triangle(an, b, bn);
        }
    }
    painter.add(Shape::mesh(mesh));
}

/// Fondo ambiental estático: base oscura y tres manchas de color.
pub fn ambient(painter: &Painter, rect: Rect) {
    painter.rect_filled(rect, 0, t::BG_BASE);
    let w = rect.width().max(rect.height() * 0.5);
    let c = |r, g, b| Color32::from_rgba_unmultiplied(r, g, b, 46);
    radial(
        painter,
        rect.left_top() + vec2(w * 0.15, 40.0),
        w * 0.85,
        c(0x9D, 0x8C, 0xFF),
    );
    radial(
        painter,
        pos2(rect.right() - w * 0.05, rect.center().y),
        w * 0.8,
        c(0x2F, 0xE6, 0xD6),
    );
    radial(
        painter,
        pos2(rect.left() + w * 0.1, rect.bottom() - 60.0),
        w * 0.8,
        c(0xE0, 0x5A, 0xD6),
    );
}

// ------------------------------------------------------------------ lámina

/// Dibuja la lámina de cristal ocupando todo el `ui` y su contenido dentro.
/// Devuelve el rect de la lámina en puntos (para que el host desenfoque la
/// escena justo detrás).
pub fn glass_slab(ui: &mut Ui, backdrop: Backdrop, add: impl FnOnce(&mut Ui)) -> Rect {
    let full = ui.max_rect();
    let painter = ui.painter().with_clip_rect(full);
    if backdrop == Backdrop::Ambient {
        ambient(&painter, full);
    }
    let slab = full.shrink(SLAB_MARGIN);
    let tint = match backdrop {
        Backdrop::Live => t::GLASS_TINT_LIVE,
        Backdrop::Ambient => t::GLASS_TINT_AMBIENT,
    };
    // Sombra muy suave para despegarla de la escena.
    if backdrop == Backdrop::Live {
        painter.add(
            egui::Shadow {
                offset: [0, 6],
                blur: 22,
                spread: 0,
                color: Color32::from_black_alpha(90),
            }
            .as_shape(slab, t::R_SLAB),
        );
    }
    painter.rect(
        slab,
        t::R_SLAB,
        tint,
        line(1.0, t::HAIRLINE),
        StrokeKind::Inside,
    );
    let r = t::R_SLAB as f32;
    hline_fade(
        &painter,
        slab.left() + r * 0.5,
        slab.right() - r * 0.5,
        slab.top() + 1.0,
        t::BEVEL,
    );

    let inner = Rect::from_min_max(slab.min + vec2(12.0, 12.0), slab.max - vec2(4.0, 4.0));
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(inner)
            .layout(Layout::top_down(Align::Min)),
    );
    child.set_clip_rect(inner.intersect(ui.clip_rect()));
    add(&mut child);
    ui.advance_cursor_after_rect(full);
    slab
}

/// Envuelve el contenido dejando el canal derecho para la barra de scroll
/// (la lámina reserva 4 px; esto añade 8 → 12, simétrico con la izquierda).
pub fn gutter<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .inner_margin(Margin {
            left: 0,
            right: 8,
            top: 0,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

// ------------------------------------------------------------------ tarjetas

/// Tarjeta de cristal sin cabecera (p. ej. el transporte).
pub fn plain_card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let resp = egui::Frame::new()
        .fill(t::CARD_FILL)
        .stroke(line(1.0, t::HAIRLINE))
        .corner_radius(t::R_CARD)
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        });
    let rect = resp.response.rect;
    let r = t::R_CARD as f32;
    hline_fade(
        ui.painter(),
        rect.left() + r,
        rect.right() - r,
        rect.top() + 1.0,
        t::BEVEL,
    );
    resp.inner
}

/// Chevron pintado (gira con `openness`: 0 = derecha, 1 = abajo).
fn chevron(painter: &Painter, c: Pos2, openness: f32, color: Color32) {
    let ang = openness * std::f32::consts::FRAC_PI_2;
    let rot = |v: egui::Vec2| {
        let (s, co) = ang.sin_cos();
        c + vec2(v.x * co - v.y * s, v.x * s + v.y * co)
    };
    let pts = vec![
        rot(vec2(-2.0, -4.0)),
        rot(vec2(2.0, 0.0)),
        rot(vec2(-2.0, 4.0)),
    ];
    painter.add(Shape::line(pts, line(1.5, color)));
}

/// Pinta la cabecera de una tarjeta en `rect` (icono + título).
fn paint_card_title(ui: &Ui, rect: Rect, icn: &str, title: &str, max_w: f32) {
    let p = ui.painter();
    p.text(
        pos2(rect.left() + 8.0, rect.center().y),
        Align2::CENTER_CENTER,
        icn,
        FontId::proportional(13.0),
        t::ACCENT,
    );
    let g = fit_galley(ui, title, t::strong(13.5), t::TEXT_1, max_w - 26.0);
    p.galley(
        pos2(rect.left() + 24.0, rect.center().y - g.size().y * 0.5),
        g,
        t::TEXT_1,
    );
}

/// Tarjeta de sección plegable. La cabecera entera es clicable.
pub fn card(
    ui: &mut Ui,
    id_salt: &str,
    icn: &str,
    title: &str,
    default_open: bool,
    body: impl FnOnce(&mut Ui),
) {
    let id = Id::new(("enjambre_card", id_salt));
    plain_card(ui, |ui| {
        let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, default_open);
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 22.0), Sense::click());
        let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
        if resp.clicked() {
            state.toggle(ui);
        }
        let open = state.openness(ui.ctx());
        paint_card_title(ui, rect, icn, title, w - 20.0);
        let cc = if resp.hovered() { t::TEXT_1 } else { t::TEXT_3 };
        chevron(
            ui.painter(),
            pos2(rect.right() - 7.0, rect.center().y),
            open,
            cc,
        );
        state.show_body_unindented(ui, |ui| {
            ui.add_space(4.0);
            body(ui);
        });
    });
}

/// Tarjeta cuyo interruptor de cabecera activa la sección: el cuerpo solo se
/// muestra (animado) mientras está encendida. Devuelve la respuesta del switch.
pub fn card_switch(
    ui: &mut Ui,
    id_salt: &str,
    icn: &str,
    title: &str,
    on: &mut bool,
    body: impl FnOnce(&mut Ui),
) -> Response {
    let id = Id::new(("enjambre_card_sw", id_salt));
    plain_card(ui, |ui| {
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
        paint_card_title(ui, rect, icn, title, w - 44.0);
        let sw_rect = Rect::from_min_size(
            pos2(rect.right() - 32.0, rect.center().y - 9.0),
            vec2(32.0, 18.0),
        );
        let sw = ui.put(sw_rect, toggle(on));
        let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, *on);
        state.set_open(*on);
        state.show_body_unindented(ui, |ui| {
            ui.add_space(4.0);
            body(ui);
        });
        sw
    })
}

// ------------------------------------------------------------------ interruptor

/// Interruptor tipo píldora (32×18) con el pomo animado.
pub fn toggle(on: &mut bool) -> impl Widget + '_ {
    move |ui: &mut Ui| {
        let (rect, mut resp) = ui.allocate_exact_size(vec2(32.0, 18.0), Sense::click());
        if resp.clicked() {
            *on = !*on;
            resp.mark_changed();
        }
        resp.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), *on, ""));
        if ui.is_rect_visible(rect) {
            let k = ui.ctx().animate_bool_with_time(resp.id, *on, 0.12);
            let off = if resp.hovered() { white(46) } else { white(31) };
            let fill = mix(off, t::ACCENT, k);
            let p = ui.painter();
            p.rect(
                rect,
                9,
                fill,
                line(1.0, mix(t::HAIRLINE, Color32::TRANSPARENT, k)),
                StrokeKind::Inside,
            );
            let cx = egui::lerp((rect.left() + 9.0)..=(rect.right() - 9.0), k);
            let knob = if *on { Color32::WHITE } else { t::TEXT_1 };
            p.circle_filled(pos2(cx, rect.center().y), 6.5, knob);
        }
        resp.on_hover_cursor(CursorIcon::PointingHand)
    }
}

/// Fila "etiqueta ……… [tecla] (switch)". La etiqueta también conmuta.
/// Devuelve una respuesta con `changed()` como la de un checkbox.
pub fn switch_row(ui: &mut Ui, label: &str, on: &mut bool, key: Option<&str>) -> Response {
    let right_w = 32.0 + if key.is_some() { 30.0 } else { 0.0 };
    ui.horizontal(|ui| {
        let lw = (ui.available_width() - right_w - 8.0).max(40.0);
        let mut lr = ui
            .allocate_ui_with_layout(vec2(lw, 18.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(lw);
                ui.add(
                    egui::Label::new(RichText::new(label).color(t::TEXT_1))
                        .wrap()
                        .sense(Sense::click()),
                )
            })
            .inner;
        if lr.clicked() {
            *on = !*on;
            lr.mark_changed();
        }
        let sw = ui
            .with_layout(Layout::right_to_left(Align::Center), |ui| {
                let r = ui.add(toggle(on));
                if let Some(k) = key {
                    kbd(ui, k);
                }
                r
            })
            .inner;
        sw.union(lr)
    })
    .inner
}

/// Controles que dependen de un interruptor: sangría con regla de acento.
pub fn nested<R>(ui: &mut Ui, body: impl FnOnce(&mut Ui) -> R) -> R {
    let resp = egui::Frame::new()
        .inner_margin(Margin {
            left: 12,
            right: 0,
            top: 0,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui)
        });
    let r = resp.response.rect;
    ui.painter().line_segment(
        [
            pos2(r.left() + 3.0, r.top() + 2.0),
            pos2(r.left() + 3.0, r.bottom() - 2.0),
        ],
        line(2.0, t::ACCENT.gamma_multiply(0.45)),
    );
    resp.inner
}

// ------------------------------------------------------------------ slider

/// Fila de slider: etiqueta y valor editable arriba, riel a todo el ancho.
pub struct SliderRow<'a, N: Numeric> {
    label: &'a str,
    value: &'a mut N,
    range: RangeInclusive<N>,
    log: bool,
    suffix: &'a str,
    decimals: Option<usize>,
    hint: Option<&'a str>,
}

pub fn slider_row<'a, N: Numeric>(
    label: &'a str,
    value: &'a mut N,
    range: RangeInclusive<N>,
) -> SliderRow<'a, N> {
    SliderRow {
        label,
        value,
        range,
        log: false,
        suffix: "",
        decimals: None,
        hint: None,
    }
}

impl<'a, N: Numeric> SliderRow<'a, N> {
    pub fn log(mut self) -> Self {
        self.log = true;
        self
    }
    pub fn suffix(mut self, s: &'a str) -> Self {
        self.suffix = s;
        self
    }
    pub fn decimals(mut self, d: usize) -> Self {
        self.decimals = Some(d);
        self
    }
    pub fn hint(mut self, h: &'a str) -> Self {
        self.hint = Some(h);
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let SliderRow {
            label,
            value,
            range,
            log,
            suffix,
            decimals,
            hint,
        } = self;
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            let (lo, hi) = (range.start().to_f64(), range.end().to_f64());
            let row = ui
                .horizontal(|ui| {
                    let l = ui.label(RichText::new(label).color(t::TEXT_2));
                    if let Some(h) = hint {
                        l.on_hover_text(h);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.style_mut().override_font_id = Some(t::mono(12.0));
                        ui.spacing_mut().button_padding = vec2(6.0, 1.0);
                        ui.spacing_mut().interact_size.y = 20.0;
                        let v = &mut ui.visuals_mut().widgets;
                        v.inactive.weak_bg_fill = Color32::TRANSPARENT;
                        v.inactive.bg_stroke = Stroke::NONE;
                        v.inactive.fg_stroke = line(1.0, t::TEXT_1);
                        let speed = if N::INTEGRAL {
                            ((hi - lo) / 200.0).max(0.05)
                        } else {
                            (hi - lo) / 400.0
                        };
                        let mut dv = egui::DragValue::new(value)
                            .range(range.clone())
                            .suffix(suffix)
                            .speed(speed);
                        dv = match decimals {
                            Some(d) => dv.fixed_decimals(d),
                            None if N::INTEGRAL => dv,
                            None => dv.max_decimals(if hi - lo >= 50.0 {
                                0
                            } else if hi - lo >= 5.0 {
                                1
                            } else {
                                2
                            }),
                        };
                        ui.add(dv)
                    })
                    .inner
                })
                .inner;
            // Riel: gris opaco, pomo-anillo, relleno iris hasta el pomo.
            let w = ui.available_width();
            ui.spacing_mut().slider_width = w;
            ui.spacing_mut().interact_size.y = 16.0;
            let vis = ui.visuals_mut();
            vis.selection.bg_fill = t::ACCENT;
            vis.widgets.inactive.bg_fill = Color32::from_rgb(0x2B, 0x2E, 0x3A);
            vis.widgets.inactive.fg_stroke = line(1.5, t::TEXT_1);
            vis.widgets.hovered.bg_fill = t::TEXT_1;
            vis.widgets.hovered.fg_stroke = line(1.5, Color32::WHITE);
            vis.widgets.active.bg_fill = t::ACCENT;
            vis.widgets.active.fg_stroke = line(1.5, Color32::WHITE);
            let s = ui.add(
                egui::Slider::new(value, range)
                    .show_value(false)
                    .logarithmic(log),
            );
            row.union(s)
        })
        .inner
    }
}

// ------------------------------------------------------------------ selectores

/// Control segmentado: celdas del mismo ancho a todo el ancho.
pub fn segmented<T: PartialEq + Clone>(ui: &mut Ui, value: &mut T, opts: &[(T, &str)]) -> bool {
    segmented_tips(ui, value, opts, &[])
}

/// Como `segmented`, con tooltips por índice de opción.
pub fn segmented_tips<T: PartialEq + Clone>(
    ui: &mut Ui,
    value: &mut T,
    opts: &[(T, &str)],
    tips: &[(usize, &str)],
) -> bool {
    let n = opts.len().max(1);
    let h = 26.0;
    let w = ui.available_width();
    let (rect, base) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let base = base.id;
    ui.painter().rect(
        rect,
        t::R_CTRL,
        white(10),
        line(1.0, t::HAIRLINE),
        StrokeKind::Inside,
    );
    let cw = w / n as f32;
    let mut changed = false;
    for (i, (v, label)) in opts.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + i as f32 * cw, rect.top()), vec2(cw, h));
        let mut resp = ui
            .interact(r, base.with(i), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        if let Some((_, tip)) = tips.iter().find(|(k, _)| *k == i) {
            resp = resp.on_hover_text(*tip);
        }
        let selected = *value == *v;
        let p = ui.painter();
        if selected {
            p.rect(
                r.shrink(2.0),
                t::R_CTRL - 2,
                t::ACCENT_SOFT,
                line(1.0, t::ACCENT.gamma_multiply(0.55)),
                StrokeKind::Inside,
            );
        } else if resp.hovered() {
            p.rect_filled(r.shrink(2.0), t::R_CTRL - 2, white(13));
        }
        let col = if selected || resp.hovered() {
            t::TEXT_1
        } else {
            t::TEXT_2
        };
        let g = fit_galley(ui, label, FontId::proportional(12.5), col, cw - 6.0);
        p.galley(r.center() - g.size() * 0.5, g, col);
        if resp.clicked() && !selected {
            *value = v.clone();
            changed = true;
        }
    }
    changed
}

/// Píldoras que saltan de línea (para listas de más de 4 opciones).
pub fn chips<T: PartialEq + Clone>(ui: &mut Ui, value: &mut T, opts: &[(T, &str)]) -> bool {
    chips_tips(ui, value, opts, &[])
}

/// Como `chips`, con tooltips por índice de opción.
pub fn chips_tips<T: PartialEq + Clone>(
    ui: &mut Ui,
    value: &mut T,
    opts: &[(T, &str)],
    tips: &[(usize, &str)],
) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        for (i, (v, label)) in opts.iter().enumerate() {
            let selected = *value == *v;
            let mut resp = chip(ui, label, selected);
            if let Some((_, tip)) = tips.iter().find(|(k, _)| *k == i) {
                resp = resp.on_hover_text(*tip);
            }
            if resp.clicked() && !selected {
                *value = v.clone();
                changed = true;
            }
        }
    });
    changed
}

/// Una píldora seleccionable suelta.
pub fn chip(ui: &mut Ui, label: &str, selected: bool) -> Response {
    let font = FontId::proportional(12.5);
    let g = ui.fonts(|f| f.layout_no_wrap(label.to_owned(), font, t::TEXT_1));
    let size = vec2(g.size().x + 20.0, 24.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let (fill, stroke, col) = if selected {
        (t::ACCENT_SOFT, t::ACCENT.gamma_multiply(0.7), t::TEXT_1)
    } else if resp.hovered() {
        (white(20), white(40), t::TEXT_1)
    } else {
        (white(10), t::HAIRLINE, t::TEXT_2)
    };
    let p = ui.painter();
    p.rect(rect, 12, fill, line(1.0, stroke), StrokeKind::Inside);
    p.galley_with_override_text_color(rect.center() - g.size() * 0.5, g, col);
    resp
}

/// Muestras de color redondas; anillo en la elegida.
pub fn swatches(ui: &mut Ui, selected: &mut usize, colors: &[Color32], names: &[&str]) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        for (i, c) in colors.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
            let resp = resp
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text(names.get(i).copied().unwrap_or(""));
            let p = ui.painter();
            let sel = *selected == i;
            p.circle_filled(rect.center(), 8.0, *c);
            if sel {
                p.circle_stroke(rect.center(), 11.0, line(1.5, t::TEXT_1));
            } else if resp.hovered() {
                p.circle_stroke(rect.center(), 11.0, line(1.0, white(70)));
            }
            if resp.clicked() && !sel {
                *selected = i;
                changed = true;
            }
        }
    });
    changed
}

// ------------------------------------------------------------------ botones

/// Botón con icono (TEXT_2) y etiqueta (TEXT_1).
pub fn button(ui: &mut Ui, icn: &str, label: &str) -> Response {
    ui.add(
        egui::Button::new(icon_label(icn, label, t::TEXT_2, t::TEXT_1)).min_size(vec2(0.0, 28.0)),
    )
}

/// Botón de ancho fijo `w` (alto 30); `primary` lo rellena de iris tenue.
pub fn button_w(ui: &mut Ui, icn: &str, label: &str, w: f32, primary: bool) -> Response {
    let ic = if primary { t::ACCENT } else { t::TEXT_2 };
    let mut b = egui::Button::new(icon_label(icn, label, ic, t::TEXT_1)).min_size(vec2(w, 30.0));
    if primary {
        b = b
            .fill(t::ACCENT_SOFT)
            .stroke(line(1.0, t::ACCENT.gamma_multiply(0.6)));
    }
    ui.add(b)
}

/// Botón principal (relleno iris tenue).
pub fn primary_button(ui: &mut Ui, icn: &str, label: &str) -> Response {
    ui.add(
        egui::Button::new(icon_label(icn, label, t::ACCENT, t::TEXT_1))
            .fill(t::ACCENT_SOFT)
            .stroke(line(1.0, t::ACCENT.gamma_multiply(0.6)))
            .min_size(vec2(0.0, 28.0)),
    )
}

/// Botón de grabar a todo el ancho: contorno rojo; relleno rojo mientras graba.
pub fn danger_button(ui: &mut Ui, icn: &str, label: &str, active: bool) -> Response {
    let (fill, stroke, ic, tx) = if active {
        let pulse = 0.75 + 0.25 * (ui.input(|i| i.time) as f32 * 4.0).sin();
        ui.ctx().request_repaint();
        (
            t::DANGER.gamma_multiply(0.85),
            t::DANGER,
            Color32::WHITE.gamma_multiply(pulse),
            Color32::WHITE,
        )
    } else {
        (
            t::DANGER.gamma_multiply(0.14),
            t::DANGER.gamma_multiply(0.7),
            t::DANGER,
            t::TEXT_1,
        )
    };
    let w = ui.available_width();
    // Layout centrado: egui alinea el texto del botón según el layout padre.
    ui.vertical_centered(|ui| {
        ui.add(
            egui::Button::new(icon_label(icn, label, ic, tx))
                .fill(fill)
                .stroke(line(1.0, stroke))
                .min_size(vec2(w, 34.0)),
        )
    })
    .inner
}

fn icon_label(icn: &str, label: &str, ic: Color32, tx: Color32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let fmt = |c| egui::TextFormat {
        font_id: FontId::proportional(13.0),
        color: c,
        valign: Align::Center,
        ..Default::default()
    };
    job.append(icn, 0.0, fmt(ic));
    job.append(label, 7.0, fmt(tx));
    job
}

/// Botón de solo icono (28×28), fantasma: fondo al pasar el ratón.
pub struct IconButton<'a> {
    icn: &'a str,
    selected: bool,
    danger: bool,
}

pub fn icon_button(icn: &str) -> IconButton<'_> {
    IconButton {
        icn,
        selected: false,
        danger: false,
    }
}

impl IconButton<'_> {
    pub fn selected(mut self, s: bool) -> Self {
        self.selected = s;
        self
    }
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
}

impl Widget for IconButton<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
        let enabled = ui.is_enabled();
        if ui.is_rect_visible(rect) {
            let p = ui.painter();
            let bg = if self.selected {
                t::ACCENT_SOFT
            } else if resp.is_pointer_button_down_on() {
                white(36)
            } else if resp.hovered() && enabled {
                white(20)
            } else {
                Color32::TRANSPARENT
            };
            p.rect_filled(rect, t::R_CTRL, bg);
            let col = if !enabled {
                t::TEXT_3.gamma_multiply(0.6)
            } else if self.selected {
                t::ACCENT
            } else if resp.hovered() && self.danger {
                t::DANGER
            } else if resp.hovered() {
                t::TEXT_1
            } else {
                t::TEXT_2
            };
            p.text(
                rect.center(),
                Align2::CENTER_CENTER,
                self.icn,
                FontId::proportional(13.0),
                col,
            );
        }
        if enabled {
            resp.on_hover_cursor(CursorIcon::PointingHand)
        } else {
            resp
        }
    }
}

/// Atajo: botón de icono con tooltip.
pub fn icon_btn(ui: &mut Ui, icn: &str, tip: &str) -> Response {
    ui.add(icon_button(icn)).on_hover_text(tip)
}

// ------------------------------------------------------------------ texto y chips

/// Insignia de tecla (atajo de teclado).
pub fn kbd(ui: &mut Ui, key: &str) -> Response {
    let g = ui.fonts(|f| f.layout_no_wrap(key.to_owned(), t::mono(10.5), t::TEXT_2));
    let size = vec2((g.size().x + 8.0).max(18.0), 18.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter();
    p.rect(rect, 4, white(14), line(1.0, white(38)), StrokeKind::Inside);
    // Canto inferior algo más marcado: lee como una tecla.
    p.hline(
        (rect.left() + 2.0)..=(rect.right() - 2.0),
        rect.bottom() - 0.5,
        line(1.0, white(24)),
    );
    p.galley(rect.center() - g.size() * 0.5, g, t::TEXT_2);
    resp.on_hover_text(format!("Atajo de teclado: {key}"))
}

/// Chip de estado (cifras en mono) con punto de color opcional.
pub fn stat_chip(ui: &mut Ui, text: &str, dot: Option<Color32>) -> Response {
    let g = ui.fonts(|f| f.layout_no_wrap(text.to_owned(), t::mono(11.5), t::TEXT_2));
    let lead = if dot.is_some() { 14.0 } else { 0.0 };
    let size = vec2(g.size().x + 16.0 + lead, 22.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter();
    p.rect(
        rect,
        11,
        white(10),
        line(1.0, t::HAIRLINE),
        StrokeKind::Inside,
    );
    if let Some(c) = dot {
        p.circle_filled(pos2(rect.left() + 12.0, rect.center().y), 3.5, c);
    }
    p.galley(
        pos2(rect.left() + 8.0 + lead, rect.center().y - g.size().y * 0.5),
        g,
        t::TEXT_2,
    );
    resp
}

/// Chip rojo "● REC mm:ss" con el punto latiendo.
pub fn rec_chip(ui: &mut Ui, secs: f32) -> Response {
    let s = secs.max(0.0) as u32;
    let text = format!("REC {:02}:{:02}", s / 60, s % 60);
    let g = ui.fonts(|f| f.layout_no_wrap(text, t::strong(11.5), t::DANGER));
    let size = vec2(g.size().x + 30.0, 22.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter();
    p.rect(
        rect,
        11,
        t::DANGER.gamma_multiply(0.16),
        line(1.0, t::DANGER.gamma_multiply(0.6)),
        StrokeKind::Inside,
    );
    let pulse = 0.55 + 0.45 * (ui.input(|i| i.time) as f32 * 4.0).sin().abs();
    p.circle_filled(
        pos2(rect.left() + 12.0, rect.center().y),
        4.0,
        t::DANGER.gamma_multiply(pulse),
    );
    p.galley(
        pos2(rect.left() + 22.0, rect.center().y - g.size().y * 0.5),
        g,
        t::DANGER,
    );
    ui.ctx().request_repaint();
    resp
}

/// Pista: texto explicativo pequeño y tenue.
pub fn hint(ui: &mut Ui, text: impl Into<String>) -> Response {
    ui.add(egui::Label::new(RichText::new(text.into()).size(11.5).color(t::TEXT_3)).wrap())
}

/// Aviso ámbar con icono.
pub fn warn(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Label::new(
            RichText::new(format!("{}  {text}", icon::WARN))
                .size(11.5)
                .color(t::WARN),
        )
        .wrap(),
    )
}

/// Subtítulo dentro de una tarjeta (pequeño, en mayúsculas).
pub fn sub_label(ui: &mut Ui, text: &str) -> Response {
    ui.add_space(2.0);
    ui.label(
        RichText::new(text.to_uppercase())
            .font(t::strong(10.5))
            .color(t::TEXT_3),
    )
}

/// Separador fino dentro de una tarjeta.
pub fn divider(ui: &mut Ui) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 5.0), Sense::hover());
    ui.painter()
        .hline(rect.x_range(), rect.center().y, line(1.0, t::HAIRLINE));
}

/// Fila "etiqueta ……… contenido a la derecha".
pub fn row<R>(ui: &mut Ui, label: &str, right: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(t::TEXT_2));
        ui.with_layout(Layout::right_to_left(Align::Center), right)
            .inner
    })
    .inner
}

// ------------------------------------------------------------------ listas

/// Fila de lista: icono + nombre (clicable) y controles al final. El fondo se
/// resalta al pasar el ratón y en la fila seleccionada (con barra de acento).
pub fn list_row(
    ui: &mut Ui,
    selected: bool,
    icn: &str,
    label: &str,
    color: Color32,
    trailing: impl FnOnce(&mut Ui),
) -> Response {
    let w = ui.available_width();
    let bg_idx = ui.painter().add(Shape::Noop);
    let (rect, base) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
    let mut right = ui.new_child(
        UiBuilder::new()
            .max_rect(rect.shrink2(vec2(2.0, 0.0)))
            .layout(Layout::right_to_left(Align::Center)),
    );
    right.spacing_mut().item_spacing.x = 0.0;
    trailing(&mut right);
    let used = right.min_rect().width();
    let label_rect = Rect::from_min_max(rect.min, pos2(rect.right() - used - 4.0, rect.bottom()));
    let resp = ui
        .interact(label_rect, base.id.with("row"), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let hovered = ui.rect_contains_pointer(rect);
    let fill = if selected {
        t::ACCENT_SOFT
    } else if hovered {
        white(12)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().set(
        bg_idx,
        RectShape::filled(rect, CornerRadius::same(t::R_CTRL), fill),
    );
    let p = ui.painter();
    if selected {
        p.rect_filled(
            Rect::from_min_size(
                pos2(rect.left(), rect.top() + 7.0),
                vec2(2.0, rect.height() - 14.0),
            ),
            1,
            t::ACCENT,
        );
    }
    let mut x = rect.left() + 10.0;
    if !icn.is_empty() {
        let ic = if selected { t::ACCENT } else { t::TEXT_3 };
        p.text(
            pos2(x + 6.0, rect.center().y),
            Align2::CENTER_CENTER,
            icn,
            FontId::proportional(12.0),
            ic,
        );
        x += 20.0;
    }
    let g = fit_galley(
        ui,
        label,
        FontId::proportional(13.0),
        color,
        label_rect.right() - x - 4.0,
    );
    p.galley(pos2(x, rect.center().y - g.size().y * 0.5), g, color);
    resp
}

// ------------------------------------------------------------------ pestañas

/// Barra de pestañas: 4 celdas iguales (icono arriba, etiqueta abajo) con un
/// indicador animado bajo la activa.
pub fn tab_bar(ui: &mut Ui, tab: &mut PanelTab) {
    const TABS: [(PanelTab, &str, &str); 4] = [
        (PanelTab::Sim, icon::H_INTERACT, "Simulación"),
        (PanelTab::Look, icon::H_COLOR, "Aspecto"),
        (PanelTab::Show, icon::H_SCENES, "Show"),
        (PanelTab::Out, "\u{f03d}", "Salida"),
    ];
    let w = ui.available_width();
    let h = 46.0;
    let (rect, base) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let base = base.id;
    ui.painter().rect(
        rect,
        t::R_CARD,
        white(8),
        line(1.0, t::HAIRLINE),
        StrokeKind::Inside,
    );
    let cw = w / TABS.len() as f32;
    let cur = TABS.iter().position(|(t, _, _)| t == tab).unwrap_or(0) as f32;
    let x = ui
        .ctx()
        .animate_value_with_time(base.with("ind"), cur, 0.16);
    let sel = Rect::from_min_size(pos2(rect.left() + x * cw, rect.top()), vec2(cw, h)).shrink(3.0);
    ui.painter().rect_filled(sel, t::R_CTRL + 1, t::ACCENT_SOFT);
    ui.painter().rect_filled(
        Rect::from_center_size(pos2(sel.center().x, sel.bottom() - 1.5), vec2(18.0, 2.0)),
        1,
        t::ACCENT,
    );
    for (i, (v, icn, label)) in TABS.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + i as f32 * cw, rect.top()), vec2(cw, h));
        let resp = ui
            .interact(r, base.with(i), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        let active = tab == v;
        if resp.clicked() {
            *tab = *v;
        }
        let p = ui.painter();
        let ic = if active {
            t::ACCENT
        } else if resp.hovered() {
            t::TEXT_1
        } else {
            t::TEXT_3
        };
        let tc = if active || resp.hovered() {
            t::TEXT_1
        } else {
            t::TEXT_2
        };
        p.text(
            pos2(r.center().x, r.top() + 15.0),
            Align2::CENTER_CENTER,
            *icn,
            FontId::proportional(13.0),
            ic,
        );
        let font = if active {
            t::strong(11.5)
        } else {
            FontId::proportional(11.5)
        };
        let g = fit_galley(ui, label, font, tc, cw - 6.0);
        p.galley(pos2(r.center().x - g.size().x * 0.5, r.top() + 25.0), g, tc);
    }
}

// ------------------------------------------------------------------ matriz

/// Editor de la matriz de atracción: celdas pintadas (positivo iris, negativo
/// coral; intensidad = |v|). Arrastrar ajusta, doble clic pone 0. Devuelve si
/// cambió algo.
#[allow(clippy::needless_range_loop)]
pub fn matrix_editor<const N: usize>(
    ui: &mut Ui,
    m: &mut [[f32; N]; N],
    colors: &[Color32],
    names: &[&str],
) -> bool {
    let w = ui.available_width();
    let head = 16.0;
    let gap = 3.0;
    let cell = ((w - head - gap * N as f32) / N as f32).clamp(18.0, 44.0);
    let total = vec2(
        head + N as f32 * (cell + gap),
        head + N as f32 * (cell + gap),
    );
    let (rect, base) = ui.allocate_exact_size(total, Sense::hover());
    let base = base.id;
    let mut changed = false;
    let origin = rect.min + vec2(head + gap, head + gap);
    // Cabeceras: columna = ejerce (arriba), fila = recibe (izquierda).
    for k in 0..N {
        let c = colors.get(k).copied().unwrap_or(t::TEXT_2);
        let off = k as f32 * (cell + gap) + cell * 0.5;
        ui.painter()
            .circle_filled(pos2(origin.x + off, rect.top() + head * 0.5), 4.0, c);
        ui.painter()
            .circle_filled(pos2(rect.left() + head * 0.5, origin.y + off), 4.0, c);
    }
    for i in 0..N {
        for j in 0..N {
            let r = Rect::from_min_size(
                origin + vec2(j as f32 * (cell + gap), i as f32 * (cell + gap)),
                vec2(cell, cell),
            );
            let resp = ui
                .interact(r, base.with((i, j)), Sense::click_and_drag())
                .on_hover_cursor(CursorIcon::ResizeHorizontal);
            let v = &mut m[i][j];
            if resp.dragged() {
                let d = resp.drag_delta();
                let nv = (*v + (d.x - d.y) * 0.01).clamp(-1.0, 1.0);
                if nv != *v {
                    *v = nv;
                    changed = true;
                }
            }
            if resp.double_clicked() && *v != 0.0 {
                *v = 0.0;
                changed = true;
            }
            let a = v.abs();
            let base_c = if *v >= 0.0 {
                t::ACCENT
            } else {
                Color32::from_rgb(0xFF, 0x7A, 0x6B)
            };
            let fill = if a < 0.02 {
                white(10)
            } else {
                base_c.gamma_multiply(0.12 + 0.68 * a)
            };
            let p = ui.painter();
            let stroke = if resp.hovered() || resp.dragged() {
                line(1.0, t::TEXT_1.gamma_multiply(0.8))
            } else {
                line(1.0, t::HAIRLINE)
            };
            p.rect(r, 5, fill, stroke, StrokeKind::Inside);
            let tc = if a > 0.45 { Color32::WHITE } else { t::TEXT_2 };
            let txt = format!("{:.1}", v).replace("-0.0", "0.0");
            p.text(r.center(), Align2::CENTER_CENTER, txt, t::mono(10.0), tc);
            let (ni, nj) = (
                names.get(i).copied().unwrap_or("?"),
                names.get(j).copied().unwrap_or("?"),
            );
            let val = *v;
            resp.on_hover_text(format!(
                "{ni} ← {nj}: {val:+.2}\nArrastra para ajustar · doble clic = 0"
            ));
        }
    }
    changed
}
