//! velora-fsck: checks a VeloraFS or FAT volume inside a disk image on
//! the host.
//!
//! ```text
//! velora-fsck IMAGE [--offset BYTES] [--size BYTES] [--list]
//! ```
//!
//! VeloraFS: mounts the volume (replaying the journal in memory if a
//! committed transaction is pending) and runs the full consistency check.
//! FAT: walks every directory and reads every file. `--list` prints the
//! file tree. The image file is never modified.

use std::process::ExitCode;
use velora_vfs::{FileSystem, Ino, Kind, MemDisk};

fn parse_size(s: &str) -> Result<u64, String> {
    let (num, mul) = match s.as_bytes().last() {
        Some(b'K') => (&s[..s.len() - 1], 1024),
        Some(b'M') => (&s[..s.len() - 1], 1024 * 1024),
        Some(b'G') => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        _ => (s, 1),
    };
    num.parse::<u64>()
        .map(|n| n * mul)
        .map_err(|_| format!("bad size '{s}'"))
}

/// Visits every file below `dir`; returns (files, directories, bytes).
fn walk(
    fs: &mut dyn FileSystem,
    dir: Ino,
    path: &str,
    list: bool,
    errors: &mut Vec<String>,
) -> (u64, u64, u64) {
    let (mut files, mut dirs, mut bytes) = (0, 1, 0);
    let mut pos = 0;
    loop {
        let (e, next) = match fs.readdir(dir, pos) {
            Ok(Some(x)) => x,
            Ok(None) => break,
            Err(err) => {
                errors.push(format!("{path}: readdir: {err}"));
                break;
            }
        };
        pos = next;
        let p = format!("{}/{}", path.trim_end_matches('/'), e.name);
        if list {
            println!("{}{}", p, if e.kind == Kind::Dir { "/" } else { "" });
        }
        if e.kind == Kind::Dir {
            let (f, d, b) = walk(fs, e.ino, &p, list, errors);
            files += f;
            dirs += d;
            bytes += b;
            continue;
        }
        files += 1;
        let size = match fs.stat(e.ino) {
            Ok(st) => st.size,
            Err(err) => {
                errors.push(format!("{p}: stat: {err}"));
                continue;
            }
        };
        let mut buf = vec![0u8; 1 << 20];
        let mut off = 0;
        while off < size {
            match fs.read(e.ino, off, &mut buf) {
                Ok(0) => {
                    errors.push(format!("{p}: ends at {off} of {size} bytes"));
                    break;
                }
                Ok(n) => off += n as u64,
                Err(err) => {
                    errors.push(format!("{p}: read: {err}"));
                    break;
                }
            }
        }
        bytes += off;
    }
    (files, dirs, bytes)
}

fn run() -> Result<bool, String> {
    let mut image = None;
    let mut offset = 0u64;
    let mut size = None;
    let mut list = false;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--offset" => offset = parse_size(&it.next().ok_or("--offset needs a value")?)?,
            "--size" => size = Some(parse_size(&it.next().ok_or("--size needs a value")?)?),
            "--list" => list = true,
            "-h" | "--help" => {
                println!("usage: velora-fsck IMAGE [--offset BYTES] [--size BYTES] [--list]");
                return Ok(true);
            }
            s if image.is_none() => image = Some(s.to_string()),
            other => return Err(format!("unexpected argument '{other}'")),
        }
    }
    let image = image.ok_or("no image given")?;
    let data = std::fs::read(&image).map_err(|e| format!("cannot read {image}: {e}"))?;
    let start = offset as usize;
    let end = size.map_or(data.len(), |s| start + s as usize);
    if !start.is_multiple_of(512)
        || end > data.len()
        || end <= start
        || !(end - start).is_multiple_of(512)
    {
        return Err("offset and size must be inside the image and multiples of 512".into());
    }
    let mut disk = MemDisk::from_image(512, data[start..end].to_vec());

    let mut errors = Vec::new();
    let mut fs: Box<dyn FileSystem> = if velorafs::probe(&mut disk).is_some() {
        let fs = velorafs::VeloraFs::mount(disk, false).map_err(|e| format!("mount: {e}"))?;
        let sb = fs.superblock();
        println!(
            "velora-fsck: VeloraFS '{}', {} blocks ({} free), {} inodes ({} free){}",
            sb.label_str(),
            sb.total_blocks,
            sb.free_blocks,
            sb.inode_count,
            sb.free_inodes,
            if fs.recovered() {
                ", journal replayed"
            } else {
                ""
            }
        );
        Box::new(fs)
    } else if let Some(t) = velora_fat::volume::probe(&mut disk) {
        println!("velora-fsck: {t:?} volume");
        Box::new(velora_fat::FatVolume::mount(disk, true).map_err(|e| format!("mount: {e}"))?)
    } else {
        return Err("no VeloraFS or FAT16/FAT32 volume found".into());
    };
    if let Ok(found) = fs.fsck() {
        errors.extend(found);
    }
    let root = fs.root();
    let (files, dirs, bytes) = walk(&mut *fs, root, "/", list, &mut errors);
    println!("velora-fsck: {files} files, {dirs} directories, {bytes} bytes of data");
    for e in &errors {
        println!("velora-fsck: ERROR {e}");
    }
    Ok(errors.is_empty())
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => {
            println!("velora-fsck: clean");
            ExitCode::SUCCESS
        }
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("velora-fsck: {e}");
            ExitCode::from(2)
        }
    }
}
