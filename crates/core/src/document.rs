//! Documento: dimensões, cor de fundo e a pilha de camadas.
//!
//! É a raiz do modelo — a "fonte única da verdade" da arquitetura. Toda
//! alteração no desenho acontece aqui; render e io apenas leem este estado.

use crate::color::Color;
use crate::layer::Layer;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub width: u32,
    pub height: u32,
    pub background: Color,
    /// Camadas de baixo (índice 0) para cima (última é a do topo).
    pub layers: Vec<Layer>,
}

impl Document {
    /// Cria um documento novo já com uma camada inicial.
    pub fn new(width: u32, height: u32, background: Color) -> Self {
        let mut doc = Self {
            width,
            height,
            background,
            layers: Vec::new(),
        };
        doc.add_layer("Camada 1");
        doc
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
}
