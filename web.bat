@echo off
rem Starts the web app: builds the WebAssembly engine, installs web dependencies
rem on first run, then serves the app and opens it in the default browser.
setlocal EnableExtensions
cd /d "%~dp0web"

where cargo >nul 2>nul
if errorlevel 1 (
    echo [ERROR] Rust/cargo not found. Install it from https://rustup.rs
    pause
    exit /b 1
)
where npm >nul 2>nul
if errorlevel 1 (
    echo [ERROR] Node.js/npm not found. Install it from https://nodejs.org
    pause
    exit /b 1
)
where wasm-bindgen >nul 2>nul
if errorlevel 1 (
    echo Installing the WebAssembly tooling (one-time)...
    rustup target add wasm32-unknown-unknown
    cargo install wasm-bindgen-cli --version 0.2.129 --locked
    if errorlevel 1 (
        echo [ERROR] Could not install wasm-bindgen.
        pause
        exit /b 1
    )
)

if not exist node_modules (
    echo Installing dependencies...
    call npm install
    if errorlevel 1 (
        echo [ERROR] npm install failed.
        pause
        exit /b 1
    )
)

echo Building the engine...
call npm run wasm
if errorlevel 1 (
    echo [ERROR] WebAssembly build failed.
    pause
    exit /b 1
)

echo.
echo The app opens in your browser; its address is on the "Local:" line below. Press Ctrl+C to stop.
call npm exec -- vite --port 5173 --open
