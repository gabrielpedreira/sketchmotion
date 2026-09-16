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
use sketchmotion_core::{
    Anchor, Camera, CameraKeyframe, Color, Document, Frame, ImageObject, Interp, Layer,
    PieceLibrary, VectorObject,
};
use sketchmotion_render::{rasterize_images, render_frame_alpha, render_layers_alpha, PixelImage};
use sketchmotion_trace::{trace_bilevel, trace_quantized, BilevelParams, QuantParams, TracedRegion};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use sketchmotion_tools::Tool;

const CANVAS_W: u32 = 800;
const CANVAS_H: u32 = 520;

/// Nº de colunas da grade de cores básicas.
const BASICAS_COLS: usize = 16;

/// Fontes exibidas no painel de texto (aplicação real virá com o módulo de texto).
const FONTES: [&str; 4] = ["Sans", "Serif", "Monospace", "Manuscrito"];
const FORMAS: [&str; 4] = ["Retângulo", "Elipse", "Triângulo", "Polígono"];
const BRUSHES: [&str; 8] = [
    "Duro",
    "Macio",
    "Aquarela",
    "Aerógrafo",
    "Giz de cera",
    "Caneta (fino-grosso)",
    "Pontilhado",
    "Esfumador",
];

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
/// Ajusta uma camada de outro documento ao tamanho (w, h) atual: mesma → clona;
/// diferente → cria uma nova e copia a região que couber (topo-esquerda).
fn fit_layer(src: &Layer, w: u32, h: u32) -> Layer {
    if src.width() == w && src.height() == h {
        return src.clone();
    }
    let mut nl = Layer::new(src.name.clone(), w, h);
    let cw = w.min(src.width());
    let ch = h.min(src.height());
    for y in 0..ch {
        for x in 0..cw {
            if let Some(c) = src.get_pixel(x, y) {
                nl.set_pixel(x, y, c);
            }
        }
    }
    nl.visible = src.visible;
    nl.locked = src.locked;
    nl.set_opacity(src.opacity());
    nl
}

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

// ---------- Prévia visual dos pincéis ----------

fn pv_blend(buf: &mut [u8], w: usize, h: usize, x: i32, y: i32, c: Color, cover: f32) {
    if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
        return;
    }
    let i = ((y as usize) * w + x as usize) * 4;
    let sa = (c.a as f32 / 255.0) * cover.clamp(0.0, 1.0);
    if sa <= 0.0 {
        return;
    }
    let da = buf[i + 3] as f32 / 255.0;
    let oa = sa + da * (1.0 - sa);
    if oa <= 0.0 {
        return;
    }
    let ch = [c.r, c.g, c.b];
    for k in 0..3 {
        let sc = ch[k] as f32 / 255.0;
        let dc = buf[i + k] as f32 / 255.0;
        buf[i + k] = (((sc * sa + dc * da * (1.0 - sa)) / oa) * 255.0).round() as u8;
    }
    buf[i + 3] = (oa * 255.0).round() as u8;
}

fn pv_rng(state: &mut u32) -> f32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    (x >> 8) as f32 / 16_777_216.0
}

fn pv_hard(buf: &mut [u8], w: usize, h: usize, x: i32, y: i32, r: i32, c: Color) {
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy <= r * r {
                pv_blend(buf, w, h, x + dx, y + dy, c, 1.0);
            }
        }
    }
}

fn pv_soft(buf: &mut [u8], w: usize, h: usize, x: i32, y: i32, r: i32, c: Color, master: f32) {
    let rf = (r as f32).max(0.5);
    for dy in -r..=r {
        for dx in -r..=r {
            let d2 = (dx * dx + dy * dy) as f32;
            if d2 > rf * rf {
                continue;
            }
            let t = (1.0 - d2.sqrt() / rf).clamp(0.0, 1.0);
            pv_blend(buf, w, h, x + dx, y + dy, c, t * t * master);
        }
    }
}

fn pv_spray(buf: &mut [u8], w: usize, h: usize, x: i32, y: i32, r: i32, c: Color, rng: &mut u32) {
    let rf = r as f32;
    let n = ((r as f32) * 2.2).max(6.0) as i32;
    for _ in 0..n {
        let a = pv_rng(rng) * std::f32::consts::TAU;
        let rad = rf * pv_rng(rng).sqrt();
        pv_blend(
            buf,
            w,
            h,
            x + (rad * a.cos()).round() as i32,
            y + (rad * a.sin()).round() as i32,
            c,
            0.3,
        );
    }
}

fn pv_crayon(buf: &mut [u8], w: usize, h: usize, x: i32, y: i32, r: i32, c: Color, rng: &mut u32) {
    let rf = (r as f32).max(0.5);
    for dy in -r..=r {
        for dx in -r..=r {
            let d2 = (dx * dx + dy * dy) as f32;
            if d2 > rf * rf {
                continue;
            }
            let g = pv_rng(rng);
            if g < 0.45 {
                continue;
            }
            let t = (1.0 - d2.sqrt() / rf).clamp(0.0, 1.0);
            pv_blend(buf, w, h, x + dx, y + dy, c, t * 0.9 * (0.55 + 0.45 * g));
        }
    }
}

/// Renderiza um traço de amostra representando a ponta `kind`.
fn preview_brush(kind: usize) -> egui::ColorImage {
    let (w, h) = (72usize, 26usize);
    let mut buf = vec![0u8; w * h * 4];
    for px in buf.chunks_exact_mut(4) {
        px[0] = 250;
        px[1] = 250;
        px[2] = 250;
        px[3] = 255;
    }
    if kind == 7 {
        // Esfumador: gradiente horizontal (mistura de duas cores).
        let a = Color::rgb(60, 110, 200);
        let b = Color::rgb(220, 130, 40);
        for y in 0..h {
            for x in 0..w {
                let t = x as f32 / (w as f32 - 1.0);
                let vy = 1.0 - ((y as f32 / h as f32 - 0.5).abs() * 2.0);
                let c = Color::rgb(
                    (a.r as f32 * (1.0 - t) + b.r as f32 * t) as u8,
                    (a.g as f32 * (1.0 - t) + b.g as f32 * t) as u8,
                    (a.b as f32 * (1.0 - t) + b.b as f32 * t) as u8,
                );
                pv_blend(&mut buf, w, h, x as i32, y as i32, c, (vy * 0.9).clamp(0.0, 1.0));
            }
        }
        return egui::ColorImage::from_rgba_unmultiplied([w, h], &buf);
    }
    let col = Color::rgb(40, 40, 40);
    let mut rng: u32 = 0x9E37_79B9;
    let (cx0, cx1) = (6.0f32, w as f32 - 6.0);
    let steps = (cx1 - cx0) as i32;
    let mut prev_dot = -100.0f32;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let x = cx0 + (cx1 - cx0) * t;
        let y = h as f32 * 0.5 + (t * std::f32::consts::TAU * 1.1).sin() * (h as f32 * 0.30);
        let r = if kind == 5 {
            (1.0 + 3.0 * (1.0 - (2.0 * (t - 0.5)).abs())).round() as i32
        } else {
            3
        };
        match kind {
            1 => pv_soft(&mut buf, w, h, x as i32, y as i32, r, col, 0.55),
            2 => pv_soft(&mut buf, w, h, x as i32, y as i32, r, col, 0.30),
            3 => pv_spray(&mut buf, w, h, x as i32, y as i32, r + 1, col, &mut rng),
            4 => pv_crayon(&mut buf, w, h, x as i32, y as i32, r, col, &mut rng),
            6 => {
                if x - prev_dot >= 6.0 {
                    prev_dot = x;
                    pv_hard(&mut buf, w, h, x as i32, y as i32, 2, col);
                }
            }
            _ => pv_hard(&mut buf, w, h, x as i32, y as i32, r, col),
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([w, h], &buf)
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
/// Polígono (em coordenadas de TELA) que representa a forma de um osso,
/// do ponto `o` (origem/articulação) ao ponto `t` (ponta).
fn rig_shape_points(shape: sketchmotion_core::BoneShape, o: egui::Pos2, t: egui::Pos2) -> Vec<egui::Pos2> {
    use sketchmotion_core::BoneShape as BS;
    let dir = t - o;
    let len = dir.length().max(1.0);
    let u = dir / len;
    let n = egui::vec2(-u.y, u.x);
    let base = |tt: f32| o + dir * tt;
    match shape {
        BS::Limb => {
            let bw = (len * 0.12).clamp(3.0, 12.0);
            let mid = o + dir * 0.18;
            vec![o, mid + n * bw, t, mid - n * bw]
        }
        BS::Torso => {
            let h = len * 0.34;
            vec![
                o,
                base(0.62) + n * h,
                t + n * (h * 0.55),
                t - n * (h * 0.55),
                base(0.62) - n * h,
            ]
        }
        BS::Hip => {
            let h = len * 0.46;
            vec![
                o,
                base(0.5) + n * h,
                t + n * (h * 0.6),
                t - n * (h * 0.6),
                base(0.5) - n * h,
            ]
        }
        BS::Head => {
            let ra = len * 0.5;
            let rb = len * 0.38;
            let c = o + dir * 0.5;
            (0..24)
                .map(|k| {
                    let a = k as f32 / 24.0 * std::f32::consts::TAU;
                    c + u * (ra * a.cos()) + n * (rb * a.sin())
                })
                .collect()
        }
        BS::Mao => {
            let h = len * 0.28;
            vec![o + n * (h * 0.5), o - n * (h * 0.5), t - n * h, t + n * h]
        }
        BS::Pe => {
            let h = len * 0.5;
            vec![o, t + n * h, t - n * (h * 0.2)]
        }
    }
}

// ---------------------------------------------------------------------------
// Esqueletos prontos (presets). Coordenadas normalizadas (x p/ direita, y p/
// baixo) em torno de (0,0); o builder aplica (cx,cy) + escala. Ângulos em
// radianos: cima = -PI/2, baixo = +PI/2, direita = 0.
// ---------------------------------------------------------------------------
use sketchmotion_core::{BoneShape as PBS, Skeleton as PSkel};

fn pw(cx: f32, cy: f32, s: f32, nx: f32, ny: f32) -> (f32, f32) {
    (cx + nx * s, cy + ny * s)
}

fn preset_humano(cx: f32, cy: f32, s: f32) -> PSkel {
    let mut sk = PSkel::new("Humano");
    let up = -std::f32::consts::FRAC_PI_2;
    let (hx, hy) = pw(cx, cy, s, 0.0, 0.55);
    let hip = sk.add_bone_world(None, hx, hy, up, 0.35 * s, PBS::Hip);
    let (tx, ty) = pw(cx, cy, s, 0.0, 0.2);
    let torso = sk.add_bone_world(Some(hip), tx, ty, up, 0.85 * s, PBS::Torso);
    let (hex, hey) = pw(cx, cy, s, 0.0, -0.65);
    sk.add_bone_world(Some(torso), hex, hey, up, 0.45 * s, PBS::Head);
    // Braços
    for side in [1.0_f32, -1.0] {
        let (sx, sy) = pw(cx, cy, s, 0.28 * side, -0.5);
        let a1 = (0.5_f32).atan2(0.18 * side);
        let ua = sk.add_bone_world(Some(torso), sx, sy, a1, 0.5 * s, PBS::Limb);
        let (ex, ey) = pw(cx, cy, s, 0.28 * side + 0.18 * side, 0.0);
        let a2 = (0.5_f32).atan2(0.05 * side);
        let fa = sk.add_bone_world(Some(ua), ex, ey, a2, 0.45 * s, PBS::Limb);
        let (wx, wy) = pw(cx, cy, s, 0.28 * side + 0.2 * side, 0.45);
        sk.add_bone_world(Some(fa), wx, wy, a2, 0.14 * s, PBS::Limb);
    }
    // Pernas
    for side in [1.0_f32, -1.0] {
        let (px, py) = pw(cx, cy, s, 0.16 * side, 0.55);
        let a1 = (0.5_f32).atan2(0.06 * side);
        let th = sk.add_bone_world(Some(hip), px, py, a1, 0.7 * s, PBS::Limb);
        let (kx, ky) = pw(cx, cy, s, 0.16 * side, 1.05);
        let sh = sk.add_bone_world(Some(th), kx, ky, std::f32::consts::FRAC_PI_2, 0.65 * s, PBS::Limb);
        let (ax, ay) = pw(cx, cy, s, 0.16 * side, 1.6);
        sk.add_bone_world(Some(sh), ax, ay, 0.24 * side, 0.22 * s, PBS::Limb);
    }
    sk
}

fn preset_cavalo(cx: f32, cy: f32, s: f32) -> PSkel {
    let mut sk = PSkel::new("Cavalo");
    // corpo horizontal, cabeça para a direita (+x)
    let (rx, ry) = pw(cx, cy, s, -0.9, 0.0);
    let torso = sk.add_bone_world(None, rx, ry, 0.0, 1.7 * s, PBS::Torso);
    // pescoço + cabeça (frente, +x, subindo)
    let (nx, ny) = pw(cx, cy, s, 0.62, -0.1);
    let neck = sk.add_bone_world(Some(torso), nx, ny, (-0.55_f32).atan2(0.6), 0.6 * s, PBS::Limb);
    let (hx, hy) = pw(cx, cy, s, 1.05, -0.55);
    sk.add_bone_world(Some(neck), hx, hy, (-0.15_f32).atan2(0.5), 0.42 * s, PBS::Head);
    // cauda (atrás, -x, subindo)
    let (tx, ty) = pw(cx, cy, s, -0.88, -0.08);
    sk.add_bone_world(Some(torso), tx, ty, (-0.35_f32).atan2(-0.5), 0.55 * s, PBS::Limb);
    // 4 patas (2 frente, 2 trás), cada uma coxa+canela+casco
    let front_x = 0.55;
    let hind_x = -0.6;
    for (bx, off) in [(front_x, 0.06_f32), (front_x - 0.08, -0.06), (hind_x, 0.06), (hind_x + 0.08, -0.06)] {
        let (sx, sy) = pw(cx, cy, s, bx + off, 0.15);
        let th = sk.add_bone_world(Some(torso), sx, sy, std::f32::consts::FRAC_PI_2 + off, 0.55 * s, PBS::Limb);
        let (kx, ky) = pw(cx, cy, s, bx + off, 0.7);
        let sh = sk.add_bone_world(Some(th), kx, ky, std::f32::consts::FRAC_PI_2, 0.5 * s, PBS::Limb);
        let (fx, fy) = pw(cx, cy, s, bx + off, 1.2);
        sk.add_bone_world(Some(sh), fx, fy, std::f32::consts::FRAC_PI_2, 0.18 * s, PBS::Limb);
    }
    sk
}

fn preset_raptor(cx: f32, cy: f32, s: f32) -> PSkel {
    let mut sk = PSkel::new("Raptor");
    let ss = s * 3.6;
    let sp = |nx: f32, ny: f32| (cx + (nx - 0.5) * ss, cy + (ny - 0.5) * ss * 0.6551);
    let place = |sk: &mut PSkel, parent: Option<u32>, o: (f32, f32), t: (f32, f32), idx: u16| -> u32 {
        let ow = sp(o.0, o.1);
        let tw = sp(t.0, t.1);
        let (dx, dy) = (tw.0 - ow.0, tw.1 - ow.1);
        let ang = dy.atan2(dx);
        let len = (dx * dx + dy * dy).sqrt().max(2.0);
        let id = sk.add_bone_world(parent, ow.0, ow.1, ang, len, PBS::Limb);
        if let Some(b) = sk.bone_mut(id) {
            b.img = Some(idx);
        }
        id
    };
    let tronco = place(&mut sk, None, (0.308, 0.365), (0.532, 0.393), 4);
    let toraxica = place(&mut sk, Some(tronco), (0.308, 0.365), (0.262, 0.3), 3);
    let cervical = place(&mut sk, Some(toraxica), (0.262, 0.3), (0.196, 0.15), 2);
    let maxilar = place(&mut sk, Some(cervical), (0.196, 0.15), (0.075, 0.115), 1);
    let mandibula = place(&mut sk, Some(maxilar), (0.196, 0.15), (0.075, 0.085), 0);
    let c1 = place(&mut sk, Some(tronco), (0.532, 0.393), (0.609, 0.352), 5);
    let c2 = place(&mut sk, Some(c1), (0.609, 0.352), (0.726, 0.279), 6);
    let c3 = place(&mut sk, Some(c2), (0.726, 0.279), (0.834, 0.245), 7);
    let c4 = place(&mut sk, Some(c3), (0.834, 0.245), (0.955, 0.215), 8);
    let umerod = place(&mut sk, Some(tronco), (0.32, 0.457), (0.353, 0.564), 9);
    let radiod = place(&mut sk, Some(umerod), (0.353, 0.564), (0.293, 0.662), 11);
    let maod = place(&mut sk, Some(radiod), (0.293, 0.662), (0.235, 0.72), 13);
    let umeroe = place(&mut sk, Some(tronco), (0.288, 0.433), (0.278, 0.539), 10);
    let radioe = place(&mut sk, Some(umeroe), (0.278, 0.539), (0.192, 0.58), 12);
    let maoe = place(&mut sk, Some(radioe), (0.192, 0.58), (0.15, 0.628), 14);
    let femurd = place(&mut sk, Some(tronco), (0.462, 0.404), (0.468, 0.577), 15);
    let tibiad = place(&mut sk, Some(femurd), (0.468, 0.577), (0.553, 0.724), 17);
    let ped = place(&mut sk, Some(tibiad), (0.553, 0.724), (0.575, 0.86), 19);
    let femure = place(&mut sk, Some(tronco), (0.488, 0.408), (0.414, 0.583), 16);
    let tibiae = place(&mut sk, Some(femure), (0.414, 0.583), (0.4875, 0.7315), 18);
    let pee = place(&mut sk, Some(tibiae), (0.4875, 0.7315), (0.47, 0.87), 20);
    let _ = (mandibula, c4, maod, maoe, ped, pee, cervical, radiod, radioe, tibiad, tibiae);
    sk
}

fn preset_skeleton(key: &str, cx: f32, cy: f32, s: f32) -> PSkel {
    match key {
        "cavalo" => preset_cavalo(cx, cy, s),
        "raptor" => preset_raptor(cx, cy, s),
        _ => preset_humano(cx, cy, s),
    }
}

/// Extensão (bbox) de um esqueleto em coordenadas de mundo.
fn skeleton_extent(sk: &PSkel) -> (f32, f32, f32, f32) {
    let (mut mnx, mut mny, mut mxx, mut mxy) =
        (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for b in &sk.bones {
        for (x, y) in [sk.origin(b.id), sk.tip(b.id)] {
            mnx = mnx.min(x);
            mny = mny.min(y);
            mxx = mxx.max(x);
            mxy = mxy.max(y);
        }
    }
    if !mnx.is_finite() {
        return (0.0, 0.0, 1.0, 1.0);
    }
    (mnx, mny, mxx, mxy)
}

/// Desenha um esqueleto ajustado dentro de `rect` (miniatura de prévia).
fn desenhar_preview_esqueleto(
    painter: &egui::Painter,
    rect: egui::Rect,
    sk: &PSkel,
    col: egui::Color32,
) {
    let (mnx, mny, mxx, mxy) = skeleton_extent(sk);
    let (w, h) = ((mxx - mnx).max(1e-3), (mxy - mny).max(1e-3));
    let scale = (rect.width() / w).min(rect.height() / h) * 0.9;
    let cxw = (mnx + mxx) * 0.5;
    let cyw = (mny + mxy) * 0.5;
    let map = |x: f32, y: f32| {
        egui::pos2(
            rect.center().x + (x - cxw) * scale,
            rect.center().y + (y - cyw) * scale,
        )
    };
    let fill = egui::Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 70);
    for b in &sk.bones {
        let (ox, oy) = sk.origin(b.id);
        let (tx, ty) = sk.tip(b.id);
        painter.add(egui::Shape::convex_polygon(
            rig_shape_points(b.shape, map(ox, oy), map(tx, ty)),
            fill,
            egui::Stroke::new(1.2_f32, col),
        ));
    }
}

struct PartDef {
    name: &'static str,
    aspect: f32,
    origin: (f32, f32),
    tip: (f32, f32),
    conns: &'static [(f32, f32)],
}

const RAPTOR_PARTS: [PartDef; 21] = [
    PartDef { name: "mandibula_superior", aspect: 0.4573, origin: (0.907, 0.3192), tip: (0.001, 0.3663), conns: &[(0.907, 0.3192)] },
    PartDef { name: "maxilar_inferior", aspect: 0.9244, origin: (0.88, 0.1192), tip: (0.002, 0.8911), conns: &[(0.88, 0.1192)] },
    PartDef { name: "pescoco_cervical", aspect: 1.3077, origin: (0.811, 1.1194), tip: (0.187, 0.1857), conns: &[(0.187, 0.1857), (0.811, 1.1194)] },
    PartDef { name: "pescoco_toraxica", aspect: 1.0769, origin: (0.831, 0.9078), tip: (0.168, 0.168), conns: &[(0.168, 0.168), (0.831, 0.9078)] },
    PartDef { name: "tronco", aspect: 0.5016, origin: (0.128, 0.1219), tip: (0.955, 0.1776), conns: &[(0.128, 0.1219), (0.726, 0.1771), (0.955, 0.1776), (0.624, 0.1821), (0.045, 0.2598), (0.143, 0.2809)] },
    PartDef { name: "cauda_1", aspect: 0.6735, origin: (0.139, 0.3839), tip: (0.822, 0.1522), conns: &[(0.822, 0.1522), (0.139, 0.3839)] },
    PartDef { name: "cauda_2", aspect: 0.5384, origin: (0.099, 0.4388), tip: (0.9, 0.098), conns: &[(0.9, 0.098), (0.099, 0.4388)] },
    PartDef { name: "cauda_3", aspect: 0.3919, origin: (0.109, 0.2806), tip: (0.888, 0.1086), conns: &[(0.888, 0.1086), (0.109, 0.2806)] },
    PartDef { name: "cauda_4", aspect: 0.3527, origin: (0.119, 0.1192), tip: (0.998, 0.3421), conns: &[(0.119, 0.1192)] },
    PartDef { name: "umero_direito", aspect: 1.0576, origin: (0.17, 0.1703), tip: (0.828, 0.8852), conns: &[(0.17, 0.1703), (0.828, 0.8852)] },
    PartDef { name: "umero_esquerdo", aspect: 2.2402, origin: (0.298, 0.2935), tip: (0.699, 1.9378), conns: &[(0.298, 0.2935), (0.699, 1.9378)] },
    PartDef { name: "radio_direito", aspect: 1.0278, origin: (0.826, 0.1706), tip: (0.172, 0.8551), conns: &[(0.826, 0.1706), (0.172, 0.8551)] },
    PartDef { name: "radio_esquerdo", aspect: 0.503, origin: (0.861, 0.1368), tip: (0.138, 0.3647), conns: &[(0.861, 0.1368), (0.138, 0.3647)] },
    PartDef { name: "mao_direita", aspect: 1.7315, origin: (0.546, 0.2268), tip: (0.728, 1.7246), conns: &[(0.546, 0.2268)] },
    PartDef { name: "mao_esquerda", aspect: 1.396, origin: (0.805, 0.1926), tip: (0.316, 1.3792), conns: &[(0.805, 0.1926)] },
    PartDef { name: "femur_direito", aspect: 2.6515, origin: (0.74, 0.2572), tip: (0.429, 2.3917), conns: &[(0.74, 0.2572), (0.429, 2.3917)] },
    PartDef { name: "femur_esquerdo", aspect: 2.6515, origin: (0.74, 0.2572), tip: (0.428, 2.3917), conns: &[(0.74, 0.2572), (0.428, 2.3917)] },
    PartDef { name: "tibia_direita", aspect: 0.977, origin: (0.112, 0.1104), tip: (0.887, 0.8637), conns: &[(0.112, 0.1104), (0.887, 0.8637)] },
    PartDef { name: "tibia_esquerda", aspect: 0.9916, origin: (0.114, 0.114), tip: (0.885, 0.8756), conns: &[(0.114, 0.114), (0.885, 0.8756)] },
    PartDef { name: "pe_direito", aspect: 1.6545, origin: (0.821, 0.177), tip: (0.0, 1.5503), conns: &[(0.821, 0.177)] },
    PartDef { name: "pe_esquerdo", aspect: 1.6545, origin: (0.821, 0.177), tip: (0.0, 1.5503), conns: &[(0.821, 0.177)] },
];

/// Geometria de uma peça-imagem do rig, em unidades da imagem (x/W, y/W).
struct PieceDef {
    aspect: f32,
    origin: (f32, f32),
    tip: (f32, f32),
    conns: &'static [(f32, f32)],
    tex: usize,
}

fn piece_def(shape: sketchmotion_core::BoneShape) -> PieceDef {
    use sketchmotion_core::BoneShape as BS;
    match shape {
        BS::Limb => PieceDef {
            aspect: 8.6106,
            origin: (0.498, 0.3789),
            tip: (0.498, 8.2317),
            conns: &[(0.498, 0.3789), (0.498, 8.2317)],
            tex: 0,
        },
        BS::Torso => PieceDef {
            aspect: 1.5409,
            origin: (0.500, 1.4823),
            tip: (0.500, 0.0570),
            conns: &[(0.500, 1.4823), (0.500, 0.0570), (0.074, 0.4268), (0.925, 0.4268)],
            tex: 1,
        },
        BS::Hip => PieceDef {
            aspect: 0.7157,
            origin: (0.500, 0.1546),
            tip: (0.498, 0.5114),
            conns: &[(0.500, 0.1546), (0.095, 0.5053), (0.901, 0.5175)],
            tex: 2,
        },
        BS::Head => PieceDef {
            aspect: 1.7784,
            origin: (0.519, 1.6397),
            tip: (0.5, 0.02),
            conns: &[(0.519, 1.6397)],
            tex: 3,
        },
        BS::Mao => PieceDef {
            aspect: 1.0541,
            origin: (0.500, 0.2024),
            tip: (0.5, 1.03),
            conns: &[(0.500, 0.2024)],
            tex: 4,
        },
        BS::Pe => PieceDef {
            aspect: 0.4400,
            origin: (0.906, 0.0933),
            tip: (0.03, 0.40),
            conns: &[(0.906, 0.0933)],
            tex: 5,
        },
    }
}

/// Similaridade por 2 pontos: mapeia (px,py) do espaço da imagem da peça para o
/// mundo, dado que p_orig->w_orig e p_tip->w_tip.
fn map_piece(
    p_orig: (f32, f32),
    p_tip: (f32, f32),
    w_orig: (f32, f32),
    w_tip: (f32, f32),
    px: f32,
    py: f32,
) -> (f32, f32) {
    let aimg = (p_tip.0 - p_orig.0, p_tip.1 - p_orig.1);
    let aw = (w_tip.0 - w_orig.0, w_tip.1 - w_orig.1);
    let limg = (aimg.0 * aimg.0 + aimg.1 * aimg.1).sqrt().max(1e-4);
    let lw = (aw.0 * aw.0 + aw.1 * aw.1).sqrt().max(1e-4);
    let ang = aw.1.atan2(aw.0) - aimg.1.atan2(aimg.0);
    let sc = lw / limg;
    let (s, c) = ang.sin_cos();
    let (cc, ss) = (sc * c, sc * s);
    let (dx, dy) = (px - p_orig.0, py - p_orig.1);
    (w_orig.0 + dx * cc - dy * ss, w_orig.1 + dx * ss + dy * cc)
}

fn upscale_nn(w: u32, h: u32, rgba: &[u8], scale: u32) -> (u32, u32, Vec<u8>) {
    if scale <= 1 {
        return (w, h, rgba.to_vec());
    }
    let (w2, h2) = (w * scale, h * scale);
    let mut out = vec![0u8; (w2 * h2 * 4) as usize];
    for y in 0..h2 {
        let sy = y / scale;
        for x in 0..w2 {
            let sx = x / scale;
            let sidx = ((sy * w + sx) * 4) as usize;
            let didx = ((y * w2 + x) * 4) as usize;
            if sidx + 4 <= rgba.len() {
                out[didx..didx + 4].copy_from_slice(&rgba[sidx..sidx + 4]);
            }
        }
    }
    (w2, h2, out)
}

fn nudge_i32(ui: &mut egui::Ui, v: &mut i32, lo: i32, hi: i32) {
    if ui.small_button("◀").on_hover_text("Diminuir").clicked() {
        *v = (*v - 1).clamp(lo, hi);
    }
    if ui.small_button("▶").on_hover_text("Aumentar").clicked() {
        *v = (*v + 1).clamp(lo, hi);
    }
}

fn nudge_u32(ui: &mut egui::Ui, v: &mut u32, lo: u32, hi: u32) {
    if ui.small_button("◀").on_hover_text("Diminuir").clicked() {
        *v = (*v).saturating_sub(1).max(lo);
    }
    if ui.small_button("▶").on_hover_text("Aumentar").clicked() {
        *v = (*v + 1).min(hi);
    }
}

fn refl_x(axis: Option<f32>, x: f32) -> f32 {
    match axis {
        Some(a) => 2.0 * a - x,
        None => x,
    }
}

fn resize_cursor(sx: f32, sy: f32) -> egui::CursorIcon {
    use egui::CursorIcon as CI;
    match (sx as i32, sy as i32) {
        (0, _) => CI::ResizeVertical,
        (_, 0) => CI::ResizeHorizontal,
        (a, b) if a == b => CI::ResizeNwSe,
        _ => CI::ResizeNeSw,
    }
}

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
/// Presets do Traçado de Imagem (índice = `trace_preset`).
const TRACE_PRESETS: [&str; 12] = [
    "[Padrão]",
    "Foto de alta fidelidade",
    "Foto de baixa fidelidade",
    "3 cores",
    "6 cores",
    "16 cores",
    "Tonalidades de cinza",
    "Logotipo preto-e-branco",
    "Esboço artístico",
    "Silhuetas",
    "Traçado",
    "Desenho técnico",
];

/// Receita de traçado (parâmetros já resolvidos a partir do preset) — enviada a
/// uma thread de trabalho para vetorizar sem travar a interface.
enum TraceRecipe {
    Quant(QuantParams),
    Bilevel(BilevelParams),
}

/// Resolve o preset + remoção de fundo + escala numa receita concreta.
fn trace_recipe(preset: usize, remove_bg: bool, escala: f32) -> TraceRecipe {
    let es = escala.max(0.1);
    let ee = es.sqrt(); // epsilon cresce devagar → mais nós, mais preciso
    let ea = (es * es).max(1.0); // área mínima proporcional à resolução
    let q = |colors: usize, gray: bool, simp: f32, min_a: usize| {
        TraceRecipe::Quant(QuantParams {
            colors,
            grayscale: gray,
            simplify: simp * ee,
            min_area: ((min_a as f32) * ea) as usize,
            remove_bg,
        })
    };
    let b = |th: u8, inv: bool, alpha: bool, simp: f32, min_a: usize| {
        TraceRecipe::Bilevel(BilevelParams {
            threshold: th,
            invert: inv,
            alpha_only: alpha,
            simplify: simp * ee,
            min_area: ((min_a as f32) * ea) as usize,
        })
    };
    match preset {
        0 => q(12, false, 1.0, 20),          // Padrão
        1 => q(32, false, 0.5, 8),           // Foto alta
        2 => q(8, false, 1.6, 40),           // Foto baixa
        3 => q(3, false, 1.0, 16),           // 3 cores
        4 => q(6, false, 1.0, 16),           // 6 cores
        5 => q(16, false, 0.8, 12),          // 16 cores
        6 => q(6, true, 1.2, 20),            // Tons de cinza
        7 => b(128, false, false, 0.8, 16),  // Logotipo P&B
        8 => b(120, false, false, 1.2, 12),  // Esboço
        9 => b(210, false, false, 2.0, 30),  // Silhuetas
        10 => b(128, false, false, 0.8, 8),  // Traçado
        _ => q(4, false, 2.4, 30),           // Desenho técnico
    }
}

/// Executa a receita sobre a imagem (rgba, w, h), reportando progresso.
fn apply_recipe(r: &TraceRecipe, rgba: &[u8], w: u32, h: u32, prog: &AtomicU32) -> Vec<TracedRegion> {
    match r {
        TraceRecipe::Quant(p) => trace_quantized(rgba, w, h, p, prog),
        TraceRecipe::Bilevel(p) => trace_bilevel(rgba, w, h, p, prog),
    }
}

/// Tipo de exportação para o job de fundo.
#[derive(Clone, Copy)]
enum ExportKind {
    Gif,
    Mp4,
    Seq,
    Sheet,
}

/// Abertura de projeto em andamento (thread de fundo → tela de carregamento).
struct OpenJob {
    rx: mpsc::Receiver<Result<Document, String>>,
    started: std::time::Instant,
    path: std::path::PathBuf,
}

/// Trabalho pesado (exportar/salvar) em andamento: barra de progresso + bloqueio.
struct BusyJob {
    rx: mpsc::Receiver<Result<String, String>>,
    progress: Arc<AtomicU32>,
    started: std::time::Instant,
    titulo: String,
    determinate: bool,
    set_path: Option<std::path::PathBuf>,
}

/// Aplica o enquadramento da câmera (frame `pf`) sobre `full` — versão livre
/// (sem `self`), para rodar na thread de exportação.
fn aplicar_camera_livre(cam: &Camera, full: &[u8], dw: u32, dh: u32, pf: usize) -> Vec<u8> {
    let s = cam.sample(pf);
    let (ow, oh) = (dw as usize, dh as usize);
    let mut out = vec![0u8; ow * oh * 4];
    if full.len() < ow * oh * 4 {
        return out;
    }
    let ang = s.rotation.to_radians();
    let (sin, cos) = ang.sin_cos();
    for oy in 0..oh {
        for ox in 0..ow {
            let u = (ox as f32 + 0.5) / ow as f32 - 0.5;
            let v = (oy as f32 + 0.5) / oh as f32 - 0.5;
            let lx = u * s.w;
            let ly = v * s.h;
            let dx = s.x + lx * cos - ly * sin;
            let dy = s.y + lx * sin + ly * cos;
            let sx = dx.floor() as i32;
            let sy = dy.floor() as i32;
            let di = (oy * ow + ox) * 4;
            if sx >= 0 && sy >= 0 && (sx as u32) < dw && (sy as u32) < dh {
                let si = ((sy as u32 * dw + sx as u32) * 4) as usize;
                out[di..di + 4].copy_from_slice(&full[si..si + 4]);
            }
        }
    }
    out
}

/// Renderiza + codifica a exportação (roda na thread), reportando progresso.
#[allow(clippy::too_many_arguments)]
fn run_export(
    kind: ExportKind,
    path: std::path::PathBuf,
    frames: Vec<Frame>,
    camera: Camera,
    export_camera: bool,
    w: u32,
    h: u32,
    fps: u32,
    scale: u32,
    cols: u32,
    progress: &AtomicU32,
) -> Result<String, String> {
    let sc = scale.max(1);
    let (ew, eh) = (w * sc, h * sc);
    let nf = frames.len().max(1) as u32;
    let render = |i: usize, f: &Frame| -> Vec<u8> {
        let img = render_frame_alpha(w, h, &f.layers, &f.vectors, &f.images);
        let base = if export_camera {
            aplicar_camera_livre(&camera, &img.rgba, w, h, i)
        } else {
            img.rgba
        };
        let (_, _, up) = upscale_nn(w, h, &base, sc);
        up
    };
    match kind {
        ExportKind::Gif | ExportKind::Mp4 => {
            let mut ups: Vec<Vec<u8>> = Vec::with_capacity(frames.len());
            for (i, f) in frames.iter().enumerate() {
                ups.push(render(i, f));
                progress.store((i as u32 * 900 / nf).min(899), Ordering::Relaxed);
            }
            progress.store(910, Ordering::Relaxed);
            let r = match kind {
                ExportKind::Gif => sketchmotion_io::export_gif(ew, eh, &ups, fps, &path),
                _ => sketchmotion_io::export_mp4(ew, eh, &ups, fps, &path),
            };
            progress.store(1000, Ordering::Relaxed);
            r.map(|_| format!("Exportado ({} frames): {}", ups.len(), path.display()))
        }
        ExportKind::Seq => {
            let mut ok = 0usize;
            for (i, f) in frames.iter().enumerate() {
                let up = render(i, f);
                let fp = path.join(format!("frame_{:04}.png", i + 1));
                sketchmotion_io::export_png(ew, eh, &up, &fp)?;
                ok += 1;
                progress.store((i as u32 * 1000 / nf).min(999), Ordering::Relaxed);
            }
            progress.store(1000, Ordering::Relaxed);
            Ok(format!("{ok} PNG(s) salvos em {}", path.display()))
        }
        ExportKind::Sheet => {
            let (fw, fh) = (ew, eh);
            let ncols = if cols == 0 { nf } else { cols.max(1) };
            let rows = (nf + ncols - 1) / ncols;
            let (sw, sh) = (fw * ncols, fh * rows);
            let mut sheet = vec![0u8; (sw * sh * 4) as usize];
            for (i, f) in frames.iter().enumerate() {
                let up = render(i, f);
                let (cx, cy) = (i as u32 % ncols, i as u32 / ncols);
                let (ox, oy) = (cx * fw, cy * fh);
                let rowlen = (fw * 4) as usize;
                for y in 0..fh {
                    let dst = (((oy + y) * sw + ox) * 4) as usize;
                    let src = ((y * fw) * 4) as usize;
                    if dst + rowlen <= sheet.len() && src + rowlen <= up.len() {
                        sheet[dst..dst + rowlen].copy_from_slice(&up[src..src + rowlen]);
                    }
                }
                progress.store((i as u32 * 1000 / nf).min(999), Ordering::Relaxed);
            }
            progress.store(1000, Ordering::Relaxed);
            sketchmotion_io::export_png(sw, sh, &sheet, &path)
                .map(|_| format!("Sprite sheet {sw}x{sh}: {}", path.display()))
        }
    }
}

/// Trabalho de vetorização em andamento (thread de fundo + progresso).
struct TraceJob {
    rx: mpsc::Receiver<Vec<TracedRegion>>,
    progress: Arc<AtomicU32>,
    sw: f32,
    sh: f32,
    from_float: bool,
    img_idx: Option<usize>,
    preset: usize,
}

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
#[derive(Clone)]
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
    /// Camada à qual este elemento pertence. O bloqueio e a confirmação
    /// seguem ESTA camada, não a camada ativa no momento.
    layer: usize,
    /// `true` quando a flutuante é um OBJETO DE IMAGEM (Caminho B): ao soltar,
    /// vira um `ImageObject` persistente em vez de ser integrada aos pixels.
    /// Só o botão "Integrar" a rasteriza na camada.
    is_image: bool,
}

/// Área de transferência de OBJETOS (vetores e/ou imagens) para
/// recortar/copiar/colar mantendo posição e tamanho, inclusive entre frames,
/// camadas ou arquivos.
#[derive(Clone, Default)]
struct ObjClip {
    vectors: Vec<VectorObject>,
    images: Vec<ImageObject>,
}

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    /// Splash de inicialização (GIF que roda uma vez e congela no último frame).
    Splash,
    Home,
    Editor,
}

#[derive(Clone, Copy, PartialEq)]
enum Eyedropper {
    Off,
    ToBrush,
    ToArea(usize),
}

#[derive(Clone, Copy, PartialEq)]
enum RigMode {
    /// Criar ossos arrastando (encadeia a partir da ponta de um osso existente).
    Create,
    /// Posar: mover/rotacionar um osso; filhos acompanham (FK).
    Pose,
}

#[derive(Clone, Copy, PartialEq)]
enum RigDrag {
    None,
    Move,
    Rotate,
    Resize,
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
    // objetos/peças reutilizáveis (biblioteca global, colar como seleção)
    pieces: PieceLibrary,
    pieces_dirty: bool,
    selected_piece: Option<usize>,
    place_piece: bool,
    win_pieces: bool,
    /// Captura pendente aguardando confirmação de "agrupar" (w, h, rgba).
    pending_group: Option<(u32, u32, Vec<u8>)>,
    pending_name: String,
    // edição isolada de uma peça (um "novo canvas" com o objeto desenhado)
    win_edit_piece: Option<usize>,
    edit_w: u32,
    edit_h: u32,
    edit_buf: Vec<u8>,
    edit_erase: bool,
    edit_size: i32,
    edit_zoom: f32,
    edit_tex: Option<egui::TextureHandle>,
    edit_dirty: bool,
    icon_r_color: Option<egui::Rect>,
    icon_r_palette: Option<egui::Rect>,
    reopen_color: bool,
    reopen_palette: bool,
    eyedropper: Eyedropper,
    active_layer: usize,
    win_layers: bool,
    reopen_layers: bool,
    icon_r_layers: Option<egui::Rect>,
    // Rig 2D (painel direito)
    win_rig: bool,
    reopen_rig: bool,
    icon_r_rig: Option<egui::Rect>,
    show_bones: bool,
    rig_skel: usize,
    rig_mode: RigMode,
    rig_sel_bone: Option<u32>,
    rig_dragging: RigDrag,
    rig_grab: (f32, f32),
    rig_start: Option<(f32, f32)>,
    rig_preview: Option<(f32, f32)>,
    rig_shape: sketchmotion_core::BoneShape,
    cursor_tex: [Option<egui::TextureHandle>; 6],
    piece_tex: [Option<egui::TextureHandle>; 6],
    part_tex: [Option<egui::TextureHandle>; 21],
    raptor_mini_tex: Option<egui::TextureHandle>,
    rig_snap_hint: Option<(u32, f32, f32)>,
    rig_resize_enabled: bool,
    rig_sep_guard: Option<(u32, f32, f32)>,
    sel_rig: Option<usize>,
    current_path: Option<std::path::PathBuf>,
    export_scale: u32,
    export_cols: u32,
    /// Exportar já enquadrado pela câmera (keyframes). Ligado por padrão.
    export_camera: bool,
    bg_white: bool,
    /// Fundo escuro (cinza bem escuro) — só visual, ajuda a ver certos detalhes.
    bg_dark: bool,
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
    wand_contiguo: bool,
    pen_width: i32,
    // estado vetorial
    selected_obj: Option<usize>,
    /// Seleção múltipla de objetos vetoriais (para mover em conjunto / agrupar).
    sel_set: Vec<usize>,
    /// Próximo id de grupo a distribuir.
    next_group: u32,
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
    // aviso temporário (ex.: tentativa de editar camada bloqueada)
    warn_ticks: u32,
    // seleção retangular raster (estilo Paint)
    float_sel: Option<FloatSel>,
    float_tex: Option<egui::TextureHandle>,
    // camadas ABAIXO e ACIMA da flutuante, para a flutuante respeitar o
    // empilhamento (aparecer entre as camadas certas, não sempre no topo).
    below_tex: Option<egui::TextureHandle>,
    above_tex: Option<egui::TextureHandle>,
    split_for: Option<usize>,
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
    /// Área de transferência de pixels (largura, altura, RGBA) para copiar/colar
    /// uma seleção — inclusive entre frames.
    clip: Option<(u32, u32, Vec<u8>)>,
    // frames / animação
    onion: bool,
    onion_tex: Option<egui::TextureHandle>,
    onion_for: Option<usize>,
    // timelines (faixas)
    view_both: bool,
    play_both: bool,
    /// Objetos recortados/copiados (vetores e imagens) — colar mantém posição.
    obj_clip: ObjClip,
    /// A última cópia foi de OBJETOS (true) ou de pixels/seleção raster (false)?
    clip_objetos: bool,
    /// Frame copiado (Copiar/Colar frames, inclusive entre timelines/arquivos).
    /// Seleção de vários frames (na faixa ativa) para copiar em conjunto.
    frame_sel: Vec<usize>,
    /// Modo "selecionar frames": clique alterna a seleção em vez de navegar.
    frame_sel_mode: bool,
    /// Conjunto de frames copiados (colados em sequência).
    frames_clip: Vec<Frame>,
    /// Arrasto de frame em andamento: (faixa, índice do frame).
    drag_frame: Option<(usize, usize)>,
    /// Recentralizar o canvas na próxima renderização (pedido único).
    center_canvas: bool,
    /// Fator da margem ao redor do canvas (pasteboard) — "estender a área".
    workspace_pad: f32,
    /// Timeline recolhida (mostra só uma barra para reabrir).
    timeline_hidden: bool,
    /// Altura (px) da área de timeline; arrastável pela alça superior.
    timeline_h: f32,
    // Câmera: janela de controle do enquadramento animado por keyframes.
    win_camera: bool,
    /// Interpolação padrão ao criar novos keyframes de câmera.
    cam_interp: Interp,
    /// Manipulação direta do retângulo: 0 nada, 1 mover, 2 redimensionar,
    /// 3 rotacionar, 4 pivô.
    cam_act: u8,
    /// Canto sendo arrastado (0 TL, 1 TR, 2 BR, 3 BL) quando cam_act == 2.
    cam_h: usize,
    /// Estado do keyframe no início do arrasto (referência).
    cam_orig: CameraKeyframe,
    /// Offset (doc) do ponteiro em relação ao centro, para mover.
    cam_grab: (f32, f32),
    /// Ângulo inicial do ponteiro (rad) para rotacionar.
    cam_start_ang: f32,
    /// Frame de origem ao arrastar um keyframe na trilha da câmera (timeline).
    cam_kf_drag: Option<usize>,
    /// Textura da logo (tela inicial), carregada uma vez.
    logo_tex: Option<egui::TextureHandle>,
    /// Ícone de página pixel art (tela inicial), carregado uma vez.
    page_tex: Option<egui::TextureHandle>,
    /// Ícone de página normal (tela inicial), carregado uma vez.
    page_normal_tex: Option<egui::TextureHandle>,
    /// Ícone PNG da ferramenta Pivô (assets/pivo.png), carregado uma vez.
    tex_pivo: Option<egui::TextureHandle>,
    /// Ícone PNG do Traçado de Imagem (assets/vetor_trace.png), carregado uma vez.
    tex_vetor_trace: Option<egui::TextureHandle>,
    // Ferramenta Pivô (eixo de transformação personalizado).
    win_pivot: bool,
    /// Pivô atualmente selecionado na lista/edição.
    pivot_sel: Option<usize>,
    /// Fluxo de criação: 0 = ocioso, 1 = aguardando clique do EIXO (associa o
    /// objeto), 2 = aguardando clique do ponto de MOVIMENTAÇÃO.
    pivot_stage: u8,
    /// Arraste em andamento: 0 = nenhum, 1 = eixo (azul), 2 = movimentação
    /// (laranja), 3 = objeto.
    pivot_drag: u8,
    /// Janelinha de nome ao criar um pivô novo.
    pivot_naming: bool,
    /// Texto do nome sendo digitado.
    pivot_name_buf: String,
    /// Ícone PNG do Vetor de Direção (assets/vetor_direcao.png).
    tex_dirvec: Option<egui::TextureHandle>,
    // Ferramenta Vetor de Direção (animação automática por interpolação).
    win_dirvec: bool,
    /// Vetor de direção selecionado na lista.
    dirvec_sel: Option<usize>,
    /// Fluxo de criação: 0 = ocioso, 1 = aguardando clique no objeto (associar).
    dirvec_stage: u8,
    /// Arrastando o objeto para definir a posição final?
    dirvec_dragging: bool,
    /// Rotação acumulada no arraste (rad), quando o objeto tem pivô.
    dirvec_drag_angle: f32,
    /// Posição (centro) do "fantasma" — o objeto em suspensão num frame onde
    /// ele ainda não existe, à espera de ser arrastado para a posição final.
    dirvec_ghost_pos: (f32, f32),
    /// Rotação do fantasma (rad), quando há pivô.
    dirvec_ghost_ang: f32,
    /// Frame para o qual o fantasma está preparado (None = sem fantasma).
    dirvec_ghost_frame: Option<usize>,
    /// Arrastando o fantasma?
    dirvec_ghost_drag: bool,
    /// Textura do fantasma da imagem (pré-visualização da própria peça).
    dirvec_ghost_tex: Option<egui::TextureHandle>,
    /// Índice do vetor para o qual a textura do fantasma foi montada.
    dirvec_ghost_tex_for: Option<usize>,
    /// Janelinha de nome ao criar um vetor novo.
    dirvec_naming: bool,
    /// Texto do nome sendo digitado.
    dirvec_name_buf: String,
    // Traçado de Imagem (raster → vetor).
    win_trace: bool,
    /// Imagem de origem para traçar (já reduzida): (w, h, rgba).
    trace_src: Option<(u32, u32, Vec<u8>)>,
    /// Transformação do objeto de imagem no canvas: (cx, cy, hw, hh, angle).
    /// Os contornos são mapeados por ela (fica alinhado sobre a imagem).
    trace_tf: (f32, f32, f32, f32, f32),
    /// A origem é a flutuante de imagem atual (para acompanhar mover/escalar).
    trace_from_float: bool,
    /// Índice do objeto de imagem (document.images) usado como origem, se aplicável.
    trace_img_idx: Option<usize>,
    /// Preset aguardando confirmação "deseja vetorizar?" (índice em TRACE_PRESETS).
    trace_confirm: Option<usize>,
    /// Preset de traçado escolhido (índice em TRACE_PRESETS).
    trace_preset: usize,
    /// Remover fundo (cor de borda vira transparência nos modos coloridos).
    trace_remove_bg: bool,
    /// Prévia calculada (regiões em coords da imagem de origem).
    trace_regions: Vec<TracedRegion>,
    trace_dirty: bool,
    /// Vetorização final rodando em segundo plano (com barra de progresso).
    trace_job: Option<TraceJob>,
    /// Exportar/salvar rodando em segundo plano (barra de progresso + bloqueio).
    busy_job: Option<BusyJob>,
    /// Abertura de projeto em andamento (tela de carregamento).
    open_job: Option<OpenJob>,
    /// Splash: frames do GIF (textura + duração em segundos).
    splash_frames: Vec<(egui::TextureHandle, f32)>,
    /// Splash: já decodificou o GIF?
    splash_loaded: bool,
    /// Splash: índice do frame atual.
    splash_idx: usize,
    /// Splash: quando o frame atual começou a ser exibido.
    splash_frame_started: Option<std::time::Instant>,
    /// Splash: chegou ao último frame (congelado)?
    splash_done: bool,
    /// Splash: instante em que congelou no último frame.
    splash_done_at: Option<std::time::Instant>,
    /// Splash: janela já ajustada ao tamanho do GIF (sem bordas)?
    splash_win_set: bool,
    /// Splash: janela já centralizada na tela?
    splash_centered: bool,
    /// O trabalho foi modificado desde o último salvamento.
    modificado: bool,
    /// Diálogo "salvar antes de sair?" visível.
    win_fechar: bool,
    /// Fechamento já confirmado (permite a janela fechar sem novo diálogo).
    confirmado_fechar: bool,
    // Prancheta: redimensionar o papel (canvas) sem mexer no desenho.
    win_prancheta: bool,
    pr_w: u32,
    pr_h: u32,
    /// Alça de prancheta em arrasto: 0=largura, 1=altura, 2=ambos.
    prancheta_drag: Option<u8>,
    /// Tamanho de pré-visualização durante o arrasto da prancheta.
    prancheta_preview: Option<(u32, u32)>,
    /// Cache de miniaturas por faixa/frame (evita re-renderizar tudo a cada quadro).
    track_thumbs: Vec<Vec<Option<egui::TextureHandle>>>,
    /// Cache das texturas de sobreposição entre timelines (onion/ver ambas).
    between_cache: Vec<Option<egui::TextureHandle>>,
    /// Chave do cache acima: (faixa ativa, página atual, nº de faixas, onion_between, view_both).
    between_key: Option<(usize, usize, usize, bool, bool)>,
    // pincéis
    brush_kind: usize,
    rng: u32,
    smudge: Option<[f32; 4]>,
    brush_prev: Vec<Option<egui::TextureHandle>>,
    playing: bool,
    play_frame: usize,
    play_accum: f32,
    play_loop: bool,
    play_done: bool,
    /// Na reprodução, mostrar já enquadrado pela câmera (keyframes animados).
    play_camera: bool,
    play_tex: Option<egui::TextureHandle>,
}

impl SketchMotionApp {
    fn new() -> Self {
        let library = sketchmotion_io::default_library_path()
            .and_then(|p| sketchmotion_io::load_library(&p).ok())
            .unwrap_or_default();
        let selected_char = if library.characters.is_empty() { None } else { Some(0) };
        let pieces = sketchmotion_io::default_pieces_path()
            .and_then(|p| sketchmotion_io::load_pieces(&p).ok())
            .unwrap_or_default();

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
            pieces,
            pieces_dirty: false,
            selected_piece: None,
            place_piece: false,
            win_pieces: false,
            pending_group: None,
            pending_name: String::new(),
            win_edit_piece: None,
            edit_w: 0,
            edit_h: 0,
            edit_buf: Vec::new(),
            edit_erase: true,
            edit_size: 3,
            edit_zoom: 4.0,
            edit_tex: None,
            edit_dirty: false,
            icon_r_color: None,
            icon_r_palette: None,
            reopen_color: false,
            reopen_palette: false,
            eyedropper: Eyedropper::Off,
            active_layer: 0,
            win_layers: false,
            reopen_layers: false,
            icon_r_layers: None,
            win_rig: false,
            reopen_rig: false,
            icon_r_rig: None,
            show_bones: true,
            rig_skel: 0,
            rig_mode: RigMode::Create,
            rig_sel_bone: None,
            rig_dragging: RigDrag::None,
            rig_grab: (0.0, 0.0),
            rig_start: None,
            rig_preview: None,
            rig_shape: sketchmotion_core::BoneShape::Limb,
            cursor_tex: [None, None, None, None, None, None],
            piece_tex: [None, None, None, None, None, None],
            part_tex: std::array::from_fn(|_| None),
            raptor_mini_tex: None,
            rig_snap_hint: None,
            rig_resize_enabled: false,
            rig_sep_guard: None,
            sel_rig: None,
            current_path: None,
            export_scale: 1,
            export_cols: 0,
            export_camera: true,
            bg_white: false,
            bg_dark: false,
            pixel_mode: false,
            screen: Screen::Splash,
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
            wand_contiguo: true,
            pen_width: 2,
            selected_obj: None,
            sel_set: Vec::new(),
            next_group: 1,
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
            warn_ticks: 0,
            float_sel: None,
            float_tex: None,
            below_tex: None,
            above_tex: None,
            split_for: None,
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
            clip: None,
            onion: true,
            onion_tex: None,
            onion_for: None,
            view_both: false,
            play_both: false,
            frame_sel: Vec::new(),
            frame_sel_mode: false,
            obj_clip: ObjClip::default(),
            clip_objetos: false,
            frames_clip: Vec::new(),
            drag_frame: None,
            center_canvas: true,
            workspace_pad: 0.9,
            timeline_hidden: false,
            timeline_h: 260.0,
            win_camera: false,
            cam_interp: Interp::Linear,
            cam_act: 0,
            cam_h: 0,
            cam_orig: CameraKeyframe::default(),
            cam_grab: (0.0, 0.0),
            cam_start_ang: 0.0,
            cam_kf_drag: None,
            logo_tex: None,
            page_tex: None,
            page_normal_tex: None,
            tex_pivo: None,
            tex_vetor_trace: None,
            win_pivot: false,
            pivot_sel: None,
            pivot_stage: 0,
            pivot_drag: 0,
            pivot_naming: false,
            pivot_name_buf: String::new(),
            tex_dirvec: None,
            win_dirvec: false,
            dirvec_sel: None,
            dirvec_stage: 0,
            dirvec_dragging: false,
            dirvec_drag_angle: 0.0,
            dirvec_ghost_pos: (0.0, 0.0),
            dirvec_ghost_ang: 0.0,
            dirvec_ghost_frame: None,
            dirvec_ghost_drag: false,
            dirvec_ghost_tex: None,
            dirvec_ghost_tex_for: None,
            dirvec_naming: false,
            dirvec_name_buf: String::new(),
            win_trace: false,
            trace_src: None,
            trace_tf: (0.0, 0.0, 0.0, 0.0, 0.0),
            trace_from_float: false,
            trace_img_idx: None,
            trace_confirm: None,
            trace_preset: 3, // "3 cores" por padrão
            trace_remove_bg: true,
            trace_regions: Vec::new(),
            trace_dirty: false,
            trace_job: None,
            busy_job: None,
            open_job: None,
            splash_frames: Vec::new(),
            splash_loaded: false,
            splash_idx: 0,
            splash_frame_started: None,
            splash_done: false,
            splash_done_at: None,
            splash_win_set: false,
            splash_centered: false,
            modificado: false,
            win_fechar: false,
            confirmado_fechar: false,
            win_prancheta: false,
            pr_w: CANVAS_W,
            pr_h: CANVAS_H,
            prancheta_drag: None,
            prancheta_preview: None,
            track_thumbs: Vec::new(),
            between_cache: Vec::new(),
            between_key: None,
            brush_kind: 0,
            rng: 0x2545_F491,
            smudge: None,
            brush_prev: Vec::new(),
            playing: false,
            play_frame: 0,
            play_accum: 0.0,
            play_loop: true,
            play_done: false,
            play_camera: true,
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

    /// A camada `li` está bloqueada? (também true se não existir.)
    fn layer_locked(&self, li: usize) -> bool {
        self.document.layer(li).map_or(true, |l| l.locked)
    }

    /// A camada ativa está bloqueada? (também true se não existir camada.)
    fn active_locked(&self) -> bool {
        self.layer_locked(self.active_layer)
    }

    /// A camada à qual a seleção/imagem flutuante pertence está bloqueada?
    fn float_locked(&self) -> bool {
        match &self.float_sel {
            Some(f) => self.layer_locked(f.layer),
            None => false,
        }
    }

    /// Dispara um aviso visível de que a camada `li` está bloqueada.
    ///
    /// Só NOTIFICA quando `li` é a camada ATIVA (você está nela). Se você está
    /// em outra camada e tentou interagir com algo de uma camada bloqueada, a
    /// interação é apenas recusada (pelo `return` de quem chamou), sem aviso.
    fn warn_locked_layer(&mut self, li: usize) {
        if li != self.active_layer {
            return;
        }
        let nome = self
            .document
            .layer(li)
            .map(|l| l.name.clone())
            .unwrap_or_default();
        self.status = format!("⚠ Camada \"{nome}\" bloqueada — desbloqueie na aba Camadas para editar");
        self.warn_ticks = 150;
    }

    /// Aviso de bloqueio da camada ativa (para ferramentas de desenho).
    fn warn_lock(&mut self) {
        let li = self.active_layer;
        self.warn_locked_layer(li);
    }

    /// True se a camada ATIVA não pode receber edição agora — bloqueada ou
    /// oculta — já emitindo o aviso apropriado. As ferramentas de desenho e de
    /// preenchimento checam isto antes de agir.
    fn active_bloqueada_para_edicao(&mut self) -> bool {
        let li = self.active_layer;
        let (existe, locked, visible, nome) = match self.document.layer(li) {
            Some(l) => (true, l.locked, l.visible, l.name.clone()),
            None => (false, true, false, String::new()),
        };
        if !existe {
            return true;
        }
        if locked {
            self.warn_lock();
            return true;
        }
        if !visible {
            self.status =
                format!("⚠ Camada \"{nome}\" oculta — mostre-a (ícone do olho) para editar");
            self.warn_ticks = 150;
            return true;
        }
        false
    }

    /// Salva o estado atual no histórico e limpa o refazer. O histórico é
    /// ilimitado (cresce conforme as ações; só a memória disponível o limita).
    fn push_undo(&mut self) {
        self.undo_stack.push(self.document.clone());
        self.redo_stack.clear();
        self.modificado = true;
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.document.clone());
            self.document = prev;
            self.active_layer = self
                .active_layer
                .min(self.document.layers.len().saturating_sub(1));
            self.last_pos = None;
            // Uma colagem pendente (flutuante não confirmada) é cancelada pelo
            // undo — assim cada colar é revertido isoladamente.
            self.float_sel = None;
            self.float_tex = None;
            self.sel_set.clear();
            self.selected_obj = None;
            self.modificado = true;
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
            self.float_sel = None;
            self.float_tex = None;
            self.sel_set.clear();
            self.selected_obj = None;
            self.modificado = true;
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

    fn rand_u32(&mut self) -> u32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        x
    }
    fn rand_f(&mut self) -> f32 {
        (self.rand_u32() >> 8) as f32 / 16_777_216.0
    }

    /// Mistura uma cor sobre um pixel da camada ativa (alpha over com cobertura).
    fn blend_px(&mut self, li: usize, x: i32, y: i32, c: Color, cover: f32) {
        if x < 0 || y < 0 {
            return;
        }
        let sa = (c.a as f32 / 255.0) * cover.clamp(0.0, 1.0);
        if sa <= 0.0 {
            return;
        }
        if let Some(layer) = self.document.layer_mut(li) {
            if let Some(d) = layer.get_pixel(x as u32, y as u32) {
                let da = d.a as f32 / 255.0;
                let oa = sa + da * (1.0 - sa);
                if oa <= 0.0 {
                    return;
                }
                let bl = |sc: u8, dc: u8| {
                    (((sc as f32 / 255.0 * sa + dc as f32 / 255.0 * da * (1.0 - sa)) / oa) * 255.0)
                        .round() as u8
                };
                layer.set_pixel(
                    x as u32,
                    y as u32,
                    Color::rgba(bl(c.r, d.r), bl(c.g, d.g), bl(c.b, d.b), (oa * 255.0).round() as u8),
                );
            }
        }
    }

    fn stamp_hard(&mut self, x: i32, y: i32, r: i32) {
        let color = self.active_color();
        let li = self.active_layer;
        let pixel = self.pixel_mode;
        if let Some(layer) = self.document.layer_mut(li) {
            if pixel {
                // Pixel art: pincel quadrado de lado = raio (mínimo = 1 pixel).
                let s = r.max(1);
                let half = s / 2;
                for dy in -half..=(s - 1 - half) {
                    for dx in -half..=(s - 1 - half) {
                        let (px, py) = (x + dx, y + dy);
                        if px >= 0 && py >= 0 {
                            layer.set_pixel(px as u32, py as u32, color);
                        }
                    }
                }
            } else if r <= 1 {
                // Tamanho mínimo = 1 pixel (precisão), mesmo fora do pixel art.
                if x >= 0 && y >= 0 {
                    layer.set_pixel(x as u32, y as u32, color);
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
        }
    }

    fn stamp_soft(&mut self, x: i32, y: i32, r: i32, master: f32) {
        let color = self.active_color();
        let li = self.active_layer;
        let rf = (r as f32).max(0.5);
        for dy in -r..=r {
            for dx in -r..=r {
                let d2 = (dx * dx + dy * dy) as f32;
                if d2 > rf * rf {
                    continue;
                }
                let t = (1.0 - d2.sqrt() / rf).clamp(0.0, 1.0);
                self.blend_px(li, x + dx, y + dy, color, t * t * master);
            }
        }
    }

    fn stamp_spray(&mut self, x: i32, y: i32, r: i32) {
        let color = self.active_color();
        let li = self.active_layer;
        let rf = r as f32;
        let n = ((r as f32) * 1.6).max(4.0) as i32;
        for _ in 0..n {
            let a = self.rand_f() * std::f32::consts::TAU;
            let rad = rf * self.rand_f().sqrt();
            let px = x + (rad * a.cos()).round() as i32;
            let py = y + (rad * a.sin()).round() as i32;
            self.blend_px(li, px, py, color, 0.22);
        }
    }

    fn stamp_crayon(&mut self, x: i32, y: i32, r: i32) {
        let color = self.active_color();
        let li = self.active_layer;
        let rf = (r as f32).max(0.5);
        for dy in -r..=r {
            for dx in -r..=r {
                let d2 = (dx * dx + dy * dy) as f32;
                if d2 > rf * rf {
                    continue;
                }
                let g = self.rand_f();
                if g < 0.45 {
                    continue;
                }
                let t = (1.0 - d2.sqrt() / rf).clamp(0.0, 1.0);
                self.blend_px(li, x + dx, y + dy, color, t * 0.9 * (0.55 + 0.45 * g));
            }
        }
    }

    fn stamp_smudge(&mut self, x: i32, y: i32, r: i32) {
        let li = self.active_layer;
        let rf = (r as f32).max(0.5);
        let (mut sr, mut sg, mut sb, mut sa, mut cnt) = (0.0, 0.0, 0.0, 0.0, 0.0);
        if let Some(layer) = self.document.layer(li) {
            for dy in -r..=r {
                for dx in -r..=r {
                    if (dx * dx + dy * dy) as f32 > rf * rf {
                        continue;
                    }
                    let (px, py) = (x + dx, y + dy);
                    if px < 0 || py < 0 {
                        continue;
                    }
                    if let Some(c) = layer.get_pixel(px as u32, py as u32) {
                        sr += c.r as f32;
                        sg += c.g as f32;
                        sb += c.b as f32;
                        sa += c.a as f32;
                        cnt += 1.0;
                    }
                }
            }
        }
        if cnt <= 0.0 {
            return;
        }
        let avg = [sr / cnt, sg / cnt, sb / cnt, sa / cnt];
        let carried = match self.smudge {
            Some(c) => {
                let m = [
                    c[0] * 0.5 + avg[0] * 0.5,
                    c[1] * 0.5 + avg[1] * 0.5,
                    c[2] * 0.5 + avg[2] * 0.5,
                    c[3] * 0.5 + avg[3] * 0.5,
                ];
                self.smudge = Some(m);
                m
            }
            None => {
                self.smudge = Some(avg);
                avg
            }
        };
        let cc = Color::rgba(carried[0] as u8, carried[1] as u8, carried[2] as u8, carried[3] as u8);
        for dy in -r..=r {
            for dx in -r..=r {
                let d2 = (dx * dx + dy * dy) as f32;
                if d2 > rf * rf {
                    continue;
                }
                let t = (1.0 - d2.sqrt() / rf).clamp(0.0, 1.0);
                self.blend_px(li, x + dx, y + dy, cc, t * 0.35);
            }
        }
    }

    /// Carimba um dab conforme o tipo de pincel (ou quadrado no pixel art).
    fn stamp(&mut self, x: i32, y: i32, r: i32) {
        let li = self.active_layer;
        if self.active_bloqueada_para_edicao() {
            return;
        }
        if self.pixel_mode {
            let color = self.active_color();
            let half = (r - 1) / 2;
            if let Some(layer) = self.document.layer_mut(li) {
                for dy in 0..r {
                    for dx in 0..r {
                        let (px, py) = (x + dx - half, y + dy - half);
                        if px >= 0 && py >= 0 {
                            layer.set_pixel(px as u32, py as u32, color);
                        }
                    }
                }
            }
            self.dirty = true;
            return;
        }
        if self.tool == Tool::Eraser {
            self.stamp_hard(x, y, r);
            self.dirty = true;
            return;
        }
        match self.brush_kind {
            1 => self.stamp_soft(x, y, r, 0.55),
            2 => self.stamp_soft(x, y, r, 0.22),
            3 => self.stamp_spray(x, y, r),
            4 => self.stamp_crayon(x, y, r),
            7 => self.stamp_smudge(x, y, r),
            _ => self.stamp_hard(x, y, r),
        }
        self.dirty = true;
    }

    fn paint_dab(&mut self, x: i32, y: i32) {
        let r = self.active_radius();
        self.stamp(x, y, r);
    }

    fn paint_line(&mut self, from: (i32, i32), to: (i32, i32)) {
        let (x0, y0) = from;
        let (x1, y1) = to;
        let (dxf, dyf) = ((x1 - x0) as f32, (y1 - y0) as f32);
        let seg = (dxf * dxf + dyf * dyf).sqrt();
        let base = self.active_radius();
        // Caneta fino-grosso: afina quando o traço é rápido (segmento longo).
        let r_eff = if !self.pixel_mode && self.tool != Tool::Eraser && self.brush_kind == 5 {
            (((base as f32) * (1.0 - (seg / 40.0).min(0.75))).round() as i32).max(1)
        } else {
            base
        };
        // Pontilhado: dabs espaçados.
        let spacing = if !self.pixel_mode && self.tool != Tool::Eraser && self.brush_kind == 6 {
            (base as f32 * 1.8).max(3.0)
        } else {
            1.0
        };
        let steps = (seg / spacing).ceil().max(1.0) as i32;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let x = (x0 as f32 + dxf * t).round() as i32;
            let y = (y0 as f32 + dyf * t).round() as i32;
            self.stamp(x, y, r_eff);
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
        self.document.pixel_art = pixel;
        self.zoom = if pixel {
            (512.0 / (w.max(h) as f32)).floor().max(1.0)
        } else {
            1.0
        };
        self.dirty = true;
        self.center_canvas = true;
        self.track_thumbs.clear();
        self.between_cache.clear();
        self.between_key = None;
        self.onion_tex = None;
        self.onion_for = None;
        self.float_sel = None;
        self.float_tex = None;
        self.split_for = None;
        self.below_tex = None;
        self.above_tex = None;
        self.status = if pixel {
            format!("Novo documento pixel art {w}x{h}")
        } else {
            format!("Novo documento {w}x{h}")
        };
    }

    /// Salva o projeto em segundo plano (indicador "Salvando…" bloqueante).
    fn iniciar_salvar(&mut self, path: std::path::PathBuf) {
        self.document.sync_to_frames();
        let doc = self.document.clone();
        let progress = Arc::new(AtomicU32::new(0));
        let (tx, rx) = mpsc::channel();
        let p2 = path.clone();
        std::thread::spawn(move || {
            let r = sketchmotion_io::save(&doc, &p2).map(|_| format!("Salvo em {}", p2.display()));
            let _ = tx.send(r);
        });
        self.busy_job = Some(BusyJob {
            rx,
            progress,
            started: std::time::Instant::now(),
            titulo: "Salvando…".into(),
            determinate: false,
            set_path: Some(path),
        });
    }

    fn salvar(&mut self) {
        if let Some(path) = self.current_path.clone() {
            self.iniciar_salvar(path);
        } else {
            self.salvar_como();
        }
    }

    /// Salva de forma SÍNCRONA (bloqueante) — usado ao fechar o app. Devolve
    /// true se salvou (ou false se o usuário cancelou o diálogo / deu erro).
    fn salvar_sync(&mut self) -> bool {
        self.document.sync_to_frames();
        let path = match self.current_path.clone() {
            Some(p) => p,
            None => match rfd::FileDialog::new()
                .add_filter("SketchMotion", &[sketchmotion_io::PROJECT_EXTENSION])
                .set_file_name("desenho.sketchmotion")
                .save_file()
            {
                Some(p) => p,
                None => return false,
            },
        };
        match sketchmotion_io::save(&self.document, &path) {
            Ok(()) => {
                self.current_path = Some(path);
                self.modificado = false;
                self.status = "Salvo".into();
                true
            }
            Err(e) => {
                self.status = format!("Erro ao salvar: {e}");
                false
            }
        }
    }

    fn salvar_como(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("SketchMotion", &[sketchmotion_io::PROJECT_EXTENSION])
            .set_file_name("desenho.sketchmotion")
            .save_file()
        {
            self.iniciar_salvar(path);
        }
    }

    fn exportar(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("PNG", &["png"])
            .add_filter("JPEG", &["jpg", "jpeg"])
            .set_file_name("desenho.png")
            .save_file()
        {
            let (w, h) = (self.document.width, self.document.height);
            let img = render_frame_alpha(
                w,
                h,
                &self.document.layers,
                &self.document.vectors,
                &self.document.images,
            );
            let base = if self.export_camera {
                self.aplicar_camera_rgba(&img.rgba, w, h, self.document.current)
            } else {
                img.rgba
            };
            let (ew, eh, ergba) = upscale_nn(w, h, &base, self.export_scale);
            self.status = match sketchmotion_io::export_png(ew, eh, &ergba, &path) {
                Ok(()) => format!("Exportado ({ew}x{eh}): {}", path.display()),
                Err(e) => format!("Erro ao exportar: {e}"),
            };
        }
    }

    /// Abre um projeto: o carregamento (que pode ser lento em arquivos grandes)
    /// roda numa thread e mostra a tela de carregamento (open_job). Devolve true
    /// se iniciou a abertura (o documento é aplicado quando a thread termina).
    fn abrir(&mut self) -> bool {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("SketchMotion", &[sketchmotion_io::PROJECT_EXTENSION])
            .pick_file()
        {
            let p2 = path.clone();
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(sketchmotion_io::load(&p2));
            });
            self.open_job = Some(OpenJob {
                rx,
                started: std::time::Instant::now(),
                path,
            });
            return true;
        }
        false
    }

    /// Aplica um documento recém-aberto ao estado do app.
    fn aplicar_documento_aberto(&mut self, doc: Document, path: std::path::PathBuf) {
        self.document = doc;
        self.active_layer = 0;
        self.last_pos = None;
        self.dirty = true;
        self.track_thumbs.clear();
        self.between_cache.clear();
        self.between_key = None;
        self.onion_tex = None;
        self.onion_for = None;
        self.float_sel = None;
        self.float_tex = None;
        self.split_for = None;
        self.below_tex = None;
        self.above_tex = None;
        self.sel_set.clear();
        self.selected_obj = None;
        self.frame_sel.clear();
        self.pixel_mode = self.document.pixel_art;
        if self.pixel_mode {
            let m = self.document.width.max(self.document.height) as f32;
            self.zoom = (512.0 / m).floor().max(1.0);
        }
        self.center_canvas = true;
        self.current_path = Some(path.clone());
        self.modificado = false;
        self.screen = Screen::Editor;
        self.status = format!("Aberto: {}", path.display());
    }

    /// Abre OUTRO arquivo .sketchmotion como timelines adicionais neste trabalho
    /// (para reaproveitar frames/elementos/referências). NÃO troca o arquivo de
    /// salvamento: quem é salvo continua sendo o primeiro (current_path).
    fn abrir_referencia(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("SketchMotion", &[sketchmotion_io::PROJECT_EXTENSION])
            .pick_file()
        else {
            return;
        };
        let doc2 = match sketchmotion_io::load(&path) {
            Ok(d) => d,
            Err(e) => {
                self.status = format!("Erro ao abrir referência: {e}");
                return;
            }
        };
        // Grava a faixa ativa antes de acrescentar as novas.
        self.document.sync_to_frames();
        let (dw, dh) = (self.document.width, self.document.height);
        let base = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("referência")
            .to_string();
        let varias = doc2.tracks.len() > 1;
        let mut adicionadas = 0usize;
        for t in &doc2.tracks {
            let mut frames_fit: Vec<Frame> = Vec::with_capacity(t.frames.len());
            for f in &t.frames {
                let layers_fit: Vec<Layer> =
                    f.layers.iter().map(|l| fit_layer(l, dw, dh)).collect();
                frames_fit.push(Frame {
                    layers: layers_fit,
                    vectors: f.vectors.clone(),
                    images: f.images.clone(),
                });
            }
            if frames_fit.is_empty() {
                continue;
            }
            let name = if varias {
                format!("{base} · {}", t.name)
            } else {
                base.clone()
            };
            self.document.push_track(name, frames_fit, t.fps);
            adicionadas += 1;
        }
        self.track_thumbs.clear();
        self.onion_for = None;
        self.dirty = true;
        self.status = if doc2.width != dw || doc2.height != dh {
            format!(
                "Referência aberta ({} timeline(s)) — tamanhos diferentes ({}x{}), elementos ajustados ao topo-esquerda",
                adicionadas, doc2.width, doc2.height
            )
        } else {
            format!("Referência aberta: {base} ({adicionadas} timeline(s))")
        };
    }

    fn salvar_biblioteca(&mut self) {
        if let Some(path) = sketchmotion_io::default_library_path() {
            if let Err(e) = sketchmotion_io::save_library(&self.library, &path) {
                self.status = format!("Erro ao salvar paletas: {e}");
            }
        }
    }

    /// Importar imagem via diálogo de arquivo (vira seleção flutuante editável).
    fn importar(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Imagens", &["png", "jpg", "jpeg", "gif", "bmp", "webp"])
            .pick_file()
        {
            match sketchmotion_io::load_image(&path) {
                Ok((w, h, rgba)) => self.colocar_imagem(w, h, rgba),
                Err(e) => self.status = format!("Erro ao importar: {e}"),
            }
        }
    }

    /// Coloca uma imagem RGBA no canvas como seleção flutuante (mover/
    /// redimensionar/girar/opacidade/espelhar pela ferramenta Seleção).
    fn colocar_imagem(&mut self, w: u32, h: u32, rgba: Vec<u8>) {
        if w == 0 || h == 0 || rgba.len() < (w * h * 4) as usize {
            self.status = "Imagem inválida".into();
            return;
        }
        self.push_undo();
        self.drop_float();
        let (dw, dh) = (self.document.width as f32, self.document.height as f32);
        let fit = (dw * 0.8 / w as f32)
            .min(dh * 0.8 / h as f32)
            .min(1.0)
            .max(0.02);
        let (dwid, dhei) = (w as f32 * fit, h as f32 * fit);
        self.float_sel = Some(FloatSel {
            pixels: rgba,
            ow: w,
            oh: h,
            cx: dw / 2.0,
            cy: dh / 2.0,
            hw: dwid / 2.0,
            hh: dhei / 2.0,
            angle: 0.0,
            opacity: 1.0,
            layer: self.active_layer,
            is_image: true,
        });
        self.float_tex = None;
        self.tool = Tool::Select;
        self.selected_obj = None;
        self.dirty = true;
        self.status = "Imagem importada — mova/redimensione e confirme".into();
    }

    /// Espelha horizontalmente os pixels da seleção flutuante.
    fn float_flip_h(&mut self) {
        let li = match &self.float_sel {
            Some(f) => f.layer,
            None => return,
        };
        if self.layer_locked(li) {
            self.warn_locked_layer(li);
            return;
        }
        if let Some(f) = &mut self.float_sel {
            let (w, h) = (f.ow as usize, f.oh as usize);
            let mut np = vec![0u8; f.pixels.len()];
            for y in 0..h {
                for x in 0..w {
                    let sidx = (y * w + x) * 4;
                    let didx = (y * w + (w - 1 - x)) * 4;
                    if sidx + 4 <= f.pixels.len() && didx + 4 <= np.len() {
                        np[didx..didx + 4].copy_from_slice(&f.pixels[sidx..sidx + 4]);
                    }
                }
            }
            f.pixels = np;
        }
        self.float_tex = None;
        self.dirty = true;
    }

    /// Espelha verticalmente os pixels da seleção flutuante.
    fn float_flip_v(&mut self) {
        let li = match &self.float_sel {
            Some(f) => f.layer,
            None => return,
        };
        if self.layer_locked(li) {
            self.warn_locked_layer(li);
            return;
        }
        if let Some(f) = &mut self.float_sel {
            let (w, h) = (f.ow as usize, f.oh as usize);
            let mut np = vec![0u8; f.pixels.len()];
            for y in 0..h {
                for x in 0..w {
                    let sidx = (y * w + x) * 4;
                    let didx = ((h - 1 - y) * w + x) * 4;
                    if sidx + 4 <= f.pixels.len() && didx + 4 <= np.len() {
                        np[didx..didx + 4].copy_from_slice(&f.pixels[sidx..sidx + 4]);
                    }
                }
            }
            f.pixels = np;
        }
        self.float_tex = None;
        self.dirty = true;
    }

    /// Escala a seleção/imagem flutuante por um fator (respeita o bloqueio).
    fn escalar_float(&mut self, fator: f32) {
        let li = match &self.float_sel {
            Some(f) => f.layer,
            None => return,
        };
        if self.layer_locked(li) {
            self.warn_locked_layer(li);
            return;
        }
        if let Some(f) = &mut self.float_sel {
            f.hw = (f.hw * fator).clamp(1.0, 20000.0);
            f.hh = (f.hh * fator).clamp(1.0, 20000.0);
        }
        self.float_tex = None;
        self.dirty = true;
    }

    /// Carimba a flutuante atual na camada e cria uma cópia deslocada para
    /// continuar posicionando (respeita o bloqueio da camada).
    fn duplicar_float(&mut self) {
        let li = match &self.float_sel {
            Some(f) => f.layer,
            None => return,
        };
        if self.layer_locked(li) {
            self.warn_locked_layer(li);
            return;
        }
        if let Some(orig) = self.float_sel.clone() {
            self.drop_float();
            let mut copia = orig;
            copia.cx += 12.0;
            copia.cy += 12.0;
            self.float_sel = Some(copia);
            self.float_tex = None;
            self.dirty = true;
            self.status = "Cópia criada — posicione e confirme".into();
        }
    }

    /// Exporta a animação como GIF (cada frame no FPS do documento).
    /// Inicia uma exportação em segundo plano (barra de progresso + bloqueio).
    fn iniciar_export(&mut self, kind: ExportKind) {
        self.document.sync_to_frames();
        if self.document.frames.is_empty() {
            self.status = "Nada para exportar".into();
            return;
        }
        let path = match kind {
            ExportKind::Gif => rfd::FileDialog::new()
                .add_filter("GIF animado", &["gif"])
                .set_file_name("animacao.gif")
                .save_file(),
            ExportKind::Mp4 => rfd::FileDialog::new()
                .add_filter("MP4 (vídeo)", &["mp4"])
                .set_file_name("animacao.mp4")
                .save_file(),
            ExportKind::Sheet => rfd::FileDialog::new()
                .add_filter("PNG", &["png"])
                .set_file_name("spritesheet.png")
                .save_file(),
            ExportKind::Seq => rfd::FileDialog::new().pick_folder(),
        };
        let Some(path) = path else {
            return;
        };
        let frames = self.document.frames.clone();
        let camera = self.document.camera.clone();
        let (w, h, fps, sc, cols) = (
            self.document.width,
            self.document.height,
            self.document.fps,
            self.export_scale.max(1),
            self.export_cols,
        );
        let export_camera = self.export_camera;
        let progress = Arc::new(AtomicU32::new(0));
        let prog2 = progress.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let r = run_export(
                kind,
                path,
                frames,
                camera,
                export_camera,
                w,
                h,
                fps,
                sc,
                cols,
                &prog2,
            );
            let _ = tx.send(r);
        });
        let titulo = match kind {
            ExportKind::Gif => "Exportando GIF…",
            ExportKind::Mp4 => "Exportando MP4…",
            ExportKind::Seq => "Exportando sequência PNG…",
            ExportKind::Sheet => "Exportando sprite sheet…",
        }
        .to_string();
        self.busy_job = Some(BusyJob {
            rx,
            progress,
            started: std::time::Instant::now(),
            titulo,
            determinate: true,
            set_path: None,
        });
    }

    /// Verifica jobs de exportar/salvar; mostra a tela de progresso (bloqueante).
    /// Devolve true enquanto um job está ativo (o `update` deve parar aí).
    fn busy_poll(&mut self, ctx: &egui::Context) -> bool {
        // Abertura de projeto (tela de carregamento — arquivos grandes demoram).
        if self.open_job.is_some() {
            ctx.request_repaint();
            let elapsed = self.open_job.as_ref().unwrap().started.elapsed().as_secs_f32();
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.4);
                    ui.heading("Abrindo projeto…");
                    ui.add_space(10.0);
                    ui.add(egui::Spinner::new().size(30.0));
                    ui.add_space(6.0);
                    ui.colored_label(
                        egui::Color32::from_gray(150),
                        format!("Carregando… ({:.0}s)", elapsed),
                    );
                });
            });
            match self.open_job.as_ref().unwrap().rx.try_recv() {
                Ok(res) => {
                    let job = self.open_job.take().unwrap();
                    match res {
                        Ok(doc) => self.aplicar_documento_aberto(doc, job.path),
                        Err(e) => self.status = format!("Erro ao abrir: {e}"),
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.open_job = None;
                    self.status = "Falha ao abrir".into();
                }
            }
            return true;
        }
        let (frac, elapsed, titulo, determinate) = {
            let Some(job) = &self.busy_job else {
                return false;
            };
            (
                (job.progress.load(Ordering::Relaxed) as f32 / 1000.0).clamp(0.0, 1.0),
                job.started.elapsed().as_secs_f32(),
                job.titulo.clone(),
                job.determinate,
            )
        };
        ctx.request_repaint();
        let eta = if determinate && frac > 0.03 {
            let total = elapsed / frac;
            let rem = (total - elapsed).max(0.0);
            if rem >= 60.0 {
                format!("~{}min {}s restantes", (rem / 60.0) as u32, (rem % 60.0) as u32)
            } else {
                format!("~{}s restantes", rem.ceil() as u32)
            }
        } else {
            String::new()
        };
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.4);
                ui.heading(&titulo);
                ui.add_space(10.0);
                if determinate {
                    ui.add(
                        egui::ProgressBar::new(frac)
                            .desired_width(360.0)
                            .show_percentage(),
                    );
                    ui.add_space(4.0);
                    ui.colored_label(egui::Color32::from_gray(170), eta);
                } else {
                    ui.add(egui::Spinner::new().size(28.0));
                }
                ui.add_space(6.0);
                ui.colored_label(
                    egui::Color32::from_gray(150),
                    "Aguarde — outras ações estão bloqueadas até terminar.",
                );
            });
        });
        // Verifica conclusão.
        match self.busy_job.as_ref().unwrap().rx.try_recv() {
            Ok(res) => {
                let job = self.busy_job.take().unwrap();
                match res {
                    Ok(msg) => {
                        if let Some(p) = job.set_path {
                            self.current_path = Some(p);
                            self.modificado = false;
                        }
                        self.status = msg;
                    }
                    Err(e) => self.status = format!("Erro: {e}"),
                }
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.busy_job = None;
                self.status = "Processo interrompido".into();
            }
        }
        true
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
        // Do TOPO para baixo: clicar em QUALQUER parte do objeto seleciona —
        // dentro do preenchimento (área) ou perto do traço (contorno).
        for i in (0..self.document.vectors.len()).rev() {
            let obj = &self.document.vectors[i];
            let flat = obj.flatten(20);
            // Dentro da área preenchida (objeto fechado com fill)?
            if obj.fill.is_some() && obj.closed && flat.len() >= 3 && ponto_no_poligono(px, py, &flat)
            {
                return Some(i);
            }
            // Perto do contorno?
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
            if dmin <= tol {
                return Some(i);
            }
        }
        None
    }

    /// Índices dos objetos vetoriais cuja caixa cruza a marca (rubber-band).
    fn vetores_na_marca(&self, a: (i32, i32), b: (i32, i32)) -> Vec<usize> {
        let (x0, x1) = (a.0.min(b.0) as f32, a.0.max(b.0) as f32);
        let (y0, y1) = (a.1.min(b.1) as f32, a.1.max(b.1) as f32);
        if (x1 - x0) < 2.0 && (y1 - y0) < 2.0 {
            return Vec::new(); // foi clique, não marca
        }
        let mut out = Vec::new();
        for (i, obj) in self.document.vectors.iter().enumerate() {
            if let Some((minx, miny, maxx, maxy)) = obj.bounds() {
                if minx <= x1 && maxx >= x0 && miny <= y1 && maxy >= y0 {
                    out.push(i);
                }
            }
        }
        out
    }

    /// Agrupa os objetos do conjunto atual (passam a mover/selecionar juntos).
    fn agrupar_selecao(&mut self) {
        if self.sel_set.len() < 2 {
            self.status = "Selecione 2+ objetos (Shift-clique ou marca) para agrupar".into();
            return;
        }
        self.push_undo();
        let g = self.next_group;
        self.next_group += 1;
        for idx in 0..self.sel_set.len() {
            let i = self.sel_set[idx];
            if i < self.document.vectors.len() {
                self.document.vectors[i].group = Some(g);
            }
        }
        self.dirty = true;
        self.status = format!("{} objetos agrupados", self.sel_set.len());
    }

    /// Desagrupa os objetos do conjunto atual (e seus colegas de grupo).
    fn desagrupar_selecao(&mut self) {
        if self.sel_set.is_empty() {
            self.status = "Nada selecionado para desagrupar".into();
            return;
        }
        self.push_undo();
        let grupos: Vec<u32> = self
            .sel_set
            .iter()
            .filter_map(|&i| self.document.vectors.get(i).and_then(|o| o.group))
            .collect();
        for obj in self.document.vectors.iter_mut() {
            if let Some(g) = obj.group {
                if grupos.contains(&g) {
                    obj.group = None;
                }
            }
        }
        self.dirty = true;
        self.status = "Desagrupado".into();
    }

    /// Índices dos objetos vetoriais atualmente selecionados (conjunto/grupo,
    /// ou o objeto único). Ordenados e sem repetição.
    fn objetos_selecionados(&self) -> Vec<usize> {
        let mut v: Vec<usize> = if !self.sel_set.is_empty() {
            self.sel_set
                .iter()
                .copied()
                .filter(|&i| i < self.document.vectors.len())
                .collect()
        } else if let Some(i) = self.selected_obj {
            if i < self.document.vectors.len() {
                vec![i]
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Copia (ou recorta, se `recortar`) os objetos vetoriais selecionados para
    /// a área de transferência de objetos. Devolve true se havia algo.
    fn copiar_objetos(&mut self, recortar: bool) -> bool {
        let idxs = self.objetos_selecionados();
        if idxs.is_empty() {
            return false;
        }
        let vectors: Vec<VectorObject> =
            idxs.iter().map(|&i| self.document.vectors[i].clone()).collect();
        let n = vectors.len();
        self.obj_clip = ObjClip { vectors, images: Vec::new() };
        self.clip_objetos = true;
        if recortar {
            self.push_undo();
            for &i in idxs.iter().rev() {
                self.document.vectors.remove(i);
            }
            self.sel_set.clear();
            self.selected_obj = None;
            self.dirty = true;
            self.status = format!("{n} objeto(s) recortado(s) — Ctrl+V para colar");
        } else {
            self.status = format!("{n} objeto(s) copiado(s) — Ctrl+V para colar");
        }
        true
    }

    /// Cola os objetos da área de transferência mantendo posição e tamanho.
    /// Vetores entram no frame atual; imagens vão para a camada ativa (mesma
    /// posição/escala/rotação). Devolve true se colou algo.
    fn colar_objetos(&mut self) -> bool {
        if self.obj_clip.vectors.is_empty() && self.obj_clip.images.is_empty() {
            return false;
        }
        self.push_undo();
        // Vetores: remapeia ids de grupo para novos, preservando o agrupamento
        // relativo sem colidir com grupos já existentes no documento.
        let start = self.document.vectors.len();
        let mut map: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        let clip_vecs = self.obj_clip.vectors.clone();
        for obj in clip_vecs {
            let mut no = obj;
            if let Some(g) = no.group {
                let ng = *map.entry(g).or_insert_with(|| {
                    let v = self.next_group;
                    self.next_group += 1;
                    v
                });
                no.group = Some(ng);
            }
            self.document.vectors.push(no);
        }
        // Imagens: mesma posição/tamanho, associadas à camada ativa.
        let clip_imgs = self.obj_clip.images.clone();
        for mut im in clip_imgs {
            im.layer = self.active_layer;
            self.document.images.push(im);
        }
        // Seleciona os vetores recém-colados (para mover/ajustar em seguida).
        if self.document.vectors.len() > start {
            self.sel_set = (start..self.document.vectors.len()).collect();
            self.selected_obj = self.sel_set.first().copied();
        }
        self.tool = Tool::Select;
        self.dirty = true;
        let nv = self.document.vectors.len() - start;
        let ni = self.obj_clip.images.len();
        self.status = format!("Colado na mesma posição ({nv} vetor(es), {ni} imagem(ns))");
        true
    }

    /// Contorno azul ao redor de cada objeto do conjunto (seleção múltipla).
    fn desenhar_sel_set(&self, ui: &mut egui::Ui, rect: egui::Rect, zoom: f32) {
        if self.sel_set.len() < 2 {
            return;
        }
        let painter = ui.painter_at(rect);
        let blue = egui::Color32::from_rgb(0x2F, 0x84, 0xFE);
        for &i in &self.sel_set {
            if let Some(obj) = self.document.vectors.get(i) {
                if let Some((minx, miny, maxx, maxy)) = obj.bounds() {
                    let r = egui::Rect::from_min_max(
                        egui::pos2(rect.min.x + minx * zoom, rect.min.y + miny * zoom),
                        egui::pos2(rect.min.x + maxx * zoom, rect.min.y + maxy * zoom),
                    );
                    painter.rect_stroke(r, 0.0, egui::Stroke::new(1.0_f32, blue));
                }
            }
        }
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
        if self.active_locked() {
            self.warn_lock();
            return;
        }
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
            layer: li,
            is_image: false,
        });
        self.float_tex = None;
        self.dirty = true;
        self.status = "Seleção recortada — arraste para mover".into();
    }

    /// Carimba a seleção flutuante de volta na camada ativa (alpha over).
    fn commit_float(&mut self) {
        // Confirma na camada à qual a flutuante pertence (respeita o bloqueio dela).
        let li0 = match &self.float_sel {
            Some(f) => f.layer,
            None => return,
        };
        if self.layer_locked(li0) {
            self.warn_locked_layer(li0);
            return;
        }
        if let Some(fs) = self.float_sel.take() {
            let li = fs.layer.min(self.document.layers.len().saturating_sub(1));
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

    /// Solta a flutuante conforme o Caminho B: se for OBJETO DE IMAGEM, guarda
    /// como `ImageObject` persistente (salvo no arquivo, exportado na resolução
    /// do trabalho) em vez de integrá-la aos pixels; caso contrário (seleção
    /// raster comum), integra como sempre.
    fn drop_float(&mut self) {
        let is_img = matches!(&self.float_sel, Some(f) if f.is_image);
        if !is_img {
            self.commit_float();
            return;
        }
        if let Some(fs) = self.float_sel.take() {
            self.document.images.push(ImageObject::new(
                fs.pixels, fs.ow, fs.oh, fs.cx, fs.cy, fs.hw, fs.hh, fs.angle, fs.opacity, fs.layer,
            ));
            // Grava no frame atual para persistir (salvar/exportar) já refletir.
            self.document.sync_to_frames();
            self.float_tex = None;
            self.dirty = true;
            self.status = "Imagem posicionada (objeto editável) — use Integrar para rasterizar".into();
        }
    }

    /// Tenta pegar um OBJETO DE IMAGEM sob o ponto (espaço do documento) para
    /// editá-lo (mover/escalar/girar). Remove-o da lista e o transforma em
    /// flutuante. Devolve true se pegou algum.
    fn pick_image_at(&mut self, dp: (f32, f32)) -> bool {
        let mut hit: Option<usize> = None;
        for i in (0..self.document.images.len()).rev() {
            let o = &self.document.images[i];
            if o.hw <= 0.0 || o.hh <= 0.0 {
                continue;
            }
            // Ponto local (desfaz a rotação em torno do centro).
            let (s, c) = (-o.angle).sin_cos();
            let (rx, ry) = (dp.0 - o.cx, dp.1 - o.cy);
            let lx = rx * c - ry * s;
            let ly = rx * s + ry * c;
            if lx.abs() <= o.hw && ly.abs() <= o.hh {
                hit = Some(i);
                break;
            }
        }
        let Some(i) = hit else {
            return false;
        };
        // Objeto em camada bloqueada não pode ser pego. Só notifica se essa
        // camada for a ativa (você está nela); em outra camada, recusa em
        // silêncio e o clique passa adiante (você continua podendo desenhar).
        if self.layer_locked(self.document.images[i].layer) {
            self.warn_locked_layer(self.document.images[i].layer);
            return false;
        }
        let o = self.document.images.remove(i);
        let layer = o.layer.min(self.document.layers.len().saturating_sub(1));
        self.float_sel = Some(FloatSel {
            pixels: o.pixels,
            ow: o.ow,
            oh: o.oh,
            cx: o.cx,
            cy: o.cy,
            hw: o.hw,
            hh: o.hh,
            angle: o.angle,
            opacity: o.opacity,
            layer,
            is_image: true,
        });
        self.float_tex = None;
        self.selected_obj = None;
        self.dirty = true;
        self.status = "Imagem selecionada — mova/gire/redimensione; Integrar para rasterizar".into();
        true
    }

    /// Detecção do clique sobre a seleção flutuante (rotação / alça / mover).
    /// Devolve true se o clique foi consumido por ela.
    fn float_press(&mut self, p: egui::Pos2, rect: egui::Rect, zoom: f32) -> bool {
        // A imagem/seleção flutuante respeita o bloqueio da SUA camada: se ela
        // estiver bloqueada, nem pode ser selecionada/movida (mesmo estando
        // ativa outra camada). Só avisa.
        let flayer = self.float_sel.as_ref().map(|f| f.layer);
        if let Some(li) = flayer {
            if self.layer_locked(li) {
                self.warn_locked_layer(li);
                return true;
            }
        }
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

    /// Cursor apropriado quando o ponteiro está sobre a seleção flutuante
    /// (alça de rotação, quadradinhos de redimensionar ou corpo p/ mover).
    /// Devolve None se o ponteiro não estiver sobre o gizmo.
    fn float_cursor(&self, p: egui::Pos2, rect: egui::Rect, zoom: f32) -> Option<egui::CursorIcon> {
        use egui::CursorIcon as CI;
        // Interação em andamento vence a detecção por hover.
        if self.float_rotating {
            return Some(CI::Grabbing);
        }
        if let Some((sx, sy)) = self.float_resize {
            return Some(resize_cursor(sx, sy));
        }
        if self.float_dragging {
            return Some(CI::Grabbing);
        }
        let fs = self.float_sel.as_ref()?;
        let (cx, cy, hw, hh, ang) = (fs.cx, fs.cy, fs.hw, fs.hh, fs.angle);
        let dp = ((p.x - rect.min.x) / zoom, (p.y - rect.min.y) / zoom);
        let scr = |wx: f32, wy: f32| egui::pos2(rect.min.x + wx * zoom, rect.min.y + wy * zoom);
        // Alça de rotação.
        let (tmx, tmy) = float_corner(cx, cy, hw, hh, ang, 0.0, -1.0);
        let tm = scr(tmx, tmy);
        let cc = scr(cx, cy);
        let dir = (tm - cc).normalized();
        let roth = tm + dir * 22.0;
        if roth.distance(p) <= 12.0 {
            return Some(CI::Grab);
        }
        // Quadradinhos de redimensionar.
        for &(sx, sy) in HSIGNS.iter() {
            let (hx, hy) = float_corner(cx, cy, hw, hh, ang, sx, sy);
            if scr(hx, hy).distance(p) <= 8.0 {
                return Some(resize_cursor(sx, sy));
            }
        }
        // Corpo da seleção.
        let (lx, ly) = float_local(cx, cy, ang, dp.0, dp.1);
        if lx.abs() <= hw && ly.abs() <= hh {
            return Some(CI::Grab);
        }
        None
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
        if self.active_locked() {
            self.warn_lock();
            return;
        }
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
            layer: li,
            is_image: false,
        });
        self.float_tex = None;
        self.dirty = true;
        self.status = "Seleção livre recortada — arraste para mover".into();
    }

    /// Captura (SEM apagar) o conteúdo visível dentro do contorno para virar uma
    /// peça reutilizável, e abre a confirmação de "agrupar".
    fn capturar_grupo(&mut self, pts: &[(f32, f32)]) {
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
        // Composição atual (todas as camadas + vetores) = o que está visível.
        let comp = render_frame_alpha(
            self.document.width,
            self.document.height,
            &self.document.layers,
            &self.document.vectors,
            &self.document.images,
        );
        let cw = self.document.width as usize;
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        for yy in 0..h {
            for xx in 0..w {
                let wx = x0 as f32 + xx as f32 + 0.5;
                let wy = y0 as f32 + yy as f32 + 0.5;
                if ponto_no_poligono(wx, wy, pts) {
                    let sx = x0 as usize + xx as usize;
                    let sy = y0 as usize + yy as usize;
                    let si = (sy * cw + sx) * 4;
                    let di = ((yy * w + xx) * 4) as usize;
                    if si + 4 <= comp.rgba.len() {
                        pixels[di..di + 4].copy_from_slice(&comp.rgba[si..si + 4]);
                    }
                }
            }
        }
        let n = self.pieces.pieces.len() + 1;
        self.pending_group = Some((w, h, pixels));
        self.pending_name = format!("Objeto {n}");
        self.win_pieces = true;
        self.status = "Agrupar como objeto? Confirme na janela Objetos".into();
    }

    /// Adiciona a peça selecionada como seleção flutuante (colar), centrada em
    /// (dx, dy), entrando em modo de seleção para mover antes de confirmar.
    fn colar_peca(&mut self, dx: f32, dy: f32) {
        let Some(i) = self.selected_piece else {
            return;
        };
        let (ow, oh, pixels) = match self.pieces.pieces.get(i) {
            Some(p) if p.w > 0 && p.h > 0 => (p.w, p.h, p.rgba.clone()),
            _ => return,
        };
        self.drop_float();
        self.push_undo();
        self.float_sel = Some(FloatSel {
            pixels,
            ow,
            oh,
            cx: dx,
            cy: dy,
            hw: ow as f32 / 2.0,
            hh: oh as f32 / 2.0,
            angle: 0.0,
            opacity: 1.0,
            layer: self.active_layer,
            is_image: false,
        });
        self.float_tex = None;
        self.tool = Tool::Select;
        self.selected_obj = None;
        // Encerra a suspensão: cursor volta ao normal (agora é o modo Seleção).
        self.place_piece = false;
        self.dirty = true;
        self.status = "Peça adicionada — mova e confirme".into();
    }

    /// Salva a biblioteca de objetos no arquivo global.
    fn salvar_pecas(&mut self) {
        if let Some(path) = sketchmotion_io::default_pieces_path() {
            if let Err(e) = sketchmotion_io::save_pieces(&self.pieces, &path) {
                self.status = format!("Erro ao salvar objetos: {e}");
            }
        }
    }

    /// Janela "Objetos": confirmação de agrupar + lista de peças salvas
    /// (miniatura, renomear, colar, excluir).
    fn janela_objetos(&mut self, ctx: &egui::Context) {
        // Confirmação de agrupar (após o laço capturar a seleção).
        if self.pending_group.is_some() {
            let mut salvar = false;
            let mut cancelar = false;
            egui::Window::new("Agrupar objeto")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.label("Salvar essa seleção como um objeto reutilizável?");
                    if let Some((w, h, rgba)) = &self.pending_group {
                        let full = PixelImage {
                            width: *w as i32,
                            height: *h as i32,
                            rgba: rgba.clone(),
                        };
                        let img = thumb_image(&full, 80, 80);
                        let tex = ui.ctx().load_texture(
                            "pending_thumb",
                            img,
                            egui::TextureOptions::NEAREST,
                        );
                        ui.add(egui::Image::from_texture(egui::load::SizedTexture::new(
                            tex.id(),
                            egui::vec2(80.0, 80.0),
                        )));
                    }
                    ui.horizontal(|ui| {
                        ui.label("Nome:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.pending_name)
                                .desired_width(170.0),
                        );
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Salvar").clicked() {
                            salvar = true;
                        }
                        if ui.button("Cancelar").clicked() {
                            cancelar = true;
                        }
                    });
                });
            if salvar {
                if let Some((w, h, rgba)) = self.pending_group.take() {
                    let name = if self.pending_name.trim().is_empty() {
                        "Objeto".to_string()
                    } else {
                        self.pending_name.trim().to_string()
                    };
                    let idx = self.pieces.add(name, w, h, rgba);
                    self.selected_piece = Some(idx);
                    self.pieces_dirty = true;
                    self.win_pieces = true;
                    self.status = "Objeto salvo".into();
                }
            }
            if cancelar {
                self.pending_group = None;
            }
        }

        let mut open = self.win_pieces;
        let mut usar: Option<usize> = None;
        let mut del: Option<usize> = None;
        let mut abrir_edit: Option<usize> = None;
        egui::Window::new("Objetos")
            .open(&mut open)
            .default_width(290.0)
            .show(ctx, |ui| {
                if ui
                    .button("＋ Novo objeto (laço)")
                    .on_hover_text("Desenhe um laço em volta do que quer agrupar")
                    .clicked()
                {
                    self.place_piece = false;
                    self.selected_piece = None;
                    self.tool = Tool::Grupo;
                    self.status = "Desenhe um laço em volta do objeto para agrupar".into();
                }
                ui.separator();
                if self.pieces.pieces.is_empty() {
                    ui.label("Nenhum objeto salvo ainda.");
                }
                egui::ScrollArea::vertical().max_height(340.0).show(ui, |ui| {
                    for i in 0..self.pieces.pieces.len() {
                        let armed = self.place_piece && self.selected_piece == Some(i);
                        let (w, h) = (self.pieces.pieces[i].w, self.pieces.pieces[i].h);
                        let full = PixelImage {
                            width: w as i32,
                            height: h as i32,
                            rgba: self.pieces.pieces[i].rgba.clone(),
                        };
                        let img = thumb_image(&full, 46, 46);
                        let tex = ui.ctx().load_texture(
                            format!("obj_thumb_{i}"),
                            img,
                            egui::TextureOptions::NEAREST,
                        );
                        ui.group(|ui| {
                                ui.horizontal(|ui| {
                                    ui.add(egui::Image::from_texture(
                                        egui::load::SizedTexture::new(
                                            tex.id(),
                                            egui::vec2(46.0, 46.0),
                                        ),
                                    ));
                                    ui.vertical(|ui| {
                                        if armed {
                                            ui.colored_label(
                                                egui::Color32::from_rgb(0x2F, 0x84, 0xFE),
                                                "● pronto para colar",
                                            );
                                        }
                                        if ui
                                            .add(
                                                egui::TextEdit::singleline(
                                                    &mut self.pieces.pieces[i].name,
                                                )
                                                .desired_width(160.0),
                                            )
                                            .changed()
                                        {
                                            self.pieces_dirty = true;
                                        }
                                        ui.horizontal(|ui| {
                                            if ui
                                                .button("Colar")
                                                .on_hover_text("Clique no canvas para adicionar")
                                                .clicked()
                                            {
                                                usar = Some(i);
                                            }
                                            if ui
                                                .button("Editar")
                                                .on_hover_text("Abrir a peça para limpar/retocar")
                                                .clicked()
                                            {
                                                abrir_edit = Some(i);
                                            }
                                            if ui
                                                .button(egui_phosphor::regular::TRASH)
                                                .on_hover_text("Excluir objeto")
                                                .clicked()
                                            {
                                                del = Some(i);
                                            }
                                        });
                                    });
                                });
                            });
                        ui.add_space(4.0);
                    }
                });
                if self.place_piece {
                    if let Some(i) = self.selected_piece {
                        if let Some(p) = self.pieces.pieces.get(i) {
                            ui.separator();
                            ui.colored_label(
                                egui::Color32::from_rgb(0x2F, 0x84, 0xFE),
                                format!("\"{}\" pronto — clique no canvas para colar", p.name),
                            );
                        }
                    }
                }
            });
        if let Some(i) = usar {
            self.selected_piece = Some(i);
            self.place_piece = true;
            self.tool = Tool::Grupo;
            self.status = "Clique no canvas para colar a peça".into();
        }
        if let Some(i) = abrir_edit {
            if let Some(p) = self.pieces.pieces.get(i) {
                self.edit_w = p.w;
                self.edit_h = p.h;
                self.edit_buf = p.rgba.clone();
                self.win_edit_piece = Some(i);
                self.edit_tex = None;
                self.edit_dirty = true;
                let m = p.w.max(p.h).max(1) as f32;
                self.edit_zoom = (360.0 / m).floor().clamp(1.0, 24.0);
            }
        }
        if let Some(i) = del {
            self.pieces.remove(i);
            self.pieces_dirty = true;
            match self.selected_piece {
                Some(s) if s == i => {
                    self.selected_piece = None;
                    self.place_piece = false;
                }
                Some(s) if s > i => self.selected_piece = Some(s - 1),
                _ => {}
            }
        }
        self.win_pieces = open;
    }

    /// Pinta/apaga um quadrado no buffer de edição da peça (coords em pixel).
    fn pintar_edit(&mut self, px: i32, py: i32) {
        let (w, h) = (self.edit_w as i32, self.edit_h as i32);
        if w <= 0 || h <= 0 {
            return;
        }
        let s = self.edit_size.max(1);
        let half = (s - 1) / 2;
        let (r, g, b, a) = if self.edit_erase {
            (0u8, 0u8, 0u8, 0u8)
        } else {
            (
                self.brush_color.r(),
                self.brush_color.g(),
                self.brush_color.b(),
                self.brush_color.a(),
            )
        };
        for dy in 0..s {
            for dx in 0..s {
                let x = px + dx - half;
                let y = py + dy - half;
                if x >= 0 && y >= 0 && x < w && y < h {
                    let idx = ((y * w + x) * 4) as usize;
                    if idx + 4 <= self.edit_buf.len() {
                        self.edit_buf[idx] = r;
                        self.edit_buf[idx + 1] = g;
                        self.edit_buf[idx + 2] = b;
                        self.edit_buf[idx + 3] = a;
                    }
                }
            }
        }
        self.edit_dirty = true;
    }

    /// Tela de edição isolada da peça: um "novo canvas" com o objeto desenhado,
    /// com pincel/borracha para limpar o entorno, e a opção de salvar.
    fn janela_editar_peca(&mut self, ctx: &egui::Context) {
        let Some(pi) = self.win_edit_piece else {
            return;
        };
        if pi >= self.pieces.pieces.len() || self.edit_w == 0 || self.edit_h == 0 {
            self.win_edit_piece = None;
            return;
        }
        let (w, h) = (self.edit_w, self.edit_h);
        // (Re)constrói a textura de pré-visualização quando o buffer muda.
        if self.edit_dirty || self.edit_tex.is_none() {
            let ci = egui::ColorImage::from_rgba_unmultiplied(
                [w as usize, h as usize],
                &self.edit_buf,
            );
            match &mut self.edit_tex {
                Some(t) => t.set(ci, egui::TextureOptions::NEAREST),
                None => {
                    self.edit_tex =
                        Some(ctx.load_texture("edit_piece", ci, egui::TextureOptions::NEAREST))
                }
            }
            self.edit_dirty = false;
        }
        let mut salvar = false;
        let mut cancelar = false;
        let mut open = true;
        egui::Window::new("Editar objeto")
            .open(&mut open)
            .default_size(egui::vec2(560.0, 540.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.edit_erase, false, "Pincel");
                    ui.selectable_value(&mut self.edit_erase, true, "Borracha");
                    ui.separator();
                    ui.label("Tamanho:");
                    ui.add(egui::Slider::new(&mut self.edit_size, 1..=40));
                    ui.separator();
                    let (rc, resp) =
                        ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::click());
                    ui.painter().rect_filled(rc, 3.0, self.brush_color);
                    ui.painter().rect_stroke(
                        rc,
                        3.0,
                        egui::Stroke::new(1.0_f32, egui::Color32::from_gray(120)),
                    );
                    if resp
                        .on_hover_text("Cor atual do pincel (clique para escolher)")
                        .clicked()
                    {
                        self.win_color = true;
                        self.reopen_color = true;
                    }
                    ui.separator();
                    if ui.button("－").on_hover_text("Menos zoom").clicked() {
                        self.edit_zoom = (self.edit_zoom / 1.25).max(1.0);
                    }
                    if ui.button("＋").on_hover_text("Mais zoom").clicked() {
                        self.edit_zoom = (self.edit_zoom * 1.25).min(32.0);
                    }
                });
                ui.separator();
                let zoom = self.edit_zoom.max(1.0);
                let size = egui::vec2(w as f32 * zoom, h as f32 * zoom);
                egui::ScrollArea::both().max_height(400.0).show(ui, |ui| {
                    let (rect, _response) =
                        ui.allocate_exact_size(size, egui::Sense::click_and_drag());
                    let p = ui.painter_at(rect);
                    // Fundo xadrez (transparência).
                    let cell = 8.0_f32.max(zoom);
                    p.rect_filled(rect, 0.0, egui::Color32::from_gray(210));
                    let nx = (rect.width() / cell).ceil() as i32;
                    let ny = (rect.height() / cell).ceil() as i32;
                    for j in 0..ny {
                        for i in 0..nx {
                            if (i + j) % 2 == 0 {
                                continue;
                            }
                            let x = rect.left() + i as f32 * cell;
                            let y = rect.top() + j as f32 * cell;
                            let cr = egui::Rect::from_min_size(
                                egui::pos2(x, y),
                                egui::vec2(cell, cell),
                            )
                            .intersect(rect);
                            p.rect_filled(cr, 0.0, egui::Color32::from_gray(165));
                        }
                    }
                    if let Some(t) = &self.edit_tex {
                        p.image(
                            t.id(),
                            rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                    let down = ui.input(|i| i.pointer.primary_down());
                    let pos = ui.input(|i| i.pointer.latest_pos());
                    if down {
                        if let Some(pp) = pos {
                            if rect.contains(pp) {
                                let px = ((pp.x - rect.min.x) / zoom).floor() as i32;
                                let py = ((pp.y - rect.min.y) / zoom).floor() as i32;
                                self.pintar_edit(px, py);
                            }
                        }
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Salvar alterações").clicked() {
                        salvar = true;
                    }
                    if ui.button("Cancelar").clicked() {
                        cancelar = true;
                    }
                });
                ui.label(
                    "Apague o entorno com a Borracha (fica transparente) e salve a peça limpa.",
                );
            });
        if salvar {
            if pi < self.pieces.pieces.len() {
                self.pieces.pieces[pi].rgba = self.edit_buf.clone();
                self.pieces.pieces[pi].w = w;
                self.pieces.pieces[pi].h = h;
                self.pieces_dirty = true;
                self.status = "Objeto atualizado".into();
            }
            self.win_edit_piece = None;
            self.edit_tex = None;
        }
        if cancelar || !open {
            self.win_edit_piece = None;
            self.edit_tex = None;
        }
    }

    /// Varinha mágica: seleciona a região de cor semelhante (contígua ou toda a
    /// camada) e recorta para uma seleção flutuante.
    fn lift_wand(&mut self, x0: i32, y0: i32) {
        let w = self.document.width as i32;
        let h = self.document.height as i32;
        if x0 < 0 || y0 < 0 || x0 >= w || y0 >= h {
            return;
        }
        if self.active_bloqueada_para_edicao() {
            return;
        }
        let li = self.active_layer;
        let target = match self
            .document
            .layer(li)
            .and_then(|l| l.get_pixel(x0 as u32, y0 as u32))
        {
            Some(c) => c,
            None => return,
        };
        let tol = self.wand_tolerance;
        let close = |c: Color| {
            (c.r as i32 - target.r as i32)
                .abs()
                .max((c.g as i32 - target.g as i32).abs())
                .max((c.b as i32 - target.b as i32).abs())
                .max((c.a as i32 - target.a as i32).abs())
                <= tol
        };
        let mut mask = vec![false; (w * h) as usize];
        if self.wand_contiguo {
            let mut stack = vec![(x0, y0)];
            while let Some((x, y)) = stack.pop() {
                if x < 0 || y < 0 || x >= w || y >= h {
                    continue;
                }
                let idx = (y * w + x) as usize;
                if mask[idx] {
                    continue;
                }
                match self.document.layer(li).and_then(|l| l.get_pixel(x as u32, y as u32)) {
                    Some(c) if close(c) => {}
                    _ => continue,
                }
                mask[idx] = true;
                stack.push((x + 1, y));
                stack.push((x - 1, y));
                stack.push((x, y + 1));
                stack.push((x, y - 1));
            }
        } else if let Some(layer) = self.document.layer(li) {
            for y in 0..h {
                for x in 0..w {
                    if let Some(c) = layer.get_pixel(x as u32, y as u32) {
                        if close(c) {
                            mask[(y * w + x) as usize] = true;
                        }
                    }
                }
            }
        }
        let (mut minx, mut miny, mut maxx, mut maxy) = (w, h, -1, -1);
        for y in 0..h {
            for x in 0..w {
                if mask[(y * w + x) as usize] {
                    minx = minx.min(x);
                    miny = miny.min(y);
                    maxx = maxx.max(x);
                    maxy = maxy.max(y);
                }
            }
        }
        if maxx < minx {
            return;
        }
        let (bw, bh) = ((maxx - minx + 1) as u32, (maxy - miny + 1) as u32);
        self.push_undo();
        let mut pixels = vec![0u8; (bw * bh * 4) as usize];
        if let Some(layer) = self.document.layer_mut(li) {
            for yy in 0..bh {
                for xx in 0..bw {
                    let (gx, gy) = (minx + xx as i32, miny + yy as i32);
                    if mask[(gy * w + gx) as usize] {
                        if let Some(c) = layer.get_pixel(gx as u32, gy as u32) {
                            let di = ((yy * bw + xx) * 4) as usize;
                            pixels[di] = c.r;
                            pixels[di + 1] = c.g;
                            pixels[di + 2] = c.b;
                            pixels[di + 3] = c.a;
                            layer.set_pixel(gx as u32, gy as u32, Color::TRANSPARENT);
                        }
                    }
                }
            }
        }
        self.float_sel = Some(FloatSel {
            pixels,
            ow: bw,
            oh: bh,
            cx: minx as f32 + bw as f32 / 2.0,
            cy: miny as f32 + bh as f32 / 2.0,
            hw: bw as f32 / 2.0,
            hh: bh as f32 / 2.0,
            angle: 0.0,
            opacity: 1.0,
            layer: li,
            is_image: false,
        });
        self.float_tex = None;
        self.dirty = true;
        self.status = "Seleção por cor — arraste para mover".into();
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
            && matches!(t, Tool::Pen | Tool::Shapes | Tool::Text | Tool::DirectSelect | Tool::Rig)
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
        if self.active_bloqueada_para_edicao() {
            return;
        }
        let li = self.active_layer;
        let PixelImage { width, height, mut rgba } =
            render_layers_alpha(self.document.width, self.document.height, &self.document.layers);
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
        let both = self.play_both && self.document.tracks.len() > 1;
        let n = if both {
            self.document
                .tracks
                .iter()
                .map(|t| t.frames.len())
                .max()
                .unwrap_or(1)
                .max(1)
        } else {
            self.document.frame_count()
        };
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
        let dw = self.document.width;
        let dh = self.document.height;
        // Composição do frame (todas as camadas/vetores/imagens).
        let base_rgba = if both {
            self.compose_tracks_at(pf)
        } else {
            render_frame_alpha(
                dw,
                dh,
                &self.document.frames[pf].layers,
                &self.document.frames[pf].vectors,
                &self.document.frames[pf].images,
            )
            .rgba
        };
        // Enquadramento da câmera (keyframes animados): aplica na reprodução.
        let final_rgba = if self.play_camera {
            self.aplicar_camera_rgba(&base_rgba, dw, dh, pf)
        } else {
            base_rgba
        };
        let ci = egui::ColorImage::from_rgba_unmultiplied(
            [dw as usize, dh as usize],
            &final_rgba,
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
                    ui.checkbox(&mut self.play_camera, "Ver pela câmera")
                        .on_hover_text("Mostra a animação já enquadrada pela câmera (keyframes).");
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

    /// Garante o cache de miniaturas (`track_thumbs[faixa][frame]`), refazendo
    /// apenas o que mudou. Sem edição em andamento nada é re-renderizado.
    fn ensure_track_thumbs(&mut self, ctx: &egui::Context, th_w: usize, th_h: usize) {
        let (dw, dh) = (self.document.width, self.document.height);
        let ntr = self.document.tracks.len();
        let active = self.document.active_track;
        if self.track_thumbs.len() != ntr {
            self.track_thumbs = vec![Vec::new(); ntr];
        }
        for ti in 0..ntr {
            let total = if ti == active {
                self.document.frames.len()
            } else {
                self.document.tracks[ti].frames.len()
            };
            if self.track_thumbs[ti].len() != total {
                self.track_thumbs[ti] = vec![None; total];
            }
            for fi in 0..total {
                // O frame atual da faixa ativa é refeito só quando há edição.
                if ti == active && fi == self.document.current && self.dirty {
                    self.track_thumbs[ti][fi] = None;
                }
                if self.track_thumbs[ti][fi].is_some() {
                    continue;
                }
                let full = if ti == active {
                    if fi == self.document.current {
                        render_frame_alpha(
                            dw,
                            dh,
                            &self.document.layers,
                            &self.document.vectors,
                            &self.document.images,
                        )
                    } else {
                        render_frame_alpha(
                            dw,
                            dh,
                            &self.document.frames[fi].layers,
                            &self.document.frames[fi].vectors,
                            &self.document.frames[fi].images,
                        )
                    }
                } else {
                    render_frame_alpha(
                        dw,
                        dh,
                        &self.document.tracks[ti].frames[fi].layers,
                        &self.document.tracks[ti].frames[fi].vectors,
                        &self.document.tracks[ti].frames[fi].images,
                    )
                };
                let img = thumb_image(&full, th_w, th_h);
                self.track_thumbs[ti][fi] = Some(ctx.load_texture(
                    format!("thumb_{ti}_{fi}"),
                    img,
                    egui::TextureOptions::NEAREST,
                ));
            }
        }
    }

    /// Timeline (rodapé): cada faixa de animação é uma "lane" completa, com
    /// seu botão de selecionar, excluir, controles de frame e tira de frames.
    fn barra_frames(&mut self, ctx: &egui::Context) {
        // Timeline recolhida: mostra só uma barra fina para reabrir.
        if self.timeline_hidden {
            egui::TopBottomPanel::bottom("timeline_bar")
                .resizable(false)
                .exact_height(26.0)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .button("▲ Mostrar timeline")
                            .on_hover_text("Reabrir a área de timeline")
                            .clicked()
                        {
                            self.timeline_hidden = false;
                        }
                        ui.separator();
                        if ui
                            .button("Centralizar canvas")
                            .on_hover_text("Recentralizar o canvas na área de trabalho")
                            .clicked()
                        {
                            self.center_canvas = true;
                        }
                    });
                });
            return;
        }
        let (dw, dh) = (self.document.width, self.document.height);
        let th_h = 44usize;
        let th_w = (((th_h as f32) * dw as f32 / dh as f32).round() as usize).clamp(20, 140);

        // Ações diferidas (aplicadas depois de fechar os painéis).
        let mut track_add = false;
        let mut sel_track: Option<usize> = None;
        let mut del_track: Option<usize> = None;
        let mut do_play: Option<usize> = None;
        let mut do_add: Option<usize> = None;
        let mut do_dup: Option<usize> = None;
        let mut do_delframe: Option<usize> = None;
        let mut do_prev: Option<usize> = None;
        let mut do_next: Option<usize> = None;
        let mut goto: Option<(usize, usize)> = None;
        let mut open_ref = false;
        let mut do_copy: Option<usize> = None;
        let mut do_paste: Option<usize> = None;
        // Arrasto de frames: início (faixa, frame) e aplicação (faixa, de, para).
        let mut drag_start: Option<(usize, usize)> = None;
        let mut drag_apply: Option<(usize, usize, usize)> = None;
        // Trilha da câmera (deferidos).
        let mut cam_goto: Option<usize> = None;
        let mut cam_kf_toggle: Option<usize> = None;
        let mut cam_kf_move: Option<(usize, usize)> = None;
        let mut cam_reset = false;

        let ntr = self.document.tracks.len();
        let active = self.document.active_track;

        // Prepara o cache de miniaturas ANTES do painel: durante o desenho
        // (self.dirty) só a miniatura do frame atual da faixa ativa é refeita.
        self.ensure_track_thumbs(ctx, th_w, th_h);

        let mut new_timeline_h: Option<f32> = None;
        egui::TopBottomPanel::bottom("timeline")
            .resizable(false)
            .exact_height(self.timeline_h)
            .show(ctx, |ui| {
                // Alça de redimensionar (fica PARADA; só move ao arrastar).
                let (hrect, hresp) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 6.0),
                    egui::Sense::drag(),
                );
                let hcolor = if hresp.hovered() || hresp.dragged() {
                    egui::Color32::from_rgb(0x2F, 0x84, 0xFE)
                } else {
                    egui::Color32::from_gray(90)
                };
                ui.painter().rect_filled(hrect, 2.0, hcolor);
                if hresp.hovered() || hresp.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                }
                if hresp.dragged() {
                    let nh = (self.timeline_h - hresp.drag_delta().y).clamp(90.0, 640.0);
                    new_timeline_h = Some(nh);
                }
                ui.add_space(2.0);
                // Barra superior: adicionar timeline + opções globais.
                ui.horizontal(|ui| {
                    if ui
                        .button("▼ Ocultar")
                        .on_hover_text("Recolher a área de timeline (deixa só uma barra)")
                        .clicked()
                    {
                        self.timeline_hidden = true;
                    }
                    if ui
                        .button("Centralizar")
                        .on_hover_text("Recentralizar o canvas na área de trabalho")
                        .clicked()
                    {
                        self.center_canvas = true;
                    }
                    ui.separator();
                    if ui
                        .button("＋ Timeline")
                        .on_hover_text("Adicionar nova timeline (uma camada de animação)")
                        .clicked()
                    {
                        track_add = true;
                    }
                    if ui
                        .button("Abrir outro trabalho neste projeto")
                        .on_hover_text(
                            "Abre outro .sketchmotion como timelines abaixo (referência). \
                             O salvamento continua sendo o arquivo atual.",
                        )
                        .clicked()
                    {
                        open_ref = true;
                    }
                    ui.separator();
                    if ui
                        .checkbox(&mut self.document.onion_between, "Onion skin entre timelines")
                        .on_hover_text(
                            "Vê a mesma página das outras timelines translúcida \
                             (desliga o onion entre páginas)",
                        )
                        .changed()
                    {
                        if self.document.onion_between {
                            self.onion = false;
                        }
                        self.dirty = true;
                    }
                    ui.separator();
                    ui.checkbox(&mut self.view_both, "Ver ambas")
                        .on_hover_text("Compõe todas as timelines visíveis no canvas");
                    ui.checkbox(&mut self.play_both, "Play ambas")
                        .on_hover_text("Reproduz todas as timelines juntas");
                    ui.separator();
                    // Botão de modo "Selecionar frames" (clique alterna a seleção).
                    let sel_btn = egui::Button::new("Selecionar frames").fill(if self.frame_sel_mode {
                        egui::Color32::from_rgb(0x2F, 0x84, 0xFE)
                    } else {
                        egui::Color32::from_gray(60)
                    });
                    if ui
                        .add(sel_btn)
                        .on_hover_text("Ligado: clicar nos frames marca vários (para copiar em conjunto)")
                        .clicked()
                    {
                        self.frame_sel_mode = !self.frame_sel_mode;
                    }
                    if !self.frame_sel.is_empty() {
                        ui.label(format!("{} selec.", self.frame_sel.len()));
                        if ui.button("Limpar").clicked() {
                            self.frame_sel.clear();
                        }
                    }
                });
                ui.separator();

                // Uma lane por timeline. Altura fixa derivada da altura do painel
                // (evita o loop que fazia a barra "pular" para o topo).
                let lanes_h = (self.timeline_h - 84.0).max(48.0);
                egui::ScrollArea::vertical()
                    .max_height(lanes_h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for ti in 0..ntr {
                            let is_active = ti == active;
                            ui.horizontal(|ui| {
                                // Coluna esquerda: selecionar (●) + excluir (✕).
                                ui.vertical(|ui| {
                                    let mark = if is_active { "●" } else { "○" };
                                    let sel_btn = egui::Button::new(mark)
                                        .min_size(egui::vec2(36.0, 30.0))
                                        .fill(if is_active {
                                            egui::Color32::from_rgb(0x2F, 0x84, 0xFE)
                                        } else {
                                            egui::Color32::from_gray(60)
                                        });
                                    if ui
                                        .add(sel_btn)
                                        .on_hover_text("Selecionar (só a selecionada é editável)")
                                        .clicked()
                                    {
                                        sel_track = Some(ti);
                                    }
                                    if ui
                                        .add_enabled(
                                            ntr > 1,
                                            egui::Button::new(egui_phosphor::regular::TRASH)
                                                .min_size(egui::vec2(36.0, 24.0)),
                                        )
                                        .on_hover_text("Excluir esta timeline")
                                        .clicked()
                                    {
                                        del_track = Some(ti);
                                    }
                                });
                                ui.add_space(4.0);
                                // Coluna direita: controles + tira de frames.
                                ui.vertical(|ui| {
                                    ui.horizontal(|ui| {
                                        let (cur, total) = if is_active {
                                            (
                                                self.document.current,
                                                self.document.frames.len().max(1),
                                            )
                                        } else {
                                            let t = &self.document.tracks[ti];
                                            let tot = t.frames.len().max(1);
                                            (t.current.min(tot - 1), tot)
                                        };
                                        let play_lbl = if self.playing && is_active {
                                            "⏸ Parar"
                                        } else {
                                            "▶ Play"
                                        };
                                        if ui
                                            .button(play_lbl)
                                            .on_hover_text("Rodar esta timeline no FPS definido")
                                            .clicked()
                                        {
                                            do_play = Some(ti);
                                        }
                                        ui.separator();
                                        if ui
                                            .button("＋ Frame")
                                            .on_hover_text("Novo frame após o atual")
                                            .clicked()
                                        {
                                            do_add = Some(ti);
                                        }
                                        if ui.button("Duplicar").clicked() {
                                            do_dup = Some(ti);
                                        }
                                        if ui
                                            .add_enabled(total > 1, egui::Button::new("Excluir"))
                                            .clicked()
                                        {
                                            do_delframe = Some(ti);
                                        }
                                        ui.separator();
                                        if ui
                                            .button("Copiar")
                                            .on_hover_text("Copiar o frame atual")
                                            .clicked()
                                        {
                                            do_copy = Some(ti);
                                        }
                                        if ui
                                            .add_enabled(
                                                !self.frames_clip.is_empty(),
                                                egui::Button::new("Colar"),
                                            )
                                            .on_hover_text("Cola o(s) frame(s) copiado(s) em sequência")
                                            .clicked()
                                        {
                                            do_paste = Some(ti);
                                        }
                                        ui.separator();
                                        if ui.button("◀").clicked() {
                                            do_prev = Some(ti);
                                        }
                                        ui.label(format!("Frame {}/{}", cur + 1, total));
                                        if ui.button("▶").clicked() {
                                            do_next = Some(ti);
                                        }
                                        ui.separator();
                                        ui.label("FPS:");
                                        if is_active {
                                            let mut fps = self.document.fps as i32;
                                            if ui.add(egui::Slider::new(&mut fps, 1..=60)).changed() {
                                                self.document.fps = fps.clamp(1, 60) as u32;
                                                self.dirty = true;
                                            }
                                            nudge_u32(ui, &mut self.document.fps, 1, 60);
                                        } else {
                                            let mut fps = self.document.tracks[ti].fps as i32;
                                            if ui.add(egui::Slider::new(&mut fps, 1..=60)).changed() {
                                                self.document.tracks[ti].fps =
                                                    fps.clamp(1, 60) as u32;
                                                self.dirty = true;
                                            }
                                            nudge_u32(ui, &mut self.document.tracks[ti].fps, 1, 60);
                                        }
                                        ui.separator();
                                        if ui
                                            .checkbox(&mut self.onion, "Onion skin")
                                            .on_hover_text("Mostra o frame anterior a 30%")
                                            .changed()
                                        {
                                            if self.onion {
                                                self.document.onion_between = false;
                                            }
                                        }
                                    });
                                    // Tira de frames desta timeline (arrastável para reordenar).
                                    ui.push_id(ti, |ui| {
                                        egui::ScrollArea::horizontal()
                                            .drag_to_scroll(false)
                                            .show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                let total = if is_active {
                                                    self.document.frames.len()
                                                } else {
                                                    self.document.tracks[ti].frames.len()
                                                };
                                                let cur = if is_active {
                                                    self.document.current
                                                } else {
                                                    self.document.tracks[ti].current
                                                };
                                                let dragging_this = matches!(
                                                    self.drag_frame,
                                                    Some((dt, _)) if dt == ti
                                                );
                                                let mut rects: Vec<egui::Rect> =
                                                    Vec::with_capacity(total);
                                                for fi in 0..total {
                                                    let sel = fi == cur;
                                                    let tex = match self
                                                        .track_thumbs
                                                        .get(ti)
                                                        .and_then(|v| v.get(fi))
                                                        .and_then(|o| o.as_ref())
                                                    {
                                                        Some(t) => t.clone(),
                                                        None => continue,
                                                    };
                                                    let (rect, resp) = ui.allocate_exact_size(
                                                        egui::vec2(
                                                            th_w as f32,
                                                            th_h as f32 + 14.0,
                                                        ),
                                                        egui::Sense::click_and_drag(),
                                                    );
                                                    let img_rect = egui::Rect::from_min_size(
                                                        rect.min,
                                                        egui::vec2(th_w as f32, th_h as f32),
                                                    );
                                                    rects.push(img_rect);
                                                    let being = self.drag_frame == Some((ti, fi));
                                                    let painter = ui.painter_at(rect);
                                                    painter.rect_filled(
                                                        img_rect,
                                                        0.0,
                                                        egui::Color32::from_gray(30),
                                                    );
                                                    painter.image(
                                                        tex.id(),
                                                        img_rect,
                                                        egui::Rect::from_min_max(
                                                            egui::pos2(0.0, 0.0),
                                                            egui::pos2(1.0, 1.0),
                                                        ),
                                                        egui::Color32::WHITE,
                                                    );
                                                    if being {
                                                        painter.rect_filled(
                                                            img_rect,
                                                            0.0,
                                                            egui::Color32::from_black_alpha(130),
                                                        );
                                                    }
                                                    let cor = if being || (sel && is_active) {
                                                        egui::Color32::from_rgb(0x2F, 0x84, 0xFE)
                                                    } else if sel {
                                                        egui::Color32::from_gray(150)
                                                    } else {
                                                        egui::Color32::from_gray(90)
                                                    };
                                                    painter.rect_stroke(
                                                        img_rect,
                                                        0.0,
                                                        egui::Stroke::new(
                                                            if sel || being { 2.0 } else { 1.0 },
                                                            cor,
                                                        ),
                                                    );
                                                    painter.text(
                                                        egui::pos2(
                                                            rect.center().x,
                                                            img_rect.bottom() + 7.0,
                                                        ),
                                                        egui::Align2::CENTER_CENTER,
                                                        format!("{}", fi + 1),
                                                        egui::FontId::proportional(11.0),
                                                        cor,
                                                    );
                                                    // Marca de seleção múltipla (Shift-clique): faixa laranja no topo.
                                                    if is_active && self.frame_sel.contains(&fi) {
                                                        let bar = egui::Rect::from_min_max(
                                                            img_rect.left_top(),
                                                            egui::pos2(
                                                                img_rect.right(),
                                                                img_rect.top() + 4.0,
                                                            ),
                                                        );
                                                        painter.rect_filled(
                                                            bar,
                                                            0.0,
                                                            egui::Color32::from_rgb(0xE0, 0xB0, 0x3A),
                                                        );
                                                    }
                                                    if resp.drag_started() {
                                                        drag_start = Some((ti, fi));
                                                    }
                                                    if resp.clicked() {
                                                        let shift = ui.input(|i| i.modifiers.shift);
                                                        // Modo seleção OU Shift: alterna a seleção.
                                                        if is_active && (shift || self.frame_sel_mode)
                                                        {
                                                            if let Some(p) = self
                                                                .frame_sel
                                                                .iter()
                                                                .position(|&x| x == fi)
                                                            {
                                                                self.frame_sel.remove(p);
                                                            } else {
                                                                self.frame_sel.push(fi);
                                                            }
                                                        } else {
                                                            self.frame_sel.clear();
                                                            goto = Some((ti, fi));
                                                        }
                                                    }
                                                    ui.add_space(5.0);
                                                }
                                                // Tarja azul de inserção enquanto arrasta nesta faixa.
                                                if dragging_this && !rects.is_empty() {
                                                    if let Some(px) =
                                                        ui.input(|i| i.pointer.interact_pos()).map(|p| p.x)
                                                    {
                                                        let mut gap = rects
                                                            .iter()
                                                            .filter(|r| r.center().x < px)
                                                            .count();
                                                        if gap > rects.len() {
                                                            gap = rects.len();
                                                        }
                                                        let bar_x = if gap == 0 {
                                                            rects[0].left() - 3.0
                                                        } else if gap >= rects.len() {
                                                            rects[rects.len() - 1].right() + 3.0
                                                        } else {
                                                            (rects[gap - 1].right()
                                                                + rects[gap].left())
                                                                * 0.5
                                                        };
                                                        ui.painter().line_segment(
                                                            [
                                                                egui::pos2(bar_x, rects[0].top()),
                                                                egui::pos2(bar_x, rects[0].bottom()),
                                                            ],
                                                            egui::Stroke::new(
                                                                3.0,
                                                                egui::Color32::from_rgb(
                                                                    0x2F, 0x84, 0xFE,
                                                                ),
                                                            ),
                                                        );
                                                        if ui.input(|i| i.pointer.any_released()) {
                                                            if let Some((_, from)) = self.drag_frame {
                                                                drag_apply = Some((ti, from, gap));
                                                            }
                                                        }
                                                    }
                                                }
                                            });
                                        });
                                    });
                                });
                            });
                            ui.separator();
                        }
                    });

                // ---- Trilha da CÂMERA (keyframes na timeline ativa) ----
                ui.separator();
                ui.horizontal(|ui| {
                    ui.strong("Câmera");
                    let cur = self.document.current;
                    let has = self.document.camera.keyframe_index(cur).is_some();
                    if ui
                        .button(if has {
                            "◆ Remover keyframe"
                        } else {
                            "◇ Keyframe aqui"
                        })
                        .on_hover_text("Cria/remove keyframe da câmera no frame atual")
                        .clicked()
                    {
                        cam_kf_toggle = Some(cur);
                    }
                    ui.separator();
                    ui.label("Interp:");
                    egui::ComboBox::from_id_salt("cam_interp_tl")
                        .selected_text(self.cam_interp.label())
                        .show_ui(ui, |ui| {
                            for it in Interp::all() {
                                ui.selectable_value(&mut self.cam_interp, it, it.label());
                            }
                        });
                    ui.separator();
                    if ui.button("Reset").on_hover_text("Câmera = canvas cheio").clicked() {
                        cam_reset = true;
                    }
                    ui.separator();
                    if ui
                        .button("Controles…")
                        .on_hover_text("Abrir a janela de controles da câmera")
                        .clicked()
                    {
                        self.win_camera = true;
                        self.tool = Tool::Camera;
                    }
                    ui.weak("(arraste um ◆ para mover de frame)");
                });
                ui.push_id("cam_track_strip", |ui| {
                egui::ScrollArea::horizontal()
                    .max_height(30.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let total = self.document.frames.len();
                            let cur = self.document.current;
                            let cell = 26.0_f32;
                            let mut cells: Vec<egui::Rect> = Vec::with_capacity(total);
                            for fi in 0..total {
                                let (rect, resp) = ui.allocate_exact_size(
                                    egui::vec2(cell, 22.0),
                                    egui::Sense::click_and_drag(),
                                );
                                cells.push(rect);
                                let has_kf = self.document.camera.keyframe_index(fi).is_some();
                                let is_cur = fi == cur;
                                let p = ui.painter_at(rect);
                                p.rect_filled(
                                    rect,
                                    0.0,
                                    if is_cur {
                                        egui::Color32::from_rgb(0x22, 0x3A, 0x5A)
                                    } else {
                                        egui::Color32::from_gray(38)
                                    },
                                );
                                p.rect_stroke(
                                    rect,
                                    0.0,
                                    egui::Stroke::new(1.0, egui::Color32::from_gray(70)),
                                );
                                if has_kf {
                                    let cc = rect.center();
                                    let r = 5.0;
                                    let pts = vec![
                                        egui::pos2(cc.x, cc.y - r),
                                        egui::pos2(cc.x + r, cc.y),
                                        egui::pos2(cc.x, cc.y + r),
                                        egui::pos2(cc.x - r, cc.y),
                                    ];
                                    p.add(egui::Shape::convex_polygon(
                                        pts,
                                        egui::Color32::from_rgb(0x2F, 0x84, 0xFE),
                                        egui::Stroke::new(1.0, egui::Color32::WHITE),
                                    ));
                                }
                                if resp.drag_started() && has_kf {
                                    self.cam_kf_drag = Some(fi);
                                }
                                if resp.clicked() {
                                    cam_goto = Some(fi);
                                }
                            }
                            // Soltar um keyframe arrastado sobre outra célula = mover.
                            if let Some(from) = self.cam_kf_drag {
                                if ui.input(|i| i.pointer.any_released()) {
                                    if let Some(px) =
                                        ui.input(|i| i.pointer.interact_pos()).map(|p| p.x)
                                    {
                                        if let Some((to, _)) = cells
                                            .iter()
                                            .enumerate()
                                            .min_by(|a, b| {
                                                (a.1.center().x - px)
                                                    .abs()
                                                    .partial_cmp(&(b.1.center().x - px).abs())
                                                    .unwrap_or(std::cmp::Ordering::Equal)
                                            })
                                        {
                                            cam_kf_move = Some((from, to));
                                        }
                                    }
                                    self.cam_kf_drag = None;
                                }
                            }
                        });
                    });
                });
                ui.add_space(2.0);
            });
        if let Some(nh) = new_timeline_h {
            self.timeline_h = nh;
        }
        if let Some(fi) = cam_goto {
            let at = self.document.active_track;
            self.document.go_to_frame(fi);
            let _ = at;
            self.dirty = true;
            self.playing = false;
        }
        if let Some(cur) = cam_kf_toggle {
            if self.document.camera.keyframe_index(cur).is_some() {
                self.document.camera.remove_keyframe(cur);
            } else {
                self.camera_keyframe_atual();
            }
            self.dirty = true;
        }
        if let Some((from, to)) = cam_kf_move {
            self.document.camera.move_keyframe(from, to);
            self.dirty = true;
        }
        if cam_reset {
            let (w, h) = (self.document.width, self.document.height);
            self.document.camera.reset(w, h);
            self.dirty = true;
        }

        // ---- Aplicar ações ----
        if track_add {
            self.document.add_track();
            self.onion_for = None;
            self.dirty = true;
            self.playing = false;
        }
        if let Some(ti) = sel_track {
            self.document.go_to_track(ti);
            self.onion_for = None;
            self.dirty = true;
            self.playing = false;
        }
        if let Some(ti) = del_track {
            self.document.remove_track(ti);
            self.onion_for = None;
            self.dirty = true;
            self.playing = false;
        }
        if let Some((ti, fi)) = goto {
            if ti != self.document.active_track {
                self.document.go_to_track(ti);
            }
            self.document.go_to_frame(fi);
            self.onion_for = None;
            self.dirty = true;
            self.playing = false;
        }
        if let Some(ti) = do_prev {
            if ti != self.document.active_track {
                self.document.go_to_track(ti);
            }
            if self.document.current > 0 {
                self.document.go_to_frame(self.document.current - 1);
            }
            self.onion_for = None;
            self.dirty = true;
            self.playing = false;
        }
        if let Some(ti) = do_next {
            if ti != self.document.active_track {
                self.document.go_to_track(ti);
            }
            if self.document.current + 1 < self.document.frames.len() {
                self.document.go_to_frame(self.document.current + 1);
            }
            self.onion_for = None;
            self.dirty = true;
            self.playing = false;
        }
        if let Some(ti) = do_add {
            if ti != self.document.active_track {
                self.document.go_to_track(ti);
            }
            self.document.add_frame();
            self.onion_for = None;
            self.dirty = true;
            self.playing = false;
        }
        if let Some(ti) = do_dup {
            if ti != self.document.active_track {
                self.document.go_to_track(ti);
            }
            self.document.duplicate_frame();
            self.onion_for = None;
            self.dirty = true;
            self.playing = false;
        }
        if let Some(ti) = do_delframe {
            if ti != self.document.active_track {
                self.document.go_to_track(ti);
            }
            let c = self.document.current;
            self.document.remove_frame(c);
            self.onion_for = None;
            self.dirty = true;
            self.playing = false;
        }
        if let Some(ti) = do_play {
            if ti != self.document.active_track {
                self.document.go_to_track(ti);
            }
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
        if let Some(ti) = do_copy {
            // Se há vários frames selecionados na faixa ativa, copia o conjunto
            // (em ordem); senão, copia o frame atual/da faixa.
            if ti == self.document.active_track && !self.frame_sel.is_empty() {
                self.document.sync_to_frames();
                let mut idxs = self.frame_sel.clone();
                idxs.sort_unstable();
                idxs.dedup();
                let clip: Vec<Frame> = idxs
                    .iter()
                    .filter_map(|&i| self.document.frames.get(i).cloned())
                    .collect();
                let n = clip.len();
                self.frames_clip = clip;
                self.status = format!("{n} frames copiados — Colar insere na sequência");
            } else {
                let f = if ti == self.document.active_track {
                    self.document.current_frame_clone()
                } else if let Some(t) = self.document.tracks.get(ti) {
                    let c = t.current.min(t.frames.len().saturating_sub(1));
                    t.frames
                        .get(c)
                        .cloned()
                        .unwrap_or_else(|| self.document.current_frame_clone())
                } else {
                    self.document.current_frame_clone()
                };
                self.frames_clip = vec![f];
                self.status = "Frame copiado — use Colar na timeline desejada".into();
            }
        }
        if let Some(ti) = do_paste {
            if !self.frames_clip.is_empty() {
                if ti != self.document.active_track {
                    self.document.go_to_track(ti);
                }
                let n = self.frames_clip.len();
                for f in self.frames_clip.clone() {
                    self.document.paste_frame(f);
                }
                self.track_thumbs.clear();
                self.onion_for = None;
                self.dirty = true;
                self.playing = false;
                self.status = if n > 1 {
                    format!("{n} frames colados em sequência")
                } else {
                    "Frame colado após o atual".into()
                };
            }
        }
        // Arrasto de frames: registra início e aplica movimento ao soltar.
        if let Some((ti, fi)) = drag_start {
            self.drag_frame = Some((ti, fi));
        }
        if let Some((ti, from, to)) = drag_apply {
            if ti != self.document.active_track {
                self.document.go_to_track(ti);
            }
            self.document.move_frame(from, to);
            self.drag_frame = None;
            self.track_thumbs.clear();
            self.onion_for = None;
            self.dirty = true;
            self.playing = false;
        } else if self.drag_frame.is_some() && ctx.input(|i| !i.pointer.any_down()) {
            // Arrasto terminou sem alvo válido: cancela.
            self.drag_frame = None;
        }
        if open_ref {
            self.abrir_referencia();
        }
    }

    /// Compõe todas as timelines visíveis no playhead dado (para "Play ambas").
    /// Faz over-blend de baixo (índice 0) para cima; devolve RGBA não-premult.
    fn compose_tracks_at(&self, playhead: usize) -> Vec<u8> {
        let w = self.document.width as usize;
        let h = self.document.height as usize;
        let mut base = vec![0u8; w * h * 4];
        for t in &self.document.tracks {
            if !t.visible || t.frames.is_empty() {
                continue;
            }
            let idx = playhead % t.frames.len();
            let top = render_frame_alpha(
                self.document.width,
                self.document.height,
                &t.frames[idx].layers,
                &t.frames[idx].vectors,
                &t.frames[idx].images,
            );
            for p in 0..(w * h) {
                let sa = top.rgba[p * 4 + 3] as f32 / 255.0;
                if sa <= 0.0 {
                    continue;
                }
                let da = base[p * 4 + 3] as f32 / 255.0;
                let outa = sa + da * (1.0 - sa);
                for c in 0..3 {
                    let sc = top.rgba[p * 4 + c] as f32;
                    let dc = base[p * 4 + c] as f32;
                    let outc = (sc * sa + dc * da * (1.0 - sa)) / outa.max(1e-6);
                    base[p * 4 + c] = outc.round().clamp(0.0, 255.0) as u8;
                }
                base[p * 4 + 3] = (outa * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
        base
    }

    /// Terceira barra (abaixo das duas do topo): mostra dinamicamente arquivo,
    /// timeline, frame e camada selecionados — para o usuário não se perder.
    fn barra_status(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("status").exact_height(24.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                let arq = self
                    .current_path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .and_then(|s| s.to_str())
                    .unwrap_or("Sem título");
                ui.strong("Arquivo:");
                ui.label(arq);
                ui.separator();
                let at = self.document.active_track;
                let tname = self
                    .document
                    .tracks
                    .get(at)
                    .map(|t| t.name.clone())
                    .unwrap_or_else(|| format!("Timeline {}", at + 1));
                ui.strong("Timeline:");
                ui.label(tname);
                ui.separator();
                ui.strong("Frame:");
                ui.label(format!(
                    "{}/{}",
                    self.document.current + 1,
                    self.document.frames.len().max(1)
                ));
                ui.separator();
                ui.strong("Camada:");
                let (lname, locked) = self
                    .document
                    .layers
                    .get(self.active_layer)
                    .map(|l| (l.name.clone(), l.locked))
                    .unwrap_or_else(|| (format!("Camada {}", self.active_layer + 1), false));
                if locked {
                    ui.colored_label(
                        egui::Color32::from_rgb(0xE0, 0x6C, 0x3A),
                        format!("{lname} (bloqueada)"),
                    );
                } else {
                    ui.label(lname);
                }
            });
        });
    }

    /// Quarta barra (abaixo da barra de status): ações rápidas. Hoje, integrar
    /// objetos de imagem (Caminho B) aos pixels da camada. Mais botões de função
    /// serão adicionados aqui no futuro.
    fn barra_acoes(&mut self, ctx: &egui::Context) {
        let mut integrar_sel = false;
        let mut integrar_todas = false;
        let mut do_group = false;
        let mut do_ungroup = false;
        egui::TopBottomPanel::top("acoes")
            .exact_height(30.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    ui.strong("Imagem:");
                    let tem_sel = matches!(&self.float_sel, Some(f) if f.is_image);
                    let n_obj = self.document.images.len();
                    ui.add_enabled_ui(tem_sel, |ui| {
                        if ui
                            .button("Integrar")
                            .on_hover_text(
                                "Rasteriza a imagem selecionada na camada (deixa de ser objeto móvel)",
                            )
                            .clicked()
                        {
                            integrar_sel = true;
                        }
                    });
                    ui.add_enabled_ui(n_obj > 0 || tem_sel, |ui| {
                        if ui
                            .button(format!("Integrar todas ({n_obj})"))
                            .on_hover_text("Rasteriza TODOS os objetos de imagem deste frame")
                            .clicked()
                        {
                            integrar_todas = true;
                        }
                    });
                    ui.separator();
                    ui.strong("Vetores:");
                    let n_sel = self.sel_set.len();
                    ui.add_enabled_ui(n_sel >= 2, |ui| {
                        if ui
                            .button("Agrupar")
                            .on_hover_text("Agrupa os objetos selecionados (movem juntos)")
                            .clicked()
                        {
                            do_group = true;
                        }
                    });
                    ui.add_enabled_ui(n_sel >= 1, |ui| {
                        if ui
                            .button("Desagrupar")
                            .on_hover_text("Solta os objetos do grupo selecionado")
                            .clicked()
                        {
                            do_ungroup = true;
                        }
                    });
                    ui.label(format!("({n_sel} selec.)"));
                });
            });
        if integrar_sel {
            self.push_undo();
            self.commit_float();
            self.status = "Imagem integrada aos pixels".into();
        }
        if integrar_todas {
            self.integrar_todas_imagens();
        }
        if do_group {
            self.agrupar_selecao();
        }
        if do_ungroup {
            self.desagrupar_selecao();
        }
    }

    /// Integra (rasteriza) todos os objetos de imagem do frame atual nos pixels
    /// das respectivas camadas. Objetos em camada bloqueada são preservados.
    fn integrar_todas_imagens(&mut self) {
        let tem_float = matches!(&self.float_sel, Some(f) if f.is_image);
        if !tem_float && self.document.images.is_empty() {
            return;
        }
        self.push_undo();
        if tem_float {
            self.commit_float();
            // Se ficou por bloqueio, devolve como objeto para não se perder.
            if let Some(fs) = self.float_sel.take() {
                self.document.images.push(ImageObject::new(
                    fs.pixels, fs.ow, fs.oh, fs.cx, fs.cy, fs.hw, fs.hh, fs.angle, fs.opacity,
                    fs.layer,
                ));
                self.float_tex = None;
            }
        }
        let objs = std::mem::take(&mut self.document.images);
        let mut restantes: Vec<ImageObject> = Vec::new();
        for o in objs {
            let li = o.layer.min(self.document.layers.len().saturating_sub(1));
            if self.layer_locked(li) {
                restantes.push(o);
                continue;
            }
            self.float_sel = Some(FloatSel {
                pixels: o.pixels,
                ow: o.ow,
                oh: o.oh,
                cx: o.cx,
                cy: o.cy,
                hw: o.hw,
                hh: o.hh,
                angle: o.angle,
                opacity: o.opacity,
                layer: li,
                is_image: true,
            });
            self.commit_float();
            if let Some(fs) = self.float_sel.take() {
                restantes.push(ImageObject::new(
                    fs.pixels, fs.ow, fs.oh, fs.cx, fs.cy, fs.hw, fs.hh, fs.angle, fs.opacity,
                    fs.layer,
                ));
            }
        }
        self.document.images = restantes;
        self.document.sync_to_frames();
        self.float_tex = None;
        self.dirty = true;
        self.status = "Imagens integradas aos pixels".into();
    }

    /// Rodapé: versão, criador e link do GitHub.
    fn rodape(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("rodape")
            .exact_height(22.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("SketchMotion v{}", env!("CARGO_PKG_VERSION")));
                    ui.separator();
                    ui.label("por Gabriel Pedreira");
                    ui.separator();
                    ui.hyperlink_to(
                        "github.com/gabrielpedreira",
                        "https://github.com/gabrielpedreira",
                    );
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
                        Tool::Lasso => self.opcoes_laco(ui),
                        Tool::Text => self.opcoes_texto(ui),
                        Tool::Pen => self.opcoes_caneta(ui),
                        Tool::MagicWand => self.opcoes_varinha(ui),
                        Tool::DirectSelect => self.opcoes_selecao_direta(ui),
                        Tool::Shapes => self.opcoes_formas(ui),
                        Tool::Fill => self.opcoes_balde(ui),
                        Tool::Rig => self.opcoes_rig(ui),
                        Tool::Grupo => self.opcoes_objetos(ui),
                        Tool::Camera => self.opcoes_camera(ui),
                        Tool::Pivot => self.opcoes_pivo(ui),
                        Tool::DirVector => self.opcoes_dirvec(ui),
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
        if self.brush_prev.len() != BRUSHES.len() {
            self.brush_prev = vec![None; BRUSHES.len()];
        }
        for i in 0..BRUSHES.len() {
            if self.brush_prev[i].is_none() {
                let img = preview_brush(i);
                self.brush_prev[i] = Some(ui.ctx().load_texture(
                    format!("brush_prev{i}"),
                    img,
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
        if self.pixel_mode {
            ui.label("Pincel: Duro (no pixel art só o pincel comum)");
        } else {
            ui.label("Pincel:");
            if let Some(tex) = &self.brush_prev[self.brush_kind] {
                ui.add(egui::Image::from_texture(egui::load::SizedTexture::new(
                    tex.id(),
                    egui::vec2(50.0, 18.0),
                )));
            }
            egui::ComboBox::from_id_salt("tipo_pincel")
                .selected_text(BRUSHES[self.brush_kind])
                .width(190.0)
                .show_ui(ui, |ui| {
                    for i in 0..BRUSHES.len() {
                        ui.horizontal(|ui| {
                            if let Some(tex) = &self.brush_prev[i] {
                                ui.add(egui::Image::from_texture(egui::load::SizedTexture::new(
                                    tex.id(),
                                    egui::vec2(64.0, 20.0),
                                )));
                            }
                            ui.selectable_value(&mut self.brush_kind, i, BRUSHES[i]);
                        });
                    }
                });
        }
        ui.separator();
        ui.label("Tamanho:");
        ui.add(egui::Slider::new(&mut self.brush_radius, 1..=40));
        nudge_i32(ui, &mut self.brush_radius, 1, 40);
        ui.separator();
        self.swatch_cor(ui);
        ui.separator();
        self.botao_limpar(ui);
    }

    fn opcoes_borracha(&mut self, ui: &mut egui::Ui) {
        ui.label("Tamanho:");
        ui.add(egui::Slider::new(&mut self.eraser_radius, 1..=60));
        nudge_i32(ui, &mut self.eraser_radius, 1, 60);
        ui.separator();
        self.botao_limpar(ui);
    }

    /// Ferramenta Seleção: opera sobre o objeto vetorial selecionado.
    fn opcoes_selecao(&mut self, ui: &mut egui::Ui) {
        if self.float_sel.is_some() {
            let locked = self.float_locked();
            let lname = self
                .float_sel
                .as_ref()
                .and_then(|f| self.document.layer(f.layer))
                .map(|l| l.name.clone())
                .unwrap_or_default();
            ui.label(format!("Imagem/seleção (camada \"{lname}\")."));
            if locked {
                ui.separator();
                ui.colored_label(
                    egui::Color32::from_rgb(0xE0, 0x6C, 0x3A),
                    "🔒 Camada bloqueada — desbloqueie para editar",
                );
            }
            ui.separator();
            ui.add_enabled_ui(!locked, |ui| {
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
                ui.horizontal(|ui| {
                    if ui.button("Espelhar H").clicked() {
                        self.float_flip_h();
                    }
                    if ui.button("Espelhar V").clicked() {
                        self.float_flip_v();
                    }
                    if ui.button("－").on_hover_text("Diminuir 10%").clicked() {
                        self.escalar_float(1.0 / 1.1);
                    }
                    if ui.button("＋").on_hover_text("Aumentar 10%").clicked() {
                        self.escalar_float(1.1);
                    }
                    if ui
                        .button("Copiar")
                        .on_hover_text("Carimba a atual e cria uma cópia para posicionar")
                        .clicked()
                    {
                        self.duplicar_float();
                    }
                });
                ui.separator();
                if ui.button("Confirmar").clicked() {
                    self.drop_float();
                }
            });
            // Excluir a flutuante não altera a camada — permitido mesmo bloqueada.
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
                if ui.button("－").on_hover_text("Diminuir 10%").clicked() {
                    self.push_undo();
                    self.document.vectors[idx].scale_self(1.0 / 1.1);
                }
                if ui.button("＋").on_hover_text("Aumentar 10%").clicked() {
                    self.push_undo();
                    self.document.vectors[idx].scale_self(1.1);
                }
                if ui
                    .button("Copiar")
                    .on_hover_text("Duplica o traço selecionado")
                    .clicked()
                {
                    self.push_undo();
                    let mut copia = self.document.vectors[idx].clone();
                    copia.translate(12.0, 12.0);
                    self.document.vectors.push(copia);
                    self.selected_obj = Some(self.document.vectors.len() - 1);
                    self.dirty = true;
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
        ui.checkbox(&mut self.wand_contiguo, "Contíguo")
            .on_hover_text("Ligado: só a região conectada da cor. Desligado: toda a cor na camada.");
        ui.separator();
        ui.weak("Clique numa cor para selecioná-la (vira seleção móvel).");
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

    fn opcoes_objetos(&mut self, ui: &mut egui::Ui) {
        if self.place_piece && self.selected_piece.is_some() {
            ui.label("Clique no canvas para colar a peça.");
            if ui.button("Parar de colar (voltar ao laço)").clicked() {
                self.place_piece = false;
                self.selected_piece = None;
            }
        } else {
            ui.label("Laço: contorne o objeto e confirme para agrupar.");
        }
        ui.separator();
        if ui.button("Janela de objetos").clicked() {
            self.win_pieces = true;
        }
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

    /// Carrega uma textura de ícone PNG embutido (uma vez).
    fn carregar_icone_png(ctx: &egui::Context, nome: &str, bytes: &[u8]) -> Option<egui::TextureHandle> {
        let img = image::load_from_memory(bytes).ok()?;
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        let ci = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw());
        Some(ctx.load_texture(nome, ci, egui::TextureOptions::LINEAR))
    }

    /// Botão de ícone a partir de uma textura PNG (mesma pegada visual do
    /// `icon_button` glífico: destaca quando ativo).
    fn icon_img_button(ui: &mut egui::Ui, active: bool, tex: &egui::TextureHandle) -> egui::Response {
        let size = egui::vec2(22.0, 22.0);
        let img = egui::Image::new(egui::load::SizedTexture::new(tex.id(), size));
        ui.add(egui::ImageButton::new(img).selected(active))
    }

    fn barra_icones(&mut self, ctx: &egui::Context) {
        use egui_phosphor::regular as icon;
        if self.tex_pivo.is_none() {
            const B: &[u8] = include_bytes!("../../../assets/pivo.png");
            self.tex_pivo = Self::carregar_icone_png(ctx, "ic_pivo", B);
        }
        if self.tex_vetor_trace.is_none() {
            const B: &[u8] = include_bytes!("../../../assets/vetor_trace.png");
            self.tex_vetor_trace = Self::carregar_icone_png(ctx, "ic_vetor_trace", B);
        }
        if self.tex_dirvec.is_none() {
            const B: &[u8] = include_bytes!("../../../assets/vetor_direcao.png");
            self.tex_dirvec = Self::carregar_icone_png(ctx, "ic_dirvec", B);
        }
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
                    ui.add_space(6.0);

                    let resp_r = icon_button(ui, self.win_rig, icon::PERSON_SIMPLE)
                        .on_hover_text("Rig (esqueletos)");
                    self.icon_r_rig = Some(resp_r.rect);
                    if resp_r.clicked() {
                        self.win_rig = !self.win_rig;
                        if self.win_rig {
                            self.reopen_rig = true;
                        }
                    }
                    ui.add_space(6.0);

                    let resp_o = icon_button(ui, self.win_pieces, icon::COPY)
                        .on_hover_text("Objetos — agrupar por laço e reutilizar peças");
                    if resp_o.clicked() {
                        self.win_pieces = !self.win_pieces;
                        if self.win_pieces {
                            self.tool = Tool::Grupo;
                            self.place_piece = false;
                            self.eyedropper = Eyedropper::Off;
                        }
                    }
                    ui.add_space(6.0);

                    let resp_pr = icon_button(ui, self.win_prancheta, icon::SELECTION)
                        .on_hover_text("Prancheta — mudar o tamanho do papel (canvas)");
                    if resp_pr.clicked() {
                        self.win_prancheta = !self.win_prancheta;
                        if self.win_prancheta {
                            self.pr_w = self.document.width;
                            self.pr_h = self.document.height;
                        } else {
                            self.prancheta_drag = None;
                            self.prancheta_preview = None;
                        }
                    }
                    ui.add_space(6.0);

                    let resp_cam = icon_button(ui, self.win_camera, icon::VIDEO_CAMERA)
                        .on_hover_text("Câmera — enquadramento animado por keyframes");
                    if resp_cam.clicked() {
                        self.win_camera = !self.win_camera;
                        if self.win_camera {
                            self.tool = Tool::Camera;
                            self.eyedropper = Eyedropper::Off;
                        }
                    }
                    ui.add_space(6.0);

                    let resp_tr = match &self.tex_vetor_trace {
                        Some(tex) => Self::icon_img_button(ui, self.win_trace, tex),
                        None => icon_button(ui, self.win_trace, icon::VECTOR_TWO),
                    }
                    .on_hover_text("Traçado de Imagem — vetorizar (raster → vetor)");
                    if resp_tr.clicked() {
                        self.win_trace = !self.win_trace;
                    }
                    ui.add_space(6.0);

                    let resp_pv = match &self.tex_pivo {
                        Some(tex) => Self::icon_img_button(ui, self.win_pivot, tex),
                        None => icon_button(ui, self.win_pivot, icon::CROSSHAIR),
                    }
                    .on_hover_text("Pivô — eixo de transformação personalizado");
                    if resp_pv.clicked() {
                        self.win_pivot = !self.win_pivot;
                        if self.win_pivot {
                            self.tool = Tool::Pivot;
                            self.eyedropper = Eyedropper::Off;
                        }
                    }
                    ui.add_space(6.0);

                    let resp_dv = match &self.tex_dirvec {
                        Some(tex) => Self::icon_img_button(ui, self.win_dirvec, tex),
                        None => icon_button(ui, self.win_dirvec, icon::ARROW_RIGHT),
                    }
                    .on_hover_text("Vetor de Direção — animar objeto entre dois frames");
                    if resp_dv.clicked() {
                        self.win_dirvec = !self.win_dirvec;
                        if self.win_dirvec {
                            self.tool = Tool::DirVector;
                            self.eyedropper = Eyedropper::Off;
                        }
                    }
                });
            });
    }

    /// Aplica um novo tamanho de papel (redimensiona o canvas, sem escalar o
    /// desenho) e recentraliza.
    fn aplicar_prancheta(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 || (w == self.document.width && h == self.document.height) {
            return;
        }
        self.push_undo();
        self.document.resize_canvas(w, h);
        self.pr_w = w;
        self.pr_h = h;
        self.center_canvas = true;
        self.dirty = true;
        self.track_thumbs.clear();
        self.between_cache.clear();
        self.between_key = None;
        self.split_for = None;
        self.below_tex = None;
        self.above_tex = None;
        self.onion_for = None;
        self.status = format!("Papel: {w} x {h} px");
    }

    /// Janela Prancheta: muda o tamanho do papel (numérico) — o arrasto manual
    /// pelas alças ao redor do canvas é tratado no próprio canvas.
    fn janela_prancheta(&mut self, ctx: &egui::Context) {
        let mut open = self.win_prancheta;
        let mut aplicar: Option<(u32, u32)> = None;
        egui::Window::new("Prancheta — tamanho do papel")
            .open(&mut open)
            .default_width(330.0)
            .show(ctx, |ui| {
                ui.label(format!(
                    "Atual: {} x {} px",
                    self.document.width, self.document.height
                ));
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Largura:");
                    ui.add(
                        egui::DragValue::new(&mut self.pr_w)
                            .range(1..=8192)
                            .suffix(" px"),
                    );
                    ui.add_space(8.0);
                    ui.label("Altura:");
                    ui.add(
                        egui::DragValue::new(&mut self.pr_h)
                            .range(1..=8192)
                            .suffix(" px"),
                    );
                });
                ui.horizontal(|ui| {
                    if ui.button("Girar (trocar L×A)").clicked() {
                        std::mem::swap(&mut self.pr_w, &mut self.pr_h);
                    }
                    if ui.button("Usar atual").clicked() {
                        self.pr_w = self.document.width;
                        self.pr_h = self.document.height;
                    }
                });
                ui.separator();
                ui.label("Predefinições:");
                ui.horizontal_wrapped(|ui| {
                    for (nome, w, h) in [
                        ("32", 32u32, 32u32),
                        ("64", 64, 64),
                        ("128", 128, 128),
                        ("256", 256, 256),
                        ("512", 512, 512),
                        ("800×520", 800, 520),
                        ("1920×1080", 1920, 1080),
                    ] {
                        if ui.button(nome).clicked() {
                            self.pr_w = w;
                            self.pr_h = h;
                        }
                    }
                });
                ui.separator();
                if ui
                    .add(egui::Button::new("Aplicar").min_size(egui::vec2(100.0, 0.0)))
                    .clicked()
                {
                    aplicar = Some((self.pr_w, self.pr_h));
                }
                ui.label(
                    "Arraste os quadrados ao redor do canvas para redimensionar à mão. \
                     O desenho não é escalado — só o papel muda (vale no pixel art também).",
                );
            });
        if let Some((w, h)) = aplicar {
            self.aplicar_prancheta(w, h);
        }
        self.win_prancheta = open;
        if !self.win_prancheta {
            self.prancheta_drag = None;
            self.prancheta_preview = None;
        }
    }

    /// Painel da ferramenta Câmera na barra de opções do topo (atalhos rápidos).
    fn opcoes_camera(&mut self, ui: &mut egui::Ui) {
        ui.weak("Enquadramento animado por keyframes (retângulo azul).");
        ui.separator();
        if ui.button("Abrir controles da câmera").clicked() {
            self.win_camera = true;
        }
        ui.separator();
        let cur = self.document.current;
        let has = self.document.camera.keyframe_index(cur).is_some();
        if ui
            .button(if has {
                "Atualizar keyframe aqui"
            } else {
                "Criar keyframe aqui"
            })
            .on_hover_text("Fixa o enquadramento atual como keyframe no frame corrente")
            .clicked()
        {
            self.camera_keyframe_atual();
        }
    }

    /// Cria/atualiza um keyframe de câmera no frame atual com o estado amostrado.
    fn camera_keyframe_atual(&mut self) {
        let cur = self.document.current;
        let s = self.document.camera.sample(cur);
        self.document.camera.set_keyframe(CameraKeyframe {
            frame: cur,
            x: s.x,
            y: s.y,
            w: s.w,
            h: s.h,
            rotation: s.rotation,
            anchor_x: s.anchor_x,
            anchor_y: s.anchor_y,
            interp: self.cam_interp,
        });
        self.dirty = true;
        self.status = format!("Keyframe de câmera criado no frame {}", cur + 1);
    }

    // =====================  Ferramenta Pivô  =====================

    /// Descobre a que objeto/grupo o ponto (doc) pertence, para associar ao
    /// pivô. Um vetor solto ganha um grupo próprio (para a associação sobreviver
    /// a mudanças de índice/frames).
    fn pivot_hit_target(&mut self, dp: (f32, f32)) -> sketchmotion_core::PivotTarget {
        use sketchmotion_core::PivotTarget;
        if let Some(i) = self.hit_test(dp, 6.0) {
            let g = match self.document.vectors[i].group {
                Some(g) => g,
                None => {
                    let ng = self.next_group;
                    self.next_group += 1;
                    self.document.vectors[i].group = Some(ng);
                    ng
                }
            };
            return PivotTarget::Group(g);
        }
        for i in (0..self.document.images.len()).rev() {
            let o = &self.document.images[i];
            if o.hw <= 0.0 || o.hh <= 0.0 {
                continue;
            }
            let (s, c) = (-o.angle).sin_cos();
            let (rx, ry) = (dp.0 - o.cx, dp.1 - o.cy);
            let lx = rx * c - ry * s;
            let ly = rx * s + ry * c;
            if lx.abs() <= o.hw && ly.abs() <= o.hh {
                return PivotTarget::Image(i);
            }
        }
        PivotTarget::None
    }

    /// O ponto (doc) está sobre o objeto controlado pelo pivô `pi`?
    fn pivot_point_on_object(&self, pi: usize, dp: (f32, f32)) -> bool {
        use sketchmotion_core::PivotTarget;
        let Some(pv) = self.document.pivots.get(pi) else {
            return false;
        };
        match &pv.target {
            PivotTarget::Group(g) => self.document.vectors.iter().any(|o| {
                o.group == Some(*g)
                    && o.bounds().map_or(false, |(a, b, c, d)| {
                        dp.0 >= a && dp.0 <= c && dp.1 >= b && dp.1 <= d
                    })
            }),
            PivotTarget::Image(i) => {
                if let Some(o) = self.document.images.get(*i) {
                    let (s, c) = (-o.angle).sin_cos();
                    let (rx, ry) = (dp.0 - o.cx, dp.1 - o.cy);
                    let lx = rx * c - ry * s;
                    let ly = rx * s + ry * c;
                    lx.abs() <= o.hw && ly.abs() <= o.hh
                } else {
                    false
                }
            }
            PivotTarget::None => false,
        }
    }

    /// Translada o objeto controlado e os dois pontos do pivô por (dx, dy).
    fn pivot_translate(&mut self, pi: usize, dx: f32, dy: f32) {
        use sketchmotion_core::PivotTarget;
        let target = match self.document.pivots.get(pi) {
            Some(p) => p.target.clone(),
            None => return,
        };
        match target {
            PivotTarget::Group(g) => {
                for o in self.document.vectors.iter_mut() {
                    if o.group == Some(g) {
                        o.translate(dx, dy);
                    }
                }
            }
            PivotTarget::Image(i) => {
                if let Some(o) = self.document.images.get_mut(i) {
                    o.cx += dx;
                    o.cy += dy;
                }
            }
            PivotTarget::None => {}
        }
        if let Some(pv) = self.document.pivots.get_mut(pi) {
            pv.axis.0 += dx;
            pv.axis.1 += dy;
            pv.mov.0 += dx;
            pv.mov.1 += dy;
        }
        self.dirty = true;
        self.modificado = true;
    }

    /// Rotaciona o objeto controlado (e o ponto laranja) por `ang` rad ao redor
    /// do EIXO azul, que permanece fixo. É a transformação matemática real do
    /// pivô: posição_relativa → rotação → nova posição.
    fn pivot_rotate(&mut self, pi: usize, ang: f32) {
        use sketchmotion_core::PivotTarget;
        let (ax, target) = match self.document.pivots.get(pi) {
            Some(p) => (p.axis, p.target.clone()),
            None => return,
        };
        match target {
            PivotTarget::Group(g) => {
                for o in self.document.vectors.iter_mut() {
                    if o.group == Some(g) {
                        o.rotate_around(ax.0, ax.1, ang);
                    }
                }
            }
            PivotTarget::Image(i) => {
                if let Some(o) = self.document.images.get_mut(i) {
                    let (s, c) = ang.sin_cos();
                    let (dx, dy) = (o.cx - ax.0, o.cy - ax.1);
                    o.cx = ax.0 + dx * c - dy * s;
                    o.cy = ax.1 + dx * s + dy * c;
                    o.angle += ang;
                }
            }
            PivotTarget::None => {}
        }
        if let Some(pv) = self.document.pivots.get_mut(pi) {
            let (s, c) = ang.sin_cos();
            let (dx, dy) = (pv.mov.0 - ax.0, pv.mov.1 - ax.1);
            pv.mov.0 = ax.0 + dx * c - dy * s;
            pv.mov.1 = ax.1 + dx * s + dy * c;
        }
        self.dirty = true;
        self.modificado = true;
    }

    /// Interação da ferramenta Pivô no canvas (criação e arraste de pontos).
    fn interacao_pivo(
        &mut self,
        pressed: bool,
        down: bool,
        hover: Option<egui::Pos2>,
        ppos: Option<egui::Pos2>,
        pdelta: egui::Vec2,
        rect: egui::Rect,
        zoom: f32,
    ) {
        let to_doc = |p: egui::Pos2| ((p.x - rect.min.x) / zoom, (p.y - rect.min.y) / zoom);
        if pressed {
            if let Some(p) = hover {
                let dp = to_doc(p);
                // Fluxo de criação: primeiro clique = objeto + EIXO; segundo = MOV.
                if self.pivot_stage == 1 {
                    if let Some(pi) = self.pivot_sel {
                        let target = self.pivot_hit_target(dp);
                        let achou = !matches!(target, sketchmotion_core::PivotTarget::None);
                        if let Some(pv) = self.document.pivots.get_mut(pi) {
                            pv.target = target;
                            pv.axis = dp;
                            pv.has_axis = true;
                        }
                        self.pivot_stage = 2;
                        self.modificado = true;
                        self.status = if achou {
                            "Eixo definido. Agora clique no ponto de movimentação.".into()
                        } else {
                            "Eixo definido (nenhum objeto sob o clique). Defina o ponto de movimentação.".into()
                        };
                    }
                    return;
                }
                if self.pivot_stage == 2 {
                    if let Some(pi) = self.pivot_sel {
                        if let Some(pv) = self.document.pivots.get_mut(pi) {
                            pv.mov = dp;
                            pv.has_mov = true;
                        }
                        self.pivot_stage = 0;
                        self.modificado = true;
                        self.status =
                            "Pivô pronto — arraste o laranja para girar; o azul para mover.".into();
                    }
                    return;
                }
                // Ocioso: inicia arraste sobre azul / laranja / objeto.
                if let Some(pi) = self.pivot_sel {
                    let completo = self.document.pivots.get(pi).map_or(false, |p| p.completo());
                    if completo {
                        let (ax, mv) = {
                            let pv = &self.document.pivots[pi];
                            (pv.axis, pv.mov)
                        };
                        let r = 10.0 / zoom;
                        let da = ((dp.0 - ax.0).powi(2) + (dp.1 - ax.1).powi(2)).sqrt();
                        let dm = ((dp.0 - mv.0).powi(2) + (dp.1 - mv.1).powi(2)).sqrt();
                        if dm <= r && dm <= da {
                            self.pivot_drag = 2;
                            self.push_undo();
                        } else if da <= r {
                            self.pivot_drag = 1;
                            self.push_undo();
                        } else if self.pivot_point_on_object(pi, dp) {
                            self.pivot_drag = 3;
                            self.push_undo();
                        } else {
                            self.pivot_drag = 0;
                        }
                    }
                }
            }
        }
        if down && self.pivot_drag != 0 {
            if let (Some(pp), Some(pi)) = (ppos, self.pivot_sel) {
                match self.pivot_drag {
                    2 => {
                        // Laranja: gira o objeto ao redor do eixo pelo ângulo que
                        // o ponteiro varreu em torno do eixo.
                        let cur = to_doc(pp);
                        let ax = self.document.pivots[pi].axis;
                        let prev = (cur.0 - pdelta.x / zoom, cur.1 - pdelta.y / zoom);
                        let a0 = (prev.1 - ax.1).atan2(prev.0 - ax.0);
                        let a1 = (cur.1 - ax.1).atan2(cur.0 - ax.0);
                        let delta = a1 - a0;
                        if delta.abs() > f32::EPSILON {
                            self.pivot_rotate(pi, delta);
                        }
                    }
                    _ => {
                        // Azul ou objeto: translada tudo pelo deslocamento.
                        let (dx, dy) = (pdelta.x / zoom, pdelta.y / zoom);
                        if dx != 0.0 || dy != 0.0 {
                            self.pivot_translate(pi, dx, dy);
                        }
                    }
                }
            }
        }
        if !down {
            self.pivot_drag = 0;
        }
    }

    /// Desenha os pontos do(s) pivô(s): eixo azul, movimentação laranja e a
    /// linha que os liga. Apenas overlay de interface (não entra na exportação).
    fn desenhar_pivos(&self, ui: &mut egui::Ui, rect: egui::Rect, zoom: f32) {
        let painter = ui.painter_at(rect);
        let scr = |p: (f32, f32)| egui::pos2(rect.min.x + p.0 * zoom, rect.min.y + p.1 * zoom);
        let azul = egui::Color32::from_rgb(30, 120, 255);
        let laranja = egui::Color32::from_rgb(255, 140, 0);
        let branco = egui::Color32::WHITE;
        for (idx, pv) in self.document.pivots.iter().enumerate() {
            if !pv.has_axis {
                continue;
            }
            let sel = Some(idx) == self.pivot_sel;
            let ax = scr(pv.axis);
            let ra = if sel { 6.0 } else { 4.0 };
            if pv.has_mov {
                let mv = scr(pv.mov);
                painter.line_segment(
                    [ax, mv],
                    egui::Stroke::new(if sel { 2.0 } else { 1.0 }, egui::Color32::from_gray(180)),
                );
                painter.circle_filled(mv, ra, laranja);
                painter.circle_stroke(mv, ra, egui::Stroke::new(1.5, branco));
                if sel {
                    let hr = egui::Rect::from_center_size(mv, egui::vec2(16.0, 16.0));
                    ui.interact(hr, ui.id().with(("pv_mov", idx)), egui::Sense::hover())
                        .on_hover_text("Ponto de Movimentação");
                }
            }
            painter.circle_filled(ax, ra, azul);
            painter.circle_stroke(ax, ra, egui::Stroke::new(1.5, branco));
            if sel {
                let hr = egui::Rect::from_center_size(ax, egui::vec2(16.0, 16.0));
                ui.interact(hr, ui.id().with(("pv_ax", idx)), egui::Sense::hover())
                    .on_hover_text("Eixo / Pivô");
            }
        }
    }

    /// Barra de opções da ferramenta Pivô (topo).
    fn opcoes_pivo(&mut self, ui: &mut egui::Ui) {
        match self.pivot_stage {
            1 => {
                ui.colored_label(
                    egui::Color32::from_rgb(30, 120, 255),
                    "Clique no objeto para definir o EIXO (ponto azul).",
                );
            }
            2 => {
                ui.colored_label(
                    egui::Color32::from_rgb(255, 140, 0),
                    "Clique para definir o ponto de MOVIMENTAÇÃO (laranja).",
                );
            }
            _ => {
                if let Some(pi) = self.pivot_sel.and_then(|i| self.document.pivots.get(i)) {
                    ui.label(format!("Pivô ativo: \"{}\"", pi.name));
                    ui.separator();
                    ui.weak("Arraste laranja = girar • azul/objeto = mover");
                } else {
                    ui.weak("Abra o painel Pivô (ícone à direita) para criar ou escolher um pivô.");
                }
            }
        }
    }

    /// Painel de gerenciamento de pivôs (criar / listar / editar / excluir).
    fn janela_pivo(&mut self, ctx: &egui::Context) {
        if !self.win_pivot {
            return;
        }
        let mut open = self.win_pivot;
        let mut do_criar = false;
        let mut do_add_ponto = false;
        let mut do_excluir = false;
        let mut do_renomear = false;
        egui::Window::new("Pivô")
            .open(&mut open)
            .default_width(240.0)
            .show(ctx, |ui| {
                if self.pivot_naming {
                    ui.label("Nome do pivô:");
                    let resp = ui.text_edit_singleline(&mut self.pivot_name_buf);
                    resp.request_focus();
                    ui.horizontal(|ui| {
                        let ok = ui.button("Confirmar").clicked()
                            || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                        if ok {
                            do_criar = true;
                        }
                        if ui.button("Cancelar").clicked() {
                            self.pivot_naming = false;
                            self.pivot_name_buf.clear();
                        }
                    });
                } else if ui.button("＋ Criar novo").clicked() {
                    self.pivot_naming = true;
                    self.pivot_name_buf = format!("Pivô {}", self.document.pivots.len() + 1);
                }
                ui.separator();
                ui.label("Pivôs existentes:");
                egui::ScrollArea::vertical()
                    .max_height(180.0)
                    .show(ui, |ui| {
                        if self.document.pivots.is_empty() {
                            ui.weak("(nenhum ainda)");
                        }
                        for i in 0..self.document.pivots.len() {
                            let (nome, on, completo) = {
                                let p = &self.document.pivots[i];
                                (p.name.clone(), p.enabled, p.completo())
                            };
                            ui.horizontal(|ui| {
                                let sel = self.pivot_sel == Some(i);
                                let rotulo = if completo {
                                    format!("● {nome}")
                                } else {
                                    format!("○ {nome} (incompleto)")
                                };
                                if ui.selectable_label(sel, rotulo).clicked() {
                                    self.pivot_sel = Some(i);
                                    self.pivot_stage = 0;
                                }
                                let mut en = on;
                                if ui.checkbox(&mut en, "").on_hover_text("Ativar/desativar").changed()
                                {
                                    self.document.pivots[i].enabled = en;
                                }
                            });
                        }
                    });
                ui.separator();
                let tem_sel = self.pivot_sel.is_some();
                if self.pivot_sel.map_or(false, |i| i >= self.document.pivots.len()) {
                    self.pivot_sel = None;
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(tem_sel, egui::Button::new("＋ Adicionar ponto"))
                        .on_hover_text("Redefinir eixo e ponto de movimentação clicando no objeto")
                        .clicked()
                    {
                        do_add_ponto = true;
                    }
                });
                ui.horizontal(|ui| {
                    if ui.add_enabled(tem_sel, egui::Button::new("Editar")).clicked() {
                        do_renomear = true;
                    }
                    if ui.add_enabled(tem_sel, egui::Button::new("Excluir")).clicked() {
                        do_excluir = true;
                    }
                });
                if self.pivot_stage != 0 {
                    ui.separator();
                    ui.colored_label(
                        egui::Color32::from_rgb(200, 120, 0),
                        "Configurando: clique no canvas para posicionar os pontos.",
                    );
                }
            });
        // Ações fora do closure (evita conflito de empréstimo).
        if do_criar {
            let nome = if self.pivot_name_buf.trim().is_empty() {
                format!("Pivô {}", self.document.pivots.len() + 1)
            } else {
                self.pivot_name_buf.trim().to_string()
            };
            if self.pivot_stage == 3 {
                // "Editar": apenas renomeia o pivô selecionado (mantém pontos).
                if let Some(pi) = self.pivot_sel {
                    if let Some(pv) = self.document.pivots.get_mut(pi) {
                        pv.name = nome;
                    }
                }
                self.pivot_naming = false;
                self.pivot_name_buf.clear();
                self.pivot_stage = 0;
                self.modificado = true;
                self.status = "Nome do pivô atualizado.".into();
            } else {
                self.document.pivots.push(sketchmotion_core::Pivot::new(nome));
                self.pivot_sel = Some(self.document.pivots.len() - 1);
                self.pivot_naming = false;
                self.pivot_name_buf.clear();
                self.pivot_stage = 1;
                self.tool = Tool::Pivot;
                self.modificado = true;
                self.status = "Clique no objeto para definir o eixo (ponto azul).".into();
            }
        }
        if do_add_ponto {
            if let Some(pi) = self.pivot_sel {
                if let Some(pv) = self.document.pivots.get_mut(pi) {
                    pv.has_axis = false;
                    pv.has_mov = false;
                }
                self.pivot_stage = 1;
                self.tool = Tool::Pivot;
                self.status = "Clique no objeto para redefinir o eixo (ponto azul).".into();
            }
        }
        if do_renomear {
            if let Some(pi) = self.pivot_sel {
                self.pivot_naming = true;
                self.pivot_name_buf = self
                    .document
                    .pivots
                    .get(pi)
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                // "Editar" aqui reaproveita o campo de nome; renomeia ao confirmar.
                // Para não criar um novo, sinalizamos via pivot_stage especial.
                self.pivot_stage = 3;
            }
        }
        if do_excluir {
            if let Some(pi) = self.pivot_sel {
                if pi < self.document.pivots.len() {
                    self.document.pivots.remove(pi);
                    self.pivot_sel = None;
                    self.pivot_stage = 0;
                    self.modificado = true;
                    self.status = "Pivô excluído (o objeto foi mantido).".into();
                }
            }
        }
        self.win_pivot = open;
    }

    // ==================  Ferramenta Vetor de Direção  ==================

    /// Centro (bounds) dos vetores da cópia de trabalho que pertencem ao grupo.
    fn dirvec_group_centroid(&self, g: u32) -> Option<(f32, f32)> {
        let mut bb: Option<(f32, f32, f32, f32)> = None;
        for o in self.document.vectors.iter().filter(|o| o.group == Some(g)) {
            if let Some((a, b, c, d)) = o.bounds() {
                bb = Some(match bb {
                    Some((x0, y0, x1, y1)) => (x0.min(a), y0.min(b), x1.max(c), y1.max(d)),
                    None => (a, b, c, d),
                });
            }
        }
        bb.map(|(a, b, c, d)| ((a + c) / 2.0, (b + d) / 2.0))
    }

    /// Índice de um pivô completo cujo alvo casa com `target` (integração Pivô).
    fn dirvec_find_pivot(&self, target: &sketchmotion_core::PivotTarget) -> Option<usize> {
        self.document
            .pivots
            .iter()
            .position(|p| p.completo() && &p.target == target)
    }

    /// Captura o ESTADO INICIAL do vetor `vi` no frame atual: geometria-base,
    /// centro, e (se houver pivô associado) o eixo e o ângulo inicial.
    fn dirvec_definir_inicio(&mut self, vi: usize) {
        use sketchmotion_core::PivotTarget;
        let target = match self.document.dir_vectors.get(vi) {
            Some(v) => v.target.clone(),
            None => return,
        };
        let pivot = self.dirvec_find_pivot(&target);
        // start_angle é sempre 0 (base = geometria inicial); a rotação vem do
        // arraste (acumulada em end_angle). O eixo, se houver pivô, é o centro.
        let (has_p, axis) = match pivot {
            Some(pi) => {
                let p = &self.document.pivots[pi];
                (true, p.axis)
            }
            None => (false, (0.0, 0.0)),
        };
        let cur = self.document.current;
        match target {
            PivotTarget::Group(g) => {
                let base: Vec<_> = self
                    .document
                    .vectors
                    .iter()
                    .filter(|o| o.group == Some(g))
                    .cloned()
                    .collect();
                if base.is_empty() {
                    self.status = "Objeto do vetor não está neste frame.".into();
                    return;
                }
                let centroid = self.dirvec_group_centroid(g).unwrap_or((0.0, 0.0));
                // Seleciona a peça (feedback: o usuário vê com o que trabalha).
                self.sel_set = (0..self.document.vectors.len())
                    .filter(|&k| self.document.vectors[k].group == Some(g))
                    .collect();
                self.selected_obj = self.sel_set.first().copied();
                let v = &mut self.document.dir_vectors[vi];
                v.base_vectors = base;
                v.base_image = None;
                v.start_centroid = centroid;
                v.start_angle = 0.0;
                v.end_angle = 0.0;
                v.has_pivot = has_p;
                v.pivot_axis = axis;
                v.start_frame = cur;
                v.has_start = true;
                v.has_end = false;
            }
            PivotTarget::Image(i) => {
                let Some(o) = self.document.images.get(i).cloned() else {
                    self.status = "Imagem do vetor não está neste frame.".into();
                    return;
                };
                self.sel_set.clear();
                self.selected_obj = None;
                let v = &mut self.document.dir_vectors[vi];
                v.start_centroid = (o.cx, o.cy);
                v.base_image = Some(o);
                v.base_image_index = i;
                v.base_vectors = Vec::new();
                v.start_angle = 0.0;
                v.end_angle = 0.0;
                v.has_pivot = has_p;
                v.pivot_axis = axis;
                v.start_frame = cur;
                v.has_start = true;
                v.has_end = false;
            }
            PivotTarget::None => {
                self.status = "Vincule um objeto ao vetor primeiro.".into();
                return;
            }
        }
        // Força remontar a textura do fantasma (a base pode ter mudado).
        self.dirvec_ghost_tex = None;
        self.dirvec_ghost_tex_for = None;
        self.modificado = true;
        self.status = format!("Estado inicial fixado no frame {}.", cur + 1);
    }

    /// Captura o ESTADO FINAL do vetor `vi` no frame atual (posição/ângulo já
    /// alterados pelo usuário).
    fn dirvec_definir_fim(&mut self, vi: usize) {
        use sketchmotion_core::PivotTarget;
        let target = match self.document.dir_vectors.get(vi) {
            Some(v) => v.target.clone(),
            None => return,
        };
        let ang1 = match self.dirvec_find_pivot(&target) {
            Some(pi) => self.document.pivots[pi].angulo(),
            None => self
                .document
                .dir_vectors
                .get(vi)
                .map(|v| v.start_angle)
                .unwrap_or(0.0),
        };
        let cur = self.document.current;
        let centroid = match &target {
            PivotTarget::Group(g) => self.dirvec_group_centroid(*g),
            PivotTarget::Image(i) => self.document.images.get(*i).map(|o| (o.cx, o.cy)),
            PivotTarget::None => None,
        };
        let Some(centroid) = centroid else {
            self.status = "Objeto do vetor não está neste frame.".into();
            return;
        };
        let v = &mut self.document.dir_vectors[vi];
        v.end_centroid = centroid;
        v.end_angle = ang1;
        v.end_frame = cur;
        v.has_end = true;
        self.modificado = true;
        self.status = format!("Estado final fixado no frame {}.", cur + 1);
    }

    /// Aplica a transformação interpolada de `dv` à geometria-base para o
    /// parâmetro `t` (vetores).
    fn dirvec_transformar_vetores(
        base: &mut [sketchmotion_core::VectorObject],
        dv: &sketchmotion_core::DirVector,
        t: f32,
    ) {
        let s = dv.scale_start + (dv.scale_end - dv.scale_start) * t;
        if dv.has_pivot {
            let ang = (dv.end_angle - dv.start_angle) * t;
            for o in base.iter_mut() {
                o.rotate_around(dv.pivot_axis.0, dv.pivot_axis.1, ang);
                if (s - 1.0).abs() > 1e-4 {
                    o.scale_around(dv.pivot_axis.0, dv.pivot_axis.1, s);
                }
            }
        } else {
            let ang = (dv.end_angle - dv.start_angle) * t;
            let (cx, cy) = dv.start_centroid;
            let dx = (dv.end_centroid.0 - dv.start_centroid.0) * t;
            let dy = (dv.end_centroid.1 - dv.start_centroid.1) * t;
            for o in base.iter_mut() {
                if ang.abs() > 1e-6 {
                    o.rotate_around(cx, cy, ang);
                }
                if (s - 1.0).abs() > 1e-4 {
                    o.scale_around(cx, cy, s);
                }
                o.translate(dx, dy);
            }
        }
    }

    /// Idem para um objeto de imagem.
    fn dirvec_transformar_imagem(
        base: &mut sketchmotion_core::ImageObject,
        dv: &sketchmotion_core::DirVector,
        t: f32,
    ) {
        let s = dv.scale_start + (dv.scale_end - dv.scale_start) * t;
        if dv.has_pivot {
            let ang = (dv.end_angle - dv.start_angle) * t;
            let (sn, c) = ang.sin_cos();
            let (dx, dy) = (base.cx - dv.pivot_axis.0, base.cy - dv.pivot_axis.1);
            base.cx = dv.pivot_axis.0 + dx * c - dy * sn;
            base.cy = dv.pivot_axis.1 + dx * sn + dy * c;
            base.angle += ang;
        } else {
            let ang = (dv.end_angle - dv.start_angle) * t;
            base.angle += ang;
            base.cx += (dv.end_centroid.0 - dv.start_centroid.0) * t;
            base.cy += (dv.end_centroid.1 - dv.start_centroid.1) * t;
        }
        if (s - 1.0).abs() > 1e-4 {
            base.hw *= s;
            base.hh *= s;
        }
    }

    /// Gera (bake) os frames intermediários do vetor `vi` a partir do estado
    /// inicial + final + interpolação. Não destrutivo na origem: regenera sempre
    /// a partir da geometria-base guardada.
    fn dirvec_aplicar(&mut self, vi: usize) {
        use sketchmotion_core::PivotTarget;
        let dv = match self.document.dir_vectors.get(vi) {
            Some(v) if v.completo() => v.clone(),
            _ => {
                self.status = "Defina o estado inicial e o final (em frames diferentes).".into();
                return;
            }
        };
        self.push_undo();
        self.document.sync_to_frames();
        let (a, b) = dv.faixa();
        let b = b.min(self.document.frames.len().saturating_sub(1));
        for f in a..=b {
            let t = dv.t_para_frame(f);
            match &dv.target {
                PivotTarget::Group(g) => {
                    let mut base = dv.base_vectors.clone();
                    Self::dirvec_transformar_vetores(&mut base, &dv, t);
                    let mut fv = self.document.frame_vectors(f);
                    fv.retain(|o| o.group != Some(*g));
                    fv.extend(base);
                    self.document.set_frame_vectors(f, fv);
                }
                PivotTarget::Image(_) => {
                    if let Some(mut base) = dv.base_image.clone() {
                        Self::dirvec_transformar_imagem(&mut base, &dv, t);
                        let mut fi = self
                            .document
                            .frames
                            .get(f)
                            .map(|fr| fr.images.clone())
                            .unwrap_or_default();
                        let idx = dv.base_image_index;
                        if idx < fi.len() {
                            fi[idx] = base;
                        } else {
                            fi.push(base);
                        }
                        self.document.set_frame_images(f, fi);
                    }
                }
                PivotTarget::None => {}
            }
        }
        self.document.reload_working();
        self.dirty = true;
        self.modificado = true;
        self.status = format!("Animação gerada nos frames {}–{}.", a + 1, b + 1);
    }

    /// Interação da ferramenta no canvas: durante a criação, clicar vincula o
    /// objeto sob o cursor e fixa o estado inicial.
    /// O alvo existe na cópia de trabalho (frame) atual?
    fn dirvec_target_in_frame(&self, target: &sketchmotion_core::PivotTarget) -> bool {
        use sketchmotion_core::PivotTarget;
        match target {
            PivotTarget::Group(g) => self.document.vectors.iter().any(|o| o.group == Some(*g)),
            PivotTarget::Image(i) => *i < self.document.images.len(),
            PivotTarget::None => false,
        }
    }

    /// Mantém o estado do "fantasma": quando a ferramenta está ativa, há um vetor
    /// com início definido, e o frame atual NÃO é o do início E o objeto não está
    /// neste frame, prepara um fantasma (na posição inicial) para ser arrastado.
    fn dirvec_tick_ghost(&mut self) {
        if self.tool != Tool::DirVector {
            self.dirvec_ghost_frame = None;
            return;
        }
        let Some(vi) = self.dirvec_sel else {
            self.dirvec_ghost_frame = None;
            return;
        };
        let (has_start, start_frame, start_centroid, target) = {
            match self.document.dir_vectors.get(vi) {
                Some(v) => (v.has_start, v.start_frame, v.start_centroid, v.target.clone()),
                None => {
                    self.dirvec_ghost_frame = None;
                    return;
                }
            }
        };
        let cur = self.document.current;
        let present = self.dirvec_target_in_frame(&target);
        if !has_start || cur == start_frame || present {
            self.dirvec_ghost_frame = None;
            return;
        }
        // Precisa de fantasma. Se acabou de mudar de frame (e não está no meio de
        // um arraste), (re)posiciona-o no estado inicial.
        if self.dirvec_ghost_frame != Some(cur) && !self.dirvec_ghost_drag {
            self.dirvec_ghost_pos = start_centroid;
            self.dirvec_ghost_ang = 0.0;
            self.dirvec_ghost_frame = Some(cur);
        }
    }

    /// Monta (uma vez) a textura do fantasma a partir da imagem-base do vetor,
    /// para desenhar a própria peça em suspensão.
    fn dirvec_prepara_ghost_tex(&mut self, ctx: &egui::Context) {
        if self.dirvec_ghost_frame.is_none() {
            return;
        }
        let Some(vi) = self.dirvec_sel else { return };
        if self.dirvec_ghost_tex.is_some() && self.dirvec_ghost_tex_for == Some(vi) {
            return;
        }
        let Some(v) = self.document.dir_vectors.get(vi) else {
            return;
        };
        if let Some(im) = &v.base_image {
            if im.ow > 0 && im.oh > 0 && im.pixels.len() == (im.ow * im.oh * 4) as usize {
                let ci = egui::ColorImage::from_rgba_unmultiplied(
                    [im.ow as usize, im.oh as usize],
                    &im.pixels,
                );
                self.dirvec_ghost_tex =
                    Some(ctx.load_texture(format!("dv_ghost_{vi}"), ci, egui::TextureOptions::LINEAR));
                self.dirvec_ghost_tex_for = Some(vi);
            }
        } else {
            self.dirvec_ghost_tex = None;
            self.dirvec_ghost_tex_for = None;
        }
    }

    /// O ponto (doc) está sobre o objeto do alvo, na cópia de trabalho atual?
    fn dirvec_target_under_point(&self, target: &sketchmotion_core::PivotTarget, dp: (f32, f32)) -> bool {
        use sketchmotion_core::PivotTarget;
        match target {
            PivotTarget::Group(g) => self.document.vectors.iter().any(|o| {
                o.group == Some(*g)
                    && o.bounds().map_or(false, |(a, b, c, d)| {
                        dp.0 >= a - 2.0 && dp.0 <= c + 2.0 && dp.1 >= b - 2.0 && dp.1 <= d + 2.0
                    })
            }),
            PivotTarget::Image(i) => {
                if let Some(o) = self.document.images.get(*i) {
                    let (s, c) = (-o.angle).sin_cos();
                    let (rx, ry) = (dp.0 - o.cx, dp.1 - o.cy);
                    let lx = rx * c - ry * s;
                    let ly = rx * s + ry * c;
                    lx.abs() <= o.hw && ly.abs() <= o.hh
                } else {
                    false
                }
            }
            PivotTarget::None => false,
        }
    }

    /// Translada o alvo na cópia de trabalho atual (arraste do Vetor de Direção).
    fn dirvec_target_translate(&mut self, target: &sketchmotion_core::PivotTarget, dx: f32, dy: f32) {
        use sketchmotion_core::PivotTarget;
        match target {
            PivotTarget::Group(g) => {
                for o in self.document.vectors.iter_mut() {
                    if o.group == Some(*g) {
                        o.translate(dx, dy);
                    }
                }
            }
            PivotTarget::Image(i) => {
                if let Some(o) = self.document.images.get_mut(*i) {
                    o.cx += dx;
                    o.cy += dy;
                }
            }
            PivotTarget::None => {}
        }
    }

    /// Rotaciona o alvo ao redor de um centro (arraste orbital com pivô).
    fn dirvec_target_rotate(&mut self, target: &sketchmotion_core::PivotTarget, cx: f32, cy: f32, ang: f32) {
        use sketchmotion_core::PivotTarget;
        match target {
            PivotTarget::Group(g) => {
                for o in self.document.vectors.iter_mut() {
                    if o.group == Some(*g) {
                        o.rotate_around(cx, cy, ang);
                    }
                }
            }
            PivotTarget::Image(i) => {
                if let Some(o) = self.document.images.get_mut(*i) {
                    let (s, c) = ang.sin_cos();
                    let (dx, dy) = (o.cx - cx, o.cy - cy);
                    o.cx = cx + dx * c - dy * s;
                    o.cy = cy + dx * s + dy * c;
                    o.angle += ang;
                }
            }
            PivotTarget::None => {}
        }
    }

    /// Interação da ferramenta: manual e direta.
    /// - Modo "vincular" (após criar/Selecionar objeto): o clique escolhe a peça,
    ///   fixa o início no frame atual e a deixa selecionada.
    /// - Depois: basta ARRASTAR o objeto (neste frame ou em outro). Ao soltar num
    ///   frame diferente do início, o fim é gravado e a animação é gerada sozinha.
    fn interacao_dirvec(
        &mut self,
        pressed: bool,
        down: bool,
        hover: Option<egui::Pos2>,
        ppos: Option<egui::Pos2>,
        pdelta: egui::Vec2,
        rect: egui::Rect,
        zoom: f32,
    ) {
        let to_doc = |p: egui::Pos2| ((p.x - rect.min.x) / zoom, (p.y - rect.min.y) / zoom);

        // --- Modo FANTASMA: o objeto está "em suspensão" neste frame (ainda não
        // existe aqui). Arrastar em qualquer ponto move o fantasma; ao soltar,
        // grava o fim e gera a animação. ---
        let ghost_ativo =
            self.dirvec_stage == 0 && self.dirvec_ghost_frame == Some(self.document.current);
        if ghost_ativo {
            if pressed {
                self.dirvec_ghost_drag = true;
            }
            if down && self.dirvec_ghost_drag {
                if let Some(vi) = self.dirvec_sel {
                    let (has_pivot, axis) = {
                        let v = &self.document.dir_vectors[vi];
                        (v.has_pivot, v.pivot_axis)
                    };
                    if has_pivot {
                        if let Some(pp) = ppos {
                            let cur = to_doc(pp);
                            let prev = (cur.0 - pdelta.x / zoom, cur.1 - pdelta.y / zoom);
                            let a0 = (prev.1 - axis.1).atan2(prev.0 - axis.0);
                            let a1 = (cur.1 - axis.1).atan2(cur.0 - axis.0);
                            let d = a1 - a0;
                            if d.abs() > f32::EPSILON {
                                self.dirvec_ghost_ang += d;
                                self.dirty = true;
                            }
                        }
                    } else {
                        let (dx, dy) = (pdelta.x / zoom, pdelta.y / zoom);
                        if dx != 0.0 || dy != 0.0 {
                            self.dirvec_ghost_pos.0 += dx;
                            self.dirvec_ghost_pos.1 += dy;
                            self.dirty = true;
                        }
                    }
                }
            }
            if !down && self.dirvec_ghost_drag {
                self.dirvec_ghost_drag = false;
                if let Some(vi) = self.dirvec_sel {
                    let hp = self.document.dir_vectors[vi].has_pivot;
                    let cur = self.document.current;
                    {
                        let v = &mut self.document.dir_vectors[vi];
                        if hp {
                            v.end_angle = self.dirvec_ghost_ang;
                        } else {
                            v.end_centroid = self.dirvec_ghost_pos;
                        }
                        v.end_frame = cur;
                        v.has_end = true;
                    }
                    self.dirvec_aplicar(vi);
                    self.dirvec_ghost_frame = None; // objeto agora existe aqui
                }
            }
            return;
        }

        if pressed {
            if let Some(p) = hover {
                let dp = to_doc(p);
                // Vincular o objeto (modo criação/Selecionar).
                if self.dirvec_stage == 1 {
                    if let Some(vi) = self.dirvec_sel {
                        let target = self.pivot_hit_target(dp);
                        if matches!(target, sketchmotion_core::PivotTarget::None) {
                            self.status = "Clique sobre uma forma, imagem ou objeto vetorial (traço a lápis é pixel — use Formas ou importe imagem).".into();
                        } else {
                            self.document.dir_vectors[vi].target = target;
                            self.dirvec_stage = 0;
                            self.dirvec_definir_inicio(vi);
                            self.status = "Objeto selecionado e início fixado aqui. Vá ao frame final e ARRASTE o objeto para a posição desejada.".into();
                        }
                    }
                    return;
                }
                // Ocioso: começa a arrastar se clicou sobre o objeto do vetor.
                if let Some(vi) = self.dirvec_sel {
                    let target = self.document.dir_vectors[vi].target.clone();
                    if self.document.dir_vectors[vi].has_start
                        && self.dirvec_target_under_point(&target, dp)
                    {
                        self.dirvec_dragging = true;
                        self.dirvec_drag_angle = 0.0;
                        self.push_undo();
                    }
                }
            }
        }
        if down && self.dirvec_dragging {
            if let (Some(pp), Some(vi)) = (ppos, self.dirvec_sel) {
                let (target, has_pivot, axis) = {
                    let v = &self.document.dir_vectors[vi];
                    (v.target.clone(), v.has_pivot, v.pivot_axis)
                };
                if has_pivot {
                    // Arraste orbital: gira ao redor do eixo pelo ângulo varrido.
                    let cur = to_doc(pp);
                    let prev = (cur.0 - pdelta.x / zoom, cur.1 - pdelta.y / zoom);
                    let a0 = (prev.1 - axis.1).atan2(prev.0 - axis.0);
                    let a1 = (cur.1 - axis.1).atan2(cur.0 - axis.0);
                    let delta = a1 - a0;
                    if delta.abs() > f32::EPSILON {
                        self.dirvec_target_rotate(&target, axis.0, axis.1, delta);
                        self.dirvec_drag_angle += delta;
                        self.dirty = true;
                    }
                } else {
                    let (dx, dy) = (pdelta.x / zoom, pdelta.y / zoom);
                    if dx != 0.0 || dy != 0.0 {
                        self.dirvec_target_translate(&target, dx, dy);
                        self.dirty = true;
                    }
                }
            }
        }
        if !down && self.dirvec_dragging {
            self.dirvec_dragging = false;
            if let Some(vi) = self.dirvec_sel {
                let (sf, hp) = {
                    let v = &self.document.dir_vectors[vi];
                    (v.start_frame, v.has_pivot)
                };
                let cur = self.document.current;
                if cur != sf {
                    // Frame diferente do início = estado FINAL. Grava e gera tudo.
                    if hp {
                        let v = &mut self.document.dir_vectors[vi];
                        v.end_angle = self.dirvec_drag_angle;
                        v.end_frame = cur;
                        v.has_end = true;
                    } else {
                        self.dirvec_definir_fim(vi);
                    }
                    self.dirvec_aplicar(vi);
                } else {
                    // Mesmo frame do início: apenas reposiciona o início.
                    self.dirvec_definir_inicio(vi);
                    self.status =
                        "Início reposicionado. Vá a outro frame e arraste o objeto para o fim.".into();
                }
            }
        }
    }

    /// Desenha a trajetória do vetor selecionado (linha reta ou arco orbital).
    /// Apenas overlay de interface (não entra na exportação).
    fn desenhar_dirvec(&self, ui: &mut egui::Ui, rect: egui::Rect, zoom: f32) {
        let Some(vi) = self.dirvec_sel else { return };
        let Some(dv) = self.document.dir_vectors.get(vi) else {
            return;
        };
        if !dv.has_start {
            return;
        }
        let painter = ui.painter_at(rect);
        let scr = |p: (f32, f32)| egui::pos2(rect.min.x + p.0 * zoom, rect.min.y + p.1 * zoom);
        let verde = egui::Color32::from_rgb(0x3A, 0xC0, 0x50);
        let ini = scr(dv.start_centroid);
        if dv.has_pivot {
            // Arco orbital ao redor do eixo, do ângulo inicial ao final.
            let ax = scr(dv.pivot_axis);
            let r = ((dv.start_centroid.0 - dv.pivot_axis.0).powi(2)
                + (dv.start_centroid.1 - dv.pivot_axis.1).powi(2))
            .sqrt();
            painter.circle_stroke(ax, 4.0, egui::Stroke::new(1.5, egui::Color32::from_rgb(30, 120, 255)));
            if dv.has_end && r > 0.5 {
                let steps = 32;
                let mut prev = None;
                for k in 0..=steps {
                    let t = k as f32 / steps as f32;
                    let ang = dv.start_angle + (dv.end_angle - dv.start_angle) * t;
                    let pt = (
                        dv.pivot_axis.0 + r * ang.cos(),
                        dv.pivot_axis.1 + r * ang.sin(),
                    );
                    let sp = scr(pt);
                    if let Some(pp) = prev {
                        painter.line_segment([pp, sp], egui::Stroke::new(2.0, verde));
                    }
                    prev = Some(sp);
                }
            }
        } else if dv.has_end {
            let fim = scr(dv.end_centroid);
            painter.line_segment([ini, fim], egui::Stroke::new(2.0, verde));
            // Setinha no fim.
            let dir = fim - ini;
            let len = dir.length().max(1.0);
            let d = dir / len;
            let n = egui::vec2(-d.y, d.x);
            painter.line_segment([fim, fim - d * 10.0 + n * 5.0], egui::Stroke::new(2.0, verde));
            painter.line_segment([fim, fim - d * 10.0 - n * 5.0], egui::Stroke::new(2.0, verde));
            painter.circle_filled(fim, 4.0, verde);
        }
        painter.circle_filled(ini, 4.0, egui::Color32::from_rgb(30, 120, 255));

        // Fantasma: o objeto em suspensão neste frame, à espera do arraste.
        if self.dirvec_ghost_frame == Some(self.document.current) {
            let laranja = egui::Color32::from_rgb(255, 150, 40);
            // Transforma a geometria-base para a posição/ângulo atuais do fantasma.
            let (dx, dy) = (
                self.dirvec_ghost_pos.0 - dv.start_centroid.0,
                self.dirvec_ghost_pos.1 - dv.start_centroid.1,
            );
            let (sn, cs) = self.dirvec_ghost_ang.sin_cos();
            let (ax, ay) = dv.pivot_axis;
            let transf = |x: f32, y: f32| -> (f32, f32) {
                if dv.has_pivot {
                    let (rx, ry) = (x - ax, y - ay);
                    (ax + rx * cs - ry * sn, ay + rx * sn + ry * cs)
                } else {
                    (x + dx, y + dy)
                }
            };
            if !dv.base_vectors.is_empty() {
                for o in &dv.base_vectors {
                    let flat = o.flatten(20);
                    if flat.len() >= 2 {
                        let pts: Vec<egui::Pos2> =
                            flat.iter().map(|&(x, y)| scr(transf(x, y))).collect();
                        painter.add(egui::Shape::line(
                            pts,
                            egui::Stroke::new(2.0, laranja),
                        ));
                    }
                }
            } else if let Some(im) = &dv.base_image {
                // Cantos da imagem no espaço do documento (respeitando o ângulo
                // próprio da imagem) e depois a transformação do fantasma.
                let (isn, ics) = im.angle.sin_cos();
                let canto = |sx: f32, sy: f32| -> egui::Pos2 {
                    let (lx, ly) = (sx * im.hw, sy * im.hh);
                    let (wx, wy) = (im.cx + lx * ics - ly * isn, im.cy + lx * isn + ly * ics);
                    scr(transf(wx, wy))
                };
                let c = [
                    canto(-1.0, -1.0),
                    canto(1.0, -1.0),
                    canto(1.0, 1.0),
                    canto(-1.0, 1.0),
                ];
                // Fantasma = a PRÓPRIA imagem, semitransparente (melhor base).
                if let Some(tex) = &self.dirvec_ghost_tex {
                    let tint = egui::Color32::from_white_alpha(160);
                    let uv = [
                        egui::pos2(0.0, 0.0),
                        egui::pos2(1.0, 0.0),
                        egui::pos2(1.0, 1.0),
                        egui::pos2(0.0, 1.0),
                    ];
                    let mut mesh = egui::Mesh::with_texture(tex.id());
                    for i in 0..4 {
                        mesh.vertices.push(egui::epaint::Vertex {
                            pos: c[i],
                            uv: uv[i],
                            color: tint,
                        });
                    }
                    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
                    painter.add(egui::Shape::mesh(mesh));
                }
                // Contorno laranja por cima (destaque de que é editável).
                painter.add(egui::Shape::closed_line(
                    c.to_vec(),
                    egui::Stroke::new(1.5, laranja),
                ));
            }
            let gc = scr(self.dirvec_ghost_pos);
            painter.circle_filled(gc, 4.0, laranja);
        }
    }

    /// Barra de opções (topo) da ferramenta Vetor de Direção.
    fn opcoes_dirvec(&mut self, ui: &mut egui::Ui) {
        if self.dirvec_stage == 1 {
            ui.colored_label(
                egui::Color32::from_rgb(0x3A, 0xC0, 0x50),
                "Clique no objeto para vinculá-lo ao vetor.",
            );
        } else if let Some(v) = self.dirvec_sel.and_then(|i| self.document.dir_vectors.get(i)) {
            ui.label(format!("Vetor ativo: \"{}\"", v.name));
            ui.separator();
            ui.weak("Use o painel para definir início/fim e aplicar.");
        } else {
            ui.weak("Abra o painel Vetor de Direção (ícone à direita) para criar um vetor.");
        }
    }

    /// Painel de gerenciamento dos vetores de direção.
    fn janela_dirvec(&mut self, ctx: &egui::Context) {
        if !self.win_dirvec {
            return;
        }
        let mut open = self.win_dirvec;
        let mut do_criar = false;
        let mut do_vincular = false;
        let mut do_aplicar = false;
        let mut do_excluir = false;
        egui::Window::new("Vetor de Direção")
            .open(&mut open)
            .default_width(260.0)
            .show(ctx, |ui| {
                if self.dirvec_naming {
                    ui.label("Nome do vetor:");
                    let resp = ui.text_edit_singleline(&mut self.dirvec_name_buf);
                    resp.request_focus();
                    ui.horizontal(|ui| {
                        let ok = ui.button("Confirmar").clicked()
                            || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                        if ok {
                            do_criar = true;
                        }
                        if ui.button("Cancelar").clicked() {
                            self.dirvec_naming = false;
                            self.dirvec_name_buf.clear();
                        }
                    });
                } else if ui.button("＋ Adicionar novo vetor").clicked() {
                    self.dirvec_naming = true;
                    self.dirvec_name_buf =
                        format!("Vetor {}", self.document.dir_vectors.len() + 1);
                }
                ui.separator();
                ui.label("Vetores existentes:");
                egui::ScrollArea::vertical()
                    .max_height(140.0)
                    .show(ui, |ui| {
                        if self.document.dir_vectors.is_empty() {
                            ui.weak("(nenhum ainda)");
                        }
                        for i in 0..self.document.dir_vectors.len() {
                            let (nome, on, completo) = {
                                let v = &self.document.dir_vectors[i];
                                (v.name.clone(), v.enabled, v.completo())
                            };
                            ui.horizontal(|ui| {
                                let sel = self.dirvec_sel == Some(i);
                                let rot = if completo {
                                    format!("● {nome}")
                                } else {
                                    format!("○ {nome}")
                                };
                                if ui.selectable_label(sel, rot).clicked() {
                                    self.dirvec_sel = Some(i);
                                }
                                let mut en = on;
                                if ui.checkbox(&mut en, "").changed() {
                                    self.document.dir_vectors[i].enabled = en;
                                }
                            });
                        }
                    });
                ui.separator();
                if self.dirvec_sel.map_or(false, |i| i >= self.document.dir_vectors.len()) {
                    self.dirvec_sel = None;
                }
                if let Some(vi) = self.dirvec_sel {
                    let (sf, ef, hs, he, hp) = {
                        let v = &self.document.dir_vectors[vi];
                        (v.start_frame, v.end_frame, v.has_start, v.has_end, v.has_pivot)
                    };
                    // Passo a passo, manual e direto.
                    if !hs {
                        ui.colored_label(
                            egui::Color32::from_rgb(0x3A, 0xC0, 0x50),
                            "1) Clique em \"Selecionar objeto\" e clique na peça.",
                        );
                    } else if !he {
                        ui.label(format!("Início: frame {} ✓", sf + 1));
                        ui.weak("2) Vá a outro frame e ARRASTE o objeto para o fim.");
                    } else {
                        ui.label(format!("Início: frame {}  →  Fim: frame {} ✓", sf + 1, ef + 1));
                        ui.weak("Animação gerada. Arraste de novo para ajustar o fim.");
                    }
                    if hp {
                        ui.colored_label(
                            egui::Color32::from_rgb(30, 120, 255),
                            "Tem pivô: o arraste gira ao redor do eixo.",
                        );
                    }
                    ui.add_space(4.0);
                    if ui.button("🎯 Selecionar objeto").clicked() {
                        do_vincular = true;
                    }
                    ui.separator();
                    {
                        let v = &mut self.document.dir_vectors[vi];
                        egui::ComboBox::from_label("Suavização")
                            .selected_text(v.interp.label())
                            .show_ui(ui, |ui| {
                                for it in sketchmotion_core::Interp::all() {
                                    ui.selectable_value(&mut v.interp, it, it.label());
                                }
                            });
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(hs && he, egui::Button::new("Recalcular"))
                            .clicked()
                        {
                            do_aplicar = true;
                        }
                        if ui.button("Excluir").clicked() {
                            do_excluir = true;
                        }
                    });
                } else {
                    ui.weak("Crie um vetor com \"＋ Adicionar novo vetor\".");
                }
            });
        // Ações fora do closure.
        if do_criar {
            let nome = if self.dirvec_name_buf.trim().is_empty() {
                format!("Vetor {}", self.document.dir_vectors.len() + 1)
            } else {
                self.dirvec_name_buf.trim().to_string()
            };
            self.document
                .dir_vectors
                .push(sketchmotion_core::DirVector::new(nome));
            self.dirvec_sel = Some(self.document.dir_vectors.len() - 1);
            self.dirvec_naming = false;
            self.dirvec_name_buf.clear();
            self.dirvec_stage = 1;
            self.tool = Tool::DirVector;
            self.modificado = true;
            self.status = "Clique no objeto para vinculá-lo ao vetor.".into();
        }
        if do_vincular {
            self.dirvec_stage = 1;
            self.tool = Tool::DirVector;
            self.status = "Clique na peça que você quer animar.".into();
        }
        if do_aplicar {
            if let Some(vi) = self.dirvec_sel {
                self.dirvec_aplicar(vi);
            }
        }
        if do_excluir {
            if let Some(vi) = self.dirvec_sel {
                if vi < self.document.dir_vectors.len() {
                    self.document.dir_vectors.remove(vi);
                    self.dirvec_sel = None;
                    self.dirvec_stage = 0;
                    self.modificado = true;
                    self.status = "Vetor de direção excluído (o objeto foi mantido).".into();
                }
            }
        }
        self.win_dirvec = open;
    }

    /// Janela de controle da câmera: campos numéricos, keyframes e Reset.
    fn janela_camera(&mut self, ctx: &egui::Context) {
        let mut open = self.win_camera;
        let cur = self.document.current;
        let bw = self.document.camera.base_w.max(1.0);
        let bh = self.document.camera.base_h.max(1.0);
        let mut do_remove = false;
        let mut do_reset = false;
        let mut do_add = false;
        let mut goto: Option<usize> = None;
        let verde = egui::Color32::from_rgb(0x3A, 0xC0, 0x50);
        egui::Window::new("Câmera — enquadramento")
            .open(&mut open)
            .default_width(320.0)
            .show(ctx, |ui| {
                ui.label(format!("Frame atual: {}", cur + 1));
                let has_idx = self.document.camera.keyframe_index(cur);
                if has_idx.is_some() {
                    ui.colored_label(verde, "● keyframe neste frame");
                } else {
                    ui.weak("mexa em qualquer valor para criar um keyframe aqui");
                }
                ui.separator();

                {
                    // Auto-keyframe: mexer em QUALQUER valor cria/atualiza o
                    // keyframe deste frame (sem botão extra). Sem keyframe aqui,
                    // parte do valor interpolado atual.
                    let mut k = match has_idx {
                        Some(i) => self.document.camera.keyframes[i],
                        None => {
                            let s = self.document.camera.sample(cur);
                            CameraKeyframe {
                                frame: cur,
                                x: s.x,
                                y: s.y,
                                w: s.w,
                                h: s.h,
                                rotation: s.rotation,
                                anchor_x: s.anchor_x,
                                anchor_y: s.anchor_y,
                                interp: self.cam_interp,
                            }
                        }
                    };
                    let mut changed = false;
                    egui::Grid::new("cam_grid")
                        .num_columns(2)
                        .spacing([8.0, 4.0])
                        .show(ui, |ui| {
                            ui.label("Position X");
                            changed |= ui.add(egui::DragValue::new(&mut k.x).speed(1.0)).changed();
                            ui.end_row();
                            ui.label("Position Y");
                            changed |= ui.add(egui::DragValue::new(&mut k.y).speed(1.0)).changed();
                            ui.end_row();
                            ui.label("Zoom");
                            let mut zoom = (bw / k.w.max(1.0)) * 100.0;
                            if ui
                                .add(
                                    egui::DragValue::new(&mut zoom)
                                        .speed(1.0)
                                        .range(5.0..=2000.0)
                                        .suffix(" %"),
                                )
                                .changed()
                            {
                                let z = (zoom / 100.0).max(0.01);
                                k.w = bw / z;
                                k.h = bh / z;
                                changed = true;
                            }
                            ui.end_row();
                            ui.label("Rotation");
                            changed |= ui
                                .add(egui::DragValue::new(&mut k.rotation).speed(1.0).suffix(" °"))
                                .changed();
                            ui.end_row();
                            ui.label("Largura");
                            changed |= ui
                                .add(
                                    egui::DragValue::new(&mut k.w)
                                        .speed(1.0)
                                        .range(1.0..=20000.0),
                                )
                                .changed();
                            ui.end_row();
                            ui.label("Altura");
                            changed |= ui
                                .add(
                                    egui::DragValue::new(&mut k.h)
                                        .speed(1.0)
                                        .range(1.0..=20000.0),
                                )
                                .changed();
                            ui.end_row();
                            ui.label("Âncora X");
                            changed |= ui
                                .add(egui::DragValue::new(&mut k.anchor_x).speed(1.0))
                                .changed();
                            ui.end_row();
                            ui.label("Âncora Y");
                            changed |= ui
                                .add(egui::DragValue::new(&mut k.anchor_y).speed(1.0))
                                .changed();
                            ui.end_row();
                            ui.label("Interpolação");
                            egui::ComboBox::from_id_salt("cam_interp")
                                .selected_text(k.interp.label())
                                .show_ui(ui, |ui| {
                                    for it in Interp::all() {
                                        if ui
                                            .selectable_value(&mut k.interp, it, it.label())
                                            .clicked()
                                        {
                                            changed = true;
                                        }
                                    }
                                });
                            ui.end_row();
                        });
                    if changed {
                        k.frame = cur;
                        self.cam_interp = k.interp;
                        self.document.camera.set_keyframe(k);
                        self.dirty = true;
                    }
                }

                ui.separator();
                ui.horizontal(|ui| {
                    if has_idx.is_some() {
                        if ui.button("Remover keyframe").clicked() {
                            do_remove = true;
                        }
                    } else if ui
                        .button("Fixar keyframe aqui")
                        .on_hover_text("Cria um keyframe com o enquadramento atual, sem alterar valores")
                        .clicked()
                    {
                        do_add = true;
                    }
                    if ui.button("Reset Camera").clicked() {
                        do_reset = true;
                    }
                });

                ui.separator();
                let n = self.document.camera.keyframes.len();
                ui.label(format!("Keyframes ({n}) — clique para ir ao frame:"));
                let frames: Vec<usize> =
                    self.document.camera.keyframes.iter().map(|k| k.frame + 1).collect();
                ui.horizontal_wrapped(|ui| {
                    for f in frames {
                        let atual = f == cur + 1;
                        if ui.selectable_label(atual, format!("f{f}")).clicked() {
                            goto = Some(f - 1);
                        }
                    }
                });
                ui.weak(
                    "Como usar: num frame, mexa nos valores (ex.: Zoom) — vira keyframe. \
                     Vá para outro frame e mude de novo — o movimento entre eles é \
                     interpolado sozinho. Rode a animação com 'Ver pela câmera' ligado.",
                );
            });
        if let Some(g) = goto {
            self.document.go_to_frame(g);
            self.dirty = true;
        }
        if do_add {
            self.camera_keyframe_atual();
        }
        if do_remove {
            self.document.camera.remove_keyframe(cur);
            self.dirty = true;
            self.status = "Keyframe de câmera removido".into();
        }
        if do_reset {
            let (w, h) = (self.document.width, self.document.height);
            self.document.camera.reset(w, h);
            self.dirty = true;
            self.status = "Câmera resetada (enquadramento = canvas)".into();
        }
        self.win_camera = open;
    }

    /// Desenha o retângulo azul da câmera (estado amostrado no frame atual).
    fn desenhar_camera(&self, ui: &mut egui::Ui, rect: egui::Rect, zoom: f32) {
        let s = self.document.camera.sample(self.document.current);
        let painter = ui.painter_at(rect);
        let blue = egui::Color32::from_rgb(0x2F, 0x84, 0xFE);
        let ang = s.rotation.to_radians();
        let (sin, cos) = ang.sin_cos();
        let (hw, hh) = (s.w * 0.5, s.h * 0.5);
        let corner = |sx: f32, sy: f32| {
            let (lx, ly) = (sx * hw, sy * hh);
            let dx = s.x + lx * cos - ly * sin;
            let dy = s.y + lx * sin + ly * cos;
            egui::pos2(rect.min.x + dx * zoom, rect.min.y + dy * zoom)
        };
        let c = [
            corner(-1.0, -1.0),
            corner(1.0, -1.0),
            corner(1.0, 1.0),
            corner(-1.0, 1.0),
        ];
        for i in 0..4 {
            painter.line_segment(
                [c[i], c[(i + 1) % 4]],
                egui::Stroke::new(2.0_f32, blue),
            );
        }
        for p in c {
            let hr = egui::Rect::from_center_size(p, egui::vec2(8.0, 8.0));
            painter.rect_filled(hr, 1.0, egui::Color32::WHITE);
            painter.rect_stroke(hr, 1.0, egui::Stroke::new(1.0_f32, blue));
        }
        // Alças laterais (meios das bordas) — largura/altura independentes.
        let edges = [
            egui::pos2((c[0].x + c[1].x) * 0.5, (c[0].y + c[1].y) * 0.5),
            egui::pos2((c[1].x + c[2].x) * 0.5, (c[1].y + c[2].y) * 0.5),
            egui::pos2((c[2].x + c[3].x) * 0.5, (c[2].y + c[3].y) * 0.5),
            egui::pos2((c[3].x + c[0].x) * 0.5, (c[3].y + c[0].y) * 0.5),
        ];
        for p in edges {
            let hr = egui::Rect::from_center_size(p, egui::vec2(7.0, 7.0));
            painter.rect_filled(hr, 1.0, egui::Color32::WHITE);
            painter.rect_stroke(hr, 1.0, egui::Stroke::new(1.0_f32, blue));
        }
        // Alça de rotação: acima do meio do topo.
        let topc = egui::pos2((c[0].x + c[1].x) * 0.5, (c[0].y + c[1].y) * 0.5);
        let mut dir = topc - egui::pos2((c[2].x + c[3].x) * 0.5, (c[2].y + c[3].y) * 0.5);
        let len = dir.length();
        if len > 0.001 {
            dir /= len;
        }
        let rotp = topc + dir * 24.0;
        painter.line_segment([topc, rotp], egui::Stroke::new(1.5_f32, blue));
        painter.circle_filled(rotp, 5.0, egui::Color32::WHITE);
        painter.circle_stroke(rotp, 5.0, egui::Stroke::new(1.5_f32, blue));
        // Marcador do pivô/âncora (offset a partir do centro).
        let piv = egui::pos2(
            rect.min.x + (s.x + s.anchor_x) * zoom,
            rect.min.y + (s.y + s.anchor_y) * zoom,
        );
        painter.circle_stroke(piv, 6.0, egui::Stroke::new(1.5_f32, blue));
        painter.circle_filled(piv, 2.0, blue);
    }

    /// Manipulação direta do retângulo azul da câmera (ferramenta Câmera):
    /// mover (arrastar dentro), redimensionar (4 cantos, preservando o centro),
    /// rotacionar (alça acima do topo) e mover o pivô. Grava no keyframe do
    /// frame atual (cria automaticamente se não houver — auto-keyframe).
    fn interacao_camera(
        &mut self,
        pressed: bool,
        down: bool,
        hover: Option<egui::Pos2>,
        ppos: Option<egui::Pos2>,
        rect: egui::Rect,
        zoom: f32,
    ) {
        let cur = self.document.current;
        let to_doc = |p: egui::Pos2| ((p.x - rect.min.x) / zoom, (p.y - rect.min.y) / zoom);
        let s = self.document.camera.sample(cur);
        // Ponto (doc) de um canto normalizado (sx, sy ∈ {-1,1}).
        let cam_point = |sx: f32, sy: f32| -> egui::Pos2 {
            let (sin, cos) = s.rotation.to_radians().sin_cos();
            let (lx, ly) = (sx * s.w * 0.5, sy * s.h * 0.5);
            egui::pos2(
                rect.min.x + (s.x + lx * cos - ly * sin) * zoom,
                rect.min.y + (s.y + lx * sin + ly * cos) * zoom,
            )
        };
        let corners = [
            cam_point(-1.0, -1.0),
            cam_point(1.0, -1.0),
            cam_point(1.0, 1.0),
            cam_point(-1.0, 1.0),
        ];
        // Alças laterais (meios das bordas): 0 topo, 1 direita, 2 base, 3 esquerda.
        let edges = [
            cam_point(0.0, -1.0),
            cam_point(1.0, 0.0),
            cam_point(0.0, 1.0),
            cam_point(-1.0, 0.0),
        ];
        // Alça de rotação (mesma geometria do desenho).
        let topc = egui::pos2(
            (corners[0].x + corners[1].x) * 0.5,
            (corners[0].y + corners[1].y) * 0.5,
        );
        let botc = egui::pos2(
            (corners[2].x + corners[3].x) * 0.5,
            (corners[2].y + corners[3].y) * 0.5,
        );
        let mut dir = topc - botc;
        let len = dir.length();
        if len > 0.001 {
            dir /= len;
        }
        let rotp = topc + dir * 24.0;
        let pivp = egui::pos2(
            rect.min.x + (s.x + s.anchor_x) * zoom,
            rect.min.y + (s.y + s.anchor_y) * zoom,
        );

        if pressed {
            if let Some(p) = hover {
                let dp = to_doc(p);
                let orig = match self.document.camera.keyframe_index(cur) {
                    Some(i) => self.document.camera.keyframes[i],
                    None => CameraKeyframe {
                        frame: cur,
                        x: s.x,
                        y: s.y,
                        w: s.w,
                        h: s.h,
                        rotation: s.rotation,
                        anchor_x: s.anchor_x,
                        anchor_y: s.anchor_y,
                        interp: self.cam_interp,
                    },
                };
                self.cam_orig = orig;
                let near = |a: egui::Pos2, b: egui::Pos2| a.distance(b) <= 10.0;
                if near(p, rotp) {
                    self.cam_act = 3;
                    self.cam_start_ang = (dp.1 - s.y).atan2(dp.0 - s.x);
                } else if near(p, pivp) {
                    self.cam_act = 4;
                } else if let Some(hi) = corners.iter().position(|&h| near(p, h)) {
                    self.cam_act = 2;
                    self.cam_h = hi;
                } else if let Some(ei) = edges.iter().position(|&h| near(p, h)) {
                    self.cam_act = 2;
                    self.cam_h = ei + 4;
                } else if self.camera_ponto_dentro(&s, dp) {
                    self.cam_act = 1;
                    self.cam_grab = (dp.0 - s.x, dp.1 - s.y);
                } else {
                    self.cam_act = 0;
                }
            }
        }

        if down && self.cam_act != 0 {
            if let Some(pp) = ppos {
                let dp = to_doc(pp);
                let mut k = self.cam_orig;
                match self.cam_act {
                    1 => {
                        k.x = dp.0 - self.cam_grab.0;
                        k.y = dp.1 - self.cam_grab.1;
                    }
                    2 => {
                        // Ponteiro em coords LOCAIS (desfaz a rotação em torno do centro).
                        let (sin, cos) = (-self.cam_orig.rotation.to_radians()).sin_cos();
                        let (rx, ry) = (dp.0 - self.cam_orig.x, dp.1 - self.cam_orig.y);
                        let lx = rx * cos - ry * sin;
                        let ly = rx * sin + ry * cos;
                        let hw0 = self.cam_orig.w * 0.5;
                        let hh0 = self.cam_orig.h * 0.5;
                        if self.cam_h < 4 {
                            // Cantos: preservam o centro (nova meia-dim = |local|).
                            k.w = (lx.abs() * 2.0).max(4.0);
                            k.h = (ly.abs() * 2.0).max(4.0);
                            k.x = self.cam_orig.x;
                            k.y = self.cam_orig.y;
                        } else {
                            // Laterais: ancoram o lado OPOSTO (o centro desloca).
                            // 4 topo, 5 direita, 6 base, 7 esquerda.
                            let (mut cxl, mut cyl) = (0.0f32, 0.0f32);
                            match self.cam_h {
                                5 => {
                                    let fixed = -hw0;
                                    k.w = (lx - fixed).abs().max(4.0);
                                    cxl = (lx + fixed) * 0.5;
                                }
                                7 => {
                                    let fixed = hw0;
                                    k.w = (lx - fixed).abs().max(4.0);
                                    cxl = (lx + fixed) * 0.5;
                                }
                                4 => {
                                    let fixed = hh0;
                                    k.h = (ly - fixed).abs().max(4.0);
                                    cyl = (ly + fixed) * 0.5;
                                }
                                6 => {
                                    let fixed = -hh0;
                                    k.h = (ly - fixed).abs().max(4.0);
                                    cyl = (ly + fixed) * 0.5;
                                }
                                _ => {}
                            }
                            // Converte o novo centro local (cxl, cyl) para o documento.
                            let (fs, fc) = self.cam_orig.rotation.to_radians().sin_cos();
                            k.x = self.cam_orig.x + cxl * fc - cyl * fs;
                            k.y = self.cam_orig.y + cxl * fs + cyl * fc;
                        }
                    }
                    3 => {
                        let ang = (dp.1 - self.cam_orig.y).atan2(dp.0 - self.cam_orig.x);
                        let delta = (ang - self.cam_start_ang).to_degrees();
                        k.rotation = self.cam_orig.rotation + delta;
                    }
                    4 => {
                        k.anchor_x = dp.0 - self.cam_orig.x;
                        k.anchor_y = dp.1 - self.cam_orig.y;
                    }
                    _ => {}
                }
                k.frame = cur;
                self.document.camera.set_keyframe(k);
                self.dirty = true;
            }
        }

        if !down {
            self.cam_act = 0;
        }
    }

    /// Testa se um ponto (doc) está dentro do retângulo (rotacionado) da câmera.
    fn camera_ponto_dentro(&self, s: &sketchmotion_core::CameraState, dp: (f32, f32)) -> bool {
        let (sin, cos) = (-s.rotation.to_radians()).sin_cos();
        let (rx, ry) = (dp.0 - s.x, dp.1 - s.y);
        let lx = rx * cos - ry * sin;
        let ly = rx * sin + ry * cos;
        lx.abs() <= s.w * 0.5 && ly.abs() <= s.h * 0.5
    }

    /// Aplica o enquadramento da câmera (no frame `pf`) sobre a composição
    /// `full` (tamanho do documento), devolvendo um RGBA do MESMO tamanho já
    /// "visto pela câmera": a região do retângulo azul preenche a saída, com
    /// zoom/pan/rotação. Não altera os dados originais — só a visualização.
    fn aplicar_camera_rgba(&self, full: &[u8], dw: u32, dh: u32, pf: usize) -> Vec<u8> {
        let s = self.document.camera.sample(pf);
        let (ow, oh) = (dw as usize, dh as usize);
        let mut out = vec![0u8; ow * oh * 4];
        if full.len() < ow * oh * 4 {
            return out;
        }
        let ang = s.rotation.to_radians();
        let (sin, cos) = ang.sin_cos();
        for oy in 0..oh {
            for ox in 0..ow {
                // Ponto normalizado (centro-base) dentro do retângulo da câmera.
                let u = (ox as f32 + 0.5) / ow as f32 - 0.5;
                let v = (oy as f32 + 0.5) / oh as f32 - 0.5;
                let lx = u * s.w;
                let ly = v * s.h;
                // Roda pelo ângulo da câmera e soma o centro → coord. do documento.
                let dx = s.x + lx * cos - ly * sin;
                let dy = s.y + lx * sin + ly * cos;
                let sx = dx.floor() as i32;
                let sy = dy.floor() as i32;
                let di = (oy * ow + ox) * 4;
                if sx >= 0 && sy >= 0 && (sx as u32) < dw && (sy as u32) < dh {
                    let si = ((sy as u32 * dw + sx as u32) * 4) as usize;
                    out[di..di + 4].copy_from_slice(&full[si..si + 4]);
                }
            }
        }
        out
    }

    // ---------------- Traçado de Imagem (raster → vetor) ----------------

    /// Reduz uma imagem RGBA para no máx. `max` px no maior lado (prévia ágil).
    fn trace_reduzir(ow: u32, oh: u32, px: &[u8], max: u32) -> (u32, u32, Vec<u8>) {
        if let Some(img) = image::RgbaImage::from_raw(ow, oh, px.to_vec()) {
            let d = image::DynamicImage::ImageRgba8(img).thumbnail(max, max).to_rgba8();
            let (w, h) = d.dimensions();
            (w, h, d.into_raw())
        } else {
            (ow, oh, px.to_vec())
        }
    }

    /// Mapeia (px, py) da imagem (dims sw×sh) para o canvas via a transformação
    /// do objeto de imagem (posição/escala/rotação).
    fn trace_map_dims(&self, px: f32, py: f32, sw: f32, sh: f32) -> (f32, f32) {
        let (cx, cy, hw, hh, angle) = self.trace_tf;
        let u = if sw > 0.0 { px / sw } else { 0.0 };
        let v = if sh > 0.0 { py / sh } else { 0.0 };
        let lx = (u - 0.5) * 2.0 * hw;
        let ly = (v - 0.5) * 2.0 * hh;
        let (s, c) = angle.sin_cos();
        (cx + lx * c - ly * s, cy + lx * s + ly * c)
    }

    /// Mapeia usando as dimensões da imagem de PRÉVIA (trace_src).
    fn trace_map(&self, px: f32, py: f32) -> (f32, f32) {
        let (sw, sh) = match &self.trace_src {
            Some((w, h, _)) => (*w as f32, *h as f32),
            None => return (px, py),
        };
        self.trace_map_dims(px, py, sw, sh)
    }

    /// Define como origem do traçado a flutuante de imagem atual (se houver).
    fn trace_usar_selecionada(&mut self) -> bool {
        self.trace_confirm = None;
        if let Some(fs) = &self.float_sel {
            if fs.is_image {
                let (w, h, rgba) = Self::trace_reduzir(fs.ow, fs.oh, &fs.pixels, 640);
                self.trace_tf = (fs.cx, fs.cy, fs.hw, fs.hh, fs.angle);
                self.trace_src = Some((w, h, rgba));
                self.trace_from_float = true;
                self.trace_img_idx = None;
                self.trace_dirty = true;
                self.status = "Traçando a imagem selecionada".into();
                return true;
            }
        }
        // Sem flutuante: tenta o último objeto de imagem solto no frame.
        if !self.document.images.is_empty() {
            let i = self.document.images.len() - 1;
            let o = &self.document.images[i];
            let (w, h, rgba) = Self::trace_reduzir(o.ow, o.oh, &o.pixels, 640);
            self.trace_tf = (o.cx, o.cy, o.hw, o.hh, o.angle);
            self.trace_src = Some((w, h, rgba));
            self.trace_from_float = false;
            self.trace_img_idx = Some(i);
            self.trace_dirty = true;
            self.status = "Traçando o objeto de imagem do frame".into();
            return true;
        }
        self.status = "Selecione/importe uma imagem primeiro (Caminho B ou Carregar)".into();
        false
    }

    /// Abre uma imagem (PNG/JPG/BMP), coloca no canvas como objeto móvel
    /// (Caminho B) e a define como origem do traçado.
    fn trace_carregar(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Imagem", &["png", "jpg", "jpeg", "bmp"])
            .pick_file()
        {
            match image::open(&path) {
                Ok(img) => {
                    let rgba = img.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    self.colocar_imagem(w, h, rgba.into_raw());
                    self.trace_usar_selecionada();
                    self.status = format!("Imagem carregada p/ traçar: {w}×{h}");
                }
                Err(e) => self.status = format!("Erro ao abrir imagem: {e}"),
            }
        }
    }

    /// Recalcula a PRÉVIA (na imagem reduzida — rápido) conforme o preset.
    fn trace_recalcular(&mut self) {
        let Some((w, h, rgba)) = self.trace_src.clone() else {
            self.trace_regions.clear();
            return;
        };
        let recipe = trace_recipe(self.trace_preset, self.trace_remove_bg, 1.0);
        let prog = AtomicU32::new(0);
        self.trace_regions = apply_recipe(&recipe, &rgba, w, h, &prog);
    }

    /// Se a origem é a flutuante, acompanha o transform atual dela (mover/escalar).
    fn trace_sync_float(&mut self) {
        if self.trace_from_float {
            if let Some(fs) = &self.float_sel {
                if fs.is_image {
                    self.trace_tf = (fs.cx, fs.cy, fs.hw, fs.hh, fs.angle);
                }
            }
        }
    }

    /// Cria os VectorObjects a partir das regiões traçadas (entra no Undo).
    /// Vetoriza no preset dado: EXPAND (cria os vetores) + DESAGRUPA (cada região
    /// vira um objeto solto) e SUBSTITUI a imagem de origem (some o raster).
    /// Inicia a vetorização em RESOLUÇÃO CHEIA numa thread de fundo (com barra de
    /// progresso). O resultado é aplicado quando a thread termina (trace_poll).
    fn trace_vetorizar(&mut self, preset: usize) {
        if self.trace_src.is_none() {
            self.status = "Selecione/carregue uma imagem primeiro".into();
            return;
        }
        if self.trace_job.is_some() {
            return; // já vetorizando
        }
        self.trace_preset = preset;
        self.trace_sync_float();
        // Pixels em alta resolução do objeto de origem (limite 2000px por lado).
        let cheio: Option<(u32, u32, Vec<u8>)> = if self.trace_from_float {
            self.float_sel
                .as_ref()
                .filter(|f| f.is_image)
                .map(|f| Self::trace_reduzir(f.ow, f.oh, &f.pixels, 2000))
        } else if let Some(i) = self.trace_img_idx {
            self.document
                .images
                .get(i)
                .map(|o| Self::trace_reduzir(o.ow, o.oh, &o.pixels, 2000))
        } else {
            self.trace_src.clone()
        };
        let (sw, sh, rgba) = match cheio {
            Some(v) => v,
            None => {
                self.status = "Não encontrei a imagem de origem".into();
                return;
            }
        };
        let escala = (sw.max(sh) as f32 / 640.0).max(1.0);
        let recipe = trace_recipe(preset, self.trace_remove_bg, escala);
        let progress = Arc::new(AtomicU32::new(0));
        let prog2 = progress.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let regs = apply_recipe(&recipe, &rgba, sw, sh, &prog2);
            let _ = tx.send(regs);
        });
        self.trace_job = Some(TraceJob {
            rx,
            progress,
            sw: sw as f32,
            sh: sh as f32,
            from_float: self.trace_from_float,
            img_idx: self.trace_img_idx,
            preset,
        });
        self.trace_confirm = None;
        self.status = "Vetorizando… (pode levar alguns segundos)".into();
    }

    /// Verifica o andamento da vetorização; mostra a barra e finaliza quando pronto.
    fn trace_poll(&mut self, ctx: &egui::Context) {
        let done: Option<Result<Vec<TracedRegion>, ()>> = if let Some(job) = &self.trace_job {
            ctx.request_repaint();
            let frac = job.progress.load(Ordering::Relaxed) as f32 / 1000.0;
            egui::Window::new("Vetorizando…")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.label("Convertendo a imagem em vetores.");
                    ui.label("Em imagens grandes isso pode demorar um pouco.");
                    ui.add(egui::ProgressBar::new(frac.clamp(0.0, 1.0)).show_percentage());
                });
            match job.rx.try_recv() {
                Ok(regs) => Some(Ok(regs)),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(())),
            }
        } else {
            None
        };
        match done {
            Some(Ok(regs)) => {
                if let Some(job) = self.trace_job.take() {
                    self.trace_finalizar(job, regs);
                }
            }
            Some(Err(())) => {
                self.trace_job = None;
                self.status = "Falha na vetorização".into();
            }
            None => {}
        }
    }

    /// Cria os vetores do resultado, substitui a imagem e limpa o estado.
    fn trace_finalizar(&mut self, job: TraceJob, regioes: Vec<TracedRegion>) {
        if regioes.is_empty() {
            self.status = "Nada para vetorizar (tente outro tipo)".into();
            return;
        }
        self.trace_sync_float();
        self.push_undo();
        let (swf, shf) = (job.sw, job.sh);
        let mut criados = 0usize;
        for reg in &regioes {
            if reg.points.len() < 3 {
                continue;
            }
            let fill = Color::rgba(reg.color[0], reg.color[1], reg.color[2], reg.color[3]);
            let mut obj = VectorObject::new(Color::TRANSPARENT, 1.0);
            obj.points = reg
                .points
                .iter()
                .map(|&(x, y)| {
                    let (cx, cy) = self.trace_map_dims(x, y, swf, shf);
                    Anchor::new(cx, cy)
                })
                .collect();
            obj.closed = true;
            obj.fill = Some(fill);
            self.document.vectors.push(obj);
            criados += 1;
        }
        // Substitui a imagem de origem: some o raster (fica só o vetor).
        if job.from_float {
            self.float_sel = None;
            self.float_tex = None;
        } else if let Some(i) = job.img_idx {
            if i < self.document.images.len() {
                self.document.images.remove(i);
            }
        }
        self.document.sync_to_frames();
        self.trace_src = None;
        self.trace_regions.clear();
        self.trace_from_float = false;
        self.trace_img_idx = None;
        self.trace_confirm = None;
        self.tool = Tool::Select;
        self.dirty = true;
        self.status = format!(
            "Vetorizado ({}): {criados} componentes soltos",
            TRACE_PRESETS[job.preset]
        );
    }

    /// Janela da ferramenta Traçado de Imagem.
    fn janela_trace(&mut self, ctx: &egui::Context) {
        let mut open = self.win_trace;
        let mut do_load = false;
        let mut do_sel = false;
        let mut do_close = false;
        let mut do_vec: Option<usize> = None;
        let amarelo = egui::Color32::from_rgb(0xE0, 0xB0, 0x3A);
        egui::Window::new("Traçado de Imagem")
            .open(&mut open)
            .default_width(300.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .button("Usar imagem selecionada")
                        .on_hover_text("Usa a imagem-objeto selecionada no canvas (Caminho B)")
                        .clicked()
                    {
                        do_sel = true;
                    }
                    if ui
                        .button("Carregar imagem…")
                        .on_hover_text("Abre um PNG/JPG/BMP e coloca no canvas")
                        .clicked()
                    {
                        do_load = true;
                    }
                });
                let tem_img = self.trace_src.is_some();
                if let Some((w, h, _)) = &self.trace_src {
                    ui.label(format!("Imagem pronta: {w}×{h}"));
                } else {
                    ui.weak("Selecione ou carregue uma imagem para vetorizar.");
                }
                ui.separator();

                // Confirmação "deseja vetorizar?" após clicar num tipo.
                if let Some(pi) = self.trace_confirm {
                    ui.colored_label(amarelo, format!("Vetorizar em \"{}\"?", TRACE_PRESETS[pi]));
                    ui.horizontal(|ui| {
                        if ui.button("Sim, vetorizar").clicked() {
                            do_vec = Some(pi);
                        }
                        if ui.button("Não").clicked() {
                            self.trace_confirm = None;
                        }
                    });
                    ui.separator();
                }

                ui.strong("Tipo de traçado:");
                let livre = tem_img && self.trace_job.is_none();
                ui.add_enabled_ui(livre, |ui| {
                    for (i, nome) in TRACE_PRESETS.iter().enumerate() {
                        if ui.selectable_label(self.trace_preset == i, *nome).clicked() {
                            // Mostra a prévia deste tipo e pede confirmação.
                            self.trace_preset = i;
                            self.trace_dirty = true;
                            self.trace_confirm = Some(i);
                        }
                    }
                });
                ui.separator();
                if ui
                    .checkbox(&mut self.trace_remove_bg, "Remover fundo")
                    .on_hover_text("A cor do fundo (borda) vira transparência (modos coloridos)")
                    .changed()
                {
                    self.trace_dirty = true;
                }
                ui.horizontal(|ui| {
                    if ui.button("Fechar").clicked() {
                        do_close = true;
                    }
                });
                ui.weak(
                    "Clique num tipo, confirme, e a imagem vira vetores: cada componente fica \
                     SOLTO (mover/editar/cor/Undo). A imagem original é substituída pelos vetores.",
                );
            });
        self.win_trace = open && !do_close;
        if do_sel {
            self.trace_usar_selecionada();
        }
        if do_load {
            self.trace_carregar();
        }
        self.trace_sync_float();
        if self.trace_dirty {
            self.trace_recalcular();
            self.trace_dirty = false;
        }
        if let Some(pi) = do_vec {
            self.trace_vetorizar(pi);
        }
    }

    /// Desenha a prévia do traçado (contornos azuis) sobre o canvas.
    fn desenhar_trace_preview(&self, ui: &mut egui::Ui, rect: egui::Rect, zoom: f32) {
        if self.trace_src.is_none() {
            return;
        }
        let painter = ui.painter_at(rect);
        let blue = egui::Color32::from_rgb(0x2F, 0x84, 0xFE);
        for reg in &self.trace_regions {
            if reg.points.len() < 2 {
                continue;
            }
            let pts: Vec<egui::Pos2> = reg
                .points
                .iter()
                .map(|&(x, y)| {
                    let (cx, cy) = self.trace_map(x, y);
                    egui::pos2(rect.min.x + cx * zoom, rect.min.y + cy * zoom)
                })
                .collect();
            painter.add(egui::Shape::closed_line(
                pts,
                egui::Stroke::new(1.2_f32, blue),
            ));
        }
    }

    /// Desenha as alças de redimensionamento ao redor do canvas e trata o
    /// arrasto (ancorado no topo-esquerda; muda o tamanho do papel ao soltar).
    fn prancheta_handles(&mut self, ui: &mut egui::Ui, rect: egui::Rect, zoom: f32) {
        let blue = egui::Color32::from_rgb(0x2F, 0x84, 0xFE);
        let painter = ui.painter_at(ui.clip_rect());
        painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.5, blue));
        // Quadradinhos de seleção em volta (8, estilo Illustrator).
        let visuais = [
            rect.left_top(),
            rect.center_top(),
            rect.right_top(),
            rect.left_center(),
            rect.left_bottom(),
            rect.center_bottom(),
            rect.right_center(),
            rect.right_bottom(),
        ];
        for p in visuais {
            let hr = egui::Rect::from_center_size(p, egui::vec2(9.0, 9.0));
            painter.rect_filled(hr, 1.0, egui::Color32::WHITE);
            painter.rect_stroke(hr, 1.0, egui::Stroke::new(1.0, blue));
        }
        // Alças funcionais (ancoradas no topo-esquerda): largura, altura, ambos.
        let funcionais = [
            (
                rect.right_center(),
                0u8,
                egui::CursorIcon::ResizeHorizontal,
            ),
            (
                rect.center_bottom(),
                1u8,
                egui::CursorIcon::ResizeVertical,
            ),
            (rect.right_bottom(), 2u8, egui::CursorIcon::ResizeNwSe),
        ];
        for (p, id, cur) in funcionais {
            let hr = egui::Rect::from_center_size(p, egui::vec2(14.0, 14.0));
            let resp = ui.interact(hr, ui.id().with(("prancheta_h", id)), egui::Sense::drag());
            let ativo = resp.hovered() || self.prancheta_drag == Some(id);
            if ativo {
                painter.rect_filled(
                    egui::Rect::from_center_size(p, egui::vec2(11.0, 11.0)),
                    1.0,
                    blue,
                );
                ui.ctx().set_cursor_icon(cur);
            }
            if resp.drag_started() {
                self.prancheta_drag = Some(id);
            }
        }
        // Arrasto em andamento: preview + aplica ao soltar.
        if let Some(id) = self.prancheta_drag {
            if let Some(pp) = ui.input(|i| i.pointer.interact_pos()) {
                let px = (((pp.x - rect.left()) / zoom).round() as i32).clamp(1, 8192) as u32;
                let py = (((pp.y - rect.top()) / zoom).round() as i32).clamp(1, 8192) as u32;
                let mut nw = self.document.width;
                let mut nh = self.document.height;
                if id == 0 || id == 2 {
                    nw = px;
                }
                if id == 1 || id == 2 {
                    nh = py;
                }
                let prect = egui::Rect::from_min_size(
                    rect.min,
                    egui::vec2(nw as f32 * zoom, nh as f32 * zoom),
                );
                painter.rect_stroke(prect, 0.0, egui::Stroke::new(2.0, blue));
                painter.text(
                    prect.right_bottom() + egui::vec2(6.0, 6.0),
                    egui::Align2::LEFT_TOP,
                    format!("{nw} x {nh}"),
                    egui::FontId::proportional(13.0),
                    blue,
                );
                self.pr_w = nw;
                self.pr_h = nh;
                if ui.input(|i| i.pointer.any_released()) {
                    self.prancheta_drag = None;
                    self.aplicar_prancheta(nw, nh);
                }
            } else if ui.input(|i| !i.pointer.any_down()) {
                self.prancheta_drag = None;
            }
        }
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
            self.push_undo();
            if let Some(l) = self.document.layer_mut(i) {
                l.visible = !l.visible;
            }
            self.dirty = true;
        }
        if let Some(i) = toggle_lock {
            self.push_undo();
            if let Some(l) = self.document.layer_mut(i) {
                l.locked = !l.locked;
            }
        }
        if let Some(i) = set_active {
            self.active_layer = i;
        }
        if let Some(i) = move_up {
            if i + 1 < self.document.layers.len() {
                self.push_undo();
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
                self.push_undo();
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
            self.push_undo();
            let nome = format!("Camada {}", self.document.layers.len() + 1);
            let idx = self.document.add_layer_above(self.active_layer, nome);
            self.active_layer = idx;
            self.dirty = true;
        }
        if excluir {
            // remove_layer se recusa a apagar a última camada; nesse caso, avisa.
            if self.document.layers.len() <= 1 {
                self.status = "Não é possível excluir a única camada".into();
            } else {
                self.push_undo();
                self.document.remove_layer(self.active_layer);
                if self.active_layer >= self.document.layers.len() {
                    self.active_layer = self.document.layers.len() - 1;
                }
                self.dirty = true;
            }
        }

        self.reopen_layers = false;
        self.win_layers = open;
    }

    /// Índice do esqueleto ativo (clampado), ou None se não há esqueletos.
    fn rig_active_index(&self) -> Option<usize> {
        let n = self.document.skeletons.len();
        if n == 0 {
            None
        } else {
            Some(self.rig_skel.min(n - 1))
        }
    }

    /// Eixo (x em doc) de espelhamento do esqueleto `si`, ou None se não espelhado.
    fn rig_axis(&self, si: usize) -> Option<f32> {
        let sk = self.document.skeletons.get(si)?;
        if !sk.flip_h {
            return None;
        }
        let root = sk.bones.iter().find(|b| b.parent.is_none())?;
        Some(sk.origin(root.id).0)
    }

    /// Inverte (espelha) o esqueleto ativo horizontalmente.
    fn rig_mirror(&mut self, si: usize) {
        self.push_undo();
        if let Some(sk) = self.document.skeletons.get_mut(si) {
            sk.flip_h = !sk.flip_h;
        }
        self.dirty = true;
    }

    /// Hit-test do rig em qualquer esqueleto visível (usado pela ferramenta
    /// Seleção). Se acertar um osso, ativa esse esqueleto, faz o pick (com
    /// resize liberado) e devolve o índice do esqueleto.
    fn rig_pick_any(&mut self, p: (f32, f32)) -> Option<usize> {
        if !self.show_bones {
            return None;
        }
        let mut hit = None;
        for (si, sk) in self.document.skeletons.iter().enumerate() {
            if !sk.visible {
                continue;
            }
            let axis = self.rig_axis(si);
            let pp = (refl_x(axis, p.0), p.1);
            for b in &sk.bones {
                let wt = sk.tip(b.id);
                let wo = sk.origin(b.id);
                let dt = ((wt.0 - pp.0).powi(2) + (wt.1 - pp.1).powi(2)).sqrt();
                let doo = ((wo.0 - pp.0).powi(2) + (wo.1 - pp.1).powi(2)).sqrt();
                if dt <= 12.0 || doo <= 12.0 || self.piece_hit(sk, b.id, pp) {
                    hit = Some(si);
                }
            }
        }
        let si = hit?;
        self.rig_skel = si;
        let axis = self.rig_axis(si);
        let pp = (refl_x(axis, p.0), p.1);
        self.rig_pick(si, pp, self.rig_resize_enabled);
        Some(si)
    }

    /// Cria um osso arrastando de `start` a `end`. Se `start` estiver perto da
    /// ponta de um osso existente, o novo osso vira filho dele (encadeia).
    fn rig_create_bone(&mut self, si: usize, start: (f32, f32), end: (f32, f32)) {
        let (dx, dy) = (end.0 - start.0, end.1 - start.1);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 4.0 {
            return;
        }
        // Encaixa o começo no ponto de ligação mais próximo (encadeia).
        let mut parent: Option<u32> = None;
        let mut snap = start;
        {
            let sk = &self.document.skeletons[si];
            let mut best = 16.0_f32;
            for b in &sk.bones {
                for cp in self.bone_conns(sk, b.id) {
                    let d = ((cp.0 - start.0).powi(2) + (cp.1 - start.1).powi(2)).sqrt();
                    if d < best {
                        best = d;
                        parent = Some(b.id);
                        snap = cp;
                    }
                }
            }
        }
        self.push_undo();
        let ang = (end.1 - snap.1).atan2(end.0 - snap.0);
        let seg = ((end.0 - snap.0).powi(2) + (end.1 - snap.1).powi(2)).sqrt().max(1.0);
        let sk = &mut self.document.skeletons[si];
        let id = sk.add_bone_world(parent, snap.0, snap.1, ang, seg, self.rig_shape);
        self.rig_sel_bone = Some(id);
        self.dirty = true;
    }

    /// Seleciona o osso sob o ponto (prioriza a articulação para mover).
    fn rig_pick(&mut self, si: usize, p: (f32, f32), allow_resize: bool) {
        let mut found: Option<u32> = None;
        let mut mode = RigDrag::Rotate;
        {
            let sk = &self.document.skeletons[si];
            // 1) ponta (bola) -> redimensionar (quando permitido)
            if allow_resize {
                let mut best = 12.0_f32;
                for b in &sk.bones {
                    let (tx, ty) = sk.tip(b.id);
                    let d = ((tx - p.0).powi(2) + (ty - p.1).powi(2)).sqrt();
                    if d < best {
                        best = d;
                        found = Some(b.id);
                        mode = RigDrag::Resize;
                    }
                }
            }
            // 2) origem (bola) -> mover
            if found.is_none() {
                let mut best = 12.0_f32;
                for b in &sk.bones {
                    let (ox, oy) = sk.origin(b.id);
                    let d = ((ox - p.0).powi(2) + (oy - p.1).powi(2)).sqrt();
                    if d < best {
                        best = d;
                        found = Some(b.id);
                        mode = RigDrag::Move;
                    }
                }
            }
            // 3) corpo da peça -> girar (o de cima vence)
            if found.is_none() {
                for b in &sk.bones {
                    if self.piece_hit(sk, b.id, p) {
                        found = Some(b.id);
                        mode = RigDrag::Rotate;
                    }
                }
            }
        }
        self.rig_sel_bone = found;
        if let Some(bid) = found {
            self.push_undo();
            self.rig_dragging = mode;
            let grab = {
                let sk = &self.document.skeletons[si];
                let (ox, oy) = sk.origin(bid);
                match mode {
                    RigDrag::Move => (ox - p.0, oy - p.1),
                    RigDrag::Rotate => {
                        let cur = sk.world(bid).angle;
                        let aim = (p.1 - oy).atan2(p.0 - ox);
                        (cur - aim, 0.0)
                    }
                    RigDrag::Resize | RigDrag::None => (0.0, 0.0),
                }
            };
            self.rig_grab = grab;
        } else {
            self.rig_dragging = RigDrag::None;
        }
    }

    /// Aplica a manipulação em andamento (girar em torno da articulação ou
    /// mover a articulação). Filhos acompanham porque o mundo é recalculado.
    fn rig_drag_to(&mut self, si: usize, p: (f32, f32)) {
        let bid = match self.rig_sel_bone {
            Some(b) => b,
            None => return,
        };
        match self.rig_dragging {
            RigDrag::Rotate => {
                let (ox, oy, parent_ang) = {
                    let sk = &self.document.skeletons[si];
                    let (ox, oy) = sk.origin(bid);
                    let pa = sk
                        .bone(bid)
                        .and_then(|b| b.parent)
                        .map(|pp| sk.world(pp).angle)
                        .unwrap_or(0.0);
                    (ox, oy, pa)
                };
                let aim = (p.1 - oy).atan2(p.0 - ox) + self.rig_grab.0;
                if let Some(b) = self.document.skeletons[si].bone_mut(bid) {
                    b.angle = aim - parent_ang;
                }
                self.rig_snap_hint = None;
                self.dirty = true;
            }
            RigDrag::Resize => {
                let (ox, oy, parent_ang) = {
                    let sk = &self.document.skeletons[si];
                    let (ox, oy) = sk.origin(bid);
                    let pa = sk
                        .bone(bid)
                        .and_then(|b| b.parent)
                        .map(|pp| sk.world(pp).angle)
                        .unwrap_or(0.0);
                    (ox, oy, pa)
                };
                let (dx, dy) = (p.0 - ox, p.1 - oy);
                let nlen = (dx * dx + dy * dy).sqrt().max(2.0);
                let aim = dy.atan2(dx);
                if let Some(b) = self.document.skeletons[si].bone_mut(bid) {
                    b.length = nlen;
                    b.angle = aim - parent_ang;
                }
                self.rig_snap_hint = None;
                self.dirty = true;
            }
            RigDrag::Move => {
                let desired = (p.0 + self.rig_grab.0, p.1 + self.rig_grab.1);
                let (nx, ny) = {
                    let sk = &self.document.skeletons[si];
                    match sk.bone(bid).and_then(|b| b.parent) {
                        None => desired,
                        Some(pp) => {
                            let pw = sk.world(pp);
                            let (dx, dy) = (desired.0 - pw.x, desired.1 - pw.y);
                            let (s, c) = (-pw.angle).sin_cos();
                            let sc = pw.scale.max(1e-4);
                            ((dx * c - dy * s) / sc, (dx * s + dy * c) / sc)
                        }
                    }
                };
                if let Some(b) = self.document.skeletons[si].bone_mut(bid) {
                    b.x = nx;
                    b.y = ny;
                }
                let origin_now = self.document.skeletons[si].origin(bid);
                if let Some((gid, gx, gy)) = self.rig_sep_guard {
                    if gid == bid
                        && ((origin_now.0 - gx).powi(2) + (origin_now.1 - gy).powi(2)).sqrt() > 34.0
                    {
                        self.rig_sep_guard = None;
                    }
                }
                self.rig_snap_hint = self.nearest_snap_target(si, bid, origin_now);
                self.dirty = true;
            }
            RigDrag::None => {}
        }
    }

    /// Opções da ferramenta Rig na barra superior.
    /// Garante que as texturas de cursor personalizadas estejam carregadas
    /// (decodifica + reduz os PNGs uma vez). Índices: 0 caneta, 1 balde,
    /// 2 conta-gotas, 3 laço, 4 varinha.
    /// Carrega (uma vez) as texturas das peças do rig a partir de ossos/*.png.
    /// Ordem: 0 membro, 1 tronco, 2 quadril, 3 cabeca, 4 mao, 5 pe.
    fn ensure_piece_textures(&mut self, ctx: &egui::Context) {
        if self.piece_tex.iter().all(|t| t.is_some()) {
            return;
        }
        const DATA: [&[u8]; 6] = [
            include_bytes!("../../../ossos/membro.png"),
            include_bytes!("../../../ossos/tronco.png"),
            include_bytes!("../../../ossos/quadril.png"),
            include_bytes!("../../../ossos/cabeca.png"),
            include_bytes!("../../../ossos/mao.png"),
            include_bytes!("../../../ossos/pe.png"),
        ];
        const NAMES: [&str; 6] =
            ["p_membro", "p_tronco", "p_quadril", "p_cabeca", "p_mao", "p_pe"];
        for i in 0..6 {
            if self.piece_tex[i].is_some() {
                continue;
            }
            if let Ok(img) = image::load_from_memory(DATA[i]) {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                let color = egui::ColorImage::from_rgba_unmultiplied(
                    [w as usize, h as usize],
                    rgba.as_raw(),
                );
                self.piece_tex[i] =
                    Some(ctx.load_texture(NAMES[i], color, egui::TextureOptions::LINEAR));
            }
        }
    }

    /// Geometria (aspecto, origem, ponta, pontos de ligacao) de um osso, seja
    /// peca generica (shape) ou peca-imagem propria (img).
    fn bone_geom(
        &self,
        shape: sketchmotion_core::BoneShape,
        img: Option<u16>,
    ) -> (f32, (f32, f32), (f32, f32), &'static [(f32, f32)]) {
        if let Some(i) = img {
            let i = i as usize;
            if i < RAPTOR_PARTS.len() {
                let pd = &RAPTOR_PARTS[i];
                return (pd.aspect, pd.origin, pd.tip, pd.conns);
            }
        }
        let pd = piece_def(shape);
        (pd.aspect, pd.origin, pd.tip, pd.conns)
    }

    fn bone_tex(
        &self,
        shape: sketchmotion_core::BoneShape,
        img: Option<u16>,
    ) -> Option<&egui::TextureHandle> {
        if let Some(i) = img {
            let i = i as usize;
            if i < self.part_tex.len() {
                return self.part_tex[i].as_ref();
            }
        }
        self.piece_tex[piece_def(shape).tex].as_ref()
    }

    /// Carrega (uma vez) as texturas das pecas do raptor (ossos/raptor) e a
    /// miniatura do botao.
    fn ensure_part_textures(&mut self, ctx: &egui::Context) {
        if self.raptor_mini_tex.is_some() && self.part_tex.iter().all(|t| t.is_some()) {
            return;
        }
        const DATA: [&[u8]; 21] = [
            include_bytes!("../../../ossos/raptor/mandibula_superior.png"),
            include_bytes!("../../../ossos/raptor/maxilar_inferior.png"),
            include_bytes!("../../../ossos/raptor/pescoco_cervical.png"),
            include_bytes!("../../../ossos/raptor/pescoco_toraxica.png"),
            include_bytes!("../../../ossos/raptor/tronco.png"),
            include_bytes!("../../../ossos/raptor/cauda_1.png"),
            include_bytes!("../../../ossos/raptor/cauda_2.png"),
            include_bytes!("../../../ossos/raptor/cauda_3.png"),
            include_bytes!("../../../ossos/raptor/cauda_4.png"),
            include_bytes!("../../../ossos/raptor/umero_direito.png"),
            include_bytes!("../../../ossos/raptor/umero_esquerdo.png"),
            include_bytes!("../../../ossos/raptor/radio_direito.png"),
            include_bytes!("../../../ossos/raptor/radio_esquerdo.png"),
            include_bytes!("../../../ossos/raptor/mao_direita.png"),
            include_bytes!("../../../ossos/raptor/mao_esquerda.png"),
            include_bytes!("../../../ossos/raptor/femur_direito.png"),
            include_bytes!("../../../ossos/raptor/femur_esquerdo.png"),
            include_bytes!("../../../ossos/raptor/tibia_direita.png"),
            include_bytes!("../../../ossos/raptor/tibia_esquerda.png"),
            include_bytes!("../../../ossos/raptor/pe_direito.png"),
            include_bytes!("../../../ossos/raptor/pe_esquerdo.png"),
        ];
        for i in 0..21 {
            if self.part_tex[i].is_some() {
                continue;
            }
            if let Ok(img) = image::load_from_memory(DATA[i]) {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                let color = egui::ColorImage::from_rgba_unmultiplied(
                    [w as usize, h as usize],
                    rgba.as_raw(),
                );
                self.part_tex[i] = Some(ctx.load_texture(
                    RAPTOR_PARTS[i].name,
                    color,
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
        if self.raptor_mini_tex.is_none() {
            if let Ok(img) = image::load_from_memory(include_bytes!(
                "../../../ossos/raptor/miniatura_raptor.png"
            )) {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                let color = egui::ColorImage::from_rgba_unmultiplied(
                    [w as usize, h as usize],
                    rgba.as_raw(),
                );
                self.raptor_mini_tex =
                    Some(ctx.load_texture("raptor_mini", color, egui::TextureOptions::LINEAR));
            }
        }
    }

    /// Pontos de ligação de um osso em coordenadas de mundo (o primeiro é a
    /// origem/pino que se conecta ao pai).
    fn bone_conns(&self, sk: &sketchmotion_core::Skeleton, id: u32) -> Vec<(f32, f32)> {
        let (shape, img) = match sk.bone(id) {
            Some(b) => (b.shape, b.img),
            None => return Vec::new(),
        };
        let (_aspect, oi, ti, conns) = self.bone_geom(shape, img);
        let wo = sk.origin(id);
        let wt = sk.tip(id);
        conns
            .iter()
            .map(|&(cx, cy)| map_piece(oi, ti, wo, wt, cx, cy))
            .collect()
    }

    /// Testa se o ponto de mundo `p` está dentro do retângulo da peça (ignora
    /// transparência — suficiente para seleção).
    fn piece_hit(&self, sk: &sketchmotion_core::Skeleton, id: u32, p: (f32, f32)) -> bool {
        let (shape, img) = match sk.bone(id) {
            Some(b) => (b.shape, b.img),
            None => return false,
        };
        let (aspect, oi, ti, _c) = self.bone_geom(shape, img);
        let wo = sk.origin(id);
        let wt = sk.tip(id);
        let aimg = (ti.0 - oi.0, ti.1 - oi.1);
        let aw = (wt.0 - wo.0, wt.1 - wo.1);
        let limg = (aimg.0 * aimg.0 + aimg.1 * aimg.1).sqrt().max(1e-4);
        let lw = (aw.0 * aw.0 + aw.1 * aw.1).sqrt().max(1e-4);
        let ang = aw.1.atan2(aw.0) - aimg.1.atan2(aimg.0);
        let sc = lw / limg;
        let (s, c) = (-ang).sin_cos();
        let (dx, dy) = (p.0 - wo.0, p.1 - wo.1);
        let ix = (dx * c - dy * s) / sc + oi.0;
        let iy = (dx * s + dy * c) / sc + oi.1;
        ix >= 0.0 && ix <= 1.0 && iy >= 0.0 && iy <= aspect
    }

    /// True se `node` é descendente de `ancestor` (para evitar ciclos).
    fn is_descendant(&self, sk: &sketchmotion_core::Skeleton, mut node: u32, ancestor: u32) -> bool {
        while let Some(b) = sk.bone(node) {
            match b.parent {
                Some(pp) => {
                    if pp == ancestor {
                        return true;
                    }
                    node = pp;
                }
                None => break,
            }
        }
        false
    }

    /// Ponto de ligação mais próximo de `p` pertencente a outro osso (não a
    /// `bid` nem a um descendente dele). Devolve (id_alvo, x, y).
    fn nearest_snap_target(
        &self,
        si: usize,
        bid: u32,
        p: (f32, f32),
    ) -> Option<(u32, f32, f32)> {
        let sk = &self.document.skeletons[si];
        let mut best = 18.0_f32;
        let mut res = None;
        // Ponto de encaixe suprimido logo após "Separar" (evita reconectar sem
        // querer). Só volta a valer depois que a peça se afasta bem dele.
        let guard = match self.rig_sep_guard {
            Some((gid, gx, gy)) if gid == bid => Some((gx, gy)),
            _ => None,
        };
        for b in &sk.bones {
            if b.id == bid || self.is_descendant(sk, b.id, bid) {
                continue;
            }
            for cp in self.bone_conns(sk, b.id) {
                if let Some((gx, gy)) = guard {
                    if ((cp.0 - gx).powi(2) + (cp.1 - gy).powi(2)).sqrt() < 22.0 {
                        continue;
                    }
                }
                let d = ((cp.0 - p.0).powi(2) + (cp.1 - p.1).powi(2)).sqrt();
                if d < best {
                    best = d;
                    res = Some((b.id, cp.0, cp.1));
                }
            }
        }
        res
    }

    /// Conecta `bid` ao osso `target`, encaixando a origem de `bid` no ponto.
    fn rig_connect(&mut self, si: usize, bid: u32, target: u32, point: (f32, f32)) {
        {
            let sk = &self.document.skeletons[si];
            if target == bid || self.is_descendant(sk, target, bid) {
                return;
            }
        }
        let world_ang = self.document.skeletons[si].world(bid).angle;
        self.push_undo();
        let sk = &mut self.document.skeletons[si];
        if let Some(b) = sk.bone_mut(bid) {
            b.parent = Some(target);
        }
        let pw = sk.world(target);
        let (dx, dy) = (point.0 - pw.x, point.1 - pw.y);
        let (s, c) = (-pw.angle).sin_cos();
        let scl = pw.scale.max(1e-4);
        let lx = (dx * c - dy * s) / scl;
        let ly = (dx * s + dy * c) / scl;
        let la = world_ang - pw.angle;
        if let Some(b) = sk.bone_mut(bid) {
            b.x = lx;
            b.y = ly;
            b.angle = la;
        }
        self.dirty = true;
    }

    /// Separa o osso selecionado da sua conexão (vira raiz, mantendo a posição).
    fn rig_separar(&mut self, si: usize, bid: u32) {
        let w = self.document.skeletons[si].world(bid);
        self.push_undo();
        let sk = &mut self.document.skeletons[si];
        if let Some(b) = sk.bone_mut(bid) {
            b.parent = None;
            b.x = w.x;
            b.y = w.y;
            b.angle = w.angle;
        }
        // Guarda o ponto onde estava conectada: não reconecta aqui até a peça
        // se afastar e voltar.
        self.rig_sep_guard = Some((bid, w.x, w.y));
        self.dirty = true;
    }

    /// Fim do gesto de posar: se estava movendo e há alvo de encaixe, conecta.
    fn rig_release_pose(&mut self, si: usize) {
        if self.rig_dragging == RigDrag::Move {
            if let (Some(bid), Some((tid, tx, ty))) = (self.rig_sel_bone, self.rig_snap_hint) {
                self.rig_connect(si, bid, tid, (tx, ty));
            }
        }
        self.rig_snap_hint = None;
        self.rig_dragging = RigDrag::None;
    }

    fn ensure_cursor_textures(&mut self, ctx: &egui::Context) {
        if self.cursor_tex.iter().all(|t| t.is_some()) {
            return;
        }
        const DATA: [&[u8]; 6] = [
            include_bytes!("../../../cursores/render/caneta.png"),
            include_bytes!("../../../cursores/render/balde.png"),
            include_bytes!("../../../cursores/render/contagotas.png"),
            include_bytes!("../../../cursores/render/laco.png"),
            include_bytes!("../../../cursores/render/varinha_magica.png"),
            include_bytes!("../../../cursores/colocar_objeto.png"),
        ];
        const NAMES: [&str; 6] = [
            "cur_caneta",
            "cur_balde",
            "cur_conta",
            "cur_laco",
            "cur_varinha",
            "cur_colocar",
        ];
        for i in 0..6 {
            if self.cursor_tex[i].is_some() {
                continue;
            }
            if let Ok(img) = image::load_from_memory(DATA[i]) {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                let scale = (64.0_f32 / w.max(h) as f32).min(1.0);
                let nw = ((w as f32 * scale) as u32).max(1);
                let nh = ((h as f32 * scale) as u32).max(1);
                let small =
                    image::imageops::resize(&rgba, nw, nh, image::imageops::FilterType::Triangle);
                let color = egui::ColorImage::from_rgba_unmultiplied(
                    [nw as usize, nh as usize],
                    small.as_raw(),
                );
                self.cursor_tex[i] =
                    Some(ctx.load_texture(NAMES[i], color, egui::TextureOptions::LINEAR));
            }
        }
    }

    /// Desenha o cursor personalizado (índice) ancorado por `hotspot` (fração
    /// da imagem que coincide com o ponteiro real). Devolve false se a textura
    /// não estiver disponível.
    fn desenhar_cursor_img(
        &self,
        painter: &egui::Painter,
        ctx: &egui::Context,
        hp: egui::Pos2,
        idx: usize,
        hotspot: (f32, f32),
    ) -> bool {
        if let Some(tex) = &self.cursor_tex[idx] {
            let raw = tex.size_vec2();
            // Desenha pequeno (~30px), como um cursor de seta.
            let scl = 30.0_f32 / raw.x.max(raw.y).max(1.0);
            let size = raw * scl;
            let tl = hp - egui::vec2(size.x * hotspot.0, size.y * hotspot.1);
            let rect = egui::Rect::from_min_size(tl, size);
            painter.image(
                tex.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            ctx.set_cursor_icon(egui::CursorIcon::None);
            true
        } else {
            false
        }
    }

    fn opcoes_rig(&mut self, ui: &mut egui::Ui) {
        use sketchmotion_core::BoneShape as BS;
        ui.label("Rig:");
        ui.selectable_value(&mut self.rig_mode, RigMode::Create, "Criar ossos");
        ui.selectable_value(&mut self.rig_mode, RigMode::Pose, "Posar");
        if self.rig_mode == RigMode::Create {
            ui.separator();
            ui.label("Peça:");
            ui.selectable_value(&mut self.rig_shape, BS::Limb, "Membro");
            ui.selectable_value(&mut self.rig_shape, BS::Torso, "Tronco");
            ui.selectable_value(&mut self.rig_shape, BS::Hip, "Quadril");
            ui.selectable_value(&mut self.rig_shape, BS::Head, "Cabeça");
            ui.selectable_value(&mut self.rig_shape, BS::Mao, "Mão");
            ui.selectable_value(&mut self.rig_shape, BS::Pe, "Pé");
        }
        ui.separator();
        ui.weak("Painel Rig (à direita) gerencia os esqueletos.");
    }

    /// Overlay dos esqueletos (ossos + articulações) sobre o canvas.
    fn desenhar_rig(&self, ui: &egui::Ui, rect: egui::Rect, zoom: f32) {
        if !self.show_bones {
            return;
        }
        let painter = ui.painter_at(rect);
        let sp = |x: f32, y: f32| egui::pos2(rect.min.x + x * zoom, rect.min.y + y * zoom);
        // Mapeia as 4 quinas da imagem da peça para a tela.
        let corners = |aspect: f32, oi: (f32, f32), ti: (f32, f32), wo: (f32, f32), wt: (f32, f32), axis: Option<f32>| -> [egui::Pos2; 4] {
            let c = [(0.0, 0.0), (1.0, 0.0), (1.0, aspect), (0.0, aspect)];
            let mut out = [egui::pos2(0.0, 0.0); 4];
            for (k, &(cx, cy)) in c.iter().enumerate() {
                let (wx, wy) = map_piece(oi, ti, wo, wt, cx, cy);
                out[k] = sp(refl_x(axis, wx), wy);
            }
            out
        };
        let active = self.rig_active_index();
        for (si, sk) in self.document.skeletons.iter().enumerate() {
            if !sk.visible {
                continue;
            }
            let is_active = active == Some(si);
            let axis = self.rig_axis(si);
            for b in &sk.bones {
                let (aspect, oi, ti, _conns) = self.bone_geom(b.shape, b.img);
                let wo = sk.origin(b.id);
                let wt = sk.tip(b.id);
                let v = corners(aspect, oi, ti, wo, wt, axis);
                if let Some(tex) = self.bone_tex(b.shape, b.img) {
                    let uv = [
                        egui::pos2(0.0, 0.0),
                        egui::pos2(1.0, 0.0),
                        egui::pos2(1.0, 1.0),
                        egui::pos2(0.0, 1.0),
                    ];
                    let mut mesh = egui::Mesh::with_texture(tex.id());
                    for k in 0..4 {
                        mesh.vertices.push(egui::epaint::Vertex {
                            pos: v[k],
                            uv: uv[k],
                            color: egui::Color32::WHITE,
                        });
                    }
                    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
                    painter.add(egui::Shape::mesh(mesh));
                } else {
                    let o = sp(refl_x(axis, wo.0), wo.1);
                    let t = sp(refl_x(axis, wt.0), wt.1);
                    painter.add(egui::Shape::convex_polygon(
                        rig_shape_points(b.shape, o, t),
                        egui::Color32::from_rgba_unmultiplied(0xFF, 0x9F, 0x1C, 70),
                        egui::Stroke::new(2.0, egui::Color32::from_rgb(0xFF, 0x9F, 0x1C)),
                    ));
                }
                if is_active && self.rig_sel_bone == Some(b.id) {
                    let azul = egui::Color32::from_rgb(0x2F, 0x84, 0xFE);
                    painter.add(egui::Shape::closed_line(
                        v.to_vec(),
                        egui::Stroke::new(2.0, azul),
                    ));
                    if self.rig_resize_enabled {
                        // alça de redimensionar (bola da ponta)
                        let tb = sp(refl_x(axis, wt.0), wt.1);
                        painter.circle_stroke(tb, 7.0, egui::Stroke::new(2.0, azul));
                    }
                }
            }
        }
        // Realce do encaixe (durante o arrasto para conectar).
        let hax = active.and_then(|si| self.rig_axis(si));
        if let Some((_, hx, hy)) = self.rig_snap_hint {
            painter.circle_stroke(
                sp(refl_x(hax, hx), hy),
                12.0,
                egui::Stroke::new(2.5, egui::Color32::from_rgb(0x3A, 0xC0, 0x50)),
            );
        }
        // Preview da peça sendo criada (arrastar em modo Criar).
        if self.tool == Tool::Rig && self.rig_mode == RigMode::Create {
            if let (Some(a), Some(b)) = (self.rig_start, self.rig_preview) {
                let pax = active.and_then(|si| self.rig_axis(si));
                let (aspect, oi, ti, _) = self.bone_geom(self.rig_shape, None);
                let v = corners(aspect, oi, ti, a, b, pax);
                painter.add(egui::Shape::closed_line(
                    v.to_vec(),
                    egui::Stroke::new(1.5, egui::Color32::from_rgb(0x2F, 0x84, 0xFE)),
                ));
            }
        }
    }

    /// Janela: Rig — lista de esqueletos e modos de edição (painel direito).
    fn janela_rig(&mut self, ctx: &egui::Context) {
        use egui_phosphor::regular as icon;
        self.ensure_part_textures(ctx);
        let mut open = self.win_rig;

        let mut add_skel = false;
        let mut del_skel: Option<usize> = None;
        let mut set_active: Option<usize> = None;
        let mut toggle_vis: Option<usize> = None;
        let mut set_mode: Option<RigMode> = None;
        let mut do_reset = false;
        let mut mirror_skel = false;
        let mut do_setrest = false;
        let mut del_bone = false;
        let mut separar_bone = false;
        let mut insert_preset: Option<&'static str> = None;

        let mut win = egui::Window::new("Rig — esqueletos")
            .open(&mut open)
            .default_width(300.0);
        if let Some(r) = self.icon_r_rig {
            let pos = egui::pos2(r.left() - 8.0, r.top());
            win = win.pivot(egui::Align2::RIGHT_TOP);
            win = if self.reopen_rig { win.current_pos(pos) } else { win.default_pos(pos) };
        }
        win.show(ctx, |ui| {
            ui.checkbox(&mut self.show_bones, "Mostrar esqueletos");
            ui.add_space(2.0);

            egui::CollapsingHeader::new("Esqueletos prontos")
                .default_open(true)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for (lbl, key) in
                            [("Humano", "humano"), ("Cavalo", "cavalo"), ("Raptor", "raptor")]
                        {
                            let (rct, resp) = ui
                                .allocate_exact_size(egui::vec2(82.0, 92.0), egui::Sense::click());
                            let pt = ui.painter_at(rct);
                            let bg = if resp.hovered() {
                                egui::Color32::from_gray(52)
                            } else {
                                egui::Color32::from_gray(34)
                            };
                            pt.rect_filled(rct, 6.0, bg);
                            let ir = egui::Rect::from_min_size(
                                rct.min,
                                egui::vec2(rct.width(), rct.height() - 16.0),
                            )
                            .shrink(6.0);
                            if key == "raptor" {
                                if let Some(tex) = &self.raptor_mini_tex {
                                    let sz = tex.size_vec2();
                                    let scl = (ir.width() / sz.x).min(ir.height() / sz.y);
                                    let r = egui::Rect::from_center_size(
                                        ir.center(),
                                        egui::vec2(sz.x * scl, sz.y * scl),
                                    );
                                    pt.image(
                                        tex.id(),
                                        r,
                                        egui::Rect::from_min_max(
                                            egui::pos2(0.0, 0.0),
                                            egui::pos2(1.0, 1.0),
                                        ),
                                        egui::Color32::WHITE,
                                    );
                                }
                            } else {
                                let prev = preset_skeleton(key, 0.0, 0.0, 1.0);
                                desenhar_preview_esqueleto(
                                    &pt,
                                    ir,
                                    &prev,
                                    egui::Color32::from_rgb(0x8F, 0xB7, 0xFF),
                                );
                            }
                            pt.text(
                                egui::pos2(rct.center().x, rct.bottom() - 8.0),
                                egui::Align2::CENTER_CENTER,
                                lbl,
                                egui::FontId::proportional(12.0),
                                egui::Color32::WHITE,
                            );
                            if resp.on_hover_text(format!("Inserir esqueleto {lbl}")).clicked() {
                                insert_preset = Some(key);
                            }
                        }
                    });
                });
            ui.add_space(4.0);
            ui.label(egui::RichText::new("Meus esqueletos").strong());

            let n = self.document.skeletons.len();
            if n == 0 {
                ui.weak("Nenhum ainda — insira um pronto ou crie vazio.");
            }
            let active = self.rig_active_index();
            for i in 0..n {
                let vis = self.document.skeletons[i].visible;
                let is_active = active == Some(i);
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(is_active, icon::CIRCLE)
                        .on_hover_text("Esqueleto ativo")
                        .clicked()
                    {
                        set_active = Some(i);
                    }
                    if ui
                        .selectable_label(false, if vis { icon::EYE } else { icon::EYE_SLASH })
                        .on_hover_text("Ver/ocultar")
                        .clicked()
                    {
                        toggle_vis = Some(i);
                    }
                    ui.add(
                        egui::TextEdit::singleline(&mut self.document.skeletons[i].name)
                            .desired_width(120.0),
                    );
                    let nb = self.document.skeletons[i].bones.len();
                    ui.weak(format!("{nb} osso(s)"));
                    if ui.small_button(icon::TRASH).on_hover_text("Excluir esqueleto").clicked() {
                        del_skel = Some(i);
                    }
                });
            }
            ui.separator();
            if ui.button(format!("{}  Novo (vazio)", icon::PLUS)).clicked() {
                add_skel = true;
            }

            ui.separator();
            if self.pixel_mode {
                ui.weak("Rig indisponível no modo pixel art.");
            } else if self.document.skeletons.is_empty() {
                ui.weak("Crie um esqueleto para editar.");
            } else {
                let editing = self.tool == Tool::Rig;
                ui.label("Edição:");
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(editing && self.rig_mode == RigMode::Create, "Criar ossos")
                        .clicked()
                    {
                        set_mode = Some(RigMode::Create);
                    }
                    if ui
                        .selectable_label(editing && self.rig_mode == RigMode::Pose, "Posar")
                        .clicked()
                    {
                        set_mode = Some(RigMode::Pose);
                    }
                });
                if editing && self.rig_mode == RigMode::Create {
                    ui.horizontal_wrapped(|ui| {
                        use sketchmotion_core::BoneShape as BS;
                        ui.label("Peça:");
                        ui.selectable_value(&mut self.rig_shape, BS::Limb, "Membro");
                        ui.selectable_value(&mut self.rig_shape, BS::Torso, "Tronco");
                        ui.selectable_value(&mut self.rig_shape, BS::Hip, "Quadril");
                        ui.selectable_value(&mut self.rig_shape, BS::Head, "Cabeça");
                        ui.selectable_value(&mut self.rig_shape, BS::Mao, "Mão");
                        ui.selectable_value(&mut self.rig_shape, BS::Pe, "Pé");
                    });
                }
                ui.checkbox(
                    &mut self.rig_resize_enabled,
                    "Redimensionar (arrastar a ponta)",
                );
                ui.horizontal(|ui| {
                    if ui.button("Resetar pose").clicked() {
                        do_reset = true;
                    }
                    if ui.button("Definir descanso").clicked() {
                        do_setrest = true;
                    }
                    if ui.button("Inverter (espelhar)").clicked() {
                        mirror_skel = true;
                    }
                });
                if self.rig_sel_bone.is_some() {
                    ui.horizontal(|ui| {
                        if ui.button("Separar selecionado").clicked() {
                            separar_bone = true;
                        }
                        if ui
                            .button(format!("{}  Excluir", icon::TRASH))
                            .clicked()
                        {
                            del_bone = true;
                        }
                    });
                }
                ui.weak(
                    "Criar: arraste uma peça; comece sobre um ponto de ligação para encaixar. Posar: arraste o corpo para girar, a bola da origem para mover (encaixa ao soltar perto de outro ponto) e a bola da ponta para redimensionar.",
                );
            }
        });

        if add_skel {
            let name = format!("Esqueleto {}", self.document.skeletons.len() + 1);
            self.push_undo();
            self.document
                .skeletons
                .push(sketchmotion_core::Skeleton::new(name));
            self.rig_skel = self.document.skeletons.len() - 1;
            self.rig_sel_bone = None;
            self.show_bones = true;
            self.dirty = true;
        }
        if let Some(key) = insert_preset {
            self.push_undo();
            let cx = self.document.width as f32 / 2.0;
            let cy = self.document.height as f32 / 2.0;
            let base = self.document.width.min(self.document.height) as f32;
            let sk = preset_skeleton(key, cx, cy, base * 0.26);
            self.document.skeletons.push(sk);
            self.rig_skel = self.document.skeletons.len() - 1;
            self.rig_sel_bone = None;
            self.show_bones = true;
            self.dirty = true;
        }
        if let Some(i) = del_skel {
            self.push_undo();
            if i < self.document.skeletons.len() {
                self.document.skeletons.remove(i);
            }
            if self.rig_skel >= self.document.skeletons.len() {
                self.rig_skel = self.document.skeletons.len().saturating_sub(1);
            }
            self.rig_sel_bone = None;
            self.dirty = true;
        }
        if let Some(i) = set_active {
            self.rig_skel = i;
            self.rig_sel_bone = None;
        }
        if let Some(i) = toggle_vis {
            if let Some(sk) = self.document.skeletons.get_mut(i) {
                sk.visible = !sk.visible;
            }
            self.dirty = true;
        }
        if let Some(m) = set_mode {
            self.tool = Tool::Rig;
            self.rig_mode = m;
            self.show_bones = true;
            self.eyedropper = Eyedropper::Off;
        }
        if do_reset {
            if let Some(si) = self.rig_active_index() {
                self.push_undo();
                self.document.skeletons[si].reset_pose();
                self.dirty = true;
            }
        }
        if do_setrest {
            if let Some(si) = self.rig_active_index() {
                self.document.skeletons[si].set_rest();
            }
        }
        if mirror_skel {
            if let Some(si) = self.rig_active_index() {
                self.rig_mirror(si);
            }
        }
        if del_bone {
            if let (Some(si), Some(bid)) = (self.rig_active_index(), self.rig_sel_bone) {
                self.push_undo();
                self.document.skeletons[si].remove_bone(bid);
                self.rig_sel_bone = None;
                self.dirty = true;
            }
        }
        if separar_bone {
            if let (Some(si), Some(bid)) = (self.rig_active_index(), self.rig_sel_bone) {
                self.rig_separar(si, bid);
            }
        }

        self.reopen_rig = false;
        self.win_rig = open;
    }

    /// Decodifica o GIF de splash em texturas (uma vez), guardando cada frame
    /// com sua duração em segundos.
    fn carregar_splash(&mut self, ctx: &egui::Context) {
        const GIF: &[u8] = include_bytes!("../../../assets/animação_splash_intro1.gif");
        use image::AnimationDecoder;
        let dec = match image::codecs::gif::GifDecoder::new(std::io::Cursor::new(GIF)) {
            Ok(d) => d,
            Err(_) => return,
        };
        let frames = match dec.into_frames().collect_frames() {
            Ok(f) => f,
            Err(_) => return,
        };
        for (i, fr) in frames.iter().enumerate() {
            let (num, den) = fr.delay().numer_denom_ms();
            let mut secs = if den == 0 { 0.1 } else { (num as f32 / den as f32) / 1000.0 };
            if secs <= 0.0 {
                secs = 0.05;
            }
            let buf = fr.buffer();
            let (w, h) = (buf.width() as usize, buf.height() as usize);
            let img = egui::ColorImage::from_rgba_unmultiplied([w, h], buf.as_raw());
            let tex = ctx.load_texture(format!("splash_{i}"), img, egui::TextureOptions::LINEAR);
            self.splash_frames.push((tex, secs));
        }
    }

    /// Splash de inicialização: exibe o GIF uma única vez, congela no último
    /// frame por um instante e então abre a tela inicial.
    fn tela_splash(&mut self, ctx: &egui::Context) {
        if !self.splash_loaded {
            self.splash_loaded = true;
            self.carregar_splash(ctx);
            self.splash_frame_started = Some(std::time::Instant::now());
        }
        // Falha ao decodificar → não trava a inicialização.
        if self.splash_frames.is_empty() {
            self.screen = Screen::Home;
            ctx.request_repaint();
            return;
        }
        // Ajusta a janela ao tamanho exato do GIF, sem bordas.
        let sz = self.splash_frames[0].0.size_vec2();
        if !self.splash_win_set {
            self.splash_win_set = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(sz));
        }
        // Centraliza na tela (tenta a cada frame até conhecer o tamanho do monitor).
        if !self.splash_centered {
            if let Some(mon) = ctx.input(|i| i.viewport().monitor_size) {
                let pos = ((mon - sz) * 0.5).to_pos2();
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
                self.splash_centered = true;
            }
        }
        if !self.splash_done {
            let start = self
                .splash_frame_started
                .get_or_insert_with(std::time::Instant::now);
            let delay = self.splash_frames[self.splash_idx].1;
            if start.elapsed().as_secs_f32() >= delay {
                if self.splash_idx + 1 < self.splash_frames.len() {
                    self.splash_idx += 1;
                    self.splash_frame_started = Some(std::time::Instant::now());
                } else {
                    // Último frame: congela.
                    self.splash_done = true;
                    self.splash_done_at = Some(std::time::Instant::now());
                }
            }
        } else {
            // Congelado no último frame: segura ~0,6 s e vai para a tela inicial.
            let held = self
                .splash_done_at
                .map(|t| t.elapsed().as_secs_f32())
                .unwrap_or(1.0);
            if held >= 0.6 {
                self.restaurar_janela(ctx);
                self.screen = Screen::Home;
            }
        }
        ctx.request_repaint();

        let tex_id = self.splash_frames[self.splash_idx].0.id();
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(egui::Color32::from_rgb(15, 15, 18)))
            .show(ctx, |ui| {
                // Janela == tamanho do GIF: preenche todo o painel.
                let rect = ui.max_rect();
                ui.painter().image(
                    tex_id,
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            });
    }

    /// Restaura a janela ao tamanho/estado normal (com bordas), centralizada,
    /// ao sair do splash e entrar na tela inicial.
    fn restaurar_janela(&mut self, ctx: &egui::Context) {
        let sz = egui::vec2(1120.0, 760.0);
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(sz));
        if let Some(mon) = ctx.input(|i| i.viewport().monitor_size) {
            let pos = ((mon - sz) * 0.5).to_pos2();
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
        }
    }

    /// Tela inicial: escolher o tamanho do documento (presets ou personalizado),
    /// marcar se é pixel art, e então entrar no editor.
    fn tela_inicial(&mut self, ctx: &egui::Context) {
        let mut criar: Option<(u32, u32, bool)> = None;
        let mut abrir = false;

        // Carrega logo e ícone de página uma única vez (embutidos no binário).
        if self.logo_tex.is_none() {
            const LOGO: &[u8] = include_bytes!("../../../assets/logo_horizontal.png");
            if let Ok(img) = image::load_from_memory(LOGO) {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                let ci = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw());
                self.logo_tex = Some(ctx.load_texture("logo", ci, egui::TextureOptions::LINEAR));
            }
        }
        if self.page_tex.is_none() {
            const PAGE: &[u8] = include_bytes!("../../../assets/pagina_icone.png");
            if let Ok(img) = image::load_from_memory(PAGE) {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                let ci = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw());
                self.page_tex = Some(ctx.load_texture("page", ci, egui::TextureOptions::LINEAR));
            }
        }
        if self.page_normal_tex.is_none() {
            const PAGEN: &[u8] = include_bytes!("../../../assets/pagina_icone_normal.png");
            if let Ok(img) = image::load_from_memory(PAGEN) {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                let ci = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw());
                self.page_normal_tex =
                    Some(ctx.load_texture("page_normal", ci, egui::TextureOptions::LINEAR));
            }
        }
        let page_px = self.page_tex.as_ref().map(|t| (t.id(), t.size_vec2()));
        let page_nm = self.page_normal_tex.as_ref().map(|t| (t.id(), t.size_vec2()));

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(28.0);
            ui.vertical_centered(|ui| {
                if let Some(tex) = &self.logo_tex {
                    let size = tex.size_vec2();
                    let escala = (300.0_f32 / size.x).min(1.0);
                    ui.add(egui::Image::from_texture(egui::load::SizedTexture::new(
                        tex.id(),
                        size * escala,
                    )));
                }
                ui.add_space(6.0);
                ui.heading("Vamos começar algo novo.");
                ui.add_space(2.0);
                ui.colored_label(
                    egui::Color32::from_gray(160),
                    "Escolha uma predefinição ou defina o tamanho do seu documento.",
                );
            });
            ui.add_space(22.0);

            let presets = [
                ("Ilustração", 800u32, 520u32, false),
                ("Quadrado", 1024, 1024, false),
                ("HD 1920×1080", 1920, 1080, false),
                ("Pixel art 32", 32, 32, true),
                ("Pixel art 64", 64, 64, true),
                ("Pixel art 128", 128, 128, true),
            ];
            // Centraliza a grade de cartões.
            let card = egui::vec2(150.0, 150.0);
            let gap = 14.0;
            let cols = ((ui.available_width() / (card.x + gap)).floor() as usize).clamp(1, 6);
            let grid_w = cols as f32 * card.x + (cols as f32 - 1.0) * gap;
            let indent = ((ui.available_width() - grid_w) * 0.5).max(0.0);
            ui.horizontal_wrapped(|ui| {
                ui.add_space(indent);
                ui.spacing_mut().item_spacing = egui::vec2(gap, gap);
                for (nome, w, h, px) in presets {
                    let (rect, resp) = ui.allocate_exact_size(card, egui::Sense::click());
                    let p = ui.painter_at(rect);
                    let hov = resp.hovered();
                    let bg = if hov {
                        egui::Color32::from_gray(58)
                    } else {
                        egui::Color32::from_gray(40)
                    };
                    let borda = if hov {
                        egui::Color32::from_rgb(0x2F, 0x84, 0xFE)
                    } else {
                        egui::Color32::from_gray(72)
                    };
                    p.rect_filled(rect, 8.0, bg);
                    p.rect_stroke(rect, 8.0, egui::Stroke::new(1.0, borda));
                    let icone = if px { page_px } else { page_nm };
                    if let Some((tid, tsz)) = icone {
                        let alvo = 84.0_f32;
                        let esc = (alvo / tsz.x.max(tsz.y)).min(1.0);
                        let isz = tsz * esc;
                        let center = egui::pos2(rect.center().x, rect.top() + 58.0);
                        let ir = egui::Rect::from_center_size(center, isz);
                        p.image(
                            tid,
                            ir,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                    p.text(
                        egui::pos2(rect.center().x, rect.bottom() - 34.0),
                        egui::Align2::CENTER_CENTER,
                        nome,
                        egui::FontId::proportional(14.0),
                        egui::Color32::from_gray(235),
                    );
                    p.text(
                        egui::pos2(rect.center().x, rect.bottom() - 16.0),
                        egui::Align2::CENTER_CENTER,
                        format!("{w} × {h} px"),
                        egui::FontId::proportional(11.0),
                        egui::Color32::from_gray(160),
                    );
                    if resp.clicked() {
                        criar = Some((w, h, px));
                    }
                }
            });

            ui.add_space(20.0);
            ui.separator();
            ui.add_space(8.0);
            ui.vertical_centered(|ui| {
                ui.horizontal(|ui| {
                    ui.label("Tamanho personalizado:");
                    ui.label("Largura");
                    ui.add(egui::DragValue::new(&mut self.home_w).range(1..=8192).suffix(" px"));
                    ui.label("Altura");
                    ui.add(egui::DragValue::new(&mut self.home_h).range(1..=8192).suffix(" px"));
                    ui.checkbox(&mut self.home_pixel, "Pixel art");
                    if ui
                        .add(egui::Button::new("Criar").min_size(egui::vec2(90.0, 0.0)))
                        .clicked()
                    {
                        criar = Some((self.home_w, self.home_h, self.home_pixel));
                    }
                    ui.separator();
                    if ui.button("Abrir arquivo existente…").clicked() {
                        abrir = true;
                    }
                });
            });
        });

        if let Some((w, h, px)) = criar {
            self.novo_documento(w, h, px);
            self.modificado = false;
            self.screen = Screen::Editor;
        }
        if abrir {
            // Inicia a abertura; a tela de carregamento cuida do resto e troca
            // para o editor quando terminar (aplicar_documento_aberto).
            self.abrir();
        }
    }
}

impl eframe::App for SketchMotionApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Abrir/Exportar/Salvar em andamento: tela de progresso, bloqueia o resto
        // (inclusive a tela inicial — a abertura pode começar a partir dela).
        if self.open_job.is_some() || self.busy_job.is_some() {
            self.busy_poll(ctx);
            return;
        }
        // Splash de inicialização: roda antes de tudo, uma única vez.
        if self.screen == Screen::Splash {
            self.tela_splash(ctx);
            return;
        }
        // Fechar com trabalho não salvo → pergunta antes de sair.
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.modificado && !self.confirmado_fechar && self.screen == Screen::Editor {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.win_fechar = true;
            }
        }
        if self.win_fechar {
            let mut do_salvar = false;
            let mut do_descartar = false;
            let mut do_cancelar = false;
            egui::Window::new("Salvar alterações?")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.label("O trabalho foi modificado. Deseja salvar antes de sair?");
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Salvar e sair").clicked() {
                            do_salvar = true;
                        }
                        if ui.button("Sair sem salvar").clicked() {
                            do_descartar = true;
                        }
                        if ui.button("Cancelar").clicked() {
                            do_cancelar = true;
                        }
                    });
                });
            if do_salvar {
                if self.salvar_sync() {
                    self.win_fechar = false;
                    self.confirmado_fechar = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else {
                    self.win_fechar = false; // cancelou o salvar: permanece aberto
                }
            }
            if do_descartar {
                self.win_fechar = false;
                self.confirmado_fechar = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            if do_cancelar {
                self.win_fechar = false;
            }
        }
        if self.screen == Screen::Home {
            self.tela_inicial(ctx);
            return;
        }
        // Importar imagens arrastando de fora para dentro do canvas.
        let dropped: Vec<std::path::PathBuf> =
            ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        for path in dropped {
            let ext_ok = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| {
                    matches!(
                        e.to_lowercase().as_str(),
                        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp"
                    )
                })
                .unwrap_or(false);
            if ext_ok {
                match sketchmotion_io::load_image(&path) {
                    Ok((w, h, rgba)) => self.colocar_imagem(w, h, rgba),
                    Err(e) => self.status = format!("Erro ao importar: {e}"),
                }
            } else {
                self.status = format!("Arquivo não suportado para importação: {}", path.display());
            }
        }
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let scr = ctx.screen_rect();
            let pt = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("drop_overlay"),
            ));
            pt.rect_filled(scr, 0.0, egui::Color32::from_black_alpha(120));
            pt.text(
                scr.center(),
                egui::Align2::CENTER_CENTER,
                "Solte a imagem para importar",
                egui::FontId::proportional(28.0),
                egui::Color32::WHITE,
            );
        }
        // Aviso temporário sobre o canvas (ex.: tentativa de editar camada bloqueada).
        if self.warn_ticks > 0 {
            self.warn_ticks -= 1;
            ctx.request_repaint();
            let scr = ctx.screen_rect();
            let pt = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("lock_warn"),
            ));
            let msg = self.status.clone();
            let cor_txt = egui::Color32::from_rgb(0xFF, 0xD9, 0xB3);
            let galley =
                pt.layout_no_wrap(msg, egui::FontId::proportional(15.0), cor_txt);
            let pad = egui::vec2(16.0, 10.0);
            let size = galley.size() + pad * 2.0;
            let center = egui::pos2(scr.center().x, scr.top() + 90.0);
            let rect = egui::Rect::from_center_size(center, size);
            pt.rect_filled(
                rect,
                8.0,
                egui::Color32::from_rgba_unmultiplied(60, 34, 20, 240),
            );
            pt.rect_stroke(
                rect,
                8.0,
                egui::Stroke::new(1.5, egui::Color32::from_rgb(0xE0, 0x6C, 0x3A)),
            );
            pt.galley(rect.min + pad, galley, cor_txt);
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
        if !matches!(self.tool, Tool::Select | Tool::Lasso | Tool::MagicWand) && self.float_sel.is_some() {
            self.drop_float();
        }
        if self.bloqueada_pixel(self.tool) {
            self.tool = Tool::Pencil;
        }
        let mut do_undo = false;
        let mut do_redo = false;
        let mut k_enter = false;
        let mut k_esc = false;
        let mut k_del = false;
        let mut k_copy = false;
        let mut k_cut = false;
        let mut k_paste = false;
        // Ações do menu de contexto (botão direito) do canvas.
        let mut m_copiar = false;
        let mut m_recortar = false;
        let mut m_colar = false;
        let mut m_agrupar = false;
        let mut m_desagrupar = false;
        let mut m_integrar = false;
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
            // Copiar/colar: no eframe (Windows) o Ctrl+C/Ctrl+V normalmente NÃO
            // chega como tecla — vira Event::Copy / Event::Paste. Detectamos os
            // dois caminhos para garantir que funcione.
            if i.modifiers.command && i.key_pressed(egui::Key::C) {
                k_copy = true;
            }
            if i.modifiers.command && i.key_pressed(egui::Key::V) {
                k_paste = true;
            }
            if i.modifiers.command && i.key_pressed(egui::Key::X) {
                k_cut = true;
            }
            for e in &i.events {
                match e {
                    egui::Event::Copy => k_copy = true,
                    egui::Event::Cut => k_cut = true,
                    egui::Event::Paste(_) => k_paste = true,
                    _ => {}
                }
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
            let PixelImage { width, height, rgba } = render_frame_alpha(
                self.document.width,
                self.document.height,
                &self.document.layers,
                &self.document.vectors,
                &self.document.images,
            );
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
            // Invalida só a miniatura do frame atual da faixa ativa (barato).
            let at = self.document.active_track;
            let cf = self.document.current;
            if let Some(tv) = self.track_thumbs.get_mut(at) {
                if cf < tv.len() {
                    tv[cf] = None;
                }
            }
            self.onion_for = None;
            self.split_for = None; // camadas mudaram → refazer below/above
        }
        let tex_id = self.texture.as_ref().unwrap().id();

        // Empilhamento da flutuante: monta as camadas ABAIXO (até a camada dela)
        // e ACIMA (as de cima + vetores), para ela aparecer no z-index correto.
        let float_layer = self.float_sel.as_ref().map(|f| {
            f.layer
                .min(self.document.layers.len().saturating_sub(1))
        });
        if let Some(l) = float_layer {
            if self.split_for != Some(l)
                || self.below_tex.is_none()
                || self.above_tex.is_none()
            {
                let (dw, dh) = (self.document.width, self.document.height);
                let below = render_layers_alpha(dw, dh, &self.document.layers[..=l]);
                let mut above = render_layers_alpha(dw, dh, &self.document.layers[l + 1..]);
                self.rasterizar_vetores(&mut above.rgba, dw, dh);
                // Objetos de imagem já soltos ficam POR CIMA (topo) — para não
                // sumirem enquanto outra flutuante está em edição.
                rasterize_images(dw, dh, &self.document.images, &mut above.rgba);
                let bimg = egui::ColorImage::from_rgba_unmultiplied(
                    [dw as usize, dh as usize],
                    &below.rgba,
                );
                let aimg = egui::ColorImage::from_rgba_unmultiplied(
                    [dw as usize, dh as usize],
                    &above.rgba,
                );
                self.below_tex =
                    Some(ctx.load_texture("below", bimg, egui::TextureOptions::NEAREST));
                self.above_tex =
                    Some(ctx.load_texture("above", aimg, egui::TextureOptions::NEAREST));
                self.split_for = Some(l);
            }
        }
        // Qual textura serve de base do canvas: sem flutuante = frame inteiro;
        // com flutuante = só as camadas de baixo (o resto vai por cima dela).
        let base_id = if float_layer.is_some() {
            self.below_tex.as_ref().map(|t| t.id()).unwrap_or(tex_id)
        } else {
            tex_id
        };

        let mut a_novo = false;
        let mut a_abrir = false;
        let mut a_salvar = false;
        let mut a_salvar_como = false;
        let mut a_exportar = false;
        let mut a_importar = false;
        let mut a_export_gif = false;
        let mut a_export_mp4 = false;
        let mut a_export_seq = false;
        let mut a_export_sheet = false;
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
                    if ui.button("Importar imagem...").clicked() {
                        a_importar = true;
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
                    if ui.button("Exportar imagem...").clicked() {
                        a_exportar = true;
                        ui.close_menu();
                    }
                    if ui.button("Exportar animação (GIF)...").clicked() {
                        a_export_gif = true;
                        ui.close_menu();
                    }
                    if ui.button("Exportar vídeo (MP4)...").clicked() {
                        a_export_mp4 = true;
                        ui.close_menu();
                    }
                    if ui.button("Exportar sequência PNG...").clicked() {
                        a_export_seq = true;
                        ui.close_menu();
                    }
                    if ui.button("Exportar sprite sheet (PNG)...").clicked() {
                        a_export_sheet = true;
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
                ui.separator();
                ui.label("Escala export:")
                    .on_hover_text("Multiplica o tamanho na exportação (pixel art nítido)");
                ui.add(
                    egui::DragValue::new(&mut self.export_scale)
                        .range(1..=16)
                        .suffix("x"),
                );
                nudge_u32(ui, &mut self.export_scale, 1, 16);
                ui.label("Colunas:")
                    .on_hover_text("Colunas do sprite sheet (0 = tudo numa linha)");
                ui.add(egui::DragValue::new(&mut self.export_cols).range(0..=64));
                nudge_u32(ui, &mut self.export_cols, 0, 64);
                ui.separator();
                ui.checkbox(&mut self.export_camera, "Exportar pela câmera")
                    .on_hover_text(
                        "Aplica o enquadramento da câmera (keyframes) no GIF/MP4/PNG. \
                         Desligado exporta a composição inteira.",
                    );
                ui.separator();
                if ui
                    .checkbox(&mut self.bg_white, "Fundo branco")
                    .on_hover_text("Só visual — o arquivo salvo/exportado é sempre transparente")
                    .changed()
                    && self.bg_white
                {
                    self.bg_dark = false;
                }
                if ui
                    .checkbox(&mut self.bg_dark, "Fundo escuro")
                    .on_hover_text("Cinza bem escuro — ajuda a enxergar detalhes claros. Só visual.")
                    .changed()
                    && self.bg_dark
                {
                    self.bg_white = false;
                }
                let mut pm = self.pixel_mode;
                if ui
                    .checkbox(&mut pm, "Pixel art")
                    .on_hover_text("Grade + pincel quadrado (1px). Fica salvo no arquivo.")
                    .changed()
                {
                    self.pixel_mode = pm;
                    self.document.pixel_art = pm;
                    self.dirty = true;
                }
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
        if a_importar {
            self.importar();
        }
        if a_export_gif {
            self.iniciar_export(ExportKind::Gif);
        }
        if a_export_mp4 {
            self.iniciar_export(ExportKind::Mp4);
        }
        if a_export_seq {
            self.iniciar_export(ExportKind::Seq);
        }
        if a_export_sheet {
            self.iniciar_export(ExportKind::Sheet);
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
            if k_del && matches!(self.tool, Tool::Select | Tool::Lasso | Tool::MagicWand) {
                if self.float_sel.is_some() {
                    self.float_sel = None;
                    self.float_tex = None;
                    self.dirty = true;
                } else if !self.sel_set.is_empty() {
                    // Apaga TODOS os objetos selecionados (conjunto/grupo).
                    self.push_undo();
                    let mut idxs = self.sel_set.clone();
                    idxs.sort_unstable();
                    idxs.dedup();
                    for &i in idxs.iter().rev() {
                        if i < self.document.vectors.len() {
                            self.document.vectors.remove(i);
                        }
                    }
                    self.sel_set.clear();
                    self.selected_obj = None;
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
        // Terceira barra: arquivo/timeline/frame/camada selecionados.
        self.barra_status(ctx);
        // Quarta barra: ações (integrar imagem etc.).
        self.barra_acoes(ctx);
        // Ferramentas (esquerda) e painéis (direita), sempre visíveis.
        self.barra_ferramentas(ctx);
        self.barra_icones(ctx);
        self.janela_cor(ctx);
        self.janela_paletas(ctx);
        self.janela_camadas(ctx);
        self.janela_rig(ctx);
        self.janela_objetos(ctx);
        self.janela_editar_peca(ctx);
        self.janela_prancheta(ctx);
        self.janela_camera(ctx);
        self.janela_trace(ctx);
        self.janela_pivo(ctx);
        self.janela_dirvec(ctx);
        self.trace_poll(ctx);
        self.ensure_piece_textures(ctx);
        self.ensure_part_textures(ctx);
        // Rodapé (fica no fundo, criado antes da timeline).
        self.rodape(ctx);
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
            let avail = ui.available_size();
            let cw = doc_w * zoom;
            let ch = doc_h * zoom;
            // "Pasteboard": margem ao redor do canvas para centralizá-lo e poder
            // movê-lo com rolagem H/V (estilo Illustrator).
            let pad_x = (avail.x * self.workspace_pad).max(120.0);
            let pad_y = (avail.y * self.workspace_pad).max(120.0);
            let mut area = egui::ScrollArea::both()
                .auto_shrink([false, false])
                .drag_to_scroll(false);
            if std::mem::take(&mut self.center_canvas) {
                let off = egui::vec2(
                    (pad_x + cw * 0.5 - avail.x * 0.5).max(0.0),
                    (pad_y + ch * 0.5 - avail.y * 0.5).max(0.0),
                );
                area = area.scroll_offset(off);
            }
            area
                .show(ui, |ui| {
                    // Conteúdo = canvas + margem simétrica (pasteboard).
                    let content = egui::vec2(cw + 2.0 * pad_x, ch + 2.0 * pad_y);
                    let (content_rect, _cresp) =
                        ui.allocate_exact_size(content, egui::Sense::hover());
                    let rect = egui::Rect::from_min_size(
                        content_rect.min + egui::vec2(pad_x, pad_y),
                        egui::vec2(cw, ch),
                    );
                    let response = ui.interact(
                        rect,
                        ui.id().with("canvas_area"),
                        egui::Sense::click_and_drag(),
                    );
                    // Botão direito: seleciona o objeto sob o cursor (se houver) e
                    // abre o menu de contexto com as ações à mão.
                    if response.secondary_clicked() {
                        if let Some(p) = response.interact_pointer_pos() {
                            let dp = ((p.x - rect.min.x) / zoom, (p.y - rect.min.y) / zoom);
                            let thr = 6.0 / zoom;
                            if let Some(i) = self.hit_test(dp, thr) {
                                let g = self.document.vectors[i].group;
                                if let Some(g) = g {
                                    self.sel_set = (0..self.document.vectors.len())
                                        .filter(|&k| self.document.vectors[k].group == Some(g))
                                        .collect();
                                } else if !self.sel_set.contains(&i) {
                                    self.sel_set = vec![i];
                                }
                                self.selected_obj = Some(i);
                            }
                        }
                    }
                    response.context_menu(|ui| {
                        ui.set_min_width(150.0);
                        if ui.button("Copiar").clicked() {
                            m_copiar = true;
                            ui.close_menu();
                        }
                        if ui.button("Recortar").clicked() {
                            m_recortar = true;
                            ui.close_menu();
                        }
                        if ui.button("Colar").clicked() {
                            m_colar = true;
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Agrupar").clicked() {
                            m_agrupar = true;
                            ui.close_menu();
                        }
                        if ui.button("Desagrupar").clicked() {
                            m_desagrupar = true;
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Integrar").clicked() {
                            m_integrar = true;
                            ui.close_menu();
                        }
                    });
                    // Fundo xadrez indica transparência; a imagem (com alfa) vai por cima.
                    {
                        let p = ui.painter_at(rect);
                        if self.bg_white {
                            p.rect_filled(rect, 0.0, egui::Color32::WHITE);
                        } else if self.bg_dark {
                            p.rect_filled(rect, 0.0, egui::Color32::from_gray(28));
                        } else {
                        let cell = 8.0_f32.max(zoom);
                        p.rect_filled(rect, 0.0, egui::Color32::from_gray(210));
                        let nx = (rect.width() / cell).ceil() as i32;
                        let ny = (rect.height() / cell).ceil() as i32;
                        for j in 0..ny {
                            for i in 0..nx {
                                if (i + j) % 2 == 0 {
                                    continue;
                                }
                                let x = rect.left() + i as f32 * cell;
                                let y = rect.top() + j as f32 * cell;
                                let cr = egui::Rect::from_min_size(
                                    egui::pos2(x, y),
                                    egui::vec2(cell, cell),
                                )
                                .intersect(rect);
                                p.rect_filled(cr, 0.0, egui::Color32::from_gray(165));
                            }
                        }
                        }
                        p.image(
                            base_id,
                            rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }

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

                    if self.onion && !self.document.onion_between && self.document.current > 0 {
                        let prev = self.document.current - 1;
                        if self.onion_for != Some(prev) || self.onion_tex.is_none() {
                            let oi = render_frame_alpha(
                                self.document.width,
                                self.document.height,
                                &self.document.frames[prev].layers,
                                &self.document.frames[prev].vectors,
                                &self.document.frames[prev].images,
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

                    // Onion entre timelines / ver ambas: desenha as outras faixas.
                    // As texturas ficam em cache; só refazemos quando muda a
                    // página, a faixa ativa, o nº de faixas ou o modo de exibição.
                    if (self.document.onion_between || self.view_both)
                        && self.document.tracks.len() > 1
                    {
                        let cur = self.document.current;
                        let act = self.document.active_track;
                        let ntr = self.document.tracks.len();
                        let alpha: u8 = if self.document.onion_between { 90 } else { 200 };
                        let key = (act, cur, ntr, self.document.onion_between, self.view_both);
                        if self.between_key != Some(key) {
                            let mut cache: Vec<Option<egui::TextureHandle>> = vec![None; ntr];
                            for ti in 0..ntr {
                                if ti == act {
                                    continue;
                                }
                                let visible = self.document.tracks[ti].visible;
                                let flen = self.document.tracks[ti].frames.len();
                                if !visible || flen == 0 {
                                    continue;
                                }
                                let page = if self.document.onion_between {
                                    cur.min(flen - 1)
                                } else {
                                    self.document.tracks[ti].current.min(flen - 1)
                                };
                                let oi = render_frame_alpha(
                                    self.document.width,
                                    self.document.height,
                                    &self.document.tracks[ti].frames[page].layers,
                                    &self.document.tracks[ti].frames[page].vectors,
                                    &self.document.tracks[ti].frames[page].images,
                                );
                                let ci = egui::ColorImage::from_rgba_unmultiplied(
                                    [oi.width as usize, oi.height as usize],
                                    &oi.rgba,
                                );
                                cache[ti] = Some(ui.ctx().load_texture(
                                    format!("track_overlay_{ti}"),
                                    ci,
                                    egui::TextureOptions::NEAREST,
                                ));
                            }
                            self.between_cache = cache;
                            self.between_key = Some(key);
                        }
                        for ti in 0..ntr {
                            if let Some(Some(tex)) = self.between_cache.get(ti) {
                                ui.painter_at(rect).image(
                                    tex.id(),
                                    rect,
                                    egui::Rect::from_min_max(
                                        egui::pos2(0.0, 0.0),
                                        egui::pos2(1.0, 1.0),
                                    ),
                                    egui::Color32::from_white_alpha(alpha),
                                );
                            }
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

                    // Prancheta aberta: mostra as alças e suspende o desenho normal.
                    if self.win_prancheta {
                        self.prancheta_handles(ui, rect, zoom);
                    }
                    // Câmera: retângulo azul do enquadramento (janela aberta ou ferramenta ativa).
                    if self.win_camera || self.tool == Tool::Camera {
                        self.desenhar_camera(ui, rect, zoom);
                    }
                    // Traçado de Imagem: prévia dos contornos vetorizados.
                    if self.win_trace && self.trace_src.is_some() {
                        self.desenhar_trace_preview(ui, rect, zoom);
                    }
                    // Seleção múltipla de vetores: contorno em cada objeto.
                    if self.tool == Tool::Select {
                        self.desenhar_sel_set(ui, rect, zoom);
                    }
                    // Pivô: pontos azul (eixo) e laranja (movimentação).
                    if self.tool == Tool::Pivot || self.win_pivot {
                        self.desenhar_pivos(ui, rect, zoom);
                    }
                    // Vetor de Direção: trajetória (linha reta ou arco orbital).
                    if self.tool == Tool::DirVector || self.win_dirvec {
                        self.dirvec_tick_ghost();
                        self.dirvec_prepara_ghost_tex(ui.ctx());
                        self.desenhar_dirvec(ui, rect, zoom);
                    }
                    let pressed = pressed && !self.win_prancheta;
                    let down = down && !self.win_prancheta;

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
                        // Rig: clicar num osso comporta-se como Posar (mover/girar/redimensionar).
                        if pressed && self.sel_rig.is_none() && self.float_sel.is_none() {
                            if let Some(p) = hover {
                                self.sel_rig = self.rig_pick_any(to_doc(p));
                            }
                        }
                        let on_rig = self.sel_rig.is_some();
                        if on_rig {
                            if let Some(si) = self.sel_rig {
                                if down {
                                    if let Some(p) = ppos {
                                        let d = to_doc(p);
                                        self.rig_drag_to(si, (refl_x(self.rig_axis(si), d.0), d.1));
                                    }
                                }
                                if !down {
                                    self.rig_release_pose(si);
                                    self.sel_rig = None;
                                }
                            }
                            self.last_pos = None;
                        }
                        if !on_rig {
                        if pressed {
                            if let Some(p) = hover {
                                let dp = to_doc(p);
                                // seleção flutuante (raster): dentro move; fora confirma
                                let consumed = self.float_press(p, rect, zoom);
                                if !consumed && self.float_sel.is_some() {
                                    self.drop_float();
                                }
                                if !consumed {
                                    // Caminho B: clicar sobre um objeto de imagem
                                    // o "pega" para editar (vira flutuante). Tem
                                    // prioridade sobre traços vetoriais/marquee.
                                    self.pick_image_at(dp);
                                    let mut grabbed = self.float_sel.is_some();
                                    if !grabbed { if let Some(si) = self.selected_obj {
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
                                    } }
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
                                            let shift = ui.input(|i| i.modifiers.shift);
                                            match self.hit_test(dp, thr) {
                                                Some(i) => {
                                                    self.push_undo();
                                                    if shift {
                                                        // Shift-clique: alterna no conjunto.
                                                        if let Some(p) =
                                                            self.sel_set.iter().position(|&x| x == i)
                                                        {
                                                            self.sel_set.remove(p);
                                                        } else {
                                                            self.sel_set.push(i);
                                                        }
                                                    } else {
                                                        // Clique: se o objeto tem grupo, seleciona o
                                                        // grupo todo; senão, ele sozinho (a não ser
                                                        // que já esteja no conjunto atual, p/ mover junto).
                                                        let g = self.document.vectors[i].group;
                                                        if let Some(g) = g {
                                                            self.sel_set = (0..self
                                                                .document
                                                                .vectors
                                                                .len())
                                                                .filter(|&k| {
                                                                    self.document.vectors[k].group
                                                                        == Some(g)
                                                                })
                                                                .collect();
                                                        } else if !self.sel_set.contains(&i) {
                                                            self.sel_set = vec![i];
                                                        }
                                                    }
                                                    self.selected_obj = Some(i);
                                                    self.dragging_obj = true;
                                                }
                                                None => {
                                                    if !shift {
                                                        self.sel_set.clear();
                                                    }
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
                            if pdelta.x != 0.0 || pdelta.y != 0.0 {
                                let (dx, dy) = (pdelta.x / zoom, pdelta.y / zoom);
                                // Move TODOS os objetos do conjunto (grupo/seleção).
                                for idx in 0..self.sel_set.len() {
                                    let i = self.sel_set[idx];
                                    if i < self.document.vectors.len() {
                                        self.document.vectors[i].translate(dx, dy);
                                    }
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
                                // Marca sobre vetores = seleção múltipla; senão, lift raster.
                                let picked = self.vetores_na_marca(start, self.marquee_cur);
                                if !picked.is_empty() {
                                    self.selected_obj = picked.first().copied();
                                    self.sel_set = picked;
                                } else {
                                    self.lift_selection(start, self.marquee_cur);
                                }
                            }
                        }
                        self.last_pos = None;
                        }
                    } else if self.tool == Tool::Lasso {
                        if pressed {
                            if let Some(pp) = hover {
                                let consumed = self.float_press(pp, rect, zoom);
                                if !consumed && self.float_sel.is_some() {
                                    self.drop_float();
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
                    } else if self.tool == Tool::MagicWand {
                        if pressed {
                            if let Some(pp) = hover {
                                let consumed = self.float_press(pp, rect, zoom);
                                if !consumed {
                                    if self.float_sel.is_some() {
                                        self.drop_float();
                                    }
                                    let (x, y) = to_pixel(pp);
                                    self.lift_wand(x, y);
                                }
                            }
                        }
                        if down {
                            self.float_down(ppos, rect, zoom);
                        }
                        if !down {
                            self.float_release();
                        }
                        self.last_pos = None;
                    } else if self.tool == Tool::Grupo {
                        if self.place_piece && self.selected_piece.is_some() {
                            // Peça armada: um clique cola no ponto clicado.
                            if pressed {
                                if let Some(pp) = hover {
                                    let (dx, dy) = to_doc(pp);
                                    self.colar_peca(dx, dy);
                                }
                            }
                        } else {
                            // Laço para capturar/agrupar (não destrutivo).
                            if pressed {
                                if let Some(pp) = hover {
                                    self.lasso_points = vec![to_doc(pp)];
                                }
                            }
                            if down && !self.lasso_points.is_empty() {
                                if let Some(pp) = ppos {
                                    self.lasso_points.push(to_doc(pp));
                                }
                            }
                            if !down && !self.lasso_points.is_empty() {
                                let pts = std::mem::take(&mut self.lasso_points);
                                self.capturar_grupo(&pts);
                                // Laço é uma ação única: volta para Seleção (cursor normal).
                                self.tool = Tool::Select;
                            }
                        }
                        self.last_pos = None;
                    } else if self.tool == Tool::Camera {
                        self.interacao_camera(pressed, down, hover, ppos, rect, zoom);
                        self.last_pos = None;
                    } else if self.tool == Tool::Pivot {
                        self.interacao_pivo(pressed, down, hover, ppos, pdelta, rect, zoom);
                        self.last_pos = None;
                    } else if self.tool == Tool::DirVector {
                        self.interacao_dirvec(pressed, down, hover, ppos, pdelta, rect, zoom);
                        self.last_pos = None;
                    } else if self.tool == Tool::Rig {
                        if let Some(si) = self.rig_active_index() {
                            match self.rig_mode {
                                RigMode::Create => {
                                    let ax = self.rig_axis(si);
                                    if pressed {
                                        if let Some(pp) = hover {
                                            let d = to_doc(pp);
                                            self.rig_start = Some((refl_x(ax, d.0), d.1));
                                        }
                                    }
                                    if down {
                                        if let Some(pp) = ppos {
                                            let d = to_doc(pp);
                                            self.rig_preview = Some((refl_x(ax, d.0), d.1));
                                        }
                                    }
                                    if !down {
                                        if let Some(start) = self.rig_start.take() {
                                            if let Some(pp) = ppos {
                                                let d = to_doc(pp);
                                                self.rig_create_bone(si, start, (refl_x(ax, d.0), d.1));
                                            }
                                        }
                                        self.rig_preview = None;
                                    }
                                }
                                RigMode::Pose => {
                                    let ax = self.rig_axis(si);
                                    if pressed {
                                        if let Some(pp) = hover {
                                            let d = to_doc(pp);
                                            self.rig_pick(si, (refl_x(ax, d.0), d.1), self.rig_resize_enabled);
                                        }
                                    }
                                    if down {
                                        if let Some(pp) = ppos {
                                            let d = to_doc(pp);
                                            self.rig_drag_to(si, (refl_x(ax, d.0), d.1));
                                        }
                                    }
                                    if !down {
                                        self.rig_release_pose(si);
                                    }
                                }
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
                            self.smudge = None;
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

                    if matches!(self.tool, Tool::Lasso | Tool::Grupo)
                        && self.lasso_points.len() >= 2
                    {
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
                        self.ensure_cursor_textures(ui.ctx());
                        let painter = ui.painter_at(rect);
                        if self.eyedropper != Eyedropper::Off {
                            match hover {
                                Some(hp)
                                    if self.desenhar_cursor_img(
                                        &painter,
                                        ui.ctx(),
                                        hp,
                                        2,
                                        (0.0647, 0.0279),
                                    ) => {}
                                _ => ui.ctx().set_cursor_icon(CI::Crosshair),
                            }
                        } else {
                            match self.tool {
                                Tool::Pencil | Tool::Eraser => {
                                    if let Some(hp) = hover {
                                        if self.pixel_mode {
                                            // Contorno do quadrado exato que será pintado.
                                            let s = self.active_radius().max(1);
                                            let half = (s - 1) / 2;
                                            let cellx = ((hp.x - rect.min.x) / zoom).floor() as i32;
                                            let celly = ((hp.y - rect.min.y) / zoom).floor() as i32;
                                            let (tlx, tly) = (cellx - half, celly - half);
                                            let r0 = egui::Rect::from_min_size(
                                                egui::pos2(
                                                    rect.min.x + tlx as f32 * zoom,
                                                    rect.min.y + tly as f32 * zoom,
                                                ),
                                                egui::vec2(s as f32 * zoom, s as f32 * zoom),
                                            );
                                            painter.rect_stroke(
                                                r0,
                                                0.0,
                                                egui::Stroke::new(
                                                    1.5_f32,
                                                    egui::Color32::from_black_alpha(180),
                                                ),
                                            );
                                            painter.rect_stroke(
                                                r0.expand(1.0),
                                                0.0,
                                                egui::Stroke::new(
                                                    1.0_f32,
                                                    egui::Color32::from_white_alpha(180),
                                                ),
                                            );
                                        } else {
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
                                    }
                                    ui.ctx().set_cursor_icon(CI::None);
                                }
                                Tool::Shapes => {
                                    ui.ctx().set_cursor_icon(CI::Crosshair);
                                }
                                Tool::Pen => {
                                    match hover {
                                        Some(hp)
                                            if self.desenhar_cursor_img(
                                                &painter,
                                                ui.ctx(),
                                                hp,
                                                0,
                                                (0.0568, 0.0427),
                                            ) => {}
                                        _ => ui.ctx().set_cursor_icon(CI::Crosshair),
                                    }
                                }
                                Tool::Fill => {
                                    match hover {
                                        Some(hp)
                                            if self.desenhar_cursor_img(
                                                &painter,
                                                ui.ctx(),
                                                hp,
                                                1,
                                                (0.0297, 0.7874),
                                            ) => {}
                                        _ => ui.ctx().set_cursor_icon(CI::Crosshair),
                                    }
                                }
                                Tool::Rig => {
                                    let c = if self.rig_mode == RigMode::Create {
                                        CI::Crosshair
                                    } else {
                                        CI::Grab
                                    };
                                    ui.ctx().set_cursor_icon(c);
                                }
                                Tool::Lasso | Tool::MagicWand => {
                                    if let Some(c) =
                                        hover.and_then(|hp| self.float_cursor(hp, rect, zoom))
                                    {
                                        ui.ctx().set_cursor_icon(c);
                                    } else if let Some(hp) = hover {
                                        let (idx, hs) = if self.tool == Tool::Lasso {
                                            (3usize, (0.0426_f32, 0.0416_f32))
                                        } else {
                                            (4usize, (0.2286_f32, 0.1169_f32))
                                        };
                                        if !self.desenhar_cursor_img(&painter, ui.ctx(), hp, idx, hs)
                                        {
                                            ui.ctx().set_cursor_icon(CI::Crosshair);
                                        }
                                    } else {
                                        ui.ctx().set_cursor_icon(CI::Crosshair);
                                    }
                                }
                                Tool::Grupo => {
                                    if let Some(hp) = hover {
                                        // Colando peça = cursor colocar_objeto; senão = laço.
                                        let (idx, hs) = if self.place_piece
                                            && self.selected_piece.is_some()
                                        {
                                            (5usize, (0.06_f32, 0.05_f32))
                                        } else {
                                            (3usize, (0.0426_f32, 0.0416_f32))
                                        };
                                        if !self.desenhar_cursor_img(&painter, ui.ctx(), hp, idx, hs)
                                        {
                                            ui.ctx().set_cursor_icon(CI::Crosshair);
                                        }
                                    } else {
                                        ui.ctx().set_cursor_icon(CI::Crosshair);
                                    }
                                }
                                Tool::Select => 'sel: {
                                    if let Some(c) =
                                        hover.and_then(|hp| self.float_cursor(hp, rect, zoom))
                                    {
                                        ui.ctx().set_cursor_icon(c);
                                        break 'sel;
                                    }
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
                    self.desenhar_rig(ui, rect, zoom);

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
                        // Camadas ACIMA da flutuante desenham por cima dela, para
                        // ela ficar no z-index correto (não sempre no topo).
                        if let Some(atex) = &self.above_tex {
                            painter.image(
                                atex.id(),
                                rect,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                egui::Color32::WHITE,
                            );
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
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-66.0, 96.0))
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
        // Ações do menu de contexto (botão direito) — reaproveitam os mesmos
        // caminhos do teclado.
        if m_copiar {
            k_copy = true;
        }
        if m_recortar {
            k_cut = true;
        }
        if m_colar {
            k_paste = true;
        }
        if m_agrupar {
            self.agrupar_selecao();
        }
        if m_desagrupar {
            self.desagrupar_selecao();
        }
        if m_integrar {
            if self.float_sel.is_some() {
                self.push_undo();
                self.commit_float();
                self.status = "Imagem integrada aos pixels".into();
            } else {
                self.integrar_todas_imagens();
            }
        }
        // Copiar/Recortar (Ctrl+C / Ctrl+X). Prioridade: objetos vetoriais
        // selecionados → objeto de imagem em edição → seleção raster (pixels).
        // Fica aqui, fora do gate de teclado, igual ao undo/redo.
        if k_copy || k_cut {
            let recortar = k_cut;
            if !self.objetos_selecionados().is_empty() {
                self.copiar_objetos(recortar);
            } else if matches!(&self.float_sel, Some(f) if f.is_image) {
                if let Some(f) = &self.float_sel {
                    let im = ImageObject::new(
                        f.pixels.clone(), f.ow, f.oh, f.cx, f.cy, f.hw, f.hh, f.angle, f.opacity,
                        f.layer,
                    );
                    self.obj_clip = ObjClip { vectors: Vec::new(), images: vec![im] };
                    self.clip_objetos = true;
                }
                if recortar {
                    self.float_sel = None;
                    self.float_tex = None;
                    self.dirty = true;
                    self.status = "Objeto recortado — Ctrl+V para colar".into();
                } else {
                    self.status = "Objeto copiado — Ctrl+V para colar".into();
                }
            } else if self.float_sel.is_some() {
                let data = self
                    .float_sel
                    .as_ref()
                    .map(|fs| (fs.ow, fs.oh, fs.pixels.clone()));
                if let Some(d) = data {
                    self.clip = Some(d);
                    self.clip_objetos = false;
                    if recortar {
                        self.float_sel = None;
                        self.float_tex = None;
                        self.dirty = true;
                        self.status = "Seleção recortada — Ctrl+V para colar".into();
                    } else {
                        self.drop_float();
                        self.status =
                            "Seleção copiada — Ctrl+V para colar (inclusive em outro frame)".into();
                    }
                }
            } else {
                self.status = "Nada selecionado para copiar/recortar".into();
            }
        }
        // Colar (Ctrl+V). Objetos → mesma posição/tamanho; seleção raster → nova
        // flutuante no centro do canvas.
        if k_paste
            && self.clip_objetos
            && (!self.obj_clip.vectors.is_empty() || !self.obj_clip.images.is_empty())
        {
            self.colar_objetos();
        } else if k_paste {
            if let Some((ow, oh, px)) = self.clip.clone() {
                // Finaliza a colagem anterior ANTES de registrar o histórico, para
                // que cada colar seja uma ação de undo/redo separada (não uma só).
                self.drop_float();
                self.push_undo();
                let (dw, dh) = (self.document.width as f32, self.document.height as f32);
                self.float_sel = Some(FloatSel {
                    pixels: px,
                    ow,
                    oh,
                    cx: dw / 2.0,
                    cy: dh / 2.0,
                    hw: ow as f32 / 2.0,
                    hh: oh as f32 / 2.0,
                    angle: 0.0,
                    opacity: 1.0,
                    layer: self.active_layer,
                    is_image: false,
                });
                self.float_tex = None;
                self.tool = Tool::Select;
                self.selected_obj = None;
                self.dirty = true;
                self.status = "Colado — mova e confirme".into();
            }
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
        if self.pieces_dirty {
            self.salvar_pecas();
            self.pieces_dirty = false;
        }
        // Se algo mudou o desenho neste frame (ex.: mover/soltar um objeto), a
        // textura do canvas só é refeita no topo do PRÓXIMO update(). Sem um
        // novo frame, a posição antiga fica "presa" na tela até o próximo evento
        // (aquele fantasma/delay). Pedir repaint garante a atualização imediata.
        if self.dirty {
            ctx.request_repaint();
        }
    }
}
