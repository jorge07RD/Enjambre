# Enjambre — sistema de diseño del panel: "cristal de instrumento"

Una lámina de cristal ahumado flotando sobre el enjambre en vivo, con tarjetas de
cristal más claras encima. Un único acento (iris), colores semánticos solo para
estado y cifras en mono, para que lean como lecturas de un aparato. Derivado de
lo construido en `shared/src/ui_theme.rs` (tokens), `shared/src/ui_kit.rs`
(componentes) y `shared/src/panel_ui/` (composición).

## Materiales

| Capa | Cómo se hace |
|---|---|
| Fondo (`sim-gpu`) | la escena real, reducida a 1/4 y desenfocada (gauss separable ×2) **solo detrás de la lámina**: `sim-gpu/src/glass.rs` + `shaders/glass.wgsl`. La composición sube un poco la saturación y pone un techo de luminancia de 0.8, para que el texto se lea siempre |
| Fondo (`sim`, `panel`) | `BG_BASE` + 3 manchas radiales estáticas (iris, teal, magenta): `ui_kit::ambient` |
| Lámina | margen de 10 px, radio 16, tinte `GLASS_TINT_LIVE`/`_AMBIENT`, borde `HAIRLINE` y bisel superior `BEVEL` (una línea de 1 px que se desvanece hacia los extremos) |
| Tarjeta | relleno `CARD_FILL`, borde `HAIRLINE`, radio 12, margen interior 12 y el mismo bisel |

El host elige el material con `PanelState.backdrop` (`Backdrop::Live` | `Ambient`);
`config_panel` escribe en `PanelState.glass_rect` el rect de la lámina para el blur.

## Tokens (`ui_theme.rs`)

| Token | Valor | Uso |
|---|---|---|
| `BG_BASE` | `#0A0C11` | fondo opaco |
| `GLASS_TINT_LIVE` / `_AMBIENT` | rgb(14,16,24) α .58 / .72 | tinte de la lámina |
| `CARD_FILL` / `_HOVER` | blanco α .035 / .055 | tarjetas |
| `HAIRLINE` · `BEVEL` | blanco α .08 · .14 | bordes · canto superior |
| `TEXT_1/2/3` | `#ECEEF4` `#A6ABBA` `#6C7183` | primario · etiquetas · pistas |
| `ACCENT` · `ACCENT_SOFT` | `#9D8CFF` · α .20 | selección, relleno de slider, switch, pestaña |
| `OK` `WARN` `DANGER` | `#5EE6A8` `#F5B84B` `#FF5A63` | en vivo / FPS · avisos · REC y borrar |

- **Radios:** lámina 16 · tarjeta 12 · controles 8 · chips, switch y kbd: píldora/4.
- **Espaciado:** escala de 4 · `item_spacing (8,8)` · margen interior de tarjeta 12.
- **Tipografía:** Inter Regular / SemiBold (`assets/fonts`, OFL) y Hack (mono) para las cifras. Cuerpo 13 · título de tarjeta 13.5 SemiBold · wordmark 17 · pista 11.5 · cifras 11.5–12.
- **Iconos:** Nerd Font (`assets/icons.ttf`), colocada justo detrás de Inter en la cadena de fallback (Ubuntu, la fuente por defecto de egui, tiene "fi"/"fl" en U+F001/F002).
- **Movimiento:** solo `animate_bool`/`animate_value` (unos 120–160 ms) en el switch, el plegado de tarjetas y el indicador de pestaña. Lo único que late en bucle es el punto de REC.

## Composición

- **Cabecera fija** (sin scroll): wordmark + punto de estado (verde corriendo / ámbar en pausa), separar/ocultar, chips de partículas y FPS (+ REC), tarjeta de transporte (Pausa · Paso · Reiniciar), velocidad con riel y control segmentado 10–100, y "cambio suave" en una sola fila.
- **Pestañas:** Simulación (Interacción · Física · Partículas y herramienta) · Aspecto (Apariencia · Dinámica del color · Lienzo y vista) · Show (Mensaje/Forma · Escenas · Secuenciador) · Salida (Grabación · Música y sincronía · Reactivo al audio).
- Debajo, una zona de scroll con las tarjetas plegables (su estado se guarda en la memoria de egui).

## Componentes (`ui_kit.rs`)

`glass_slab`, `gutter`, `card`, `card_switch` (el interruptor de la cabecera activa la sección), `plain_card`,
`toggle` / `switch_row` (con `kbd` opcional; la etiqueta también conmuta), `nested` (dependientes con una regla
de acento), `slider_row` (etiqueta y valor editable arriba, riel a todo el ancho), `segmented(_tips)`
(≤4 opciones), `chips(_tips)` (>4), `swatches`, `button` / `button_w` / `primary_button` / `danger_button`,
`icon_button` / `icon_btn`, `kbd`, `stat_chip`, `rec_chip`, `hint`, `warn`, `sub_label`, `divider`,
`row`, `list_row`, `tab_bar`, `matrix_editor` (celdas en mapa de color iris/coral; arrastrar para ajustar, doble clic = 0).

## Reglas

- Un atajo de teclado se muestra como `kbd` y se repite en el tooltip; no va metido en la etiqueta entre paréntesis.
- Las explicaciones son `hint`, no texto de cuerpo. Los avisos son `warn`.
- Una fila con algo pegado a la derecha va dentro de `ui.horizontal(...)` + `right_to_left`: así no se come el alto restante.
- `TextEdit::desired_width` no incluye su margen: hay que restarlo (véase `tab_show::text_field`).
- Los colores con alfa se crean sin premultiplicar (`ui_theme::rgba` / `from_rgba_unmultiplied`).
- En hosts que aplican el tema dentro del frame (`sim`), el panel no se dibuja en el frame en que se llama a `set_fonts`.
