# zcode-acp

Standalone **ACP adapter for ZCode Desktop**, with Coding Plan, Start Plan and
quota APIs. / Самостоятельный ACP-адаптер ZCode с планами и API квот.

- **English:** [Documentation](https://ichinya.github.io/bb-plugin-zcode/en/runtime.html)
- **Русский:** [Документация](https://ichinya.github.io/bb-plugin-zcode/ru/runtime.html)
- **BB plugin:** [bb-plugin-zcode](https://github.com/ichinya/bb-plugin-zcode)

Requires Node.js 24+ and signed-in ZCode Desktop. No BB SDK or runtime npm dependencies.
Нужны Node.js 24+ и вход в ZCode Desktop; SDK BB и runtime-зависимости npm не требуются.

```sh
node bin/zcode-acp.mjs --doctor
node bin/zcode-acp.mjs --permission=accept-edits
```

The second command serves ACP on stdin/stdout. / Вторая команда запускает ACP через stdin/stdout.

Apache-2.0 · Included MIT attribution: [NOTICE](NOTICE), [LICENSES](LICENSES/).
