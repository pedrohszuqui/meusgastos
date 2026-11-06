//! Compila a UI e embute as credenciais OAuth do Google adequadas à plataforma.
//!
//! Credenciais (uma por cliente criado no Google Cloud Console), na pasta `oauth/`:
//!
//! | arquivo               | tipo do cliente no Console              | uso                                   |
//! |-----------------------|-----------------------------------------|---------------------------------------|
//! | `oauth/computador.json` | App para computador                   | login direto no Windows/Linux/macOS   |
//! | `oauth/android.json`    | App para computador (só para o celular) | login direto no Android (opcional: sem ele, o Android usa `computador.json`) |
//! | `oauth/tv.json`         | TVs e dispositivos de entrada limitada | login por código (alternativa)        |
//!
//! Também aceitos: variáveis `GASTOS_<CLIENTE>_CLIENT_ID` / `GASTOS_<CLIENTE>_CLIENT_SECRET`
//! (CLIENTE = COMPUTADOR, ANDROID ou TV) e o antigo `google-oauth.json` (tratado como TV).
//! Sem nenhuma credencial o app funciona normalmente, só sem sincronização.

use std::path::Path;

fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("material".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("falha ao compilar a UI Slint");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // Ícone do programa no Windows (Explorer, atalhos e barra de tarefas).
    if target_os == "windows" {
        println!("cargo:rerun-if-changed=assets/app.rc");
        println!("cargo:rerun-if-changed=assets/icone.ico");
        embed_resource::compile("assets/app.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("falha ao embutir o ícone do Windows");
    }

    let computador = load("COMPUTADOR", "oauth/computador.json");
    let android = load("ANDROID", "oauth/android.json");
    let tv = load("TV", "oauth/tv.json").or_else(|| load("", "google-oauth.json"));

    // Login direto (navegador + 127.0.0.1): cliente dedicado da plataforma, se houver.
    let direto = if target_os == "android" { android.or_else(|| computador.clone()) } else { computador };

    emit("GASTOS_LOOPBACK", direto);
    emit("GASTOS_DEVICE", tv);
}

/// Lê um cliente de variáveis de ambiente ou de um JSON baixado do Google Cloud.
fn load(env_name: &str, file: &str) -> Option<(String, String)> {
    println!("cargo:rerun-if-changed={file}");
    if !env_name.is_empty() {
        let id_var = format!("GASTOS_{env_name}_CLIENT_ID");
        let secret_var = format!("GASTOS_{env_name}_CLIENT_SECRET");
        println!("cargo:rerun-if-env-changed={id_var}");
        println!("cargo:rerun-if-env-changed={secret_var}");
        if let Ok(id) = std::env::var(&id_var) {
            if !id.is_empty() {
                return Some((id, std::env::var(&secret_var).unwrap_or_default()));
            }
        }
    }
    if !Path::new(file).exists() {
        return None;
    }
    let text = std::fs::read_to_string(file).unwrap_or_else(|e| panic!("não foi possível ler {file}: {e}"));
    let v: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|_| panic!("{file} não é um JSON válido"));
    // Formato baixado do Console ({"installed": {...}}) ou simples ({"client_id": ...}).
    let obj = v.get("installed").unwrap_or(&v);
    if v.get("web").is_some() {
        panic!("{file} é de um cliente do tipo \"Aplicativo da Web\"; crie um \"App para computador\" ou \"TV\".");
    }
    let id = obj.get("client_id").and_then(|x| x.as_str()).unwrap_or_default().to_string();
    let secret = obj.get("client_secret").and_then(|x| x.as_str()).unwrap_or_default().to_string();
    if id.is_empty() {
        panic!("{file} não contém client_id");
    }
    Some((id, secret))
}

fn emit(prefix: &str, client: Option<(String, String)>) {
    let (id, secret) = client.unwrap_or_default();
    println!("cargo:rustc-env={prefix}_CLIENT_ID={id}");
    println!("cargo:rustc-env={prefix}_CLIENT_SECRET={secret}");
}
