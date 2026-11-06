//! Conversão de um trabalho em alta resolução para PIXEL ART.
//!
//! Implementação própria, inspirada nas ideias de ferramentas como PixelOE
//! (redução "sensível a contraste" que preserva contornos) e pyxelate (paleta
//! reduzida). Três etapas:
//!
//! 1. **Redução por bloco** (`downscale`): cada pixel do resultado olha o
//!    bloco correspondente da arte original. O bloco só vira pixel opaco se o
//!    conteúdo ocupar pelo menos a área de um TRAÇO atravessando o bloco (assim
//!    linhas finas sobrevivem e o "pó" de anti-aliasing some). Se houver um
//!    grupo de pixels bem mais escuro que a mediana (um contorno) com espessura
//!    suficiente, ele vence; senão usa a média do miolo (quartis), que dá uma
//!    cor estável sem ruído de textura.
//! 2. **Paleta única** (`build_palette`): median cut + refinamento k-means sobre
//!    as cores de TODOS os frames/camadas — a animação inteira fica com as
//!    mesmas cores (nada "pisca" entre frames).
//! 3. **Mapeamento** (`apply_palette`): cada pixel vai para a cor mais próxima.
//!
//! Sem pontilhado (dithering) de propósito: em animação ele "ferve" de um frame
//! para o outro.

use crate::document::Document;
use crate::image_object::ImageObject;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

/// Parâmetros da conversão.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PixelArtParams {
    /// Lado MAIOR do resultado, em pixels (32, 64, 128…). O outro lado segue a
    /// proporção do papel.
    pub long_side: u32,
    /// Máximo de cores da paleta (0 = sem limite).
    pub colors: u32,
    /// 0..1 — quanto preservar traços finos (1 = até linhas de 1 px da arte
    /// original sobrevivem; 0 = só formas grossas).
    pub detail: f32,
}

impl Default for PixelArtParams {
    fn default() -> Self {
        Self { long_side: 64, colors: 32, detail: 0.7 }
    }
}

/// Tamanho final mantendo a proporção: o lado maior vira `long`.
pub fn target_size(w: u32, h: u32, long: u32) -> (u32, u32) {
    let long = long.max(1);
    let (w, h) = (w.max(1), h.max(1));
    if w >= h {
        let th = ((h as f32) * long as f32 / w as f32).round().max(1.0) as u32;
        (long, th)
    } else {
        let tw = ((w as f32) * long as f32 / h as f32).round().max(1.0) as u32;
        (tw, long)
    }
}

#[inline]
fn lum(c: &[u8; 4]) -> f32 {
    (0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32) / 255.0
}

/// Limites (em px da origem) do bloco `t` de `tn` blocos ao longo de `sn` px.
#[inline]
fn faixa(t: u32, tn: u32, sn: u32) -> (u32, u32) {
    let a = ((t as u64 * sn as u64) / tn as u64) as u32;
    let b = ((((t + 1) as u64 * sn as u64) / tn as u64) as u32).max(a + 1).min(sn);
    (a, b)
}

/// Reduz um buffer RGBA `sw×sh` para `tw×th` preservando traços e cores.
///
/// Passo 1: luminância média de cada bloco (mapa grosso da imagem).
/// Passo 2: em cada bloco, separa os pixels em grupo ESCURO e grupo CLARO
/// (relativos à mediana). Um grupo só conta como "traço" se tiver a área de um
/// traço atravessando o bloco. Entre os candidatos, vence o que MAIS se
/// diferencia da vizinhança (5×5 blocos) — assim contorno escuro sobre fundo
/// claro e traço claro sobre fundo escuro sobrevivem igualmente. Sem traço, a
/// cor é a mediana do bloco. A cor é sempre um pixel REAL da arte (nunca média),
/// para não criar cores "misturadas" nas divisas.
pub fn downscale(src: &[u8], sw: u32, sh: u32, tw: u32, th: u32, detail: f32) -> Vec<u8> {
    let mut out = vec![0u8; (tw as usize) * (th as usize) * 4];
    if sw == 0 || sh == 0 || tw == 0 || th == 0 || src.len() < (sw as usize) * (sh as usize) * 4 {
        return out;
    }
    let detail = detail.clamp(0.0, 1.0);
    // Espessura mínima (em px da arte ORIGINAL) de um traço que deve sobreviver.
    let largura_traco = 4.0 - 3.0 * detail; // 4 px … 1 px
    // Diferença de luminância que caracteriza um traço/contorno.
    let contraste = 0.10_f32;
    let (twu, thu) = (tw as usize, th as usize);

    // --- Passo 1: luminância média por bloco (só pixels sólidos) ---
    let mut lum_bloco = vec![f32::NAN; twu * thu];
    for ty in 0..th {
        let (y0, y1) = faixa(ty, th, sh);
        for tx in 0..tw {
            let (x0, x1) = faixa(tx, tw, sw);
            let (mut s, mut n) = (0.0f32, 0.0f32);
            for y in y0..y1 {
                let linha = (y as usize) * (sw as usize) * 4;
                for x in x0..x1 {
                    let i = linha + (x as usize) * 4;
                    if src[i + 3] >= 128 {
                        s += lum(&[src[i], src[i + 1], src[i + 2], 255]);
                        n += 1.0;
                    }
                }
            }
            if n > 0.0 {
                lum_bloco[ty as usize * twu + tx as usize] = s / n;
            }
        }
    }

    // --- Passo 2: escolha do pixel de cada bloco ---
    let mut amostras: Vec<(f32, [u8; 4])> = Vec::new();
    for ty in 0..th {
        let (y0, y1) = faixa(ty, th, sh);
        for tx in 0..tw {
            let (x0, x1) = faixa(tx, tw, sw);
            let n = ((x1 - x0) * (y1 - y0)) as f32;
            amostras.clear();
            let mut area = 0.0f32; // área coberta, ponderada pelo alfa
            for y in y0..y1 {
                let linha = (y as usize) * (sw as usize) * 4;
                for x in x0..x1 {
                    let i = linha + (x as usize) * 4;
                    let a = src[i + 3];
                    if a >= 24 {
                        let c = [src[i], src[i + 1], src[i + 2], a];
                        area += a as f32 / 255.0;
                        amostras.push((lum(&c), c));
                    }
                }
            }
            if amostras.is_empty() {
                continue;
            }
            // Área de um traço com a espessura mínima atravessando o bloco.
            let minimo = (largura_traco * n.sqrt()).min(n * 0.5).max(1.0);
            if area < minimo {
                continue; // vazio / só poeira de anti-aliasing
            }
            // Para a COR, só os pixels sólidos (o anti-aliasing semitransparente
            // das bordas viraria pixels cinzas). Arte toda translúcida fica como é.
            if amostras.iter().any(|(_, c)| c[3] >= 128) {
                amostras.retain(|(_, c)| c[3] >= 128);
            }
            amostras.sort_unstable_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let m = amostras.len();
            let med = amostras[m / 2].0;
            let exigido = minimo.min(m as f32 * 0.5).max(1.0);
            // Grupos (as amostras estão em ordem de luminância).
            let n_escuro = amostras.iter().take_while(|(l, _)| *l < med - contraste).count();
            let n_claro = amostras.iter().rev().take_while(|(l, _)| *l > med + contraste).count();
            // Luminância da vizinhança (5×5 blocos).
            let mut viz = (0.0f32, 0.0f32);
            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let (vx, vy) = (tx as i32 + dx, ty as i32 + dy);
                    if vx < 0 || vy < 0 || vx >= tw as i32 || vy >= th as i32 {
                        continue;
                    }
                    let l = lum_bloco[vy as usize * twu + vx as usize];
                    if !l.is_nan() {
                        viz.0 += l;
                        viz.1 += 1.0;
                    }
                }
            }
            let ref_lum = if viz.1 > 0.0 { viz.0 / viz.1 } else { med };
            // Um grupo só é "traço FINO" se nenhum bloco vizinho já for quase todo
            // daquela cor. Se for, estamos só na borda de uma forma GROSSA (que o
            // vizinho já representa) — deixar o grupo vencer aqui incharia e
            // serrilharia a borda.
            let ja_representado = |fl: f32| -> bool {
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let (vx, vy) = (tx as i32 + dx, ty as i32 + dy);
                        if vx < 0 || vy < 0 || vx >= tw as i32 || vy >= th as i32 {
                            continue;
                        }
                        let l = lum_bloco[vy as usize * twu + vx as usize];
                        if !l.is_nan() && (l - fl).abs() < 0.15 {
                            return true;
                        }
                    }
                }
                false
            };
            let mut escolha: Option<(f32, [u8; 4])> = None; // (destaque, cor)
            if n_escuro > 0 && n_escuro as f32 >= exigido {
                let c = amostras[n_escuro / 2];
                if !ja_representado(c.0) {
                    escolha = Some(((ref_lum - c.0).abs(), c.1));
                }
            }
            if n_claro > 0 && n_claro as f32 >= exigido {
                let c = amostras[m - 1 - n_claro / 2];
                let d = (c.0 - ref_lum).abs();
                if !ja_representado(c.0) && escolha.map_or(true, |(e, _)| d > e) {
                    escolha = Some((d, c.1));
                }
            }
            let c = match escolha {
                // O traço só vence se destoar da vizinhança mais que o miolo.
                Some((d, c)) if d > (med - ref_lum).abs() => c,
                _ => amostras[m / 2].1,
            };
            let o = ((ty as usize) * twu + tx as usize) * 4;
            out[o..o + 3].copy_from_slice(&c[..3]);
            // Pixel art é nítido: o bloco já passou no teste de área, então fica
            // opaco — a menos que a própria arte seja translúcida (vidro, brilho).
            out[o + 3] = if c[3] >= 128 { 255 } else { c[3] };
        }
    }
    out
}

/// Paleta de até `k` cores a partir de um histograma (cor → contagem):
/// median cut + 4 iterações de k-means ponderado.
pub fn build_palette(hist: &HashMap<[u8; 3], u32>, k: usize) -> Vec<[u8; 3]> {
    let mut cores: Vec<([u8; 3], u32)> = hist.iter().map(|(c, n)| (*c, *n)).collect();
    if k == 0 || cores.is_empty() {
        return Vec::new();
    }
    if cores.len() <= k {
        return cores.into_iter().map(|(c, _)| c).collect();
    }
    // --- median cut ---
    let mut caixas: Vec<Vec<([u8; 3], u32)>> = vec![std::mem::take(&mut cores)];
    while caixas.len() < k {
        // caixa com maior (amplitude × população) que ainda dá para dividir
        let mut melhor: Option<(usize, usize, u64)> = None;
        for (i, cx) in caixas.iter().enumerate() {
            if cx.len() < 2 {
                continue;
            }
            let mut lo = [255u8; 3];
            let mut hi = [0u8; 3];
            let mut pop = 0u64;
            for (c, n) in cx {
                for j in 0..3 {
                    lo[j] = lo[j].min(c[j]);
                    hi[j] = hi[j].max(c[j]);
                }
                pop += *n as u64;
            }
            let (canal, faixa) = (0..3).map(|j| (j, (hi[j] - lo[j]) as u64)).max_by_key(|x| x.1).unwrap();
            let peso = faixa * pop.max(1);
            if faixa > 0 && melhor.map_or(true, |m| peso > m.2) {
                melhor = Some((i, canal, peso));
            }
        }
        let Some((i, canal, _)) = melhor else { break };
        let mut cx = caixas.swap_remove(i);
        cx.sort_unstable_by_key(|(c, _)| c[canal]);
        let total: u64 = cx.iter().map(|x| x.1 as u64).sum();
        let mut acc = 0u64;
        let mut corte = cx.len() / 2;
        for (j, (_, n)) in cx.iter().enumerate() {
            acc += *n as u64;
            if acc * 2 >= total {
                corte = (j + 1).clamp(1, cx.len() - 1);
                break;
            }
        }
        let b = cx.split_off(corte);
        caixas.push(cx);
        caixas.push(b);
    }
    let mut pal: Vec<[f32; 3]> = caixas
        .iter()
        .map(|cx| {
            let mut s = [0f64; 3];
            let mut t = 0f64;
            for (c, n) in cx {
                for j in 0..3 {
                    s[j] += c[j] as f64 * *n as f64;
                }
                t += *n as f64;
            }
            [(s[0] / t) as f32, (s[1] / t) as f32, (s[2] / t) as f32]
        })
        .collect();
    // --- refinamento k-means ---
    let todas: Vec<([u8; 3], u32)> = caixas.into_iter().flatten().collect();
    for _ in 0..4 {
        let mut s = vec![[0f64; 4]; pal.len()];
        for (c, n) in &todas {
            let i = mais_proxima_f(&pal, c);
            for j in 0..3 {
                s[i][j] += c[j] as f64 * *n as f64;
            }
            s[i][3] += *n as f64;
        }
        for (i, acc) in s.iter().enumerate() {
            if acc[3] > 0.0 {
                pal[i] = [(acc[0] / acc[3]) as f32, (acc[1] / acc[3]) as f32, (acc[2] / acc[3]) as f32];
            }
        }
    }
    let mut out: Vec<[u8; 3]> = pal
        .iter()
        .map(|p| [p[0].round() as u8, p[1].round() as u8, p[2].round() as u8])
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

#[inline]
fn dist(a: [f32; 3], b: &[u8; 3]) -> f32 {
    // pesos perceptuais simples (verde pesa mais)
    let dr = a[0] - b[0] as f32;
    let dg = a[1] - b[1] as f32;
    let db = a[2] - b[2] as f32;
    2.0 * dr * dr + 4.0 * dg * dg + 3.0 * db * db
}

fn mais_proxima_f(pal: &[[f32; 3]], c: &[u8; 3]) -> usize {
    let mut best = 0;
    let mut bd = f32::INFINITY;
    for (i, p) in pal.iter().enumerate() {
        let d = dist(*p, c);
        if d < bd {
            bd = d;
            best = i;
        }
    }
    best
}

/// Leva cada pixel (não transparente) para a cor mais próxima da paleta.
pub fn apply_palette(rgba: &mut [u8], pal: &[[u8; 3]]) {
    if pal.is_empty() {
        return;
    }
    let palf: Vec<[f32; 3]> = pal.iter().map(|c| [c[0] as f32, c[1] as f32, c[2] as f32]).collect();
    let mut cache: HashMap<[u8; 3], [u8; 3]> = HashMap::new();
    for px in rgba.chunks_exact_mut(4) {
        if px[3] == 0 {
            continue;
        }
        let c = [px[0], px[1], px[2]];
        let n = *cache.entry(c).or_insert_with(|| pal[mais_proxima_f(&palf, &c)]);
        px[0] = n[0];
        px[1] = n[1];
        px[2] = n[2];
    }
}

/// Um buffer a converter (camada ou peça), deduplicado pelo ponteiro do Arc.
struct Trabalho {
    src: Arc<Vec<u8>>,
    sw: u32,
    sh: u32,
    tw: u32,
    th: u32,
}

type Chave = (usize, u32, u32);

fn chave(a: &Arc<Vec<u8>>, tw: u32, th: u32) -> Chave {
    (Arc::as_ptr(a) as usize, tw, th)
}

/// Tamanho de uma peça (imagem) no novo papel.
fn tam_peca(o: &ImageObject, sx: f32, sy: f32) -> (u32, u32) {
    let w = (o.hw * 2.0 * sx).round().max(1.0) as u32;
    let h = (o.hh * 2.0 * sy).round().max(1.0) as u32;
    (w, h)
}

impl Document {
    /// Converte o documento INTEIRO (todas as faixas, frames, camadas e peças)
    /// em pixel art no tamanho pedido, reescalando também vetores, câmera,
    /// pivôs, vetores de direção, rig e guias. Liga o modo pixel art.
    /// `progress` vai de 0 a 1000.
    pub fn convert_to_pixel_art(&mut self, p: PixelArtParams, progress: &AtomicU32) {
        progress.store(0, Ordering::Relaxed);
        self.sync_to_frames();
        let (sw, sh) = (self.width.max(1), self.height.max(1));
        let (tw, th) = target_size(sw, sh, p.long_side);
        let sx = tw as f32 / sw as f32;
        let sy = th as f32 / sh as f32;
        let s = (sx + sy) * 0.5;

        // 1) Lista de trabalhos únicos (camadas iguais entre frames compartilham
        //    o mesmo buffer: são processadas uma vez só).
        let mut fila: Vec<Trabalho> = Vec::new();
        let mut vistos: HashMap<Chave, usize> = HashMap::new();
        let mut add = |a: &Arc<Vec<u8>>, w: u32, h: u32, nw: u32, nh: u32, fila: &mut Vec<Trabalho>| {
            let k = chave(a, nw, nh);
            if !vistos.contains_key(&k) {
                vistos.insert(k, fila.len());
                fila.push(Trabalho { src: a.clone(), sw: w, sh: h, tw: nw, th: nh });
            }
        };
        for t in &self.tracks {
            for f in &t.frames {
                for l in &f.layers {
                    add(l.pixels_arc(), l.width(), l.height(), tw, th, &mut fila);
                }
                for o in &f.images {
                    let (nw, nh) = tam_peca(o, sx, sy);
                    add(&o.pixels, o.ow, o.oh, nw, nh, &mut fila);
                }
            }
        }
        for dv in &self.dir_vectors {
            if let Some(o) = &dv.base_image {
                let (nw, nh) = tam_peca(o, sx, sy);
                add(&o.pixels, o.ow, o.oh, nw, nh, &mut fila);
            }
        }

        // 2) Redução em paralelo (todos os núcleos do processador).
        let total = fila.len().max(1);
        let feitos = AtomicUsize::new(0);
        let proximo = AtomicUsize::new(0);
        let mut resultados: Vec<Vec<u8>> = vec![Vec::new(); fila.len()];
        {
            let nthreads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 16);
            let fila_ref = &fila;
            let saida: Vec<std::sync::Mutex<Option<Vec<u8>>>> =
                (0..fila.len()).map(|_| std::sync::Mutex::new(None)).collect();
            std::thread::scope(|sc| {
                for _ in 0..nthreads {
                    sc.spawn(|| loop {
                        let i = proximo.fetch_add(1, Ordering::Relaxed);
                        if i >= fila_ref.len() {
                            break;
                        }
                        let t = &fila_ref[i];
                        let r = downscale(&t.src, t.sw, t.sh, t.tw, t.th, p.detail);
                        *saida[i].lock().unwrap() = Some(r);
                        let d = feitos.fetch_add(1, Ordering::Relaxed) + 1;
                        progress.store((d * 800 / total) as u32, Ordering::Relaxed);
                    });
                }
            });
            for (i, m) in saida.into_iter().enumerate() {
                resultados[i] = m.into_inner().unwrap().unwrap_or_default();
            }
        }

        // 3) Paleta única para a animação inteira.
        if p.colors > 0 {
            let mut hist: HashMap<[u8; 3], u32> = HashMap::new();
            for r in &resultados {
                for px in r.chunks_exact(4) {
                    if px[3] > 0 {
                        *hist.entry([px[0], px[1], px[2]]).or_insert(0) += 1;
                    }
                }
            }
            let pal = build_palette(&hist, p.colors as usize);
            for r in resultados.iter_mut() {
                apply_palette(r, &pal);
            }
        }
        progress.store(900, Ordering::Relaxed);

        // 4) Devolve os buffers convertidos às camadas e peças.
        let novos: Vec<Arc<Vec<u8>>> = resultados.into_iter().map(Arc::new).collect();
        let pega = |a: &Arc<Vec<u8>>, nw: u32, nh: u32| -> Option<Arc<Vec<u8>>> {
            vistos.get(&chave(a, nw, nh)).map(|&i| novos[i].clone())
        };
        let conv_peca = |o: &mut ImageObject| {
            let (nw, nh) = tam_peca(o, sx, sy);
            if let Some(b) = pega(&o.pixels, nw, nh) {
                o.pixels = b;
                o.ow = nw;
                o.oh = nh;
            }
            o.cx *= sx;
            o.cy *= sy;
            o.hw *= sx;
            o.hh *= sy;
        };
        for t in self.tracks.iter_mut() {
            for f in t.frames.iter_mut() {
                for l in f.layers.iter_mut() {
                    if let Some(b) = pega(l.pixels_arc(), tw, th) {
                        l.set_buffer(tw, th, b);
                    }
                }
                for o in f.images.iter_mut() {
                    conv_peca(o);
                }
                for v in f.vectors.iter_mut() {
                    v.scale_around(0.0, 0.0, s);
                    v.stroke_width = (v.stroke_width * s).max(1.0);
                }
            }
        }

        // 5) Tudo que tem coordenada no papel acompanha a nova escala.
        for k in self.camera.keyframes.iter_mut() {
            k.x *= sx;
            k.y *= sy;
            k.w = (k.w * sx).max(1.0);
            k.h = (k.h * sy).max(1.0);
            k.anchor_x *= sx;
            k.anchor_y *= sy;
        }
        self.camera.base_w = tw as f32;
        self.camera.base_h = th as f32;
        for g in self.guides.iter_mut() {
            g.pos *= if g.is_horizontal() { sy } else { sx };
        }
        for pv in self.pivots.iter_mut() {
            pv.axis = (pv.axis.0 * sx, pv.axis.1 * sy);
            pv.mov = (pv.mov.0 * sx, pv.mov.1 * sy);
            pv.img_local_axis = (pv.img_local_axis.0 * s, pv.img_local_axis.1 * s);
            pv.img_local_mov = (pv.img_local_mov.0 * s, pv.img_local_mov.1 * s);
        }
        for dv in self.dir_vectors.iter_mut() {
            dv.start_centroid = (dv.start_centroid.0 * sx, dv.start_centroid.1 * sy);
            dv.end_centroid = (dv.end_centroid.0 * sx, dv.end_centroid.1 * sy);
            dv.pivot_axis = (dv.pivot_axis.0 * sx, dv.pivot_axis.1 * sy);
            for v in dv.base_vectors.iter_mut() {
                v.scale_around(0.0, 0.0, s);
                v.stroke_width = (v.stroke_width * s).max(1.0);
            }
            if let Some(o) = dv.base_image.as_mut() {
                conv_peca(o);
            }
        }
        for sk in self.skeletons.iter_mut() {
            for b in sk.bones.iter_mut() {
                b.x *= sx;
                b.y *= sy;
                b.length *= s;
                b.rest.x *= sx;
                b.rest.y *= sy;
            }
        }

        self.width = tw;
        self.height = th;
        self.pixel_art = true;
        self.load_active_track_mirror();
        progress.store(1000, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;

    #[test]
    fn tamanho_mantem_proporcao() {
        assert_eq!(target_size(1920, 1080, 64), (64, 36));
        assert_eq!(target_size(1080, 1920, 64), (36, 64));
        assert_eq!(target_size(500, 500, 32), (32, 32));
    }

    #[test]
    fn linha_fina_sobrevive_e_poeira_some() {
        // 120x120 transparente com uma linha preta vertical de 2 px em x=60.
        let (w, h) = (120u32, 120u32);
        let mut src = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 60..62 {
                let i = ((y * w + x) * 4) as usize;
                src[i..i + 4].copy_from_slice(&[0, 0, 0, 255]);
            }
        }
        // poeira: um pixel isolado em (5,5)
        let i = ((5 * w + 5) * 4) as usize;
        src[i..i + 4].copy_from_slice(&[255, 0, 0, 255]);
        let out = downscale(&src, w, h, 12, 12, 0.7);
        // coluna 6 (x 60..70) deve estar opaca e escura em todas as linhas
        for ty in 0..12 {
            let o = ((ty * 12 + 6) * 4) as usize;
            assert_eq!(out[o + 3], 255, "linha sumiu na linha {ty}");
            assert!(out[o] < 40);
        }
        assert_eq!(out[3], 0, "poeira deveria sumir");
    }

    #[test]
    fn contorno_vence_no_bloco() {
        // bloco 10x10 amarelo com contorno preto de 2 px no topo
        let (w, h) = (10u32, 10u32);
        let mut src = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let c = if y < 2 { [0, 0, 0, 255] } else { [250, 220, 0, 255] };
                src[i..i + 4].copy_from_slice(&c);
            }
        }
        let out = downscale(&src, w, h, 1, 1, 0.7);
        assert!(out[0] < 40, "o contorno deveria vencer: {:?}", &out[..4]);
        // sem contorno, fica a cor do miolo
        let mut liso = src.clone();
        for px in liso.chunks_exact_mut(4) {
            px.copy_from_slice(&[250, 220, 0, 255]);
        }
        let out2 = downscale(&liso, w, h, 1, 1, 0.7);
        assert_eq!(&out2[..4], &[250, 220, 0, 255]);
    }

    #[test]
    fn paleta_limita_cores() {
        let mut hist = HashMap::new();
        for r in 0..32u8 {
            for g in 0..8u8 {
                hist.insert([r * 8, g * 32, 100], 1 + r as u32);
            }
        }
        let pal = build_palette(&hist, 8);
        assert!(pal.len() <= 8 && pal.len() >= 4);
    }

    #[test]
    fn documento_inteiro_vira_pixel_art() {
        let mut d = Document::new(200, 100, Color::TRANSPARENT);
        for x in 0..200 {
            for y in 40..60 {
                d.layers[0].set_pixel(x, y, Color::rgba(200, 30, 30, 255));
            }
        }
        // uma peça 100x100 (verde) centrada em (100,50), exibida com 50x50
        let mut peca = ImageObject::new(vec![20, 200, 40, 255].repeat(100 * 100), 100, 100, 100.0, 50.0, 25.0, 25.0, 0.0, 1.0, 0);
        peca.id = 7;
        d.images.push(peca);
        d.sync_to_frames();
        d.add_frame();
        d.add_layer("Camada 2");
        let prog = AtomicU32::new(0);
        d.convert_to_pixel_art(PixelArtParams { long_side: 40, colors: 8, detail: 0.7 }, &prog);
        assert_eq!((d.width, d.height), (40, 20));
        assert!(d.pixel_art);
        assert_eq!(prog.load(Ordering::Relaxed), 1000);
        for t in &d.tracks {
            for f in &t.frames {
                for l in &f.layers {
                    assert_eq!((l.width(), l.height()), (40, 20));
                }
            }
        }
        // a faixa vermelha (y 40..60 → 8..12) continua lá no frame 1
        let c = d.tracks[0].frames[0].layers[0].get_pixel(20, 10).unwrap();
        assert_eq!(c.a, 255);
        assert!(c.r > 150 && c.g < 80);
        assert_eq!(d.camera.base_w, 40.0);
        // a peça encolheu junto (50 px exibidos → 10 px) e manteve o id
        let o = &d.tracks[0].frames[0].images[0];
        assert_eq!((o.ow, o.oh, o.id), (10, 10, 7));
        assert!((o.cx - 20.0).abs() < 0.01 && (o.hw - 5.0).abs() < 0.01);
        assert_eq!(&o.pixels[..4], &[20, 200, 40, 255]);
    }
}
