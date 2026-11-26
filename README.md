<p align="center"><img src="docs/images/velora-logo-dark.png" alt="Velora OS" width="320"></p>

# Velora OS

[Русский](#русский) | [English](#english)

---

## Русский

Velora OS — операционная система для x86_64, написанная с нуля. Это не дистрибутив
Linux и не форк: своё ядро, свои драйверы и своё пользовательское окружение.
Сейчас система работает только в текстовом режиме. Графическая оболочка Lumi и
запуск программ Windows (.exe) запланированы на следующие этапы.

### Состояние

Готовы этапы 1–13 (v0.13.0). Ядро загружается через UEFI, выводит текст на экран
и в последовательный порт, обрабатывает исключения процессора и паники с
подробным отчётом. Ядро управляет памятью само: аллокатор физических страниц,
свои таблицы страниц с правами W^X, куча (`alloc`), стеки с защитными
страницами. Работают прерывания (ACPI, APIC, таймер 100 Гц) и клавиатура PS/2
с раскладками EN и RU. Есть процессы и потоки с вытесняющим планировщиком,
режим пользователя (ring 3) с изоляцией памяти и системные вызовы. Программы
(ELF) пишутся на Rust с библиотекой vlibc. Есть драйверы PCI, дисков
virtio-blk, AHCI и NVMe и часов RTC. Есть VFS и файловые системы: своя
VeloraFS (права, журнал, контрольные суммы, проверка целостности) и FAT16/FAT32
для чтения и записи. initramfs смонтирован в `/`, диски — в `/mnt`. После
загрузки появляется приглашение входа; оболочка vsh поддерживает историю,
Tab-дополнение, конвейеры и перенаправления, есть основные команды (`ls`,
`cat`, `cp`, `mv`, `rm`, `ps`, `kill`, `velofetch` и другие). После
загрузки занято 8,1 МиБ (вместе с initramfs, в котором есть и файлы для
установки), приглашение входа появляется через 1,9 с после запуска QEMU с
KVM.

Систему можно запустить на реальном компьютере с UEFI: сборка создаёт
образ для флешки `velora-usb.img`, инструкция —
[docs/hardware.md](docs/hardware.md). При панике отчёт сохраняется в файл
`velora-panic.log` на разделе EFI, `dmesg` показывает сообщения ядра.

Есть текстовый редактор `plume` (уровня nano: поиск, вырезать и вставить,
кириллица, вопрос о сохранении при выходе).

Есть сеть: драйвер virtio-net и свой стек (DHCP, ping, TCP и UDP;
команды `ip`, `ping`, `nc`, см. [docs/network.md](docs/network.md)).
Систему можно установить на диск: в Live-режиме войти как `root` и
запустить `velora-setup` (см. [installer/README.md](installer/README.md)).

Вход в Live-образе: пользователь `user` или `root`, пароль `velora`. Пароль
можно задать при сборке переменными `VELORA_ROOT_PASSWORD` и
`VELORA_USER_PASSWORD`; в образ попадают только хеши. Список команд —
`help`.
План этапов — ниже и в [docs/roadmap.md](docs/roadmap.md).

### Архитектура

- Ядро: Rust (`no_std`), монолитное, модульное. Драйверы и файловые системы
  работают в ядре за общими интерфейсами. Обоснование — в
  [docs/decisions.md](docs/decisions.md).
- Загрузчик: Limine (единственный сторонний бинарный компонент, BSD-2).
- Ядро находится в верхней половине адресного пространства (`0xffffffff80000000`),
  физическая память отображена через HHDM. Подробно — в
  [docs/memory-map.md](docs/memory-map.md).
- Будущие Lumi и поддержка .exe учтены заранее: «personality»-слой системных
  вызовов, интерфейс загрузчиков программ и framebuffer-интерфейс. См.
  [docs/architecture.md](docs/architecture.md).

```
boot/       конфигурация загрузчика
kernel/     ядро (arch, boot, console, mm, sched, syscall, drivers, ...);
            kernel/mm, kernel/acpi, kernel/abi, kernel/net — библиотеки
drivers/    аппаратно-независимые части драйверов (input, pci, rtc)
fs/         vfs (интерфейс и монтирование), velorafs, fat (FAT16/FAT32), gpt, tar (initramfs)
libc/       vlibc — библиотека для программ; auth (пароли), shell (язык vsh), editor (Plume)
userland/   программы: login, vsh, plume, команды (coreutils), hello, vtest
installer/  velora-setup — установщик
tools/      mkiso (сборка ISO), mkusb (образ для флешки), mkinitrd (initramfs), mkpasswd (хеши паролей), kstrip, fsck (проверка образов), fontgen
tests/      запуск QEMU и проверки
docs/       документация
```

### Сборка и запуск

Нужно: Rust (rustup; версия закреплена в `rust-toolchain.toml`), curl, unzip,
sha256sum, bash. Для тестов — QEMU и Python 3. Работает в Linux и в Windows
(Git Bash).

```sh
./build.sh          # собирает velora.iso и velora-usb.img
./test.sh           # юнит-тесты, тесты ядра в QEMU, тест загрузки
./test.sh --host    # только юнит-тесты на хосте
```

Запуск в QEMU (UEFI):

```sh
qemu-system-x86_64 -M q35 -m 256M \
  -drive if=pflash,format=raw,readonly=on,file=/usr/share/qemu/edk2-x86_64-code.fd \
  -cdrom velora.iso -serial stdio
```

В Windows прошивка лежит в `C:\Program Files\qemu\share\edk2-x86_64-code.fd`.
Для дисков virtio-blk нужен QEMU 9.1 или новее (с 8.2 есть известная
проблема, #19).

### Дальнейшие этапы

| Этап | Содержание | Версия |
|---|---|---|
| 1 | Загрузка, консоль, исключения, паника | v0.1.0 (готово) |
| 2 | Физический аллокатор, страничная память, куча | v0.2.0 (готово) |
| 3 | APIC, таймер, клавиатура PS/2 | v0.3.0 (готово) |
| 4 | Процессы, потоки, ring 3, системные вызовы | v0.4.0 (готово) |
| 5 | Загрузчик ELF, интерфейс для PE, initramfs, vlibc | v0.5.0 (готово) |
| 6 | PCI, virtio-blk, AHCI, NVMe, RTC | v0.6.0 (готово) |
| 7 | VFS, FAT32, VeloraFS с журналом | v0.7.0 (готово) |
| 8 | vsh, команды, пользователи и вход, velofetch | v0.8.0 (готово) |
| 9 | Установщик velora-setup | v0.9.0 (готово) |
| 10 | virtio-net, TCP/IP | v0.10.0 (готово) |
| 11 | Текстовый редактор Plume | v0.11.0 (готово) |
| 12 | Готовность к реальному ПК: консоль GOP, HPET/TSC, журнал паники на диске, образ для флешки | v0.12.0 (готово) |
| 13 | ACPI: выключение, перезагрузка, кнопка питания | v0.13.0 (готово) |
| 14 | Многопроцессорность (SMP) | v0.14.0 |
| 15 | USB: xHCI, клавиатура, мышь, флешки | v0.15.0 |
| 16 | Сеть на реальном ПК: e1000, DNS, `fetch` | v0.16.0 |

После этого: графическая оболочка Lumi (компоновщик в пространстве
пользователя поверх framebuffer) и Win32-слой для запуска .exe (загрузчик
PE и перевод вызовов Win32 в системные вызовы Velora).

### Лицензия

GPL-3.0-or-later, см. [LICENSE](LICENSE). Библиотека для программ vlibc и
crate, код которых в неё входит (`kernel/abi`, `kernel/mm`), — LGPL-3.0-or-later
([libc/vlibc/LICENSE](libc/vlibc/LICENSE)). Сторонние компоненты перечислены в
[docs/third-party.md](docs/third-party.md).

---

## English

Velora OS is an x86_64 operating system written from scratch. It is not a Linux
distribution or a fork: it has its own kernel, drivers and userland. For now it
is text-mode only. The Lumi graphical shell and running Windows programs (.exe)
are planned for later stages.

### Status

Stages 1–13 are done (v0.13.0). The kernel boots via UEFI, prints to the screen
and the serial port, and handles CPU exceptions and panics with a detailed
report. It manages memory itself: a physical frame allocator, its own page
tables with W^X permissions, a heap (`alloc`) and stacks with guard pages.
Interrupts work (ACPI, APIC, 100 Hz timer), as does the PS/2 keyboard with EN
and RU layouts. There are processes and threads with a preemptive scheduler,
user mode (ring 3) with memory isolation, and system calls. Programs (ELF) are
written in Rust with the vlibc library. There are drivers for PCI, virtio-blk,
AHCI and NVMe disks and the RTC. There is a VFS with file systems: the native
VeloraFS (permissions, journal, checksums, integrity check) and FAT16/FAT32 with
read and write. The initramfs is mounted at `/`, disks under `/mnt`. Boot ends
at a login prompt; the vsh shell has history, Tab completion, pipes and
redirections, and the basic commands exist (`ls`, `cat`, `cp`, `mv`, `rm`,
`ps`, `kill`, `velofetch` and others). 8.1 MiB are in use after boot
(initramfs included, which also holds the files to install); the login prompt
appears 1.9 s after QEMU starts with KVM.

The system can run on a real UEFI computer: the build makes a USB drive
image, `velora-usb.img`; see [docs/hardware.md](docs/hardware.md). A panic
report is saved to `velora-panic.log` on the EFI partition, and `dmesg` shows
the kernel messages.

There is a text editor, `plume` (nano-like: search, cut and paste,
Cyrillic, a save question on exit).

There is networking: a virtio-net driver and a stack of our own (DHCP, ping,
TCP and UDP; commands `ip`, `ping`, `nc`, see [docs/network.md](docs/network.md)).
The system can be installed on a disk: in the Live system log in as `root`
and run `velora-setup` (see [installer/README.md](installer/README.md)).

Live image login: user `user` or `root`, password `velora`. The passwords
can be set at build time with `VELORA_ROOT_PASSWORD` and
`VELORA_USER_PASSWORD`; only hashes go into the image. `help` lists the
commands. The stage plan is below and in [docs/roadmap.md](docs/roadmap.md).

### Architecture

- Kernel: Rust (`no_std`), monolithic and modular. Drivers and filesystems run
  in the kernel behind common interfaces. Rationale in
  [docs/decisions.md](docs/decisions.md).
- Bootloader: Limine (the only third-party binary component, BSD-2).
- The kernel lives in the higher half (`0xffffffff80000000`); physical memory
  is reached through the HHDM. Details in [docs/memory-map.md](docs/memory-map.md).
- Lumi and .exe support are planned for: a syscall "personality" layer, an
  executable loader interface and a framebuffer interface. See
  [docs/architecture.md](docs/architecture.md).

The directory layout is shown in the Russian section above.

### Building and running

Requirements: Rust (rustup; the version is pinned in `rust-toolchain.toml`),
curl, unzip, sha256sum, bash. Tests also need QEMU and Python 3. Works on Linux
and on Windows (Git Bash).

```sh
./build.sh          # builds velora.iso and velora-usb.img
./test.sh           # unit tests, kernel tests in QEMU, boot test
./test.sh --host    # host unit tests only
```

Running in QEMU (UEFI):

```sh
qemu-system-x86_64 -M q35 -m 256M \
  -drive if=pflash,format=raw,readonly=on,file=/usr/share/qemu/edk2-x86_64-code.fd \
  -cdrom velora.iso -serial stdio
```

On Windows the firmware is at `C:\Program Files\qemu\share\edk2-x86_64-code.fd`.
virtio-blk disks need QEMU 9.1 or newer (there is a known problem with 8.2,
#19).

### Next stages

| Stage | Scope | Version |
|---|---|---|
| 1 | Boot, console, exceptions, panic | v0.1.0 (done) |
| 2 | Physical allocator, paging, kernel heap | v0.2.0 (done) |
| 3 | APIC, timer, PS/2 keyboard | v0.3.0 (done) |
| 4 | Processes, threads, ring 3, system calls | v0.4.0 (done) |
| 5 | ELF loader, PE loader interface, initramfs, vlibc | v0.5.0 (done) |
| 6 | PCI, virtio-blk, AHCI, NVMe, RTC | v0.6.0 (done) |
| 7 | VFS, FAT32, VeloraFS with a journal | v0.7.0 (done) |
| 8 | vsh, commands, users and login, velofetch | v0.8.0 (done) |
| 9 | velora-setup installer | v0.9.0 (done) |
| 10 | virtio-net, TCP/IP | v0.10.0 (done) |
| 11 | Plume text editor | v0.11.0 (done) |
| 12 | Ready for a real PC: GOP console, HPET/TSC, panic log on disk, USB drive image | v0.12.0 (done) |
| 13 | ACPI: power off, restart, power button | v0.13.0 (done) |
| 14 | Multiprocessing (SMP) | v0.14.0 |
| 15 | USB: xHCI, keyboard, mouse, flash drives | v0.15.0 |
| 16 | Networking on a real PC: e1000, DNS, `fetch` | v0.16.0 |

After that: the Lumi graphical shell (a user-space compositor on top of the
framebuffer) and a Win32 layer for .exe files (a PE loader plus translation
of Win32 calls into Velora system calls).

### License

GPL-3.0-or-later, see [LICENSE](LICENSE). The vlibc program library and the
crates whose code it includes (`kernel/abi`, `kernel/mm`) are LGPL-3.0-or-later
([libc/vlibc/LICENSE](libc/vlibc/LICENSE)). Third-party components are listed in
[docs/third-party.md](docs/third-party.md).
