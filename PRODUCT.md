# Enjambre — producto

## Qué es
Instrumento audiovisual de partículas (vida artificial / *particle life*) para
**crear vídeo vertical** (TikTok, Reels) y **actuar en directo**: el enjambre
reacciona a reglas de atracción, a la música y al audio en vivo, forma textos,
imágenes y vídeos, y se graba a 1080p/2K con la música mezclada.

Tres binarios comparten la misma UI de control (`shared::config_panel`):
`sim-gpu` (motor wgpu, el principal), `sim` (motor CPU, macroquad) y `panel`
(el panel en una ventana del SO aparte, por IPC).

## Para quién
Un solo usuario: el autor, que es a la vez programador, creador de contenido y
quien maneja el show. Conoce a fondo cada parámetro; no hace falta onboarding,
pero sí poder encontrar y ajustar cualquier cosa rápido, a menudo a mitad de
una toma o de una actuación, de noche y junto a un lienzo oscuro.

## Superficies
| Superficie | Modo | Éxito |
|---|---|---|
| Panel de control (los 3 hosts) | **Operate** | ajustar el enjambre sin perder de vista el lienzo; lo frecuente (transporte, velocidad, grabar) siempre a mano |

## Principios
- El lienzo es el protagonista: el panel acompaña y no compite (cristal oscuro, un único acento).
- Lo frecuente arriba y fijo; lo demás, agrupado por tarea en pestañas y tarjetas.
- Cada control conserva su atajo de teclado y lo muestra.
- Nada del panel entra en el vídeo grabado.

## Supuestos (deducidos, no confirmados)
- El uso principal es `sim-gpu` en un monitor grande; `sim` y la ventana aparte son secundarios.
- Interfaz solo en español y solo en tema oscuro.
- No hay usuarios externos ni requisitos de accesibilidad formales más allá de un buen contraste.
