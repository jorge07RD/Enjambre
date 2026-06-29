# Enjambre — Puntos de Atracción

Un simulador interactivo de **vida de partículas** (*particle life*) escrito en Rust.
Miles de puntos de colores se mueven según reglas simples de atracción y repulsión
y, a partir de ellas, emergen patrones complejos: enjambres, células, anillos,
cadenas y estructuras que parecen vivas.

![Modo "mismo color": cada color se agrupa en anillos](docs/img/01-inicio.png)

## ¿Qué es esto?

Cada partícula tiene un **color** (un matiz en la rueda de color) y siente una fuerza
hacia las demás que depende de:

- **La distancia** entre ellas (con un radio máximo de influencia `r_max`).
- **El color** del par, según el modo de interacción elegido.

Muy de cerca todas se **repelen** (no se apilan); a media distancia se **atraen o se
repelen** según las reglas de color. Con esas dos reglas básicas, más una pizca de
fricción, aparecen comportamientos colectivos sorprendentes — sin que nadie los
programe explícitamente.

## Características

- **Hasta decenas de miles de partículas** en tiempo real. El cálculo de fuerzas usa
  un *hash* espacial (rejilla CSR) y se reparte entre todos los núcleos con
  [`rayon`](https://crates.io/crates/rayon).
- **Tres modos de interacción:**
  - **Mismo color** — solo los iguales se atraen (opcionalmente, los distintos se repelen).
  - **Matriz** — una tabla 6×6 editable define cuánto atrae/repele cada color a cada otro,
    al estilo *particle life* clásico. Botón para aleatorizar las reglas.
  - **Similitud** — la atracción depende de lo parecidos que sean los matices en la rueda
    de color (los tonos vecinos se atraen, los opuestos se repelen).
- **Física ajustable en vivo:** fuerza, radio, repulsión (β), fricción, velocidad y
  bordes **toroidales** (la pantalla se enrolla) o de **rebote**.
- **Dinámica del color:** cambios aleatorios de color, deriva lenta y gradual de
  colores y reglas, con transiciones suaves opcionales.
- **Lienzo + cámara:** lienzo de tamaño variable con zoom y desplazamiento (rueda para
  zoom hacia el cursor, botón derecho para mover).
- **Pincel:** pinta o borra partículas del color que quieras directamente sobre el lienzo.
- **Tres estilos de dibujo:** sólido, brillo (*glow*) y sólido con halo.

![Modo "matriz": clústeres orgánicos mezclando colores según la tabla 6×6](docs/img/02-matriz.png)

## Tecnologías

| Componente | Biblioteca |
|------------|------------|
| Render / ventana | [`macroquad`](https://crates.io/crates/macroquad) |
| Interfaz de control | [`egui-macroquad`](https://crates.io/crates/egui-macroquad) |
| Paralelismo | [`rayon`](https://crates.io/crates/rayon) |
| Aleatoriedad | [`rand`](https://crates.io/crates/rand) |

## Compilar y ejecutar

Necesitas [Rust](https://rustup.rs/) instalado.

```bash
# Ejecutar en modo optimizado (recomendado, va mucho más fluido)
cargo run --release
```

El binario se llama `puntos_atraccion`.

### Benchmark

Hay una prueba de rendimiento que mide los pasos de simulación por segundo para
5 000, 20 000 y 50 000 partículas:

```bash
cargo test --release throughput -- --nocapture
```

## Controles rápidos

- **Rueda del ratón** — zoom hacia el cursor.
- **Botón derecho / central** — mover la vista (*pan*).
- **Botón izquierdo sobre el lienzo** — pintar o borrar (según la brocha activa).
- Todo lo demás se ajusta desde el **panel lateral derecho**.

## Estructura del código

| Archivo | Responsabilidad |
|---------|-----------------|
| `src/main.rs` | Bucle principal, interfaz (egui) y cámara. |
| `src/simulation.rs` | Partículas, integración de la física y perfil de fuerza. |
| `src/grid.rs` | Hash espacial uniforme (CSR) para buscar vecinos rápido. |
| `src/config.rs` | Parámetros, modos de interacción y utilidades de color. |
| `src/render.rs` | Dibujo por lotes de las partículas con texturas (sólido/glow/halo). |

---

> Las capturas muestran 2 000 partículas; el simulador admite muchas más.
> Prueba a subir la cantidad, cambiar de modo y pulsar **🎲 Aleatorizar reglas**.
