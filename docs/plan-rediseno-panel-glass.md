# Rediseño del panel de Enjambre — "Cristal de instrumento"

> Plan autocontenido: al aprobarlo se copia tal cual a `docs/plan-rediseno-panel-glass.md`
> para que lo implemente otro modelo. Todo lo que dice "verificado" se comprobó leyendo el código.

## Contexto

El panel de control (egui 0.31) es hoy una sola columna de 11 `CollapsingHeader` seguidos,
con widgets egui por defecto (checkboxes, sliders con la etiqueta a la derecha, filas de
`selectable_value`, botones ●/○ para elegir color) y un tema "oscuro neón" cian mínimo.
Funciona, pero se lee como un formulario largo: no hay jerarquía, las secciones no se
distinguen y no parece una herramienta profesional.

Objetivo: rediseño completo **profesional y moderno, con secciones bien definidas y estilo
de cristal con blur**. El usuario pidió que no le hagan elegir: la dirección de abajo ya está
decidida. Se mantienen toda la funcionalidad, los textos y los atajos de teclado; cambia solo
la presentación.

### Hechos verificados

- `shared/src/panel_ui.rs` (1418 líneas): `PanelState`, `PanelEvent`, `config_panel(ui, params, st) -> Vec<PanelEvent>`.
  Re-exportados por `shared/src/lib.rs` como `shared::{config_panel, PanelState, PanelEvent}`.
- `shared/src/ui_theme.rs`: `apply(ctx)` (fuentes + visuals), `ACCENT` (solo se usa en panel_ui.rs:297), módulo `icon` (Nerd Font, `assets/icons.ttf`).
- Tres hosts llaman a `config_panel`:
  | Host | Dónde | Contenedor actual | ¿Se ve la escena detrás? |
  |---|---|---|---|
  | `sim-gpu` (wgpu 24 + egui-wgpu 0.31) | `sim-gpu/src/main.rs:1376` | `SidePanel::left`, 330 px | **Sí**: la escena ocupa toda la ventana y egui se pinta encima (`main.rs:~1690`) |
  | `sim` (macroquad + egui-macroquad 0.17) | `sim/src/main.rs:1449` | `SidePanel::right`, 310 px | No: el lienzo reserva el ancho del panel (`main.rs:1052`) |
  | `panel` (eframe, ventana aparte) | `panel/src/main.rs:258` | `CentralPanel` | No (ventana propia) |
- `PanelState` se construye con `..PanelState::default()` en `panel/src/main.rs:96`, `sim/src/main.rs:936` y `PanelState::default()` en `sim-gpu/src/main.rs:403`: los campos nuevos solo necesitan valor en `Default`.
- `GpuSim` renderiza la escena a una textura HDR `offscreen_view` (`rgba16float`, `TEXTURE_BINDING`, `gpu_sim.rs:1404`) y la vuelca con `fs_blit` (`shaders/post.wgsl`, devuelve el rgb tal cual). La grabación copia la escena **antes** de egui (`blit_to`), así que nada del panel entra en el vídeo.
- egui 0.31 tiene `Spacing::slider_rail_height`, `Visuals::slider_trailing_fill`, `Visuals::handle_shape`, `Frame::new()/Frame::NONE`, `CornerRadius`, `Shadow`. `Color32` es **premultiplicado**: usar `Color32::from_rgba_unmultiplied` / `from_white_alpha`.
- `NUM_COLORS = 6` (`shared/src/config.rs:8`).
- El blur del compositor de Hyprland está **apagado** en esta máquina (`decoration:blur:enabled = false`), así que la ventana aparte no puede depender de él.

## Dirección de diseño (decidida)

**Modo:** Operate. Es un instrumento para un show visual en vivo y para grabar vídeo: se usa de
noche, junto a un lienzo oscuro lleno de partículas, muchas veces a mitad de una actuación. Lo
primero es poder escanearlo rápido; la marca se nota en los detalles precisos.

**Mundo: "cristal de instrumento".** Una lámina de cristal ahumado flotando sobre el enjambre en
vivo: en `sim-gpu` el blur es real (se ven las partículas desenfocadas detrás). Encima van tarjetas
de cristal más claras, con borde de 1 px y un brillo superior de 1 px, como el bisel de una pieza
de cristal. Una sola tinta de acento (iris), los estados con color semántico, y los números en mono
para que lean como lecturas de un aparato.

### Tokens (en `ui_theme.rs`)

| Token | Valor | Uso |
|---|---|---|
| `BG_BASE` | `#0A0C11` | fondo opaco donde no hay escena detrás |
| `GLASS_TINT_LIVE` | rgba(14,16,24, 0.58) | tinte de la lámina sobre el blur real |
| `GLASS_TINT_AMBIENT` | rgba(14,16,24, 0.72) | tinte de la lámina sobre el fondo ambiental |
| `CARD_FILL` / `CARD_FILL_HOVER` | blanco α 0.035 / 0.055 | tarjetas de sección |
| `HAIRLINE` | blanco α 0.08 | bordes |
| `BEVEL` | blanco α 0.14 | brillo de 1 px en el borde superior de la lámina y las tarjetas |
| `TEXT_1` / `TEXT_2` / `TEXT_3` | `#ECEEF4` / `#A6ABBA` / `#6C7183` | texto primario / secundario / pistas |
| `ACCENT` | `#9D8CFF` (iris) | selección, relleno del slider, switch encendido, pestaña activa |
| `ACCENT_SOFT` | iris α 0.20 | fondos de lo seleccionado |
| `OK` / `WARN` / `DANGER` | `#5EE6A8` / `#F5B84B` / `#FF5A63` | "en vivo" / avisos / REC y borrar |

Radios: lámina 16 · tarjeta 12 · controles 8 · chips y switch: píldora.
Espaciado (escala 4/8/12/16): `item_spacing (8,8)`, margen interior de tarjeta 12, separación entre tarjetas 10, margen de la lámina 10.
Tipografía: **Inter** Regular + SemiBold embebidas (OFL) y Hack (la mono por defecto de egui) para las cifras.
Escala: wordmark 17 semibold · título de tarjeta 13 semibold · cuerpo 13 · pista 11.5 · cifras mono 12.
Movimiento: solo `ctx.animate_bool` / `animate_value` (switch, plegado de tarjetas, hover de pestañas, ~120 ms). No hay animaciones en bucle salvo el punto de REC que late.

### Arquitectura de la información

Las 11 secciones de hoy quedan en una **cabecera fija** y **4 pestañas**; cada sección es una tarjeta:

```
┌ lámina de cristal ───────────────────────┐
│ Enjambre ●                        ⧉  ⊘  │  wordmark · separar/reacoplar · ocultar
│ [2 000 partículas] [60 FPS] [● REC 00:12]│  chips de estado (mono)
│ ┌ Transporte ──────────────────────────┐ │
│ │ [⏸ Pausa][⏭ Paso][⟳ Reiniciar]       │ │  fijo: lo más usado
│ │ Velocidad                    100 %   │ │
│ │ ━━━━━━━━━━━━━━━━●──────────          │ │
│ │ 10 20 30 40 50 60 70 80 90 100       │ │  segmentado (teclas 1…0)
│ │ Cambio suave  ○─  · 1.0 s            │ │
│ └──────────────────────────────────────┘ │
│  Simulación │ Aspecto │ Show │ Salida    │  barra de pestañas
│ ┌ scroll ──────────────────────────────┐ │
│ │ ▾ Interacción          tarjeta       │ │
│ │ ▸ Física                             │ │
│ │ ▸ Partículas y herramienta           │ │
│ └──────────────────────────────────────┘ │
└──────────────────────────────────────────┘
```

| Pestaña | Tarjetas (en orden) |
|---|---|
| **Simulación** | Interacción (abierta) · Física · Partículas y herramienta |
| **Aspecto** | Apariencia (abierta) · Dinámica del color · Lienzo y vista |
| **Show** | Mensaje / Forma (abierta) · Escenas · Secuenciador |
| **Salida** | Grabación (abierta) · Música y sincronía (sale de Grabación) · Reactivo al audio (switch en la cabecera) |

Cómo se sustituyen los widgets (en todas las pestañas):

| Hoy | Nuevo |
|---|---|
| `ui.checkbox` de activar/desactivar | `switch_row(label, &mut bool, kbd)`: etiqueta a la izquierda, tecla + switch a la derecha |
| Controles que dependen de un switch | `nested(ui, …)`: sangría de 10 px con una regla de acento de 2 px a la izquierda |
| `Slider … .text("X")` | `slider_row`: etiqueta a la izquierda y valor editable (mono) a la derecha, y debajo el riel a todo el ancho |
| Fila de `selectable_value` (≤4 opciones) | `segmented`: celdas del mismo ancho, a todo el ancho |
| Fila de `selectable_value` (>4: modos, BeatAction, presets de encuadre) | `chips`: píldoras que saltan de línea |
| Botones de color ●/○ | `swatches`: círculos de 20 px con anillo de acento en el elegido |
| `ui.label("explicación…")` y `.weak().small()` | `hint` (TEXT_3, 11.5) |
| Avisos ámbar | `warn` (icono + texto WARN) |
| Filas de escenas / formas / secuenciador | `list_row`: hover con relleno, nombre, botones de icono al final |
| `Grid` de `DragValue` de la matriz | `matrix_editor`: celdas pintadas (positivo iris, negativo coral, intensidad = \|v\|), arrastrar para cambiar, doble clic = 0, tooltip con el valor |
| Atajo metido en el texto, p. ej. "Llenar (F)" | botón "Llenar" + insignia `kbd("F")`; el tooltip conserva el atajo |

Todos los textos se conservan. Solo cambian los atajos, que pasan del texto entre paréntesis a una insignia `kbd`.

## Pasos de implementación (cada paso compila)

### 1. Fuentes y tokens — `shared/src/ui_theme.rs`, `assets/fonts/`
- Descargar Inter 4.1 (`https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip`) y copiar `extras/ttf/Inter-Regular.ttf` y `Inter-SemiBold.ttf` a `assets/fonts/`, junto con `LICENSE.txt` (OFL) como `assets/fonts/Inter-LICENSE.txt`.
- `install_fonts`: Proportional = `[inter, …defaults, icons]`; nueva familia `FontFamily::Name("strong")` = `[inter_semibold, icons]`; Monospace = `[…defaults(Hack), icons]`. Exponer `pub fn strong(size) -> FontId`.
- Reescribir `install_visuals` con los tokens: `panel_fill = TRANSPARENT` (la lámina la pinta el kit), `window_fill` rgba(18,20,28,0.95), `window_stroke` HAIRLINE, `window_shadow`/`popup_shadow` = `Shadow{offset:[0,8], blur:24, spread:0, color: negro α 0.47}`. En los widgets: inactive `weak_bg_fill` blanco α 0.05 con `bg_stroke` HAIRLINE y `fg_stroke` TEXT_2; hovered α 0.09, stroke blanco α 0.16 y fg TEXT_1; active `ACCENT_SOFT` con stroke ACCENT; `selection.bg_fill = ACCENT_SOFT`, `selection.stroke = ACCENT`; `slider_trailing_fill = true`; `handle_shape = Circle`; `override_text_color` vacío; radios según los tokens.
- `text_styles`: Body 13, Button 13, Small 11.5, Heading 17 (strong), Monospace 12. `spacing`: `item_spacing (8,8)`, `button_padding (10,5)`, `interact_size.y 26`, `slider_rail_height 4`, `scroll = ScrollStyle::floating()` y barra fina.
- Mantener `pub const ACCENT` (ahora iris) y todo el módulo `icon`. Añadir los iconos que falten: `CHEVRON_DOWN \u{f078}`, `CHEVRON_RIGHT \u{f054}`, `WARN \u{f071}`, `DOT \u{f111}` (ya existe como REC).

### 2. Kit de componentes — nuevo `shared/src/ui_kit.rs` (+ `pub mod ui_kit;` en lib.rs)
Funciones (todas con `&mut egui::Ui`, sin estado global; el estado de plegado va en `ui.data_mut` con `Id`):
- `glass_slab(ui, backdrop, add) -> Rect`: pinta el fondo (si es `Ambient`: `BG_BASE` + 3 manchas radiales, véase abajo), la lámina redondeada con un margen de 10 px, el tinte según el backdrop, `HAIRLINE` y el `BEVEL` superior (una línea de 1 px de ancho, recortada a la curva con `painter.line_segment` entre `x0+r` y `x1-r`). Devuelve el rect de la lámina **en puntos**.
- `ambient(painter, rect)`: 3 `epaint::Mesh` en abanico (32 segmentos; centro con color α≈0.20, borde transparente) en iris (arriba a la izquierda), teal `#2FE6D6` (centro a la derecha) y magenta `#E05AD6` (abajo a la izquierda), con radio de ≈0.8·ancho. Es estático.
- `card(ui, id, icon, title, default_open, body)` y `card_switch(ui, id, icon, title, &mut bool, body)`: un `egui::Frame` con `CARD_FILL`, stroke `HAIRLINE`, radio 12, margen interior 12 y bisel superior. La cabecera entera es clicable y lleva un chevron animado (`animate_bool`). En `card_switch` el switch va a la derecha de la cabecera y el cuerpo solo se dibuja si está encendido.
- `switch(ui, &mut bool) -> Response`: 32×18 en forma de píldora, con el pomo animado; encendido = ACCENT, apagado = blanco α 0.12.
- `switch_row(ui, label, &mut bool, kbd: Option<&str>) -> Response` (devuelve `changed()` como un checkbox, para conservar la lógica `.changed()` que hay hoy).
- `SliderRow` builder: `slider_row(label, &mut value, range).log().suffix(" %").decimals(2).hint(text).show(ui) -> Response`. Por dentro: fila con la etiqueta (TEXT_2) a la izquierda y un `DragValue` mono a la derecha (mismo rango, sufijo y decimales); debajo `ui.add_sized([ancho, 18], Slider::new(..).show_value(false))`. Hace la OR de los dos `Response` para que `.changed()` siga funcionando (la velocidad y la longitud de estela dependen de ello).
- `segmented<T: PartialEq+Clone>(ui, &mut T, &[(T, &str)]) -> bool`, `chips<T>(…) -> bool`, `swatches(ui, &mut usize, &[Color32], &[&str]) -> bool`.
- `icon_btn(ui, icon, tip)`, `button(ui, icon, label)`, `primary_button`, `danger_button`, `kbd(ui, key)`, `stat_chip(ui, text, color)`, `hint`, `warn`, `sub_label` (11.5, TEXT_3, en mayúsculas), `nested(ui, body)`, `list_row(ui, selected, icon, label, trailing) -> Response`, `tab_bar(ui, &mut PanelTab)` (4 celdas del mismo ancho, con indicador de 2 px bajo la activa y fondo `ACCENT_SOFT`).
- `matrix_editor(ui, &mut [[f32; NUM_COLORS]; NUM_COLORS], palette: &[Color32]) -> bool`: tamaño de celda = `(ancho - cabecera) / 7`, con cabeceras de fila y columna como swatches. Arrastrar: `v += (dx - dy) * 0.01`, con clamp a −1..1. Valor mono pequeño centrado. Debajo, la pista "Fila = recibe · Columna = ejerce".

### 3. `PanelState` + división de `panel_ui.rs` en módulo
- Convertir `shared/src/panel_ui.rs` en `shared/src/panel_ui/mod.rs` (con `PanelState`, `PanelEvent`, `select_shape`, `load_image_preview`, `egui_color` y `config_panel`) más `header.rs`, `tab_sim.rs`, `tab_look.rs`, `tab_show.rs` y `tab_out.rs`. La API pública no cambia.
- Campos nuevos en `PanelState` (solo de UI, no viajan por IPC; cada uno con su valor en `Default`):
  - `tab: PanelTab` (`enum PanelTab { Sim, Look, Show, Out }`, por defecto `Sim`).
  - `backdrop: Backdrop` (`enum Backdrop { Ambient, Live }`, por defecto `Ambient`; lo exporta `ui_kit`).
  - `glass_rect: Option<egui::Rect>`: rect de la lámina en puntos, lo escribe `config_panel` en cada frame.
- `config_panel`: `glass_slab` → header (wordmark, chips, transporte y velocidad: no hacen scroll) → `tab_bar` → `ScrollArea::vertical()` con las tarjetas de la pestaña activa.
- **Regla estricta:** se mueve la lógica de cada sección *tal cual*. Cada `events.push(...)`, cada mutación de `params`/`st`, `snap_before`, `trigger`, `changed`, `swap`/`delete` y las exclusiones mutuas (`shape_tint` frente a `shape_photo_color`) quedan idénticos. Solo cambian los widgets, según la tabla de sustitución.
- "Música y sincronía" se separa de Grabación: el selector de música, quitar música, analizar, preescucha y todo `music_sync` van a esa tarjeta; Grabación se queda con REC, encuadre, presets, centrar y carpeta.
- El botón de grabar pasa a ser `danger_button` a todo el ancho (con el punto que late cuando está grabando).

### 4. Hosts: contenedor transparente
- `sim-gpu/src/main.rs:1376`: `SidePanel::left("panel").frame(Frame::NONE).resizable(true).default_width(340.0)`; antes, `self.panel.backdrop = Backdrop::Live`.
- `sim/src/main.rs:1449`: `.frame(Frame::NONE.fill(ui_theme::BG_BASE))`, `default_width(330.0)`; se queda en `Ambient`.
- `panel/src/main.rs:258`: `CentralPanel::default().frame(Frame::NONE.fill(BG_BASE))`; se queda en `Ambient`. Cambiar el título de la ventana a `"Enjambre — Panel"`.

### 5. Blur real detrás de la lámina — `sim-gpu`
- Nuevo `sim-gpu/src/shaders/glass.wgsl`:
  ```wgsl
  struct G { texel: vec2f, dir: vec2f, spread: f32, _p0: f32, _p1: f32, _p2: f32 };
  @group(0) @binding(0) var src: texture_2d<f32>;
  @group(0) @binding(1) var smp: sampler;
  @group(0) @binding(2) var<uniform> g: G;
  // vs_fullscreen: idéntico al de post.wgsl (uv con Y invertida)
  @fragment fn fs_down(in: FsIn) -> @location(0) vec4f {      // 4 taps bilineales = caja 4×4
      let o = g.texel * 0.5;
      let c = textureSample(src, smp, in.uv + vec2f(-o.x,-o.y)) + textureSample(src, smp, in.uv + vec2f(o.x,-o.y))
            + textureSample(src, smp, in.uv + vec2f(-o.x, o.y)) + textureSample(src, smp, in.uv + vec2f(o.x, o.y));
      return vec4f(c.rgb * 0.25, 1.0);
  }
  @fragment fn fs_blur(in: FsIn) -> @location(0) vec4f {      // gauss 9 taps con 5 lecturas lineales
      let d = g.dir * g.texel * g.spread;
      var c = textureSample(src, smp, in.uv).rgb * 0.2270270270;
      c += (textureSample(src, smp, in.uv + d*1.3846153846).rgb + textureSample(src, smp, in.uv - d*1.3846153846).rgb) * 0.3162162162;
      c += (textureSample(src, smp, in.uv + d*3.2307692308).rgb + textureSample(src, smp, in.uv - d*3.2307692308).rgb) * 0.0702702703;
      return vec4f(c, 1.0);
  }
  @fragment fn fs_composite(in: FsIn) -> @location(0) vec4f { // algo más de saturación, techo para que el texto se lea
      var c = textureSample(src, smp, in.uv).rgb;
      let l = dot(c, vec3f(0.2126, 0.7152, 0.0722));
      c = min(mix(vec3f(l), c, 1.25), vec3f(0.80));
      return vec4f(c, 1.0);
  }
  ```
- Nuevo `sim-gpu/src/glass.rs` con `struct Glass`: dos texturas `rgba16float` a **1/4 de resolución** (A y B, con `RENDER_ATTACHMENT | TEXTURE_BINDING`), un sampler lineal clamp, 3 pipelines (`fs_down` y `fs_blur` hacia rgba16float; `fs_composite` hacia `config.format`), bind groups `down` (src = escena), `h` (src A, dir (1,0)), `v` (src B, dir (0,1)) y `comp` (src A), cada uno con su uniform buffer. API: `new(device, format, scene_view, w, h)`, `resize(device, scene_view, w, h)` y `encode(encoder, target_view, rect_px: [u32;4])`. Este último hace down→A, h A→B, v B→A, h A→B, v B→A (2 iteraciones con spread 1.5) y luego composite a `target_view` con `LoadOp::Load` y `set_scissor_rect(rect_px)` (con clamp al tamaño de la superficie y saltándose el paso si el ancho o el alto valen 0).
- `gpu_sim.rs`: añadir `pub fn scene_view(&self) -> &wgpu::TextureView { &self.offscreen_view }`.
- `sim-gpu/src/main.rs`: crear `Glass` junto a `egui_renderer` (~l.400); en el resize, después de `sim.resize(...)`, llamar a `glass.resize(...)` (hay que recrearlo porque la escena se recrea); en `render()`, después de `sim.frame`/`draw_photo` y **antes** del pase de egui, `if panel_visible { if let Some(r) = self.panel.glass_rect { glass.encode(&mut encoder, &view, r × out.pixels_per_point redondeado) } }`.
  Nota: la foto superpuesta se dibuja directamente en `view`, no en la escena HDR, así que no aparece en el desenfoque. Es aceptable.

### 6. Contexto de impeccable
- Crear `PRODUCT.md` en la raíz, deducido del encargo y del código (con la sección "Supuestos" marcada): un instrumento de show audiovisual de partículas para crear vídeo vertical y actuar en directo; usuario único, el autor; superficie de panel en modo Operate.
- Al terminar la implementación, generar `DESIGN.md` con el subagente `impeccable:impeccable-documenter` a partir de lo construido.

## Archivos críticos

- `shared/src/ui_theme.rs` (reescrito) · `shared/src/ui_kit.rs` (nuevo) · `shared/src/panel_ui.rs` → `shared/src/panel_ui/{mod,header,tab_sim,tab_look,tab_show,tab_out}.rs` · `shared/src/lib.rs`
- `sim-gpu/src/glass.rs` + `sim-gpu/src/shaders/glass.wgsl` (nuevos) · `sim-gpu/src/gpu_sim.rs` (getter) · `sim-gpu/src/main.rs`
- `sim/src/main.rs:1449` · `panel/src/main.rs:17,258`
- `assets/fonts/Inter-{Regular,SemiBold}.ttf` + licencia · `PRODUCT.md`, `DESIGN.md`

## Verificación

1. `cargo build --release --workspace` sin warnings nuevos; `cargo clippy -p shared -p sim-gpu`.
2. `cargo run --release -p sim-gpu`: la lámina flota con 10 px de margen y las partículas se ven desenfocadas detrás al moverse; redimensionar la ventana y el panel (sin panic, el blur sigue el rect); `H` lo oculta y desaparece el blur; grabar con `R` y comprobar en el .mp4 que no aparece ni el panel ni el blur.
3. `cargo run --release -p sim` (fondo ambiental) y separar con `D` → el `panel` en ventana aparte tiene el mismo aspecto; reacoplar.
4. Un solo pase de capturas (`grim -g "$(slurp)"`) de las 4 pestañas con el panel a 300 px y a 420 px. Revisar: jerarquía, que no se corten textos, el contraste del texto sobre zonas con muchas partículas y el estado de REC. Arreglar todo en una sola tanda y hacer como mucho una ronda más.
5. Prueba de regresión funcional, pestaña por pestaña: cambiar de modo dispara la transición; Aleatorizar/Restablecer la matriz; arrastrar una celda de la matriz cambia la simulación; Llenar/Vaciar; pincel y fuerza; texto/imagen/guardar/soltar y biblioteca; guardar/cargar/predeterminada/borrar escena; secuenciador (añadir, reordenar, duración, transición propia, play, saltar); encuadre y presets; música → analizar → sincronía; reactivo al audio. Todo debe comportarse igual que antes.
