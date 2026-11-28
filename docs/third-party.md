# Сторонние компоненты / Third-party components

| Компонент / Component | Версия / Version | Лицензия / License | Как используется / How it is used |
|---|---|---|---|
| Limine | 12.11.1 | BSD-2-Clause | Загрузчик UEFI. Скачивается при сборке, проверяется SHA-256, в репозитории не хранится. / UEFI bootloader; downloaded at build time, verified by SHA-256, not stored in the repo. |
| Spleen 8x16 | 2.3.0 | BSD-2-Clause | Растровый шрифт консоли, преобразован в `kernel/src/console/font_data.rs`. / Console bitmap font, converted into `kernel/src/console/font_data.rs`. |
| Inter (Regular, Medium, SemiBold) | 4.1 | SIL OFL 1.1 | Шрифт графических программ (`velora-ui`). Скачивается при сборке (`tools/fetch-fonts.sh`), проверяется SHA-256, в репозитории не хранится; в образе — `/usr/share/fonts`. / Font of the graphical programs (`velora-ui`); downloaded at build time (`tools/fetch-fonts.sh`), verified by SHA-256, not stored in the repo; in the image under `/usr/share/fonts`. |
| JetBrains Mono (Regular) | 2.304 | SIL OFL 1.1 | Моноширинный шрифт терминала Lumi. Скачивается при сборке (`tools/fetch-fonts.sh`), проверяется SHA-256, в репозитории не хранится; в образе — `/usr/share/fonts`. / Monospace font of the Lumi terminal; downloaded at build time (`tools/fetch-fonts.sh`), verified by SHA-256, not stored in the repo; in the image under `/usr/share/fonts`. |
| fontdue | 0.9.5 | MIT OR Apache-2.0 OR Zlib (используется MIT / used under MIT) | Растеризация TrueType со сглаживанием в `velora-ui`. / Anti-aliased TrueType rasterisation in `velora-ui`. |
| ttf-parser 0.25.1, hashbrown 0.15.5, foldhash 0.1.5, allocator-api2 0.2.22, equivalent 1.0.3, core_maths 0.1.1, libm 0.2.16 | — | MIT, MIT OR Apache-2.0 (используется MIT / used under MIT); foldhash — Zlib | Зависимости fontdue без std. Версии закреплены в `Cargo.lock`, исходники берутся с crates.io. / Dependencies of fontdue without std; versions pinned in `Cargo.lock`, sources from crates.io. |

Тексты лицензий: [docs/licenses/](licenses/) (`INTER.txt`, `JETBRAINS-MONO.txt`, библиотеки Rust —
`RUST-CRATES.txt`). В ISO они лежат в `/licenses` (`inter.txt`, `jetbrains-mono.txt`,
`crates.txt`).

License texts: [docs/licenses/](licenses/) (`INTER.txt`, `JETBRAINS-MONO.txt`; the Rust libraries in
`RUST-CRATES.txt`); in the ISO they are under `/licenses` (`inter.txt`,
`jetbrains-mono.txt`, `crates.txt`).

Остальной код проекта написан для Velora OS и распространяется по
GPL-3.0-or-later.

All other code was written for Velora OS and is licensed under GPL-3.0-or-later.
