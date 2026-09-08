//! Modelo vetorial: objetos selecionáveis feitos de pontos (e, adiante, curvas
//! bézier). É a base da ilustração vetorial, da seleção ponto a ponto e,
//! futuramente, da animação e do rigging. Convive com as camadas raster
//! (modelo híbrido): pixel art/pintura continuam em pixels; traços/formas
//! selecionáveis vivem aqui, como objetos.

use crate::color::Color;
use serde::{Deserialize, Serialize};

/// Ponto-âncora do caminho. `hin`/`hout` são as alças de curva bézier,
/// relativas ao ponto (None = canto reto). Nesta fatia usamos só retas; as
/// alças já ficam no formato para as curvas entrarem sem migração de arquivo.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    pub x: f32,
    pub y: f32,
    #[serde(default)]
    pub hin: Option<(f32, f32)>,
    #[serde(default)]
    pub hout: Option<(f32, f32)>,
}

impl Anchor {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y, hin: None, hout: None }
    }
}

/// Objeto vetorial: um caminho (lista de âncoras) + traço. Preenchimento e
/// transformação própria entram nas próximas fatias.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VectorObject {
    pub points: Vec<Anchor>,
    #[serde(default)]
    pub closed: bool,
    pub stroke: Color,
    pub stroke_width: f32,
    #[serde(default = "one")]
    pub opacity: f32,
}

fn one() -> f32 {
    1.0
}

impl VectorObject {
    pub fn new(stroke: Color, stroke_width: f32) -> Self {
        Self {
            points: Vec::new(),
            closed: false,
            stroke,
            stroke_width,
            opacity: 1.0,
        }
    }

    /// Caixa delimitadora (min_x, min_y, max_x, max_y) pelas âncoras.
    pub fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let mut it = self.points.iter();
        let f = it.next()?;
        let (mut minx, mut miny, mut maxx, mut maxy) = (f.x, f.y, f.x, f.y);
        for p in it {
            minx = minx.min(p.x);
            miny = miny.min(p.y);
            maxx = maxx.max(p.x);
            maxy = maxy.max(p.y);
        }
        Some((minx, miny, maxx, maxy))
    }

    /// Centro geométrico da caixa delimitadora.
    pub fn center(&self) -> Option<(f32, f32)> {
        self.bounds().map(|(a, b, c, d)| ((a + c) / 2.0, (b + d) / 2.0))
    }

    /// Move todo o objeto por (dx, dy).
    pub fn translate(&mut self, dx: f32, dy: f32) {
        self.map_points(|x, y| (x + dx, y + dy));
    }

    /// Aplica uma função a cada coordenada (âncora e alças).
    fn map_points<F: Fn(f32, f32) -> (f32, f32)>(&mut self, f: F) {
        for p in &mut self.points {
            let (x, y) = f(p.x, p.y);
            p.x = x;
            p.y = y;
            if let Some((hx, hy)) = p.hin {
                p.hin = Some(f(hx, hy));
            }
            if let Some((hx, hy)) = p.hout {
                p.hout = Some(f(hx, hy));
            }
        }
    }

    pub fn flip_h(&mut self, w: f32) {
        self.map_points(|x, y| (w - x, y));
    }
    pub fn flip_v(&mut self, h: f32) {
        self.map_points(|x, y| (x, h - y));
    }
    pub fn rotate_180(&mut self, w: f32, h: f32) {
        self.map_points(|x, y| (w - x, h - y));
    }
    pub fn rotate_90_cw(&mut self, old_h: f32) {
        self.map_points(|x, y| (old_h - y, x));
    }
    pub fn rotate_90_ccw(&mut self, old_w: f32) {
        self.map_points(|x, y| (y, old_w - x));
    }
}
