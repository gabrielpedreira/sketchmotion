//! Pivô / eixo de transformação: um ponto de rotação personalizado associado a
//! um objeto (ou grupo). Guarda dois pontos no espaço do documento:
//!
//! - **eixo** (visual azul): o centro real de transformação/rotação;
//! - **movimentação** (visual laranja): a "alavanca" que o usuário arrasta para
//!   girar o objeto ao redor do eixo, mantendo o eixo fixo.
//!
//! O pivô é uma propriedade *estrutural* do objeto (não um mero enfeite): é a
//! base para a ferramenta Vetor de Direção, rotação, escala, câmera e, adiante,
//! bones/IK. Por isso vive no documento e persiste no arquivo, associado ao
//! alvo por **id de grupo** (que sobrevive entre frames) ou por objeto de
//! imagem.

use serde::{Deserialize, Serialize};

/// A que o pivô está associado.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PivotTarget {
    /// Nada associado ainda.
    None,
    /// Um grupo de vetores (também usado para um objeto único, que ganha um
    /// grupo próprio ao ser associado — assim a ligação sobrevive a reordenações
    /// e mudanças de índice entre frames).
    Group(u32),
    /// Um objeto de imagem, pelo índice no frame.
    Image(usize),
}

impl Default for PivotTarget {
    fn default() -> Self {
        PivotTarget::None
    }
}

/// Um pivô nomeado: eixo (azul), ponto de movimentação (laranja) e alvo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pivot {
    pub name: String,
    /// Pivô ligado/desligado (quando desligado, não transforma o objeto).
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Ponto EIXO (azul), em coordenadas do documento.
    pub axis: (f32, f32),
    /// Ponto de MOVIMENTAÇÃO (laranja), em coordenadas do documento.
    pub mov: (f32, f32),
    /// Objeto/grupo controlado.
    #[serde(default)]
    pub target: PivotTarget,
    /// O eixo já foi posicionado? (fluxo de criação: primeiro clique.)
    #[serde(default)]
    pub has_axis: bool,
    /// O ponto de movimentação já foi posicionado? (segundo clique.)
    #[serde(default)]
    pub has_mov: bool,
}

fn yes() -> bool {
    true
}

impl Pivot {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            enabled: true,
            axis: (0.0, 0.0),
            mov: (0.0, 0.0),
            target: PivotTarget::None,
            has_axis: false,
            has_mov: false,
        }
    }

    /// Já está totalmente configurado (tem eixo e ponto de movimentação)?
    pub fn completo(&self) -> bool {
        self.has_axis && self.has_mov
    }

    /// Ângulo (rad) do vetor eixo→movimentação. Base para a ferramenta Vetor de
    /// Direção calcular a rotação entre frames.
    pub fn angulo(&self) -> f32 {
        (self.mov.1 - self.axis.1).atan2(self.mov.0 - self.axis.0)
    }

    /// Distância entre eixo e ponto de movimentação (o "raio" da alavanca).
    pub fn raio(&self) -> f32 {
        let (dx, dy) = (self.mov.0 - self.axis.0, self.mov.1 - self.axis.1);
        (dx * dx + dy * dy).sqrt()
    }
}
