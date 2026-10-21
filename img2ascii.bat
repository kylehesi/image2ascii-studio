@echo off
rem Test launcher: drag an image onto this file, or run it and type the path.
rem Builds the release CLI, plays the reveal animation and saves <image>.txt, .svg and .png next to the image.
setlocal EnableExtensions
cd /d "%~dp0"

where cargo >nul 2>nul
if errorlevel 1 (
    echo [ERROR] Rust/cargo not found. Install it from https://rustup.rs
    pause
    exit /b 1
)

echo Building... the first build can take a few minutes.
cargo build --release -q -p img2ascii
if errorlevel 1 (
    echo [ERROR] Build failed.
    pause
    exit /b 1
)

set "IMG=%~1"
if not defined IMG set /p "IMG=Image file (you can drag it here): "
if not defined IMG (
    echo [ERROR] No image given.
    pause
    exit /b 1
)
set "IMG=%IMG:"=%"
if not exist "%IMG%" (
    echo [ERROR] File not found: "%IMG%"
    pause
    exit /b 1
)

rem Default width: the console width, so lines don't wrap.
set "TERMW="
for /f %%W in ('powershell -NoProfile -Command "$Host.UI.RawUI.WindowSize.Width" 2^>nul') do set "TERMW=%%W"
if not defined TERMW set "TERMW=121"
set /a DEFCOLS=TERMW-1
if %DEFCOLS% GTR 200 set "DEFCOLS=200"
if %DEFCOLS% LSS 20 set "DEFCOLS=120"

set "COLS="
set /p "COLS=Width in characters [%DEFCOLS%]: "
if not defined COLS set "COLS=%DEFCOLS%"

set "MODE="
set /p "MODE=Mode  1=shape (detailed), 2=density (smooth) [1]: "
set "MODEARG=shape"
if "%MODE%"=="2" set "MODEARG=density"

set "BG="
set /p "BG=Polarity  1=dark background, 2=negative/light background [1]: "
set "BGARG=dark"
if "%BG%"=="2" set "BGARG=light"

for %%F in ("%IMG%") do set "OUT=%%~dpnF"

echo.
"target\release\img2ascii.exe" "%IMG%" -c %COLS% -m %MODEARG% --background %BGARG% -o "%OUT%.txt" -o "%OUT%.svg" -o "%OUT%.png" --reveal --score
if errorlevel 1 (
    echo [ERROR] Conversion failed.
    pause
    exit /b 1
)
echo.
echo Saved: "%OUT%.txt", "%OUT%.svg" (animated), "%OUT%.png"
pause
