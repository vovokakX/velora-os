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
