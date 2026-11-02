//! Sets the SwiftUI app's evidence renders beside koan-gui's walk of the same
//! library, page by page, and marks where their edges disagree.
//!
//! For each `window-<page>-<scheme>.png` found in both directories it writes
//! one sheet to the output directory: the Mac app's window, koan-gui's, and
//! an overlay of the two at the same size, with every edge one app draws
//! that the other has no edge within a pixel of: the Mac's alone in red,
//! koan-gui's alone in cyan, edges they share in grey. It prints, per page,
//! how much of each app's edge is unmatched, which is the number to bring
//! down. Neither app is shown and nothing is read from the screen: both
//! inputs are offscreen renders (`KOAN_RENDER_EVIDENCE` and the `walk`
//! example).
//!
//! cargo run --release -p koan-gui --example compare -- MAC_DIR GUI_DIR OUT_DIR

use std::error::Error;
use std::path::{Path, PathBuf};

use image::imageops::{FilterType, resize};
use image::{GrayImage, Luma, Rgba, RgbaImage};

/// How far apart two edges may be and still be the same edge, in pixels of
/// the compared size.
const SLACK: i64 = 1;
/// A luminance step this large, between neighbours, is an edge.
const EDGE: i16 = 24;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let [mac, gui, out] = args.as_slice() else {
        return Err("usage: compare MAC_DIR GUI_DIR OUT_DIR".into());
    };
    std::fs::create_dir_all(out)?;
    let mut pages: Vec<String> = std::fs::read_dir(gui)?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with("window-") && n.ends_with(".png"))
        .filter(|n| mac.join(n).exists())
        .collect();
    pages.sort();
    if pages.is_empty() {
        return Err(format!(
            "no page rendered by both: compare {} with {}",
            mac.display(),
            gui.display()
        )
        .into());
    }
    println!("page                         mac edge unmatched   gui edge unmatched");
    for name in pages {
        let (mac_only, gui_only) = compare(&mac.join(&name), &gui.join(&name), &out.join(&name))?;
        println!(
            "{:<28} {:>17.1}% {:>19.1}%",
            name.trim_end_matches(".png"),
            mac_only * 100.0,
            gui_only * 100.0
        );
    }
    println!("sheets in {}", out.display());
    Ok(())
}

/// Writes the sheet for one page; returns the share of each app's edge
/// pixels the other has nothing near.
fn compare(mac: &Path, gui: &Path, out: &Path) -> Result<(f64, f64), Box<dyn Error>> {
    let mac = image::open(mac)?.to_rgba8();
    let gui = image::open(gui)?.to_rgba8();
    // Compared at the smaller of the two sizes, so a 2× render and a 1× one
    // line up point for point.
    let (w, h) = (mac.width().min(gui.width()), mac.height().min(gui.height()));
    let fit = |img: &RgbaImage| {
        if img.dimensions() == (w, h) {
            img.clone()
        } else {
            resize(img, w, h, FilterType::Triangle)
        }
    };
    let (mac, gui) = (fit(&mac), fit(&gui));
    let (mac_edges, gui_edges) = (edges(&mac), edges(&gui));

    let mut overlay = RgbaImage::from_pixel(w, h, Rgba([12, 12, 14, 255]));
    let (mut mac_total, mut mac_alone, mut gui_total, mut gui_alone) = (0u64, 0u64, 0u64, 0u64);
    for y in 0..h {
        for x in 0..w {
            let (m, g) = (on(&mac_edges, x, y), on(&gui_edges, x, y));
            if m {
                mac_total += 1;
            }
            if g {
                gui_total += 1;
            }
            let colour = match (m, g) {
                (true, _) if !near(&gui_edges, x, y) => {
                    mac_alone += 1;
                    Some([235, 70, 70, 255])
                }
                (_, true) if !near(&mac_edges, x, y) => {
                    gui_alone += 1;
                    Some([70, 210, 235, 255])
                }
                (true, _) | (_, true) => Some([110, 110, 116, 255]),
                _ => None,
            };
            if let Some(c) = colour {
                overlay.put_pixel(x, y, Rgba(c));
            }
        }
    }

    let gap = 8;
    let mut sheet = RgbaImage::from_pixel(w * 3 + gap * 2, h, Rgba([0, 0, 0, 255]));
    image::imageops::replace(&mut sheet, &mac, 0, 0);
    image::imageops::replace(&mut sheet, &gui, (w + gap) as i64, 0);
    image::imageops::replace(&mut sheet, &overlay, (2 * (w + gap)) as i64, 0);
    sheet.save(out)?;
    let share = |alone: u64, total: u64| {
        if total == 0 {
            0.0
        } else {
            alone as f64 / total as f64
        }
    };
    Ok((share(mac_alone, mac_total), share(gui_alone, gui_total)))
}

/// Where the luminance steps by more than `EDGE` to the right or below.
fn edges(img: &RgbaImage) -> GrayImage {
    let luma = |x: u32, y: u32| {
        let p = img.get_pixel(x, y).0;
        ((u16::from(p[0]) * 3 + u16::from(p[1]) * 6 + u16::from(p[2])) / 10) as i16
    };
    let (w, h) = img.dimensions();
    GrayImage::from_fn(w, h, |x, y| {
        let here = luma(x, y);
        let right = if x + 1 < w { luma(x + 1, y) } else { here };
        let below = if y + 1 < h { luma(x, y + 1) } else { here };
        let edge = (here - right).abs() > EDGE || (here - below).abs() > EDGE;
        Luma([if edge { 255 } else { 0 }])
    })
}

fn on(edges: &GrayImage, x: u32, y: u32) -> bool {
    edges.get_pixel(x, y).0[0] > 0
}

/// An edge within `SLACK` pixels of (x, y).
fn near(edges: &GrayImage, x: u32, y: u32) -> bool {
    let (w, h) = (i64::from(edges.width()), i64::from(edges.height()));
    (-SLACK..=SLACK).any(|dy| {
        (-SLACK..=SLACK).any(|dx| {
            let (nx, ny) = (i64::from(x) + dx, i64::from(y) + dy);
            nx >= 0 && ny >= 0 && nx < w && ny < h && on(edges, nx as u32, ny as u32)
        })
    })
}
