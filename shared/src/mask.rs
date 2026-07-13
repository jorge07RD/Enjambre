//! Utilidades sobre rejillas de ocupación booleanas (usadas por el mosaico del
//! efecto foto/vídeo en `sim` y `sim-gpu`).

/// Marca como "ocupadas" también las celdas transparentes que NO son
/// alcanzables desde el borde de la rejilla (encerradas por celdas opacas por
/// todos lados): sin esto, una forma cerrada en el vídeo (p. ej. un anillo o
/// un círculo) deja un hueco transparente en su interior donde el enjambre
/// puede quedar atrapado (repelido por el anillo desde todas direcciones,
/// sin salida). Tratando ese interior como "ocupado" para la evitación, el
/// enjambre nunca lo considera hueco: queda fuera de toda la forma, no solo
/// de su contorno.
///
/// `occ` es la rejilla `cols`×`rows` (row-major, `occ[y*cols+x]`) de
/// opaco/transparente; se muta in-place. Flood-fill BFS desde el borde:
/// O(cols·rows), barato para el tamaño de rejilla del mosaico (unos miles de
/// celdas como mucho).
pub fn fill_enclosed(occ: &mut [bool], cols: usize, rows: usize) {
    if cols == 0 || rows == 0 || occ.len() != cols * rows {
        return;
    }
    let mut reachable = vec![false; cols * rows];
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let seed = |x: usize, y: usize, reachable: &mut [bool], stack: &mut Vec<(usize, usize)>| {
        let idx = y * cols + x;
        if !occ[idx] && !reachable[idx] {
            reachable[idx] = true;
            stack.push((x, y));
        }
    };
    for x in 0..cols {
        seed(x, 0, &mut reachable, &mut stack);
        seed(x, rows - 1, &mut reachable, &mut stack);
    }
    for y in 0..rows {
        seed(0, y, &mut reachable, &mut stack);
        seed(cols - 1, y, &mut reachable, &mut stack);
    }
    while let Some((x, y)) = stack.pop() {
        let visit = |nx: usize, ny: usize, reachable: &mut [bool], stack: &mut Vec<(usize, usize)>| {
            let idx = ny * cols + nx;
            if !occ[idx] && !reachable[idx] {
                reachable[idx] = true;
                stack.push((nx, ny));
            }
        };
        if x > 0 {
            visit(x - 1, y, &mut reachable, &mut stack);
        }
        if x + 1 < cols {
            visit(x + 1, y, &mut reachable, &mut stack);
        }
        if y > 0 {
            visit(x, y - 1, &mut reachable, &mut stack);
        }
        if y + 1 < rows {
            visit(x, y + 1, &mut reachable, &mut stack);
        }
    }
    for i in 0..occ.len() {
        if !occ[i] && !reachable[i] {
            occ[i] = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rellena_un_hueco_encerrado() {
        // Rejilla 5x5: un anillo opaco de borde a borde en el perímetro
        // interior deja el centro encerrado.
        let cols = 5;
        let rows = 5;
        let mut occ = vec![false; cols * rows];
        // Anillo: todas las celdas del borde de un cuadrado 3x3 centrado.
        for &(x, y) in &[
            (1, 1), (2, 1), (3, 1),
            (1, 2), (3, 2),
            (1, 3), (2, 3), (3, 3),
        ] {
            occ[y * cols + x] = true;
        }
        assert!(!occ[2 * cols + 2], "el centro debe partir transparente");
        fill_enclosed(&mut occ, cols, rows);
        assert!(occ[2 * cols + 2], "el centro encerrado debe quedar ocupado");
        // Las esquinas (conectadas al borde) siguen libres.
        assert!(!occ[0]);
        assert!(!occ[cols * rows - 1]);
    }

    #[test]
    fn no_toca_transparente_conectado_al_borde() {
        let cols = 4;
        let rows = 4;
        let occ_before = vec![false; cols * rows];
        let mut occ = occ_before.clone();
        fill_enclosed(&mut occ, cols, rows);
        assert_eq!(occ, occ_before, "sin nada opaco, todo sigue transparente");
    }
}
