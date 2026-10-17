//! JNI surface of the native library. Kotlin sees four static functions on
//! `app.marginalia.Native`; everything richer travels as JSON strings.
//!
//!   init(dataDir, modelsDir, tzOffsetSecs) -> JSON    create the core and start loading models
//!   call(method, argsJson)                 -> JSON    {"ok": …} or {"error": "…"}
//!   pushAudio(short[], n)                  -> float   16 kHz mono PCM16 in, input level out
//!   setListener(listener)                             listener.onEvent(String json) receives events

use crate::config::Paths;
use crate::engine::embed::EmbedHandle;
use crate::engine::llm::{ChatMessage, GenEvent, GenRequest, GenStats, LlmHandle, Role};
use crate::session::{Core, Sink};
use jni::objects::{GlobalRef, JClass, JObject, JShortArray, JString, JValue};
use jni::sys::{jfloat, jint, jstring};
use jni::{JNIEnv, JavaVM};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

static CORE: OnceLock<Arc<Core>> = OnceLock::new();
static VM: OnceLock<JavaVM> = OnceLock::new();
static LISTENER: Mutex<Option<GlobalRef>> = Mutex::new(None);
/// Kotlin object that runs chat generations when LiteRT-LM is the chosen engine.
static GENERATOR: Mutex<Option<GlobalRef>> = Mutex::new(None);
/// Kotlin object that turns texts into EmbeddingGemma vectors (LiteRT-LM).
static EMBEDDER: Mutex<Option<GlobalRef>> = Mutex::new(None);
/// In-flight external generations: request id -> (kind, text) events from Kotlin.
static REQUESTS: Mutex<Option<HashMap<u64, crossbeam_channel::Sender<(i32, String)>>>> = Mutex::new(None);
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

#[cfg(target_os = "android")]
mod logcat {
    use std::ffi::CString;
    use std::io::Write;
    use tracing_subscriber::fmt::MakeWriter;

    pub struct Logcat;
    pub struct Line(Vec<u8>);

    impl Write for Line {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Drop for Line {
        fn drop(&mut self) {
            let text = String::from_utf8_lossy(&self.0).replace('\0', "");
            if let (Ok(tag), Ok(msg)) = (CString::new("Marginalia"), CString::new(text.trim_end())) {
                unsafe {
                    android_log_sys::__android_log_write(android_log_sys::LogPriority::INFO as i32, tag.as_ptr(), msg.as_ptr());
                }
            }
        }
    }

    impl<'a> MakeWriter<'a> for Logcat {
        type Writer = Line;
        fn make_writer(&'a self) -> Line {
            Line(Vec::new())
        }
    }
}

fn init_logging(data_dir: &str) {
    #[cfg(target_os = "android")]
    {
        // `touch files/debug-log` turns on detailed timing logs (whisper decode times and so on).
        let level = if Path::new(data_dir).join("debug-log").exists() { tracing::Level::DEBUG } else { tracing::Level::INFO };
        let _ = tracing_subscriber::fmt()
            .with_writer(logcat::Logcat)
            .with_ansi(false)
            .without_time()
            .with_max_level(level)
            .try_init();
    }
    #[cfg(not(target_os = "android"))]
    let _ = data_dir;
}

fn jstr(env: &mut JNIEnv, s: &JString) -> String {
    env.get_string(s).map(|s| s.into()).unwrap_or_default()
}

fn out(env: &mut JNIEnv, v: Value) -> jstring {
    env.new_string(v.to_string()).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
}

fn guarded(f: impl FnOnce() -> anyhow::Result<Value>) -> Value {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(Ok(v)) => json!({ "ok": v }),
        Ok(Err(e)) => json!({ "error": format!("{e:#}") }),
        Err(_) => json!({ "error": "internal error (panic)" }),
    }
}

#[no_mangle]
pub extern "system" fn Java_app_marginalia_Native_init<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    data_dir: JString<'l>,
    models_dir: JString<'l>,
    tz_offset: jint,
    llm_backend: JString<'l>,
    external_embedder: jni::sys::jboolean,
) -> jstring {
    let data = jstr(&mut env, &data_dir);
    init_logging(&data);
    if let Ok(vm) = env.get_java_vm() {
        let _ = VM.set(vm);
    }
    let models = jstr(&mut env, &models_dir);
    let llm_backend = jstr(&mut env, &llm_backend);
    let res = guarded(|| {
        if let Some(core) = CORE.get() {
            core.boot_if_idle();
            return Ok(json!({"already": true}));
        }
        if let Err(e) = crate::remote::init(Path::new(&data)) {
            tracing::warn!("home computers are not available: {e:#}");
        }
        let paths = Paths::new(Path::new(&data), Path::new(&models));
        let core = Core::new(paths, tz_offset)?;
        if llm_backend.starts_with("litert") {
            core.set_external_llm(external_llm());
        }
        if external_embedder != 0 {
            core.set_external_embedder(external_embed());
        }
        let missing = core.missing_models();
        let _ = CORE.set(core.clone());
        install_sink(&core);
        if missing.is_empty() {
            core.boot();
        }
        Ok(json!({"missing": missing}))
    });
    out(&mut env, res)
}

#[no_mangle]
pub extern "system" fn Java_app_marginalia_Native_call<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    method: JString<'l>,
    args: JString<'l>,
) -> jstring {
    let method = jstr(&mut env, &method);
    let args = jstr(&mut env, &args);
    let res = guarded(|| {
        let core = CORE.get().ok_or_else(|| anyhow::anyhow!("native core is not initialised"))?;
        let args: Value = if args.trim().is_empty() { json!({}) } else { serde_json::from_str(&args)? };
        core.call(&method, &args)
    });
    out(&mut env, res)
}

#[no_mangle]
pub extern "system" fn Java_app_marginalia_Native_pushAudio<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    samples: JShortArray<'l>,
    len: jint,
) -> jfloat {
    let Some(core) = CORE.get() else { return 0.0 };
    let n = len.max(0) as usize;
    let mut buf = vec![0i16; n];
    if env.get_short_array_region(&samples, 0, &mut buf).is_err() {
        return 0.0;
    }
    let pcm: Vec<f32> = buf.iter().map(|&s| s as f32 / 32768.0).collect();
    core.push_audio(&pcm)
}

#[no_mangle]
pub extern "system" fn Java_app_marginalia_Native_setListener<'l>(
    env: JNIEnv<'l>,
    _class: JClass<'l>,
    listener: JObject<'l>,
) {
    let global = if listener.is_null() { None } else { env.new_global_ref(&listener).ok() };
    *LISTENER.lock() = global;
    if let Some(core) = CORE.get() {
        install_sink(core);
    }
}

fn install_sink(core: &Arc<Core>) {
    let sink: Sink = Arc::new(|json: &str| {
        let Some(vm) = VM.get() else { return };
        let Some(listener) = LISTENER.lock().clone() else { return };
        let Ok(mut env) = vm.attach_current_thread() else { return };
        if let Ok(s) = env.new_string(json) {
            let _ = env.call_method(listener.as_obj(), "onEvent", "(Ljava/lang/String;)V", &[JValue::Object(&s)]);
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_clear();
            }
        }
    });
    core.set_sink(Some(sink));
}

/// `Native.emit(requestId, kind, text)`: Kotlin streams a generation back. kind 0 = text so
/// far added, 1 = finished (text is a stats JSON), 2 = failed (text is the message).
#[no_mangle]
pub extern "system" fn Java_app_marginalia_Native_emit<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    request: jni::sys::jlong,
    kind: jint,
    text: JString<'l>,
) {
    let text = jstr(&mut env, &text);
    if let Some(tx) = REQUESTS.lock().as_ref().and_then(|m| m.get(&(request as u64))) {
        let _ = tx.send((kind, text));
    }
}

#[no_mangle]
pub extern "system" fn Java_app_marginalia_Native_setGenerator<'l>(env: JNIEnv<'l>, _class: JClass<'l>, generator: JObject<'l>) {
    *GENERATOR.lock() = if generator.is_null() { None } else { env.new_global_ref(&generator).ok() };
}

/// The models load in the background after the database opens, so a request that arrives early
/// waits (up to two minutes) for Kotlin to hand over its engine.
fn wait_for(slot: &Mutex<Option<GlobalRef>>, what: &str) -> anyhow::Result<GlobalRef> {
    for _ in 0..600 {
        if let Some(g) = slot.lock().clone() {
            return Ok(g);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    anyhow::bail!("{what} is not ready yet")
}

fn call_generator(method: &str, sig: &str, args: &[JValue]) -> anyhow::Result<()> {
    let vm = VM.get().ok_or_else(|| anyhow::anyhow!("no JVM"))?;
    let gen = wait_for(&GENERATOR, "The AI")?;
    let mut env = vm.attach_current_thread()?;
    env.call_method(gen.as_obj(), method, sig, args)?;
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
        anyhow::bail!("generator threw in {method}");
    }
    Ok(())
}

/// A chat model whose generations run in Kotlin (LiteRT-LM), behind high and low
/// priority queues.
fn external_llm() -> LlmHandle {
    LlmHandle::external(Arc::new(|req: &GenRequest| -> anyhow::Result<GenStats> {
        // A home computer takes the job when one is paired, on, and reachable. If it cannot start
        // the job, the phone does it itself, as it always did.
        if let Some(done) = crate::remote::get().and_then(|r| r.chat(req)) {
            return done;
        }
        let id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = crossbeam_channel::unbounded::<(i32, String)>();
        REQUESTS.lock().get_or_insert_with(HashMap::new).insert(id, tx);
        let role = |r: Role| match r { Role::System => "system", Role::User => "user", Role::Assistant => "assistant" };
        let body = json!({
            "messages": req.messages.iter().map(|m: &ChatMessage| json!({"role": role(m.role), "text": m.content})).collect::<Vec<_>>(),
            "images": req.images.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "max_tokens": req.params.max_tokens, "temperature": req.params.temperature,
            "top_p": req.params.top_p, "top_k": req.params.top_k,
        });
        let result = (|| {
            let vm = VM.get().ok_or_else(|| anyhow::anyhow!("no JVM"))?;
            let mut env = vm.attach_current_thread()?;
            let s = env.new_string(body.to_string())?;
            call_generator("start", "(JLjava/lang/String;)V", &[JValue::Long(id as i64), JValue::Object(&s)])?;
            let mut cancelled = false;
            let mut stats = GenStats::default();
            loop {
                match rx.recv_timeout(std::time::Duration::from_millis(30)) {
                    Ok((0, t)) => (req.on_event)(GenEvent::Token(t)),
                    Ok((1, s)) => {
                        let v: Value = serde_json::from_str(&s).unwrap_or(json!({}));
                        stats.prompt_tokens = v["prompt_tokens"].as_u64().unwrap_or(0) as usize;
                        stats.generated_tokens = v["generated_tokens"].as_u64().unwrap_or(0) as usize;
                        break;
                    }
                    Ok((_, e)) => anyhow::bail!(e),
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                        if req.cancel.load(Ordering::Relaxed) && !cancelled {
                            cancelled = true;
                            let _ = call_generator("cancel", "(J)V", &[JValue::Long(id as i64)]);
                        }
                    }
                    Err(_) => break,
                }
            }
            Ok(stats)
        })();
        if let Some(m) = REQUESTS.lock().as_mut() {
            m.remove(&id);
        }
        result
    }))
}

#[no_mangle]
pub extern "system" fn Java_app_marginalia_Native_setEmbedder<'l>(env: JNIEnv<'l>, _class: JClass<'l>, embedder: JObject<'l>) {
    *EMBEDDER.lock() = if embedder.is_null() { None } else { env.new_global_ref(&embedder).ok() };
}

/// Embeddings computed in Kotlin by LiteRT-LM, behind [`EmbedHandle`].
fn external_embed() -> EmbedHandle {
    EmbedHandle::external(Arc::new(|texts: Vec<String>| -> anyhow::Result<Vec<Vec<f32>>> {
        if let Some(v) = crate::remote::get().and_then(|r| r.embed(&texts)) {
            return Ok(v);
        }
        let vm = VM.get().ok_or_else(|| anyhow::anyhow!("no JVM"))?;
        let emb = wait_for(&EMBEDDER, "the search model")?;
        let mut env = vm.attach_current_thread()?;
        let string_class = env.find_class("java/lang/String")?;
        let empty = env.new_string("")?;
        let arr = env.new_object_array(texts.len() as i32, &string_class, &empty)?;
        for (i, t) in texts.iter().enumerate() {
            let js = env.new_string(t)?;
            env.set_object_array_element(&arr, i as i32, &js)?;
        }
        let res = env.call_method(emb.as_obj(), "embed", "([Ljava/lang/String;)[[F", &[JValue::Object(&arr)]);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
            anyhow::bail!("embedder threw");
        }
        let outer = jni::objects::JObjectArray::from(res?.l()?);
        let n = env.get_array_length(&outer)?;
        let mut out = Vec::with_capacity(n as usize);
        for i in 0..n {
            let row = jni::objects::JFloatArray::from(env.get_object_array_element(&outer, i)?);
            let len = env.get_array_length(&row)? as usize;
            let mut v = vec![0f32; len];
            env.get_float_array_region(&row, 0, &mut v)?;
            out.push(v);
        }
        Ok(out)
    }))
}
