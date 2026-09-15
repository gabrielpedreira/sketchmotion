//! Objeto de imagem: uma imagem raster colocada sobre o canvas como ELEMENTO
//! (estilo Illustrator) — móvel, redimensionável e girável — que **não** é
//! integrada aos pixels a menos que o usuário peça (botão "Integrar").
//!
//! Diferente de uma camada, o objeto guarda os próprios pixels de origem
//! (`ow`x`oh`) e uma transformação (centro, meias-dimensões, ângulo, opacidade).
//! É persistido no arquivo do projeto e composto na exportação, na mesma
//! resolução do trabalho (o render aplica a transformação sobre o buffer final).

use serde::{Deserialize, Serialize};

fn default_opacity() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageObject {
    /// Pixels de origem em RGBA8 (ow * oh * 4 bytes).
    pub pixels: Vec<u8>,
    /// Largura/altura originais da imagem (em pixels da origem).
    pub ow: u32,
    pub oh: u32,
    /// Centro do objeto no espaço do documento (em pixels).
    pub cx: f32,
    pub cy: f32,
    /// Meias-dimensões (metade da largura/altura em tela) — controlam a escala.
    pub hw: f32,
    pub hh: f32,
    /// Ângulo de rotação em radianos.
    #[serde(default)]
    pub angle: f32,
    /// Opacidade (0.0–1.0).
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    /// Camada à qual o objeto está associado (para z-order/integração).
    #[serde(default)]
    pub layer: usize,
}

impl ImageObject {
    /// Cria um objeto de imagem já posicionado.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pixels: Vec<u8>,
        ow: u32,
        oh: u32,
        cx: f32,
        cy: f32,
        hw: f32,
        hh: f32,
        angle: f32,
        opacity: f32,
        layer: usize,
    ) -> Self {
        Self {
            pixels,
            ow,
            oh,
            cx,
            cy,
            hw,
            hh,
            angle,
            opacity,
            layer,
        }
    }
}
