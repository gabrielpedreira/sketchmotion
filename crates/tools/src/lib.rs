//! sketchmotion-tools — ferramentas de desenho e edição.
//!
//! v0.2: além de pincel e borracha (raster), o enum passa a listar as
//! ferramentas da barra lateral (seleção, seleção direta, varinha, caneta,
//! texto). As que ainda não têm lógica ficam marcadas por `em_desenvolvimento`
//! e serão implementadas, cada uma no seu módulo, nas próximas versões.

use sketchmotion_core::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// Seta de seleção — selecionar e mover um elemento (em desenvolvimento).
    Select,
    /// Seleção livre (laço) — recorta pixels dentro de um contorno.
    Lasso,
    /// Seleção direta — editar por pontos (em desenvolvimento).
    DirectSelect,
    /// Varinha mágica — selecionar por cor (em desenvolvimento).
    MagicWand,
    /// Caneta — desenhar por pontos (em desenvolvimento).
    Pen,
    /// Texto (em desenvolvimento).
    Text,
    /// Formas geométricas (retângulo, elipse, triângulo, polígono).
    Shapes,
    /// Balde de preenchimento (flood fill estilo Paint).
    Fill,
    /// Pincel — desenho livre raster.
    Pencil,
    /// Borracha — pinta transparente, revelando o que está por baixo.
    Eraser,
    /// Rig 2D — criar/posar esqueletos de ossos (painel direito).
    Rig,
}

impl Tool {
    /// Cor efetivamente aplicada, dada a cor atual do pincel.
    pub fn effective_color(self, brush: Color) -> Color {
        match self {
            Tool::Eraser => Color::TRANSPARENT,
            _ => brush,
        }
    }

    /// True quando a ferramenta pinta livremente no canvas (pincel/borracha).
    pub fn paints(self) -> bool {
        matches!(self, Tool::Pencil | Tool::Eraser)
    }

    pub fn label(self) -> &'static str {
        match self {
            Tool::Select => "Seleção",
            Tool::Lasso => "Laço",
            Tool::DirectSelect => "Seleção direta",
            Tool::MagicWand => "Varinha mágica",
            Tool::Pen => "Caneta",
            Tool::Text => "Texto",
            Tool::Shapes => "Formas",
            Tool::Fill => "Balde",
            Tool::Pencil => "Pincel",
            Tool::Eraser => "Borracha",
            Tool::Rig => "Rig",
        }
    }
}
