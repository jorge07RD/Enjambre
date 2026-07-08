<h1 align="center">🐝 Enjambre — Puntos de Atracción</h1>

<p align="center">
  <em>Simulador interactivo de <strong>vida de partículas</strong> en Rust: miles de puntos de colores que se organizan solos en enjambres, células, anillos y estructuras vivas.</em>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-2021-000000?logo=rust&logoColor=white" alt="Rust">
  <img src="https://img.shields.io/badge/render-macroquad-FF6B00" alt="macroquad">
  <img src="https://img.shields.io/badge/GPU-wgpu%20(experimental)-CC6600" alt="wgpu">
  <img src="https://img.shields.io/badge/UI-egui%20%2F%20eframe-1E88E5?logo=egui&logoColor=white" alt="egui / eframe">
  <img src="https://img.shields.io/badge/paralelismo-rayon-8E44AD" alt="rayon">
  <img src="https://img.shields.io/badge/Linux-Wayland%20%2F%20Hyprland-1793D1?logo=linux&logoColor=white" alt="Linux · Wayland/Hyprland">
  <img src="https://img.shields.io/badge/v%C3%ADdeo-TikTok%209%3A16-000000?logo=tiktok&logoColor=white" alt="Vídeo vertical">
  <img src="https://img.shields.io/badge/estado-activo-2ECC71" alt="Estado: activo">
</p>

A partir de reglas simples de atracción y repulsión emergen patrones complejos —sin
que nadie los programe explícitamente—: enjambres, células, cadenas y ondas viajeras.

## 🎬 Demo

Vídeo vertical **9:16** grabado desde la propia app (tecla `R`), como los que se suben a TikTok:

<p align="center">
  <img src="docs/img/demo.gif" width="300" alt="Demo animada: células orgánicas de partículas en formato vertical">
</p>

<p align="center">
  <img src="docs/img/03-video-rosetas.png" width="220" alt="Rosetas de colores">
  <img src="docs/img/04-video-flujo.png" width="220" alt="Estructuras orgánicas en flujo">
  <img src="docs/img/05-video-denso.png" width="220" alt="Campo denso de células">
</p>

> ▶️ **Vídeo en alta calidad** (1080×1920, 120 fps): [`docs/img/demo.mp4`](docs/img/demo.mp4).
> GitHub muestra un reproductor con la etiqueta de abajo; en otros visores, usa el enlace o el GIF.

<video src="docs/img/demo.mp4" controls loop muted width="300"></video>

![Modo "mismo color": cada color se agrupa en anillos](docs/img/01-inicio.png)

## 🌌 ¿Qué es esto?

Cada partícula tiene un **color** (un matiz en la rueda de color) y siente una fuerza
hacia las demás que depende de:

- **La distancia** entre ellas (con un radio máximo de influencia `r_max`).
- **El color** del par, según el modo de interacción elegido.

Muy de cerca todas se **repelen** (no se apilan); a media distancia se **atraen o se
repelen** según las reglas de color. Con esas dos reglas básicas, más una pizca de
fricción, aparecen comportamientos colectivos sorprendentes — sin que nadie los
programe explícitamente.

## ✨ Características

- 🐝 **Hasta decenas de miles de partículas** en tiempo real. El cálculo de fuerzas usa
  un *hash* espacial (rejilla CSR) y se reparte entre todos los núcleos con
  [`rayon`](https://crates.io/crates/rayon).
- 🎨 **Ocho modos de interacción:**
  - **Mismo color** — solo los iguales se atraen (opcionalmente, los distintos se repelen).
  - **Matriz** — una tabla 6×6 editable define cuánto atrae/repele cada color a cada otro,
    al estilo *particle life* clásico. Botón para aleatorizar las reglas.
  - **Similitud** — la atracción depende de lo parecidos que sean los matices en la rueda
    de color (los tonos vecinos se atraen, los opuestos se repelen).
  - **Cíclico** (piedra-papel-tijera) — cada color persigue al siguiente de la rueda y huye
    del anterior: persecuciones, espirales y ondas viajeras.
  - **Opuestos** — los colores complementarios se atraen y los parecidos se repelen.
  - **Depredador–presa** — un bando caza y el otro huye en manada (interacción asimétrica).
  - **Repulsión propia** — el mismo color se repele y los distintos se atraen (mezclas
    homogéneas, espumas y mosaicos).
  - 🐦 **Bandada (Boids)** — murmuraciones de estorninos al estilo Craig Reynolds (1986):
    cada partícula sigue solo tres reglas locales —**separación** (no chocar),
    **alineación** (ir hacia donde van los vecinos) y **cohesión** (acercarse al grupo)— y
    emerge una nube coordinada de fibras cambiantes, sin líder. Ámbito ajustable (todas
    juntas, una bandada por color o híbrido) y velocidad de crucero para que no se detengan.
- ⚙️ **Física ajustable en vivo:** fuerza, radio, repulsión (β), fricción, velocidad (en %,
  con cambio suave y atajos 1…0) y bordes **toroidales** (la pantalla se enrolla) o de **rebote**.
- 🌈 **Dinámica del color:** cambios aleatorios de color, deriva lenta y gradual de
  colores y reglas, con transiciones suaves opcionales. La matriz puede **auto-aleatorizarse
  cada X segundos** para animaciones que evolucionan solas.
- 🎥 **Grabación de vídeo:** define con un **recuadro ajustable** sobre el lienzo (mover/redimensionar,
  con rejilla de tercios; tecla **`G`** para mostrar/ocultar) la zona a grabar, elige un **tamaño
  sugerido** (TikTok 9:16, 4:5, 1:1, 16:9…) y graba a 120 fps con la tecla **`R`** o el botón (contador
  `REC mm:ss` en el HUD mientras graba). El vídeo se codifica con `ffmpeg` a H.264; puedes **elegir la
  carpeta de guardado** desde la app (recuerda la última carpeta usada, también para música e imágenes).
- 🎞️ **Escenas:** guarda la configuración actual como una **instantánea con nombre** para reproducir un
  escenario cuando quieras, marca una como **predeterminada** (se carga al arrancar) y **cambia entre
  escenas** de forma instantánea o con una **transición suave** (interpola los parámetros y cruza el
  modo de interacción). Se guardan en `~/.config/enjambre/scenes.json`.
- 🎬 **Secuenciador de escenas (playlist):** encadena escenas con una **duración propia por entrada**
  (transición incluida) para montar un *show* completo y reproducible; reordena, repite al terminar
  (**loop**) o arranca la secuencia automáticamente **al empezar a grabar**. Transporte ⏮ ▶ ⏸ ⏭ y salto
  directo a cualquier entrada. Se guarda en `~/.config/enjambre/playlist.json`.
- 🎧 **Sincronía con música:** analiza una pista (**envolvente de energía + bandas de graves/medios/agudos
  + beats/onsets + BPM estimado**, vía `ffmpeg`) para disparar acciones sobre el enjambre al ritmo —además
  de la modulación en vivo por micrófono— con preescucha (`ffplay`) para comprobar la detección antes de grabar.
- 🖼️ **Lienzo + cámara:** lienzo de tamaño variable con zoom y desplazamiento (rueda para
  zoom hacia el cursor, botón derecho para mover). Botón **«Lienzo = pantalla»** que iguala
  el mundo a los píxeles de la ventana (1:1), para que llene el lienzo sea cual sea el
  tamaño que le dé el gestor de ventanas (ideal para tiling como Hyprland).
- 🪟 **Panel separable:** el panel de control puede vivir embebido a la derecha del lienzo
  o, con un clic, abrirse como **ventana del SO aparte** (proceso `panel`) que se puede
  tilear/redimensionar por separado. Ambos hablan por un socket Unix.
- 🖌️ **Pincel:** pinta o borra partículas del color que quieras directamente sobre el lienzo.
- 💡 **Tres estilos de dibujo:** sólido, brillo (*glow*) y sólido con halo.
- 🌠 **Estelas de movimiento:** buffer que se desvanece para que las partículas dejen rastro
  (longitud ajustable). Espectacular en bandada y cíclico; también se graba en el vídeo.
- 🧭 **Orientar según movimiento:** dibuja cada partícula como una flecha (triángulo) apuntando
  hacia donde va — la bandada se lee como pájaros de verdad.
- 🖱️ **Fuerza con el ratón:** cambia el pincel por la herramienta **Fuerza** para atraer o
  **espantar** el enjambre en vivo alrededor del cursor (radio e intensidad ajustables).
- 🎵 **Reactivo al audio:** el sonido del micrófono/entrada modula la **velocidad**, la **fuerza**
  o el **brillo**; el enjambre "baila" con la música. Usa `ffmpeg` (PulseAudio/PipeWire o ALSA);
  si no hay entrada, la opción simplemente no hace nada.
- ✍️ **Formar texto e imágenes:** escribe un mensaje o importa un PNG (logo/emoji/silueta) y las
  partículas se agrupan para formarlo. Slider de **fijación** (de "texto vivo" que respira a
  "nítido" para leerse claro), opción de **teñir** de un color o mantener el arcoíris, y botón
  **Soltar** para liberar el enjambre. Cuantas más partículas, más legible.
- 🖼️ **Recrear una foto o un vídeo con las partículas:** importa una imagen (o un **vídeo**) y marca
  **«Recrear colores de la foto»**: las partículas se acomodan en un **mosaico puntillista** que toma
  los colores reales de la imagen y, ya formadas, la foto nítida se **funde encima**. Con **vídeo**,
  una vez formada la imagen **se reproduce sobre el enjambre** y, al acabar, **sale sola** en reverso
  (la imagen se desvanece, quedan las partículas con la silueta y se liberan). Las partículas que no
  forman la imagen siguen su comportamiento y **chocan** con ella. Al **grabar**, el **audio del vídeo**
  se incluye en el `.mp4`. (Requiere `ffmpeg`.)

![Modo "matriz": clústeres orgánicos mezclando colores según la tabla 6×6](docs/img/02-matriz.png)

## 🧰 Tecnologías

| Componente | Biblioteca |
|------------|------------|
| Render / ventana (lienzo, `sim`) | [`macroquad`](https://crates.io/crates/macroquad) |
| Motor GPU experimental (`sim-gpu`) | [`wgpu`](https://crates.io/crates/wgpu) (compute shaders WGSL) / [`winit`](https://crates.io/crates/winit) |
| Panel embebido | [`egui-macroquad`](https://crates.io/crates/egui-macroquad) (`sim`) / [`egui-wgpu`](https://crates.io/crates/egui-wgpu) (`sim-gpu`) |
| Panel en ventana aparte | [`eframe`](https://crates.io/crates/eframe) / [`egui`](https://crates.io/crates/egui) |
| IPC panel ↔ simulación | socket Unix + [`serde`](https://crates.io/crates/serde) (JSON) |
| Paralelismo | [`rayon`](https://crates.io/crates/rayon) |
| Aleatoriedad | [`rand`](https://crates.io/crates/rand) |
| Diálogo de carpeta/fichero | [`rfd`](https://crates.io/crates/rfd) (portal XDG) |
| Vídeo (grabar + decodificar fotogramas), audio | [`ffmpeg`](https://ffmpeg.org/) / `ffprobe` (externo) |
| Análisis de música (envolvente/beats/BPM) | [`realfft`](https://crates.io/crates/realfft) + `ffmpeg` (decodificación) |
| Formas de texto (`sim-gpu`) | [`ab_glyph`](https://crates.io/crates/ab_glyph) (fuente del sistema) + [`image`](https://crates.io/crates/image) |

## 🚀 Compilar y ejecutar

Necesitas [Rust](https://rustup.rs/) instalado.

El proyecto es un *workspace* de Cargo con cuatro crates: `sim` (la simulación en CPU
y el lienzo, la app principal), `sim-gpu` (motor experimental con la física entera en
compute shaders wgpu, ver más abajo), `panel` (el panel en ventana aparte) y `shared`
(parámetros, UI del panel, escenas/playlist/formas y utilidades comunes a los tres).

```bash
# 1) Compilar TODO el workspace (sim + sim-gpu + panel) en modo optimizado
cargo build --release

# 2) Ejecutar la simulación (va mucho más fluido en release)
cargo run -p sim --release
```

> **Importante:** compila el *workspace* entero (`cargo build`), no solo `-p sim`.
> El botón «Separar panel» lanza el binario `panel`, así que tiene que existir en
> `target/debug/` o `target/release/`. Si falta, el `sim` lo avisa por la terminal y
> sigue con el panel embebido.

El panel arranca embebido. Para separarlo, pulsa **«🗗 Separar panel en otra ventana»**:
el `sim` lanza automáticamente el binario `panel`. (También puedes ejecutarlo a mano con
`cargo run -p panel --release` mientras el `sim` está abierto.)

### 📊 Benchmark

Hay una prueba de rendimiento que mide los pasos de simulación por segundo para
5 000, 20 000 y 50 000 partículas:

```bash
cargo test -p sim --release throughput -- --nocapture
```

## 🎮 Controles rápidos

- **Rueda del ratón** — zoom hacia el cursor.
- **Botón derecho / central** — mover la vista (*pan*).
- **Botón izquierdo sobre el lienzo** — pintar o borrar (o mover/redimensionar el recuadro
  de grabación si está visible).
- Todo lo demás se ajusta desde el **panel de control** (embebido a la derecha o en
  su ventana aparte).

### ⌨️ Atajos de teclado (sobre la ventana del lienzo)

| Tecla | Acción | | Tecla | Acción |
|-------|--------|-|-------|--------|
| **Espacio** | Pausa / Reanudar | | **L** | Lienzo = pantalla |
| **.** | Paso a paso | | **Z** | Ajustar zoom al lienzo |
| **C** | Vaciar / Reiniciar | | **D** | Separar / reacoplar panel |
| **F** | Llenar aleatorio | | **R** | Grabar / detener vídeo |
| **M** | Aleatorizar matriz | | **G** | Mostrar / ocultar encuadre |
| **1…9 / 0** | Velocidad 10 %…100 % | | **A** | Atraer zonas activas al centro |
| **N / P** | Escena siguiente / anterior | | | |

## 🎥 Grabación de vídeo

1. Pulsa **`G`** (o el checkbox del panel) para mostrar el **recuadro de encuadre**; arrástralo
   para moverlo o coge una esquina para redimensionarlo. Elige un **Tamaño** sugerido en el panel.
2. Opcional: **📁 Carpeta de guardado…** abre un diálogo nativo para elegir dónde guardar.
3. Pulsa **`R`** (o el botón) para grabar y de nuevo para parar. Sale un `enjambre_<timestamp>.mp4`
   a 120 fps con exactamente la zona del recuadro, a la resolución del preset.

> Requiere **`ffmpeg`** instalado (`sudo pacman -S ffmpeg`). El diálogo de carpeta usa el **portal
> XDG**; en Hyprland instala `xdg-desktop-portal` + un backend (p. ej. `xdg-desktop-portal-gtk`
> o `-hyprland`). Sin portal, se guarda en el directorio actual.

![Recuadro de encuadre 9:16 con rejilla de tercios sobre el lienzo, y el panel de control](docs/img/06-encuadre.png)

## 🖼️ Formar una foto o un vídeo con las partículas

En la sección **Mensaje / Forma** del panel:

1. Pulsa **«🖼️ Imagen…»** y elige una **imagen** (PNG/JPG/WEBP…) o un **vídeo**
   (MP4/MOV/MKV/WEBM/AVI/M4V).
2. Marca **«Recrear colores de la foto»** (se activa **sola** al elegir un vídeo). Las partículas
   se acomodan formando un **mosaico** con los colores reales de la imagen y, al terminar de
   formarse, la imagen nítida se **funde encima**.
3. Con un **vídeo**, la reproducción arranca **cuando la imagen ya está formada** y se reproduce
   **una vez** sobre el enjambre; al acabar **sale sola** (la misma transición en reverso). Pulsa
   **Soltar** para liberarla antes de tiempo.
4. **Grabando** (tecla `R`): el **audio del vídeo** se muxea en el `.mp4` en el momento en que
   aparece; si además hay **música** cargada, se **mezclan**.

> Una imagen con **transparencia** (PNG sin fondo) recluta partículas **solo** donde hay dibujo; el
> resto del enjambre sigue su comportamiento y **choca** con la figura. Requiere **`ffmpeg`**/`ffprobe`
> (decodifica los fotogramas del vídeo por streaming, sin cargarlo entero en memoria).

## 🎞️ Escenas (instantáneas de configuración)

En la sección **Escenas** del panel:

1. Escribe un nombre y pulsa **📸 Guardar** para guardar la configuración actual como escena.
2. En la lista, **▶** carga una escena, **⟳** la **actualiza** con la configuración actual,
   **★** la marca como predeterminada (se carga al arrancar) y **🗑** la borra.
3. Marca **«Transición suave entre escenas»** (con su duración) para que al cargar una escena los
   parámetros se **interpolen** y el modo de interacción se **cruce** gradualmente; si lo desmarcas,
   el cambio es instantáneo.
4. **Ciclar:** botones **⏮/⏭** o teclas **`P`/`N`** para ir a la escena anterior/siguiente. Con
   **«Auto-avance (slideshow)»** cambia sola cada X segundos.
5. **Compartir/respaldar:** **⬆ Exportar todas** vuelca la colección a un `.json` (diálogo nativo) y
   **⬇ Importar…** la fusiona con la tuya.

Las escenas se guardan en `~/.config/enjambre/scenes.json` (solo los ajustes; las partículas se
rehacen con las nuevas reglas). **En el primer arranque** se siembran unas escenas de ejemplo
(Enjambres, Células, Cazadores, Cíclico, Espuma, Opuestos).

## 🎼 Secuenciador (montar un show) y sincronía con música

En la sección **Secuenciador** del panel se arma una **playlist**: cada entrada es una escena +
duración (+ transición propia opcional). Con **▶** se reproduce la secuencia completa cambiando de
escena sola; **«Repetir al terminar»** la vuelve a empezar en bucle y **«Arrancar con la grabación»**
hace que pulsar `R` dispare el show desde el principio — ideal para grabar un vídeo largo sin tocar
nada. Se guarda en `~/.config/enjambre/playlist.json`.

En la sección **Grabación** también está la **sincronía con música**: elige una pista y pulsa
**Analizar** para extraer su envolvente de energía, beats/onsets y BPM estimado (todo con `ffmpeg`,
en segundo plano); el botón de **preescucha** la reproduce con `ffplay` para comprobar la detección.
Con eso, el modo **«Reactivo al audio»** puede seguir la pista ya analizada además del micrófono en vivo.

## 🖥️ Motor GPU experimental (`sim-gpu`)

Segundo motor, independiente de `sim`, con la física entera corriendo en **compute shaders** (wgpu)
y el render leyendo los buffers de partículas sin pasar por la CPU. Reutiliza el mismo panel
(`shared::config_panel`, embebido vía `egui-wgpu`) y las mismas escenas/playlist/formas — pensado para
enjambres mucho más grandes o hardware sin CPU multinúcleo potente. Tiene paridad casi completa con
`sim`: los ocho modos de interacción y Boids, ambos contornos, transiciones suaves, estilos/flechas/
bloom/estelas, formas de texto e imagen (con física de resortes en el propio shader), y grabación de
vídeo con audio/música. Recarga en caliente: si editas `scenes.json`/`playlist.json`/`shapes.json` a
mano mientras `sim-gpu` está abierto, los relee del disco automáticamente, sin reiniciar.

```bash
# Por defecto: 20 000 partículas, horizontal, grabación a 1080p
cargo run -p sim-gpu --release

# Vertical (retrato 9:19.5, pantalla completa de móvil) grabando en 4K
cargo run -p sim-gpu --release -- 40000 vertical 4k
```

Argumentos posicionales (en cualquier orden): número de partículas · `vertical`/`v`/`movil` (por
defecto horizontal) · `2k`/`4k` (por defecto 1080p) — la calidad afecta solo a la **grabación**, que
se renderiza a esa resolución exacta independientemente del tamaño de la ventana. Tecla **`H`** para
mostrar/ocultar el panel, **`R`** para grabar; el resto de atajos son los mismos que en `sim` (ver
la cabecera de `sim-gpu/src/main.rs` para el listado completo). Por ahora no tiene panel separable
por IPC ni recuadro de encuadre arrastrable: la ventana entera es el lienzo.

## 🗒️ Autoría de escenas/playlist/formas por JSON

Los tres ficheros de `~/.config/enjambre/` (`scenes.json`, `playlist.json`, `shapes.json`) se pueden
editar directamente a mano —además de desde el panel— para crear escenas, montar un show o añadir
formas sin abrir la app. Antes de escribir, valida contra los tipos reales:

```bash
cargo run -q -p shared --example validate_json -- scenes ~/.config/enjambre/scenes.json
cargo run -q -p shared --example validate_json -- playlist ~/.config/enjambre/playlist.json
cargo run -q -p shared --example validate_json -- shapes ~/.config/enjambre/shapes.json
```

> ⚠️ Si `sim`/`sim-gpu` está abierto, es dueño de estos JSON en memoria y los sobreescribe al hacer
> cualquier operación de escena (guardar/borrar/predeterminada) — edítalos con la app cerrada, o haz
> una copia de seguridad antes y verifica el contenido después de guardar.

## 🗂️ Estructura del código

| Archivo | Responsabilidad |
|---------|-----------------|
| `sim/src/main.rs` | Bucle principal, cámara, modo embebido/separado y servidor IPC. |
| `sim/src/simulation.rs` | Partículas, integración de la física y perfil de fuerza. |
| `sim/src/grid.rs` | Hash espacial uniforme (CSR) para buscar vecinos rápido. |
| `sim/src/render.rs` | Dibujo por lotes de las partículas con texturas (sólido/glow/halo). |
| `sim-gpu/src/main.rs` | App wgpu/winit: ventana, entrada, panel embebido, secuenciador, recarga en caliente de los JSON. |
| `sim-gpu/src/gpu_sim.rs` | Buffers y *compute shaders* (`shaders/*.wgsl`): física, color, render de partículas. |
| `sim-gpu/src/rec.rs` | Grabación de vídeo en GPU (blit a textura + anillo de *staging buffers*). |
| `sim-gpu/src/shape.rs` | Rasterizado de texto/imagen para las formas del enjambre en `sim-gpu`. |
| `shared/src/config.rs` | Parámetros (`SimParams`), modos de interacción y utilidades de color. |
| `shared/src/panel_ui.rs` | UI egui del panel, compartida por los tres binarios. |
| `shared/src/ipc.rs` | Tipos de mensaje y encuadre del socket Unix. |
| `shared/src/scenes.rs` / `playlist.rs` / `shapes.rs` | Persistencia en JSON de escenas, secuenciador y biblioteca de formas. |
| `shared/src/music.rs` / `audio.rs` | Análisis offline de pistas (envolvente/beats/BPM) y entrada de audio en vivo. |
| `shared/src/video.rs` | Streaming de fotogramas de vídeo (`ffmpeg`) para el efecto foto/vídeo. |
| `shared/src/dialog_dirs.rs` | Recuerda la última carpeta usada por cada diálogo nativo (`rfd`). |
| `shared/examples/validate_json.rs` | Validador de `scenes.json`/`playlist.json`/`shapes.json` contra los tipos reales. |
| `panel/src/main.rs` | Panel en ventana del SO aparte (cliente IPC, `eframe`). |

---

> Las capturas muestran 2 000 partículas; el simulador admite muchas más.
> Prueba a subir la cantidad, cambiar de modo y pulsar **🎲 Aleatorizar reglas**.
