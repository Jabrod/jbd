//! The JBD logo as a real 3D cube, drawn with egui meshes: the splash screen's landing spin, the
//! still logo on the sign-in screen, and JBD's one loader (a small turning cube) everywhere.
//!
//! The logo is a cube seen isometrically: turned 45° and tilted 35.264° toward you, the one angle
//! where a cube's outline is a perfect hexagon. Three faces show: the film strip on the left, the
//! timeline (with the white playhead) on the right, and the play button on top. The hidden three
//! repeat the art so the cube looks right from every angle while it spins.
//!
//! Each face is one of four PNGs in `assets/jbd/` (`cube-film`, `cube-timeline`, `cube-play`, and
//! `cube-blank` for a face whose design hasn't arrived yet), rendered from the same SVG artwork as
//! the first JBD (`jbd-media/tools/cube-faces.cjs`).
//!
//! HOW IT DRAWS: the 8 corners are rotated (yaw about the vertical axis, then pitch), projected
//! with a gentle perspective (camera 10 edges away, like the CSS original), faces turned away are
//! skipped, and each visible face is a textured grid of triangles (a grid, not two triangles, so
//! the texture follows the perspective).

use egui::{Color32, Mesh, Pos2, TextureHandle, TextureOptions, pos2};

const FILM: &[u8] = include_bytes!("../../../../assets/jbd/cube-film.png");
const TIMELINE: &[u8] = include_bytes!("../../../../assets/jbd/cube-timeline.png");
const PLAY: &[u8] = include_bytes!("../../../../assets/jbd/cube-play.png");
const BLANK: &[u8] = include_bytes!("../../../../assets/jbd/cube-blank.png");

/// The logo's pose: tilt toward you, in degrees.
pub const ISO_PITCH: f32 = 35.264;
/// The logo's pose: turn, in degrees.
pub const ISO_YAW: f32 = 45.0;

/// Where the cube is turned and how big it is drawn (1 = `edge`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub pitch: f32,
    pub yaw: f32,
    pub scale: f32,
}

impl Pose {
    /// The logo's exact pose.
    pub const LOGO: Pose = Pose { pitch: ISO_PITCH, yaw: ISO_YAW, scale: 1.0 };
}

/// How far each of the three logo faces has been "covered" with its design, 0–1 (the splash
/// reveals them one by one; 1 everywhere else).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reveal {
    pub film: f32,
    pub timeline: f32,
    pub play: f32,
}

impl Reveal {
    pub const ALL: Reveal = Reveal { film: 1.0, timeline: 1.0, play: 1.0 };
    pub const NONE: Reveal = Reveal { film: 0.0, timeline: 0.0, play: 0.0 };
}

#[derive(Clone, Copy, PartialEq)]
enum Art {
    Film,
    Timeline,
    Play,
}

/// The four face textures, decoded once per egui context.
#[derive(Clone)]
struct Textures {
    film: TextureHandle,
    timeline: TextureHandle,
    play: TextureHandle,
    blank: TextureHandle,
}

fn textures(ctx: &egui::Context) -> Option<Textures> {
    let id = egui::Id::new("jbd-cube-textures");
    if let Some(t) = ctx.data(|d| d.get_temp::<Textures>(id)) {
        return Some(t);
    }
    let load = |name: &str, bytes: &[u8]| -> Option<TextureHandle> {
        let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?.to_rgba8();
        let color = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
        Some(ctx.load_texture(name, color, TextureOptions::LINEAR))
    };
    let t = Textures {
        film: load("jbd-cube-film", FILM)?,
        timeline: load("jbd-cube-timeline", TIMELINE)?,
        play: load("jbd-cube-play", PLAY)?,
        blank: load("jbd-cube-blank", BLANK)?,
    };
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    Some(t)
}

type V3 = [f32; 3];

/// The six faces: corners in texture order (top-left, top-right, bottom-right, bottom-left) as
/// seen from outside the cube, the art, and whether it is one of the three logo faces.
const FACES: [([V3; 4], Art, bool); 6] = [
    // left (−x): film strip
    ([[-0.5, 0.5, -0.5], [-0.5, 0.5, 0.5], [-0.5, -0.5, 0.5], [-0.5, -0.5, -0.5]], Art::Film, true),
    // front (+z): timeline, seen on the right in the logo pose
    ([[-0.5, 0.5, 0.5], [0.5, 0.5, 0.5], [0.5, -0.5, 0.5], [-0.5, -0.5, 0.5]], Art::Timeline, true),
    // top (+y): play button
    ([[-0.5, 0.5, -0.5], [0.5, 0.5, -0.5], [0.5, 0.5, 0.5], [-0.5, 0.5, 0.5]], Art::Play, true),
    // right (+x), back (−z), bottom (−y): hidden in the pose, seen while spinning
    ([[0.5, 0.5, 0.5], [0.5, 0.5, -0.5], [0.5, -0.5, -0.5], [0.5, -0.5, 0.5]], Art::Film, false),
    ([[0.5, 0.5, -0.5], [-0.5, 0.5, -0.5], [-0.5, -0.5, -0.5], [0.5, -0.5, -0.5]], Art::Timeline, false),
    ([[-0.5, -0.5, 0.5], [0.5, -0.5, 0.5], [0.5, -0.5, -0.5], [-0.5, -0.5, -0.5]], Art::Play, false),
];

/// Rotate a corner: yaw about the vertical axis, then pitch (positive tilts the top toward you).
fn rotate(p: V3, pose: Pose) -> V3 {
    let (sy, cy) = pose.yaw.to_radians().sin_cos();
    let (sp, cp) = pose.pitch.to_radians().sin_cos();
    let x = p[0] * cy + p[2] * sy;
    let z = -p[0] * sy + p[2] * cy;
    let y = p[1];
    [x, y * cp - z * sp, y * sp + z * cp]
}

/// Screen position of a rotated point (z toward the viewer, camera 10 edges away).
fn project(p: V3, center: Pos2, edge: f32, scale: f32) -> Pos2 {
    const CAMERA: f32 = 10.0;
    let k = CAMERA / (CAMERA - p[2]).max(0.5) * edge * scale;
    pos2(center.x + p[0] * k, center.y - p[1] * k)
}

fn lerp3(a: V3, b: V3, t: f32) -> V3 {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Point on a face at texture coordinates (`u`, `v`).
fn on_face(c: &[V3; 4], u: f32, v: f32) -> V3 {
    lerp3(lerp3(c[0], c[1], u), lerp3(c[3], c[2], u), v)
}

/// Twice the signed area of the projected quad (positive = facing the viewer; y points down).
fn facing(q: &[Pos2; 4]) -> f32 {
    (0..4)
        .map(|i| {
            let (a, b) = (q[i], q[(i + 1) % 4]);
            a.x * b.y - b.x * a.y
        })
        .sum()
}

/// Add a face (or the top `v_max` of it) to `mesh` as a textured grid.
fn face_mesh(mesh: &mut Mesh, corners: &[V3; 4], pose: Pose, center: Pos2, edge: f32, v_max: f32, tint: Color32) {
    const N: usize = 8;
    let base = mesh.vertices.len() as u32;
    for j in 0..=N {
        let v = v_max * j as f32 / N as f32;
        for i in 0..=N {
            let u = i as f32 / N as f32;
            let p = project(rotate(on_face(corners, u, v), pose), center, edge, pose.scale);
            mesh.vertices.push(egui::epaint::Vertex { pos: p, uv: pos2(u, v), color: tint });
        }
    }
    let row = (N + 1) as u32;
    for j in 0..N as u32 {
        for i in 0..N as u32 {
            let a = base + j * row + i;
            mesh.add_triangle(a, a + 1, a + row + 1);
            mesh.add_triangle(a, a + row + 1, a + row);
        }
    }
}

/// Draw the cube at `center`, `edge` points per side, turned by `pose`, the logo faces revealed by
/// `reveal`, at `opacity`.
pub fn paint(painter: &egui::Painter, center: Pos2, edge: f32, pose: Pose, reveal: Reveal, opacity: f32) {
    let Some(tex) = textures(painter.ctx()) else { return };
    let tint = Color32::from_white_alpha((opacity.clamp(0.0, 1.0) * 255.0).round() as u8);
    for (corners, art, logo_face) in FACES.iter() {
        let q = corners.map(|c| project(rotate(c, pose), center, edge, pose.scale));
        if facing(&q) <= 0.0 {
            continue; // turned away
        }
        let (art_tex, shown) = match art {
            Art::Film => (&tex.film, reveal.film),
            Art::Timeline => (&tex.timeline, reveal.timeline),
            Art::Play => (&tex.play, reveal.play),
        };
        let shown = if *logo_face { shown.clamp(0.0, 1.0) } else { 1.0 };
        if shown < 1.0 {
            // the bare face underneath, then the design wiping down over it
            let mut blank = Mesh::with_texture(tex.blank.id());
            face_mesh(&mut blank, corners, pose, center, edge, 1.0, tint);
            painter.add(blank);
        }
        if shown > 0.0 {
            let mut m = Mesh::with_texture(art_tex.id());
            face_mesh(&mut m, corners, pose, center, edge, shown, tint);
            painter.add(m);
        }
    }
}

/// The loader: the cube turning, one turn every 2 s. Allocates `size`×`size` points, keeps
/// repainting while shown. Use it everywhere something is in progress (JBD has no other spinner).
pub fn loader(ui: &mut egui::Ui, size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        paint_loader(ui.painter(), rect.center(), size, ui.input(|i| i.time));
        ui.ctx().request_repaint();
    }
    resp
}

/// The turning cube at `center`, fitting a `size`-point square, at time `t` (seconds).
pub fn paint_loader(painter: &egui::Painter, center: Pos2, size: f32, t: f64) {
    let yaw = ISO_YAW + ((t * 180.0) % 360.0) as f32;
    // the hexagon of an isometric cube is about 1.5 edges wide
    paint(painter, center, size / 1.6, Pose { pitch: ISO_PITCH, yaw, scale: 1.0 }, Reveal::ALL, 1.0);
}

/// The still logo cube (sign-in screen, About).
pub fn still(ui: &mut egui::Ui, size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    paint(ui.painter(), rect.center(), size / 1.6, Pose::LOGO, Reveal::ALL, 1.0);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_logo_pose_shows_exactly_the_three_logo_faces() {
        let shown: Vec<bool> = FACES
            .iter()
            .map(|(c, _, _)| {
                let q = c.map(|p| project(rotate(p, Pose::LOGO), pos2(0.0, 0.0), 100.0, 1.0));
                facing(&q) > 0.0
            })
            .collect();
        assert_eq!(shown, vec![true, true, true, false, false, false]);
    }

    #[test]
    fn in_the_logo_pose_film_is_left_timeline_right_play_on_top() {
        let centre_of = |i: usize| {
            let c = FACES[i].0;
            project(rotate(on_face(&c, 0.5, 0.5), Pose::LOGO), pos2(0.0, 0.0), 100.0, 1.0)
        };
        let (film, timeline, play) = (centre_of(0), centre_of(1), centre_of(2));
        assert!(film.x < 0.0 && timeline.x > 0.0, "{film:?} {timeline:?}");
        assert!(play.y < film.y && play.y < timeline.y, "{play:?}");
        // the hexagon is symmetric: film and timeline mirror each other
        assert!((film.x + timeline.x).abs() < 0.5 && (film.y - timeline.y).abs() < 0.5);
    }
}
