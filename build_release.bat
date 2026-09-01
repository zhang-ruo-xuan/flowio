@echo off
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
set PATH=%PATH%;C:\Users\taotao\.cargo\bin
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
