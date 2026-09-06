//! sketchmotion-color — paletas nomeadas por personagem e utilidades de cor.
//!
//! Parte A da feature de paletas: só a estrutura de dados e a conversão hex,
//! testáveis sem UI. O seletor de cores, o painel de gerenciamento e a
//! persistência (biblioteca global + projeto) vêm nas partes B, C e D.

mod hex;
mod palette;

pub use hex::{from_hex, to_hex, to_hex_rgba};
pub use palette::{CharacterPalette, NamedColor, PaletteGroup, PaletteLibrary};

#[cfg(test)]
mod tests {
    use super::*;
    use sketchmotion_core::Color;

    #[test]
    fn hex_ida_e_volta() {
        let c = Color::rgb(0xD1, 0x8A, 0x62);
        assert_eq!(to_hex(c), "#D18A62");
        assert_eq!(from_hex("#D18A62"), Some(c));
        assert_eq!(from_hex("d18a62"), Some(c)); // sem # e minúsculo
    }

    #[test]
    fn hex_curto_e_invalido() {
        assert_eq!(from_hex("#FFF"), Some(Color::rgb(255, 255, 255)));
        assert_eq!(from_hex("#12"), None);
        assert_eq!(from_hex("xyz"), None);
    }

    #[test]
    fn montar_paleta_de_personagem() {
        let mut lib = PaletteLibrary::new();
        let p = lib.add_character("Personagem N1");
        let personagem = &mut lib.characters[p];
        let g = personagem.add_group("Pele");
        personagem.groups[g].add_color("base", Color::rgb(0xD1, 0x8A, 0x62));
        personagem.groups[g].add_color("luz", Color::rgb(0xFF, 0xA8, 0x78));
        personagem.groups[g].add_color("sombra", Color::rgb(0x8B, 0x57, 0x2A));

        assert_eq!(lib.characters.len(), 1);
        assert_eq!(lib.characters[0].groups.len(), 1);
        assert_eq!(lib.characters[0].groups[0].name, "Pele");
        assert_eq!(lib.characters[0].groups[0].colors.len(), 3);
        assert_eq!(lib.characters[0].groups[0].colors[1].label, "luz");
    }
}
