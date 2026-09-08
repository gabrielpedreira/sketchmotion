//! sketchmotion-io — salvar, carregar e (futuramente) exportar.
//!
//! Persiste dois tipos de coisa:
//! - O **projeto** (`Document`) no formato `.sketchmotion` (CBOR).
//! - A **biblioteca de paletas** (`PaletteLibrary`) num arquivo `.smpalette`,
//!   guardado no diretório de dados do usuário para ser reutilizado entre
//!   projetos.
//!
//! Erros são devolvidos como String por ora.

use sketchmotion_color::PaletteLibrary;
use sketchmotion_core::Document;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

/// Extensão do formato de projeto.
pub const PROJECT_EXTENSION: &str = "sketchmotion";
/// Extensão do arquivo de biblioteca de paletas.
pub const LIBRARY_EXTENSION: &str = "smpalette";

// ---------- Projeto ----------

/// Salva o documento no caminho dado (formato `.sketchmotion` / CBOR).
pub fn save(doc: &Document, path: &Path) -> Result<(), String> {
    let file = File::create(path).map_err(|e| e.to_string())?;
    ciborium::into_writer(doc, BufWriter::new(file)).map_err(|e| e.to_string())
}

/// Carrega um documento a partir de um arquivo `.sketchmotion`.
pub fn load(path: &Path) -> Result<Document, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut doc: Document =
        ciborium::from_reader(BufReader::new(file)).map_err(|e| e.to_string())?;
    doc.normalize();
    Ok(doc)
}

// ---------- Biblioteca global de paletas ----------

/// Caminho padrão da biblioteca global de paletas
/// (ex.: `%APPDATA%/SketchMotion/paletas.smpalette` no Windows).
pub fn default_library_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("SketchMotion").join("paletas.smpalette"))
}

/// Salva a biblioteca de paletas no caminho dado (cria as pastas se preciso).
pub fn save_library(lib: &PaletteLibrary, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let file = File::create(path).map_err(|e| e.to_string())?;
    ciborium::into_writer(lib, BufWriter::new(file)).map_err(|e| e.to_string())
}

/// Carrega a biblioteca de paletas de um arquivo `.smpalette`.
pub fn load_library(path: &Path) -> Result<PaletteLibrary, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    ciborium::from_reader(BufReader::new(file)).map_err(|e| e.to_string())
}

/// Exporta um buffer RGBA como imagem (PNG/JPEG conforme a extensão do caminho).
pub fn export_png(width: u32, height: u32, rgba: &[u8], path: &Path) -> Result<(), String> {
    image::save_buffer(
        path,
        rgba,
        width,
        height,
        image::ExtendedColorType::Rgba8,
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sketchmotion_color::PaletteLibrary;
    use sketchmotion_core::{Color, Document};

    #[test]
    fn salvar_e_carregar_projeto_preserva_o_desenho() {
        let mut doc = Document::new(8, 8, Color::WHITE);
        doc.layer_mut(0).unwrap().set_pixel(3, 4, Color::rgb(10, 20, 30));

        let path = std::env::temp_dir().join("sketchmotion_teste.sketchmotion");
        save(&doc, &path).unwrap();
        let carregado = load(&path).unwrap();

        assert_eq!(carregado.width, 8);
        assert_eq!(
            carregado.layer(0).unwrap().get_pixel(3, 4),
            Some(Color::rgb(10, 20, 30))
        );
    }

    #[test]
    fn salvar_e_carregar_biblioteca_preserva_paletas() {
        let mut lib = PaletteLibrary::new();
        let ci = lib.add_character("Personagem N1");
        let gi = lib.characters[ci].add_group("Pele");
        lib.characters[ci].groups[gi].add_color("base", Color::rgb(0xD1, 0x8A, 0x62));

        let path = std::env::temp_dir().join("sketchmotion_teste.smpalette");
        save_library(&lib, &path).unwrap();
        let carregada = load_library(&path).unwrap();

        assert_eq!(carregada.characters.len(), 1);
        assert_eq!(carregada.characters[0].groups[0].colors[0].label, "base");
    }
}
