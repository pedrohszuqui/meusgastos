//! Armazenamento dos dados e das fotos.
//!
//! - Android e computador: arquivos na pasta de dados do app (`gastos.json` e `fotos/`).
//! - Navegador (WebAssembly): `localStorage` para os textos e IndexedDB para as fotos.
//!   As fotos ficam também num cache em memória, carregado ao abrir o app, para que a
//!   interface possa lê-las de forma síncrona, como nas outras versões.

use std::path::Path;

// ------------------------------------------------------------------ nativo (arquivos)

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use std::fs;
    use std::path::Path;

    pub fn read_text(dir: &Path, name: &str) -> Option<String> {
        fs::read_to_string(dir.join(name)).ok()
    }

    pub fn write_text(dir: &Path, name: &str, text: &str) -> std::io::Result<()> {
        fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("{name}.tmp"));
        fs::write(&tmp, text)?;
        fs::rename(&tmp, dir.join(name))
    }

    pub fn read_photo(dir: &Path, name: &str) -> Option<Vec<u8>> {
        fs::read(dir.join("fotos").join(name)).ok()
    }

    pub fn write_photo(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
        let photos = dir.join("fotos");
        fs::create_dir_all(&photos).map_err(|e| e.to_string())?;
        let tmp = photos.join(format!("{name}.tmp"));
        fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
        fs::rename(&tmp, photos.join(name)).map_err(|e| e.to_string())
    }

    pub fn delete_photo(dir: &Path, name: &str) {
        let _ = fs::remove_file(dir.join("fotos").join(name));
    }

    pub fn has_photo(dir: &Path, name: &str) -> bool {
        dir.join("fotos").join(name).exists()
    }
}

// ------------------------------------------------------------------ navegador

#[cfg(target_arch = "wasm32")]
mod imp {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::path::Path;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use web_sys::{IdbDatabase, IdbRequest, IdbTransactionMode};

    const PREFIX: &str = "meusgastos/";
    const DB_NAME: &str = "meusgastos";
    const STORE: &str = "fotos";

    thread_local! {
        static PHOTOS: RefCell<HashMap<String, Vec<u8>>> = RefCell::new(HashMap::new());
        static DB: RefCell<Option<IdbDatabase>> = const { RefCell::new(None) };
    }

    fn local_storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok().flatten()
    }

    pub fn read_text(_dir: &Path, name: &str) -> Option<String> {
        local_storage()?.get_item(&format!("{PREFIX}{name}")).ok().flatten()
    }

    pub fn write_text(_dir: &Path, name: &str, text: &str) -> std::io::Result<()> {
        let ls = local_storage().ok_or_else(|| std::io::Error::other("armazenamento do navegador indisponível"))?;
        ls.set_item(&format!("{PREFIX}{name}"), text)
            .map_err(|_| std::io::Error::other("sem espaço no armazenamento do navegador"))
    }

    pub fn read_photo(_dir: &Path, name: &str) -> Option<Vec<u8>> {
        PHOTOS.with(|p| p.borrow().get(name).cloned())
    }

    pub fn has_photo(_dir: &Path, name: &str) -> bool {
        PHOTOS.with(|p| p.borrow().contains_key(name))
    }

    pub fn write_photo(_dir: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
        PHOTOS.with(|p| p.borrow_mut().insert(name.to_string(), bytes.to_vec()));
        DB.with(|db| {
            if let Some(db) = &*db.borrow() {
                let tx = db
                    .transaction_with_str_and_mode(STORE, IdbTransactionMode::Readwrite)
                    .map_err(|_| "IndexedDB indisponível".to_string())?;
                let store = tx.object_store(STORE).map_err(|_| "IndexedDB indisponível".to_string())?;
                store
                    .put_with_key(&js_sys::Uint8Array::from(bytes), &JsValue::from_str(name))
                    .map_err(|_| "não foi possível gravar a foto".to_string())?;
            }
            Ok(())
        })
    }

    pub fn delete_photo(_dir: &Path, name: &str) {
        PHOTOS.with(|p| p.borrow_mut().remove(name));
        DB.with(|db| {
            if let Some(db) = &*db.borrow() {
                if let Ok(tx) = db.transaction_with_str_and_mode(STORE, IdbTransactionMode::Readwrite) {
                    if let Ok(store) = tx.object_store(STORE) {
                        let _ = store.delete(&JsValue::from_str(name));
                    }
                }
            }
        });
    }

    /// Converte um IDBRequest numa Promise (resolve com `request.result`).
    fn request_promise(req: &IdbRequest) -> js_sys::Promise {
        let req = req.clone();
        js_sys::Promise::new(&mut |resolve: js_sys::Function, reject: js_sys::Function| {
            let r = req.clone();
            let ok = Closure::once_into_js(move |_e: web_sys::Event| {
                let _ = resolve.call1(&JsValue::NULL, &r.result().unwrap_or(JsValue::UNDEFINED));
            });
            req.set_onsuccess(Some(ok.unchecked_ref()));
            let err = Closure::once_into_js(move |_e: web_sys::Event| {
                let _ = reject.call0(&JsValue::NULL);
            });
            req.set_onerror(Some(err.unchecked_ref()));
        })
    }

    /// Abre o IndexedDB e carrega as fotos para o cache em memória.
    pub async fn init() -> Result<(), JsValue> {
        let factory = web_sys::window()
            .and_then(|w| w.indexed_db().ok().flatten())
            .ok_or_else(|| JsValue::from_str("IndexedDB indisponível"))?;
        let open = factory.open_with_u32(DB_NAME, 1)?;
        let upgrade = Closure::once_into_js(move |e: web_sys::Event| {
            let req: IdbRequest = e.target().unwrap().unchecked_into();
            let db: IdbDatabase = req.result().unwrap().unchecked_into();
            if !db.object_store_names().contains(STORE) {
                let _ = db.create_object_store(STORE);
            }
        });
        open.set_onupgradeneeded(Some(upgrade.unchecked_ref()));
        let db: IdbDatabase = wasm_bindgen_futures::JsFuture::from(request_promise(&open)).await?.unchecked_into();

        let tx = db.transaction_with_str(STORE)?;
        let store = tx.object_store(STORE)?;
        let keys = wasm_bindgen_futures::JsFuture::from(request_promise(&store.get_all_keys()?)).await?;
        let values = wasm_bindgen_futures::JsFuture::from(request_promise(&store.get_all()?)).await?;
        let keys: js_sys::Array = keys.unchecked_into();
        let values: js_sys::Array = values.unchecked_into();
        PHOTOS.with(|p| {
            let mut p = p.borrow_mut();
            for i in 0..keys.length() {
                if let Some(name) = keys.get(i).as_string() {
                    p.insert(name, js_sys::Uint8Array::new(&values.get(i)).to_vec());
                }
            }
        });
        DB.with(|d| *d.borrow_mut() = Some(db));
        Ok(())
    }
}

#[cfg(target_arch = "wasm32")]
pub use imp::init;

/// Lê um arquivo de texto de dados (ex.: `gastos.json`).
pub fn read_text(dir: &Path, name: &str) -> Option<String> {
    imp::read_text(dir, name)
}

/// Grava um arquivo de texto de dados (no Android/PC de forma atômica).
pub fn write_text(dir: &Path, name: &str, text: &str) -> std::io::Result<()> {
    imp::write_text(dir, name, text)
}

pub fn read_photo(dir: &Path, name: &str) -> Option<Vec<u8>> {
    imp::read_photo(dir, name)
}

pub fn write_photo(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    imp::write_photo(dir, name, bytes)
}

pub fn delete_photo(dir: &Path, name: &str) {
    imp::delete_photo(dir, name)
}

pub fn has_photo(dir: &Path, name: &str) -> bool {
    imp::has_photo(dir, name)
}
