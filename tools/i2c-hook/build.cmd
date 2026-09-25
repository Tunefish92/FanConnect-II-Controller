@echo off
rem Builds the 32-bit i2c_hook.dll and inject.exe with MSVC.
setlocal
set VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe
for /f "usebackq delims=" %%i in (`"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set VSDIR=%%i
if not defined VSDIR (
    echo MSVC C++ build tools not found. Install the "Desktop development with C++" workload.
    exit /b 1
)
call "%VSDIR%\VC\Auxiliary\Build\vcvarsall.bat" x86 >nul 2>nul || exit /b 1
cd /d "%~dp0"
if not exist out mkdir out
set MH=third_party\minhook
cl /nologo /W3 /O2 /MT /LD /I%MH%\include i2c_hook.c %MH%\src\buffer.c %MH%\src\hook.c %MH%\src\trampoline.c %MH%\src\hde\hde32.c /Foout\ /Feout\i2c_hook.dll /link /NOLOGO || exit /b 1
cl /nologo /W4 /O2 /MT inject.c /Foout\ /Feout\inject.exe /link /NOLOGO || exit /b 1
echo Built %~dp0out\i2c_hook.dll and %~dp0out\inject.exe
