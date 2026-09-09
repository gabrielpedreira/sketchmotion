//! sketchmotion-core — modelo de documento (fonte única da verdade).
//!
//! Etapa 3 (v0.1): estruturas mínimas — `Color`, `Layer` e `Document`. Ainda
//! sem histórico/undo (v0.2), frames (v0.3) ou rig (v0.5). Não depende de
//! nenhum outro crate do workspace, então pode ser testado sem abrir janela.

mod color;
mod document;
mod frame;
mod layer;
mod skeleton;
mod vector;

pub use color::Color;
pub use document::Document;
pub use frame::Frame;
pub use layer::Layer;
pub use skeleton::{Bone, BonePose, BoneShape, Skeleton, World};
pub use vector::{Anchor, VectorObject};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn novo_documento_ja_tem_uma_camada() {
        let doc = Document::new(32, 24, Color::WHITE);
        assert_eq!(doc.width, 32);
        assert_eq!(doc.height, 24);
        assert_eq!(doc.layers.len(), 1);
    }

    #[test]
    fn adicionar_camada_incrementa_a_pilha() {
        let mut doc = Document::new(10, 10, Color::WHITE);
        let idx = doc.add_layer("Camada 2");
        assert_eq!(idx, 1);
        assert_eq!(doc.layers.len(), 2);
    }

    #[test]
    fn camada_nasce_transparente() {
        let layer = Layer::new("teste", 4, 4);
        assert_eq!(layer.pixels().len(), 4 * 4 * 4);
        assert!(layer.pixels().iter().all(|&b| b == 0));
        assert_eq!(layer.get_pixel(0, 0), Some(Color::TRANSPARENT));
    }

    #[test]
    fn pintar_e_ler_um_pixel() {
        let mut layer = Layer::new("teste", 4, 4);
        let vermelho = Color::rgb(255, 0, 0);
        layer.set_pixel(1, 2, vermelho);
        assert_eq!(layer.get_pixel(1, 2), Some(vermelho));
        // O vizinho continua transparente.
        assert_eq!(layer.get_pixel(0, 0), Some(Color::TRANSPARENT));
    }

    #[test]
    fn pixel_fora_dos_limites_e_ignorado() {
        let mut layer = Layer::new("teste", 4, 4);
        layer.set_pixel(100, 100, Color::WHITE); // não deve entrar em pânico
        assert_eq!(layer.get_pixel(100, 100), None);
    }
}
