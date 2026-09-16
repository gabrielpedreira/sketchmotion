//! Câmera animada por keyframes.
//!
//! A câmera é uma ENTIDADE INDEPENDENTE da timeline de desenho: ela não gera
//! frames; guarda apenas keyframes de transformação (posição, tamanho, rotação
//! e pivô) e o estado intermediário de qualquer frame é CALCULADO por
//! interpolação matemática (`sample`). Assim, "Frame 1 → Frame 11" é uma única
//! transformação contínua, e não um estado por frame.
//!
//! A câmera controla apenas o ENQUADRAMENTO/visualização da composição final —
//! nunca transforma os objetos (camadas/vetores/imagens/rig) em si.

use serde::{Deserialize, Serialize};

/// Tipo de interpolação de um segmento (do keyframe atual até o próximo).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Interp {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    /// Segura o valor até o próximo keyframe (sem transição).
    Constant,
}

impl Default for Interp {
    fn default() -> Self {
        Interp::Linear
    }
}

impl Interp {
    /// Remapeia t∈[0,1] conforme a curva. `Constant` devolve 0 (mantém o valor
    /// do keyframe de origem até o próximo).
    pub fn ease(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Interp::Linear => t,
            Interp::EaseIn => t * t,
            Interp::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
            Interp::EaseInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
                }
            }
            Interp::Constant => 0.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Interp::Linear => "Linear",
            Interp::EaseIn => "Ease In",
            Interp::EaseOut => "Ease Out",
            Interp::EaseInOut => "Ease In/Out",
            Interp::Constant => "Constante (hold)",
        }
    }

    pub fn all() -> [Interp; 5] {
        [
            Interp::Linear,
            Interp::EaseIn,
            Interp::EaseOut,
            Interp::EaseInOut,
            Interp::Constant,
        ]
    }
}

fn default_interp() -> Interp {
    Interp::Linear
}

/// Um keyframe da câmera, atrelado a um frame da timeline.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct CameraKeyframe {
    /// Frame (índice na timeline) a que este keyframe está atrelado.
    pub frame: usize,
    /// Centro do enquadramento (coords do documento, em pixels).
    pub x: f32,
    pub y: f32,
    /// Tamanho do retângulo de enquadramento (pixels do documento).
    pub w: f32,
    pub h: f32,
    /// Rotação do enquadramento em GRAUS.
    #[serde(default)]
    pub rotation: f32,
    /// Pivô/âncora como OFFSET a partir do centro (pixels). 0,0 = centro.
    #[serde(default)]
    pub anchor_x: f32,
    #[serde(default)]
    pub anchor_y: f32,
    /// Interpolação deste keyframe até o próximo.
    #[serde(default = "default_interp")]
    pub interp: Interp,
}

/// Estado (já interpolado) da câmera num frame qualquer.
#[derive(Debug, Clone, Copy)]
pub struct CameraState {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub rotation: f32,
    pub anchor_x: f32,
    pub anchor_y: f32,
}

/// A câmera do documento: tamanho base (para Reset) + lista de keyframes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Camera {
    /// Tamanho "cheio" (o do canvas) — usado pelo Reset e pelo zoom 100%.
    pub base_w: f32,
    pub base_h: f32,
    /// Keyframes ordenados por `frame` crescente.
    pub keyframes: Vec<CameraKeyframe>,
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

impl Camera {
    /// Câmera nova enquadrando exatamente o canvas (um keyframe no frame 0).
    pub fn new(w: u32, h: u32) -> Self {
        let (bw, bh) = (w as f32, h as f32);
        Self {
            base_w: bw,
            base_h: bh,
            keyframes: vec![CameraKeyframe {
                frame: 0,
                x: bw / 2.0,
                y: bh / 2.0,
                w: bw,
                h: bh,
                rotation: 0.0,
                anchor_x: 0.0,
                anchor_y: 0.0,
                interp: Interp::Linear,
            }],
        }
    }

    /// Estado padrão (câmera cheia = canvas), usado quando não há keyframes.
    pub fn full_state(&self) -> CameraState {
        CameraState {
            x: self.base_w / 2.0,
            y: self.base_h / 2.0,
            w: self.base_w.max(1.0),
            h: self.base_h.max(1.0),
            rotation: 0.0,
            anchor_x: 0.0,
            anchor_y: 0.0,
        }
    }

    /// Garante base coerente e ao menos um keyframe (para projetos migrados).
    pub fn ensure(&mut self, w: u32, h: u32) {
        if self.base_w <= 0.0 || self.base_h <= 0.0 {
            self.base_w = w as f32;
            self.base_h = h as f32;
        }
        if self.keyframes.is_empty() {
            *self = Camera::new(w, h);
        }
    }

    /// Índice do keyframe exatamente no frame dado, se existir.
    pub fn keyframe_index(&self, frame: usize) -> Option<usize> {
        self.keyframes.iter().position(|k| k.frame == frame)
    }

    /// Cria/atualiza o keyframe no `frame` dado, mantendo a ordem.
    pub fn set_keyframe(&mut self, kf: CameraKeyframe) {
        if let Some(i) = self.keyframe_index(kf.frame) {
            self.keyframes[i] = kf;
        } else {
            self.keyframes.push(kf);
            self.keyframes.sort_by_key(|k| k.frame);
        }
    }

    /// Remove o keyframe no `frame`, se houver (mantém ao menos um).
    pub fn remove_keyframe(&mut self, frame: usize) {
        if self.keyframes.len() <= 1 {
            return;
        }
        if let Some(i) = self.keyframe_index(frame) {
            self.keyframes.remove(i);
        }
    }

    /// Reseta para uma câmera cheia (um keyframe no frame 0 = canvas).
    pub fn reset(&mut self, w: u32, h: u32) {
        *self = Camera::new(w, h);
    }

    /// Estado interpolado da câmera no `frame` pedido.
    pub fn sample(&self, frame: usize) -> CameraState {
        if self.keyframes.is_empty() {
            return self.full_state();
        }
        let f = frame as f32;
        let first = &self.keyframes[0];
        if frame <= first.frame {
            return self.state_of(first);
        }
        let last = &self.keyframes[self.keyframes.len() - 1];
        if frame >= last.frame {
            return self.state_of(last);
        }
        // Encontra o par [a, b] com a.frame <= frame < b.frame.
        for pair in self.keyframes.windows(2) {
            let a = &pair[0];
            let b = &pair[1];
            if frame >= a.frame && frame < b.frame {
                let span = (b.frame - a.frame).max(1) as f32;
                let raw = (f - a.frame as f32) / span;
                let t = a.interp.ease(raw);
                return CameraState {
                    x: lerp(a.x, b.x, t),
                    y: lerp(a.y, b.y, t),
                    w: lerp(a.w, b.w, t).max(1.0),
                    h: lerp(a.h, b.h, t).max(1.0),
                    rotation: lerp(a.rotation, b.rotation, t),
                    anchor_x: lerp(a.anchor_x, b.anchor_x, t),
                    anchor_y: lerp(a.anchor_y, b.anchor_y, t),
                };
            }
        }
        self.state_of(last)
    }

    fn state_of(&self, k: &CameraKeyframe) -> CameraState {
        CameraState {
            x: k.x,
            y: k.y,
            w: k.w.max(1.0),
            h: k.h.max(1.0),
            rotation: k.rotation,
            anchor_x: k.anchor_x,
            anchor_y: k.anchor_y,
        }
    }
}
