//! Resolve exact Catalog families outside GPUI and send typed tune intents.

use gpui::Context;
use sparrow_core::{CatalogGeneration, ChannelDirection, ChannelFamily, ChannelId, ChannelSummary};
use sparrow_installed_playback::tv::{TvPlaybackCommand as Command, TvTuneTarget};
use sparrow_tv_guide::{
    VariantPreferences, guide_screen::ScreenFocus, normalize_preference_title, now_focus::NowFocus,
    search::SearchTune, timeline_focus::TimelineFocus,
};

use super::{GuideSurface, channel_title};

const MAX_ZAP_KEYS: usize = 32;

impl GuideSurface {
    pub(in super::super) fn guide_tune(&mut self, repeat: bool, cx: &mut Context<Self>) -> bool {
        let (index, chip) = match self.screen.focus() {
            ScreenFocus::Board(NowFocus::Channel(index))
            | ScreenFocus::TimelineBoard(
                TimelineFocus::Channel(index) | TimelineFocus::Programme { family: index, .. },
            ) => (index, None),
            ScreenFocus::Board(NowFocus::Variant { family, chip })
            | ScreenFocus::TimelineBoard(TimelineFocus::Variant { family, chip }) => {
                (family, Some(chip))
            }
            _ => return false,
        };
        let Some(family) = self.screen.families().get(index) else {
            return false;
        };
        let preferred = family.preferred_variant(self.screen.variant_preferences());
        let channel = chip
            .and_then(|chip| family.quality_variants().nth(chip))
            .unwrap_or(preferred)
            .channel()
            .id()
            .clone();
        let Some(generation) = self.screen.generation() else {
            return false;
        };
        self.tune(generation, channel, chip.is_some(), repeat, cx);
        if !repeat {
            self.pocket.tuned(self.host.form_factor(), self.profile);
        }
        true
    }

    pub(in super::super) fn search_tune(&mut self, tune: SearchTune, cx: &mut Context<Self>) {
        if Some(tune.generation) == self.screen.generation() {
            self.tune(tune.generation, tune.channel, false, false, cx);
            self.pocket.tuned(self.host.form_factor(), self.profile);
        }
    }

    pub(super) fn tune(
        &mut self,
        generation: CatalogGeneration,
        channel: ChannelId,
        prefer_quality: bool,
        repeat: bool,
        cx: &mut Context<Self>,
    ) {
        if repeat {
            return;
        }
        self.playback.cancel_zap();
        let revision = self.playback.tune_revision;
        let activation = self.activation;
        let Some(query) = self.catalog.playback_query(move |core| {
            let channel_number = core.channel_number(generation, &channel).ok()??;
            let family = core.channel_family(generation, channel_number).ok()??;
            let selected = family
                .variants()
                .iter()
                .find(|variant| variant.id() == &channel)?;
            let preference = family
                .variants()
                .first()
                .filter(|_| prefer_quality)
                .map(|first| {
                    (
                        first.group().to_owned(),
                        normalize_preference_title(channel_title(first)),
                    )
                });
            Some((TvTuneTarget::from_family(&family, selected), preference))
        }) else {
            self.show_feedback(Some("Playback is unavailable."), cx);
            return;
        };
        cx.spawn(async move |view, cx| {
            let target = query.await.ok().flatten().flatten();
            let _ = view.update(cx, |view, cx| {
                if revision != view.playback.tune_revision
                    || view.screen.generation() != Some(generation)
                    || view.activation.is_none()
                    || view.activation != activation
                {
                    return;
                }
                if let Some((target, preference)) = target {
                    let chosen = target.quality;
                    if view.send_playback(Command::Tune { target, repeat }) {
                        if let (Some((group, title)), Some(quality)) = (preference, chosen) {
                            view.save_variant_preference(group, title, quality, cx);
                        }
                        return;
                    }
                }
                view.show_feedback(Some("Playback is unavailable."), cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn save_variant_preference(
        &mut self,
        group: String,
        title: String,
        quality: sparrow_core::ChannelQuality,
        cx: &mut Context<Self>,
    ) {
        let message = "Couldn’t save the Quality Variant choice.";
        let Some(write) = self.catalog.prefer_variant(group, title, quality) else {
            self.show_feedback(Some(message), cx);
            cx.notify();
            return;
        };
        cx.spawn(async move |view, cx| {
            if !write.await.unwrap_or(false) {
                let _ = view.update(cx, |view, cx| {
                    view.show_feedback(Some(message), cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(super) fn zap(
        &mut self,
        direction: ChannelDirection,
        repeat: bool,
        cx: &mut Context<Self>,
    ) {
        if self.playback.state.selected.is_some() && self.playback.zap_keys.len() < MAX_ZAP_KEYS {
            let now = std::time::Instant::now();
            let last = self.playback.last_zap.replace(now);
            self.playback.roll = if !repeat
                && !cx.reduce_motion()
                && last.is_none_or(|last| now.duration_since(last).as_millis() >= 240)
                && !self.playback.zap_running
            {
                self.playback
                    .shown_subject()
                    .map(|subject| subject.start_roll(self.now.to_utc(), direction))
            } else {
                None
            };
            self.playback.zap_keys.push_back(direction);
            self.advance_zap(cx);
        }
    }

    fn advance_zap(&mut self, cx: &mut Context<Self>) {
        if self.playback.zap_running {
            return;
        }
        let Some(direction) = self.playback.zap_keys.pop_front() else {
            return;
        };
        let Some(subject) = self
            .playback
            .zap_target
            .as_ref()
            .or_else(|| self.playback.shown())
        else {
            return;
        };
        let channel_id = subject.channel.clone();
        let Some(generation) = self.screen.generation() else {
            return;
        };
        let excluded = self.screen.exclusions().clone();
        let preferences = self.screen.variant_preferences().clone();
        let Some(query) = self.catalog.playback_query(move |core| {
            let number = core.channel_number(generation, &channel_id).ok()??;
            let family = core
                .adjacent_channel_family(generation, number, direction, &excluded)
                .ok()??;
            let channel = preferred(&family, &preferences)?;
            Some(TvTuneTarget::from_family(&family, channel))
        }) else {
            return;
        };
        self.playback.zap_running = true;
        let revision = self.playback.tune_revision;
        let zap_revision = self.playback.state.zap_revision;
        let activation = self.activation;
        cx.spawn(async move |view, cx| {
            let target = query.await.ok().flatten().flatten();
            let _ = view.update(cx, |view, cx| {
                if revision != view.playback.tune_revision
                    || zap_revision != view.playback.state.zap_revision
                {
                    return;
                }
                view.playback.zap_running = false;
                if view.activation != activation
                    || view.activation.is_none()
                    || view.screen.generation() != Some(generation)
                {
                    view.playback.cancel_zap();
                    return;
                }
                if let Some(target) = target
                    && view.send_playback(Command::Zap {
                        target: target.clone(),
                        revision: zap_revision,
                    })
                {
                    view.playback.zap_target = Some(target);
                }
                view.advance_zap(cx);
            });
        })
        .detach();
    }
}

fn preferred<'a>(
    family: &'a ChannelFamily,
    preferences: &VariantPreferences,
) -> Option<&'a ChannelSummary> {
    let first = family.variants().first()?;
    let wanted = preferences.quality(first.group(), channel_title(first));
    family.variants().iter().max_by_key(|channel| {
        let quality = channel.variant().map(|variant| variant.quality());
        (quality.is_some() && quality == wanted, quality)
    })
}
