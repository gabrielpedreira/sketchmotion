@echo off
chcp 65001 >nul
title SketchMotion
pushd "%~dp0"

rem Garante que o cargo (Rust) esteja no PATH mesmo fora do terminal.
where cargo >nul 2>nul
if errorlevel 1 (
  if exist "%USERPROFILE%\.cargo\bin\cargo.exe" (
    set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
  ) else (
    echo.
    echo Nao encontrei o Rust/cargo. Instale em https://rustup.rs e tente de novo.
    echo.
    pause
    popd
    exit /b 1
  )
)

echo ============================================
echo    SketchMotion
echo ============================================
echo.
echo Preparando a versao mais recente...
echo (a primeira vez demora; depois e rapido)
echo.
cargo build --release 1>build.log 2>&1
if errorlevel 1 (
  echo.
  echo *** ERRO ao compilar. Abra build.log para ver os detalhes. ***
  echo.
  pause
  popd
  exit /b 1
)

if not exist "%~dp0target\release\sketchmotion.exe" (
  echo.
  echo *** Nao encontrei o executavel apos compilar. Veja build.log. ***
  echo.
  pause
  popd
  exit /b 1
)

echo Pronto! Abrindo o SketchMotion...
start "" "%~dp0target\release\sketchmotion.exe"
popd
exit /b 0
