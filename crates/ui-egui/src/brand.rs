//! The JBD logo: the isometric cube (Jabrod trademark, `assets/jbd/`, not open source) next to
//! the "JBD" name, decoded once and kept as a texture. Used in About and on the Home screen.

const LOGO: &[u8] = include_bytes!("../../../assets/jbd/jbd-logo.png");

/// The cube artwork as a texture (the same on light and dark themes).
pub fn logo(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let id = egui::Id::new("jbd-logo");
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(t);
    }
    let img = image::load_from_memory_with_format(LOGO, image::ImageFormat::Png).ok()?;
    // Large enough to stay sharp at 2x for the sizes we draw (≤ 64 pt tall).
    let img = img.resize(256, 256, image::imageops::FilterType::Lanczos3).to_rgba8();
    let color = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
    let tex = ctx.load_texture("jbd-logo", color, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    Some(tex)
}

/// Draw the cube `height` points tall followed by "JBD", with the left edge at `left_center`
/// (vertically centred). `dark` picks the text colour. Returns the drawn rect (cube and name).
pub fn paint_wordmark(ui: &egui::Ui, left_center: egui::Pos2, height: f32, dark: bool) -> Option<egui::Rect> {
    let tex = logo(ui.ctx())?;
    let [w, h] = tex.size();
    let width = height * w as f32 / h.max(1) as f32;
    let cube = egui::Rect::from_min_size(egui::pos2(left_center.x, left_center.y - height / 2.0), egui::vec2(width, height));
    ui.painter().image(tex.id(), cube, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
    let ink = if dark { egui::Color32::from_rgb(0xf4, 0xf4, 0xf5) } else { egui::Color32::from_rgb(0x0b, 0x0b, 0x0c) };
    let name = ui.painter().text(
        egui::pos2(cube.max.x + height * 0.3, left_center.y),
        egui::Align2::LEFT_CENTER,
        "JBD",
        crate::theme::Tokens::semibold(height * 0.8),
        ink,
    );
    Some(cube.union(name))
}
