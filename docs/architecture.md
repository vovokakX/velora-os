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
| 0x22 | SCI — события ACPI (с этапа 13) |
| 0xe0 | перепланирование — межпроцессорное прерывание (с этапа 14) |
| 0xf1 | пробуждение процессора, ждущего большую блокировку ядра (с этапа 14) |
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

### Сеть (этап 10)

- Драйвер virtio-net, стек `kernel/net` (`velora-net`, без ввода-вывода,
  тесты на хосте), поток ядра `net`, сокеты как файловые дескрипторы.
  Подробно — [network.md](network.md), решения D36–D37.

### Текстовый редактор (этап 11)

- `libc/editor` (`velora-editor`) — вся логика Plume без ввода-вывода:
  декодер клавиш, буфер строк (UTF-8, LF или CRLF), команды, подсказки,
  вопросы и отрисовка кадра. Проверяется на хосте. Запись файлов идёт
  через трейт `Io`, поэтому тесты подставляют файлы в памяти.
- `userland/plume` переводит консоль в «сырой» режим, читает клавиши,
  выводит разницу между кадрами (`Screen`, только изменившиеся строки)
  и сохраняет файлы через vlibc с `sync` после записи (D38).
- Консоль ядра для этого рисует курсор (подчёркивание), понимает
  инверсию (SGR 7/27), скрытие курсора (`ESC[?25l`/`ESC[?25h`) и
  `ESC[J`. Символ UTF-8, разорванный между двумя вызовами `write`,
  собирается в `tty::write`.

### Готовность к реальному ПК (этап 12)

- Таймеры: TSC и таймер APIC калибруются по лучшим доступным опорным
  часам: HPET (таблица HPET, `arch/x86_64/hpet.rs`), затем таймер ACPI PM
  (порт из FADT, `pmtimer.rs`), затем PIT. Если источник не считает, берётся
  следующий; паника — только если не работает ни один (D40). В журнале
  загрузки видно, по чему шла калибровка и инвариантен ли TSC.
- Консоль: шрифт 8×16 увеличивается в 2 раза на экранах шириной от 2560
  точек, параметр `velora.fontscale=1..4` задаёт масштаб явно.
- Журнал ядра: последние 64 КиБ вывода консоли в статическом кольцевом
  буфере (`console/klog.rs`), читается через `/dev/kmsg` (`dmesg`).
- Журнал паники (`crashlog.rs`, D39): файл `velora-panic.log` на томе FAT,
  секторы которого известны заранее; при панике и фатальном исключении
  ядро пишет в них журнал напрямую через драйвер диска.
- Образ для флешки `velora-usb.img` (`tools/mkusb`): GPT и раздел EFI
  FAT32 с теми же файлами, что в ISO (D41). Инструкция —
  [hardware.md](hardware.md).

### ACPI: питание (этап 13)

- `power.rs` при загрузке включает режим ACPI (SMI_CMD/ACPI_ENABLE), если
  прошивка его не включила, выключает все события общего назначения (GPE),
  направляет SCI на вектор 0x22 и включает фиксированное событие кнопки
  питания. Нажатие будит поток ядра `acpi`, который записывает данные на
  диски и выключает машину (D42).
- Выключение: S5 из DSDT или SSDT (разбор `_S5` без интерпретатора AML,
  все кодировки целых), запись SLP_TYP в PM1_CNT с сохранением остальных
  битов или в регистр SLEEP_CONTROL при ACPI с аппаратным сокращением.
  Расширенные адреса FADT (X_PM1*, X_GPE*) используются, если это порты.
- Перезагрузка: регистр сброса ACPI в портах, памяти или конфигурации
  PCI, затем 0xCF9, контроллер клавиатуры и тройная ошибка.

### Многопроцессорность (этап 14)

- `smp.rs`: остальные процессоры (AP) запускаются через запрос MP Limine.
  AP сразу переходит на таблицы страниц ядра и свой стек (память
  загрузчика освобождается рано), ждёт, затем настраивает свои GDT, TSS
  со стеками IST, данные процессора (`percpu`, через GS), локальный APIC
  и таймер и становится своим потоком простоя.
- Большая блокировка ядра (BKL): билетная, рекурсивная для процессора.
  Каждый вход в ядро (системный вызов, прерывание, исключение) берёт её,
  возврат в режим пользователя отдаёт. Код ядра выполняется на одном
  процессоре за раз, программы — на всех параллельно. Глубина вложенности
  сохраняется в потоке при переключении. На прерывании, пришедшем с
  включёнными прерываниями (точка, где ядро и так может быть вытеснено),
  блокировка отдаётся ждущим процессорам (D43).
- Планировщик: общая очередь, у каждого процессора свои текущий поток,
  поток простоя и квант. Все потоки процесса работают на одном процессоре,
  выбранном при запуске (наименее загруженном); потоки ядра — на любом.
  Пробуждение потока будит нужный процессор прерыванием 0xE0.
  Процессор, который долго ждёт блокировку, не крутится в цикле, а
  останавливается (`hlt`) с повышенным приоритетом APIC; при освобождении
  его будит прерывание 0xF1. В виртуальной машине крутящиеся процессоры
  отнимали время хоста у эмулируемых устройств.
- Сброс TLB на других процессорах не нужен: память процесса бывает в TLB
  только его процессора, а страницы ядра после снятия не переиспользуются
  с другим содержимым (стеки ядра кешируются отображёнными).
- Паника и фатальное исключение останавливают остальные процессоры через
  NMI.

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

### Network (stage 10)

A virtio-net driver, the `kernel/net` stack (`velora-net`, without I/O of its
own, tested on the host), the `net` kernel thread, and sockets as file
descriptors. Details in [network.md](network.md), decisions D36–D37.

### Text editor (stage 11)

`libc/editor` (`velora-editor`) holds all of Plume's logic without I/O: the key
decoder, the line buffer (UTF-8, LF or CRLF), commands, hints, questions and
frame drawing. It is tested on the host; files are written through the `Io`
trait, so tests use files in memory. `userland/plume` puts the console in raw
mode, reads keys, prints the difference between frames (`Screen`, changed rows
only) and saves files through vlibc with a `sync` after writing (D38). For
this the kernel console draws a cursor (an underline), understands reverse
video (SGR 7/27), cursor hiding (`ESC[?25l`/`ESC[?25h`) and `ESC[J`; a UTF-8
character split between two `write` calls is put back together in
`tty::write`.

### Ready for a real PC (stage 12)

The TSC and the APIC timer are calibrated against the best reference clock
available: the HPET (HPET table, `arch/x86_64/hpet.rs`), then the ACPI PM
timer (port from the FADT, `pmtimer.rs`), then the PIT; a source that does not
count is skipped, and the kernel panics only if none works (D40). The boot log
names the reference and whether the TSC is invariant. The 8×16 console font is
doubled on screens at least 2560 pixels wide; `velora.fontscale=1..4` sets the
scale. The kernel log keeps the last 64 KiB of console output in a static ring
buffer (`console/klog.rs`), readable through `/dev/kmsg` (`dmesg`). The panic
log (`crashlog.rs`, D39) is `velora-panic.log` on a FAT volume whose sectors
are known in advance; on a panic or a fatal exception the kernel writes the
log straight to them through the disk driver. `velora-usb.img`
(`tools/mkusb`) is a GPT disk with a FAT32 EFI partition holding the same files
as the ISO (D41); see [hardware.md](hardware.md).

### ACPI power (stage 13)

At boot `power.rs` switches the firmware to ACPI mode (SMI_CMD/ACPI_ENABLE)
when needed, turns off all general-purpose events (GPEs), routes the SCI to
vector 0x22 and enables the fixed power button event; a press wakes the `acpi`
kernel thread, which flushes the disks and powers off (D42). Power off takes S5
from the DSDT or an SSDT (`_S5` parsed without an AML interpreter, every
integer encoding) and writes SLP_TYP to PM1_CNT keeping the other bits, or to
the SLEEP_CONTROL register on hardware-reduced ACPI; extended FADT addresses
(X_PM1*, X_GPE*) are used when they are I/O ports. Restart uses the ACPI reset
register in I/O, memory or PCI configuration space, then 0xCF9, the keyboard
controller and a triple fault.

### Multiprocessing (stage 14)

`smp.rs` starts the application processors (APs) through the Limine MP
request. An AP moves to the kernel's page tables and its own stack at once
(bootloader memory is freed early), waits, then sets up its own GDT, TSS with
IST stacks, per-CPU data (`percpu`, through GS), local APIC and timer, and
becomes its own idle thread. The big kernel lock (BKL) is a ticket lock,
recursive per CPU: every entry into the kernel (system call, interrupt,
exception) takes it and every return to user mode releases it. Kernel code
runs on one CPU at a time, programs on all CPUs in parallel; the nesting depth
is saved per thread across switches. On an interrupt that arrived with
interrupts enabled (a point where the kernel may be preempted anyway) the lock
is handed to waiting CPUs (D43). The scheduler keeps one queue; every CPU has
its own current thread, idle thread and time slice. All threads of a process
run on one CPU chosen at start (the least loaded); kernel threads run on any.
Waking a thread interrupts the right CPU with vector 0xE0. A CPU that waits long for the lock halts with its APIC priority raised and is woken by vector 0xF1 when the lock is released, so waiting CPUs do not spin (in a virtual machine a spinning CPU takes host time from the emulated devices). No cross-CPU TLB
shootdown is needed: a process's memory is only in its CPU's TLB, and kernel
pages are never remapped to other contents after use (kernel stacks are cached
mapped). A panic or a fatal exception stops the other CPUs with an NMI.

### USB (этап 15) / USB (stage 15)

- `drivers/usb` (`velora-usb`, тесты на хосте): запросы и дескрипторы,
  отчёты клавиатуры и мыши HID boot protocol, перевод клавиш HID в
  скан-коды PS/2 набора 1 (USB-клавиатура идёт через тот же декодер и
  раскладки EN/RU), CBW/CSW и команды SCSI для флешек.
- `kernel/src/drivers/xhci.rs`: передача контроллера от BIOS, сброс,
  кольца команд и событий, сброс портов, Enable Slot / Address Device /
  Configure Endpoint, управляющие, массовые и прерывающие передачи. Всё
  опросом (D21, D44): команды и передачи флешки ждут своего события, поток
  `usb` раз в тик читает отчёты клавиатуры и мыши.
- Флешка — блочное устройство `usbN`; разделы (`usbNp1`) и FAT32
  монтируются как у других дисков, на разделе EFI Velora хранится журнал
  паники. Мышь — `/dev/mouse` (пакеты по 4 байта).

`drivers/usb` (`velora-usb`, host-tested) has requests and descriptors, HID
boot protocol keyboard and mouse reports, HID key to PS/2 set 1 scancode
translation (a USB keyboard uses the same decoder and EN/RU layouts) and
CBW/CSW plus SCSI commands for flash drives. `kernel/src/drivers/xhci.rs` does
the BIOS handoff, reset, command and event rings, port reset, Enable Slot /
Address Device / Configure Endpoint and control, bulk and interrupt transfers,
all polled (D21, D44): commands and drive transfers wait for their event, and
the `usb` thread reads keyboard and mouse reports every tick. A flash drive is
block device `usbN`; its partitions (`usbNp1`) and FAT32 are mounted like any
disk's, and a Velora EFI partition keeps the panic log. The mouse is
`/dev/mouse` (4-byte packets).

### Изображения в терминале / Images in the terminal

- `kernel/gfx` (`velora-gfx`, тесты на хосте): протокол Kitty (разбор
  команд по байту, склейка частей `m=1`, ответы, запрос поддержки, фильтр
  строк APC), base64, изображения RGBA с предумноженной альфой,
  масштабирование усреднением по площади, декодер PNG и inflate для
  программ.
- `kernel/src/console/text.rs`: состояние APC в разборе escape-
  последовательностей; изображение рисуется в ячейках у курсора, экран
  прокручивается, если оно не помещается; ответы на запросы копятся и
  через `tty::write` становятся вводом программы. COM1 и `dmesg` получают
  текст без изображений (D46).
- `velofetch` проверяет, что ввод и вывод — консоль, есть экран
  (`tty(3)`) и терминал отвечает на запрос, и тогда показывает
  `assets/velora-logo.png`: фон картинки становится прозрачным и
  обрезается, знак занимает высоту сводки (13 строк вместе с палитрой
  цветов). Иначе — ASCII-логотип
  (также `velofetch --ascii` и вывод в канал).

`kernel/gfx` (`velora-gfx`, host-tested) has the Kitty protocol (byte-by-byte
command parsing, joining `m=1` chunks, answers, the support query, an APC
filter), base64, premultiplied RGBA images, area-average scaling, and a PNG
decoder with inflate for programs. `kernel/src/console/text.rs` has an APC
state in its escape parser; an image is drawn in cells at the cursor, and the
screen scrolls first if it does not fit; answers to queries are collected and
become program input through `tty::write`. COM1 and `dmesg` get the text
without images (D46). `velofetch` checks that input and output are the
console, there is a screen (`tty(3)`) and the terminal answers the query, and
then shows `assets/velora-logo.png`: the picture's background is made
transparent and cut away, and the mark takes the height of the summary (13
rows with the colour palette). Otherwise it prints the ASCII logo (also with
`velofetch --ascii` and into a pipe).

### Графическая основа velora-ui / Graphical toolkit velora-ui

- `libc/ui` (`velora-ui`, тесты на хосте): холст с альфа-смешиванием,
  скруглённые прямоугольники, круги, дуги, линии и мягкие тени через
  функции расстояния в фиксированной точке (1/256 пикселя, без плавающей
  точки), градиенты с упорядоченным дизерингом; текст Inter через fontdue с
  кешем глифов; значки из примитивов; картинки PNG (`velora-gfx`); указатели
  мыши из `assets/cursors`; темы (графитовая и светлая), фон с волнами и
  окна (`decor`). Элементы в немедленном режиме (`Ui`, `Ctx`): кнопка,
  переключатель, радиокнопки, выпадающий список, список выбора, поле ввода,
  полоса и кольцо прогресса. Фокус — Tab/Shift+Tab по порядку рисования,
  Enter/Пробел, стрелки, Escape. Клавиши подаются по одной на кадр.
- Модуль `screen` (feature `velora`): `display` отображает framebuffer,
  программа рисует в холст (двойная буферизация) и копирует изменённую
  часть; указатель мыши рисуется только при копировании. Ввод —
  `/dev/keyboard` и `/dev/mouse`.
- Ядро: `display.rs` (владелец экрана, отображение с write-combining,
  возврат экрана консоли после выхода владельца); консоль в это время
  рисует только в копию в памяти. Мышь PS/2 на втором порту 8042 (с
  колесом), разбор пакетов — `velora-input::ps2mouse`.
- `/bin/uidemo` показывает все элементы; тест `boot` запускает его, снимает
  экран (`build/logs/uidemo-*.png`), нажимает клавиши, двигает мышь и
  проверяет, что консоль вернулась. `cargo run -p velora-ui --example render`
  рисует ту же галерею на хосте.

`libc/ui` (`velora-ui`, host-tested) has a canvas with alpha blending; rounded
rectangles, circles, arcs, lines and soft shadows from fixed-point distance
functions (1/256 pixel, no floating point); gradients with ordered dithering;
Inter text through fontdue with a glyph cache; icons built from primitives;
PNG pictures (`velora-gfx`); mouse pointers from `assets/cursors`; themes
(graphite and light), the wavy backdrop and windows (`decor`). Widgets are
immediate-mode (`Ui`, `Ctx`): button, switch, radio buttons, drop-down list,
choice list, text field, progress bar and ring. Focus moves with Tab/Shift+Tab
in drawing order; Enter/Space, arrows and Escape work; keys are fed one per
frame. The `screen` module (feature `velora`) maps the frame buffer with
`display`; the program draws into the canvas (double buffering) and copies the
changed part; the mouse pointer is drawn only on the way to the screen. Input
comes from `/dev/keyboard` and `/dev/mouse`. In the kernel, `display.rs` keeps
the screen's owner, maps the frame buffer write-combining and gives the screen
back to the console after the owner exits; meanwhile the console draws only
into its RAM copy. A PS/2 mouse on the 8042's second port (with a wheel) is
supported; packets are decoded by `velora-input::ps2mouse`. `/bin/uidemo`
shows every widget; the `boot` test runs it, takes screenshots
(`build/logs/uidemo-*.png`), presses keys, moves the mouse and checks that the
console comes back. `cargo run -p velora-ui --example render` draws the same
gallery on the host.

### Стартовое меню и установщик / Start menu and installer

- `installer/core` (`velora-install`, тесты на хосте): план установки,
  проверки, разметка GPT (автоматическая и с заданными размерами), FAT32 с
  Limine и ядром, VeloraFS с программами и шрифтами, пользователи,
  `/etc/velora/*.conf`. Прогресс — этапы «Копирование файлов», «Установка
  загрузчика», «Настройка системы», «Завершение». Модуль `live` (feature
  `velora`): диски `/dev` с моделью (`diskinfo`) и файлы Live-системы.
- `installer/gui` (`velora-installer-gui`, тесты на хосте): экраны
  установщика и меню как функции состояния (`App`, `Chooser`).
- `userland/start` (`/bin/velora-start`): ядро запускает его в
  Live-режиме перед `login`; меню, графический установщик (`--install`
  открывает его сразу), запуск `velora-setup`, текстовое меню без экрана.
  Строки `velora-start: ...` на последовательном порту отмечают экраны.
- Тест `gui` в `tests/qemu_run.py`: меню, все экраны клавиатурой, режим
  «Эксперт» с EFI 128 МиБ, установка на образ, загрузка с диска и проверка
  настроек; снимки каждого экрана (`--screens`, `--resolution`,
  `--screens-only`). Остальные тесты Live-образа выбирают в меню «3».

`installer/core` (`velora-install`, host-tested) holds the plan, its checks,
GPT partitioning (automatic or with given sizes), FAT32 with Limine and the
kernel, VeloraFS with the programs and fonts, users and `/etc/velora/*.conf`;
progress comes in the phases "copying files", "installing the bootloader",
"setting up the system", "finishing". Its `live` module (feature `velora`)
lists `/dev` disks with their model (`diskinfo`) and reads the Live system's
files. `installer/gui` (`velora-installer-gui`, host-tested) has the
installer and menu screens as functions of their state (`App`, `Chooser`).
`userland/start` (`/bin/velora-start`) is run by the kernel in the Live
system before `login`: the menu, the graphical installer (`--install` opens
it directly), `velora-setup`, and a text menu without a screen; lines
`velora-start: ...` on the serial port mark the screens. The `gui` test in
`tests/qemu_run.py` goes through the menu and every screen with the keyboard,
picks Expert mode with a 128 MiB EFI partition, installs onto an image, boots
from it and checks the settings, saving a screenshot of every screen
(`--screens`, `--resolution`, `--screens-only`). The other tests of the Live
image choose "3" in the menu.

### Чёткость, акцент, анимации / Sharpness, accent, animations

- Масштаб окна установщика — ступени 1x…4x (`installer/gui::SCALES`),
  кадр рисуется в родном разрешении экрана (D49). Текст — смешивание в
  линейном свете и привязка высоты строчных букв к пикселям
  (`Fonts::snapped_size`).
- Темы: тёмная, светлая и полуночная, у каждой акцентный цвет
  (`Theme::with_accent`, по умолчанию `theme::AMBER`); `Theme::mix`
  смешивает две темы.
- `libc/ui/src/anim.rs`: кривые ease-in-out, `Tweens` (значения, которые
  плавно доходят до новой цели) и `Transition` (затухание или сдвиг между
  последним показанным кадром и новыми). Элементы анимируют наведение,
  нажатие, фокус, положение переключателя, раскрытие списков;
  `Ui::animating` просит у программы следующий кадр.
- `Screen::present` пишет в видеопамять только изменившиеся участки строк
  (хранится копия показанного). Ввод указателя — `/dev/pointer`.
- Ядро: `velora-usb::report` разбирает report descriptor; xHCI читает
  планшеты и мыши, которые не являются boot-устройствами, в report
  protocol (`Function::Pointer`). `/dev/pointer` отдаёт пакеты по 8 байт.
- `tools/run-qemu.sh`: ручной запуск с usb-tablet и окном без растяжения в
  Windows. Тест `clicks` (`velora-uitest.iso`, `tests/uiclick.py`) щёлкает
  каждый элемент установщика.

The installer window is drawn at a scale step from 1x to 4x
(`installer/gui::SCALES`) in the screen's own resolution (D49). Text is
blended in linear light and the x-height is snapped to whole pixels
(`Fonts::snapped_size`). The dark, light and midnight themes each have an
accent colour (`Theme::with_accent`, `theme::AMBER` by default); `Theme::mix`
blends two themes. `libc/ui/src/anim.rs` has ease-in-out curves, `Tweens`
(values that move smoothly to a new target) and `Transition` (a fade or a
slide between the last frame shown and the new ones); widgets animate hover,
press, focus, a switch's knob and a list opening, and `Ui::animating` asks the
program for another frame. `Screen::present` writes only the row spans that
changed (a copy of what is shown is kept); pointer input comes from
`/dev/pointer`. In the kernel, `velora-usb::report` reads report descriptors,
and xHCI reads tablets and mice that are not boot devices in report protocol
(`Function::Pointer`); `/dev/pointer` gives 8-byte packets.
`tools/run-qemu.sh` runs QEMU by hand with a USB tablet and, on Windows, a
window that is not stretched. The `clicks` test (`velora-uitest.iso`,
`tests/uiclick.py`) clicks every element of the installer.
