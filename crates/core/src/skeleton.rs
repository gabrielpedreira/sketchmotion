//! Rigging 2D — um esqueleto é uma árvore de ossos (bones). Cada osso é apenas
//! um *transform* (posição da articulação, rotação e escala) relativo ao pai;
//! o desenho é anexado a ossos depois (0.5.2). Aqui ficam o modelo e o FK
//! (forward kinematics): resolver o transform de mundo varrendo a hierarquia.

use serde::{Deserialize, Serialize};

/// Pose de um osso (usada como pose de descanso e, no futuro, em keyframes).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BonePose {
    pub x: f32,
    pub y: f32,
    pub angle: f32,
    pub scale: f32,
}

/// Forma visual do osso: membro afinado (padrão) ou peças de corpo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BoneShape {
    /// Membro/limbo — losango afinado (braço, perna, dedo, cauda).
    #[default]
    Limb,
    /// Tronco — pentagono com ombros.
    Torso,
    /// Quadril/pelve — pentagono mais largo.
    Hip,
    /// Cabeca — elipse.
    Head,
    /// Mao.
    Mao,
    /// Pe.
    Pe,
}

/// Um osso: transform LOCAL relativo ao pai + comprimento visual.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bone {
    pub id: u32,
    pub parent: Option<u32>,
    pub name: String,
    /// Posição da articulação (origem), no espaço local do pai.
    pub x: f32,
    pub y: f32,
    /// Rotação local (radianos) e escala local.
    pub angle: f32,
    pub scale: f32,
    /// Comprimento do osso ao longo do eixo X local rotacionado.
    pub length: f32,
    /// Forma visual do osso.
    #[serde(default)]
    pub shape: BoneShape,
    /// Imagem propria da peca (indice numa tabela do app); None = usa `shape`.
    #[serde(default)]
    pub img: Option<u16>,
    /// Pose de descanso, para "resetar pose" e futuros keyframes.
    pub rest: BonePose,
}

/// Transform de mundo já resolvido (origem + rotação + escala acumuladas).
#[derive(Debug, Clone, Copy)]
pub struct World {
    pub x: f32,
    pub y: f32,
    pub angle: f32,
    pub scale: f32,
}

fn vis_true() -> bool {
    true
}
fn one() -> u32 {
    1
}

/// Um esqueleto = conjunto de ossos em árvore (por `parent`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skeleton {
    pub name: String,
    #[serde(default = "vis_true")]
    pub visible: bool,
    pub bones: Vec<Bone>,
    #[serde(default = "one")]
    next_id: u32,
}

impl Skeleton {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            visible: true,
            bones: Vec::new(),
            next_id: 1,
        }
    }

    pub fn bone(&self, id: u32) -> Option<&Bone> {
        self.bones.iter().find(|b| b.id == id)
    }
    pub fn bone_mut(&mut self, id: u32) -> Option<&mut Bone> {
        self.bones.iter_mut().find(|b| b.id == id)
    }

    /// Cria um osso com o transform local dado; devolve o id novo.
    pub fn add_bone(
        &mut self,
        parent: Option<u32>,
        x: f32,
        y: f32,
        angle: f32,
        length: f32,
        shape: BoneShape,
    ) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let n = self.bones.len() + 1;
        self.bones.push(Bone {
            id,
            parent,
            name: format!("Osso {n}"),
            x,
            y,
            angle,
            scale: 1.0,
            length,
            shape,
            img: None,
            rest: BonePose {
                x,
                y,
                angle,
                scale: 1.0,
            },
        });
        id
    }

    /// Como `add_bone`, mas recebe a origem e o ângulo em coordenadas de MUNDO;
    /// converte para o espaço local do pai. Útil para montar esqueletos prontos.
    pub fn add_bone_world(
        &mut self,
        parent: Option<u32>,
        wx: f32,
        wy: f32,
        world_angle: f32,
        length: f32,
        shape: BoneShape,
    ) -> u32 {
        let (lx, ly, la) = match parent {
            None => (wx, wy, world_angle),
            Some(pp) => {
                let pw = self.world(pp);
                let (dx, dy) = (wx - pw.x, wy - pw.y);
                let (s, c) = (-pw.angle).sin_cos();
                let sc = pw.scale.max(1e-4);
                ((dx * c - dy * s) / sc, (dx * s + dy * c) / sc, world_angle - pw.angle)
            }
        };
        self.add_bone(parent, lx, ly, la, length, shape)
    }

    /// Transform de mundo do osso (FK: sobe pela cadeia de pais).
    pub fn world(&self, id: u32) -> World {
        let b = match self.bone(id) {
            Some(b) => b,
            None => {
                return World {
                    x: 0.0,
                    y: 0.0,
                    angle: 0.0,
                    scale: 1.0,
                }
            }
        };
        match b.parent {
            None => World {
                x: b.x,
                y: b.y,
                angle: b.angle,
                scale: b.scale,
            },
            Some(p) => {
                let pw = self.world(p);
                let (s, c) = pw.angle.sin_cos();
                let lx = b.x * pw.scale;
                let ly = b.y * pw.scale;
                World {
                    x: pw.x + lx * c - ly * s,
                    y: pw.y + lx * s + ly * c,
                    angle: pw.angle + b.angle,
                    scale: pw.scale * b.scale,
                }
            }
        }
    }

    /// Origem (articulação) do osso em coordenadas de mundo.
    pub fn origin(&self, id: u32) -> (f32, f32) {
        let w = self.world(id);
        (w.x, w.y)
    }

    /// Ponta do osso em mundo (origem + length ao longo do X rotacionado).
    pub fn tip(&self, id: u32) -> (f32, f32) {
        let w = self.world(id);
        let len = self.bone(id).map(|b| b.length).unwrap_or(0.0) * w.scale;
        let (s, c) = w.angle.sin_cos();
        (w.x + len * c, w.y + len * s)
    }

    /// Remove um osso e reparenta os filhos ao avô (evita órfãos soltos).
    pub fn remove_bone(&mut self, id: u32) {
        let parent = self.bone(id).and_then(|b| b.parent);
        for b in &mut self.bones {
            if b.parent == Some(id) {
                b.parent = parent;
            }
        }
        self.bones.retain(|b| b.id != id);
    }

    /// Volta todos os ossos para a pose de descanso.
    pub fn reset_pose(&mut self) {
        for b in &mut self.bones {
            b.x = b.rest.x;
            b.y = b.rest.y;
            b.angle = b.rest.angle;
            b.scale = b.rest.scale;
        }
    }

    /// Grava a pose atual como nova pose de descanso.
    pub fn set_rest(&mut self) {
        for b in &mut self.bones {
            b.rest = BonePose {
                x: b.x,
                y: b.y,
                angle: b.angle,
                scale: b.scale,
            };
        }
    }
}
