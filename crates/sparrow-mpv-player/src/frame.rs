use std::time::Duration;

use sparrow_installed_playback::{
    android::AndroidPlaybackViewport,
    tv::{TvPresentationKey, TvRecoveryFrame},
};

/// A GPUI-ready, tightly packed opaque BGRA frame. Keep one reference while
/// painting and let go of superseded frames so the render worker can reuse them.
pub struct VideoFrame {
    pub key: TvPresentationKey,
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    pub serial: u64,
    /// The left, right and bottom Ambient Playback Lighting zones as 0xRRGGBB.
    pub ambient: [u32; 3],
    /// Time in libmpv's software renderer, or OpenGL render and readback.
    pub native_render_time: Duration,
    /// Render and copy time, excluding queueing and GPUI texture upload.
    pub render_time: Duration,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrameSize {
    pub width: u32,
    pub height: u32,
}

impl FrameSize {
    pub(crate) fn fit(width: u32, height: u32, limit_width: u32, limit_height: u32) -> Self {
        let width_fit = width.min(limit_width);
        let height_fit = (height * width_fit / width).max(1);
        if height_fit > limit_height {
            return Self {
                width: (width * limit_height / height).max(1),
                height: limit_height,
            };
        }
        Self {
            width: width_fit,
            height: height_fit,
        }
    }
}

impl From<AndroidPlaybackViewport> for FrameSize {
    fn from(viewport: AndroidPlaybackViewport) -> Self {
        // Bound the offscreen allocation independently of the window geometry.
        Self::fit(viewport.width(), viewport.height(), 4096, 2160)
    }
}

impl VideoFrame {
    pub(crate) fn still(&self) -> TvRecoveryFrame {
        let FrameSize { width, height } = FrameSize::fit(self.width, self.height, 640, 360);
        let mut bgra = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let offset =
                    ((y * self.height / height * self.width + x * self.width / width) * 4) as usize;
                bgra.extend_from_slice(&self.bgra[offset..offset + 4]);
            }
        }
        TvRecoveryFrame {
            width,
            height,
            bgra,
        }
    }
}

pub(crate) fn ambient(bgra: &[u8], width: u32, height: u32) -> [u32; 3] {
    // Same grid centers and zone positions as Shield.
    let colors: [[u32; 3]; 12] = std::array::from_fn(|i| {
        let x = (2 * (i as u32 % 4) + 1) * width / 8;
        let y = (2 * (i as u32 / 4) + 1) * height / 6;
        let offset = ((y * width + x) * 4) as usize;
        [
            bgra[offset + 2].into(),
            bgra[offset + 1].into(),
            bgra[offset].into(),
        ]
    });
    let average = |zone: &[usize]| {
        let channel =
            |channel| zone.iter().map(|&i| colors[i][channel]).sum::<u32>() / zone.len() as u32;
        (channel(0) << 16) | (channel(1) << 8) | channel(2)
    };
    [
        average(&[0, 1, 4]),
        average(&[2, 3, 7]),
        average(&[5, 6, 9, 10]),
    ]
}

#[cfg(test)]
mod tests {
    use super::ambient;

    #[test]
    fn ambient_uses_shields_zone_centres_and_packs_rgb() {
        let mut bgra = vec![255; 8 * 6 * 4];
        for y in 0..3 {
            for x in 0..4 {
                let index = (2 * y + 1) * 8 + 2 * x + 1;
                let value = (4 * y + x) as u8;
                bgra[index * 4..index * 4 + 4].copy_from_slice(&[
                    value * 12,
                    255 - value * 3,
                    value * 6,
                    255,
                ]);
            }
        }
        assert_eq!(ambient(&bgra, 8, 6), [0x0afa14, 0x18f330, 0x2de85a]);
    }
}
