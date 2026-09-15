//! Biblioteca de "objetos" (peças) reutilizáveis: recortes RGBA salvos pelo
//! usuário (cabeças, braços, poses, personagens…) para reaproveitar em qualquer
//! trabalho, como uma função colar. Persistida entre projetos (ver crate io).

use serde::{Deserialize, Serialize};

/// Uma peça salva: nome + imagem RGBA (largura × altura).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ObjectPiece {
    pub name: String,
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// Coleção de peças salvas.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PieceLibrary {
    pub pieces: Vec<ObjectPiece>,
}

impl PieceLibrary {
    pub fn new() -> Self {
        Self { pieces: Vec::new() }
    }

    /// Adiciona uma peça e devolve o índice dela.
    pub fn add(&mut self, name: impl Into<String>, w: u32, h: u32, rgba: Vec<u8>) -> usize {
        self.pieces.push(ObjectPiece {
            name: name.into(),
            w,
            h,
            rgba,
        });
        self.pieces.len() - 1
    }

    /// Remove a peça de índice `i` (ignora índice inválido).
    pub fn remove(&mut self, i: usize) {
        if i < self.pieces.len() {
            self.pieces.remove(i);
        }
    }
}
