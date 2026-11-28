//! Draws every installer screen and the start menu on the host, as PPM
//! files: `cargo run --release -p velora-installer-gui --example screens
//! -- OUT_DIR [WIDTHxHEIGHT] [dark|light|gamers] [ACCENT]`, where ACCENT
//! is an index into `settings::ACCENTS` (0 amber, the default).

use velora_install::{Phase, Progress};
use velora_installer_gui::chooser::Chooser;
use velora_installer_gui::{App, Assets, DiskEntry, Screen};
use velora_ui::image::{Picture, Pointer};
use velora_ui::text::Fonts;
use velora_ui::{Canvas, Ui};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = args
        .first()
        .cloned()
        .unwrap_or_else(|| String::from("build/installer-preview"));
    let (w, h) = args
        .get(1)
        .and_then(|s| s.split_once('x'))
        .map(|(a, b)| (a.parse().unwrap(), b.parse().unwrap()))
        .unwrap_or((1280, 720));
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let read = |p: &str| std::fs::read(format!("{}/{}", root, p)).expect(p);
    let mut f = Fonts::new(
        &read("build/fonts/Inter-Regular.ttf"),
        &read("build/fonts/Inter-Medium.ttf"),
        &read("build/fonts/Inter-SemiBold.ttf"),
    )
    .unwrap();
    let p = |n: &str| Pointer::from_png(&read(&format!("assets/cursors/{}.png", n))).unwrap();
    let mut a = Assets {
        logo: Picture::from_png_on_background(&read("assets/velora-logo.png")).unwrap(),
        pointers: [p("classic"), p("blade"), p("drop")],
    };
    std::fs::create_dir_all(&out).unwrap();
    let save = |name: &str, c: &Canvas| {
        let path = format!("{}/{}.ppm", out, name);
        std::fs::write(&path, c.to_ppm()).unwrap();
        println!("{}", path);
    };
    let mut c = Canvas::new(w, h);
    let mut ui = Ui::new(256);
    let mut ch = Chooser::default();
    ui.begin(&[]);
    ch.draw(&mut ui, &mut c, &mut f, &mut a);
    save("00-start", &c);
    let disks = vec![DiskEntry {
        name: String::from("nvme0n1"),
        size: 512_110_190_592,
        model: String::from("Samsung SSD 970 EVO"),
        bus: String::from("NVMe"),
        in_use: false,
    }];
    let mut app = App::new(disks);
    app.animate = false;
    app.settings.theme = match args.get(2).map(|s| s.as_str()) {
        Some("light") => 1,
        Some("gamers") => 2,
        _ => 0,
    };
    app.settings.accent = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
    app.user = String::from("anna");
    app.password = String::from("secret");
    app.root_password = String::from("secret");
    let screens = [
        Screen::Welcome,
        Screen::Location,
        Screen::Keyboard,
        Screen::Appearance,
        Screen::Disk,
        Screen::Partitions,
        Screen::User,
        Screen::Confirm,
        Screen::Installing,
        Screen::Done,
    ];
    for (i, s) in screens.iter().enumerate() {
        app.screen = *s;
        if *s == Screen::Partitions {
            app.mode = 2;
        }
        if *s == Screen::Installing {
            app.set_progress(Progress {
                phase: Phase::Configure,
                permille: 720,
            });
        }
        ui.begin(&[]);
        app.draw(&mut ui, &mut c, &mut f, &mut a);
        save(&format!("{:02}-{}", i + 1, s.name()), &c);
    }
    // The disk screen with a disk whose partitions are mounted, and with
    // no disk at all (the system started from a USB drive).
    app.mode = 0;
    app.screen = Screen::Disk;
    let mut mounted = app.disks.clone();
    mounted[0].in_use = true;
    app.set_disks(mounted);
    ui.begin(&[]);
    app.draw(&mut ui, &mut c, &mut f, &mut a);
    save("05b-disk-mounted", &c);
    app.set_disks(Vec::new());
    app.boot_disk = Some(String::from("usb0"));
    ui.begin(&[]);
    app.draw(&mut ui, &mut c, &mut f, &mut a);
    save("05c-disk-none", &c);
}
