//! SketchMotion — binário da aplicação (egui/eframe).
//!
//! Paletas, Parte D (biblioteca global): as paletas são carregadas do
//! diretório do usuário quando o app abre e salvas automaticamente sempre que
//! mudam, ficando disponíveis em qualquer projeto.

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
    show_tools: bool,
    show_colors: bool,
    show_palettes: bool,
    library: PaletteLibrary,
    /// A biblioteca mudou e precisa ser salva no disco.
    library_dirty: bool,
    selected_char: Option<usize>,
    new_char_name: String,
    new_group_name: String,
    new_color_label: String,
}

impl SketchMotionApp {
    fn new() -> Self {
        // Carrega a biblioteca global de paletas, se existir.
        let library = sketchmotion_io::default_library_path()
            .and_then(|p| sketchmotion_io::load_library(&p).ok())
            .unwrap_or_default();
        let selected_char = if library.characters.is_empty() { None } else { Some(0) };

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
            show_tools: true,
            show_colors: true,
            show_palettes: true,
            library,
            library_dirty: false,
            selected_char,
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

    /// Persiste a biblioteca global de paletas no disco.
    fn salvar_biblioteca(&mut self) {
        if let Some(path) = sketchmotion_io::default_library_path() {
            if let Err(e) = sketchmotion_io::save_library(&self.library, &path) {
                self.status = format!("Erro ao salvar paletas: {e}");
            }
        }
    }

    fn ui_ferramentas(&mut self, ui: &mut egui::Ui) {
        ui.heading("Ferramentas");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.tool, Tool::Pencil, "Lápis");
            ui.selectable_value(&mut self.tool, Tool::Eraser, "Borracha");
        });
        ui.add(egui::Slider::new(&mut self.brush_radius, 1..=30).text("Tamanho"));
        if ui.button("Limpar tudo").clicked() {
            self.document = Document::new(CANVAS_W, CANVAS_H, Color::WHITE);
            self.last_pos = None;
            self.dirty = true;
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
                let btn = egui::Button::new("").fill(*cor).min_size(egui::vec2(26.0, 26.0));
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

    fn ui_paletas(&mut self, ui: &mut egui::Ui) {
        ui.heading("Paletas");
        ui.label("(salvas automaticamente e reusáveis entre projetos)");
        let current = self.brush_core_color();

        let mut pick: Option<Color> = None;
        let mut remove_group: Option<usize> = None;
        let mut add_to_group: Option<(usize, String)> = None;
        let mut remove_char = false;

        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.new_char_name)
                    .desired_width(140.0)
                    .hint_text("Novo personagem"),
            );
            if ui.button("Criar").clicked() && !self.new_char_name.trim().is_empty() {
                let idx = self.library.add_character(self.new_char_name.trim());
                self.selected_char = Some(idx);
                self.new_char_name.clear();
                self.library_dirty = true;
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
                    .desired_width(140.0)
                    .hint_text("Nova área: Pele..."),
            );
            if ui.button("Adicionar área").clicked() && !self.new_group_name.trim().is_empty() {
                self.library.characters[ci].add_group(self.new_group_name.trim());
                self.new_group_name.clear();
                self.library_dirty = true;
            }
        });

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
                            .min_size(egui::vec2(30.0, 30.0));
                        if ui.add(btn).clicked() {
                            pick = Some(nc.color);
                        }
                        ui.vertical(|ui| {
                            ui.strong(&nc.label);
                            ui.monospace(sketchmotion_color::to_hex(nc.color));
                        });
                    });
                }
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_color_label)
                            .desired_width(100.0)
                            .hint_text("rótulo"),
                    );
                    let sw = egui::Button::new("")
                        .fill(to_color32(current))
                        .min_size(egui::vec2(20.0, 20.0));
                    ui.add_enabled(false, sw);
                    if ui.button("+ salvar cor").clicked() {
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

        if let Some((gi, label)) = add_to_group {
            if gi < self.library.characters[ci].groups.len() {
                self.library.characters[ci].groups[gi].add_color(label, current);
                self.new_color_label.clear();
                self.library_dirty = true;
            }
        }
        if let Some(gi) = remove_group {
            self.library.characters[ci].remove_group(gi);
            self.library_dirty = true;
        }
        if remove_char {
            self.library.remove_character(ci);
            self.selected_char = if self.library.characters.is_empty() { None } else { Some(0) };
            self.library_dirty = true;
        }
        if let Some(c) = pick {
            self.brush_color = to_color32(c);
            self.tool = Tool::Pencil;
            self.status = format!("Cor {} da paleta", sketchmotion_color::to_hex(c));
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
                if !self.status.is_empty() {
                    ui.separator();
                    ui.label(&self.status);
                }
            });
        });

        egui::SidePanel::right("toolbox").min_width(240.0).show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.toggle_value(&mut self.show_tools, "Ferramentas");
                ui.toggle_value(&mut self.show_colors, "Cor");
                ui.toggle_value(&mut self.show_palettes, "Paletas");
            });
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                if self.show_tools {
                    self.ui_ferramentas(ui);
                    ui.separator();
                }
                if self.show_colors {
                    self.ui_cor(ui);
                    ui.separator();
                }
                if self.show_palettes {
                    self.ui_paletas(ui);
                    ui.separator();
                }
            });
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

        // Salva a biblioteca de paletas quando ela muda.
        if self.library_dirty {
            self.salvar_biblioteca();
            self.library_dirty = false;
        }
    }
}
