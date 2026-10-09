@echo off
chcp 65001 >nul
title SketchMotion
cd /d "%~dp0"
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
  exit /b 1
)
echo Pronto! Abrindo o SketchMotion...
start "" "%~dp0target\release\sketchmotion.exe"
exit /b 0
