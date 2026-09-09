//! Documento: dimensões, cor de fundo e a pilha de camadas.
//!
//! É a raiz do modelo — a "fonte única da verdade" da arquitetura. Toda
//! alteração no desenho acontece aqui; render e io apenas leem este estado.

use crate::color::Color;
use crate::frame::Frame;
use crate::layer::Layer;
use crate::skeleton::Skeleton;
use crate::vector::VectorObject;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub width: u32,
    pub height: u32,
    pub background: Color,
    /// Camadas de baixo (índice 0) para cima (última é a do topo).
    pub layers: Vec<Layer>,
    /// Objetos vetoriais (ilustração vetorial), desenhados sobre as camadas.
    #[serde(default)]
    pub vectors: Vec<VectorObject>,
    /// Frames da animação (cada um com suas camadas/vetores). `layers`/`vectors`
    /// acima são a cópia de trabalho do frame atual.
    #[serde(default)]
    pub frames: Vec<Frame>,
    #[serde(default)]
    pub current: usize,
    #[serde(default = "default_fps")]
    pub fps: u32,
    /// Esqueletos de rigging 2D presentes na cena (independentes das camadas).
    #[serde(default)]
    pub skeletons: Vec<Skeleton>,
}

fn default_fps() -> u32 {
    12
}

impl Document {
    /// Cria um documento novo já com uma camada inicial.
    pub fn new(width: u32, height: u32, background: Color) -> Self {
        let mut doc = Self {
            width,
            height,
            background,
            layers: Vec::new(),
            vectors: Vec::new(),
            frames: Vec::new(),
            current: 0,
            fps: 12,
            skeletons: Vec::new(),
        };
        doc.add_layer("Camada 1");
        doc.frames = vec![Frame {
            layers: doc.layers.clone(),
            vectors: doc.vectors.clone(),
        }];
        doc
    }

    /// Ajusta o estado após carregar (migra projetos antigos sem frames e
    /// carrega a cópia de trabalho a partir do frame atual).
    pub fn normalize(&mut self) {
        if self.frames.is_empty() {
            self.frames = vec![Frame {
                layers: std::mem::take(&mut self.layers),
                vectors: std::mem::take(&mut self.vectors),
            }];
        }
        for f in &mut self.frames {
            if f.layers.is_empty() {
                f.layers.push(Layer::new("Camada 1", self.width, self.height));
            }
        }
        if self.current >= self.frames.len() {
            self.current = 0;
        }
        if self.fps == 0 {
            self.fps = 12;
        }
        self.layers = self.frames[self.current].layers.clone();
        self.vectors = self.frames[self.current].vectors.clone();
    }

    /// Grava a cópia de trabalho no frame atual.
    pub fn sync_to_frames(&mut self) {
        let f = Frame {
            layers: self.layers.clone(),
            vectors: self.vectors.clone(),
        };
        if self.frames.is_empty() {
            self.frames.push(f);
            self.current = 0;
        } else if self.current < self.frames.len() {
            self.frames[self.current] = f;
        }
    }

    /// Troca o frame atual (salvando o anterior antes).
    pub fn go_to_frame(&mut self, i: usize) {
        if i >= self.frames.len() || i == self.current {
            if i < self.frames.len() {
                self.current = i;
            }
            return;
        }
        self.sync_to_frames();
        self.current = i;
        self.layers = self.frames[i].layers.clone();
        self.vectors = self.frames[i].vectors.clone();
    }

    /// Insere um frame em branco após o atual e vai para ele.
    pub fn add_frame(&mut self) -> usize {
        self.sync_to_frames();
        let at = (self.current + 1).min(self.frames.len());
        self.frames
            .insert(at, Frame::blank("Camada 1", self.width, self.height));
        self.current = at;
        self.layers = self.frames[at].layers.clone();
        self.vectors = self.frames[at].vectors.clone();
        at
    }

    /// Duplica o frame atual e vai para a cópia.
    pub fn duplicate_frame(&mut self) -> usize {
        self.sync_to_frames();
        let copy = self.frames[self.current].clone();
        let at = self.current + 1;
        self.frames.insert(at, copy);
        self.current = at;
        self.layers = self.frames[at].layers.clone();
        self.vectors = self.frames[at].vectors.clone();
        at
    }

    /// Remove um frame (mantém ao menos um).
    pub fn remove_frame(&mut self, i: usize) {
        if self.frames.len() <= 1 || i >= self.frames.len() {
            return;
        }
        self.frames.remove(i);
        if self.current >= self.frames.len() {
            self.current = self.frames.len() - 1;
        }
        self.layers = self.frames[self.current].layers.clone();
        self.vectors = self.frames[self.current].vectors.clone();
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len().max(1)
    }

    /// Adiciona uma camada transparente no topo e devolve o índice dela.
    pub fn add_layer(&mut self, name: impl Into<String>) -> usize {
        self.layers.push(Layer::new(name, self.width, self.height));
        self.layers.len() - 1
    }

    pub fn layer(&self, index: usize) -> Option<&Layer> {
        self.layers.get(index)
    }

    pub fn layer_mut(&mut self, index: usize) -> Option<&mut Layer> {
        self.layers.get_mut(index)
    }

    /// Insere uma camada nova logo acima da de índice `index` (na pilha) e
    /// devolve o índice dela.
    pub fn add_layer_above(&mut self, index: usize, name: impl Into<String>) -> usize {
        let at = (index + 1).min(self.layers.len());
        self.layers
            .insert(at, Layer::new(name, self.width, self.height));
        at
    }

    /// Remove a camada de índice `index` (mantém ao menos uma camada).
    pub fn remove_layer(&mut self, index: usize) {
        if self.layers.len() > 1 && index < self.layers.len() {
            self.layers.remove(index);
        }
    }

    /// Troca duas camadas de posição (reordenação na pilha).
    pub fn swap_layers(&mut self, a: usize, b: usize) {
        if a < self.layers.len() && b < self.layers.len() {
            self.layers.swap(a, b);
        }
    }

    /// Espelha todo o documento horizontalmente.
    pub fn flip_h(&mut self) {
        for l in &mut self.layers {
            l.flip_h();
        }
        let w = self.width as f32;
        for v in &mut self.vectors {
            v.flip_h(w);
        }
    }

    /// Espelha todo o documento verticalmente.
    pub fn flip_v(&mut self) {
        for l in &mut self.layers {
            l.flip_v();
        }
        let h = self.height as f32;
        for v in &mut self.vectors {
            v.flip_v(h);
        }
    }

    /// Gira todo o documento 180°.
    pub fn rotate_180(&mut self) {
        for l in &mut self.layers {
            l.rotate_180();
        }
        let (w, h) = (self.width as f32, self.height as f32);
        for v in &mut self.vectors {
            v.rotate_180(w, h);
        }
    }

    /// Gira todo o documento 90° horário (troca largura/altura).
    pub fn rotate_90_cw(&mut self) {
        let old_h = self.height as f32;
        for l in &mut self.layers {
            l.rotate_90_cw();
        }
        for v in &mut self.vectors {
            v.rotate_90_cw(old_h);
        }
        std::mem::swap(&mut self.width, &mut self.height);
    }

    /// Gira todo o documento 90° anti-horário (troca largura/altura).
    pub fn rotate_90_ccw(&mut self) {
        let old_w = self.width as f32;
        for l in &mut self.layers {
            l.rotate_90_ccw();
        }
        for v in &mut self.vectors {
            v.rotate_90_ccw(old_w);
        }
        std::mem::swap(&mut self.width, &mut self.height);
    }
}
