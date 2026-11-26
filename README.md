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

Готовы этапы 1–4 (v0.4.0). Ядро загружается через UEFI, выводит текст на экран
и в последовательный порт, обрабатывает исключения процессора и паники с
подробным отчётом. Ядро управляет памятью само: аллокатор физических страниц,
свои таблицы страниц с правами W^X, куча (`alloc`), стеки с защитными
страницами. Работают прерывания (ACPI, APIC, таймер 100 Гц) и клавиатура PS/2
с раскладками EN и RU. Есть процессы и потоки с вытесняющим планировщиком,
режим пользователя (ring 3) с изоляцией памяти и системные вызовы. После
загрузки занято 5,3 МиБ. Программ в файлах и оболочки пока нет: при загрузке
запускается встроенная тестовая программа, а затем временная строка ввода с
эхом для проверки клавиатуры.
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
            kernel/mm, kernel/acpi, kernel/abi — библиотеки
drivers/    аппаратно-независимые части драйверов (input: клавиатура и раскладки)
fs/         файловые системы: velora-fat (FAT16 + LFN), позже VeloraFS
libc/       vlibc (с этапа 5)
userland/   vsh и базовые команды (с этапа 8)
installer/  velora-setup (с этапа 9)
tools/      mkiso (сборка ISO), fontgen
tests/      запуск QEMU и проверки
docs/       документация
```

### Сборка и запуск

Нужно: Rust (rustup; версия закреплена в `rust-toolchain.toml`), curl, unzip,
sha256sum, bash. Для тестов — QEMU и Python 3. Работает в Linux и в Windows
(Git Bash).

```sh
./build.sh          # собирает velora.iso
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

### Дальнейшие этапы

| Этап | Содержание | Версия |
|---|---|---|
| 1 | Загрузка, консоль, исключения, паника | v0.1.0 (готово) |
| 2 | Физический аллокатор, страничная память, куча | v0.2.0 (готово) |
| 3 | APIC, таймер, клавиатура PS/2 | v0.3.0 (готово) |
| 4 | Процессы, потоки, ring 3, системные вызовы | v0.4.0 (готово) |
| 5 | Загрузчик ELF, интерфейс для PE, initramfs, vlibc | v0.5.0 |
| 6 | PCI, virtio-blk, AHCI, NVMe, RTC | v0.6.0 |
| 7 | VFS, FAT32, VeloraFS с журналом | v0.7.0 |
| 8 | vsh, команды, пользователи и вход, velofetch | v0.8.0 |
| 9 | Установщик velora-setup | v0.9.0 |
| 10 | virtio-net, TCP/IP | v0.10.0 |

После этого: USB (xHCI), многопроцессорность, графическая оболочка Lumi
(компоновщик в пространстве пользователя поверх framebuffer) и Win32-слой
для запуска .exe (загрузчик PE и перевод вызовов Win32 в системные вызовы
Velora).

### Лицензия

GPL-3.0-or-later, см. [LICENSE](LICENSE). Сторонние компоненты перечислены в
[docs/third-party.md](docs/third-party.md).

---

## English

Velora OS is an x86_64 operating system written from scratch. It is not a Linux
distribution or a fork: it has its own kernel, drivers and userland. For now it
is text-mode only. The Lumi graphical shell and running Windows programs (.exe)
are planned for later stages.

### Status

Stages 1–4 (v0.4.0) are done. The kernel boots via UEFI, prints to the screen
and the serial port, and handles CPU exceptions and panics with a detailed
report. It manages memory itself: a physical frame allocator, its own page
tables with W^X permissions, a heap (`alloc`) and stacks with guard pages.
Interrupts work (ACPI, APIC, 100 Hz timer), as does the PS/2 keyboard with EN
and RU layouts. There are processes and threads with a preemptive scheduler,
user mode (ring 3) with memory isolation, and system calls. 5.3 MiB are in use
after boot. There are no programs in files and no shell yet: boot runs a
built-in test program and then a temporary echoing line editor to try the
keyboard. The stage plan is below and in [docs/roadmap.md](docs/roadmap.md).

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
./build.sh          # builds velora.iso
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

### Next stages

| Stage | Scope | Version |
|---|---|---|
| 1 | Boot, console, exceptions, panic | v0.1.0 (done) |
| 2 | Physical allocator, paging, kernel heap | v0.2.0 (done) |
| 3 | APIC, timer, PS/2 keyboard | v0.3.0 (done) |
| 4 | Processes, threads, ring 3, system calls | v0.4.0 (done) |
| 5 | ELF loader, PE loader interface, initramfs, vlibc | v0.5.0 |
| 6 | PCI, virtio-blk, AHCI, NVMe, RTC | v0.6.0 |
| 7 | VFS, FAT32, VeloraFS with a journal | v0.7.0 |
| 8 | vsh, commands, users and login, velofetch | v0.8.0 |
| 9 | velora-setup installer | v0.9.0 |
| 10 | virtio-net, TCP/IP | v0.10.0 |

After that: USB (xHCI), SMP, the Lumi graphical shell (a user-space compositor
on top of the framebuffer) and a Win32 layer for .exe files (a PE loader plus
translation of Win32 calls into Velora system calls).

### License

GPL-3.0-or-later, see [LICENSE](LICENSE). Third-party components are listed in
[docs/third-party.md](docs/third-party.md).
