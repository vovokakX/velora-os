# Журнал решений / Decision log

Каждая запись: дата, решение, причина, отвергнутые варианты.
Each entry: date, decision, reason, rejected alternatives.

---

## D1. Язык ядра: Rust (`no_std`) / Kernel language: Rust

**2026-11-26**

**RU.** Ядро пишется на Rust без стандартной библиотеки.
- Большинство ошибок в ядрах ОС — ошибки памяти в драйверах и разборщиках
  данных (ELF, PE, файловые системы, сетевые пакеты). Rust исключает
  большую часть из них при компиляции. `unsafe` остаётся там, где без него
  нельзя: регистры, порты, таблицы страниц, MMIO. Каждый такой блок
  сопровождается комментарием `SAFETY`.
- Библиотеки, не зависящие от железа (`velora-fat`, позже VeloraFS, разбор
  ELF и GPT), собираются и для ядра, и для хоста. Поэтому их можно тестировать
  обычным `cargo test`.
- Используется стабильный Rust (1.100.0, закреплён в `rust-toolchain.toml`) и
  цель `x86_64-unknown-none` (без SSE, без red zone). Nightly не нужен:
  обработчики прерываний написаны на ассемблере через `global_asm!`.
- Отвергнуто: C. Проще в сборке и привычнее, но защиту памяти пришлось бы
  обеспечивать только дисциплиной и анализаторами.

**EN.** The kernel is written in Rust without the standard library. Most
kernel bugs are memory errors in drivers and parsers; Rust rules out most of
them at compile time, and `unsafe` is limited to registers, ports, page tables
and MMIO, each with a `SAFETY` comment. Hardware-independent crates are built
for both the kernel and the host, so they are tested with `cargo test`. Stable
Rust 1.100.0 (pinned) and the `x86_64-unknown-none` target; no nightly is
needed because interrupt entry stubs are written with `global_asm!`.
Rejected: C — simpler toolchain, but memory safety would depend only on
discipline and analysers.

## D2. Загрузчик: Limine / Bootloader: Limine

**2026-11-26**

**RU.** Limine 12.11.1, протокол Limine, base revision 6.
- Сразу даёт long mode, ядро в верхней половине, HHDM, карту памяти,
  framebuffer, модули (initramfs) и командную строку.
- Лицензия BSD-2 совместима с GPL-3.0.
- Скачивается при сборке с проверкой SHA-256 и в репозиторий не попадает.
- Отвергнуто: свой UEFI-загрузчик (большая работа, не относящаяся к ядру;
  возможно, позже), GRUB/Multiboot2 (нет верхней половины и HHDM «из коробки»,
  сложнее конфигурация).

**EN.** Limine 12.11.1, Limine protocol, base revision 6. It provides long
mode, a higher-half kernel, the HHDM, the memory map, the framebuffer,
modules and the command line. BSD-2 is GPL-compatible. It is downloaded at
build time with a SHA-256 check and is not stored in the repository.
Rejected: a custom UEFI loader (much work unrelated to the kernel; maybe
later), GRUB/Multiboot2 (no higher half or HHDM out of the box).

## D3. Тип ядра: монолитное модульное / Kernel type: modular monolithic

**2026-11-26**

**RU.** Драйверы и файловые системы работают в ядре за трейтами. Это проще
и быстрее микроядра: нет передачи сообщений на каждый ввод-вывод. Границы
между модулями поддерживаются трейтами и отдельными crate-ами. Перенос
драйверов в пространство пользователя остаётся возможным позже.

**EN.** Drivers and filesystems run in the kernel behind traits. This is
simpler and faster than a microkernel (no message passing per I/O). Module
boundaries are kept by traits and separate crates; moving drivers to user
space later remains possible.

## D4. Лицензия: GPL-3.0-or-later / License: GPL-3.0-or-later

**2026-11-26**

**RU.** Выбрано владельцем проекта. Любые изменённые версии Velora, которые
распространяются, должны оставаться открытыми. Limine (BSD-2) и шрифт Spleen
(BSD-2) совместимы с GPL-3.0. Тексты их лицензий лежат в `docs/licenses/` и
в ISO (`/licenses`). Последствие: закрытые драйверы, собранные вместе с
ядром, распространять нельзя.

**EN.** Chosen by the project owner. Distributed modified versions of Velora
must stay open. Limine (BSD-2) and the Spleen font (BSD-2) are compatible;
their license texts are in `docs/licenses/` and in the ISO (`/licenses`).
Consequence: closed-source drivers linked into the kernel cannot be
distributed.

## D5. Сборка ISO своим инструментом / Own ISO builder

**2026-11-26**

**RU.** `tools/mkiso` (Rust) строит ISO 9660 с загрузочной записью El Torito
для UEFI. `fs/fat` строит FAT16-образ для EFI. Так сборка одинакова в Windows
и Linux и не зависит от xorriso и mtools. Найдено при отладке: Limine при
загрузке с CD ищет конфиг и ядро в файловой системе ISO 9660, а не во
FAT-образе EFI. Поэтому во FAT-образе лежит только `BOOTX64.EFI`, а остальное —
в дереве ISO.

**EN.** `tools/mkiso` (Rust) writes ISO 9660 with an El Torito UEFI boot
entry; `fs/fat` builds the FAT16 EFI image. The build is the same on Windows
and Linux and needs neither xorriso nor mtools. Found while debugging: when
booted from a CD, Limine looks for its config and the kernel in the ISO 9660
filesystem, not in the EFI FAT image. So the FAT image contains only
`BOOTX64.EFI` and everything else is in the ISO tree.

## D6. Тесты ядра / Kernel tests

**2026-11-26**

**RU.** Тесты ядра — обычные функции, перечисленные в `kernel/src/ktest.rs`.
Они собираются с флагом `--features ktest` и запускаются в QEMU. Результат
возвращается через устройство `isa-debug-exit`: статус 33 означает успех.
Последний шаг — намеренная паника, которая проверяет сам обработчик паники.
Нестабильная функция `custom_test_frameworks` не используется, чтобы не
зависеть от nightly.

**EN.** Kernel tests are plain functions listed in `kernel/src/ktest.rs`,
built with `--features ktest` and run in QEMU. The result is reported through
`isa-debug-exit` (status 33 means success). The last step is a deliberate
panic that checks the panic handler itself. The unstable
`custom_test_frameworks` feature is not used, to avoid nightly.

## D7. Шрифт консоли: Spleen 8x16 / Console font: Spleen 8x16

**2026-11-26**

**RU.** Растровый шрифт (BSD-2) с латиницей, кириллицей и псевдографикой.
`tools/fontgen/bdf2rs.py` превращает BDF в таблицу Rust. Это данные, а не
код. Рисовать 400 символов вручную нет смысла.

**EN.** A bitmap font (BSD-2) with Latin, Cyrillic and box drawing.
`tools/fontgen/bdf2rs.py` converts BDF into a Rust table. It is data, not
code; drawing 400 glyphs by hand would add nothing.

## D8. Аллокатор физических страниц: битовая карта / Frame allocator: bitmap

**2026-11-26**

**RU.** Один бит на страницу 4 КиБ, поиск next-fit, отдельная функция для
непрерывных блоков с выравниванием (понадобится для DMA на этапе 6). Для 4 ГиБ
RAM карта занимает 128 КиБ. Битовая карта проще buddy-аллокатора, её легко
проверить, и она сразу ловит двойное освобождение. Минус — линейный поиск
непрерывных блоков. Если он станет узким местом, перейдём на buddy, не меняя
интерфейс `mm::frame`.

**EN.** One bit per 4 KiB frame, next-fit search, plus a contiguous aligned
allocation for DMA (stage 6). 128 KiB of bitmap per 4 GiB of RAM. Simpler and
easier to verify than a buddy allocator, and it detects double frees. The
downside is a linear search for contiguous blocks; if that becomes a
bottleneck we switch to a buddy allocator behind the same `mm::frame` API.

## D9. Куча ядра: классы размеров + страничные блоки / Kernel heap: size classes + page runs

**2026-11-26**

**RU.** Запросы до 2048 байт обслуживаются классами размеров (степени двойки
от 16 байт), большие — целыми страницами из списка свободных участков с
объединением соседей. Алгоритм вынесен в `velora-mm` и проверяется на хосте,
в том числе случайным нагрузочным тестом. Известное ограничение: страницы,
отданные классу размеров, обратно не возвращаются.

**EN.** Requests up to 2048 bytes use power-of-two size classes starting at
16 bytes; larger ones take whole pages from a list of free runs that merges
neighbours. The algorithm lives in `velora-mm` and is tested on the host,
including a randomised stress test. Known limitation: pages given to a size
class are never returned.

## D10. Свои таблицы страниц и освобождение памяти загрузчика / Own page tables and releasing bootloader memory

**2026-11-26**

**RU.** Ядро не правит таблицы Limine, а строит свои: так права W^X и
раскладка адресного пространства полностью под контролем ядра, а память
загрузчика (около 45 МиБ в QEMU) можно вернуть. Перед освобождением данные
Limine копируются в `BootInfo`, а ядро переходит на свой стек. Обращение к
ответам Limine после этого вызывает панику, а не тихо читает чужую память.
Записи верхней половины PML4 создаются заранее (1 МиБ), чтобы ядро было
общим для всех будущих адресных пространств.

**EN.** The kernel builds its own page tables instead of editing Limine's, so
W^X and the address space layout are fully under kernel control and the
bootloader memory (about 45 MiB in QEMU) can be reused. Before releasing it,
Limine data is copied into `BootInfo` and the kernel moves to its own stack;
any later use of a Limine response panics instead of silently reading reused
memory. Upper-half PML4 entries are created up front (1 MiB) so the kernel
half can be shared by all future address spaces.

## D11. Прерывания: LAPIC + IOAPIC, 8259 отключён / Interrupts: LAPIC + IOAPIC, 8259 disabled

**2026-11-26**

**RU.** Контроллеры 8259 перенастраиваются на векторы 0x70–0x7f и полностью
маскируются: ложные прерывания от них не путаются с исключениями. Все
прерывания идут через локальный APIC и IOAPIC по таблице MADT, с учётом
переназначений ISA-IRQ (например, IRQ0 → GSI2 в QEMU). Поддерживаются режимы
xAPIC (MMIO) и x2APIC (MSR), какой оставила прошивка. Без APIC ядро не
запускается: на x86_64 он есть всегда.

**EN.** The 8259 PICs are remapped to vectors 0x70–0x7f and fully masked so
their spurious interrupts cannot look like exceptions. All interrupts go
through the local APIC and the I/O APICs from the MADT, honouring ISA IRQ
overrides (e.g. IRQ0 → GSI2 in QEMU). Both xAPIC (MMIO) and x2APIC (MSR) are
supported, whichever the firmware left enabled. The kernel requires an APIC;
every x86_64 system has one.

## D12. Время: TSC + таймер APIC 100 Гц, калибровка по PIT / Time: TSC + 100 Hz APIC timer, PIT calibration

**2026-11-26**

**RU.** TSC и таймер APIC калибруются за 50 мс по каналу 2 PIT, без
прерываний. Точное время с момента загрузки берётся из TSC, тик таймера
(100 Гц) нужен для сна и будущего планировщика. 100 Гц выбрано ради малого
числа пробуждений в простое; если планировщику понадобится больше, частота
меняется одной константой. Если PIT отсутствует, загрузка останавливается с
понятной ошибкой. Запасной путь через CPUID 0x15 запланирован.

**EN.** The TSC and the APIC timer are calibrated over 50 ms against PIT
channel 2, without interrupts. Precise time since boot comes from the TSC;
the 100 Hz tick is used for sleeping and the future scheduler. 100 Hz keeps
idle wakeups low; the scheduler can raise it by changing one constant. If
there is no PIT, boot stops with a clear error; a CPUID 0x15 fallback is
planned.

## D13. Клавиатура: общий crate ввода и раскладки EN/RU / Keyboard: shared input crate and EN/RU layouts

**2026-11-26**

**RU.** Декодер скан-кодов, модификаторы и раскладки вынесены в
`drivers/input` и проверяются на хосте. Драйвер PS/2 только передаёт байты.
Будущий драйвер USB HID будет выдавать те же `KeyCode`, поэтому раскладки и
обработка ввода не изменятся. Раскладки EN и RU переключаются по Alt+Shift.
События кладутся в очередь ядра; позже из неё будут читать `/dev/input` и Lumi.

**EN.** Scancode decoding, modifiers and layouts live in `drivers/input` and
are tested on the host; the PS/2 driver only forwards bytes. A future USB HID
driver will produce the same `KeyCode`s, so layouts and input handling stay
the same. EN and RU layouts switch with Alt+Shift. Events go into a kernel
queue that `/dev/input` and Lumi will read later.

## D14. Планировщик и вытеснение / Scheduler and preemption

**2026-11-26**

**RU.** Round-robin с тремя приоритетами (High, Normal, Idle), квант — один
тик таймера (10 мс). Пользовательский код и потоки ядра вытесняются по
таймеру. Внутри системных вызовов прерывания выключены, поэтому ядро в
системном вызове не вытесняется. Так проще и надёжнее: не нужно делать каждую
часть ядра безопасной для вытеснения. Блокирующие вызовы передают процессор
явно. Цена — системный вызов задерживает переключение. Вызовы короткие,
поэтому для однопроцессорной системы это приемлемо. На SMP вопрос будет
пересмотрен. Мёртвый поток не может освободить стек, на котором работает,
поэтому стеки освобождает следующий поток сразу после переключения.

**EN.** Round robin with three priorities (High, Normal, Idle) and a one-tick
(10 ms) time slice. User code and kernel threads are preempted by the timer.
Interrupts are off inside system calls, so the kernel is not preempted there:
simpler and safer, because not every kernel path has to be preemption-safe.
Blocking calls give up the CPU explicitly. The cost is that a system call
delays a switch; calls are short, so this is acceptable on one CPU and will be
revisited for SMP. A dead thread cannot free the stack it runs on, so stacks
are freed by the next thread right after the switch.

## D15. Плоские тестовые программы до ELF / Flat test programs until ELF

**2026-11-26**

**RU.** Загрузчик ELF появится на этапе 5. Чтобы проверить ring 3 уже на
этапе 4, в ядро встроены небольшие программы на ассемблере (`userprog.rs`),
не зависящие от адреса. Ядро копирует их по адресу 0x400000. Это временное
средство для тестов, а не формат программ Velora. После этапа 5 программы
будут обычными файлами ELF.

**EN.** The ELF loader arrives in stage 5. To test ring 3 already in stage 4,
small position-independent assembly programs are embedded in the kernel
(`userprog.rs`) and copied to 0x400000. This is a test aid, not a Velora
program format; from stage 5 on programs are ordinary ELF files.

## D16. Номера ошибок как в Linux / Linux-compatible error numbers

**2026-11-26**

**RU.** Коды ошибок совпадают с Linux (EFAULT = 14, ENOSYS = 38 и т. д.).
Номера системных вызовов свои. Совпадение кодов ошибок упрощает перенос
программ и библиотек и ничего не стоит.

**EN.** Error numbers match Linux (EFAULT = 14, ENOSYS = 38, ...); system call
numbers are Velora's own. Matching error codes makes porting programs and
libraries easier at no cost.

## D17. Программы: статический ELF (ET_EXEC) / Programs: static ELF (ET_EXEC)

**2026-11-26**

**RU.** Загружаются только статически собранные ELF64 типа `ET_EXEC` с
фиксированным адресом 0x400000. Для позиционно-независимых файлов (`ET_DYN`)
нужны перемещения, для динамически собранных — динамический загрузчик. Ни то
ни другое пока не нужно, а без них загрузчик проще и проверяемее. Каждая
секция начинается с новой страницы, RELRO отключён (`-z norelro`), чтобы у
каждой страницы был один набор прав. Загрузчик отклоняет сегменты, которые
одновременно доступны на запись и исполнение или делят страницу. ASLR
появится вместе с `ET_DYN`.

**EN.** Only statically linked ELF64 `ET_EXEC` files at the fixed address
0x400000 are loaded. Position-independent files (`ET_DYN`) need relocations
and dynamically linked ones a dynamic loader; neither is needed yet, and the
loader is simpler and easier to verify without them. Every section starts on
its own page and RELRO is disabled (`-z norelro`) so each page has one set of
permissions; the loader rejects writable+executable segments and segments that
share a page. ASLR comes together with `ET_DYN`.

## D18. initramfs в формате ustar / ustar initramfs

**2026-11-26**

**RU.** Формат tar прост, хорошо описан, и его можно читать прямо в памяти
без копирования. Архив собирает свой инструмент `mkinitrd`: время
модификации и владелец фиксированы, поэтому образ воспроизводим. До VFS
(этап 7) initramfs — единственная файловая система, и доступна она только
для чтения.

**EN.** tar is simple, well documented and can be read in place without
copying. The archive is built by the own `mkinitrd` tool with fixed times and
owners, so the image is reproducible. Until the VFS (stage 7) the initramfs is
the only file system and it is read-only.

## D19. Отдельный workspace для программ / Separate workspace for programs

**2026-11-26**

**RU.** Ядро и программы собираются под одну цель (`x86_64-unknown-none`), но
с разными флагами: ядро — `code-model=kernel` в верхней половине, программы —
обычная модель в нижней половине. Cargo задаёт флаги на цель, а не на пакет,
поэтому программы — отдельный workspace (`userland/`) с флагами из `build.sh`
и своим каталогом сборки. vlibc лежит в `libc/` и подключается программами
по пути.

**EN.** The kernel and programs build for the same target
(`x86_64-unknown-none`) but with different flags: the kernel with
`code-model=kernel` in the higher half, programs with the normal model in the
lower half. Cargo sets flags per target, not per package, so programs are a
separate workspace (`userland/`) with flags from `build.sh` and their own
target directory. vlibc lives in `libc/` and programs use it by path.

## D20. Формат PE: распознавание без запуска / PE format: recognised, not run

**2026-11-26**

**RU.** Интерфейс `ExecFormat` уже содержит реализацию для PE. Она читает
заголовки (тип процессора, подсистема, точка входа) и отвечает «нужен
Win32-слой». Так `spawn` чужого .exe даёт точную ошибку, а не «неизвестный
формат». Когда появится Win32-слой, `PeFormat::load` будет отображать секции и
выдавать процессу personality Win32. Остальное ядро менять не придётся.

**EN.** The `ExecFormat` interface already has a PE implementation: it reads
the headers (machine, subsystem, entry point) and answers "the Win32 layer is
needed", so spawning a foreign .exe gives a precise error rather than "unknown
format". When the Win32 layer exists, `PeFormat::load` will map the sections
and give the process the Win32 personality without changes elsewhere.

## D21. Драйверы дисков: опрос и промежуточный буфер / Disk drivers: polling and bounce buffers

**2026-11-26**

**RU.** Драйверы virtio-blk, AHCI и NVMe в первой версии ждут окончания
команды опросом и выполняют одну команду за раз. Данные идут через
промежуточный буфер DMA размером 64 КиБ. Так драйверы проще и их легче
проверить: не нужны MSI/MSI-X, очереди ожидания и закрепление
пользовательских страниц. Цена — процессор занят, пока диск работает, и
лишнее копирование. В QEMU команда занимает доли миллисекунды. Прерывания
(MSI-X) и несколько команд одновременно — отдельная задача после этапа 7,
когда будет понятна нагрузка от файловой системы.

**EN.** The first versions of the virtio-blk, AHCI and NVMe drivers poll for
completion, run one command at a time and pass data through a 64 KiB DMA
bounce buffer. This keeps the drivers simple and verifiable: no MSI/MSI-X, no
wait queues, no pinning of user pages. The cost is a busy CPU while the disk
works and an extra copy; in QEMU a command takes a fraction of a millisecond.
Interrupts (MSI-X) and several commands in flight are a separate task after
stage 7, once the file system load is known.

## D22. PCI через абстрактный доступ / PCI over an abstract config access

**2026-11-26**

**RU.** Перечисление шины, определение размеров BAR и обход capabilities
вынесены в `velora-pci` и работают через трейт `ConfigAccess`. Ядро даёт
реализацию через ECAM или порты, тесты — имитацию шины с мостами и
64-битными BAR. Так самые хрупкие места (размер BAR, мосты, зацикленные
списки capabilities) проверяются на хосте.

**EN.** Bus enumeration, BAR sizing and capability walking live in
`velora-pci` behind the `ConfigAccess` trait. The kernel implements it with
ECAM or ports; tests use a simulated bus with bridges and 64-bit BARs, so the
fragile parts (BAR sizing, bridges, looping capability lists) are tested on
the host.

## D23. Основной CI на QEMU 9.1 (Fedora) / Main CI on QEMU 9.1 (Fedora)

**2026-11-26**

**RU.** С QEMU 8.2 из Ubuntu 24.04 драйвер virtio-blk не получает ответов (#19).
С QEMU 9.1 и 11.1 тот же код проходит все тесты. Пока причина не найдена,
основная проверка CI и сборка релиза выполняются в контейнере Fedora 41
(QEMU 9.1, KVM). Проблема записана в известных проблемах. В тестовом
скрипте есть переменные `VELORA_QEMU_EXTRA` (дополнительные аргументы QEMU,
например трассировка) и `VELORA_VIRTIO_OPTS` (параметры устройства
virtio-blk) для таких расследований.

**EN.** With QEMU 8.2 from Ubuntu 24.04 the virtio-blk driver gets no answers
(#19); with QEMU 9.1 and 11.1 the same code passes every test. Until the cause
is found, the main CI check and the release build run in a Fedora 41 container
(QEMU 9.1, KVM), and the problem is listed as a known issue. The test runner
accepts `VELORA_QEMU_EXTRA` (extra QEMU arguments, e.g. tracing) and
`VELORA_VIRTIO_OPTS` (virtio-blk device options) for such investigations.

## D24. vlibc под LGPL-3.0 / vlibc under LGPL-3.0

**2026-11-26**

**RU.** Решение владельца проекта. vlibc (`libc/vlibc`) распространяется по
LGPL-3.0-or-later. С ней могут компоноваться программы под любой лицензией, в
том числе закрытые. Изменения самой vlibc при этом остаются открытыми. Код из
`kernel/abi` (номера вызовов, константы) и `kernel/mm` (аллокатор кучи) входит
в vlibc при сборке, поэтому эти crate тоже переведены на LGPL-3.0-or-later.
LGPL совместима с GPL, так что ядро, которое их использует, остаётся под
GPL-3.0-or-later. Остальной проект не меняется.

**EN.** Decided by the project owner. vlibc (`libc/vlibc`) is distributed under
LGPL-3.0-or-later, so programs under any license, closed ones included, can link
with it while changes to vlibc itself stay open. Code from `kernel/abi` (call
numbers, constants) and `kernel/mm` (the heap allocator) is built into vlibc, so
these crates moved to LGPL-3.0-or-later as well. The LGPL is GPL-compatible, so
the kernel that uses them stays GPL-3.0-or-later. Nothing else changes.

## D25. Одна блокировка VFS / One VFS lock

**2026-11-26**

**RU.** Вся VFS (таблица монтирования и все файловые системы) защищена одной
спин-блокировкой, которая выключает прерывания. Операция выполняется
целиком, драйверы дисков ждут ответа опросом (D21). Ядро однопроцессорное и
не вытесняется внутри системных вызовов, поэтому этого достаточно и нет
риска взаимных блокировок между файловыми системами. Цена: пока идёт
дисковая операция, другие потоки не работают, а таймерные тики
откладываются. Порядок блокировок: планировщик, затем VFS (закрытие файлов
завершившегося процесса); обратного порядка нет. С SMP и прерываниями
дисков (этап SMP) блокировка будет разделена по монтированиям, а ожидание
диска станет блокирующим.

**EN.** The whole VFS (mount table and every file system) is behind one
spinlock that disables interrupts; an operation runs to completion and disk
drivers poll (D21). The kernel is single-CPU and not preempted inside system
calls, so this is enough and rules out deadlocks between file systems. The
cost: while a disk operation runs, other threads do not, and timer ticks are
delayed. Lock order: scheduler, then VFS (closing the files of an exiting
process), never the reverse. With SMP and disk interrupts the lock will be split
per mount and disk waits will block.

## D26. VeloraFS: экстенты и журнал метаданных / VeloraFS: extents and a metadata journal

**2026-11-26**

**RU.** Своя файловая система нужна для корня (права, журнал, проверка
целостности), FAT этого не даёт. Выбрано: блоки 4 КиБ, экстенты (мало
метаданных для больших файлов), журнал только метаданных в режиме ordered
(данные пишутся до фиксации транзакции), одна транзакция на операцию,
CRC32C на всех блоках метаданных. Полный журнал данных удвоил бы запись;
контрольные суммы данных пока не сделаны. Транзакция на операцию — это
4 сброса кеша диска на изменение. Это медленнее, чем группировка
транзакций, зато проще и надёжнее. Группировку можно добавить позже без
смены формата. Формат описан в [velorafs.md](velorafs.md).

**EN.** A native file system is needed for the root (permissions, journal,
integrity checks), which FAT cannot provide. Chosen: 4 KiB blocks, extents
(little metadata for large files), a metadata-only journal in ordered mode
(data written before the transaction commits), one transaction per operation,
CRC32C on every metadata block. Full data journaling would double writes;
data checksums are not done yet. One transaction per operation costs up to
four disk cache flushes per change: slower than grouped transactions but
simpler; grouping can come later without a format change. The format is in
[velorafs.md](velorafs.md).

## D27. Номера inode на FAT / Inode numbers on FAT

**2026-11-26**

**RU.** В FAT нет inode. Номером файла служит байтовая позиция его 8.3-записи
в каталоге (корень — 1). При переименовании запись переезжает, и номер
меняется: открытый дескриптор такого файла после переименования
указывает на старое место. Для раздела EFI и обмена файлами этого
достаточно; для системы используется VeloraFS.

**EN.** FAT has no inodes. A file's number is the byte position of its 8.3
directory entry (the root is 1). Renaming moves the entry and changes the
number, so a descriptor opened before a rename points at the old place. This
is enough for the EFI partition and file exchange; the system uses VeloraFS.

## D28. Тесты файловых систем: две загрузки / File system tests: two boots

**2026-11-26**

**RU.** Тест «файлы сохраняются после перезагрузки» должен включать
настоящую перезагрузку. `tests/qemu_run.py` дважды запускает тестовое ядро с
одним и тем же чистым диском на 128 МиБ. Первая загрузка создаёт VeloraFS и
FAT32, проверяет их и оставляет в журнале VeloraFS зафиксированную, но не
записанную на места транзакцию (как при отключении питания). Вторая
загрузка монтирует том при старте (журнал воспроизводится) и проверяет
файлы. После каждой загрузки тома проверяются на хосте программой
`velora-fsck` из этого репозитория, а FAT32 ещё и `fsck.fat` из dosfstools
(в CI). Обрыв питания после каждой отдельной записи проверяется
юнит-тестами на хосте: в QEMU так перебирать точки обрыва слишком долго.

**EN.** "Files persist across a reboot" must include a real reboot.
`tests/qemu_run.py` boots the test kernel twice with the same blank 128 MiB
disk. The first boot creates VeloraFS and FAT32, checks them and leaves a
committed but not checkpointed VeloraFS transaction in the journal (as a power
cut would). The second boot mounts the volume at startup (replaying the
journal) and checks the files. After each boot the volumes are checked on the
host by `velora-fsck` from this repository, and FAT32 also by `fsck.fat` from
dosfstools (in CI). A power cut after every single write is covered by host
unit tests, where iterating over all cut points is fast.

## D29. Терминал в ядре / The terminal in the kernel

**2026-11-26**

**RU.** Редактирование строки, эхо и Ctrl+C для консоли делает ядро
(`tty.rs`), а не каждая программа: тогда `cat`, `login` и любая простая
программа получают готовую строку. vsh переключает консоль в «сырой» режим
и редактирует строку сам (история, Tab). Ctrl+C перехватывается при
поступлении клавиши и обрабатывается потоком ядра `tty-intr`. Поэтому
завершается и программа, которая не читает клавиатуру. Сигналов в системе
нет: Ctrl+C завершает процесс переднего плана со статусом 130.

**EN.** Line editing, echo and Ctrl+C for the console are done by the kernel
(`tty.rs`), not by every program, so `cat`, `login` and any simple program get
finished lines. vsh switches the console to raw mode and edits its own line
(history, Tab). Ctrl+C is caught as the key arrives and handled by the
`tty-intr` kernel thread, so it also ends a program that does not read the
keyboard. There are no signals: Ctrl+C ends the foreground process with status
130.

## D30. Пароли: PBKDF2, только хеши / Passwords: PBKDF2, hashes only

**2026-11-26**

**RU.** Пароли хранятся как PBKDF2-HMAC-SHA-256 с 10 000 итераций и
случайной солью 16 байт (`$pbkdf2-sha256$...`) в `/etc/shadow` с правами
0600. SHA-256 и PBKDF2 реализованы в `libc/auth` и проверены на векторах
из RFC. В репозитории нет ни паролей, ни хешей: `/etc/shadow` создаётся при
сборке программой `tools/mkpasswd`. Пароли берутся из переменных окружения
`VELORA_ROOT_PASSWORD` и `VELORA_USER_PASSWORD`, по умолчанию `velora`. Это
общеизвестный пароль Live-образа, а не секрет, и он указан в README.
Установщик (этап 9) будет спрашивать пароли и записывать новые хеши.
Сравнение хешей идёт за постоянное время, а для неизвестного пользователя
хеш тоже вычисляется.

**EN.** Passwords are stored as PBKDF2-HMAC-SHA-256 with 10,000 iterations and a
random 16-byte salt (`$pbkdf2-sha256$...`) in `/etc/shadow`, mode 0600. SHA-256
and PBKDF2 are implemented in `libc/auth` and checked against RFC vectors. The
repository holds no passwords or hashes: `/etc/shadow` is generated at build
time by `tools/mkpasswd` from the `VELORA_ROOT_PASSWORD` and
`VELORA_USER_PASSWORD` environment variables (default `velora`, the
documented Live image password, not a secret). The installer (stage 9) will
ask for passwords and write new hashes. Hashes are compared in constant time,
and a hash is computed for unknown users too.

## D31. spawn вместо fork; наследуются только 0–2 / spawn instead of fork; only 0–2 are inherited

**2026-11-26**

**RU.** Процессы создаются вызовом `spawn` (загрузить программу и
запустить), без `fork`. Shell готовит дескрипторы 0–2 через `dup2` (канал,
файл перенаправления), запускает программу и восстанавливает свои. Потомок
получает только дескрипторы 0–2. Иначе концы каналов, которые shell держит
открытыми, попадали бы во все программы конвейера, и читатель никогда не
получил бы конец ввода. Флаг close-on-exec поэтому не нужен.

**EN.** Processes are created with `spawn` (load a program and run it), without
`fork`. The shell sets descriptors 0–2 with `dup2` (a pipe, a redirection
file), spawns the program and restores its own. The child gets only
descriptors 0–2; otherwise pipe ends the shell holds would leak into every
program of a pipeline and the reader would never see the end of input. A
close-on-exec flag is therefore not needed.

## D32. Live-режим: домашние каталоги в памяти / Live system: home directories in memory

**2026-11-26**

**RU.** Корень Live-образа — initramfs только для чтения. Чтобы
пользователи могли работать в своих каталогах, ядро монтирует в `/home` и
`/root` файловые системы в памяти, а `login` создаёт `/home/<имя>` при
первом входе. После перезагрузки их содержимое пропадает. В установленной
системе (этап 9) корень и домашние каталоги будут на VeloraFS.

**EN.** The Live root is the read-only initramfs. So users can work in their own
directories, the kernel mounts in-memory file systems at `/home` and `/root`,
and `login` creates `/home/<name>` on first login. Their contents are lost on
reboot. In an installed system (stage 9) the root and home directories will be
on VeloraFS.

## D33. Разметка установленной системы / Layout of an installed system

**2026-11-26**

**RU.** GPT с двумя разделами. Первый — EFI system partition (FAT32,
64 МиБ, выровнен на 1 МиБ): Limine (`/EFI/BOOT/BOOTX64.EFI` — путь для
съёмных носителей, работает без записей в NVRAM), ядро и `limine.conf`.
Второй — корень (VeloraFS) на остальном месте. Для типа корневого раздела
выбран свой GUID `5E1F7A30-9C42-4D8B-A1E7-56454C4F5241`. Ядро находит корень
по уникальному GUID раздела из параметра `velora.root=PARTUUID=...`, а не по
имени устройства: имя зависит от порядка контроллеров. initramfs в
установленной системе не используется, программы лежат на VeloraFS. 64 МиБ —
наименьший размер, при котором FAT32 с кластерами по 512 байт получает
нужные 65 525 кластеров с запасом.

**EN.** GPT with two partitions. The first is the EFI system partition (FAT32,
64 MiB, 1 MiB aligned): Limine (`/EFI/BOOT/BOOTX64.EFI`, the removable-media
path, which works without NVRAM entries), the kernel and `limine.conf`. The
second is the root (VeloraFS) in the rest. The root partition type uses its own
GUID `5E1F7A30-9C42-4D8B-A1E7-56454C4F5241`. The kernel finds the root by the
partition's unique GUID from `velora.root=PARTUUID=...`, not by device name,
since names depend on controller order. An installed system uses no
initramfs; the programs live on VeloraFS. 64 MiB is the smallest size at which
FAT32 with 512-byte clusters gets the required 65,525 clusters with margin.

## D34. Установщик пишет файловые системы сам / The installer writes the file systems itself

**2026-11-26**

**RU.** Установщику не нужен `mount`. Он открывает `/dev/<диск>`, делает
поверх него `BlockIo` и пишет GPT, FAT32 и VeloraFS библиотеками, которые
использует и ядро. Логика форматирования одна, проверена юнит-тестами, а
ядро не нужно учить монтированию во время работы. Диски, смонтированные в
`/mnt`, установщик не трогает: запись в обход ядра испортила бы
смонтированную файловую систему. Вызовы `mount`/`umount` (30, 31) остаются
в планах.

**EN.** The installer needs no `mount`. It opens `/dev/<disk>`, wraps it as a
`BlockIo` and writes GPT, FAT32 and VeloraFS with the libraries the kernel uses
too, so the formatting logic exists once, is unit-tested, and the kernel needs
no run-time mounting. Disks mounted under `/mnt` are refused, because writing
behind the kernel's back would corrupt a mounted file system. `mount`/`umount`
(30, 31) remain planned.

## D35. Источник случайных чисел / Random numbers

**2026-11-26**

**RU.** `getrandom` берёт RDRAND (если процессор его поддерживает) и
смешивает с TSC через splitmix64. Этого достаточно для солей паролей и
GUID, но это не криптографический генератор с накоплением энтропии. Без
RDRAND результат предсказуемее, это записано в известных проблемах.

**EN.** `getrandom` takes RDRAND (when the CPU has it) and mixes it with the TSC
through splitmix64. This is enough for password salts and GUIDs but is not a
cryptographic generator with an entropy pool; without RDRAND the output is
more predictable, which is listed as a known issue.

## D36. Сетевой стек без ввода-вывода / A network stack without I/O

**2026-11-26**

**RU.** Стек `velora-net` — обычная структура данных: принимает кадры,
выдаёт кадры, время ему передаётся. Ядро добавляет только драйвер и поток,
который переносит кадры. Поэтому протоколы проверяются на хосте без QEMU:
два стека соединяются напрямую, а потери и задержки задаются в тесте.
Своя реализация, а не сторонняя библиотека: так требует задание («своё
ядро, свои драйверы»), а объём (ARP, IPv4, ICMP, UDP, DHCP, TCP) остаётся
небольшим. Поток `net` с высоким приоритетом просыпается по прерыванию
MSI-X и раз в тик для таймеров. Системные вызовы ждут на общей очереди
ожидания, которую поток будит после каждой обработки.

**EN.** The `velora-net` stack is a plain data structure: frames in, frames out,
time passed in. The kernel adds only the driver and a thread that moves frames,
so the protocols are tested on the host without QEMU, with two stacks connected
directly and loss and delay chosen by the test. It is our own implementation
rather than a third-party library, as the specification requires ("own kernel,
own drivers"), and the scope (ARP, IPv4, ICMP, UDP, DHCP, TCP) stays small. The
high-priority `net` thread wakes on the MSI-X interrupt and every tick for the
timers; system calls wait on a shared wait queue the thread wakes after each
round.

## D37. Упрощения TCP / TCP simplifications

**2026-11-26**

**RU.** Получатель принимает только сегменты по порядку. Сегменты не по
порядку отбрасываются и подтверждаются повторным ACK. Отправитель после
трёх повторных ACK или по таймеру повторяет данные начиная с первого
неподтверждённого байта (go-back-N). Без управления перегрузкой это
надёжно и просто, но при потерях медленнее, чем в полноценных
реализациях; для локальной сети и QEMU этого достаточно. Ожидание SYN
короче (5 повторов, около 15 с), чем для данных (8 повторов).

**EN.** The receiver takes in-order segments only; out-of-order ones are dropped
and answered with a duplicate ACK. After three duplicate ACKs or a timeout the
sender repeats data from the first unacknowledged byte (go-back-N). Without
congestion control this is reliable and simple but slower under loss than full
implementations, which is enough for a LAN and QEMU. A SYN gives up sooner
(5 retries, about 15 s) than data (8 retries).

## D38. Plume: логика в библиотеке, вывод разницей кадров / Plume: logic in a library, output as frame differences

**2026-11-26**

**RU.** Редактор разделён на библиотеку `velora-editor` без ввода-вывода и
небольшую программу `plume`. Библиотека получает клавиши и отдаёт кадр
(строки ячеек и положение курсора), запись файлов идёт через трейт `Io`.
Так все команды, вопросы и вид экрана проверяются юнит-тестами на хосте,
а тест в QEMU проверяет только связку с консолью. Экран выводится
разницей с предыдущим кадром: отправляются только изменившиеся строки.
Это важно для последовательного порта и для framebuffer без аппаратного
ускорения. Сохранение пишет файл на месте (`TRUNCATE`), а не через
временный файл и переименование: так сохраняются владелец и права файла
(например, у `/etc/passwd`). После записи вызывается `sync`, чтобы файл
был на диске сразу. Файл с неверным UTF-8 открывается только для чтения,
чтобы сохранение не заменило байты на U+FFFD. Отмены действий (undo) нет.

**EN.** The editor is split into the `velora-editor` library, which has no I/O,
and a small `plume` program. The library takes keys and returns a frame (rows
of cells and the cursor position); files are written through the `Io` trait.
All commands, questions and the screen layout are therefore checked by host
unit tests, and the QEMU test only checks the connection to the console. The
screen is updated with the difference from the previous frame: only changed
rows are sent, which matters on the serial port and on a framebuffer without
acceleration. Saving writes the file in place (`TRUNCATE`) rather than through
a temporary file and a rename, so the file keeps its owner and permissions
(for example `/etc/passwd`); `sync` follows so the file is on disk at once. A
file with invalid UTF-8 opens read-only so that saving cannot replace bytes
with U+FFFD. There is no undo.

## D39. Журнал паники пишется прямо в секторы файла / The panic log is written straight to a file's sectors

**2026-11-26**

**RU.** После паники нельзя доверять ни VFS, ни куче, ни блокировкам: паника
могла случиться внутри них. Поэтому файл журнала готовится при загрузке:
он создаётся размером 64 КиБ, записывается на диск, и запоминаются его
секторы (`FileSystem::data_sectors`). При панике текст собирается в
статическом буфере и пишется в эти секторы драйвером диска с опросом;
блокировка драйвера снимается принудительно. Используется только FAT: у
файла FAT нет контрольных сумм и журнала, и прямая запись данных не
нарушает целостность тома. Том выбирается сам, только если на нём есть
`/boot/velora` (раздел EFI Velora) или уже есть файл `velora-panic.log`. Так
ядро не пишет в разделы EFI других систем. При следующей загрузке журнал
переносится в `velora-panic.prev.log`. Ограничение: если паника случилась
посреди запроса к тому же диску, состояние очереди драйвера может быть
испорчено, и запись не удастся. Об этом выводится сообщение.

**EN.** After a panic neither the VFS, the heap nor any lock can be trusted: the
panic may have happened inside them. The log file is therefore prepared at
boot: created 64 KiB long, written to disk, and its sectors recorded
(`FileSystem::data_sectors`). On a panic the text is assembled in a static
buffer and written to those sectors by the polling disk driver, whose lock is
forcibly released. Only FAT is used: a FAT file has no checksums or journal,
so writing its data directly keeps the volume consistent. A volume is chosen
automatically only if it holds `/boot/velora` (a Velora EFI partition) or
already has `velora-panic.log`, so the kernel never writes to other systems'
EFI partitions. At the next boot the log moves to `velora-panic.prev.log`.
Limitation: if the panic interrupted a request on the same disk, the driver's
queue may be in a bad state and the write fails; a message says so.

## D40. Порядок опорных часов для калибровки / Order of reference clocks for calibration

**2026-11-26**

**RU.** Сначала HPET (точный, 64-битный счётчик в памяти), затем таймер ACPI
PM (есть на каждом ПК с ACPI, порт ввода-вывода), затем PIT. На новых ПК
PIT бывает отключён или эмулируется неточно, поэтому он только запасной.
Каждый источник проверяется: если счётчик не меняется, берётся следующий,
а причина пишется в журнал. Ни один источник не программирует прерывания:
нужен только счётчик. Тесты ядра сравнивают частоту TSC, измеренную по
всем трём источникам. Тест `usb` загружает систему без HPET, и
калибровка проходит по таймеру ACPI PM.

**EN.** The HPET comes first (precise, a 64-bit counter in memory), then the
ACPI PM timer (present on every ACPI PC, an I/O port), then the PIT. On newer
PCs the PIT may be disabled or poorly emulated, so it is only the fallback.
Each source is checked: if its counter does not move the next one is used and
the reason is logged. No source programs interrupts; only the counter is read.
The kernel tests compare the TSC rate measured against all three, and the
`usb` test boots without an HPET so calibration uses the ACPI PM timer.

## D41. Отдельный образ для флешки вместо гибридного ISO / A separate USB image instead of a hybrid ISO

**2026-11-26**

**RU.** Прошивка UEFI загружается с флешки через таблицу разделов и раздел
EFI, а не через El Torito. Гибридный ISO потребовал бы общих данных у ISO
9660 и FAT. Проще и надёжнее отдельный образ `velora-usb.img`: GPT и один
раздел EFI FAT32 с загрузчиком, настройками Limine, ядром и initramfs.
Limine, загруженный с этого раздела, читает файлы `boot():/` с него же.
Образ собирается на хосте тем же кодом GPT и FAT, что использует ядро и
установщик, и получается одинаковым при одинаковых входных файлах.
Раздел — обычный FAT32, поэтому настройки можно поправить на любой
системе. Образ записывает на флешку сам пользователь. Сборка и тесты
работают только с файлом.

**EN.** UEFI firmware boots a USB drive through its partition table and EFI
partition, not through El Torito. A hybrid ISO would need ISO 9660 and FAT to
share data; a separate image, `velora-usb.img`, is simpler and more reliable: a
GPT and one FAT32 EFI partition with the bootloader, the Limine settings, the
kernel and the initramfs. Limine loaded from that partition reads `boot():/`
files from it. The image is built on the host with the same GPT and FAT code
the kernel and installer use, and the same inputs give the same image. The
partition is plain FAT32, so any system can edit the settings. The user writes
the image to a drive; the build and the tests only handle the file.

## D42. ACPI без интерпретатора AML / ACPI without an AML interpreter

**2026-11-26**

**RU.** Полный интерпретатор AML — большой объём кода (своего или чужого),
а для терминальной системы нужно немногое: выключение, перезагрузка и
кнопка питания. Всё это доступно через фиксированное оборудование ACPI:
значения `_S5` — это константы, их можно найти разбором байтов, а кнопка
питания на настольных ПК и в QEMU — фиксированное событие PWRBTN в PM1.
Поэтому интерпретатора нет, а внешние библиотеки не добавляются (проект
пишет своё). Все события общего назначения (GPE) выключаются: без AML их
некому обработать, а включённое событие с установленным статусом вызывало
бы SCI без конца. Последствия, записанные как известные проблемы: `_PTS`
перед выключением не выполняется; кнопка питания, которая сделана методом
управления (часто у ноутбуков со встроенным контроллером), и устройства
событий ACPI с аппаратным сокращением не обрабатываются. Выключение
командой `shutdown` на таких машинах работает.

**EN.** A full AML interpreter is a lot of code (ours or someone else's), and a
terminal system needs little: power off, restart and the power button. All of
it is available through ACPI fixed hardware: the `_S5` values are constants
that byte parsing finds, and on desktops and in QEMU the power button is the
fixed PWRBTN event in PM1. So there is no interpreter and no external library
(the project writes its own code). All general-purpose events (GPEs) are turned
off: without AML nothing could handle them, and an enabled event with its
status set would raise the SCI forever. Consequences, listed as known issues:
`_PTS` is not run before powering off; a control-method power button (common
on laptops with an embedded controller) and hardware-reduced ACPI event devices
are not handled. Powering off with `shutdown` works on such machines.

## D43. SMP с большой блокировкой ядра / SMP with a big kernel lock

**2026-11-27**

**RU.** Ядро писалось для одного процессора: системные вызовы идут с
выключенными прерываниями и считают, что больше никто не работает с
данными ядра. Тонкие блокировки по всему ядру — большой и рискованный
объём работы. Поэтому выбрана большая блокировка ядра, как в ранних
многопроцессорных Unix: код ядра выполняется на одном процессоре за раз,
программы — на всех. Для терминальной системы, где почти всё время
работают программы, этого достаточно; тест показывает, что четыре
вычисляющие программы на четырёх ядрах идут с той же скоростью, что одна.
Блокировка билетная (порядок честный) и рекурсивная для процессора; её
глубина сохраняется в потоке, потому что переключение потоков происходит
под блокировкой. Поток ядра, занятый вычислениями, отдаёт её ждущим на
каждом тике таймера. Все потоки процесса закреплены за одним процессором,
поэтому снятие страниц пользователя не требует сброса TLB на других
процессорах; стеки ядра после освобождения остаются отображёнными и
переиспользуются, поэтому и страницы ядра не меняют содержимого.
Недостатки: системные вызовы разных процессоров не идут параллельно;
опрос диска с выключенными прерываниями (D21) задерживает вход в ядро на
остальных процессорах; потоки одной программы не работают параллельно.
Поток ядра, который вычисляет не блокируясь, держит блокировку между
тиками; потоки ядра на других процессорах тогда получают её только по
очереди на тиках и сильно замедляются (тест ядра, который проверял это на
четырёх процессорах, иногда не укладывался в минуту; теперь его потоки
работают на процессоре 0). В Velora таких потоков нет.

**EN.** The kernel was written for one CPU: system calls run with interrupts off
and assume nothing else touches kernel data. Fine-grained locking across the
kernel would be a large and risky change, so a big kernel lock was chosen, as
in early multiprocessor Unix systems: kernel code runs on one CPU at a time,
programs on all. That is enough for a terminal system, where programs run most
of the time; a test shows four computing programs on four cores each run as
fast as one alone. The lock is a ticket lock (fair order), recursive per CPU;
its depth is saved per thread because threads are switched under the lock. A
kernel thread that computes hands it to waiting CPUs at every timer tick. All
threads of a process are pinned to one CPU, so unmapping user pages needs no
TLB shootdown on other CPUs; freed kernel stacks stay mapped and are reused, so
kernel pages never change contents either. Drawbacks: system calls on
different CPUs do not run in parallel; disk polling with interrupts off (D21)
delays kernel entry on the other CPUs; threads of one program do not run in
parallel. A kernel thread that computes without blocking holds the lock between
ticks; kernel threads on other CPUs then only get it in turns at ticks and
slow down a lot (a kernel test that checked this on four CPUs sometimes did
not finish within a minute; its threads now run on CPU 0). Velora has no such
threads.

## D44. USB: только корневые порты, опрос, boot protocol / USB: root ports only, polling, boot protocol

**2026-11-27**

**RU.** Терминальной системе от USB нужны клавиатура, мышь и флешки.
Клавиатура и мышь работают в boot protocol: отчёт фиксированного формата,
без разбора дескрипторов отчётов HID; так их поддерживает и прошивка.
Клавиши переводятся в скан-коды PS/2, поэтому раскладки и сочетания
клавиш общие с PS/2. Контроллер опрашивается, без прерываний MSI, как диски
(D21): поток `usb` раз в 10 мс читает события, этого достаточно для
набора текста. Поддерживаются устройства на корневых портах, подключённые
при загрузке. Хабы и горячее подключение — отдельная большая работа
(перечисление через хаб, transaction translator для медленных устройств
за хабом USB 2.0); это записано как NOT IMPLEMENTED.

**EN.** A terminal system needs a keyboard, a mouse and flash drives from USB.
The keyboard and mouse use the boot protocol: a fixed report format with no HID
report descriptor parsing, which is also how firmware supports them. Keys
become PS/2 scancodes, so layouts and key combinations are shared with PS/2.
The controller is polled without MSI interrupts, like the disks (D21): the
`usb` thread reads events every 10 ms, which is enough for typing. Devices on
root ports present at boot are supported; hubs and hot plugging are separate
large work (enumeration through a hub, transaction translators for slow
devices behind a USB 2.0 hub) and are marked NOT IMPLEMENTED.

## D45. DNS и HTTP в пространстве пользователя / DNS and HTTP in user space

**2026-11-27**

**RU.** Ядро даёт только сокеты UDP и TCP. Разрешение имён и HTTP —
обычный код программ: DNS в vlibc (его используют все программы), HTTP —
в библиотеке для `fetch`. Обе части без ввода-вывода и проверяются на
хосте. Код DNS входит в vlibc, поэтому крейт `velora-dns` под LGPL.
Переменная `DNS_SERVER` позволяет выбрать сервер без изменения настроек
DHCP; через неё же тест `net` использует DNS-сервер на хосте и не зависит
от интернета. HTTPS не поддерживается: своей реализации TLS нет, а
сторонние библиотеки проект не берёт.

**EN.** The kernel only provides UDP and TCP sockets. Name resolution and HTTP
are ordinary program code: DNS in vlibc (every program uses it), HTTP in a
library for `fetch`. Both have no I/O of their own and are tested on the host.
The DNS code is part of vlibc, so `velora-dns` is LGPL. `DNS_SERVER` picks a
server without changing the DHCP settings; the `net` test uses it to reach a
DNS server on the host and does not depend on the Internet. HTTPS is not
supported: there is no TLS implementation of our own, and the project does not
take third-party libraries.

## D46. Изображения в терминале: Kitty graphics protocol / Images in the terminal: the Kitty graphics protocol

**2026-11-27**

**RU.** Консоль ядра показывает изображения по протоколу Kitty, а не Sixel.
Причины:
- Kitty передаёт пиксели как есть (RGB или RGBA с прозрачностью, 24 бита
  на цвет). Sixel — палитра до 256 цветов и полосы по 6 пикселей; логотип
  пришлось бы квантовать, а края сглаживания теряются.
- Размер на экране задаётся в ячейках (`c`, `r`), поэтому изображение
  естественно привязано к шрифту, а масштаб 1–4 консоли учитывается сам.
- Разбор проще и легко проверяется на хосте: пары `ключ=значение` и base64,
  а не отдельный язык команд с повторами и сменой палитры.
- Есть стандартная проверка поддержки: запрос `a=q` с идентификатором и
  следом DA1 (`CSI c`). Программа читает ответы до ответа DA1, поэтому не
  зависает, если графики нет.
- Протокол поддерживают kitty, WezTerm, Konsole, Ghostty, так что
  программы Velora с той же логикой работают и в этих терминалах.
Sixel остаётся возможным позже: изображение и масштабирование
(`velora-gfx::image`) не зависят от протокола.

Изображение рисуется прямо в текстовый экран (в буфер кадра), а не
отдельным слоем. Так оно прокручивается вместе с текстом и не требует
повторной отрисовки, но удалить уже показанное изображение командой `a=d`
нельзя: оно исчезает, когда поверх пишется текст или экран прокручивается.
Хранимые изображения (`a=t`, `a=p`) ограничены 16 МиБ, одно изображение —
8 МиБ. Ядро не разбирает PNG (`f=100`) и сжатие (`o=z`): это лишний разбор
сложных данных в ядре и рост кучи ядра, которая не возвращает страницы.
Программы декодируют PNG сами (`velora-gfx::png`) и шлют RGBA ровно в
размере ячеек, поэтому терминалу не нужно масштабировать.

Последовательный порт и журнал ядра получают вывод программ без строк
APC: base64 изображения там бесполезен и занял бы журнал. Ответы
терминала (графика, DA1, размеры `CSI 14/16/18 t`) попадают во ввод
программы, как в любом терминале. Размер ячейки в пикселях программа
узнаёт вызовом `tty(3)` (`ENODEV` без экрана — графики нет).

**EN.** The kernel console shows images with the Kitty protocol, not Sixel.
Reasons:
- Kitty carries pixels as they are (RGB or RGBA with transparency, 24-bit
  colour). Sixel is a palette of at most 256 colours in 6-pixel bands; the
  logo would have to be quantised and its anti-aliased edges would suffer.
- The size on screen is given in cells (`c`, `r`), so an image is tied to
  the font, and the console's scale 1-4 is taken into account by itself.
- Parsing is simpler and easy to test on the host: `key=value` pairs and
  base64, not a command language with repeats and palette changes.
- There is a standard support check: an `a=q` query with an id followed by
  DA1 (`CSI c`). A program reads answers up to the DA1 answer, so it does
  not hang when there are no graphics.
- kitty, WezTerm, Konsole and Ghostty support the protocol, so Velora
  programs with the same logic also work in those terminals.
Sixel stays possible later: images and scaling (`velora-gfx::image`) do not
depend on the protocol.

An image is drawn straight into the text screen (the frame buffer), not as a
separate layer. It scrolls with the text and needs no redrawing, but an image
already shown cannot be removed with `a=d`: it goes away when text is written
over it or the screen scrolls. Stored images (`a=t`, `a=p`) are limited to
16 MiB, one image to 8 MiB. The kernel does not decode PNG (`f=100`) or
compression (`o=z`): that would be more parsing of complex data in the kernel
and growth of the kernel heap, which does not give pages back. Programs decode
PNG themselves (`velora-gfx::png`) and send RGBA in the exact size of the
cells, so the terminal does not have to scale.

The serial port and the kernel log get program output without APC strings:
image base64 is useless there and would fill the log. The terminal's answers
(graphics, DA1, sizes `CSI 14/16/18 t`) become the program's input, as in any
terminal. A program learns the cell size in pixels with `tty(3)` (`ENODEV`
without a screen: no graphics).

## D47. Графическая основа: velora-ui, Inter и fontdue / Graphical toolkit: velora-ui, Inter and fontdue

**2026-11-27**

**RU.** Графический установщик и будущая оболочка Lumi рисуются в
программе, а не в ядре. Ядро отдаёт framebuffer одному процессу
(`display`), даёт события клавиш (`/dev/keyboard`) и мыши (`/dev/mouse`,
теперь и PS/2). Причины: сложный разбор шрифтов и картинок остаётся вне
ядра; ошибка программы не роняет систему; консоль сохраняет свой текст и
возвращается, когда программа завершается, в том числе аварийно.

Шрифт — Inter (OFL 1.1, есть кириллица), растеризация — fontdue (MIT). Это
первые внешние crate проекта (D42), добавлены с согласия владельца
репозитория. Свой растеризатор TrueType писался бы дольше и был бы хуже
проверен. fontdue собирается без std (с hashbrown) и работает без SSE:
программы собираются без плавающей точки в регистрах, она эмулируется.
Поэтому загрузка трёх начертаний под TCG занимает около 2 с; глифы
кешируются. Остальное рисование целочисленное: функции расстояния в 1/256
пикселя дают сглаживание, тени и дуги без плавающей точки.

Элементы — в немедленном режиме: программа каждый кадр рисует экран
заново из своего состояния, а `Ui` помнит только фокус, открытый список и
прокрутку. Так экраны установщика остаются простыми функциями от
состояния, и их можно отрисовать на хосте и сравнить. Клавиши подаются по
одной на кадр, чтобы их порядок не терялся между элементами. Масштаб
дробный (пикселей на единицу дизайна), поэтому один макет работает в
1280x720, 1920x1080 и 4K.

**EN.** The graphical installer and the future Lumi shell draw in a program,
not in the kernel. The kernel gives the frame buffer to one process
(`display`), key events (`/dev/keyboard`) and mouse events (`/dev/mouse`, now
from PS/2 too). Reasons: complex font and image parsing stays out of the
kernel; a crashing program does not bring the system down; the console keeps
its text and comes back when the program exits, also when it crashes.

The font is Inter (OFL 1.1, with Cyrillic) and rasterisation is fontdue
(MIT). They are the project's first external crates (D42), added with the
repository owner's consent. An own TrueType rasteriser would take longer and
be less proven. fontdue builds without std (with hashbrown) and works without
SSE: programs are built without floating point in registers, so it is
emulated. Loading three faces therefore takes about 2 s under TCG; glyphs are
cached. All other drawing is integer: distance functions in 1/256 pixel give
anti-aliasing, shadows and arcs without floating point.

Widgets are immediate-mode: every frame the program draws the screen again
from its state, and `Ui` only remembers the focus, the open list and scroll
positions. Installer screens thus stay plain functions of the state and can
be drawn on the host and compared. Keys are fed one per frame so their order
is not lost between widgets. The scale is fractional (pixels per design unit),
so one layout works at 1280x720, 1920x1080 and 4K.

## D48. Стартовое меню и графический установщик / Start menu and graphical installer

**2026-11-27**

**RU.** Live-образ сначала показывает меню (`/bin/velora-start`, его
запускает ядро, если корень — initramfs): графическая установка (по
умолчанию, через 10 с), установка в терминале (`velora-setup`) и
Live-режим (дальше обычный вход). Меню — программа, а не пункты Limine:
оно нарисовано в стиле установщика, выбирается мышью и клавиатурой и без
экрана показывает текстовый вариант. Меню и установщик — одна программа,
поэтому шрифты загружаются один раз. `velora.nomenu` в командной строке
отключает меню.

Логика установки одна: crate `velora-install` (разметка, файловые
системы, загрузчик, пользователи, настройки) с прогрессом по четырём
этапам. `velora-setup` и графический установщик только собирают план.
Источник файлов — интерфейс `Source`, поэтому вся установка проверяется
тестом на хосте на диске в памяти.

Отличия от концепта (`docs/design/installer-concept.png`):
- Шаг «Пользователь» (имя, пароль, пароль root) и отдельный экран
  подтверждения: без них установить систему нельзя, в концепте их нет.
- Ручная разметка и «Эксперт» открывают экран разделов с настоящими
  параметрами (размер EFI, размер корня, метка, таблица GPT), а не
  заглушку.
- Три стиля курсора, а не четыре: в `assets/cursors` три картинки.
- Список языков без японского: в Inter нет иероглифов.
- Язык установщика — русский или английский (переводов больше нет).
- Выбор оформления, языка, региона и раскладки пишется в
  `/etc/velora/*.conf`. NOT IMPLEMENTED: система их пока не читает
  (оболочки Lumi нет, раскладки по-прежнему EN/RU с Alt+Shift).

Неизменные части экрана (фон, окно, волны, панель) рисуются один раз и
копируются в каждый кадр: программы собраны без SSE, и большие мягкие
фигуры — самое медленное. Под TCG это ускорило кадр примерно в 10 раз.

QEMU с OVMF не даёт режимов больше 2560x1440, поэтому тест в QEMU снимает
1280x720, 1920x1080 и 2560x1440, а 3840x2160 проверяется отрисовкой тех
же экранов на хосте (`cargo run -p velora-installer-gui --example
screens`).

**EN.** The Live image first shows a menu (`/bin/velora-start`, run by the
kernel when the root is the initramfs): graphical installation (the default,
after 10 s), installation in the terminal (`velora-setup`) and the Live
system (the usual login follows). The menu is a program, not Limine entries:
it has the installer's look, works with the mouse and the keyboard and falls
back to text without a screen. Menu and installer are one program, so the
fonts load once. `velora.nomenu` on the command line turns the menu off.

There is one installation logic: the `velora-install` crate (partitions,
file systems, bootloader, users, settings) with progress in four phases.
`velora-setup` and the graphical installer only build the plan. Files come
through the `Source` interface, so the whole installation is tested on the
host on a disk in memory.

Differences from the concept (`docs/design/installer-concept.png`):
- A "User" step (name, password, root password) and a separate
  confirmation screen: the system cannot be installed without them, and the
  concept has neither.
- Manual partitioning and Expert mode open a partitions screen with real
  settings (EFI size, root size, label, the GPT table), not a placeholder.
- Three pointer styles, not four: `assets/cursors` has three pictures.
- No Japanese in the language list: Inter has no CJK glyphs.
- The installer speaks Russian or English (no other translations yet).
- Appearance, language, region and layout choices are written to
  `/etc/velora/*.conf`. NOT IMPLEMENTED: the system does not read them yet
  (there is no Lumi shell; layouts are still EN/RU with Alt+Shift).

The parts of a screen that do not change (backdrop, window, waves, panel)
are drawn once and copied into each frame: programs are built without SSE and
large soft shapes are the slowest thing to draw. Under TCG this made a frame
about 10 times faster.

QEMU with OVMF offers no modes above 2560x1440, so the QEMU test captures
1280x720, 1920x1080 and 2560x1440, and 3840x2160 is checked by drawing the
same screens on the host (`cargo run -p velora-installer-gui --example
screens`).

## D49. Чёткость, акцент и анимации установщика; абсолютный указатель / Installer sharpness, accent and animations; absolute pointer

**2026-11-27**

**RU.** Режим экрана берётся из EDID монитора: его выбирает Limine, это
родное разрешение. Окно рисуется сразу в нём, в одном из масштабов 1x,
1.25x, 1.5x, 1.75x, 2x, 2.5x, 3x, 3.5x, 4x (самый большой, который
помещается с полями): 1280x720 — 1.5x, 1920x1080 — 2x, 2560x1440 — 3x,
3840x2160 — 4x. Готовый кадр никогда не растягивается. Текст смешивается
в линейном свете (гамма 2) с небольшим усилением контраста. Размер шрифта
меняется не больше чем на 6%, чтобы высота строчных букв была целой в
пикселях: это лёгкая замена хинтингу, которого в fontdue нет. Картинки
уменьшаются усреднением площади.

Размытость, которую было видно в окне QEMU, давала Windows: при
масштабе экрана 125% окно QEMU без поддержки DPI растягивается системой
(окно для кадра 1280x720 занимало 1620x949 пикселей). `tools/run-qemu.sh`
задаёт процессу QEMU `SDL_WINDOWS_DPI_AWARENESS=permonitorv2`, и пиксели
выводятся 1:1.

Акцентный цвет темы (по умолчанию янтарный #FFB547) используется для
включённых переключателей, выделения, кольца фокуса, прогресса и отметок
шагов. Тема и акцент, выбранные на экране «Оформление» или переключателем
в боковой панели, применяются к установщику сразу, через плавный переход
(260 мс, ease-in-out). Переход сделан смешиванием последнего показанного
кадра с новыми: так меняются все цвета разом, а фон с волнами не нужно
перерисовывать в каждом кадре. Шаги сменяются сдвигом с затуханием.
Состояния элементов (наведение, нажатие, фокус, неактивное) анимируются
значениями `Tweens` с часами программы. Переключатель «Эффекты анимации»
выключает анимации и в самом установщике. Кадры анимации идут примерно 60
раз в секунду. Таймер ядра тикает 100 раз в секунду, поэтому последние
миллисекунды до кадра программа ждёт через `yield`. На экран пишутся
только пиксели, которые отличаются от показанных.

Абсолютный указатель: USB-устройства HID, которые не являются
boot-мышью (планшеты, в том числе `usb-tablet` в QEMU), читаются в report
protocol по их report descriptor (`velora-usb::report`). События с
позицией идут в `/dev/pointer` (пакеты по 8 байт); `/dev/mouse` остаётся
для относительных. С планшетом мышь в окне QEMU работает без захвата, а
тест `clicks` щёлкает в точные пиксели через QMP.

Тест «каждый элемент реагирует»: образ `velora-uitest.iso`
(`velora.uitest` в командной строке) печатает после каждого изменения
дайджест состояния, фокус, открытый список, последний сработавший элемент
и области всех интерактивных элементов. `tests/uiclick.py` проходит все
экраны и щёлкает каждый элемент, а для списков ещё крутит колесо.
Проверяется, что состояние изменилось.

NOT IMPLEMENTED: синхронизации с кадровой развёрткой нет (у GOP её нет),
поэтому разрывы изображения возможны, хотя кадр всегда выводится целиком
из заднего буфера. Пока кадр рисуется, указатель мыши не двигается.
Хинтинга по командам шрифта нет.

**EN.** The display mode comes from the monitor's EDID: Limine picks it,
and it is the native resolution. The window is drawn in it directly at one
of the scales 1x, 1.25x, 1.5x, 1.75x, 2x, 2.5x, 3x, 3.5x, 4x (the largest
that fits with margins): 1280x720 gets 1.5x, 1920x1080 2x, 2560x1440 3x,
3840x2160 4x. A finished frame is never stretched. Text is blended in linear
light (gamma 2) with a slight contrast boost. Font sizes change by at most 6%
so that the x-height is a whole number of pixels, a light stand-in for the
hinting fontdue does not have. Pictures are scaled down by area averaging.

The blur seen in the QEMU window came from Windows: with 125% display scaling
it stretches a QEMU window that is not DPI-aware (the window for a 1280x720
frame took 1620x949 pixels). `tools/run-qemu.sh` sets
`SDL_WINDOWS_DPI_AWARENESS=permonitorv2` for the QEMU process, and pixels
are shown 1:1.

A theme's accent colour (warm amber #FFB547 by default) is used for switches
that are on, selections, the focus ring, progress and the step marks. The
theme and accent picked on the Appearance screen or with the switch in the
sidebar apply to the installer at once, with a smooth transition (260 ms,
ease-in-out). The transition mixes the last frame shown with the new frames:
every colour changes together, and the backdrop with its waves need not be
drawn again for each frame. Steps change with a slide and a fade. Widget
states (hover, press, focus, disabled) are animated with `Tweens` on the
program's clock. The "Animations" switch also turns the installer's own
animations off. Animation frames come about 60 times a second; the kernel
timer ticks 100 times a second, so the program waits the last milliseconds
before a frame with `yield`. Only pixels that differ from what is shown are
written to the screen.

Absolute pointer: USB HID devices that are not boot mice (tablets, QEMU's
`usb-tablet` among them) are read in report protocol using their report
descriptor (`velora-usb::report`). Events with a position go to
`/dev/pointer` (8-byte packets); `/dev/mouse` keeps the relative ones. With
the tablet the mouse works in a QEMU window without being grabbed, and the
`clicks` test clicks exact pixels through QMP.

The "every element reacts" test: `velora-uitest.iso` (`velora.uitest` on the
command line) prints, after every change, a digest of the state, the focus,
the open list, the widget that last took input and the areas of all
interactive widgets. `tests/uiclick.py` goes through every screen, clicks
each element (and turns the wheel over the lists) and checks that the state
changed.

NOT IMPLEMENTED: there is no synchronisation with the display refresh (GOP
has none), so tearing is possible, although a frame is always shown whole
from the back buffer. The pointer does not move while a frame is being drawn.
There is no hinting from the font's instructions.

## D50. Кадры установщика по изменениям, ожидание ввода, освобождение диска, тема Velora Gamers / Installer frames by changes, waiting for input, freeing a disk, the Velora Gamers theme

**2026-11-27**

**RU.** Кадр рисуется только там, где что-то изменилось. Перед кадром
`Ui::plan` смотрит на прошлый кадр: какие элементы указатель покинул
или задел и у каких анимированных значений текущее значение по часам
отличается от нарисованного. Результат: ничего (кадр пропускается, на
экране двигается только указатель), часть экрана (объединение областей
этих элементов с запасом под кольцо фокуса и тени) или весь экран (клавиши,
щелчки, колесо, смена экрана, переходы и анимации без своего элемента).
Часть рисуется с отсечением `Canvas::set_frame`: вложенные области
отсечения не выходят за неё, фон копируется из кеша только в ней, на
экран выводится только она. Тест на хосте рисует установщик двумя
способами и после каждого шага сравнивает картинки попиксельно.

Кадры идут не чаще 60 в секунду (16,7 мс от начала прошлого кадра), время
анимаций берётся из часов, а не из числа кадров. Указатель рисуется
отдельным слоем сразу при чтении его событий. Программа больше не спит по
10 мс между опросами: новый вызов `input_wait(timeout_ms)` (52) будит её,
как только ядро получило событие клавиатуры или указателя. Таймер ядра
по-прежнему 100 Гц, поэтому последние миллисекунды до кадра ждутся через
`yield`.

Смешивание цветов считает красный и синий одним умножением, а заливки
(прямоугольники, внутренность скруглённых фигур) — по два пикселя в `u64`.
Результат побитово совпадает с прежней формулой. Скруглённые
прямоугольники и обводки считают расстояние до края только у краёв:
фигуры выпуклые, середина строки — один отрезок. Квадратный корень для
текста берётся из таблицы, а не двоичным поиском.

Счётчик кадров (F12) показывает кадры и перемещения указателя в секунду,
среднее и самое долгое время кадра и интервал между кадрами анимации;
каждую секунду строка уходит и в консоль. Режим `qemu_run.py fps` по
этим строкам меряет установщик в 1920x1080.

Диски. Ядро при загрузке подключает каждую известную ему файловую систему
в `/mnt`. Раньше установщик прятал такие диски, и диск с прошлой
установкой выглядел как «диск не найден». Теперь он показывает их с
предупреждением, а перед записью отключает их разделы новым вызовом
`umount(path)` (31, только root, только `/mnt/…`, `EBUSY`, пока открыты
файлы). Журнал паники перестаёт писаться на отключённый раздел. Диск, с
которого запущена система, не предлагается: Limine сообщает GUID раздела,
с которого загружено ядро (запрос Executable File), и `diskinfo` ставит
такому диску флаг `disk_kind::BOOT`. Кнопка «Обновить» перечитывает список.
Новые диски после загрузки не появятся, потому что ядро ищет их только
при загрузке.

Тема Velora Gamers заменила Midnight: только цвета (фон #0B0A12, панели
#15131F, обводки #2A2540, текст #F4F2FF и #A39DBD, акцент #A855F7, детали
#38E1FF). Контраст текста не ниже 4.5:1, это проверяет тест. У темы свой
акцент: если акцент не меняли, при выборе темы он становится её
собственным. Цвет деталей (`Theme::detail`) — курсор ввода и значок
текущего этапа установки. У тёмной и светлой тем он совпадает с акцентом.

**EN.** A frame is drawn only where something changed. Before a frame
`Ui::plan` looks at the last one: which widgets the pointer left or touched,
and which animated values differ, on the clock, from the value drawn. The
answer is nothing (the frame is skipped; only the pointer moves on the
screen), a part of the screen (the union of those widgets' areas with a
margin for focus rings and shadows) or the whole screen (keys, clicks, the
wheel, a new screen, transitions and animations without a widget of their
own). A part is drawn with the `Canvas::set_frame` clip: nested clips stay
inside it, the background is copied from its cache only there, and only that
part is shown. A host test draws the installer both ways and compares the
pictures pixel by pixel after every step.

Frames come at most 60 times a second (16.7 ms from the start of the last
one); animations take their time from the clock, not from frame counts. The
pointer is a layer of its own, drawn as soon as its events are read. The
program no longer sleeps 10 ms between polls: the new call
`input_wait(timeout_ms)` (52) wakes it as soon as the kernel has a key or
pointer event. The kernel timer is still 100 Hz, so the last milliseconds
before a frame are waited by yielding.

Colour blending computes red and blue with one multiplication, and fills
(rectangles, the inside of rounded shapes) blend two pixels per `u64`; the
result is bit for bit that of the earlier formula. Rounded rectangles and
outlines compute the distance to the edge only near the edges: the shapes are
convex, the middle of a row is one span. The square root for text comes from
a table instead of a binary search.

The frame counter (F12) shows frames and pointer moves per second, the
average and longest frame and the interval between frames of an animation;
every second the line also goes to the console. `qemu_run.py fps` measures
the installer at 1920x1080 from these lines.

Disks. At boot the kernel mounts every file system it knows under `/mnt`. The
installer used to hide such disks, so a disk with an earlier installation
looked like "no disks found". It now offers them with a warning and, before
writing, unmounts their partitions with the new call `umount(path)` (31, root
only, only `/mnt/...`, `EBUSY` while files are open); the panic log stops
using an unmounted partition. The disk the system was started from is not
offered: Limine reports the GUID of the partition the kernel came from (the
Executable File request), and `diskinfo` gives that disk the
`disk_kind::BOOT` flag. Refresh reads the list again. Disks connected after
boot do not appear, because the kernel looks for disks only at boot.

The Velora Gamers theme replaced Midnight: colours only (backdrop #0B0A12,
panels #15131F, borders #2A2540, text #F4F2FF and #A39DBD, accent #A855F7,
details #38E1FF). Text contrast is at least 4.5:1, which a test checks. The
theme has an accent of its own: when the accent was not changed, choosing the
theme switches to it. The detail colour (`Theme::detail`) is used for the text
cursor and the mark of the running installation step; in the dark and light
themes it is the accent.

## D51. Прерывания xHCI (MSI-X, MSI) и указатель по кадрам / xHCI interrupts (MSI-X, MSI) and the pointer by frames

**2026-11-28**

**RU.** Раньше поток `usb` читал кольцо событий xHCI раз в тик (100 Гц):
мышь читалась 100 раз в секунду, а движения рывками ложились на кадры
60 Гц (в один кадр то одно движение, то два). Теперь контроллер получает
прерывание: сначала MSI-X, если его нет — MSI, если нет и его (или в
командной строке `velora.xhci_poll`) — прежний опрос по тику. Вектор
0x48 + номер контроллера (до четырёх), прерывание идёт на процессор,
который настраивал контроллер (процессор 0). Модерация прерываний —
не чаще раза в миллисекунду.

Обработчик сам читает кольцо событий (как PS/2 — свои байты): если бы он
только будил поток `usb`, каждый отчёт мыши переключал бы потоки и
адресные пространства, сбрасывая TLB программы, которая рисует.
Блокировка контроллера выключает прерывания, пока занята, поэтому на
своём процессоре обработчик всегда её получает; если её держит другой
процессор (ввод-вывод флешки, который сам разбирает пришедшие отчёты),
поток `usb` смотрит кольцо на следующем тике. Поток закреплён за
процессором 0 и проверяет флаг с выключенными прерываниями, поэтому
пробуждение не теряется.

На каждой конечной точке клавиатуры и мыши стоят восемь передач вместо
одной: контроллер может отдать несколько отчётов, пока драйвер не
посмотрел кольцо. При опросе раньше доходил один отчёт за тик, очередь
планшета QEMU переполнялась, и терялись нажатия кнопок.

Очередь мыши в ядре складывает движение с последним событием, если
ничего не теряется (те же кнопки, нет колеса, сумма помещается; у
планшета — последняя позиция), поэтому программа, которая читает раз в
кадр, получает всё движение.

Программа рисует указатель один раз за кадр, там, куда его поставил
последний отчёт: чтение ввода только двигает позицию, все движения между
кадрами складываются. Перед выводом кадра ввод читается ещё раз; кадр,
в котором ничего не меняется, только показывает указатель. Указатель
двигается не чаще 60 раз в секунду, без рывков от лишних отчётов.

Проверка: тест ядра требует MSI-X у xHCI в QEMU и прерываний во время
движения мыши; `qemu_run.py fps` требует больше 150 чтений указателя в
секунду с прерываниями (QMP шлёт 250 позиций), не больше 62 показов
указателя в секунду, а с `--xhci-polled` (у xHCI в QEMU выключены MSI и
MSI-X) — опрос и работающий указатель.

**EN.** The `usb` thread used to read the xHCI event ring every tick
(100 Hz): the mouse was read 100 times a second, and its movements fell
unevenly on 60 Hz frames (one movement in a frame, then two). The controller
now gets an interrupt: MSI-X, else MSI, else (or with `velora.xhci_poll` on
the command line) the old polling every tick. The vector is 0x48 plus the
controller's number (up to four); the interrupt goes to the CPU that set the
controller up (CPU 0). Interrupt moderation allows at most one a millisecond.

The handler reads the event ring itself (as the PS/2 handler reads its
bytes): if it only woke the `usb` thread, every mouse report would switch
threads and address spaces and flush the TLB of the program that draws. The
controller lock turns interrupts off while held, so on its own CPU the
handler always gets it; if another CPU holds it (flash drive I/O, which
handles the reports that arrive meanwhile), the `usb` thread looks at the
ring on the next tick. That thread runs on CPU 0 and checks its flag with
interrupts off, so no wake-up is lost.

Each keyboard and mouse endpoint has eight transfers queued instead of one:
the controller can deliver several reports before the driver looks at the
ring. With polling only one report a tick got through, QEMU's tablet queue
overflowed, and button presses were lost.

The kernel's mouse queue joins a movement to the last event when nothing is
lost (the same buttons, no wheel, the sum fits; for a tablet, the last
position), so a program that reads once a frame gets the whole movement.

The program draws the pointer once a frame, where the last report put it:
reading input only moves the position, and all movements between frames add
up. Input is read once more before a frame is shown; a frame in which nothing
changes only shows the pointer. The pointer moves at most 60 times a second,
without jumps from extra reports.

Tests: the kernel test requires MSI-X on QEMU's xHCI and interrupts while
the mouse moves; `qemu_run.py fps` requires more than 150 pointer reads a
second with interrupts (QMP sends 250 positions), at most 62 pointer moves
shown a second, and with `--xhci-polled` (MSI and MSI-X off on QEMU's xHCI)
polling and a working pointer.

## D52. Графическая оболочка Lumi / The Lumi graphical shell

**2026-11-28**

**RU.** Lumi — одна программа (`/bin/lumi`, логика и рисование — crate
`velora-lumi` в `lumi/`, проверяется на хосте с системой в памяти,
`lumi::fake`). Она владеет экраном (`display`), рисует экран входа, окна,
панель, меню приложений и встроенные приложения: Терминал, Файлы,
Настройки, О системе, просмотр картинок. Внешние программы работают в
окне терминала (vsh, Plume). Протокола для окон чужих графических
программ пока нет (NOT IMPLEMENTED): экран отдаётся одному процессу
(D47), а разделяемой памяти и передачи событий между процессами в ядре
нет. Одна программа — меньше памяти и проще, чем сервер и клиенты.

Как рисуется:
- У каждого окна своя картинка (заголовок и содержимое); перерисовывается
  только изменённая часть (`Ui::plan`, у терминала — изменённые строки).
- Экран собирается только в изменённых областях: обои (нарисованы один
  раз), тени (из кэша по девяти частям), картинки окон со скруглёнными
  углами, панель и меню. Перемещение окна копирует его картинку.
- Тест на хосте сравнивает кадры по областям с полной перерисовкой
  попиксельно.
- Указатель — отдельный слой (`Screen`, D51).
- Кадры идут 60 раз в секунду, пока что-то движется; в покое программа
  спит в `input_wait` до ввода, вывода терминала или таймера (часы,
  курсор).
- Анимации: окна появляются с масштабом и прозрачностью, закрываются,
  сворачиваются к значку на панели, разворачиваются и прилипают к краям
  плавно; меню приложений растёт из 94% и проявляется; тема меняется
  переходом всего экрана (`Transition`, D49).

Запуск: ядро запускает Lumi на установленной системе и в Live-режиме,
если в стартовом меню выбран рабочий стол (пункт 3; клавиша 4 — Live в
консоли, её используют тесты). Lumi завершается с кодом 2 без экрана,
без шрифтов или при `session=console` в `/etc/velora/desktop.conf`
(`velora-setup --console`, вопрос в интерактивной установке), и тогда
ядро запускает консольный `login`. `velora.text` в командной строке тоже
выбирает консоль. Выход из сеанса — код 0, ядро снова запускает Lumi.

Вход: Lumi работает от root до входа, проверяет пароль (PBKDF2, как
`login`) и делает `setuid` в пользователя; дальше окна, файлы и программы
терминала принадлежат ему. Выключать и перезагружать может процесс,
которому принадлежит экран (человек перед ним, как с кнопкой питания), а
не только root.

Настройки: системные — `/etc/velora/desktop.conf` (пишет установщик),
регион — `locale.conf`; изменения из Настроек пишутся в
`~/.config/velora/desktop.conf` (у пользователя нет прав на `/etc`).
Часовой пояс — по региону: смещение и правило летнего времени (ЕС, США).

Отличия от концептов: значка почты из первого концепта нет (почтовой
программы нет); значок громкости сообщает, что звукового устройства нет
(NOT IMPLEMENTED: драйвера звука нет); значок сети показывает проводное
подключение (Wi-Fi нет). Масштаб интерфейса — ступени установщика по
высоте экрана: 1x при 720 строках, 1.25x при 1080, 1.5x при 1440, 2.5x
при 2160.

**EN.** Lumi is one program (`/bin/lumi`; its logic and drawing are the
`velora-lumi` crate in `lumi/`, tested on the host with an in-memory system,
`lumi::fake`). It owns the screen (`display`) and draws the login screen,
windows, the panel, the app menu and the built-in apps: Terminal, Files,
Settings, About and a picture viewer. Other programs run in a terminal
window (vsh, Plume). There is no protocol for windows of other graphical
programs yet (NOT IMPLEMENTED): the screen goes to one process (D47) and the
kernel has no shared memory or event passing between processes. One program
uses less memory and is simpler than a server with clients.

How it draws:
- Each window has its own picture (title bar and content); only the part
  that changed is drawn again (`Ui::plan`; the terminal draws changed rows).
- The screen is put together only in the changed areas: the wallpaper
  (drawn once), shadows (from a cached nine-part picture), the window
  pictures with rounded corners, the panel and the menus. Moving a window
  copies its picture.
- A host test compares frames drawn by areas with a full redraw, pixel by
  pixel.
- The pointer is a separate layer (`Screen`, D51).
- Frames come 60 times a second while something moves; at rest the program
  sleeps in `input_wait` until input, terminal output or a timer (the clock,
  the cursor).
- Animations: windows open with scale and opacity, close, minimize to their
  panel icon, maximize and snap to the screen edges smoothly; the app menu
  grows from 94% and fades in; a theme changes with a transition of the
  whole screen (`Transition`, D49).

Start: the kernel starts Lumi on an installed system, and on the Live system
when the start menu chooses the desktop (item 3; key 4 is the Live system on
the console, which the tests use). Lumi ends with status 2 without a screen,
without fonts or with `session=console` in `/etc/velora/desktop.conf`
(`velora-setup --console`, a question in the interactive installation), and
the kernel then starts the console `login`. `velora.text` on the command
line also chooses the console. Logging out is status 0, and the kernel
starts Lumi again.

Login: Lumi runs as root until someone logs in, checks the password (PBKDF2,
as `login`) and calls `setuid` to become that user; windows, files and
terminal programs then belong to them. The process that owns the screen (the
person in front of it, as with the power button) may power off and restart,
not only root.

Settings: the system's in `/etc/velora/desktop.conf` (written by the
installer), the region in `locale.conf`; changes made in Settings go to
`~/.config/velora/desktop.conf` (users cannot write `/etc`). The time zone
follows the region: an offset and a daylight saving rule (EU, US).

Differences from the concepts: no mail icon from the first concept (there is
no mail program); the volume icon tells that there is no sound device (NOT
IMPLEMENTED: no sound driver); the network icon shows the wired connection
(no Wi-Fi). The interface scale uses the installer's steps by screen height:
1x at 720 lines, 1.25x at 1080, 1.5x at 1440, 2.5x at 2160.

## D53. Псевдотерминалы / Pseudo-terminals

**2026-11-28**

**RU.** vsh и Plume работают с консолью ядра: режим строки, сырые
клавиши, размер, процесс на переднем плане. Чтобы они шли в окне Lumi без
изменений, в ядре есть псевдотерминалы: вызов `pty` (53) создаёт пару
дескрипторов. Мастер (терминал Lumi) пишет набранные байты и читает вывод
без ожидания (`EAGAIN` — пусто, 0 — программа закрыла свою сторону).
Подчинённая сторона для программы выглядит как консоль (`fstat` —
символьное устройство), а вызовы `tty` процесса, у которого дескриптор 0 —
подчинённая сторона, действуют на его псевдотерминал. Дисциплина линии
(эхо, Backspace по символам UTF-8, Ctrl+C и Ctrl+D, сырой режим) вынесена
в crate `velora-tty` с тестами на хосте. Новый вывод будит программу в
`input_wait`, поэтому Lumi не опрашивает терминалы в покое. Изображений
по протоколу Kitty в псевдотерминале нет (`CELL_SIZE` — `ENODEV`):
velofetch показывает логотип текстом. Сигналов нет, поэтому о новом
размере программа узнаёт, только спросив его снова (Plume — при запуске).

**EN.** vsh and Plume work with the kernel console: line mode, raw keys, the
size, the foreground process. To run them unchanged in a Lumi window, the
kernel has pseudo-terminals: the `pty` call (53) makes a pair of
descriptors. The master (the Lumi terminal) writes typed bytes and reads
output without waiting (`EAGAIN`: nothing yet; 0: the program closed its
side). The slave side looks like the console to the program (`fstat` gives a
character device), and the `tty` calls of a process whose descriptor 0 is a
slave act on its pseudo-terminal. The line discipline (echo, Backspace by
UTF-8 characters, Ctrl+C and Ctrl+D, raw mode) is the `velora-tty` crate,
tested on the host. New output wakes the program waiting in `input_wait`,
so Lumi does not poll terminals at rest. There are no Kitty graphics in a
pseudo-terminal (`CELL_SIZE` gives `ENODEV`): velofetch shows its logo as
text. There are no signals, so a program learns about a new size only when
it asks again (Plume, at start).

## D54. Клавиши для графических программ / Keys for graphical programs

**2026-11-28**

**RU.** `/dev/keyboard` теперь сообщает клавишу Super (флаг `SUPER`), сами
клавиши-модификаторы (нажатие и отпускание: `SHIFT_KEY`, `CTRL_KEY`,
`ALT_KEY`, `SUPER_KEY`), Insert и Ctrl+буква как букву с флагом Ctrl (на
консоли это управляющий символ). Без отпускания Alt нельзя сделать
Alt+Tab, без Super — меню по клавише Super. В velora-ui клавиши с Ctrl, Alt
или Super приходят как `Event::Chord` (элементы их не берут, поэтому Ctrl+C
не печатает «c» в поле установщика); модификаторы и правая кнопка мыши —
только если программа попросит (`Screen::extended_input`).

**EN.** `/dev/keyboard` now reports the Super key (the `SUPER` flag), the
modifier keys themselves (presses and releases: `SHIFT_KEY`, `CTRL_KEY`,
`ALT_KEY`, `SUPER_KEY`), Insert, and Ctrl+letter as the letter with the Ctrl
flag (on the console it is a control character). Alt+Tab needs the release
of Alt, and the app menu on the Super key needs Super. In velora-ui keys
with Ctrl, Alt or Super come as `Event::Chord` (widgets do not take them, so
Ctrl+C does not type "c" into an installer field); modifiers and the right
mouse button only when the program asks (`Screen::extended_input`).

## D55. Память графических программ / Memory of graphical programs

**2026-11-28**

**RU.** Первый замер Lumi в 1920x1080 с открытым терминалом — 95 МиБ
(система — 105 МиБ при цели меньше 64 МиБ). Причины и что сделано:
- Куча программ не отдаёт память ядру, поэтому временные картинки
  (размытие обоев, переходы, файлы шрифтов) оставались занятыми. Теперь
  блоки от 256 КиБ отображаются отдельно (`mmap`) и освобождаются
  `munmap`; это помогает и установщику.
- fontdue разбирает контуры всех глифов шрифта при загрузке: Inter —
  8,4 МиБ на начертание. `tools/fontsubset` при сборке оставляет глифы
  латиницы с европейскими расширениями, кириллицы, пунктуации, стрелок и
  (для терминала) псевдографики; остальные контуры пустые, номера глифов
  те же (таблицы символов, кернинга и замен остаются верными). Стало
  2,1 МиБ на начертание. Шрифты под OFL 1.1, изменённые версии
  разрешены, зарезервированных имён у них нет.
- Размытая копия обоев хранится в 1/8 разрешения и растягивается при
  выводе (130 КиБ вместо 8 МиБ).

После: Lumi — 47 МиБ с открытым терминалом, система — 56,4 МиБ (QEMU,
WHPX, `qemu_run.py lumi`). Основное — три картинки во весь экран по 8 МиБ
(задний буфер, копия видимого, обои) и шрифты (7,5 МиБ). В 4K буферы
вчетверо больше.

**EN.** The first measurement of Lumi at 1920x1080 with a terminal open was
95 MiB (the system 105 MiB against the goal of less than 64 MiB). Causes and
what was done:
- A program's heap does not give memory back to the kernel, so temporary
  pictures (the wallpaper blur, transitions, font files) stayed taken.
  Blocks from 256 KiB now get their own mapping (`mmap`) and are freed with
  `munmap`; this helps the installer too.
- fontdue turns the outlines of all glyphs into segments on loading: Inter
  took 8.4 MiB per face. `tools/fontsubset` keeps, at build time, the
  glyphs of Latin with its European extensions, Cyrillic, punctuation,
  arrows and (for the terminal) box drawing; the other outlines are emptied
  and glyph numbers stay (the character map, kerning and substitution
  tables stay valid). Now 2.1 MiB per face. The fonts are under the OFL 1.1,
  which allows modified versions; they have no Reserved Font Names.
- The blurred copy of the wallpaper is kept at 1/8 of the resolution and
  scaled up when drawn (130 KiB instead of 8 MiB).

After: Lumi takes 47 MiB with a terminal open, the system 56.4 MiB (QEMU,
WHPX, `qemu_run.py lumi`). Most of it is three full-screen pictures of 8 MiB
(the back buffer, the copy of what is shown, the wallpaper) and the fonts
(7.5 MiB). At 4K the buffers are four times larger.

## D56. Обои кодом / Wallpapers drawn by code

**2026-11-28**

**RU.** Встроенные обои («Волны» как на концепте, «Графит», «Сияние»,
«Угли») рисуются кодом сразу в разрешении экрана: большие круги-листы со
светлым краем на градиенте и мелкое зерно против полос. В 1080p это около
100 мс на хосте и 90–300 мс в QEMU, один раз при входе или смене обоев.
Готовые PNG в высоком разрешении весили бы мегабайты в образе, 4K-картинка
при распаковке занимает 33 МиБ памяти, а на экранах другого размера такие
картинки масштабировались бы с потерей чёткости. PNG из `assets/wallpapers`
попадают в `/usr/share/wallpapers` (и с ним на установленную систему) и
появляются в выборе вместе с `~/Pictures`. Они масштабируются с
заполнением: середина сохраняется, лишние края обрезаются; уменьшение —
усреднением площади, увеличение — билинейно.

**EN.** The built-in wallpapers (Waves as in the concept, Graphite, Aurora,
Ember) are drawn by code right in the screen's resolution: large sheet-like
circles with light rims on a gradient and a fine grain against banding. At
1080p this takes about 100 ms on the host and 90–300 ms in QEMU, once at
login or when the wallpaper changes. Ready PNGs in a high resolution would
take megabytes in the image, a 4K picture takes 33 MiB of memory while it is
decoded, and on screens of another size such pictures would be scaled and
lose sharpness. PNGs from `assets/wallpapers` go to `/usr/share/wallpapers`
(and with it to the installed system) and appear in the choice together with
`~/Pictures`. They are scaled to fill: the middle is kept, the extra edges
are cut; down by area averaging, up bilinearly.
