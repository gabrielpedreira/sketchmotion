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
use sketchmotion_render::{render_document, render_frame, render_frame_alpha, PixelImage};
use sketchmotion_tools::Tool;

const CANVAS_W: u32 = 800;
const CANVAS_H: u32 = 520;
const MAX_UNDO: usize = 10;

/// Nº de colunas da grade de cores básicas.
const BASICAS_COLS: usize = 16;

/// Fontes exibidas no painel de texto (aplicação real virá com o módulo de texto).
const FONTES: [&str; 4] = ["Sans", "Serif", "Monospace", "Manuscrito"];
const FORMAS: [&str; 4] = ["Retângulo", "Elipse", "Triângulo", "Polígono"];

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

/// Reduz uma imagem RGBA a uma miniatura (amostragem nearest).
fn thumb_image(full: &PixelImage, tw: usize, th: usize) -> egui::ColorImage {
    let (fw, fh) = (full.width as usize, full.height as usize);
    let mut out = vec![0u8; tw * th * 4];
    for ty in 0..th {
        for tx in 0..tw {
            let sx = (tx * fw / tw).min(fw.saturating_sub(1));
            let sy = (ty * fh / th).min(fh.saturating_sub(1));
            let si = (sy * fw + sx) * 4;
            let di = (ty * tw + tx) * 4;
            out[di..di + 4].copy_from_slice(&full.rgba[si..si + 4]);
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([tw, th], &out)
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

/// Posição (tela) do ponto de rotação: acima do centro-topo da caixa.
fn rotate_handle_screen(r: egui::Rect) -> egui::Pos2 {
    egui::pos2(r.center().x, r.top() - 22.0)
}

/// Sinais locais das 8 alças (cantos e meios), na ordem canônica.
const HSIGNS: [(f32, f32); 8] = [
    (-1.0, -1.0),
    (0.0, -1.0),
    (1.0, -1.0),
    (1.0, 0.0),
    (1.0, 1.0),
    (0.0, 1.0),
    (-1.0, 1.0),
    (-1.0, 0.0),
];

/// Ponto (mundo) de um canto/lado da seleção flutuante girada.
fn float_corner(cx: f32, cy: f32, hw: f32, hh: f32, ang: f32, sx: f32, sy: f32) -> (f32, f32) {
    let (s, c) = ang.sin_cos();
    let (lx, ly) = (sx * hw, sy * hh);
    (cx + lx * c - ly * s, cy + lx * s + ly * c)
}

/// Converte um ponto do mundo para o sistema local (não girado) da seleção.
fn float_local(cx: f32, cy: f32, ang: f32, wx: f32, wy: f32) -> (f32, f32) {
    let (s, c) = ang.sin_cos();
    let (dx, dy) = (wx - cx, wy - cy);
    (dx * c + dy * s, -dx * s + dy * c)
}

/// Teste ponto-dentro-do-polígono (ray casting) para a seleção livre.
fn ponto_no_poligono(x: f32, y: f32, poly: &[(f32, f32)]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if ((yi > y) != (yj > y))
            && (x < (xj - xi) * (y - yi) / (yj - yi + f32::EPSILON) + xi)
        {
            inside = !inside;
        }
        j = i;
    }
    inside
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

/// Reamostra uma polilinha inserindo pontos ~a cada `spacing` (para apagar
/// trechos finos de um traço, não só nos vértices).
fn densify(pts: &[(f32, f32)], spacing: f32) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    if pts.is_empty() {
        return out;
    }
    out.push(pts[0]);
    for w in pts.windows(2) {
        let (ax, ay) = w[0];
        let (bx, by) = w[1];
        let len = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
        let n = (len / spacing.max(0.5)).ceil().max(1.0) as usize;
        for k in 1..=n {
            let t = k as f32 / n as f32;
            out.push((ax + (bx - ax) * t, ay + (by - ay) * t));
        }
    }
    out
}

/// Carimba um disco opaco de cor `col` no buffer RGBA (para rasterizar bordas).
fn stamp_disc(rgba: &mut [u8], w: u32, h: u32, cx: f32, cy: f32, r: f32, col: Color) {
    let r2 = r * r;
    let x0 = ((cx - r).floor() as i32).max(0);
    let x1 = ((cx + r).ceil() as i32).min(w as i32 - 1);
    let y0 = ((cy - r).floor() as i32).max(0);
    let y1 = ((cy + r).ceil() as i32).min(h as i32 - 1);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            if dx * dx + dy * dy <= r2 {
                let i = ((y as u32 * w + x as u32) * 4) as usize;
                rgba[i] = col.r;
                rgba[i + 1] = col.g;
                rgba[i + 2] = col.b;
                rgba[i + 3] = 255;
            }
        }
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
/// Seleção retangular de pixels recortada de uma camada (raster), flutuando
/// sobre o canvas até ser movida e confirmada (estilo Paint).
struct FloatSel {
    pixels: Vec<u8>,
    ow: u32,
    oh: u32,
    cx: f32,
    cy: f32,
    hw: f32,
    hh: f32,
    angle: f32,
    opacity: f32,
}

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
    // ferramenta Formas
    shape_kind: usize,
    shape_sides: u32,
    shape_stroke: i32,
    shape_fill: bool,
    fill_color: egui::Color32,
    shape_start: Option<(f32, f32)>,
    eraser_radius: i32,
    // rotação manual do objeto selecionado
    rotating: bool,
    rotate_center: (f32, f32),
    rotate_start: f32,
    rotate_orig: Vec<Anchor>,
    // seleção retangular raster (estilo Paint)
    float_sel: Option<FloatSel>,
    float_tex: Option<egui::TextureHandle>,
    float_dragging: bool,
    float_grab: (f32, f32),
    float_resize: Option<(f32, f32)>,
    fr_fixed: (f32, f32),
    fr_wh: (f32, f32),
    fr_angle: f32,
    float_rotating: bool,
    float_rot_grab: f32,
    marquee_start: Option<(i32, i32)>,
    marquee_cur: (i32, i32),
    lasso_points: Vec<(f32, f32)>,
    fill_tolerance: i32,
    // frames / animação
    onion: bool,
    frame_thumbs: Vec<Option<egui::TextureHandle>>,
    onion_tex: Option<egui::TextureHandle>,
    onion_for: Option<usize>,
    playing: bool,
    play_frame: usize,
    play_accum: f32,
    play_loop: bool,
    play_done: bool,
    play_tex: Option<egui::TextureHandle>,
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
            shape_kind: 0,
            shape_sides: 5,
            shape_stroke: 2,
            shape_fill: true,
            fill_color: egui::Color32::from_rgb(180, 180, 180),
            shape_start: None,
            eraser_radius: 8,
            rotating: false,
            rotate_center: (0.0, 0.0),
            rotate_start: 0.0,
            rotate_orig: Vec::new(),
            float_sel: None,
            float_tex: None,
            float_dragging: false,
            float_grab: (0.0, 0.0),
            float_resize: None,
            fr_fixed: (0.0, 0.0),
            fr_wh: (0.0, 0.0),
            fr_angle: 0.0,
            float_rotating: false,
            float_rot_grab: 0.0,
            marquee_start: None,
            marquee_cur: (0, 0),
            lasso_points: Vec::new(),
            fill_tolerance: 24,
            onion: true,
            frame_thumbs: Vec::new(),
            onion_tex: None,
            onion_for: None,
            playing: false,
            play_frame: 0,
            play_accum: 0.0,
            play_loop: true,
            play_done: false,
            play_tex: None,
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

    /// Raio ativo do desenho: borracha usa o próprio tamanho.
    fn active_radius(&self) -> i32 {
        if self.tool == Tool::Eraser {
            self.eraser_radius
        } else {
            self.brush_radius
        }
    }

    fn paint_dab(&mut self, x: i32, y: i32) {
        let color = self.active_color();
        let r = self.active_radius();
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
        self.frame_thumbs.clear();
        self.onion_tex = None;
        self.onion_for = None;
        self.status = if pixel {
            format!("Novo documento pixel art {w}x{h}")
        } else {
            format!("Novo documento {w}x{h}")
        };
    }

    fn salvar(&mut self) {
        self.document.sync_to_frames();
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
        self.document.sync_to_frames();
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
            let img = render_frame(
                self.document.width,
                self.document.height,
                self.document.background,
                &self.document.layers,
                &self.document.vectors,
            );
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
                    self.frame_thumbs.clear();
                    self.onion_tex = None;
                    self.onion_for = None;
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

    /// Apaga apenas a parte dos traços vetoriais que passa pelo círculo da
    /// borracha (raio em coords do documento), dividindo o traço em pedaços.
    /// Traços viram polilinhas ao serem apagados (perdem as curvas naquele ponto).
    fn erase_vetores(&mut self, pos: (f32, f32), radius: f32) {
        let r2 = radius * radius;
        let vectors = std::mem::take(&mut self.document.vectors);
        let mut result: Vec<VectorObject> = Vec::with_capacity(vectors.len());
        let mut mudou = false;
        for obj in vectors {
            // descarte rápido por caixa
            let dentro = obj.bounds().map_or(false, |(minx, miny, maxx, maxy)| {
                pos.0 >= minx - radius
                    && pos.0 <= maxx + radius
                    && pos.1 >= miny - radius
                    && pos.1 <= maxy + radius
            });
            if !dentro {
                result.push(obj);
                continue;
            }
            let dense = densify(&obj.flatten(32), (radius * 0.5).max(1.0));
            let mut runs: Vec<Vec<(f32, f32)>> = Vec::new();
            let mut cur: Vec<(f32, f32)> = Vec::new();
            let mut apagou = false;
            for &(x, y) in &dense {
                if (x - pos.0).powi(2) + (y - pos.1).powi(2) <= r2 {
                    apagou = true;
                    if cur.len() >= 2 {
                        runs.push(std::mem::take(&mut cur));
                    } else {
                        cur.clear();
                    }
                } else {
                    cur.push((x, y));
                }
            }
            if cur.len() >= 2 {
                runs.push(cur);
            }
            if !apagou {
                result.push(obj);
            } else {
                mudou = true;
                for run in runs {
                    let mut no = VectorObject::new(obj.stroke, obj.stroke_width);
                    no.opacity = obj.opacity;
                    no.points = run.iter().map(|(x, y)| Anchor::new(*x, *y)).collect();
                    result.push(no);
                }
            }
        }
        self.document.vectors = result;
        if mudou {
            self.selected_obj = None;
            self.dirty = true;
        }
    }

    /// Recorta a região retangular (coords do documento) da camada ativa para
    /// uma seleção flutuante e limpa esses pixels na camada.
    fn lift_selection(&mut self, start: (i32, i32), end: (i32, i32)) {
        let x0 = start.0.min(end.0).max(0);
        let y0 = start.1.min(end.1).max(0);
        let x1 = start.0.max(end.0).min(self.document.width as i32);
        let y1 = start.1.max(end.1).min(self.document.height as i32);
        if x1 - x0 < 1 || y1 - y0 < 1 {
            return;
        }
        let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
        self.push_undo();
        let li = self.active_layer;
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        if let Some(layer) = self.document.layer_mut(li) {
            for yy in 0..h {
                for xx in 0..w {
                    let (px, py) = (x0 as u32 + xx, y0 as u32 + yy);
                    if let Some(c) = layer.get_pixel(px, py) {
                        let di = ((yy * w + xx) * 4) as usize;
                        pixels[di] = c.r;
                        pixels[di + 1] = c.g;
                        pixels[di + 2] = c.b;
                        pixels[di + 3] = c.a;
                        layer.set_pixel(px, py, Color::TRANSPARENT);
                    }
                }
            }
        }
        self.float_sel = Some(FloatSel {
            pixels,
            ow: w,
            oh: h,
            cx: x0 as f32 + w as f32 / 2.0,
            cy: y0 as f32 + h as f32 / 2.0,
            hw: w as f32 / 2.0,
            hh: h as f32 / 2.0,
            angle: 0.0,
            opacity: 1.0,
        });
        self.float_tex = None;
        self.dirty = true;
        self.status = "Seleção recortada — arraste para mover".into();
    }

    /// Carimba a seleção flutuante de volta na camada ativa (alpha over).
    fn commit_float(&mut self) {
        if let Some(fs) = self.float_sel.take() {
            let li = self.active_layer;
            let (dw, dh) = (self.document.width as i32, self.document.height as i32);
            let corners = [
                float_corner(fs.cx, fs.cy, fs.hw, fs.hh, fs.angle, -1.0, -1.0),
                float_corner(fs.cx, fs.cy, fs.hw, fs.hh, fs.angle, 1.0, -1.0),
                float_corner(fs.cx, fs.cy, fs.hw, fs.hh, fs.angle, 1.0, 1.0),
                float_corner(fs.cx, fs.cy, fs.hw, fs.hh, fs.angle, -1.0, 1.0),
            ];
            let (mut minx, mut miny) = (f32::INFINITY, f32::INFINITY);
            let (mut maxx, mut maxy) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
            for (x, y) in corners {
                minx = minx.min(x);
                miny = miny.min(y);
                maxx = maxx.max(x);
                maxy = maxy.max(y);
            }
            let x0 = (minx.floor() as i32).max(0);
            let y0 = (miny.floor() as i32).max(0);
            let x1 = (maxx.ceil() as i32).min(dw);
            let y1 = (maxy.ceil() as i32).min(dh);
            if let Some(layer) = self.document.layer_mut(li) {
                for py in y0..y1 {
                    for px in x0..x1 {
                        let (lx, ly) =
                            float_local(fs.cx, fs.cy, fs.angle, px as f32 + 0.5, py as f32 + 0.5);
                        let u = lx / (2.0 * fs.hw) + 0.5;
                        let v = ly / (2.0 * fs.hh) + 0.5;
                        if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                            continue;
                        }
                        let sx = ((u * fs.ow as f32) as u32).min(fs.ow.saturating_sub(1));
                        let sy = ((v * fs.oh as f32) as u32).min(fs.oh.saturating_sub(1));
                        let si = ((sy * fs.ow + sx) * 4) as usize;
                        let a = (fs.pixels[si + 3] as f32 * fs.opacity).round() as u32;
                        if a == 0 {
                            continue;
                        }
                        let src =
                            Color::rgba(fs.pixels[si], fs.pixels[si + 1], fs.pixels[si + 2], a as u8);
                        let (upx, upy) = (px as u32, py as u32);
                        if a >= 255 {
                            layer.set_pixel(upx, upy, src);
                        } else if let Some(d) = layer.get_pixel(upx, upy) {
                            let ia = 255 - a;
                            let bl = |sc: u8, dd: u8| ((sc as u32 * a + dd as u32 * ia) / 255) as u8;
                            let na = (a + (d.a as u32) * ia / 255).min(255) as u8;
                            layer.set_pixel(
                                upx,
                                upy,
                                Color::rgba(bl(src.r, d.r), bl(src.g, d.g), bl(src.b, d.b), na.max(a as u8)),
                            );
                        }
                    }
                }
            }
            self.float_tex = None;
            self.dirty = true;
        }
    }

    /// Detecção do clique sobre a seleção flutuante (rotação / alça / mover).
    /// Devolve true se o clique foi consumido por ela.
    fn float_press(&mut self, p: egui::Pos2, rect: egui::Rect, zoom: f32) -> bool {
        let dp = ((p.x - rect.min.x) / zoom, (p.y - rect.min.y) / zoom);
        let mut consumed = false;
        if let Some(fs) = &self.float_sel {
            let (cx, cy, hw, hh, ang) = (fs.cx, fs.cy, fs.hw, fs.hh, fs.angle);
            let scr = |wx: f32, wy: f32| egui::pos2(rect.min.x + wx * zoom, rect.min.y + wy * zoom);
            let (tmx, tmy) = float_corner(cx, cy, hw, hh, ang, 0.0, -1.0);
            let tm = scr(tmx, tmy);
            let cc = scr(cx, cy);
            let dir = (tm - cc).normalized();
            let roth = tm + dir * 22.0;
            if roth.distance(p) <= 12.0 {
                self.float_rotating = true;
                self.float_rot_grab = (dp.1 - cy).atan2(dp.0 - cx) - ang;
                consumed = true;
            } else {
                let mut grabbed_h = None;
                for &(sx, sy) in HSIGNS.iter() {
                    let (hx, hy) = float_corner(cx, cy, hw, hh, ang, sx, sy);
                    if scr(hx, hy).distance(p) <= 8.0 {
                        grabbed_h = Some((sx, sy));
                        break;
                    }
                }
                if let Some((sx, sy)) = grabbed_h {
                    self.fr_fixed = float_corner(cx, cy, hw, hh, ang, -sx, -sy);
                    self.fr_wh = (2.0 * hw, 2.0 * hh);
                    self.fr_angle = ang;
                    self.float_resize = Some((sx, sy));
                    consumed = true;
                } else {
                    let (lx, ly) = float_local(cx, cy, ang, dp.0, dp.1);
                    if lx.abs() <= hw && ly.abs() <= hh {
                        self.float_grab = (dp.0 - cx, dp.1 - cy);
                        self.float_dragging = true;
                        consumed = true;
                    }
                }
            }
        }
        consumed
    }

    /// Aplica a manipulação da seleção flutuante em andamento (redimensionar/
    /// girar/mover). Devolve true se algo estava ativo.
    fn float_down(&mut self, ppos: Option<egui::Pos2>, rect: egui::Rect, zoom: f32) -> bool {
        if !(self.float_resize.is_some() || self.float_rotating || self.float_dragging) {
            return false;
        }
        if let Some(pp) = ppos {
            let cur = ((pp.x - rect.min.x) / zoom, (pp.y - rect.min.y) / zoom);
            if let Some((gx, gy)) = self.float_resize {
                let ang = self.fr_angle;
                let (pfx, pfy) = self.fr_fixed;
                let (w0, h0) = self.fr_wh;
                let (s, c) = ang.sin_cos();
                let (dx, dy) = (cur.0 - pfx, cur.1 - pfy);
                let (lx, ly) = (dx * c + dy * s, -dx * s + dy * c);
                let nw = if gx != 0.0 { lx.abs().max(1.0) } else { w0 };
                let nh = if gy != 0.0 { ly.abs().max(1.0) } else { h0 };
                let (hw, hh) = (nw / 2.0, nh / 2.0);
                let (flx, fly) = (-gx * hw, -gy * hh);
                let cxn = pfx - (flx * c - fly * s);
                let cyn = pfy - (flx * s + fly * c);
                if let Some(fs) = &mut self.float_sel {
                    fs.cx = cxn;
                    fs.cy = cyn;
                    fs.hw = hw;
                    fs.hh = hh;
                }
            } else if self.float_rotating {
                let grab = self.float_rot_grab;
                if let Some(fs) = &mut self.float_sel {
                    fs.angle = (cur.1 - fs.cy).atan2(cur.0 - fs.cx) - grab;
                }
            } else if self.float_dragging {
                let (gx, gy) = self.float_grab;
                if let Some(fs) = &mut self.float_sel {
                    fs.cx = cur.0 - gx;
                    fs.cy = cur.1 - gy;
                }
            }
            self.dirty = true;
        }
        true
    }

    fn float_release(&mut self) {
        self.float_dragging = false;
        self.float_resize = None;
        self.float_rotating = false;
    }

    /// Recorta os pixels dentro do contorno (seleção livre) para uma flutuante.
    fn lift_lasso(&mut self, pts: &[(f32, f32)]) {
        if pts.len() < 3 {
            return;
        }
        let (mut minx, mut miny) = (f32::INFINITY, f32::INFINITY);
        let (mut maxx, mut maxy) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
        for &(x, y) in pts {
            minx = minx.min(x);
            miny = miny.min(y);
            maxx = maxx.max(x);
            maxy = maxy.max(y);
        }
        let x0 = (minx.floor() as i32).max(0);
        let y0 = (miny.floor() as i32).max(0);
        let x1 = (maxx.ceil() as i32).min(self.document.width as i32);
        let y1 = (maxy.ceil() as i32).min(self.document.height as i32);
        if x1 - x0 < 1 || y1 - y0 < 1 {
            return;
        }
        let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
        self.push_undo();
        let li = self.active_layer;
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        if let Some(layer) = self.document.layer_mut(li) {
            for yy in 0..h {
                for xx in 0..w {
                    let wx = x0 as f32 + xx as f32 + 0.5;
                    let wy = y0 as f32 + yy as f32 + 0.5;
                    if ponto_no_poligono(wx, wy, pts) {
                        let (px, py) = (x0 as u32 + xx, y0 as u32 + yy);
                        if let Some(c) = layer.get_pixel(px, py) {
                            let di = ((yy * w + xx) * 4) as usize;
                            pixels[di] = c.r;
                            pixels[di + 1] = c.g;
                            pixels[di + 2] = c.b;
                            pixels[di + 3] = c.a;
                            layer.set_pixel(px, py, Color::TRANSPARENT);
                        }
                    }
                }
            }
        }
        self.float_sel = Some(FloatSel {
            pixels,
            ow: w,
            oh: h,
            cx: x0 as f32 + w as f32 / 2.0,
            cy: y0 as f32 + h as f32 / 2.0,
            hw: w as f32 / 2.0,
            hh: h as f32 / 2.0,
            angle: 0.0,
            opacity: 1.0,
        });
        self.float_tex = None;
        self.dirty = true;
        self.status = "Seleção livre recortada — arraste para mover".into();
    }

    fn opcoes_laco(&mut self, ui: &mut egui::Ui) {
        if self.float_sel.is_some() {
            self.opcoes_selecao(ui);
        } else {
            ui.weak("Contorne uma área à mão livre (arraste) para selecionar os pixels.");
        }
    }

    /// Ferramentas que não trabalham em pixels ficam bloqueadas no pixel art.
    fn bloqueada_pixel(&self, t: Tool) -> bool {
        self.pixel_mode
            && matches!(
                t,
                Tool::Pen | Tool::Shapes | Tool::Text | Tool::DirectSelect | Tool::MagicWand
            )
    }

    /// Desenha os traços vetoriais no buffer RGBA (borda para o flood fill).
    fn rasterizar_vetores(&self, rgba: &mut [u8], w: u32, h: u32) {
        for obj in &self.document.vectors {
            let r = (obj.stroke_width * 0.5).max(0.6);
            let dense = densify(&obj.flatten(24), r.max(1.0));
            for &(fx, fy) in &dense {
                stamp_disc(rgba, w, h, fx, fy, r, obj.stroke);
            }
        }
    }

    /// Balde de preenchimento (flood fill) na camada ativa, usando como
    /// referência a composição raster + vetores (respeita pincel e formas).
    fn balde_preencher(&mut self, x0: i32, y0: i32) {
        let w = self.document.width as i32;
        let h = self.document.height as i32;
        if x0 < 0 || y0 < 0 || x0 >= w || y0 >= h {
            return;
        }
        let li = self.active_layer;
        if self.document.layer(li).map_or(true, |l| l.locked) {
            self.status = "Camada bloqueada".into();
            return;
        }
        let PixelImage { width, height, mut rgba } = render_document(&self.document);
        self.rasterizar_vetores(&mut rgba, width as u32, height as u32);
        let idx = |x: i32, y: i32| ((y * w + x) * 4) as usize;
        let ti = idx(x0, y0);
        let target = [rgba[ti], rgba[ti + 1], rgba[ti + 2], rgba[ti + 3]];
        let tol = self.fill_tolerance;
        let fill = self.brush_core_color();
        self.push_undo();
        let mut visited = vec![false; (w * h) as usize];
        let mut stack: Vec<(i32, i32)> = vec![(x0, y0)];
        while let Some((x, y)) = stack.pop() {
            if x < 0 || y < 0 || x >= w || y >= h {
                continue;
            }
            let vi = (y * w + x) as usize;
            if visited[vi] {
                continue;
            }
            let ci = idx(x, y);
            let d = (rgba[ci] as i32 - target[0] as i32)
                .abs()
                .max((rgba[ci + 1] as i32 - target[1] as i32).abs())
                .max((rgba[ci + 2] as i32 - target[2] as i32).abs())
                .max((rgba[ci + 3] as i32 - target[3] as i32).abs());
            if d > tol {
                continue;
            }
            visited[vi] = true;
            if let Some(layer) = self.document.layer_mut(li) {
                layer.set_pixel(x as u32, y as u32, fill);
            }
            stack.push((x + 1, y));
            stack.push((x - 1, y));
            stack.push((x, y + 1));
            stack.push((x, y - 1));
        }
        self.dirty = true;
        self.status = "Preenchido".into();
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

    /// Verdadeiro se `hp` está sobre o ponto de rotação do objeto `si`.
    fn rotate_handle_at(&self, si: usize, hp: egui::Pos2, rect: egui::Rect, zoom: f32) -> bool {
        if let Some(obj) = self.document.vectors.get(si) {
            if let Some((minx, miny, maxx, maxy)) = obj.bounds() {
                let r = egui::Rect::from_min_max(
                    egui::pos2(rect.min.x + minx * zoom, rect.min.y + miny * zoom),
                    egui::pos2(rect.min.x + maxx * zoom, rect.min.y + maxy * zoom),
                )
                .expand(3.0);
                return rotate_handle_screen(r).distance(hp) <= 14.0;
            }
        }
        false
    }

    /// Desenha os objetos vetoriais (curvas), a caixa/alças de seleção e o
    /// traço em progresso da Caneta, como overlay sobre o canvas.
    fn desenhar_vetores(&self, ui: &egui::Ui, rect: egui::Rect, zoom: f32) {
        let painter = ui.painter_at(rect);
        let sp = |x: f32, y: f32| egui::pos2(rect.min.x + x * zoom, rect.min.y + y * zoom);
        let azul = egui::Color32::from_rgb(0x2F, 0x84, 0xFE);
        for (idx, obj) in self.document.vectors.iter().enumerate() {
            let op = obj.opacity.clamp(0.0, 1.0);
            let col = to_color32(obj.stroke).linear_multiply(op);
            let w = (obj.stroke_width * zoom).max(1.0);
            let pts: Vec<egui::Pos2> = obj.flatten(24).iter().map(|(x, y)| sp(*x, *y)).collect();
            let stroke = egui::Stroke::new(w, col);
            if let Some(fill) = obj.fill {
                let fc = to_color32(fill).linear_multiply(op);
                if pts.len() >= 3 {
                    painter.add(egui::Shape::convex_polygon(pts, fc, stroke));
                } else if pts.len() >= 2 {
                    painter.add(egui::Shape::line(pts, stroke));
                }
            } else if obj.closed && pts.len() >= 3 {
                painter.add(egui::Shape::closed_line(pts, stroke));
            } else if pts.len() >= 2 {
                painter.add(egui::Shape::line(pts, stroke));
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
                    // ponto de rotação dedicado (acima da caixa)
                    let rh = rotate_handle_screen(r);
                    painter.line_segment(
                        [r.center_top(), rh],
                        egui::Stroke::new(1.0_f32, azul),
                    );
                    painter.circle_filled(rh, 8.0, egui::Color32::WHITE);
                    painter.circle_stroke(rh, 8.0, egui::Stroke::new(1.0_f32, azul));
                    painter.text(
                        rh,
                        egui::Align2::CENTER_CENTER,
                        egui_phosphor::regular::ARROW_CLOCKWISE,
                        egui::FontId::proportional(12.0),
                        azul,
                    );
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
                        (Tool::Lasso, icon::LASSO, "Seleção livre (laço)"),
                        (Tool::DirectSelect, icon::SELECTION, "Seleção direta — editar por pontos"),
                        (Tool::MagicWand, icon::MAGIC_WAND, "Varinha mágica — selecionar por cor"),
                    ] {
                        let bloq = self.bloqueada_pixel(t);
                        let ativa =
                            self.tool == t && self.eyedropper == Eyedropper::Off && !bloq;
                        let resp = icon_button(ui, ativa, ic);
                        if bloq {
                            ui.painter().rect_filled(
                                resp.rect,
                                5.0,
                                egui::Color32::from_black_alpha(130),
                            );
                            resp.on_hover_text("Indisponível no modo pixel art");
                        } else if resp.on_hover_text(hint).clicked() {
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
                        (Tool::Shapes, icon::SHAPES, "Formas geométricas"),
                        (Tool::Fill, icon::PAINT_BUCKET, "Balde de preenchimento"),
                    ] {
                        let bloq = self.bloqueada_pixel(t);
                        let ativa =
                            self.tool == t && self.eyedropper == Eyedropper::Off && !bloq;
                        let resp = icon_button(ui, ativa, ic);
                        if bloq {
                            ui.painter().rect_filled(
                                resp.rect,
                                5.0,
                                egui::Color32::from_black_alpha(130),
                            );
                            resp.on_hover_text("Indisponível no modo pixel art");
                        } else if resp.on_hover_text(hint).clicked() {
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

    /// Janela de reprodução da animação (Play): roda os frames no FPS definido,
    /// em loop ou sequência finita.
    fn janela_reproducao(&mut self, ctx: &egui::Context) {
        if !self.playing {
            return;
        }
        let n = self.document.frame_count();
        let fps = self.document.fps.max(1) as f32;
        let step = 1.0 / fps;
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        if !self.play_done {
            self.play_accum += dt;
            while self.play_accum >= step {
                self.play_accum -= step;
                if self.play_frame + 1 >= n {
                    if self.play_loop {
                        self.play_frame = 0;
                    } else {
                        self.play_done = true;
                        break;
                    }
                } else {
                    self.play_frame += 1;
                }
            }
        }
        ctx.request_repaint();
        let pf = self.play_frame.min(n.saturating_sub(1));
        let img = render_frame(
            self.document.width,
            self.document.height,
            self.document.background,
            &self.document.frames[pf].layers,
            &self.document.frames[pf].vectors,
        );
        let ci = egui::ColorImage::from_rgba_unmultiplied(
            [img.width as usize, img.height as usize],
            &img.rgba,
        );
        match &mut self.play_tex {
            Some(t) => t.set(ci, egui::TextureOptions::NEAREST),
            None => {
                self.play_tex = Some(ctx.load_texture("play", ci, egui::TextureOptions::NEAREST))
            }
        }
        let mut open = true;
        egui::Window::new("Reprodução")
            .open(&mut open)
            .default_size(egui::vec2(380.0, 340.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.play_loop, "Em loop")
                        .on_hover_text("Ligado: repete ao terminar. Desligado: roda uma vez e para.");
                    ui.separator();
                    if ui.button("Reiniciar").clicked() {
                        self.play_frame = 0;
                        self.play_accum = 0.0;
                        self.play_done = false;
                    }
                    ui.separator();
                    ui.label(format!("Frame {}/{}", pf + 1, n));
                    if self.play_done {
                        ui.label("• fim");
                    }
                });
                ui.separator();
                if let Some(t) = &self.play_tex {
                    let avail = ui.available_size();
                    let (dw, dh) = (self.document.width as f32, self.document.height as f32);
                    let scale = (avail.x / dw)
                        .min((avail.y.max(60.0)) / dh)
                        .clamp(0.02, 8.0);
                    let size = egui::vec2(dw * scale, dh * scale);
                    ui.add(
                        egui::Image::from_texture(egui::load::SizedTexture::new(t.id(), size))
                            .fit_to_exact_size(size),
                    );
                }
            });
        if !open {
            self.playing = false;
        }
    }

    /// Timeline de frames (rodapé): miniaturas selecionáveis, navegação, FPS
    /// e onion skin.
    fn barra_frames(&mut self, ctx: &egui::Context) {
        let (dw, dh, bg) = (self.document.width, self.document.height, self.document.background);
        let th_h = 56usize;
        let th_w = (((th_h as f32) * dw as f32 / dh as f32).round() as usize).clamp(24, 160);
        let n = self.document.frame_count();
        if self.frame_thumbs.len() != n {
            self.frame_thumbs.resize(n, None);
        }
        for i in 0..n {
            if self.frame_thumbs[i].is_none() {
                let full = if i == self.document.current {
                    render_frame(dw, dh, bg, &self.document.layers, &self.document.vectors)
                } else {
                    render_frame(
                        dw,
                        dh,
                        bg,
                        &self.document.frames[i].layers,
                        &self.document.frames[i].vectors,
                    )
                };
                let img = thumb_image(&full, th_w, th_h);
                self.frame_thumbs[i] =
                    Some(ctx.load_texture(format!("thumb{i}"), img, egui::TextureOptions::NEAREST));
            }
        }
        let mut act_add = false;
        let mut act_dup = false;
        let mut act_del = false;
        let mut act_prev = false;
        let mut act_next = false;
        let mut act_play = false;
        let mut goto: Option<usize> = None;
        egui::TopBottomPanel::bottom("timeline")
            .resizable(false)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let play_lbl = if self.playing { "⏸ Parar" } else { "▶ Play" };
                    if ui
                        .button(play_lbl)
                        .on_hover_text("Rodar a animação no FPS definido")
                        .clicked()
                    {
                        act_play = true;
                    }
                    ui.separator();
                    if ui.button("＋ Frame").on_hover_text("Novo frame após o atual").clicked() {
                        act_add = true;
                    }
                    if ui.button("Duplicar").clicked() {
                        act_dup = true;
                    }
                    if ui.add_enabled(n > 1, egui::Button::new("Excluir")).clicked() {
                        act_del = true;
                    }
                    ui.separator();
                    if ui.button("◀").clicked() {
                        act_prev = true;
                    }
                    ui.label(format!("Frame {}/{}", self.document.current + 1, n));
                    if ui.button("▶").clicked() {
                        act_next = true;
                    }
                    ui.separator();
                    ui.label("FPS:");
                    let mut fps = self.document.fps as i32;
                    if ui.add(egui::Slider::new(&mut fps, 1..=60)).changed() {
                        self.document.fps = fps.clamp(1, 60) as u32;
                    }
                    ui.separator();
                    ui.checkbox(&mut self.onion, "Onion skin")
                        .on_hover_text("Mostra o frame anterior a 30%");
                });
                ui.add_space(4.0);
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for i in 0..n {
                            let sel = i == self.document.current;
                            let (rect, resp) = ui.allocate_exact_size(
                                egui::vec2(th_w as f32, th_h as f32 + 16.0),
                                egui::Sense::click(),
                            );
                            let img_rect = egui::Rect::from_min_size(
                                rect.min,
                                egui::vec2(th_w as f32, th_h as f32),
                            );
                            let painter = ui.painter_at(rect);
                            painter.rect_filled(img_rect, 0.0, egui::Color32::from_gray(30));
                            if let Some(tex) = &self.frame_thumbs[i] {
                                painter.image(
                                    tex.id(),
                                    img_rect,
                                    egui::Rect::from_min_max(
                                        egui::pos2(0.0, 0.0),
                                        egui::pos2(1.0, 1.0),
                                    ),
                                    egui::Color32::WHITE,
                                );
                            }
                            let cor = if sel {
                                egui::Color32::from_rgb(0x2F, 0x84, 0xFE)
                            } else {
                                egui::Color32::from_gray(90)
                            };
                            painter.rect_stroke(
                                img_rect,
                                0.0,
                                egui::Stroke::new(if sel { 2.0 } else { 1.0 }, cor),
                            );
                            painter.text(
                                egui::pos2(rect.center().x, img_rect.bottom() + 8.0),
                                egui::Align2::CENTER_CENTER,
                                format!("{}", i + 1),
                                egui::FontId::proportional(12.0),
                                cor,
                            );
                            if resp.clicked() {
                                goto = Some(i);
                            }
                            ui.add_space(6.0);
                        }
                    });
                });
                ui.add_space(4.0);
            });
        if goto.is_some() || act_add || act_dup || act_del || act_prev || act_next {
            self.playing = false;
        }
        if act_play {
            if self.playing {
                self.playing = false;
            } else {
                self.document.sync_to_frames();
                self.play_frame = 0;
                self.play_accum = 0.0;
                self.play_done = false;
                self.playing = true;
            }
        }
        if let Some(i) = goto {
            self.document.go_to_frame(i);
            self.dirty = true;
            self.onion_for = None;
        }
        if act_prev && self.document.current > 0 {
            self.document.go_to_frame(self.document.current - 1);
            self.dirty = true;
            self.onion_for = None;
        }
        if act_next && self.document.current + 1 < n {
            self.document.go_to_frame(self.document.current + 1);
            self.dirty = true;
            self.onion_for = None;
        }
        if act_add {
            self.document.add_frame();
            self.frame_thumbs.clear();
            self.dirty = true;
            self.onion_for = None;
        }
        if act_dup {
            self.document.duplicate_frame();
            self.frame_thumbs.clear();
            self.dirty = true;
            self.onion_for = None;
        }
        if act_del {
            let c = self.document.current;
            self.document.remove_frame(c);
            self.frame_thumbs.clear();
            self.dirty = true;
            self.onion_for = None;
        }
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
                        Tool::Lasso => self.opcoes_laco(ui),
                        Tool::Text => self.opcoes_texto(ui),
                        Tool::Pen => self.opcoes_caneta(ui),
                        Tool::MagicWand => self.opcoes_varinha(ui),
                        Tool::DirectSelect => self.opcoes_selecao_direta(ui),
                        Tool::Shapes => self.opcoes_formas(ui),
                        Tool::Fill => self.opcoes_balde(ui),
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
        ui.add(egui::Slider::new(&mut self.eraser_radius, 1..=60));
        ui.separator();
        self.botao_limpar(ui);
    }

    /// Ferramenta Seleção: opera sobre o objeto vetorial selecionado.
    fn opcoes_selecao(&mut self, ui: &mut egui::Ui) {
        if self.float_sel.is_some() {
            ui.label("Seleção de pixels.");
            ui.separator();
            ui.label("Opacidade:");
            let mut pct = self.float_sel.as_ref().map(|f| f.opacity).unwrap_or(1.0) * 100.0;
            if ui
                .add(egui::Slider::new(&mut pct, 0.0..=100.0).suffix("%"))
                .changed()
            {
                if let Some(f) = &mut self.float_sel {
                    f.opacity = (pct / 100.0).clamp(0.0, 1.0);
                }
                self.dirty = true;
            }
            ui.separator();
            if ui.button("Confirmar").clicked() {
                self.commit_float();
            }
            if ui.button("Excluir").clicked() {
                self.float_sel = None;
                self.float_tex = None;
                self.dirty = true;
            }
            return;
        }
        match self.selected_obj {
            Some(idx) if idx < self.document.vectors.len() => {
                ui.label("Traço selecionado.");
                ui.separator();
                if ui.button("↺ 90°").on_hover_text("Girar 90° à esquerda").clicked() {
                    self.push_undo();
                    self.document.vectors[idx].rotate_ccw_self();
                }
                if ui.button("↻ 90°").on_hover_text("Girar 90° à direita").clicked() {
                    self.push_undo();
                    self.document.vectors[idx].rotate_cw_self();
                }
                if ui.button("180°").on_hover_text("Girar 180°").clicked() {
                    self.push_undo();
                    self.document.vectors[idx].rotate_180_self();
                }
                ui.separator();
                if ui.button("Espelhar H").on_hover_text("Espelhar horizontalmente").clicked() {
                    self.push_undo();
                    self.document.vectors[idx].flip_h_self();
                }
                if ui.button("Espelhar V").on_hover_text("Espelhar verticalmente").clicked() {
                    self.push_undo();
                    self.document.vectors[idx].flip_v_self();
                }
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
                ui.label("Traço:");
                let mut sc = to_color32(self.document.vectors[idx].stroke);
                if ui.color_edit_button_srgba(&mut sc).changed() {
                    self.document.vectors[idx].stroke = Color::rgba(sc.r(), sc.g(), sc.b(), sc.a());
                }
                let mut tem_fill = self.document.vectors[idx].fill.is_some();
                if ui.checkbox(&mut tem_fill, "Preencher").changed() {
                    self.document.vectors[idx].fill = if tem_fill {
                        Some(self.document.vectors[idx].stroke)
                    } else {
                        None
                    };
                }
                if let Some(fc0) = self.document.vectors[idx].fill {
                    let mut fc = to_color32(fc0);
                    if ui.color_edit_button_srgba(&mut fc).changed() {
                        self.document.vectors[idx].fill =
                            Some(Color::rgba(fc.r(), fc.g(), fc.b(), fc.a()));
                    }
                }
                ui.separator();
                if ui.button("Aplicar cor atual (traço)").clicked() {
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

    fn opcoes_formas(&mut self, ui: &mut egui::Ui) {
        ui.label("Forma:");
        egui::ComboBox::from_id_salt("forma_tipo")
            .selected_text(FORMAS[self.shape_kind])
            .show_ui(ui, |ui| {
                for (i, f) in FORMAS.iter().enumerate() {
                    ui.selectable_value(&mut self.shape_kind, i, *f);
                }
            });
        if self.shape_kind == 3 {
            ui.label("Lados:");
            ui.add(egui::Slider::new(&mut self.shape_sides, 3..=12));
        }
        ui.separator();
        ui.label("Espessura:");
        ui.add(egui::Slider::new(&mut self.shape_stroke, 1..=40));
        ui.separator();
        ui.label("Traço:");
        self.swatch_cor(ui);
        ui.separator();
        ui.checkbox(&mut self.shape_fill, "Preencher");
        ui.label("Cor:");
        ui.color_edit_button_srgba(&mut self.fill_color);
        ui.separator();
        ui.weak("Arraste no canvas para desenhar a forma.");
    }

    fn opcoes_balde(&mut self, ui: &mut egui::Ui) {
        ui.label("Tolerância:");
        ui.add(egui::Slider::new(&mut self.fill_tolerance, 0..=150));
        ui.separator();
        ui.label("Cor:");
        self.swatch_cor(ui);
        ui.separator();
        ui.weak("Clique numa área fechada para preencher (respeita traços e formas).");
    }

    /// Pontos (coords do documento) da forma atual no retângulo start..end.
    fn forma_pontos(&self, start: (f32, f32), end: (f32, f32)) -> Vec<(f32, f32)> {
        let minx = start.0.min(end.0);
        let maxx = start.0.max(end.0);
        let miny = start.1.min(end.1);
        let maxy = start.1.max(end.1);
        let (cx, cy) = ((minx + maxx) / 2.0, (miny + maxy) / 2.0);
        let (rx, ry) = ((maxx - minx) / 2.0, (maxy - miny) / 2.0);
        let mut v = Vec::new();
        match self.shape_kind {
            0 => v.extend_from_slice(&[(minx, miny), (maxx, miny), (maxx, maxy), (minx, maxy)]),
            1 => {
                let n = 48;
                for k in 0..n {
                    let t = k as f32 / n as f32 * std::f32::consts::TAU;
                    v.push((cx + rx * t.cos(), cy + ry * t.sin()));
                }
            }
            2 => v.extend_from_slice(&[(cx, miny), (maxx, maxy), (minx, maxy)]),
            _ => {
                let n = self.shape_sides.max(3);
                for k in 0..n {
                    let t = -std::f32::consts::FRAC_PI_2 + k as f32 / n as f32 * std::f32::consts::TAU;
                    v.push((cx + rx * t.cos(), cy + ry * t.sin()));
                }
            }
        }
        v
    }

    /// Cria uma forma vetorial a partir do retângulo arrastado.
    fn criar_forma(&mut self, start: (f32, f32), end: (f32, f32)) {
        if (start.0 - end.0).abs() < 1.0 || (start.1 - end.1).abs() < 1.0 {
            self.shape_start = None;
            return;
        }
        let pts = self.forma_pontos(start, end);
        self.push_undo();
        let mut obj = VectorObject::new(self.brush_core_color(), self.shape_stroke as f32);
        obj.points = pts.iter().map(|(x, y)| Anchor::new(*x, *y)).collect();
        obj.closed = true;
        if self.shape_fill {
            let c = self.fill_color;
            obj.fill = Some(Color::rgba(c.r(), c.g(), c.b(), c.a()));
        }
        self.document.vectors.push(obj);
        self.selected_obj = Some(self.document.vectors.len() - 1);
        self.shape_start = None;
        self.dirty = true;
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
        if !matches!(self.tool, Tool::Select | Tool::Lasso) && self.float_sel.is_some() {
            self.commit_float();
        }
        if self.bloqueada_pixel(self.tool) {
            self.tool = Tool::Pencil;
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
            let cf = self.document.current;
            if cf < self.frame_thumbs.len() {
                self.frame_thumbs[cf] = None;
            }
            self.onion_for = None;
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
            if k_del && matches!(self.tool, Tool::Select | Tool::Lasso) {
                if self.float_sel.is_some() {
                    self.float_sel = None;
                    self.float_tex = None;
                    self.dirty = true;
                } else if let Some(i) = self.selected_obj {
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
        self.barra_frames(ctx);
        self.janela_reproducao(ctx);

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

                    if self.onion && self.document.current > 0 {
                        let prev = self.document.current - 1;
                        if self.onion_for != Some(prev) || self.onion_tex.is_none() {
                            let oi = render_frame_alpha(
                                self.document.width,
                                self.document.height,
                                &self.document.frames[prev].layers,
                                &self.document.frames[prev].vectors,
                            );
                            let ci = egui::ColorImage::from_rgba_unmultiplied(
                                [oi.width as usize, oi.height as usize],
                                &oi.rgba,
                            );
                            self.onion_tex = Some(ui.ctx().load_texture(
                                "onion",
                                ci,
                                egui::TextureOptions::NEAREST,
                            ));
                            self.onion_for = Some(prev);
                        }
                        if let Some(tex) = &self.onion_tex {
                            ui.painter_at(rect).image(
                                tex.id(),
                                rect,
                                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                                egui::Color32::from_white_alpha(77),
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
                                let dp = to_doc(p);
                                // seleção flutuante (raster): dentro move; fora confirma
                                let consumed = self.float_press(p, rect, zoom);
                                if !consumed && self.float_sel.is_some() {
                                    self.commit_float();
                                }
                                if !consumed {
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
                                        let mut did_rot = false;
                                        if let Some(si) = self.selected_obj {
                                            if si < self.document.vectors.len()
                                                && self.rotate_handle_at(si, p, rect, zoom)
                                            {
                                                self.push_undo();
                                                let c = self.document.vectors[si]
                                                    .center()
                                                    .unwrap_or((0.0, 0.0));
                                                self.rotate_center = c;
                                                self.rotate_orig =
                                                    self.document.vectors[si].points.clone();
                                                self.rotate_start = (dp.1 - c.1).atan2(dp.0 - c.0);
                                                self.rotating = true;
                                                self.dragging_obj = false;
                                                did_rot = true;
                                            }
                                        }
                                        if !did_rot {
                                            match self.hit_test(dp, thr) {
                                                Some(i) => {
                                                    self.selected_obj = Some(i);
                                                    self.push_undo();
                                                    self.dragging_obj = true;
                                                }
                                                None => {
                                                    self.selected_obj = None;
                                                    self.dragging_obj = false;
                                                    self.marquee_start = Some((
                                                        dp.0.floor() as i32,
                                                        dp.1.floor() as i32,
                                                    ));
                                                    self.marquee_cur =
                                                        (dp.0.floor() as i32, dp.1.floor() as i32);
                                                }
                                            }
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
                        } else if down && self.rotating {
                            if let Some(si) = self.selected_obj {
                                if si < self.document.vectors.len() {
                                    if let Some(pp) = ppos {
                                        let cur = to_doc(pp);
                                        let (cx, cy) = self.rotate_center;
                                        let ang = (cur.1 - cy).atan2(cur.0 - cx);
                                        let delta = ang - self.rotate_start;
                                        let (sn, cs) = delta.sin_cos();
                                        let orig = self.rotate_orig.clone();
                                        let obj = &mut self.document.vectors[si];
                                        if obj.points.len() == orig.len() {
                                            let rot = |x: f32, y: f32| {
                                                let (dx, dy) = (x - cx, y - cy);
                                                (cx + dx * cs - dy * sn, cy + dx * sn + dy * cs)
                                            };
                                            for (dst, src) in obj.points.iter_mut().zip(orig.iter()) {
                                                let (nx, ny) = rot(src.x, src.y);
                                                dst.x = nx;
                                                dst.y = ny;
                                                dst.hin = src.hin.map(|(hx, hy)| rot(hx, hy));
                                                dst.hout = src.hout.map(|(hx, hy)| rot(hx, hy));
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
                        if down {
                            self.float_down(ppos, rect, zoom);
                        }
                        if down && self.marquee_start.is_some() {
                            if let Some(pp) = ppos {
                                let d = to_doc(pp);
                                self.marquee_cur = (d.0.floor() as i32, d.1.floor() as i32);
                            }
                        }
                        if !down {
                            self.dragging_obj = false;
                            self.resize_handle = None;
                            self.rotating = false;
                            self.float_release();
                            if let Some(start) = self.marquee_start.take() {
                                self.lift_selection(start, self.marquee_cur);
                            }
                        }
                        self.last_pos = None;
                    } else if self.tool == Tool::Lasso {
                        if pressed {
                            if let Some(pp) = hover {
                                let consumed = self.float_press(pp, rect, zoom);
                                if !consumed && self.float_sel.is_some() {
                                    self.commit_float();
                                }
                                if !consumed {
                                    self.lasso_points = vec![to_doc(pp)];
                                }
                            }
                        }
                        if down {
                            if !self.float_down(ppos, rect, zoom) && !self.lasso_points.is_empty() {
                                if let Some(pp) = ppos {
                                    self.lasso_points.push(to_doc(pp));
                                }
                            }
                        }
                        if !down {
                            self.float_release();
                            if !self.lasso_points.is_empty() {
                                let pts = std::mem::take(&mut self.lasso_points);
                                self.lift_lasso(&pts);
                            }
                        }
                        self.last_pos = None;
                    } else if self.tool == Tool::Shapes {
                        if pressed {
                            if let Some(p) = hover {
                                self.shape_start = Some(to_doc(p));
                            }
                        }
                        if !down {
                            if let Some(start) = self.shape_start.take() {
                                if let Some(pp) = ppos {
                                    self.criar_forma(start, to_doc(pp));
                                }
                            }
                        }
                        self.last_pos = None;
                    } else if self.tool == Tool::Fill {
                        if pressed {
                            if let Some(p) = hover {
                                let (x, y) = to_pixel(p);
                                self.balde_preencher(x, y);
                            }
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
                        if self.tool == Tool::Eraser {
                            if let Some(pp) = ppos {
                                self.erase_vetores(to_doc(pp), self.active_radius() as f32);
                            }
                        }
                    } else {
                        self.last_pos = None;
                    }

                    // Prévia da forma sendo desenhada (durante o arraste).
                    if self.tool == Tool::Shapes {
                        if let (Some(start), Some(pp)) = (self.shape_start, ppos) {
                            let dpts = self.forma_pontos(start, to_doc(pp));
                            if dpts.len() >= 2 {
                                let painter = ui.painter_at(rect);
                                let spts: Vec<egui::Pos2> = dpts
                                    .iter()
                                    .map(|(x, y)| {
                                        egui::pos2(rect.min.x + x * zoom, rect.min.y + y * zoom)
                                    })
                                    .collect();
                                painter.add(egui::Shape::closed_line(
                                    spts,
                                    egui::Stroke::new(
                                        (self.shape_stroke as f32 * zoom).max(1.0),
                                        to_color32(self.brush_core_color()),
                                    ),
                                ));
                            }
                        }
                    }

                    if self.tool == Tool::Lasso && self.lasso_points.len() >= 2 {
                        let painter = ui.painter_at(rect);
                        let pts: Vec<egui::Pos2> = self
                            .lasso_points
                            .iter()
                            .map(|(x, y)| egui::pos2(rect.min.x + x * zoom, rect.min.y + y * zoom))
                            .collect();
                        painter.add(egui::Shape::line(
                            pts,
                            egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(0x2F, 0x84, 0xFE)),
                        ));
                    }

                    // Cursor personalizado por ferramenta + decorações no canvas.
                    if response.hovered() {
                        use egui::CursorIcon as CI;
                        let painter = ui.painter_at(rect);
                        if self.eyedropper != Eyedropper::Off {
                            ui.ctx().set_cursor_icon(CI::Crosshair);
                        } else {
                            match self.tool {
                                Tool::Pencil | Tool::Eraser => {
                                    if let Some(hp) = hover {
                                        let rr = (self.active_radius() as f32 * zoom).max(1.5);
                                        painter.circle_stroke(
                                            hp,
                                            rr,
                                            egui::Stroke::new(1.5_f32, egui::Color32::from_black_alpha(160)),
                                        );
                                        painter.circle_stroke(
                                            hp,
                                            rr + 1.0,
                                            egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(180)),
                                        );
                                        painter.circle_filled(hp, 1.0, egui::Color32::from_black_alpha(160));
                                    }
                                    ui.ctx().set_cursor_icon(CI::None);
                                }
                                Tool::Pen | Tool::Shapes | Tool::Fill | Tool::Lasso => {
                                    ui.ctx().set_cursor_icon(CI::Crosshair);
                                }
                                Tool::Select => {
                                    let rot_zone = self.rotating
                                        || match (self.selected_obj, hover) {
                                            (Some(si), Some(hp)) => {
                                                self.rotate_handle_at(si, hp, rect, zoom)
                                            }
                                            _ => false,
                                        };
                                    if rot_zone {
                                        ui.ctx().set_cursor_icon(if self.rotating {
                                            CI::Grabbing
                                        } else {
                                            CI::Grab
                                        });
                                    } else if self.dragging_obj || self.resize_handle.is_some() {
                                        ui.ctx().set_cursor_icon(CI::Grabbing);
                                    } else if let (Some(si), Some(hp)) = (self.selected_obj, hover) {
                                        if let Some(hi) = self.handle_at(si, hp, rect, zoom) {
                                            let c = match hi {
                                                0 | 4 => CI::ResizeNwSe,
                                                2 | 6 => CI::ResizeNeSw,
                                                1 | 5 => CI::ResizeVertical,
                                                _ => CI::ResizeHorizontal,
                                            };
                                            ui.ctx().set_cursor_icon(c);
                                        } else if self.hit_test(to_doc(hp), 6.0 / zoom).is_some() {
                                            ui.ctx().set_cursor_icon(CI::Grab);
                                        } else {
                                            ui.ctx().set_cursor_icon(CI::Default);
                                        }
                                    } else if let Some(hp) = hover {
                                        if self.hit_test(to_doc(hp), 6.0 / zoom).is_some() {
                                            ui.ctx().set_cursor_icon(CI::Grab);
                                        } else {
                                            ui.ctx().set_cursor_icon(CI::Default);
                                        }
                                    } else {
                                        ui.ctx().set_cursor_icon(CI::Default);
                                    }
                                }
                                _ => {
                                    ui.ctx().set_cursor_icon(CI::Default);
                                }
                            }
                        }
                    }

                    // Overlay vetorial: objetos, seleção e traço em progresso.
                    self.desenhar_vetores(ui, rect, zoom);

                    // Seleção retangular raster: pixels flutuantes + marca.
                    if self.float_sel.is_some() && self.float_tex.is_none() {
                        if let Some(fs) = &self.float_sel {
                            let img = egui::ColorImage::from_rgba_unmultiplied(
                                [fs.ow as usize, fs.oh as usize],
                                &fs.pixels,
                            );
                            self.float_tex = Some(ui.ctx().load_texture(
                                "float_sel",
                                img,
                                egui::TextureOptions::NEAREST,
                            ));
                        }
                    }
                    if let Some(fs) = &self.float_sel {
                        let scr = |wx: f32, wy: f32| {
                            egui::pos2(rect.min.x + wx * zoom, rect.min.y + wy * zoom)
                        };
                        let painter = ui.painter_at(rect);
                        let cw = [
                            float_corner(fs.cx, fs.cy, fs.hw, fs.hh, fs.angle, -1.0, -1.0),
                            float_corner(fs.cx, fs.cy, fs.hw, fs.hh, fs.angle, 1.0, -1.0),
                            float_corner(fs.cx, fs.cy, fs.hw, fs.hh, fs.angle, 1.0, 1.0),
                            float_corner(fs.cx, fs.cy, fs.hw, fs.hh, fs.angle, -1.0, 1.0),
                        ];
                        let cs: Vec<egui::Pos2> = cw.iter().map(|(x, y)| scr(*x, *y)).collect();
                        if let Some(tex) = &self.float_tex {
                            let col = egui::Color32::from_white_alpha((fs.opacity * 255.0) as u8);
                            let uvs = [
                                egui::pos2(0.0, 0.0),
                                egui::pos2(1.0, 0.0),
                                egui::pos2(1.0, 1.0),
                                egui::pos2(0.0, 1.0),
                            ];
                            let mut mesh = egui::Mesh::with_texture(tex.id());
                            for i in 0..4 {
                                mesh.vertices.push(egui::epaint::Vertex {
                                    pos: cs[i],
                                    uv: uvs[i],
                                    color: col,
                                });
                            }
                            mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
                            painter.add(egui::Shape::mesh(mesh));
                        }
                        let azul = egui::Color32::from_rgb(0x2F, 0x84, 0xFE);
                        for i in 0..4 {
                            painter.line_segment(
                                [cs[i], cs[(i + 1) % 4]],
                                egui::Stroke::new(1.0_f32, azul),
                            );
                        }
                        for &(sx, sy) in HSIGNS.iter() {
                            let (hx, hy) = float_corner(fs.cx, fs.cy, fs.hw, fs.hh, fs.angle, sx, sy);
                            let hr = egui::Rect::from_center_size(scr(hx, hy), egui::vec2(8.0, 8.0));
                            painter.rect_filled(hr, 0.0, egui::Color32::WHITE);
                            painter.rect_stroke(hr, 0.0, egui::Stroke::new(1.0_f32, azul));
                        }
                        let (tmx, tmy) = float_corner(fs.cx, fs.cy, fs.hw, fs.hh, fs.angle, 0.0, -1.0);
                        let tm = scr(tmx, tmy);
                        let cc = scr(fs.cx, fs.cy);
                        let dir = (tm - cc).normalized();
                        let roth = tm + dir * 22.0;
                        painter.line_segment([tm, roth], egui::Stroke::new(1.0_f32, azul));
                        painter.circle_filled(roth, 8.0, egui::Color32::WHITE);
                        painter.circle_stroke(roth, 8.0, egui::Stroke::new(1.0_f32, azul));
                        painter.text(
                            roth,
                            egui::Align2::CENTER_CENTER,
                            egui_phosphor::regular::ARROW_CLOCKWISE,
                            egui::FontId::proportional(12.0),
                            azul,
                        );
                    }
                    if let Some((sx, sy)) = self.marquee_start {
                        let (cx, cy) = self.marquee_cur;
                        let a = egui::pos2(
                            rect.min.x + sx.min(cx) as f32 * zoom,
                            rect.min.y + sy.min(cy) as f32 * zoom,
                        );
                        let b = egui::pos2(
                            rect.min.x + sx.max(cx) as f32 * zoom,
                            rect.min.y + sy.max(cy) as f32 * zoom,
                        );
                        let mr = egui::Rect::from_min_max(a, b);
                        let painter = ui.painter_at(rect);
                        painter.rect_stroke(mr, 0.0, egui::Stroke::new(1.0_f32, egui::Color32::WHITE));
                        painter.rect_stroke(
                            mr.expand(1.0),
                            0.0,
                            egui::Stroke::new(1.0_f32, egui::Color32::from_black_alpha(160)),
                        );
                    }
                });
        });
        let mut do_zoom_in = false;
        let mut do_zoom_out = false;
        egui::Area::new(egui::Id::new("acoes_canvas"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-66.0, -104.0))
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
