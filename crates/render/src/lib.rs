//! sketchmotion-render — renderização via skia-safe.
//!
//! Etapa 2 (spike): prova que conseguimos desenhar com o skia num buffer de
//! pixels e entregá-lo ao app (egui) como textura. Ainda não desenha o
//! documento real do `core` — apenas uma imagem de teste. É o ponto de maior
//! risco técnico do projeto (ver docs/02-stack-tecnologica.md).

use skia_safe::{surfaces, AlphaType, Color, ColorType, ImageInfo, Paint, Rect};

/// Imagem em pixels, pronta para virar uma textura no egui.
pub struct PixelImage {
    pub width: i32,
    pub height: i32,
    /// Pixels em RGBA8888 (não pré-multiplicado): 4 bytes por pixel.
    pub rgba: Vec<u8>,
}

/// Desenha uma imagem de teste com o skia e devolve os pixels em RGBA.
///
/// Usa uma surface *raster* (em CPU): o skia desenha num buffer de memória
/// que depois é entregue ao egui. É a abordagem mais simples e portável para
/// validar a integração, sem precisar compartilhar contexto de GPU entre as
/// duas bibliotecas. Se o desempenho exigir, migramos para GPU depois — sem
/// mudar quem chama esta função.
pub fn render_test_image(width: i32, height: i32) -> PixelImage {
    // 1) Surface raster (buffer em CPU) no tamanho pedido.
    let mut surface =
        surfaces::raster_n32_premul((width, height)).expect("falha ao criar surface skia");
    let canvas = surface.canvas();

    // 2) Fundo no cinza do canvas do design system (#1B1B1B).
    canvas.clear(Color::from_argb(0xFF, 0x1B, 0x1B, 0x1B));

    // 3) Retângulo arredondado no azul de accent (#2F84FE).
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_color(Color::from_argb(0xFF, 0x2F, 0x84, 0xFE));
    let margin = 40.0;
    let rect = Rect::from_xywh(
        margin,
        margin,
        width as f32 - margin * 2.0,
        height as f32 - margin * 2.0,
    );
    canvas.draw_round_rect(rect, 16.0, 16.0, &paint);

    // 4) Um círculo na cor de seleção (#2FD4FE), só para ter duas formas.
    paint.set_color(Color::from_argb(0xFF, 0x2F, 0xD4, 0xFE));
    canvas.draw_circle((width as f32 / 2.0, height as f32 / 2.0), 60.0, &paint);

    // 5) Lê os pixels da surface em RGBA8888 não pré-multiplicado — formato
    //    que o egui espera para montar a textura.
    let info = ImageInfo::new((width, height), ColorType::RGBA8888, AlphaType::Unpremul, None);
    let row_bytes = (width * 4) as usize;
    let mut rgba = vec![0u8; row_bytes * height as usize];
    let ok = surface.read_pixels(&info, &mut rgba, row_bytes, (0, 0));
    assert!(ok, "falha ao ler os pixels da surface skia");

    PixelImage { width, height, rgba }
}
