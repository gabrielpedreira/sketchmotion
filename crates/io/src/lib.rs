//! sketchmotion-io — salvar, carregar e (futuramente) exportar.
//!
//! Etapa 7 (v0.1): formato próprio `.sketchmotion`, serializado em CBOR
//! (binário compacto) via serde + ciborium, preservando todo o Document
//! (dimensões, fundo e camadas). Exportadores de imagem (PNG/GIF) entram na
//! v0.4. Os erros são devolvidos como String por ora — um tipo de erro
//! dedicado pode vir depois, se necessário.

use sketchmotion_core::Document;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;

/// Extensão do formato de projeto.
pub const PROJECT_EXTENSION: &str = "sketchmotion";

/// Salva o documento no caminho dado (formato `.sketchmotion` / CBOR).
pub fn save(doc: &Document, path: &Path) -> Result<(), String> {
    let file = File::create(path).map_err(|e| e.to_string())?;
    let writer = BufWriter::new(file);
    ciborium::into_writer(doc, writer).map_err(|e| e.to_string())
}

/// Carrega um documento a partir de um arquivo `.sketchmotion`.
pub fn load(path: &Path) -> Result<Document, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let reader = BufReader::new(file);
    ciborium::from_reader(reader).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sketchmotion_core::{Color, Document};

    #[test]
    fn salvar_e_carregar_preserva_o_desenho() {
        let mut doc = Document::new(8, 8, Color::WHITE);
        doc.layer_mut(0).unwrap().set_pixel(3, 4, Color::rgb(10, 20, 30));

        let path = std::env::temp_dir().join("sketchmotion_teste.sketchmotion");
        save(&doc, &path).unwrap();
        let carregado = load(&path).unwrap();

        assert_eq!(carregado.width, 8);
        assert_eq!(carregado.layers.len(), 1);
        assert_eq!(
            carregado.layer(0).unwrap().get_pixel(3, 4),
            Some(Color::rgb(10, 20, 30))
        );
    }
}
