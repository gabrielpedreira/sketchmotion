//! sketchmotion-tools — ferramentas de desenho.
//!
//! v0.1: lápis e borracha, como um enum simples. Quando surgirem ferramentas
//! com estado próprio (seleção, formas — v0.2), isto vira uma máquina de estado
//! por trait, uma por ferramenta, sem alterar as demais.

use sketchmotion_core::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Pencil,
    Eraser,
}

impl Tool {
    /// Cor efetivamente aplicada, dada a cor atual do pincel.
    /// A borracha pinta transparente, revelando o que está por baixo.
    pub fn effective_color(self, brush: Color) -> Color {
        match self {
            Tool::Pencil => brush,
            Tool::Eraser => Color::TRANSPARENT,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tool::Pencil => "Lápis",
            Tool::Eraser => "Borracha",
        }
    }
}
