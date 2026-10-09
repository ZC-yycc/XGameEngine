@echo off
setlocal

rem ===================================================================
rem  XGameEngine - one-click release build.
rem
rem  Double-click this file to build the editor package into .\dist
rem  (dist\xgame-editor.exe and everything it needs).
rem
rem  It forwards to tools\build-release.ps1 with an execution-policy
rem  bypass, so it works on machines with default PowerShell settings
rem  and without cargo on PATH.
rem
rem  Optional: set XGAME_NO_PAUSE=1 to skip the final "Press any key"
rem  prompt, e.g. when running from CI.
rem
rem  KEEP THIS FILE ASCII-ONLY. cmd.exe reads .cmd files using the
rem  system ANSI code page (GBK on this machine). Non-ASCII bytes can
rem  be mis-decoded, and the tail of a comment line may then be run as
rem  a command.
rem ===================================================================

set "PS1=%~dp0tools\build-release.ps1"

if not exist "%PS1%" goto :no_script

powershell -NoProfile -ExecutionPolicy Bypass -File "%PS1%" %*
set "EXITCODE=%ERRORLEVEL%"
goto :finish

:no_script
set "EXITCODE=1"
echo.
echo [ERROR] Script not found:
echo         %PS1%
echo.
echo This .cmd file must stay in the repository root, next to the
echo "tools" folder.

:finish
echo.
echo ===================================================================
if "%EXITCODE%"=="0" (
    echo  Build succeeded.
) else (
    echo  Build FAILED with exit code %EXITCODE%.
)
echo.
echo  Output folder : %~dp0dist
echo  Editor        : dist\xgame-editor.exe  ^(double-click to run^)
echo  Build log     : build-release.log
echo ===================================================================
echo.

if not defined XGAME_NO_PAUSE pause
endlocal
