# drivers

Аппаратно-независимые части драйверов. Они тестируются на хосте.

- `input/` (`velora-input`): коды клавиш, декодер скан-кодов PS/2 (set 1),
  модификаторы, раскладки EN (QWERTY) и RU (ЙЦУКЕН). С этапа 3.
- `pci/` (`velora-pci`): перечисление шины PCI, BAR, capabilities. С этапа 6.
- `rtc/` (`velora-rtc`): разбор регистров CMOS RTC, календарь, время UNIX. С этапа 6.
- `hda/` (`velora-hda`): Intel HDA — команды кодека, граф виджетов, путь от
  выхода к ЦАП, громкость, кольцевой буфер вывода, текст `/dev/mixer` (D62).

Код, которому нужен прямой доступ к железу (порты, прерывания), находится в
`kernel/src/drivers/`: PS/2, доступ к PCI, virtio-blk, AHCI, NVMe, RTC, буферы
DMA и интерфейс `BlockDevice`. Позже — virtio-net (этап 10) и xHCI.

---

Hardware-independent driver parts, tested on the host.

- `input/` (`velora-input`): key codes, PS/2 scancode set 1 decoder,
  modifiers, EN (QWERTY) and RU (ЙЦУКЕН) layouts. Since stage 3.
- `pci/` (`velora-pci`): PCI enumeration, BARs, capabilities. Since stage 6.
- `rtc/` (`velora-rtc`): CMOS RTC decoding, calendar, UNIX time. Since stage 6.
- `hda/` (`velora-hda`): Intel HDA — codec verbs, the widget graph, the path
  from an output to a DAC, volume, the playback ring, the `/dev/mixer` text (D62).

Code that needs direct hardware access (ports, interrupts) lives in
`kernel/src/drivers/`: PS/2, PCI access, virtio-blk, AHCI, NVMe, RTC, DMA
buffers and the `BlockDevice` interface. virtio-net (stage 10) and xHCI come
later.
