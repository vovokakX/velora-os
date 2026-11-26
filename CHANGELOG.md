# Changelog

Формат: для каждой версии — что нового, исправления, известные проблемы.
Сначала на русском, затем на английском.

Format: for each version — what is new, fixes, known issues.
Russian first, then English.

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
