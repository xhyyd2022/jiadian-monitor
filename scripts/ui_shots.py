# -*- coding: utf-8 -*-
"""780×540 六页 + 关键状态自动截图（静态 mock 预览，Edge headless + CDP）。

用法：先起静态服务（见 scripts/ui_preview.sh），再运行：
    python scripts/ui_shots.py [URL] [输出目录]
输出：<输出目录>/{light,dark}-*.png（1170×810 = 780×540 CSS @DSF 1.5）
"""
import asyncio
import base64
import json
import os
import subprocess
import sys
import shutil
import tempfile
import time
import urllib.request
from urllib.parse import urlparse

import websockets

EDGE = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
URL = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8899/?page=home"
OUT = sys.argv[2] if len(sys.argv) > 2 else os.path.join(
    os.path.dirname(__file__), "..", "..", "outputs", "shots", "iter")
NETLOC = urlparse(URL).netloc
PORT = int(os.environ.get("EDGE_CDP_PORT", "9347"))
PROFILE = tempfile.mkdtemp(prefix="edge-ui-shots-")

PAGES = [
    ("home", "首页"),
    ("perf", "性能"),
    ("peripherals", "外设"),
]


def target_ws():
    for _ in range(100):
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
    os.makedirs(OUT, exist_ok=True)
    proc = subprocess.Popen(
        [
            EDGE,
            "--headless=new",
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-extensions",
            f"--remote-debugging-port={PORT}",
            f"--user-data-dir={PROFILE}",
            "--window-size=780,540",
            URL,
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        async with websockets.connect(target_ws(), max_size=64 * 1024 * 1024) as ws:
            n = 0

            async def call(method, params=None):
                nonlocal n
                n += 1
                mid = n
                await ws.send(json.dumps({"id": mid, "method": method, "params": params or {}}))
                while True:
                    msg = json.loads(await asyncio.wait_for(ws.recv(), 30))
                    if msg.get("id") == mid:
                        if "error" in msg:
                            raise RuntimeError(f"{method}: {msg['error']}")
                        return msg.get("result", {})

            async def ev(expr):
                r = await call("Runtime.evaluate", {"expression": expr, "returnByValue": True})
                return r.get("result", {}).get("value")

            async def shot(name):
                r = await call("Page.captureScreenshot", {"format": "png"})
                path = os.path.join(OUT, name + ".png")
                with open(path, "wb") as f:
                    f.write(base64.b64decode(r["data"]))
                info = await ev(
                    "(()=>{const v=document.querySelector('.view');"
                    "const over=[...document.querySelectorAll('body *')].filter(e=>e.offsetParent&&"
                    "(e.scrollWidth>e.clientWidth+2||e.scrollHeight>e.clientHeight+2)).slice(0,6)"
                    ".map(e=>(e.className&&e.className.toString().slice(0,40))||e.tagName);"
                    "return JSON.stringify({vw:innerWidth,vh:innerHeight,viewScroll:v?[v.scrollWidth,v.clientWidth,v.scrollHeight,v.clientHeight]:null,over})})()"
                )
                print("saved", os.path.basename(path), info)

            async def set_theme(light: bool):
                current = await ev("document.documentElement.classList.contains('light')")
                if current != light:
                    await ev("document.querySelector('.side-theme')?.click(); true")
                    await asyncio.sleep(0.4)

            async def nav(label: str):
                await ev(
                    "(()=>{const b=[...document.querySelectorAll('.side-btn')].find(x=>(x.textContent||'').includes(%s));"
                    "b&&b.click();return !!b})()" % json.dumps(label)
                )
                await asyncio.sleep(0.7)
                await ev("(()=>{const v=document.querySelector('.view');if(v)v.scrollTop=0;return true})()")

            async def click_text(label: str) -> bool:
                sel = (
                    "(()=>{const sels=['.el-tabs__item','[role=tab]','.el-radio-button__inner','.el-segmented__item',"
                    "'button','[role=button]'];\n"
                    "for(const q of sels){const els=[...document.querySelectorAll(q)];\n"
                    "const e=els.find(x=>!x.disabled&&x.offsetParent!==null&&x.getClientRects().length>0\n"
                    "  &&((x.textContent||'').replace(/\\s+/g,'').includes(%s)));\n"
                    "if(e){e.click();return true}}return false})()"
                ) % json.dumps(label)
                return bool(await ev(sel))

            async def click_run() -> bool:
                sel = (
                    "(()=>{const els=[...document.querySelectorAll('button,[role=button]')];\n"
                    "const b=els.find(e=>!e.disabled&&/启动|应用并启动/.test((e.getAttribute('aria-label')||'')+(e.textContent||'')));\n"
                    "if(b){b.click();return true}return false})()"
                )
                return bool(await ev(sel))

            await call("Page.enable")
            await call("Emulation.setDeviceMetricsOverride", {
                "width": 780, "height": 540, "deviceScaleFactor": 1.5, "mobile": False,
            })
            await asyncio.sleep(4.5)

            async def press_esc():
                for t in ("keyDown", "keyUp"):
                    await call("Input.dispatchKeyEvent", {"type": t, "key": "Escape", "code": "Escape",
                                                          "windowsVirtualKeyCode": 27, "nativeVirtualKeyCode": 27})
                await asyncio.sleep(0.45)

            async def click_aria(label: str) -> bool:
                sel = (
                    "(()=>{const els=[...document.querySelectorAll('button,[role=button]')];\n"
                    "const b=els.find(e=>!e.disabled&&e.offsetParent!==null&&e.getClientRects().length>0&&((e.getAttribute('aria-label')||'')+(e.getAttribute('title')||'')+(e.textContent||'')).includes(%s)));\n"
                    "if(b){b.click();return true}return false})()"
                ) % json.dumps(label)
                return bool(await ev(sel))

            async def click_any(label: str) -> bool:
                """点任意可见元素（折叠行/卡片头不是 button 时兜底）。"""
                sel = (
                    "(()=>{const els=[...document.querySelectorAll('div,span,li,button,a,summary')];\n"
                    "const e=els.filter(x=>x.offsetParent&&(x.textContent||'').replace(/\\s+/g,'').includes(%s))\n"
                    ".sort((a,b)=>(a.textContent||'').length-(b.textContent||'').length)[0];\n"
                    "if(e){e.click();return true}return false})()"
                ) % json.dumps(label)
                return bool(await ev(sel))

            BASE_URL = URL

            for theme_name, light in (("light", True), ("dark", False)):
                await set_theme(light)
                # 首页：默认即自动启动（新交互）；截图记录运行态
                await nav("首页")
                await asyncio.sleep(0.6)
                await shot(f"{theme_name}-home")
                if await click_run():
                    await asyncio.sleep(2.8)
                    await shot(f"{theme_name}-home-running")
                else:
                    await asyncio.sleep(1.6)
                    await shot(f"{theme_name}-home-running")
                for key, label in PAGES[1:]:
                    await nav(label)
                    await asyncio.sleep(0.4)
                    await shot(f"{theme_name}-{key}")
                # 素材：本地 / 在线 / 生成指引（每次先回“本地素材”，避免标签残留）
                await nav("素材")
                await click_text("本地素材")
                await asyncio.sleep(0.4)
                await shot(f"{theme_name}-materials-local")
                if await click_text("在线素材"):
                    await asyncio.sleep(0.6)
                    await shot(f"{theme_name}-materials-online")
                if await click_text("生成指引"):
                    await asyncio.sleep(0.6)
                    await shot(f"{theme_name}-materials-guide")
                if await click_text("本地素材"):
                    await asyncio.sleep(0.3)
                # 设置：默认 + 外观分类
                await nav("设置")
                await asyncio.sleep(0.4)
                await shot(f"{theme_name}-settings")
                if await click_text("主题"):
                    await asyncio.sleep(0.6)
                    await click_text("配色")
                    await asyncio.sleep(0.3)
                    await shot(f"{theme_name}-settings-appearance")
                    if await click_text("高级"):
                        await asyncio.sleep(0.5)
                        await shot(f"{theme_name}-settings-theme-advanced")
                    if await click_text("背景"):
                        await asyncio.sleep(0.5)
                        await shot(f"{theme_name}-settings-theme-bg")
                    if await click_text("主题文件"):
                        await asyncio.sleep(0.5)
                        await shot(f"{theme_name}-settings-theme-file")
                else:
                    await ev("(()=>{const v=document.querySelector('.view');if(v)v.scrollTop=v.scrollHeight;return true})()")
                    await asyncio.sleep(0.4)
                    await shot(f"{theme_name}-settings-appearance")

                if not light:
                    continue
                # ---- 仅浅色：关键状态与弹层 ----
                # 设备详情弹窗
                await nav("首页")
                await asyncio.sleep(0.4)
                if await click_text("设备详情"):
                    await asyncio.sleep(0.7)
                    await shot("light-home-device")
                    await press_esc()
                # 停止后回到未运行
                if await click_aria("停止") or await click_text("停止"):
                    await asyncio.sleep(1.6)
                    await shot("light-home-stopped")
                # 性能页详情展开
                await nav("性能")
                await asyncio.sleep(0.4)
                if await click_text("详情"):
                    await asyncio.sleep(0.6)
                    await shot("light-perf-details")
                await nav("外设")
                if await click_any("高级"):
                    await asyncio.sleep(0.6)
                    await shot("light-peripherals-advanced")
                if await click_aria("改显示名") or await click_text("重命名"):
                    await asyncio.sleep(0.7)
                    await shot("light-peripherals-rename")
                    await press_esc()
                await nav("素材")
                await click_text("在线素材")
                await asyncio.sleep(0.5)
                if await click_text("新建在线素材"):
                    await asyncio.sleep(0.7)
                    await shot("light-materials-online-new")
                    if await click_text("保存"):
                        await asyncio.sleep(0.7)
                        await shot("light-materials-online-validation")
                    await press_esc()
                # 空态：mock ?online=empty
                await call("Page.navigate", {"url": BASE_URL + "&online=empty"})
                await asyncio.sleep(5.0)
                await nav("素材")
                if await click_text("在线素材"):
                    await asyncio.sleep(0.6)
                    await shot("light-materials-online-empty")
                await call("Page.navigate", {"url": BASE_URL})
                await asyncio.sleep(4.5)

    finally:
        proc.terminate()
        time.sleep(0.4)
        try:
            proc.kill()
        except Exception:
            pass
        shutil.rmtree(PROFILE, ignore_errors=True)


asyncio.run(main())
