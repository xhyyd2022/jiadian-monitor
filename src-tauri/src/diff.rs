//! 两帧 RGB565 大端 → 脏矩形（≤ `MAX_RECTS`）。
//!
//! 与旧版「逐字节严格比较」不同：先按 **RGB888 级别**做容差判定，滤掉 RGB565
//! 量化 / 抗锯齿 / 重采样带来的 1~2 级噪声；整块轻微但成片的变化仍会通过
//! 「块内平均差」检出，避免漏掉渐变 / 淡入淡出。
//!
//! 策略：像素差 → 16×8 块判脏（块均值或显著像素）→ 贪心网格分解
//! （行游程 + 向下整行扩展）→ 贪心合并到 ≤6 框 → 每框收紧到真实变化像素的外接框。

pub const TILE_W: usize = 16;
pub const TILE_H: usize = 8;
pub const MAX_RECTS: usize = 6;

/// 单像素「显著变化」阈值（0..255，按 RGB888 展开后比较）。
/// 1 级 RGB565 差在 R/B 上约 8、G 上约 4；12 能滤掉量化噪声，又能抓住小面积文字/图标变化。
const PIXEL_SPIKE: u16 = 12;

/// 块内平均通道差阈值（0..255）。整块淡变、抗锯齿边缘移动靠它检出。
const TILE_MEAN: f32 = 2.0;

/// 收紧外接框时的像素阈值（0..255）：比块均值更细，但能排除纯量化噪声。
const TIGHT_DIFF: u16 = 2;

/// RGB565 大端两字节 → RGB888（按位扩展，和常见 565→888 展开一致）。
#[inline]
fn unpack(hi: u8, lo: u8) -> (u16, u16, u16) {
    let v = u16::from_be_bytes([hi, lo]);
    let r5 = (v >> 11) & 0x1F;
    let g6 = (v >> 5) & 0x3F;
    let b5 = v & 0x1F;
    (
        (r5 << 3) | (r5 >> 2),
        (g6 << 2) | (g6 >> 4),
        (b5 << 3) | (b5 >> 2),
    )
}

#[inline]
fn max3(a: u16, b: u16, c: u16) -> u16 {
    a.max(b).max(c)
}

/// 返回脏矩形 `(x, y, w, h)` 列表（像素坐标，最多 `MAX_RECTS` 个）。
pub fn dirty_rects(prev: &[u8], cur: &[u8], w: usize, h: usize) -> Vec<(usize, usize, usize, usize)> {
    if prev.len() != cur.len() || prev.len() != w * h * 2 {
        return vec![(0, 0, w, h)];
    }
    let cols = w.div_ceil(TILE_W);
    let rows = h.div_ceil(TILE_H);

    // ① 逐块判脏：块内平均差 > TILE_MEAN，或存在单像素显著变化（PIXEL_SPIKE）。
    //    这样既能滤掉稀疏的量化噪声，也不会像「任一像素不同就整块脏」那样
    //    在动画/抗锯齿场景把整屏都判脏。
    let mut mask = vec![false; rows * cols];
    for ty in 0..rows {
        let y0 = ty * TILE_H;
        let y1 = (y0 + TILE_H).min(h);
        for tx in 0..cols {
            let x0 = tx * TILE_W;
            let x1 = (x0 + TILE_W).min(w);
            let mut sum: u32 = 0;
            let mut count: u32 = 0;
            let mut spike = false;
            for y in y0..y1 {
                let row = y * w * 2;
                for x in x0..x1 {
                    let o = row + x * 2;
                    if prev[o] == cur[o] && prev[o + 1] == cur[o + 1] {
                        count += 1;
                        continue;
                    }
                    let (pr, pg, pb) = unpack(prev[o], prev[o + 1]);
                    let (cr, cg, cb) = unpack(cur[o], cur[o + 1]);
                    let dr = pr.abs_diff(cr);
                    let dg = pg.abs_diff(cg);
                    let db = pb.abs_diff(cb);
                    sum += (dr + dg + db) as u32;
                    if max3(dr, dg, db) > PIXEL_SPIKE {
                        spike = true;
                    }
                    count += 1;
                }
            }
            if count == 0 {
                continue;
            }
            let mean = sum as f32 / (count as f32 * 3.0);
            if spike || mean > TILE_MEAN {
                mask[ty * cols + tx] = true;
            }
        }
    }
    if !mask.iter().any(|&b| b) {
        return Vec::new();
    }

    // ② 贪心网格分解
    let mut g = mask.clone();
    let mut tile_rects: Vec<(usize, usize, usize, usize)> = Vec::new();
    for ty in 0..rows {
        let mut tx = 0;
        while tx < cols {
            if !g[ty * cols + tx] {
                tx += 1;
                continue;
            }
            let mut tx1 = tx;
            while tx1 + 1 < cols && g[ty * cols + tx1 + 1] {
                tx1 += 1;
            }
            let mut ty1 = ty;
            while ty1 + 1 < rows && (tx..=tx1).all(|x| g[(ty1 + 1) * cols + x]) {
                ty1 += 1;
            }
            for yy in ty..=ty1 {
                for xx in tx..=tx1 {
                    g[yy * cols + xx] = false;
                }
            }
            tile_rects.push((tx, ty, tx1 - tx + 1, ty1 - ty + 1));
            tx = tx1 + 1;
        }
    }

    let mut rects: Vec<(usize, usize, usize, usize)> = tile_rects
        .iter()
        .map(|(x, y, tw, th)| (x * TILE_W, y * TILE_H, tw * TILE_W, th * TILE_H))
        .collect();

    // ③ 贪心合并到 ≤ MAX_RECTS（代价 = 面积增量最小）
    while rects.len() > MAX_RECTS {
        let mut best: Option<(usize, usize, (usize, usize, usize, usize), usize)> = None;
        for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                let (ax, ay, aw, ah) = rects[i];
                let (bx, by, bw, bh) = rects[j];
                let nx0 = ax.min(bx);
                let ny0 = ay.min(by);
                let nx1 = (ax + aw).max(bx + bw);
                let ny1 = (ay + ah).max(by + bh);
                let grow = (nx1 - nx0) * (ny1 - ny0);
                let grow = grow.saturating_sub(aw * ah + bw * bh);
                if best.as_ref().map(|b| grow < b.3).unwrap_or(true) {
                    best = Some((i, j, (nx0, ny0, nx1 - nx0, ny1 - ny0), grow));
                }
            }
        }
        let Some((i, j, nr, _)) = best else { break };
        rects[i] = nr;
        rects.remove(j);
    }

    // ④ 收紧到真实变化像素的外接框
    let mut out = Vec::new();
    for (x, y, rw, rh) in rects {
        let x1 = (x + rw).min(w);
        let y1 = (y + rh).min(h);
        if x1 <= x || y1 <= y {
            continue;
        }
        if let Some(r) = tight_bbox(prev, cur, w, x, y, x1, y1) {
            out.push(r);
        }
    }
    out
}

fn tight_bbox(
    prev: &[u8],
    cur: &[u8],
    w: usize,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
) -> Option<(usize, usize, usize, usize)> {
    let (mut minx, mut miny, mut maxx, mut maxy) = (x1, y1, x0, y0);
    let mut any = false;
    for y in y0..y1 {
        let row = y * w * 2;
        for x in x0..x1 {
            let o = row + x * 2;
            if prev[o] == cur[o] && prev[o + 1] == cur[o + 1] {
                continue;
            }
            let (pr, pg, pb) = unpack(prev[o], prev[o + 1]);
            let (cr, cg, cb) = unpack(cur[o], cur[o + 1]);
            let md = max3(pr.abs_diff(cr), pg.abs_diff(cg), pb.abs_diff(cb));
            if md <= TIGHT_DIFF {
                continue;
            }
            any = true;
            minx = minx.min(x);
            maxx = maxx.max(x);
            miny = miny.min(y);
            maxy = maxy.max(y);
        }
    }
    if any {
        Some((minx, miny, maxx - minx + 1, maxy - miny + 1))
    } else {
        // 块被判脏但找不到“单像素显著变化”时（整块低幅淡变），不能把块丢掉。
        Some((x0, y0, x1 - x0, y1 - y0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 64;
    const H: usize = 32;

    fn blank() -> Vec<u8> {
        vec![0u8; W * H * 2]
    }

    fn put(buf: &mut [u8], x: usize, y: usize, r5: u16, g6: u16, b5: u16) {
        let v = ((r5 & 0x1F) << 11) | ((g6 & 0x3F) << 5) | (b5 & 0x1F);
        let o = (y * W + x) * 2;
        buf[o] = (v >> 8) as u8;
        buf[o + 1] = v as u8;
    }

    #[test]
    fn identical_frames_produce_no_rects() {
        assert!(dirty_rects(&blank(), &blank(), W, H).is_empty());
    }

    #[test]
    fn sparse_one_level_noise_is_ignored() {
        let a = blank();
        let mut b = a.clone();
        put(&mut b, 3, 3, 1, 0, 0); // R +1 级（约 8/255）
        assert!(dirty_rects(&a, &b, W, H).is_empty());
    }

    #[test]
    fn small_bright_spot_is_detected() {
        let a = blank();
        let mut b = a.clone();
        for dy in 0..2 {
            for dx in 0..2 {
                put(&mut b, 20 + dx, 10 + dy, 31, 63, 31);
            }
        }
        let r = dirty_rects(&a, &b, W, H);
        assert!(!r.is_empty());
        assert!(r
            .iter()
            .any(|&(x, y, w, h)| x <= 20 && y <= 10 && x + w >= 22 && y + h >= 12));
    }

    #[test]
    fn broad_subtle_change_is_detected() {
        let a = blank();
        let mut b = a.clone();
        for y in 0..H {
            for x in 0..W {
                put(&mut b, x, y, 2, 0, 0); // R +2 级（约 16/255）
            }
        }
        assert!(!dirty_rects(&a, &b, W, H).is_empty());
    }

    #[test]
    fn single_pixel_rect_is_tight() {
        let a = blank();
        let mut b = a.clone();
        put(&mut b, 40, 20, 31, 63, 31);
        let r = dirty_rects(&a, &b, W, H);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0], (40, 20, 1, 1));
    }
}
