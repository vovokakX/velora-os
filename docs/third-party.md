# Сторонние компоненты / Third-party components

| Компонент / Component | Версия / Version | Лицензия / License | Как используется / How it is used |
|---|---|---|---|
| Limine | 12.11.1 | BSD-2-Clause | Загрузчик UEFI. Скачивается при сборке, проверяется SHA-256, в репозитории не хранится. / UEFI bootloader; downloaded at build time, verified by SHA-256, not stored in the repo. |
| Spleen 8x16 | 2.3.0 | BSD-2-Clause | Растровый шрифт консоли, преобразован в `kernel/src/console/font_data.rs`. / Console bitmap font, converted into `kernel/src/console/font_data.rs`. |

Тексты лицензий: [docs/licenses/](licenses/). В ISO они лежат в `/licenses`.

License texts: [docs/licenses/](licenses/); in the ISO they are under `/licenses`.

Остальной код проекта написан для Velora OS и распространяется по
GPL-3.0-or-later.

All other code was written for Velora OS and is licensed under GPL-3.0-or-later.
