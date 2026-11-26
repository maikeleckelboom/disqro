"""Build a scratch qualification player that holds exactly two seconds of observed PCM.

Use the same script on both revisions. Live performance runs use the unmodified binary.
"""
import argparse
from pathlib import Path
import shutil
import subprocess


def replace_once(path, old, new):
    text = path.read_text(encoding="utf-8")
    if text.count(old) != 1:
        raise ValueError(f"Unexpected source layout in {path}")
    path.write_text(text.replace(old, new), encoding="utf-8")


def build(repo, scratch, target):
    scratch.mkdir(parents=True, exist_ok=False)
    for name in ("Cargo.toml", "Cargo.lock", "build.rs", "rust-toolchain.toml"):
        shutil.copy2(repo / name, scratch)
    for name in ("src", "assets"):
        shutil.copytree(repo / name, scratch / name)
    with (scratch / "Cargo.toml").open("a", encoding="utf-8") as manifest:
        manifest.write('\n[[bin]]\nname = "field-reference"\npath = "src/main.rs"\n')
    streams = scratch / "src/spectrum_worker/visual_streams.rs"
    replace_once(streams, "struct Group {", "struct Group {\n    reference: Vec<kira::Frame>,\n    held: bool,")
    replace_once(streams, "field.push(h.tail(BLOCK));", """
                    assert!(crate::audio::silent_qualification());
                    if !g.held {
                        let input = h.tail(BLOCK);
                        let first = if g.reference.is_empty() {
                            input.iter().position(|f| *f != kira::Frame::ZERO).unwrap_or(input.len())
                        } else { 0 };
                        let count = (96_000 - g.reference.len()).min(input.len() - first);
                        g.reference.extend_from_slice(&input[first..first+count]);
                        if g.reference.len() == 96_000 {
                            for block in g.reference.chunks(BLOCK) { field.push(block); }
                            let pcm: Vec<_> = g.reference.iter()
                                .flat_map(|f| [f.left, f.right]).flat_map(f32::to_le_bytes).collect();
                            let dir = std::path::PathBuf::from(std::env::var_os("DISQRO_PROFILE_DIR").unwrap());
                            std::fs::write(dir.join("observed.f32"), pcm).unwrap();
                            g.held = true;
                        }
                    }""")
    plot = scratch / "src/panels/instruments/plot.rs"
    replace_once(plot, "cache.field_image(a, f, r, w);", """cache.field_image(a, f, r, w);
        static RECORDED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        if RECORDED.get().is_none() {
            let dir = std::path::PathBuf::from(std::env::var_os("DISQRO_PROFILE_DIR").unwrap());
            if dir.join("observed.f32").exists() {
                let scale = w.scale_factor();
                std::fs::write(dir.join("geometry.csv"), format!("{},{},{},{}", scale,
                    f32::from(a.left())*scale, f32::from(a.top())*scale, f32::from(a.size.width)*scale)).unwrap();
                RECORDED.set(()).unwrap();
            }
        }""")
    # Deliver a DPI notification inside the private process. Its suggested RECT
    # is valid in the recipient address space. The owner's monitor stays untouched.
    main = scratch / "src/main.rs"
    replace_once(main, 'window.set_window_title("Disqro");', '''window.set_window_title("Disqro");
                assert!(crate::audio::silent_qualification());
                use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                if let RawWindowHandle::Win32(raw) = window.window_handle().unwrap().as_raw() {
                    let handle = raw.hwnd.get();
                    std::thread::spawn(move || {
                        use windows::Win32::{Foundation::*, UI::WindowsAndMessaging::*};
                        let dpi: usize = std::env::var("DISQRO_REFERENCE_DPI").unwrap().parse().unwrap();
                        if dpi != 0 {
                            std::thread::sleep(std::time::Duration::from_millis(700));
                            let hwnd = HWND(handle as _);
                            let mut rect = RECT::default();
                            unsafe {
                                GetWindowRect(hwnd, &mut rect).unwrap();
                                SendMessageW(hwnd, WM_DPICHANGED, Some(WPARAM(dpi | dpi << 16)),
                                    Some(LPARAM((&rect as *const RECT) as isize)));
                            }
                        }
                    });
                }''')
    subprocess.run(["cargo", "build", "--bin", "field-reference", "--release", "--locked", "--features", "qualification",
                    "--target-dir", str(target)], cwd=scratch, check=True)
    shutil.copy2(target / "release/field-reference.exe", scratch / "reference.exe")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("repo", "scratch", "target"):
        parser.add_argument(name, type=lambda value: Path(value).resolve())
    args = parser.parse_args()
    build(args.repo, args.scratch, args.target)
