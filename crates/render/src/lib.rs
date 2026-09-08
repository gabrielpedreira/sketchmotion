//! sketchmotion-render — composição do documento em pixels.
//!
//! Etapa 4 (v0.1): lê o estado do `core` (fundo + camadas) e produz um buffer
//! RGBA que o app exibe como textura. É **somente leitura** sobre o `core`.
//!
//! A composição aqui é feita em CPU (alpha "over"), simples e robusta para a
//! v0.1. Quando entrarem formas vetoriais, blending avançado e onion skin
//! (v0.2+), migramos esta etapa para o skia — por isso ele já é dependência.

use sketchmotion_core::{Color, Document, Layer};

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
