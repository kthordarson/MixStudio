// User-adjustable interface colours. Kept in atomics so the hundreds of per-frame colour lookups stay lock-free.
use super::*;
use std::sync::atomic::{AtomicU32, Ordering};

pub(super) const SLOTS: [&str; 7] = ["Accent", "Background", "Panels", "Rows", "Lines", "Text", "Dim text"];
const DEFAULT: [[u8; 3]; 7] = [[232, 224, 0], [26, 27, 31], [36, 38, 43], [30, 32, 36], [52, 55, 61], [220, 222, 226], [140, 144, 152]];
const PRESETS: [(&str, [[u8; 3]; 7]); 3] = [
    ("Default", DEFAULT),
    ("Ocean", [[64, 196, 255], [20, 26, 34], [28, 36, 46], [24, 31, 40], [46, 58, 72], [214, 226, 238], [128, 146, 166]]),
    ("High contrast", [[255, 214, 0], [0, 0, 0], [16, 16, 16], [8, 8, 8], [90, 90, 90], [255, 255, 255], [190, 190, 190]]),
];
static THEME: [AtomicU32; 7] = [const { AtomicU32::new(0) }; 7];
static LOADED: std::sync::Once = std::sync::Once::new();

fn pack(c: [u8; 3]) -> u32 { u32::from_be_bytes([0, c[0], c[1], c[2]]) }
pub(super) fn get(i: usize) -> [u8; 3] {
    LOADED.call_once(|| set_all(DEFAULT));
    let [_, r, g, b] = THEME[i].load(Ordering::Relaxed).to_be_bytes(); [r, g, b]
}
fn col(i: usize) -> C { let [r, g, b] = get(i); C::from_rgb(r, g, b) }
fn set_all(t: [[u8; 3]; 7]) { for (a, c) in THEME.iter().zip(t) { a.store(pack(c), Ordering::Relaxed); } }
fn all() -> [[u8; 3]; 7] { std::array::from_fn(get) }

pub(super) fn accent() -> C { col(0) }
pub(super) fn bg() -> C { col(1) }
pub(super) fn panel() -> C { col(2) }
pub(super) fn row() -> C { col(3) }
pub(super) fn line() -> C { col(4) }
pub(super) fn txt() -> C { col(5) }
pub(super) fn dim() -> C { col(6) }

fn path() -> PathBuf { crate::library::config_dir().join("theme.json") }
/// Loads the saved theme (falls back to the default on a missing or unreadable file).
pub(super) fn load() {
    let t = std::fs::read(path()).ok().and_then(|b| serde_json::from_slice::<[[u8; 3]; 7]>(&b).ok()).unwrap_or(DEFAULT);
    LOADED.call_once(|| {}); set_all(t);
}
fn save() -> std::io::Result<()> {
    let p = path(); if let Some(d) = p.parent() { std::fs::create_dir_all(d)?; }
    std::fs::write(p, serde_json::to_vec(&all())?)
}

fn mix(a: C, b: C, t: f32) -> C { let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8; C::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b())) }

/// Rebuilds the egui style from the current theme.
pub(super) fn apply_style(ctx: &egui::Context) {
    let mut st = egui::Style::default();
    st.visuals = egui::Visuals::dark();
    st.visuals.panel_fill = panel();
    st.visuals.window_fill = panel();
    st.visuals.extreme_bg_color = bg();
    st.visuals.selection.bg_fill = mix(panel(), accent(), 0.45);
    st.visuals.selection.stroke = Stroke::new(1.0_f32, accent());
    st.visuals.widgets.inactive.bg_fill = mix(panel(), txt(), 0.08);
    st.visuals.widgets.inactive.weak_bg_fill = mix(panel(), txt(), 0.08);
    st.visuals.widgets.hovered.weak_bg_fill = mix(panel(), txt(), 0.15);
    for w in [&mut st.visuals.widgets.inactive, &mut st.visuals.widgets.hovered, &mut st.visuals.widgets.active] {
        w.rounding = Rounding::same(6.0);
    }
    st.visuals.widgets.noninteractive.fg_stroke.color = txt();
    st.visuals.widgets.inactive.fg_stroke.color = txt();
    st.spacing.button_padding = vec2(8.0, 4.0);
    // Calmer chrome: hairline borders, accent hint on hover, matching rounding for popups and tooltips.
    st.spacing.item_spacing = vec2(8.0, 6.0);
    st.spacing.interact_size.y = 22.0;
    st.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, line());
    st.visuals.widgets.noninteractive.rounding = Rounding::same(6.0);
    st.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, mix(line(), txt(), 0.05));
    st.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, mix(panel(), accent(), 0.6));
    st.visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, accent());
    st.visuals.window_rounding = Rounding::same(8.0);
    st.visuals.menu_rounding = Rounding::same(6.0);
    st.visuals.window_stroke = Stroke::new(1.0_f32, line());
    st.visuals.faint_bg_color = row();
    st.visuals.hyperlink_color = accent();
    st.visuals.warn_fg_color = C::from_rgb(240, 200, 80);
    ctx.set_style(st);
}

/// Contents of the colour menu in the top bar.
pub(super) fn menu(ui: &mut egui::Ui) {
    ui.label(egui::RichText::new("Colours").strong());
    let mut t = all(); let before = t;
    egui::Grid::new("theme_grid").num_columns(2).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
        for (name, c) in SLOTS.iter().zip(t.iter_mut()) { ui.label(*name); ui.color_edit_button_srgb(c); ui.end_row(); }
    });
    ui.separator();
    ui.horizontal(|ui| { ui.label("Presets:"); for (n, p) in PRESETS { if ui.button(n).clicked() { t = p; } } });
    if t != before {
        set_all(t); apply_style(ui.ctx());
        if let Err(e) = save() { ui.colored_label(C::LIGHT_RED, format!("Could not save colours: {e}")); }
    }
}
