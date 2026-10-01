//! Guias de régua: linhas de referência globais, horizontais ou verticais,
//! visíveis em QUALQUER camada e frame. Não pertencem a nenhuma camada nem
//! frame — vivem no documento inteiro (fora dessas repartições) e persistem no
//! arquivo do projeto. Servem só de apoio visual (alinhamento); não entram na
//! composição/exportação.

use serde::{Deserialize, Serialize};

/// Orientação de uma guia.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GuideOrient {
    /// Linha horizontal — posição no eixo Y (em pixels do documento).
    Horizontal,
    /// Linha vertical — posição no eixo X (em pixels do documento).
    Vertical,
}

/// Uma linha-guia da régua.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Guide {
    pub orient: GuideOrient,
    /// Posição em coordenadas do documento: Y para horizontal, X para vertical.
    pub pos: f32,
}

impl Guide {
    pub fn horizontal(y: f32) -> Self {
        Self { orient: GuideOrient::Horizontal, pos: y }
    }
    pub fn vertical(x: f32) -> Self {
        Self { orient: GuideOrient::Vertical, pos: x }
    }
    pub fn is_horizontal(&self) -> bool {
        matches!(self.orient, GuideOrient::Horizontal)
    }
}
