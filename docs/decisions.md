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
