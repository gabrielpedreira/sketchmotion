//! Documento: dimensões, cor de fundo e a pilha de camadas.
//!
//! É a raiz do modelo — a "fonte única da verdade" da arquitetura. Toda
//! alteração no desenho acontece aqui; render e io apenas leem este estado.

use crate::camera::Camera;
use crate::color::Color;
use crate::frame::Frame;
use crate::image_object::ImageObject;
use crate::layer::Layer;
use crate::pivot::Pivot;
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
    /// Objetos de imagem (cópia de trabalho do frame atual) — raster colocado
    /// como elemento móvel/selecionável, não integrado aos pixels.
    #[serde(default)]
    pub images: Vec<ImageObject>,
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
    /// Modo pixel art (grade/nitidez) — persiste no arquivo.
    #[serde(default)]
    pub pixel_art: bool,
    /// Timelines (faixas) de animação. Cada uma tem seus próprios frames/FPS.
    /// `frames`/`current`/`fps` acima são o ESPELHO da faixa ativa (a editável).
    #[serde(default)]
    pub tracks: Vec<Track>,
    /// Índice da faixa ativa (a única que pode ser editada/desenhada).
    #[serde(default)]
    pub active_track: usize,
    /// Onion skin ENTRE timelines: mostra a mesma página das outras faixas
    /// translúcida (para usar uma como rascunho e outra como arte final).
    #[serde(default)]
    pub onion_between: bool,
    /// Câmera animada por keyframes (entidade independente da timeline de
    /// desenho). Enquadra a composição final; não transforma os objetos.
    #[serde(default)]
    pub camera: Camera,
    /// Pivôs (eixos de transformação personalizados) associados a objetos/grupos.
    /// Propriedade estrutural do documento; persiste entre frames.
    #[serde(default)]
    pub pivots: Vec<Pivot>,
}

/// Uma timeline (faixa) de animação: lista própria de frames, FPS e
/// visibilidade. O documento pode empilhar várias.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub name: String,
    pub frames: Vec<Frame>,
    #[serde(default)]
    pub current: usize,
    #[serde(default = "default_fps")]
    pub fps: u32,
    #[serde(default = "vis_true")]
    pub visible: bool,
}

fn default_fps() -> u32 {
    12
}

fn vis_true() -> bool {
    true
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
            images: Vec::new(),
            frames: Vec::new(),
            current: 0,
            fps: 12,
            skeletons: Vec::new(),
            pixel_art: false,
            tracks: Vec::new(),
            active_track: 0,
            onion_between: false,
            camera: Camera::new(width, height),
            pivots: Vec::new(),
        };
        doc.add_layer("Camada 1");
        doc.frames = vec![Frame {
            layers: doc.layers.clone(),
            vectors: doc.vectors.clone(),
            images: doc.images.clone(),
        }];
        doc.tracks = vec![Track {
            name: "Timeline 1".to_string(),
            frames: doc.frames.clone(),
            current: 0,
            fps: doc.fps,
            visible: true,
        }];
        doc.active_track = 0;
        doc
    }

    /// Ajusta o estado após carregar (migra projetos antigos sem frames e
    /// carrega a cópia de trabalho a partir do frame atual).
    pub fn normalize(&mut self) {
        if self.frames.is_empty() {
            self.frames = vec![Frame {
                layers: std::mem::take(&mut self.layers),
                vectors: std::mem::take(&mut self.vectors),
                images: std::mem::take(&mut self.images),
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
        self.images = self.frames[self.current].images.clone();
        // Migra projetos antigos (sem timelines) e carrega a faixa ativa.
        if self.tracks.is_empty() {
            self.tracks = vec![Track {
                name: "Timeline 1".to_string(),
                frames: self.frames.clone(),
                current: self.current,
                fps: self.fps,
                visible: true,
            }];
            self.active_track = 0;
        }
        if self.active_track >= self.tracks.len() {
            self.active_track = 0;
        }
        self.camera.ensure(self.width, self.height);
        self.load_active_track_mirror();
    }

    /// Carrega os dados da faixa ativa no espelho (frames/current/fps) e
    /// atualiza a cópia de trabalho (layers/vectors).
    fn load_active_track_mirror(&mut self) {
        let mut frames = self.tracks[self.active_track].frames.clone();
        let fps = self.tracks[self.active_track].fps.max(1);
        let mut current = self.tracks[self.active_track].current;
        if frames.is_empty() {
            frames.push(Frame::blank("Camada 1", self.width, self.height));
            current = 0;
        }
        if current >= frames.len() {
            current = frames.len() - 1;
        }
        self.frames = frames;
        self.current = current;
        self.fps = fps;
        self.layers = self.frames[self.current].layers.clone();
        self.vectors = self.frames[self.current].vectors.clone();
        self.images = self.frames[self.current].images.clone();
    }

    /// Copia o espelho (frames/current/fps) de volta para a faixa ativa.
    fn mirror_to_track(&mut self) {
        if self.active_track < self.tracks.len() {
            self.tracks[self.active_track].frames = self.frames.clone();
            self.tracks[self.active_track].current = self.current;
            self.tracks[self.active_track].fps = self.fps;
        }
    }

    /// Grava a cópia de trabalho no frame atual.
    pub fn sync_to_frames(&mut self) {
        let f = Frame {
            layers: self.layers.clone(),
            vectors: self.vectors.clone(),
            images: self.images.clone(),
        };
        if self.frames.is_empty() {
            self.frames.push(f);
            self.current = 0;
        } else if self.current < self.frames.len() {
            self.frames[self.current] = f;
        }
        self.mirror_to_track();
    }

    pub fn track_count(&self) -> usize {
        self.tracks.len().max(1)
    }

    /// Troca a faixa ativa (a selecionada = editável), salvando a anterior.
    pub fn go_to_track(&mut self, i: usize) {
        if i >= self.tracks.len() || i == self.active_track {
            return;
        }
        self.sync_to_frames();
        self.active_track = i;
        self.load_active_track_mirror();
    }

    /// Cria uma faixa nova (um frame em branco) e vai para ela.
    pub fn add_track(&mut self) -> usize {
        self.sync_to_frames();
        let n = self.tracks.len() + 1;
        self.tracks.push(Track {
            name: format!("Timeline {n}"),
            frames: vec![Frame::blank("Camada 1", self.width, self.height)],
            current: 0,
            fps: self.fps,
            visible: true,
        });
        let idx = self.tracks.len() - 1;
        self.active_track = idx;
        self.load_active_track_mirror();
        idx
    }

    /// Redimensiona o PAPEL (canvas) para (w, h) em pixels, preservando o
    /// desenho ancorado no topo-esquerda em TODOS os frames de TODAS as faixas.
    /// NÃO escala a imagem — só muda o tamanho do papel.
    pub fn resize_canvas(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 || (w == self.width && h == self.height) {
            return;
        }
        self.sync_to_frames();
        for t in &mut self.tracks {
            for f in &mut t.frames {
                for l in &mut f.layers {
                    l.resize(w, h);
                }
            }
        }
        self.width = w;
        self.height = h;
        // Atualiza a base da câmera (para o Reset/zoom 100% seguir o novo papel).
        self.camera.base_w = w as f32;
        self.camera.base_h = h as f32;
        self.load_active_track_mirror();
    }

    /// Acrescenta uma faixa já pronta (ex.: importar de outro arquivo como
    /// referência). NÃO troca a faixa ativa. Devolve o índice dela.
    pub fn push_track(&mut self, name: impl Into<String>, frames: Vec<Frame>, fps: u32) -> usize {
        self.tracks.push(Track {
            name: name.into(),
            frames,
            current: 0,
            fps: fps.max(1),
            visible: true,
        });
        self.tracks.len() - 1
    }

    /// Remove uma faixa (mantém ao menos uma).
    pub fn remove_track(&mut self, i: usize) {
        if self.tracks.len() <= 1 || i >= self.tracks.len() {
            return;
        }
        self.sync_to_frames();
        self.tracks.remove(i);
        if self.active_track >= self.tracks.len() {
            self.active_track = self.tracks.len() - 1;
        } else if i < self.active_track {
            self.active_track -= 1;
        }
        self.load_active_track_mirror();
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
        self.images = self.frames[i].images.clone();
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
        self.images = self.frames[at].images.clone();
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
        self.images = self.frames[at].images.clone();
        at
    }

    /// Clona o frame atual (cópia de trabalho) — para copiar/colar frames.
    pub fn current_frame_clone(&self) -> Frame {
        Frame {
            layers: self.layers.clone(),
            vectors: self.vectors.clone(),
            images: self.images.clone(),
        }
    }

    /// Insere um frame (colado) logo após o atual e vai para ele.
    pub fn paste_frame(&mut self, frame: Frame) -> usize {
        self.sync_to_frames();
        let at = (self.current + 1).min(self.frames.len());
        self.frames.insert(at, frame);
        self.current = at;
        self.layers = self.frames[at].layers.clone();
        self.vectors = self.frames[at].vectors.clone();
        self.images = self.frames[at].images.clone();
        self.mirror_to_track();
        at
    }

    /// Move um frame de `from` para a lacuna `to` (0..=len) na faixa ativa.
    pub fn move_frame(&mut self, from: usize, to: usize) {
        if from >= self.frames.len() {
            return;
        }
        self.sync_to_frames();
        let f = self.frames.remove(from);
        let mut t = to.min(self.frames.len() + 1);
        if t > from {
            t -= 1;
        }
        if t > self.frames.len() {
            t = self.frames.len();
        }
        self.frames.insert(t, f);
        self.current = t;
        self.layers = self.frames[self.current].layers.clone();
        self.vectors = self.frames[self.current].vectors.clone();
        self.images = self.frames[self.current].images.clone();
        self.mirror_to_track();
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
        self.images = self.frames[self.current].images.clone();
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
