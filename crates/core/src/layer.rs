//! Camada: um buffer de pixels RGBA do tamanho do documento.
//!
//! Na v0.1 o desenho vai direto na camada. Frames (animação) entram na v0.3 e
//! opacidade/bloqueio/grupos na v0.2 — a struct já reserva `visible` para isso.

use crate::color::Color;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layer {
    pub name: String,
    pub visible: bool,
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
}
