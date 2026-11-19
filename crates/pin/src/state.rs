//! 与 Win32 无关的贴图窗口状态：缩放、不透明度、旋转、穿透、拖动与导出。

use lenswin_core::error::FrameError;
use lenswin_core::frame::CroppedImage;
use lenswin_core::geometry::{Point, Rect};
use lenswin_core::pin::{
    OPACITY_DEFAULT, Rotation, ZOOM_DEFAULT, ZOOM_MAX, opacity_to_alpha, rotate_around_center,
    rotate_image, scaled_size, step_opacity, step_zoom, zoom_around_cursor,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DragState {
    start_cursor: Point,
    start_origin: Point,
}

/// 单张贴图的纯状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PinState {
    image: CroppedImage,
    rect: Rect,
    zoom_permille: u32,
    opacity_pct: u32,
    rotation: Rotation,
    passthrough: bool,
    focused: bool,
    drag: Option<DragState>,
}

impl PinState {
    pub(crate) fn new(image: CroppedImage, at: Rect) -> Result<Self, String> {
        if at.is_empty() {
            return Err("贴图矩形宽或高为 0".to_string());
        }
        let zoom_permille = initial_zoom(image.width(), image.height(), at);
        Ok(Self {
            image,
            rect: at,
            zoom_permille,
            opacity_pct: OPACITY_DEFAULT,
            rotation: Rotation::Deg0,
            passthrough: false,
            focused: false,
            drag: None,
        })
    }

    pub(crate) fn image(&self) -> &CroppedImage {
        &self.image
    }

    pub(crate) fn rect(&self) -> Rect {
        self.rect
    }

    pub(crate) fn zoom_permille(&self) -> u32 {
        self.zoom_permille
    }

    pub(crate) fn rotation(&self) -> Rotation {
        self.rotation
    }

    pub(crate) fn is_passthrough(&self) -> bool {
        self.passthrough
    }

    pub(crate) fn is_focused(&self) -> bool {
        self.focused
    }

    pub(crate) fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub(crate) fn alpha(&self) -> u8 {
        opacity_to_alpha(self.opacity_pct)
    }

    pub(crate) fn set_focused(&mut self, focused: bool) -> bool {
        if self.focused == focused {
            return false;
        }
        self.focused = focused;
        true
    }

    pub(crate) fn begin_drag(&mut self, cursor: Point) {
        if self.passthrough {
            return;
        }
        self.drag = Some(DragState {
            start_cursor: cursor,
            start_origin: Point::new(self.rect.x, self.rect.y),
        });
    }

    pub(crate) fn update_drag(&mut self, cursor: Point) -> Option<Point> {
        let drag = self.drag?;
        let dx = i64::from(cursor.x) - i64::from(drag.start_cursor.x);
        let dy = i64::from(cursor.y) - i64::from(drag.start_cursor.y);
        let x = i64::from(drag.start_origin.x)
            .saturating_add(dx)
            .clamp(i64::from(i32::MIN), i64::from(i32::MAX));
        let y = i64::from(drag.start_origin.y)
            .saturating_add(dy)
            .clamp(i64::from(i32::MIN), i64::from(i32::MAX));
        let nx = i32::try_from(x).unwrap_or(self.rect.x);
        let ny = i32::try_from(y).unwrap_or(self.rect.y);
        if nx == self.rect.x && ny == self.rect.y {
            return None;
        }
        self.rect.x = nx;
        self.rect.y = ny;
        Some(Point::new(nx, ny))
    }

    pub(crate) fn end_drag(&mut self) -> bool {
        self.drag.take().is_some()
    }

    pub(crate) fn zoom_at(&mut self, cursor: Point, delta_steps: i32) -> bool {
        if delta_steps == 0 {
            return false;
        }
        let next_zoom = step_zoom(self.zoom_permille, delta_steps);
        let (rot_w, rot_h) = self
            .rotation
            .dimensions(self.image.width(), self.image.height());
        let (new_w, new_h) = scaled_size(rot_w, rot_h, next_zoom);
        let next_rect = zoom_around_cursor(self.rect, cursor, new_w, new_h);
        if next_zoom == self.zoom_permille && next_rect == self.rect {
            return false;
        }
        self.zoom_permille = next_zoom;
        self.rect = next_rect;
        true
    }

    pub(crate) fn adjust_opacity(&mut self, delta_steps: i32) -> bool {
        if delta_steps == 0 {
            return false;
        }
        let next = step_opacity(self.opacity_pct, delta_steps);
        if next == self.opacity_pct {
            return false;
        }
        self.opacity_pct = next;
        true
    }

    pub(crate) fn rotate_cw(&mut self) -> Rect {
        self.apply_rotation(self.rotation.clockwise())
    }

    pub(crate) fn rotate_ccw(&mut self) -> Rect {
        self.apply_rotation(self.rotation.counter_clockwise())
    }

    fn apply_rotation(&mut self, next: Rotation) -> Rect {
        self.rotation = next;
        let (rot_w, rot_h) = next.dimensions(self.image.width(), self.image.height());
        let (new_w, new_h) = scaled_size(rot_w, rot_h, self.zoom_permille);
        self.rect = rotate_around_center(self.rect, new_w, new_h);
        self.rect
    }

    pub(crate) fn set_passthrough(&mut self, enabled: bool) -> bool {
        if self.passthrough == enabled {
            return false;
        }
        self.passthrough = enabled;
        if enabled {
            self.drag = None;
        }
        true
    }

    pub(crate) fn export_image(&self) -> Result<CroppedImage, FrameError> {
        rotate_image(&self.image, self.rotation)
    }
}

fn initial_zoom(img_w: u32, img_h: u32, at: Rect) -> u32 {
    if at.width == img_w && at.height == img_h {
        return ZOOM_DEFAULT;
    }
    let zw = (u64::from(at.width) * 1000 + u64::from(img_w) / 2) / u64::from(img_w.max(1));
    let zh = (u64::from(at.height) * 1000 + u64::from(img_h) / 2) / u64::from(img_h.max(1));
    u32::try_from(zw.min(zh).clamp(1, u64::from(ZOOM_MAX))).unwrap_or(ZOOM_DEFAULT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lenswin_core::pin::fit_work_area;

    fn sample_image() -> CroppedImage {
        // 3x2 图像：第一行 1,2,3；第二行 4,5,6
        let mut rgba = Vec::new();
        for id in 1u8..=6 {
            rgba.extend_from_slice(&[id, 0, 0, 255]);
        }
        CroppedImage::new(3, 2, rgba).expect("构造样图")
    }

    #[test]
    fn reject_empty_rect() {
        assert!(PinState::new(sample_image(), Rect::new(0, 0, 0, 10)).is_err());
    }

    #[test]
    fn initial_zoom_matches_fit_work_area() {
        let img = CroppedImage::new(2000, 1000, vec![255; 2000 * 1000 * 4]).expect("大图");
        let (rect, zoom) = fit_work_area(img.width(), img.height(), Rect::new(0, 0, 1000, 800));
        let state = PinState::new(img, rect).expect("构造状态");
        assert_eq!(zoom, 500);
        assert_eq!(state.zoom_permille(), 500);
    }

    #[test]
    fn drag_moves_without_jitter() {
        let mut state = PinState::new(sample_image(), Rect::new(100, 200, 3, 2)).expect("构造");
        state.begin_drag(Point::new(101, 201));
        assert!(state.is_dragging());
        assert_eq!(
            state.update_drag(Point::new(111, 191)),
            Some(Point::new(110, 190))
        );
        assert_eq!(state.update_drag(Point::new(111, 191)), None);
        assert!(state.end_drag());
        assert!(!state.is_dragging());
        assert_eq!(state.rect(), Rect::new(110, 190, 3, 2));
    }

    #[test]
    fn zoom_opacity_and_rotation_preserve_original_pixels() {
        let mut state = PinState::new(sample_image(), Rect::new(100, 100, 3, 2)).expect("构造");
        assert!(state.adjust_opacity(-5));
        assert_eq!(state.alpha(), 128);

        let r90 = state.rotate_cw();
        assert_eq!(r90.width, 2);
        assert_eq!(r90.height, 3);

        let exported = state.export_image().expect("导出");
        assert_eq!((exported.width(), exported.height()), (2, 3));
        let reds: Vec<u8> = exported
            .rgba()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|px| *px.first().unwrap_or(&0))
            .collect();
        // 导出仅应用旋转，不应用缩放与透明度
        assert_eq!(reds, vec![4, 1, 5, 2, 6, 3]);
        assert!(
            exported
                .rgba()
                .as_chunks::<4>()
                .0
                .iter()
                .all(|px| px[3] == 255)
        );

        let r0 = state.rotate_ccw();
        assert_eq!(r0.width, 3);
        assert_eq!(r0.height, 2);

        let big = CroppedImage::new(20, 10, vec![255; 20 * 10 * 4]).expect("20x10");
        let mut zoomed = PinState::new(big, Rect::new(100, 100, 20, 10)).expect("构造大图");
        assert!(zoomed.zoom_at(Point::new(110, 105), 1));
        assert_eq!(zoomed.zoom_permille(), 1100);
        assert_eq!((zoomed.rect().width, zoomed.rect().height), (22, 11));
        let rotated = zoomed.rotate_cw();
        assert_eq!((rotated.width, rotated.height), (11, 22));
    }

    #[test]
    fn passthrough_cancels_active_drag() {
        let mut state = PinState::new(sample_image(), Rect::new(0, 0, 3, 2)).expect("构造");
        state.begin_drag(Point::new(1, 1));
        assert!(state.set_passthrough(true));
        assert!(state.is_passthrough());
        assert!(!state.is_dragging());
        assert!(!state.set_passthrough(true));
        assert!(state.set_passthrough(false));
        assert!(!state.is_passthrough());
    }
}
