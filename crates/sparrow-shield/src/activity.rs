//! Plain-Activity calls and events correlated to the owning Shield window.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex, OnceLock},
};

use gpui::{App, Window};
use sparrow_core::ChannelDirection;
use sparrow_guide_ui::{ClipboardTarget, FormFactor, GuideHost, HostKey, Insets, TextEditorHost};
use sparrow_installed_catalog::SavedSourceLocations;
use sparrow_tv_guide::{now_focus::Direction, setup::SetupField};

use jni::{
    Env, JavaVM, jni_sig, jni_str,
    objects::{Global, JObject, JString, JValue},
};

static JVM: OnceLock<JavaVM> = OnceLock::new();
type ActivityRef = Arc<Global<JObject<'static>>>;
static ACTIVITIES: LazyLock<Mutex<HashMap<u64, RegisteredActivity>>> =
    LazyLock::new(Mutex::default);

struct RegisteredActivity {
    activity: ActivityRef,
    host: ShieldActivity,
    insets: Insets,
}

#[derive(Clone, Copy)]
pub(super) struct ShieldActivity {
    id: u64,
    form: FormFactor,
    picture_in_picture: bool,
}

impl GuideHost for ShieldActivity {
    fn id(&self) -> u64 {
        self.id
    }

    fn form_factor(&self) -> FormFactor {
        self.form
    }

    fn initial_insets(&self) -> Insets {
        ACTIVITIES
            .lock()
            .unwrap()
            .get(&self.id)
            .map_or(Insets::default(), |entry| entry.insets)
    }

    fn set_immersive(&self, immersive: bool) {
        call(self.id, |env, activity| {
            env.call_method(
                activity,
                jni_str!("setImmersive"),
                jni_sig!("(JZ)V"),
                &[JValue::Long(self.id as i64), JValue::Bool(immersive)],
            )?;
            Ok(())
        });
    }

    fn supports_picture_in_picture(&self) -> bool {
        self.picture_in_picture
    }

    fn enter_picture_in_picture(&self) -> bool {
        let mut queued = false;
        self.picture_in_picture
            && call(self.id, |env, activity| {
                queued = env
                    .call_method(
                        activity,
                        jni_str!("enterPictureInPicture"),
                        jni_sig!("(J)Z"),
                        &[JValue::Long(self.id as i64)],
                    )?
                    .z()?;
                Ok(())
            })
            && queued
    }

    fn guide_painted(&self) {
        guide_presented(self.id);
    }

    fn finish(&self, _: &mut Window) {
        finish(self.id);
    }

    fn copy_diagnostics(&self, target: ClipboardTarget, text: &str, _: &mut App) -> bool {
        match target {
            ClipboardTarget::Source { session, attempt } => {
                copy_source_diagnostics(self.id, session, attempt, text)
            }
            ClipboardTarget::Playback { attempt } => {
                copy_playback_diagnostics(self.id, attempt, text)
            }
        }
    }
}

impl TextEditorHost for ShieldActivity {
    fn open_query(&self, generation: u64, rect: [i32; 4], initial: &str) -> bool {
        open_text_query(self.id, generation, rect, initial)
    }

    fn close_editor(&self, generation: u64) {
        close_text_editor(self.id, generation);
    }

    fn begin_draft(&self, session: u64, seed: Option<SavedSourceLocations>) -> bool {
        begin_source_draft(self.id, session, seed)
    }

    fn close_draft(&self, session: u64) {
        close_source_draft(self.id, session);
    }

    fn edit_field(&self, session: u64, generation: u64, field: SetupField, rect: [i32; 4]) -> bool {
        edit_source(self.id, session, generation, field_index(field), rect)
    }

    fn clear_field(&self, session: u64, generation: u64, field: SetupField) -> bool {
        clear_source_field(self.id, session, generation, field_index(field))
    }

    fn submit_draft(&self, session: u64, attempt: u64) -> bool {
        submit_sources(self.id, session, attempt)
    }

    fn finish_save(&self, session: u64, attempt: u64, saved: bool) {
        finish_source_save(self.id, session, attempt, saved);
    }
}

const fn field_index(field: SetupField) -> i32 {
    match field {
        SetupField::M3u => 0,
        SetupField::Epg => 1,
    }
}

pub(super) const fn setup_field(index: i32) -> Option<SetupField> {
    match index {
        0 => Some(SetupField::M3u),
        1 => Some(SetupField::Epg),
        _ => None,
    }
}

pub(super) const fn host_key(code: i32) -> HostKey {
    match code {
        19 => HostKey::Arrow(Direction::Up),
        20 => HostKey::Arrow(Direction::Down),
        21 => HostKey::Arrow(Direction::Left),
        22 => HostKey::Arrow(Direction::Right),
        23 | 66 => HostKey::Activate,
        102 | 167 => HostKey::Channel(ChannelDirection::Previous),
        103 | 166 => HostKey::Channel(ChannelDirection::Next),
        85 => HostKey::TogglePause,
        86 => HostKey::Stop,
        126 => HostKey::Resume,
        127 => HostKey::Pause,
        _ => HostKey::Other,
    }
}

pub(super) fn register(
    env: &mut Env<'_>,
    id: u64,
    activity: &JObject<'_>,
    television: bool,
    picture_in_picture: bool,
) -> jni::errors::Result<()> {
    if JVM.get().is_none() {
        let _ = JVM.set(env.get_java_vm()?);
    }
    let activity = env.new_global_ref(activity)?;
    ACTIVITIES.lock().unwrap().insert(
        id,
        RegisteredActivity {
            activity: Arc::new(activity),
            host: ShieldActivity {
                id,
                form: if television {
                    FormFactor::Television
                } else {
                    FormFactor::Handheld
                },
                picture_in_picture,
            },
            insets: Insets::default(),
        },
    );
    Ok(())
}

pub(super) fn registered(id: u64) -> Option<ShieldActivity> {
    ACTIVITIES.lock().unwrap().get(&id).map(|entry| entry.host)
}

pub(super) fn update_insets(id: u64, insets: Insets) -> bool {
    let mut activities = ACTIVITIES.lock().unwrap();
    let Some(entry) = activities.get_mut(&id) else {
        return false;
    };
    if entry.insets == insets {
        return false;
    }
    entry.insets = insets;
    true
}

pub(super) fn unregister(id: u64) {
    ACTIVITIES.lock().unwrap().remove(&id);
}

pub(super) fn call(
    id: u64,
    action: impl FnOnce(&mut Env<'_>, &JObject<'_>) -> jni::errors::Result<()>,
) -> bool {
    let Some(vm) = JVM.get() else {
        return false;
    };
    let Some(activity) = ACTIVITIES
        .lock()
        .unwrap()
        .get(&id)
        .map(|entry| Arc::clone(&entry.activity))
    else {
        return false;
    };
    vm.attach_current_thread(|env| {
        let result = action(env, activity.as_ref().as_ref());
        if result.is_err() && env.exception_check() {
            env.exception_clear();
        }
        result
    })
    .is_ok()
}

pub(super) fn enable_key_routing(id: u64) {
    call(id, |env, activity| {
        env.call_method(
            activity,
            jni_str!("setHostKeyRouting"),
            jni_sig!("(JZ)V"),
            &[JValue::Long(id as i64), JValue::Bool(true)],
        )?;
        Ok(())
    });
}

fn guide_presented(id: u64) {
    call(id, |env, activity| {
        env.call_method(
            activity,
            jni_str!("guidePresented"),
            jni_sig!("(J)V"),
            &[JValue::Long(id as i64)],
        )?;
        Ok(())
    });
}

fn finish(id: u64) {
    call(id, |env, activity| {
        env.call_method(
            activity,
            jni_str!("finishFromNative"),
            jni_sig!("(J)V"),
            &[JValue::Long(id as i64)],
        )?;
        Ok(())
    });
}

fn open_text_query(id: u64, generation: u64, rect: [i32; 4], initial: &str) -> bool {
    let [left, top, width, height] = rect;
    call(id, |env, activity| {
        let text = JString::from_str(env, initial)?;
        env.call_method(
            activity,
            jni_str!("openTextEditor"),
            jni_sig!("(JJIIIIILjava/lang/String;)V"),
            &[
                JValue::Long(id as i64),
                JValue::Long(generation as i64),
                JValue::Int(0),
                JValue::Int(left),
                JValue::Int(top),
                JValue::Int(width),
                JValue::Int(height),
                JValue::Object(text.as_ref()),
            ],
        )?;
        Ok(())
    })
}

fn close_text_editor(id: u64, generation: u64) {
    call(id, |env, activity| {
        env.call_method(
            activity,
            jni_str!("closeTextEditor"),
            jni_sig!("(JJ)V"),
            &[JValue::Long(id as i64), JValue::Long(generation as i64)],
        )?;
        Ok(())
    });
}

/// Opens a Source Configuration draft, prefilled with saved locations when given.
/// The locations pass straight to the Activity's draft and are not retained.
fn begin_source_draft(id: u64, session: u64, seed: Option<SavedSourceLocations>) -> bool {
    call(id, |env, activity| {
        let (m3u, epg) = seed.map_or((None, None), |seed| {
            let (m3u, epg) = seed.into_locations();
            (Some(m3u), epg)
        });
        let m3u = m3u.map(|text| JString::from_str(env, &text)).transpose()?;
        let epg = epg.map(|text| JString::from_str(env, &text)).transpose()?;
        let null = JObject::null();
        env.call_method(
            activity,
            jni_str!("beginSourceDraft"),
            jni_sig!("(JJLjava/lang/String;Ljava/lang/String;)V"),
            &[
                JValue::Long(id as i64),
                JValue::Long(session as i64),
                JValue::Object(m3u.as_ref().map_or(&null, |text| text.as_ref())),
                JValue::Object(epg.as_ref().map_or(&null, |text| text.as_ref())),
            ],
        )?;
        Ok(())
    })
}

fn close_source_draft(id: u64, session: u64) {
    call(id, |env, activity| {
        env.call_method(
            activity,
            jni_str!("closeSourceDraft"),
            jni_sig!("(JJ)V"),
            &[JValue::Long(id as i64), JValue::Long(session as i64)],
        )?;
        Ok(())
    });
}

fn clear_source_field(id: u64, session: u64, generation: u64, field: i32) -> bool {
    call(id, |env, activity| {
        env.call_method(
            activity,
            jni_str!("clearSourceField"),
            jni_sig!("(JJJI)V"),
            &[
                JValue::Long(id as i64),
                JValue::Long(session as i64),
                JValue::Long(generation as i64),
                JValue::Int(field),
            ],
        )?;
        Ok(())
    })
}

fn edit_source(id: u64, session: u64, generation: u64, field: i32, rect: [i32; 4]) -> bool {
    let [left, top, width, height] = rect;
    call(id, |env, activity| {
        env.call_method(
            activity,
            jni_str!("openSourceField"),
            jni_sig!("(JJJIIIII)V"),
            &[
                JValue::Long(id as i64),
                JValue::Long(session as i64),
                JValue::Long(generation as i64),
                JValue::Int(field),
                JValue::Int(left),
                JValue::Int(top),
                JValue::Int(width),
                JValue::Int(height),
            ],
        )?;
        Ok(())
    })
}

fn copy_source_diagnostics(id: u64, session: u64, attempt: u64, text: &str) -> bool {
    call(id, |env, activity| {
        let text = JString::from_str(env, text)?;
        env.call_method(
            activity,
            jni_str!("copySourceDiagnostics"),
            jni_sig!("(JJJLjava/lang/String;)V"),
            &[
                JValue::Long(id as i64),
                JValue::Long(session as i64),
                JValue::Long(attempt as i64),
                JValue::Object(text.as_ref()),
            ],
        )?;
        Ok(())
    })
}

fn copy_playback_diagnostics(id: u64, attempt: u64, text: &str) -> bool {
    call(id, |env, activity| {
        let text = JString::from_str(env, text)?;
        env.call_method(
            activity,
            jni_str!("copyPlaybackDiagnostics"),
            jni_sig!("(JJLjava/lang/String;)V"),
            &[
                JValue::Long(id as i64),
                JValue::Long(attempt as i64),
                JValue::Object(text.as_ref()),
            ],
        )?;
        Ok(())
    })
}

fn submit_sources(id: u64, session: u64, attempt: u64) -> bool {
    call(id, |env, activity| {
        env.call_method(
            activity,
            jni_str!("saveSourceDraft"),
            jni_sig!("(JJJ)V"),
            &[
                JValue::Long(id as i64),
                JValue::Long(session as i64),
                JValue::Long(attempt as i64),
            ],
        )?;
        Ok(())
    })
}

fn finish_source_save(id: u64, session: u64, attempt: u64, saved: bool) {
    call(id, |env, activity| {
        env.call_method(
            activity,
            jni_str!("finishSourceSave"),
            jni_sig!("(JJJZ)V"),
            &[
                JValue::Long(id as i64),
                JValue::Long(session as i64),
                JValue::Long(attempt as i64),
                JValue::Bool(saved),
            ],
        )?;
        Ok(())
    });
}
