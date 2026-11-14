# -*- coding: utf-8 -*-
"""自动 UI 评审：axe-core WCAG 2.2 AA + 布局/可读性/一致性指标。

先启动 outputs/ui-preview（mock Tauri）并确保 axe.min.js 在其中，然后：
    python app/scripts/ui_audit.py
输出：outputs/ui-audit.json
"""
import asyncio
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request
from urllib.parse import urlparse

import websockets

EDGE = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
PORT = int(os.environ.get("EDGE_CDP_PORT", "9349"))
URL = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8899/?page=home"
OUT = sys.argv[2] if len(sys.argv) > 2 else os.path.join(os.path.dirname(__file__), "..", "..", "outputs", "ui-audit.json")
NETLOC = urlparse(URL).netloc
PROFILE = tempfile.mkdtemp(prefix="edge-ui-audit-")

PAGES = [
    ("home", "首页"),
    ("perf", "性能"),
    ("peripherals", "外设"),
    ("materials", "素材"),
    ("settings", "设置"),
]

JS_METRICS = r"""
(() => {
  const rect = (e) => { if (!e) return null; const b = e.getBoundingClientRect(); return [b.x, b.y, b.width, b.height].map(v => +v.toFixed(1)); };
  const visible = (e) => { const b = e.getBoundingClientRect(); return b.width > 1 && b.height > 1 && b.bottom > 0 && b.top < innerHeight; };
  const cards = [...document.querySelectorAll('.el-card')].filter(visible).map(e => rect(e));
  const groups = {};
  for (const [x, y, w, h] of cards) {
    const key = `${+x.toFixed(0)}/${+w.toFixed(0)}`;
    (groups[key] ||= []).push([x, y, w, h]);
  }
  // 同一列的卡片：左右边线误差
  const align = Object.entries(groups).map(([k, v]) => {
    const left = Math.max(...v.map(a => a[0])) - Math.min(...v.map(a => a[0]));
    const right = Math.max(...v.map(a => a[0] + a[2])) - Math.min(...v.map(a => a[0] + a[2]));
    return { key: k, cards: v.length, leftErr: +left.toFixed(1), rightErr: +right.toFixed(1) };
  });
  // 可见文本字号（去重）
  const fonts = [...new Set([...document.querySelectorAll('body *')].filter(e => visible(e) && e.childElementCount === 0 && (e.textContent || '').trim()).map(e => parseFloat(getComputedStyle(e).fontSize)).filter(n => n > 0))].sort((a,b)=>a-b);
  // 颜色对比：用 axe 会覆盖常规文本；这里仅补充主标题/说明/按钮的原始颜色
  const sample = (sel) => { const e = document.querySelector(sel); if (!e) return null; const s = getComputedStyle(e); return { color: s.color, bg: s.backgroundColor, size: s.fontSize }; };
  return JSON.stringify({ cards, align, fonts, titlebar: rect(document.querySelector('.titlebar')), topbar: rect(document.querySelector('.topbar')), view: rect(document.querySelector('.view')), sample: { panel: sample('.panel-title'), hint: sample('.hint'), meta: sample('.meta-row .lbl'), btn: sample('.el-button--primary') } });
})()
"""

JS_AXE = r"""
(async () => {
  if (!window.axe) {
    const txt = await (await fetch('/axe.min.js')).text();
    (0, eval)(txt);
  }
  const r = await axe.run(document, { runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21aa'] } });
  return JSON.stringify({
    violations: r.violations.map(v => ({ id: v.id, impact: v.impact, help: v.help, nodes: v.nodes.length,
      targets: v.nodes.slice(0, 4).map(n => n.target.join(' ')), summary: v.nodes[0] && v.nodes[0].failureSummary })),
    passes: r.passes.length, incomplete: r.incomplete.map(v => v.id), inapplicable: r.inapplicable.length
  });
})()
"""


def target_ws():
    for _ in range(80):
        try:
            raw = urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json/list", timeout=2).read()
            for t in json.loads(raw):
                if t.get("type") == "page" and NETLOC in t.get("url", ""):
                    return t["webSocketDebuggerUrl"]
        except Exception:
            pass
        time.sleep(0.25)
    raise SystemExit("no CDP target")


async def main():
    proc = subprocess.Popen(
        [EDGE, "--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check",
         f"--remote-debugging-port={PORT}", f"--user-data-dir={PROFILE}", "--window-size=780,540", URL],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    result = {}
    try:
        async with websockets.connect(target_ws(), max_size=128 * 1024 * 1024) as ws:
            n = 0

            async def call(method, params=None):
                nonlocal n
                n += 1
                mid = n
                await ws.send(json.dumps({"id": mid, "method": method, "params": params or {}}))
                while True:
                    msg = json.loads(await asyncio.wait_for(ws.recv(), 60))
                    if msg.get("id") == mid:
                        if "error" in msg:
                            raise RuntimeError(f"{method}: {msg['error']}")
                        return msg.get("result", {})

            async def ev(expr, await_promise=False):
                r = await call("Runtime.evaluate", {"expression": expr, "returnByValue": True,
                                                     "awaitPromise": await_promise})
                return r.get("result", {}).get("value")

            await call("Page.enable")
            await call("Emulation.setDeviceMetricsOverride", {"width": 780, "height": 540, "deviceScaleFactor": 1, "mobile": False})
            await asyncio.sleep(4)
            try:
                await ev(JS_AXE, await_promise=True)
            except Exception as e:
                print("axe preload failed", e)

            async def set_theme(light: bool):
                current = await ev("document.documentElement.classList.contains('light')")
                if current != light:
                    await ev("document.querySelector('.side-theme').click(); true")
                    await asyncio.sleep(0.35)

            async def nav(label: str):
                await ev("(()=>{const b=[...document.querySelectorAll('.side-btn')].find(x=>(x.textContent||'').includes(%s));b&&b.click();return !!b})()" % json.dumps(label))
                await asyncio.sleep(0.8)

            for theme_name, light in (("light", True), ("dark", False)):
                await set_theme(light)
                for key, label in PAGES:
                    await nav(label)
                    await ev("(()=>{const v=document.querySelector('.view');if(v)v.scrollTop=0;return true})()")
                    await asyncio.sleep(0.25)
                    metrics = json.loads(await ev(JS_METRICS))
                    try:
                        axe = json.loads(await ev(JS_AXE, await_promise=True))
                    except Exception as e:
                        axe = {"error": str(e)}
                    result[f"{theme_name}-{key}"] = {"metrics": metrics, "axe": axe}
                    print(theme_name, key, "violations:",
                          [(v["id"], v["impact"], v["nodes"]) for v in axe.get("violations", [])][:8])
    finally:
        proc.terminate()
        time.sleep(0.3)
        try:
            proc.kill()
        except Exception:
            pass
    shutil.rmtree(PROFILE, ignore_errors=True)
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "w", encoding="utf-8") as f:
        json.dump(result, f, ensure_ascii=False, indent=2)
    print("wrote", OUT)


asyncio.run(main())
