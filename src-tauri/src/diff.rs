//! 两帧 RGB565 大端 → 脏矩形（≤ `MAX_RECTS`），移植自 `tools/css_panel_test/fastdiff.py`。
//!
//! 策略：像素掩码 → 16×8 脏块网格 → 贪心网格分解（行游程 + 向下整行扩展）→
//! 贪心合并到 ≤6 框 → 每框收紧到真实变化像素的外接框。

pub const TILE_W: usize = 16;
pub const TILE_H: usize = 8;
pub const MAX_RECTS: usize = 6;

/// 返回脏矩形 `(x, y, w, h)` 列表（像素坐标，最多 `MAX_RECTS` 个）。
pub fn dirty_rects(prev: &[u8], cur: &[u8], w: usize, h: usize) -> Vec<(usize, usize, usize, usize)> {
    if prev.len() != cur.len() || prev.len() != w * h * 2 {
        return vec![(0, 0, w, h)];
    }
    let cols = w.div_ceil(TILE_W);
    let rows = h.div_ceil(TILE_H);

    let mut mask = vec![false; rows * cols];
    for y in 0..h {
        let row = y * w * 2;
        for x in 0..w {
            let o = row + x * 2;
            if prev[o] != cur[o] || prev[o + 1] != cur[o + 1] {
                mask[(y / TILE_H) * cols + (x / TILE_W)] = true;
            }
        }
    }
    if !mask.iter().any(|&b| b) {
        return Vec::new();
    }

    // 贪心网格分解
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

    // 贪心合并到 ≤ MAX_RECTS（代价 = 面积增量最小）
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

    // 收紧到真实变化像素的外接框
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
            if prev[o] != cur[o] || prev[o + 1] != cur[o + 1] {
                any = true;
                minx = minx.min(x);
                maxx = maxx.max(x);
                miny = miny.min(y);
                maxy = maxy.max(y);
            }
        }
    }
    if any {
        Some((minx, miny, maxx - minx + 1, maxy - miny + 1))
    } else {
        None
    }
}
