<p align="center"><img src="assets/logo-completo.jpg" alt="Meus Gastos — controle seu dinheiro" width="420"></p>

# Meus Gastos

App de controle de gastos mensais para **Android, computador** (Windows/Linux/macOS) **e web**
(inclusive **iPhone/iPad**), escrito em **Rust** com UI em **Slint**, com sincronização opcional
pelo **Google Drive**.

**Versão web:** <https://pedrohszuqui.github.io/meusgastos/app/> — no iPhone, abra no Safari e use
*Compartilhar → Adicionar à Tela de Início*.

> O GPUI (framework do Zed) não tem suporte a Android, por isso a UI usa o Slint,
> que roda nativamente no Android e no computador com o mesmo código e o mesmo visual.

## Funcionalidades

Três seções — no celular ficam na barra inferior; no computador, numa barra lateral:

- **Painel** (dashboard interativo)
  - **Saldo do mês** = saldo que sobrou dos meses anteriores + renda − gastos. O que sobra (ou falta)
    passa automaticamente para o mês seguinte, aparecendo como "+ R$ X que sobrou até outubro".
    O saldo é calculado a partir dos lançamentos: editar um gasto antigo atualiza os meses seguintes.
  - Quadros de Renda e Gastos e barra de quanto do dinheiro disponível já foi gasto
    (toque em "Renda" para lançar uma receita)
  - Comparação com o mês anterior, nº de despesas, média por dia e maior gasto
  - Gráfico de rosca por categoria: toque numa fatia (ou na legenda) para destacá-la e ver valor/percentual; botão "Ver despesas de …" abre a lista filtrada
  - Barras dos últimos 6 meses (gastos e renda lado a lado): toque numa barra para ir àquele mês
  - Gastos por dia: toque numa barra para ver o valor e a quantidade de despesas do dia
  - Maiores despesas do mês (toque para editar)
- **Extrato**: alterna entre **Despesas** (agrupadas por dia, com filtros por categoria) e **Receitas**
- **Categorias**: total e nº de despesas por categoria no mês; adicionar e remover categorias

Além disso:

- Receitas (dinheiro recebido): valor, descrição (atalhos "Salário", "Freelance", "Outros") e data
- **Compras parceladas e despesas futuras**
  - No formulário: **À vista** ou **Parcelado** (2 a 60x), informando o valor total ou o da parcela.
    O app cria uma parcela por mês e mostra uma prévia ("3x de R$ 1.000,00 · de 11/2026 a 01/2027").
  - A parcela paga no dia da compra entra como **gasto**; as de vencimento futuro (e qualquer despesa
    com data futura) entram como **gasto planejado**.
  - Gasto planejado **não conta** no total, no saldo, na rosca nem nos gráficos de gasto até você
    **confirmar o pagamento**. Ele aparece separado ("Planejado no mês" e "saldo previsto"), com a
    etiqueta *Planejado* (ou *Vencido*) no Extrato, e empilhado em laranja no gráfico de 6 meses.
  - **Lembrete no Painel** (sem pop-up): o card "Pagamentos para confirmar" lista os vencidos e os que
    vencem nos próximos 3 dias, com o botão **Pago**. Também dá para confirmar abrindo a parcela.
  - Ao editar uma parcela: **Excluir esta e as próximas parcelas** remove o restante da compra.- Despesas: valor em destaque (aceita `49,90`, `1.234,56`), descrição, categoria (chips coloridos) e data (atalhos "Hoje" e "Ontem")
- Foto opcional em cada despesa: no celular, escolhida na galeria; no computador, por uma janela de arquivos
- O botão "voltar" do Android volta uma tela (formulário → painel) em vez de fechar o app
- Dados salvos localmente (`gastos.json` + pasta `fotos/`) e, se conectado, sincronizados com o Google Drive

## Versão para computador

Mesmo app, mesmo visual. Quando a janela tem pelo menos 900 px de largura, o layout muda sozinho:
barra lateral com navegação, botões "Nova despesa"/"Nova receita" e status da sincronização,
cards do painel em duas colunas e listas/formulários centralizados.

```bash
cargo build --release --bin gastos-desktop
```

O executável fica em `target/release/gastos-desktop.exe` (um único arquivo, sem instalação).
Os dados ficam em `%APPDATA%\MeusGastos` (ou na pasta definida por `GASTOS_DATA_DIR`).

## Versão web (iPhone, iPad e navegadores)

O mesmo código compilado para WebAssembly (`wasm32-unknown-unknown`), com o mesmo visual e a mesma
sincronização. Os dados ficam no `localStorage` e as fotos no IndexedDB do navegador; o login com o
Google é feito por redirecionamento (cliente OAuth "Aplicativo da Web", `oauth/web.json`).
É um PWA: dá para adicionar à Tela de Início e usar sem internet.

```powershell
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130
.uild-web.ps1          # gera docs/app/pkg
python -m http.server 8765 --directory docs   # teste: http://localhost:8765/app/
```

Hospedagem gratuita (GitHub Pages, Cloudflare Pages ou Netlify): veja a seção 5.1 do [PUBLICAR.md](PUBLICAR.md).

## Sincronização com o Google Drive

- Os dados vão para a pasta **Meus Gastos** no seu Drive (escopo `drive.file`): o app só enxerga
  os arquivos que ele mesmo criou — não vê nem altera o resto do seu Drive.
- Não apague nem renomeie a pasta `Meus Gastos`: se ela sumir, o app cria outra e reenvia os dados deste aparelho.
- Sincroniza ao abrir o app, ~3 s depois de cada alteração, a cada 5 minutos e no botão "Sincronizar agora".
- Pode usar o celular e o computador ao mesmo tempo: as cópias são **mescladas**
  (vale a alteração mais recente de cada lançamento; exclusões se propagam). Fotos também são sincronizadas.
- Login **direto pela conta Google**: o app abre o navegador, você escolhe a conta e toca em
  "Permitir". Não há código para digitar nem senha dentro do app (OAuth com retorno para
  `127.0.0.1` e PKCE).
- Para **distribuir para amigos** (publicar no Cloud Console, assinar o APK, instruções de uso),
  veja **[PUBLICAR.md](PUBLICAR.md)**.

### Configuração (uma vez, no Google Cloud)

O app precisa de credenciais OAuth próprias. Sem elas ele funciona normalmente, só sem sincronização.

1. Acesse <https://console.cloud.google.com/> e crie um projeto (ex.: "Meus Gastos").
2. Em **APIs e serviços → Biblioteca**, ative a **Google Drive API**.
3. Em **APIs e serviços → Tela de consentimento OAuth** (ou "Google Auth Platform"):
   tipo **Externo**, preencha nome do app e e-mail, adicione o escopo
   `https://www.googleapis.com/auth/drive.file`.
   - Enquanto estiver em **Teste**, adicione sua conta em "Usuários de teste". Nesse modo o Google
     faz o login expirar a cada **7 dias**. Para não expirar, clique em **Publicar app** (modo Produção).
4. Crie os clientes OAuth (um por plataforma) e salve os JSON na pasta `oauth/`:
   `computador.json` (App para computador), `android.json` (opcional, App para computador
   só do celular) e `tv.json` (TVs e dispositivos de entrada limitada, para o login por código).
   Detalhes no [PUBLICAR.md](PUBLICAR.md#1-criar-os-clientes-oauth-um-por-plataforma).
5. Gere o app de novo (APK e/ou executável). No app: ícone de nuvem → **Conectar com Google**.

> As credenciais ficam embutidas no app. Para apps instalados o Google não trata o "client secret"
> como segredo real, mas não distribua publicamente um build com as suas credenciais.

## Estrutura

| Arquivo | Conteúdo |
|---|---|
| `ui/app.slint` | Todas as telas e componentes visuais (layout de celular e de computador) |
| `src/lib.rs` | Lógica do app e pontos de entrada (Android `android_main` e desktop) |
| `src/dashboard.rs` | Cálculos do painel: rosca (caminhos SVG), meses, dias, saldo, listas |
| `src/model.rs` | Modelo de dados, persistência JSON, mesclagem entre aparelhos, formatação, fotos |
| `src/sync.rs` | Google: login pelo navegador (OAuth loopback + PKCE) e API do Drive |
| `src/cloud.rs` | Liga a sincronização à interface (automática, manual, login) |
| `src/android.rs` | JNI: galeria de fotos (MediaStore), permissões, abrir navegador |
| `src/main.rs` | Executável de computador |
| `build.rs` | Compila a UI e embute os clientes OAuth da pasta `oauth/` adequados a cada plataforma |

## Pré-requisitos

- Rust + alvos Android: `rustup target add aarch64-linux-android x86_64-linux-android`
- `cargo install cargo-apk`
- Android SDK com `platforms/android-35` e `build-tools`, NDK r27 (`ndk/27.2.12479018`)
- JDK 17 (usado pelo `apksigner`/`keytool`)

Variáveis de ambiente (PowerShell):

```powershell
$env:JAVA_HOME = "C:\Program Files\Microsoft\jdk-17.0.21.21-hotspot"
$env:Path = "$env:JAVA_HOME\bin;$env:Path"
$env:ANDROID_HOME = "$env:LOCALAPPDATA\Android\Sdk"
$env:ANDROID_NDK_ROOT = "$env:ANDROID_HOME\ndk\27.2.12479018"
```

## Gerar o APK

Defina a chave de assinatura (veja [PUBLICAR.md](PUBLICAR.md#3-criar-a-sua-chave-de-assinatura-do-android-uma-vez)) e gere:

```powershell
$env:CARGO_APK_RELEASE_KEYSTORE = "caminho\para\sua-chave.jks"
$env:CARGO_APK_RELEASE_KEYSTORE_PASSWORD = Read-Host "Senha da chave"
cargo apk build --release --lib
```

O APK fica em `target/release/apk/gastos.apk`.

> **Downloads prontos:** veja a página de [Releases](https://github.com/pedrohszuqui/meusgastos/releases).
> **Site do app:** <https://pedrohszuqui.github.io/meusgastos/>

## Instalar no celular

Ative a *Depuração USB* no celular, conecte no PC e rode:

```bash
adb install -r target/release/apk/gastos.apk
```

Ou copie o `.apk` para o celular e abra-o (permitindo "instalar apps desconhecidos").

## Testes

```bash
cargo test
```
