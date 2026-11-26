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

С этапа 2 ядро строит свои таблицы страниц (`kernel/src/mm/paging.rs`)
и переключается на них сразу после запуска аллокатора страниц:

- HHDM: все области карты памяти, кроме reserved и bad, с правами RW и NX.
  Где позволяет выравнивание, используются страницы по 2 МиБ.
  Framebuffer отображается с кешированием write-combining (PAT 5).
- Образ ядра: страницы по 4 КиБ с правами по секциям (W^X).
- Куча ядра растёт по требованию с `0xffff_c000_0000_0000`.
- Стеки ядра выделяются с `0xffff_e000_0000_0000`. Под каждым стеком
  остаётся неотображённая защитная страница.
- Все 256 записей верхней половины PML4 создаются заранее (256 таблиц PDPT,
  1 МиБ). Тогда на этапе 4 каждое адресное пространство процесса сможет
  получить ядро простым копированием этих записей, и они больше не изменятся.

### Пространство пользователя (этап 4)

| Адрес | Что |
|---|---|
| `0x0000_0000_0040_0000` | код программы (R-X), сейчас — плоские тестовые программы |
| `0x0000_0000_0060_0000` | область данных 64 КиБ (RW-, NX) |
| `0x0000_7fff_ffef_0000` – `0x0000_7fff_fff0_0000` | стек 64 КиБ (RW-, NX) |
| `0x0000_7fff_ffff_f000` и выше | никогда не отображается (защита `sysret`) |

С этапа 5 раскладку будет задавать загрузчик ELF.

### Образ ядра

Задаётся в `kernel/linker.ld`. Секции выровнены по 4 КиБ, чтобы на этапе 2
задать права страниц: `.text` — R-X, `.rodata` — R--, `.data`/`.bss` — RW-
(NX). Метки `__text_start`, `__rodata_start`, `__data_start`, `__kernel_end`
доступны коду ядра.

### Физическая память

Ядро получает карту памяти от Limine (`MEMMAP`) и копирует её вместе с
остальными данными загрузчика в `boot::BootInfo`.

Физические страницы учитывает битовая карта, по биту на страницу 4 КиБ
(`velora-mm::bitmap`). Она лежит в начале первой подходящей области usable.
Страница 0 никогда не выделяется.

Порядок освобождения памяти загрузчика:
1. данные Limine скопированы в `BootInfo`;
2. ядро переключилось на свои таблицы страниц;
3. ядро перешло на свой стек (64 КиБ с защитной страницей);
4. области bootloader-reclaimable отдаются аллокатору. После этого обращение
   к ответам Limine вызывает панику.

### Стеки

| Стек | Размер | Где |
|---|---|---|
| загрузочный стек | 64 КиБ | от Limine; освобождается после перехода на основной стек |
| основной стек ядра (поток kmain) | 64 КиБ + защитная страница | `0xffff_e000_0000_1000` |
| стеки потоков | 32 КиБ + защитная страница | слоты по 68 КиБ с `0xffff_e000_...`, освобождаются и используются повторно |
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

Since stage 2 the kernel builds its own page tables (`kernel/src/mm/paging.rs`)
right after the frame allocator starts: the HHDM covers every memory map
region except reserved and bad ones (RW, NX, 2 MiB pages where aligned,
framebuffer write-combining via PAT 5); the kernel image uses 4 KiB pages
with per-section W^X permissions; the heap grows on demand from
`0xffff_c000_0000_0000`; kernel stacks start at `0xffff_e000_0000_0000`,
each with an unmapped guard page below it. All 256 upper-half PML4 entries
are created up front (256 PDPTs, 1 MiB), so in stage 4 every process address
space can share the kernel half by copying these entries, which never change
afterwards.

### User space (stage 4)

Program code at `0x400000` (R-X; flat test programs for now), a 64 KiB data
area at `0x600000` (RW-, NX), a 64 KiB stack ending at `0x7fff_fff0_0000`
(RW-, NX); `0x7fff_ffff_f000` and above are never mapped (protects `sysret`).
From stage 5 the ELF loader decides the layout.

### Kernel image

Defined in `kernel/linker.ld`. Sections are 4 KiB aligned so that stage 2 can
set page permissions: `.text` R-X, `.rodata` R--, `.data`/`.bss` RW- (NX).
The symbols `__text_start`, `__rodata_start`, `__data_start` and
`__kernel_end` are available to kernel code.

### Physical memory

The kernel copies the Limine memory map and other boot data into
`boot::BootInfo`. Physical frames are tracked by a bitmap, one bit per 4 KiB
frame (`velora-mm::bitmap`), stored at the start of the first suitable usable
region; frame 0 is never handed out. Bootloader-reclaimable memory is released
after (1) the boot data is copied, (2) the kernel runs on its own page tables
and (3) on its own stack. After that, using a Limine response panics.

### Stacks

Boot stack: 64 KiB from Limine, released after the switch to the main kernel
stack (64 KiB plus a guard page at `0xffff_e000_0000_1000`). IST1 (double fault), IST2 (NMI) and IST3
(machine check): 16 KiB each in the kernel `.bss`.
