//! Vetor de Direção: anima automaticamente um objeto entre um estado inicial
//! (num frame) e um estado final (em outro frame), gerando os estados
//! intermediários por interpolação. É NÃO DESTRUTIVO na origem: guarda a
//! geometria-base capturada no início + as regras (frames, transformações,
//! interpolação, eixo/pivô) e regenera os frames intermediários a partir disso,
//! de modo que editar depois recalcula tudo.
//!
//! Integra-se ao sistema de Pivô: quando o objeto tem um eixo/pivô associado, o
//! vetor rotaciona ao redor desse eixo (movimento orbital), mantendo o eixo
//! fixo; sem pivô, translada (com rotação própria e escala opcionais).

use crate::camera::Interp;
use crate::image_object::ImageObject;
use crate::pivot::PivotTarget;
use crate::vector::VectorObject;
use serde::{Deserialize, Serialize};

/// Um vetor de direção (trajetória de transformação de um objeto/grupo).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirVector {
    pub name: String,
    /// Objeto/grupo controlado (reaproveita o alvo do sistema de Pivô).
    #[serde(default)]
    pub target: PivotTarget,
    pub start_frame: usize,
    pub end_frame: usize,
    #[serde(default)]
    pub interp: Interp,
    #[serde(default = "yes")]
    pub enabled: bool,

    // ---- Estado inicial / final ----
    /// Centro (bounds) do objeto no estado inicial e final (espaço do documento).
    #[serde(default)]
    pub start_centroid: (f32, f32),
    #[serde(default)]
    pub end_centroid: (f32, f32),
    /// Rotação (rad) no início e no fim. Com pivô: ângulo orbital ao redor do
    /// eixo. Sem pivô: rotação própria ao redor do centro.
    #[serde(default)]
    pub start_angle: f32,
    #[serde(default)]
    pub end_angle: f32,
    /// Escala no início e no fim (1.0 = sem mudança).
    #[serde(default = "one")]
    pub scale_start: f32,
    #[serde(default = "one")]
    pub scale_end: f32,

    // ---- Integração com pivô ----
    /// Usa rotação orbital ao redor de um eixo fixo?
    #[serde(default)]
    pub has_pivot: bool,
    /// Eixo (centro de rotação) quando `has_pivot`.
    #[serde(default)]
    pub pivot_axis: (f32, f32),

    // ---- Geometria-base (capturada no estado inicial) ----
    #[serde(default)]
    pub base_vectors: Vec<VectorObject>,
    #[serde(default)]
    pub base_image: Option<ImageObject>,
    #[serde(default)]
    pub base_image_index: usize,

    // ---- Fluxo ----
    #[serde(default)]
    pub has_start: bool,
    #[serde(default)]
    pub has_end: bool,
}

fn yes() -> bool {
    true
}
fn one() -> f32 {
    1.0
}

impl DirVector {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            target: PivotTarget::None,
            start_frame: 0,
            end_frame: 0,
            interp: Interp::Linear,
            enabled: true,
            start_centroid: (0.0, 0.0),
            end_centroid: (0.0, 0.0),
            start_angle: 0.0,
            end_angle: 0.0,
            scale_start: 1.0,
            scale_end: 1.0,
            has_pivot: false,
            pivot_axis: (0.0, 0.0),
            base_vectors: Vec::new(),
            base_image: None,
            base_image_index: 0,
            has_start: false,
            has_end: false,
        }
    }

    /// Pronto para aplicar? (tem início, fim e um intervalo válido.)
    pub fn completo(&self) -> bool {
        self.has_start && self.has_end && self.end_frame != self.start_frame
    }

    /// Menor e maior frame do intervalo (aceita início > fim).
    pub fn faixa(&self) -> (usize, usize) {
        (
            self.start_frame.min(self.end_frame),
            self.start_frame.max(self.end_frame),
        )
    }

    /// Parâmetro interpolado t∈[0,1] para um frame `f` do intervalo, já com a
    /// curva de easing aplicada. Respeita o sentido (início→fim).
    pub fn t_para_frame(&self, f: usize) -> f32 {
        let span = self.end_frame as f32 - self.start_frame as f32;
        if span == 0.0 {
            return 0.0;
        }
        let raw = (f as f32 - self.start_frame as f32) / span;
        self.interp.ease(raw.clamp(0.0, 1.0))
    }
}
