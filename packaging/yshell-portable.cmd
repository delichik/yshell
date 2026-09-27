@echo off
rem YShell portable launcher.
rem Keeps configuration, known hosts, logs and the local secret store in
rem .\data next to this script instead of %APPDATA%.
setlocal
set "YSHELL_CONFIG_DIR=%~dp0data"
"%~dp0yshell.exe" %*
endlocal
