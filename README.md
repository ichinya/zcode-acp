# zcode-acp

Standalone **Rust ACP adapter for ZCode**: Coding Plan, Start Plan, personal providers and quota commands. Works independently of BB.

Самостоятельный **Rust-адаптер ACP для ZCode**: Coding Plan, Start Plan, личные провайдеры и команды квот. Работает без BB.

- [English documentation](https://ichinya.github.io/bb-plugin-zcode/en/runtime.html)
- [Документация на русском](https://ichinya.github.io/bb-plugin-zcode/ru/runtime.html)
- [BB plugin](https://github.com/ichinya/bb-plugin-zcode)

```sh
cargo build --release --locked
cargo install --path . --locked
zcode-acp --doctor
zcode-acp --permission=accept-edits
zcode-acp usage --plan start --json
```

Requires signed-in ZCode Desktop. The adapter automatically uses ZCode's bundled Electron/Node runtime; a separate Node.js installation is optional. A loose CLI without a recognized bundle uses Node.js 24+ from PATH. Rust is needed only when compiling. Windows session ACLs use Windows PowerShell 5.1.

ACP 0.3.2+ reads ZCode 3.18.2 `enc:v2` sign-in through the existing OS credential store, or Desktop's configured secret/secret file. Legacy `enc:v1` remains supported. Desktop files and credential-store entries are never rewritten.

Нужен вход в ZCode Desktop. Адаптер автоматически использует встроенный Electron/Node runtime приложения; отдельная установка Node.js необязательна. Для CLI вне распознанной установки используется Node.js 24+ из PATH. Rust требуется только для сборки. Права на сессии Windows устанавливаются через Windows PowerShell 5.1.

ACP 0.3.2+ читает вход ZCode 3.18.2 в формате `enc:v2` через существующее хранилище ключей ОС либо настроенный в Desktop секрет/файл секрета. Формат `enc:v1` также поддерживается. Файлы Desktop и записи хранилища ключей не перезаписываются.

The `lib/`, `vendor/` and original JS tests are retained as migration references. Production transport, auth, state and quota collection are in `src/`; the BB plugin calls the compiled executable and imports no JS adapter API. `package.json` is a private test harness, not an npm runtime package.

Publishing a GitHub Release (including a prerelease) builds Windows x64, Linux x64 and macOS ARM64/x64 archives and attaches them with SHA-256 checksums to that release. The tag must match `v` + the version in `Cargo.toml`; drafts do not start a build. Re-running the workflow updates the same release assets.

Публикация GitHub Release, включая предварительный релиз, запускает сборки Windows x64, Linux x64 и macOS ARM64/x64. Архивы и SHA-256 прикрепляются к этому же релизу. Тег должен совпадать с `v` + версией в `Cargo.toml`; черновик сборку не запускает. Повторный запуск обновляет вложения того же релиза.

Apache-2.0 · MIT account-flow attribution: [NOTICE](NOTICE), [LICENSES](LICENSES/).
