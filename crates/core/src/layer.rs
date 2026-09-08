//! Camada: um buffer de pixels RGBA do tamanho do documento.
//!
//! Na v0.1 o desenho vai direto na camada. Frames (animação) entram na v0.3 e
//! opacidade/bloqueio/grupos na v0.2 — a struct já reserva `visible` para isso.

use crate::color::Color;
use serde::{Deserialize, Serialize};

fn default_opacity() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layer {
    pub name: String,
    pub visible: bool,
    /// Camada bloqueada não recebe desenho.
    #[serde(default)]
    pub locked: bool,
    /// Opacidade da camada (0.0–1.0). Multiplica o alpha na composição.
    #[serde(default = "default_opacity")]
    opacity: f32,
    width: u32,
    height: u32,
    /// Pixels RGBA, linha a linha: width * height * 4 bytes.
    pixels: Vec<u8>,
}

impl Layer {
    /// Cria uma camada transparente do tamanho dado.
    pub fn new(name: impl Into<String>, width: u32, height: u32) -> Self {
        Self {
            name: name.into(),
            visible: true,
            locked: false,
            opacity: 1.0,
            width,
            height,
            pixels: vec![0; (width as usize) * (height as usize) * 4],
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Índice do byte R do pixel (x, y), se estiver dentro dos limites.
    fn index(&self, x: u32, y: u32) -> Option<usize> {
        if x < self.width && y < self.height {
            Some(((y * self.width + x) * 4) as usize)
        } else {
            None
        }
    }

    /// Pinta um pixel (substitui, sem blending — suficiente para a v0.1).
    pub fn set_pixel(&mut self, x: u32, y: u32, color: Color) {
        if let Some(i) = self.index(x, y) {
            self.pixels[i] = color.r;
            self.pixels[i + 1] = color.g;
            self.pixels[i + 2] = color.b;
            self.pixels[i + 3] = color.a;
        }
    }

    /// Lê a cor de um pixel, se estiver dentro dos limites.
    pub fn get_pixel(&self, x: u32, y: u32) -> Option<Color> {
        let i = self.index(x, y)?;
        Some(Color::rgba(
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ))
    }

    pub fn opacity(&self) -> f32 {
        self.opacity
    }

    pub fn set_opacity(&mut self, o: f32) {
        self.opacity = o.clamp(0.0, 1.0);
    }

    /// Espelha o conteúdo horizontalmente (dimensões inalteradas).
    pub fn flip_h(&mut self) {
        let w = self.width as usize;
        let h = self.height as usize;
        for y in 0..h {
            for x in 0..w / 2 {
                let a = (y * w + x) * 4;
                let b = (y * w + (w - 1 - x)) * 4;
                for k in 0..4 {
                    self.pixels.swap(a + k, b + k);
                }
            }
        }
    }

    /// Espelha o conteúdo verticalmente (dimensões inalteradas).
    pub fn flip_v(&mut self) {
        let w = self.width as usize;
        let h = self.height as usize;
        let stride = w * 4;
        for y in 0..h / 2 {
            let top = y * stride;
            let bot = (h - 1 - y) * stride;
            for i in 0..stride {
                self.pixels.swap(top + i, bot + i);
            }
        }
    }

    /// Gira 180° (dimensões inalteradas).
    pub fn rotate_180(&mut self) {
        let n = (self.width as usize) * (self.height as usize);
        for i in 0..n / 2 {
            let a = i * 4;
            let b = (n - 1 - i) * 4;
            for k in 0..4 {
                self.pixels.swap(a + k, b + k);
            }
        }
    }

    /// Gira 90° no sentido horário (troca largura/altura).
    pub fn rotate_90_cw(&mut self) {
        let ow = self.width as usize;
        let oh = self.height as usize;
        let (nw, nh) = (oh, ow);
        let mut np = vec![0u8; nw * nh * 4];
        for oy in 0..oh {
            for ox in 0..ow {
                let nx = oh - 1 - oy;
                let ny = ox;
                let si = (oy * ow + ox) * 4;
                let di = (ny * nw + nx) * 4;
                np[di..di + 4].copy_from_slice(&self.pixels[si..si + 4]);
            }
        }
        self.pixels = np;
        self.width = nw as u32;
        self.height = nh as u32;
    }

    /// Gira 90° no sentido anti-horário (troca largura/altura).
    pub fn rotate_90_ccw(&mut self) {
        let ow = self.width as usize;
        let oh = self.height as usize;
        let (nw, nh) = (oh, ow);
        let mut np = vec![0u8; nw * nh * 4];
        for oy in 0..oh {
            for ox in 0..ow {
                let nx = oy;
                let ny = ow - 1 - ox;
                let si = (oy * ow + ox) * 4;
                let di = (ny * nw + nx) * 4;
                np[di..di + 4].copy_from_slice(&self.pixels[si..si + 4]);
            }
        }
        self.pixels = np;
        self.width = nw as u32;
        self.height = nh as u32;
    }
}
