//! Sincronização com o Google Drive.
//!
//! - Vários clientes OAuth (um por tipo/plataforma, escolhidos no build.rs):
//!   * login direto (cliente "App para computador"): o app abre o navegador e recebe a
//!     autorização em http://127.0.0.1 (fluxo "loopback" com PKCE);
//!   * login por código (cliente "TV"): alternativa, digita-se um código em google.com/device.
//!   O token guarda com qual cliente foi obtido, para ser renovado com o mesmo.
//! - Os dados ficam na pasta `Meus Gastos` do Drive, com o escopo `drive.file`:
//!   o app só enxerga os arquivos que ele mesmo criou (não vê o resto do Drive).
//! - A cópia local e a da nuvem são mescladas com `model::merge`, então é possível
//!   usar o celular e o computador ao mesmo tempo.

use crate::model::{self, Data};
use crate::storage;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Cliente OAuth embutido no app (definido em tempo de compilação pelo build.rs).
pub struct Client {
    id: &'static str,
    secret: &'static str,
}

/// Login direto (cliente "App para computador"; no Android pode ser um cliente dedicado).
const LOOPBACK: Client = Client { id: env!("GASTOS_LOOPBACK_CLIENT_ID"), secret: env!("GASTOS_LOOPBACK_CLIENT_SECRET") };
/// Login por código (cliente "TVs e dispositivos de entrada limitada").
const DEVICE: Client = Client { id: env!("GASTOS_DEVICE_CLIENT_ID"), secret: env!("GASTOS_DEVICE_CLIENT_SECRET") };

/// Forma de login (e, portanto, qual cliente OAuth) usada para obter um token.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Method {
    #[default]
    Loopback,
    Device,
}

impl Method {
    fn client(self) -> &'static Client {
        match self {
            Method::Loopback => &LOOPBACK,
            Method::Device => &DEVICE,
        }
    }
}

/// Login direto disponível neste build?
pub fn has_loopback() -> bool {
    !LOOPBACK.id.is_empty()
}

/// Login por código disponível neste build?
pub fn has_device() -> bool {
    !DEVICE.id.is_empty()
}
const SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
const FOLDER_NAME: &str = "Meus Gastos";
const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const DATA_FILE: &str = "gastos.json";

/// O app foi compilado com pelo menos uma credencial do Google?
pub fn configured() -> bool {
    has_loopback() || has_device()
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .build()
}

fn http_err(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let msg = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| {
                    v.pointer("/error/message")
                        .or_else(|| v.get("error_description"))
                        .or_else(|| v.get("error"))
                        .and_then(|m| m.as_str().map(String::from))
                })
                .unwrap_or_else(|| body.chars().take(160).collect());
            format!("Erro do Google ({code}): {msg}")
        }
        ureq::Error::Transport(t) => format!("Sem conexão com a internet ({})", t.kind()),
    }
}

// ---------------------------------------------------------------- token

#[derive(Serialize, Deserialize, Clone)]
struct Token {
    refresh_token: String,
    access_token: String,
    /// ms desde 1970
    expires_at: i64,
    /// Cliente com que o token foi obtido (a renovação precisa usar o mesmo).
    #[serde(default)]
    method: Method,
}

fn token_path(dir: &Path) -> PathBuf {
    dir.join("google_token.json")
}

fn load_token(dir: &Path) -> Option<Token> {
    serde_json::from_slice(&std::fs::read(token_path(dir)).ok()?).ok()
}

fn save_token(dir: &Path, t: &Token) -> Result<(), String> {
    std::fs::write(token_path(dir), serde_json::to_vec(t).unwrap()).map_err(|e| e.to_string())
}

pub fn connected(dir: &Path) -> bool {
    load_token(dir).is_some()
}

/// Esquece a conexão (e tenta revogar o acesso no Google).
pub fn disconnect(dir: &Path) {
    if let Some(t) = load_token(dir) {
        let _ = std::fs::remove_file(token_path(dir));
        std::thread::spawn(move || {
            let _ = agent()
                .post("https://oauth2.googleapis.com/revoke")
                .send_form(&[("token", t.refresh_token.as_str())]);
        });
    }
}

fn access_token(dir: &Path) -> Result<String, String> {
    let mut t = load_token(dir).ok_or("Conecte sua conta Google primeiro.")?;
    if t.expires_at - 60_000 > model::now_ms() {
        return Ok(t.access_token);
    }
    let client = t.method.client();
    if client.id.is_empty() {
        let _ = std::fs::remove_file(token_path(dir));
        return Err("Esta versão do app não tem o cliente usado no login anterior. Conecte novamente.".into());
    }
    let resp = agent().post(TOKEN_URL).send_form(&[
        ("client_id", client.id),
        ("client_secret", client.secret),
        ("refresh_token", t.refresh_token.as_str()),
        ("grant_type", "refresh_token"),
    ]);
    match resp {
        Ok(r) => {
            let v: serde_json::Value = r.into_json().map_err(|e| e.to_string())?;
            t.access_token = v["access_token"].as_str().ok_or("Resposta inválida do Google")?.to_string();
            t.expires_at = model::now_ms() + v["expires_in"].as_i64().unwrap_or(3600) * 1000;
            save_token(dir, &t)?;
            Ok(t.access_token)
        }
        Err(ureq::Error::Status(400 | 401, _)) => {
            // Acesso revogado ou expirado: é preciso conectar de novo.
            let _ = std::fs::remove_file(token_path(dir));
            Err("A conexão com o Google expirou. Conecte novamente.".into())
        }
        Err(e) => Err(http_err(e)),
    }
}

// ---------------------------------------------------------------- login (navegador + PKCE)
//
// Fluxo "loopback" do Google para apps instalados: o app abre a página de login no
// navegador; depois que a pessoa escolhe a conta e toca em "Permitir", o Google
// redireciona para http://127.0.0.1:<porta>, onde o próprio app está escutando.
// O PKCE impede que outro programa use o código de autorização.

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

fn random_urlsafe(bytes: usize) -> Result<String, String> {
    use base64::Engine;
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf).map_err(|e| e.to_string())?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf))
}

fn pkce_challenge(verifier: &str) -> String {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub struct PendingLogin {
    listener: std::net::TcpListener,
    redirect_uri: String,
    verifier: String,
    state: String,
    /// Endereço da página de login do Google (abrir no navegador).
    pub url: String,
}

/// Prepara o login: abre a porta local e monta o endereço da página do Google.
pub fn begin_login() -> Result<PendingLogin, String> {
    if !has_loopback() {
        return Err("Este app foi compilado sem o cliente de login direto (veja o PUBLICAR.md).".into());
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| format!("Não foi possível abrir a porta local: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}");
    let verifier = random_urlsafe(48)?;
    let state = random_urlsafe(16)?;
    let url = url::Url::parse_with_params(
        AUTH_URL,
        &[
            ("client_id", LOOPBACK.id),
            ("redirect_uri", redirect_uri.as_str()),
            ("response_type", "code"),
            ("scope", SCOPE),
            ("code_challenge", pkce_challenge(&verifier).as_str()),
            ("code_challenge_method", "S256"),
            ("state", state.as_str()),
            ("access_type", "offline"),
            ("prompt", "consent select_account"),
        ],
    )
    .map_err(|e| e.to_string())?
    .to_string();
    Ok(PendingLogin { listener, redirect_uri, verifier, state, url })
}

fn page(title: &str, text: &str) -> String {
    // No Android, o botão traz o app de volta para a frente (como tocar no ícone).
    let back = if cfg!(target_os = "android") {
        r#"<p><a class="b" href="intent:#Intent;action=android.intent.action.MAIN;category=android.intent.category.LAUNCHER;package=br.com.meusgastos;end">Voltar ao Meus Gastos</a></p>"#
    } else {
        "<p>Você já pode fechar esta aba e voltar ao app.</p>"
    };
    format!(
        "<!doctype html><html lang=pt-BR><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'>\
         <title>Meus Gastos</title><style>body{{font-family:system-ui,sans-serif;background:#f2f5f1;color:#1d241d;\
         display:flex;min-height:90vh;align-items:center;justify-content:center;text-align:center}}\
         div{{background:#fff;border-radius:18px;padding:32px;max-width:420px;box-shadow:0 3px 12px #0002}}\
         h1{{color:#2e7d32;font-size:22px}}.b{{display:inline-block;background:#2e7d32;color:#fff;padding:14px 22px;\
         border-radius:26px;text-decoration:none;font-weight:600}}</style><div><h1>{title}</h1><p>{text}</p>{back}</div></html>"
    )
}

fn respond(stream: &mut std::net::TcpStream, status: &str, body: &str) {
    use std::io::Write;
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.flush();
}

/// Espera o Google redirecionar de volta e troca o código pelo token.
/// `cancelled` é consultado periodicamente para permitir cancelar o login.
pub fn finish_login(dir: &Path, p: PendingLogin, cancelled: impl Fn() -> bool) -> Result<(), String> {
    p.listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + LOGIN_TIMEOUT;
    loop {
        if cancelled() {
            return Err(String::new());
        }
        if std::time::Instant::now() > deadline {
            return Err("O login demorou demais. Toque em Conectar para tentar de novo.".into());
        }
        let mut stream = match p.listener.accept() {
            Ok((s, _)) => s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
            Err(e) => return Err(e.to_string()),
        };
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
        // Lê só a primeira linha: "GET /?state=...&code=... HTTP/1.1"
        let mut buf = [0u8; 8192];
        let n = stream.read(&mut buf).unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]);
        let path = req.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("");
        let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
        let params: std::collections::HashMap<String, String> = url::form_urlencoded::parse(query.as_bytes()).into_owned().collect();

        if params.get("state").map(String::as_str) != Some(p.state.as_str()) {
            // favicon, pré-carregamento do navegador etc.: ignora e continua esperando.
            respond(&mut stream, "404 Not Found", "");
            continue;
        }
        if let Some(err) = params.get("error") {
            respond(&mut stream, "200 OK", &page("Login cancelado", "O acesso não foi autorizado."));
            return Err(if err == "access_denied" {
                "Acesso não autorizado na conta Google.".into()
            } else {
                format!("Erro do Google: {err}")
            });
        }
        let Some(code) = params.get("code") else {
            respond(&mut stream, "400 Bad Request", &page("Erro", "Resposta inesperada do Google."));
            return Err("Resposta inesperada do Google.".into());
        };

        let resp = agent().post(TOKEN_URL).send_form(&[
            ("client_id", LOOPBACK.id),
            ("client_secret", LOOPBACK.secret),
            ("code", code.as_str()),
            ("code_verifier", p.verifier.as_str()),
            ("redirect_uri", p.redirect_uri.as_str()),
            ("grant_type", "authorization_code"),
        ]);
        let result = match resp {
            Ok(r) => {
                let v: serde_json::Value = r.into_json().map_err(|e| e.to_string())?;
                match (v["access_token"].as_str(), v["refresh_token"].as_str()) {
                    (Some(a), Some(rt)) => save_token(
                        dir,
                        &Token {
                            refresh_token: rt.to_string(),
                            access_token: a.to_string(),
                            expires_at: model::now_ms() + v["expires_in"].as_i64().unwrap_or(3600) * 1000,
                            method: Method::Loopback,
                        },
                    ),
                    _ => Err("O Google não devolveu o token de acesso.".into()),
                }
            }
            Err(e) => Err(http_err(e)),
        };
        match &result {
            Ok(()) => respond(&mut stream, "200 OK", &page("Pronto!", "Sua conta Google foi conectada ao Meus Gastos.")),
            Err(e) => respond(&mut stream, "200 OK", &page("Não deu certo", e)),
        }
        return result;
    }
}
// ---------------------------------------------------------------- login por código (alternativa)
//
// Fluxo "device" do Google (cliente do tipo TV): o app mostra um código e a pessoa o
// digita em google.com/device, em qualquer aparelho. Útil se o login direto não abrir.

pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub interval: u64,
    pub expires_in: u64,
}

pub fn start_device_flow() -> Result<DeviceCode, String> {
    if !has_device() {
        return Err("Este app foi compilado sem o cliente de login por código (veja o PUBLICAR.md).".into());
    }
    let v: serde_json::Value = agent()
        .post("https://oauth2.googleapis.com/device/code")
        .send_form(&[("client_id", DEVICE.id), ("scope", SCOPE)])
        .map_err(http_err)?
        .into_json()
        .map_err(|e| e.to_string())?;
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    let url = if v["verification_url"].is_string() { s("verification_url") } else { s("verification_uri") };
    Ok(DeviceCode {
        device_code: s("device_code"),
        user_code: s("user_code"),
        verification_url: url,
        interval: v["interval"].as_u64().unwrap_or(5),
        expires_in: v["expires_in"].as_u64().unwrap_or(1800),
    })
}

pub enum Poll {
    Pending,
    SlowDown,
    Done,
    Failed(String),
}

/// Verifica se o código já foi autorizado. Em caso de sucesso, guarda o token.
pub fn poll(dir: &Path, dc: &DeviceCode) -> Poll {
    let resp = agent().post(TOKEN_URL).send_form(&[
        ("client_id", DEVICE.id),
        ("client_secret", DEVICE.secret),
        ("device_code", dc.device_code.as_str()),
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
    ]);
    match resp {
        Ok(r) => {
            let Ok(v) = r.into_json::<serde_json::Value>() else { return Poll::Failed("Resposta inválida do Google".into()) };
            let (Some(access), Some(refresh)) = (v["access_token"].as_str(), v["refresh_token"].as_str()) else {
                return Poll::Failed("O Google não devolveu o token de acesso.".into());
            };
            let t = Token {
                refresh_token: refresh.to_string(),
                access_token: access.to_string(),
                expires_at: model::now_ms() + v["expires_in"].as_i64().unwrap_or(3600) * 1000,
                method: Method::Device,
            };
            match save_token(dir, &t) {
                Ok(()) => Poll::Done,
                Err(e) => Poll::Failed(e),
            }
        }
        Err(ureq::Error::Status(_, r)) => {
            let v: serde_json::Value = r.into_json().unwrap_or_default();
            match v["error"].as_str().unwrap_or_default() {
                "authorization_pending" => Poll::Pending,
                "slow_down" => Poll::SlowDown,
                "access_denied" => Poll::Failed("Acesso não autorizado na conta Google.".into()),
                "expired_token" => Poll::Failed("O código expirou. Tente de novo.".into()),
                other => Poll::Failed(format!("Erro do Google: {other}")),
            }
        }
        // Falha de rede momentânea: tenta de novo no próximo ciclo.
        Err(ureq::Error::Transport(_)) => Poll::Pending,
    }
}
// ---------------------------------------------------------------- Drive

struct DriveFile {
    id: String,
    name: String,
}

fn bearer(tok: &str) -> String {
    format!("Bearer {tok}")
}

fn list_files(tok: &str, q: &str) -> Result<Vec<DriveFile>, String> {
    let mut out = Vec::new();
    let mut page: Option<String> = None;
    loop {
        let mut req = agent()
            .get("https://www.googleapis.com/drive/v3/files")
            .set("Authorization", &bearer(tok))
            .query("q", q)
            .query("spaces", "drive")
            .query("fields", "nextPageToken,files(id,name)")
            .query("pageSize", "1000");
        if let Some(p) = &page {
            req = req.query("pageToken", p);
        }
        let v: serde_json::Value = req.call().map_err(http_err)?.into_json().map_err(|e| e.to_string())?;
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

fn download(tok: &str, id: &str) -> Result<Vec<u8>, String> {
    let resp = agent()
        .get(&format!("https://www.googleapis.com/drive/v3/files/{id}"))
        .query("alt", "media")
        .set("Authorization", &bearer(tok))
        .call()
        .map_err(http_err)?;
    let mut buf = Vec::new();
    resp.into_reader()
        .take(50 * 1024 * 1024)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

/// Encontra (ou cria) a pasta do app no Drive. Com o escopo drive.file, a busca
/// só retorna pastas criadas por este app — no celular ou no computador.
fn app_folder(tok: &str) -> Result<String, String> {
    let q = format!("name = '{FOLDER_NAME}' and mimeType = '{FOLDER_MIME}' and trashed = false");
    if let Some(f) = list_files(tok, &q)?.into_iter().next() {
        return Ok(f.id);
    }
    let v: serde_json::Value = agent()
        .post("https://www.googleapis.com/drive/v3/files")
        .set("Authorization", &bearer(tok))
        .send_json(serde_json::json!({ "name": FOLDER_NAME, "mimeType": FOLDER_MIME }))
        .map_err(http_err)?
        .into_json()
        .map_err(|e| e.to_string())?;
    v["id"].as_str().map(String::from).ok_or_else(|| "O Google não devolveu o id da pasta.".to_string())
}

fn upload_new(tok: &str, folder: &str, name: &str, mime: &str, bytes: &[u8]) -> Result<(), String> {
    let boundary = format!("gastos-{}", model::new_id());
    let meta = serde_json::json!({ "name": name, "parents": [folder] }).to_string();
    let mut body = Vec::with_capacity(bytes.len() + 512);
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{meta}\r\n--{boundary}\r\nContent-Type: {mime}\r\n\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    agent()
        .post("https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart")
        .set("Authorization", &bearer(tok))
        .set("Content-Type", &format!("multipart/related; boundary={boundary}"))
        .send_bytes(&body)
        .map_err(http_err)?;
    Ok(())
}

fn upload_update(tok: &str, id: &str, mime: &str, bytes: &[u8]) -> Result<(), String> {
    agent()
        .request("PATCH", &format!("https://www.googleapis.com/upload/drive/v3/files/{id}?uploadType=media"))
        .set("Authorization", &bearer(tok))
        .set("Content-Type", mime)
        .send_bytes(bytes)
        .map_err(http_err)?;
    Ok(())
}

/// Nomes de foto vindos da nuvem só podem conter caracteres seguros
/// (evita gravar fora da pasta `fotos/`).
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
pub fn sync(dir: &Path, local: Data) -> Result<SyncResult, String> {
    let tok = access_token(dir)?;
    let folder = app_folder(&tok)?;
    let files = list_files(&tok, &format!("'{folder}' in parents and trashed = false"))?;
    let find = |name: &str| files.iter().find(|f| f.name == name);

    let remote: Option<Data> = match find(DATA_FILE) {
        Some(f) => {
            let bytes = download(&tok, &f.id)?;
            Some(
                serde_json::from_slice(&bytes)
                    .map_err(|_| "O arquivo na nuvem está em um formato desconhecido. Nada foi alterado.".to_string())?,
            )
        }
        None => None,
    };
    let merged = match &remote {
        Some(r) => model::merge(&local, r),
        None => local,
    };
    if remote.as_ref() != Some(&merged) {
        let bytes = serde_json::to_vec(&merged).map_err(|e| e.to_string())?;
        match find(DATA_FILE) {
            Some(f) => upload_update(&tok, &f.id, "application/json", &bytes)?,
            None => upload_new(&tok, &folder, DATA_FILE, "application/json", &bytes)?,
        }
    }

    // Fotos: envia as que só existem aqui e baixa as que só existem na nuvem.
    let referenced: BTreeSet<&str> = merged.expenses.iter().filter_map(|e| e.photo.as_deref()).collect();
    let (mut up, mut down) = (0, 0);
    for name in referenced {
        if !safe_photo_name(name) {
            continue;
        }
        match (storage::has_photo(dir, name), find(name)) {
            (true, None) => {
                let bytes = storage::read_photo(dir, name).ok_or("foto local ilegível")?;
                upload_new(&tok, &folder, name, "image/jpeg", &bytes)?;
                up += 1;
            }
            (false, Some(f)) => {
                let bytes = download(&tok, &f.id)?;
                storage::write_photo(dir, name, &bytes)?;
                down += 1;
            }
            _ => {}
        }
    }
    Ok(SyncResult { data: merged, photos_up: up, photos_down: down })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_rfc7636_example() {
        // Exemplo do Apêndice B da RFC 7636.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert_ne!(random_urlsafe(16).unwrap(), random_urlsafe(16).unwrap());
    }

    #[test]
    fn loopback_ignores_noise_and_reports_denied() {
        if !has_loopback() {
            return; // sem o cliente de login direto, begin_login recusa
        }
        use std::io::{Read, Write};
        let p = begin_login().unwrap();
        assert!(p.url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
        assert!(p.url.contains("code_challenge_method=S256"));
        let port = p.listener.local_addr().unwrap().port();
        let state = p.state.clone();
        let client = std::thread::spawn(move || {
            let get = |path: String| {
                let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
                write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
                let mut out = String::new();
                let _ = s.read_to_string(&mut out);
                out
            };
            assert!(get("/favicon.ico".into()).starts_with("HTTP/1.1 404"));
            assert!(get("/?state=errado&code=x".into()).starts_with("HTTP/1.1 404"));
            get(format!("/?state={state}&error=access_denied"))
        });
        let dir = std::env::temp_dir();
        let r = finish_login(&dir, p, || false);
        assert_eq!(r, Err("Acesso não autorizado na conta Google.".to_string()));
        assert!(client.join().unwrap().contains("Login cancelado"));
    }

    #[test]
    fn photo_names() {
        assert!(safe_photo_name("foto_20261106_101010_123.jpg"));
        assert!(!safe_photo_name("../gastos.json"));
        assert!(!safe_photo_name("..\\x.jpg"));
        assert!(!safe_photo_name("a/b.jpg"));
        assert!(!safe_photo_name(".hidden"));
        assert!(!safe_photo_name("gastos.json"));
        assert!(!safe_photo_name(""));
    }
}
