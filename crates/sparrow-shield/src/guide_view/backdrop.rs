use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, BoxShadow, Div, Rgba, Window, div, linear_color_stop, linear_gradient,
    prelude::*, rgb,
};
use sparrow_installed_catalog::GuideBackdrop;

use super::{INK, Layout};

#[derive(Clone, Copy)]
struct Wash {
    strength: f32,
    drift: f32,
    glow: Rgba,
}

impl Wash {
    fn new(choice: GuideBackdrop, tint: Option<u32>, elapsed: Duration) -> Self {
        let drift = if choice == GuideBackdrop::AuroraDrift {
            (1. - (elapsed.as_secs_f32() * std::f32::consts::TAU / 38.).cos()) * 0.5
        } else {
            0.
        };
        let glow = if choice == GuideBackdrop::ChannelTint {
            tint.map_or(rgb(0x2a6f99), rgb)
        } else {
            mix(rgb(0x2a6f99), rgb(0x59408b), drift)
        };
        Self {
            strength: if choice == GuideBackdrop::Flat {
                0.
            } else {
                1.
            },
            drift,
            glow,
        }
    }

    fn blend(self, target: Self, t: f32) -> Self {
        Self {
            strength: self.strength + (target.strength - self.strength) * t,
            drift: self.drift + (target.drift - self.drift) * t,
            glow: mix(self.glow, target.glow, t),
        }
    }

    fn paint(self, layout: Layout, layer: Div) -> Div {
        if self.strength == 0. {
            return layer;
        }
        let glow = |x, y, w, h, mut color: Rgba, opacity| {
            color.a = opacity * self.strength;
            layout
                .rect(x, y, w, h)
                .rounded(layout.px(h / 2.))
                .shadow(vec![
                    BoxShadow::new(layout.px(0.), layout.px(0.), color.into())
                        .blur_radius(layout.px(220.)),
                ])
        };
        layer
            .bg(linear_gradient(
                165.,
                linear_color_stop(mix(rgb(INK), rgb(0x101c30), self.strength), 0.),
                linear_color_stop(mix(rgb(INK), rgb(0x070c17), self.strength), 1.),
            ))
            .child(glow(
                900. - 480. * self.drift,
                -430. + 180. * self.drift,
                1300.,
                720.,
                self.glow,
                0.34,
            ))
            .child(glow(
                -420. + 340. * self.drift,
                layout.height - 540. - 100. * self.drift,
                1100.,
                700.,
                rgb(0x123d4f),
                0.55,
            ))
    }
}

fn mix(a: Rgba, mut b: Rgba, t: f32) -> Rgba {
    b.a = t;
    a.blend(b)
}

#[derive(Clone, Copy)]
struct Fade {
    choice: GuideBackdrop,
    tint: Option<u32>,
    start: Instant,
    duration: Duration,
    from: Wash,
}

impl Fade {
    fn sample(&self, now: Instant) -> Wash {
        let elapsed = now.duration_since(self.start);
        let target = Wash::new(self.choice, self.tint, elapsed);
        if elapsed >= self.duration {
            target
        } else {
            self.from
                .blend(target, elapsed.as_secs_f32() / self.duration.as_secs_f32())
        }
    }
}

#[derive(Default)]
pub(in super::super) struct BackdropMotion {
    fade: Option<Fade>,
    revision: usize,
}

impl BackdropMotion {
    pub(in super::super) fn render(
        &mut self,
        mut choice: GuideBackdrop,
        tint: Option<u32>,
        window: &Window,
        reduced: bool,
    ) -> Div {
        if reduced && choice == GuideBackdrop::AuroraDrift {
            choice = GuideBackdrop::NordicGradient;
        }
        let tint = tint.filter(|_| choice == GuideBackdrop::ChannelTint);
        let now = Instant::now();
        let fade = self.fade.get_or_insert_with(|| Fade {
            choice,
            tint,
            start: now,
            duration: Duration::ZERO,
            from: Wash::new(choice, tint, Duration::ZERO),
        });
        if fade.choice != choice || fade.tint != tint {
            let duration = if fade.choice == choice {
                Duration::from_secs(1)
            } else {
                Duration::from_millis(320)
            };
            *fade = Fade {
                choice,
                tint,
                start: now,
                duration,
                from: fade.sample(now),
            };
            self.revision = self.revision.wrapping_add(1);
        }
        if reduced {
            fade.duration = Duration::ZERO;
        }
        let layout = Layout::new(window.viewport_size());
        let layer = div().absolute().inset_0();
        let animation = if choice == GuideBackdrop::AuroraDrift {
            Some(
                Animation::new(Duration::from_secs(38))
                    .repeat()
                    .with_max_fps(10.),
            )
        } else if now.duration_since(fade.start) < fade.duration {
            Some(Animation::new(fade.duration).with_max_fps(30.))
        } else {
            None
        };
        if let Some(animation) = animation {
            let fade = *fade;
            div().absolute().inset_0().child(layer.with_animation(
                ("guide-backdrop", self.revision),
                animation,
                move |layer, _| fade.sample(Instant::now()).paint(layout, layer),
            ))
        } else {
            fade.sample(now).paint(layout, layer)
        }
    }
}
