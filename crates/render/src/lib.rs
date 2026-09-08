//! sketchmotion-render — composição do documento em pixels.
//!
//! Etapa 4 (v0.1): lê o estado do `core` (fundo + camadas) e produz um buffer
//! RGBA que o app exibe como textura. É **somente leitura** sobre o `core`.
//!
//! A composição aqui é feita em CPU (alpha "over"), simples e robusta para a
//! v0.1. Quando entrarem formas vetoriais, blending avançado e onion skin
//! (v0.2+), migramos esta etapa para o skia — por isso ele já é dependência.

use sketchmotion_core::Document;

/// Imagem em pixels, pronta para virar textura no egui.
pub struct PixelImage {
    pub width: i32,
    pub height: i32,
    /// Pixels em RGBA8888 (não pré-multiplicado): 4 bytes por pixel.
    pub rgba: Vec<u8>,
}

/// Compõe o documento inteiro (fundo opaco + camadas visíveis) num buffer RGBA.
pub fn render_document(doc: &Document) -> PixelImage {
    let w = doc.width as usize;
    let h = doc.height as usize;
    let mut rgba = vec![0u8; w * h * 4];

    // 1) Preenche com a cor de fundo (opaca).
    let bg = doc.background;
    for px in rgba.chunks_exact_mut(4) {
        px[0] = bg.r;
        px[1] = bg.g;
        px[2] = bg.b;
        px[3] = 255;
    }

    // 2) Compõe cada camada visível por cima (alpha over), de baixo para cima.
    for layer in &doc.layers {
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
                continue; // transparente ou opacidade 0: não altera o fundo
            }
            let ia = 255 - sa; // inverso do alpha
            dst[0] = ((s[0] as u32 * sa + dst[0] as u32 * ia) / 255) as u8;
            dst[1] = ((s[1] as u32 * sa + dst[1] as u32 * ia) / 255) as u8;
            dst[2] = ((s[2] as u32 * sa + dst[2] as u32 * ia) / 255) as u8;
            dst[3] = 255;
        }
    }

    PixelImage {
        width: doc.width as i32,
        height: doc.height as i32,
        rgba,
    }
}
