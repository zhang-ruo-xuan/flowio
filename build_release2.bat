@echo off
REM 不依赖 vcvars/reg.exe（沙箱会拦截 reg.exe，导致 LINK 找不到 user32.lib）
set VS=C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207
set SDK=C:\Program Files (x86)\Windows Kits\10
set SDKVER=10.0.26100.0

set PATH=%VS%\bin\Hostx64\x64;%SDK%\bin\%SDKVER%\x64;C:\Users\taotao\.cargo\bin;%PATH%
set LIB=%VS%\lib\x64;%SDK%\lib\%SDKVER%\um\x64;%SDK%\lib\%SDKVER%\ucrt\x64
set INCLUDE=%VS%\include;%SDK%\Include\%SDKVER%\um;%SDK%\Include\%SDKVER%\shared;%SDK%\Include\%SDKVER%\ucrt

cd /d D:\projects\flowio\src-tauri
set CODEBUDDY_SESSION_ID=
set CLAUDE_SESSION_ID=
set NODE_OPTIONS=
cargo build --release
if errorlevel 1 (
  echo BUILD_FAILED
  exit /b 1
) else (
  echo BUILD_OK
)
