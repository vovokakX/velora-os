# Архитектура / Architecture

[Русский](#русский) | [English](#english)

## Русский

### Общая схема

```
+---------------------------------------------------------------+
|  Пользовательские программы: vsh, команды, velora-setup, ...  |
|  (позже: Lumi, программы Win32 через Win32-слой)              |
+---------------------------------------------------------------+
|  vlibc (нативный ABI)          |  Win32-слой (позже, ring 3)  |
+---------------------------------------------------------------+
|  Personality: таблица системных вызовов процесса              |  ring 0
|  Процессы и потоки, планировщик, IPC                          |
|  VFS -> VeloraFS, FAT32, devfs, initramfs                     |
|  ExecFormat -> ELF, PE (позже)                                |
|  Память: физический аллокатор, страницы, куча                 |
|  Драйверы: PCI, virtio, AHCI, NVMe, PS/2, xHCI, RTC, UART     |
|  arch/x86_64: GDT, IDT, APIC, syscall                         |
+---------------------------------------------------------------+
|  Limine (UEFI) -> ядро в верхней половине                     |
+---------------------------------------------------------------+
```

### Тип ядра

Монолитное модульное ядро. Драйверы и файловые системы работают в ring 0,
но связаны с остальным ядром только через трейты:

| Трейт | Назначение | Реализации |
|---|---|---|
| `BlockDevice` | чтение и запись секторов | virtio-blk, AHCI, NVMe |
| `FileSystem` | монтирование, inode | VeloraFS, FAT32, initramfs, devfs |
| `ExecFormat` | разбор и загрузка программы | ELF; PE позже |
| `Display` | framebuffer | Limine framebuffer; позже драйверы GPU |
| `InputDevice` | события клавиатуры и мыши | PS/2; позже USB HID |
| `NetDevice` | отправка и приём кадров | virtio-net |

На этапе 1 есть только прообраз `Display`: `console::framebuffer::Framebuffer`.
Остальные трейты появятся на своих этапах.

### Место для Lumi

- Консоль ядра — лишь один из клиентов framebuffer. Lumi станет процессом
  пользователя, который получит framebuffer через системный вызов
  (отображение памяти) и будет сам рисовать окна.
- Ввод идёт через очередь событий (`/dev/input/*`), а не напрямую в консоль.
  Поэтому графическая оболочка сможет забрать ввод без изменения драйверов.

### Место для .exe

- **Загрузчики программ.** Ядро выбирает загрузчик по сигнатуре файла через
  `ExecFormat`. ELF реализуется на этапе 5. PE на том же этапе получает
  явно помеченную заготовку: распознавание сигнатуры `MZ`/`PE` и ошибку
  «not implemented».
- **Personality.** У каждого процесса есть поле personality, которое выбирает
  таблицу системных вызовов. Нативная таблица — Velora ABI. Для Windows-программ
  планируется отдельный путь: DLL Win32-слоя в пространстве пользователя
  (аналог ntdll и kernel32) переводят вызовы в нативные. Так совместимость
  с Windows не усложняет ядро.
- Селекторы GDT заранее расположены так, как требуют `syscall`/`sysret`.

### Загрузка

1. Прошивка UEFI загружает `EFI/BOOT/BOOTX64.EFI` (Limine) из FAT-образа,
   на который указывает каталог El Torito.
2. Limine читает `boot/limine.conf` и `boot/velora` из файловой системы ISO 9660.
3. Limine переводит процессор в long mode, отображает ядро в
   `0xffffffff80000000`, создаёт HHDM и передаёт управление в `kmain`.
4. Ядро: COM1, framebuffer-консоль, GDT/TSS, IDT, отчёт о загрузке.

### Обработка ошибок

- Каждое исключение проходит через общую заглушку `isr_common`, которая
  сохраняет все регистры в `InterruptFrame`.
- Неустранимое исключение печатает отчёт и останавливает процессор.
  Double fault, NMI и machine check работают на отдельных стеках (IST),
  поэтому отчёт выводится даже при переполнении стека.
- Механизм восстановления (`arm_recovery`) позволяет продолжить работу после
  ожидаемого исключения. Сейчас его используют тесты, позже на нём будет
  построено безопасное копирование из памяти пользователя.

### Прерывания и время (этап 3)

| Вектор | Источник |
|---|---|
| 0x00–0x1f | исключения процессора |
| 0x20 | таймер локального APIC, 100 Гц |
| 0x21 | клавиатура (ISA IRQ 1 через IOAPIC) |
| 0x30–0x6f | прерывания устройств (с этапа 6) |
| 0x70–0x7f | ложные прерывания от замаскированных 8259 (игнорируются) |
| 0xfe | ошибка APIC |
| 0xff | ложное прерывание APIC |

- `arch/x86_64/irq.rs`: таблица обработчиков по векторам; EOI отправляется
  после обработчика, ложные прерывания не подтверждаются.
- `acpi.rs` находит MADT, `ioapic.rs` направляет GSI на векторы с учётом
  переназначений.
- `time.rs`: время с загрузки по TSC, тики, `sleep_ms` (CPU спит в `hlt`).
- Ввод: драйвер PS/2 → `velora-input` → очередь событий ядра (`input.rs`).
  Временный потребитель — строка ввода `kbdtest.rs`, на этапе 8 её заменит vsh.

### Процессы и потоки (этап 4)

- `sched/`: потоки (своё ядро стека 32 КиБ, сохранённые регистры, область
  FXSAVE 512 байт), процессы (адресное пространство, список потоков, статус
  завершения, personality), очереди ожидания.
- Переключение (`arch/x86_64/context.rs`): сохраняются callee-saved регистры,
  меняется RSP; при смене процесса меняется CR3; TSS.rsp0 и стек для
  `syscall` указывают на стек нового потока; регистры SSE сохраняются
  и восстанавливаются.
- Половина ядра одинакова во всех адресных пространствах: верхние 256 записей
  PML4 копируются из таблиц ядра и больше не меняются.
- Вход в ring 3 — через `iretq`, возврат из системных вызовов — через `sysretq`.
  GS указывает на данные процессора в ядре (`swapgs` на входе и выходе).
- Ошибка процессора в программе завершает только эту программу.

### Программы из файлов (этап 5)

- Корневая файловая система Live-режима — initramfs: архив ustar,
  который Limine загружает как модуль (`boot/initramfs.tar`). С этапа 7 VFS
  монтирует его в `/` только для чтения (`fs/tarfs.rs`, разбор в
  `velora-tar`).
- `exec.rs`: интерфейс `ExecFormat` с двумя реализациями. ELF64 загружается
  полностью (разбор и проверка в `velora-exe`). PE распознаётся, но не
  запускается: ядро отвечает, что нужен Win32-слой.
- Начальный стек по System V: argc, argv, envp, auxv (`AT_PAGESZ`, `AT_ENTRY`).
- `libc/vlibc` — библиотека для программ: `_start`, системные вызовы, вывод,
  аргументы и окружение, куча (тот же аллокатор, что в ядре), процессы,
  небольшой C API.
- `userland/` — отдельный workspace программ, собирается своими флагами.

### Драйверы (этап 6)

- PCI: `velora-pci` перечисляет шину (с мостами), определяет BAR и
  capabilities через абстрактный `ConfigAccess`. Ядро реализует его через
  ECAM (таблица MCFG) или порты 0xCF8/0xCFC.
- Диски реализуют `BlockDevice` (`drivers/block.rs`): virtio-blk (только
  современный virtio 1.x), AHCI (диски SATA, ATAPI пропускается), NVMe
  (namespace 1). Имена: `vd0`, `sd0`, `nvme0n1`.
- Обмен с устройствами идёт через буферы DMA (`drivers/dma.rs`, непрерывная
  физическая память), окончание команды определяется опросом (решение D21).
- RTC читается при загрузке, дальше время UTC считается по TSC
  (`time::realtime_ns`).

### Файловые системы (этап 7)

- `velora-vfs` (`fs/vfs`): интерфейс `FileSystem` (работает с номерами
  inode), `BlockIo` (секторы устройства), таблица монтирования, разбор путей,
  проверка прав POSIX, файловая система в памяти `MemFs` и общий набор
  проверок `conformance`, который проходят все файловые системы.
- `velorafs` (`fs/velorafs`): своя файловая система — экстенты, права,
  журнал метаданных, CRC32C, fsck. Формат — [velorafs.md](velorafs.md).
- `velora-fat` (`fs/fat`): тома FAT16 и FAT32, чтение и запись с длинными
  именами, форматирование FAT32. Нужна для раздела EFI (этап 9).
- Все три crate — `no_std` и проверяются на хосте; ядро только подключает
  их к своим дискам (`fs/disk.rs`).
- В ядре (`kernel/src/fs/`): `/` — initramfs, `/dev` — devfs (null, zero,
  console, диски), `/tmp` — `MemFs`, `/mnt/<диск>` — каждый диск с VeloraFS
  или FAT, найденный при загрузке.
- VFS защищена одной блокировкой с выключенными прерываниями; операция
  выполняется целиком (решение D25).
- У процесса таблица дескрипторов (до 64) и учётные данные (uid, gid).
  Дескрипторы 0–2 открыты на консоли. Программы загружаются через VFS с
  проверкой бита `x`.

### Пользовательское окружение (этап 8)

- После загрузки ядро запускает `/bin/login` и перезапускает его после
  каждого сеанса. `login` проверяет пароль по `/etc/passwd` и
  `/etc/shadow` (хеши PBKDF2-HMAC-SHA-256, `libc/auth`), создаёт домашний
  каталог, вызывает `setuid` и запускает оболочку.
- Терминал консоли (`kernel/src/tty.rs`): строчный режим с эхом, режим без
  эха для паролей и «сырой» режим для vsh. Ctrl+C обрабатывает отдельный
  поток ядра (решение D29).
- Каналы (`fs/pipe.rs`), `dup`/`dup2`, рабочий каталог. `spawn` передаёт
  потомку только дескрипторы 0–2 (решение D31); fork нет.
- vsh (`userland/vsh`): разбор языка и редактор строки — в `libc/shell`
  с тестами на хосте. Команды — `userland/coreutils` (по программе на
  команду, общий код в библиотеке crate).
- Выключение и перезагрузка — `kernel/src/power.rs` (ACPI S5 по данным из
  DSDT, регистр сброса ACPI, порт 0xCF9, контроллер клавиатуры).
- В Live-режиме `/home` и `/root` — файловые системы в памяти (D32).

### Установка (этап 9)

- `velora-gpt` (`fs/gpt`) читает и пишет GPT. Ядро при загрузке
  регистрирует разделы как устройства `<диск>p<N>` (`drivers/block.rs`).
- Установленная система: раздел EFI (FAT32) с Limine, ядром и
  `limine.conf`; корень — раздел VeloraFS, его GUID передаётся ядру
  параметром `velora.root=PARTUUID=...`. initramfs не нужен (D33).
- `installer/velora-setup` пишет GPT, FAT32 и VeloraFS через `/dev/<диск>`
  теми же crate, что ядро (D34). Ядро и Limine для установки лежат в
  initramfs Live-образа в `/usr/lib/velora`.

## English

### Overview

See the diagram above. Layers from the bottom: Limine boots the higher-half
kernel; `arch/x86_64` sets up the CPU; drivers, memory management, the VFS,
executable loaders and the scheduler form the kernel; a per-process
personality selects the system call table; vlibc (native ABI) and, later, a
Win32 layer sit in user space.

### Kernel type

A monolithic, modular kernel. Drivers and filesystems run in ring 0 but talk to
the rest of the kernel only through traits (`BlockDevice`, `FileSystem`,
`ExecFormat`, `Display`, `InputDevice`, `NetDevice`; see the table above).
Stage 1 only has the first form of `Display`:
`console::framebuffer::Framebuffer`.

### Room for Lumi

- The kernel console is just one framebuffer client. Lumi will be a user
  process that maps the framebuffer through a system call and draws windows
  itself.
- Input goes through an event queue (`/dev/input/*`), not straight into the
  console, so a graphical shell can take over input without driver changes.

### Room for .exe

- **Executable loaders.** The kernel picks a loader by file signature through
  `ExecFormat`. ELF is implemented in stage 5. PE gets an explicitly marked
  stub in the same stage: it recognises `MZ`/`PE` and returns
  "not implemented".
- **Personality.** Every process has a personality that selects its system call
  table. The native table is the Velora ABI. Windows programs will go through a
  Win32 layer in user space (DLLs playing the role of ntdll and kernel32) that
  translates calls to native ones, so Windows compatibility does not complicate
  the kernel.
- GDT selectors are already laid out the way `syscall`/`sysret` require.

### Boot

1. UEFI firmware loads `EFI/BOOT/BOOTX64.EFI` (Limine) from the FAT image the
   El Torito catalog points to.
2. Limine reads `boot/limine.conf` and `boot/velora` from the ISO 9660 filesystem.
3. Limine enters long mode, maps the kernel at `0xffffffff80000000`, sets up
   the HHDM and jumps to `kmain`.
4. The kernel sets up COM1, the framebuffer console, GDT/TSS and IDT, then
   prints a boot report.

### Error handling

- Every exception goes through the common `isr_common` stub, which saves all
  registers into an `InterruptFrame`.
- A fatal exception prints a report and halts. Double fault, NMI and machine
  check run on separate IST stacks, so the report is printed even after a
  stack overflow.
- The recovery hook (`arm_recovery`) lets the kernel continue after an expected
  exception. Tests use it now; later it will back safe copies from user memory.

### Interrupts and time (stage 3)

See the vector table in the Russian section. `arch/x86_64/irq.rs` keeps a
per-vector handler table and sends EOI after the handler (spurious interrupts
are not acknowledged); `acpi.rs` finds the MADT and `ioapic.rs` routes GSIs
to vectors honouring overrides; `time.rs` provides TSC-based time since boot,
ticks and `sleep_ms` (the CPU halts in `hlt`). Input flows from the PS/2
driver through `velora-input` into the kernel event queue (`input.rs`); the
temporary consumer is the `kbdtest.rs` line editor, replaced by vsh in stage 8.

### Processes and threads (stage 4)

`sched/` holds threads (own 32 KiB kernel stack, saved registers, a 512-byte
FXSAVE area), processes (address space, thread list, exit status,
personality) and wait queues. A switch (`arch/x86_64/context.rs`) saves the
callee-saved registers and swaps RSP, reloads CR3 when the process changes,
points TSS.rsp0 and the `syscall` stack at the new thread and saves/restores
the SSE registers. The kernel half is identical in every address space (the
upper 256 PML4 entries are copied from the kernel tables and never change).
Ring 3 is entered with `iretq`; system calls return with `sysretq`; GS points at
per-CPU data in the kernel (`swapgs` on entry and exit). A CPU exception in a
program ends only that program.

### Programs from files (stage 5)

The Live root file system is, for now, the initramfs: a ustar archive Limine
loads as a module (`boot/initramfs.tar`), read-only (`initramfs.rs`, parsing in
`velora-tar`). `exec.rs` defines the `ExecFormat` interface with two
implementations: ELF64 loads fully (parsing and validation in `velora-exe`);
PE is recognised but not run, and the kernel reports that the Win32 layer is
needed. The initial stack follows System V: argc, argv, envp, auxv
(`AT_PAGESZ`, `AT_ENTRY`). `libc/vlibc` is the program library (`_start`,
system calls, output, arguments and environment, a heap using the kernel's
allocator, processes, a small C API); `userland/` is a separate workspace of
programs built with their own flags.

### Drivers (stage 6)

PCI: `velora-pci` enumerates the bus (following bridges) and decodes BARs and
capabilities over an abstract `ConfigAccess`, which the kernel implements with
ECAM (MCFG table) or ports 0xCF8/0xCFC. Disks implement `BlockDevice`
(`drivers/block.rs`): virtio-blk (modern virtio 1.x only), AHCI (SATA disks,
ATAPI skipped), NVMe (namespace 1), named `vd0`, `sd0`, `nvme0n1`. Transfers use
DMA buffers (`drivers/dma.rs`, physically contiguous memory); completion is
polled (decision D21). The RTC is read at boot and UTC time then advances with
the TSC (`time::realtime_ns`).

### File systems (stage 7)

`velora-vfs` (`fs/vfs`) defines the `FileSystem` interface (inode numbers),
`BlockIo` (device sectors), the mount table, path handling, POSIX permission
checks, the in-memory `MemFs` and the shared `conformance` suite every file
system passes. `velorafs` (`fs/velorafs`) is the native file system: extents,
permissions, a metadata journal, CRC32C, fsck; the format is in
[velorafs.md](velorafs.md). `velora-fat` (`fs/fat`) reads and writes FAT16 and
FAT32 volumes with long names and formats FAT32 (needed for the EFI partition
in stage 9). All three are `no_std` and tested on the host; the kernel only
connects them to its disks (`fs/disk.rs`). In the kernel (`kernel/src/fs/`):
`/` is the initramfs (read-only since stage 7), `/dev` is devfs (null, zero,
console, disks), `/tmp` is a `MemFs`, and every disk with VeloraFS or FAT found
at boot is mounted at `/mnt/<disk>`. The VFS is behind one lock with interrupts
off and each operation runs to completion (decision D25). Every process has a
descriptor table (up to 64) and credentials (uid, gid); descriptors 0–2 start
on the console. Programs are loaded through the VFS and need an `x` bit.

### User environment (stage 8)

After boot the kernel starts `/bin/login` and starts it again after every
session. `login` checks the password against `/etc/passwd` and `/etc/shadow`
(PBKDF2-HMAC-SHA-256 hashes, `libc/auth`), creates the home directory, calls
`setuid` and starts the shell. The console terminal (`kernel/src/tty.rs`) has
a line mode with echo, a no-echo mode for passwords and a raw mode for vsh;
Ctrl+C is handled by a kernel thread (decision D29). There are pipes
(`fs/pipe.rs`), `dup`/`dup2` and a working directory; `spawn` passes only
descriptors 0–2 to the child (decision D31), there is no fork. vsh
(`userland/vsh`) keeps its parser and line editor in `libc/shell`, tested on
the host; the commands are in `userland/coreutils` (one program per command,
shared code in the crate's library). Power off and restart are in
`kernel/src/power.rs` (ACPI S5 from the DSDT, the ACPI reset register, port
0xCF9, the keyboard controller). In the Live system `/home` and `/root` are
file systems in memory (D32).

### Installation (stage 9)

`velora-gpt` (`fs/gpt`) reads and writes GPT; at boot the kernel registers
partitions as devices `<disk>p<N>` (`drivers/block.rs`). An installed system
has an EFI partition (FAT32) with Limine, the kernel and `limine.conf`, and a
VeloraFS root partition whose GUID reaches the kernel as
`velora.root=PARTUUID=...`; no initramfs is needed (D33).
`installer/velora-setup` writes GPT, FAT32 and VeloraFS through `/dev/<disk>`
with the same crates the kernel uses (D34). The kernel and Limine to install
are in the Live image's initramfs under `/usr/lib/velora`.
