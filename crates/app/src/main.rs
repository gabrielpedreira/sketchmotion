//! SketchMotion — binário da aplicação (egui/eframe).
//!
//! Etapas 5 e 6 (v0.1): ferramentas lápis/borracha, seleção de cor e tamanho
//! de pincel, num painel lateral. O desenho continua indo para o Document do
//! core; o render compõe; o egui exibe.

use eframe::egui;
use sketchmotion_core::{Color, Document};
use sketchmotion_render::{render_document, PixelImage};
use sketchmotion_tools::Tool;

const CANVAS_W: u32 = 800;
const CANVAS_H: u32 = 520;

/// Cores de acesso rápido no painel (swatches).
const SWATCHES: [egui::Color32; 8] = [
    egui::Color32::BLACK,
    egui::Color32::WHITE,
    egui::Color32::from_rgb(0xFF, 0x5C, 0x5C), // vermelho
    egui::Color32::from_rgb(0x2F, 0xB3, 0x74), // verde
    egui::Color32::from_rgb(0x2F, 0x84, 0xFE), // azul (accent)
    egui::Color32::from_rgb(0xF5, 0xA6, 0x23), // laranja
    egui::Color32::from_rgb(0x9B, 0x51, 0xE0), // roxo
    egui::Color32::from_rgb(0x8B, 0x57, 0x2A), // marrom
];

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1040.0, 720.0]),
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
    dirty: bool,
    last_pos: Option<(i32, i32)>,
    tool: Tool,
    brush_color: egui::Color32,
    brush_radius: i32,
}

impl SketchMotionApp {
    fn new() -> Self {
        Self {
            document: Document::new(CANVAS_W, CANVAS_H, Color::WHITE),
            texture: None,
            dirty: true,
            last_pos: None,
            tool: Tool::Pencil,
            brush_color: egui::Color32::BLACK,
            brush_radius: 2,
        }
    }

    /// Cor do core que a ferramenta atual aplica (borracha => transparente).
    fn active_color(&self) -> Color {
        let c = self.brush_color;
        self.tool
            .effective_color(Color::rgba(c.r(), c.g(), c.b(), c.a()))
    }

    /// Pinta um carimbo circular do pincel centrado em (x, y).
    fn paint_dab(&mut self, x: i32, y: i32) {
        let color = self.active_color();
        let r = self.brush_radius;
        if let Some(layer) = self.document.layer_mut(0) {
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

        // --- Painel lateral direito: ferramentas, pincel e cores ---
        egui::SidePanel::right("painel").min_width(180.0).show(ctx, |ui| {
            ui.add_space(6.0);
            ui.heading("Ferramentas");
            ui.selectable_value(&mut self.tool, Tool::Pencil, "Lápis");
            ui.selectable_value(&mut self.tool, Tool::Eraser, "Borracha");

            ui.separator();
            ui.heading("Pincel");
            ui.add(egui::Slider::new(&mut self.brush_radius, 1..=30).text("Tamanho"));

            ui.separator();
            ui.heading("Cor");
            ui.horizontal(|ui| {
                ui.color_edit_button_srgba(&mut self.brush_color);
                ui.label("Cor atual");
            });
            ui.add_space(4.0);
            ui.label("Paleta:");
            egui::Grid::new("swatches").spacing([4.0, 4.0]).show(ui, |ui| {
                for (i, cor) in SWATCHES.iter().enumerate() {
                    let btn = egui::Button::new("").fill(*cor).min_size(egui::vec2(24.0, 24.0));
                    if ui.add(btn).clicked() {
                        self.brush_color = *cor;
                        self.tool = Tool::Pencil; // escolher cor volta para o lápis
                    }
                    if (i + 1) % 4 == 0 {
                        ui.end_row();
                    }
                }
            });

            ui.separator();
            if ui.button("Limpar tudo").clicked() {
                self.document = Document::new(CANVAS_W, CANVAS_H, Color::WHITE);
                self.last_pos = None;
                self.dirty = true;
            }
        });

        // --- Canvas central ---
        egui::CentralPanel::default().show(ctx, |ui| {
            let image = egui::Image::from_texture(egui::load::SizedTexture::new(tex_id, size))
                .fit_to_exact_size(size)
                .sense(egui::Sense::click_and_drag());
            let response = ui.add(image);

            if let Some(pointer) = response.interact_pointer_pos() {
                let local = pointer - response.rect.min;
                let p = (local.x.round() as i32, local.y.round() as i32);
                match self.last_pos {
                    Some(prev) => self.paint_line(prev, p),
                    None => self.paint_dab(p.0, p.1),
                }
                self.last_pos = Some(p);
            } else {
                self.last_pos = None;
            }
        });
    }
}
