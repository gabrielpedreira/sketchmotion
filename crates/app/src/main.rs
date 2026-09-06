//! SketchMotion — binário da aplicação (egui/eframe).
//!
//! Paletas, Parte C: janela de gerenciamento das paletas por personagem —
//! criar personagem, áreas (Pele, Roupa...), adicionar cores nomeadas e clicar
//! numa cor da paleta para usá-la como pincel.

use eframe::egui;
use sketchmotion_color::PaletteLibrary;
use sketchmotion_core::{Color, Document};
use sketchmotion_render::{render_document, PixelImage};
use sketchmotion_tools::Tool;

const CANVAS_W: u32 = 800;
const CANVAS_H: u32 = 520;

const SWATCHES: [egui::Color32; 8] = [
    egui::Color32::BLACK,
    egui::Color32::WHITE,
    egui::Color32::from_rgb(0xFF, 0x5C, 0x5C),
    egui::Color32::from_rgb(0x2F, 0xB3, 0x74),
    egui::Color32::from_rgb(0x2F, 0x84, 0xFE),
    egui::Color32::from_rgb(0xF5, 0xA6, 0x23),
    egui::Color32::from_rgb(0x9B, 0x51, 0xE0),
    egui::Color32::from_rgb(0x8B, 0x57, 0x2A),
];

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1120.0, 760.0]),
        ..Default::default()
    };
    eframe::run_native(
        "SketchMotion",
        options,
        Box::new(|_cc| Ok(Box::new(SketchMotionApp::new()))),
    )
}

fn to_color32(c: Color) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

struct SketchMotionApp {
    document: Document,
    texture: Option<egui::TextureHandle>,
    dirty: bool,
    last_pos: Option<(i32, i32)>,
    tool: Tool,
    brush_color: egui::Color32,
    brush_radius: i32,
    hex_input: String,
    status: String,
    // --- paletas ---
    library: PaletteLibrary,
    palettes_open: bool,
    selected_char: Option<usize>,
    new_char_name: String,
    new_group_name: String,
    new_color_label: String,
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
            hex_input: String::new(),
            status: String::new(),
            library: PaletteLibrary::new(),
            palettes_open: false,
            selected_char: None,
            new_char_name: String::new(),
            new_group_name: String::new(),
            new_color_label: String::new(),
        }
    }

    fn brush_core_color(&self) -> Color {
        let c = self.brush_color;
        Color::rgba(c.r(), c.g(), c.b(), c.a())
    }

    fn active_color(&self) -> Color {
        self.tool.effective_color(self.brush_core_color())
    }

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

    fn salvar(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("SketchMotion", &[sketchmotion_io::PROJECT_EXTENSION])
            .set_file_name("desenho.sketchmotion")
            .save_file()
        {
            self.status = match sketchmotion_io::save(&self.document, &path) {
                Ok(()) => format!("Salvo em {}", path.display()),
                Err(e) => format!("Erro ao salvar: {e}"),
            };
        }
    }

    fn abrir(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("SketchMotion", &[sketchmotion_io::PROJECT_EXTENSION])
            .pick_file()
        {
            match sketchmotion_io::load(&path) {
                Ok(doc) => {
                    self.document = doc;
                    self.last_pos = None;
                    self.dirty = true;
                    self.status = format!("Aberto: {}", path.display());
                }
                Err(e) => self.status = format!("Erro ao abrir: {e}"),
            }
        }
    }

    fn ui_cor(&mut self, ui: &mut egui::Ui) {
        ui.heading("Cor");
        ui.horizontal(|ui| {
            ui.color_edit_button_srgba(&mut self.brush_color);
            ui.monospace(sketchmotion_color::to_hex(self.brush_core_color()));
        });
        ui.horizontal(|ui| {
            ui.label("Código:");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.hex_input)
                    .desired_width(84.0)
                    .hint_text("#RRGGBB"),
            );
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button("Ir").clicked() || enter) && !self.hex_input.trim().is_empty() {
                match sketchmotion_color::from_hex(&self.hex_input) {
                    Some(c) => {
                        self.brush_color = to_color32(c);
                        self.tool = Tool::Pencil;
                        self.status = format!("Cor {} selecionada", sketchmotion_color::to_hex(c));
                    }
                    None => self.status = "Código hex inválido".to_owned(),
                }
            }
        });
        ui.add_space(4.0);
        ui.label("Paleta rápida:");
        egui::Grid::new("swatches").spacing([4.0, 4.0]).show(ui, |ui| {
            for (i, cor) in SWATCHES.iter().enumerate() {
                let btn = egui::Button::new("").fill(*cor).min_size(egui::vec2(24.0, 24.0));
                if ui.add(btn).clicked() {
                    self.brush_color = *cor;
                    self.tool = Tool::Pencil;
                }
                if (i + 1) % 4 == 0 {
                    ui.end_row();
                }
            }
        });
    }

    /// Janela de gerenciamento das paletas por personagem.
    fn paletas_window(&mut self, ctx: &egui::Context) {
        let mut open = self.palettes_open;
        let current = self.brush_core_color();

        // Ações adiadas (para não mutar a biblioteca durante a iteração dela).
        let mut pick: Option<Color> = None;
        let mut remove_group: Option<usize> = None;
        let mut add_to_group: Option<(usize, String)> = None;
        let mut remove_char = false;

        egui::Window::new("Paletas de personagem")
            .open(&mut open)
            .default_width(300.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_char_name)
                            .desired_width(160.0)
                            .hint_text("Nome do personagem"),
                    );
                    if ui.button("Novo").clicked() && !self.new_char_name.trim().is_empty() {
                        let idx = self.library.add_character(self.new_char_name.trim());
                        self.selected_char = Some(idx);
                        self.new_char_name.clear();
                    }
                });

                if self.library.characters.is_empty() {
                    ui.label("Nenhum personagem ainda — crie um acima.");
                    return;
                }

                let sel_name = self
                    .selected_char
                    .and_then(|i| self.library.characters.get(i))
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| "—".to_owned());
                egui::ComboBox::from_label("Personagem")
                    .selected_text(sel_name)
                    .show_ui(ui, |ui| {
                        for (i, c) in self.library.characters.iter().enumerate() {
                            ui.selectable_value(&mut self.selected_char, Some(i), c.name.as_str());
                        }
                    });

                let Some(ci) = self.selected_char else {
                    return;
                };
                if ci >= self.library.characters.len() {
                    return;
                }

                if ui.button("Remover personagem").clicked() {
                    remove_char = true;
                }
                ui.separator();

                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_group_name)
                            .desired_width(160.0)
                            .hint_text("Nova área: Pele, Roupa..."),
                    );
                    if ui.button("Adicionar área").clicked()
                        && !self.new_group_name.trim().is_empty()
                    {
                        self.library.characters[ci].add_group(self.new_group_name.trim());
                        self.new_group_name.clear();
                    }
                });

                egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
                    for (gi, group) in self.library.characters[ci].groups.iter().enumerate() {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.strong(&group.name);
                                if ui.small_button("remover área").clicked() {
                                    remove_group = Some(gi);
                                }
                            });
                            for nc in &group.colors {
                                ui.horizontal(|ui| {
                                    let btn = egui::Button::new("")
                                        .fill(to_color32(nc.color))
                                        .min_size(egui::vec2(22.0, 22.0));
                                    if ui.add(btn).clicked() {
                                        pick = Some(nc.color);
                                    }
                                    ui.label(format!(
                                        "{} — {}",
                                        nc.label,
                                        sketchmotion_color::to_hex(nc.color)
                                    ));
                                });
                            }
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.new_color_label)
                                        .desired_width(110.0)
                                        .hint_text("rótulo"),
                                );
                                if ui.button("+ cor atual").clicked() {
                                    let label = if self.new_color_label.trim().is_empty() {
                                        "cor".to_owned()
                                    } else {
                                        self.new_color_label.trim().to_owned()
                                    };
                                    add_to_group = Some((gi, label));
                                }
                            });
                        });
                    }
                });
            });

        // Aplica as ações adiadas.
        if let Some(ci) = self.selected_char {
            if ci < self.library.characters.len() {
                if let Some((gi, label)) = add_to_group {
                    if gi < self.library.characters[ci].groups.len() {
                        self.library.characters[ci].groups[gi].add_color(label, current);
                        self.new_color_label.clear();
                    }
                }
                if let Some(gi) = remove_group {
                    self.library.characters[ci].remove_group(gi);
                }
                if remove_char {
                    self.library.remove_character(ci);
                    self.selected_char =
                        if self.library.characters.is_empty() { None } else { Some(0) };
                }
            }
        }
        if let Some(c) = pick {
            self.brush_color = to_color32(c);
            self.tool = Tool::Pencil;
            self.status = format!("Cor {} da paleta", sketchmotion_color::to_hex(c));
        }
        self.palettes_open = open;
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
        let size = egui::vec2(self.document.width as f32, self.document.height as f32);

        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.strong("SketchMotion");
                ui.separator();
                if ui.button("Salvar").clicked() {
                    self.salvar();
                }
                if ui.button("Abrir").clicked() {
                    self.abrir();
                }
                if ui.button("Paletas").clicked() {
                    self.palettes_open = !self.palettes_open;
                }
                if !self.status.is_empty() {
                    ui.separator();
                    ui.label(&self.status);
                }
            });
        });

        egui::SidePanel::right("painel").min_width(190.0).show(ctx, |ui| {
            ui.add_space(6.0);
            ui.heading("Ferramentas");
            ui.selectable_value(&mut self.tool, Tool::Pencil, "Lápis");
            ui.selectable_value(&mut self.tool, Tool::Eraser, "Borracha");

            ui.separator();
            ui.heading("Pincel");
            ui.add(egui::Slider::new(&mut self.brush_radius, 1..=30).text("Tamanho"));

            ui.separator();
            self.ui_cor(ui);

            ui.separator();
            if ui.button("Limpar tudo").clicked() {
                self.document = Document::new(CANVAS_W, CANVAS_H, Color::WHITE);
                self.last_pos = None;
                self.dirty = true;
            }
        });

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

        self.paletas_window(ctx);
    }
}
