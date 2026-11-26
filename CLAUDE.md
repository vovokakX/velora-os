# CLAUDE.md — Velora OS

Инструкции для агентов и разработчиков, которые продолжают работу над проектом.

## Цель проекта

Velora OS — операционная система для x86_64, написанная с нуля: своё ядро,
свои драйверы, своё пользовательское окружение. Это не дистрибутив Linux и не
форк. Сейчас система работает только в текстовом режиме. Позже появятся
графическая оболочка **Lumi** и запуск программ Windows (.exe) через
Win32-слой. Архитектура уже оставляет для них место: personality процесса,
интерфейс `ExecFormat` с заготовкой PE, абстракция framebuffer.

Цели производительности: в простое меньше 64 МиБ ОЗУ (сейчас 7,7 МиБ);
загрузка до приглашения входа в QEMU меньше 3 с (сейчас 1,7 с под KVM в
CI вместе с прошивкой, у установленной системы 1,65 с).

Репозиторий: https://github.com/vovokakX/velora-os

## План этапов и статус

| # | Ветка | Содержание | Статус |
|---|---|---|---|
| 1 | `stage-1-boot` | UEFI-загрузка (Limine), framebuffer и COM1, исключения, паника, сборка ISO, CI | готово, v0.1.0 |
| 2 | `stage-2-memory` | Аллокатор страниц, свои таблицы страниц (W^X), куча, стеки с защитной страницей | готово, v0.2.0 |
| 3 | `stage-3-interrupts` | ACPI, LAPIC/IOAPIC, таймер 100 Гц, PS/2, раскладки EN/RU | готово, v0.3.0 |
| 4 | `stage-4-processes` | Потоки, процессы, вытесняющий планировщик, ring 3, `syscall` | готово, v0.4.0 |
| 5 | `stage-5-exec` | Загрузчик ELF, `ExecFormat` + заготовка PE, initramfs, vlibc | готово, v0.5.0 |
| 6 | `stage-6-drivers` | PCI, virtio-blk, AHCI, NVMe, RTC | готово, v0.6.0 |
| 7 | `stage-7-fs` | VFS, FAT16/FAT32, VeloraFS (права, журнал, CRC32C, fsck), файловые вызовы | готово, v0.7.0 |
| 8 | `stage-8-userland` | vsh, команды, пользователи и вход (PBKDF2), velofetch, каналы, терминал | готово, v0.8.0 |
| 9 | `stage-9-installer` | velora-setup: GPT, FAT32 ESP + VeloraFS root, загрузчик; полный E2E-тест | готово, v0.9.0 |
| 10 | `stage-10-network` | virtio-net, ARP, IPv4, ICMP, UDP, DHCP, TCP | **следующий** |

Дальше: xHCI/USB HID, SMP, гибридный ISO для USB, загрузка через BIOS, Lumi,
Win32-слой. Подробности — `docs/roadmap.md`.

## Где мы сейчас

Этапы 1–9 завершены и выпущены (последний релиз — v0.9.0). Следующий —
**этап 10** (issue #10). Что нужно сделать:

- Драйвер virtio-net (PCI, современный virtio 1.x, как virtio-blk в
  `kernel/src/drivers/virtio_blk.rs`).
- Сетевой стек (логику — в `no_std` crate с тестами на хосте): Ethernet,
  ARP, IPv4, ICMP (echo), UDP, DHCP-клиент, TCP (установка соединения,
  передача, повтор, закрытие).
- Системные вызовы сокетов (новые номера, `docs/syscalls.md`) и функции
  vlibc; команды, например `ping`, `ifconfig`/`ip`, простой TCP-клиент
  (`nc`/`fetch`).
- Тесты: в QEMU с user-mode сетью (`-netdev user`): DHCP получает адрес,
  `ping` шлюза 10.0.2.2, обмен по TCP с сервером на хосте
  (`hostfwd`/guestfwd или сервер в тестовом скрипте). Критерий готовности
  из плана: ping и TCP-обмен с хостом QEMU.

Как устроено то, что уже есть: `docs/architecture.md` (файловые системы,
пользовательское окружение, установка), `docs/velorafs.md`,
`installer/README.md`, решения D25–D35. Пароль Live-образа по умолчанию
`velora`. Тесты: `ktest` (две загрузки), `boot` (вход и команды),
`install` (установка, загрузка с диска, сохранение файлов).

Открытые issues:
- **#19** — с QEMU 8.2 (Ubuntu 24.04) virtio-blk не получает ответов. С QEMU 9.1
  и 11.1 работает. Причина не найдена, факты собраны в issue. Из-за этого CI
  и сборка релиза идут в контейнере Fedora 41.
- **#12** — actions/checkout@v4 и upload-artifact@v4 используют устаревший Node 20.
- Issues #8–#10 — будущие этапы.

## Правила проекта

### GitHub-процесс

- `main` защищена: изменения только через Pull Request, нужна зелёная проверка
  «build and test» (правило действует и для администратора). Прямой push в
  `main` запрещён.
- Каждый этап делается в своей ветке `stage-N-<slug>` и закрывает свой issue
  (`Closes #N` в описании PR). Прочие изменения — тоже в отдельных ветках.
- Коммиты маленькие, на английском, в формате `area: what changed`,
  например `kernel: add page frame allocator`. В конце каждого коммита строка
  `Co-Authored-By: ...`, которую задаёт среда.
- Описание PR содержит три раздела: что сделано, как проверено, что осталось.
- Найденные ошибки оформляются как issues с меткой `bug`, даже если
  исправлены в том же PR.
- PR вливается merge-коммитом (`gh pr merge --merge`) только при зелёном CI.
- После этапа ставится тег `vX.Y.0` (этап N → v0.N.0). Workflow `release.yml`
  прогоняет тесты, собирает `velora.iso` и `velora.iso.sha256` и создаёт
  релиз из `docs/releases/vX.Y.Z.md`. Этот файл должен быть в `main` до тега.
- Для коммитов используется адрес `218248768+vovokakX@users.noreply.github.com`,
  личная почта в истории не появляется.

### Тексты

- README, CHANGELOG, описания PR и issues, заметки к релизам — на двух языках
  в одном тексте: сначала русский, затем английский.
- Код, комментарии в коде и сообщения коммитов — только на английском.
- Стиль: коротко, по делу, только факты. Без эмодзи, без рекламных слов
  («революционный», «мощный», «seamless», «blazing fast»), без восклицаний.
- Шаблон релиза (обязательный, без вступлений и дополнительных разделов):

  ```
  ## Velora OS vX.Y.Z
  ### Что нового
  - ...
  ### Исправления
  - ...
  ### Известные проблемы
  - ...
  ---
  ### What's new
  - ...
  ### Fixes
  - ...
  ### Known issues
  - ...
  ```
  То же содержимое добавляется в `CHANGELOG.md` под заголовком `## [X.Y.Z] — дата`.
- Не заменять реализацию заглушками без явной пометки: в коде
  `// NOT IMPLEMENTED: ...` (или комментарий «NOT IMPLEMENTED» в doc-комментарии),
  в PR и в известных проблемах.
- Проблемы описываются честно: если что-то не получилось или причина не
  найдена, так и пишется.

### Лицензии

- Проект: **GPL-3.0-or-later** (`LICENSE` в корне). Ядро, драйверы, инструменты,
  программы.
- vlibc (`libc/vlibc`): **LGPL-3.0-or-later** (`libc/vlibc/LICENSE`), чтобы
  программы с любой лицензией могли с ней компоноваться. Крейты, код которых
  входит в vlibc, тоже LGPL-3.0-or-later: `kernel/abi` (velora-abi) и
  `kernel/mm` (velora-mm). LGPL совместима с GPL, поэтому ядро, которое их
  использует, остаётся под GPL.
- Сторонние компоненты: только Limine (BSD-2, скачивается при сборке с проверкой
  SHA-256) и шрифт Spleen (BSD-2). Список — `docs/third-party.md`, тексты
  лицензий — `docs/licenses/` и `/licenses` в ISO. Новый сторонний код
  добавляется только с совместимой лицензией и записью в этих местах.
- В репозиторий не попадают ключи, пароли, собранные образы и прочие бинарные
  файлы (кроме изображений для документации).

## Сборка и тесты

Нужно: rustup (версия закреплена в `rust-toolchain.toml`, сейчас 1.100.0),
curl, unzip, sha256sum, bash, Python 3, QEMU с прошивкой OVMF. Работает в Linux
и в Windows (Git Bash; QEMU в `C:\Program Files\qemu`, его нужно добавить в
PATH для тестов).

```sh
./build.sh           # velora.iso: ядро + программы + initramfs + Limine
./build.sh --test    # velora-test.iso: ядро с тестами (feature ktest)
./test.sh            # юнит-тесты на хосте, тесты ядра в QEMU, тест загрузки
./test.sh --host     # только юнит-тесты на хосте
python tests/qemu_run.py ktest --timestamps   # один прогон тестов ядра
python tests/qemu_run.py boot                 # загрузка, вход, сценарий команд, shutdown
python tests/qemu_run.py install              # установка, загрузка с диска, файлы сохраняются
```

Перед PR (то же проверяет CI):

```sh
cargo fmt --all && (cd userland && cargo fmt --all)
cargo clippy --workspace --exclude velora-kernel --all-targets -- -D warnings
cargo clippy -p velora-kernel --target x86_64-unknown-none -- -D warnings
cargo clippy -p velora-kernel --target x86_64-unknown-none --features ktest -- -D warnings
(cd userland && CARGO_TARGET_DIR=../target/user \
  RUSTFLAGS="-C relocation-model=static -C force-frame-pointers=yes" \
  cargo clippy --target x86_64-unknown-none -- -D warnings)
./test.sh
```

Как устроены тесты:
- Юнит-тесты на хосте — в аппаратно-независимых crate (`kernel/mm`,
  `kernel/acpi`, `kernel/abi`, `kernel/exe`, `drivers/*`, `fs/*`, `tools/*`).
  Логику, которую можно проверить без железа, выносить именно туда.
  Любая файловая система должна проходить `velora_vfs::conformance::run`;
  для тестов дисков есть `velora_vfs::MemDisk` (кеш записи, обрыв питания).
- Тесты ядра — функции в `kernel/src/ktest/`, список в `ktest/mod.rs`. Они
  запускаются в QEMU, результат передаётся через `isa-debug-exit` (33 —
  успех). Последний шаг — намеренная паника, которая проверяет отчёт о панике.
- `/bin/vtest` проверяет vlibc и системные вызовы из режима пользователя.
- Режим `boot` в `tests/qemu_run.py` вводит текст через монитор QEMU
  (`sendkey`, таблица `KEY_NAMES`): вход, команды vsh, Tab, история,
  Ctrl+C, выход, вход root и `shutdown`. Вывод ищется только после
  предыдущего шага (класс `Session`), поэтому маркеры должны отличаться от
  набранного текста (например, `X=abc; echo $X$X` → `abcabc`).
- `tests/qemu_run.py`: в режиме `ktest` подключает три диска (virtio-blk,
  AHCI, NVMe) с метками и чистый диск на 128 МиБ для файловых систем и
  загружает тестовое ядро дважды (D28). После каждой загрузки проверяет
  записи на дисках и тома программой `velora-fsck` (и `fsck.fat`, если он
  установлен; в CI он есть). Через монитор QEMU нажимает клавиши. Переменные окружения: `QEMU`, `OVMF_CODE`,
  `OVMF_VARS`, `VELORA_ACCEL`, `VELORA_QEMU_EXTRA` (доп. аргументы, например
  `-trace virtio*`), `VELORA_VIRTIO_OPTS`.
- CI (`.github/workflows/ci.yml`) работает в контейнере Fedora 41 (QEMU 9.1,
  KVM). Раннеры бывают и на Intel, и на AMD — поведение может отличаться
  (так нашли #20).

Отладка: адреса из отчёта о панике переводятся в функции через
`llvm-objdump`/`llvm-addr2line` из компонента `llvm-tools`
(`rustup component add llvm-tools`) по файлу
`target/x86_64-unknown-none/release/velora` — той же сборки, что дала отчёт.

## Устройство репозитория

```
boot/        конфигурация Limine
kernel/      ядро (arch, boot, console, mm, sched, syscall, drivers, exec, ...)
kernel/mm, kernel/acpi, kernel/abi, kernel/exe   библиотеки ядра с тестами на хосте
kernel/src/fs  VFS в ядре: монтирование, tarfs, devfs, каналы, дескрипторы
drivers/     аппаратно-независимые части драйверов (input, pci, rtc)
fs/          vfs (интерфейс, монтирование, MemFs, conformance), velorafs,
             fat (FAT16/FAT32), gpt, tar (initramfs)
libc/vlibc   библиотека для программ (LGPL)
libc/auth    SHA-256, PBKDF2, /etc/passwd и /etc/shadow (тесты на хосте)
libc/shell   язык vsh и редактор строки (тесты на хосте)
userland/    отдельный workspace программ (login, vsh, coreutils, hello, vtest), свои флаги сборки
tools/       mkiso, mkinitrd, mkpasswd, kstrip, fsck (velora-fsck), fontgen
installer/   velora-setup (собирается в workspace userland)
tests/       qemu_run.py
docs/        архитектура, карта памяти, системные вызовы, формат VeloraFS, решения (D1–D35),
             план, сторонние компоненты, заметки к релизам
```

Важные решения записаны в `docs/decisions.md`. Новые решения добавлять туда
же, на двух языках, со следующим номером.
