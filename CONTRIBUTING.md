# Участие в разработке / Contributing

[Русский](#русский) | [English](#english)

## Русский

### Порядок работы

- `main` всегда собирается и проходит тесты. Прямые коммиты в `main` запрещены.
- Каждый этап или исправление делается в отдельной ветке (`stage-2-memory`,
  `fix-serial-timeout` и т. п.) и вливается через Pull Request.
- PR вливается только при зелёном CI.
- Найденные ошибки оформляются как issue.

### Коммиты

- На английском, в формате `область: что сделано`, например
  `kernel: add page frame allocator`, `mkiso: support nested directories`.
- Один коммит — одно логическое изменение.

### Тексты

- README, CHANGELOG, описания PR, issues и релизов пишутся на двух языках:
  сначала русский, затем английский.
- Код, комментарии и сообщения коммитов — только на английском.
- Коротко и по делу, без эмодзи и рекламных слов. Только факты: что
  добавлено, что исправлено, какие проблемы известны.

### Перед отправкой PR

```sh
cargo fmt --all
cargo clippy --workspace --exclude velora-kernel --all-targets -- -D warnings
cargo clippy -p velora-kernel --target x86_64-unknown-none --features ktest -- -D warnings
./test.sh
```

В репозиторий не добавляются ключи, пароли, собранные образы и другие
бинарные файлы (кроме изображений для документации).

### Заглушки

Если что-то реализовано не полностью, это явно помечается в коде
(`// NOT IMPLEMENTED: ...`) и в описании PR.

## English

### Workflow

- `main` always builds and passes the tests. No direct commits to `main`.
- Every stage or fix goes into its own branch (`stage-2-memory`,
  `fix-serial-timeout`, ...) and is merged through a pull request.
- A PR is merged only when CI is green.
- Bugs are filed as issues.

### Commits

- In English, formatted as `area: what changed`, for example
  `kernel: add page frame allocator`, `mkiso: support nested directories`.
- One commit per logical change.

### Texts

- README, CHANGELOG, PR descriptions, issues and release notes are written in
  two languages: Russian first, then English.
- Code, comments and commit messages are English only.
- Short and factual, no emoji or marketing words: what was added, what was
  fixed, what is known to be broken.

### Before opening a PR

Run the commands listed in the Russian section above. Do not commit keys,
passwords, built images or other binaries (images used by the docs are fine).

### Stubs

Anything that is not fully implemented is marked explicitly in the code
(`// NOT IMPLEMENTED: ...`) and in the PR description.
