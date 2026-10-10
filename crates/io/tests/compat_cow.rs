//! Garante que o formato de arquivo não mudou com as camadas compartilhadas
//! (Arc) e que editar uma cópia não altera o original (base do desfazer).
use serde::{Deserialize, Serialize};
use sketchmotion_core::{Color, Document, Layer};

/// Espelho do formato ANTIGO da camada (pixels: Vec<u8>).
#[derive(Serialize, Deserialize)]
struct LayerAntiga {
    name: String,
    visible: bool,
    locked: bool,
    opacity: f32,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

#[test]
fn arquivo_antigo_continua_abrindo() {
    let antiga = LayerAntiga {
        name: "c".into(),
        visible: true,
        locked: false,
        opacity: 1.0,
        width: 2,
        height: 1,
        pixels: vec![10, 20, 30, 255, 1, 2, 3, 4],
    };
    let mut buf = Vec::new();
    ciborium::into_writer(&antiga, &mut buf).unwrap();
    let nova: Layer = ciborium::from_reader(&buf[..]).unwrap();
    assert_eq!(nova.get_pixel(0, 0).unwrap().r, 10);
    assert_eq!(nova.get_pixel(1, 0).unwrap().a, 4);
    let mut buf2 = Vec::new();
    ciborium::into_writer(&nova, &mut buf2).unwrap();
    assert_eq!(buf, buf2, "o formato gravado deve ser idêntico ao antigo");
}

#[test]
fn editar_copia_nao_altera_original() {
    let mut a = Layer::new("a", 4, 4);
    a.set_pixel(1, 1, Color::rgba(9, 9, 9, 255));
    let mut b = a.clone();
    b.set_pixel(1, 1, Color::rgba(200, 0, 0, 255));
    b.flip_h();
    assert_eq!(a.get_pixel(1, 1).unwrap().r, 9);
    assert_eq!(b.get_pixel(2, 1).unwrap().r, 200);
}

#[test]
fn documento_salva_e_abre_igual() {
    let mut d = Document::new(8, 8, Color::TRANSPARENT);
    d.layers[0].set_pixel(3, 4, Color::rgba(1, 2, 3, 255));
    d.sync_to_frames();
    let p = std::env::temp_dir().join("sm_compat_test.sketchmotion");
    sketchmotion_io::save(&d, &p).unwrap();
    let e = sketchmotion_io::load(&p).unwrap();
    assert_eq!(e.layers[0].get_pixel(3, 4).unwrap().g, 2);
}
