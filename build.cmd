@echo off
rem Double-click to build Lumen and start it. Output is saved to build.log.
cd /d "%~dp0"
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build.ps1"
if errorlevel 1 (echo. & echo BUILD FAILED - see build.log & pause)
