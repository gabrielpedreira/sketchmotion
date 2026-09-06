//! Conversão entre `Color` e código hexadecimal ("#RRGGBB", "#RRGGBBAA" ou
//! "#RGB"). Usado para exibir o código de cada cor e para entrada por código.

use sketchmotion_core::Color;

/// Código "#RRGGBB" (sem alpha) — o formato que aparece nas paletas.
pub fn to_hex(c: Color) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b)
}

/// Código "#RRGGBBAA" (com alpha), quando a transparência importa.
pub fn to_hex_rgba(c: Color) -> String {
    format!("#{:02X}{:02X}{:02X}{:02X}", c.r, c.g, c.b, c.a)
}

/// Interpreta um código hex flexível: aceita com/sem "#", e nos formatos
/// RGB (3), RRGGBB (6) e RRGGBBAA (8). Devolve `None` se for inválido.
pub fn from_hex(s: &str) -> Option<Color> {
    let s = s.trim().trim_start_matches('#');
    let byte = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    match s.len() {
        3 => {
            // #RGB -> cada dígito é duplicado (F -> FF).
            let nib = |i: usize| u8::from_str_radix(&s[i..i + 1], 16).ok();
            let (r, g, b) = (nib(0)?, nib(1)?, nib(2)?);
            Some(Color::rgb(r * 17, g * 17, b * 17))
        }
        6 => Some(Color::rgb(byte(0)?, byte(2)?, byte(4)?)),
        8 => Some(Color::rgba(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
        _ => None,
    }
}
