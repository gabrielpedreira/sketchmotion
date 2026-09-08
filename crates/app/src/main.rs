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
use sketchmotion_core::{Anchor, Color, Document, VectorObject};
use sketchmotion_render::{render_document, PixelImage};
use sketchmotion_tools::Tool;

const CANVAS_W: u32 = 800;
const CANVAS_H: u32 = 520;
const MAX_UNDO: usize = 10;

/// Nº de colunas da grade de cores básicas.
const BASICAS_COLS: usize = 16;

/// Fontes exibidas no painel de texto (aplicação real virá com o módulo de texto).
const FONTES: [&str; 4] = ["Sans", "Serif", "Monospace", "Manuscrito"];

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1120.0, 760.0])
            .with_icon(std::sync::Arc::new(load_icon())),
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

/// Carrega o ícone da janela a partir do .ico embutido no binário.
fn load_icon() -> egui::IconData {
    let bytes = include_bytes!("../../../assets/logo-oficial.ico");
    match image::load_from_memory(bytes) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            egui::IconData { rgba: rgba.into_raw(), width: w, height: h }
        }
        Err(_) => egui::IconData { rgba: vec![0, 0, 0, 0], width: 1, height: 1 },
    }
}

fn to_color32(c: Color) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

/// Distância de um ponto (px,py) ao segmento (x1,y1)-(x2,y2).
fn dist_point_seg(px: f32, py: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let len2 = dx * dx + dy * dy;
    if len2 <= f32::EPSILON {
        return ((px - x1).powi(2) + (py - y1).powi(2)).sqrt();
    }
    let t = (((px - x1) * dx + (py - y1) * dy) / len2).clamp(0.0, 1.0);
    let cx = x1 + t * dx;
    let cy = y1 + t * dy;
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// As 8 alças da caixa de seleção, na ordem canônica.
fn handle_positions(r: egui::Rect) -> [egui::Pos2; 8] {
    [
        r.left_top(),
        r.center_top(),
        r.right_top(),
        r.right_center(),
        r.right_bottom(),
        r.center_bottom(),
        r.left_bottom(),
        r.left_center(),
    ]
}

/// Para a alça `hi`: ponto agarrado, ponto fixo (oposto) e quais eixos escalam.
fn handle_geometry(
    hi: usize,
    minx: f32,
    miny: f32,
    maxx: f32,
    maxy: f32,
    cx: f32,
    cy: f32,
) -> ((f32, f32), (f32, f32), (bool, bool)) {
    match hi {
        0 => ((minx, miny), (maxx, maxy), (true, true)),
        1 => ((cx, miny), (cx, maxy), (false, true)),
        2 => ((maxx, miny), (minx, maxy), (true, true)),
        3 => ((maxx, cy), (minx, cy), (true, false)),
        4 => ((maxx, maxy), (minx, miny), (true, true)),
        5 => ((cx, maxy), (cx, miny), (false, true)),
        6 => ((minx, maxy), (maxx, miny), (true, true)),
        _ => ((minx, cy), (maxx, cy), (true, false)),
    }
}

fn safe_ratio(num: f32, den: f32) -> f32 {
    if den.abs() < 1e-4 {
        1.0
    } else {
        num / den
    }
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
        .circle_stroke(egui::pos2(px, py), 5.0_f32, egui::Stroke::new(2.0_f32, egui::Color32::WHITE));
    ui.painter()
        .circle_stroke(egui::pos2(px, py), 6.0_f32, egui::Stroke::new(1.0_f32, egui::Color32::BLACK));

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
        egui::Stroke::new(2.0_f32, egui::Color32::WHITE),
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

/// Estado do conta-gotas: desligado, capturar para o pincel, ou capturar e
/// adicionar a uma área (índice do grupo) do personagem selecionado.
/// Tela atual do app: inicial (escolher documento) ou editor.
#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Home,
    Editor,
}

#[derive(Clone, Copy, PartialEq)]
enum Eyedropper {
    Off,
    ToBrush,
    ToArea(usize),
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
    win_color: bool,
    win_palette: bool,
    // paletas por personagem
    library: PaletteLibrary,
    library_dirty: bool,
    selected_char: Option<usize>,
    new_char_name: String,
    new_group_name: String,
    icon_r_color: Option<egui::Rect>,
    icon_r_palette: Option<egui::Rect>,
    reopen_color: bool,
    reopen_palette: bool,
    eyedropper: Eyedropper,
    active_layer: usize,
    win_layers: bool,
    reopen_layers: bool,
    icon_r_layers: Option<egui::Rect>,
    current_path: Option<std::path::PathBuf>,
    pixel_mode: bool,
    screen: Screen,
    home_w: u32,
    home_h: u32,
    home_pixel: bool,
    undo_stack: Vec<Document>,
    redo_stack: Vec<Document>,
    zoom: f32,
    // configurações por ferramenta (barra de opções do topo)
    text_font: usize,
    text_size: f32,
    text_bold: bool,
    text_italic: bool,
    text_underline: bool,
    text_outline: bool,
    wand_tolerance: i32,
    pen_width: i32,
    // estado vetorial
    selected_obj: Option<usize>,
    pen_anchors: Vec<Anchor>,
    pen_drag_idx: Option<usize>,
    dragging_obj: bool,
    // redimensionamento pelas alças da seleção
    resize_handle: Option<usize>,
    resize_orig: Vec<Anchor>,
    resize_fixed: (f32, f32),
    resize_grab: (f32, f32),
    resize_axes: (bool, bool),
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
            win_color: false,
            win_palette: false,
            library,
            library_dirty: false,
            selected_char,
            new_char_name: String::new(),
            new_group_name: String::new(),
            icon_r_color: None,
            icon_r_palette: None,
            reopen_color: false,
            reopen_palette: false,
            eyedropper: Eyedropper::Off,
            active_layer: 0,
            win_layers: false,
            reopen_layers: false,
            icon_r_layers: None,
            current_path: None,
            pixel_mode: false,
            screen: Screen::Home,
            home_w: 800,
            home_h: 520,
            home_pixel: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            zoom: 1.0,
            text_font: 0,
            text_size: 24.0,
            text_bold: false,
            text_italic: false,
            text_underline: false,
            text_outline: false,
            wand_tolerance: 32,
            pen_width: 2,
            selected_obj: None,
            pen_anchors: Vec::new(),
            pen_drag_idx: None,
            dragging_obj: false,
            resize_handle: None,
            resize_orig: Vec::new(),
            resize_fixed: (0.0, 0.0),
            resize_grab: (0.0, 0.0),
            resize_axes: (true, true),
        }
    }

    fn brush_core_color(&self) -> Color {
        let c = self.brush_color;
        Color::rgba(c.r(), c.g(), c.b(), c.a())
    }

    fn active_color(&self) -> Color {
        self.tool.effective_color(self.brush_core_color())
    }

    /// Salva o estado atual no histórico (limitado a MAX_UNDO) e limpa o refazer.
    fn push_undo(&mut self) {
        self.undo_stack.push(self.document.clone());
        if self.undo_stack.len() > MAX_UNDO {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.document.clone());
            self.document = prev;
            self.active_layer = self
                .active_layer
                .min(self.document.layers.len().saturating_sub(1));
            self.last_pos = None;
            self.dirty = true;
            self.status = "Desfeito".to_owned();
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.document.clone());
            self.document = next;
            self.active_layer = self
                .active_layer
                .min(self.document.layers.len().saturating_sub(1));
            self.last_pos = None;
            self.dirty = true;
            self.status = "Refeito".to_owned();
        }
    }

    /// Cor do documento no pixel (x, y): a da camada se opaca, senão o fundo.
    fn cor_no_pixel(&self, x: i32, y: i32) -> Color {
        if x < 0 || y < 0 {
            return self.document.background;
        }
        let (xu, yu) = (x as u32, y as u32);
        let bg = self.document.background;
        let (mut r, mut g, mut b) = (bg.r as u32, bg.g as u32, bg.b as u32);
        for layer in &self.document.layers {
            if !layer.visible {
                continue;
            }
            if let Some(px) = layer.get_pixel(xu, yu) {
                let a = px.a as u32;
                if a == 0 {
                    continue;
                }
                let ia = 255 - a;
                r = (px.r as u32 * a + r * ia) / 255;
                g = (px.g as u32 * a + g * ia) / 255;
                b = (px.b as u32 * a + b * ia) / 255;
            }
        }
        Color::rgb(r as u8, g as u8, b as u8)
    }

    fn paint_dab(&mut self, x: i32, y: i32) {
        let color = self.active_color();
        let r = self.brush_radius;
        let li = self.active_layer;
        let pixel = self.pixel_mode;
        let mut pintou = false;
        if let Some(layer) = self.document.layer_mut(li) {
            if !layer.locked {
                if pixel {
                    // Pixel art: quadrado de lado `r` células, encaixado no grid.
                    let half = (r - 1) / 2;
                    for dy in 0..r {
                        for dx in 0..r {
                            let px = x + dx - half;
                            let py = y + dy - half;
                            if px >= 0 && py >= 0 {
                                layer.set_pixel(px as u32, py as u32, color);
                            }
                        }
                    }
                } else {
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
                pintou = true;
            }
        }
        if pintou {
            self.dirty = true;
        }
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

    /// Cria um documento novo em branco (w x h). `pixel` marca o modo pixel art
    /// (comportamento específico virá com o painel de configuração).
    fn novo_documento(&mut self, w: u32, h: u32, pixel: bool) {
        self.document = Document::new(w, h, Color::WHITE);
        self.active_layer = 0;
        self.last_pos = None;
        self.current_path = None;
        self.pixel_mode = pixel;
        self.zoom = if pixel {
            (512.0 / (w.max(h) as f32)).floor().max(1.0)
        } else {
            1.0
        };
        self.dirty = true;
        self.status = if pixel {
            format!("Novo documento pixel art {w}x{h}")
        } else {
            format!("Novo documento {w}x{h}")
        };
    }

    fn salvar(&mut self) {
        if let Some(path) = self.current_path.clone() {
            self.status = match sketchmotion_io::save(&self.document, &path) {
                Ok(()) => format!("Salvo em {}", path.display()),
                Err(e) => format!("Erro ao salvar: {e}"),
            };
        } else {
            self.salvar_como();
        }
    }

    fn salvar_como(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("SketchMotion", &[sketchmotion_io::PROJECT_EXTENSION])
            .set_file_name("desenho.sketchmotion")
            .save_file()
        {
            match sketchmotion_io::save(&self.document, &path) {
                Ok(()) => {
                    self.status = format!("Salvo em {}", path.display());
                    self.current_path = Some(path);
                }
                Err(e) => self.status = format!("Erro ao salvar: {e}"),
            }
        }
    }

    fn exportar(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("PNG", &["png"])
            .add_filter("JPEG", &["jpg", "jpeg"])
            .set_file_name("desenho.png")
            .save_file()
        {
            let img = render_document(&self.document);
            self.status = match sketchmotion_io::export_png(
                img.width as u32,
                img.height as u32,
                &img.rgba,
                &path,
            ) {
                Ok(()) => format!("Exportado: {}", path.display()),
                Err(e) => format!("Erro ao exportar: {e}"),
            };
        }
    }

    fn abrir(&mut self) -> bool {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("SketchMotion", &[sketchmotion_io::PROJECT_EXTENSION])
            .pick_file()
        {
            match sketchmotion_io::load(&path) {
                Ok(doc) => {
                    self.document = doc;
                    self.active_layer = 0;
                    self.last_pos = None;
                    self.dirty = true;
                    self.current_path = Some(path.clone());
                    self.status = format!("Aberto: {}", path.display());
                    return true;
                }
                Err(e) => self.status = format!("Erro ao abrir: {e}"),
            }
        }
        false
    }

    fn salvar_biblioteca(&mut self) {
        if let Some(path) = sketchmotion_io::default_library_path() {
            if let Err(e) = sketchmotion_io::save_library(&self.library, &path) {
                self.status = format!("Erro ao salvar paletas: {e}");
            }
        }
    }

    /// Barra direita de ícones (uma ferramenta por ícone).
    /// Finaliza o traço da Caneta, criando um objeto vetorial.
    fn finalizar_caneta(&mut self) {
        if self.pen_anchors.len() >= 2 {
            self.push_undo();
            let mut obj = VectorObject::new(self.brush_core_color(), self.pen_width as f32);
            obj.points = self.pen_anchors.clone();
            self.document.vectors.push(obj);
            self.selected_obj = Some(self.document.vectors.len() - 1);
            self.status = "Traço vetorial criado".into();
        }
        self.pen_anchors.clear();
        self.pen_drag_idx = None;
        self.dirty = true;
    }

    /// Índice do objeto vetorial mais próximo do ponto (coords do documento),
    /// dentro da tolerância `thr`; None se nenhum estiver perto.
    fn hit_test(&self, ponto: (f32, f32), thr: f32) -> Option<usize> {
        let (px, py) = ponto;
        let mut best: Option<(usize, f32)> = None;
        for (i, obj) in self.document.vectors.iter().enumerate() {
            let flat = obj.flatten(20);
            let mut dmin = f32::INFINITY;
            if flat.len() == 1 {
                dmin = ((flat[0].0 - px).powi(2) + (flat[0].1 - py).powi(2)).sqrt();
            } else {
                for w in flat.windows(2) {
                    let d = dist_point_seg(px, py, w[0].0, w[0].1, w[1].0, w[1].1);
                    if d < dmin {
                        dmin = d;
                    }
                }
            }
            let tol = thr + obj.stroke_width * 0.5;
            if dmin <= tol && best.map_or(true, |(_, bd)| dmin < bd) {
                best = Some((i, dmin));
            }
        }
        best.map(|(i, _)| i)
    }

    /// Índice da alça da caixa do objeto `si` sob o ponto de tela `hp`.
    fn handle_at(&self, si: usize, hp: egui::Pos2, rect: egui::Rect, zoom: f32) -> Option<usize> {
        let obj = self.document.vectors.get(si)?;
        let (minx, miny, maxx, maxy) = obj.bounds()?;
        let r = egui::Rect::from_min_max(
            egui::pos2(rect.min.x + minx * zoom, rect.min.y + miny * zoom),
            egui::pos2(rect.min.x + maxx * zoom, rect.min.y + maxy * zoom),
        )
        .expand(3.0);
        handle_positions(r)
            .iter()
            .position(|hc| hc.distance(hp) <= 8.0)
    }

    /// Desenha os objetos vetoriais (curvas), a caixa/alças de seleção e o
    /// traço em progresso da Caneta, como overlay sobre o canvas.
    fn desenhar_vetores(&self, ui: &egui::Ui, rect: egui::Rect, zoom: f32) {
        let painter = ui.painter_at(rect);
        let sp = |x: f32, y: f32| egui::pos2(rect.min.x + x * zoom, rect.min.y + y * zoom);
        let azul = egui::Color32::from_rgb(0x2F, 0x84, 0xFE);
        for (idx, obj) in self.document.vectors.iter().enumerate() {
            let col = to_color32(obj.stroke).linear_multiply(obj.opacity.clamp(0.0, 1.0));
            let w = (obj.stroke_width * zoom).max(1.0);
            let pts: Vec<egui::Pos2> = obj.flatten(24).iter().map(|(x, y)| sp(*x, *y)).collect();
            if pts.len() >= 2 {
                painter.add(egui::Shape::line(pts, egui::Stroke::new(w, col)));
            } else if pts.len() == 1 {
                painter.circle_filled(pts[0], (w / 2.0).max(1.5), col);
            }
            if Some(idx) == self.selected_obj {
                if let Some((minx, miny, maxx, maxy)) = obj.bounds() {
                    let r = egui::Rect::from_min_max(sp(minx, miny), sp(maxx, maxy)).expand(3.0);
                    painter.rect_stroke(r, 0.0, egui::Stroke::new(1.0_f32, azul));
                    for c in handle_positions(r) {
                        let h = egui::Rect::from_center_size(c, egui::vec2(8.0, 8.0));
                        painter.rect_filled(h, 0.0, egui::Color32::WHITE);
                        painter.rect_stroke(h, 0.0, egui::Stroke::new(1.0_f32, azul));
                    }
                }
            }
        }
        // Traço em progresso da Caneta: curva + âncoras + alças bézier.
        if self.tool == Tool::Pen && !self.pen_anchors.is_empty() {
            let mut temp = VectorObject::new(self.brush_core_color(), self.pen_width as f32);
            temp.points = self.pen_anchors.clone();
            let pts: Vec<egui::Pos2> = temp.flatten(24).iter().map(|(x, y)| sp(*x, *y)).collect();
            if pts.len() >= 2 {
                painter.add(egui::Shape::line(
                    pts,
                    egui::Stroke::new(
                        (self.pen_width as f32 * zoom).max(1.0),
                        to_color32(self.brush_core_color()),
                    ),
                ));
            }
            for a in &self.pen_anchors {
                let p = sp(a.x, a.y);
                if let Some((hx, hy)) = a.hout {
                    let hp = sp(hx, hy);
                    painter.line_segment([p, hp], egui::Stroke::new(1.0_f32, azul));
                    painter.circle_filled(hp, 3.0, azul);
                }
                if let Some((hx, hy)) = a.hin {
                    let hp = sp(hx, hy);
                    painter.line_segment([p, hp], egui::Stroke::new(1.0_f32, azul));
                    painter.circle_filled(hp, 3.0, azul);
                }
                painter.circle_filled(p, 3.5, egui::Color32::WHITE);
                painter.circle_stroke(p, 3.5, egui::Stroke::new(1.5_f32, azul));
            }
        }
    }

    /// Barra de ferramentas à esquerda (estilo Illustrator): ícones de uso
    /// direto. Clicar seleciona a ferramenta na hora, sem abrir janela.
    fn barra_ferramentas(&mut self, ctx: &egui::Context) {
        use egui_phosphor::regular as icon;
        egui::SidePanel::left("barra_ferramentas")
            .exact_width(46.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.add_space(8.0);
                ui.vertical_centered(|ui| {
                    // Grupo: seleção
                    for (t, ic, hint) in [
                        (Tool::Select, icon::CURSOR, "Seleção — selecionar e mover um elemento"),
                        (Tool::DirectSelect, icon::SELECTION, "Seleção direta — editar por pontos"),
                        (Tool::MagicWand, icon::MAGIC_WAND, "Varinha mágica — selecionar por cor"),
                    ] {
                        let ativa = self.tool == t && self.eyedropper == Eyedropper::Off;
                        if icon_button(ui, ativa, ic).on_hover_text(hint).clicked() {
                            self.tool = t;
                            self.eyedropper = Eyedropper::Off;
                        }
                        ui.add_space(4.0);
                    }
                    ui.separator();
                    ui.add_space(4.0);
                    // Grupo: desenho
                    for (t, ic, hint) in [
                        (Tool::Pen, icon::PEN_NIB, "Caneta — desenhar por pontos"),
                        (Tool::Text, icon::TEXT_T, "Texto"),
                        (Tool::Pencil, icon::PAINT_BRUSH, "Pincel"),
                        (Tool::Eraser, icon::ERASER, "Borracha"),
                    ] {
                        let ativa = self.tool == t && self.eyedropper == Eyedropper::Off;
                        if icon_button(ui, ativa, ic).on_hover_text(hint).clicked() {
                            self.tool = t;
                            self.eyedropper = Eyedropper::Off;
                        }
                        ui.add_space(4.0);
                    }
                    ui.separator();
                    ui.add_space(4.0);
                    // Conta-gotas (reaproveita o mecanismo de captura de cor)
                    let ativa_ed = self.eyedropper != Eyedropper::Off;
                    if icon_button(ui, ativa_ed, icon::EYEDROPPER)
                        .on_hover_text("Conta-gotas — capturar cor do desenho")
                        .clicked()
                    {
                        self.eyedropper = Eyedropper::ToBrush;
                        self.status = "Conta-gotas: clique no desenho para capturar a cor".into();
                    }
                    ui.add_space(10.0);
                    // Amostra da cor atual (clique abre a seleção de cores)
                    let (rect, resp) =
                        ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::click());
                    ui.painter().rect_filled(rect, 4.0, self.brush_color);
                    ui.painter().rect_stroke(
                        rect,
                        4.0,
                        egui::Stroke::new(1.0_f32, egui::Color32::from_gray(120)),
                    );
                    if resp.on_hover_text("Cor atual — clique para escolher").clicked() {
                        self.win_color = true;
                        self.reopen_color = true;
                    }
                });
            });
    }

    /// Barra de opções (abaixo do menu): cada ferramenta abre aqui o seu
    /// próprio painel, com os controles que fazem sentido para ela.
    fn barra_opcoes(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("opcoes").show(ctx, |ui| {
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                ui.add_space(4.0);
                ui.strong(self.tool.label());
                ui.separator();
                if self.eyedropper != Eyedropper::Off {
                    ui.weak("Clique no desenho para capturar uma cor.");
                } else {
                    match self.tool {
                        Tool::Pencil => self.opcoes_pincel(ui),
                        Tool::Eraser => self.opcoes_borracha(ui),
                        Tool::Select => self.opcoes_selecao(ui),
                        Tool::Text => self.opcoes_texto(ui),
                        Tool::Pen => self.opcoes_caneta(ui),
                        Tool::MagicWand => self.opcoes_varinha(ui),
                        Tool::DirectSelect => self.opcoes_selecao_direta(ui),
                    }
                }
            });
            ui.add_space(2.0);
        });
    }

    /// Amostra da cor atual; clicar abre a janela de seleção de cores.
    fn swatch_cor(&mut self, ui: &mut egui::Ui) {
        ui.label("Cor:");
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(24.0, 18.0), egui::Sense::click());
        ui.painter().rect_filled(rect, 3.0, self.brush_color);
        ui.painter().rect_stroke(
            rect,
            3.0,
            egui::Stroke::new(1.0_f32, egui::Color32::from_gray(120)),
        );
        if resp.on_hover_text("Cor atual — clique para escolher").clicked() {
            self.win_color = true;
            self.reopen_color = true;
        }
    }

    /// Botão "Limpar tudo": recomeça o documento (mantendo tamanho e fundo).
    fn botao_limpar(&mut self, ui: &mut egui::Ui) {
        if ui
            .button("Limpar tudo")
            .on_hover_text("Apaga tudo e recomeça o documento")
            .clicked()
        {
            self.push_undo();
            let (w, h) = (self.document.width, self.document.height);
            let bg = self.document.background;
            self.document = Document::new(w, h, bg);
            self.active_layer = 0;
            self.last_pos = None;
            self.dirty = true;
        }
    }

    fn opcoes_pincel(&mut self, ui: &mut egui::Ui) {
        ui.label("Tamanho:");
        ui.add(egui::Slider::new(&mut self.brush_radius, 1..=30));
        ui.separator();
        self.swatch_cor(ui);
        ui.separator();
        self.botao_limpar(ui);
    }

    fn opcoes_borracha(&mut self, ui: &mut egui::Ui) {
        ui.label("Tamanho:");
        ui.add(egui::Slider::new(&mut self.brush_radius, 1..=30));
        ui.separator();
        self.botao_limpar(ui);
    }

    /// Ferramenta Seleção: opera sobre o objeto vetorial selecionado.
    fn opcoes_selecao(&mut self, ui: &mut egui::Ui) {
        match self.selected_obj {
            Some(idx) if idx < self.document.vectors.len() => {
                ui.label("Traço selecionado.");
                ui.separator();
                ui.label("Espessura:");
                let mut w = self.document.vectors[idx].stroke_width;
                if ui.add(egui::Slider::new(&mut w, 1.0..=40.0)).changed() {
                    self.document.vectors[idx].stroke_width = w;
                }
                ui.separator();
                ui.label("Opacidade:");
                let mut pct = self.document.vectors[idx].opacity * 100.0;
                if ui
                    .add(egui::Slider::new(&mut pct, 0.0..=100.0).suffix("%"))
                    .changed()
                {
                    self.document.vectors[idx].opacity = (pct / 100.0).clamp(0.0, 1.0);
                }
                ui.separator();
                if ui.button("Aplicar cor atual").clicked() {
                    self.document.vectors[idx].stroke = self.brush_core_color();
                }
                if ui
                    .button("Excluir")
                    .on_hover_text("Excluir o traço (tecla Del)")
                    .clicked()
                {
                    self.push_undo();
                    self.document.vectors.remove(idx);
                    self.selected_obj = None;
                }
            }
            _ => {
                ui.weak("Clique num traço para selecionar; arraste para mover (Del apaga).");
            }
        }
    }

    fn opcoes_texto(&mut self, ui: &mut egui::Ui) {
        ui.label("Fonte:");
        egui::ComboBox::from_id_salt("fonte_texto")
            .selected_text(FONTES[self.text_font])
            .show_ui(ui, |ui| {
                for (i, f) in FONTES.iter().enumerate() {
                    ui.selectable_value(&mut self.text_font, i, *f);
                }
            });
        ui.separator();
        ui.label("Tamanho:");
        ui.add(egui::Slider::new(&mut self.text_size, 6.0..=200.0).suffix(" pt"));
        ui.separator();
        ui.toggle_value(&mut self.text_bold, "N").on_hover_text("Negrito");
        ui.toggle_value(&mut self.text_italic, "I").on_hover_text("Itálico");
        ui.toggle_value(&mut self.text_underline, "S")
            .on_hover_text("Sublinhado");
        ui.separator();
        ui.checkbox(&mut self.text_outline, "Bordas");
        ui.separator();
        self.swatch_cor(ui);
        ui.separator();
        ui.weak("Clique no canvas para inserir texto: em desenvolvimento.");
    }

    fn opcoes_caneta(&mut self, ui: &mut egui::Ui) {
        ui.label("Espessura:");
        ui.add(egui::Slider::new(&mut self.pen_width, 1..=30));
        ui.separator();
        self.swatch_cor(ui);
        ui.separator();
        let n = self.pen_anchors.len();
        if ui
            .add_enabled(n >= 2, egui::Button::new("Finalizar traço"))
            .clicked()
        {
            self.finalizar_caneta();
        }
        if ui.add_enabled(n > 0, egui::Button::new("Cancelar")).clicked() {
            self.pen_anchors.clear();
            self.pen_drag_idx = None;
            self.dirty = true;
        }
        ui.separator();
        ui.weak(format!(
            "Clique para adicionar pontos ({n}); Enter/Finalizar cria o traço; Esc cancela."
        ));
    }

    fn opcoes_varinha(&mut self, ui: &mut egui::Ui) {
        ui.label("Tolerância:");
        ui.add(egui::Slider::new(&mut self.wand_tolerance, 0..=255));
        ui.separator();
        ui.weak("Seleção por cor: em desenvolvimento.");
    }

    fn opcoes_selecao_direta(&mut self, ui: &mut egui::Ui) {
        ui.weak("Edição por pontos (vetorial): em desenvolvimento.");
    }

    fn barra_icones(&mut self, ctx: &egui::Context) {
        use egui_phosphor::regular as icon;
        egui::SidePanel::right("barra_icones")
            .exact_width(50.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.add_space(8.0);
                ui.vertical_centered(|ui| {
                    let resp_c = icon_button(ui, self.win_color, icon::PALETTE)
                        .on_hover_text("Seleção de cores");
                    self.icon_r_color = Some(resp_c.rect);
                    if resp_c.clicked() {
                        self.win_color = !self.win_color;
                        if self.win_color {
                            self.reopen_color = true;
                        }
                    }
                    ui.add_space(6.0);

                    let resp_p = icon_button(ui, self.win_palette, icon::SWATCHES)
                        .on_hover_text("Paleta personalizada");
                    self.icon_r_palette = Some(resp_p.rect);
                    if resp_p.clicked() {
                        self.win_palette = !self.win_palette;
                        if self.win_palette {
                            self.reopen_palette = true;
                        }
                    }
                    ui.add_space(6.0);

                    let resp_l = icon_button(ui, self.win_layers, icon::STACK)
                        .on_hover_text("Camadas");
                    self.icon_r_layers = Some(resp_l.rect);
                    if resp_l.clicked() {
                        self.win_layers = !self.win_layers;
                        if self.win_layers {
                            self.reopen_layers = true;
                        }
                    }
                });
            });
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
            let pos = egui::pos2(r.left() - 8.0, r.top());
            win = win.pivot(egui::Align2::RIGHT_TOP);
            win = if self.reopen_color { win.current_pos(pos) } else { win.default_pos(pos) };
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
                        egui::Stroke::new(1.0_f32, egui::Color32::from_gray(90)),
                    );
                    if ui
                        .button(egui_phosphor::regular::EYEDROPPER)
                        .on_hover_text("Conta-gotas: capturar cor do canvas")
                        .clicked()
                    {
                        self.eyedropper = Eyedropper::ToBrush;
                    }
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
        self.reopen_color = false;
        self.win_color = open;
    }

    /// Janela: Paleta personalizada (swatches por personagem/área).
    fn janela_paletas(&mut self, ctx: &egui::Context) {
        let mut open = self.win_palette;
        let current = self.brush_core_color();

        let mut pick: Option<Color> = None;
        let mut remove_group: Option<usize> = None;
        let mut add_to_group: Option<usize> = None;
        let mut remove_color: Option<(usize, usize)> = None;
        let mut remove_char = false;

        let mut win = egui::Window::new("Paleta personalizada")
            .open(&mut open)
            .default_width(400.0);
        if let Some(r) = self.icon_r_palette {
            let pos = egui::pos2(r.left() - 8.0, r.top());
            win = win.pivot(egui::Align2::RIGHT_TOP);
            win = if self.reopen_palette { win.current_pos(pos) } else { win.default_pos(pos) };
        }
        win.show(ctx, |ui| {
            // Abas de personagem
            ui.horizontal_wrapped(|ui| {
                for (i, c) in self.library.characters.iter().enumerate() {
                    let sel = self.selected_char == Some(i);
                    if ui
                        .selectable_label(sel, egui::RichText::new(&c.name).size(15.0).strong())
                        .clicked()
                    {
                        self.selected_char = Some(i);
                    }
                }
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_char_name)
                        .desired_width(110.0)
                        .hint_text("novo personagem"),
                );
                if ui.button("+").clicked() && !self.new_char_name.trim().is_empty() {
                    let idx = self.library.add_character(self.new_char_name.trim());
                    self.selected_char = Some(idx);
                    self.new_char_name.clear();
                    self.library_dirty = true;
                }
            });
            ui.separator();

            if self.library.characters.is_empty() {
                ui.label("Crie um personagem acima.");
                return;
            }
            let Some(ci) = self.selected_char else {
                return;
            };
            if ci >= self.library.characters.len() {
                return;
            }

            // Lista de áreas: cada uma numa faixa (nome + cores em fila).
            for (gi, group) in self.library.characters[ci].groups.iter().enumerate() {
                let full_w = ui.available_width();
                let (row, resp) =
                    ui.allocate_exact_size(egui::vec2(full_w, 32.0), egui::Sense::click());
                let click = if resp.clicked() {
                    resp.interact_pointer_pos()
                } else {
                    None
                };
                let rclick = if resp.secondary_clicked() {
                    resp.interact_pointer_pos()
                } else {
                    None
                };
                {
                    let p = ui.painter();
                    p.rect_filled(row, 4.0, egui::Color32::from_rgb(0x5A, 0x70, 0x88));
                    p.text(
                        row.left_center() + egui::vec2(10.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        &group.name,
                        egui::FontId::proportional(15.0),
                        egui::Color32::WHITE,
                    );
                    let sw = 24.0;
                    let gap = 3.0;
                    let mut x = row.left() + 100.0;
                    let cy = row.center().y - sw / 2.0;
                    for (idx, nc) in group.colors.iter().enumerate() {
                        let cell = egui::Rect::from_min_size(egui::pos2(x, cy), egui::vec2(sw, sw));
                        p.rect_filled(cell, 2.0, to_color32(nc.color));
                        p.rect_stroke(cell, 2.0, egui::Stroke::new(1.0_f32, egui::Color32::from_gray(35)));
                        if let Some(cp) = click {
                            if cell.contains(cp) {
                                pick = Some(nc.color);
                            }
                        }
                        if let Some(cp) = rclick {
                            if cell.contains(cp) {
                                remove_color = Some((gi, idx));
                            }
                        }
                        x += sw + gap;
                    }
                    // conta-gotas: captura uma cor do canvas e adiciona à área
                    let eye_c = egui::pos2(row.right() - 86.0, row.center().y);
                    p.text(
                        eye_c,
                        egui::Align2::CENTER_CENTER,
                        egui_phosphor::regular::EYEDROPPER,
                        egui::FontId::proportional(16.0),
                        egui::Color32::WHITE,
                    );
                    // "+" adiciona a cor atual à área
                    let add_c = egui::pos2(row.right() - 54.0, row.center().y);
                    p.circle_stroke(add_c, 9.0_f32, egui::Stroke::new(1.5_f32, egui::Color32::WHITE));
                    p.text(
                        add_c,
                        egui::Align2::CENTER_CENTER,
                        "+",
                        egui::FontId::proportional(15.0),
                        egui::Color32::WHITE,
                    );
                    // lixeira: remove a área inteira
                    let del_c = egui::pos2(row.right() - 22.0, row.center().y);
                    p.text(
                        del_c,
                        egui::Align2::CENTER_CENTER,
                        egui_phosphor::regular::TRASH,
                        egui::FontId::proportional(16.0),
                        egui::Color32::from_gray(235),
                    );
                    if let Some(cp) = click {
                        if cp.distance(eye_c) <= 12.0 {
                            self.eyedropper = Eyedropper::ToArea(gi);
                        } else if cp.distance(add_c) <= 11.0 {
                            add_to_group = Some(gi);
                        } else if cp.distance(del_c) <= 12.0 {
                            remove_group = Some(gi);
                        }
                    }
                }
                ui.add_space(5.0);
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
            if ui.small_button("remover personagem").clicked() {
                remove_char = true;
            }
        });

        // Ações adiadas
        if let Some(ci) = self.selected_char {
            if ci < self.library.characters.len() {
                if let Some(gi) = add_to_group {
                    if gi < self.library.characters[ci].groups.len() {
                        let label = sketchmotion_color::to_hex(current);
                        self.library.characters[ci].groups[gi].add_color(label, current);
                        self.library_dirty = true;
                    }
                }
                if let Some((gi, idx)) = remove_color {
                    if gi < self.library.characters[ci].groups.len() {
                        self.library.characters[ci].groups[gi].remove_color(idx);
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
        self.reopen_palette = false;
        self.win_palette = open;
    }

    fn janela_camadas(&mut self, ctx: &egui::Context) {
        use egui_phosphor::regular as icon;
        let mut open = self.win_layers;

        let mut toggle_vis: Option<usize> = None;
        let mut toggle_lock: Option<usize> = None;
        let mut set_active: Option<usize> = None;
        let mut move_up: Option<usize> = None;
        let mut move_down: Option<usize> = None;
        let mut nova = false;
        let mut excluir = false;

        let mut win = egui::Window::new("Camadas").open(&mut open).default_width(320.0);
        if let Some(r) = self.icon_r_layers {
            let pos = egui::pos2(r.left() - 8.0, r.top());
            win = win.pivot(egui::Align2::RIGHT_TOP);
            win = if self.reopen_layers { win.current_pos(pos) } else { win.default_pos(pos) };
        }
        win.show(ctx, |ui| {
            let n = self.document.layers.len();
            // Lista do topo da pilha (índice maior) para baixo.
            for i in (0..n).rev() {
                let vis = self.document.layers[i].visible;
                let lck = self.document.layers[i].locked;
                let ativa = self.active_layer == i;
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(false, if vis { icon::EYE } else { icon::EYE_SLASH })
                        .on_hover_text("Ver/ocultar")
                        .clicked()
                    {
                        toggle_vis = Some(i);
                    }
                    if ui
                        .selectable_label(lck, if lck { icon::LOCK } else { icon::LOCK_OPEN })
                        .on_hover_text("Bloquear/desbloquear")
                        .clicked()
                    {
                        toggle_lock = Some(i);
                    }
                    if ui
                        .selectable_label(ativa, icon::CIRCLE)
                        .on_hover_text("Selecionar (camada ativa)")
                        .clicked()
                    {
                        set_active = Some(i);
                    }
                    ui.add(
                        egui::TextEdit::singleline(&mut self.document.layers[i].name)
                            .desired_width(130.0),
                    );
                    if ui.small_button(icon::ARROW_UP).clicked() {
                        move_up = Some(i);
                    }
                    if ui.small_button(icon::ARROW_DOWN).clicked() {
                        move_down = Some(i);
                    }
                });
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(icon::PLUS).on_hover_text("Nova camada").clicked() {
                    nova = true;
                }
                if ui.button(icon::TRASH).on_hover_text("Excluir camada ativa").clicked() {
                    excluir = true;
                }
                ui.label(format!("{n} camada(s)"));
            });
            ui.weak("A camada ativa recebe o desenho. Ordem = pilha (topo em cima).");
        });

        if let Some(i) = toggle_vis {
            if let Some(l) = self.document.layer_mut(i) {
                l.visible = !l.visible;
            }
            self.dirty = true;
        }
        if let Some(i) = toggle_lock {
            if let Some(l) = self.document.layer_mut(i) {
                l.locked = !l.locked;
            }
        }
        if let Some(i) = set_active {
            self.active_layer = i;
        }
        if let Some(i) = move_up {
            if i + 1 < self.document.layers.len() {
                self.document.swap_layers(i, i + 1);
                if self.active_layer == i {
                    self.active_layer = i + 1;
                } else if self.active_layer == i + 1 {
                    self.active_layer = i;
                }
                self.dirty = true;
            }
        }
        if let Some(i) = move_down {
            if i > 0 {
                self.document.swap_layers(i, i - 1);
                if self.active_layer == i {
                    self.active_layer = i - 1;
                } else if self.active_layer == i - 1 {
                    self.active_layer = i;
                }
                self.dirty = true;
            }
        }
        if nova {
            let nome = format!("Camada {}", self.document.layers.len() + 1);
            let idx = self.document.add_layer_above(self.active_layer, nome);
            self.active_layer = idx;
            self.dirty = true;
        }
        if excluir {
            self.document.remove_layer(self.active_layer);
            if self.active_layer >= self.document.layers.len() {
                self.active_layer = self.document.layers.len() - 1;
            }
            self.dirty = true;
        }

        self.reopen_layers = false;
        self.win_layers = open;
    }

    /// Tela inicial: escolher o tamanho do documento (presets ou personalizado),
    /// marcar se é pixel art, e então entrar no editor.
    fn tela_inicial(&mut self, ctx: &egui::Context) {
        let mut criar: Option<(u32, u32, bool)> = None;
        let mut abrir = false;

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.heading("SketchMotion");
                ui.label("Crie um novo documento ou abra um existente.");
            });
            ui.add_space(20.0);

            ui.label("Criar um novo arquivo:");
            ui.add_space(6.0);
            let presets = [
                ("Ilustração", 800u32, 520u32, false),
                ("Quadrado", 1024, 1024, false),
                ("HD 1920x1080", 1920, 1080, false),
                ("Pixel art 32", 32, 32, true),
                ("Pixel art 64", 64, 64, true),
                ("Pixel art 128", 128, 128, true),
            ];
            ui.horizontal_wrapped(|ui| {
                for (nome, w, h, px) in presets {
                    let texto = format!("{nome}\n{w} x {h} px");
                    if ui.add_sized([150.0, 84.0], egui::Button::new(texto)).clicked() {
                        criar = Some((w, h, px));
                    }
                }
            });

            ui.add_space(14.0);
            ui.separator();
            ui.add_space(6.0);
            ui.label("Tamanho personalizado:");
            ui.horizontal(|ui| {
                ui.label("Largura");
                ui.add(egui::DragValue::new(&mut self.home_w).range(1..=8192));
                ui.label("Altura");
                ui.add(egui::DragValue::new(&mut self.home_h).range(1..=8192));
                ui.checkbox(&mut self.home_pixel, "Pixel art");
                if ui.button("Criar").clicked() {
                    criar = Some((self.home_w, self.home_h, self.home_pixel));
                }
            });

            ui.add_space(16.0);
            if ui.button("Abrir arquivo existente...").clicked() {
                abrir = true;
            }
        });

        if let Some((w, h, px)) = criar {
            self.novo_documento(w, h, px);
            self.screen = Screen::Editor;
        }
        if abrir && self.abrir() {
            self.screen = Screen::Editor;
        }
    }
}

impl eframe::App for SketchMotionApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.screen == Screen::Home {
            self.tela_inicial(ctx);
            return;
        }
        // Ao sair da Caneta com um traço em aberto, finaliza-o (vira objeto)
        // em vez de descartá-lo — assim ele não "some" ao trocar de ferramenta.
        if self.tool != Tool::Pen && !self.pen_anchors.is_empty() {
            if self.pen_anchors.len() >= 2 {
                self.finalizar_caneta();
            } else {
                self.pen_anchors.clear();
                self.pen_drag_idx = None;
            }
        }
        let mut do_undo = false;
        let mut do_redo = false;
        let mut k_enter = false;
        let mut k_esc = false;
        let mut k_del = false;
        ctx.input(|i| {
            if i.modifiers.command && i.key_pressed(egui::Key::Z) {
                if i.modifiers.shift {
                    do_redo = true;
                } else {
                    do_undo = true;
                }
            }
            if i.modifiers.command && i.key_pressed(egui::Key::Y) {
                do_redo = true;
            }
            if i.key_pressed(egui::Key::Enter) {
                k_enter = true;
            }
            if i.key_pressed(egui::Key::Escape) {
                k_esc = true;
            }
            if i.key_pressed(egui::Key::Delete) {
                k_del = true;
            }
        });
        let editando = ctx.wants_keyboard_input();
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

        let mut a_novo = false;
        let mut a_abrir = false;
        let mut a_salvar = false;
        let mut a_salvar_como = false;
        let mut a_exportar = false;
        let mut img_rccw = false;
        let mut img_rcw = false;
        let mut img_r180 = false;
        let mut img_fh = false;
        let mut img_fv = false;
        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("Arquivo", |ui| {
                    if ui.button("Novo...").clicked() {
                        a_novo = true;
                        ui.close_menu();
                    }
                    if ui.button("Abrir...").clicked() {
                        a_abrir = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Salvar").clicked() {
                        a_salvar = true;
                        ui.close_menu();
                    }
                    if ui.button("Salvar como...").clicked() {
                        a_salvar_como = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Exportar...").clicked() {
                        a_exportar = true;
                        ui.close_menu();
                    }
                });
                ui.menu_button("Imagem", |ui| {
                    if ui.button("Girar 90° à esquerda").clicked() {
                        img_rccw = true;
                        ui.close_menu();
                    }
                    if ui.button("Girar 90° à direita").clicked() {
                        img_rcw = true;
                        ui.close_menu();
                    }
                    if ui.button("Girar 180°").clicked() {
                        img_r180 = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Espelhar horizontal").clicked() {
                        img_fh = true;
                        ui.close_menu();
                    }
                    if ui.button("Espelhar vertical").clicked() {
                        img_fv = true;
                        ui.close_menu();
                    }
                });
                if !self.status.is_empty() {
                    ui.separator();
                    ui.label(&self.status);
                }
            });
        });
        if a_novo {
            self.screen = Screen::Home;
        }
        if a_abrir {
            self.abrir();
        }
        if a_salvar {
            self.salvar();
        }
        if a_salvar_como {
            self.salvar_como();
        }
        if a_exportar {
            self.exportar();
        }
        if img_rccw || img_rcw || img_r180 || img_fh || img_fv {
            self.push_undo();
            if img_rccw { self.document.rotate_90_ccw(); }
            if img_rcw { self.document.rotate_90_cw(); }
            if img_r180 { self.document.rotate_180(); }
            if img_fh { self.document.flip_h(); }
            if img_fv { self.document.flip_v(); }
            self.last_pos = None;
            self.dirty = true;
            self.status = "Transformação aplicada ao desenho".into();
        }
        if !editando {
            if k_enter && self.tool == Tool::Pen {
                self.finalizar_caneta();
            }
            if k_esc {
                if !self.pen_anchors.is_empty() {
                    self.pen_anchors.clear();
                    self.pen_drag_idx = None;
                    self.dirty = true;
                }
                self.selected_obj = None;
            }
            if k_del && self.tool == Tool::Select {
                if let Some(i) = self.selected_obj {
                    if i < self.document.vectors.len() {
                        self.push_undo();
                        self.document.vectors.remove(i);
                        self.selected_obj = None;
                        self.dirty = true;
                    }
                }
            }
        }

        // Barra de opções da ferramenta ativa (abaixo do menu).
        self.barra_opcoes(ctx);
        // Ferramentas (esquerda) e painéis (direita), sempre visíveis.
        self.barra_ferramentas(ctx);
        self.barra_icones(ctx);
        self.janela_cor(ctx);
        self.janela_paletas(ctx);
        self.janela_camadas(ctx);

        egui::CentralPanel::default().show(ctx, |ui| {
            let doc_w = self.document.width as f32;
            let doc_h = self.document.height as f32;
            // Zoom efetivo: inteiro no pixel art (nitidez), livre na ilustração.
            let zoom = if self.pixel_mode {
                self.zoom.round().max(1.0)
            } else {
                self.zoom.max(0.05)
            };
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .drag_to_scroll(false)
                .show(ui, |ui| {
                    let size = egui::vec2(doc_w * zoom, doc_h * zoom);
                    let image =
                        egui::Image::from_texture(egui::load::SizedTexture::new(tex_id, size))
                            .fit_to_exact_size(size)
                            .sense(egui::Sense::click_and_drag());
                    let response = ui.add(image);
                    let rect = response.rect;

                    if self.pixel_mode && zoom >= 6.0 {
                        let painter = ui.painter_at(rect);
                        let cor = egui::Color32::from_rgba_unmultiplied(120, 120, 120, 90);
                        for i in 0..=self.document.width {
                            let x = rect.left() + i as f32 * zoom;
                            painter.line_segment(
                                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                                egui::Stroke::new(1.0_f32, cor),
                            );
                        }
                        for j in 0..=self.document.height {
                            let y = rect.top() + j as f32 * zoom;
                            painter.line_segment(
                                [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                                egui::Stroke::new(1.0_f32, cor),
                            );
                        }
                    }

                    let to_pixel = |pos: egui::Pos2| -> (i32, i32) {
                        let l = pos - rect.min;
                        ((l.x / zoom).floor() as i32, (l.y / zoom).floor() as i32)
                    };
                    let to_doc = |pos: egui::Pos2| -> (f32, f32) {
                        ((pos.x - rect.min.x) / zoom, (pos.y - rect.min.y) / zoom)
                    };
                    // Leitura do ponteiro em baixo nível (a ScrollArea filtra
                    // clicked()/drag_started(); estes primitivos não passam por ela).
                    let hover = response.hover_pos();
                    let pressed = ui.input(|i| i.pointer.primary_pressed());
                    let down = ui.input(|i| i.pointer.primary_down());
                    let pdelta = ui.input(|i| i.pointer.delta());
                    let ppos = ui.input(|i| i.pointer.latest_pos());

                    if self.eyedropper != Eyedropper::Off {
                        if pressed {
                            if let Some(pointer) = hover {
                                let (x, y) = to_pixel(pointer);
                                let cor = self.cor_no_pixel(x, y);
                                self.brush_color = to_color32(cor);
                                if let Eyedropper::ToArea(gi) = self.eyedropper {
                                    if let Some(ci) = self.selected_char {
                                        if ci < self.library.characters.len()
                                            && gi < self.library.characters[ci].groups.len()
                                        {
                                            let label = sketchmotion_color::to_hex(cor);
                                            self.library.characters[ci].groups[gi]
                                                .add_color(label, cor);
                                            self.library_dirty = true;
                                        }
                                    }
                                }
                                self.eyedropper = Eyedropper::Off;
                                self.tool = Tool::Pencil;
                                self.status =
                                    format!("Cor {} capturada", sketchmotion_color::to_hex(cor));
                            }
                        }
                        self.last_pos = None;
                    } else if self.tool == Tool::Pen {
                        if response.double_clicked() {
                            self.finalizar_caneta();
                        } else {
                            if pressed {
                                if let Some(p) = hover {
                                    let (dx, dy) = to_doc(p);
                                    self.pen_anchors.push(Anchor::new(dx, dy));
                                    self.pen_drag_idx = Some(self.pen_anchors.len() - 1);
                                }
                            }
                            if down {
                                if let Some(i) = self.pen_drag_idx {
                                    if i < self.pen_anchors.len() {
                                        if let Some(pp) = ppos {
                                            let (cx, cy) = to_doc(pp);
                                            let a = &mut self.pen_anchors[i];
                                            let (ddx, ddy) = (cx - a.x, cy - a.y);
                                            if ddx * ddx + ddy * ddy > 4.0 {
                                                a.hout = Some((cx, cy));
                                                a.hin = Some((a.x - ddx, a.y - ddy));
                                            }
                                        }
                                    }
                                }
                            }
                            if !down {
                                self.pen_drag_idx = None;
                            }
                        }
                        self.last_pos = None;
                    } else if self.tool == Tool::Select {
                        let thr = 6.0 / zoom;
                        if pressed {
                            if let Some(p) = hover {
                                let mut grabbed = false;
                                if let Some(si) = self.selected_obj {
                                    if si < self.document.vectors.len() {
                                        if let Some((minx, miny, maxx, maxy)) =
                                            self.document.vectors[si].bounds()
                                        {
                                            let r = egui::Rect::from_min_max(
                                                egui::pos2(
                                                    rect.min.x + minx * zoom,
                                                    rect.min.y + miny * zoom,
                                                ),
                                                egui::pos2(
                                                    rect.min.x + maxx * zoom,
                                                    rect.min.y + maxy * zoom,
                                                ),
                                            )
                                            .expand(3.0);
                                            let hs = handle_positions(r);
                                            let (cx, cy) =
                                                ((minx + maxx) / 2.0, (miny + maxy) / 2.0);
                                            for (hi, hc) in hs.iter().enumerate() {
                                                if hc.distance(p) <= 8.0 {
                                                    self.push_undo();
                                                    let (grab, fixed, axes) = handle_geometry(
                                                        hi, minx, miny, maxx, maxy, cx, cy,
                                                    );
                                                    self.resize_handle = Some(hi);
                                                    self.resize_orig =
                                                        self.document.vectors[si].points.clone();
                                                    self.resize_grab = grab;
                                                    self.resize_fixed = fixed;
                                                    self.resize_axes = axes;
                                                    self.dragging_obj = false;
                                                    grabbed = true;
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                }
                                if !grabbed {
                                    match self.hit_test(to_doc(p), thr) {
                                        Some(i) => {
                                            self.selected_obj = Some(i);
                                            self.push_undo();
                                            self.dragging_obj = true;
                                        }
                                        None => {
                                            self.selected_obj = None;
                                            self.dragging_obj = false;
                                        }
                                    }
                                }
                            }
                        }
                        if down && self.resize_handle.is_some() {
                            if let Some(si) = self.selected_obj {
                                if si < self.document.vectors.len() {
                                    if let Some(pp) = ppos {
                                        let cur = to_doc(pp);
                                        let (fx, fy) = self.resize_fixed;
                                        let (gx, gy) = self.resize_grab;
                                        let (ax, ay) = self.resize_axes;
                                        let sx = if ax {
                                            safe_ratio(cur.0 - fx, gx - fx)
                                        } else {
                                            1.0
                                        };
                                        let sy = if ay {
                                            safe_ratio(cur.1 - fy, gy - fy)
                                        } else {
                                            1.0
                                        };
                                        let orig = self.resize_orig.clone();
                                        let obj = &mut self.document.vectors[si];
                                        if obj.points.len() == orig.len() {
                                            for (dst, src) in
                                                obj.points.iter_mut().zip(orig.iter())
                                            {
                                                dst.x = fx + (src.x - fx) * sx;
                                                dst.y = fy + (src.y - fy) * sy;
                                                dst.hin = src.hin.map(|(hx, hy)| {
                                                    (fx + (hx - fx) * sx, fy + (hy - fy) * sy)
                                                });
                                                dst.hout = src.hout.map(|(hx, hy)| {
                                                    (fx + (hx - fx) * sx, fy + (hy - fy) * sy)
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                        } else if down && self.dragging_obj {
                            if let Some(i) = self.selected_obj {
                                if i < self.document.vectors.len()
                                    && (pdelta.x != 0.0 || pdelta.y != 0.0)
                                {
                                    self.document.vectors[i]
                                        .translate(pdelta.x / zoom, pdelta.y / zoom);
                                }
                            }
                        }
                        if !down {
                            self.dragging_obj = false;
                            self.resize_handle = None;
                        }
                        self.last_pos = None;
                    } else if self.tool.paints()
                        && (response.is_pointer_button_down_on() || response.dragged())
                    {
                        let mut pontos: Vec<(i32, i32)> = ui.input(|i| {
                            i.events
                                .iter()
                                .filter_map(|e| {
                                    if let egui::Event::PointerMoved(pos) = e {
                                        Some(to_pixel(*pos))
                                    } else {
                                        None
                                    }
                                })
                                .collect()
                        });
                        if let Some(pos) = response.interact_pointer_pos() {
                            pontos.push(to_pixel(pos));
                        }
                        if self.last_pos.is_none() && !pontos.is_empty() {
                            self.push_undo();
                        }
                        for p in pontos {
                            match self.last_pos {
                                Some(prev) => self.paint_line(prev, p),
                                None => self.paint_dab(p.0, p.1),
                            }
                            self.last_pos = Some(p);
                        }
                    } else {
                        self.last_pos = None;
                    }

                    // Cursor contextual: muda conforme a ferramenta e o que
                    // está sob o ponteiro (objeto interativo / alça).
                    if response.hovered() {
                        use egui::CursorIcon as CI;
                        let ci = if self.eyedropper != Eyedropper::Off {
                            CI::Crosshair
                        } else {
                            match self.tool {
                                Tool::Pen | Tool::Pencil | Tool::Eraser => CI::Crosshair,
                                Tool::Select => {
                                    if self.dragging_obj || self.resize_handle.is_some() {
                                        CI::Grabbing
                                    } else if let (Some(si), Some(hp)) = (self.selected_obj, hover) {
                                        if let Some(hi) = self.handle_at(si, hp, rect, zoom) {
                                            match hi {
                                                0 | 4 => CI::ResizeNwSe,
                                                2 | 6 => CI::ResizeNeSw,
                                                1 | 5 => CI::ResizeVertical,
                                                _ => CI::ResizeHorizontal,
                                            }
                                        } else if self.hit_test(to_doc(hp), 6.0 / zoom).is_some() {
                                            CI::Grab
                                        } else {
                                            CI::Default
                                        }
                                    } else if let Some(hp) = hover {
                                        if self.hit_test(to_doc(hp), 6.0 / zoom).is_some() {
                                            CI::Grab
                                        } else {
                                            CI::Default
                                        }
                                    } else {
                                        CI::Default
                                    }
                                }
                                _ => CI::Default,
                            }
                        };
                        ui.ctx().set_cursor_icon(ci);
                    }

                    // Overlay vetorial: objetos, seleção e traço em progresso.
                    self.desenhar_vetores(ui, rect, zoom);
                });
        });
        let mut do_zoom_in = false;
        let mut do_zoom_out = false;
        egui::Area::new(egui::Id::new("acoes_canvas"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-66.0, -16.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let can_undo = !self.undo_stack.is_empty();
                        let can_redo = !self.redo_stack.is_empty();
                        if ui
                            .add_enabled(
                                can_undo,
                                egui::Button::new(egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE)
                                    .min_size(egui::vec2(32.0, 32.0)),
                            )
                            .on_hover_text("Desfazer (Ctrl+Z)")
                            .clicked()
                        {
                            do_undo = true;
                        }
                        if ui
                            .add_enabled(
                                can_redo,
                                egui::Button::new(egui_phosphor::regular::ARROW_CLOCKWISE)
                                    .min_size(egui::vec2(32.0, 32.0)),
                            )
                            .on_hover_text("Refazer (Ctrl+Shift+Z)")
                            .clicked()
                        {
                            do_redo = true;
                        }
                        ui.separator();
                        if ui
                            .button(egui_phosphor::regular::MAGNIFYING_GLASS_MINUS)
                            .on_hover_text("Diminuir zoom")
                            .clicked()
                        {
                            do_zoom_out = true;
                        }
                        let efetivo = if self.pixel_mode {
                            self.zoom.round().max(1.0)
                        } else {
                            self.zoom
                        };
                        ui.label(format!("{}%", (efetivo * 100.0).round() as i32));
                        if ui
                            .button(egui_phosphor::regular::MAGNIFYING_GLASS_PLUS)
                            .on_hover_text("Aumentar zoom")
                            .clicked()
                        {
                            do_zoom_in = true;
                        }
                    });
                });
            });
        if do_undo {
            self.undo();
        }
        if do_redo {
            self.redo();
        }
        if do_zoom_in {
            self.zoom = (self.zoom * 1.25).min(64.0);
        }
        if do_zoom_out {
            self.zoom = (self.zoom / 1.25).max(0.1);
        }

        if self.library_dirty {
            self.salvar_biblioteca();
            self.library_dirty = false;
        }
    }
}
