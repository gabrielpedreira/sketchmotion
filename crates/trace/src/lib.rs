//! sketchmotion-trace — vetorização (raster → vetor).
//!
//! Etapa T1: modo BILEVEL (limiar / silhueta), o caminho mais simples e útil
//! (logos, ícones, line art de alto contraste). Pipeline em etapas trocáveis:
//!
//! máscara binária → componentes conexos (8-conex) → contorno (Moore) →
//! simplificação (Ramer-Douglas-Peucker) → regiões poligonais.
//!
//! Devolve dados simples (pontos + cor); quem monta os `VectorObject` reais é o
//! app, reusando o sistema de vetores/nós/seleção/undo já existente. Assim, dá
//! para trocar/adicionar algoritmos (quantização de cor, bordas, Bézier) sem
//! reescrever o resto.
//!
//! As funções de traçado recebem um `progress: &AtomicU32` (0..=1000) que elas
//! atualizam durante a varredura — para o app mostrar uma barra de progresso
//! enquanto vetoriza em segundo plano.

use std::sync::atomic::{AtomicU32, Ordering};

/// Parâmetros do traçado bilevel.
#[derive(Debug, Clone, Copy)]
pub struct BilevelParams {
    /// Limiar de luminância (0–255): pixels com luminância <= limiar são "tinta".
    pub threshold: u8,
    /// Inverte primeiro plano/fundo.
    pub invert: bool,
    /// Só alfa: qualquer pixel opaco é primeiro plano (silhueta); ignora luminância.
    pub alpha_only: bool,
    /// Epsilon do RDP (em pixels da imagem de origem). 0 = fiel (mais nós).
    pub simplify: f32,
    /// Área mínima (px) para manter uma região (remove ruído/sujeira).
    pub min_area: usize,
}

impl Default for BilevelParams {
    fn default() -> Self {
        Self {
            threshold: 128,
            invert: false,
            alpha_only: false,
            simplify: 1.5,
            min_area: 24,
        }
    }
}

/// Região traçada: polígono fechado (coords na imagem de origem) + cor média.
#[derive(Debug, Clone)]
pub struct TracedRegion {
    pub points: Vec<(f32, f32)>,
    pub color: [u8; 4],
}

#[inline]
fn luminance(r: u8, g: u8, b: u8) -> u16 {
    ((r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000) as u16
}

/// Traça uma imagem RGBA em regiões poligonais (modo bilevel).
pub fn trace_bilevel(
    rgba: &[u8],
    w: u32,
    h: u32,
    p: &BilevelParams,
    progress: &AtomicU32,
) -> Vec<TracedRegion> {
    let (wi, hi) = (w as i32, h as i32);
    let n = (w as usize) * (h as usize);
    if n == 0 || rgba.len() < n * 4 {
        progress.store(1000, Ordering::Relaxed);
        return Vec::new();
    }

    // 1) Máscara binária (primeiro plano = "tinta").
    let mut fg = vec![false; n];
    for i in 0..n {
        let a = rgba[i * 4 + 3];
        let f = if p.alpha_only {
            a >= 128
        } else {
            a >= 128 && (luminance(rgba[i * 4], rgba[i * 4 + 1], rgba[i * 4 + 2]) as u8) <= p.threshold
        };
        fg[i] = if p.invert { !f } else { f };
    }

    // 2) Componentes conexos (8-conex) via flood fill, com cor média por região.
    let mut label = vec![0i32; n];
    let mut regions: Vec<TracedRegion> = Vec::new();
    let mut cur = 0i32;
    let mut stack: Vec<(i32, i32)> = Vec::new();
    for sy in 0..hi {
        progress.store((sy as u32 * 1000 / hi.max(1) as u32).min(999), Ordering::Relaxed);
        for sx in 0..wi {
            let si = (sy * wi + sx) as usize;
            if !fg[si] || label[si] != 0 {
                continue;
            }
            cur += 1;
            stack.clear();
            stack.push((sx, sy));
            label[si] = cur;
            let (mut area, mut sr, mut sg, mut sb, mut sa) = (0usize, 0u64, 0u64, 0u64, 0u64);
            while let Some((x, y)) = stack.pop() {
                let i = (y * wi + x) as usize;
                area += 1;
                sr += rgba[i * 4] as u64;
                sg += rgba[i * 4 + 1] as u64;
                sb += rgba[i * 4 + 2] as u64;
                sa += rgba[i * 4 + 3] as u64;
                const NB: [(i32, i32); 8] = [
                    (-1, 0),
                    (1, 0),
                    (0, -1),
                    (0, 1),
                    (-1, -1),
                    (-1, 1),
                    (1, -1),
                    (1, 1),
                ];
                for (dx, dy) in NB {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx >= 0 && ny >= 0 && nx < wi && ny < hi {
                        let ni = (ny * wi + nx) as usize;
                        if fg[ni] && label[ni] == 0 {
                            label[ni] = cur;
                            stack.push((nx, ny));
                        }
                    }
                }
            }
            if area < p.min_area {
                continue;
            }
            let a64 = area as u64;
            let color = [
                (sr / a64) as u8,
                (sg / a64) as u8,
                (sb / a64) as u8,
                (sa / a64) as u8,
            ];
            // 3) Contorno externo (Moore) começando no topo-esquerda do componente.
            let contour = moore_contour(&label, wi, hi, cur, sx, sy);
            if contour.len() < 3 {
                continue;
            }
            // 4) Simplificação (RDP) — controla o número de nós.
            let pts: Vec<(f32, f32)> = contour
                .iter()
                .map(|&(x, y)| (x as f32 + 0.5, y as f32 + 0.5))
                .collect();
            let simp = if p.simplify > 0.0 {
                rdp(&pts, p.simplify)
            } else {
                pts
            };
            if simp.len() >= 3 {
                regions.push(TracedRegion {
                    points: simp,
                    color,
                });
            }
        }
    }
    progress.store(1000, Ordering::Relaxed);
    regions
}

/// Traçado de contorno de Moore-Neighbor (varredura radial) para o componente
/// `this`, começando em (sx, sy) (topo-esquerda garantido pela ordem de scan).
fn moore_contour(label: &[i32], w: i32, h: i32, this: i32, sx: i32, sy: i32) -> Vec<(i32, i32)> {
    // Vizinhança 8 em ordem horária: 0:O,1:NO,2:N,3:NE,4:L,5:SE,6:S,7:SO.
    const OFF: [(i32, i32); 8] = [
        (-1, 0),
        (-1, -1),
        (0, -1),
        (1, -1),
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
    ];
    let at = |x: i32, y: i32| -> bool {
        x >= 0 && y >= 0 && x < w && y < h && label[(y * w + x) as usize] == this
    };
    let start = (sx, sy);
    let mut contour = vec![start];
    let mut b = start;
    // Chegamos ao início vindo do Oeste (o pixel à esquerda é fundo).
    let mut from = 0usize;
    let max_iter = (w as usize) * (h as usize) * 4 + 32;
    for _ in 0..max_iter {
        let mut found: Option<(usize, (i32, i32))> = None;
        for k in 1..=8 {
            let dir = (from + k) % 8;
            let (dx, dy) = OFF[dir];
            let (nx, ny) = (b.0 + dx, b.1 + dy);
            if at(nx, ny) {
                found = Some((dir, (nx, ny)));
                break;
            }
        }
        let (dir, nb) = match found {
            Some(v) => v,
            None => break, // pixel isolado
        };
        from = (dir + 4) % 8; // backtrack = de onde viemos
        b = nb;
        if b == start {
            break;
        }
        contour.push(b);
    }
    contour
}

// ======================= Quantização de cor (T2) =======================

/// Parâmetros do traçado colorido (várias regiões por cor).
#[derive(Debug, Clone, Copy)]
pub struct QuantParams {
    /// Número de cores/regiões da paleta.
    pub colors: usize,
    /// Converte para tons de cinza antes de quantizar.
    pub grayscale: bool,
    /// Epsilon do RDP.
    pub simplify: f32,
    /// Área mínima (px) para manter uma região.
    pub min_area: usize,
    /// Remove o fundo (a cor de paleta mais presente na borda vira transparência).
    pub remove_bg: bool,
}

impl Default for QuantParams {
    fn default() -> Self {
        Self {
            colors: 6,
            grayscale: false,
            simplify: 1.2,
            min_area: 20,
            remove_bg: true,
        }
    }
}

fn color_avg(bucket: &[[u8; 3]]) -> [u8; 3] {
    if bucket.is_empty() {
        return [0, 0, 0];
    }
    let (mut r, mut g, mut b) = (0u64, 0u64, 0u64);
    for c in bucket {
        r += c[0] as u64;
        g += c[1] as u64;
        b += c[2] as u64;
    }
    let n = bucket.len() as u64;
    [(r / n) as u8, (g / n) as u8, (b / n) as u8]
}

/// Median-cut: reduz amostras de cor a uma paleta de até `k` cores.
fn median_cut(mut samples: Vec<[u8; 3]>, k: usize) -> Vec<[u8; 3]> {
    if samples.is_empty() || k == 0 {
        return vec![[0, 0, 0]];
    }
    let mut buckets: Vec<Vec<[u8; 3]>> = vec![std::mem::take(&mut samples)];
    while buckets.len() < k {
        // Escolhe o bucket com maior amplitude num canal.
        let mut best_i = None;
        let mut best_range = 0i32;
        let mut best_axis = 0usize;
        for (i, b) in buckets.iter().enumerate() {
            if b.len() < 2 {
                continue;
            }
            for axis in 0..3 {
                let (mut lo, mut hi) = (255i32, 0i32);
                for c in b {
                    let v = c[axis] as i32;
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
                let r = hi - lo;
                if r > best_range {
                    best_range = r;
                    best_i = Some(i);
                    best_axis = axis;
                }
            }
        }
        let Some(bi) = best_i else {
            break; // nada mais para dividir
        };
        buckets[bi].sort_by_key(|c| c[best_axis]);
        let mid = buckets[bi].len() / 2;
        let tail = buckets[bi].split_off(mid);
        buckets.push(tail);
    }
    buckets.iter().map(|b| color_avg(b)).collect()
}

#[inline]
fn nearest(palette: &[[u8; 3]], c: [u8; 3]) -> usize {
    let mut best = 0usize;
    let mut bestd = i32::MAX;
    for (i, p) in palette.iter().enumerate() {
        let dr = c[0] as i32 - p[0] as i32;
        let dg = c[1] as i32 - p[1] as i32;
        let db = c[2] as i32 - p[2] as i32;
        let d = dr * dr + dg * dg + db * db;
        if d < bestd {
            bestd = d;
            best = i;
        }
    }
    best
}

fn poly_area(pts: &[(f32, f32)]) -> f32 {
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    let mut a = 0.0f32;
    for i in 0..n {
        let (x1, y1) = pts[i];
        let (x2, y2) = pts[(i + 1) % n];
        a += x1 * y2 - x2 * y1;
    }
    (a * 0.5).abs()
}

/// Traça uma imagem RGBA em regiões coloridas (quantização de cor).
pub fn trace_quantized(
    rgba: &[u8],
    w: u32,
    h: u32,
    p: &QuantParams,
    progress: &AtomicU32,
) -> Vec<TracedRegion> {
    let (wi, hi) = (w as i32, h as i32);
    let n = (w as usize) * (h as usize);
    if n == 0 || rgba.len() < n * 4 {
        progress.store(1000, Ordering::Relaxed);
        return Vec::new();
    }
    let colr = |i: usize| -> [u8; 3] {
        if p.grayscale {
            let l = luminance(rgba[i * 4], rgba[i * 4 + 1], rgba[i * 4 + 2]) as u8;
            [l, l, l]
        } else {
            [rgba[i * 4], rgba[i * 4 + 1], rgba[i * 4 + 2]]
        }
    };

    // 1) Amostras de cor (pixels opacos), subamostradas para agilidade.
    let step = ((n / 20000).max(1)) as usize;
    let mut samples: Vec<[u8; 3]> = Vec::new();
    let mut i = 0;
    while i < n {
        if rgba[i * 4 + 3] >= 128 {
            samples.push(colr(i));
        }
        i += step;
    }
    let palette = median_cut(samples, p.colors.max(1));
    if palette.is_empty() {
        return Vec::new();
    }

    // 2) Índice de paleta por pixel (-1 = transparente/pular).
    let mut idx = vec![-1i32; n];
    for i in 0..n {
        if rgba[i * 4 + 3] >= 128 {
            idx[i] = nearest(&palette, colr(i)) as i32;
        }
    }

    // 3) Cor de fundo = índice DOMINANTE na borda — mas só remove se ele
    //    realmente domina (evita apagar uma cor de conteúdo que só encosta na
    //    borda). Se a borda é majoritariamente transparente, não remove nada.
    if p.remove_bg {
        let mut counts = vec![0usize; palette.len()];
        let mut transp = 0usize;
        let mut total = 0usize;
        let push = |x: i32, y: i32, counts: &mut Vec<usize>, transp: &mut usize, total: &mut usize| {
            let k = (y * wi + x) as usize;
            *total += 1;
            if idx[k] >= 0 {
                counts[idx[k] as usize] += 1;
            } else {
                *transp += 1;
            }
        };
        for x in 0..wi {
            push(x, 0, &mut counts, &mut transp, &mut total);
            push(x, hi - 1, &mut counts, &mut transp, &mut total);
        }
        for y in 0..hi {
            push(0, y, &mut counts, &mut transp, &mut total);
            push(wi - 1, y, &mut counts, &mut transp, &mut total);
        }
        if let Some((bg, &c)) = counts.iter().enumerate().max_by_key(|(_, &c)| c) {
            // Só considera fundo se cobre >45% da borda e supera a transparência.
            let thresh = (total as f32 * 0.45) as usize;
            if c > 0 && c >= thresh && c >= transp {
                for v in idx.iter_mut() {
                    if *v == bg as i32 {
                        *v = -1;
                    }
                }
            }
        }
    }

    // 4) Componentes conexos por índice (8-conex) → contorno → RDP.
    let mut label = vec![0i32; n];
    let mut cur = 0i32;
    let mut stack: Vec<(i32, i32)> = Vec::new();
    let mut out: Vec<(f32, TracedRegion)> = Vec::new();
    for sy in 0..hi {
        progress.store((sy as u32 * 1000 / hi.max(1) as u32).min(999), Ordering::Relaxed);
        for sx in 0..wi {
            let si = (sy * wi + sx) as usize;
            if idx[si] < 0 || label[si] != 0 {
                continue;
            }
            let this_idx = idx[si];
            cur += 1;
            stack.clear();
            stack.push((sx, sy));
            label[si] = cur;
            let mut area = 0usize;
            while let Some((x, y)) = stack.pop() {
                area += 1;
                const NB: [(i32, i32); 8] = [
                    (-1, 0),
                    (1, 0),
                    (0, -1),
                    (0, 1),
                    (-1, -1),
                    (-1, 1),
                    (1, -1),
                    (1, 1),
                ];
                for (dx, dy) in NB {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx >= 0 && ny >= 0 && nx < wi && ny < hi {
                        let ni = (ny * wi + nx) as usize;
                        if idx[ni] == this_idx && label[ni] == 0 {
                            label[ni] = cur;
                            stack.push((nx, ny));
                        }
                    }
                }
            }
            if area < p.min_area {
                continue;
            }
            let contour = moore_contour(&label, wi, hi, cur, sx, sy);
            if contour.len() < 3 {
                continue;
            }
            let pts: Vec<(f32, f32)> = contour
                .iter()
                .map(|&(x, y)| (x as f32 + 0.5, y as f32 + 0.5))
                .collect();
            let simp = if p.simplify > 0.0 {
                rdp(&pts, p.simplify)
            } else {
                pts
            };
            if simp.len() < 3 {
                continue;
            }
            let pc = palette[this_idx as usize];
            let a = poly_area(&simp);
            out.push((
                a,
                TracedRegion {
                    points: simp,
                    color: [pc[0], pc[1], pc[2], 255],
                },
            ));
        }
    }
    // Maiores atrás, menores na frente (empilhamento visual correto).
    out.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    progress.store(1000, Ordering::Relaxed);
    out.into_iter().map(|(_, r)| r).collect()
}

/// Simplificação Ramer-Douglas-Peucker (iterativa) de uma polilinha.
pub fn rdp(points: &[(f32, f32)], eps: f32) -> Vec<(f32, f32)> {
    let n = points.len();
    if n < 3 {
        return points.to_vec();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (ax, ay) = points[a];
        let (bx, by) = points[b];
        let (dx, dy) = (bx - ax, by - ay);
        let len = (dx * dx + dy * dy).sqrt().max(1e-6);
        let mut dmax = 0.0f32;
        let mut idx = a;
        for i in (a + 1)..b {
            let (px, py) = points[i];
            let d = (dy * px - dx * py + bx * ay - by * ax).abs() / len;
            if d > dmax {
                dmax = d;
                idx = i;
            }
        }
        if dmax > eps {
            keep[idx] = true;
            stack.push((a, idx));
            stack.push((idx, b));
        }
    }
    points
        .iter()
        .enumerate()
        .filter(|(i, _)| keep[*i])
        .map(|(_, p)| *p)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quadrado_preto_vira_uma_regiao() {
        // 8x8 branco com um quadrado preto 4x4 no centro.
        let (w, h) = (8u32, 8u32);
        let mut rgba = vec![255u8; (w * h * 4) as usize];
        for y in 2..6 {
            for x in 2..6 {
                let i = ((y * w + x) * 4) as usize;
                rgba[i] = 0;
                rgba[i + 1] = 0;
                rgba[i + 2] = 0;
            }
        }
        let p = BilevelParams {
            threshold: 128,
            invert: false,
            alpha_only: false,
            simplify: 0.0,
            min_area: 1,
        };
        let regs = trace_bilevel(&rgba, w, h, &p, &AtomicU32::new(0));
        assert_eq!(regs.len(), 1);
        assert!(regs[0].points.len() >= 4);
    }
}
