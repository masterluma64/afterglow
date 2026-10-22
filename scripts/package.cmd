@echo off
rem Runs package.ps1 without changing PowerShell's script policy. Double-click
rem to build (the zip lands in dist\), or pass -SkipTests.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0package.ps1" %*
set result=%errorlevel%
rem Keep the window open on failure, so the error can be read.
if not %result%==0 pause
exit /b %result%
