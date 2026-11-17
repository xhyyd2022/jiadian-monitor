@echo off
setlocal
chcp 65001 >nul
cd /d "%~dp0"
echo.
echo 佳点监控 · 清除旧版 JSON 协议库
echo 新版只读取 peripherals\index.toml 与 brands\*.toml。
echo.
set "found=0"
if exist "peripherals\index.json" (
  del /f /q "peripherals\index.json" >nul 2>nul
  echo 已删除 peripherals\index.json
  set "found=1"
)
if exist "peripherals\brands" (
  for %%F in ("peripherals\brands\*.json") do (
    del /f /q "%%~fF" >nul 2>nul
    echo 已删除 %%~nxF
    set "found=1"
  )
)
if "%found%"=="0" (
  echo 没有发现旧版 JSON 协议文件。
) else (
  echo.
  echo 完成：旧版 JSON 已清除，新版 TOML 已生效。
)
echo.
pause
