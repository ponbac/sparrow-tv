use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, BoxShadow, Div, Hsla, Rgba, Window, div, hsla, linear_color_stop,
    linear_gradient, prelude::*, rgb,
};
use sparrow_installed_catalog::GuideBackdrop;

use super::{INK, Layout};

#[derive(Clone, Copy)]
struct Wash {
    strength: f32,
    drift: [f32; 3],
    primary: Hsla,
    secondary: Hsla,
}

impl Wash {
    fn new(choice: GuideBackdrop, tint: Option<f32>, elapsed: Duration) -> Self {
        let drift = [38., 46., 31.].map(|period| {
            if choice == GuideBackdrop::AuroraDrift {
                (1. - (elapsed.as_secs_f32() * std::f32::consts::TAU / period).cos()) * 0.5
            } else {
                0.
            }
        });
        let (primary, secondary) = if choice == GuideBackdrop::ChannelTint
            && let Some(hue) = tint
        {
            let saturation = if (0.14..0.25).contains(&hue) {
                0.46
            } else {
                0.54
            };
            (
                hsla(hue, saturation, 0.37, 1.),
                hsla((hue + 1. / 12.).fract(), 0.42, 0.22, 1.),
            )
        } else {
            (
                mix(rgb(0x2a6f99), rgb(0x59408b), drift[0]).into(),
                rgb(0x123d4f).into(),
            )
        };
        Self {
            strength: if choice == GuideBackdrop::Flat {
                0.
            } else {
                1.
            },
            drift,
            primary,
            secondary,
        }
    }

    fn blend(self, target: Self, t: f32) -> Self {
        Self {
            strength: self.strength + (target.strength - self.strength) * t,
            drift: std::array::from_fn(|i| self.drift[i] + (target.drift[i] - self.drift[i]) * t),
            primary: mix_hue(self.primary, target.primary, t),
            secondary: mix_hue(self.secondary, target.secondary, t),
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
                linear_color_stop(mix(rgb(INK), rgb(0x13233d), self.strength), 0.),
                linear_color_stop(mix(rgb(INK), rgb(0x070c17), self.strength), 1.),
            ))
            .child(glow(
                900. - 480. * self.drift[0],
                -380. + 180. * self.drift[0],
                1300.,
                720.,
                self.primary.into(),
                0.48,
            ))
            .child(glow(
                -420. + 340. * self.drift[1],
                layout.height - 540. - 100. * self.drift[1],
                1100.,
                700.,
                self.secondary.into(),
                0.55,
            ))
            .when(self.drift[2] > 0., |layer| {
                layer.child(glow(
                    760. + 240. * self.drift[2],
                    layout.height - 180.,
                    820.,
                    440.,
                    rgb(0x514568),
                    0.24 * self.drift[2],
                ))
            })
    }
}

fn mix_hue(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let arc = (b.h - a.h + 0.5).rem_euclid(1.) - 0.5;
    hsla(
        (a.h + t * arc).rem_euclid(1.),
        a.s + (b.s - a.s) * t,
        a.l + (b.l - a.l) * t,
        1.,
    )
}

fn mix(a: Rgba, mut b: Rgba, t: f32) -> Rgba {
    b.a = t;
    a.blend(b)
}

#[derive(Clone, Copy)]
struct Fade {
    choice: GuideBackdrop,
    tint: Option<f32>,
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
            let t = elapsed.as_secs_f32() / self.duration.as_secs_f32();
            self.from.blend(target, 1. - (1. - t).powi(3))
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
        tint: Option<f32>,
        defer_tint: bool,
        window: &Window,
        reduced: bool,
    ) -> Div {
        if reduced && choice == GuideBackdrop::AuroraDrift {
            choice = GuideBackdrop::NordicGradient;
        }
        let tint = if defer_tint {
            self.fade.and_then(|fade| fade.tint)
        } else {
            tint
        };
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
