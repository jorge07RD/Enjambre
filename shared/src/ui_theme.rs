//! Tema visual del panel (compartido por el panel embebido en `sim`/`sim-gpu`
//! y el separado en `panel`, todos sobre egui 0.31): "cristal de instrumento".
//!
//! Una lámina de cristal ahumado (la pinta `ui_kit::glass_slab`) con tarjetas
//! de cristal más claras encima; un único acento (iris), colores semánticos
//! para estado y cifras en mono. Tipografía Inter (Regular + SemiBold, OFL)
//! embebida, con la Nerd Font de iconos como *fallback* para los glifos.
//!
//! Llamar `apply(ctx)` una sola vez por proceso, tras crear el contexto egui.

use egui::{Color32, CornerRadius, FontFamily, FontId, Shadow, Stroke, TextStyle};
use std::sync::Arc;

// ------------------------------------------------------------------ tokens

/// Color con alfa *no* premultiplicado, evaluable en `const` (egui guarda los
/// `Color32` premultiplicados).
const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    Color32::from_rgba_premultiplied(pm(r, a), pm(g, a), pm(b, a), a)
}

const fn pm(c: u8, a: u8) -> u8 {
    ((c as u16 * a as u16 + 127) / 255) as u8
}

/// Fondo opaco donde no hay escena detrás (sim CPU, ventana aparte).
pub const BG_BASE: Color32 = Color32::from_rgb(0x0A, 0x0C, 0x11);
/// Tinte de la lámina sobre el blur real de la escena (`sim-gpu`).
pub const GLASS_TINT_LIVE: Color32 = rgba(14, 16, 24, 148);
/// Tinte de la lámina sobre el fondo ambiental.
pub const GLASS_TINT_AMBIENT: Color32 = rgba(14, 16, 24, 184);
/// Relleno de las tarjetas de sección (y en hover).
pub const CARD_FILL: Color32 = rgba(255, 255, 255, 9);
pub const CARD_FILL_HOVER: Color32 = rgba(255, 255, 255, 14);
/// Borde fino de cristal.
pub const HAIRLINE: Color32 = rgba(255, 255, 255, 20);
/// Brillo de 1 px en el canto superior (bisel).
pub const BEVEL: Color32 = rgba(255, 255, 255, 36);
/// Texto primario / secundario / pistas.
pub const TEXT_1: Color32 = Color32::from_rgb(0xEC, 0xEE, 0xF4);
pub const TEXT_2: Color32 = Color32::from_rgb(0xA6, 0xAB, 0xBA);
pub const TEXT_3: Color32 = Color32::from_rgb(0x6C, 0x71, 0x83);
/// Acento (iris): selección, relleno de slider, switch encendido, pestaña activa.
pub const ACCENT: Color32 = Color32::from_rgb(0x9D, 0x8C, 0xFF);
/// Acento tenue para fondos de lo seleccionado.
pub const ACCENT_SOFT: Color32 = rgba(0x9D, 0x8C, 0xFF, 51);
/// Estado: en vivo / aviso / peligro (REC, borrar).
pub const OK: Color32 = Color32::from_rgb(0x5E, 0xE6, 0xA8);
pub const WARN: Color32 = Color32::from_rgb(0xF5, 0xB8, 0x4B);
pub const DANGER: Color32 = Color32::from_rgb(0xFF, 0x5A, 0x63);

/// Radios: lámina, tarjeta y controles.
pub const R_SLAB: u8 = 16;
pub const R_CARD: u8 = 12;
pub const R_CTRL: u8 = 8;

/// Familia de la fuente SemiBold (títulos, wordmark, etiquetas fuertes).
pub const STRONG: &str = "strong";

/// `FontId` SemiBold del tamaño dado.
pub fn strong(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(STRONG.into()))
}

/// `FontId` monoespaciada (cifras) del tamaño dado.
pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// Instala fuentes (Inter + Nerd Font de iconos como fallback) y la paleta.
pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    install_visuals(ctx);
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "inter".to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../../assets/fonts/Inter-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "inter_semibold".to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../../assets/fonts/Inter-SemiBold.ttf"
        ))),
    );
    // Fuente de iconos empaquetada (Nerd Font: cubre Font Awesome + Material).
    fonts.font_data.insert(
        "icons".to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../../assets/icons.ttf"
        ))),
    );

    // Los iconos van justo detrás de Inter: las fuentes por defecto de egui
    // (Ubuntu) tienen ligaduras "fi"/"fl" en U+F001/F002, que taparían iconos
    // de Font Awesome como la nota musical.
    let prop = fonts.families.entry(FontFamily::Proportional).or_default();
    prop.insert(0, "inter".to_owned());
    prop.insert(1, "icons".to_owned());
    let fallback: Vec<String> = prop.iter().skip(1).cloned().collect();

    // SemiBold: Inter SemiBold y, detrás, el mismo fallback.
    let mut strong_fam = vec!["inter_semibold".to_owned()];
    strong_fam.extend(fallback);
    fonts
        .families
        .insert(FontFamily::Name(STRONG.into()), strong_fam);

    // Mono (Hack, la de egui) para cifras, con iconos de fallback.
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(1, "icons".to_owned());
    ctx.set_fonts(fonts);
}

fn install_visuals(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();

    // La lámina de cristal la pinta el kit; los contenedores de egui quedan
    // transparentes para dejarla ver.
    v.panel_fill = Color32::TRANSPARENT;
    v.window_fill = Color32::from_rgba_unmultiplied(18, 20, 28, 242);
    v.window_stroke = Stroke::new(1.0_f32, HAIRLINE);
    v.extreme_bg_color = Color32::from_rgba_unmultiplied(0, 0, 0, 90);
    v.faint_bg_color = Color32::from_rgba_unmultiplied(255, 255, 255, 6);
    v.code_bg_color = Color32::from_rgba_unmultiplied(255, 255, 255, 12);
    let shadow = Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(120),
    };
    v.window_shadow = shadow;
    v.popup_shadow = shadow;

    v.hyperlink_color = ACCENT;
    v.warn_fg_color = WARN;
    v.error_fg_color = DANGER;
    v.selection.bg_fill = ACCENT_SOFT;
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    v.override_text_color = None;

    let ctrl = CornerRadius::same(R_CTRL);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = Color32::TRANSPARENT;
    w.noninteractive.weak_bg_fill = Color32::TRANSPARENT;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, HAIRLINE);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_2);

    w.inactive.bg_fill = Color32::from_rgba_unmultiplied(255, 255, 255, 22);
    w.inactive.weak_bg_fill = Color32::from_rgba_unmultiplied(255, 255, 255, 13);
    w.inactive.bg_stroke = Stroke::new(1.0_f32, HAIRLINE);
    w.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT_1);

    w.hovered.bg_fill = Color32::from_rgba_unmultiplied(255, 255, 255, 32);
    w.hovered.weak_bg_fill = Color32::from_rgba_unmultiplied(255, 255, 255, 23);
    w.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 41));
    w.hovered.fg_stroke = Stroke::new(1.2_f32, TEXT_1);
    w.hovered.expansion = 0.0;

    w.active.bg_fill = ACCENT;
    w.active.weak_bg_fill = ACCENT_SOFT;
    w.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    w.active.fg_stroke = Stroke::new(1.5_f32, TEXT_1);
    w.active.expansion = 0.0;

    w.open.weak_bg_fill = ACCENT_SOFT;
    w.open.bg_stroke = Stroke::new(1.0_f32, ACCENT);

    for ws in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        ws.corner_radius = ctrl;
    }
    v.window_corner_radius = CornerRadius::same(R_CARD);
    v.menu_corner_radius = CornerRadius::same(R_CTRL);

    ctx.set_visuals(v);

    let mut style = (*ctx.style()).clone();
    style.text_styles = [
        (TextStyle::Heading, strong(17.0)),
        (TextStyle::Body, FontId::proportional(13.0)),
        (TextStyle::Button, FontId::proportional(13.0)),
        (TextStyle::Small, FontId::proportional(11.5)),
        (TextStyle::Monospace, mono(12.0)),
    ]
    .into();
    let s = &mut style.spacing;
    s.item_spacing = egui::vec2(8.0, 8.0);
    s.button_padding = egui::vec2(10.0, 5.0);
    s.interact_size.y = 26.0;
    s.slider_rail_height = 4.0;
    s.icon_width = 16.0;
    s.icon_width_inner = 9.0;
    s.combo_width = 120.0;
    let mut scroll = egui::style::ScrollStyle::floating();
    scroll.floating_width = 4.0;
    scroll.bar_width = 6.0;
    s.scroll = scroll;
    style.interaction.tooltip_delay = 0.35;
    ctx.set_style(style);
}

/// Codepoints de iconos (Nerd Font, rango clásico Font Awesome, estable). Se usan
/// en las etiquetas de los botones del panel: `format!("{} Guardar", icon::SAVE)`.
pub mod icon {
    pub const PLAY: &str = "\u{f04b}";
    pub const PAUSE: &str = "\u{f04c}";
    pub const STOP: &str = "\u{f04d}";
    pub const STEP: &str = "\u{f051}"; // step-forward
    pub const PREV: &str = "\u{f048}"; // step-backward
    pub const NEXT: &str = "\u{f051}"; // step-forward
    pub const RESET: &str = "\u{f021}"; // refresh
    pub const REPEAT: &str = "\u{f01e}"; // repeat (actualizar)
    pub const REC: &str = "\u{f111}"; // circle
    pub const FRAME: &str = "\u{f065}"; // expand
    pub const CROSSHAIRS: &str = "\u{f05b}"; // centrar
    pub const FOLDER: &str = "\u{f07c}"; // folder-open
    pub const MUSIC: &str = "\u{f001}";
    pub const SAVE: &str = "\u{f0c7}"; // floppy
    pub const STAR: &str = "\u{f005}";
    pub const TRASH: &str = "\u{f1f8}";
    pub const DETACH: &str = "\u{f08e}"; // external-link
    pub const REATTACH: &str = "\u{f066}"; // compress
    pub const HIDE: &str = "\u{f070}"; // eye-slash
    pub const TEXT: &str = "\u{f031}"; // font
    pub const IMAGE: &str = "\u{f03e}"; // picture
    pub const APPLY: &str = "\u{f00c}"; // check
    pub const RELEASE: &str = "\u{f00d}"; // times
    pub const RANDOM: &str = "\u{f074}";
    pub const CAMERA: &str = "\u{f030}";
    pub const UPLOAD: &str = "\u{f093}";
    pub const DOWNLOAD: &str = "\u{f019}";
    pub const FILL: &str = "\u{f067}"; // plus
    pub const ERASE: &str = "\u{f12d}"; // eraser
    pub const DESKTOP: &str = "\u{f108}";
    pub const FIT: &str = "\u{f065}"; // expand
    pub const UP: &str = "\u{f062}"; // arrow-up
    pub const DOWN: &str = "\u{f063}"; // arrow-down
    pub const CHEVRON_DOWN: &str = "\u{f078}";
    pub const CHEVRON_RIGHT: &str = "\u{f054}";
    pub const WARN: &str = "\u{f071}"; // exclamation-triangle
    pub const DOT: &str = "\u{f111}"; // circle

    // Iconos de cabecera de secciones.
    pub const H_INTERACT: &str = "\u{f0d0}"; // magic
    pub const H_PHYSICS: &str = "\u{f0c3}"; // flask
    pub const H_LOOK: &str = "\u{f0eb}"; // lightbulb
    pub const H_COLOR: &str = "\u{f1fc}"; // paint-brush
    pub const H_AUDIO: &str = "\u{f028}"; // volume-up
    pub const H_TOOL: &str = "\u{f245}"; // mouse-pointer
    pub const H_SHAPE: &str = "\u{f075}"; // comment
    pub const H_SCENES: &str = "\u{f008}"; // film
    pub const H_SEQ: &str = "\u{f03a}"; // list
    pub const H_REC: &str = "\u{f111}"; // circle
    pub const H_CANVAS: &str = "\u{f125}"; // crop
    pub const H_MUSIC: &str = "\u{f001}"; // music
}
