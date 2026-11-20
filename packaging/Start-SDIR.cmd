@echo off
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0check-runtime.ps1" -Executable "%~dp0SDIR.exe"
if errorlevel 1 pause
