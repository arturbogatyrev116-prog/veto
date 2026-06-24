@echo off
title Veto — Build Client
cd /d "%~dp0\messenger-app"

echo.
echo  ========================================================
echo    Veto Messenger Client — Build
echo  ========================================================
echo.

:: Check Rust
cargo --version >nul 2>&1
if errorlevel 1 (
    echo  [ERROR] Rust is not installed.
    echo  Install from: https://rustup.rs/
    pause
    exit /b 1
)

:: Check Node.js
node --version >nul 2>&1
if errorlevel 1 (
    echo  [ERROR] Node.js is not installed.
    echo  Install from: https://nodejs.org/
    pause
    exit /b 1
)

:: Install JS deps (includes @tauri-apps/cli from devDependencies)
if not exist "node_modules" (
    echo  Installing JS dependencies...
    call npm install
)

echo  Building client (first time: 5-10 minutes)...
echo.
call npm run tauri -- build

if errorlevel 1 (
    echo.
    echo  [ERROR] Build failed. See output above.
    pause
    exit /b 1
)

echo.
echo  ========================================================
echo    Build complete!
echo.
echo    Installer: src-tauri\target\release\bundle\msi\
echo               src-tauri\target\release\bundle\nsis\
echo.
echo    Run the .msi or .exe to install Veto on any machine.
echo  ========================================================
echo.
pause
