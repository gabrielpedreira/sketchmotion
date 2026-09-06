//! Tipo de cor primitivo do modelo (RGBA, 8 bits por canal).
//!
//! Vive no `core` porque é um dado fundamental do documento. O crate `color`
//! (separado) cuidará de paleta e seleção de cores — lógica de UI —, não deste
//! tipo básico. Assim o `core` continua sem depender de ninguém.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const TRANSPARENT: Color = Color::rgba(0, 0, 0, 0);
    pub const WHITE: Color = Color::rgb(255, 255, 255);
    pub const BLACK: Color = Color::rgb(0, 0, 0);
}
