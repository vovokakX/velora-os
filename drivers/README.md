# drivers

Аппаратно-независимые части драйверов. Они тестируются на хосте.

- `input/` (`velora-input`): коды клавиш, декодер скан-кодов PS/2 (set 1),
  модификаторы, раскладки EN (QWERTY) и RU (ЙЦУКЕН). С этапа 3.

Код, которому нужен прямой доступ к железу (порты, прерывания), находится в
`kernel/src/drivers/` (сейчас — контроллер PS/2). На этапе 6 сюда добавятся
PCI, virtio-blk, AHCI, NVMe и RTC, позже virtio-net (этап 10) и xHCI.

---

Hardware-independent driver parts, tested on the host.

- `input/` (`velora-input`): key codes, PS/2 scancode set 1 decoder,
  modifiers, EN (QWERTY) and RU (ЙЦУКЕН) layouts. Since stage 3.

Code that needs direct hardware access (ports, interrupts) lives in
`kernel/src/drivers/` (currently the PS/2 controller). Stage 6 adds PCI,
virtio-blk, AHCI, NVMe and RTC; virtio-net (stage 10) and xHCI come later.
