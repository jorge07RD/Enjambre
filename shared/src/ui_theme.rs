//! Tema visual del panel (compartido por el panel embebido en `sim` y el
//! separado en `panel`, ambos sobre egui 0.31). Aplica un aspecto "oscuro neón"
//! y registra una fuente de iconos (Nerd Font) como *fallback*, para que los
//! glifos de icono se dibujen (egui por defecto solo trae un subconjunto de
//! emoji y muchos salían como □).
//!
//! Llamar `apply(ctx)` una sola vez por proceso, tras crear el contexto egui.

use std::sync::Arc;

/// Color de acento (cian neón) del tema.
pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x2f, 0xe6, 0xd6);
/// Acento tenue para rellenos de selección/hover.
const ACCENT_DIM: egui::Color32 = egui::Color32::from_rgb(0x12, 0x53, 0x4e);

/// Instala fuentes (con la Nerd Font como fallback de iconos) y la paleta oscura.
pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    install_visuals(ctx);
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    // Fuente de iconos empaquetada (Nerd Font: cubre Font Awesome + Material).
    fonts.font_data.insert(
        "icons".to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../../assets/icons.ttf"
        ))),
    );
    // Añadirla al FINAL de cada familia = fallback: el texto normal usa las
    // fuentes por defecto y solo los glifos de icono caen en la Nerd Font.
    for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(fam).or_default().push("icons".to_owned());
    }
    ctx.set_fonts(fonts);
}

fn install_visuals(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();

    // Fondos algo más oscuros y fríos.
    v.panel_fill = egui::Color32::from_rgb(0x12, 0x14, 0x18);
    v.window_fill = egui::Color32::from_rgb(0x16, 0x19, 0x1f);
    v.extreme_bg_color = egui::Color32::from_rgb(0x0b, 0x0d, 0x10);

    // Acento neón en enlaces y selección.
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT_DIM;
    v.selection.stroke = egui::Stroke::new(1.0, ACCENT);

    // Realce de acento al pasar/pulsar controles.
    v.widgets.hovered.bg_fill = ACCENT_DIM;
    v.widgets.hovered.weak_bg_fill = ACCENT_DIM;
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.2, ACCENT);
    v.widgets.active.bg_fill = ACCENT;
    v.widgets.active.weak_bg_fill = ACCENT;

    // Esquinas redondeadas (aspecto más suave).
    let r = egui::CornerRadius::same(5);
    v.widgets.noninteractive.corner_radius = r;
    v.widgets.inactive.corner_radius = r;
    v.widgets.hovered.corner_radius = r;
    v.widgets.active.corner_radius = r;
    v.widgets.open.corner_radius = r;
    v.window_corner_radius = egui::CornerRadius::same(8);
    v.menu_corner_radius = egui::CornerRadius::same(6);

    ctx.set_visuals(v);

    // Un poco más de aire entre controles.
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
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
}
