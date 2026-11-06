//! Versão web (WebAssembly): login com Google, sincronização com o Drive,
//! escolha de foto e abertura de links no navegador.
//!
//! A sincronização segue exatamente o mesmo formato das versões Android e computador
//! (pasta "Meus Gastos" no Drive, `gastos.json` mesclado com `model::merge` e as fotos),
//! então todas as versões enxergam os mesmos dados.
//!
//! Login: fluxo OAuth por redirecionamento (a página vai ao Google e volta com o token
//! no endereço). Funciona bem no Safari do iPhone, sem pop-ups. Como não há servidor,
//! o Google entrega só um token de acesso de ~1 hora; depois disso basta tocar em
//! "Conectar com Google" de novo (sem precisar repetir o consentimento).

use crate::model::{self, Data};
use crate::storage;
use crate::AppWindow;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

const CLIENT_ID: &str = env!("GASTOS_WEB_CLIENT_ID");
const SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
const FOLDER_NAME: &str = "Meus Gastos";
const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
const DATA_FILE: &str = "gastos.json";
const TOKEN_KEY: &str = "meusgastos/google_token";
const STATE_KEY: &str = "meusgastos/oauth_state";

/// O app web foi gerado com o cliente OAuth "Aplicativo da Web"?
pub fn configured() -> bool {
    !CLIENT_ID.is_empty()
}

fn window() -> web_sys::Window {
    web_sys::window().expect("sem window")
}

fn local() -> Option<web_sys::Storage> {
    window().local_storage().ok().flatten()
}

// ---------------------------------------------------------------- token

#[derive(serde::Serialize, serde::Deserialize)]
struct Token {
    access_token: String,
    /// ms desde 1970
    expires_at: i64,
}

fn load_token() -> Option<Token> {
    serde_json::from_str(&local()?.get_item(TOKEN_KEY).ok()??).ok()
}

fn clear_token() {
    if let Some(ls) = local() {
        let _ = ls.remove_item(TOKEN_KEY);
    }
}

pub fn connected(_dir: &Path) -> bool {
    load_token().is_some()
}

/// Endereço desta página (para onde o Google devolve o login).
fn redirect_uri() -> String {
    let loc = window().location();
    let origin = loc.origin().unwrap_or_default();
    let path = loc.pathname().unwrap_or_default();
    let path = path.strip_suffix("index.html").unwrap_or(&path).to_string();
    format!("{origin}{path}")
}

/// Vai para a página de login do Google (a página atual é recarregada na volta).
pub fn start_login() -> Result<(), String> {
    if !configured() {
        return Err("Esta versão web foi gerada sem o cliente OAuth do Google (veja o PUBLICAR.md).".into());
    }
    let mut rnd = [0u8; 16];
    getrandom::getrandom(&mut rnd).map_err(|e| e.to_string())?;
    let state: String = rnd.iter().map(|b| format!("{b:02x}")).collect();
    if let Some(ls) = local() {
        let _ = ls.set_item(STATE_KEY, &state);
    }
    let params = web_sys::UrlSearchParams::new().map_err(|_| "erro interno")?;
    for (k, v) in [
        ("client_id", CLIENT_ID),
        ("redirect_uri", redirect_uri().as_str()),
        ("response_type", "token"),
        ("scope", SCOPE),
        ("include_granted_scopes", "true"),
        ("state", state.as_str()),
    ] {
        params.append(k, v);
    }
    let url = format!("https://accounts.google.com/o/oauth2/v2/auth?{}", String::from(params.to_string()));
    window().location().set_href(&url).map_err(|_| "não foi possível abrir o login".to_string())
}

/// Ao abrir a página: se ela voltou do login do Google, guarda o token.
/// Retorna `Some(Ok)` se acabou de conectar, `Some(Err)` em caso de erro.
pub fn handle_login_redirect() -> Option<Result<(), String>> {
    let loc = window().location();
    let hash = loc.hash().ok()?;
    let hash = hash.strip_prefix('#')?;
    let params = web_sys::UrlSearchParams::new_with_str(hash).ok()?;
    if params.get("access_token").is_none() && params.get("error").is_none() {
        return None;
    }
    // Limpa o endereço (o token não deve ficar visível nem no histórico).
    if let Ok(history) = window().history() {
        let clean = format!("{}{}", loc.pathname().unwrap_or_default(), loc.search().unwrap_or_default());
        let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&clean));
    }
    let expected = local().and_then(|ls| ls.get_item(STATE_KEY).ok().flatten());
    if let Some(ls) = local() {
        let _ = ls.remove_item(STATE_KEY);
    }
    if let Some(err) = params.get("error") {
        return Some(Err(if err == "access_denied" {
            "Acesso não autorizado na conta Google.".into()
        } else {
            format!("Erro do Google: {err}")
        }));
    }
    if params.get("state") != expected {
        return Some(Err("Resposta de login inválida. Tente de novo.".into()));
    }
    let token = Token {
        access_token: params.get("access_token")?,
        expires_at: model::now_ms() + params.get("expires_in").and_then(|s| s.parse::<i64>().ok()).unwrap_or(3600) * 1000,
    };
    let saved = local()
        .ok_or("armazenamento indisponível".to_string())
        .and_then(|ls| {
            ls.set_item(TOKEN_KEY, &serde_json::to_string(&token).unwrap())
                .map_err(|_| "não foi possível guardar o login".to_string())
        });
    Some(saved)
}

/// Esquece o login (e revoga o acesso no Google).
pub fn disconnect(_dir: &Path) {
    if let Some(t) = load_token() {
        clear_token();
        let url = format!("https://oauth2.googleapis.com/revoke?token={}", t.access_token);
        wasm_bindgen_futures::spawn_local(async move {
            let _ = fetch("POST", &url, None, None).await;
        });
    }
}

fn access_token() -> Result<String, String> {
    let t = load_token().ok_or("Conecte sua conta Google primeiro.")?;
    if t.expires_at - 60_000 <= model::now_ms() {
        clear_token();
        return Err("A sessão do Google expirou. Toque em \"Conectar com Google\" para continuar.".into());
    }
    Ok(t.access_token)
}

// ---------------------------------------------------------------- HTTP (fetch)

async fn fetch(
    method: &str,
    url: &str,
    token: Option<&str>,
    body: Option<(&[u8], &str)>,
) -> Result<(u16, Vec<u8>), String> {
    let init = web_sys::RequestInit::new();
    init.set_method(method);
    init.set_mode(web_sys::RequestMode::Cors);
    let headers = web_sys::Headers::new().map_err(|_| "erro interno")?;
    if let Some(t) = token {
        let _ = headers.set("Authorization", &format!("Bearer {t}"));
    }
    if let Some((bytes, ctype)) = body {
        let _ = headers.set("Content-Type", ctype);
        init.set_body(&js_sys::Uint8Array::from(bytes));
    }
    init.set_headers(&headers);
    let req = web_sys::Request::new_with_str_and_init(url, &init).map_err(|_| "pedido inválido")?;
    let resp: web_sys::Response = JsFuture::from(window().fetch_with_request(&req))
        .await
        .map_err(|_| "Sem conexão com a internet.".to_string())?
        .unchecked_into();
    let buf = JsFuture::from(resp.array_buffer().map_err(|_| "resposta inválida")?)
        .await
        .map_err(|_| "resposta inválida")?;
    Ok((resp.status(), js_sys::Uint8Array::new(&buf).to_vec()))
}

/// Chamada à API do Drive; erros viram mensagens legíveis.
async fn api(method: &str, url: &str, tok: &str, body: Option<(&[u8], &str)>) -> Result<Vec<u8>, String> {
    let (status, bytes) = fetch(method, url, Some(tok), body).await?;
    if status == 401 {
        clear_token();
        return Err("A sessão do Google expirou. Toque em \"Conectar com Google\" para continuar.".into());
    }
    if !(200..300).contains(&status) {
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
        let msg = v.pointer("/error/message").and_then(|m| m.as_str()).unwrap_or("erro desconhecido");
        return Err(format!("Erro do Google ({status}): {msg}"));
    }
    Ok(bytes)
}

fn enc(s: &str) -> String {
    String::from(js_sys::encode_uri_component(s))
}

struct DriveFile {
    id: String,
    name: String,
}

async fn list_files(tok: &str, q: &str) -> Result<Vec<DriveFile>, String> {
    let mut out = Vec::new();
    let mut page: Option<String> = None;
    loop {
        let mut url = format!(
            "https://www.googleapis.com/drive/v3/files?q={}&spaces=drive&fields={}&pageSize=1000",
            enc(q),
            enc("nextPageToken,files(id,name)")
        );
        if let Some(p) = &page {
            url += &format!("&pageToken={}", enc(p));
        }
        let v: serde_json::Value = serde_json::from_slice(&api("GET", &url, tok, None).await?).map_err(|e| e.to_string())?;
        for f in v["files"].as_array().cloned().unwrap_or_default() {
            out.push(DriveFile {
                id: f["id"].as_str().unwrap_or_default().to_string(),
                name: f["name"].as_str().unwrap_or_default().to_string(),
            });
        }
        match v["nextPageToken"].as_str() {
            Some(p) => page = Some(p.to_string()),
            None => return Ok(out),
        }
    }
}

async fn app_folder(tok: &str) -> Result<String, String> {
    let q = format!("name = '{FOLDER_NAME}' and mimeType = '{FOLDER_MIME}' and trashed = false");
    if let Some(f) = list_files(tok, &q).await?.into_iter().next() {
        return Ok(f.id);
    }
    let meta = serde_json::json!({ "name": FOLDER_NAME, "mimeType": FOLDER_MIME }).to_string();
    let v: serde_json::Value = serde_json::from_slice(
        &api("POST", "https://www.googleapis.com/drive/v3/files", tok, Some((meta.as_bytes(), "application/json"))).await?,
    )
    .map_err(|e| e.to_string())?;
    v["id"].as_str().map(String::from).ok_or_else(|| "O Google não devolveu o id da pasta.".to_string())
}

async fn download(tok: &str, id: &str) -> Result<Vec<u8>, String> {
    api("GET", &format!("https://www.googleapis.com/drive/v3/files/{id}?alt=media"), tok, None).await
}

async fn upload_new(tok: &str, folder: &str, name: &str, mime: &str, bytes: &[u8]) -> Result<(), String> {
    let boundary = format!("gastos-{}", model::new_id());
    let meta = serde_json::json!({ "name": name, "parents": [folder] }).to_string();
    let mut body = Vec::with_capacity(bytes.len() + 512);
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{meta}\r\n--{boundary}\r\nContent-Type: {mime}\r\n\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    api(
        "POST",
        "https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart",
        tok,
        Some((&body, &format!("multipart/related; boundary={boundary}"))),
    )
    .await?;
    Ok(())
}

async fn upload_update(tok: &str, id: &str, mime: &str, bytes: &[u8]) -> Result<(), String> {
    api(
        "PATCH",
        &format!("https://www.googleapis.com/upload/drive/v3/files/{id}?uploadType=media"),
        tok,
        Some((bytes, mime)),
    )
    .await?;
    Ok(())
}

/// Mesma regra das outras versões: nomes de foto vindos da nuvem só com caracteres seguros.
fn safe_photo_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() < 100
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        && name != DATA_FILE
}

pub struct SyncResult {
    pub data: Data,
    pub photos_up: usize,
    pub photos_down: usize,
}

/// Mescla os dados locais com os da nuvem, envia o resultado e sincroniza as fotos.
pub async fn sync(dir: PathBuf, local: Data) -> Result<SyncResult, String> {
    let tok = access_token()?;
    let folder = app_folder(&tok).await?;
    let files = list_files(&tok, &format!("'{folder}' in parents and trashed = false")).await?;
    let find = |name: &str| files.iter().find(|f| f.name == name);

    let remote: Option<Data> = match find(DATA_FILE) {
        Some(f) => Some(
            serde_json::from_slice(&download(&tok, &f.id).await?)
                .map_err(|_| "O arquivo na nuvem está em um formato desconhecido. Nada foi alterado.".to_string())?,
        ),
        None => None,
    };
    let merged = match &remote {
        Some(r) => model::merge(&local, r),
        None => local,
    };
    if remote.as_ref() != Some(&merged) {
        let bytes = serde_json::to_vec(&merged).map_err(|e| e.to_string())?;
        match find(DATA_FILE) {
            Some(f) => upload_update(&tok, &f.id, "application/json", &bytes).await?,
            None => upload_new(&tok, &folder, DATA_FILE, "application/json", &bytes).await?,
        }
    }

    let referenced: BTreeSet<String> = merged.expenses.iter().filter_map(|e| e.photo.clone()).collect();
    let (mut up, mut down) = (0, 0);
    for name in referenced {
        if !safe_photo_name(&name) {
            continue;
        }
        match (storage::has_photo(&dir, &name), find(&name)) {
            (true, None) => {
                let bytes = storage::read_photo(&dir, &name).ok_or("foto local ilegível")?;
                upload_new(&tok, &folder, &name, "image/jpeg", &bytes).await?;
                up += 1;
            }
            (false, Some(f)) => {
                let bytes = download(&tok, &f.id).await?;
                storage::write_photo(&dir, &name, &bytes)?;
                down += 1;
            }
            _ => {}
        }
    }
    Ok(SyncResult { data: merged, photos_up: up, photos_down: down })
}

// ---------------------------------------------------------------- foto e links

/// Abre o seletor de fotos do navegador (no iPhone: câmera, fototeca ou arquivos).
pub fn pick_photo(w: slint::Weak<AppWindow>, dir: PathBuf, name: String) {
    let doc = window().document().expect("sem document");
    let Ok(input) = doc.create_element("input") else { return };
    let input: web_sys::HtmlInputElement = input.unchecked_into();
    input.set_type("file");
    input.set_accept("image/*");
    let _ = input.style().set_property("display", "none");
    if let Some(body) = doc.body() {
        let _ = body.append_child(&input);
    }
    let inp = input.clone();
    let onchange = Closure::once_into_js(move |_e: web_sys::Event| {
        let file = inp.files().and_then(|f| f.get(0));
        inp.remove();
        let Some(file) = file else { return };
        if let Some(ui) = w.upgrade() {
            ui.set_busy(true);
        }
        wasm_bindgen_futures::spawn_local(async move {
            let result = match JsFuture::from(file.array_buffer()).await {
                Ok(buf) => model::import_photo(&js_sys::Uint8Array::new(&buf).to_vec(), &dir, &name),
                Err(_) => Err("não foi possível ler o arquivo".to_string()),
            };
            if let Some(ui) = w.upgrade() {
                match result {
                    Ok(()) => ui.invoke_photo_imported(name.into()),
                    Err(e) => ui.invoke_photo_failed(format!("Não foi possível abrir a foto: {e}").into()),
                }
            }
        });
    });
    input.set_onchange(Some(onchange.unchecked_ref()));
    input.click();
}

pub fn open_url(url: &str) {
    let _ = window().open_with_url_and_target(url, "_blank");
}

/// Faz a janela do app ocupar a área útil da página e acompanhar mudanças de tamanho
/// (girar o aparelho, teclado, redimensionar o navegador).
pub fn fit_to_page(w: slint::Weak<AppWindow>) {
    use slint::ComponentHandle;
    let fit = move || {
        let Some(ui) = w.upgrade() else { return };
        let Some(area) = window().document().and_then(|d| d.get_element_by_id("area")) else { return };
        let (width, height) = (area.client_width(), area.client_height());
        if width > 0 && height > 0 {
            ui.window().set_size(slint::LogicalSize::new(width as f32, height as f32));
        }
    };
    fit();
    let cb = Closure::<dyn Fn()>::new(fit);
    let _ = window().add_event_listener_with_callback("resize", cb.as_ref().unchecked_ref());
    cb.forget();
}
