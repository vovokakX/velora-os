# Карта памяти / Memory map

[Русский](#русский) | [English](#english)

## Русский

### Виртуальное адресное пространство (x86_64, 4-уровневые страницы)

| Диапазон | Размер | Назначение | С какого этапа |
|---|---|---|---|
| `0x0000_0000_0000_0000` – `0x0000_0000_003f_ffff` | 4 МиБ | не отображается (ловит обращения по нулевому указателю) | 1 |
| `0x0000_0000_0040_0000` – `0x0000_7fff_ffff_ffff` | ~128 ТиБ | пространство пользователя | 4 |
| `0xffff_8000_0000_0000` – … | по объёму RAM | HHDM: вся физическая память (смещение даёт Limine) | 1 |
| `0xffff_c000_0000_0000` – `0xffff_c0ff_ffff_ffff` | 1 ТиБ | куча ядра | 2 |
| `0xffff_d000_0000_0000` – `0xffff_d0ff_ffff_ffff` | 1 ТиБ | MMIO устройств (APIC, PCI BAR, framebuffer) | 2–6 |
| `0xffff_e000_0000_0000` – `0xffff_e0ff_ffff_ffff` | 1 ТиБ | стеки ядра для потоков, с защитными страницами | 4 |
| `0xffff_ffff_8000_0000` – конец | 2 ГиБ | образ ядра (`.text`, `.rodata`, `.data`, `.bss`) | 1 |

Сейчас (этап 1) действуют только HHDM и образ ядра. Таблицы страниц
создал Limine. Свои таблицы страниц появятся на этапе 2.

### Образ ядра

Задаётся в `kernel/linker.ld`. Секции выровнены по 4 КиБ, чтобы на этапе 2
задать права страниц: `.text` — R-X, `.rodata` — R--, `.data`/`.bss` — RW-
(NX). Метки `__text_start`, `__rodata_start`, `__data_start`, `__kernel_end`
доступны коду ядра.

### Физическая память

Ядро получает карту памяти от Limine (`MEMMAP`). Типы областей:
usable, reserved, ACPI reclaimable, ACPI NVS, bad, bootloader reclaimable,
executable and modules, framebuffer, reserved mapped. Пока физический
аллокатор (этап 2) не готов, ядро только выводит сводку.

На этапе 2 память типа «bootloader reclaimable» будет освобождена после
того, как ядро перестанет пользоваться структурами Limine и стеком загрузки.

### Стеки

| Стек | Размер | Где |
|---|---|---|
| загрузочный стек | 64 КиБ | от Limine, bootloader-reclaimable |
| IST1: double fault | 16 КиБ | `.bss` ядра |
| IST2: NMI | 16 КиБ | `.bss` ядра |
| IST3: machine check | 16 КиБ | `.bss` ядра |

## English

### Virtual address space (x86_64, 4-level paging)

See the table above. Summary: the lowest 4 MiB stay unmapped to catch null
pointer accesses; user space is `0x400000`–`0x7fff_ffff_ffff` (stage 4); the
HHDM maps all physical memory at the offset Limine reports (stage 1); the
kernel heap (`0xffff_c000…`, stage 2), device MMIO (`0xffff_d000…`) and kernel
thread stacks with guard pages (`0xffff_e000…`, stage 4) get 1 TiB windows each;
the kernel image is in the top 2 GiB (`0xffff_ffff_8000_0000`).

In stage 1 only the HHDM and the kernel image exist; the page tables are the
ones Limine created. The kernel builds its own in stage 2.

### Kernel image

Defined in `kernel/linker.ld`. Sections are 4 KiB aligned so that stage 2 can
set page permissions: `.text` R-X, `.rodata` R--, `.data`/`.bss` RW- (NX).
The symbols `__text_start`, `__rodata_start`, `__data_start` and
`__kernel_end` are available to kernel code.

### Physical memory

The kernel gets the memory map from Limine (`MEMMAP`). Until the physical
allocator exists (stage 2) the kernel only prints a summary. In stage 2,
bootloader-reclaimable memory is freed once the kernel no longer uses Limine
structures or the boot stack.

### Stacks

Boot stack: 64 KiB from Limine. IST1 (double fault), IST2 (NMI) and IST3
(machine check): 16 KiB each in the kernel `.bss`.
