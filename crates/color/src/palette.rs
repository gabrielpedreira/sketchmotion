//! Estrutura de paletas por personagem.
//!
//! Hierarquia: `PaletteLibrary` (acervo reutilizável) contém vários
//! `CharacterPalette` (um por personagem); cada personagem tem vários
//! `PaletteGroup` (áreas: pele, roupa, cabelo...); cada grupo tem vários
//! `NamedColor` (cores com rótulo livre: base, luz, sombra...).
//!
//! Tudo serializável (serde) para poder ser salvo tanto no projeto quanto numa
//! biblioteca global reutilizável entre projetos.

use serde::{Deserialize, Serialize};
use sketchmotion_core::Color;

/// Uma cor com um rótulo livre (ex.: "base", "luz", "sombra").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedColor {
    pub label: String,
    pub color: Color,
}

impl NamedColor {
    pub fn new(label: impl Into<String>, color: Color) -> Self {
        Self {
            label: label.into(),
            color,
        }
    }
}

/// Uma área do personagem (ex.: "Pele") e suas cores.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PaletteGroup {
    pub name: String,
    pub colors: Vec<NamedColor>,
}

impl PaletteGroup {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            colors: Vec::new(),
        }
    }

    pub fn add_color(&mut self, label: impl Into<String>, color: Color) {
        self.colors.push(NamedColor::new(label, color));
    }

    pub fn remove_color(&mut self, index: usize) {
        if index < self.colors.len() {
            self.colors.remove(index);
        }
    }
}

/// Todas as áreas/cores de um personagem.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CharacterPalette {
    pub name: String,
    pub groups: Vec<PaletteGroup>,
}

impl CharacterPalette {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            groups: Vec::new(),
        }
    }

    /// Cria um grupo novo e devolve o índice dele.
    pub fn add_group(&mut self, name: impl Into<String>) -> usize {
        self.groups.push(PaletteGroup::new(name));
        self.groups.len() - 1
    }

    pub fn remove_group(&mut self, index: usize) {
        if index < self.groups.len() {
            self.groups.remove(index);
        }
    }
}

/// Acervo reutilizável de personagens (a "biblioteca" de paletas).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PaletteLibrary {
    pub characters: Vec<CharacterPalette>,
}

impl PaletteLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adiciona um personagem novo e devolve o índice dele.
    pub fn add_character(&mut self, name: impl Into<String>) -> usize {
        self.characters.push(CharacterPalette::new(name));
        self.characters.len() - 1
    }

    pub fn remove_character(&mut self, index: usize) {
        if index < self.characters.len() {
            self.characters.remove(index);
        }
    }
}
