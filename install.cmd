@echo off
rem One-click installer: double-click this file. See README.md.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" %*
