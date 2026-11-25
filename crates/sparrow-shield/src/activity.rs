//! Plain-Activity calls and events correlated to the owning Shield window.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex, OnceLock},
};

use sparrow_installed_catalog::SavedSourceLocations;

use jni::{
    Env, JavaVM, jni_sig, jni_str,
    objects::{Global, JObject, JString, JValue},
};

static JVM: OnceLock<JavaVM> = OnceLock::new();
type ActivityRef = Arc<Global<JObject<'static>>>;
static ACTIVITIES: LazyLock<Mutex<HashMap<u64, ActivityRef>>> = LazyLock::new(Mutex::default);

pub(super) enum Event {
    Back,
    Key { code: i32, repeat: i32 },
    Edited { generation: u64, edit: Edit },
    Source { session: u64, update: SourceEvent },
    PlaybackDiagnosticsCopied { attempt: u64, copied: bool },
}

pub(super) enum SourceEvent {
    Copied {
        attempt: u64,
        copied: bool,
    },
    DraftStatus {
        attempt: u64,
        status: i32,
    },
    Field {
        generation: u64,
        field: i32,
        event: i32,
        has_text: bool,
    },
    Submit {
        attempt: u64,
        m3u: String,
        epg: Option<String>,
    },
}

pub(super) enum Edit {
    Changed(String),
    Committed(String),
    Cancelled,
    Rejected,
}

pub(super) fn register(
    env: &mut Env<'_>,
    id: u64,
    activity: &JObject<'_>,
) -> jni::errors::Result<()> {
    if JVM.get().is_none() {
        let _ = JVM.set(env.get_java_vm()?);
    }
    let activity = env.new_global_ref(activity)?;
    ACTIVITIES.lock().unwrap().insert(id, Arc::new(activity));
    Ok(())
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
    let Some(activity) = ACTIVITIES.lock().unwrap().get(&id).cloned() else {
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

pub(super) fn finish(id: u64) {
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

pub(super) fn open_text_query(id: u64, generation: u64, rect: [i32; 4], initial: &str) -> bool {
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

pub(super) fn close_text_editor(id: u64, generation: u64) {
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
pub(super) fn begin_source_draft(
    id: u64,
    session: u64,
    seed: Option<SavedSourceLocations>,
) -> bool {
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

pub(super) fn close_source_draft(id: u64, session: u64) {
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

pub(super) fn clear_source_field(id: u64, session: u64, generation: u64, field: i32) -> bool {
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

pub(super) fn edit_source(
    id: u64,
    session: u64,
    generation: u64,
    field: i32,
    rect: [i32; 4],
) -> bool {
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

pub(super) fn copy_source_diagnostics(id: u64, session: u64, attempt: u64, text: &str) -> bool {
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

pub(super) fn copy_playback_diagnostics(id: u64, attempt: u64, text: &str) -> bool {
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

pub(super) fn submit_sources(id: u64, session: u64, attempt: u64) -> bool {
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

pub(super) fn finish_source_save(id: u64, session: u64, attempt: u64, saved: bool) {
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
