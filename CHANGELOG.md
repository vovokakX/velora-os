# Changelog

Формат: для каждой версии — что нового, исправления, известные проблемы.
Сначала на русском, затем на английском.

Format: for each version — what is new, fixes, known issues.
Russian first, then English.

## [0.4.0] — 2026-11-26

### Что нового
- Потоки ядра и процессы с собственными адресными пространствами. Половина ядра общая для всех процессов, пользовательская память изолирована.
- Вытесняющий планировщик: round-robin с тремя приоритетами (High, Normal, Idle), квант 10 мс, поток простоя с `hlt`. Сон потоков, очереди ожидания, освобождение стеков и памяти завершившихся потоков и процессов.
- Режим пользователя (ring 3): вход через `iretq`, системные вызовы через `syscall`/`sysret`, `swapgs` и данные процессора через GS. Регистры SSE/FPU сохраняются для каждого потока.
- Системные вызовы: `exit`, `write`, `read` (клавиатура), `yield`, `getpid`, `clock_gettime`, `sleep_ms`. Все указатели проверяются, неверные дают `-EFAULT`. Номера и коды ошибок вынесены в библиотеку `velora-abi`.
- Personality процесса выбирает таблицу системных вызовов. Сейчас есть только нативная; это место для будущего Win32-слоя.
- Права W^X для памяти программ: код нельзя изменить, данные и стек нельзя исполнить.
- Программа, вызвавшая ошибку процессора (обращение к памяти ядра, привилегированная инструкция и т. п.), завершается с понятным сообщением и статусом `128 + номер исключения`. Ядро продолжает работать.
- Стеки потоков используют общие слоты с защитной страницей; освобождённые слоты переиспользуются.
- При загрузке запускается встроенная тестовая программа: «hello from ring 3».
- Тесты: 52 юнит-теста на хосте (2 новых), 60 тестов ядра (17 новых): вытеснение потоков ядра и программ, изоляция памяти, защита памяти ядра, привилегированные инструкции, W^X, раздельные регистры SSE, системные вызовы, отсутствие утечек памяти.

### Исправления
- Виртуальные адреса стеков ядра теперь используются повторно (известная проблема v0.2.0).

### Известные проблемы
- Загрузчика программ из файлов ещё нет: проверочные программы встроены в ядро как плоские бинарники (загрузчик ELF — этап 5).
- Внутри системного вызова ядро не вытесняется (решение D14).
- Используется один процессор.
- `read` возвращает символы сразу, без построчного редактирования (терминальный слой — этап 8).
- Приоритеты строгие: поток High может занять процессор и не пустить потоки Normal (пока никто не использует High).
- Таймер под эмуляцией TCG может идти неточно.

---

### What's new
- Kernel threads and processes with their own address spaces. The kernel half is shared by all processes; user memory is isolated.
- Preemptive scheduler: round robin with three priorities (High, Normal, Idle), a 10 ms time slice, an idle thread using `hlt`. Thread sleep, wait queues, freeing the stacks and memory of finished threads and processes.
- User mode (ring 3): entry via `iretq`, system calls via `syscall`/`sysret`, `swapgs` and per-CPU data through GS. SSE/FPU registers are saved per thread.
- System calls: `exit`, `write`, `read` (keyboard), `yield`, `getpid`, `clock_gettime`, `sleep_ms`. Every pointer is checked; bad ones give `-EFAULT`. Numbers and error codes live in the `velora-abi` library.
- A process personality selects the system call table. Only the native one exists; this is where the Win32 layer will plug in.
- W^X for program memory: code cannot be modified, data and stack cannot be executed.
- A program that causes a CPU exception (touching kernel memory, a privileged instruction, ...) is ended with a clear message and status `128 + vector`; the kernel keeps running.
- Thread stacks use shared slots with guard pages; freed slots are reused.
- Boot runs a built-in test program: "hello from ring 3".
- Tests: 52 host unit tests (2 new), 60 kernel tests (17 new): preemption of kernel threads and programs, memory isolation, kernel memory protection, privileged instructions, W^X, per-thread SSE registers, system calls, no memory leaks.

### Fixes
- Kernel stack virtual addresses are now reused (known issue in v0.2.0).

### Known issues
- No loading programs from files yet: the test programs are built into the kernel as flat binaries (ELF loader in stage 5).
- The kernel is not preempted inside system calls (decision D14).
- Only one CPU is used.
- `read` returns characters immediately, without line editing (terminal layer in stage 8).
- Priorities are strict: a High thread can starve Normal ones (nothing uses High yet).
- The timer can be inaccurate under TCG emulation.

## [0.3.0] — 2026-11-26

### Что нового
- Разбор таблиц ACPI: RSDP, RSDT/XSDT, MADT. Проверяются контрольные суммы и длины. Разбор вынесен в отдельную библиотеку `velora-acpi` с тестами на хосте.
- Локальный APIC в режимах xAPIC и x2APIC, IOAPIC с учётом переназначений ISA-IRQ из MADT. Старые контроллеры 8259 перенастроены и замаскированы.
- Таблица обработчиков прерываний, учёт ложных и необработанных прерываний.
- Калибровка TSC и таймера APIC по PIT. Таймер 100 Гц, время с момента загрузки с точностью до наносекунд, `sleep_ms` с остановкой процессора в простое.
- Драйвер контроллера PS/2 и клавиатуры: скан-коды set 1, расширенные клавиши, Pause, индикаторы Caps/Num/Scroll Lock.
- Раскладки EN (QWERTY) и RU (ЙЦУКЕН), переключение по Alt+Shift. Ctrl+буква даёт управляющий код (Ctrl+C = 0x03) в любой раскладке.
- Очередь событий ввода в ядре. Для проверки клавиатуры добавлена временная строка ввода с эхом; на этапе 8 её заменит оболочка vsh.
- Тесты: 50 юнит-тестов на хосте (22 новых: ACPI, клавиатура и раскладки), 43 теста ядра (9 новых: APIC, IOAPIC, частота таймера, обработчики, ввод с клавиатуры). Тест загрузки печатает текст в обеих раскладках через монитор QEMU и проверяет эхо.

### Исправления
- Нет.

### Известные проблемы
- Нет запасного способа калибровки без PIT. На машинах без PIT загрузка остановится с сообщением об ошибке.
- Используется только один процессор (SMP — после этапа 10).
- USB-клавиатуры (xHCI/HID) не поддерживаются, работает только PS/2 или эмуляция PS/2 в прошивке.
- Индикатор раскладки в строке ввода обновляется только на следующей строке.
- Под эмуляцией без KVM (TCG) таймер может идти неточно (в тестах — до трети).
- Часов реального времени пока нет (RTC — этап 6).

---

### What's new
- ACPI table parsing: RSDP, RSDT/XSDT, MADT, with checksum and length checks. Parsing lives in the separate `velora-acpi` library with host tests.
- Local APIC in xAPIC and x2APIC modes; I/O APICs honouring ISA IRQ overrides from the MADT. The legacy 8259 PICs are remapped and masked.
- Interrupt handler table; spurious and unhandled interrupts are counted.
- TSC and APIC timer calibrated against the PIT. 100 Hz timer, nanosecond time since boot, `sleep_ms` that halts the CPU while idle.
- PS/2 controller and keyboard driver: scancode set 1, extended keys, Pause, Caps/Num/Scroll Lock LEDs.
- EN (QWERTY) and RU (ЙЦУКЕН) layouts, switched with Alt+Shift. Ctrl+letter gives the control code (Ctrl+C = 0x03) in any layout.
- Kernel input event queue. A temporary echoing line editor lets you try the keyboard; the vsh shell replaces it in stage 8.
- Tests: 50 host unit tests (22 new: ACPI, keyboard and layouts), 43 kernel tests (9 new: APIC, I/O APIC, timer rate, handlers, keyboard input). The boot test types text in both layouts through the QEMU monitor and checks the echo.

### Fixes
- None.

### Known issues
- No calibration fallback without a PIT; on such machines boot stops with an error message.
- Only one CPU is used (SMP after stage 10).
- USB keyboards (xHCI/HID) are not supported; only PS/2 or the firmware's PS/2 emulation works.
- The layout indicator in the input line only updates on the next line.
- Without KVM (TCG emulation) the timer can be inaccurate (up to a third off in tests).
- No real-time clock yet (RTC in stage 6).

## [0.2.0] — 2026-11-26

### Что нового
- Аллокатор физических страниц на битовой карте: одиночные страницы, непрерывные блоки с выравниванием, обнаружение двойного освобождения.
- Свои таблицы страниц вместо таблиц загрузчика. HHDM со страницами по 2 МиБ, framebuffer с кешированием write-combining, образ ядра с правами W^X: `.text` только чтение и исполнение, `.rodata` только чтение, данные без исполнения (NX).
- Куча ядра: работают `Box`, `Vec`, `String`, `BTreeMap`. Мелкие объекты выделяются по классам размеров, крупные — целыми страницами; освобождённые страницы используются повторно.
- Стеки ядра с неотображённой защитной страницей. Ядро работает на своём стеке.
- Освобождение памяти загрузчика (около 45 МиБ в QEMU). Данные Limine заранее копируются в ядро, а обращение к ним после освобождения вызывает панику.
- Консоль хранит копию экрана в RAM и больше не читает видеопамять. На реальном железе прокрутка стала быстрее.
- Отчёт о памяти при загрузке. В простое занято 5,2 МиБ (QEMU, 256 МиБ RAM); тест загрузки проверяет, что меньше 64 МиБ.
- Тесты: 28 юнит-тестов на хосте (13 новых для аллокаторов), 34 теста ядра в QEMU (18 новых: страницы, W^X, NX, защитная страница, куча, освобождение памяти загрузчика).
- Скрипт тестов умеет показывать время каждой строки вывода (`--timestamps`).

### Исправления
- Медленная прокрутка консоли из-за чтения видеопамяти (известная проблема v0.1.0).

### Известные проблемы
- Только UEFI; загрузка через BIOS и с USB-накопителя (гибридный образ) не поддерживается.
- Страницы, отданные классам размеров кучи, не возвращаются обратно.
- Виртуальные адреса стеков ядра не используются повторно (освобождение стеков появится вместе с потоками на этапе 4).
- Адреса в стеке вызовов не переводятся в имена функций.
- Прерывания и ввод с клавиатуры ещё не включены. Системой пока нельзя пользоваться: нет процессов и оболочки (этапы 3–8).

---

### What's new
- Bitmap physical frame allocator: single frames, aligned contiguous blocks, double-free detection.
- Own page tables instead of the bootloader's. HHDM with 2 MiB pages, a write-combining framebuffer, a kernel image with W^X permissions: `.text` read+execute, `.rodata` read-only, data non-executable (NX).
- Kernel heap: `Box`, `Vec`, `String`, `BTreeMap` work. Small objects use size classes, large ones whole pages; freed pages are reused.
- Kernel stacks with an unmapped guard page. The kernel runs on its own stack.
- Bootloader memory is released (about 45 MiB in QEMU). Limine data is copied into the kernel first, and using it afterwards panics.
- The console keeps a copy of the screen in RAM and no longer reads video memory, so scrolling on real hardware is faster.
- Memory report at boot. 5.2 MiB in use when idle (QEMU, 256 MiB RAM); the boot test checks it stays below 64 MiB.
- Tests: 28 host unit tests (13 new for the allocators), 34 kernel tests in QEMU (18 new: frames, W^X, NX, guard page, heap, releasing bootloader memory).
- The test runner can timestamp every output line (`--timestamps`).

### Fixes
- Slow console scrolling caused by reading video memory (known issue in v0.1.0).

### Known issues
- UEFI only; BIOS boot and booting from a USB drive (hybrid image) are not supported.
- Pages given to heap size classes are never returned.
- Kernel stack virtual addresses are not reused (freeing stacks comes with threads in stage 4).
- Backtrace addresses are not resolved to function names.
- Interrupts and keyboard input are not enabled yet. The system is not usable yet: no processes or shell (stages 3–8).

## [0.1.0] — 2026-11-26

### Что нового
- Загрузка через UEFI с помощью Limine 12.11.1 (протокол Limine, base revision 6).
- Вывод в последовательный порт COM1 (16550, 115200 бод) и текстовая консоль
  на framebuffer со шрифтом Spleen 8x16 (латиница, кириллица, псевдографика).
- В консоли поддерживаются ANSI-цвета, перемещение курсора и очистка экрана.
- Своя GDT с TSS и отдельными стеками (IST) для double fault, NMI и machine check.
- IDT на все 256 векторов. Исключения процессора выводят отчёт: тип,
  код ошибки, причину page fault, все регистры, CR0–CR4, стек вызовов.
- Обработчик паники с сообщением, местом в коде, регистрами и стеком вызовов.
- Сборка одной командой (`./build.sh`) в Linux и Windows. Свой сборщик ISO
  (`mkiso`: ISO 9660 + El Torito UEFI) и свой генератор FAT16 с длинными
  именами (`velora-fat`).
- Тесты: 15 юнит-тестов на хосте, 16 тестов ядра в QEMU и проверка пути
  паники, тест загрузки с замером времени. CI на GitHub Actions.

### Исправления
- Нет (первая версия).

### Известные проблемы
- Загрузка только через UEFI; BIOS не поддерживается.
- ISO загружается как CD. Запись ISO на USB-накопитель (гибридный образ) не поддерживается.
- Консоль работает только с framebuffer 32 бит на пиксель.
- Прокрутка читает framebuffer напрямую, на реальном железе это медленно.
  Будет исправлено с теневым буфером на этапе 2.
- Адреса в стеке вызовов не переводятся в имена функций. Для этого нужен
  `addr2line` и файл ядра.
- Прерывания (таймер, клавиатура) ещё не включены, ввода нет.

---

### What's new
- UEFI boot through Limine 12.11.1 (Limine protocol, base revision 6).
- Output to the COM1 serial port (16550, 115200 baud) and a framebuffer text
  console with the Spleen 8x16 font (Latin, Cyrillic, box drawing).
- The console supports ANSI colours, cursor movement and screen clearing.
- Own GDT with a TSS and separate IST stacks for double fault, NMI and machine check.
- IDT for all 256 vectors. CPU exceptions print a report: type, error code,
  page fault cause, all registers, CR0–CR4 and a backtrace.
- Panic handler with the message, source location, registers and a backtrace.
- One-command build (`./build.sh`) on Linux and Windows. Own ISO builder
  (`mkiso`: ISO 9660 + El Torito UEFI) and own FAT16 image builder with long
  file names (`velora-fat`).
- Tests: 15 host unit tests, 16 kernel tests in QEMU plus a check of the panic
  path, and a boot test with timing. CI on GitHub Actions.

### Fixes
- None (first release).

### Known issues
- UEFI boot only; BIOS is not supported.
- The ISO boots as a CD. Writing the ISO to a USB drive (hybrid image) is not supported.
- The console only works with 32 bits-per-pixel framebuffers.
- Scrolling reads the framebuffer directly, which is slow on real hardware.
  To be fixed with a shadow buffer in stage 2.
- Backtrace addresses are not resolved to function names. Use `addr2line`
  with the kernel file.
- Interrupts (timer, keyboard) are not enabled yet, so there is no input.
