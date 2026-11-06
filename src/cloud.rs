//! Liga a sincronização (`sync.rs`) à interface: login por código, sincronização
//! automática (ao abrir, alguns segundos após cada alteração e a cada 5 minutos)
//! e sincronização manual.

use crate::{model, refresh, AppWindow, State};
#[cfg(not(target_arch = "wasm32"))]
use crate::sync;
#[cfg(not(target_arch = "wasm32"))]
use crate::sync as backend;
#[cfg(target_arch = "wasm32")]
use crate::web as backend;
use chrono::Local;
use slint::{ComponentHandle, Timer, TimerMode};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::AtomicU64;
use std::time::Duration;

thread_local! {
    static APP: RefCell<Option<(Rc<RefCell<State>>, slint::Weak<AppWindow>)>> = const { RefCell::new(None) };
    static DEBOUNCE: Timer = Timer::default();
    static PERIODIC: Timer = Timer::default();
}

static RUNNING: AtomicBool = AtomicBool::new(false);
static PENDING: AtomicBool = AtomicBool::new(false);
/// Incrementado para cancelar um login em andamento.
#[cfg(not(target_arch = "wasm32"))]
static LOGIN_GEN: AtomicU64 = AtomicU64::new(0);

fn with_app<R>(f: impl FnOnce(&Rc<RefCell<State>>, &AppWindow) -> R) -> Option<R> {
    APP.with(|a| {
        let a = a.borrow();
        let (state, weak) = a.as_ref()?;
        let ui = weak.upgrade()?;
        Some(f(state, &ui))
    })
}

fn data_dir(state: &Rc<RefCell<State>>) -> PathBuf {
    state.borrow().store.dir().to_path_buf()
}

pub fn install(ui: &AppWindow, state: Rc<RefCell<State>>) {
    let dir = data_dir(&state);
    APP.with(|a| *a.borrow_mut() = Some((state.clone(), ui.as_weak())));
    ui.set_sync_configured(backend::configured());
    #[cfg(not(target_arch = "wasm32"))]
    {
        ui.set_sync_has_direct(sync::has_loopback());
        ui.set_sync_has_code(sync::has_device());
    }
    #[cfg(target_arch = "wasm32")]
    {
        // No navegador o login é sempre pelo redirecionamento do Google.
        ui.set_sync_has_direct(backend::configured());
        ui.set_sync_has_code(false);
    }
    ui.set_sync_connected(backend::connected(&dir));

    ui.on_sync_now(|| run(true));

    #[cfg(not(target_arch = "wasm32"))]
    {
        let s = state.clone();
        let w = ui.as_weak();
        ui.on_sync_connect(move || start_login(w.clone(), data_dir(&s)));

        let s = state.clone();
        let w = ui.as_weak();
        ui.on_sync_connect_code(move || start_code_login(w.clone(), data_dir(&s)));
    }
    #[cfg(target_arch = "wasm32")]
    {
        let w = ui.as_weak();
        ui.on_sync_connect(move || {
            if let Err(e) = backend::start_login() {
                if let Some(ui) = w.upgrade() {
                    ui.set_sync_error(e.into());
                }
            }
        });
    }

    let w = ui.as_weak();
    ui.on_sync_cancel(move || {
        #[cfg(not(target_arch = "wasm32"))]
        LOGIN_GEN.fetch_add(1, SeqCst);
        if let Some(ui) = w.upgrade() {
            ui.set_sync_pending(0);
            ui.set_sync_code("".into());
        }
    });

    let s = state.clone();
    let w = ui.as_weak();
    ui.on_sync_disconnect(move || {
        #[cfg(not(target_arch = "wasm32"))]
        LOGIN_GEN.fetch_add(1, SeqCst);
        backend::disconnect(&data_dir(&s));
        if let Some(ui) = w.upgrade() {
            ui.set_sync_connected(false);
            ui.set_sync_status("".into());
            ui.set_sync_error("".into());
        }
    });

    ui.on_open_url(|url| crate::open_url(&url));

    PERIODIC.with(|t| t.start(TimerMode::Repeated, Duration::from_secs(300), || run(false)));
    Timer::single_shot(Duration::from_millis(1500), || run(false));
}

/// Chamado após cada alteração salva: agenda uma sincronização em 3 s.
pub fn changed() {
    DEBOUNCE.with(|t| t.start(TimerMode::SingleShot, Duration::from_secs(3), || run(false)));
}

/// Inicia uma sincronização (se conectado e se não houver outra em andamento).
pub fn run(manual: bool) {
    with_app(|state, ui| {
        let st = state.borrow();
        let dir = st.store.dir().to_path_buf();
        if !backend::connected(&dir) {
            ui.set_sync_connected(false);
            if manual {
                ui.set_sync_error("Conecte sua conta Google para sincronizar.".into());
            }
            return;
        }
        if RUNNING.swap(true, SeqCst) {
            PENDING.store(true, SeqCst);
            return;
        }
        ui.set_sync_busy(true);
        if manual {
            ui.set_sync_error("".into());
        }
        let snapshot = st.store.data.clone();
        let rev = st.revision.get();
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::spawn(move || {
            let result = sync::sync(&dir, snapshot);
            let _ = slint::invoke_from_event_loop(move || finish(result, rev));
        });
        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(async move {
            let result = backend::sync(dir, snapshot).await;
            finish(result, rev);
        });
    });
}

fn finish(result: Result<backend::SyncResult, String>, rev: u64) {
    let again = with_app(|state, ui| {
        RUNNING.store(false, SeqCst);
        ui.set_sync_busy(false);
        let mut st = state.borrow_mut();
        match result {
            Ok(r) => {
                // Mescla de novo: o usuário pode ter alterado algo durante a sincronização.
                st.store.data = model::merge(&st.store.data, &r.data);
                let _ = st.store.save();
                let mut status = format!("Sincronizado às {}", Local::now().format("%H:%M"));
                if r.photos_up + r.photos_down > 0 {
                    status += &format!(" · fotos: {} enviada(s), {} recebida(s)", r.photos_up, r.photos_down);
                }
                ui.set_sync_status(status.into());
                ui.set_sync_error("".into());
                ui.set_sync_connected(true);
                refresh(ui, &st);
                st.revision.get() != rev
            }
            Err(e) => {
                ui.set_sync_error(e.into());
                ui.set_sync_connected(backend::connected(st.store.dir()));
                false
            }
        }
    })
    .unwrap_or(false);
    if again || PENDING.swap(false, SeqCst) {
        changed();
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn start_login(w: slint::Weak<AppWindow>, dir: PathBuf) {
    let gen = LOGIN_GEN.fetch_add(1, SeqCst) + 1;
    let Some(ui) = w.upgrade() else { return };
    let pending = match sync::begin_login() {
        Ok(p) => p,
        Err(e) => {
            ui.set_sync_error(e.into());
            return;
        }
    };
    ui.set_sync_pending(1);
    ui.set_sync_error("".into());
    ui.set_sync_url(pending.url.clone().into());
    crate::open_url(&pending.url);

    std::thread::spawn(move || {
        let result = sync::finish_login(&dir, pending, || LOGIN_GEN.load(SeqCst) != gen);
        let _ = w.upgrade_in_event_loop(move |ui| {
            if LOGIN_GEN.load(SeqCst) != gen {
                return; // cancelado
            }
            ui.set_sync_pending(0);
            match result {
                Ok(()) => {
                    ui.set_sync_connected(true);
                    ui.set_sync_status("Conectado. Sincronizando...".into());
                    run(true);
                }
                Err(e) => ui.set_sync_error(e.into()),
            }
        });
    });
}

/// Login por código (cliente "TV"): mostra o código e espera a autorização.
#[cfg(not(target_arch = "wasm32"))]
fn start_code_login(w: slint::Weak<AppWindow>, dir: PathBuf) {
    let gen = LOGIN_GEN.fetch_add(1, SeqCst) + 1;
    if let Some(ui) = w.upgrade() {
        ui.set_sync_pending(2);
        ui.set_sync_code("".into());
        ui.set_sync_url("".into());
        ui.set_sync_error("".into());
    }
    std::thread::spawn(move || {
        let post = |f: Box<dyn FnOnce(AppWindow) + Send>| {
            let _ = w.upgrade_in_event_loop(move |ui| {
                if LOGIN_GEN.load(SeqCst) == gen {
                    f(ui)
                }
            });
        };
        let dc = match sync::start_device_flow() {
            Ok(dc) => dc,
            Err(e) => {
                post(Box::new(move |ui| {
                    ui.set_sync_pending(0);
                    ui.set_sync_error(e.into());
                }));
                return;
            }
        };
        let (code, url) = (dc.user_code.clone(), dc.verification_url.clone());
        post(Box::new(move |ui| {
            ui.set_sync_code(code.into());
            ui.set_sync_url(url.into());
        }));

        let deadline = std::time::Instant::now() + Duration::from_secs(dc.expires_in);
        let mut interval = dc.interval.max(1);
        loop {
            for _ in 0..interval {
                std::thread::sleep(Duration::from_secs(1));
                if LOGIN_GEN.load(SeqCst) != gen {
                    return; // cancelado
                }
            }
            if std::time::Instant::now() > deadline {
                post(Box::new(|ui| {
                    ui.set_sync_pending(0);
                    ui.set_sync_error("O código expirou. Tente de novo.".into());
                }));
                return;
            }
            match sync::poll(&dir, &dc) {
                sync::Poll::Pending => {}
                sync::Poll::SlowDown => interval += 5,
                sync::Poll::Done => {
                    post(Box::new(|ui| {
                        ui.set_sync_pending(0);
                        ui.set_sync_code("".into());
                        ui.set_sync_connected(true);
                        ui.set_sync_status("Conectado. Sincronizando...".into());
                        run(true);
                    }));
                    return;
                }
                sync::Poll::Failed(e) => {
                    post(Box::new(move |ui| {
                        ui.set_sync_pending(0);
                        ui.set_sync_error(e.into());
                    }));
                    return;
                }
            }
        }
    });
}
/// Versão web: resultado do retorno da página de login do Google.
#[cfg(target_arch = "wasm32")]
pub fn login_result(ui: &AppWindow, result: Result<(), String>) {
    match result {
        Ok(()) => {
            ui.set_sync_connected(true);
            ui.set_sync_status("Conectado. Sincronizando...".into());
        }
        Err(e) => ui.set_sync_error(e.into()),
    }
}
