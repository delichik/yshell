@echo off
rem YShell portable launcher.
rem Keeps configuration, known hosts, logs and the local secret store in
rem .\data next to this script instead of %APPDATA%.
rem Configuration is passed as a CLI argument (`--config-dir`), not an
rem environment variable: application configuration never comes from the env.
setlocal
"%~dp0yshell.exe" --config-dir "%~dp0data" %*
endlocal
