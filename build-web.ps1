# Gera a versão web (WebAssembly) do Meus Gastos em docs/app/pkg.
# Requisitos (uma vez só):
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version 0.2.130
# Para a sincronização com o Google Drive funcionar no site, salve antes o cliente
# OAuth "Aplicativo da Web" em oauth/web.json (veja PUBLICAR.md, seção 6).
Set-Location $PSScriptRoot

if (-not (Test-Path "oauth/web.json")) {
    Write-Warning "oauth/web.json não encontrado: o app web vai funcionar, mas sem sincronização."
}

# Otimizado para tamanho (o download no celular fica menor)
$env:CARGO_PROFILE_RELEASE_OPT_LEVEL = "s"
$env:CARGO_PROFILE_RELEASE_LTO = "fat"
$env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS = "1"
cargo build --lib --release --target wasm32-unknown-unknown
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

wasm-bindgen --target web --no-typescript --out-dir docs/app/pkg target/wasm32-unknown-unknown/release/gastos.wasm
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "Pronto: docs/app (teste local: python -m http.server 8765 --directory docs e abra http://localhost:8765/app/)"
