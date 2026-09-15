//! Frame: um quadro da animação = suas camadas + objetos vetoriais.
//! O documento guarda uma lista de frames; a "cópia de trabalho" (Document.
//! layers/vectors) espelha o frame atual para que todo o desenho já existente
//! continue operando sobre ele sem mudanças.

use crate::image_object::ImageObject;
use crate::layer::Layer;
use crate::vector::VectorObject;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame {
    pub layers: Vec<Layer>,
    #[serde(default)]
    pub vectors: Vec<VectorObject>,
    /// Objetos de imagem (raster colocado como elemento, não integrado).
    #[serde(default)]
    pub images: Vec<ImageObject>,
}

impl Frame {
    /// Frame em branco, com uma única camada transparente.
    pub fn blank(name: impl Into<String>, width: u32, height: u32) -> Self {
        Self {
            layers: vec![Layer::new(name, width, height)],
            vectors: Vec::new(),
            images: Vec::new(),
        }
    }
}
