//! SketchMotion — binário da aplicação (egui/eframe).
//!
//! Etapa 2 (spike): abre uma janela egui e mostra, como textura, uma imagem
//! desenhada pelo skia (crate `render`). Objetivo: provar o pipeline
//! skia -> buffer de pixels -> textura egui antes de construir qualquer
//! funcionalidade em cima disso.

use eframe::egui;
use sketchmotion_render::{render_test_image, PixelImage};

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "SketchMotion — spike skia + egui",
        options,
        Box::new(|_cc| Ok(Box::new(SketchMotionApp::default()))),
    )
}

#[derive(Default)]
struct SketchMotionApp {
    /// Textura gerada a partir do desenho do skia (criada uma única vez).
    texture: Option<egui::TextureHandle>,
}

impl eframe::App for SketchMotionApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Na primeira frame, pede ao skia para desenhar e sobe como textura.
        let texture = self.texture.get_or_insert_with(|| {
            let PixelImage { width, height, rgba } = render_test_image(820, 480);
            let image =
                egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba);
            ctx.load_texture("skia_test", image, egui::TextureOptions::LINEAR)
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Spike: skia-safe desenhando dentro do egui");
            ui.label(
                "A imagem abaixo foi desenhada pelo skia (fundo + retângulo + \
                 círculo) e está sendo exibida como textura no egui.",
            );
            ui.add_space(12.0);
            ui.image(egui::load::SizedTexture::new(texture.id(), texture.size_vec2()));
        });
    }
}
