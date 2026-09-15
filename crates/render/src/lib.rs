//! sketchmotion-render — composição do documento em pixels.
//!
//! Etapa 4 (v0.1): lê o estado do `core` (fundo + camadas) e produz um buffer
//! RGBA que o app exibe como textura. É **somente leitura** sobre o `core`.
//!
//! A composição aqui é feita em CPU (alpha "over"), simples e robusta para a
//! v0.1. Quando entrarem formas vetoriais, blending avançado e onion skin
//! (v0.2+), migramos esta etapa para o skia — por isso ele já é dependência.

use sketchmotion_core::{Color, Document, ImageObject, Layer, VectorObject};

/// Imagem em pixels, pronta para virar textura no egui.
pub struct PixelImage {
    pub width: i32,
    pub height: i32,
    /// Pixels em RGBA8888 (não pré-multiplicado): 4 bytes por pixel.
    pub rgba: Vec<u8>,
}

/// Compõe o documento (frame atual) com fundo opaco.
pub fn render_document(doc: &Document) -> PixelImage {
    render_layers(doc.width, doc.height, doc.background, &doc.layers)
}

/// Compõe camadas sobre um fundo opaco `background`.
pub fn render_layers(width: u32, height: u32, background: Color, layers: &[Layer]) -> PixelImage {
    let (w, h) = (width as usize, height as usize);
    let mut rgba = vec![0u8; w * h * 4];
    for px in rgba.chunks_exact_mut(4) {
        px[0] = background.r;
        px[1] = background.g;
        px[2] = background.b;
        px[3] = 255;
    }
    for layer in layers {
        if !layer.visible {
            continue;
        }
        let op = layer.opacity();
        if op <= 0.0 {
            continue;
        }
        let src = layer.pixels();
        for (dst, s) in rgba.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            let sa = ((s[3] as f32) * op).round() as u32;
            if sa == 0 {
                continue;
            }
            let ia = 255 - sa;
            dst[0] = ((s[0] as u32 * sa + dst[0] as u32 * ia) / 255) as u8;
            dst[1] = ((s[1] as u32 * sa + dst[1] as u32 * ia) / 255) as u8;
            dst[2] = ((s[2] as u32 * sa + dst[2] as u32 * ia) / 255) as u8;
            dst[3] = 255;
        }
    }
    PixelImage {
        width: width as i32,
        height: height as i32,
        rgba,
    }
}

/// Compõe camadas sobre fundo TRANSPARENTE (para onion skin do frame anterior).
pub fn render_layers_alpha(width: u32, height: u32, layers: &[Layer]) -> PixelImage {
    let (w, h) = (width as usize, height as usize);
    let mut rgba = vec![0u8; w * h * 4];
    for layer in layers {
        if !layer.visible {
            continue;
        }
        let op = layer.opacity();
        if op <= 0.0 {
            continue;
        }
        let src = layer.pixels();
        for (dst, s) in rgba.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            let sa = (s[3] as f32 / 255.0) * op;
            if sa <= 0.0 {
                continue;
            }
            let da = dst[3] as f32 / 255.0;
            let oa = sa + da * (1.0 - sa);
            if oa <= 0.0 {
                continue;
            }
            for k in 0..3 {
                let sc = s[k] as f32 / 255.0;
                let dc = dst[k] as f32 / 255.0;
                dst[k] = (((sc * sa + dc * da * (1.0 - sa)) / oa) * 255.0).round() as u8;
            }
            dst[3] = (oa * 255.0).round() as u8;
        }
    }
    PixelImage {
        width: width as i32,
        height: height as i32,
        rgba,
    }
}

// ---------- Vetores (caneta/formas) rasterizados em pixels ----------

fn blend_px(rgba: &mut [u8], i: usize, c: Color, cover: f32) {
    let sa = (c.a as f32 / 255.0) * cover;
    if sa <= 0.0 {
        return;
    }
    let da = rgba[i + 3] as f32 / 255.0;
    let oa = sa + da * (1.0 - sa);
    if oa <= 0.0 {
        return;
    }
    let ch = [c.r, c.g, c.b];
    for k in 0..3 {
        let sc = ch[k] as f32 / 255.0;
        let dc = rgba[i + k] as f32 / 255.0;
        rgba[i + k] = (((sc * sa + dc * da * (1.0 - sa)) / oa) * 255.0).round() as u8;
    }
    rgba[i + 3] = (oa * 255.0).round() as u8;
}

fn stamp_disc(rgba: &mut [u8], w: i32, h: i32, cx: f32, cy: f32, r: f32, c: Color, op: f32) {
    let r2 = r * r;
    let x0 = ((cx - r).floor() as i32).max(0);
    let x1 = ((cx + r).ceil() as i32).min(w - 1);
    let y0 = ((cy - r).floor() as i32).max(0);
    let y1 = ((cy + r).ceil() as i32).min(h - 1);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            if dx * dx + dy * dy <= r2 {
                blend_px(rgba, ((y * w + x) * 4) as usize, c, op);
            }
        }
    }
}

fn densify(pts: &[(f32, f32)], spacing: f32) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    if pts.is_empty() {
        return out;
    }
    out.push(pts[0]);
    for wnd in pts.windows(2) {
        let (ax, ay) = wnd[0];
        let (bx, by) = wnd[1];
        let len = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
        let n = (len / spacing.max(0.5)).ceil().max(1.0) as usize;
        for k in 1..=n {
            let t = k as f32 / n as f32;
            out.push((ax + (bx - ax) * t, ay + (by - ay) * t));
        }
    }
    out
}

fn fill_polygon(rgba: &mut [u8], w: i32, h: i32, poly: &[(f32, f32)], c: Color, op: f32) {
    let n = poly.len();
    if n < 3 {
        return;
    }
    let (mut miny, mut maxy) = (f32::INFINITY, f32::NEG_INFINITY);
    for &(_, y) in poly {
        miny = miny.min(y);
        maxy = maxy.max(y);
    }
    let y0 = (miny.floor() as i32).max(0);
    let y1 = (maxy.ceil() as i32).min(h - 1);
    for y in y0..=y1 {
        let yf = y as f32 + 0.5;
        let mut xs: Vec<f32> = Vec::new();
        for i in 0..n {
            let (x1, y1e) = poly[i];
            let (x2, y2e) = poly[(i + 1) % n];
            if (y1e <= yf && y2e > yf) || (y2e <= yf && y1e > yf) {
                let t = (yf - y1e) / (y2e - y1e);
                xs.push(x1 + t * (x2 - x1));
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mut k = 0;
        while k + 1 < xs.len() {
            let xa = (xs[k].round() as i32).max(0);
            let xb = (xs[k + 1].round() as i32).min(w - 1);
            for x in xa..xb {
                blend_px(rgba, ((y * w + x) * 4) as usize, c, op);
            }
            k += 2;
        }
    }
}

/// Desenha os objetos vetoriais (preenchimento + traço) sobre o buffer RGBA.
pub fn rasterize_vectors(width: u32, height: u32, vectors: &[VectorObject], rgba: &mut [u8]) {
    let (w, h) = (width as i32, height as i32);
    for obj in vectors {
        let op = obj.opacity.clamp(0.0, 1.0);
        if op <= 0.0 {
            continue;
        }
        let flat = obj.flatten(24);
        if flat.len() < 2 {
            if let Some(&(x, y)) = flat.first() {
                let r = (obj.stroke_width * 0.5).max(0.6);
                stamp_disc(rgba, w, h, x, y, r, obj.stroke, op);
            }
            continue;
        }
        if let Some(fill) = obj.fill {
            fill_polygon(rgba, w, h, &flat, fill, op);
        }
        let r = (obj.stroke_width * 0.5).max(0.5);
        for &(x, y) in &densify(&flat, r.max(1.0)) {
            stamp_disc(rgba, w, h, x, y, r, obj.stroke, op);
        }
    }
}

// ---------- Objetos de imagem (raster colocado como elemento) ----------

/// Ponto local (relativo ao centro, desfeita a rotação) de um ponto de tela.
fn img_local(cx: f32, cy: f32, angle: f32, px: f32, py: f32) -> (f32, f32) {
    let (s, c) = (-angle).sin_cos();
    let (dx, dy) = (px - cx, py - cy);
    (dx * c - dy * s, dx * s + dy * c)
}

/// Um canto do retângulo do objeto (sx, sy ∈ {-1, 1}) no espaço do documento.
fn img_corner(cx: f32, cy: f32, hw: f32, hh: f32, angle: f32, sx: f32, sy: f32) -> (f32, f32) {
    let (s, c) = angle.sin_cos();
    let (lx, ly) = (sx * hw, sy * hh);
    (cx + lx * c - ly * s, cy + lx * s + ly * c)
}

/// Compõe os objetos de imagem (com transformação: centro/escala/rotação/opac.)
/// sobre o buffer RGBA já existente — mesma resolução do documento.
pub fn rasterize_images(width: u32, height: u32, images: &[ImageObject], rgba: &mut [u8]) {
    let (w, h) = (width as i32, height as i32);
    for obj in images {
        let op = obj.opacity.clamp(0.0, 1.0);
        if op <= 0.0 || obj.ow == 0 || obj.oh == 0 || obj.hw <= 0.0 || obj.hh <= 0.0 {
            continue;
        }
        if obj.pixels.len() < (obj.ow * obj.oh * 4) as usize {
            continue;
        }
        // Caixa envolvente (do retângulo girado) recortada ao documento.
        let corners = [
            img_corner(obj.cx, obj.cy, obj.hw, obj.hh, obj.angle, -1.0, -1.0),
            img_corner(obj.cx, obj.cy, obj.hw, obj.hh, obj.angle, 1.0, -1.0),
            img_corner(obj.cx, obj.cy, obj.hw, obj.hh, obj.angle, 1.0, 1.0),
            img_corner(obj.cx, obj.cy, obj.hw, obj.hh, obj.angle, -1.0, 1.0),
        ];
        let (mut minx, mut miny) = (f32::INFINITY, f32::INFINITY);
        let (mut maxx, mut maxy) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
        for (x, y) in corners {
            minx = minx.min(x);
            miny = miny.min(y);
            maxx = maxx.max(x);
            maxy = maxy.max(y);
        }
        let x0 = (minx.floor() as i32).max(0);
        let y0 = (miny.floor() as i32).max(0);
        let x1 = (maxx.ceil() as i32).min(w);
        let y1 = (maxy.ceil() as i32).min(h);
        for py in y0..y1 {
            for px in x0..x1 {
                let (lx, ly) = img_local(obj.cx, obj.cy, obj.angle, px as f32 + 0.5, py as f32 + 0.5);
                let u = lx / (2.0 * obj.hw) + 0.5;
                let v = ly / (2.0 * obj.hh) + 0.5;
                if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                    continue;
                }
                let sx = ((u * obj.ow as f32) as i32).clamp(0, obj.ow as i32 - 1);
                let sy = ((v * obj.oh as f32) as i32).clamp(0, obj.oh as i32 - 1);
                let si = ((sy * obj.ow as i32 + sx) * 4) as usize;
                let sa = obj.pixels[si + 3];
                if sa == 0 {
                    continue;
                }
                let c = Color::rgba(
                    obj.pixels[si],
                    obj.pixels[si + 1],
                    obj.pixels[si + 2],
                    sa,
                );
                blend_px(rgba, ((py * w + px) * 4) as usize, c, op);
            }
        }
    }
}

/// Frame completo (camadas + vetores + imagens) sobre fundo opaco.
pub fn render_frame(
    width: u32,
    height: u32,
    background: Color,
    layers: &[Layer],
    vectors: &[VectorObject],
    images: &[ImageObject],
) -> PixelImage {
    let mut img = render_layers(width, height, background, layers);
    rasterize_vectors(width, height, vectors, &mut img.rgba);
    rasterize_images(width, height, images, &mut img.rgba);
    img
}

/// Frame completo (camadas + vetores + imagens) sobre fundo TRANSPARENTE.
pub fn render_frame_alpha(
    width: u32,
    height: u32,
    layers: &[Layer],
    vectors: &[VectorObject],
    images: &[ImageObject],
) -> PixelImage {
    let mut img = render_layers_alpha(width, height, layers);
    rasterize_vectors(width, height, vectors, &mut img.rgba);
    rasterize_images(width, height, images, &mut img.rgba);
    img
}
