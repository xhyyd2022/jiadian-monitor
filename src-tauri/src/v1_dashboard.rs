//! V1 Dashboard image generator — renders a 320x170 PNG for V1 GUI carousel.
//! Uses the same metrics as V1P (LibreHardwareMonitor shared memory + mouse HTTP).

use std::io::BufWriter;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use fontdue::{Font, FontSettings};

const OUTPUT_PATH: &str = r"E:\OtherFiles\Data\V1Pro\test_v1.png";
const W: usize = 320;
const H: usize = 170;

static RUNNING: AtomicBool = AtomicBool::new(false);

pub fn start() {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        let font_data = std::fs::read(r"C:\Windows\Fonts\arialbd.ttf").unwrap_or_default();
        let font = Font::from_bytes(font_data, FontSettings::default()).ok();
        loop {
            if !RUNNING.load(Ordering::SeqCst) { break; }
            if let Some(ref f) = font {
                if let Err(e) = generate(f) {
                    crate::logging::warn(&format!("V1 dashboard: {e}"));
                }
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
    crate::logging::info("V1 dashboard generator started");
}

pub fn stop() {
    RUNNING.store(false, Ordering::SeqCst);
}

fn get_metric(key: &str) -> Option<f64> {
    crate::metrics::basic().iter().find(|m| m.key == key).and_then(|m| m.value)
}

fn generate(font: &Font) -> Result<(), String> {
    let gpu_t = get_metric("gpu1_temp").map(|v| v.round() as i32).unwrap_or(-1);
    let cpu_t = get_metric("cpu_temp").map(|v| v.round() as i32).unwrap_or(-1);
    let mouse_pct = get_metric("mouse1_pct").map(|v| v as u32);

    let gpu_str = if gpu_t >= 0 { format!("{gpu_t}") } else { "--".into() };
    let cpu_str = if cpu_t >= 0 { format!("{cpu_t}") } else { "--".into() };
    let mouse_str = mouse_pct.map(|p| p.to_string()).unwrap_or("--".into());

    let mut buf = vec![15u8; W * H * 3];

    draw_text(font, &mut buf, "GPU T", 20, 25, 100, 200, 255, 13.0);
    draw_text(font, &mut buf, &format!("{gpu_str}C"), 20, 45, 255, 255, 255, 26.0);
    draw_text(font, &mut buf, "CPU T", 120, 25, 100, 200, 255, 13.0);
    draw_text(font, &mut buf, &format!("{cpu_str}C"), 120, 45, 255, 255, 255, 26.0);
    let (mr, mg, mb) = match mouse_pct {
        Some(p) if p <= 15 => (255, 80, 80),
        Some(p) if p <= 30 => (255, 200, 50),
        _ => (100, 255, 100),
    };
    draw_text(font, &mut buf, "MOUSE", 220, 25, 100, 200, 255, 13.0);
    draw_text(font, &mut buf, &format!("{mouse_str}%"), 220, 45, mr, mg, mb, 26.0);

    // Rotate 180
    let mut rotated = vec![0u8; W * H * 3];
    for y in 0..H {
        for x in 0..W {
            let src = (y * W + x) * 3;
            let dst = ((H - 1 - y) * W + (W - 1 - x)) * 3;
            rotated[dst] = buf[src];
            rotated[dst + 1] = buf[src + 1];
            rotated[dst + 2] = buf[src + 2];
        }
    }

    let file = std::fs::File::create(OUTPUT_PATH).map_err(|e| format!("create: {e}"))?;
    let writer = BufWriter::new(file);
    let mut encoder = png::Encoder::new(writer, W as u32, H as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| format!("png: {e}"))?;
    writer.write_image_data(&rotated).map_err(|e| format!("png data: {e}"))?;
    Ok(())
}

fn draw_text(font: &Font, buf: &mut [u8], text: &str, x: i32, y: i32, r: u8, g: u8, b: u8, size: f32) {
    let mut pen_x = x;
    for ch in text.chars() {
        let (metrics, coverage) = font.rasterize(ch, size);
        for row in 0..metrics.height {
            for col in 0..metrics.width {
                let px = pen_x + metrics.xmin as i32 + col as i32;
                let py = y + metrics.ymin as i32 + row as i32;
                if px < 0 || px >= W as i32 || py < 0 || py >= H as i32 { continue; }
                if coverage[row * metrics.width + col] > 30 {
                    let idx = (py as usize * W + px as usize) * 3;
                    buf[idx] = r; buf[idx + 1] = g; buf[idx + 2] = b;
                }
            }
        }
        pen_x += metrics.advance_width as i32;
    }
}
