# Обои / Wallpapers

Встроенные обои Lumi — «Волны», «Графит», «Сияние», «Угли» — рисуются
программно (`lumi/src/wallpaper.rs`) сразу в родном разрешении экрана:
они чёткие на любом мониторе и не занимают места в образе. Посмотреть их
без QEMU: `cargo run --release -p velora-lumi --example wallpapers --
build/lumi-wallpapers 3840x2160`.

Картинки PNG, положенные в эту папку, попадают в образ
(`/usr/share/wallpapers`) и появляются в Настройках → Обои. Они
масштабируются под экран без искажений: середина картинки сохраняется,
края с лишней стороны обрезаются. Свои картинки пользователь может
положить и в `~/Pictures`.

The built-in Lumi wallpapers (Waves, Graphite, Aurora, Ember) are drawn
by code (`lumi/src/wallpaper.rs`) in the screen's native resolution: they
are sharp on any monitor and take no space in the image. To look at them
without QEMU: `cargo run --release -p velora-lumi --example wallpapers --
build/lumi-wallpapers 3840x2160`.

PNG pictures put in this folder go into the image
(`/usr/share/wallpapers`) and appear in Settings → Wallpaper. They are
scaled to the screen without distortion: the middle of the picture is
kept and the extra edges are cut. Users can also put pictures in
`~/Pictures`.
