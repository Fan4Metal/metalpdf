//! The painted icons of the apps of this family, in one place so that they
//! look alike: the default fonts lack most symbols (▲, ☀, ⏏), and an icon
//! font would be another dependency. Every icon is drawn on a 14-point
//! grid around its centre (−7…7), in lines 1.3 points wide, and scales
//! with `size` (lines included). Collected from qview (toolbar), MetalMD
//! (toolbar, Copy), Metal Player (`ui/icons.rs`), disk_flashlight (search
//! field, chart) and qrec (title row, trimming).
//!
//! Corners of a line icon are rounded (`rounded`) and its open ends are
//! round (`round_line`) where sharp ones look rough at 14 points: egui joins
//! lines with mitres and ends them flat. Open, Text and Download are drawn
//! so (MetalMD).
//!
//! A new icon is added here, on the same grid and with the same line,
//! and looked at with `cargo run --example icons` next to the others in
//! both themes. An app uses a few of them; the rest stay for the next one.

#![allow(dead_code)]

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, vec2};
use std::f32::consts::{FRAC_PI_2, PI, TAU};

/// The grid an icon is drawn on, in points: the size of a toolbar icon in
/// an `egui::Button` (`ui::icon_button`). qview's large toolbar buttons
/// (32 × 28) take 17.
pub const SIZE: f32 = 14.0;
/// The width of the lines at `SIZE`.
pub const LINE: f32 = 1.3;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Icon {
    // Common to every app: the toolbar's right end, dialogs, the title row.
    /// About: an "i" in a circle.
    Info,
    /// The theme switch while following Windows: a half-filled circle.
    ThemeSystem,
    /// The light theme: a sun.
    ThemeLight,
    /// The dark theme: a crescent (cut out with `fill`).
    ThemeDark,
    /// A newer version: an arrow down onto a tray, in the link colour.
    Download,
    Close,
    Minimize,
    Maximize,
    /// Maximized: two overlapping squares.
    Restore,
    // Files.
    /// Open a file: an open folder.
    Open,
    Folder,
    File,
    /// Two sheets, the front one hiding the back one (cut out with `fill`).
    Copy,
    /// Done (after a copy): a tick.
    Check,
    Delete,
    /// Search: a lens.
    Search,
    /// Clear a field: a disc with a cross cut out of it (with `fill`).
    Clear,
    // Moving about.
    Prev,
    Next,
    /// The folder above.
    Up,
    /// Thumbnails: four squares.
    Gallery,
    /// A list of headings, two of them indented.
    Contents,
    /// A plain list: a playlist.
    List,
    Sort,
    // Viewing and editing.
    ZoomIn,
    ZoomOut,
    /// Four corners pointing out: fit, fullscreen.
    Fullscreen,
    /// Four corners pointing in.
    ExitFullscreen,
    RotateLeft,
    RotateRight,
    Crop,
    /// Open in the editor: a pencil.
    Edit,
    /// Fonts and text size: a large and a small "A".
    Text,
    /// Trim: scissors.
    Cut,
    Star,
    /// A favourite: the star filled (`color` is the star's colour).
    StarFilled,
    /// Settings: a gear (its hole cut out with `fill`).
    Settings,
    Plus,
    Minus,
    // Media.
    Play,
    Pause,
    Stop,
    /// A filled circle, red at the caller's choice.
    Record,
    SkipBack,
    SkipForward,
    /// Open a file in a player.
    Eject,
    /// A loudspeaker with 0, 1 or 2 waves.
    Volume(u8),
    Mute,
    /// Two crossing arrows (metalmpv).
    Shuffle,
    /// Two arrows round a loop (metalmpv); `RepeatOne` with a 1 inside.
    Repeat,
    RepeatOne,
    /// Two arcs round a centre, each ending in a solid triangle at three
    /// and nine o'clock (the common sync sign): scan or load again (metalamp).
    Refresh,
    /// A picture: a frame with a sun and hills (metalamp: the cover's
    /// toggle).
    Image,
    /// Books on a shelf, filled, the last one leaning on its neighbour:
    /// the media library (metalamp: its folders).
    Library,
    /// Pages one under another: the continuous view (MetalPDF).
    Continuous,
    /// One page with a folded corner, filled lines: one page at a time
    /// (MetalPDF).
    SinglePage,
    /// A panel at the left of a window: the sidebar toggle (MetalPDF).
    Sidebar,
    /// A window with a page filling its width, cut top and bottom: zoom
    /// to fit the width (MetalPDF).
    FitWidth,
    /// A window with a whole page inside it: zoom to fit the page
    /// (MetalPDF).
    FitPage,
    /// "1:1" drawn in strokes: the actual size, 100 % (MetalPDF).
    ActualSize,
    /// Four arrows from the centre: the hand tool, dragging moves the
    /// pages (MetalPDF).
    Move,
    /// An I-beam, the text cursor: the text selection tool (MetalPDF).
    SelectText,
}

impl Icon {
    /// Every icon, for the sheet in `examples/icons.rs`.
    pub const ALL: &[Icon] = &[
        Icon::Info,
        Icon::ThemeSystem,
        Icon::ThemeLight,
        Icon::ThemeDark,
        Icon::Download,
        Icon::Close,
        Icon::Minimize,
        Icon::Maximize,
        Icon::Restore,
        Icon::Open,
        Icon::Folder,
        Icon::File,
        Icon::Copy,
        Icon::Check,
        Icon::Delete,
        Icon::Search,
        Icon::Clear,
        Icon::Prev,
        Icon::Next,
        Icon::Up,
        Icon::Gallery,
        Icon::Contents,
        Icon::List,
        Icon::Sort,
        Icon::ZoomIn,
        Icon::ZoomOut,
        Icon::Fullscreen,
        Icon::ExitFullscreen,
        Icon::RotateLeft,
        Icon::RotateRight,
        Icon::Crop,
        Icon::Edit,
        Icon::Text,
        Icon::Cut,
        Icon::Star,
        Icon::StarFilled,
        Icon::Settings,
        Icon::Plus,
        Icon::Minus,
        Icon::Play,
        Icon::Pause,
        Icon::Stop,
        Icon::Record,
        Icon::SkipBack,
        Icon::SkipForward,
        Icon::Eject,
        Icon::Volume(0),
        Icon::Volume(1),
        Icon::Volume(2),
        Icon::Mute,
        Icon::Shuffle,
        Icon::Repeat,
        Icon::RepeatOne,
        Icon::Refresh,
        Icon::Image,
        Icon::Library,
        Icon::Continuous,
        Icon::SinglePage,
        Icon::Sidebar,
        Icon::FitWidth,
        Icon::FitPage,
        Icon::ActualSize,
        Icon::Move,
        Icon::SelectText,
    ];
}

/// Paint `icon` `size` points wide around `c` in `color`; `fill` is the
/// colour under it (the button's fill), for the parts cut out of a shape.
pub fn paint(painter: &Painter, icon: Icon, c: Pos2, size: f32, color: Color32, fill: Color32) {
    let k = size / SIZE;
    let p = |x: f32, y: f32| c + vec2(x, y) * k;
    let pen = |width: f32| Stroke::new(width * k, color);
    let line = pen(LINE);
    let polyline = |points: &[(f32, f32)]| Shape::line(points.iter().map(|&(x, y)| p(x, y)).collect(), line);
    let closed = |points: &[(f32, f32)]| Shape::closed_line(points.iter().map(|&(x, y)| p(x, y)).collect(), line);
    let solid = |points: &[(f32, f32)], color: Color32| {
        Shape::convex_polygon(points.iter().map(|&(x, y)| p(x, y)).collect(), color, Stroke::NONE)
    };
    let rect = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(p(x, y), vec2(w, h) * k);
    match icon {
        Icon::Shuffle => {
            for y in [-3.6, 3.6] {
                round_line(
                    painter,
                    rounded(
                        &[p(-6.5, -y), p(-2.6, -y), p(2.4, y), p(5.8, y)],
                        &[0.0, 1.6 * k, 1.6 * k, 0.0],
                    ),
                    line,
                );
                round_line(
                    painter,
                    vec![p(3.8, y - 2.0), p(5.9, y), p(3.8, y + 2.0)],
                    line,
                );
            }
        }
        Icon::Repeat | Icon::RepeatOne => {
            // The top arrow up the left side and to the right, the bottom
            // one the same turned half round.
            for t in [1.0, -1.0] {
                let q = |x: f32, y: f32| p(x * t, y * t);
                round_line(
                    painter,
                    rounded(
                        &[q(-6.2, 1.2), q(-6.2, -3.8), q(5.6, -3.8)],
                        &[0.0, 2.2 * k, 0.0],
                    ),
                    line,
                );
                round_line(
                    painter,
                    vec![q(3.7, -5.7), q(5.7, -3.8), q(3.7, -1.9)],
                    line,
                );
            }
            if icon == Icon::RepeatOne {
                round_line(
                    painter,
                    vec![p(-1.3, -0.9), p(0.3, -2.0), p(0.3, 2.2)],
                    pen(1.2),
                );
            }
        }
        Icon::Refresh => {
            // As the common sync sign: two arcs of 122° round the centre,
            // clockwise, each ending in a solid triangle whose flat base
            // straddles the ring, at three and nine o'clock, pointing down
            // on the right and up on the left; the arcs start well clear of
            // the other head.
            let r = 4.3 * k;
            for t in [1.0f32, -1.0] {
                let half = if t > 0.0 { 0.0 } else { PI };
                let (from, to) = (222f32.to_radians() + half, 344f32.to_radians() + half);
                let arc: Vec<Pos2> = (0..=16)
                    .map(|i| {
                        let a = from + (to - from) * i as f32 / 16.0;
                        c + r * vec2(a.cos(), a.sin())
                    })
                    .collect();
                painter.circle_filled(arc[0], 0.725 * k, color);
                painter.add(Shape::line(arc, pen(1.45)));
                let q = |x: f32, y: f32| p(x * t, y * t);
                painter.add(Shape::convex_polygon(
                    vec![q(2.0, -1.3), q(6.8, -1.3), q(4.4, 2.2)],
                    color,
                    Stroke::NONE,
                ));
            }
        }
        Icon::Image => {
            painter.rect_stroke(rect(-6.0, -5.0, 12.0, 10.0), 1.5 * k, line, StrokeKind::Middle);
            painter.circle_filled(p(-2.6, -2.0), 1.25 * k, color);
            painter.add(polyline(&[
                (-5.3, 4.3),
                (-1.8, 0.6),
                (0.4, 2.9),
                (2.3, 0.9),
                (5.3, 4.3),
            ]));
        }
        Icon::Library => {
            // Three filled spines: two upright of two heights and a third
            // leaning left on the second, standing on its bottom left
            // corner. Filled, as Gallery: outlined spines kept no room
            // inside at 14 points, and bands cut across them read as the
            // dots of an "i".
            painter.rect_filled(rect(-6.6, -6.0, 2.8, 12.0), 0.7 * k, color);
            painter.rect_filled(rect(-2.9, -4.0, 2.8, 10.0), 0.7 * k, color);
            // Tilted 20°: across (cos, -sin), up (-sin, -cos).
            let (sn, co) = (20f32.to_radians().sin(), 20f32.to_radians().cos());
            let at = |x: f32, y: f32| (4.01 + x * co - y * sn, 6.0 - x * sn - y * co);
            painter.add(solid(
                &[at(0.0, 0.0), at(2.8, 0.0), at(2.8, 9.4), at(0.0, 9.4)],
                color,
            ));
        }
        Icon::Info => {
            painter.circle_stroke(c, 6.0 * k, line);
            painter.circle_filled(p(0.0, -2.8), 0.95 * k, color);
            painter.line_segment([p(0.0, -0.6), p(0.0, 3.4)], pen(1.4));
        }
        Icon::ThemeSystem => {
            let r = 5.5 * k;
            painter.circle_stroke(c, r, line);
            // The left half filled: a convex half disc.
            let half = (0..=16)
                .map(|i| {
                    let a = PI * (0.5 + i as f32 / 16.0);
                    c + vec2(r * a.cos(), -r * a.sin())
                })
                .collect();
            painter.add(Shape::convex_polygon(half, color, Stroke::NONE));
        }
        Icon::ThemeLight => {
            painter.circle_filled(c, 3.2 * k, color);
            for i in 0..8 {
                let a = TAU * i as f32 / 8.0;
                let d = vec2(a.cos(), a.sin());
                painter.line_segment([c + d * 5.0 * k, c + d * 6.8 * k], line);
            }
        }
        Icon::ThemeDark => {
            // A disc with another in the colour under it over its upper right.
            painter.circle_filled(c, 6.0 * k, color);
            painter.circle_filled(p(3.0, -2.2), 5.0 * k, fill);
        }
        Icon::Download => {
            let stroke = pen(1.5);
            round_line(painter, vec![p(0.0, -5.5), p(0.0, 2.5)], stroke);
            let head = [p(-4.0, -1.5), p(0.0, 2.5), p(4.0, -1.5)];
            round_line(painter, rounded(&head, &[0.0, 1.0 * k, 0.0]), stroke);
            round_line(painter, vec![p(-5.5, 5.5), p(5.5, 5.5)], stroke);
        }
        Icon::Close => {
            painter.line_segment([p(-5.0, -5.0), p(5.0, 5.0)], line);
            painter.line_segment([p(-5.0, 5.0), p(5.0, -5.0)], line);
        }
        Icon::Minimize => {
            painter.line_segment([p(-5.0, 0.0), p(5.0, 0.0)], line);
        }
        Icon::Maximize => {
            painter.rect_stroke(rect(-5.0, -5.0, 10.0, 10.0), 0.0, line, StrokeKind::Middle);
        }
        Icon::Restore => {
            painter.add(polyline(&[(-3.0, -5.0), (5.0, -5.0), (5.0, 3.0)]));
            painter.rect_stroke(rect(-5.0, -3.0, 8.0, 8.0), 0.0, line, StrokeKind::Middle);
        }
        Icon::Open => {
            // The back with its tab, from where its rounded bottom left
            // corner meets the front flap to where the flap's top is, and
            // the flap opened forward: a parallelogram leaning right, sharp
            // only where it leaves the back's corner.
            let back = [(-4.5, 5.0), (-6.5, 5.0), (-6.5, -5.0), (-2.0, -5.0), (-0.5, -3.0), (5.0, -3.0), (5.0, -0.5)];
            let back: Vec<Pos2> = back.iter().map(|&(x, y)| p(x, y)).collect();
            round_line(painter, rounded(&back, &[0.0, 2.0, 2.0, 1.2, 1.2, 2.0, 0.0].map(|r| r * k)), line);
            let flap = [(-4.5, 5.0), (-2.0, -0.5), (7.0, -0.5), (4.5, 5.0), (-4.5, 5.0)];
            let flap: Vec<Pos2> = flap.iter().map(|&(x, y)| p(x, y)).collect();
            round_line(painter, rounded(&flap, &[0.0, 1.5, 1.5, 1.5, 0.0].map(|r| r * k)), line);
        }
        Icon::Folder => {
            painter.add(closed(&[(-6.5, -5.0), (-2.0, -5.0), (-0.5, -3.0), (6.5, -3.0), (6.5, 5.0), (-6.5, 5.0)]));
        }
        Icon::Continuous => {
            // Two pages one under another, the lower one cut by the edge.
            painter.rect_stroke(rect(-4.5, -7.0, 9.0, 7.5), 1.0 * k, line, StrokeKind::Middle);
            painter.add(polyline(&[(-4.5, 7.0), (-4.5, 2.5), (4.5, 2.5), (4.5, 7.0)]));
            painter.line_segment([p(-2.5, -4.5), p(2.5, -4.5)], pen(1.0));
            painter.line_segment([p(-2.5, -2.0), p(2.5, -2.0)], pen(1.0));
        }
        Icon::SinglePage => {
            // A page with a folded corner and two lines of text.
            painter.add(closed(&[(-4.5, -6.0), (1.0, -6.0), (4.5, -2.5), (4.5, 6.0), (-4.5, 6.0)]));
            painter.add(polyline(&[(1.0, -6.0), (1.0, -2.5), (4.5, -2.5)]));
            painter.line_segment([p(-2.5, 0.5), p(2.5, 0.5)], pen(1.0));
            painter.line_segment([p(-2.5, 3.0), p(2.5, 3.0)], pen(1.0));
        }
        Icon::FitWidth => {
            // A window, and a page as wide as its inside, running off the
            // top and the bottom.
            painter.rect_stroke(rect(-6.5, -5.5, 13.0, 11.0), 1.5 * k, line, StrokeKind::Middle);
            painter.rect_filled(rect(-3.75, -4.85, 7.5, 9.7), 0.0, color);
        }
        Icon::FitPage => {
            // A window with a whole page inside it, room all round.
            painter.rect_stroke(rect(-6.5, -5.5, 13.0, 11.0), 1.5 * k, line, StrokeKind::Middle);
            painter.rect_filled(rect(-2.5, -3.25, 5.0, 6.5), 0.0, color);
        }
        Icon::ActualSize => {
            // 1:1 — two ones with their flags, two dots between.
            for x in [-3.75, 3.75] {
                painter.line_segment([p(x, -4.5), p(x, 4.5)], line);
                painter.line_segment([p(x - 1.8, -2.7), p(x, -4.5)], line);
            }
            painter.circle_filled(p(0.0, -1.6), 0.95 * k, color);
            painter.circle_filled(p(0.0, 1.6), 0.95 * k, color);
        }
        Icon::Move => {
            // Four arrows from the centre.
            painter.line_segment([p(-6.0, 0.0), p(6.0, 0.0)], line);
            painter.line_segment([p(0.0, -6.0), p(0.0, 6.0)], line);
            for (dx, dy) in [(1.0f32, 0.0f32), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                let (tx, ty) = (6.0 * dx, 6.0 * dy);
                // The head's base on the shaft, its sides across it.
                let (bx, by) = (tx - 2.2 * dx, ty - 2.2 * dy);
                painter.add(polyline(&[(bx - 2.2 * dy, by - 2.2 * dx), (tx, ty), (bx + 2.2 * dy, by + 2.2 * dx)]));
            }
        }
        Icon::SelectText => {
            // An I-beam, the text cursor.
            painter.line_segment([p(0.0, -5.5), p(0.0, 5.5)], line);
            for y in [-6.0, 6.0] {
                painter.line_segment([p(-2.6, y), p(-0.8, y)], line);
                painter.line_segment([p(0.8, y), p(2.6, y)], line);
            }
        }
        Icon::Sidebar => {
            // A window with a panel on its left.
            painter.rect_stroke(rect(-6.5, -5.0, 13.0, 10.0), 1.5 * k, line, StrokeKind::Middle);
            painter.line_segment([p(-2.0, -5.0), p(-2.0, 5.0)], line);
            painter.line_segment([p(-5.0, -2.0), p(-3.5, -2.0)], pen(1.0));
            painter.line_segment([p(-5.0, 0.0), p(-3.5, 0.0)], pen(1.0));
        }
        Icon::File => {
            // A page with its top right corner folded.
            painter.add(closed(&[(-4.5, -6.0), (1.0, -6.0), (4.5, -2.5), (4.5, 6.0), (-4.5, 6.0)]));
            painter.add(polyline(&[(1.0, -6.0), (1.0, -2.5), (4.5, -2.5)]));
        }
        Icon::Copy => {
            painter.rect_stroke(rect(-6.0, -7.0, 8.0, 10.0), 1.5 * k, line, StrokeKind::Middle);
            painter.rect_filled(rect(-2.0, -3.0, 8.0, 10.0), 1.5 * k, fill);
            painter.rect_stroke(rect(-2.0, -3.0, 8.0, 10.0), 1.5 * k, line, StrokeKind::Middle);
        }
        Icon::Check => {
            painter.add(polyline(&[(-5.0, 0.5), (-1.5, 4.0), (5.0, -3.5)]));
        }
        Icon::Delete => {
            // A bin: the handle, the lid, the body narrowing downwards and
            // two ribs.
            painter.add(polyline(&[(-2.2, -4.8), (-2.2, -6.6), (2.2, -6.6), (2.2, -4.8)]));
            painter.line_segment([p(-6.2, -4.8), p(6.2, -4.8)], line);
            painter.add(polyline(&[(-4.4, -2.6), (-3.5, 6.2), (3.5, 6.2), (4.4, -2.6)]));
            painter.line_segment([p(-1.3, -0.4), p(-1.1, 4.0)], pen(1.0));
            painter.line_segment([p(1.3, -0.4), p(1.1, 4.0)], pen(1.0));
        }
        Icon::Search | Icon::ZoomIn | Icon::ZoomOut => {
            // A lens with a heavier handle to the bottom right.
            let (lens, r) = (p(-1.2, -1.2), 4.6 * k);
            painter.circle_stroke(lens, r, line);
            let d = std::f32::consts::FRAC_1_SQRT_2;
            painter.line_segment([lens + vec2(d, d) * r, p(5.8, 5.8)], pen(2.1));
            if icon != Icon::Search {
                painter.line_segment([lens - vec2(2.4, 0.0) * k, lens + vec2(2.4, 0.0) * k], line);
            }
            if icon == Icon::ZoomIn {
                painter.line_segment([lens - vec2(0.0, 2.4) * k, lens + vec2(0.0, 2.4) * k], line);
            }
        }
        Icon::Clear => {
            painter.circle_filled(c, 6.0 * k, color);
            let stroke = Stroke::new(1.4 * k, fill);
            painter.line_segment([p(-2.2, -2.2), p(2.2, 2.2)], stroke);
            painter.line_segment([p(-2.2, 2.2), p(2.2, -2.2)], stroke);
        }
        Icon::Prev | Icon::Next => {
            let s = if icon == Icon::Prev { -1.0 } else { 1.0 };
            painter.line_segment([p(-6.0 * s, 0.0), p(5.7 * s, 0.0)], line);
            painter.add(polyline(&[(1.2 * s, -4.5), (6.0 * s, 0.0), (1.2 * s, 4.5)]));
        }
        Icon::Up => {
            painter.line_segment([p(0.0, 5.7), p(0.0, -6.0)], line);
            painter.add(polyline(&[(-4.5, -1.2), (0.0, -6.0), (4.5, -1.2)]));
        }
        Icon::Gallery => {
            for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                painter.rect_filled(Rect::from_center_size(p(3.2 * x, 3.2 * y), vec2(5.0, 5.0) * k), 1.0 * k, color);
            }
        }
        Icon::Contents | Icon::List => {
            let indent = if icon == Icon::Contents { 3.0 } else { 0.0 };
            for (y, indent) in [(-4.0, 0.0), (0.0, indent), (4.0, indent)] {
                painter.circle_filled(p(-5.0 + indent, y), 1.1 * k, color);
                painter.line_segment([p(-2.5 + indent, y), p(6.0, y)], line);
            }
        }
        Icon::Sort => {
            // Bars getting shorter, an arrow down beside them.
            for (i, w) in [7.0, 5.0, 3.0].into_iter().enumerate() {
                let y = -5.0 + 5.0 * i as f32;
                painter.line_segment([p(-6.5, y), p(-6.5 + w, y)], line);
            }
            painter.line_segment([p(4.0, -6.0), p(4.0, 6.0)], line);
            painter.add(polyline(&[(1.5, 3.5), (4.0, 6.0), (6.5, 3.5)]));
        }
        Icon::Fullscreen | Icon::ExitFullscreen => {
            // Corners at the edges with their arms along them, or set in
            // with their arms pointing out.
            let (corner, arm) = if icon == Icon::Fullscreen { (5.5, -3.5) } else { (2.0, 3.5) };
            for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let (cx, cy) = (x * corner, y * corner);
                painter.add(polyline(&[(cx + x * arm, cy), (cx, cy), (cx, cy + y * arm)]));
            }
        }
        Icon::RotateLeft | Icon::RotateRight => {
            // A clockwise arc ending at the top, its arrow pointing right;
            // mirrored for the left turn. Set a little low, as the arrow
            // stands above the arc.
            let (o, r) = (p(0.0, 1.0), 5.0 * k);
            let end = 1.5 * PI;
            let mut arc: Vec<Pos2> = (0..=24)
                .map(|i| {
                    let a = end - 1.62 * PI * (1.0 - i as f32 / 24.0);
                    o + r * vec2(a.cos(), a.sin())
                })
                .collect();
            let tip = o + vec2(0.0, -r);
            let mut head = vec![tip + vec2(-2.0, -3.0) * k, tip + vec2(2.6, 0.0) * k, tip + vec2(-2.0, 3.0) * k];
            if icon == Icon::RotateLeft {
                for q in arc.iter_mut().chain(head.iter_mut()) {
                    q.x = 2.0 * c.x - q.x;
                }
            }
            painter.add(Shape::line(arc, line));
            painter.add(Shape::convex_polygon(head, color, Stroke::NONE));
        }
        Icon::Crop => {
            // Two right angles crossing, as on a cropping tool.
            painter.add(polyline(&[(-3.7, -6.5), (-3.7, 3.7), (6.5, 3.7)]));
            painter.add(polyline(&[(-6.5, -3.7), (3.7, -3.7), (3.7, 6.5)]));
        }
        Icon::Edit => {
            // A pencil from the bottom left, its point down: along `d`,
            // `n` across; where the point is sharpened, and the band by the
            // end.
            let (d, n) = (vec2(1.0, -1.0).normalized() * k, vec2(1.0, 1.0).normalized() * k);
            let tip = p(-5.7, 5.7);
            let at = |along: f32, across: f32| tip + d * along + n * across;
            painter.add(Shape::closed_line(
                vec![tip, at(4.1, 2.3), at(14.3, 2.3), at(14.3, -2.3), at(4.1, -2.3)],
                line,
            ));
            painter.line_segment([at(4.1, 2.3), at(4.1, -2.3)], pen(1.0));
            painter.line_segment([at(11.9, 2.3), at(11.9, -2.3)], pen(1.0));
        }
        Icon::Text => {
            // A large and a small "A" with rounded apexes and round ends:
            // (apex, left foot, right foot, crossbar height).
            for ((ax, ay), left, right, bar) in [((-3.0, -6.0), -7.0, 1.0, 2.0), ((4.5, -0.5), 2.0, 7.0, 3.8)] {
                let foot = 6.0;
                let legs = [p(left, foot), p(ax, ay), p(right, foot)];
                round_line(painter, rounded(&legs, &[0.0, 1.5 * k, 0.0]), line);
                let t = (foot - bar) / (foot - ay);
                round_line(painter, vec![p(left + (ax - left) * t, bar), p(right + (ax - right) * t, bar)], line);
            }
        }
        Icon::Cut => {
            // Two rings on the left, a handle from each crossing the other
            // in the middle into a solid blade pointing right, rounded on
            // its outer edge at the tip (qrec).
            let blade = [(-1.1, -0.9), (5.1, -5.15), (5.9, -5.2), (6.5, -4.8), (6.8, -4.2), (6.8, -3.8), (1.2, 0.1)];
            for side in [-1.0f32, 1.0] {
                let centre = vec2(-4.6, 3.6 * side);
                painter.circle_stroke(p(centre.x, centre.y), 1.6 * k, pen(1.4));
                let end = vec2(0.6, -0.5 * side);
                let start = centre + (end - centre).normalized() * 1.6;
                painter.line_segment([p(start.x, start.y), p(end.x, end.y)], pen(1.3));
                // Clockwise on the screen: mirrored, the points go the other way.
                let mut points: Vec<Pos2> = blade.iter().map(|&(x, y)| p(x, y * side)).collect();
                if side < 0.0 {
                    points.reverse();
                }
                painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
            }
        }
        Icon::Star | Icon::StarFilled => {
            let (r, inner) = (6.8 * k, 6.8 * k * 0.382);
            let points: Vec<Pos2> = (0..10)
                .map(|i| {
                    let a = -FRAC_PI_2 + i as f32 * PI / 5.0;
                    let radius = if i % 2 == 0 { r } else { inner };
                    c + radius * vec2(a.cos(), a.sin())
                })
                .collect();
            if icon == Icon::StarFilled {
                // Not convex: the pentagon inside and the five tips.
                let pentagon = (0..5).map(|i| points[2 * i + 1]).collect();
                painter.add(Shape::convex_polygon(pentagon, color, Stroke::NONE));
                for i in 0..5 {
                    let tip = vec![points[(2 * i + 9) % 10], points[2 * i], points[2 * i + 1]];
                    painter.add(Shape::convex_polygon(tip, color, Stroke::NONE));
                }
            }
            painter.add(Shape::closed_line(points, line));
        }
        Icon::Settings => {
            for i in 0..8 {
                let a = TAU * i as f32 / 8.0;
                let d = vec2(a.cos(), a.sin()) * k;
                painter.line_segment([c + d * 3.2, c + d * 6.4], pen(2.1));
            }
            painter.circle_filled(c, 4.4 * k, color);
            painter.circle_filled(c, 2.0 * k, fill);
        }
        Icon::Plus => {
            painter.line_segment([p(-5.0, 0.0), p(5.0, 0.0)], pen(1.5));
            painter.line_segment([p(0.0, -5.0), p(0.0, 5.0)], pen(1.5));
        }
        Icon::Minus => {
            painter.line_segment([p(-5.0, 0.0), p(5.0, 0.0)], pen(1.5));
        }
        Icon::Play => {
            // Shifted right: a triangle's centre of mass is left of its box's.
            painter.add(solid(&[(-3.3, -6.0), (6.3, 0.0), (-3.3, 6.0)], color));
        }
        Icon::Pause => {
            painter.rect_filled(rect(-3.8, -5.5, 2.6, 11.0), 1.0 * k, color);
            painter.rect_filled(rect(1.2, -5.5, 2.6, 11.0), 1.0 * k, color);
        }
        Icon::Stop => {
            painter.rect_filled(rect(-4.5, -4.5, 9.0, 9.0), 1.5 * k, color);
        }
        Icon::Record => {
            painter.circle_filled(c, 5.5 * k, color);
        }
        Icon::SkipBack | Icon::SkipForward => {
            // Two triangles and a bar: |◀◀ or ▶▶|.
            let s = if icon == Icon::SkipBack { -1.0 } else { 1.0 };
            for x in [-5.4, -0.3] {
                painter.add(solid(&[(x * s, -4.5), ((x + 5.1) * s, 0.0), (x * s, 4.5)], color));
            }
            painter.line_segment([p(5.7 * s, -4.5), p(5.7 * s, 4.5)], pen(1.3));
        }
        Icon::Eject => {
            painter.add(solid(&[(-5.9, 1.0), (0.0, -5.5), (5.9, 1.0)], color));
            painter.rect_filled(rect(-5.9, 2.9, 11.8, 2.6), 1.0 * k, color);
        }
        Icon::Volume(_) | Icon::Mute => {
            // A loudspeaker, its waves or a cross to the right.
            let o = p(-2.3, 0.0);
            let at = |x: f32, y: f32| o + vec2(x, y) * k;
            painter.add(Shape::convex_polygon(
                vec![at(-4.2, -2.0), at(-1.6, -2.0), at(1.6, -4.9), at(1.6, 4.9), at(-1.6, 2.0), at(-4.2, 2.0)],
                color,
                Stroke::NONE,
            ));
            if icon == Icon::Mute {
                let x = at(6.2, 0.0);
                painter.line_segment([x + vec2(-2.1, -2.1) * k, x + vec2(2.1, 2.1) * k], line);
                painter.line_segment([x + vec2(-2.1, 2.1) * k, x + vec2(2.1, -2.1) * k], line);
            } else if let Icon::Volume(waves) = icon {
                for i in 0..waves.min(2) {
                    let r = (3.6 + 2.6 * i as f32) * k;
                    let wave = (0..=8)
                        .map(|j| {
                            let a = -0.8 + 1.6 * j as f32 / 8.0;
                            at(1.3, 0.0) + r * vec2(a.cos(), a.sin())
                        })
                        .collect();
                    painter.add(Shape::line(wave, line));
                }
            }
        }
    }
}

/// The line through `points` with each corner cut `radius[i]` along both
/// its sides (less on a short side) and bridged by a curve, so that a line
/// icon looks drawn with a round pen; a radius of 0 keeps the point as it
/// is, as for the ends.
fn rounded(points: &[Pos2], radius: &[f32]) -> Vec<Pos2> {
    const STEPS: usize = 6;
    let n = points.len();
    let mut out = Vec::with_capacity(n * (STEPS + 1));
    for (i, (&p, &radius)) in points.iter().zip(radius).enumerate() {
        if radius <= 0.0 || i == 0 || i == n - 1 {
            out.push(p);
            continue;
        }
        let (prev, next) = (points[i - 1], points[i + 1]);
        let r = radius.min((prev - p).length() / 2.0).min((next - p).length() / 2.0);
        let start = p + (prev - p).normalized() * r;
        let end = p + (next - p).normalized() * r;
        // A quadratic Bézier with the corner as its control point.
        for k in 0..=STEPS {
            let t = k as f32 / STEPS as f32;
            let u = 1.0 - t;
            out.push((start.to_vec2() * (u * u) + p.to_vec2() * (2.0 * u * t) + end.to_vec2() * (t * t)).to_pos2());
        }
    }
    out
}

/// An open line with round ends: egui ends a stroke flat.
fn round_line(painter: &Painter, points: Vec<Pos2>, stroke: Stroke) {
    for end in [points.first(), points.last()].into_iter().flatten() {
        painter.circle_filled(*end, stroke.width / 2.0, stroke.color);
    }
    painter.add(Shape::line(points, stroke));
}
