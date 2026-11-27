# CLAUDE.md — Velora OS

Инструкции для агентов и разработчиков, которые продолжают работу над проектом.

## Цель проекта

Velora OS — операционная система для x86_64, написанная с нуля: своё ядро,
свои драйверы, своё пользовательское окружение. Это не дистрибутив Linux и не
форк. Сейчас система работает только в текстовом режиме. Позже появятся
графическая оболочка **Lumi** и запуск программ Windows (.exe) через
Win32-слой. Архитектура уже оставляет для них место: personality процесса,
интерфейс `ExecFormat` с заготовкой PE, абстракция framebuffer.

Цели производительности: в простое меньше 64 МиБ ОЗУ (сейчас 8,6 МиБ);
загрузка до приглашения входа в QEMU меньше 3 с (сейчас 2,1 с под KVM в
CI вместе с прошивкой, у установленной системы 1,9 с).

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
| 10 | `stage-10-network` | virtio-net, ARP, IPv4, ICMP, UDP, DHCP, TCP, сокеты | готово, v0.10.0 |
| 11 | `stage-11-plume` | Текстовый редактор Plume (`plume`, уровня nano), курсор и инверсия в консоли | готово, v0.11.0 |
| 12 | `stage-12-hardware` | Таймеры HPET/ACPI PM/PIT для калибровки, масштаб шрифта, журнал ядра (`dmesg`), журнал паники на FAT, образ `velora-usb.img`, `docs/hardware.md` | готово, v0.12.0 |
| 13 | `stage-13-acpi` | Режим ACPI, SCI, кнопка питания, выключение и перезагрузка через все виды регистров; `_S5` без интерпретатора AML (D42) | готово, v0.13.0 |
| 14 | `stage-14-smp` | Запуск AP, данные на процессор, большая блокировка ядра (D43), планировщик на несколько процессоров; все тесты QEMU с `-smp 4` | готово, v0.14.0 |
| 15 | `stage-15-usb` | xHCI, клавиатура и мышь HID boot protocol, флешки (BOT/SCSI) с разделами и FAT32, `/dev/mouse` (D44) | готово, v0.15.0 |
| 16 | `stage-16-net` | e1000/e1000e, интерфейс `NetDevice`, DNS (`velora-dns`, `DNS_SERVER`), `host`, `ping` по имени, `fetch` (HTTP/1.1) (D45) | готово, v0.16.0 |

Цель этапов 11–16: базовая терминальная ОС, которая работает в QEMU и
готова к запуску на реальном ПК (x86_64, UEFI). Графику не делаем.
Дальше: загрузка через BIOS, Lumi, Win32-слой. Подробности — `docs/roadmap.md`.

## Где мы сейчас

Все этапы 1–16 завершены (последний — v0.16.0). Новых этапов в плане нет;
дальнейшие направления — в `docs/roadmap.md` (хабы USB, Realtek, TLS,
загрузка через BIOS, Lumi, Win32-слой). Этапы делаются строго по одному: следующий
начинается, только когда тесты и CI текущего проходят. Перед каждым
этапом проверить, что уже есть в проекте: готовое и работающее —
пропустить и отметить в плане, частичное — доделать, отсутствующее —
добавить. После каждого этапа обновлять этот файл.

Что уже есть для этапов 14–16 (проверено перед этапом 11). Внешних
crate в проекте нет (D42): новые сторонние библиотеки не добавлять без
вопроса пользователю.

Известные ограничения, которые стоит закрыть в первую очередь (подробно —
в известных проблемах `docs/releases/v0.10.0.md` и прошлых версий):
сигналов и фоновых заданий нет; одна блокировка VFS и опрос дисков
(D21, D25); группировки транзакций VeloraFS нет (D26); у TCP нет сборки
сегментов не по порядку и управления перегрузкой (D37); нет DNS; нет
`mount`/`umount` для программ; только UEFI.

Как устроено: `docs/architecture.md`, `docs/velorafs.md`,
`docs/network.md`, `docs/hardware.md`, `installer/README.md`. Пароль
Live-образа по умолчанию `velora`. Тесты QEMU: `ktest` (две загрузки),
`boot` (вход, команды, редактор Plume, кнопка питания через `system_powerdown`; `--timeout 300`), `net` (DHCP,
ping, TCP с хостом), `usb` (загрузка `velora-usb.img` как USB-накопителя
на xHCI, без HPET, `reboot`), `install` (установка, загрузка с диска, сохранение
файлов). ktest после каждой загрузки читает на хосте журнал паники
(`velora-fsck --cat`). Решения D1–D45. Тест `net` идёт на e1000 (`VELORA_NET_NIC=e1000e` — 82574L) с DNS- и HTTP-серверами на хосте. ktest подключает qemu-xhci с usb-kbd, usb-mouse и usb-storage (образ флешки собирает `mkusb`); после каждой загрузки хост читает `usb.txt` с флешки. Все тесты QEMU идут с `-smp 4` (`VELORA_SMP` меняет число). В тестовой сборке BKL ведёт журнал операций и сторож: процессор, ждущий её больше 30 с, печатает журнал всех процессоров и где был владелец (через NMI).

Особенности SMP, найденные после v0.16.0: ждущий BKL процессор поднимает
приоритет задач через CR8 (не через регистр TPR в APIC: под WHPX такая
запись не держится, и загрузка зависала, #45; тест `smp::task_priority_sticks`).
Потоки ядра, которые вычисляют не блокируясь, держат BKL между тиками (D43),
поэтому счётчики теста `kernel_threads_are_preempted` закреплены за
процессором 0. Тест параллельности проверяет общую работу программ (#43):
в CI виртуальные процессоры делят ядра хоста. Если тест ядра в CI упал,
артефакт `kernel-elf` содержит точную сборку для `llvm-objdump --syms`.

Запуск вручную в Windows (окно QEMU, WHPX, 4 процессора; диск — файл в `build/`):

```sh
cp "/c/Program Files/qemu/share/edk2-i386-vars.fd" build/run-vars.fd
"/c/Program Files/qemu/qemu-system-x86_64.exe" -machine q35 -m 512M -smp 4   -accel whpx,kernel-irqchip=off -accel tcg   -drive if=pflash,format=raw,readonly=on,file="C:/Program Files/qemu/share/edk2-x86_64-code.fd"   -drive if=pflash,format=raw,file=build/run-vars.fd -cdrom velora.iso   -drive file=build/run-disk.img,format=raw,if=virtio   -netdev user,id=n0 -device e1000,netdev=n0   -device qemu-xhci -device usb-kbd -device usb-mouse -display sdl
```

Открытые issues:
- **#19** — с QEMU 8.2 (Ubuntu 24.04) virtio-blk не получает ответов. С QEMU 9.1
  и 11.1 работает. Причина не найдена, факты собраны в issue. Из-за этого CI
  и сборка релиза идут в контейнере Fedora 41.
- **#12** — actions/checkout@v4 и upload-artifact@v4 используют устаревший Node 20.
- Issues #8–#10 — будущие этапы.

## Правила проекта

### Безопасность компьютера разработчика

- Работать только внутри папки репозитория.
- Ничего не менять в системе: не устанавливать системные пакеты, не
  использовать sudo и права администратора, не менять настройки,
  переменные окружения, загрузчик и разделы.
- Никогда не обращаться к реальным дискам (`/dev/sd*`, `/dev/nvme*`,
  `\\.\PhysicalDrive*` и т. п.). Все диски для тестов — только
  файлы-образы в папке проекта. Образ для флешки собирается только как
  файл; записывает его на флешку сам пользователь.
- Если для работы чего-то не хватает — остановиться и спросить.

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
python tests/qemu_run.py net                  # DHCP, ping, TCP с сервером на хосте
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
kernel/mm, kernel/acpi, kernel/abi, kernel/exe, kernel/net   библиотеки ядра с тестами на хосте
kernel/src/fs  VFS в ядре: монтирование, tarfs, devfs, каналы, дескрипторы
drivers/     аппаратно-независимые части драйверов (input, pci, rtc, usb)
fs/          vfs (интерфейс, монтирование, MemFs, conformance), velorafs,
             fat (FAT16/FAT32), gpt, tar (initramfs)
libc/vlibc   библиотека для программ (LGPL)
libc/auth    SHA-256, PBKDF2, /etc/passwd и /etc/shadow (тесты на хосте)
libc/shell   язык vsh и редактор строки (тесты на хосте)
libc/editor  логика редактора Plume: клавиши, буфер, команды, кадры (тесты на хосте)
libc/dns     запросы и ответы DNS (LGPL, входит в vlibc), libc/http — URL и HTTP для fetch
userland/    отдельный workspace программ (login, vsh, plume, coreutils, hello, vtest), свои флаги сборки
tools/       mkiso, mkusb (velora-usb.img), mkinitrd, mkpasswd, kstrip, fsck (velora-fsck, --cat), fontgen
installer/   velora-setup (собирается в workspace userland)
tests/       qemu_run.py
docs/        архитектура, карта памяти, системные вызовы, формат VeloraFS, сеть, железо (hardware.md), решения (D1–D45),
             план, сторонние компоненты, заметки к релизам
```

Важные решения записаны в `docs/decisions.md`. Новые решения добавлять туда
же, на двух языках, со следующим номером.
