//! SketchMotion — binário da aplicação (egui/eframe).
//!
//! Etapa 4 (v0.1): canvas interativo. Mantém um `Document` do core, desenha
//! nele com o mouse (arrastar pinta um traço) e re-renderiza a textura pelo
//! render a cada mudança. Ainda com uma cor/pincel fixos — alternância de
//! ferramentas (lápis/borracha) e cor entram nas Etapas 5 e 6.

use eframe::egui;
use sketchmotion_core::{Color, Document};
use sketchmotion_render::{render_document, PixelImage};

const CANVAS_W: u32 = 800;
const CANVAS_H: u32 = 520;
const BRUSH_RADIUS: i32 = 2; // raio do pincel, em pixels

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([980.0, 700.0]),
        ..Default::default()
    };
    eframe::run_native(
        "SketchMotion",
        options,
        Box::new(|_cc| Ok(Box::new(SketchMotionApp::new()))),
    )
}

struct SketchMotionApp {
    document: Document,
    texture: Option<egui::TextureHandle>,
    /// Marca que o desenho mudou e a textura precisa ser refeita.
    dirty: bool,
    /// Último ponto pintado (para ligar os pontos do arrasto).
    last_pos: Option<(i32, i32)>,
    brush_color: Color,
}

impl SketchMotionApp {
    fn new() -> Self {
        Self {
            document: Document::new(CANVAS_W, CANVAS_H, Color::WHITE),
            texture: None,
            dirty: true,
            last_pos: None,
            brush_color: Color::BLACK,
        }
    }

    /// Pinta um "carimbo" circular do pincel centrado em (x, y).
    fn paint_dab(&mut self, x: i32, y: i32) {
        let color = self.brush_color;
        if let Some(layer) = self.document.layer_mut(0) {
            let r = BRUSH_RADIUS;
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx * dx + dy * dy <= r * r {
                        let (px, py) = (x + dx, y + dy);
                        if px >= 0 && py >= 0 {
                            layer.set_pixel(px as u32, py as u32, color);
                        }
                    }
                }
            }
        }
        self.dirty = true;
    }

    /// Liga dois pontos com carimbos para o traço não sair tracejado.
    fn paint_line(&mut self, from: (i32, i32), to: (i32, i32)) {
        let (x0, y0) = from;
        let (x1, y1) = to;
        let steps = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let x = (x0 as f32 + (x1 - x0) as f32 * t).round() as i32;
            let y = (y0 as f32 + (y1 - y0) as f32 * t).round() as i32;
            self.paint_dab(x, y);
        }
    }
}

impl eframe::App for SketchMotionApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // (Re)constrói a textura do documento quando algo mudou.
        if self.dirty || self.texture.is_none() {
            let PixelImage { width, height, rgba } = render_document(&self.document);
            let image =
                egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba);
            match &mut self.texture {
                Some(tex) => tex.set(image, egui::TextureOptions::NEAREST),
                None => {
                    self.texture =
                        Some(ctx.load_texture("canvas", image, egui::TextureOptions::NEAREST))
                }
            }
            self.dirty = false;
        }
        let tex_id = self.texture.as_ref().unwrap().id();
        let size = egui::vec2(CANVAS_W as f32, CANVAS_H as f32);

        egui::TopBottomPanel::top("barra").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.strong("SketchMotion — v0.1");
                ui.separator();
                ui.label("Arraste o mouse sobre o canvas para desenhar.");
                if ui.button("Limpar").clicked() {
                    self.document = Document::new(CANVAS_W, CANVAS_H, Color::WHITE);
                    self.last_pos = None;
                    self.dirty = true;
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let image = egui::Image::from_texture(egui::load::SizedTexture::new(tex_id, size))
                .fit_to_exact_size(size)
                .sense(egui::Sense::click_and_drag());
            let response = ui.add(image);

            // Enquanto o ponteiro estiver pressionado sobre o canvas, pinta.
            if let Some(pointer) = response.interact_pointer_pos() {
                let local = pointer - response.rect.min; // deslocamento dentro do widget
                let p = (local.x.round() as i32, local.y.round() as i32);
                match self.last_pos {
                    Some(prev) => self.paint_line(prev, p),
                    None => self.paint_dab(p.0, p.1),
                }
                self.last_pos = Some(p);
            } else {
                self.last_pos = None; // soltou o botão: recomeça o traço
            }
        });
    }
}
