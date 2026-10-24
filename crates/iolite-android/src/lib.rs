//! JNI bridge for `io.github.redhatontop.iolite.Native`.
//!
//! Networking runs on a shared Tokio runtime. Each session gets a dedicated thread, attached to
//! the JVM, that calls the Kotlin listener in event order. Handles are ids into a session table,
//! so a stale handle from Kotlin is ignored instead of dereferenced.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock};

use anyhow::Result;
use iolite_client::{Config, Event};
use iolite_proto::identity::{Fingerprint, Identity};
use iolite_proto::{ClientMsg, Hello, Input, MouseButton, TouchPhase, PROTOCOL_VERSION};
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jfloat, jint, jlong, JNI_TRUE};
use jni::{JNIEnv, JavaVM};
use tokio::sync::mpsc;

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static SESSIONS: OnceLock<Mutex<HashMap<i64, mpsc::UnboundedSender<ClientMsg>>>> =
    OnceLock::new();
static NEXT_HANDLE: AtomicI64 = AtomicI64::new(1);

fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("iolite-net")
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

fn sessions() -> &'static Mutex<HashMap<i64, mpsc::UnboundedSender<ClientMsg>>> {
    SESSIONS.get_or_init(Default::default)
}

fn init_logging() {
    #[cfg(target_os = "android")]
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("iolite"),
    );
}

fn send(handle: jlong, msg: ClientMsg) {
    if let Some(tx) = sessions().lock().unwrap().get(&handle) {
        let _ = tx.send(msg);
    }
}

fn input(handle: jlong, input: Input) {
    send(handle, ClientMsg::Input(input));
}

#[no_mangle]
pub extern "system" fn Java_io_github_redhatontop_iolite_Native_connect<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    host: JString<'local>,
    port: jint,
    data_dir: JString<'local>,
    pinned_host: JString<'local>,
    device_name: JString<'local>,
    width: jint,
    height: jint,
    audio: jboolean,
    listener: JObject<'local>,
) -> jlong {
    init_logging();
    let result = connect(
        &mut env,
        &host,
        port,
        &data_dir,
        &pinned_host,
        &device_name,
        (width, height),
        audio == JNI_TRUE,
        &listener,
    );
    match result {
        Ok(handle) => handle,
        Err(e) => {
            let _ = env.throw_new("java/lang/IllegalStateException", format!("{e:#}"));
            0
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn connect(
    env: &mut JNIEnv,
    host: &JString,
    port: jint,
    data_dir: &JString,
    pinned_host: &JString,
    device_name: &JString,
    (width, height): (jint, jint),
    audio: bool,
    listener: &JObject,
) -> Result<jlong> {
    let host: String = env.get_string(host)?.into();
    let data_dir: String = env.get_string(data_dir)?.into();
    let pinned_host = if pinned_host.is_null() {
        None
    } else {
        Fingerprint::from_hex(&String::from(env.get_string(pinned_host)?))
    };
    let device_name: String = env.get_string(device_name)?.into();
    let identity = Identity::load_or_generate(Path::new(&data_dir), "client", "iolite android")?;
    let vm = env.get_java_vm()?;
    let listener = env.new_global_ref(listener)?;

    let (commands_tx, commands_rx) = mpsc::unbounded_channel();
    let (events_tx, events_rx) = mpsc::channel(32);
    let hello = Hello {
        version: PROTOCOL_VERSION,
        device_name,
        display_width: width.max(0) as u32,
        display_height: height.max(0) as u32,
        audio,
    };
    let port = u16::try_from(port)?;
    runtime().spawn(async move {
        let addr = match tokio::net::lookup_host((host.as_str(), port)).await {
            Ok(mut addrs) => addrs.next(),
            Err(e) => {
                log::warn!("resolving {host}: {e}");
                None
            }
        };
        let Some(addr) = addr else {
            let _ = events_tx
                .send(Event::Closed(format!("cannot resolve {host}")))
                .await;
            return;
        };
        let cfg = Config {
            addr,
            identity,
            pinned_host,
            hello,
        };
        iolite_client::run(cfg, commands_rx, events_tx).await;
    });

    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    sessions().lock().unwrap().insert(handle, commands_tx);
    std::thread::Builder::new()
        .name("iolite-events".into())
        .spawn(move || deliver(vm, listener, events_rx, handle))?;
    Ok(handle)
}

fn deliver(vm: JavaVM, listener: GlobalRef, mut events: mpsc::Receiver<Event>, handle: i64) {
    let mut env = match vm.attach_current_thread() {
        Ok(env) => env,
        Err(e) => {
            log::error!("cannot attach the event thread: {e}");
            return;
        }
    };
    while let Some(event) = events.blocking_recv() {
        let closed = matches!(event, Event::Closed(_));
        if closed {
            sessions().lock().unwrap().remove(&handle);
        }
        let result = env.with_local_frame(8, |env| dispatch(env, listener.as_obj(), event));
        if let Err(e) = result {
            log::error!("listener call failed: {e}");
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
            }
        }
        if closed {
            break;
        }
    }
}

fn dispatch(env: &mut JNIEnv, listener: &JObject, event: Event) -> jni::errors::Result<()> {
    match event {
        Event::PairingCode(code) => {
            let code = env.new_string(code)?;
            env.call_method(
                listener,
                "onPairingCode",
                "(Ljava/lang/String;)V",
                &[(&code).into()],
            )?;
        }
        Event::Welcome { host, welcome } => {
            let fingerprint = env.new_string(host.to_hex())?;
            let host_name = env.new_string(&welcome.host_name)?;
            let opus_head = match &welcome.audio {
                Some(audio) => JObject::from(env.byte_array_from_slice(&audio.opus_head)?),
                None => JObject::null(),
            };
            env.call_method(
                listener,
                "onWelcome",
                "(Ljava/lang/String;Ljava/lang/String;II[B)V",
                &[
                    (&fingerprint).into(),
                    (&host_name).into(),
                    JValue::Int(welcome.width as jint),
                    JValue::Int(welcome.height as jint),
                    (&opus_head).into(),
                ],
            )?;
        }
        Event::Video(packet) => {
            let data = env.byte_array_from_slice(&packet.data)?;
            env.call_method(
                listener,
                "onVideo",
                "([BJZ)V",
                &[
                    (&data).into(),
                    JValue::Long(packet.pts_us as jlong),
                    JValue::Bool(packet.keyframe.into()),
                ],
            )?;
        }
        Event::Audio(packet) => {
            let data = env.byte_array_from_slice(&packet.data)?;
            env.call_method(
                listener,
                "onAudio",
                "([BJ)V",
                &[(&data).into(), JValue::Long(packet.pts_us as jlong)],
            )?;
        }
        Event::Closed(reason) => {
            let reason = env.new_string(reason)?;
            env.call_method(
                listener,
                "onClosed",
                "(Ljava/lang/String;)V",
                &[(&reason).into()],
            )?;
        }
    }
    Ok(())
}

#[no_mangle]
pub extern "system" fn Java_io_github_redhatontop_iolite_Native_fingerprint<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    data_dir: JString<'local>,
) -> JString<'local> {
    let result = (|| -> Result<String> {
        let dir: String = env.get_string(&data_dir)?.into();
        let id = Identity::load_or_generate(Path::new(&dir), "client", "iolite android")?;
        Ok(id.fingerprint().to_hex())
    })();
    match result.and_then(|hex| Ok(env.new_string(hex)?)) {
        Ok(s) => s,
        Err(e) => {
            let _ = env.throw_new("java/lang/IllegalStateException", format!("{e:#}"));
            JString::from(JObject::null())
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_io_github_redhatontop_iolite_Native_touch(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    phase: jint,
    slot: jint,
    x: jfloat,
    y: jfloat,
) {
    let phase = match phase {
        0 => TouchPhase::Down,
        1 => TouchPhase::Move,
        2 => TouchPhase::Up,
        _ => TouchPhase::Cancel,
    };
    input(
        handle,
        Input::Touch {
            phase,
            slot: slot.max(0) as u32,
            x,
            y,
        },
    );
}

#[no_mangle]
pub extern "system" fn Java_io_github_redhatontop_iolite_Native_pointerMove(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    x: jfloat,
    y: jfloat,
) {
    input(handle, Input::PointerMove { x, y });
}

#[no_mangle]
pub extern "system" fn Java_io_github_redhatontop_iolite_Native_pointerRelative(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    dx: jfloat,
    dy: jfloat,
) {
    input(handle, Input::PointerRelative { dx, dy });
}

#[no_mangle]
pub extern "system" fn Java_io_github_redhatontop_iolite_Native_button(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    button: jint,
    down: jboolean,
) {
    let button = match button {
        1 => MouseButton::Middle,
        2 => MouseButton::Right,
        _ => MouseButton::Left,
    };
    input(
        handle,
        Input::Button {
            button,
            down: down == JNI_TRUE,
        },
    );
}

#[no_mangle]
pub extern "system" fn Java_io_github_redhatontop_iolite_Native_scroll(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    dx: jfloat,
    dy: jfloat,
    finish: jboolean,
) {
    input(
        handle,
        Input::Scroll {
            dx,
            dy,
            finish: finish == JNI_TRUE,
        },
    );
}

#[no_mangle]
pub extern "system" fn Java_io_github_redhatontop_iolite_Native_key(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    keysym: jint,
    down: jboolean,
) {
    input(
        handle,
        Input::Key {
            keysym: keysym as u32,
            down: down == JNI_TRUE,
        },
    );
}

#[no_mangle]
pub extern "system" fn Java_io_github_redhatontop_iolite_Native_text<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    text: JString<'local>,
) {
    if let Ok(text) = env.get_string(&text) {
        input(
            handle,
            Input::Text {
                text: text.into(),
            },
        );
    }
}

#[no_mangle]
pub extern "system" fn Java_io_github_redhatontop_iolite_Native_requestKeyframe(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    send(handle, ClientMsg::RequestKeyframe);
}

#[no_mangle]
pub extern "system" fn Java_io_github_redhatontop_iolite_Native_close(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    sessions().lock().unwrap().remove(&handle);
}
