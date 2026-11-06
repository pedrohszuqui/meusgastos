//! Integração com o Android via JNI: permissão de fotos, listagem da galeria
//! (MediaStore), miniaturas e leitura da imagem escolhida.

use crate::{model, AppWindow, Thumb};
use jni::objects::{JObject, JValue};
use jni::{JNIEnv, JavaVM};
use slint::{Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel, Weak};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::sync::Mutex;

/// Incrementado a cada abertura/fechamento da galeria para cancelar cargas antigas.
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// URIs (content://) das fotos exibidas na grade, na mesma ordem das miniaturas.
static URIS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Ponteiro (referência global JNI) da Activity, obtido do AndroidApp no início.
static ACTIVITY: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn set_activity(ptr: *mut std::ffi::c_void) {
    ACTIVITY.store(ptr as usize, SeqCst);
}

const MAX_PHOTOS: usize = 150;
const THUMB_SIZE: i32 = 256;

type JResult<T> = jni::errors::Result<T>;

fn with_env<T, F>(f: F) -> Result<T, String>
where
    F: for<'a> FnOnce(&mut JNIEnv<'a>, &JObject<'a>) -> JResult<T>,
{
    let ctx = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }.map_err(|e| e.to_string())?;
    let mut guard = vm.attach_current_thread().map_err(|e| e.to_string())?;
    let env: &mut JNIEnv = &mut guard;
    let ptr = match ACTIVITY.load(SeqCst) {
        0 => ctx.context() as usize,
        p => p,
    };
    let activity = unsafe { JObject::from_raw(ptr as jni::sys::jobject) };
    let r = f(env, &activity);
    clear_exception(env);
    r.map_err(|e| e.to_string())
}

fn clear_exception(env: &mut JNIEnv) {
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
}

/// Retorna `true` se já houver permissão; caso contrário pede ao usuário e retorna `false`.
fn ensure_permission(env: &mut JNIEnv, act: &JObject) -> JResult<bool> {
    let sdk = env.get_static_field("android/os/Build$VERSION", "SDK_INT", "I")?.i()?;
    let perms: Vec<&str> = if sdk >= 34 {
        vec!["android.permission.READ_MEDIA_IMAGES", "android.permission.READ_MEDIA_VISUAL_USER_SELECTED"]
    } else if sdk >= 33 {
        vec!["android.permission.READ_MEDIA_IMAGES"]
    } else {
        vec!["android.permission.READ_EXTERNAL_STORAGE"]
    };
    for p in &perms {
        let js = env.new_string(p)?;
        let granted = env
            .call_method(act, "checkSelfPermission", "(Ljava/lang/String;)I", &[(&js).into()])?
            .i()?;
        if granted == 0 {
            return Ok(true);
        }
    }
    let arr = env.new_object_array(perms.len() as i32, "java/lang/String", JObject::null())?;
    for (i, p) in perms.iter().enumerate() {
        let js = env.new_string(p)?;
        env.set_object_array_element(&arr, i as i32, js)?;
    }
    env.call_method(act, "requestPermissions", "([Ljava/lang/String;I)V", &[(&arr).into(), JValue::Int(1)])?;
    Ok(false)
}

fn content_resolver<'a>(env: &mut JNIEnv<'a>, act: &JObject) -> JResult<JObject<'a>> {
    env.call_method(act, "getContentResolver", "()Landroid/content/ContentResolver;", &[])?.l()
}

fn parse_uri<'a>(env: &mut JNIEnv<'a>, uri: &str) -> JResult<JObject<'a>> {
    let s = env.new_string(uri)?;
    env.call_static_method("android/net/Uri", "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[(&s).into()])?
        .l()
}

/// Lista as fotos mais recentes da galeria.
fn list_images(env: &mut JNIEnv, act: &JObject) -> JResult<Vec<String>> {
    let resolver = content_resolver(env, act)?;
    let ext = env
        .get_static_field("android/provider/MediaStore$Images$Media", "EXTERNAL_CONTENT_URI", "Landroid/net/Uri;")?
        .l()?;
    let ext_str_obj = env.call_method(&ext, "toString", "()Ljava/lang/String;", &[])?.l()?;
    let base: String = env.get_string(&ext_str_obj.into())?.into();
    let col = env.new_string("_id")?;
    let proj = env.new_object_array(1, "java/lang/String", &col)?;
    let order = env.new_string("date_added DESC")?;
    let null = JObject::null();
    let cursor = env
        .call_method(
            &resolver,
            "query",
            "(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)Landroid/database/Cursor;",
            &[(&ext).into(), (&proj).into(), (&null).into(), (&null).into(), (&order).into()],
        )?
        .l()?;
    let mut out = Vec::new();
    if cursor.is_null() {
        return Ok(out);
    }
    while out.len() < MAX_PHOTOS && env.call_method(&cursor, "moveToNext", "()Z", &[])?.z()? {
        let id = env.call_method(&cursor, "getLong", "(I)J", &[JValue::Int(0)])?.j()?;
        out.push(format!("{base}/{id}"));
    }
    env.call_method(&cursor, "close", "()V", &[])?;
    Ok(out)
}

/// Gera a miniatura de uma foto (RGBA).
fn load_thumb(env: &mut JNIEnv, act: &JObject, uri: &str) -> JResult<(u32, u32, Vec<u8>)> {
    env.with_local_frame(32, |env| {
        let resolver = content_resolver(env, act)?;
        let juri = parse_uri(env, uri)?;
        let size = env.new_object("android/util/Size", "(II)V", &[JValue::Int(THUMB_SIZE), JValue::Int(THUMB_SIZE)])?;
        let null = JObject::null();
        let bmp = env
            .call_method(
                &resolver,
                "loadThumbnail",
                "(Landroid/net/Uri;Landroid/util/Size;Landroid/os/CancellationSignal;)Landroid/graphics/Bitmap;",
                &[(&juri).into(), (&size).into(), (&null).into()],
            )?
            .l()?;
        let w = env.call_method(&bmp, "getWidth", "()I", &[])?.i()?;
        let h = env.call_method(&bmp, "getHeight", "()I", &[])?.i()?;
        let arr = env.new_int_array(w * h)?;
        env.call_method(
            &bmp,
            "getPixels",
            "([IIIIIII)V",
            &[
                (&arr).into(),
                JValue::Int(0),
                JValue::Int(w),
                JValue::Int(0),
                JValue::Int(0),
                JValue::Int(w),
                JValue::Int(h),
            ],
        )?;
        let mut px = vec![0i32; (w * h) as usize];
        env.get_int_array_region(&arr, 0, &mut px)?;
        env.call_method(&bmp, "recycle", "()V", &[])?;
        let mut rgba = Vec::with_capacity(px.len() * 4);
        for p in px {
            let p = p as u32;
            rgba.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8, (p >> 24) as u8]);
        }
        Ok((w as u32, h as u32, rgba))
    })
}

/// Lê todos os bytes de uma URI content://.
fn read_uri(env: &mut JNIEnv, act: &JObject, uri: &str) -> JResult<Vec<u8>> {
    env.with_local_frame(16, |env| {
        let resolver = content_resolver(env, act)?;
        let juri = parse_uri(env, uri)?;
        let input = env
            .call_method(&resolver, "openInputStream", "(Landroid/net/Uri;)Ljava/io/InputStream;", &[(&juri).into()])?
            .l()?;
        const CHUNK: usize = 64 * 1024;
        let buf = env.new_byte_array(CHUNK as i32)?;
        let mut tmp = vec![0i8; CHUNK];
        let mut out = Vec::new();
        loop {
            let n = env.call_method(&input, "read", "([B)I", &[(&buf).into()])?.i()?;
            if n < 0 {
                break;
            }
            let n = n as usize;
            env.get_byte_array_region(&buf, 0, &mut tmp[..n])?;
            out.extend(tmp[..n].iter().map(|b| *b as u8));
        }
        env.call_method(&input, "close", "()V", &[])?;
        Ok(out)
    })
}

/// Abre um endereço no navegador (Intent ACTION_VIEW).
pub fn open_url(url: &str) -> Result<(), String> {
    with_env(|env, act| {
        let action = env.get_static_field("android/content/Intent", "ACTION_VIEW", "Ljava/lang/String;")?.l()?;
        let uri = parse_uri(env, url)?;
        let intent = env.new_object(
            "android/content/Intent",
            "(Ljava/lang/String;Landroid/net/Uri;)V",
            &[(&action).into(), (&uri).into()],
        )?;
        // FLAG_ACTIVITY_NEW_TASK
        env.call_method(&intent, "addFlags", "(I)Landroid/content/Intent;", &[JValue::Int(0x1000_0000)])?;
        env.call_method(act, "startActivity", "(Landroid/content/Intent;)V", &[(&intent).into()])?;
        Ok(())
    })
}

/// Abre (ou recarrega) a grade de fotos da galeria.
pub fn open_picker(w: Weak<AppWindow>) {
    let gen = GENERATION.fetch_add(1, SeqCst) + 1;
    if let Some(ui) = w.upgrade() {
        ui.set_picker_items(ModelRc::new(VecModel::<Thumb>::default()));
        ui.set_picker_status("Carregando fotos...".into());
    }
    std::thread::spawn(move || {
        let status = |msg: String| {
            let _ = w.upgrade_in_event_loop(move |ui| ui.set_picker_status(msg.into()));
        };
        let uris = match with_env(|env, act| {
            if !ensure_permission(env, act)? {
                return Ok(None);
            }
            list_images(env, act).map(Some)
        }) {
            Ok(Some(u)) => u,
            Ok(None) => {
                status("Permita o acesso às fotos na janela do Android e depois toque em \"Atualizar\".".into());
                return;
            }
            Err(e) => {
                status(format!("Erro ao acessar a galeria: {e}"));
                return;
            }
        };
        if uris.is_empty() {
            status("Nenhuma foto encontrada na galeria.".into());
            return;
        }
        *URIS.lock().unwrap() = uris.clone();
        status(String::new());

        let _ = with_env(|env, act| {
            for uri in &uris {
                if GENERATION.load(SeqCst) != gen {
                    break;
                }
                let thumb = match load_thumb(env, act, uri) {
                    Ok(t) => Some(t),
                    Err(_) => {
                        clear_exception(env);
                        None
                    }
                };
                let _ = w.upgrade_in_event_loop(move |ui| {
                    if GENERATION.load(SeqCst) != gen {
                        return;
                    }
                    let image = match thumb {
                        Some((tw, th, px)) => {
                            Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&px, tw, th))
                        }
                        None => Image::default(),
                    };
                    let items = ui.get_picker_items();
                    if let Some(vm) = items.as_any().downcast_ref::<VecModel<Thumb>>() {
                        vm.push(Thumb { image });
                    }
                });
            }
            Ok(())
        });
    });
}

pub fn cancel_picker() {
    GENERATION.fetch_add(1, SeqCst);
}

/// Copia a foto escolhida para a pasta privada do app (reduzida e rotacionada).
pub fn import_selected(w: Weak<AppWindow>, index: usize, dir: PathBuf, name: String) {
    let Some(uri) = URIS.lock().unwrap().get(index).cloned() else { return };
    match w.upgrade() {
        Some(ui) if !ui.get_busy() => ui.set_busy(true),
        _ => return,
    }
    std::thread::spawn(move || {
        let result = with_env(|env, act| read_uri(env, act, &uri)).and_then(|bytes| model::import_photo(&bytes, &dir, &name));
        let _ = w.upgrade_in_event_loop(move |ui| match result {
            Ok(()) => {
                cancel_picker();
                ui.set_picker_items(ModelRc::default());
                ui.invoke_photo_imported(name.into());
            }
            Err(e) => ui.invoke_photo_failed(format!("Não foi possível usar esta foto: {e}").into()),
        });
    });
}
