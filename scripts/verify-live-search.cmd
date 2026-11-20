@echo off
setlocal
cd /d "%~dp0.."
if errorlevel 1 exit /b 1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0verify-live-search.ps1"
set "sdir_result=%ERRORLEVEL%"
echo.
if "%sdir_result%"=="0" (echo Runtime verification passed. See docs\task-id\live-search\actual-live.) else (echo Runtime verification is incomplete. See docs\task-id\live-search\actual-live.)
pause
exit /b %sdir_result%
