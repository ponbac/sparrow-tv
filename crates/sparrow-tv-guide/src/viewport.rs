use std::ops::Range;

/// Vertical scroll state shared by Guide boards with different row geometry.
#[derive(Default)]
pub(crate) struct VerticalViewport {
    pub(crate) height: f32,
    pub(crate) offset: f32,
}

impl VerticalViewport {
    /// Adjusts the offset only as far as needed to reveal the target and margins.
    pub(crate) fn reveal(
        &mut self,
        content_height: f32,
        target: Option<Range<f32>>,
        top_margin: f32,
        bottom_margin: f32,
    ) {
        let limit = (content_height - self.height).max(0.);
        self.offset = self.offset.min(limit);
        if let Some(target) = target.filter(|_| self.height > 0.) {
            let preferred_height = target.end - target.start + top_margin + bottom_margin;
            let view_bottom = self.offset + self.height;
            if target.start < self.offset + top_margin || preferred_height > self.height {
                self.offset = target.start - top_margin;
            } else if target.end > view_bottom - bottom_margin {
                self.offset = target.end - self.height + bottom_margin;
            }
            self.offset = self.offset.clamp(0., limit);
        }
    }
}
