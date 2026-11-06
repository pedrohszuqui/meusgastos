# Publicar o Meus Gastos para amigos

Passo a passo para configurar o projeto no **Google Cloud Console**, gerar o app e
distribuí-lo para amigos próximos, com **login direto pela conta Google** (sem digitar
código) e com os **dados de cada pessoa visíveis só para ela**.

---

## 0. Como funciona (e por que os dados ficam privados)

- **Não existe servidor.** Cada pessoa entra com a própria conta Google e os dados dela
  são gravados **no Google Drive dela**, numa pasta `Meus Gastos`.
- O app usa o escopo **`drive.file`**: ele só enxerga os arquivos que ele mesmo criou.
  Não lê nem altera nenhum outro arquivo do Drive da pessoa.
- **Você (dono do projeto) não tem acesso aos dados de ninguém.** O Cloud Console mostra
  apenas números de uso (quantidade de requisições), nunca o conteúdo.
- A pasta `Meus Gastos` é privada como qualquer arquivo do Drive: só a dona da conta vê,
  a não ser que ela mesma compartilhe a pasta.
- Para usar no celular e no computador, a pessoa conecta **a mesma conta** nos dois.

## 1. Criar os clientes OAuth (um por plataforma)

O app aceita **vários clientes OAuth do Google**, um arquivo JSON para cada um, na pasta
`oauth/` do projeto. Na hora de compilar, cada versão leva só os clientes da sua plataforma.

| Arquivo | Tipo no Cloud Console | Para quê | Obrigatório? |
|---|---|---|---|
| `oauth/computador.json` | **App para computador** | login direto no Windows/Linux/macOS | sim, para o login sem código no PC |
| `oauth/android.json` | **App para computador** (um segundo, só do celular) | login direto no Android | não: sem ele, o Android usa o `computador.json` |
| `oauth/tv.json` | **TVs e dispositivos de entrada limitada** | botão "Entrar com código" (alternativa) | não, mas recomendado como plano B |

> Por que o cliente do Android também é do tipo "App para computador"? O tipo **"Android"**
> do Console só funciona com o login nativo do Google Play Services, que exige código
> Java/Kotlin e um projeto Gradle (veja o item 8). Um cliente separado para o celular
> serve para acompanhar o uso e poder desativar só o Android, se precisar.

Para cada cliente:

1. Abra <https://console.cloud.google.com/> e selecione o projeto do Meus Gastos.
2. Menu **Google Auth Platform → Clientes** (ou **APIs e serviços → Credenciais**).
3. **Criar cliente** (ou **Criar credenciais → ID do cliente OAuth**).
4. Escolha o tipo da tabela acima e dê um nome que identifique a plataforma
   (ex.: `Meus Gastos - Computador`, `Meus Gastos - Android`, `Meus Gastos - TV/código`).
5. **Criar → Baixar JSON** e salve com o nome da tabela dentro da pasta `oauth/`.

O app mostra os botões conforme o que estiver configurado:
- só `computador.json`/`android.json`: **Conectar com Google** (login direto);
- só `tv.json`: **Conectar com Google (código)**;
- os dois: **Conectar com Google** + **Entrar com código**.

Cada login lembra com qual cliente foi feito e é renovado com ele.

> Os arquivos de `oauth/` estão no `.gitignore`: não os publique. Para apps instalados o
> Google não trata o "client secret" como segredo de verdade, mas não há motivo para espalhá-lo.
> Também é possível usar variáveis de ambiente: `GASTOS_COMPUTADOR_CLIENT_ID`/`_SECRET`,
> `GASTOS_ANDROID_CLIENT_ID`/`_SECRET`, `GASTOS_TV_CLIENT_ID`/`_SECRET`.
## 2. Configurar a tela de consentimento

Menu **Google Auth Platform** (nas versões antigas: **APIs e serviços → Tela de consentimento OAuth**).

1. **Branding (Informações do app)**
   - Nome do app: `Meus Gastos`
   - E-mail para suporte do usuário: o seu e-mail
   - Contato do desenvolvedor: o seu e-mail
   - **Não envie logotipo.** O logotipo obriga a passar pela verificação de marca.
     Sem ele, a verificação não é necessária (veja o passo 4).
2. **Acesso a dados (Data Access) → Adicionar ou remover escopos**
   - Deixe **somente** `https://www.googleapis.com/auth/drive.file`
     (aparece como "Ver, editar, criar e excluir apenas os arquivos do Google Drive que você usa com este app").
   - Se o `drive.appdata` estiver na lista, remova.
   - Salve.
3. **Público (Audience)**
   - Tipo de usuário: **Externo**.
4. **Publicar o app**
   - Ainda em **Público**, clique em **Publicar app** e confirme. O status muda de
     "Em teste" para **"Em produção"**.
   - Como o app usa só um escopo **não sensível** (`drive.file`), o Google **não exige**
     o processo de verificação para publicar. A verificação de marca é opcional e só serve
     para exibir nome e logotipo "verificados".
   - **Por que publicar:** no modo "Em teste", só quem estiver na lista de "Usuários de teste"
     consegue entrar (máx. 100), e o Google **desconecta todo mundo a cada 7 dias**.
     Em produção, qualquer conta Google pode entrar e o login não expira.
   - Se preferir manter "Em teste", adicione o e-mail de cada amigo em **Público → Usuários de teste**
     e avise que vai precisar reconectar toda semana.

## 3. Criar a sua chave de assinatura do Android (uma vez)

O APK de teste usa uma chave com senha fraca (`android`). Para distribuir, crie a sua
própria chave. **Guarde esse arquivo e a senha:** as atualizações do app precisam ser
assinadas com a mesma chave, senão os amigos terão de desinstalar (e perder os dados locais
que ainda não sincronizaram) para instalar a nova versão.

No PowerShell, dentro da pasta do projeto:

```powershell
$env:JAVA_HOME = "C:\Program Files\Microsoft\jdk-17.0.21.21-hotspot"
& "$env:JAVA_HOME\bin\keytool.exe" -genkeypair -keystore "$env:USERPROFILE\meusgastos-release.jks" -alias meusgastos -keyalg RSA -keysize 2048 -validity 10000 -dname "CN=Meus Gastos"
```

O `keytool` vai pedir uma senha: escolha uma forte e guarde num gerenciador de senhas.
Faça backup do arquivo `meusgastos-release.jks` fora do computador (pendrive, nuvem etc.).

## 4. Gerar as versões para distribuir

Antes de cada nova versão, aumente o número em `Cargo.toml` (`version = "0.1.0"` →
`"0.2.0"`). O Android só aceita atualizar para um número maior.

```powershell
# Ambiente (igual ao do README)
$env:JAVA_HOME = "C:\Program Files\Microsoft\jdk-17.0.21.21-hotspot"
$env:Path = "$env:JAVA_HOME\bin;$env:Path"
$env:ANDROID_HOME = "$env:LOCALAPPDATA\Android\Sdk"
$env:ANDROID_NDK_ROOT = "$env:ANDROID_HOME\ndk\27.2.12479018"

# Sua chave de assinatura (a senha não fica gravada em nenhum arquivo do projeto)
$env:CARGO_APK_RELEASE_KEYSTORE = "$env:USERPROFILE\meusgastos-release.jks"
$env:CARGO_APK_RELEASE_KEYSTORE_PASSWORD = Read-Host "Senha da chave"

cargo apk build --release --lib                 # Android -> target\release\apk\gastos.apk
cargo build --release --bin gastos-desktop      # Windows -> target\release\gastos-desktop.exe
```

As credenciais da pasta `oauth/` são embutidas automaticamente: o APK leva `android.json`
(ou `computador.json`) + `tv.json`; o executável do PC leva `computador.json` + `tv.json`.

## 5. Distribuir

**Android** (`gastos.apk`, Android 10 ou mais novo)
1. Envie o arquivo por WhatsApp, Telegram, e-mail ou um link do Drive.
2. No celular do amigo: tocar no arquivo → permitir **"Instalar apps desconhecidos"** para
   o app usado para abrir (Arquivos, WhatsApp, Chrome…).
3. Se o **Play Protect** avisar que o app é desconhecido: **Mais detalhes → Instalar mesmo assim**.
   O aviso aparece porque o app não veio da Play Store, não porque há algo errado.

**Windows** (`gastos-desktop.exe`)
1. Envie o `.exe` compactado em `.zip` (alguns serviços bloqueiam `.exe` solto).
2. Ao abrir, o Windows SmartScreen pode avisar "O Windows protegeu o computador":
   **Mais informações → Executar assim mesmo** (o app não tem assinatura de código paga).
3. Não precisa instalar. Os dados ficam em `%APPDATA%\MeusGastos`.

## 6. Primeiro uso (para enviar junto aos amigos)

1. Abra o app e toque no **ícone de nuvem** (no computador: **Sincronizar**, na barra lateral).
2. Toque em **Conectar com Google**. O navegador abre sozinho.
   (Se o navegador não voltar para o app, use **Entrar com código**: abra o endereço mostrado,
   em qualquer aparelho, e digite o código.)
3. Escolha a sua conta Google e toque em **Continuar** / **Permitir**.
   - Se aparecer "O Google não verificou este app": **Avançado → Acessar Meus Gastos**.
     (Pode aparecer enquanto o app não passa pela verificação de marca; não muda nada na privacidade.)
4. Na página "Pronto!":
   - **Celular:** toque em **Voltar ao Meus Gastos**.
   - **Computador:** feche a aba e volte para o app.
5. Pronto: a sincronização é automática (ao abrir, depois de cada alteração e a cada 5 minutos).
   Para usar no celular e no computador, conecte a **mesma conta** nos dois.

**Para desconectar ou revogar:** no app, nuvem → **Desconectar**. Ou em
<https://myaccount.google.com/permissions>, remova o acesso do "Meus Gastos".
Desconectar não apaga nada: os dados continuam no aparelho e na pasta do Drive.

## 7. Atualizações

1. Aumente `version` em `Cargo.toml`.
2. Gere de novo (passo 4) **com a mesma chave**.
3. Envie o novo APK/EXE. No Android, instalar por cima mantém os dados e o login.

## 8. Limitações e riscos (leia antes de distribuir)

- **Login no Android:** o Google recomenda que apps Android usem o login nativo do Google
  Play Services, que exige código Java/Kotlin e um projeto Gradle. Este app usa o mesmo
  método do computador (navegador + retorno para `127.0.0.1`, com PKCE), que funciona no
  Android mas é **desaconselhado** pelo Google para Android. Para um grupo de amigos é
  adequado. Se um dia o Google bloquear esse uso, será preciso migrar para o login nativo.
- **Não publique na Play Store** com essa configuração: a Play Store exige o cliente OAuth
  do tipo "Android" e o login nativo.
- **Quem tiver o APK/EXE consegue extrair o client ID.** Isso não dá acesso a dados de
  ninguém: o máximo que alguém consegue é pedir que uma pessoa autorize o "Meus Gastos" na
  conta dela, com a tela de consentimento do Google.
- **Cotas:** a Drive API é gratuita e as cotas padrão sobram para um grupo de amigos.
  Dá para acompanhar o uso em **APIs e serviços → Painel**.
- **Fotos trocadas ou apagadas** continuam na pasta do Drive (não são removidas de lá).
- **Não apague nem renomeie a pasta `Meus Gastos`** no Drive. Se ela sumir, o app cria
  outra e reenvia o que estiver no aparelho.
