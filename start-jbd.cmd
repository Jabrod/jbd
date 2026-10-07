@echo off
rem Builds and starts JBD. Double-click this file, or run it from any terminal.
rem The first build compiles every crate and takes about 20 minutes; after that, a change
rem rebuilds in seconds to a few minutes. Close the JBD window (or press Ctrl+C here) to stop.
rem
rem   start-jbd.cmd           normal start (opens the Home screen)
rem   start-jbd.cmd demo      opens the demo project
rem   start-jbd.cmd control   also starts the control server on port 9876, so an agent can
rem                           drive the app and take screenshots (docs/control-protocol.md)
title JBD
cd /d "%~dp0"

rem Terminals opened before Rust was installed have an old PATH. Reload it from Windows.
for /f "usebackq delims=" %%P in (`powershell -NoProfile -Command "[Environment]::GetEnvironmentVariable('Path','Machine') + ';' + [Environment]::GetEnvironmentVariable('Path','User')"`) do set "PATH=%%P"

where cargo >nul 2>nul || (echo Rust isn't installed. See README.md, "Build and run". & pause & exit /b 1)

rem At most 6 compilers at once: with all 16 running, this 16 GB laptop runs out of memory
rem ("The paging file is too small") while linking.
set CARGO_BUILD_JOBS=6

set "ARGS="
if /i "%~1"=="demo" set "ARGS=--demo"
if /i "%~1"=="control" set "ARGS=--control 9876"

echo Building JBD (cargo run -p filmcraft)...
cargo run -p filmcraft -- %ARGS%
if errorlevel 1 pause
