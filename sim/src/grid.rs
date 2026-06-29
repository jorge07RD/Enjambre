use crate::simulation::Particle;
use macroquad::prelude::Vec2;

/// Hash espacial uniforme en formato CSR (counting sort plano).
///
/// En lugar de un `Vec<Vec<u32>>` (mala localidad de caché y allocaciones por
/// celda), guardamos:
/// - `items`: todos los índices de partícula ordenados por celda, contiguos.
/// - `cell_start`: offset donde empieza cada celda dentro de `items`.
///
/// Así, las partículas de una celda son una rebanada contigua `items[s..e]`,
/// ideal para recorrer vecinas y para acceso paralelo de solo lectura.
pub struct Grid {
    cell: f32,
    inv_cell: f32,
    cols: i32,
    rows: i32,
    cell_start: Vec<u32>, // len = cols*rows + 1
    items: Vec<u32>,      // len = n
    counts: Vec<u32>,     // scratch (conteo / cursor), reutilizado entre frames
}

impl Grid {
    pub fn new() -> Self {
        Self {
            cell: 1.0,
            inv_cell: 1.0,
            cols: 1,
            rows: 1,
            cell_start: vec![0, 0],
            items: Vec::new(),
            counts: vec![0],
        }
    }

    pub fn rebuild(&mut self, particles: &[Particle], world: Vec2, cell_size: f32) {
        self.cell = cell_size.max(1.0);
        self.inv_cell = 1.0 / self.cell;
        self.cols = ((world.x * self.inv_cell).ceil() as i32).max(1);
        self.rows = ((world.y * self.inv_cell).ceil() as i32).max(1);
        let ncells = (self.cols * self.rows) as usize;
        let n = particles.len();

        // 1) Contar partículas por celda.
        self.counts.clear();
        self.counts.resize(ncells, 0);
        for p in particles {
            let c = self.cell_index_pos(p.pos);
            self.counts[c] += 1;
        }

        // 2) Prefijo acumulado -> offsets de inicio de cada celda.
        self.cell_start.clear();
        self.cell_start.resize(ncells + 1, 0);
        let mut acc = 0u32;
        for c in 0..ncells {
            self.cell_start[c] = acc;
            acc += self.counts[c];
        }
        self.cell_start[ncells] = acc;

        // 3) Colocar los índices usando `counts` como cursor móvil.
        self.items.resize(n, 0);
        self.counts[..ncells].copy_from_slice(&self.cell_start[..ncells]);
        for (i, p) in particles.iter().enumerate() {
            let c = self.cell_index_pos(p.pos);
            let dst = self.counts[c] as usize;
            self.items[dst] = i as u32;
            self.counts[c] += 1;
        }
    }

    #[inline]
    fn clampi(v: i32, max: i32) -> i32 {
        v.clamp(0, max - 1)
    }

    #[inline]
    pub fn cell_coord(&self, pos: Vec2) -> (i32, i32) {
        let cx = (pos.x * self.inv_cell) as i32;
        let cy = (pos.y * self.inv_cell) as i32;
        (Self::clampi(cx, self.cols), Self::clampi(cy, self.rows))
    }

    #[inline]
    fn cell_index_pos(&self, pos: Vec2) -> usize {
        let (cx, cy) = self.cell_coord(pos);
        (cy * self.cols + cx) as usize
    }

    #[inline]
    pub fn cols(&self) -> i32 {
        self.cols
    }

    #[inline]
    pub fn rows(&self) -> i32 {
        self.rows
    }

    /// Índices de partícula que caen en la celda (cx, cy).
    #[inline]
    pub fn cell_items(&self, cx: i32, cy: i32) -> &[u32] {
        let idx = (cy * self.cols + cx) as usize;
        let s = self.cell_start[idx] as usize;
        let e = self.cell_start[idx + 1] as usize;
        &self.items[s..e]
    }
}
