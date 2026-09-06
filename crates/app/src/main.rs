//! SketchMotion — binário da aplicação (egui/eframe).
//!
//! Interface no modelo "barra de ícones + janelas de ferramenta":
//! - A barra direita mostra só ícones. Clicar num ícone abre/fecha a janela
//!   flutuante daquela ferramenta. Várias podem ficar abertas ao mesmo tempo.
//! - Cada ferramenta nova segue o mesmo mecanismo (um ícone + uma janela).
//! Ferramentas atuais: Ferramentas de desenho, Seleção de cores, Paleta
//! personalizada.

use eframe::egui;
use sketchmotion_color::PaletteLibrary;
use sketchmotion_core::{Color, Document};
use sketchmotion_render::{render_document, PixelImage};
use sketchmotion_tools::Tool;

const CANVAS_W: u32 = 800;
const CANVAS_H: u32 = 520;

/// Nº de colunas da grade de cores básicas.
const BASICAS_COLS: usize = 16;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1120.0, 760.0]),
        ..Default::default()
    };
    eframe::run_native(
        "SketchMotion",
        options,
        Box::new(|cc| {
            // Registra a fonte de ícones Phosphor no egui.
            let mut fonts = egui::FontDefinitions::default();
            egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
            cc.egui_ctx.set_fonts(fonts);
            Ok(Box::new(SketchMotionApp::new()))
        }),
    )
}

fn to_color32(c: Color) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

/// Gera a grade de cores básicas: uma linha de tons de cinza + linhas de
/// matizes em variações de saturação/valor (primárias, secundárias,
/// terciárias e suas variações claras/escuras).
fn cores_basicas() -> Vec<egui::Color32> {
    use egui::ecolor::Hsva;
    let cols = BASICAS_COLS;
    let mut v = Vec::new();
    for i in 0..cols {
        let g = (i as f32 / (cols - 1) as f32 * 255.0).round() as u8;
        v.push(egui::Color32::from_gray(g));
    }
    for &(sat, val) in &[
        (0.25f32, 1.0f32), // pastéis: azul bebê, rosa, lilás, verde claro
        (0.55, 1.0),       // claros
        (0.90, 0.95),      // vivos
        (1.0, 0.75),       // fortes
        (1.0, 0.48),       // escuros
        (0.65, 0.45),      // terrosos: marrom, oliva, azul acinzentado
    ] {
        for h in 0..cols {
            let hue = h as f32 / cols as f32;
            v.push(egui::Color32::from(Hsva::new(hue, sat, val, 1.0)));
        }
    }
    v
}

/// Desenha uma paleta de cores em quadrados encostados que preenchem toda a
/// largura disponível. Devolve a cor clicada, se houver.
fn ui_paleta_quadrados(
    ui: &mut egui::Ui,
    cores: &[egui::Color32],
    cols: usize,
) -> Option<egui::Color32> {
    let gap = 2.0;
    let avail = ui.available_width();
    let cell = ((avail - gap * (cols as f32 - 1.0)) / cols as f32).floor().max(8.0);
    let rows = cores.len().div_ceil(cols);
    let altura = rows as f32 * (cell + gap) - gap;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(avail, altura), egui::Sense::click());
    let click_pos = resp.interact_pointer_pos();
    let painter = ui.painter();
    let mut picked = None;
    for (i, c) in cores.iter().enumerate() {
        let x = rect.left() + (i % cols) as f32 * (cell + gap);
        let y = rect.top() + (i / cols) as f32 * (cell + gap);
        let cellrect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(cell, cell));
        painter.rect_filled(cellrect, 1.0, *c);
        if resp.clicked() {
            if let Some(pos) = click_pos {
                if cellrect.contains(pos) {
                    picked = Some(*c);
                }
            }
        }
    }
    picked
}

/// Botão de ícone da barra direita. Fica destacado quando a janela
/// correspondente está aberta.
fn icon_button(ui: &mut egui::Ui, active: bool, icon: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(38.0, 38.0), egui::Sense::click());
    let bg = if active {
        egui::Color32::from_rgb(0x2F, 0x84, 0xFE)
    } else if resp.hovered() {
        egui::Color32::from_gray(70)
    } else {
        egui::Color32::from_gray(48)
    };
    ui.painter().rect_filled(rect, 5.0, bg);
    let fg = if active {
        egui::Color32::WHITE
    } else {
        egui::Color32::from_gray(225)
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(20.0),
        fg,
    );
    resp
}

/// Área achatada de saturação (X) x valor (Y) para o matiz atual. Preenche a
/// `largura` e usa a `altura` dada. Devolve true se o usuário mudou a cor.
fn seletor_sv(ui: &mut egui::Ui, hsva: &mut egui::ecolor::Hsva, largura: f32, altura: f32) -> bool {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(largura, altura), egui::Sense::click_and_drag());
    let hue_col = egui::Color32::from(egui::ecolor::Hsva::new(hsva.h, 1.0, 1.0, 1.0));
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), egui::Color32::WHITE);
    mesh.colored_vertex(rect.right_top(), hue_col);
    mesh.colored_vertex(rect.right_bottom(), egui::Color32::BLACK);
    mesh.colored_vertex(rect.left_bottom(), egui::Color32::BLACK);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    ui.painter().add(egui::Shape::mesh(mesh));

    let px = rect.left() + hsva.s * rect.width();
    let py = rect.top() + (1.0 - hsva.v) * rect.height();
    ui.painter()
        .circle_stroke(egui::pos2(px, py), 5.0, egui::Stroke::new(2.0, egui::Color32::WHITE));
    ui.painter()
        .circle_stroke(egui::pos2(px, py), 6.0, egui::Stroke::new(1.0, egui::Color32::BLACK));

    let mut changed = false;
    if resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            hsva.s = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            hsva.v = (1.0 - (pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
            changed = true;
        }
    }
    changed
}

/// Barra de matiz (arco-íris) que preenche a largura. Devolve true se mudou.
fn seletor_hue(ui: &mut egui::Ui, hsva: &mut egui::ecolor::Hsva, largura: f32, altura: f32) -> bool {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(largura, altura), egui::Sense::click_and_drag());
    let mut mesh = egui::Mesh::default();
    let n = 72;
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let x = rect.left() + t * rect.width();
        let col = egui::Color32::from(egui::ecolor::Hsva::new(t, 1.0, 1.0, 1.0));
        mesh.colored_vertex(egui::pos2(x, rect.top()), col);
        mesh.colored_vertex(egui::pos2(x, rect.bottom()), col);
    }
    for i in 0..n {
        let a = (i * 2) as u32;
        mesh.add_triangle(a, a + 1, a + 2);
        mesh.add_triangle(a + 2, a + 1, a + 3);
    }
    ui.painter().add(egui::Shape::mesh(mesh));

    let x = rect.left() + hsva.h * rect.width();
    ui.painter().rect_stroke(
        egui::Rect::from_min_max(
            egui::pos2(x - 2.0, rect.top()),
            egui::pos2(x + 2.0, rect.bottom()),
        ),
        0.0,
        egui::Stroke::new(2.0, egui::Color32::WHITE),
    );

    let mut changed = false;
    if resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            hsva.h = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            changed = true;
        }
    }
    changed
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
    custom_colors: Vec<egui::Color32>,
    picker_hsva: egui::ecolor::Hsva,
    // janelas de ferramenta (abrir/ocultar pela barra de ícones)
    win_tools: bool,
    win_color: bool,
    win_palette: bool,
    // paletas por personagem
    library: PaletteLibrary,
    library_dirty: bool,
    selected_char: Option<usize>,
    new_char_name: String,
    new_group_name: String,
    new_color_label: String,
    icon_r_tools: Option<egui::Rect>,
    icon_r_color: Option<egui::Rect>,
    icon_r_palette: Option<egui::Rect>,
}

impl SketchMotionApp {
    fn new() -> Self {
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
            custom_colors: Vec::new(),
            picker_hsva: egui::ecolor::Hsva::from(egui::Color32::BLACK),
            win_tools: true,
            win_color: false,
            win_palette: false,
            library,
            library_dirty: false,
            selected_char,
            new_char_name: String::new(),
            new_group_name: String::new(),
            new_color_label: String::new(),
            icon_r_tools: None,
            icon_r_color: None,
            icon_r_palette: None,
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

    fn salvar_biblioteca(&mut self) {
        if let Some(path) = sketchmotion_io::default_library_path() {
            if let Err(e) = sketchmotion_io::save_library(&self.library, &path) {
                self.status = format!("Erro ao salvar paletas: {e}");
            }
        }
    }

    /// Barra direita de ícones (uma ferramenta por ícone).
    fn barra_icones(&mut self, ctx: &egui::Context) {
        use egui_phosphor::regular as icon;
        egui::SidePanel::right("barra_icones")
            .exact_width(50.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.add_space(8.0);
                ui.vertical_centered(|ui| {
                    let resp_t = icon_button(ui, self.win_tools, icon::PENCIL)
                        .on_hover_text("Ferramentas de desenho");
                    self.icon_r_tools = Some(resp_t.rect);
                    if resp_t.clicked() {
                        self.win_tools = !self.win_tools;
                    }
                    ui.add_space(6.0);

                    let resp_c = icon_button(ui, self.win_color, icon::PALETTE)
                        .on_hover_text("Seleção de cores");
                    self.icon_r_color = Some(resp_c.rect);
                    if resp_c.clicked() {
                        self.win_color = !self.win_color;
                    }
                    ui.add_space(6.0);

                    let resp_p = icon_button(ui, self.win_palette, icon::SWATCHES)
                        .on_hover_text("Paleta personalizada");
                    self.icon_r_palette = Some(resp_p.rect);
                    if resp_p.clicked() {
                        self.win_palette = !self.win_palette;
                    }
                });
            });
    }

    /// Janela: Ferramentas de desenho.
    fn janela_ferramentas(&mut self, ctx: &egui::Context) {
        let mut open = self.win_tools;
        let mut win = egui::Window::new("Ferramentas")
            .open(&mut open)
            .default_width(220.0);
        if let Some(r) = self.icon_r_tools {
            win = win
                .default_pos(egui::pos2(r.left() - 8.0, r.top()))
                .pivot(egui::Align2::RIGHT_TOP);
        }
        win.show(ctx, |ui| {
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
            });
        self.win_tools = open;
    }

    /// Janela: Seleção de cores (visual + código + cores personalizadas).
    fn janela_cor(&mut self, ctx: &egui::Context) {
        let mut open = self.win_color;
        let mut save_custom = false;
        let mut pick_custom: Option<egui::Color32> = None;

        let mut win = egui::Window::new("Seleção de cores")
            .open(&mut open)
            .default_width(560.0);
        if let Some(r) = self.icon_r_color {
            win = win
                .default_pos(egui::pos2(r.left() - 8.0, r.top()))
                .pivot(egui::Align2::RIGHT_TOP);
        }
        win.show(ctx, |ui| {
                // Ressincroniza o HSV se a cor mudou por outra via (paleta, hex...).
                if egui::Color32::from(self.picker_hsva) != self.brush_color {
                    self.picker_hsva = egui::ecolor::Hsva::from(self.brush_color);
                }
                let largura = ui.available_width();
                let mut mudou = seletor_sv(ui, &mut self.picker_hsva, largura, 130.0);
                ui.add_space(4.0);
                mudou |= seletor_hue(ui, &mut self.picker_hsva, largura, 18.0);
                if mudou {
                    self.brush_color = egui::Color32::from(self.picker_hsva);
                    self.tool = Tool::Pencil;
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Código:");
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.hex_input)
                            .desired_width(110.0)
                            .hint_text("#RRGGBB"),
                    );
                    let enter =
                        resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (ui.button("Aplicar").clicked() || enter)
                        && !self.hex_input.trim().is_empty()
                    {
                        if let Some(c) = sketchmotion_color::from_hex(&self.hex_input) {
                            self.brush_color = to_color32(c);
                            self.tool = Tool::Pencil;
                        }
                    }
                    ui.monospace(sketchmotion_color::to_hex(Color::rgba(
                        self.brush_color.r(),
                        self.brush_color.g(),
                        self.brush_color.b(),
                        self.brush_color.a(),
                    )));
                    // Amostra da cor selecionada no momento.
                    let (sw, _) =
                        ui.allocate_exact_size(egui::vec2(80.0, 22.0), egui::Sense::hover());
                    ui.painter().rect_filled(sw, 3.0, self.brush_color);
                    ui.painter().rect_stroke(
                        sw,
                        3.0,
                        egui::Stroke::new(1.0, egui::Color32::from_gray(90)),
                    );
                });
                ui.separator();
                ui.label("Cores básicas:");
                if let Some(c) = ui_paleta_quadrados(ui, &cores_basicas(), BASICAS_COLS) {
                    self.brush_color = c;
                    self.tool = Tool::Pencil;
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Cores personalizadas:");
                    if ui.button("+ salvar atual").clicked() {
                        save_custom = true;
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    if self.custom_colors.is_empty() {
                        ui.weak("(nenhuma salva ainda)");
                    }
                    for c in &self.custom_colors {
                        let b = egui::Button::new("").fill(*c).min_size(egui::vec2(26.0, 26.0));
                        if ui.add(b).clicked() {
                            pick_custom = Some(*c);
                        }
                    }
                });
            });

        if save_custom {
            self.custom_colors.push(self.brush_color);
        }
        if let Some(c) = pick_custom {
            self.brush_color = c;
            self.tool = Tool::Pencil;
        }
        self.win_color = open;
    }

    /// Janela: Paleta personalizada (swatches por personagem/área).
    fn janela_paletas(&mut self, ctx: &egui::Context) {
        let mut open = self.win_palette;
        let current = self.brush_core_color();

        let mut pick: Option<Color> = None;
        let mut remove_group: Option<usize> = None;
        let mut add_to_group: Option<(usize, String)> = None;
        let mut remove_char = false;

        let mut win = egui::Window::new("Paleta personalizada")
            .open(&mut open)
            .default_width(380.0);
        if let Some(r) = self.icon_r_palette {
            win = win
                .default_pos(egui::pos2(r.left() - 8.0, r.top()))
                .pivot(egui::Align2::RIGHT_TOP);
        }
        win.show(ctx, |ui| {
                // Abas de personagem
                ui.horizontal_wrapped(|ui| {
                    for (i, c) in self.library.characters.iter().enumerate() {
                        if ui
                            .selectable_label(self.selected_char == Some(i), c.name.clone())
                            .clicked()
                        {
                            self.selected_char = Some(i);
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_char_name)
                            .desired_width(150.0)
                            .hint_text("Novo personagem"),
                    );
                    if ui.button("+ personagem").clicked() && !self.new_char_name.trim().is_empty()
                    {
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
                let Some(ci) = self.selected_char else {
                    return;
                };
                if ci >= self.library.characters.len() {
                    return;
                }

                ui.horizontal(|ui| {
                    if ui.small_button("remover personagem").clicked() {
                        remove_char = true;
                    }
                });
                ui.separator();

                // Uma linha por área: rótulo + amostras em fila
                for (gi, group) in self.library.characters[ci].groups.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(84.0, 26.0), egui::Sense::hover());
                        ui.painter()
                            .rect_filled(rect, 3.0, egui::Color32::from_rgb(0x3A, 0x52, 0x6B));
                        ui.painter().text(
                            rect.left_center() + egui::vec2(6.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            &group.name,
                            egui::FontId::proportional(13.0),
                            egui::Color32::WHITE,
                        );
                        for nc in &group.colors {
                            let (r, resp) = ui
                                .allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::click());
                            ui.painter().rect_filled(r, 2.0, to_color32(nc.color));
                            ui.painter().rect_stroke(
                                r,
                                2.0,
                                egui::Stroke::new(1.0, egui::Color32::from_gray(60)),
                            );
                            if resp
                                .on_hover_text(format!(
                                    "{} {}",
                                    nc.label,
                                    sketchmotion_color::to_hex(nc.color)
                                ))
                                .clicked()
                            {
                                pick = Some(nc.color);
                            }
                        }
                        if ui.small_button("x").clicked() {
                            remove_group = Some(gi);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.add_space(86.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut self.new_color_label)
                                .desired_width(90.0)
                                .hint_text("rótulo"),
                        );
                        if ui.small_button("+ cor atual").clicked() {
                            let label = if self.new_color_label.trim().is_empty() {
                                "cor".to_owned()
                            } else {
                                self.new_color_label.trim().to_owned()
                            };
                            add_to_group = Some((gi, label));
                        }
                    });
                }

                ui.separator();
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_group_name)
                            .desired_width(150.0)
                            .hint_text("Nova área: Pele..."),
                    );
                    if ui.button("+ área").clicked() && !self.new_group_name.trim().is_empty() {
                        self.library.characters[ci].add_group(self.new_group_name.trim());
                        self.new_group_name.clear();
                        self.library_dirty = true;
                    }
                });
            });

        // Ações adiadas
        if let Some(ci) = self.selected_char {
            if ci < self.library.characters.len() {
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
                    self.selected_char =
                        if self.library.characters.is_empty() { None } else { Some(0) };
                    self.library_dirty = true;
                }
            }
        }
        if let Some(c) = pick {
            self.brush_color = to_color32(c);
            self.tool = Tool::Pencil;
            self.status = format!("Cor {} da paleta", sketchmotion_color::to_hex(c));
        }
        self.win_palette = open;
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

        // Barra de ícones (sempre visível) e janelas de ferramenta (flutuantes).
        self.barra_icones(ctx);
        self.janela_ferramentas(ctx);
        self.janela_cor(ctx);
        self.janela_paletas(ctx);

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

        if self.library_dirty {
            self.salvar_biblioteca();
            self.library_dirty = false;
        }
    }
}
